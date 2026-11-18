// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The repository commands (step E2): update, upgrade, install NAME,
//! search, policy, depends, rdepends, download, hold, unhold,
//! autoremove, clean, source, key, index.

use crate::control::Control;
use crate::fetch::Fetcher;
use crate::keys::{self, Kind};
use crate::ops::{self, Ctx};
use crate::pkg::{sha256_hex, Package};
use crate::repo::{self, Avail};
use crate::solve::{self, Plan, Step};
use crate::version;
use std::io::Write;

pub fn fetcher(ctx: &Ctx) -> Fetcher {
    Fetcher::new(ctx.db.path("/etc/rpg/ca-certificates.pem"))
}

fn need_lists(ctx: &Ctx) -> Result<Vec<Avail>, String> {
    if !repo::have_lists(&ctx.db) {
        return Err("no package lists yet: rpg update first".into());
    }
    repo::available(&ctx.db)
}

pub fn update(ctx: &Ctx) -> Result<bool, String> {
    let mut f = fetcher(ctx);
    let failed = repo::update(&ctx.db, &mut f)?;
    let av = repo::available(&ctx.db)?;
    let installed = ctx.db.installed()?;
    let up = upgradable(&installed, &av);
    let names: std::collections::BTreeSet<&str> = av.iter().map(|a| a.name()).collect();
    println!(
        "{} packages available; {}",
        names.len(),
        match up.len() {
            0 => "everything installed is up to date".to_string(),
            1 => "1 package can be upgraded (rpg list --upgradable; rpg upgrade)".to_string(),
            n => format!("{n} packages can be upgraded (rpg list --upgradable; rpg upgrade)"),
        }
    );
    Ok(failed == 0)
}

fn upgradable<'a>(installed: &[Control], av: &'a [Avail]) -> Vec<(Control, &'a Avail)> {
    let mut out = Vec::new();
    for c in installed {
        if let Some(a) = repo::candidate(av, c.name()) {
            if version::compare(a.version(), c.version()) == std::cmp::Ordering::Greater {
                out.push((c.clone(), a));
            }
        }
    }
    out
}

fn kib(n: u64) -> String {
    if n < 1024 * 1024 {
        format!("{} KiB", n.div_ceil(1024))
    } else {
        format!("{:.1} MiB", n as f64 / 1048576.0)
    }
}

/// Show the plan; Ok(false) if the administrator says no
fn confirm(plan: &Plan, yes: bool) -> Result<bool, String> {
    let new: Vec<&Step> = plan.steps.iter().filter(|s| s.old.is_none()).collect();
    let up: Vec<&Step> = plan.steps.iter().filter(|s| s.old.is_some()).collect();
    if !new.is_empty() {
        println!("To install:");
        for s in &new {
            let why = s.why.as_ref().map(|w| format!(", for {w}")).unwrap_or_default();
            println!("  {} {}{why}", s.c.name(), s.c.version());
        }
    }
    if !up.is_empty() {
        println!("To upgrade:");
        for s in &up {
            println!("  {} {} -> {}", s.c.name(), s.old.as_deref().unwrap_or("?"), s.c.version());
        }
    }
    for (n, why) in &plan.kept {
        println!("Kept back: {n}: {why}");
    }
    if plan.steps.is_empty() {
        return Ok(false);
    }
    let dl: u64 = plan.steps.iter().filter_map(|s| s.avail.as_ref()).map(|a| a.size).sum();
    let size: u64 = plan.steps.iter().filter_map(|s| s.c.get("Installed-Size")).filter_map(|v| v.parse::<u64>().ok()).sum();
    print!("{} to download, {} KiB installed. ", kib(dl), size);
    if yes {
        println!();
        return Ok(true);
    }
    print!("Continue? [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        println!();
        println!("Abort (no answer; rpg ... -y does not ask).");
        return Ok(false);
    }
    let a = line.trim().to_lowercase();
    if a.is_empty() || a == "y" || a == "yes" {
        Ok(true)
    } else {
        println!("Abort.");
        Ok(false)
    }
}

/// Download what the plan needs (each one's size and SHA-256 checked
/// against the signed index), then install it all
fn carry_out(ctx: &Ctx, plan: Plan, files: Vec<Package>) -> Result<(), String> {
    let cache = ctx.db.path("/var/cache/rpg/archives");
    std::fs::create_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let mut f = fetcher(ctx);
    let mut files: Vec<Option<Package>> = files.into_iter().map(Some).collect();
    let mut items = Vec::new();
    for s in plan.steps {
        let p = match (&s.avail, s.file) {
            (_, Some(i)) => files[i].take().ok_or("a package file twice")?,
            (Some(a), None) => {
                let base = a.filename.rsplit('/').next().unwrap_or(&a.filename).to_string();
                let path = cache.join(&base);
                let have = std::fs::read(&path).ok().filter(|d| d.len() as u64 == a.size && sha256_hex(d) == a.sha);
                let bytes = match have {
                    Some(d) => d,
                    None => {
                        println!("Get {} {} ({})", a.name(), a.version(), kib(a.size));
                        let d = f.get(&a.url()).map_err(|e| format!("{}: {}", a.url(), e.text()))?;
                        if d.len() as u64 != a.size || sha256_hex(&d) != a.sha {
                            return Err(format!(
                                "{} {}: the download's SHA-256 differs from the signed index: refused, nothing was installed",
                                a.name(),
                                a.version()
                            ));
                        }
                        crate::db::write_atomic(&path, &d, 0o644)?;
                        d
                    }
                };
                let p = Package::read(&bytes).map_err(|e| format!("{}: {e}", a.url()))?;
                if p.control.name() != a.name() || p.control.version() != a.version() {
                    return Err(format!("{}: the package inside is {} {}, not what the index says", a.url(), p.control.name(), p.control.version()));
                }
                ops::check_installable(&p).map_err(|e| format!("{}: {e}", a.url()))?;
                p
            }
            (None, None) => return Err("a step with nothing to install".into()),
        };
        items.push((p, s.auto));
    }
    ctx.install_packages(items)?;
    // Two system images are kept (step E4): the newest and the running one
    let extra = crate::boot::extra_images(&ctx.db, &ctx.db.installed()?);
    if !extra.is_empty() {
        println!("rpg: two system images are kept (the newest and the running one): removing {}", extra.join(", "));
        ctx.remove(&extra, false)?;
    }
    Ok(())
}

/// rpg install NAME... and ./FILE.rpg... together: names from the
/// sources, files as they are; their dependencies from the sources
pub fn install(ctx: &Ctx, args: &[String], yes: bool) -> Result<(), String> {
    let installed = ctx.db.installed()?;
    let mut files = Vec::new();
    let mut targets = Vec::new();
    let names: Vec<&String> = args.iter().filter(|a| !(a.ends_with(".rpg") || a.contains('/'))).collect();
    let av = if names.is_empty() { repo::available(&ctx.db).unwrap_or_default() } else { need_lists(ctx)? };
    for a in args {
        if a.ends_with(".rpg") || a.contains('/') {
            let p = ops::read_file(a)?;
            let old = installed.iter().find(|c| c.name() == p.control.name()).map(|c| c.version().to_string());
            targets.push(Step { c: p.control.clone(), avail: None, file: Some(files.len()), old, auto: Some(false), why: None });
            files.push(p);
            continue;
        }
        let Some(cand) = repo::candidate(&av, a) else {
            let like: Vec<&str> = av.iter().map(|x| x.name()).filter(|n| n.contains(a.as_str())).collect::<std::collections::BTreeSet<_>>().into_iter().take(5).collect();
            return Err(format!(
                "{a}: no source has it{}",
                if like.is_empty() { " (rpg search, rpg update)".to_string() } else { format!(" (did you mean {}?)", like.join(", ")) }
            ));
        };
        if let Some(c) = installed.iter().find(|c| c.name() == a) {
            if version::compare(cand.version(), c.version()) != std::cmp::Ordering::Greater {
                if c.get("Auto") == Some("yes") {
                    ctx.set_field(a, "Auto", None)?;
                    println!("{a} {}: the newest there is; now marked as installed by you", c.version());
                } else {
                    println!("{a} {}: the newest there is, installed", c.version());
                }
                continue;
            }
        }
        let old = installed.iter().find(|c| c.name() == a).map(|c| c.version().to_string());
        targets.push(Step { c: cand.c.clone(), avail: Some(cand.clone()), file: None, old, auto: Some(false), why: None });
    }
    if targets.is_empty() {
        return Ok(());
    }
    // Replaces (with Conflicts, as Debian's): an installed package that a
    // new one takes the place of is removed first (the editions: rpg
    // install rustica-desktop replaces rustica-server)
    let mut replaced: Vec<String> = Vec::new();
    for t in &targets {
        for r in t.c.rels("Replaces").iter().flatten() {
            if let Some(i) = installed.iter().find(|i| i.name() == r.name && i.name() != t.c.name() && ops::satisfied(r, std::slice::from_ref(*i))) {
                if !replaced.contains(&i.name().to_string()) {
                    replaced.push(i.name().to_string());
                }
            }
        }
    }
    let staying: Vec<Control> = installed.iter().filter(|i| !replaced.contains(&i.name().to_string())).cloned().collect();
    let plan = solve::resolve(&staying, &av, targets)?;
    for n in &replaced {
        println!("{n} is replaced: removed first");
    }
    // Package files alone install without a question (as dpkg -i);
    // anything from a source is shown and asked about
    if plan.steps.iter().any(|s| s.avail.is_some()) && !confirm(&plan, yes)? {
        return Ok(());
    }
    if !replaced.is_empty() {
        ctx.remove(&replaced, false)?;
    }
    carry_out(ctx, plan, files)
}

pub fn upgrade(ctx: &Ctx, yes: bool) -> Result<(), String> {
    let av = need_lists(ctx)?;
    let installed = ctx.db.installed()?;
    let plan = solve::upgrade(&installed, &av);
    if plan.steps.is_empty() {
        for (n, why) in &plan.kept {
            println!("Kept back: {n}: {why}");
        }
        println!("Nothing to upgrade.");
        return Ok(());
    }
    if !confirm(&plan, yes)? {
        return Ok(());
    }
    carry_out(ctx, plan, Vec::new())
}

pub fn autoremove(ctx: &Ctx, yes: bool) -> Result<(), String> {
    let installed = ctx.db.installed()?;
    let names = solve::unneeded(&installed);
    if names.is_empty() {
        println!("Nothing to remove: every package installed for another is still needed.");
        return Ok(());
    }
    println!("Installed for other packages and no longer needed:");
    for n in &names {
        let v = installed.iter().find(|c| c.name() == n).map(|c| c.version()).unwrap_or("");
        println!("  {n} {v}");
    }
    if !yes {
        print!("Remove them? [Y/n] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 || !matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes") {
            println!("Abort.");
            return Ok(());
        }
    }
    ctx.remove(&names, false)
}

pub fn search(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let names_only = args.iter().any(|a| a == "--names-only");
    let words: Vec<String> = args.iter().filter(|a| !a.starts_with("--")).map(|a| a.to_lowercase()).collect();
    if words.is_empty() {
        return Err("search for what? (rpg search WORD...)".into());
    }
    let av = need_lists(ctx)?;
    let installed = ctx.db.installed()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut hits: Vec<&Avail> = Vec::new();
    let mut names: Vec<&str> = av.iter().map(|a| a.name()).collect();
    names.sort();
    names.dedup();
    for n in names {
        let a = repo::candidate(&av, n).unwrap();
        let hay = if names_only { n.to_string() } else { format!("{n} {}", a.c.get("Description").unwrap_or("")).to_lowercase() };
        if words.iter().all(|w| hay.contains(w.as_str())) && seen.insert(n) {
            hits.push(a);
        }
    }
    if hits.is_empty() {
        println!("Nothing found.");
        return Ok(());
    }
    for a in hits {
        let mark = match installed.iter().find(|c| c.name() == a.name()) {
            Some(c) if c.version() == a.version() => "  [installed]".to_string(),
            Some(c) => format!("  [installed: {}]", c.version()),
            None => String::new(),
        };
        println!("{} {} - {}{mark}", a.name(), a.version(), a.c.summary());
    }
    Ok(())
}

/// rpg show NAME for a package that is not installed (or its candidate)
pub fn show_available(ctx: &Ctx, name: &str) -> Result<bool, String> {
    let av = repo::available(&ctx.db)?;
    let Some(a) = repo::candidate(&av, name) else { return Ok(false) };
    print!("{}", a.c.text());
    println!("Source: {} {}", a.source.base(), a.source.suite);
    Ok(true)
}

pub fn policy(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        for s in repo::sources(&ctx.db)? {
            println!("{}", s.text());
        }
        return Ok(());
    }
    let av = repo::available(&ctx.db)?;
    for n in args {
        let inst = ctx.db.installed()?.into_iter().find(|c| c.name() == n);
        let cand = repo::candidate(&av, n);
        if inst.is_none() && cand.is_none() {
            return Err(format!("{n}: not installed, and no source has it"));
        }
        println!("{n}:");
        println!("  Installed: {}", inst.as_ref().map(|c| c.version()).unwrap_or("(none)"));
        println!("  Candidate: {}", cand.map(|c| c.version()).unwrap_or("(none)"));
        if inst.as_ref().and_then(|c| c.get("Hold")) == Some("yes") {
            println!("  Held: rpg upgrade leaves it (rpg unhold {n})");
        }
        if inst.as_ref().and_then(|c| c.get("Auto")) == Some("yes") {
            println!("  Installed for another package (rpg autoremove removes it once nothing needs it)");
        }
        let mut vs: Vec<&Avail> = av.iter().filter(|a| a.name() == n).collect();
        vs.sort_by(|a, b| version::compare(b.version(), a.version()));
        println!("  Versions:");
        let iv = inst.as_ref().map(|c| c.version().to_string());
        let mut shown_installed = false;
        for a in vs {
            let star = iv.as_deref() == Some(a.version());
            shown_installed |= star;
            println!("   {} {}  {} {} {}", if star { "***" } else { "   " }, a.version(), a.source.base(), a.source.suite, a.c.get("Section").unwrap_or(""));
        }
        if let (Some(v), false) = (&iv, shown_installed) {
            println!("   *** {v}  (installed; no source has it)");
        }
    }
    Ok(())
}

pub fn depends(ctx: &Ctx, args: &[String], reverse: bool) -> Result<(), String> {
    let [name] = args else { return Err("depends of one package".into()) };
    let installed = ctx.db.installed()?;
    let av = repo::available(&ctx.db)?;
    if !reverse {
        let c = installed.iter().find(|c| c.name() == name).cloned().or_else(|| repo::candidate(&av, name).map(|a| a.c.clone()));
        let c = c.ok_or(format!("{name}: not installed, and no source has it"))?;
        println!("{} {}:", c.name(), c.version());
        for f in ["Depends", "Conflicts", "Provides"] {
            for cl in c.rels(f) {
                println!("  {f}: {}", cl.iter().map(|r| r.text()).collect::<Vec<_>>().join(" | "));
            }
        }
        return Ok(());
    }
    println!("{name} is needed by:");
    let mut any = false;
    let mut seen = std::collections::BTreeSet::new();
    for c in installed.iter().chain(av.iter().map(|a| &a.c)) {
        if c.rels("Depends").iter().flatten().any(|r| r.name == *name) && seen.insert(c.name().to_string()) {
            let inst = installed.iter().any(|i| i.name() == c.name());
            println!("  {}{}", c.name(), if inst { " (installed)" } else { "" });
            any = true;
        }
    }
    if !any {
        println!("  nothing");
    }
    Ok(())
}

pub fn download(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let av = need_lists(ctx)?;
    let mut f = fetcher(ctx);
    for n in args {
        let a = repo::candidate(&av, n).ok_or(format!("{n}: no source has it"))?;
        let d = f.get(&a.url()).map_err(|e| format!("{}: {}", a.url(), e.text()))?;
        if d.len() as u64 != a.size || sha256_hex(&d) != a.sha {
            return Err(format!("{n}: the download's SHA-256 differs from the signed index: refused"));
        }
        let base = a.filename.rsplit('/').next().unwrap_or(&a.filename);
        std::fs::write(base, &d).map_err(|e| format!("{base}: {e}"))?;
        println!("{base}: {} {} ({})", a.name(), a.version(), kib(a.size));
    }
    Ok(())
}

pub fn clean(ctx: &Ctx) -> Result<(), String> {
    let d = ctx.db.path("/var/cache/rpg/archives");
    let (mut n, mut size) = (0, 0u64);
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "rpg") {
                size += e.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(e.path()).is_ok() {
                    n += 1;
                }
            }
        }
    }
    println!("{n} downloaded packages removed ({}) from {}", kib(size), d.display());
    Ok(())
}

pub fn source(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let s = repo::sources(&ctx.db)?;
            if s.is_empty() {
                println!("No sources (rpg source add URL SUITE SECTION).");
            }
            for (i, s) in s.iter().enumerate() {
                println!("{:3}  {}    ({})", i + 1, s.text(), s.file.display());
            }
            Ok(())
        }
        Some("add") => {
            let line = args[1..].join(" ");
            let file = repo::list_file(&ctx.db);
            let s = repo::parse_source(&line, &file)?.ok_or("add what? (rpg source add URL SUITE SECTION...)")?;
            if repo::sources(&ctx.db)?.iter().any(|o| o.base() == s.base() && o.suite == s.suite) {
                return Err(format!("{} {}: already a source", s.base(), s.suite));
            }
            let mut t = std::fs::read_to_string(&file).unwrap_or_default();
            if !t.is_empty() && !t.ends_with('\n') {
                t.push('\n');
            }
            t.push_str(&s.text());
            t.push('\n');
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
            crate::db::write_atomic(&file, t.as_bytes(), 0o644)?;
            println!("added: {}; rpg update reads it", s.text());
            Ok(())
        }
        Some("remove") => {
            let n: usize = args.get(1).and_then(|n| n.parse().ok()).ok_or("remove which? (rpg source remove N; rpg source list numbers them)")?;
            let all = repo::sources(&ctx.db)?;
            let s = all.get(n.wrapping_sub(1)).ok_or(format!("there is no source {n}"))?;
            // Its line in its file, as written
            let t = std::fs::read_to_string(&s.file).map_err(|e| format!("{}: {e}", s.file.display()))?;
            let mut done = false;
            let out: Vec<&str> = t
                .lines()
                .filter(|l| {
                    if done {
                        return true;
                    }
                    let same = repo::parse_source(l, &s.file).ok().flatten().is_some_and(|o| o.base() == s.base() && o.suite == s.suite);
                    if same {
                        done = true;
                    }
                    !same
                })
                .collect();
            let mut text = out.join("\n");
            text.push('\n');
            crate::db::write_atomic(&s.file, text.as_bytes(), 0o644)?;
            println!("removed: {}", s.text());
            Ok(())
        }
        Some(x) => Err(format!("source {x}: list, add or remove")),
    }
}

pub fn key(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let apps = args.iter().any(|a| a == "--apps");
    let kind = if apps { Kind::App } else { Kind::System };
    let args: Vec<&String> = args.iter().filter(|a| *a != "--apps").collect();
    match args.first().map(|s| s.as_str()) {
        None | Some("list") => {
            for k in [Kind::System, Kind::App] {
                for key in keys::load(&ctx.db, k) {
                    println!("{}  {}  {}{}", key.id(), key.name, key.file.display(), if k == Kind::App { "  (apps)" } else { "" });
                }
            }
            Ok(())
        }
        Some("add") => {
            let f = args.get(1).ok_or("add which key? (rpg key add FILE [NAME])")?;
            let t = std::fs::read_to_string(f.as_str()).map_err(|e| format!("{f}: {e}"))?;
            let (k, n) = keys::parse(&t).map_err(|e| format!("{f}: {e}"))?;
            let name = args.get(2).map(|s| s.to_string()).or(n).unwrap_or_else(|| {
                std::path::Path::new(f.as_str()).file_stem().unwrap_or_default().to_string_lossy().trim_end_matches(".pub").to_string()
            });
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
                return Err(format!("{name:?}: a key's name is letters, digits and - _ ."));
            }
            let dir = keys::dir(&ctx.db, kind);
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let key = keys::Key { name: name.clone(), key: k, file: dir.join(format!("{name}.pub")) };
            if keys::load(&ctx.db, kind).iter().any(|o| o.key == k) {
                return Err(format!("{}: already trusted", key.id()));
            }
            crate::db::write_atomic(&key.file, format!("{}\n", key.line()).as_bytes(), 0o644)?;
            println!("trusted{}: {} {} ({})", if apps { " for apps" } else { "" }, key.id(), name, key.file.display());
            Ok(())
        }
        Some("remove") => {
            let w = args.get(1).ok_or("remove which key? (its ID or name, from rpg key list)")?;
            for k in [Kind::System, Kind::App] {
                for key in keys::load(&ctx.db, k) {
                    if key.id() == **w || key.name == **w {
                        std::fs::remove_file(&key.file).map_err(|e| format!("{}: {e}", key.file.display()))?;
                        println!("no longer trusted: {} {}", key.id(), key.name);
                        return Ok(());
                    }
                }
            }
            Err(format!("{w}: no such key (rpg key list)"))
        }
        Some(x) => Err(format!("key {x}: list, add or remove")),
    }
}

pub fn index(args: &[String]) -> Result<(), String> {
    let mut o = repo::IndexOpts { suite: "redfox".into(), section: "main".into(), archs: Vec::new(), origin: "Rustux".into() };
    let mut dir = None;
    let mut i = 0;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).cloned().ok_or(format!("{} needs a value", args[i]));
        match args[i].as_str() {
            "--suite" => o.suite = val(i)?,
            "--section" => o.section = val(i)?,
            "--arch" => o.archs.push(val(i)?),
            "--origin" => o.origin = val(i)?,
            d if !d.starts_with('-') && dir.is_none() => {
                dir = Some(d.to_string());
                i += 1;
                continue;
            }
            x => return Err(format!("index: {x}?")),
        }
        i += 2;
    }
    let dir = dir.ok_or("index which repository? (rpg index DIR: DIR/pool/... holds the .rpg files)")?;
    repo::index(std::path::Path::new(&dir), &o)
}

/// rpg list --upgradable
pub fn list_upgradable(ctx: &Ctx) -> Result<(), String> {
    let av = need_lists(ctx)?;
    let installed = ctx.db.installed()?;
    let up = upgradable(&installed, &av);
    if up.is_empty() {
        println!("Everything installed is up to date.");
    }
    for (c, a) in up {
        println!("{}  {} -> {}{}", c.name(), c.version(), a.version(), if c.get("Hold") == Some("yes") { "  (held)" } else { "" });
    }
    Ok(())
}

/// The pool path a package file belongs at (for the publishing side's
/// scripts): rpg pool-path FILE.rpg [SECTION]
pub fn pool_path(args: &[String]) -> Result<(), String> {
    let f = args.first().ok_or("pool-path FILE.rpg [SECTION]")?;
    let bytes = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
    let p = Package::read(&bytes).map_err(|e| format!("{f}: {e}"))?;
    let base = std::path::Path::new(f).file_name().unwrap().to_string_lossy().to_string();
    println!("{}", repo::pool_path(args.get(1).map(|s| s.as_str()).unwrap_or("main"), p.control.name(), &base));
    Ok(())
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Repositories (step E2): sources, `rpg update`, the package lists, and
//! `rpg index` (the publishing side).
//!
//! A repository is static files (any web server, mirror or disk):
//!
//!   dists/SUITE/Release        Origin, Suite, Date, Architectures,
//!                              Sections, and SHA256: a line per index
//!                              ("HASH SIZE SECTION/ARCH/Packages.gz")
//!   dists/SUITE/Release.sig    the Ed25519 signature of Release
//!   dists/SUITE/SECTION/ARCH/Packages.gz
//!                              a stanza per package: its control
//!                              fields and Filename, Size, SHA256
//!   pool/SECTION/L/NAME/NAME_VERSION_ARCH.rpg
//!
//! One signature covers everything: Release names each index's hash,
//! each index each package's. Sources are lines of
//! /etc/rpg/sources.list and /etc/rpg/sources.list.d/*.list:
//!
//!   https://rustux.com/download/rustica  redfox  main
//!   apps https://rustux.com/download/apps platform 1     (FoxDen's, FD3)
//!
//! An app source (FoxDen step FD3, docs/FOXDEN.md) is laid out by
//! platform level instead of release:
//!
//!   platform/LEVEL/Release, Release.sig   signed with an apps key
//!                              (/etc/rpg/keys/apps), never the archive key
//!   platform/LEVEL/ARCH/Apps.gz   a stanza per app: its control fields,
//!                              Filename, Size, SHA256, and from its
//!                              manifest App-Name, App-Summary, Categories,
//!                              Permissions, Platform, Icon (media/ID/FILE),
//!                              Screenshots (media/ID/FILE...), and
//!                              App-Developer, App-Developer-Name,
//!                              App-Source (FoxDen step FD7a)
//!   platform/LEVEL/ARCH/Apps.json.gz   the same, for web pages
//!   pool/L/ID/ID_VERSION_ARCH.rpg, media/ID/...
//!
//! Every entry of an app source must be an app, and every package from it
//! one: a system package there refuses the whole source.
//! `rpg update` keeps the lists in /var/lib/rpg/lists/; a source that
//! fails (unsigned, a wrong signature, a hash that differs, a server
//! that cannot be reached) keeps its old lists.

use crate::control::{self, Control};
use crate::db::{self, Db};
use crate::fetch::{self, Fetcher};
use crate::keys::{self, Kind};
use crate::pkg::{sha256_hex, Package};
use crate::version;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const ARCH: &str = std::env::consts::ARCH;

#[derive(Clone, Debug)]
pub struct Source {
    pub kind: Kind,
    pub url: String,
    pub suite: String,
    pub sections: Vec<String>,
    /// The file it is in
    pub file: PathBuf,
}

impl Source {
    pub fn base(&self) -> &str {
        self.url.trim_end_matches('/')
    }
    pub fn text(&self) -> String {
        format!(
            "{}{}  {}  {}",
            if self.kind == Kind::App { "apps " } else { "" },
            self.url,
            self.suite,
            self.sections.join(" ")
        )
    }
    /// The lists' file names start with this
    pub fn prefix(&self) -> String {
        let u = self.base().split_once("://").map_or(self.base(), |(_, r)| r);
        let mut s: String = u.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        s.push('_');
        s.push_str(&self.suite);
        s
    }
}

pub fn parse_source(line: &str, file: &Path) -> Result<Option<Source>, String> {
    let line = line.split('#').next().unwrap_or("");
    let mut w: Vec<&str> = line.split_whitespace().collect();
    if w.is_empty() {
        return Ok(None);
    }
    let kind = if w[0] == "apps" {
        w.remove(0);
        Kind::App
    } else {
        Kind::System
    };
    if w.len() < 3 {
        return Err(format!("{line:?}: a source is URL SUITE SECTION... (https://rustux.com/download/rustica redfox main)"));
    }
    let url = w[0];
    if !["https://", "http://", "file://"].iter().any(|p| url.starts_with(p)) {
        return Err(format!("{url}: a source's address starts with https://, http:// or file://"));
    }
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if !w[1..].iter().all(|s| ok(s)) {
        return Err(format!("{line:?}: a suite and sections are letters, digits and - _ ."));
    }
    if kind == Kind::App {
        let level = w[2].parse::<u32>().ok().filter(|l| *l >= 1);
        if w[1] != "platform" || w.len() != 3 || level.is_none() {
            return Err(format!("{line:?}: an app source is apps URL platform LEVEL (apps https://rustux.com/download/apps platform 1)"));
        }
    }
    Ok(Some(Source { kind, url: url.to_string(), suite: w[1].to_string(), sections: w[2..].iter().map(|s| s.to_string()).collect(), file: file.to_path_buf() }))
}

pub fn list_file(db: &Db) -> PathBuf {
    db.path("/etc/rpg/sources.list")
}

/// Every source, sources.list first, then sources.list.d/*.list
pub fn sources(db: &Db) -> Result<Vec<Source>, String> {
    let mut files = vec![list_file(db)];
    if let Ok(rd) = std::fs::read_dir(db.path("/etc/rpg/sources.list.d")) {
        let mut more: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "list")).collect();
        more.sort();
        files.extend(more);
    }
    let mut out = Vec::new();
    for f in files {
        let Ok(t) = std::fs::read_to_string(&f) else { continue };
        for (n, l) in t.lines().enumerate() {
            match parse_source(l, &f) {
                Ok(Some(s)) => out.push(s),
                Ok(None) => {}
                Err(e) => return Err(format!("{}:{}: {e}", f.display(), n + 1)),
            }
        }
    }
    Ok(out)
}

/// What Release says
pub struct Release {
    pub fields: Control,
    /// path (SECTION/ARCH/Packages.gz) -> (SHA-256, size)
    pub files: Vec<(String, String, u64)>,
}

pub fn parse_release(text: &str) -> Result<Release, String> {
    let c = control::parse_stanzas(text)?.into_iter().next().ok_or("an empty Release file")?;
    let mut files = Vec::new();
    for l in c.get("SHA256").unwrap_or("").lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() != 3 {
            continue;
        }
        let size = w[1].parse().map_err(|_| format!("Release: a bad size in {l:?}"))?;
        files.push((w[2].to_string(), w[0].to_lowercase(), size));
    }
    Ok(Release { fields: c, files })
}

pub fn lists_dir(db: &Db) -> PathBuf {
    db.path("/var/lib/rpg/lists")
}

/// A package a source has
#[derive(Clone, Debug)]
pub struct Avail {
    pub c: Control,
    pub source: Source,
    pub filename: String,
    pub size: u64,
    pub sha: String,
    /// From FoxDen: an app source whose index a store key signed
    /// (/etc/rpg/store-keys; FoxDen step FD7c)
    pub store: bool,
}

impl Avail {
    /// Where it comes from, as the database records it (Origin): foxden,
    /// or "source URL"
    pub fn origin(&self) -> String {
        if self.store {
            "foxden".into()
        } else {
            format!("source {}", self.source.base())
        }
    }
    pub fn url(&self) -> String {
        format!("{}/{}", self.source.base(), self.filename)
    }
    pub fn name(&self) -> &str {
        self.c.name()
    }
    pub fn version(&self) -> &str {
        self.c.version()
    }
}

/// Where a source's Release is: dists/SUITE, or platform/LEVEL for apps
pub fn dist_base(s: &Source) -> String {
    match s.kind {
        Kind::System => format!("{}/dists/{}", s.base(), s.suite),
        Kind::App => format!("{}/platform/{}", s.base(), s.sections[0]),
    }
}

/// The indexes of a source, as Release names them, with the lists' file
/// names: SECTION/ARCH/Packages.gz, or ARCH/Apps.gz
fn indexes(s: &Source) -> Vec<(String, String)> {
    match s.kind {
        Kind::System => s.sections.iter().map(|sec| (format!("{sec}/{ARCH}/Packages.gz"), format!("{}_{sec}_{ARCH}_Packages", s.prefix()))).collect(),
        Kind::App => vec![(format!("{ARCH}/Apps.gz"), format!("{}_{}_{ARCH}_Apps", s.prefix(), s.sections[0]))],
    }
}

/// rpg update: each source's Release, its signature, its indexes.
/// Returns how many sources failed.
pub fn update(db: &Db, f: &mut Fetcher) -> Result<usize, String> {
    let srcs = sources(db)?;
    let dir = lists_dir(db);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if srcs.is_empty() {
        println!("No sources: rpg source add URL SUITE SECTION (/etc/rpg/sources.list)");
    }
    let mut failed = 0;
    let mut keep: Vec<String> = Vec::new();
    for s in &srcs {
        let label = format!("{} {}", s.base(), s.suite);
        keep.push(s.prefix());
        let label = if s.kind == Kind::App { format!("{} platform {}", s.base(), s.sections[0]) } else { label };
        if s.kind == Kind::App && s.sections[0].parse::<u32>().unwrap_or(0) > crate::app::PLATFORM {
            failed += 1;
            println!("{label}: REFUSED: this system has platform {} (a newer Rustica reads it)", crate::app::PLATFORM);
            continue;
        }
        match update_one(db, f, s) {
            Ok(n) if s.kind == Kind::App => println!("{label}: {n} apps"),
            Ok(n) => println!("{label}: {n} packages"),
            Err(e) => {
                failed += 1;
                println!("{label}: REFUSED: {e}");
                let had = std::fs::read_dir(&dir).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with(&s.prefix()))).unwrap_or(false);
                if had {
                    println!("  (its lists from before are kept)");
                }
            }
        }
    }
    // Lists of sources no longer there
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !keep.iter().any(|p| n.starts_with(&format!("{p}_"))) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(failed)
}

fn update_one(db: &Db, f: &mut Fetcher, s: &Source) -> Result<usize, String> {
    let base = dist_base(s);
    let release = f.get(&format!("{base}/Release")).map_err(|e| match e {
        fetch::Error::NotFound => format!("no such repository ({base}/Release: {})", e.text()),
        e => format!("{base}/Release: {}", e.text()),
    })?;
    let sig = f.get(&format!("{base}/Release.sig")).map_err(|e| format!("not signed ({base}/Release.sig: {})", e.text()))?;
    // A key is trusted for its kind only: an app source's Release must be
    // signed by an apps key, a system source's by an archive key
    let trusted = keys::load(db, s.kind);
    if trusted.is_empty() {
        return Err(match s.kind {
            Kind::System => "no trusted keys (/etc/rpg/keys; rpg key add FILE)".into(),
            Kind::App => "no trusted apps keys (/etc/rpg/keys/apps; rpg key add --apps FILE)".to_string(),
        });
    }
    let signer = keys::verify(&trusted, &release, &sig).ok_or(match s.kind {
        Kind::System => "the signature of Release is not by a trusted key (or Release was changed after signing)",
        Kind::App => "the signature of Release is not by a trusted apps key (or Release was changed after signing)",
    })?;
    let text = String::from_utf8(release.clone()).map_err(|_| "Release is not text")?;
    let rel = parse_release(&text)?;
    if rel.fields.get("Suite") != Some(s.suite.as_str()) {
        return Err(format!("Release is for {:?}, not {}", rel.fields.get("Suite").unwrap_or("?"), s.suite));
    }
    if s.kind == Kind::App && rel.fields.get("Platform") != Some(s.sections[0].as_str()) {
        return Err(format!("Release is for platform {:?}, not {}", rel.fields.get("Platform").unwrap_or("?"), s.sections[0]));
    }
    let mut got: Vec<(String, Vec<u8>)> = Vec::new();
    let mut count = 0;
    for (path, list) in indexes(s) {
        let Some((_, hash, size)) = rel.files.iter().find(|(p, _, _)| *p == path) else {
            return Err(format!("Release has no {path} (sections: {})", rel.fields.get("Sections").unwrap_or("?")));
        };
        let gz = f.get(&format!("{base}/{path}")).map_err(|e| format!("{base}/{path}: {}", e.text()))?;
        if gz.len() as u64 != *size || sha256_hex(&gz) != *hash {
            return Err(format!("{path}: its SHA-256 differs from Release's (changed after signing?)"));
        }
        let mut plain = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut plain).map_err(|e| format!("{path}: not gzip ({e})"))?;
        let text = String::from_utf8(plain).map_err(|_| format!("{path}: not text"))?;
        let st = control::parse_stanzas(&text).map_err(|e| format!("{path}: {e}"))?;
        for c in &st {
            index_entry(c, s).map_err(|e| format!("{path}: {e}"))?;
        }
        count += st.len();
        got.push((list, text.into_bytes()));
    }
    // All of it checked: now into the lists
    let dir = lists_dir(db);
    db::write_atomic(&dir.join(format!("{}_Release", s.prefix())), &release, 0o644)?;
    db::write_atomic(&dir.join(format!("{}_Release.sig", s.prefix())), &sig, 0o644)?;
    for (n, d) in got {
        db::write_atomic(&dir.join(n), &d, 0o644)?;
    }
    let _ = signer;
    Ok(count)
}

fn index_entry(c: &Control, s: &Source) -> Result<Avail, String> {
    c.check().map_err(|e| format!("{}: {e}", c.name()))?;
    if s.kind == Kind::App {
        // An app source has apps only: a system package there is refused
        // (with the whole source), whoever signed it
        if c.kind() != "app" {
            return Err(format!("{}: a system package in an app source: refused (system packages come from the system's sources only)", c.name()));
        }
        if !crate::app::valid_id(c.name()) {
            return Err(format!("{}: not an app ID", c.name()));
        }
        let level = c.get("Platform").and_then(|v| v.trim().parse::<u32>().ok()).ok_or(format!("{}: no Platform", c.name()))?;
        if level == 0 || level > s.sections[0].parse().unwrap_or(0) {
            return Err(format!("{}: made for platform {level}, in the index of platform {}", c.name(), s.sections[0]));
        }
    }
    let filename = c.get("Filename").ok_or(format!("{}: no Filename", c.name()))?.to_string();
    if filename.starts_with('/') || filename.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err(format!("{}: a bad Filename", c.name()));
    }
    let size = c.get("Size").and_then(|v| v.parse().ok()).ok_or(format!("{}: no Size", c.name()))?;
    let sha = c.get("SHA256").filter(|h| h.len() == 64).ok_or(format!("{}: no SHA256", c.name()))?.to_lowercase();
    Ok(Avail { c: c.clone(), source: s.clone(), filename, size, sha, store: false })
}

/// The store's keys (FoxDen step FD7c): an app source whose index one of
/// them signs is FoxDen; any other is "not from FoxDen". Key IDs in
/// /etc/rpg/store-keys, one a line (# comments); without the file,
/// rustux-apps-2026's
pub const STORE_KEY: &str = "2f06e52fed979364";

pub fn store_keys(db: &Db) -> Vec<String> {
    match std::fs::read_to_string(db.path("/etc/rpg/store-keys")) {
        Ok(t) => t.lines().filter_map(|l| l.split('#').next()).filter_map(|l| l.split_whitespace().next()).map(str::to_lowercase).collect(),
        Err(_) => vec![STORE_KEY.into()],
    }
}

/// Does a store key sign this app source's index (the Release and its
/// signature rpg update kept)?
fn is_store(db: &Db, s: &Source, store: &[String]) -> bool {
    if s.kind != Kind::App {
        return false;
    }
    let dir = lists_dir(db);
    let (Ok(rel), Ok(sig)) = (std::fs::read(dir.join(format!("{}_Release", s.prefix()))), std::fs::read(dir.join(format!("{}_Release.sig", s.prefix())))) else {
        return false;
    };
    let keys = keys::load(db, Kind::App);
    keys::verify(&keys, &rel, &sig).is_some_and(|k| store.contains(&k.id()))
}

/// Every package the sources have (from the lists rpg update made)
pub fn available(db: &Db) -> Result<Vec<Avail>, String> {
    let mut out = Vec::new();
    let dir = lists_dir(db);
    let store = store_keys(db);
    for s in sources(db)? {
        let from_store = is_store(db, &s, &store);
        for (_, list) in indexes(&s) {
            let p = dir.join(list);
            let Ok(t) = std::fs::read_to_string(&p) else { continue };
            for c in control::parse_stanzas(&t).map_err(|e| format!("{}: {e}", p.display()))? {
                if c.arch() == ARCH || c.arch() == "all" {
                    let mut a = index_entry(&c, &s)?;
                    a.store = from_store;
                    out.push(a);
                }
            }
        }
    }
    Ok(out)
}

/// The version rpg would install: the highest any source has (the first
/// source's when two have the same)
pub fn candidate<'a>(av: &'a [Avail], name: &str) -> Option<&'a Avail> {
    let mut best: Option<&Avail> = None;
    for a in av.iter().filter(|a| a.name() == name) {
        if best.is_none_or(|b| version::compare(a.version(), b.version()) == std::cmp::Ordering::Greater) {
            best = Some(a);
        }
    }
    best
}

/// Were the lists ever made?
pub fn have_lists(db: &Db) -> bool {
    std::fs::read_dir(lists_dir(db)).map(|mut d| d.next().is_some()).unwrap_or(false)
}

// ---- The publishing side: rpg index ----

pub struct IndexOpts {
    pub suite: String,
    pub section: String,
    pub archs: Vec<String>,
    pub origin: String,
}

/// The architectures a repository has an index for: every one Rustux
/// knows, so a computer of any of them finds its index (with the packages
/// for `all` at least), plus any other a package names.
fn index_archs(entries: &[Control]) -> Vec<String> {
    let mut archs: Vec<String> = crate::control::ARCHS.iter().map(|a| a.to_string()).collect();
    for c in entries {
        if c.arch() != "all" && !archs.iter().any(|a| a == c.arch()) {
            archs.push(c.arch().to_string());
        }
    }
    archs
}

/// rpg index REPO: every .rpg under REPO/pool (checked whole), into
/// REPO/dists/SUITE/SECTION/ARCH/Packages.gz (packages for all in every
/// architecture's) and REPO/dists/SUITE/Release. Signing is apart.
pub fn index(repo: &Path, o: &IndexOpts) -> Result<(), String> {
    let pool = repo.join("pool");
    let mut files = Vec::new();
    walk_rpg(&pool, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("{}: no .rpg files under it", pool.display()));
    }
    let mut entries: Vec<Control> = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let p = Package::read(&bytes).map_err(|e| format!("{}: {e}", f.display()))?;
        let mut c = p.control.clone();
        if entries.iter().any(|e| e.name() == c.name() && e.version() == c.version() && e.arch() == c.arch()) {
            return Err(format!("{} {} ({}) is in the pool twice", c.name(), c.version(), c.arch()));
        }
        let rel = f.strip_prefix(repo).unwrap().to_string_lossy().to_string();
        c.set("Filename", &rel);
        c.set("Size", &bytes.len().to_string());
        c.set("SHA256", &sha256_hex(&bytes));
        entries.push(c);
    }
    let mut archs = o.archs.clone();
    if archs.is_empty() {
        archs = index_archs(&entries);
    }
    let dists = repo.join("dists").join(&o.suite);
    let mut sums = Vec::new();
    for a in &archs {
        let mut mine: Vec<&Control> = entries.iter().filter(|c| c.arch() == a || c.arch() == "all").collect();
        mine.sort_by(|x, y| x.name().cmp(y.name()).then(version::compare(x.version(), y.version())));
        let text: String = mine.iter().map(|c| c.text()).collect::<Vec<_>>().join("\n");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut gz, text.as_bytes()).map_err(|e| e.to_string())?;
        let gz = gz.finish().map_err(|e| e.to_string())?;
        let rel = format!("{}/{a}/Packages.gz", o.section);
        let p = dists.join(&rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, &gz).map_err(|e| format!("{}: {e}", p.display()))?;
        sums.push(format!(" {} {} {rel}", sha256_hex(&gz), gz.len()));
        println!("{}: {} packages", p.display(), mine.len());
    }
    let release = format!(
        "Origin: {}\nSuite: {}\nDate: {}\nArchitectures: {}\nSections: {}\nSHA256:\n{}\n",
        o.origin,
        o.suite,
        now_text(),
        archs.join(" "),
        o.section,
        sums.join("\n")
    );
    let rp = dists.join("Release");
    std::fs::write(&rp, release).map_err(|e| format!("{}: {e}", rp.display()))?;
    let _ = std::fs::remove_file(dists.join("Release.sig"));
    println!("{}: written; now sign it:", rp.display());
    println!("  openssl pkeyutl -sign -inkey KEY.pem -rawin -in {} -out {}.sig", rp.display(), rp.display());
    Ok(())
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// rpg index --apps REPO [--platform LEVEL]: every app package under
/// REPO/pool (each checked as an install would: app.rs), into
/// REPO/platform/LEVEL/ARCH/Apps.gz and Apps.json.gz and
/// REPO/platform/LEVEL/Release; each app's icon copied to
/// REPO/media/ID/. A system package in the pool is refused. Signing is
/// apart, as for rpg index.
pub fn index_apps(repo: &Path, level: u32, origin: &str) -> Result<(), String> {
    let pool = repo.join("pool");
    let mut files = Vec::new();
    walk_rpg(&pool, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("{}: no .rpg files under it", pool.display()));
    }
    let mut entries: Vec<Control> = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let p = Package::read(&bytes).map_err(|e| format!("{}: {e}", f.display()))?;
        if p.control.kind() != "app" {
            return Err(format!("{}: {} is a system package: an apps repository has apps only", f.display(), p.control.name()));
        }
        let m = crate::app::check(&p).map_err(|e| format!("{}: {e}", f.display()))?;
        if m.platform > level {
            return Err(format!("{}: made for platform {}, above this index's {level}", f.display(), m.platform));
        }
        let mut c = p.control.clone();
        if entries.iter().any(|e| e.name() == c.name() && e.version() == c.version() && e.arch() == c.arch()) {
            return Err(format!("{} {} ({}) is in the pool twice", c.name(), c.version(), c.arch()));
        }
        let rel = f.strip_prefix(repo).unwrap().to_string_lossy().to_string();
        c.set("Filename", &rel);
        c.set("Size", &bytes.len().to_string());
        c.set("SHA256", &sha256_hex(&bytes));
        c.set("App-Name", &m.name);
        c.set("App-Summary", &m.summary);
        if !m.categories.is_empty() {
            c.set("Categories", &m.categories.join(" "));
        }
        if !m.permissions.is_empty() {
            c.set("Permissions", &m.permissions.join(" "));
        }
        c.set("Platform", &m.platform.to_string());
        // Its developer and its source (FoxDen step FD7a)
        for (k, v) in [("App-Developer", &m.developer), ("App-Developer-Name", &m.developer_name), ("App-Source", &m.source)] {
            if let Some(v) = v {
                c.set(k, v);
            }
        }
        // The icon and screenshots, beside the pool (media/ID/), named by
        // their path in the app's folder ("shots/1.png" as shots-1.png)
        let media = |rel: &str| -> Result<Option<String>, String> {
            let Some(d) = p.data.get(&format!("{}/{rel}", crate::app::dir(c.name()))) else { return Ok(None) };
            let name = rel.replace('/', "-");
            let to = repo.join("media").join(c.name()).join(&name);
            std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| format!("{}: {e}", to.display()))?;
            std::fs::write(&to, d).map_err(|e| format!("{}: {e}", to.display()))?;
            Ok(Some(format!("media/{}/{name}", c.name())))
        };
        let icon = match &m.icon {
            Some(i) => media(i)?,
            None => None,
        };
        let mut shots = Vec::new();
        for s in &m.screenshots {
            if let Some(u) = media(s)? {
                shots.push(u);
            }
        }
        if let Some(i) = icon {
            c.set("Icon", &i);
        }
        if !shots.is_empty() {
            c.set("Screenshots", &shots.join(" "));
        }
        entries.push(c);
    }
    let archs = index_archs(&entries);
    let dist = repo.join("platform").join(level.to_string());
    let mut sums = Vec::new();
    for a in &archs {
        let mut mine: Vec<&Control> = entries.iter().filter(|c| c.arch() == a || c.arch() == "all").collect();
        mine.sort_by(|x, y| x.name().cmp(y.name()).then(version::compare(x.version(), y.version())));
        let text: String = mine.iter().map(|c| c.text()).collect::<Vec<_>>().join("\n");
        let json = format!(
            "[\n{}\n]\n",
            mine.iter()
                .map(|c| {
                    let f: Vec<String> = c.fields.iter().map(|(k, v)| format!("{}: {}", json_str(k), json_str(v))).collect();
                    format!("  {{{}}}", f.join(", "))
                })
                .collect::<Vec<_>>()
                .join(",\n")
        );
        for (name, data) in [("Apps.gz", text), ("Apps.json.gz", json)] {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            std::io::Write::write_all(&mut gz, data.as_bytes()).map_err(|e| e.to_string())?;
            let gz = gz.finish().map_err(|e| e.to_string())?;
            let rel = format!("{a}/{name}");
            let p = dist.join(&rel);
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", p.display()))?;
            std::fs::write(&p, &gz).map_err(|e| format!("{}: {e}", p.display()))?;
            sums.push(format!(" {} {} {rel}", sha256_hex(&gz), gz.len()));
        }
        println!("{}: {} apps", dist.join(a).join("Apps.gz").display(), mine.len());
    }
    let release = format!(
        "Origin: {origin}\nSuite: platform\nPlatform: {level}\nDate: {}\nArchitectures: {}\nSHA256:\n{}\n",
        now_text(),
        archs.join(" "),
        sums.join("\n")
    );
    let rp = dist.join("Release");
    std::fs::write(&rp, release).map_err(|e| format!("{}: {e}", rp.display()))?;
    let _ = std::fs::remove_file(dist.join("Release.sig"));
    println!("{}: written; now sign it with the apps key:", rp.display());
    println!("  openssl pkeyutl -sign -inkey APPSKEY.pem -rawin -in {} -out {}.sig", rp.display(), rp.display());
    Ok(())
}

/// The pool path of an app: pool/L/ID/FILE
pub fn app_pool_path(id: &str, file: &str) -> String {
    format!("pool/{}/{id}/{file}", &id[..1])
}

fn walk_rpg(d: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let rd = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_rpg(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "rpg") {
            out.push(p);
        }
    }
    Ok(())
}

/// The pool path for a package: pool/SECTION/L/NAME/FILE
pub fn pool_path(section: &str, name: &str, file: &str) -> String {
    let l = if name.starts_with("lib") && name.len() > 3 { &name[..4] } else { &name[..1] };
    format!("pool/{section}/{l}/{name}/{file}")
}

/// UTC now, as "2026-11-18 12:34:56 UTC"
fn now_text() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, secs) = ((t / 86400) as i64, t % 86400);
    // Days to a civil date (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", secs / 3600, secs % 3600 / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{self, Entry, Kind as AKind};
    use crate::keys::hex;
    use crate::pkg::{files_text, FType, FileEntry};
    use ed25519_dalek::{Signer, SigningKey};

    /// A package file's bytes: control, then (path, type, mode, data)
    fn rpg(control: &str, files: &[(&str, FType, u32, &[u8])]) -> Vec<u8> {
        let mut list = Vec::new();
        let mut entries = vec![Entry { path: "control".into(), mode: 0o644, kind: AKind::File(control.as_bytes().to_vec()) }];
        let mut data = Vec::new();
        for (p, t, mode, d) in files {
            let hash = if *t == FType::File { sha256_hex(d) } else { String::new() };
            list.push(FileEntry { typ: *t, mode: *mode, uid: 0, gid: 0, hash, conf: false, path: p.to_string() });
            data.push(Entry { path: format!("data{p}"), mode: *mode, kind: if *t == FType::File { AKind::File(d.to_vec()) } else { AKind::Dir } });
        }
        entries.push(Entry { path: "files".into(), mode: 0o644, kind: AKind::File(files_text(&list).into_bytes()) });
        entries.extend(data);
        archive::write(&entries)
    }

    fn app(id: &str, version: &str, perms: &str) -> Vec<u8> {
        let control = format!("Package: {id}\nVersion: {version}\nArchitecture: {ARCH}\nKind: app\nDescription: a test app\n");
        let manifest = format!("Id: {id}\nName: Test {id}\nSummary: a test app\nExec: bin/t\nIcon: icon.png\nCategories: Utility\n{perms}Platform: 1\n");
        let d = format!("/apps/{id}");
        rpg(
            &control,
            &[
                ("/apps", FType::Dir, 0o755, b""),
                (&d, FType::Dir, 0o755, b""),
                (&format!("{d}/bin"), FType::Dir, 0o755, b""),
                (&format!("{d}/bin/t"), FType::File, 0o755, b"\x7fELF"),
                (&format!("{d}/icon.png"), FType::File, 0o644, b"\x89PNG"),
                (&format!("{d}/manifest"), FType::File, 0o644, manifest.as_bytes()),
            ],
        )
    }

    struct T {
        dir: PathBuf,
    }
    impl Drop for T {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn tmp(name: &str) -> T {
        let dir = std::env::temp_dir().join(format!("rpg-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        T { dir }
    }

    fn put(repo: &Path, rel: &str, bytes: &[u8]) {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn sign(repo: &Path, k: &SigningKey) {
        let rel = repo.join("platform/1/Release");
        let sig = k.sign(&std::fs::read(&rel).unwrap()).to_bytes();
        std::fs::write(repo.join("platform/1/Release.sig"), sig).unwrap();
    }

    /// A client root trusting `apps` as an apps key and `system` as an
    /// archive key, with one source line
    fn client(t: &T, apps: &SigningKey, system: &SigningKey, line: &str) -> Db {
        let root = t.dir.join("root");
        for (d, k) in [("etc/rpg/keys/apps", apps), ("etc/rpg/keys", system)] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("k.pub"), format!("rpg-ed25519 {} test\n", hex(&k.verifying_key().to_bytes()))).unwrap();
        }
        std::fs::write(root.join("etc/rpg/sources.list"), format!("{line}\n")).unwrap();
        Db::new(&root)
    }

    #[test]
    fn every_architecture_has_an_index() {
        let all = Control::parse("Package: data\nVersion: 1\nArchitecture: all\nDescription: x\n").unwrap();
        assert_eq!(index_archs(&[all]), ["x86_64", "aarch64", "riscv64"]);
        assert_eq!(index_archs(&[]), ["x86_64", "aarch64", "riscv64"]);
    }

    #[test]
    fn app_sources_parse() {
        let f = Path::new("x");
        let s = parse_source("apps https://rustux.com/download/apps platform 1", f).unwrap().unwrap();
        assert_eq!(s.kind, Kind::App);
        assert_eq!(dist_base(&s), "https://rustux.com/download/apps/platform/1");
        assert!(parse_source("apps https://rustux.com/download/apps redfox main", f).is_err());
        assert!(parse_source("apps https://rustux.com/download/apps platform x", f).is_err());
        assert!(parse_source("apps https://rustux.com/download/apps platform 1 2", f).is_err());
        let s = parse_source("https://rustux.com/download/rustica redfox main", f).unwrap().unwrap();
        assert_eq!(dist_base(&s), "https://rustux.com/download/rustica/dists/redfox");
    }

    #[test]
    fn apps_repository() {
        let t = tmp("apps");
        let repo = t.dir.join("repo");
        let apps_key = SigningKey::from_bytes(&[3u8; 32]);
        let sys_key = SigningKey::from_bytes(&[4u8; 32]);
        put(&repo, &app_pool_path("org.test.one", "org.test.one_1.0_x86_64.rpg"), &app("org.test.one", "1.0", ""));
        put(&repo, &app_pool_path("org.test.two", "org.test.two_2.0_x86_64.rpg"), &app("org.test.two", "2.0", "Permissions: network\n"));
        index_apps(&repo, 1, "Test apps").unwrap();
        assert!(repo.join("media/org.test.one/icon.png").is_file());
        let json = {
            let mut s = String::new();
            GzDecoder::new(&std::fs::read(repo.join(format!("platform/1/{ARCH}/Apps.json.gz"))).unwrap()[..]).read_to_string(&mut s).unwrap();
            s
        };
        assert!(json.contains("\"App-Name\": \"Test org.test.two\"") && json.contains("\"Permissions\": \"network\""), "{json}");
        let line = format!("apps file://{}  platform 1", repo.display());

        // Signed with the apps key: read, two apps from an app source
        sign(&repo, &apps_key);
        let db = client(&t, &apps_key, &sys_key, &line);
        let mut f = Fetcher::new(db.path("/etc/rpg/ca.pem"));
        assert_eq!(update(&db, &mut f).unwrap(), 0);
        let av = available(&db).unwrap();
        assert_eq!(av.len(), 2);
        assert!(av.iter().all(|a| a.source.kind == Kind::App));
        let two = candidate(&av, "org.test.two").unwrap();
        assert_eq!(two.c.get("Permissions"), Some("network"));
        assert_eq!(two.c.get("Icon"), Some("media/org.test.two/icon.png"));

        // Signed with the archive key: not trusted for apps (the lists from
        // before are kept, as for any refused source)
        sign(&repo, &sys_key);
        let _ = std::fs::remove_dir_all(lists_dir(&db));
        assert_eq!(update(&db, &mut f).unwrap(), 1);
        assert!(available(&db).unwrap().is_empty());

        // Apps.gz changed after signing
        index_apps(&repo, 1, "Test apps").unwrap();
        sign(&repo, &apps_key);
        let gz = repo.join(format!("platform/1/{ARCH}/Apps.gz"));
        let mut bytes = std::fs::read(&gz).unwrap();
        bytes.push(0);
        std::fs::write(&gz, bytes).unwrap();
        assert_eq!(update(&db, &mut f).unwrap(), 1);

        // A system package in the pool: rpg index --apps refuses it
        put(&repo, "pool/h/hello/hello_1.0_x86_64.rpg", &rpg(&format!("Package: hello\nVersion: 1.0\nArchitecture: {ARCH}\nDescription: x\n"), &[]));
        assert!(index_apps(&repo, 1, "Test apps").unwrap_err().contains("system package"));
        std::fs::remove_dir_all(repo.join("pool/h")).unwrap();

        // ... and an index that lists one anyway (signed with the apps
        // key) is refused whole
        index_apps(&repo, 1, "Test apps").unwrap();
        let mut text = String::new();
        GzDecoder::new(&std::fs::read(&gz).unwrap()[..]).read_to_string(&mut text).unwrap();
        text.push_str(&format!("\nPackage: hello\nVersion: 1.0\nArchitecture: {ARCH}\nDescription: x\nFilename: pool/h/hello/hello_1.0_x86_64.rpg\nSize: 10\nSHA256: {}\n", "0".repeat(64)));
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut enc, text.as_bytes()).unwrap();
        let newgz = enc.finish().unwrap();
        std::fs::write(&gz, &newgz).unwrap();
        let rel_path = repo.join("platform/1/Release");
        let rel = std::fs::read_to_string(&rel_path).unwrap();
        let old_line = rel.lines().find(|l| l.ends_with(&format!(" {ARCH}/Apps.gz"))).unwrap().to_string();
        let rel = rel.replace(&old_line, &format!(" {} {} {ARCH}/Apps.gz", sha256_hex(&newgz), newgz.len()));
        std::fs::write(&rel_path, rel).unwrap();
        sign(&repo, &apps_key);
        let _ = std::fs::remove_dir_all(lists_dir(&db));
        assert_eq!(update(&db, &mut f).unwrap(), 1);
        assert!(available(&db).unwrap().is_empty());

        // An index for a newer platform than the source line's
        index_apps(&repo, 1, "Test apps").unwrap();
        let rel = std::fs::read_to_string(&rel_path).unwrap().replace("Platform: 1", "Platform: 2");
        std::fs::write(&rel_path, rel).unwrap();
        sign(&repo, &apps_key);
        assert_eq!(update(&db, &mut f).unwrap(), 1);
    }
}

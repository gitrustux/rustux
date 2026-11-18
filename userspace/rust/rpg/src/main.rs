// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rpg: Rustica's one package tool (phase E, PACKAGES.md in the Rustux
//! project; docs/PACKAGES.md; man rpg). Step E1: the `.rpg` format and
//! the package database; step E2: signed repositories (repo.rs), the
//! dependency solver (solve.rs) and their commands (cmd.rs).
//!
//!   rpg update                     the sources' package lists (signatures checked)
//!   rpg upgrade [-y]               every installed package a source has newer
//!   rpg install [-y] NAME|./FILE.rpg...   with what they need
//!   rpg search WORD... | policy NAME | depends NAME | rdepends NAME
//!   rpg hold NAME | unhold NAME | autoremove [-y] | download NAME | clean
//!   rpg source list|add|remove | key list|add|remove
//!   rpg index REPO                 the publishing side: Packages.gz and Release
//!   rpg build DIR [-o FILE]        a package from DIR/RPG/control and DIR's files
//!   rpg remove [--purge] NAME...   remove (--purge: its configuration files too)
//!   rpg list [--installed] [--json]
//!   rpg show NAME | ./FILE.rpg     the control fields (a file: and its files)
//!   rpg info ./FILE.rpg            a package file's control fields and files
//!   rpg files NAME                 what a package installed
//!   rpg owner PATH...              which package a file belongs to
//!   rpg verify [NAME...]           files changed or missing since installed
//!   rpg fix                        finish (or undo) an interrupted install
//!
//! --root DIR before the command works on a system under DIR (the
//! installer's, and the tests').

mod archive;
mod boot;
mod cmd;
mod control;
mod db;
mod fetch;
mod keys;
mod ops;
mod pkg;
mod repo;
mod solve;
mod sys;
mod version;

use control::Control;
use db::Db;
use ops::Ctx;
use pkg::{FType, Package};
use std::path::{Path, PathBuf};
use std::process::exit;

const USAGE: &str = "usage: rpg [--root DIR] COMMAND ...
  update                       get the sources' package lists (signatures checked)
  upgrade [-y]                 upgrade every package a source has a newer version of
  install [-y] NAME|./FILE.rpg...  install packages, with what they need
  remove [--purge] NAME...     remove packages (--purge: their configuration files too)
  autoremove [-y]              remove packages installed for others, no longer needed
  search WORD... [--names-only]  packages the sources have
  show NAME | ./FILE.rpg       a package's control fields
  policy [NAME...]             installed and candidate versions, and where from
  list [--installed|--upgradable|--held] [--json]
  depends NAME | rdepends NAME what a package needs, what needs it
  hold NAME... | unhold NAME...  keep a package at its version
  download NAME...             a package file into the current folder
  clean                        empty the download cache
  source list|add URL SUITE SECTION...|remove N
  key list|add FILE [NAME]|remove ID   [--apps: FoxDen's]
  files NAME                   the files a package installed
  owner PATH...                the package a file belongs to
  verify [NAME...]             files changed or missing since they were installed
  fix                          finish an install a power cut or crash interrupted
  build DIR [-o FILE]          make a package from DIR/RPG/control and DIR's files
  info ./FILE.rpg              a package file's control fields and files
  index REPO [--suite S] [--section S] [--arch A]   Packages.gz and Release for REPO/pool
  boot list | boot status | boot update   the system images, how each one started, and rboot's entries
  version                      rpg's version";

fn die(msg: &str) -> ! {
    eprintln!("rpg: {msg}");
    exit(1)
}

fn is_file_arg(a: &str) -> bool {
    a.ends_with(".rpg") || a.contains('/')
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = PathBuf::from("/");
    if args.first().map(String::as_str) == Some("--root") {
        if args.len() < 2 {
            die("--root needs a folder");
        }
        root = PathBuf::from(&args[1]);
        args.drain(..2);
        if !root.is_dir() {
            die(&format!("{}: not a folder", root.display()));
        }
        // Absolute: scripts run in it, and relative paths would then miss
        root = std::fs::canonicalize(&root).unwrap_or(root);
    }
    let Some(cmd) = args.first().cloned() else {
        eprintln!("{USAGE}");
        exit(1)
    };
    let rest = &args[1..];
    let ctx = Ctx { db: Db::new(&root) };
    if !matches!(cmd.as_str(), "fix" | "build" | "info" | "index" | "pool-path" | "version" | "--version" | "help" | "--help" | "-h") {
        if let Some(what) = ctx.interrupted() {
            if matches!(cmd.as_str(), "list" | "show" | "files" | "owner" | "verify" | "search" | "policy") {
                eprintln!("rpg: the {what} was interrupted; rpg fix finishes it (so does the next install or remove)");
            }
        }
    }
    let r = match cmd.as_str() {
        "build" => build(rest),
        "install" | "add" => {
            let (yes, items) = yes_and(rest);
            if items.is_empty() {
                die("install what? (rpg install NAME, or ./FILE.rpg)");
            }
            if let Some(o) = items.iter().find(|a| a.starts_with('-')) {
                die(&format!("install: {o}: -y is the option"));
            }
            ctx.begin().and_then(|_lock| cmd::install(&ctx, &items, yes))
        }
        "update" => ctx.begin().and_then(|_lock| {
            if !cmd::update(&ctx)? {
                exit(1);
            }
            Ok(())
        }),
        "upgrade" | "full-upgrade" | "dist-upgrade" => {
            let (yes, _) = yes_and(rest);
            ctx.begin().and_then(|_lock| cmd::upgrade(&ctx, yes))
        }
        "autoremove" => {
            let (yes, _) = yes_and(rest);
            ctx.begin().and_then(|_lock| cmd::autoremove(&ctx, yes))
        }
        "hold" | "unhold" => {
            if rest.is_empty() {
                die(&format!("{cmd} which package?"));
            }
            ctx.begin().and_then(|_lock| {
                for n in rest {
                    ctx.set_field(n, "Hold", if cmd == "hold" { Some("yes") } else { None })?;
                    println!("{n}: {}", if cmd == "hold" { "held: rpg upgrade leaves it at its version" } else { "no longer held" });
                }
                Ok(())
            })
        }
        "search" => cmd::search(&ctx, rest),
        "policy" => cmd::policy(&ctx, rest),
        "depends" => cmd::depends(&ctx, rest, false),
        "rdepends" => cmd::depends(&ctx, rest, true),
        "download" => cmd::download(&ctx, rest),
        "clean" => ctx.begin().and_then(|_lock| cmd::clean(&ctx)),
        "source" | "sources" => {
            if rest.first().is_some_and(|a| a == "add" || a == "remove") {
                ctx.begin().and_then(|_lock| cmd::source(&ctx, rest))
            } else {
                cmd::source(&ctx, rest)
            }
        }
        "key" | "keys" => {
            if rest.first().is_some_and(|a| a == "add" || a == "remove") {
                ctx.begin().and_then(|_lock| cmd::key(&ctx, rest))
            } else {
                cmd::key(&ctx, rest)
            }
        }
        "index" => cmd::index(rest),
        // The image packages' scripts run it while their install holds
        // the lock: no lock here (it only writes rboot's entries)
        "boot" => match rest.first().map(String::as_str) {
            Some("update") => {
                if !sys::is_root() && root == Path::new("/") {
                    die("writing the boot entries needs root (sudo rpg boot update)");
                }
                boot::update(&ctx.db)
            }
            None | Some("list") => boot::list(&ctx.db),
            Some("status") => boot::status(&ctx.db),
            Some(x) => Err(format!("boot {x}: update, list or status")),
        },
        "pool-path" => cmd::pool_path(rest),
        "remove" | "purge" => {
            let purge = cmd == "purge" || rest.iter().any(|a| a == "--purge");
            let names: Vec<String> = rest.iter().filter(|a| !a.starts_with('-')).cloned().collect();
            if names.is_empty() {
                die("remove what? (rpg remove NAME)");
            }
            ctx.begin().and_then(|_lock| ctx.remove(&names, purge))
        }
        "fix" => {
            if !sys::is_root() && root == Path::new("/") {
                die("finishing an install needs root (sudo rpg fix)");
            }
            db::Lock::take(&ctx.db).and_then(|_lock| {
                if !ctx.fix()? {
                    println!("rpg: nothing to finish");
                }
                Ok(())
            })
        }
        "list" => list(&ctx, rest),
        "show" => show(&ctx, rest, false),
        "info" => show(&ctx, rest, true),
        "files" => files(&ctx, rest),
        "owner" | "search-file" => owner(&ctx, rest),
        "verify" => verify(&ctx, rest),
        "version" | "--version" => {
            println!("rpg {} (step E4: boot counting, two images kept)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        c => Err(format!("{c}: not a command (rpg help)")),
    };
    if let Err(e) = r {
        die(&e);
    }
}

/// -y (or --yes) and the other arguments
fn yes_and(args: &[String]) -> (bool, Vec<String>) {
    let yes = args.iter().any(|a| a == "-y" || a == "--yes" || a == "--assume-yes");
    (yes, args.iter().filter(|a| !matches!(a.as_str(), "-y" | "--yes" | "--assume-yes")).cloned().collect())
}

fn build(args: &[String]) -> Result<(), String> {
    let mut dir = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
                i += 1;
            }
            d if dir.is_none() => dir = Some(d.to_string()),
            x => return Err(format!("build: {x}: one folder")),
        }
        i += 1;
    }
    let dir = dir.ok_or("build what? (rpg build DIR: DIR/RPG/control and the files)")?;
    let (bytes, name) = pkg::build(Path::new(&dir))?;
    let out = out.unwrap_or(name);
    std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    // Read back: what rpg install will see
    let p = Package::read(&bytes)?;
    println!(
        "{out}: {} {} ({} files, {} KiB installed, {} bytes)",
        p.control.name(),
        p.control.version(),
        p.files.iter().filter(|f| f.typ != FType::Dir).count(),
        p.control.get("Installed-Size").unwrap_or("?"),
        bytes.len()
    );
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

fn list(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    for a in args {
        if !["--installed", "--json", "--upgradable", "--held"].contains(&a.as_str()) {
            return Err(format!("list: {a}: --installed, --upgradable, --held, --json"));
        }
    }
    if args.iter().any(|a| a == "--upgradable") {
        return cmd::list_upgradable(ctx);
    }
    let mut pk = ctx.db.installed()?;
    if args.iter().any(|a| a == "--held") {
        pk.retain(|c| c.get("Hold") == Some("yes"));
    }
    pk.sort_by(|a, b| a.name().cmp(b.name()));
    if args.iter().any(|a| a == "--json") {
        let items: Vec<String> = pk
            .iter()
            .map(|c| {
                format!(
                    "{{\"name\":{},\"version\":{},\"architecture\":{},\"summary\":{}}}",
                    json_str(c.name()),
                    json_str(c.version()),
                    json_str(c.arch()),
                    json_str(c.summary())
                )
            })
            .collect();
        println!("[{}]", items.join(","));
        return Ok(());
    }
    if pk.is_empty() {
        println!("No packages are installed.");
        return Ok(());
    }
    let w0 = pk.iter().map(|c| c.name().len()).max().unwrap_or(4).max(4);
    let w1 = pk.iter().map(|c| c.version().len()).max().unwrap_or(7).max(7);
    for c in &pk {
        println!("{:w0$}  {:w1$}  {:6}  {}", c.name(), c.version(), c.arch(), c.summary());
    }
    Ok(())
}

fn print_control(c: &Control) {
    print!("{}", c.text());
}

fn show(ctx: &Ctx, args: &[String], file_only: bool) -> Result<(), String> {
    let [what] = args else { return Err("show one package (a name, or ./FILE.rpg)".into()) };
    if file_only || is_file_arg(what) {
        let bytes = std::fs::read(what).map_err(|e| format!("{what}: {e}"))?;
        let p = Package::read(&bytes).map_err(|e| format!("{what}: {e}"))?;
        print_control(&p.control);
        let scripts: Vec<&str> = [("postinst", &p.postinst), ("prerm", &p.prerm), ("postrm", &p.postrm)]
            .iter()
            .filter(|(_, s)| s.is_some())
            .map(|(n, _)| *n)
            .collect();
        println!("Scripts: {}", if scripts.is_empty() { "none".to_string() } else { scripts.join(", ") });
        println!("Files:");
        for f in &p.files {
            let extra = match f.typ {
                FType::Link => format!(" -> {}", f.hash),
                FType::Dir => "/".into(),
                FType::File if f.conf => "  (configuration)".into(),
                FType::File => String::new(),
            };
            println!(" {:04o} {}{}", f.mode, f.path, extra);
        }
        return Ok(());
    }
    match ctx.db.entry(what)? {
        Some(c) => {
            print_control(&c);
            Ok(())
        }
        None if cmd::show_available(ctx, what)? => Ok(()),
        None => Err(format!("{what}: not installed, and no source has it (rpg search; rpg update)")),
    }
}

fn files(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let [name] = args else { return Err("files of one package (rpg files NAME)".into()) };
    if ctx.db.entry(name)?.is_none() {
        return Err(format!("{name} is not installed"));
    }
    for f in ctx.db.files_of(name)? {
        println!("{}", f.path);
    }
    Ok(())
}

/// An absolute, clean path (no symbolic links followed)
fn absolute(p: &str) -> String {
    let full = if p.starts_with('/') { p.to_string() } else { format!("{}/{p}", std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default()) };
    let mut parts: Vec<&str> = Vec::new();
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

fn owner(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("owner of which file? (rpg owner PATH)".into());
    }
    let owners = ctx.db.owners()?;
    let mut missing = false;
    for a in args {
        let p = absolute(a);
        match owners.get(&p) {
            Some(o) => println!("{}: {p}", o.join(", ")),
            None => {
                eprintln!("rpg: {p}: no package has it");
                missing = true;
            }
        }
    }
    if missing {
        exit(1);
    }
    Ok(())
}

fn verify(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let installed = ctx.db.installed()?;
    let names: Vec<String> = if args.is_empty() { installed.iter().map(|c| c.name().to_string()).collect() } else { args.to_vec() };
    let mut bad = false;
    for n in &names {
        if !installed.iter().any(|c| c.name() == n) {
            return Err(format!("{n} is not installed"));
        }
        let files = ctx.db.files_of(n)?;
        let mut problems = 0;
        for f in &files {
            let p = ctx.db.path(&f.path);
            let state = match f.typ {
                FType::File => match std::fs::read(&p) {
                    Ok(d) if pkg::sha256_hex(&d) == f.hash => None,
                    Ok(_) => Some("changed"),
                    Err(_) => Some("missing"),
                },
                FType::Link => match std::fs::read_link(&p) {
                    Ok(t) if t.to_string_lossy() == f.hash => None,
                    Ok(_) => Some("changed"),
                    Err(_) => Some("missing"),
                },
                FType::Dir => match p.symlink_metadata() {
                    Ok(m) if m.is_dir() => None,
                    Ok(_) => Some("changed"),
                    Err(_) => Some("missing"),
                },
            };
            if let Some(s) = state {
                println!("{s:8} {}{}", f.path, if f.conf { "  (configuration file)" } else { "" });
                problems += 1;
                if !f.conf {
                    bad = true;
                }
            }
        }
        if problems == 0 {
            println!("{n}: {} files as installed", files.iter().filter(|f| f.typ != FType::Dir).count());
        }
    }
    if bad {
        exit(1);
    }
    Ok(())
}

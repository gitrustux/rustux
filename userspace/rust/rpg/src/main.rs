// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rpg: Rustica's one package tool (phase E, PACKAGES.md in the Rustux
//! project; docs/PACKAGES.md; man rpg). Step E1: the `.rpg` format and
//! the package database; repositories (`rpg update`, `rpg upgrade`,
//! `rpg install NAME`) come with step E2.
//!
//!   rpg build DIR [-o FILE]        a package from DIR/RPG/control and DIR's files
//!   rpg install ./FILE.rpg...      install or upgrade from package files
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
mod control;
mod db;
mod ops;
mod pkg;
mod sys;
mod version;

use control::Control;
use db::Db;
use ops::Ctx;
use pkg::{FType, Package};
use std::path::{Path, PathBuf};
use std::process::exit;

const USAGE: &str = "usage: rpg [--root DIR] COMMAND ...
  build DIR [-o FILE]          make a package from DIR/RPG/control and DIR's files
  install ./FILE.rpg...        install or upgrade from package files
  remove [--purge] NAME...     remove packages (--purge: their configuration files too)
  list [--installed] [--json]  the installed packages
  show NAME | ./FILE.rpg       a package's control fields
  info ./FILE.rpg              a package file's control fields and files
  files NAME                   the files a package installed
  owner PATH...                the package a file belongs to
  verify [NAME...]             files changed or missing since they were installed
  fix                          finish an install a power cut or crash interrupted
  version                      rpg's version
Repositories (rpg update, rpg upgrade, rpg install NAME) come with step E2.";

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
    }
    let Some(cmd) = args.first().cloned() else {
        eprintln!("{USAGE}");
        exit(1)
    };
    let rest = &args[1..];
    let ctx = Ctx { db: Db::new(&root) };
    if !matches!(cmd.as_str(), "fix" | "build" | "info" | "version" | "--version" | "help" | "--help" | "-h") {
        if let Some(what) = ctx.interrupted() {
            if matches!(cmd.as_str(), "list" | "show" | "files" | "owner" | "verify") {
                eprintln!("rpg: the {what} was interrupted; rpg fix finishes it (so does the next install or remove)");
            }
        }
    }
    let r = match cmd.as_str() {
        "build" => build(rest),
        "install" | "add" => {
            if rest.is_empty() {
                die("install what? (rpg install ./FILE.rpg)");
            }
            for a in rest {
                if a.starts_with('-') {
                    die(&format!("install: {a}: no options yet"));
                }
                if !is_file_arg(a) {
                    die(&format!(
                        "{a}: installing from repositories comes with step E2; give a package file (rpg install ./{a}_VERSION_ARCH.rpg)"
                    ));
                }
            }
            ctx.begin().and_then(|_lock| ctx.install(rest))
        }
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
            println!("rpg {} (step E1: package files and the database)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        "update" | "upgrade" | "search" | "policy" | "hold" | "unhold" | "autoremove" | "clean" | "source" | "key" | "index" => {
            Err(format!("{cmd}: repositories come with step E2 (PACKAGES.md)"))
        }
        c => Err(format!("{c}: not a command (rpg help)")),
    };
    if let Err(e) = r {
        die(&e);
    }
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
        if a != "--installed" && a != "--json" {
            return Err(format!("list: {a}: --installed and --json (--upgradable comes with step E2)"));
        }
    }
    let mut pk = ctx.db.installed()?;
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
        println!("Scripts: {}", match (&p.postinst, &p.prerm) {
            (Some(_), Some(_)) => "postinst, prerm",
            (Some(_), None) => "postinst",
            (None, Some(_)) => "prerm",
            (None, None) => "none",
        });
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
        None => Err(format!("{what} is not installed")),
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

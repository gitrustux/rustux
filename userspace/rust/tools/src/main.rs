// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux commands in one multi-call program (checklist step 11). The name
//! it is started by picks the command (`/bin/grep` is this program under
//! another name, see `userspace/links`); `tools NAME ARGS...` works too.

mod archive;
mod grep;
mod less;
mod man;
mod util;

use std::ffi::OsString;

const COMMANDS: &[&str] = &[
    "grep", "egrep", "fgrep", "less", "more", "man", "edit", "nano", "tar", "gzip", "gunzip", "zcat", "find",
    "xargs", "diff", "cmp",
];

fn main() {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    let mut name = args
        .first()
        .map(|a| util::base_name(&a.to_string_lossy()).to_string())
        .unwrap_or_default();
    if name == "tools" {
        if args.len() < 2 || args[1] == "--help" || args[1] == "--list" {
            println!("tools: Rustux commands in one program. Run as one of them:");
            println!("  {}", COMMANDS.join(" "));
            std::process::exit(if args.len() < 2 { 1 } else { 0 });
        }
        args.remove(0);
        name = args[0].to_string_lossy().into_owned();
    }
    let code = match name.as_str() {
        "grep" | "egrep" | "fgrep" => grep::main(&name, args),
        "less" | "more" => less::main(&name, args),
        "man" => man::main(args),
        "edit" | "nano" => edit(&name, args),
        "tar" => archive::tar_main(args),
        "gzip" | "gunzip" | "zcat" => archive::gzip_main(&name, args),
        "find" | "xargs" => {
            let strs: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            let refs: Vec<&str> = strs.iter().map(|s| s.as_str()).collect();
            if name == "find" {
                findutils::find::find_main(&refs, &findutils::find::StandardDependencies::new())
            } else {
                findutils::xargs::xargs_main(&refs)
            }
        }
        "diff" | "cmp" => {
            // diff and cmp take the arguments with their own name first
            let mut it = std::env::args_os().peekable();
            if args.len() != std::env::args_os().count() {
                it.next(); // "tools diff ...": skip "tools"
            }
            let r = if name == "diff" { diffutilslib::diff::main(it) } else { diffutilslib::cmp::main(it) };
            if r == std::process::ExitCode::SUCCESS { 0 } else { exit_code_of(r) }
        }
        _ => {
            eprintln!("tools: unknown command {name}");
            127
        }
    };
    util::flush();
    std::process::exit(code);
}

// ExitCode doesn't say its number except in its Debug form; diff and cmp
// use 1 (different) and 2 (trouble)
fn exit_code_of(c: std::process::ExitCode) -> i32 {
    let s = format!("{c:?}");
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(2)
}

/// edit, nano: the kibi editor
fn edit(name: &str, args: Vec<OsString>) -> i32 {
    let a: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if a.iter().any(|x| x == "--help" || x == "-h") {
        println!("Usage: {name} [FILE]\n\nA small text editor (kibi).\n");
        println!("  Ctrl+S save      Ctrl+Q quit (press 3 times to drop unsaved changes)");
        println!("  Ctrl+F find      Ctrl+G go to line     Ctrl+D duplicate line");
        println!("  Ctrl+E run a command and insert its output");
        println!("  Ctrl+R remove the line   Ctrl+C/Ctrl+X/Ctrl+V copy/cut/paste the line");
        return 0;
    }
    if a.len() > 1 {
        eprintln!("{name}: one file at a time");
        return 2;
    }
    let file = a.first().map(|s| s.as_str());
    let result = kibi::stdin().map_err(kibi::Error::from).and_then(|mut input| kibi::run(file, &mut input));
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{name}: {e:?}");
            1
        }
    }
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! man: show a manual page from /usr/share/man (plain text, NAME.txt),
//! through less on the console. For a command without a page, its --help.
//! `man -k WORD` lists pages whose first line mentions WORD.

use std::ffi::OsString;
use std::io::Write;
use std::process::{Command, Stdio};

const DIR: &str = "/usr/share/man";

pub fn main(args: Vec<OsString>) -> i32 {
    let args: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match args.first().map(|s| s.as_str()) {
        None => {
            eprintln!("What manual page do you want? (man intro, man -k WORD, man --help)");
            1
        }
        Some("--help") | Some("-h") => {
            println!("Usage: man PAGE\n       man -k WORD\n\nShow the manual page PAGE (from {DIR}); for a command without");
            println!("a page, its --help. -k lists the pages that mention WORD in their summary.");
            println!("Start with: man intro");
            0
        }
        Some("-k") | Some("--apropos") => apropos(args.get(1).map(|s| s.as_str()).unwrap_or("")),
        Some(page) => show(page),
    }
}

fn apropos(word: &str) -> i32 {
    let word = word.to_lowercase();
    let mut names: Vec<String> = std::fs::read_dir(DIR)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    let mut found = 0;
    for n in names {
        let Some(page) = n.strip_suffix(".txt") else { continue };
        let summary = std::fs::read_to_string(format!("{DIR}/{n}"))
            .ok()
            .and_then(|s| s.lines().find(|l| l.contains(" - ")).map(|l| l.trim().to_string()))
            .unwrap_or_default();
        if word.is_empty() || summary.to_lowercase().contains(&word) || page.contains(&word) {
            println!("{summary}");
            found += 1;
        }
    }
    if found == 0 {
        println!("{word}: nothing appropriate.");
        1
    } else {
        0
    }
}

fn show(page: &str) -> i32 {
    if page.contains('/') {
        eprintln!("man: no manual page for {page}");
        return 1;
    }
    let text = match std::fs::read_to_string(format!("{DIR}/{page}.txt")) {
        Ok(t) => t,
        Err(_) => {
            // A command's own help
            let out = Command::new(page).arg("--help").stdin(Stdio::null()).output();
            match out {
                Ok(o) if !o.stdout.is_empty() => {
                    format!("{} (no manual page; this is `{page} --help`)\n\n{}", page.to_uppercase(), String::from_utf8_lossy(&o.stdout))
                }
                _ => {
                    eprintln!("man: no manual page for {page} (see man -k WORD)");
                    return 1;
                }
            }
        }
    };
    // Through the pager when on the console
    if crate::util::is_tty(1) {
        if let Ok(mut child) = Command::new("/bin/less").arg("-F").stdin(Stdio::piped()).spawn() {
            if let Some(mut input) = child.stdin.take() {
                let _ = input.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
        }
    }
    print!("{text}");
    0
}

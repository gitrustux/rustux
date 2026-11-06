// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! less, more: page through text on the console.
//!
//! The text (files, or standard input) is read into memory. Keys come
//! from the console even when the text comes through a pipe: from standard
//! error, which is the console then. When standard output is not the
//! console, the text is just copied (like cat).

use crate::util;
use regex::RegexBuilder;
use std::ffi::OsString;
use std::io::{self, Read, Write};

const HELP: &str = "Usage: less [OPTION]... [FILE]...
Page through FILEs (or standard input).

  -N, --LINE-NUMBERS   show line numbers
  -S, --chop-long-lines  cut long lines instead of wrapping them
  -i, --ignore-case    searches ignore case
  -F, --quit-if-one-screen  just print text that fits on one screen
  -R, --RAW-CONTROL-CHARS  pass colour escape sequences through

Keys:
  Space, f, PgDn       next page          b, PgUp     previous page
  Enter, j, Down       next line          k, Up       previous line
  d / u                half a page down / up
  g, <, Home           first line         G, >, End   last line
  /PATTERN             search forward     ?PATTERN    search backward
  n / N                next / previous match
  :n / :p              next / previous file
  h                    this help          q           quit";

struct Doc {
    name: String,
    lines: Vec<String>,
}

pub fn main(name: &str, args: Vec<OsString>) -> i32 {
    let args: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let mut numbers = false;
    let mut chop = false;
    let mut icase = false;
    let mut quit_if_one = name == "more";
    let mut raw = false;
    let mut files = Vec::new();
    for a in &args {
        match a.as_str() {
            "--help" => {
                println!("{HELP}");
                return 0;
            }
            "-N" | "--LINE-NUMBERS" | "--line-numbers" => numbers = true,
            "-S" | "--chop-long-lines" => chop = true,
            "-i" | "-I" | "--ignore-case" => icase = true,
            "-F" | "--quit-if-one-screen" => quit_if_one = true,
            "-R" | "-r" | "--RAW-CONTROL-CHARS" => raw = true,
            "-X" | "-e" | "-E" | "-q" | "-s" => {}
            s if s.starts_with('-') && s != "-" => {
                // grouped flags
                for c in s[1..].chars() {
                    match c {
                        'N' => numbers = true,
                        'S' => chop = true,
                        'i' | 'I' => icase = true,
                        'F' => quit_if_one = true,
                        'R' | 'r' => raw = true,
                        'X' | 'e' | 'E' | 'q' | 's' => {}
                        _ => {
                            eprintln!("{name}: unknown option -{c} (see {name} --help)");
                            return 2;
                        }
                    }
                }
            }
            _ => files.push(a.clone()),
        }
    }
    let mut docs = Vec::new();
    let mut status = 0;
    if files.is_empty() {
        files.push("-".into());
    }
    for f in &files {
        let mut text = Vec::new();
        let r = if f == "-" {
            io::stdin().read_to_end(&mut text)
        } else {
            std::fs::File::open(f).and_then(|mut h| h.read_to_end(&mut text))
        };
        match r {
            Ok(_) => {
                let s = String::from_utf8_lossy(&text).replace('\t', "        ");
                let lines: Vec<String> = s.lines().map(|l| l.to_string()).collect();
                docs.push(Doc { name: if f == "-" { "(standard input)".into() } else { f.clone() }, lines });
            }
            Err(e) => {
                eprintln!("{name}: {f}: {}", e.to_string().split(" (os").next().unwrap_or(""));
                status = 1;
            }
        }
    }
    if docs.is_empty() {
        return status;
    }
    // Not on the console: copy
    let key_fd = if util::is_tty(0) && files.iter().all(|f| f != "-") { 0 } else { 2 };
    if !util::is_tty(1) || !util::is_tty(key_fd) {
        let mut out = io::stdout().lock();
        for d in &docs {
            for l in &d.lines {
                let _ = writeln!(out, "{l}");
            }
        }
        return status;
    }
    let (rows, cols) = util::term_size(1);
    let mut p = Pager { docs, doc: 0, top: 0, rows, cols, numbers, chop, icase, raw, pattern: None, message: String::new(), key_fd };
    if quit_if_one && p.docs.len() == 1 && p.screen_lines(0).len() < rows {
        let mut out = io::stdout().lock();
        for l in &p.docs[0].lines {
            let _ = writeln!(out, "{l}");
        }
        return status;
    }
    let Some(_raw) = util::RawMode::new(key_fd) else {
        eprintln!("{name}: the console has no raw mode");
        return 1;
    };
    print!("\x1b[?1049h\x1b[?25l");
    p.run();
    print!("\x1b[0m\x1b[?25h\x1b[?1049l");
    util::flush();
    status
}

struct Pager {
    docs: Vec<Doc>,
    doc: usize,
    top: usize, // first screen row (in wrapped rows)
    rows: usize,
    cols: usize,
    numbers: bool,
    chop: bool,
    icase: bool,
    raw: bool,
    pattern: Option<(regex::Regex, bool)>,
    message: String,
    key_fd: i32,
}

/// Visible width of text with colour sequences
fn width(s: &str) -> usize {
    let mut w = 0;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c.is_ascii_alphabetic() {
                esc = false;
            }
        } else if c == '\x1b' {
            esc = true;
        } else {
            w += 1;
        }
    }
    w
}

impl Pager {
    /// The current document as screen rows: (line index, text)
    fn screen_lines(&self, doc: usize) -> Vec<(usize, String)> {
        let d = &self.docs[doc];
        let gutter = if self.numbers { 8 } else { 0 };
        let w = self.cols.saturating_sub(gutter).max(1);
        let mut out = Vec::new();
        for (i, l) in d.lines.iter().enumerate() {
            let l: String = if self.raw { l.clone() } else { l.chars().map(|c| if (c as u32) < 0x20 { '?' } else { c }).collect() };
            if self.chop || width(&l) <= w {
                let cut: String = if width(&l) > w { l.chars().take(w).collect() } else { l };
                out.push((i, cut));
            } else {
                let chars: Vec<char> = l.chars().collect();
                for chunk in chars.chunks(w) {
                    out.push((i, chunk.iter().collect()));
                }
            }
        }
        out
    }

    fn key(&self) -> Option<u8> {
        let mut b = [0u8; 1];
        match unsafe { libc::read(self.key_fd, b.as_mut_ptr() as *mut libc::c_void, 1) } {
            1 => Some(b[0]),
            _ => None,
        }
    }

    fn prompt(&self, lead: &str) -> Option<String> {
        let mut s = String::new();
        loop {
            print!("\x1b[{};1H\x1b[K{lead}{s}", self.rows);
            util::flush();
            match self.key()? {
                b'\r' | b'\n' => return Some(s),
                0x1b | 3 => return None,
                0x7f | 8 => {
                    if s.pop().is_none() {
                        return None;
                    }
                }
                c if c >= 0x20 => s.push(c as char),
                _ => {}
            }
        }
    }

    fn draw(&mut self, lines: &[(usize, String)]) {
        let page = self.rows - 1;
        let mut out = String::new();
        out.push_str("\x1b[H");
        let mut last = usize::MAX;
        for r in 0..page {
            out.push_str("\x1b[K");
            if let Some((i, text)) = lines.get(self.top + r) {
                if self.numbers {
                    if *i != last {
                        out.push_str(&format!("\x1b[33m{:>7}\x1b[0m ", i + 1));
                    } else {
                        out.push_str("        ");
                    }
                }
                last = *i;
                match &self.pattern {
                    Some((re, _)) if !self.raw => {
                        let mut at = 0;
                        for m in re.find_iter(text).filter(|m| m.end() > m.start()) {
                            out.push_str(&text[at..m.start()]);
                            out.push_str("\x1b[7m");
                            out.push_str(m.as_str());
                            out.push_str("\x1b[0m");
                            at = m.end();
                        }
                        out.push_str(&text[at..]);
                    }
                    _ => out.push_str(text),
                }
                out.push_str("\x1b[0m");
            } else {
                out.push_str("\x1b[34m~\x1b[0m");
            }
            out.push_str("\r\n");
        }
        let total = lines.len();
        let shown_to = (self.top + page).min(total);
        let pct = if total == 0 { 100 } else { shown_to * 100 / total };
        let d = &self.docs[self.doc];
        let mut status = if self.message.is_empty() {
            let file = if self.docs.len() > 1 { format!(" (file {} of {})", self.doc + 1, self.docs.len()) } else { String::new() };
            let at_end = if shown_to >= total { "(END)" } else { "" };
            format!("{}{file}  lines {}-{} of {}  {pct}% {at_end}  h help, q quit", d.name, lines.get(self.top).map_or(0, |l| l.0 + 1), lines.get(shown_to.saturating_sub(1)).map_or(0, |l| l.0 + 1), d.lines.len())
        } else {
            std::mem::take(&mut self.message)
        };
        if width(&status) >= self.cols {
            status = status.chars().take(self.cols - 1).collect();
        }
        out.push_str(&format!("\x1b[K\x1b[7m{status}\x1b[0m"));
        print!("{out}");
        util::flush();
    }

    fn search(&mut self, lines: &[(usize, String)], forward: bool, from_next: bool) {
        let Some((re, _)) = &self.pattern else { return };
        let n = lines.len();
        let start = self.top;
        let found = if forward {
            (start + from_next as usize..n).find(|&r| re.is_match(&lines[r].1))
        } else {
            (0..start.saturating_sub(from_next as usize).min(n)).rev().find(|&r| re.is_match(&lines[r].1))
        };
        match found {
            Some(r) => self.top = r,
            None => self.message = "Pattern not found".into(),
        }
    }

    fn run(&mut self) {
        let mut lines = self.screen_lines(self.doc);
        loop {
            let page = self.rows - 1;
            let max_top = lines.len().saturating_sub(page);
            self.top = self.top.min(max_top);
            self.draw(&lines);
            let Some(k) = self.key() else { return };
            match k {
                b'q' | b'Q' | 3 => return,
                b' ' | b'f' | 6 => self.top += page,
                b'b' | 2 => self.top = self.top.saturating_sub(page),
                b'\r' | b'\n' | b'j' | b'e' | 14 => self.top += 1,
                b'k' | b'y' | 16 => self.top = self.top.saturating_sub(1),
                b'd' | 4 => self.top += page / 2,
                b'u' | 21 => self.top = self.top.saturating_sub(page / 2),
                b'g' | b'<' => self.top = 0,
                b'G' | b'>' => self.top = max_top,
                b'h' | b'H' => {
                    let saved = std::mem::replace(
                        &mut self.docs,
                        vec![Doc { name: "help".into(), lines: HELP.lines().map(|l| l.to_string()).collect() }],
                    );
                    let (doc, top) = (self.doc, self.top);
                    self.doc = 0;
                    self.top = 0;
                    self.run();
                    self.docs = saved;
                    self.doc = doc;
                    self.top = top;
                    lines = self.screen_lines(self.doc);
                }
                b'/' | b'?' => {
                    let forward = k == b'/';
                    if let Some(p) = self.prompt(if forward { "/" } else { "?" }) {
                        if !p.is_empty() {
                            match RegexBuilder::new(&p).case_insensitive(self.icase).build() {
                                Ok(re) => self.pattern = Some((re, forward)),
                                Err(_) => {
                                    self.message = "Bad pattern".into();
                                    continue;
                                }
                            }
                        }
                        self.search(&lines, forward, true);
                    }
                }
                b'n' | b'N' => {
                    let dir = self.pattern.as_ref().map_or(true, |p| p.1);
                    self.search(&lines, if k == b'n' { dir } else { !dir }, true);
                }
                b':' => match self.key() {
                    Some(b'n') if self.doc + 1 < self.docs.len() => {
                        self.doc += 1;
                        self.top = 0;
                        lines = self.screen_lines(self.doc);
                    }
                    Some(b'p') if self.doc > 0 => {
                        self.doc -= 1;
                        self.top = 0;
                        lines = self.screen_lines(self.doc);
                    }
                    Some(b'q') => return,
                    _ => {}
                },
                0x1b => {
                    // Escape sequences: arrows, PgUp/PgDn, Home/End
                    let (Some(b'[') | Some(b'O')) = self.key() else { continue };
                    let mut seq = Vec::new();
                    while let Some(c) = self.key() {
                        seq.push(c);
                        if c.is_ascii_alphabetic() || c == b'~' {
                            break;
                        }
                    }
                    match seq.as_slice() {
                        b"A" => self.top = self.top.saturating_sub(1),
                        b"B" => self.top += 1,
                        b"5~" => self.top = self.top.saturating_sub(page),
                        b"6~" => self.top += page,
                        b"H" | b"1~" | b"7~" => self.top = 0,
                        b"F" | b"4~" | b"8~" => self.top = max_top,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

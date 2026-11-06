// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! grep: print lines that match patterns, on the `regex` crate.
//!
//! Patterns are basic regular expressions by default (-G, as GNU grep:
//! `\(`, `\|`, `\{`, `\+`, `\?` are the operators), extended with -E
//! (egrep), fixed strings with -F (fgrep). `--json` prints one JSON object
//! per match (apps/CHECKLIST.md).

use crate::util;
use regex::bytes::{Regex, RegexBuilder};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};

#[derive(PartialEq, Clone, Copy)]
enum Syntax {
    Basic,
    Extended,
    Fixed,
}

#[derive(PartialEq, Clone, Copy)]
enum Binary {
    Report,
    Text,
    Skip,
}

struct Opts {
    syntax: Syntax,
    patterns: Vec<String>,
    ignore_case: bool,
    invert: bool,
    count: bool,
    files_with: bool,
    files_without: bool,
    line_number: bool,
    with_name: Option<bool>,
    only: bool,
    quiet: bool,
    no_messages: bool,
    max: Option<u64>,
    after: usize,
    before: usize,
    recursive: bool,
    line_regex: bool,
    word: bool,
    color: bool,
    json: bool,
    binary: Binary,
    include: Vec<String>,
    exclude: Vec<String>,
    exclude_dir: Vec<String>,
    byte_offset: bool,
}

const HELP: &str = "Usage: grep [OPTION]... PATTERNS [FILE]...
Search for PATTERNS in each FILE (standard input without FILE or with -).

Patterns:
  -E, --extended-regexp     extended regular expressions (egrep)
  -F, --fixed-strings       fixed strings (fgrep)
  -G, --basic-regexp        basic regular expressions (the default)
  -e, --regexp=PATTERN      a pattern (can be given more than once)
  -f, --file=FILE           patterns from FILE, one per line
  -i, --ignore-case         ignore case
  -w, --word-regexp         match whole words
  -x, --line-regexp         match whole lines
Output:
  -v, --invert-match        select lines that don't match
  -c, --count               print only a count of matching lines per file
  -l, --files-with-matches  print only the names of files with matches
  -L, --files-without-match print only the names of files without
  -n, --line-number         print line numbers
  -b, --byte-offset         print the byte offset of each line
  -H, --with-filename       print file names (the default for several files)
  -h, --no-filename         don't print file names
  -o, --only-matching       print only the matched parts
  -q, --quiet               print nothing; the exit status says whether something matched
  -s, --no-messages         no messages about unreadable files
  -m, --max-count=NUM       stop after NUM matching lines
  -A, -B, -C NUM            lines of context after, before, around matches
      --color[=WHEN]        highlight matches (always, never, auto)
      --json                one JSON object per matching line
Files:
  -r, -R, --recursive       search directories
      --include=GLOB        only files whose names match GLOB
      --exclude=GLOB        skip files whose names match GLOB
      --exclude-dir=GLOB    skip directories whose names match GLOB
  -a, --text                treat binary files as text
      --binary-files=TYPE   binary (report a match), text, without-match

Exit status: 0 if a line was selected, 1 if none, 2 on trouble.";

pub fn main(name: &str, args: Vec<OsString>) -> i32 {
    let args: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let mut o = Opts {
        syntax: match name {
            "egrep" => Syntax::Extended,
            "fgrep" => Syntax::Fixed,
            _ => Syntax::Basic,
        },
        patterns: Vec::new(),
        ignore_case: false,
        invert: false,
        count: false,
        files_with: false,
        files_without: false,
        line_number: false,
        with_name: None,
        only: false,
        quiet: false,
        no_messages: false,
        max: None,
        after: 0,
        before: 0,
        recursive: false,
        line_regex: false,
        word: false,
        color: false,
        json: false,
        binary: Binary::Report,
        include: Vec::new(),
        exclude: Vec::new(),
        exclude_dir: Vec::new(),
        byte_offset: false,
    };
    let mut have_pattern = false;
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    let mut color_auto = false;
    // A value for an option: attached or the next argument
    macro_rules! value {
        ($attached:expr, $opt:expr) => {{
            match $attached {
                Some(v) => v,
                None => {
                    i += 1;
                    match args.get(i) {
                        Some(v) => v.clone(),
                        None => {
                            eprintln!("grep: option {} needs a value", $opt);
                            return 2;
                        }
                    }
                }
            }
        }};
    }
    let mut no_more_options = false;
    while i < args.len() {
        let a = args[i].clone();
        if no_more_options || a == "-" || !a.starts_with('-') {
            if !have_pattern {
                o.patterns.push(a);
                have_pattern = true;
            } else {
                files.push(a);
            }
            i += 1;
            continue;
        }
        if a == "--" {
            no_more_options = true;
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (key, val) = match long.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (long, None),
            };
            match key {
                "help" => {
                    println!("{HELP}");
                    return 0;
                }
                "version" => {
                    println!("grep (Rustux tools) 0.1");
                    return 0;
                }
                "extended-regexp" => o.syntax = Syntax::Extended,
                "fixed-strings" => o.syntax = Syntax::Fixed,
                "basic-regexp" => o.syntax = Syntax::Basic,
                "regexp" => {
                    o.patterns.push(value!(val, "--regexp"));
                    have_pattern = true;
                }
                "file" => {
                    let f = value!(val, "--file");
                    if !read_patterns(&f, &mut o.patterns) {
                        return 2;
                    }
                    have_pattern = true;
                }
                "ignore-case" => o.ignore_case = true,
                "no-ignore-case" => o.ignore_case = false,
                "word-regexp" => o.word = true,
                "line-regexp" => o.line_regex = true,
                "invert-match" => o.invert = true,
                "count" => o.count = true,
                "files-with-matches" => o.files_with = true,
                "files-without-match" => o.files_without = true,
                "line-number" => o.line_number = true,
                "byte-offset" => o.byte_offset = true,
                "with-filename" => o.with_name = Some(true),
                "no-filename" => o.with_name = Some(false),
                "only-matching" => o.only = true,
                "quiet" | "silent" => o.quiet = true,
                "no-messages" => o.no_messages = true,
                "max-count" => o.max = value!(val, "--max-count").parse().ok(),
                "after-context" => o.after = value!(val, "--after-context").parse().unwrap_or(0),
                "before-context" => o.before = value!(val, "--before-context").parse().unwrap_or(0),
                "context" => {
                    let n = value!(val, "--context").parse().unwrap_or(0);
                    o.after = n;
                    o.before = n;
                }
                "recursive" | "dereference-recursive" => o.recursive = true,
                "color" | "colour" => match val.as_deref() {
                    None | Some("auto") => color_auto = true,
                    Some("always") => o.color = true,
                    _ => o.color = false,
                },
                "json" => o.json = true,
                "text" => o.binary = Binary::Text,
                "binary-files" => {
                    o.binary = match value!(val, "--binary-files").as_str() {
                        "text" => Binary::Text,
                        "without-match" => Binary::Skip,
                        _ => Binary::Report,
                    }
                }
                "include" => o.include.push(value!(val, "--include")),
                "exclude" => o.exclude.push(value!(val, "--exclude")),
                "exclude-dir" => o.exclude_dir.push(value!(val, "--exclude-dir")),
                _ => {
                    eprintln!("grep: unknown option --{key} (see grep --help)");
                    return 2;
                }
            }
            i += 1;
            continue;
        }
        // Short options, possibly grouped (-inr) or with a value (-A3, -e pat)
        let chars: Vec<char> = a[1..].chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            let rest: String = chars[k + 1..].iter().collect();
            let attached = if rest.is_empty() { None } else { Some(rest) };
            match c {
                'E' => o.syntax = Syntax::Extended,
                'F' => o.syntax = Syntax::Fixed,
                'G' => o.syntax = Syntax::Basic,
                'i' | 'y' => o.ignore_case = true,
                'w' => o.word = true,
                'x' => o.line_regex = true,
                'v' => o.invert = true,
                'c' => o.count = true,
                'l' => o.files_with = true,
                'L' => o.files_without = true,
                'n' => o.line_number = true,
                'b' => o.byte_offset = true,
                'H' => o.with_name = Some(true),
                'h' => o.with_name = Some(false),
                'o' => o.only = true,
                'q' => o.quiet = true,
                's' => o.no_messages = true,
                'r' | 'R' => o.recursive = true,
                'a' => o.binary = Binary::Text,
                'I' => o.binary = Binary::Skip,
                'e' | 'f' | 'm' | 'A' | 'B' | 'C' => {
                    let v = value!(attached, format!("-{c}"));
                    match c {
                        'e' => {
                            o.patterns.push(v);
                            have_pattern = true;
                        }
                        'f' => {
                            if !read_patterns(&v, &mut o.patterns) {
                                return 2;
                            }
                            have_pattern = true;
                        }
                        'm' => o.max = v.parse().ok(),
                        'A' => o.after = v.parse().unwrap_or(0),
                        'B' => o.before = v.parse().unwrap_or(0),
                        _ => {
                            let n = v.parse().unwrap_or(0);
                            o.after = n;
                            o.before = n;
                        }
                    }
                    break; // the value used the rest of this argument
                }
                d if d.is_ascii_digit() => {
                    // -3 is -C 3
                    let digits: String = chars[k..].iter().take_while(|c| c.is_ascii_digit()).collect();
                    let n = digits.parse().unwrap_or(0);
                    o.after = n;
                    o.before = n;
                    k += digits.len();
                    continue;
                }
                _ => {
                    eprintln!("grep: unknown option -{c} (see grep --help)");
                    return 2;
                }
            }
            k += 1;
        }
        i += 1;
    }
    if !have_pattern {
        eprintln!("Usage: grep [OPTION]... PATTERNS [FILE]...\nTry 'grep --help' for more.");
        return 2;
    }
    if color_auto {
        o.color = util::is_tty(1);
    }
    let re = match build_regex(&o) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("grep: {e}");
            return 2;
        }
    };
    if files.is_empty() {
        files.push(if o.recursive { ".".into() } else { "-".into() });
    }
    let show_name = o.with_name.unwrap_or(files.len() > 1 || o.recursive);
    let mut g = Grep { o: &o, re, show_name, out: io::BufWriter::new(io::stdout().lock()), matched: false, trouble: false };
    for f in &files {
        g.path(f, true);
        if g.matched && o.quiet {
            break;
        }
    }
    let _ = g.out.flush();
    if g.matched && (o.quiet || !g.trouble) {
        0
    } else if g.trouble {
        2
    } else {
        1
    }
}

fn read_patterns(file: &str, out: &mut Vec<String>) -> bool {
    match std::fs::read_to_string(file) {
        Ok(s) => {
            out.extend(s.lines().map(|l| l.to_string()));
            true
        }
        Err(e) => {
            eprintln!("grep: {file}: {e}");
            false
        }
    }
}

/// A basic regular expression in the `regex` crate's syntax
fn from_basic(p: &str) -> String {
    let mut o = String::new();
    let mut chars = p.chars().peekable();
    let mut at_start = true;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(c @ ('(' | ')' | '|' | '{' | '}' | '+' | '?')) => o.push(c),
                Some(c) => {
                    o.push('\\');
                    o.push(c);
                }
                None => o.push_str("\\\\"),
            },
            '(' | ')' | '|' | '{' | '}' | '+' | '?' => {
                o.push('\\');
                o.push(c);
            }
            // A leading '*' is literal in a basic expression
            '*' if at_start => o.push_str("\\*"),
            '[' => {
                // Copy a bracket expression unchanged
                o.push('[');
                if chars.peek() == Some(&'^') {
                    o.push(chars.next().unwrap());
                }
                if chars.peek() == Some(&']') {
                    o.push_str("\\]");
                    chars.next();
                }
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if c == '[' || c == '\\' {
                        o.push('\\');
                    }
                    o.push(c);
                }
                o.push(']');
            }
            c => o.push(c),
        }
        at_start = c == '(' && o.ends_with('(') || o.ends_with('|') || o.is_empty();
    }
    o
}

fn build_regex(o: &Opts) -> Result<Regex, String> {
    let parts: Vec<String> = o
        .patterns
        .iter()
        .flat_map(|p| p.split('\n').map(|s| s.to_string()).collect::<Vec<_>>())
        .map(|p| match o.syntax {
            Syntax::Fixed => regex::escape(&p),
            Syntax::Extended => p,
            Syntax::Basic => from_basic(&p),
        })
        .map(|p| {
            if o.line_regex {
                format!("^(?:{p})$")
            } else if o.word {
                format!(r"(?:^|\b)(?:{p})(?:\b|$)")
            } else {
                format!("(?:{p})")
            }
        })
        .collect();
    RegexBuilder::new(&parts.join("|"))
        .case_insensitive(o.ignore_case)
        .multi_line(true)
        .unicode(false)
        .build()
        .map_err(|e| e.to_string().lines().last().unwrap_or("bad pattern").trim().to_string())
}

struct Grep<'a, W: Write> {
    o: &'a Opts,
    re: Regex,
    show_name: bool,
    out: W,
    matched: bool,
    trouble: bool,
}

impl<W: Write> Grep<'_, W> {
    fn path(&mut self, path: &str, top: bool) {
        if path == "-" {
            let stdin = io::stdin();
            let mut r = stdin.lock();
            self.search("(standard input)", &mut r);
            return;
        }
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                self.error(path, &e);
                return;
            }
        };
        let name = util::base_name(path.trim_end_matches('/'));
        if meta.is_dir() {
            if !self.o.recursive {
                if !self.o.no_messages {
                    eprintln!("grep: {path}: Is a directory");
                }
                return;
            }
            if !top && self.o.exclude_dir.iter().any(|g| util::glob_match(g, name)) {
                return;
            }
            let mut entries: Vec<String> = match std::fs::read_dir(path) {
                Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
                Err(e) => {
                    self.error(path, &e);
                    return;
                }
            };
            entries.sort();
            for e in entries {
                let p = if path.ends_with('/') { format!("{path}{e}") } else { format!("{path}/{e}") };
                let p = p.strip_prefix("./").map(|s| s.to_string()).unwrap_or(p);
                self.path(&p, false);
                if self.matched && self.o.quiet {
                    return;
                }
            }
            return;
        }
        if !top || self.o.recursive {
            if !self.o.include.is_empty() && !self.o.include.iter().any(|g| util::glob_match(g, name)) {
                return;
            }
            if self.o.exclude.iter().any(|g| util::glob_match(g, name)) {
                return;
            }
        }
        match std::fs::File::open(path) {
            Ok(f) => self.search(path, &mut BufReader::with_capacity(64 * 1024, f)),
            Err(e) => self.error(path, &e),
        }
    }

    fn error(&mut self, path: &str, e: &io::Error) {
        self.trouble = true;
        if !self.o.no_messages {
            let msg = e.to_string();
            let msg = msg.split(" (os error").next().unwrap_or(&msg).to_string();
            eprintln!("grep: {path}: {msg}");
        }
    }

    fn prefix(&mut self, name: &str, line_no: u64, offset: u64, sep: char) {
        let o = self.o;
        if self.show_name {
            if o.color {
                let _ = write!(self.out, "\x1b[35m{name}\x1b[36m{sep}\x1b[0m");
            } else {
                let _ = write!(self.out, "{name}{sep}");
            }
        }
        if o.line_number {
            if o.color {
                let _ = write!(self.out, "\x1b[32m{line_no}\x1b[36m{sep}\x1b[0m");
            } else {
                let _ = write!(self.out, "{line_no}{sep}");
            }
        }
        if o.byte_offset {
            let _ = write!(self.out, "{offset}{sep}");
        }
    }

    fn search(&mut self, name: &str, input: &mut dyn BufRead) {
        let o = self.o;
        let mut line = Vec::new();
        let mut line_no = 0u64;
        let mut offset = 0u64;
        let mut count = 0u64;
        let mut before: std::collections::VecDeque<(u64, u64, Vec<u8>)> = std::collections::VecDeque::new();
        let mut after_left = 0usize;
        let mut last_printed = 0u64;
        let mut binary_checked = false;
        let mut is_binary = false;
        let context = o.after > 0 || o.before > 0;
        let listing = o.files_with || o.files_without || o.count || o.quiet;
        loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    self.error(name, &e);
                    break;
                }
            }
            line_no += 1;
            let start = offset;
            offset += line.len() as u64;
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if !binary_checked {
                binary_checked = true;
                is_binary = o.binary != Binary::Text && line.contains(&0);
                if is_binary && o.binary == Binary::Skip {
                    return;
                }
            } else if !is_binary && o.binary != Binary::Text && line.contains(&0) {
                is_binary = true;
                if o.binary == Binary::Skip {
                    return;
                }
            }
            let hit = self.re.is_match(&line) != o.invert;
            if !hit {
                if after_left > 0 && !listing {
                    after_left -= 1;
                    self.prefix(name, line_no, start, '-');
                    let _ = self.out.write_all(&line);
                    let _ = self.out.write_all(b"\n");
                    last_printed = line_no;
                } else if o.before > 0 {
                    before.push_back((line_no, start, line.clone()));
                    if before.len() > o.before {
                        before.pop_front();
                    }
                }
                continue;
            }
            count += 1;
            self.matched = true;
            if o.quiet || o.files_without {
                if o.quiet {
                    return;
                }
                break;
            }
            if o.files_with {
                let _ = writeln!(self.out, "{name}");
                return;
            }
            if !o.count {
                if is_binary {
                    let _ = self.out.flush();
                    println!("Binary file {name} matches");
                    return;
                }
                if context && last_printed > 0 && before.front().map_or(line_no, |b| b.0) > last_printed + 1 {
                    let _ = writeln!(self.out, "--");
                }
                for (n, off, l) in before.drain(..) {
                    self.prefix(name, n, off, '-');
                    let _ = self.out.write_all(&l);
                    let _ = self.out.write_all(b"\n");
                }
                if o.json {
                    let text = String::from_utf8_lossy(&line);
                    let spans: Vec<String> = if o.invert {
                        Vec::new()
                    } else {
                        self.re.find_iter(&line).map(|m| format!("[{},{}]", m.start(), m.end())).collect()
                    };
                    let _ = writeln!(
                        self.out,
                        "{{\"file\":{},\"line\":{},\"offset\":{},\"text\":{},\"matches\":[{}]}}",
                        util::json_str(name),
                        line_no,
                        start,
                        util::json_str(&text),
                        spans.join(",")
                    );
                } else if o.only && !o.invert {
                    let found: Vec<(usize, usize)> = self.re.find_iter(&line).filter(|m| m.end() > m.start()).map(|m| (m.start(), m.end())).collect();
                    for (s, e) in found {
                        self.prefix(name, line_no, start + s as u64, ':');
                        if o.color {
                            let _ = self.out.write_all(b"\x1b[1;31m");
                        }
                        let _ = self.out.write_all(&line[s..e]);
                        if o.color {
                            let _ = self.out.write_all(b"\x1b[0m");
                        }
                        let _ = self.out.write_all(b"\n");
                    }
                } else {
                    self.prefix(name, line_no, start, ':');
                    if o.color && !o.invert {
                        let mut at = 0;
                        let spans: Vec<(usize, usize)> = self.re.find_iter(&line).filter(|m| m.end() > m.start()).map(|m| (m.start(), m.end())).collect();
                        for (s, e) in spans {
                            let _ = self.out.write_all(&line[at..s]);
                            let _ = self.out.write_all(b"\x1b[1;31m");
                            let _ = self.out.write_all(&line[s..e]);
                            let _ = self.out.write_all(b"\x1b[0m");
                            at = e;
                        }
                        let _ = self.out.write_all(&line[at..]);
                    } else {
                        let _ = self.out.write_all(&line);
                    }
                    let _ = self.out.write_all(b"\n");
                }
                last_printed = line_no;
                after_left = o.after;
            }
            if o.max.is_some_and(|m| count >= m) {
                break;
            }
        }
        if o.count {
            if self.show_name {
                let _ = write!(self.out, "{name}:");
            }
            let _ = writeln!(self.out, "{count}");
        }
        if o.files_without && count == 0 {
            let _ = writeln!(self.out, "{name}");
        }
    }
}

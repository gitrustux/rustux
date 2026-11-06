// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Small helpers shared by the commands

use std::io::Write;

pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn flush() {
    let _ = std::io::stdout().flush();
}

/// Is `fd` the console?
pub fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}

/// The screen size (rows, columns) of the console behind `fd`
pub fn term_size(fd: i32) -> (usize, usize) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
        (ws.ws_row as usize, ws.ws_col as usize)
    } else {
        (25, 80)
    }
}

/// Raw mode on the console behind `fd` while this lives
pub struct RawMode {
    fd: i32,
    old: libc::termios,
}

impl RawMode {
    pub fn new(fd: i32) -> Option<RawMode> {
        let mut old: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
            return None;
        }
        let mut raw = old;
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_lflag |= libc::ISIG; // Ctrl+C still interrupts
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
        Some(RawMode { fd, old })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.old) };
    }
}

/// A JSON string literal
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Does `name` match the shell-style pattern (`*`, `?`, `[abc]`)?
pub fn glob_match(pat: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| m(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && m(&p[1..], &n[1..]),
            Some('[') => {
                let Some(close) = p.iter().skip(1).position(|&c| c == ']').map(|i| i + 1) else {
                    return !n.is_empty() && n[0] == '[' && m(&p[1..], &n[1..]);
                };
                if n.is_empty() {
                    return false;
                }
                let set = &p[1..close];
                let (neg, set) = if set.first() == Some(&'!') || set.first() == Some(&'^') { (true, &set[1..]) } else { (false, set) };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= set[i] <= n[0] && n[0] <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == n[0];
                        i += 1;
                    }
                }
                hit != neg && m(&p[close + 1..], &n[1..])
            }
            Some(&c) => !n.is_empty() && n[0] == c && m(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    m(&p, &n)
}

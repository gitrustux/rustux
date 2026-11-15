// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! A VT100/xterm-style terminal: the grid of cells that the bytes from a
//! pseudo-terminal change. The same sequences as the text console (the
//! kernel's `drivers/display/console.rs`), with true colour kept as is:
//!
//! - UTF-8 text; BS, HT, LF/VT/FF (with CR: newline mode, as the console),
//!   CR, BEL; ESC 7/8, D, M, E, c; OSC 0/2 sets the title
//! - CSI A B C D E F G ` H f d e a (cursor), J K X (erase), L M (lines),
//!   @ P (characters), S T (scroll), r (region), s u, m (bold, dim, italic
//!   as normal, underline, reverse, 16, 256 and true colours), ?25 (cursor),
//!   ?7 (wrap), ?1049/?1047/?47 (alternate screen), 20 (newline mode),
//!   n (5: status, 6: cursor position), c (device attributes)
//! - a scrollback of SCROLLBACK lines (not for the alternate screen)

use std::collections::VecDeque;

pub const SCROLLBACK: usize = 5000;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color {
    Default,
    Index(u8),
    Rgb(u32),
}

pub const BOLD: u8 = 1;
pub const UNDERLINE: u8 = 2;
pub const REVERSE: u8 = 4;
pub const DIM: u8 = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attr: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', fg: Color::Default, bg: Color::Default, attr: 0 };
}

#[derive(Clone, Copy)]
struct Saved {
    x: usize,
    y: usize,
    fg: Color,
    bg: Color,
    attr: u8,
}

enum State {
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Charset,
}

pub struct Term {
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<Vec<Cell>>,
    pub scrollback: VecDeque<Vec<Cell>>,
    alt_saved: Option<(Vec<Vec<Cell>>, Saved)>,
    pub cx: usize,
    pub cy: usize,
    fg: Color,
    bg: Color,
    attr: u8,
    saved: Saved,
    top: usize,
    bottom: usize,
    wrap_pending: bool,
    autowrap: bool,
    newline_mode: bool,
    pub cursor_visible: bool,
    state: State,
    params: Vec<u32>,
    private: bool,
    osc: Vec<u8>,
    utf8: Vec<u8>,
    /// Set by OSC 0 or 2
    pub title: String,
    /// Answers to the program (cursor position, device attributes)
    pub replies: Vec<u8>,
    /// Lines pushed into the scrollback since this was last cleared
    pub scrolled: usize,
    /// Changed since this was last cleared
    pub dirty: bool,
    pub bell: bool,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Term {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let blank = Saved { x: 0, y: 0, fg: Color::Default, bg: Color::Default, attr: 0 };
        Term {
            cols,
            rows,
            lines: vec![vec![Cell::BLANK; cols]; rows],
            scrollback: VecDeque::new(),
            alt_saved: None,
            cx: 0,
            cy: 0,
            fg: Color::Default,
            bg: Color::Default,
            attr: 0,
            saved: blank,
            top: 0,
            bottom: rows - 1,
            wrap_pending: false,
            autowrap: true,
            newline_mode: true,
            cursor_visible: true,
            state: State::Ground,
            params: Vec::new(),
            private: false,
            osc: Vec::new(),
            utf8: Vec::new(),
            title: String::new(),
            replies: Vec::new(),
            scrolled: 0,
            dirty: true,
            bell: false,
        }
    }

    pub fn alternate(&self) -> bool {
        self.alt_saved.is_some()
    }

    fn blank(&self) -> Cell {
        // Erasing uses the current background (xterm's behaviour)
        Cell { ch: ' ', fg: self.fg, bg: self.bg, attr: 0 }
    }

    /// A new size: lines are cut or padded; rows that no longer fit go to
    /// the scrollback so the cursor's line stays on the screen
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if cols == self.cols && rows == self.rows {
            return;
        }
        for l in self.lines.iter_mut() {
            l.resize(cols, Cell::BLANK);
        }
        if let Some((main, _)) = &mut self.alt_saved {
            for l in main.iter_mut() {
                l.resize(cols, Cell::BLANK);
            }
            main.resize(rows, vec![Cell::BLANK; cols]);
        }
        while self.lines.len() > rows {
            if self.cy > 0 && self.alt_saved.is_none() {
                let l = self.lines.remove(0);
                self.push_scrollback(l);
                self.cy -= 1;
            } else if self.cy > 0 {
                self.lines.remove(0);
                self.cy -= 1;
            } else {
                self.lines.pop();
            }
        }
        while self.lines.len() < rows {
            self.lines.push(vec![Cell::BLANK; cols]);
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.saved.x = self.saved.x.min(cols - 1);
        self.saved.y = self.saved.y.min(rows - 1);
        self.wrap_pending = false;
        self.dirty = true;
    }

    fn push_scrollback(&mut self, l: Vec<Cell>) {
        if self.scrollback.len() == SCROLLBACK {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(l);
        self.scrolled += 1;
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
        self.dirty = true;
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Esc => return self.after_esc(b),
            State::Csi => return self.in_csi(b),
            State::Osc => {
                match b {
                    0x07 => self.end_osc(),
                    0x1B => self.state = State::OscEsc,
                    _ if self.osc.len() < 512 => self.osc.push(b),
                    _ => {}
                }
                return;
            }
            State::OscEsc => {
                self.end_osc();
                return;
            }
            State::Charset => {
                self.state = State::Ground;
                return;
            }
            State::Ground => {}
        }
        if !self.utf8.is_empty() {
            if b & 0xC0 == 0x80 {
                self.utf8.push(b);
                let need = match self.utf8[0] {
                    0xF0..=0xF7 => 4,
                    0xE0..=0xEF => 3,
                    _ => 2,
                };
                if self.utf8.len() == need {
                    let c = std::str::from_utf8(&self.utf8).ok().and_then(|s| s.chars().next()).unwrap_or('\u{FFFD}');
                    self.utf8.clear();
                    self.put(c);
                }
                return;
            }
            self.utf8.clear();
            self.put('\u{FFFD}');
        }
        match b {
            0x1B => self.state = State::Esc,
            b'\n' | 0x0B | 0x0C => {
                if self.newline_mode {
                    self.cx = 0;
                }
                self.linefeed();
            }
            b'\r' => {
                self.cx = 0;
                self.wrap_pending = false;
            }
            b'\t' => {
                self.cx = ((self.cx + 8) & !7).min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x08 => {
                if self.wrap_pending {
                    self.wrap_pending = false;
                } else if self.cx > 0 {
                    self.cx -= 1;
                }
            }
            0x07 => self.bell = true,
            0x20..=0x7E => self.put(b as char),
            0xC2..=0xF4 => self.utf8.push(b),
            0x80..=0xFF => self.put('\u{FFFD}'),
            _ => {}
        }
    }

    fn put(&mut self, c: char) {
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.autowrap {
                self.cx = 0;
                self.linefeed();
            }
        }
        let cell = Cell { ch: c, fg: self.fg, bg: self.bg, attr: self.attr };
        self.lines[self.cy][self.cx] = cell;
        if self.cx + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.cx += 1;
        }
    }

    fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cy == self.bottom {
            self.scroll_up(1);
        } else if self.cy + 1 < self.rows {
            self.cy += 1;
        }
    }

    /// Scroll the region up by n; lines leaving the top of a full-screen
    /// region of the main screen go to the scrollback
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let keep = self.top == 0 && self.alt_saved.is_none();
        let blank = self.blank();
        for _ in 0..n {
            let l = self.lines.remove(self.top);
            if keep {
                self.push_scrollback(l);
            }
            self.lines.insert(self.bottom, vec![blank; self.cols]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let blank = self.blank();
        for _ in 0..n {
            self.lines.remove(self.bottom);
            self.lines.insert(self.top, vec![blank; self.cols]);
        }
    }

    fn end_osc(&mut self) {
        self.state = State::Ground;
        let s = String::from_utf8_lossy(&self.osc).into_owned();
        self.osc.clear();
        if let Some((n, t)) = s.split_once(';') {
            if n == "0" || n == "2" {
                self.title = t.chars().take(80).collect();
            }
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Saved { x: self.cx, y: self.cy, fg: self.fg, bg: self.bg, attr: self.attr };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved;
        self.cx = s.x.min(self.cols - 1);
        self.cy = s.y.min(self.rows - 1);
        self.fg = s.fg;
        self.bg = s.bg;
        self.attr = s.attr;
        self.wrap_pending = false;
    }

    fn after_esc(&mut self, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.private = false;
            }
            b']' => {
                self.state = State::Osc;
                self.osc.clear();
            }
            b'(' | b')' | b'*' | b'+' => self.state = State::Charset,
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.linefeed(),
            b'E' => {
                self.cx = 0;
                self.linefeed();
            }
            b'M' => {
                self.wrap_pending = false;
                if self.cy == self.top {
                    self.scroll_down(1);
                } else if self.cy > 0 {
                    self.cy -= 1;
                }
            }
            b'c' => self.reset(),
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        if let Some((main, saved)) = self.alt_saved.take() {
            self.lines = main;
            self.saved = saved;
        }
        self.fg = Color::Default;
        self.bg = Color::Default;
        self.attr = 0;
        self.newline_mode = true;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.autowrap = true;
        self.cursor_visible = true;
        for l in self.lines.iter_mut() {
            l.fill(Cell::BLANK);
        }
        self.cx = 0;
        self.cy = 0;
        self.wrap_pending = false;
    }

    fn in_csi(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                if self.params.is_empty() {
                    self.params.push(0);
                }
                let last = self.params.last_mut().unwrap();
                *last = last.saturating_mul(10).saturating_add((b - b'0') as u32);
            }
            b';' | b':' => {
                if self.params.is_empty() {
                    self.params.push(0);
                }
                if self.params.len() < 32 {
                    self.params.push(0);
                }
            }
            b'?' | b'>' | b'=' => self.private = true,
            0x20..=0x2F => {}
            0x40..=0x7E => {
                self.state = State::Ground;
                self.csi(b);
            }
            _ => self.state = State::Ground,
        }
    }

    /// Parameter i, `default` for a missing or zero one
    fn param(&self, i: usize, default: usize) -> usize {
        match self.params.get(i) {
            Some(&v) if v != 0 => v as usize,
            _ => default,
        }
    }

    fn erase_line(&mut self, y: usize, from: usize, to: usize) {
        let blank = self.blank();
        let to = to.min(self.cols);
        for c in &mut self.lines[y][from.min(to)..to] {
            *c = blank;
        }
    }

    fn csi(&mut self, cmd: u8) {
        let n = self.param(0, 1);
        let (cols, rows) = (self.cols, self.rows);
        if self.private {
            if cmd == b'h' || cmd == b'l' {
                let on = cmd == b'h';
                for p in self.params.clone() {
                    match p {
                        25 => self.cursor_visible = on,
                        7 => self.autowrap = on,
                        47 | 1047 | 1049 => self.alternate_screen(on),
                        _ => {}
                    }
                }
            } else if cmd == b'c' {
                self.replies.extend_from_slice(b"\x1b[>0;10;1c");
            }
            return;
        }
        self.wrap_pending = false;
        match cmd {
            b'h' | b'l' => {
                if self.params.contains(&20) {
                    self.newline_mode = cmd == b'h';
                }
            }
            b'A' => {
                let floor = if self.cy >= self.top { self.top } else { 0 };
                self.cy = self.cy.saturating_sub(n).max(floor);
            }
            b'B' | b'e' => {
                let ceil = if self.cy <= self.bottom { self.bottom } else { rows - 1 };
                self.cy = (self.cy + n).min(ceil);
            }
            b'C' | b'a' => self.cx = (self.cx + n).min(cols - 1),
            b'D' => self.cx = self.cx.saturating_sub(n),
            b'E' => {
                self.cy = (self.cy + n).min(rows - 1);
                self.cx = 0;
            }
            b'F' => {
                self.cy = self.cy.saturating_sub(n);
                self.cx = 0;
            }
            b'G' | b'`' => self.cx = (n - 1).min(cols - 1),
            b'd' => self.cy = (n - 1).min(rows - 1),
            b'H' | b'f' => {
                self.cy = (self.param(0, 1) - 1).min(rows - 1);
                self.cx = (self.param(1, 1) - 1).min(cols - 1);
            }
            b'J' => match self.param(0, 0) {
                0 => {
                    self.erase_line(self.cy, self.cx, cols);
                    for y in self.cy + 1..rows {
                        self.erase_line(y, 0, cols);
                    }
                }
                1 => {
                    for y in 0..self.cy {
                        self.erase_line(y, 0, cols);
                    }
                    self.erase_line(self.cy, 0, self.cx + 1);
                }
                2 => {
                    for y in 0..rows {
                        self.erase_line(y, 0, cols);
                    }
                }
                3 => self.scrollback.clear(),
                _ => {}
            },
            b'K' => match self.param(0, 0) {
                0 => self.erase_line(self.cy, self.cx, cols),
                1 => self.erase_line(self.cy, 0, self.cx + 1),
                2 => self.erase_line(self.cy, 0, cols),
                _ => {}
            },
            b'X' => self.erase_line(self.cy, self.cx, self.cx + n),
            b'L' | b'M' => {
                if self.cy >= self.top && self.cy <= self.bottom {
                    let saved_top = self.top;
                    self.top = self.cy;
                    if cmd == b'L' {
                        self.scroll_down(n);
                    } else {
                        // Deleted lines are not kept
                        let n = n.min(self.bottom - self.top + 1);
                        let blank = self.blank();
                        for _ in 0..n {
                            self.lines.remove(self.top);
                            self.lines.insert(self.bottom, vec![blank; self.cols]);
                        }
                    }
                    self.top = saved_top;
                    self.cx = 0;
                }
            }
            b'@' | b'P' => {
                let x = self.cx;
                let n = n.min(cols - x);
                let blank = self.blank();
                let line = &mut self.lines[self.cy];
                if cmd == b'@' {
                    line.copy_within(x..cols - n, x + n);
                    for c in &mut line[x..x + n] {
                        *c = blank;
                    }
                } else {
                    line.copy_within(x + n..cols, x);
                    for c in &mut line[cols - n..cols] {
                        *c = blank;
                    }
                }
            }
            b'S' => self.scroll_up(n),
            b'T' => self.scroll_down(n),
            b'r' => {
                let top = self.param(0, 1) - 1;
                let bottom = self.param(1, rows).min(rows) - 1;
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.cx = 0;
                    self.cy = 0;
                }
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'm' => self.sgr(),
            b'n' => match self.param(0, 0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let r = format!("\x1b[{};{}R", self.cy + 1, self.cx + 1);
                    self.replies.extend_from_slice(r.as_bytes());
                }
                _ => {}
            },
            b'c' => self.replies.extend_from_slice(b"\x1b[?1;2c"),
            _ => {}
        }
    }

    fn sgr(&mut self) {
        if self.params.is_empty() {
            self.fg = Color::Default;
            self.bg = Color::Default;
            self.attr = 0;
            return;
        }
        let p = self.params.clone();
        let get = |k: usize| p.get(k).copied().unwrap_or(0);
        let mut i = 0;
        while i < p.len() {
            let v = p[i];
            match v {
                0 => {
                    self.fg = Color::Default;
                    self.bg = Color::Default;
                    self.attr = 0;
                }
                1 => self.attr |= BOLD,
                2 => self.attr |= DIM,
                4 => self.attr |= UNDERLINE,
                7 => self.attr |= REVERSE,
                21 | 22 => self.attr &= !(BOLD | DIM),
                24 => self.attr &= !UNDERLINE,
                27 => self.attr &= !REVERSE,
                30..=37 => self.fg = Color::Index((v - 30) as u8),
                39 => self.fg = Color::Default,
                40..=47 => self.bg = Color::Index((v - 40) as u8),
                49 => self.bg = Color::Default,
                90..=97 => self.fg = Color::Index((v - 90 + 8) as u8),
                100..=107 => self.bg = Color::Index((v - 100 + 8) as u8),
                38 | 48 => {
                    let c = match get(i + 1) {
                        5 => {
                            let c = Color::Index(get(i + 2).min(255) as u8);
                            i += 2;
                            Some(c)
                        }
                        2 => {
                            let (r, g, b) = (get(i + 2).min(255), get(i + 3).min(255), get(i + 4).min(255));
                            i += 4;
                            Some(Color::Rgb(r << 16 | g << 8 | b))
                        }
                        _ => None,
                    };
                    if let Some(c) = c {
                        if v == 38 {
                            self.fg = c;
                        } else {
                            self.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn alternate_screen(&mut self, on: bool) {
        if on && self.alt_saved.is_none() {
            self.save_cursor();
            let blank = vec![vec![Cell::BLANK; self.cols]; self.rows];
            let main = std::mem::replace(&mut self.lines, blank);
            self.alt_saved = Some((main, self.saved));
        } else if !on {
            if let Some((main, saved)) = self.alt_saved.take() {
                self.lines = main;
                self.saved = saved;
                self.restore_cursor();
            }
        }
    }

    /// The text of the whole lines from..=to of the scrollback followed by
    /// the screen (line numbers counted from the oldest), columns c0 of the
    /// first to c1 of the last; trailing blanks of each line dropped
    pub fn text(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let mut out = String::new();
        for l in from.0..=to.0 {
            let Some(line) = self.line(l) else { break };
            let a = if l == from.0 { from.1 } else { 0 };
            let b = if l == to.0 { (to.1 + 1).min(line.len()) } else { line.len() };
            let s: String = line[a.min(b)..b].iter().map(|c| c.ch).collect();
            out.push_str(s.trim_end());
            if l != to.0 {
                out.push('\n');
            }
        }
        out
    }

    /// Line l of the scrollback followed by the screen
    pub fn line(&self, l: usize) -> Option<&Vec<Cell>> {
        if l < self.scrollback.len() {
            self.scrollback.get(l)
        } else {
            self.lines.get(l - self.scrollback.len())
        }
    }
}

/// The xterm palette in Dracula's colours (0-15), the 6x6x6 cube and the
/// grey ramp
pub fn palette(i: u8) -> u32 {
    const ANSI16: [u32; 16] = [
        0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2,
        0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF,
    ];
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let n = (i - 16) as u32;
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            level(n / 36) << 16 | level(n / 6 % 6) << 8 | level(n % 6)
        }
        _ => {
            let v = 8 + (i as u32 - 232) * 10;
            v << 16 | v << 8 | v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: &Term, y: usize) -> String {
        t.lines[y].iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn text_and_newlines() {
        let mut t = Term::new(10, 3);
        t.feed(b"ab\ncd\r\nef");
        assert_eq!(row(&t, 0), "ab");
        assert_eq!(row(&t, 1), "cd");
        assert_eq!(row(&t, 2), "ef");
        t.feed(b"\nxyz");
        assert_eq!(row(&t, 2), "xyz");
        assert_eq!(t.scrollback.len(), 1);
    }

    #[test]
    fn wrap_and_erase() {
        let mut t = Term::new(4, 2);
        t.feed(b"abcdef");
        assert_eq!(row(&t, 0), "abcd");
        assert_eq!(row(&t, 1), "ef");
        t.feed(b"\x1b[1;2H\x1b[K");
        assert_eq!(row(&t, 0), "a");
        t.feed(b"\x1b[2J");
        assert_eq!(row(&t, 1), "");
    }

    #[test]
    fn colours_and_utf8() {
        let mut t = Term::new(10, 2);
        t.feed("\x1b[1;31mé\x1b[38;2;1;2;3mx\x1b[0m".as_bytes());
        assert_eq!(t.lines[0][0].ch, 'é');
        assert_eq!(t.lines[0][0].fg, Color::Index(1));
        assert_eq!(t.lines[0][0].attr, BOLD);
        assert_eq!(t.lines[0][1].fg, Color::Rgb(0x010203));
    }

    #[test]
    fn alternate_screen_and_title() {
        let mut t = Term::new(10, 2);
        t.feed(b"main\x1b[?1049h\x1b[HALT\x1b]0;hello\x07");
        assert_eq!(row(&t, 0), "ALT");
        assert_eq!(t.title, "hello");
        t.feed(b"\x1b[?1049l");
        assert_eq!(row(&t, 0), "main");
    }

    #[test]
    fn resize_keeps_cursor_line() {
        let mut t = Term::new(10, 4);
        t.feed(b"1\n2\n3\n4");
        t.resize(5, 2);
        assert_eq!(row(&t, 1), "4");
        assert_eq!(t.cy, 1);
        assert_eq!(t.scrollback.len(), 2);
    }

    #[test]
    fn cursor_report() {
        let mut t = Term::new(10, 4);
        t.feed(b"\x1b[3;5H\x1b[6n");
        assert_eq!(t.replies, b"\x1b[3;5R");
    }
}

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Text console: a VT100/xterm-style terminal on the framebuffer.
//!
//! The screen is a grid of cells (glyph, colours, attributes) kept in
//! memory; writes change the grid and mark rows dirty, and the dirty rows
//! are drawn once at the end of each write (so `cat` of a long file redraws
//! the screen once per write, not once per line). Supported:
//!
//! - UTF-8 text: ASCII, box drawing, blocks and a few symbols have glyphs;
//!   accented Latin letters show without the accent; anything else is a box
//! - control characters BS, HT, LF, VT, FF, CR; ESC 7/8 (save/restore
//!   cursor), ESC D/M/E (index, reverse index, next line), ESC c (reset);
//!   OSC strings (window title) are skipped
//! - CSI A B C D E F G H f d (cursor), J K X (erase), L M (insert/delete
//!   lines), @ P (insert/delete characters), S T (scroll), r (scrolling
//!   region), s u (save/restore), m (bold, underline, reverse, 16 and 256
//!   colours, true colour approximated), ?25 h/l (cursor), ?7 (wrap),
//!   ?1049 / ?47 (alternate screen)
//! - scrollback: 500 lines, viewed with the Rustux sequence CSI n ^ (n = 1
//!   page up, 2 page down, 0 back to the bottom), which the console
//!   service sends for Shift+PgUp / Shift+PgDn

use crate::drivers::display::font::SimpleVgaFont;
use crate::drivers::display::framebuffer::{Color, Framebuffer, PixelFormat};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The console; writers come from syscalls and from the fault and panic
/// paths, so it is locked with interrupts off
static CONSOLE: crate::sync::IrqMutex<Option<TextConsole>> = crate::sync::IrqMutex::new(None);
static CONSOLE_INITIALIZED: AtomicBool = AtomicBool::new(false);
/// Size in characters (for `system_get_stats`)
static COLS: AtomicUsize = AtomicUsize::new(80);
static ROWS: AtomicUsize = AtomicUsize::new(25);

/// Dracula palette (the system theme)
pub const DRACULA_BG: Color = Color::new(0x28, 0x2A, 0x36);
pub const DRACULA_FG: Color = Color::new(0xF8, 0xF8, 0xF2);

/// ANSI colours 0-15 in the Dracula palette (normal, then bright)
const ANSI16: [Color; 16] = [
    Color::new(0x21, 0x22, 0x2C), // black
    Color::new(0xFF, 0x55, 0x55), // red
    Color::new(0x50, 0xFA, 0x7B), // green
    Color::new(0xF1, 0xFA, 0x8C), // yellow
    Color::new(0xBD, 0x93, 0xF9), // blue (purple)
    Color::new(0xFF, 0x79, 0xC6), // magenta (pink)
    Color::new(0x8B, 0xE9, 0xFD), // cyan
    Color::new(0xF8, 0xF8, 0xF2), // white
    Color::new(0x62, 0x72, 0xA4), // bright black (comment grey)
    Color::new(0xFF, 0x6E, 0x6E),
    Color::new(0x69, 0xFF, 0x94),
    Color::new(0xFF, 0xFF, 0xA5),
    Color::new(0xD6, 0xAC, 0xFF),
    Color::new(0xFF, 0x92, 0xDF),
    Color::new(0xA4, 0xFF, 0xFF),
    Color::new(0xFF, 0xFF, 0xFF),
];

/// Colour indices: 0-255 xterm palette, plus the theme's defaults
const DEF_FG: u16 = 256;
const DEF_BG: u16 = 257;

const ATTR_BOLD: u8 = 1;
const ATTR_UNDERLINE: u8 = 2;
const ATTR_REVERSE: u8 = 4;

const SCROLLBACK: usize = 500;

fn palette(i: u16) -> Color {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let n = i - 16;
            let level = |v: u16| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            Color::new(level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        232..=255 => {
            let v = (8 + (i - 232) * 10) as u8;
            Color::new(v, v, v)
        }
        DEF_FG => DRACULA_FG,
        _ => DRACULA_BG,
    }
}

/// Nearest palette entry for a true colour (the 6x6x6 cube)
fn nearest(r: u16, g: u16, b: u16) -> u16 {
    let q = |v: u16| if v < 48 { 0 } else if v < 115 { 1 } else { (v - 35) / 40 };
    16 + 36 * q(r).min(5) + 6 * q(g).min(5) + q(b).min(5)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    glyph: u16,
    fg: u16,
    bg: u16,
    attr: u8,
}

const BLANK: Cell = Cell { glyph: b' ' as u16, fg: DEF_FG, bg: DEF_BG, attr: 0 };

#[derive(Clone, Copy)]
struct Saved {
    x: usize,
    y: usize,
    fg: u16,
    bg: u16,
    attr: u8,
}

/// Text console with framebuffer backing
pub struct TextConsole {
    framebuffer: Framebuffer,
    cols: usize,
    rows: usize,
    grid: Vec<Cell>,
    dirty: Vec<bool>,
    /// Lines scrolled off the top (a ring), and how far back the view is
    history: Vec<Cell>,
    history_len: usize,
    history_head: usize,
    view: usize,
    /// The main screen while the alternate one is in use
    alt_saved: Option<(Vec<Cell>, Saved)>,
    cursor_x: usize,
    cursor_y: usize,
    /// A character was written in the last column: the next one wraps
    wrap_pending: bool,
    autowrap: bool,
    /// LF also returns to column 0 (ESC[20h, on by default: programs
    /// write "\n" and there is no tty layer to add the "\r")
    newline_mode: bool,
    cursor_visible: bool,
    /// Row where the cursor was drawn last (redrawn to erase it)
    cursor_row_drawn: Option<usize>,
    fg: u16,
    bg: u16,
    attr: u8,
    /// Scrolling region, inclusive
    top: usize,
    bottom: usize,
    saved: Saved,
    // Parser
    esc_state: u8, // 0 text, 1 ESC, 2 CSI, 3 OSC, 4 OSC after ESC, 5 skip one
    esc_params: [u16; 16],
    esc_nparams: usize,
    esc_private: bool,
    utf8_cp: u32,
    utf8_left: u8,
}

impl TextConsole {
    pub fn new(framebuffer: Framebuffer) -> Self {
        let cols = (framebuffer.width / SimpleVgaFont::width()).max(1);
        let rows = (framebuffer.height / SimpleVgaFont::height()).max(1);
        COLS.store(cols, Ordering::Relaxed);
        ROWS.store(rows, Ordering::Relaxed);
        let saved = Saved { x: 0, y: 0, fg: DEF_FG, bg: DEF_BG, attr: 0 };
        Self {
            framebuffer,
            cols,
            rows,
            grid: vec![BLANK; cols * rows],
            dirty: vec![true; rows],
            history: vec![BLANK; cols * SCROLLBACK],
            history_len: 0,
            history_head: 0,
            view: 0,
            alt_saved: None,
            cursor_x: 0,
            cursor_y: 0,
            wrap_pending: false,
            autowrap: true,
            newline_mode: true,
            cursor_visible: true,
            cursor_row_drawn: None,
            fg: DEF_FG,
            bg: DEF_BG,
            attr: 0,
            top: 0,
            bottom: rows - 1,
            saved,
            esc_state: 0,
            esc_params: [0; 16],
            esc_nparams: 0,
            esc_private: false,
            utf8_cp: 0,
            utf8_left: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn fg_color(&self) -> Color {
        palette(self.fg)
    }

    pub fn bg_color(&self) -> Color {
        palette(self.bg)
    }

    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Clear the screen and home the cursor
    pub fn clear(&mut self) {
        self.erase(0, self.cols * self.rows);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.wrap_pending = false;
        self.flush();
    }

    fn blank(&self) -> Cell {
        Cell { glyph: b' ' as u16, fg: self.fg, bg: self.bg, attr: self.attr & ATTR_REVERSE }
    }

    /// Blank cells [from, to) of the grid (row-major indices)
    fn erase(&mut self, from: usize, to: usize) {
        let to = to.min(self.grid.len());
        if to <= from {
            return;
        }
        let b = self.blank();
        for c in &mut self.grid[from..to] {
            *c = b;
        }
        for r in from / self.cols..=(to - 1) / self.cols {
            self.dirty[r] = true;
        }
    }

    fn mark_all(&mut self) {
        for d in self.dirty.iter_mut() {
            *d = true;
        }
    }

    /// Scroll the region up by n, blank lines at its bottom. With
    /// `keep`, lines leaving the top of the whole main screen go to the
    /// scrollback.
    fn scroll_up(&mut self, n: usize, keep: bool) {
        let (top, bottom, cols) = (self.top, self.bottom, self.cols);
        let n = n.min(bottom - top + 1);
        if keep && top == 0 && self.alt_saved.is_none() {
            for r in 0..n {
                let dst = self.history_head * cols;
                self.history[dst..dst + cols].copy_from_slice(&self.grid[r * cols..(r + 1) * cols]);
                self.history_head = (self.history_head + 1) % SCROLLBACK;
                self.history_len = (self.history_len + 1).min(SCROLLBACK);
            }
        }
        self.grid.copy_within((top + n) * cols..(bottom + 1) * cols, top * cols);
        self.erase((bottom + 1 - n) * cols, (bottom + 1) * cols);
        for r in top..=bottom {
            self.dirty[r] = true;
        }
    }

    /// Scroll the region down by n, blank lines at its top
    fn scroll_down(&mut self, n: usize) {
        let (top, bottom, cols) = (self.top, self.bottom, self.cols);
        let n = n.min(bottom - top + 1);
        self.grid.copy_within(top * cols..(bottom + 1 - n) * cols, (top + n) * cols);
        self.erase(top * cols, (top + n) * cols);
        for r in top..=bottom {
            self.dirty[r] = true;
        }
    }

    fn linefeed(&mut self) {
        self.wrap_pending = false;
        if self.cursor_y == self.bottom {
            self.scroll_up(1, true);
        } else if self.cursor_y + 1 < self.rows {
            self.cursor_y += 1;
        }
    }

    fn put_glyph(&mut self, glyph: u16) {
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.autowrap {
                self.cursor_x = 0;
                self.linefeed();
            }
        }
        let i = self.cursor_y * self.cols + self.cursor_x;
        self.grid[i] = Cell { glyph, fg: self.fg, bg: self.bg, attr: self.attr };
        self.dirty[self.cursor_y] = true;
        if self.cursor_x + 1 < self.cols {
            self.cursor_x += 1;
        } else {
            self.wrap_pending = true;
        }
    }

    /// Feed one byte of output (call `flush` to draw)
    pub fn put_char(&mut self, ch: u8) {
        if self.view != 0 {
            // New output: back to the bottom of the scrollback
            self.view = 0;
            self.mark_all();
        }
        match self.esc_state {
            1 => return self.after_esc(ch),
            2 => return self.in_csi(ch),
            3 => {
                // OSC: until BEL or ESC \
                match ch {
                    0x07 => self.esc_state = 0,
                    0x1B => self.esc_state = 4,
                    _ => {}
                }
                return;
            }
            4 | 5 => {
                // The byte after ESC in an OSC (\), or a charset designator
                self.esc_state = 0;
                return;
            }
            _ => {}
        }
        if self.utf8_left > 0 {
            if ch & 0xC0 == 0x80 {
                self.utf8_cp = (self.utf8_cp << 6) | (ch & 0x3F) as u32;
                self.utf8_left -= 1;
                if self.utf8_left == 0 {
                    self.put_glyph(glyph_for(self.utf8_cp));
                }
                return;
            }
            self.utf8_left = 0;
            self.put_glyph(GLYPH_UNKNOWN);
        }
        match ch {
            0x1B => self.esc_state = 1,
            b'\n' | 0x0B | 0x0C => {
                if self.newline_mode {
                    self.cursor_x = 0;
                }
                self.linefeed()
            }
            b'\r' => {
                self.cursor_x = 0;
                self.wrap_pending = false;
            }
            b'\t' => {
                self.cursor_x = ((self.cursor_x + 8) & !7).min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x08 => {
                // Backspace moves left only; programs erase with "\b \b"
                if self.wrap_pending {
                    self.wrap_pending = false;
                } else if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            0x20..=0x7E => self.put_glyph(ch as u16),
            0xC2..=0xDF => {
                self.utf8_cp = (ch & 0x1F) as u32;
                self.utf8_left = 1;
            }
            0xE0..=0xEF => {
                self.utf8_cp = (ch & 0x0F) as u32;
                self.utf8_left = 2;
            }
            0xF0..=0xF4 => {
                self.utf8_cp = (ch & 0x07) as u32;
                self.utf8_left = 3;
            }
            0x80..=0xFF => self.put_glyph(GLYPH_UNKNOWN),
            _ => {} // other control characters (BEL, ...)
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Saved { x: self.cursor_x, y: self.cursor_y, fg: self.fg, bg: self.bg, attr: self.attr };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved;
        self.cursor_x = s.x.min(self.cols - 1);
        self.cursor_y = s.y.min(self.rows - 1);
        self.fg = s.fg;
        self.bg = s.bg;
        self.attr = s.attr;
        self.wrap_pending = false;
    }

    fn after_esc(&mut self, ch: u8) {
        self.esc_state = 0;
        match ch {
            b'[' => {
                self.esc_state = 2;
                self.esc_params = [0; 16];
                self.esc_nparams = 0;
                self.esc_private = false;
            }
            b']' => self.esc_state = 3,
            b'(' | b')' => self.esc_state = 5,
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.linefeed(),
            b'E' => {
                self.cursor_x = 0;
                self.linefeed();
            }
            b'M' => {
                self.wrap_pending = false;
                if self.cursor_y == self.top {
                    self.scroll_down(1);
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
            }
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn reset(&mut self) {
        if let Some((grid, saved)) = self.alt_saved.take() {
            self.grid = grid;
            self.saved = saved;
        }
        self.fg = DEF_FG;
        self.bg = DEF_BG;
        self.attr = 0;
        self.newline_mode = true;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.autowrap = true;
        self.cursor_visible = true;
        self.erase(0, self.cols * self.rows);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.wrap_pending = false;
    }

    fn in_csi(&mut self, ch: u8) {
        match ch {
            b'0'..=b'9' => {
                if self.esc_nparams == 0 {
                    self.esc_nparams = 1;
                }
                let i = self.esc_nparams - 1;
                if i < self.esc_params.len() {
                    self.esc_params[i] = self.esc_params[i].saturating_mul(10).saturating_add((ch - b'0') as u16);
                }
            }
            b';' | b':' => {
                if self.esc_nparams == 0 {
                    self.esc_nparams = 1;
                }
                self.esc_nparams += 1;
            }
            b'?' | b'>' | b'=' => self.esc_private = true,
            0x20..=0x2F => {} // intermediate bytes: ignored
            0x40..=0x7E => {
                self.esc_state = 0;
                self.apply_csi(ch);
            }
            _ => self.esc_state = 0,
        }
    }

    /// Parameter i, with `default` for a missing or zero one
    fn param(&self, i: usize, default: u16) -> u16 {
        match self.esc_params.get(i) {
            Some(&v) if i < self.esc_nparams && v != 0 => v,
            _ => default,
        }
    }

    fn apply_csi(&mut self, cmd: u8) {
        let n = self.param(0, 1) as usize;
        let (cols, rows) = (self.cols, self.rows);
        if self.esc_private {
            if cmd == b'h' || cmd == b'l' {
                let on = cmd == b'h';
                for i in 0..self.esc_nparams.min(16) {
                    match self.esc_params[i] {
                        25 => self.cursor_visible = on,
                        7 => self.autowrap = on,
                        47 | 1047 | 1049 => self.alternate_screen(on),
                        _ => {}
                    }
                }
            }
            return;
        }
        self.wrap_pending = false;
        match cmd {
            b'h' | b'l' => {
                for i in 0..self.esc_nparams.min(16) {
                    if self.esc_params[i] == 20 {
                        self.newline_mode = cmd == b'h';
                    }
                }
            }
            b'A' => {
                let floor = if self.cursor_y >= self.top { self.top } else { 0 };
                self.cursor_y = self.cursor_y.saturating_sub(n).max(floor);
            }
            b'B' | b'e' => {
                let ceil = if self.cursor_y <= self.bottom { self.bottom } else { rows - 1 };
                self.cursor_y = (self.cursor_y + n).min(ceil);
            }
            b'C' | b'a' => self.cursor_x = (self.cursor_x + n).min(cols - 1),
            b'D' => self.cursor_x = self.cursor_x.saturating_sub(n),
            b'E' => {
                self.cursor_y = (self.cursor_y + n).min(rows - 1);
                self.cursor_x = 0;
            }
            b'F' => {
                self.cursor_y = self.cursor_y.saturating_sub(n);
                self.cursor_x = 0;
            }
            b'G' | b'`' => self.cursor_x = (n - 1).min(cols - 1),
            b'd' => self.cursor_y = (n - 1).min(rows - 1),
            b'H' | b'f' => {
                self.cursor_y = (self.param(0, 1) as usize - 1).min(rows - 1);
                self.cursor_x = (self.param(1, 1) as usize - 1).min(cols - 1);
            }
            b'J' => {
                let here = self.cursor_y * cols + self.cursor_x;
                match self.param(0, 0) {
                    0 => self.erase(here, cols * rows),
                    1 => self.erase(0, here + 1),
                    2 => self.erase(0, cols * rows),
                    3 => self.history_len = 0,
                    _ => {}
                }
            }
            b'K' => {
                let row = self.cursor_y * cols;
                match self.param(0, 0) {
                    0 => self.erase(row + self.cursor_x, row + cols),
                    1 => self.erase(row, row + self.cursor_x + 1),
                    2 => self.erase(row, row + cols),
                    _ => {}
                }
            }
            b'X' => {
                let at = self.cursor_y * cols + self.cursor_x;
                self.erase(at, at + n.min(cols - self.cursor_x));
            }
            b'L' | b'M' => {
                // Insert or delete lines at the cursor, inside the region
                if self.cursor_y >= self.top && self.cursor_y <= self.bottom {
                    let saved_top = self.top;
                    self.top = self.cursor_y;
                    if cmd == b'L' {
                        self.scroll_down(n);
                    } else {
                        self.scroll_up(n, false);
                    }
                    self.top = saved_top;
                    self.cursor_x = 0;
                }
            }
            b'@' | b'P' => {
                let row = self.cursor_y * cols;
                let (x, n) = (self.cursor_x, n.min(cols - self.cursor_x));
                if cmd == b'@' {
                    self.grid.copy_within(row + x..row + cols - n, row + x + n);
                    self.erase(row + x, row + x + n);
                } else {
                    self.grid.copy_within(row + x + n..row + cols, row + x);
                    self.erase(row + cols - n, row + cols);
                }
                self.dirty[self.cursor_y] = true;
            }
            b'S' => self.scroll_up(n, false),
            b'T' => self.scroll_down(n),
            b'r' => {
                let top = self.param(0, 1) as usize - 1;
                let bottom = (self.param(1, rows as u16) as usize).min(rows) - 1;
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                }
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'm' => self.sgr(),
            b'^' => self.scrollback(self.param(0, 0)),
            _ => {}
        }
    }

    fn sgr(&mut self) {
        if self.esc_nparams == 0 {
            self.fg = DEF_FG;
            self.bg = DEF_BG;
            self.attr = 0;
            return;
        }
        let get = |s: &Self, k: usize| if k < s.esc_nparams { s.esc_params.get(k).copied().unwrap_or(0) } else { 0 };
        let mut i = 0;
        while i < self.esc_nparams.min(16) {
            let p = self.esc_params[i];
            match p {
                0 => {
                    self.fg = DEF_FG;
                    self.bg = DEF_BG;
                    self.attr = 0;
                }
                1 => self.attr |= ATTR_BOLD,
                4 => self.attr |= ATTR_UNDERLINE,
                7 => self.attr |= ATTR_REVERSE,
                22 => self.attr &= !ATTR_BOLD,
                24 => self.attr &= !ATTR_UNDERLINE,
                27 => self.attr &= !ATTR_REVERSE,
                30..=37 => self.fg = p - 30,
                39 => self.fg = DEF_FG,
                40..=47 => self.bg = p - 40,
                49 => self.bg = DEF_BG,
                90..=97 => self.fg = p - 90 + 8,
                100..=107 => self.bg = p - 100 + 8,
                38 | 48 => {
                    let c = match get(self, i + 1) {
                        5 => {
                            let c = get(self, i + 2).min(255);
                            i += 2;
                            Some(c)
                        }
                        2 => {
                            let c = nearest(get(self, i + 2), get(self, i + 3), get(self, i + 4));
                            i += 4;
                            Some(c)
                        }
                        _ => None,
                    };
                    if let Some(c) = c {
                        if p == 38 {
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
            let blank = vec![BLANK; self.cols * self.rows];
            let main = core::mem::replace(&mut self.grid, blank);
            self.alt_saved = Some((main, self.saved));
            self.mark_all();
        } else if !on {
            if let Some((grid, saved)) = self.alt_saved.take() {
                self.grid = grid;
                self.saved = saved;
                self.restore_cursor();
                self.mark_all();
            }
        }
    }

    /// Rustux: move the view into the scrollback (1 page up, 2 page down,
    /// 0 back to the bottom)
    fn scrollback(&mut self, how: u16) {
        let page = self.rows.saturating_sub(1).max(1);
        let new = match how {
            1 => (self.view + page).min(self.history_len),
            2 => self.view.saturating_sub(page),
            _ => 0,
        };
        if new != self.view {
            self.view = new;
            self.mark_all();
        }
    }

    /// The cell shown at screen row `r`, column `c` (scrollback aware)
    fn shown(&self, r: usize, c: usize) -> Cell {
        if r >= self.view {
            return self.grid[(r - self.view) * self.cols + c];
        }
        // Row r of the view is history line (len - view + r), oldest first
        let oldest = (self.history_head + SCROLLBACK - self.history_len) % SCROLLBACK;
        let idx = (oldest + self.history_len - self.view + r) % SCROLLBACK;
        self.history[idx * self.cols + c]
    }

    /// Draw the dirty rows and the cursor
    pub fn flush(&mut self) {
        if let Some(r) = self.cursor_row_drawn.take() {
            if r < self.rows {
                self.dirty[r] = true;
            }
        }
        for r in 0..self.rows {
            if self.dirty[r] {
                self.dirty[r] = false;
                for c in 0..self.cols {
                    let cell = self.shown(r, c);
                    self.draw_cell(c, r, cell, false);
                }
            }
        }
        if self.cursor_visible && self.view == 0 {
            let cell = self.grid[self.cursor_y * self.cols + self.cursor_x];
            self.draw_cell(self.cursor_x, self.cursor_y, cell, true);
            self.cursor_row_drawn = Some(self.cursor_y);
        }
    }

    fn draw_cell(&mut self, col: usize, row: usize, cell: Cell, cursor: bool) {
        let (w, h) = (SimpleVgaFont::width(), SimpleVgaFont::height());
        let mut fg = cell.fg;
        if cell.attr & ATTR_BOLD != 0 && fg < 8 {
            fg += 8;
        }
        let (mut fgc, mut bgc) = (palette(fg), palette(cell.bg));
        if cell.attr & ATTR_REVERSE != 0 {
            core::mem::swap(&mut fgc, &mut bgc);
        }
        let (x0, y0) = (col * w, row * h);
        let fb = &mut self.framebuffer;
        let fast = fb.bpp == 32;
        let pack = |c: Color| -> u32 {
            match fb.format {
                PixelFormat::BGR => (c.b as u32) << 16 | (c.g as u32) << 8 | c.r as u32 | 0xFF00_0000,
                _ => (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32 | 0xFF00_0000,
            }
        };
        let (pf, pb) = (pack(fgc), pack(bgc));
        for py in 0..h {
            let y = y0 + py;
            if y >= fb.height {
                break;
            }
            let underline = cell.attr & ATTR_UNDERLINE != 0 && py == h - 2;
            let cursor_line = cursor && py >= h - 3;
            for px in 0..w {
                let x = x0 + px;
                if x >= fb.width {
                    break;
                }
                let on = (underline || glyph_pixel(cell.glyph, px, py)) != cursor_line;
                if fast {
                    unsafe {
                        let p = (fb.base_addr as *mut u8).add(y * fb.pitch + x * 4) as *mut u32;
                        core::ptr::write_volatile(p, if on { pf } else { pb });
                    }
                } else {
                    unsafe { fb.put_pixel(x, y, if on { fgc } else { bgc }) };
                }
            }
        }
    }

    pub fn write_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.put_char(b);
        }
        self.flush();
    }
}

// ---------------------------------------------------------------------
// Glyphs beyond ASCII
// ---------------------------------------------------------------------

/// Glyph ids: 0-127 from the font; from 0x100 drawn here
const GLYPH_UNKNOWN: u16 = 0x1FF;
const G_BOX: u16 = 0x100; // + segments: up 1, down 2, left 4, right 8; double +16
const G_FULL: u16 = 0x140;
const G_UPPER: u16 = 0x141;
const G_LOWER: u16 = 0x142;
const G_LEFT: u16 = 0x143;
const G_RIGHT: u16 = 0x144;
const G_SHADE1: u16 = 0x145;
const G_SHADE2: u16 = 0x146;
const G_SHADE3: u16 = 0x147;
const G_CHECK: u16 = 0x150;
const G_CROSS: u16 = 0x151;
const G_BULLET: u16 = 0x152;
const G_DEGREE: u16 = 0x153;
const G_MIDDOT: u16 = 0x154;
const G_ARROW_L: u16 = 0x155;
const G_ARROW_R: u16 = 0x156;
const G_ARROW_U: u16 = 0x157;
const G_ARROW_D: u16 = 0x158;
const G_TRI_R: u16 = 0x159;
const G_ELLIPSIS: u16 = 0x15A;
const G_SHARP_S: u16 = 0x15B;
/// An ASCII letter (low 8 bits) with an accent (bits 8-11) drawn over it
const G_ACCENTED: u16 = 0x1000;
const ACC_DIAERESIS: u16 = 1;
const ACC_ACUTE: u16 = 2;
const ACC_GRAVE: u16 = 3;
const ACC_CIRCUMFLEX: u16 = 4;
const ACC_TILDE: u16 = 5;
const ACC_RING: u16 = 6;
const ACC_CEDILLA: u16 = 7;
const ACC_STROKE: u16 = 8;

/// The glyph for an accented Latin-1 letter (À-ÿ), if it is one
fn latin1_accent(cp: u32) -> Option<u16> {
    // For each code point from 0xC0: base letter and accent (0: none)
    const T: [(u8, u16); 64] = [
        (b'A', ACC_GRAVE), (b'A', ACC_ACUTE), (b'A', ACC_CIRCUMFLEX), (b'A', ACC_TILDE),
        (b'A', ACC_DIAERESIS), (b'A', ACC_RING), (0, 0), (b'C', ACC_CEDILLA),
        (b'E', ACC_GRAVE), (b'E', ACC_ACUTE), (b'E', ACC_CIRCUMFLEX), (b'E', ACC_DIAERESIS),
        (b'I', ACC_GRAVE), (b'I', ACC_ACUTE), (b'I', ACC_CIRCUMFLEX), (b'I', ACC_DIAERESIS),
        (0, 0), (b'N', ACC_TILDE), (b'O', ACC_GRAVE), (b'O', ACC_ACUTE),
        (b'O', ACC_CIRCUMFLEX), (b'O', ACC_TILDE), (b'O', ACC_DIAERESIS), (0, 0),
        (b'O', ACC_STROKE), (b'U', ACC_GRAVE), (b'U', ACC_ACUTE), (b'U', ACC_CIRCUMFLEX),
        (b'U', ACC_DIAERESIS), (b'Y', ACC_ACUTE), (0, 0), (0, 0),
        (b'a', ACC_GRAVE), (b'a', ACC_ACUTE), (b'a', ACC_CIRCUMFLEX), (b'a', ACC_TILDE),
        (b'a', ACC_DIAERESIS), (b'a', ACC_RING), (0, 0), (b'c', ACC_CEDILLA),
        (b'e', ACC_GRAVE), (b'e', ACC_ACUTE), (b'e', ACC_CIRCUMFLEX), (b'e', ACC_DIAERESIS),
        (b'i', ACC_GRAVE), (b'i', ACC_ACUTE), (b'i', ACC_CIRCUMFLEX), (b'i', ACC_DIAERESIS),
        (0, 0), (b'n', ACC_TILDE), (b'o', ACC_GRAVE), (b'o', ACC_ACUTE),
        (b'o', ACC_CIRCUMFLEX), (b'o', ACC_TILDE), (b'o', ACC_DIAERESIS), (0, 0),
        (b'o', ACC_STROKE), (b'u', ACC_GRAVE), (b'u', ACC_ACUTE), (b'u', ACC_CIRCUMFLEX),
        (b'u', ACC_DIAERESIS), (b'y', ACC_ACUTE), (0, 0), (b'y', ACC_DIAERESIS),
    ];
    let (base, acc) = *T.get((cp as usize).wrapping_sub(0xC0))?;
    (acc != 0).then(|| G_ACCENTED | acc << 8 | base as u16)
}

/// A pixel of an accented letter: the accent sits in the two rows above
/// the letter (rows 2-3 for lower case, 0-1 for capitals), a cedilla
/// below it, a stroke across it
fn accented_pixel(g: u16, x: usize, y: usize) -> bool {
    let base = (g & 0xFF) as u8;
    let acc = (g >> 8) & 0xF;
    let top = if base.is_ascii_uppercase() { 0 } else { 2 };
    let (xi, yi) = (x as i32, y as i32);
    let mark = match acc {
        ACC_CEDILLA => (y == 12 && x == 3) || (y == 13 && (x == 2 || x == 3)),
        ACC_STROKE => (3..=12).contains(&yi) && xi == 12 - yi - if base.is_ascii_uppercase() { 0 } else { 1 },
        _ if y == top || y == top + 1 => {
            let r = y - top; // 0 upper row, 1 lower row
            match acc {
                ACC_DIAERESIS => r == 1 && (x == 1 || x == 2 || x == 4 || x == 5),
                ACC_ACUTE => if r == 0 { x == 4 || x == 5 } else { x == 3 || x == 4 },
                ACC_GRAVE => if r == 0 { x == 1 || x == 2 } else { x == 2 || x == 3 },
                ACC_CIRCUMFLEX => if r == 0 { x == 2 || x == 3 } else { x == 1 || x == 4 },
                ACC_TILDE => if r == 0 { x == 1 || x == 2 || x == 5 } else { x == 0 || x == 3 || x == 4 },
                ACC_RING => x == 2 || x == 3,
                _ => false,
            }
        }
        _ => false,
    };
    // An i loses its dot under the accent
    let letter = if base == b'i' && y < 5 { false } else { SimpleVgaFont::glyph_pixel(base, x, y) };
    letter || mark
}

const SHARP_S: [u8; 16] = [0, 0, 0x78, 0xCC, 0xCC, 0xD8, 0xD8, 0xCC, 0xC6, 0xC6, 0xC6, 0xDC, 0, 0, 0, 0];

/// Map a Unicode code point to a glyph
fn glyph_for(cp: u32) -> u16 {
    const U: u16 = 1;
    const D: u16 = 2;
    const L: u16 = 4;
    const R: u16 = 8;
    const DBL: u16 = 16;
    let b = |segs: u16| G_BOX + segs;
    match cp {
        0x20..=0x7E => cp as u16,
        0xA0 => b' ' as u16,
        // Box drawing: light, heavy (drawn double), rounded, double
        0x2500 => b(L | R),
        0x2501 => b(L | R | DBL),
        0x2502 => b(U | D),
        0x2503 => b(U | D | DBL),
        0x250C..=0x250F => b(D | R),
        0x2510..=0x2513 => b(D | L),
        0x2514..=0x2517 => b(U | R),
        0x2518..=0x251B => b(U | L),
        0x251C..=0x2523 => b(U | D | R),
        0x2524..=0x252B => b(U | D | L),
        0x252C..=0x2533 => b(D | L | R),
        0x2534..=0x253B => b(U | L | R),
        0x253C..=0x254B => b(U | D | L | R),
        0x2574 => b(L),
        0x2575 => b(U),
        0x2576 => b(R),
        0x2577 => b(D),
        0x256D => b(D | R),
        0x256E => b(D | L),
        0x256F => b(U | L),
        0x2570 => b(U | R),
        0x2550 => b(L | R | DBL),
        0x2551 => b(U | D | DBL),
        0x2552..=0x2554 => b(D | R | DBL),
        0x2555..=0x2557 => b(D | L | DBL),
        0x2558..=0x255A => b(U | R | DBL),
        0x255B..=0x255D => b(U | L | DBL),
        0x255E..=0x2560 => b(U | D | R | DBL),
        0x2561..=0x2563 => b(U | D | L | DBL),
        0x2564..=0x2566 => b(D | L | R | DBL),
        0x2567..=0x2569 => b(U | L | R | DBL),
        0x256A..=0x256C => b(U | D | L | R | DBL),
        0x2588 => G_FULL,
        0x2580 => G_UPPER,
        0x2584 => G_LOWER,
        0x258C => G_LEFT,
        0x2590 => G_RIGHT,
        0x2591 => G_SHADE1,
        0x2592 => G_SHADE2,
        0x2593 => G_SHADE3,
        0x2713 | 0x2714 | 0x2705 => G_CHECK,
        0x2717 | 0x2718 | 0x274C | 0xD7 => G_CROSS,
        0x2022 | 0x25CF | 0x2219 => G_BULLET,
        0xB0 => G_DEGREE,
        0xB7 | 0x22C5 => G_MIDDOT,
        0x2190 => G_ARROW_L,
        0x2192 => G_ARROW_R,
        0x2191 => G_ARROW_U,
        0x2193 => G_ARROW_D,
        0x25B6 | 0x25BA | 0x276F => G_TRI_R,
        0x2026 => G_ELLIPSIS,
        // Quotes and dashes
        0x2018 | 0x2019 | 0x201A | 0x2032 => b'\'' as u16,
        0x201C | 0x201D | 0x201E | 0x2033 => b'"' as u16,
        0x2010..=0x2015 | 0x2212 => b'-' as u16,
        0xAB => b'<' as u16,
        0xBB => b'>' as u16,
        // Latin-1 letters: the ASCII letter with an accent drawn over it
        0xC0..=0xFF if latin1_accent(cp).is_some() => latin1_accent(cp).unwrap(),
        0xDF => G_SHARP_S,
        0xC6 => b'A' as u16,
        0xE6 => b'a' as u16,
        0xD0 => b'D' as u16,
        0xF0 => b'd' as u16,
        0xDE => b'P' as u16,
        0xFE => b'p' as u16,
        0xF7 => b'/' as u16,
        // Other characters the keyboard layouts type: the nearest ASCII
        0xA3 => b'L' as u16,
        0xA6 => b'|' as u16,
        0xA7 => b'S' as u16,
        0xAC => b'-' as u16,
        0xB2 => b'2' as u16,
        0xB3 => b'3' as u16,
        0xB4 => b'\'' as u16,
        0xB5 => b'u' as u16,
        0x20AC => b'E' as u16,
        _ => GLYPH_UNKNOWN,
    }
}

fn glyph_pixel(g: u16, x: usize, y: usize) -> bool {
    let (w, h) = (SimpleVgaFont::width(), SimpleVgaFont::height());
    let (cx, cy) = (w / 2 - 1, h / 2 - 1); // centre: (3, 7)
    let (xi, yi) = (x as i32, y as i32);
    match g {
        0..=0xFF => SimpleVgaFont::glyph_pixel(g as u8, x, y),
        G_BOX..=0x11F => {
            let s = g - G_BOX;
            let dbl = s & 16 != 0;
            let on_v = |x: usize| if dbl { x == cx - 1 || x == cx + 1 } else { x == cx };
            let on_h = |y: usize| if dbl { y == cy - 1 || y == cy + 1 } else { y == cy };
            let reach = if dbl { 1 } else { 0 };
            (s & 1 != 0 && on_v(x) && y <= cy + reach)
                || (s & 2 != 0 && on_v(x) && y + reach >= cy)
                || (s & 4 != 0 && on_h(y) && x <= cx + reach)
                || (s & 8 != 0 && on_h(y) && x + reach >= cx)
        }
        G_FULL => true,
        G_UPPER => y < h / 2,
        G_LOWER => y >= h / 2,
        G_LEFT => x < w / 2,
        G_RIGHT => x >= w / 2,
        G_SHADE1 => (x + y * 2) % 4 == 0,
        G_SHADE2 => (x + y) % 2 == 0,
        G_SHADE3 => (x + y * 2) % 4 != 0,
        G_CHECK => (7..=11).contains(&yi) && (yi - xi == 6 || yi - xi == 5) || (3..=11).contains(&yi) && (xi + yi == 13 || xi + yi == 14),
        G_CROSS => (4..12).contains(&yi) && (xi == yi - 4 || xi == 11 - yi),
        G_BULLET => (xi - 3) * (xi - 3) + (yi - 7) * (yi - 7) <= 5,
        G_DEGREE => (2..=5).contains(&((xi - 3) * (xi - 3) + (yi - 4) * (yi - 4))),
        G_MIDDOT => (3..=4).contains(&x) && (7..=8).contains(&y),
        G_ARROW_R => y == 7 || (4..=7).contains(&xi) && (yi - 7).abs() == 7 - xi,
        G_ARROW_L => y == 7 || (0..=3).contains(&xi) && (yi - 7).abs() == xi,
        G_ARROW_U => x == 3 && (2..=13).contains(&y) || (2..=5).contains(&yi) && (xi - 3).abs() == yi - 2,
        G_ARROW_D => x == 3 && (2..=13).contains(&y) || (10..=13).contains(&yi) && (xi - 3).abs() == 13 - yi,
        G_TRI_R => (2..=6).contains(&xi) && xi - 2 <= 4 - (yi - 7).abs(),
        G_ELLIPSIS => y == 11 && (x == 0 || x == 3 || x == 6),
        G_SHARP_S => x < 8 && SHARP_S[y] & (0x80 >> x) != 0,
        G_ACCENTED..=0x1FFF => accented_pixel(g, x, y),
        _ => (2..=12).contains(&y) && (1..=6).contains(&x) && (y == 2 || y == 12 || x == 1 || x == 6),
    }
}

// ---------------------------------------------------------------------
// The global console
// ---------------------------------------------------------------------

/// Initialize the global text console
///
/// # Safety
/// This function must be called only once during kernel initialization,
/// after the framebuffer and the heap have been set up.
pub unsafe fn init(framebuffer: Framebuffer) {
    let mut console = TextConsole::new(framebuffer);
    console.clear();
    *CONSOLE.lock() = Some(console);
    CONSOLE_INITIALIZED.store(true, Ordering::Release);
}

/// Check if the console has been initialized
pub fn is_initialized() -> bool {
    CONSOLE_INITIALIZED.load(Ordering::Acquire)
}

/// Console size in characters (columns, rows)
pub fn size() -> (usize, usize) {
    (COLS.load(Ordering::Relaxed), ROWS.load(Ordering::Relaxed))
}

/// Write a string to the console
pub fn write_str(s: &str) {
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.write_str(s);
    }
}

/// Write a string unless the console is busy (for the panic path, which
/// may have interrupted this CPU in the middle of a console write)
pub fn try_write_str(s: &str) {
    if let Some(mut c) = CONSOLE.try_lock() {
        if let Some(console) = c.as_mut() {
            console.write_str(s);
        }
    }
}

/// Write raw bytes to the console (one lock for the whole buffer, so
/// writes from different CPUs don't interleave mid-line)
pub fn write_bytes(b: &[u8]) {
    if let Some(console) = CONSOLE.lock().as_mut() {
        for &c in b {
            console.put_char(c);
        }
        console.flush();
    }
}

/// Write a single character to the console
pub fn put_char(ch: u8) {
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.put_char(ch);
        console.flush();
    }
}

/// Clear the console
pub fn clear() {
    if let Some(console) = CONSOLE.lock().as_mut() {
        console.clear();
    }
}

/// Kept for compatibility; colours are set with escape sequences
pub fn set_color(_fg: Color, _bg: Color) {}

/// The current colours
pub fn get_color() -> (Color, Color) {
    match CONSOLE.lock().as_ref() {
        Some(console) => (console.fg_color(), console.bg_color()),
        None => (DRACULA_FG, DRACULA_BG),
    }
}

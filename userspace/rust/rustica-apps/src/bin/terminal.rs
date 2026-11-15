// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! terminal: Rustica's Terminal (DESKTOP.md step F7). The shell (or a
//! program) on a pseudo-terminal in a window, with the text console's
//! VT100/xterm emulation (rustica_apps::vt), colours and a scrollback;
//! tabs; copy and paste; font size.
//!
//!   terminal [-e PROGRAM [ARGS...]]
//!
//! Keys: Ctrl+Shift+T new tab, Ctrl+Shift+W close it, Ctrl+PgUp/PgDn
//! switch; Ctrl+Shift+C copy the selection (made with the mouse),
//! Ctrl+Shift+V or the middle button paste; Ctrl+= / Ctrl+- / Ctrl+0 font
//! size; Shift+PgUp/PgDn and the wheel scroll back. Everything else goes
//! to the program. A tab closes when its program ends; the window when
//! the last tab does.

use rustica_apps::pty::Pty;
use rustica_apps::vt::{self, Cell, Color, Term};
use rustica_ui::gfx::{Canvas, Face, Rect};
use rustica_ui::rustica_window::{Event, Waker, Window};
use std::io::Read;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

const FONT: &str = "/usr/share/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf";
const FONT_BOLD: &str = "/usr/share/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf";

const BG: u32 = 0x282A36;
const FG: u32 = 0xF8F8F2;
const BAR: u32 = 0x21222C;
const TAB: u32 = 0x343746;
const TAB_ON: u32 = 0x44475A;
const ACCENT: u32 = 0xBD93F9;
const DIM_TEXT: u32 = 0x9EA3BD;
const SELECTION: u32 = 0x5A4D80;

const MOD_CTRL: u32 = 0x11;
const MOD_SHIFT: u32 = 0x22;

struct Shared {
    data: Vec<u8>,
    ended: bool,
}

struct Tab {
    term: Term,
    pty: Pty,
    child: Option<Child>,
    shared: Arc<Mutex<Shared>>,
    /// Lines scrolled back (0: the bottom)
    view: usize,
    ended: bool,
    number: usize,
}

impl Tab {
    fn label(&self) -> String {
        if self.term.title.is_empty() {
            format!("Terminal {}", self.number)
        } else {
            self.term.title.clone()
        }
    }
}

fn open_tab(cmd: &[String], cols: usize, rows: usize, waker: Waker, number: usize) -> Result<Tab, String> {
    let pty = Pty::open().map_err(|e| format!("posix_openpt: {e}"))?;
    pty.set_size(cols, rows);
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
    let child = pty.spawn(&mut c).map_err(|e| format!("{}: {e}", cmd[0]))?;
    drop(c);
    let shared = Arc::new(Mutex::new(Shared { data: Vec::new(), ended: false }));
    let mut reader = pty.reader().map_err(|e| e.to_string())?;
    let sh = shared.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16384];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let mut s = sh.lock().unwrap();
                    let first = s.data.is_empty();
                    s.data.extend_from_slice(&buf[..n]);
                    drop(s);
                    if first {
                        waker.wake();
                    }
                }
                _ => {
                    sh.lock().unwrap().ended = true;
                    waker.wake();
                    return;
                }
            }
        }
    });
    Ok(Tab { term: Term::new(cols, rows), pty, child: Some(child), shared, view: 0, ended: false, number })
}

struct App {
    win: Window,
    canvas: Canvas,
    face: Face,
    bold: Face,
    scale: f32,
    size: f32, // font size in units
    cw: f32,
    ch: f32,
    tabs: Vec<Tab>,
    cur: usize,
    next_number: usize,
    cmd: Vec<String>,
    mouse: (f32, f32),
    buttons: u32,
    /// The selection: (line, column) from the oldest scrollback line
    sel: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    dirty: bool,
    focused: bool,
}

impl App {
    fn u(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }
    fn px(&self) -> f32 {
        self.size * self.scale
    }
    fn bar_h(&self) -> f32 {
        self.u(30.0)
    }
    fn pad(&self) -> f32 {
        self.u(6.0)
    }

    fn metrics(&mut self) {
        let px = self.px();
        self.cw = self.face.advances("M", px)[0].round().max(1.0);
        self.ch = self.face.line(px).2.ceil().max(1.0);
    }

    fn grid(&self) -> (usize, usize) {
        let w = self.win.width() as f32 - 2.0 * self.pad();
        let h = self.win.height() as f32 - self.bar_h() - 2.0 * self.pad();
        (((w / self.cw).floor() as usize).max(2), ((h / self.ch).floor() as usize).max(2))
    }

    fn relayout(&mut self) {
        let (cols, rows) = self.grid();
        for t in self.tabs.iter_mut() {
            if t.term.cols != cols || t.term.rows != rows {
                t.term.resize(cols, rows);
                t.pty.set_size(cols, rows);
            }
        }
        self.dirty = true;
    }

    fn new_tab(&mut self) {
        let (cols, rows) = self.grid();
        self.next_number += 1;
        match open_tab(&self.cmd, cols, rows, self.win.waker(), self.next_number) {
            Ok(t) => {
                self.tabs.push(t);
                self.cur = self.tabs.len() - 1;
            }
            Err(e) => eprintln!("terminal: {e}"),
        }
        self.sel = None;
        self.dirty = true;
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let mut t = self.tabs.remove(i);
        if let Some(mut c) = t.child.take() {
            let _ = c.kill();
        }
        if self.cur >= self.tabs.len() && self.cur > 0 {
            self.cur -= 1;
        } else if i < self.cur {
            self.cur -= 1;
        }
        self.sel = None;
        self.dirty = true;
    }

    fn set_font(&mut self, size: f32) {
        self.size = size.clamp(8.0, 32.0);
        self.metrics();
        self.relayout();
    }

    /// Take what the programs printed
    fn pump(&mut self) {
        let mut ended = Vec::new();
        for (i, t) in self.tabs.iter_mut().enumerate() {
            let (data, end) = {
                let mut s = t.shared.lock().unwrap();
                (std::mem::take(&mut s.data), s.ended)
            };
            if !data.is_empty() {
                t.term.feed(&data);
                if !t.term.replies.is_empty() {
                    let r = std::mem::take(&mut t.term.replies);
                    t.pty.write(&r);
                }
                // Keep a scrolled-back view on the same lines
                if t.view > 0 {
                    t.view = (t.view + t.term.scrolled).min(t.term.scrollback.len());
                }
                t.term.scrolled = 0;
                if i == self.cur {
                    self.dirty = true;
                    if !self.selecting {
                        self.sel = None;
                    }
                }
            }
            if end && !t.ended {
                t.ended = true;
                ended.push(i);
            }
        }
        for i in ended.into_iter().rev() {
            self.close_tab(i);
        }
    }

    fn title(&self) -> String {
        match self.tabs.get(self.cur) {
            Some(t) if !t.term.title.is_empty() => format!("{} — Terminal", t.term.title),
            _ => "Terminal".to_string(),
        }
    }

    // ---- input ----

    fn key(&mut self, usage: u32, mods: u32, text: &str) {
        let ctrl = mods & MOD_CTRL != 0;
        let shift = mods & MOD_SHIFT != 0;
        let n = self.tabs.len();
        if ctrl && shift {
            match usage {
                0x17 => return self.new_tab(),                          // T
                0x1A => return self.close_tab(self.cur),                // W
                0x06 => return self.copy(),                             // C
                0x19 => return self.paste(),                            // V
                _ => {}
            }
        }
        if ctrl && !shift {
            match usage {
                0x4B if n > 0 => {
                    self.cur = (self.cur + n - 1) % n;
                    self.dirty = true;
                    return;
                }
                0x4E if n > 0 => {
                    self.cur = (self.cur + 1) % n;
                    self.dirty = true;
                    return;
                }
                0x2E | 0x57 => return self.set_font(self.size + 1.0), // = (+), keypad +
                0x2D | 0x56 => return self.set_font(self.size - 1.0), // -, keypad -
                0x27 | 0x62 => return self.set_font(14.0),           // 0
                _ => {}
            }
        }
        let Some(t) = self.tabs.get_mut(self.cur) else { return };
        if shift && !ctrl && (usage == 0x4B || usage == 0x4E) && !t.term.alternate() {
            let page = t.term.rows.saturating_sub(1).max(1);
            t.view = if usage == 0x4B { (t.view + page).min(t.term.scrollback.len()) } else { t.view.saturating_sub(page) };
            self.dirty = true;
            return;
        }
        if !text.is_empty() {
            t.pty.write(text.as_bytes());
            if t.view != 0 {
                t.view = 0;
                self.dirty = true;
            }
            if self.sel.is_some() {
                self.sel = None;
                self.dirty = true;
            }
        }
    }

    fn copy(&mut self) {
        let Some(t) = self.tabs.get(self.cur) else { return };
        let Some((a, b)) = self.sel else { return };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let text = t.term.text(a, b);
        if !text.is_empty() {
            self.win.set_clipboard(&text);
        }
    }

    fn paste(&mut self) {
        let text = self.win.clipboard().unwrap_or_default();
        if let Some(t) = self.tabs.get_mut(self.cur) {
            // Lines end in CR, as Enter does
            let b: Vec<u8> = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
            t.pty.write(&b);
            t.view = 0;
            self.dirty = true;
        }
    }

    /// The cell under the pointer: (line from the oldest, column)
    fn cell_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let t = self.tabs.get(self.cur)?;
        let (x0, y0) = (self.pad(), self.bar_h() + self.pad());
        let col = ((x - x0) / self.cw).floor().max(0.0) as usize;
        let row = ((y - y0) / self.ch).floor().max(0.0) as usize;
        let row = row.min(t.term.rows - 1);
        let first = t.term.scrollback.len() - t.view;
        Some((first + row, col.min(t.term.cols - 1)))
    }

    /// The tab bar: Some(Ok(i)) a tab, Some(Err(i)) its close button,
    /// None for the new-tab button or nothing
    fn tab_rects(&mut self) -> (Vec<(Rect, Rect)>, Rect) {
        let s = self.scale;
        let bh = self.bar_h();
        let mut x = self.u(6.0);
        let mut out = Vec::new();
        let w = self.u(170.0).min(((self.win.width() as f32 - self.u(50.0)) / self.tabs.len().max(1) as f32).floor());
        for _ in &self.tabs {
            let r = Rect::new(x, self.u(4.0), w - self.u(4.0), bh - self.u(4.0));
            let close = Rect::new(r.right() - self.u(22.0), r.y + (r.h - self.u(18.0)) / 2.0, self.u(18.0), self.u(18.0));
            out.push((r, close));
            x += w;
        }
        let _ = s;
        let plus = Rect::new(x + self.u(2.0), self.u(4.0), self.u(28.0), bh - self.u(8.0));
        (out, plus)
    }

    fn pointer(&mut self, x: i32, y: i32, buttons: u32, wheel: i32) {
        let (x, y) = (x as f32, y as f32);
        self.mouse = (x, y);
        let pressed = buttons & !self.buttons;
        let released = self.buttons & !buttons;
        self.buttons = buttons;
        if wheel != 0 {
            if let Some(t) = self.tabs.get_mut(self.cur) {
                if !t.term.alternate() {
                    let v = t.view as i64 + wheel as i64 * 3;
                    t.view = v.clamp(0, t.term.scrollback.len() as i64) as usize;
                    self.dirty = true;
                }
            }
        }
        if y < self.bar_h() {
            if pressed & 1 != 0 {
                let (tabs, plus) = self.tab_rects();
                if plus.contains(x, y) {
                    self.new_tab();
                    return;
                }
                for (i, (r, close)) in tabs.iter().enumerate() {
                    if close.contains(x, y) {
                        self.close_tab(i);
                        return;
                    }
                    if r.contains(x, y) {
                        self.cur = i;
                        self.sel = None;
                        self.dirty = true;
                        return;
                    }
                }
            }
            return;
        }
        if pressed & 4 != 0 {
            self.paste();
        }
        if pressed & 1 != 0 {
            if let Some(c) = self.cell_at(x, y) {
                self.sel = Some((c, c));
                self.selecting = true;
                self.dirty = true;
            }
        } else if self.selecting && buttons & 1 != 0 {
            if let (Some(c), Some((a, _))) = (self.cell_at(x, y), self.sel) {
                self.sel = Some((a, c));
                self.dirty = true;
            }
        }
        if released & 1 != 0 && self.selecting {
            self.selecting = false;
            if let Some((a, b)) = self.sel {
                if a == b {
                    self.sel = None; // a click, not a selection
                    self.dirty = true;
                }
            }
        }
    }

    // ---- drawing ----

    fn color(c: Color, fg: bool) -> u32 {
        match c {
            Color::Default => {
                if fg {
                    FG
                } else {
                    BG
                }
            }
            Color::Index(i) => vt::palette(i),
            Color::Rgb(v) => v,
        }
    }

    fn draw(&mut self) {
        let (w, h) = (self.win.width(), self.win.height());
        if self.canvas.pixmap.width() != w || self.canvas.pixmap.height() != h {
            self.canvas = Canvas::new(w, h);
        }
        self.canvas.clear(BG);
        // The tab bar
        let bh = self.bar_h();
        self.canvas.fill_rect(Rect::new(0.0, 0.0, w as f32, bh), BAR);
        let (tabs, plus) = self.tab_rects();
        let ui_px = 13.0 * self.scale;
        let labels: Vec<String> = self.tabs.iter().map(|t| t.label()).collect();
        for (i, ((r, close), label)) in tabs.iter().zip(labels.iter()).enumerate() {
            let on = i == self.cur;
            self.canvas.fill_rounded(*r, self.u(6.0), if on { TAB_ON } else { TAB });
            if on {
                self.canvas.fill_rect(Rect::new(r.x + self.u(8.0), r.bottom() - self.u(2.0), r.w - self.u(16.0), self.u(2.0)), ACCENT);
            }
            let ty = r.y + (r.h - self.face.line(ui_px).2) / 2.0;
            let (clip_w, tx) = (close.x - r.x - self.u(4.0), r.x + self.u(10.0));
            self.canvas.push_clip(Rect::new(r.x, r.y, clip_w, r.h));
            self.canvas.text(&mut self.face, tx, ty, label, ui_px, if on { FG } else { DIM_TEXT });
            self.canvas.pop_clip();
            let hover = close.contains(self.mouse.0, self.mouse.1);
            if hover {
                self.canvas.fill_rounded(*close, self.u(4.0), 0x55596E);
            }
            let (cx, cy, d) = (close.x + close.w / 2.0, close.y + close.h / 2.0, self.u(4.0));
            let t = self.u(1.5).max(1.0);
            for k in 0..(2.0 * d) as i32 {
                let k = k as f32;
                self.canvas.fill_rect(Rect::new(cx - d + k, cy - d + k, t, t), DIM_TEXT);
                self.canvas.fill_rect(Rect::new(cx + d - k - t, cy - d + k, t, t), DIM_TEXT);
            }
        }
        let (cx, cy, d, t) = (plus.x + plus.w / 2.0, plus.y + plus.h / 2.0, self.u(6.0), self.u(2.0).max(1.0));
        self.canvas.fill_rect(Rect::new((cx - d).round(), (cy - t / 2.0).round(), 2.0 * d, t), DIM_TEXT);
        self.canvas.fill_rect(Rect::new((cx - t / 2.0).round(), (cy - d).round(), t, 2.0 * d), DIM_TEXT);

        // The terminal
        let Some(tab) = self.tabs.get(self.cur) else { return };
        let (x0, y0) = (self.pad(), bh + self.pad());
        let (cw, ch, px) = (self.cw, self.ch, self.px());
        let sb = tab.term.scrollback.len();
        let first = sb - tab.view.min(sb);
        let sel = self.sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let in_sel = |l: usize, c: usize| match sel {
            Some((a, b)) => (l, c) >= a && (l, c) <= b,
            None => false,
        };
        let mut glyphs: Vec<(f32, f32, char, u32, u32, bool, bool)> = Vec::new();
        for row in 0..tab.term.rows {
            let l = first + row;
            let Some(line) = tab.term.line(l) else { break };
            let y = y0 + row as f32 * ch;
            for (col, cell) in line.iter().enumerate().take(tab.term.cols) {
                let x = x0 + col as f32 * cw;
                let Cell { ch: c, fg, bg, attr } = *cell;
                let (mut fgc, mut bgc) = (Self::color(fg, true), Self::color(bg, false));
                if attr & vt::BOLD != 0 {
                    if let Color::Index(i) = fg {
                        if i < 8 {
                            fgc = vt::palette(i + 8);
                        }
                    }
                }
                if attr & vt::REVERSE != 0 {
                    std::mem::swap(&mut fgc, &mut bgc);
                }
                if attr & vt::DIM != 0 {
                    fgc = (fgc >> 1) & 0x7F7F7F | (bgc >> 1) & 0x7F7F7F;
                }
                if in_sel(l, col) {
                    bgc = SELECTION;
                }
                let cursor = tab.view == 0 && tab.term.cursor_visible && row == tab.term.cy && col == tab.term.cx && !tab.ended;
                if cursor && self.focused {
                    std::mem::swap(&mut fgc, &mut bgc);
                    if bgc == BG {
                        bgc = FG;
                    }
                }
                if bgc != BG {
                    self.canvas.fill_rect(Rect::new(x, y, cw, ch), bgc);
                }
                if cursor && !self.focused {
                    let t = self.u(1.0).max(1.0);
                    self.canvas.fill_rect(Rect::new(x, y, cw, t), FG);
                    self.canvas.fill_rect(Rect::new(x, y + ch - t, cw, t), FG);
                    self.canvas.fill_rect(Rect::new(x, y, t, ch), FG);
                    self.canvas.fill_rect(Rect::new(x + cw - t, y, t, ch), FG);
                }
                if c != ' ' {
                    glyphs.push((x, y, c, fgc, bgc, attr & vt::BOLD != 0, attr & vt::UNDERLINE != 0));
                } else if attr & vt::UNDERLINE != 0 {
                    glyphs.push((x, y, ' ', fgc, bgc, false, true));
                }
            }
        }
        let mut buf = [0u8; 4];
        let scale = self.scale;
        for (x, y, c, color, bg, bold, underline) in glyphs {
            if c != ' ' && !draw_box(&mut self.canvas, c, x, y, cw, ch, color, bg, scale) {
                let f = if bold { &mut self.bold } else { &mut self.face };
                self.canvas.text(f, x, y, c.encode_utf8(&mut buf), px, color);
            }
            if underline {
                let t = self.u(1.0).max(1.0);
                self.canvas.fill_rect(Rect::new(x, (y + ch - 2.0 * t).round(), cw, t), color);
            }
        }
        // Where the scrollback is shown
        if tab.view > 0 {
            let total = (sb + tab.term.rows) as f32;
            let area = h as f32 - y0;
            let bar_h = (area * tab.term.rows as f32 / total).max(self.u(20.0));
            let top = y0 + (area - bar_h) * (first as f32 / sb.max(1) as f32);
            self.canvas.fill_rounded(Rect::new(w as f32 - self.u(6.0), top, self.u(4.0), bar_h), self.u(2.0), 0x6272A4);
        }
    }
}

/// Box drawing (U+2500-257F) and block (U+2580-259F) characters are
/// drawn as lines and rectangles filling the cell, so they join up
/// between rows and columns: (left, right, up, down) with 1 light, 2
/// heavy, 3 double
fn box_lines(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' => [1, 1, 0, 0],
        '━' => [2, 2, 0, 0],
        '│' => [0, 0, 1, 1],
        '┃' => [0, 0, 2, 2],
        '┌' | '╭' => [0, 1, 0, 1],
        '┐' | '╮' => [1, 0, 0, 1],
        '└' | '╰' => [0, 1, 1, 0],
        '┘' | '╯' => [1, 0, 1, 0],
        '├' => [0, 1, 1, 1],
        '┤' => [1, 0, 1, 1],
        '┬' => [1, 1, 0, 1],
        '┴' => [1, 1, 1, 0],
        '┼' => [1, 1, 1, 1],
        '┏' => [0, 2, 0, 2],
        '┓' => [2, 0, 0, 2],
        '┗' => [0, 2, 2, 0],
        '┛' => [2, 0, 2, 0],
        '┣' => [0, 2, 2, 2],
        '┫' => [2, 0, 2, 2],
        '┳' => [2, 2, 0, 2],
        '┻' => [2, 2, 2, 0],
        '╋' => [2, 2, 2, 2],
        '═' => [3, 3, 0, 0],
        '║' => [0, 0, 3, 3],
        '╔' => [0, 3, 0, 3],
        '╗' => [3, 0, 0, 3],
        '╚' => [0, 3, 3, 0],
        '╝' => [3, 0, 3, 0],
        '╠' => [0, 3, 3, 3],
        '╣' => [3, 0, 3, 3],
        '╦' => [3, 3, 0, 3],
        '╩' => [3, 3, 3, 0],
        '╬' => [3, 3, 3, 3],
        _ => return None,
    })
}

/// Draw a box drawing or block character; false if it is not one
fn draw_box(canvas: &mut Canvas, c: char, x: f32, y: f32, w: f32, h: f32, color: u32, bg: u32, scale: f32) -> bool {
    let full = |canvas: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32| {
        canvas.fill_rect(Rect::new(x0.round(), y0.round(), (x1.round() - x0.round()).max(1.0), (y1.round() - y0.round()).max(1.0)), color)
    };
    match c {
        '█' => return { full(canvas, x, y, x + w, y + h); true },
        '▀' => return { full(canvas, x, y, x + w, y + h / 2.0); true },
        '▄' => return { full(canvas, x, y + h / 2.0, x + w, y + h); true },
        '▌' => return { full(canvas, x, y, x + w / 2.0, y + h); true },
        '▐' => return { full(canvas, x + w / 2.0, y, x + w, y + h); true },
        '░' | '▒' | '▓' => {
            let a = match c { '░' => 1, '▒' => 2, _ => 3 };
            let mix = |f: u32, b: u32, s: u32| ((f >> s & 0xFF) * a + (b >> s & 0xFF) * (4 - a)) / 4 << s;
            let m = mix(color, bg, 16) | mix(color, bg, 8) | mix(color, bg, 0);
            canvas.fill_rect(Rect::new(x, y, w, h), m);
            return true;
        }
        _ => {}
    }
    let Some([l, r, u, d]) = box_lines(c) else { return false };
    let t1 = scale.round().max(1.0);
    let (cx, cy) = ((x + w / 2.0).floor(), (y + h / 2.0).floor());
    let g = t1; // half the gap of a double line
    // A horizontal piece from x0 to x1 and a vertical one from y0 to y1
    let hline = |canvas: &mut Canvas, kind: u8, x0: f32, x1: f32| match kind {
        1 => full(canvas, x0, cy, x1, cy + t1),
        2 => full(canvas, x0, cy - t1, x1, cy + 2.0 * t1),
        3 => {
            full(canvas, x0, cy - g - t1, x1, cy - g);
            full(canvas, x0, cy + g, x1, cy + g + t1);
        }
        _ => {}
    };
    let vline = |canvas: &mut Canvas, kind: u8, y0: f32, y1: f32| match kind {
        1 => full(canvas, cx, y0, cx + t1, y1),
        2 => full(canvas, cx - t1, y0, cx + 2.0 * t1, y1),
        3 => {
            full(canvas, cx - g - t1, y0, cx - g, y1);
            full(canvas, cx + g, y0, cx + g + t1, y1);
        }
        _ => {}
    };
    // Pieces meet in the middle (a double line reaches across the other's gap)
    let reach = if l == 3 || r == 3 || u == 3 || d == 3 { g + t1 } else { t1 };
    if l > 0 {
        hline(canvas, l, x, cx + if u > 0 || d > 0 { reach } else { t1 });
    }
    if r > 0 {
        hline(canvas, r, cx - if u > 0 || d > 0 { reach - t1 } else { 0.0 }, x + w);
    }
    if u > 0 {
        vline(canvas, u, y, cy + if l > 0 || r > 0 { reach } else { t1 });
    }
    if d > 0 {
        vline(canvas, d, cy - if l > 0 || r > 0 { reach - t1 } else { 0.0 }, y + h);
    }
    true
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = if args.len() >= 2 && args[0] == "-e" {
        args.drain(..1);
        args
    } else {
        vec!["/bin/shell".to_string()]
    };
    let scale = rustica_ui::app::scale_from_env();
    let (face, bold) = match (Face::load(FONT), Face::load(FONT_BOLD)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("terminal: {e}");
            std::process::exit(1);
        }
    };
    let size = 14.0;
    // 80 x 24 cells and the tab bar
    let mut probe = Face::load(FONT).unwrap();
    let cw = probe.advances("M", size * scale)[0].round();
    let ch = probe.line(size * scale).2.ceil();
    let w = (cw * 80.0 + 12.0 * scale) as u32;
    let h = (ch * 24.0 + 42.0 * scale) as u32;
    let win = match Window::new("Terminal", w, h, None) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("terminal: no window: {e}");
            std::process::exit(1);
        }
    };
    let mut app = App {
        win,
        canvas: Canvas::new(w, h),
        face,
        bold,
        scale,
        size,
        cw: 8.0,
        ch: 16.0,
        tabs: Vec::new(),
        cur: 0,
        next_number: 0,
        cmd,
        mouse: (-1.0, -1.0),
        buttons: 0,
        sel: None,
        selecting: false,
        dirty: true,
        focused: true,
    };
    app.metrics();
    app.new_tab();
    if app.tabs.is_empty() {
        std::process::exit(1);
    }
    let mut title = String::new();
    loop {
        app.pump();
        if app.tabs.is_empty() {
            return;
        }
        let t = app.title();
        if t != title {
            app.win.set_title(&t);
            title = t;
        }
        if app.dirty && !app.win.frame_pending() {
            app.draw();
            app.canvas.copy_to(app.win.pixels());
            app.win.commit(&[]);
            app.dirty = false;
        }
        let Some(ev) = app.win.next_event(None) else { return };
        match ev {
            Event::Configure { width, height } => {
                if app.win.resize(width, height).is_ok() {
                    app.relayout();
                }
            }
            Event::Key { usage, mods, pressed: true, text, .. } => app.key(usage, mods, &text),
            Event::Pointer { x, y, buttons, wheel, flags } => {
                if flags & rustica_ui::rustica_window::PTR_LEAVE != 0 {
                    app.mouse = (-1.0, -1.0);
                } else {
                    app.pointer(x, y, buttons, wheel);
                }
            }
            Event::Focus(f) => {
                app.focused = f;
                app.dirty = true;
            }
            Event::CloseRequest => {
                for i in (0..app.tabs.len()).rev() {
                    app.close_tab(i);
                }
                return;
            }
            _ => {}
        }
    }
}

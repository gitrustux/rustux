// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The widgets: an immediate-mode UI. The program describes its window
//! every frame (`ui.button("OK")` draws the button and says whether it was
//! clicked); the toolkit keeps the state that must outlive a frame (text
//! being edited, scroll positions, open tree nodes, the open menu), the
//! focus, and the layout (columns and rows).
//!
//! Keyboard: Tab and Shift+Tab move the focus; Enter or Space presses the
//! focused button; arrows move in lists, trees and tabs; F10 opens the
//! menu bar, Esc closes menus and dialogs. Everything works with the
//! mouse too. Sizes are in units that the scale (1x, 2x) multiplies.

use crate::gfx::{Canvas, Color, Face, Rect};
use std::collections::{HashMap, HashSet};

pub type Id = u64;

/// A colour theme (2026-11-25): the semantic tokens of a theme file,
/// `/usr/share/rustica/themes/NAME.theme` (Rustica, Dracula, Forest,
/// Arctic, Sakura) or a user's own `~/.config/rustica/themes/NAME.theme`,
/// `key=#RRGGBB` lines. Every program draws from these; the compositor
/// reads the same files (compositor.c, themefile.h) and sends a window the
/// theme's colours when it changes (`RX_WINDOW_EV_APPEARANCE`), and the
/// console takes the ANSI colours (`theme --console`).
///
/// Field names and their tokens: background `bg`, surface `surface`,
/// surface_hi `surface-raised`, surface_hover `surface-hover`, border
/// `border`, text `text`, dim `text-muted`, accent `primary`, accent_hover
/// `primary-hover`, secondary `accent`, selection `selection`, good
/// `success`, warning `warning`, danger `danger`, info `info`, taskbar
/// `taskbar`, ansi `ansi0`-`ansi15`. accent_text (text on the primary
/// colour) and highlight (the keyboard focus, primary-hover) are worked out.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Its id: the file's name without .theme
    pub name: &'static str,
    /// What people see (name= in the file)
    pub label: &'static str,
    pub background: Color,
    pub surface: Color,
    pub surface_hi: Color,
    pub surface_hover: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_text: Color,
    pub selection: Color,
    pub border: Color,
    pub danger: Color,
    pub good: Color,
    pub warning: Color,
    pub info: Color,
    /// The panel
    pub taskbar: Color,
    /// The keyboard focus and other highlights
    pub highlight: Color,
    /// The theme's second colour (its `accent` token: Rustica's fox red)
    pub secondary: Color,
    /// The 16 ANSI colours for terminals (normal, then bright)
    pub ansi: [Color; 16],
}

/// The tokens of a theme file, in their order
pub const TOKENS: [&str; 16] = [
    "bg", "surface", "surface-raised", "surface-hover", "border", "text", "text-muted", "primary", "primary-hover",
    "accent", "selection", "success", "warning", "danger", "info", "taskbar",
];

/// Rustica, the default (also built in, should its file be missing)
pub const RUSTICA: Theme = Theme {
    name: "rustica",
    label: "Rustica",
    background: 0x101010,
    surface: 0x202024,
    surface_hi: 0x2B292B,
    surface_hover: 0x383438,
    text: 0xFFFFFF,
    dim: 0x858585,
    accent: 0xFF6500,
    accent_hover: 0xFF812B,
    accent_text: 0xFFFFFF,
    selection: 0x57230F,
    border: 0x49302A,
    danger: 0xFF3333,
    good: 0x55C878,
    warning: 0xFFB347,
    info: 0x65BDEB,
    taskbar: 0x151416,
    highlight: 0xFF812B,
    secondary: 0xD71920,
    ansi: [
        0x2B292B, 0xFF3333, 0x55C878, 0xFFB347, 0x65BDEB, 0xD96BA0, 0x5FC4C4, 0xE8E2DC, //
        0x858585, 0xFF5A5F, 0x8BE0A4, 0xFFD08A, 0x8FD3F5, 0xF08CC0, 0x8AE0E0, 0xFFFFFF,
    ],
};

/// Dracula (built in too: the server console's default)
pub const DRACULA: Theme = Theme {
    name: "dracula",
    label: "Dracula",
    background: 0x282A36,
    surface: 0x343746,
    surface_hi: 0x44475A,
    surface_hover: 0x50546A,
    text: 0xF8F8F2,
    dim: 0xA6A9B8,
    accent: 0xBD93F9,
    accent_hover: 0xD0ADFF,
    accent_text: 0x282A36,
    selection: 0x51416F,
    border: 0x6272A4,
    danger: 0xFF5555,
    good: 0x50FA7B,
    warning: 0xF1FA8C,
    info: 0x8BE9FD,
    taskbar: 0x21232E,
    highlight: 0xD0ADFF,
    secondary: 0xFF79C6,
    ansi: [
        0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2, //
        0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF,
    ],
};

/// The built-in themes (their files are the ones used; these if missing)
pub const THEMES: [Theme; 2] = [RUSTICA, DRACULA];

/// The system's themes, in the order Settings shows them
pub const SYSTEM_THEMES: [&str; 5] = ["rustica", "dracula", "forest", "arctic", "sakura"];

/// Where the system's theme files are
pub const THEME_DIR: &str = "/usr/share/rustica/themes";

/// a and b mixed (k of b)
pub fn mix(a: Color, b: Color, k: f32) -> Color {
    let ch = |s: u32| -> u32 {
        let (x, y) = ((a >> s & 255) as f32, (b >> s & 255) as f32);
        ((x + (y - x) * k).round() as u32).min(255) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Relative luminance (WCAG)
pub fn luminance(c: Color) -> f32 {
    let f = |s: u32| {
        let v = (c >> s & 255) as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(16) + 0.7152 * f(8) + 0.0722 * f(0)
}

/// The contrast ratio of two colours (1 to 21)
pub fn contrast(a: Color, b: Color) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// A name a theme may have: lower-case letters, digits and -
pub fn valid_theme_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 15 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn leak(s: &str) -> &'static str {
    // Theme names are few; each distinct one is kept once
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut v = NAMES.lock().unwrap();
    if let Some(n) = v.iter().find(|n| **n == s) {
        return n;
    }
    let n: &'static str = Box::leak(s.to_string().into_boxed_str());
    v.push(n);
    n
}

fn hex(v: &str) -> Option<Color> {
    let v = v.trim().trim_start_matches('#');
    if v.len() != 6 {
        return None;
    }
    u32::from_str_radix(v, 16).ok()
}

impl Theme {
    /// A token's colour
    pub fn get(&self, token: &str) -> Option<Color> {
        Some(match token {
            "bg" => self.background,
            "surface" => self.surface,
            "surface-raised" => self.surface_hi,
            "surface-hover" => self.surface_hover,
            "border" => self.border,
            "text" => self.text,
            "text-muted" => self.dim,
            "primary" => self.accent,
            "primary-hover" => self.accent_hover,
            "accent" => self.secondary,
            "selection" => self.selection,
            "success" => self.good,
            "warning" => self.warning,
            "danger" => self.danger,
            "info" => self.info,
            "taskbar" => self.taskbar,
            _ => {
                let i: usize = token.strip_prefix("ansi")?.parse().ok()?;
                *self.ansi.get(i)?
            }
        })
    }

    /// Set a token's colour (unknown tokens are ignored); what follows from
    /// it is worked out again
    pub fn set(&mut self, token: &str, c: Color) {
        match token {
            "bg" => self.background = c,
            "surface" => self.surface = c,
            "surface-raised" => self.surface_hi = c,
            "surface-hover" => self.surface_hover = c,
            "border" => self.border = c,
            "text" => self.text = c,
            "text-muted" => self.dim = c,
            "primary" => self.accent = c,
            "primary-hover" => self.accent_hover = c,
            "accent" => self.secondary = c,
            "selection" => self.selection = c,
            "success" => self.good = c,
            "warning" => self.warning = c,
            "danger" => self.danger = c,
            "info" => self.info = c,
            "taskbar" => self.taskbar = c,
            _ => {
                if let Some(i) = token.strip_prefix("ansi").and_then(|n| n.parse::<usize>().ok()) {
                    if i < 16 {
                        self.ansi[i] = c;
                    }
                }
            }
        }
        self.derive();
    }

    /// What is worked out from the tokens: the text on the primary colour
    /// (white where it reads well enough, else the background) and the
    /// keyboard focus
    fn derive(&mut self) {
        self.accent_text = if contrast(0xFFFFFF, self.accent) >= 2.9 { 0xFFFFFF } else { self.background };
        self.highlight = self.accent_hover;
    }

    /// A theme from a file's text (`key=#RRGGBB`, `name=`); tokens it does
    /// not have come from Rustica
    pub fn parse(name: &str, text: &str) -> Theme {
        let mut t = RUSTICA;
        t.name = leak(name);
        t.label = t.name;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let k = k.trim();
            if k == "name" {
                let v = v.trim();
                if !v.is_empty() {
                    t.label = leak(&v.chars().take(32).collect::<String>());
                }
            } else if let Some(c) = hex(v) {
                t.set(k, c);
            }
        }
        t.derive();
        t
    }

    /// The theme as a file's text
    pub fn to_text(&self) -> String {
        let mut s = format!("name={}\n", self.label);
        for k in TOKENS {
            s += &format!("{k}=#{:06X}\n", self.get(k).unwrap_or(0));
        }
        for (i, c) in self.ansi.iter().enumerate() {
            s += &format!("ansi{i}=#{c:06X}\n");
        }
        s
    }
}

/// The user's own themes' folder
pub fn user_theme_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(|h| std::path::Path::new(&h).join(".config/rustica/themes"))
}

/// The theme of that name: the user's own, then the system's, then a
/// built-in one (Rustica for an unknown name)
pub fn theme_named(name: &str) -> Theme {
    let name = name.trim();
    if valid_theme_name(name) {
        let mut files = Vec::new();
        if let Some(d) = user_theme_dir() {
            files.push(d.join(format!("{name}.theme")));
        }
        files.push(std::path::Path::new(THEME_DIR).join(format!("{name}.theme")));
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                return Theme::parse(name, &text);
            }
        }
    }
    THEMES.iter().copied().find(|t| t.name == name).unwrap_or(RUSTICA)
}

/// The theme an appearance event names: its colours as sent, else by name
pub fn theme_from_event(name: &str, colors: &str) -> Theme {
    if colors.contains('=') {
        Theme::parse(name, colors)
    } else {
        theme_named(name)
    }
}

/// The themes there are: (id, label, the user's own), the system's first
pub fn list_themes() -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for n in SYSTEM_THEMES {
        let t = theme_named(n);
        out.push((n.to_string(), t.label.to_string(), false));
    }
    if let Some(d) = user_theme_dir() {
        let mut mine: Vec<String> = std::fs::read_dir(d)
            .map(|r| {
                r.flatten()
                    .filter_map(|e| e.file_name().to_str()?.strip_suffix(".theme").map(str::to_string))
                    .filter(|n| valid_theme_name(n))
                    .collect()
            })
            .unwrap_or_default();
        mine.sort();
        for n in mine {
            if let Some(i) = out.iter().position(|o| o.0 == n) {
                // A user's file of a system theme's name: theirs is used
                out[i].2 = true;
                out[i].1 = theme_named(&n).label.to_string();
            } else {
                let t = theme_named(&n);
                out.push((n, t.label.to_string(), true));
            }
        }
    }
    out
}

/// The theme to draw in: RUSTICA_THEME_COLORS (a theme file's text) with
/// RUSTICA_THEME (its name; appctl sets them for sandboxed apps), else
/// theme= in ~/.config/rustica.conf, else in /etc/rustica/appearance, else
/// Rustica. Windows then hear of changes from the compositor.
pub fn current_theme() -> Theme {
    if let Ok(t) = std::env::var("RUSTICA_THEME") {
        if let Ok(c) = std::env::var("RUSTICA_THEME_COLORS") {
            return Theme::parse(&t, &c.replace(';', "\n"));
        }
        return theme_named(&t);
    }
    let key = |path: String| -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        text.lines().find_map(|l| l.trim().strip_prefix("theme=").map(|v| v.trim().to_string()))
    };
    let user = std::env::var("HOME").ok().and_then(|h| key(format!("{h}/.config/rustica.conf")));
    theme_named(&user.or_else(|| key("/etc/rustica/appearance".into())).unwrap_or_default())
}

// HID usages
pub mod keys {
    pub const ENTER: u32 = 0x28;
    pub const ESC: u32 = 0x29;
    pub const BACKSPACE: u32 = 0x2A;
    pub const TAB: u32 = 0x2B;
    pub const SPACE: u32 = 0x2C;
    pub const F10: u32 = 0x43;
    pub const HOME: u32 = 0x4A;
    pub const PAGE_UP: u32 = 0x4B;
    pub const DELETE: u32 = 0x4C;
    pub const END: u32 = 0x4D;
    pub const PAGE_DOWN: u32 = 0x4E;
    pub const RIGHT: u32 = 0x4F;
    pub const LEFT: u32 = 0x50;
    pub const DOWN: u32 = 0x51;
    pub const UP: u32 = 0x52;
    pub const KP_ENTER: u32 = 0x58;
    pub const A: u32 = 0x04;
    pub const C: u32 = 0x06;
    pub const V: u32 = 0x19;
    pub const X: u32 = 0x1B;
}

const MOD_CTRL: u32 = 0x11;
const MOD_SHIFT: u32 = 0x22;

/// Small pictures for table rows
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    None,
    Folder,
    File,
    Image,
    Program,
    Drive,
    Trash,
    /// The trash with something in it
    TrashFull,
    Home,
    /// The power symbol (a ring open at the top, a bar through the gap)
    Power,
    /// The Rustica fox (the menu button; /usr/share/rustica/fox.svg)
    Fox,
}

/// The Rustica fox's outline and eyes, in a 261x254 box
/// (/usr/share/rustica/fox.svg)
pub const FOX_OUTLINE: [(f32, f32); 12] = [
    (38.0, 0.0), (105.0, 51.0), (156.0, 51.0), (223.0, 0.0), (223.0, 98.0), (261.0, 134.0),
    (181.0, 188.0), (140.0, 254.0), (121.0, 254.0), (80.0, 188.0), (0.0, 134.0), (38.0, 98.0),
];
pub const FOX_EYES: [[(f32, f32); 3]; 2] = [[(76.0, 124.0), (108.0, 129.0), (110.0, 156.0)], [(185.0, 124.0), (153.0, 129.0), (151.0, 156.0)]];
/// Its colour in every theme (the brand's orange)
pub const FOX_COLOR: Color = 0xFF6500;

/// The fox filled into a square of side sz at x, y
pub fn fox_path(x: f32, y: f32, sz: f32) -> Option<tiny_skia::Path> {
    let k = sz / 261.0;
    let oy = y + (sz - 254.0 * k) / 2.0;
    let mut pb = tiny_skia::PathBuilder::new();
    let mut poly = |pts: &[(f32, f32)]| {
        pb.move_to(x + pts[0].0 * k, oy + pts[0].1 * k);
        for p in &pts[1..] {
            pb.line_to(x + p.0 * k, oy + p.1 * k);
        }
        pb.close();
    };
    poly(&FOX_OUTLINE);
    for e in &FOX_EYES {
        poly(e);
    }
    pb.finish()
}

/// A row of a table: its icon and its cells, one per column
#[derive(Clone, Debug)]
pub struct Row {
    pub icon: Icon,
    pub cells: Vec<String>,
}

/// A column of a table: header, width in units (0: what is left), right-aligned
#[derive(Clone, Copy)]
pub struct Column<'a> {
    pub title: &'a str,
    pub width: f32,
    pub right: bool,
}

/// What happened to a table this frame
#[derive(Default, Debug)]
pub struct TableResponse {
    /// The selection changed
    pub changed: bool,
    /// A row was opened: double-click or Enter
    pub activated: Option<usize>,
    /// The right button went down on a row (it is selected)
    pub context: Option<usize>,
    /// Keys the table did not use while it had the focus (Delete, F2, Ctrl+C...)
    pub keys: Vec<Key>,
    /// The pointer is over the table's rows; the row under it, if any
    pub over: bool,
    pub hover: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Key {
    pub usage: u32,
    pub mods: u32,
    pub text: String,
}

impl Key {
    pub fn ctrl(&self) -> bool {
        self.mods & MOD_CTRL != 0
    }
    pub fn shift(&self) -> bool {
        self.mods & MOD_SHIFT != 0
    }
    /// Text to insert: printable, not a control character or escape sequence
    fn printable(&self) -> Option<&str> {
        if self.ctrl() || self.text.is_empty() || self.text.chars().any(|c| c.is_control()) {
            None
        } else {
            Some(&self.text)
        }
    }
}

/// What a [`Ui::button_ex`] saw
#[derive(Clone, Copy)]
pub struct ButtonResponse {
    pub clicked: bool,
    pub right_clicked: bool,
    pub rect: Rect,
}

/// What happened since the last frame
#[derive(Default, Clone)]
pub struct Input {
    pub mouse: (f32, f32),
    pub down: bool,
    /// The right button went down (this frame)
    pub right_pressed: bool,
    pub right_held: bool,
    pub pressed: bool,
    pub released: bool,
    pub wheel: f32,
    pub keys: Vec<Key>,
}

/// The clipboard, from the window system
pub trait Platform {
    fn clipboard_get(&mut self) -> String;
    fn clipboard_set(&mut self, text: &str);
}

#[derive(Default, Clone)]
struct TextState {
    cursor: usize, // chars
    anchor: usize,
    scroll: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Column,
    Row,
}

struct Layout {
    dir: Dir,
    x: f32,
    y: f32,
    width: f32, // a column's width
    start_x: f32,
    start_y: f32,
    max: f32, // a row's height, a column's widest
    /// Where the visible space ends for remaining_height (the window's
    /// bottom, or a scroll area's, in this layout's coordinates)
    bottom: f32,
}

struct OpenMenu {
    menu: usize,
    item: usize,
    rect: Rect,
    items: Vec<String>,
}

pub struct TreeNode {
    pub label: String,
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    pub fn leaf(label: &str) -> TreeNode {
        TreeNode { label: label.into(), children: Vec::new() }
    }
    pub fn branch(label: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode { label: label.into(), children }
    }
}

pub struct Ui {
    pub canvas: Canvas,
    pub face: Face,
    pub bold: Face,
    pub theme: Theme,
    pub scale: f32,
    pub input: Input,
    platform: Box<dyn Platform>,
    focus: Option<Id>,
    focus_order: Vec<Id>,
    focus_move: i32,
    keyboard_focus: bool, // show the focus ring (the focus came from the keyboard)
    active: Option<Id>,   // the widget the mouse button went down on
    layouts: Vec<Layout>,
    texts: HashMap<Id, TextState>,
    scrolls: HashMap<Id, f32>,
    heights: HashMap<Id, f32>,
    open_nodes: HashSet<Id>,
    selected_node: HashMap<Id, Id>,
    menu: Option<OpenMenu>,
    menu_was_open: bool,
    modal: bool,      // a dialog is up: other widgets get no input
    modal_now: bool,
    consumed: HashSet<usize>, // keys used this frame
    id_stack: Vec<Id>,
    pub redraw: bool, // something wants another frame (e.g. a scroll settled)
    last_click: Option<(Id, usize, std::time::Instant)>, // for double-clicks
    /// Around the window's content, in units (12)
    pub padding: f32,
    /// Start each frame transparent and send the alpha (the desktop role)
    pub transparent: bool,
}

fn hash(parent: Id, s: &str) -> Id {
    // FNV-1a
    let mut h: u64 = 0xcbf29ce484222325 ^ parent.wrapping_mul(0x100000001b3);
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn char_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

impl Ui {
    pub fn new(face: Face, bold: Face, scale: f32, platform: Box<dyn Platform>) -> Ui {
        Ui {
            canvas: Canvas::new(1, 1),
            face,
            bold,
            theme: current_theme(),
            scale,
            input: Input::default(),
            platform,
            focus: None,
            focus_order: Vec::new(),
            focus_move: 0,
            keyboard_focus: false,
            active: None,
            layouts: Vec::new(),
            texts: HashMap::new(),
            scrolls: HashMap::new(),
            heights: HashMap::new(),
            open_nodes: HashSet::new(),
            selected_node: HashMap::new(),
            menu: None,
            menu_was_open: false,
            modal: false,
            modal_now: false,
            consumed: HashSet::new(),
            id_stack: vec![0],
            redraw: false,
            last_click: None,
            padding: 12.0,
            transparent: false,
        }
    }

    // ---- units ----

    pub fn u(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }
    pub fn font_px(&self) -> f32 {
        15.0 * self.scale
    }
    pub fn line_h(&self) -> f32 {
        (self.face.line(self.font_px()).2).ceil()
    }
    pub fn row_h(&self) -> f32 {
        self.line_h() + self.u(12.0)
    }

    pub fn id(&self, s: &str) -> Id {
        hash(*self.id_stack.last().unwrap(), s)
    }

    pub fn push_id(&mut self, s: &str) {
        let id = self.id(s);
        self.id_stack.push(id);
    }
    pub fn pop_id(&mut self) {
        self.id_stack.pop();
    }

    // ---- frames ----

    /// Start a frame on a canvas of w x h pixels
    pub fn begin(&mut self, w: u32, h: u32) {
        if self.canvas.pixmap.width() != w || self.canvas.pixmap.height() != h {
            self.canvas = Canvas::new(w, h);
        }
        if self.transparent {
            self.canvas.pixmap.fill(tiny_skia::Color::TRANSPARENT);
        } else {
            self.canvas.clear(self.theme.background);
        }
        self.focus_order.clear();
        self.consumed.clear();
        self.modal = self.modal_now;
        self.modal_now = false;
        self.menu_was_open = self.menu.is_some();
        self.redraw = false;
        // Tab and Shift+Tab: move the focus (in end, once the order is known)
        for (i, k) in self.input.keys.iter().enumerate() {
            if k.usage == keys::TAB && !k.ctrl() && !self.menu_was_open {
                self.focus_move += if k.shift() { -1 } else { 1 };
                self.consumed.insert(i);
            }
        }
        if self.input.pressed {
            self.keyboard_focus = false;
        }
        let pad = self.u(self.padding);
        self.layouts.clear();
        self.layouts.push(Layout {
            dir: Dir::Column,
            x: pad,
            y: pad,
            width: w as f32 - 2.0 * pad,
            start_x: pad,
            start_y: pad,
            max: 0.0,
            bottom: h as f32,
        });
    }

    /// Finish the frame: the menu's drop-down on top, then the focus moves
    pub fn end(&mut self) {
        if let Some(m) = &self.menu {
            let rect = m.rect;
            let items = m.items.clone();
            let sel = m.item;
            let s = self.scale;
            let th = self.theme;
            self.canvas.fill_rounded(Rect::new(rect.x + 3.0 * s, rect.y + 3.0 * s, rect.w, rect.h), 6.0 * s, 0x16171F);
            self.canvas.fill_rounded(rect, 6.0 * s, th.surface);
            self.canvas.stroke_rounded(rect, 6.0 * s, 1.0, th.border);
            let rh = self.row_h();
            for (i, it) in items.iter().enumerate() {
                let r = Rect::new(rect.x + 4.0 * s, rect.y + 4.0 * s + i as f32 * rh, rect.w - 8.0 * s, rh);
                if i == sel {
                    self.canvas.fill_rounded(r, 4.0 * s, th.accent);
                }
                let c = if i == sel { th.accent_text } else { th.text };
                let px = self.font_px();
                self.draw_text(false, r.x + 10.0 * s, r.y + self.u(6.0), it, px, c);
            }
        }
        if self.focus_move != 0 && !self.focus_order.is_empty() {
            let n = self.focus_order.len() as i32;
            let cur = self.focus.and_then(|f| self.focus_order.iter().position(|&x| x == f));
            let next = match cur {
                Some(i) => (i as i32 + self.focus_move).rem_euclid(n),
                None => {
                    if self.focus_move > 0 {
                        0
                    } else {
                        n - 1
                    }
                }
            };
            self.focus = Some(self.focus_order[next as usize]);
            self.keyboard_focus = true;
            self.redraw = true;
        }
        self.focus_move = 0;
        if !self.input.down {
            self.active = None;
        }
        self.input.pressed = false;
        self.input.right_pressed = false;
        self.input.released = false;
        self.input.wheel = 0.0;
        self.input.keys.clear();
    }

    fn draw_text(&mut self, bold: bool, x: f32, y: f32, text: &str, px: f32, c: Color) -> f32 {
        let f = if bold { &mut self.bold } else { &mut self.face };
        self.canvas.text(f, x, y, text, px, c)
    }

    // ---- input helpers ----

    fn blocked(&self) -> bool {
        self.modal || self.menu_was_open
    }

    fn hover(&self, r: Rect) -> bool {
        !self.blocked() && r.contains(self.input.mouse.0, self.input.mouse.1)
    }

    /// Keys for the focused widget id (not yet used), consumed
    fn keys_for(&mut self, id: Id) -> Vec<Key> {
        if self.focus != Some(id) || self.blocked() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, k) in self.input.keys.iter().enumerate() {
            if !self.consumed.contains(&i) {
                out.push(k.clone());
                self.consumed.insert(i);
            }
        }
        out
    }

    /// A focusable widget: Some(true) if it has the focus
    fn focusable(&mut self, id: Id, r: Rect) -> bool {
        self.focus_order.push(id);
        if self.input.pressed && self.hover(r) {
            self.focus = Some(id);
            self.active = Some(id);
        }
        self.focus == Some(id)
    }

    fn focus_ring(&mut self, r: Rect, focused: bool) {
        if focused && self.keyboard_focus {
            let s = self.scale;
            self.canvas.stroke_rounded(Rect::new(r.x - 2.0 * s, r.y - 2.0 * s, r.w + 4.0 * s, r.h + 4.0 * s), 7.0 * s, 2.0 * s, self.theme.highlight);
        }
    }

    // ---- layout ----

    /// The width left in the current column (or row)
    pub fn available_width(&self) -> f32 {
        let l = self.layouts.last().unwrap();
        match l.dir {
            Dir::Column => l.width,
            Dir::Row => l.width - (l.x - l.start_x),
        }
    }

    /// Room for a widget of w x h (w 0: the whole width)
    pub fn alloc(&mut self, w: f32, h: f32) -> Rect {
        let gap = self.u(8.0);
        let l = self.layouts.last_mut().unwrap();
        match l.dir {
            Dir::Column => {
                let w = if w <= 0.0 { l.width } else { w };
                let r = Rect::new(l.x, l.y, w, h);
                l.y += h + gap;
                l.max = l.max.max(w);
                r
            }
            Dir::Row => {
                let w = if w <= 0.0 { (l.width - (l.x - l.start_x)).max(0.0) } else { w };
                let r = Rect::new(l.x, l.y, w, h);
                l.x += w + gap;
                l.max = l.max.max(h);
                r
            }
        }
    }

    /// Widgets side by side
    pub fn row<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let (x, y, width) = {
            let l = self.layouts.last().unwrap();
            (l.x, l.y, self.available_width())
        };
        let bottom = self.layouts.last().unwrap().bottom;
        self.layouts.push(Layout { dir: Dir::Row, x, y, width, start_x: x, start_y: y, max: 0.0, bottom });
        let r = f(self);
        let l = self.layouts.pop().unwrap();
        self.alloc(l.x - l.start_x - self.u(8.0), l.max);
        r
    }

    /// A column of width w (0: what is left) inside a row
    pub fn column<R>(&mut self, w: f32, f: impl FnOnce(&mut Ui) -> R) -> R {
        let (x, y, avail) = {
            let l = self.layouts.last().unwrap();
            (l.x, l.y, self.available_width())
        };
        let width = if w <= 0.0 { avail } else { w };
        let bottom = self.layouts.last().unwrap().bottom;
        self.layouts.push(Layout { dir: Dir::Column, x, y, width, start_x: x, start_y: y, max: 0.0, bottom });
        let r = f(self);
        let l = self.layouts.pop().unwrap();
        let h = (l.y - l.start_y - self.u(8.0)).max(0.0);
        self.alloc(width, h);
        r
    }

    pub fn space(&mut self, h: f32) {
        let h = self.u(h);
        self.alloc(1.0, h);
    }

    // ---- widgets ----

    pub fn label(&mut self, text: &str) {
        let px = self.font_px();
        let w = self.face.width(text, px);
        let h = self.line_h();
        let r = self.alloc(w, h);
        self.draw_text(false, r.x, r.y, text, px, self.theme.text);
    }

    pub fn dim_label(&mut self, text: &str) {
        let px = self.font_px() * 0.9;
        let w = self.face.width(text, px);
        let h = self.line_h();
        let r = self.alloc(w, h);
        self.draw_text(false, r.x, r.y, text, px, self.theme.dim);
    }

    pub fn heading(&mut self, text: &str) {
        let px = self.font_px() * 1.4;
        let w = self.bold.width(text, px);
        let h = (self.bold.line(px).2).ceil();
        let r = self.alloc(w, h);
        self.draw_text(true, r.x, r.y, text, px, self.theme.text);
    }

    pub fn separator(&mut self) {
        let r = self.alloc(0.0, self.u(1.0));
        self.canvas.fill_rect(r, self.theme.border);
    }

    pub fn image(&mut self, img: &tiny_skia::Pixmap) {
        let r = self.alloc(img.width() as f32, img.height() as f32);
        self.canvas.image(img, r.x, r.y);
    }

    /// A button: true when clicked (or Enter or Space while focused)
    pub fn button(&mut self, label: &str) -> bool {
        let id = self.id(label);
        let px = self.font_px();
        let w = self.face.width(label, px) + self.u(32.0);
        let h = self.row_h();
        let r = self.alloc(w, h);
        let focused = self.focusable(id, r);
        let mut clicked = self.input.released && self.active == Some(id) && self.hover(r);
        for k in self.keys_for(id) {
            if k.usage == keys::ENTER || k.usage == keys::SPACE || k.usage == keys::KP_ENTER {
                clicked = true;
            }
        }
        let hot = self.hover(r);
        let pressed = self.active == Some(id) && self.input.down && hot;
        let th = self.theme;
        let bg = if pressed { th.selection } else if hot { th.surface_hover } else { th.surface };
        self.canvas.fill_rounded(r, self.u(6.0), bg);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        let tw = self.face.width(label, px);
        self.draw_text(false, r.x + (r.w - tw) / 2.0, r.y + self.u(6.0), label, px, th.text);
        self.focus_ring(r, focused);
        clicked
    }

    /// `text` shortened with an ellipsis to fit `max` pixels
    pub fn elide(&mut self, text: &str, max: f32, bold: bool) -> String {
        let px = self.font_px();
        let wid = |ui: &mut Ui, t: &str| if bold { ui.bold.width(t, px) } else { ui.face.width(t, px) };
        if wid(self, text) <= max {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut n = chars.len();
        while n > 0 {
            let t: String = chars[..n].iter().collect::<String>() + "\u{2026}";
            if wid(self, &t) <= max {
                return t;
            }
            n -= 1;
        }
        "\u{2026}".to_string()
    }

    /// A button of a given width (0: its label's; the label is shortened
    /// to fit), plain, in the accent colour (`primary`), or marked as the
    /// current one (`selected`: a highlighted background and an accent
    /// line); its id is `name`. Says whether it was clicked, and whether
    /// the right button went down on it.
    pub fn button_ex(&mut self, name: &str, label: &str, w: f32, primary: bool, selected: bool) -> ButtonResponse {
        let id = self.id(name);
        let px = self.font_px();
        let natural = if primary { self.bold.width(label, px) } else { self.face.width(label, px) } + self.u(32.0);
        let bw = if w > 0.0 { w } else { natural };
        let h = self.row_h();
        let r = self.alloc(bw, h);
        let focused = self.focusable(id, r);
        let mut clicked = self.input.released && self.active == Some(id) && self.hover(r);
        for k in self.keys_for(id) {
            if k.usage == keys::ENTER || k.usage == keys::SPACE || k.usage == keys::KP_ENTER {
                clicked = true;
            }
        }
        let right = self.input.right_pressed && self.hover(r);
        let hot = self.hover(r);
        let th = self.theme;
        let (bg, fg) = if primary {
            (if hot { th.accent_hover } else { th.accent }, th.accent_text)
        } else if selected {
            (if hot { mix(th.selection, 0xFFFFFF, 0.08) } else { th.selection }, th.text)
        } else {
            (if hot { th.surface_hover } else { th.surface }, th.text)
        };
        self.canvas.fill_rounded(r, self.u(6.0), bg);
        if !primary {
            self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        }
        if selected && !primary {
            let line = Rect::new(r.x + self.u(6.0), r.y + r.h - self.u(3.0), r.w - self.u(12.0), self.u(2.0));
            self.canvas.fill_rect(line, th.accent);
        }
        let shown = self.elide(label, r.w - self.u(16.0), primary);
        let tw = if primary { self.bold.width(&shown, px) } else { self.face.width(&shown, px) };
        self.draw_text(primary, r.x + (r.w - tw) / 2.0, r.y + self.u(6.0), &shown, px, fg);
        self.focus_ring(r, focused);
        ButtonResponse { clicked, right_clicked: right, rect: r }
    }

    /// A card: a clickable area of w x h pixels that the caller draws into
    /// (its rect is returned), with the hover and selected borders and the
    /// keyboard focus of a button; its id is `name`
    pub fn card(&mut self, name: &str, w: f32, h: f32, selected: bool) -> ButtonResponse {
        let id = self.id(name);
        let r = self.alloc(w, h);
        let focused = self.focusable(id, r);
        let mut clicked = self.input.released && self.active == Some(id) && self.hover(r);
        for k in self.keys_for(id) {
            if k.usage == keys::ENTER || k.usage == keys::SPACE || k.usage == keys::KP_ENTER {
                clicked = true;
            }
        }
        let right = self.input.right_pressed && self.hover(r);
        let th = self.theme;
        let hot = self.hover(r);
        self.canvas.fill_rounded(r, self.u(8.0), if hot { th.surface_hover } else { th.surface });
        let (bw, bc) = if selected { (2.0 * self.scale, th.accent) } else { (1.0, if hot { th.accent_hover } else { th.border }) };
        self.canvas.stroke_rounded(r, self.u(8.0), bw, bc);
        self.focus_ring(r, focused);
        ButtonResponse { clicked, right_clicked: right, rect: r }
    }

    /// A button in the accent colour
    pub fn primary_button(&mut self, label: &str) -> bool {
        let id = self.id(label);
        let px = self.font_px();
        let w = self.bold.width(label, px) + self.u(32.0);
        let h = self.row_h();
        let r = self.alloc(w, h);
        let focused = self.focusable(id, r);
        let mut clicked = self.input.released && self.active == Some(id) && self.hover(r);
        for k in self.keys_for(id) {
            if k.usage == keys::ENTER || k.usage == keys::SPACE || k.usage == keys::KP_ENTER {
                clicked = true;
            }
        }
        let th = self.theme;
        let bg = if self.hover(r) { th.accent_hover } else { th.accent };
        self.canvas.fill_rounded(r, self.u(6.0), bg);
        let tw = self.bold.width(label, px);
        self.draw_text(true, r.x + (r.w - tw) / 2.0, r.y + self.u(6.0), label, px, th.accent_text);
        self.focus_ring(r, focused);
        clicked
    }

    pub fn checkbox(&mut self, value: &mut bool, label: &str) -> bool {
        let id = self.id(label);
        let px = self.font_px();
        let bx = self.u(18.0);
        let w = bx + self.u(8.0) + self.face.width(label, px);
        let h = self.line_h().max(bx);
        let r = self.alloc(w, h);
        let focused = self.focusable(id, r);
        let mut toggled = self.input.released && self.active == Some(id) && self.hover(r);
        for k in self.keys_for(id) {
            if k.usage == keys::SPACE || k.usage == keys::ENTER {
                toggled = true;
            }
        }
        if toggled {
            *value = !*value;
        }
        let th = self.theme;
        let b = Rect::new(r.x, r.y + (h - bx) / 2.0, bx, bx);
        if *value {
            self.canvas.fill_rounded(b, self.u(4.0), th.accent);
            // A tick
            let s = self.scale;
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(b.x + 4.0 * s, b.y + 9.0 * s);
            pb.line_to(b.x + 7.5 * s, b.y + 12.5 * s);
            pb.line_to(b.x + 14.0 * s, b.y + 5.0 * s);
            pb.line_to(b.x + 15.5 * s, b.y + 6.5 * s);
            pb.line_to(b.x + 7.5 * s, b.y + 15.0 * s);
            pb.line_to(b.x + 2.5 * s, b.y + 10.5 * s);
            pb.close();
            if let Some(p) = pb.finish() {
                self.canvas.fill_path(&p, th.accent_text);
            }
        } else {
            self.canvas.fill_rounded(b, self.u(4.0), th.surface);
            self.canvas.stroke_rounded(b, self.u(4.0), 1.0, th.border);
        }
        self.draw_text(false, b.right() + self.u(8.0), r.y + (h - self.line_h()) / 2.0, label, px, th.text);
        self.focus_ring(r, focused);
        toggled
    }

    // ---- text editing ----

    /// Apply editing keys to text; true if it changed
    fn edit(&mut self, st: &mut TextState, text: &mut String, keys: &[Key], multiline: bool) -> (bool, bool) {
        let mut changed = false;
        let mut submitted = false;
        let len = text.chars().count();
        st.cursor = st.cursor.min(len);
        st.anchor = st.anchor.min(len);
        for k in keys {
            let n = text.chars().count();
            let (a, b) = (st.cursor.min(st.anchor), st.cursor.max(st.anchor));
            let delete_sel = |text: &mut String, st: &mut TextState| {
                let (a, b) = (st.cursor.min(st.anchor), st.cursor.max(st.anchor));
                let (ba, bb) = (char_byte(text, a), char_byte(text, b));
                text.replace_range(ba..bb, "");
                st.cursor = a;
                st.anchor = a;
            };
            let line_start = |text: &str, c: usize| -> usize {
                let chars: Vec<char> = text.chars().collect();
                let mut i = c;
                while i > 0 && chars[i - 1] != '\n' {
                    i -= 1;
                }
                i
            };
            let line_end = |text: &str, c: usize| -> usize {
                let chars: Vec<char> = text.chars().collect();
                let mut i = c;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                i
            };
            let mut moved = None;
            match k.usage {
                keys::LEFT => moved = Some(if a != b && !k.shift() { a } else { st.cursor.saturating_sub(1) }),
                keys::RIGHT => moved = Some(if a != b && !k.shift() { b } else { (st.cursor + 1).min(n) }),
                keys::HOME => moved = Some(if multiline { line_start(text, st.cursor) } else { 0 }),
                keys::END => moved = Some(if multiline { line_end(text, st.cursor) } else { n }),
                keys::UP | keys::DOWN if multiline => {
                    let ls = line_start(text, st.cursor);
                    let col = st.cursor - ls;
                    if k.usage == keys::UP {
                        if ls > 0 {
                            let ps = line_start(text, ls - 1);
                            moved = Some((ps + col).min(ls - 1));
                        } else {
                            moved = Some(0);
                        }
                    } else {
                        let le = line_end(text, st.cursor);
                        if le < n {
                            let ne = line_end(text, le + 1);
                            moved = Some((le + 1 + col).min(ne));
                        } else {
                            moved = Some(n);
                        }
                    }
                }
                keys::BACKSPACE => {
                    if a != b {
                        delete_sel(text, st);
                    } else if st.cursor > 0 {
                        let (ba, bb) = (char_byte(text, st.cursor - 1), char_byte(text, st.cursor));
                        text.replace_range(ba..bb, "");
                        st.cursor -= 1;
                        st.anchor = st.cursor;
                    }
                    changed = true;
                }
                keys::DELETE => {
                    if a != b {
                        delete_sel(text, st);
                    } else if st.cursor < n {
                        let (ba, bb) = (char_byte(text, st.cursor), char_byte(text, st.cursor + 1));
                        text.replace_range(ba..bb, "");
                    }
                    changed = true;
                }
                keys::A if k.ctrl() => {
                    st.anchor = 0;
                    st.cursor = n;
                }
                keys::C | keys::X if k.ctrl() => {
                    if a != b {
                        let (ba, bb) = (char_byte(text, a), char_byte(text, b));
                        let sel = text[ba..bb].to_string();
                        self.platform.clipboard_set(&sel);
                        if k.usage == keys::X {
                            delete_sel(text, st);
                            changed = true;
                        }
                    }
                }
                keys::V if k.ctrl() => {
                    let mut clip = self.platform.clipboard_get();
                    if !multiline {
                        clip = clip.replace('\n', " ");
                    }
                    delete_sel(text, st);
                    let bi = char_byte(text, st.cursor);
                    text.insert_str(bi, &clip);
                    st.cursor += clip.chars().count();
                    st.anchor = st.cursor;
                    changed = true;
                }
                keys::ENTER | keys::KP_ENTER => {
                    if multiline {
                        delete_sel(text, st);
                        let bi = char_byte(text, st.cursor);
                        text.insert(bi, '\n');
                        st.cursor += 1;
                        st.anchor = st.cursor;
                        changed = true;
                    } else {
                        submitted = true;
                    }
                }
                _ => {
                    if let Some(t) = k.printable() {
                        delete_sel(text, st);
                        let bi = char_byte(text, st.cursor);
                        text.insert_str(bi, t);
                        st.cursor += t.chars().count();
                        st.anchor = st.cursor;
                        changed = true;
                    }
                }
            }
            if let Some(m) = moved {
                st.cursor = m;
                if !k.shift() {
                    st.anchor = m;
                }
            }
        }
        (changed, submitted)
    }

    /// A one-line text field of width w: (changed, Enter pressed)
    pub fn text_field(&mut self, name: &str, text: &mut String, w: f32) -> (bool, bool) {
        self.text_field_impl(name, text, w, false)
    }

    /// A password field: dots instead of the characters; nothing is copied
    /// from it
    pub fn password_field(&mut self, name: &str, text: &mut String, w: f32) -> (bool, bool) {
        self.text_field_impl(name, text, w, true)
    }

    /// The width of text in the normal font
    pub fn text_width(&mut self, text: &str) -> f32 {
        let px = self.font_px();
        self.face.width(text, px)
    }

    fn text_field_impl(&mut self, name: &str, text: &mut String, w: f32, mask: bool) -> (bool, bool) {
        let id = self.id(name);
        let h = self.row_h();
        let w = if w <= 0.0 { 0.0 } else { self.u(w) };
        let r = self.alloc(w, h);
        let focused = self.focusable(id, r);
        let mut st = self.texts.remove(&id).unwrap_or_default();
        let px = self.font_px();
        let pad = self.u(8.0);
        // The mouse: place the cursor, drag to select
        let shown = |t: &str| -> String { if mask { "\u{2022}".repeat(t.chars().count()) } else { t.to_string() } };
        if self.active == Some(id) && self.input.down && !self.blocked() {
            let disp = shown(text);
            let adv = self.face.advances(&disp, px);
            let mut x = r.x + pad - st.scroll;
            let mut i = 0;
            while i < adv.len() && x + adv[i] / 2.0 < self.input.mouse.0 {
                x += adv[i];
                i += 1;
            }
            st.cursor = i;
            if self.input.pressed {
                st.anchor = i;
            }
        }
        let mut keys = self.keys_for(id);
        if mask {
            keys.retain(|k| !(k.ctrl() && (k.usage == crate::ui::keys::C || k.usage == crate::ui::keys::X)));
        }
        let (changed, submitted) = self.edit(&mut st, text, &keys, false);
        let disp = shown(text);
        let th = self.theme;
        self.canvas.fill_rounded(r, self.u(6.0), th.surface);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        let adv = self.face.advances(&disp, px);
        let pos = |c: usize| -> f32 { adv[..c.min(adv.len())].iter().sum() };
        // Keep the cursor visible
        let cx = pos(st.cursor);
        let inner = r.w - 2.0 * pad;
        if cx - st.scroll > inner {
            st.scroll = cx - inner;
        }
        if cx < st.scroll {
            st.scroll = cx;
        }
        self.canvas.push_clip(r.shrink(1.0));
        let ty = r.y + self.u(6.0);
        if focused && st.cursor != st.anchor {
            let (a, b) = (st.cursor.min(st.anchor), st.cursor.max(st.anchor));
            self.canvas.fill_rect(Rect::new(r.x + pad + pos(a) - st.scroll, ty, pos(b) - pos(a), self.line_h()), th.selection);
        }
        self.draw_text(false, r.x + pad - st.scroll, ty, &disp, px, th.text);
        if focused {
            self.canvas.fill_rect(Rect::new((r.x + pad + cx - st.scroll).round(), ty, self.u(2.0).max(1.0), self.line_h()), th.accent);
        }
        self.canvas.pop_clip();
        self.focus_ring(r, focused);
        self.texts.insert(id, st);
        (changed, submitted)
    }

    /// A multi-line text editor field of w x h (w 0: the whole width): changed
    pub fn text_area(&mut self, name: &str, text: &mut String, w: f32, h: f32) -> bool {
        let id = self.id(name);
        let w = if w <= 0.0 { 0.0 } else { self.u(w) };
        let r = self.alloc(w, self.u(h));
        let focused = self.focusable(id, r);
        let mut st = self.texts.remove(&id).unwrap_or_default();
        let px = self.font_px();
        let lh = self.line_h();
        let pad = self.u(8.0);
        let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        // The mouse: line from y, column from x
        if self.active == Some(id) && self.input.down && !self.blocked() {
            let li = (((self.input.mouse.1 - r.y - pad + st.scroll) / lh).floor().max(0.0) as usize).min(lines.len() - 1);
            let adv = self.face.advances(&lines[li], px);
            let mut x = r.x + pad;
            let mut col = 0;
            while col < adv.len() && x + adv[col] / 2.0 < self.input.mouse.0 {
                x += adv[col];
                col += 1;
            }
            let c: usize = lines[..li].iter().map(|l| l.chars().count() + 1).sum::<usize>() + col;
            st.cursor = c;
            if self.input.pressed {
                st.anchor = c;
            }
        }
        if self.hover(r) && self.input.wheel != 0.0 {
            st.scroll = (st.scroll - self.input.wheel * lh * 3.0).max(0.0);
            self.input.wheel = 0.0;
        }
        let keys = self.keys_for(id);
        let had_keys = !keys.is_empty();
        let (changed, _) = self.edit(&mut st, text, &keys, true);
        let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        // Cursor line and column
        let mut rem = st.cursor;
        let mut cl = 0;
        while cl < lines.len() - 1 && rem > lines[cl].chars().count() {
            rem -= lines[cl].chars().count() + 1;
            cl += 1;
        }
        let inner_h = r.h - 2.0 * pad;
        let max_scroll = (lines.len() as f32 * lh - inner_h).max(0.0);
        if had_keys {
            let cy = cl as f32 * lh;
            if cy < st.scroll {
                st.scroll = cy;
            }
            if cy + lh > st.scroll + inner_h {
                st.scroll = cy + lh - inner_h;
            }
        }
        st.scroll = st.scroll.min(max_scroll).max(0.0);
        let th = self.theme;
        self.canvas.fill_rounded(r, self.u(6.0), th.surface);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        self.canvas.push_clip(r.shrink(2.0));
        let (sa, sb) = (st.cursor.min(st.anchor), st.cursor.max(st.anchor));
        let mut start = 0usize;
        for (i, line) in lines.iter().enumerate() {
            let y = r.y + pad + i as f32 * lh - st.scroll;
            let n = line.chars().count();
            if y + lh >= r.y && y <= r.bottom() {
                let adv = self.face.advances(line, px);
                let pos = |c: usize| -> f32 { adv[..c.min(adv.len())].iter().sum() };
                if focused && sa != sb && sb > start && sa <= start + n {
                    let a = sa.max(start) - start;
                    let b = sb.min(start + n) - start;
                    let extra = if sb > start + n { self.u(6.0) } else { 0.0 };
                    self.canvas.fill_rect(Rect::new(r.x + pad + pos(a), y, pos(b) - pos(a) + extra, lh), th.selection);
                }
                self.draw_text(false, r.x + pad, y, line, px, th.text);
                if focused && i == cl {
                    self.canvas.fill_rect(Rect::new((r.x + pad + pos(rem)).round(), y, self.u(2.0).max(1.0), lh), th.accent);
                }
            }
            start += n + 1;
        }
        self.canvas.pop_clip();
        self.focus_ring(r, focused);
        self.texts.insert(id, st);
        changed
    }

    // ---- lists ----

    fn scrollbar(&mut self, r: Rect, content: f32, scroll: &mut f32, id: Id) {
        if content <= r.h {
            *scroll = 0.0;
            return;
        }
        let s = self.scale;
        let track = Rect::new(r.right() - 8.0 * s, r.y + 2.0 * s, 6.0 * s, r.h - 4.0 * s);
        let thumb_h = (track.h * r.h / content).max(20.0 * s);
        let max = content - r.h;
        // Drag the thumb
        let bar_id = id ^ 0x5c011;
        if self.input.pressed && self.hover(Rect::new(track.x - 4.0 * s, track.y, track.w + 8.0 * s, track.h)) {
            self.active = Some(bar_id);
        }
        if self.active == Some(bar_id) && self.input.down {
            let t = ((self.input.mouse.1 - track.y - thumb_h / 2.0) / (track.h - thumb_h)).clamp(0.0, 1.0);
            *scroll = t * max;
        }
        let ty = track.y + (track.h - thumb_h) * (*scroll / max);
        self.canvas.fill_rounded(track, 3.0 * s, self.theme.surface);
        self.canvas.fill_rounded(Rect::new(track.x, ty, track.w, thumb_h), 3.0 * s, self.theme.border);
    }

    /// A long list (only the visible rows are drawn): true if the
    /// selection changed (click, arrows, Page Up/Down, Home, End)
    pub fn list(&mut self, name: &str, items: &[String], selected: &mut Option<usize>, h: f32) -> bool {
        let id = self.id(name);
        let r = self.alloc(0.0, self.u(h));
        let focused = self.focusable(id, r);
        let rh = self.line_h() + self.u(8.0);
        let mut scroll = *self.scrolls.get(&id).unwrap_or(&0.0);
        let content = items.len() as f32 * rh;
        let before = *selected;
        if self.hover(r) && self.input.wheel != 0.0 {
            scroll -= self.input.wheel * rh * 3.0;
            self.input.wheel = 0.0;
        }
        let page = ((r.h / rh) as usize).max(1);
        let mut moved = false;
        for k in self.keys_for(id) {
            let n = items.len();
            if n == 0 {
                break;
            }
            let cur = selected.unwrap_or(0);
            let new = match k.usage {
                keys::DOWN => Some(if selected.is_none() { 0 } else { (cur + 1).min(n - 1) }),
                keys::UP => Some(cur.saturating_sub(1)),
                keys::PAGE_DOWN => Some((cur + page).min(n - 1)),
                keys::PAGE_UP => Some(cur.saturating_sub(page)),
                keys::HOME => Some(0),
                keys::END => Some(n - 1),
                _ => None,
            };
            if let Some(v) = new {
                *selected = Some(v);
                moved = true;
            }
        }
        let inner = Rect::new(r.x, r.y, r.w - self.u(12.0), r.h);
        if self.input.pressed && self.hover(inner) {
            let i = ((self.input.mouse.1 - r.y + scroll) / rh) as usize;
            if i < items.len() {
                *selected = Some(i);
            }
        }
        if moved {
            if let Some(s) = *selected {
                let y = s as f32 * rh;
                if y < scroll {
                    scroll = y;
                }
                if y + rh > scroll + r.h {
                    scroll = y + rh - r.h;
                }
            }
        }
        scroll = scroll.clamp(0.0, (content - r.h).max(0.0));
        let th = self.theme;
        self.canvas.fill_rounded(r, self.u(6.0), th.surface);
        self.canvas.push_clip(r.shrink(1.0));
        let first = (scroll / rh) as usize;
        let px = self.font_px();
        for i in first..items.len() {
            let y = r.y + i as f32 * rh - scroll;
            if y > r.bottom() {
                break;
            }
            let row = Rect::new(r.x + 2.0, y, inner.w - 4.0, rh);
            if Some(i) == *selected {
                self.canvas.fill_rect(Rect::new(row.x, y.round(), row.w, rh), if focused { th.selection } else { th.surface_hover });
            }
            self.draw_text(false, r.x + self.u(10.0), y + self.u(4.0), &items[i], px, th.text);
        }
        self.canvas.pop_clip();
        self.scrollbar(r, content, &mut scroll, id);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        self.focus_ring(r, focused);
        self.scrolls.insert(id, scroll);
        *selected != before
    }

    /// The height left below the current position (down to the padding)
    pub fn remaining_height(&self) -> f32 {
        let l = self.layouts.last().unwrap();
        (l.bottom - l.y - self.u(self.padding)).max(self.u(40.0))
    }

    /// Draw an icon of size `sz` with its top left at x, y
    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, sz: f32) {
        let s = sz / 16.0;
        let r = |dx: f32, dy: f32, w: f32, h: f32| Rect::new((x + dx * s).round(), (y + dy * s).round(), (w * s).round().max(1.0), (h * s).round().max(1.0));
        let th = self.theme;
        // Folders in the theme's own colour
        let (folder, folder_hi) = (th.accent, th.accent_hover);
        match icon {
            Icon::None => {}
            Icon::Folder => {
                self.canvas.fill_rounded(r(1.0, 2.0, 6.0, 3.0), s, folder);
                self.canvas.fill_rounded(r(1.0, 4.0, 14.0, 10.0), 1.5 * s, folder);
                self.canvas.fill_rounded(r(1.0, 6.0, 14.0, 8.0), 1.5 * s, folder_hi);
            }
            Icon::File | Icon::Image | Icon::Program => {
                self.canvas.fill_rounded(r(3.0, 1.0, 10.0, 14.0), 1.5 * s, th.text);
                let c = match icon {
                    Icon::Image => th.ansi[2],
                    Icon::Program => th.ansi[5],
                    _ => th.dim,
                };
                for k in 0..3 {
                    self.canvas.fill_rect(r(5.0, 5.0 + 3.0 * k as f32, 6.0, 1.0), c);
                }
            }
            Icon::Drive => {
                self.canvas.fill_rounded(r(1.0, 4.0, 14.0, 8.0), 2.0 * s, th.ansi[6]);
                self.canvas.fill_rect(r(11.0, 7.0, 2.0, 2.0), th.background);
            }
            Icon::Trash | Icon::TrashFull => {
                self.canvas.fill_rect(r(2.0, 3.0, 12.0, 2.0), th.dim);
                self.canvas.fill_rect(r(6.0, 1.0, 4.0, 2.0), th.dim);
                self.canvas.fill_rounded(r(3.0, 5.0, 10.0, 10.0), 1.5 * s, th.dim);
                if icon == Icon::TrashFull {
                    // Paper showing over the rim
                    self.canvas.fill_rect(r(4.0, 6.0, 8.0, 1.0), th.text);
                    self.canvas.fill_rect(r(5.0, 8.0, 6.0, 1.0), th.text);
                    self.canvas.fill_rect(r(4.0, 10.0, 7.0, 1.0), th.text);
                }
            }
            Icon::Power => {
                let c = th.text;
                // The ring: a stroked circle, then the gap at the top
                let ring = r(2.0, 3.0, 12.0, 12.0);
                self.canvas.stroke_rounded(ring, 6.0 * s, 1.6 * s, c);
                self.canvas.fill_rect(r(6.0, 1.0, 4.0, 5.0), th.surface);
                self.canvas.fill_rect(r(7.25, 1.0, 1.5, 7.0), c);
            }
            Icon::Fox => {
                if let Some(p) = fox_path(x, y, sz) {
                    self.canvas.fill_path_evenodd(&p, FOX_COLOR);
                }
            }
            Icon::Home => {
                // A roof in steps, the house, its door
                for k in 0..6 {
                    let k = k as f32;
                    self.canvas.fill_rect(r(6.0 - k, 1.0 + k, 4.0 + 2.0 * k, 1.0), th.accent);
                }
                self.canvas.fill_rect(r(3.0, 7.0, 10.0, 8.0), folder_hi);
                self.canvas.fill_rect(r(7.0, 10.0, 2.0, 5.0), th.background);
            }
        }
    }

    /// A table with column headers (only the visible rows are drawn; h 0:
    /// down to the bottom).
    /// Arrows, Page Up/Down, Home and End move the selection; Enter or a
    /// double-click opens a row; the right button selects one and reports
    /// it (a context menu is the program's).
    pub fn table(&mut self, name: &str, columns: &[Column], rows: &[Row], selected: &mut Option<usize>, h: f32) -> TableResponse {
        let id = self.id(name);
        let r = self.alloc(0.0, if h <= 0.0 { self.remaining_height() } else { self.u(h) });
        let focused = self.focusable(id, r);
        let rh = self.line_h() + self.u(8.0);
        let hh = rh; // the header
        let body = Rect::new(r.x, r.y + hh, r.w, r.h - hh);
        let mut scroll = *self.scrolls.get(&id).unwrap_or(&0.0);
        let content = rows.len() as f32 * rh;
        let before = *selected;
        let mut out = TableResponse::default();
        if self.hover(body) && self.input.wheel != 0.0 {
            scroll -= self.input.wheel * rh * 3.0;
            self.input.wheel = 0.0;
        }
        let page = ((body.h / rh) as usize).max(1);
        if self.hover(body) {
            out.over = true;
            let (_, my) = self.input.mouse;
            let i = ((my - body.y + scroll.clamp(0.0, (content - body.h).max(0.0))) / rh).floor();
            if i >= 0.0 && (i as usize) < rows.len() {
                out.hover = Some(i as usize);
            }
        }
        let mut moved = false;
        for k in self.keys_for(id) {
            let n = rows.len();
            let cur = selected.unwrap_or(0);
            let new = match k.usage {
                keys::DOWN if n > 0 => Some(if selected.is_none() { 0 } else { (cur + 1).min(n - 1) }),
                keys::UP if n > 0 => Some(cur.saturating_sub(1)),
                keys::PAGE_DOWN if n > 0 => Some((cur + page).min(n - 1)),
                keys::PAGE_UP if n > 0 => Some(cur.saturating_sub(page)),
                keys::HOME if n > 0 => Some(0),
                keys::END if n > 0 => Some(n - 1),
                keys::ENTER | keys::KP_ENTER if selected.is_some() => {
                    out.activated = *selected;
                    None
                }
                _ => {
                    out.keys.push(k);
                    None
                }
            };
            if let Some(v) = new {
                *selected = Some(v);
                moved = true;
            }
        }
        let inner = Rect::new(body.x, body.y, body.w - self.u(12.0), body.h);
        let hit = |ui: &Ui| -> Option<usize> {
            let i = ((ui.input.mouse.1 - body.y + scroll) / rh) as usize;
            (i < rows.len()).then_some(i)
        };
        if (self.input.pressed || self.input.right_pressed) && self.hover(inner) {
            let i = hit(self);
            if self.input.right_pressed {
                self.focus = Some(id);
                if let Some(i) = i {
                    *selected = Some(i);
                    out.context = Some(i);
                }
            } else {
                *selected = i;
                if let Some(i) = i {
                    let now = std::time::Instant::now();
                    if let Some((lid, li, t)) = self.last_click {
                        if lid == id && li == i && now.duration_since(t).as_millis() < 500 {
                            out.activated = Some(i);
                        }
                    }
                    self.last_click = if out.activated.is_some() { None } else { Some((id, i, now)) };
                }
            }
        }
        if moved {
            if let Some(s) = *selected {
                let y = s as f32 * rh;
                if y < scroll {
                    scroll = y;
                }
                if y + rh > scroll + body.h {
                    scroll = y + rh - body.h;
                }
            }
        }
        scroll = scroll.clamp(0.0, (content - body.h).max(0.0));
        let th = self.theme;
        self.canvas.fill_rounded(r, self.u(6.0), th.surface);
        // Column positions
        let fixed: f32 = columns.iter().map(|c| self.u(c.width)).sum();
        let pad = self.u(10.0);
        let mut xs = Vec::new();
        let mut x = r.x + pad;
        for c in columns {
            let w = if c.width <= 0.0 { (inner.w - fixed - 2.0 * pad).max(self.u(60.0)) } else { self.u(c.width) };
            xs.push((x, w));
            x += w;
        }
        let px = self.font_px();
        let ty_h = self.u(4.0);
        for (c, &(x, w)) in columns.iter().zip(xs.iter()) {
            let tx = if c.right { x + w - self.u(12.0) - self.face.width(c.title, px * 0.9) } else { x };
            self.canvas.push_clip(Rect::new(x, r.y, w - self.u(8.0), hh));
            self.draw_text(true, tx, r.y + ty_h, c.title, px * 0.9, th.dim);
            self.canvas.pop_clip();
        }
        self.canvas.fill_rect(Rect::new(r.x, (r.y + hh - 1.0).round(), r.w, 1.0), th.border);
        self.canvas.push_clip(body.shrink(1.0));
        let first = (scroll / rh) as usize;
        let isz = (self.line_h() * 0.9).round();
        for i in first..rows.len() {
            let y = body.y + i as f32 * rh - scroll;
            if y > body.bottom() {
                break;
            }
            if Some(i) == *selected {
                self.canvas.fill_rect(Rect::new(r.x + 2.0, y.round(), inner.w - 4.0, rh), if focused { th.selection } else { th.surface_hover });
            }
            for (ci, (cell, &(x, w))) in rows[i].cells.iter().zip(xs.iter()).enumerate() {
                let mut tx = x;
                if ci == 0 && rows[i].icon != Icon::None {
                    self.icon(rows[i].icon, x, y + (rh - isz) / 2.0, isz);
                    tx += isz + self.u(8.0);
                }
                let col = columns.get(ci).copied().unwrap_or(Column { title: "", width: 0.0, right: false });
                if col.right {
                    tx = x + w - self.u(12.0) - self.face.width(cell, px);
                }
                self.canvas.push_clip(Rect::new(x, y, w - self.u(8.0), rh));
                let c = if ci == 0 { th.text } else { th.dim };
                self.draw_text(false, tx, y + self.u(4.0), cell, px, c);
                self.canvas.pop_clip();
            }
        }
        self.canvas.pop_clip();
        self.scrollbar(body, content, &mut scroll, id);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        self.focus_ring(r, focused);
        self.scrolls.insert(id, scroll);
        out.changed = *selected != before;
        out
    }

    /// Give the keyboard focus to a widget by name only if nothing has it
    pub fn focus_if_none(&mut self, name: &str) {
        if self.focus.is_none() {
            self.focus = Some(self.id(name));
        }
    }

    /// Whether the widget with this name has the focus
    pub fn has_focus(&self, name: &str) -> bool {
        self.focus == Some(self.id(name))
    }

    /// A tree: Right opens a node, Left closes it, a click on the
    /// triangle toggles it. Returns the selected node's labels from the
    /// root when the selection changed.
    pub fn tree(&mut self, name: &str, roots: &[TreeNode], h: f32) -> Option<Vec<String>> {
        let id = self.id(name);
        // The visible rows: (id, depth, path, has children)
        let mut rows: Vec<(Id, usize, Vec<String>, bool)> = Vec::new();
        fn walk(ui: &Ui, nodes: &[TreeNode], parent: Id, depth: usize, path: &mut Vec<String>, rows: &mut Vec<(Id, usize, Vec<String>, bool)>) {
            for n in nodes {
                let nid = hash(parent, &n.label);
                path.push(n.label.clone());
                rows.push((nid, depth, path.clone(), !n.children.is_empty()));
                if ui.open_nodes.contains(&nid) {
                    walk(ui, &n.children, nid, depth + 1, path, rows);
                }
                path.pop();
            }
        }
        walk(self, roots, id, 0, &mut Vec::new(), &mut rows);
        let r = self.alloc(0.0, self.u(h));
        let focused = self.focusable(id, r);
        let rh = self.line_h() + self.u(8.0);
        let indent = self.u(18.0);
        let mut scroll = *self.scrolls.get(&id).unwrap_or(&0.0);
        if self.hover(r) && self.input.wheel != 0.0 {
            scroll -= self.input.wheel * rh * 3.0;
            self.input.wheel = 0.0;
        }
        let sel = self.selected_node.get(&id).copied();
        let mut cur = sel.and_then(|s| rows.iter().position(|row| row.0 == s));
        let before = cur;
        let mut toggled = false;
        for k in self.keys_for(id) {
            if rows.is_empty() {
                break;
            }
            match k.usage {
                keys::DOWN => cur = Some(cur.map_or(0, |c| (c + 1).min(rows.len() - 1))),
                keys::UP => cur = Some(cur.map_or(0, |c| c.saturating_sub(1))),
                keys::RIGHT => {
                    if let Some(c) = cur {
                        if rows[c].3 {
                            self.open_nodes.insert(rows[c].0);
                            toggled = true;
                        }
                    }
                }
                keys::LEFT => {
                    if let Some(c) = cur {
                        if self.open_nodes.remove(&rows[c].0) {
                            toggled = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if self.input.pressed && self.hover(r) {
            let i = ((self.input.mouse.1 - r.y + scroll) / rh) as usize;
            if i < rows.len() {
                let tri_x = r.x + self.u(8.0) + rows[i].1 as f32 * indent;
                if rows[i].3 && self.input.mouse.0 < tri_x + self.u(16.0) {
                    if !self.open_nodes.remove(&rows[i].0) {
                        self.open_nodes.insert(rows[i].0);
                    }
                    toggled = true;
                }
                cur = Some(i);
            }
        }
        if toggled {
            self.redraw = true; // the rows change: draw again with them
        }
        let content = rows.len() as f32 * rh;
        if let Some(c) = cur {
            self.selected_node.insert(id, rows[c].0);
            if cur != before {
                let y = c as f32 * rh;
                if y < scroll {
                    scroll = y;
                }
                if y + rh > scroll + r.h {
                    scroll = y + rh - r.h;
                }
            }
        }
        scroll = scroll.clamp(0.0, (content - r.h).max(0.0));
        let th = self.theme;
        self.canvas.fill_rounded(r, self.u(6.0), th.surface);
        self.canvas.push_clip(r.shrink(1.0));
        let px = self.font_px();
        let s = self.scale;
        for (i, row) in rows.iter().enumerate() {
            let y = r.y + i as f32 * rh - scroll;
            if y + rh < r.y || y > r.bottom() {
                continue;
            }
            if Some(i) == cur {
                self.canvas.fill_rect(Rect::new(r.x + 2.0, y.round(), r.w - self.u(14.0), rh), if focused { th.selection } else { th.surface_hover });
            }
            let x = r.x + self.u(8.0) + row.1 as f32 * indent;
            if row.3 {
                // A triangle: right when closed, down when open
                let mut pb = tiny_skia::PathBuilder::new();
                let (cx, cy) = (x + 6.0 * s, y + rh / 2.0);
                if self.open_nodes.contains(&row.0) {
                    pb.move_to(cx - 5.0 * s, cy - 3.0 * s);
                    pb.line_to(cx + 5.0 * s, cy - 3.0 * s);
                    pb.line_to(cx, cy + 4.0 * s);
                } else {
                    pb.move_to(cx - 3.0 * s, cy - 5.0 * s);
                    pb.line_to(cx + 4.0 * s, cy);
                    pb.line_to(cx - 3.0 * s, cy + 5.0 * s);
                }
                pb.close();
                if let Some(p) = pb.finish() {
                    self.canvas.fill_path(&p, th.dim);
                }
            }
            let label = row.2.last().unwrap().clone();
            self.draw_text(false, x + self.u(18.0), y + self.u(4.0), &label, px, th.text);
        }
        self.canvas.pop_clip();
        self.scrollbar(r, content, &mut scroll, id);
        self.canvas.stroke_rounded(r, self.u(6.0), 1.0, if focused { th.accent } else { th.border });
        self.focus_ring(r, focused);
        self.scrolls.insert(id, scroll);
        if cur != before {
            cur.map(|c| rows[c].2.clone())
        } else {
            None
        }
    }

    /// A scrolled area filling the rest of the window but `reserve` (in
    /// unscaled pixels) below it: a page that scrolls when the window is
    /// smaller than what it shows
    pub fn scroll_page(&mut self, name: &str, reserve: f32, f: impl FnOnce(&mut Ui)) {
        let h = (self.remaining_height() / self.scale - reserve).max(40.0);
        self.scroll_area(name, h, f);
    }

    /// A scrolled area of height h holding what f draws
    pub fn scroll_area(&mut self, name: &str, h: f32, f: impl FnOnce(&mut Ui)) {
        let id = self.id(name);
        let r = self.alloc(0.0, self.u(h));
        let mut scroll = *self.scrolls.get(&id).unwrap_or(&0.0);
        let content = *self.heights.get(&id).unwrap_or(&0.0);
        scroll = scroll.clamp(0.0, (content - r.h).max(0.0));
        self.canvas.push_clip(r);
        let pad = self.u(4.0);
        self.layouts.push(Layout {
            dir: Dir::Column,
            x: r.x + pad,
            y: r.y + pad - scroll,
            width: r.w - self.u(16.0),
            start_x: r.x + pad,
            start_y: r.y + pad - scroll,
            max: 0.0,
            bottom: r.y + r.h - pad - scroll,
        });
        self.push_id(name);
        f(self);
        self.pop_id();
        let l = self.layouts.pop().unwrap();
        self.canvas.pop_clip();
        let used = l.y - l.start_y + pad;
        if (used - content).abs() > 0.5 {
            self.redraw = true;
        }
        self.heights.insert(id, used);
        // The wheel, unless a list or text area inside took it
        if self.hover(r) && self.input.wheel != 0.0 {
            scroll = (scroll - self.input.wheel * self.line_h() * 3.0).clamp(0.0, (used - r.h).max(0.0));
            self.input.wheel = 0.0;
            self.redraw = true;
        }
        self.scrollbar(r, used, &mut scroll, id);
        self.scrolls.insert(id, scroll);
    }

    /// Tabs: the selected one in *selected; Left and Right when focused
    pub fn tabs(&mut self, name: &str, labels: &[&str], selected: &mut usize) -> bool {
        let id = self.id(name);
        let px = self.font_px();
        let h = self.row_h();
        let r = self.alloc(0.0, h);
        let focused = self.focusable(id, r);
        let before = *selected;
        for k in self.keys_for(id) {
            match k.usage {
                keys::RIGHT => *selected = (*selected + 1).min(labels.len() - 1),
                keys::LEFT => *selected = selected.saturating_sub(1),
                _ => {}
            }
        }
        let th = self.theme;
        let mut x = r.x;
        for (i, l) in labels.iter().enumerate() {
            let w = self.face.width(l, px) + self.u(28.0);
            let t = Rect::new(x, r.y, w, h);
            if self.input.pressed && self.hover(t) {
                *selected = i;
            }
            if i == *selected {
                self.canvas.fill_rounded(Rect::new(t.x, t.y, t.w, t.h + self.u(6.0)), self.u(6.0), th.surface_hi);
            }
            let c = if i == *selected { th.text } else { th.dim };
            self.draw_text(false, x + self.u(14.0), r.y + self.u(6.0), l, px, c);
            if i == *selected {
                self.canvas.fill_rect(Rect::new(t.x + self.u(10.0), t.bottom() - self.u(3.0), t.w - self.u(20.0), self.u(2.0)), th.accent);
            }
            x += w + self.u(4.0);
        }
        self.canvas.fill_rect(Rect::new(r.x, r.bottom(), r.w, 1.0), th.border);
        self.focus_ring(Rect::new(r.x, r.y, x - r.x, h), focused);
        *selected != before
    }

    /// A menu bar: (menu, item) when an item is chosen. F10 opens the
    /// first menu; arrows move; Enter chooses; Esc or a click elsewhere
    /// closes.
    pub fn menu_bar(&mut self, menus: &[(&str, &[&str])]) -> Option<(usize, usize)> {
        let px = self.font_px();
        let h = self.row_h();
        // The bar spans the window, above the padding
        let r = Rect::new(0.0, 0.0, self.canvas.width(), h);
        {
            let top = h + self.u(12.0);
            let l = self.layouts.last_mut().unwrap();
            if l.y < top {
                l.y = top;
                l.start_y = l.y;
            }
        }
        let th = self.theme;
        self.canvas.fill_rect(r, th.surface);
        self.canvas.fill_rect(Rect::new(0.0, h - 1.0, r.w, 1.0), th.border);
        // Title positions
        let mut titles = Vec::new();
        let mut x = self.u(6.0);
        for (t, _) in menus {
            let w = self.face.width(t, px) + self.u(24.0);
            titles.push(Rect::new(x, 0.0, w, h));
            x += w;
        }
        let rh = self.row_h();
        let drop = |ui: &Ui, m: usize| -> Rect {
            let px = ui.font_px();
            let mut w: f32 = 120.0 * ui.scale;
            for it in menus[m].1 {
                w = w.max(ui.face_width_estimate(it, px) + 40.0 * ui.scale);
            }
            Rect::new(titles[m].x, h, w, menus[m].1.len() as f32 * rh + 8.0 * ui.scale)
        };
        let mut chosen = None;
        let mut open = self.menu.as_ref().map(|m| (m.menu, m.item));
        if self.modal {
            open = None;
        }
        // Keys: F10 opens; while open, the menu has them all
        let mut close = false;
        for i in 0..self.input.keys.len() {
            if self.consumed.contains(&i) || self.modal {
                continue;
            }
            let k = self.input.keys[i].clone();
            match open {
                None => {
                    if k.usage == keys::F10 {
                        open = Some((0, 0));
                        self.consumed.insert(i);
                    }
                }
                Some((m, it)) => {
                    self.consumed.insert(i);
                    let n = menus[m].1.len();
                    match k.usage {
                        keys::ESC | keys::F10 => close = true,
                        keys::DOWN => open = Some((m, (it + 1) % n)),
                        keys::UP => open = Some((m, (it + n - 1) % n)),
                        keys::RIGHT => open = Some(((m + 1) % menus.len(), 0)),
                        keys::LEFT => open = Some(((m + menus.len() - 1) % menus.len(), 0)),
                        keys::ENTER | keys::KP_ENTER | keys::SPACE => {
                            chosen = Some((m, it));
                            close = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        // The mouse
        let (mx, my) = self.input.mouse;
        if self.input.pressed && !self.modal {
            let on_title = titles.iter().position(|t| t.contains(mx, my));
            match (open, on_title) {
                (Some((m, _)), Some(t)) if t == m => close = true,
                (_, Some(t)) => open = Some((t, 0)),
                (Some((m, _)), None) => {
                    let d = drop(self, m);
                    if !d.contains(mx, my) {
                        close = true;
                    }
                }
                _ => {}
            }
        }
        if let Some((m, _)) = open {
            let d = drop(self, m);
            if d.contains(mx, my) {
                let i = ((my - d.y - 4.0 * self.scale) / rh).floor().max(0.0) as usize;
                if i < menus[m].1.len() {
                    open = Some((m, i));
                    if self.input.released && self.menu_was_open {
                        chosen = Some((m, i));
                        close = true;
                    }
                }
            }
            // Moving over another title while open switches menus
            if let Some(t) = titles.iter().position(|t| t.contains(mx, my)) {
                if t != m {
                    open = Some((t, 0));
                }
            }
        }
        if close {
            open = None;
        }
        // Draw the titles
        for (i, (t, _)) in menus.iter().enumerate() {
            let tr = titles[i];
            if open.map(|o| o.0) == Some(i) {
                self.canvas.fill_rect(Rect::new(tr.x, tr.y + 2.0, tr.w, tr.h - 3.0), th.surface_hi);
            }
            self.draw_text(false, tr.x + self.u(12.0), self.u(6.0), t, px, th.text);
        }
        self.menu = open.map(|(m, i)| OpenMenu {
            menu: m,
            item: i,
            rect: drop(self, m),
            items: menus[m].1.iter().map(|s| s.to_string()).collect(),
        });
        if self.menu.is_some() != self.menu_was_open || chosen.is_some() {
            self.redraw = true;
        }
        chosen
    }

    fn face_width_estimate(&self, text: &str, px: f32) -> f32 {
        // Without the cache (immutable): an average advance
        text.chars().count() as f32 * px * 0.55
    }

    /// A modal dialog over the window (the message may have several lines):
    /// Some(button index) when a button is chosen (Esc: the last one). Call it every frame while it is up,
    /// after the rest of the window.
    pub fn dialog(&mut self, title: &str, message: &str, buttons: &[&str]) -> Option<usize> {
        self.modal_now = true;
        let (w, h) = (self.canvas.width(), self.canvas.height());
        let th = self.theme;
        let s = self.scale;
        // Dim the window behind
        let data = self.canvas.pixmap.data_mut();
        for p in data.chunks_exact_mut(4) {
            p[0] = (p[0] as u32 * 45 / 100) as u8;
            p[1] = (p[1] as u32 * 45 / 100) as u8;
            p[2] = (p[2] as u32 * 45 / 100) as u8;
        }
        let px = self.font_px();
        let lines: Vec<&str> = message.split('\n').collect();
        let widest = lines.iter().map(|l| self.face.width(l, px)).fold(0.0, f32::max);
        let dw = (widest + 48.0 * s).max(320.0 * s).min(w - 40.0 * s);
        let lh = self.line_h() + 4.0 * s;
        let dh = 126.0 * s + lines.len() as f32 * lh;
        let d = Rect::new(((w - dw) / 2.0).round(), ((h - dh) / 2.0).round(), dw.round(), dh);
        self.canvas.fill_rounded(Rect::new(d.x + 4.0 * s, d.y + 6.0 * s, d.w, d.h), 10.0 * s, 0x101118);
        self.canvas.fill_rounded(d, 10.0 * s, th.surface);
        self.canvas.stroke_rounded(d, 10.0 * s, 1.0, th.accent);
        self.draw_text(true, d.x + 24.0 * s, d.y + 20.0 * s, title, px * 1.15, th.text);
        for (i, l) in lines.iter().enumerate() {
            self.draw_text(false, d.x + 24.0 * s, d.y + 56.0 * s + i as f32 * lh, l, px, th.dim);
        }
        // The buttons, right-aligned; the first has the focus
        let bh = self.row_h();
        let mut x = d.right() - 24.0 * s;
        let mut rects = Vec::new();
        for b in buttons.iter().rev() {
            let bw = self.face.width(b, px) + 32.0 * s;
            x -= bw;
            rects.push(Rect::new(x, d.bottom() - bh - 20.0 * s, bw, bh));
            x -= 10.0 * s;
        }
        rects.reverse();
        let focus_id = hash(0xd1a1, title);
        let mut sel = self.texts.get(&focus_id).map(|t| t.cursor).unwrap_or(0).min(buttons.len() - 1);
        let mut chosen = None;
        for i in 0..self.input.keys.len() {
            if self.consumed.contains(&i) {
                continue;
            }
            self.consumed.insert(i);
            let k = &self.input.keys[i];
            match k.usage {
                keys::TAB | keys::RIGHT => sel = (sel + 1) % buttons.len(),
                keys::LEFT => sel = (sel + buttons.len() - 1) % buttons.len(),
                keys::ENTER | keys::KP_ENTER | keys::SPACE => chosen = Some(sel),
                keys::ESC => chosen = Some(buttons.len() - 1),
                _ => {}
            }
        }
        self.focus_move = 0; // Tab moved the dialog's focus, not the window's
        for (i, r) in rects.iter().enumerate() {
            let hot = r.contains(self.input.mouse.0, self.input.mouse.1);
            if hot && self.input.released {
                chosen = Some(i);
            }
            let primary = i == sel;
            self.canvas.fill_rounded(*r, 6.0 * s, if primary { th.accent } else if hot { th.surface_hi } else { th.background });
            self.canvas.stroke_rounded(*r, 6.0 * s, 1.0, th.border);
            let tw = self.face.width(buttons[i], px);
            let c = if primary { th.accent_text } else { th.text };
            self.draw_text(false, r.x + (r.w - tw) / 2.0, r.y + 6.0 * s, buttons[i], px, c);
        }
        self.texts.insert(focus_id, TextState { cursor: sel, anchor: 0, scroll: 0.0 });
        if chosen.is_some() {
            self.texts.remove(&focus_id);
            self.redraw = true;
        }
        chosen
    }

    /// Give the focus to a widget by name (e.g. the first field)
    pub fn set_focus(&mut self, name: &str) {
        self.focus = Some(self.id(name));
    }
}

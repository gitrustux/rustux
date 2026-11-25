// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! desk: the desktop's icons (DESKTOP.md step F12), a window in the
//! compositor's desktop role (the work area, below every window, drawn
//! over the wallpaper). The panel starts it with the session.
//!
//! - Icons: Home, Trash (full or empty), the drives mounted under /media,
//!   then what is in ~/Desktop: folders, files and app shortcuts (`.app`
//!   files, which the menu's "Add to the desktop" makes). They fill the
//!   left side in columns; one dragged elsewhere stays there (kept in
//!   ~/.config/rustica/desktop-icons).
//! - A click chooses one, a double click (or Enter) opens it: a folder in
//!   Files, a file with its application, an app by its command. Dropped on
//!   the trash, or with Delete, a file goes to the trash.
//! - A right click: on an icon, Open, Move to the trash (Empty the trash
//!   for the trash); on the desktop, New folder, Open in Terminal, Change
//!   background, Desktop settings.
//! - ~/.config/rustica/desktop.conf: icons=no hides them all; home=,
//!   trash=, drives= no hide those.

use rustica_apps::sys;
use rustica_ui::gfx::Rect;
use rustica_ui::rustica_window::ROLE_DESKTOP;
use rustica_ui::ui::keys;
use rustica_ui::{Control, Icon, Request, Ui};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn say(s: &str) {
    println!("desk: {s}");
}

#[derive(Clone, PartialEq)]
enum Kind {
    Home,
    Trash,
    Drive,
    Folder,
    File,
    App,
}

#[derive(Clone)]
struct Item {
    /// What it is kept under in desktop-icons
    key: String,
    label: String,
    path: PathBuf,
    kind: Kind,
}

fn trash_dir() -> PathBuf {
    sys::home().join(".local/share/Trash")
}

fn trash_count() -> usize {
    std::fs::read_dir(trash_dir().join("files")).map(|r| r.count()).unwrap_or(0)
}

fn desktop_dir() -> PathBuf {
    sys::home().join("Desktop")
}

fn conf() -> HashMap<String, String> {
    sys::parse_kv(&std::fs::read_to_string(sys::config_dir().join("rustica/desktop.conf")).unwrap_or_default())
}

fn shown(c: &HashMap<String, String>, k: &str) -> bool {
    !matches!(c.get(k).map(String::as_str), Some("no") | Some("false") | Some("0"))
}

fn items() -> Vec<Item> {
    let c = conf();
    let mut v = Vec::new();
    if !shown(&c, "icons") {
        return v;
    }
    if shown(&c, "home") {
        v.push(Item { key: ":home".into(), label: "Home".into(), path: sys::home(), kind: Kind::Home });
    }
    if shown(&c, "trash") {
        v.push(Item { key: ":trash".into(), label: "Trash".into(), path: trash_dir().join("files"), kind: Kind::Trash });
    }
    if shown(&c, "drives") {
        if let Ok(rd) = std::fs::read_dir("/media") {
            let mut d: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            d.sort();
            for p in d {
                let n = p.file_name().unwrap().to_string_lossy().into_owned();
                v.push(Item { key: format!(":drive:{n}"), label: n, path: p, kind: Kind::Drive });
            }
        }
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(desktop_dir()).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.retain(|p| !p.file_name().map_or(true, |n| n.to_string_lossy().starts_with('.')));
    files.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
    for p in files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let (kind, label) = if p.is_dir() {
            (Kind::Folder, name.clone())
        } else if name.ends_with(".app") {
            let t = std::fs::read_to_string(&p).unwrap_or_default();
            let n = sys::parse_kv(&t).get("name").cloned().unwrap_or_else(|| name.trim_end_matches(".app").to_string());
            (Kind::App, n)
        } else {
            (Kind::File, name.clone())
        };
        v.push(Item { key: name, label, path: p, kind });
    }
    v
}

fn positions_path() -> PathBuf {
    sys::config_dir().join("rustica/desktop-icons")
}

/// Where icons were dragged: key -> cell (column, row)
fn load_positions() -> HashMap<String, (i32, i32)> {
    let mut m = HashMap::new();
    for l in std::fs::read_to_string(positions_path()).unwrap_or_default().lines() {
        let mut w = l.rsplitn(3, ' ');
        if let (Some(y), Some(x), Some(k)) = (w.next(), w.next(), w.next()) {
            if let (Ok(x), Ok(y)) = (x.parse(), y.parse()) {
                m.insert(k.to_string(), (x, y));
            }
        }
    }
    m
}

fn save_positions(m: &HashMap<String, (i32, i32)>) {
    let p = positions_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let mut out: Vec<String> = m.iter().map(|(k, (x, y))| format!("{k} {x} {y}")).collect();
    out.sort();
    let _ = std::fs::write(p, out.join("\n") + "\n");
}

fn open(it: &Item) {
    match it.kind {
        Kind::Home | Kind::Trash | Kind::Drive | Kind::Folder => {
            let _ = std::fs::create_dir_all(&it.path);
            let _ = sys::launch("/bin/files", &[&it.path.to_string_lossy()], None);
            say(&format!("open {}", it.path.display()));
        }
        Kind::App => {
            let t = std::fs::read_to_string(&it.path).unwrap_or_default();
            if let Some(exec) = sys::parse_kv(&t).get("exec") {
                let mut w = exec.split_whitespace();
                if let Some(p) = w.next() {
                    let args: Vec<&str> = w.collect();
                    let _ = sys::launch(p, &args, None);
                    say(&format!("start {exec}"));
                }
            }
        }
        Kind::File => match sys::app_for(&it.path) {
            Some(app) => {
                let _ = sys::launch(&app.exec, &[&it.path.to_string_lossy()], None);
                say(&format!("open {} with {}", it.path.display(), app.exec));
            }
            None => {
                // Files, with the file's folder
                let _ = sys::launch("/bin/files", &[&desktop_dir().to_string_lossy()], None);
                say(&format!("no application for {}", it.path.display()));
            }
        },
    }
}

/// Move a file to the trash (freedesktop.org layout, as Files does)
fn to_trash(p: &Path) -> bool {
    let t = trash_dir();
    let _ = std::fs::create_dir_all(t.join("files"));
    let _ = std::fs::create_dir_all(t.join("info"));
    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut to = t.join("files").join(&name);
    let mut n = 1;
    while std::fs::symlink_metadata(&to).is_ok() {
        to = t.join("files").join(format!("{name} ({n})"));
        n += 1;
    }
    if std::fs::rename(p, &to).is_err() {
        return false;
    }
    let tm = sys::local(sys::now());
    let info = format!(
        "[Trash Info]\nPath={}\nDeletionDate={:04}-{:02}-{:02}T{:02}:{:02}:{:02}\n",
        p.display(),
        tm.year + 1900,
        tm.mon + 1,
        tm.mday,
        tm.hour,
        tm.min,
        tm.sec
    );
    let tname = to.file_name().unwrap().to_string_lossy().into_owned();
    let _ = std::fs::write(t.join("info").join(format!("{tname}.trashinfo")), info);
    say(&format!("trash {}", p.display()));
    true
}

fn empty_trash() {
    for sub in ["files", "info"] {
        if let Ok(rd) = std::fs::read_dir(trash_dir().join(sub)) {
            for e in rd.flatten() {
                let p = e.path();
                let _ = if p.is_dir() && !p.is_symlink() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
    }
    say("trash emptied");
}

/// A menu the desktop draws itself, at the pointer
struct Menu {
    x: f32,
    y: f32,
    /// The icon it is for (None: the desktop)
    item: Option<usize>,
    entries: Vec<&'static str>,
}

const CELL_W: f32 = 96.0;
const CELL_H: f32 = 92.0;
const ICON: f32 = 44.0;

fn main() {
    let mut list = items();
    let mut pos = load_positions();
    let mut sel: Option<usize> = None;
    let mut first = true;
    let mut menu: Option<Menu> = None;
    let mut last_click: Option<(usize, Instant)> = None;
    // A drag: the icon, where the pointer went down, whether it moved
    let mut drag: Option<(usize, (f32, f32), bool)> = None;
    let mut scanned = Instant::now();
    let mut trash_n = trash_count();
    let r = rustica_ui::run("Desktop", 800, 600, Some((0, 0)), |ui: &mut Ui, ctl: &mut Control| {
        if first {
            ui.transparent = true;
            ctl.requests.push(Request::SetRole(ROLE_DESKTOP));
            first = false;
            say(&format!("{} icons", list.len()));
        }
        ctl.wake_after = Some(Duration::from_secs(2));
        if scanned.elapsed() >= Duration::from_secs(2) {
            let n = items();
            let keys_now: Vec<&String> = n.iter().map(|i| &i.key).collect();
            let keys_old: Vec<&String> = list.iter().map(|i| &i.key).collect();
            if keys_now != keys_old || n.iter().zip(&list).any(|(a, b)| a.label != b.label) {
                list = n;
                sel = None;
                say(&format!("{} icons", list.len()));
            }
            trash_n = trash_count();
            scanned = Instant::now();
        }
        let s = ui.scale;
        let (cw, ch) = (CELL_W * s, CELL_H * s);
        let rows = ((ui.canvas.height() - 8.0 * s) / ch).floor().max(1.0) as i32;
        // Each icon's cell: where it was dragged, else the next free one
        // down the columns
        let mut cells: Vec<(i32, i32)> = Vec::new();
        let mut taken: Vec<(i32, i32)> = list.iter().filter_map(|it| pos.get(&it.key).copied()).collect();
        let mut next = 0i32;
        for it in &list {
            if let Some(&c) = pos.get(&it.key) {
                cells.push(c);
                continue;
            }
            loop {
                let c = (next / rows, next % rows);
                next += 1;
                if !taken.contains(&c) {
                    taken.push(c);
                    cells.push(c);
                    break;
                }
            }
        }
        let rect = |c: (i32, i32)| Rect::new(8.0 * s + c.0 as f32 * cw, 8.0 * s + c.1 as f32 * ch, cw - 4.0 * s, ch - 4.0 * s);
        let (mx, my) = ui.input.mouse;
        let under = (0..list.len()).find(|&i| {
            let r = rect(cells[i]);
            mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h
        });

        // The menu takes the pointer while it is open
        let mut menu_choice: Option<(Option<usize>, &'static str)> = None;
        let mut menu_close = false;
        if let Some(m) = &menu {
            let th = ui.theme;
            let rh = ui.row_h();
            let mw = m.entries.iter().map(|e| ui.text_width(e)).fold(0.0, f32::max) + 40.0 * s;
            let mh = rh * m.entries.len() as f32 + 8.0 * s;
            let x = m.x.min(ui.canvas.width() - mw - 2.0);
            let y = m.y.min(ui.canvas.height() - mh - 2.0);
            let box_ = Rect::new(x, y, mw, mh);
            ui.canvas.fill_rounded(box_, 6.0 * s, th.border);
            ui.canvas.fill_rounded(Rect::new(x + 1.0, y + 1.0, mw - 2.0, mh - 2.0), 6.0 * s, th.surface);
            let px = ui.font_px();
            let lh = ui.line_h();
            for (i, e) in m.entries.iter().enumerate() {
                let r = Rect::new(x + 4.0 * s, y + 4.0 * s + i as f32 * rh, mw - 8.0 * s, rh);
                let hot = mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h;
                if hot {
                    ui.canvas.fill_rounded(r, 4.0 * s, th.selection);
                    if ui.input.released || ui.input.pressed {
                        menu_choice = Some((m.item, e));
                    }
                }
                ui.canvas.text(&mut ui.face, r.x + 12.0 * s, r.y + (rh - lh) / 2.0, e, px, th.text);
            }
            let inside = mx >= box_.x && mx < box_.x + box_.w && my >= box_.y && my < box_.y + box_.h;
            if (ui.input.pressed || ui.input.right_pressed) && !inside {
                menu_close = true;
            }
            if ui.input.keys.iter().any(|k| k.usage == keys::ESC) {
                menu_close = true;
            }
        }

        // The icons
        let th = ui.theme;
        let px = ui.font_px();
        for (i, it) in list.iter().enumerate() {
            let mut r = rect(cells[i]);
            if let Some((d, start, true)) = drag {
                if d == i {
                    r.x += mx - start.0;
                    r.y += my - start.1;
                }
            }
            if sel == Some(i) || (under == Some(i) && menu.is_none()) {
                let c = if sel == Some(i) { th.selection } else { th.surface };
                ui.canvas.fill_rounded(r, 8.0 * s, c);
            }
            let icon = match it.kind {
                Kind::Home => Icon::Home,
                Kind::Trash => {
                    if trash_n > 0 {
                        Icon::TrashFull
                    } else {
                        Icon::Trash
                    }
                }
                Kind::Drive => Icon::Drive,
                Kind::Folder => Icon::Folder,
                Kind::App => Icon::Program,
                Kind::File => {
                    let ext = it.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    if ["png", "jpg", "jpeg", "qoi"].contains(&ext.as_str()) {
                        Icon::Image
                    } else {
                        Icon::File
                    }
                }
            };
            let isz = ICON * s;
            ui.icon(icon, r.x + (r.w - isz) / 2.0, r.y + 6.0 * s, isz);
            // The name on a dark pill, so it reads on any wallpaper
            let label = ui.elide(&it.label, r.w - 12.0 * s, false);
            let tw = ui.text_width(&label);
            let ty = r.y + 6.0 * s + isz + 6.0 * s;
            let pill = Rect::new(r.x + (r.w - tw) / 2.0 - 6.0 * s, ty - 2.0 * s, tw + 12.0 * s, ui.line_h() + 4.0 * s);
            ui.canvas.fill_rounded(pill, 5.0 * s, if sel == Some(i) { th.accent } else { th.background });
            ui.canvas.text(&mut ui.face, pill.x + 6.0 * s, ty, &label, px, if sel == Some(i) { th.accent_text } else { th.text });
        }

        // The pointer, when no menu is open
        if menu.is_none() && !menu_close {
            if ui.input.pressed {
                sel = under;
                if let Some(i) = under {
                    drag = Some((i, (mx, my), false));
                    let now = Instant::now();
                    if let Some((j, t)) = last_click {
                        if j == i && t.elapsed() < Duration::from_millis(450) {
                            open(&list[i]);
                            last_click = None;
                            drag = None;
                        } else {
                            last_click = Some((i, now));
                        }
                    } else {
                        last_click = Some((i, now));
                    }
                }
            }
            if let Some((i, start, moved)) = drag.as_mut() {
                if !*moved && ((mx - start.0).abs() > 6.0 * s || (my - start.1).abs() > 6.0 * s) {
                    *moved = true;
                    let _ = i;
                }
            }
            if ui.input.released {
                if let Some((i, _, true)) = drag {
                    // Dropped: on the trash, or on a cell of its own
                    let tgt = under.filter(|&j| j != i);
                    if tgt.map_or(false, |j| list[j].kind == Kind::Trash) && matches!(list[i].kind, Kind::File | Kind::Folder | Kind::App) {
                        if to_trash(&list[i].path) {
                            trash_n = trash_count();
                            list = items();
                            sel = None;
                        }
                    } else {
                        let c = (((mx - 8.0 * s) / cw).floor().max(0.0) as i32, ((my - 8.0 * s) / ch).floor().clamp(0.0, rows as f32 - 1.0) as i32);
                        if !cells.iter().enumerate().any(|(j, &cc)| j != i && cc == c) {
                            pos.insert(list[i].key.clone(), c);
                            save_positions(&pos);
                            say(&format!("moved {} to {} {}", list[i].key, c.0, c.1));
                        }
                    }
                }
                drag = None;
            }
            if ui.input.right_pressed {
                sel = under;
                let entries: Vec<&'static str> = match under.map(|i| &list[i].kind) {
                    Some(Kind::Trash) => vec!["Open", "Empty the trash"],
                    Some(Kind::Home) | Some(Kind::Drive) => vec!["Open"],
                    Some(_) => vec!["Open", "Move to the trash"],
                    None => vec!["New folder", "Open in Terminal", "Change background...", "Desktop settings..."],
                };
                menu = Some(Menu { x: mx, y: my, item: under, entries });
                say(if under.is_some() { "icon menu" } else { "desktop menu" });
            }
            // The keyboard: Enter opens, Delete trashes, arrows choose
            for k in ui.input.keys.clone() {
                match k.usage {
                    keys::ENTER | keys::KP_ENTER => {
                        if let Some(i) = sel {
                            open(&list[i]);
                        }
                    }
                    keys::DELETE => {
                        if let Some(i) = sel.filter(|&i| matches!(list[i].kind, Kind::File | Kind::Folder | Kind::App)) {
                            if to_trash(&list[i].path) {
                                trash_n = trash_count();
                                list = items();
                                sel = None;
                            }
                        }
                    }
                    keys::DOWN | keys::RIGHT if !list.is_empty() => sel = Some(sel.map_or(0, |s| (s + 1).min(list.len() - 1))),
                    keys::UP | keys::LEFT if !list.is_empty() => sel = Some(sel.map_or(0, |s| s.saturating_sub(1))),
                    _ => {}
                }
            }
        }
        if menu_close {
            menu = None;
        }
        if let Some((item, choice)) = menu_choice {
            menu = None;
            say(&format!("menu: {choice}"));
            match choice {
                "Open" => {
                    if let Some(i) = item {
                        open(&list[i]);
                    }
                }
                "Move to the trash" => {
                    if let Some(i) = item {
                        if to_trash(&list[i].path) {
                            list = items();
                            sel = None;
                        }
                    }
                }
                "Empty the trash" => {
                    empty_trash();
                }
                "New folder" => {
                    let d = desktop_dir();
                    let mut p = d.join("New folder");
                    let mut n = 2;
                    while p.exists() {
                        p = d.join(format!("New folder {n}"));
                        n += 1;
                    }
                    let _ = std::fs::create_dir_all(&p);
                    say(&format!("new folder {}", p.display()));
                    list = items();
                }
                "Open in Terminal" => {
                    let d = desktop_dir();
                    let _ = std::fs::create_dir_all(&d);
                    let _ = sys::launch("/bin/terminal", &[], Some(&d));
                }
                "Change background..." => {
                    let _ = sys::launch("/bin/settings", &["appearance"], None);
                }
                "Desktop settings..." => {
                    let _ = sys::launch("/bin/settings", &["desktop"], None);
                }
                _ => {}
            }
            trash_n = trash_count();
        }
        if menu.is_some() || drag.map_or(false, |d| d.2) {
            ctl.wake_after = Some(Duration::from_millis(50));
        }
    });
    if let Err(e) = r {
        eprintln!("desk: {e}");
        std::process::exit(1);
    }
}

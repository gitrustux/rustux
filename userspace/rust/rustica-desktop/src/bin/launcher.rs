// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! launcher: the menu (DESKTOP.md steps F6 and F11), from the panel's Apps
//! button or the Super key, laid out as Cinnamon's:
//!
//! - a search field, focused at once: typing lists the apps (by name,
//!   description and keywords), Settings pages, actions (lock, log out,
//!   restart, shut down) and files in the home folder whose names match;
//!   Up and Down move, Enter opens, Esc closes
//! - without a search, the categories (All, Accessories, Administration,
//!   Development, Games, Graphics, Internet, Office, Preferences, Sound &
//!   Video, Other) and the apps of the one chosen
//! - a side bar: the favourites and the places (Home, Documents,
//!   Downloads, Pictures, Trash); the session's actions along the bottom
//! - for the app chosen: Open, Favourite, Pin to the panel, Add to the
//!   desktop
//!
//! Apps come from /usr/share/applications/*.app (lines "name=", "exec=",
//! "comment=", "categories=", "keywords=") and the apps installed in
//! /apps (FoxDen step FD1: each one's manifest, Name, Summary, Categories
//! and Exec relative to its folder; started in its sandbox by `appctl run
//! ID`, step FD2). Favourites are ~/.config/rustica/favorites (an app ID a
//! line). Choosing something starts it and closes the menu. Log out and
//! Lock are the panel's: the menu ends with status 10 or 11 for them.

use rustica_ui::ui::keys;
use rustica_ui::{Control, Ui};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub exec: Vec<String>,
    pub comment: String,
    pub category: &'static str,
    pub keywords: String,
}

const CATEGORIES: [&str; 11] =
    ["All", "Accessories", "Administration", "Development", "Games", "Graphics", "Internet", "Office", "Preferences", "Sound & Video", "Other"];

/// The menu's category for freedesktop.org category names
fn category(cats: &str) -> &'static str {
    let has = |c: &str| cats.split(|ch: char| ch == ';' || ch == ',' || ch.is_whitespace()).any(|x| x.eq_ignore_ascii_case(c));
    if has("Settings") {
        "Preferences"
    } else if has("System") || has("PackageManager") {
        "Administration"
    } else if has("Development") {
        "Development"
    } else if has("Game") {
        "Games"
    } else if has("Graphics") {
        "Graphics"
    } else if has("Network") {
        "Internet"
    } else if has("Office") {
        "Office"
    } else if has("AudioVideo") || has("Audio") || has("Video") {
        "Sound & Video"
    } else if has("Utility") || has("Accessories") || has("FileManager") {
        "Accessories"
    } else {
        "Other"
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into()))
}

pub fn apps() -> Vec<App> {
    let mut v = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/usr/share/applications") {
        for e in dir.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("app") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|v| v.trim().to_string())).unwrap_or_default();
            let (name, exec) = (get("name"), get("exec"));
            if name.is_empty() || exec.is_empty() {
                continue;
            }
            v.push(App {
                id: p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string(),
                name,
                exec: exec.split_whitespace().map(str::to_string).collect(),
                comment: get("comment"),
                category: category(&get("categories")),
                keywords: get("keywords"),
            });
        }
    }
    let system = v.len();
    if let Ok(dir) = std::fs::read_dir("/apps") {
        for e in dir.flatten() {
            let folder = e.path();
            let Ok(text) = std::fs::read_to_string(folder.join("manifest")) else { continue };
            let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(':'))).map(|v| v.trim().to_string()).unwrap_or_default();
            let (name, exec) = (field("Name"), field("Exec"));
            // Exec is inside the folder: no absolute path, no ..
            if name.is_empty() || exec.is_empty() || exec.starts_with('/') || exec.split_whitespace().next().unwrap_or("").split('/').any(|c| c == "..") {
                continue;
            }
            // Started in its sandbox (FoxDen step FD2): appctl reads the
            // manifest again and runs Exec with the app's view and job
            let id = e.file_name().to_string_lossy().to_string();
            v.push(App {
                exec: vec!["/bin/appctl".into(), "run".into(), id.clone()],
                id,
                name,
                comment: field("Summary"),
                category: category(&field("Categories")),
                keywords: String::new(),
            });
        }
    }
    println!("launcher: {} applications, {} apps", system, v.len() - system);
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

fn spawn(cmd: &[String]) -> bool {
    let Some((p, a)) = cmd.split_first() else { return false };
    match std::process::Command::new(p).args(a).spawn() {
        Ok(_) => {
            println!("launcher: started {}", cmd.join(" "));
            true
        }
        Err(e) => {
            eprintln!("launcher: {}: {e}", cmd.join(" "));
            false
        }
    }
}

fn favourites_path() -> PathBuf {
    home().join(".config/rustica/favorites")
}

fn favourites() -> Vec<String> {
    match std::fs::read_to_string(favourites_path()) {
        Ok(t) => t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
        Err(_) => vec!["files".into(), "terminal".into(), "settings".into(), "foxden".into()],
    }
}

fn save_favourites(f: &[String]) {
    let p = favourites_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(&p, f.join("\n") + "\n");
}

/// Add an app to the panel's launchers (~/.config/rustica/panel.conf)
fn pin_to_panel(id: &str) {
    let user = home().join(".config/rustica/panel.conf");
    let text = std::fs::read_to_string(&user).or_else(|_| std::fs::read_to_string("/etc/rustica/panel.conf")).unwrap_or_default();
    let mut found = false;
    let mut out = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("launchers=") {
            found = true;
            let mut l: Vec<&str> = v.split_whitespace().collect();
            if !l.contains(&id) {
                l.push(id);
            }
            out.push_str(&format!("launchers={}\n", l.join(" ")));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found {
        out.push_str(&format!("launchers=foxden files terminal {id}\n"));
    }
    let _ = std::fs::create_dir_all(user.parent().unwrap());
    let _ = std::fs::write(&user, out);
    println!("launcher: pinned {id} to the panel");
}

/// A shortcut on the desktop: ~/Desktop/ID.app (DESKTOP.md step F12)
fn add_to_desktop(a: &App) {
    let dir = home().join("Desktop");
    let _ = std::fs::create_dir_all(&dir);
    let text = format!("name={}\nexec={}\ncomment={}\n", a.name, a.exec.join(" "), a.comment);
    let _ = std::fs::write(dir.join(format!("{}.app", a.id)), text);
    println!("launcher: {} on the desktop", a.id);
}

/// Files under the home folder whose names contain `q` (3 levels, at most
/// 12, hidden ones left out)
fn find_files(q: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(home(), 0)];
    let mut seen = 0;
    while let Some((d, depth)) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 3000 || out.len() >= 12 {
                return out;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if name.contains(q) {
                out.push(p.clone());
            }
            if depth < 3 && p.is_dir() {
                dirs.push((p, depth + 1));
            }
        }
    }
    out
}

const SETTINGS_PAGES: [&str; 14] =
    ["Display", "Appearance", "Panel", "Desktop", "Keyboard", "Mouse", "Power", "Network", "Date & Time", "Users", "Disks", "Updates", "Apps", "About"];

/// Something a search found
#[derive(Clone)]
enum Hit {
    App(usize),
    Setting(&'static str),
    Action(&'static str),
    File(PathBuf),
}

const ACTIONS: [&str; 4] = ["Lock screen", "Log out", "Restart", "Shut down"];

fn search(list: &[App], q: &str) -> Vec<Hit> {
    let q = q.trim().to_lowercase();
    let mut hits = Vec::new();
    let mut later = Vec::new();
    for (i, a) in list.iter().enumerate() {
        let name = a.name.to_lowercase();
        if name.starts_with(&q) {
            hits.push(Hit::App(i));
        } else if name.contains(&q) || a.comment.to_lowercase().contains(&q) || a.keywords.to_lowercase().contains(&q) {
            later.push(Hit::App(i));
        }
    }
    hits.extend(later);
    for p in SETTINGS_PAGES {
        if p.to_lowercase().contains(&q) || (q.len() >= 3 && "settings".starts_with(&q)) {
            hits.push(Hit::Setting(p));
        }
    }
    for a in ACTIONS {
        if a.to_lowercase().split(' ').any(|w| w.starts_with(&q)) {
            hits.push(Hit::Action(a));
        }
    }
    if q.len() >= 2 {
        hits.extend(find_files(&q).into_iter().map(Hit::File));
    }
    hits
}

/// What a click or Enter does
enum Go {
    Nothing,
    Close,
    /// Close with this status, for the panel (10 log out, 11 lock)
    Exit(i32),
}

fn act(list: &[App], h: &Hit) -> Go {
    match h {
        Hit::App(i) => {
            spawn(&list[*i].exec);
            Go::Close
        }
        Hit::Setting(p) => {
            spawn(&["/bin/settings".into(), p.to_lowercase()]);
            Go::Close
        }
        Hit::File(p) => {
            // Files, in the folder (or the file's folder)
            let dir = if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_else(home) };
            spawn(&["/bin/files".into(), dir.to_string_lossy().into_owned()]);
            Go::Close
        }
        Hit::Action(a) => match *a {
            "Lock screen" => Go::Exit(11),
            "Log out" => Go::Exit(10),
            "Restart" => {
                spawn(&["/bin/reboot".into()]);
                Go::Close
            }
            "Shut down" => {
                spawn(&["/bin/poweroff".into()]);
                Go::Close
            }
            _ => Go::Nothing,
        },
    }
}

fn hit_label(list: &[App], h: &Hit) -> String {
    match h {
        Hit::App(i) => list[*i].name.clone(),
        Hit::Setting(p) => format!("Settings: {p}"),
        Hit::Action(a) => a.to_string(),
        Hit::File(p) => {
            let h = home();
            let shown = p.strip_prefix(&h).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| p.display().to_string());
            format!("File: {shown}")
        }
    }
}

/// Where the menu goes: near the panel's Apps button
fn position() -> (i32, i32) {
    let user = home().join(".config/rustica/panel.conf");
    let text = std::fs::read_to_string(&user).or_else(|_| std::fs::read_to_string("/etc/rustica/panel.conf")).unwrap_or_default();
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|v| v.trim().to_string()));
    let width: i32 = get("width").and_then(|v| v.parse().ok()).unwrap_or(180);
    let size: i32 = get("size").and_then(|v| v.parse().ok()).unwrap_or(36);
    match get("position").as_deref() {
        Some("left") => (width + 12, 40),
        Some("top") => (12, size + 40),
        _ => (12, 40),
    }
}

fn main() {
    let list = apps();
    let mut favs = favourites();
    let mut query = String::new();
    let mut hits: Vec<Hit> = Vec::new();
    let mut cat: Option<usize> = Some(0);
    let mut sel: Option<usize> = if list.is_empty() { None } else { Some(0) };
    let mut first = true;
    let mut status = String::new();
    let mut exit_code = 0;
    let cat_names: Vec<String> = CATEGORIES.iter().map(|c| c.to_string()).collect();
    let r = rustica_ui::run("Menu", 680, 470, Some(position()), |ui: &mut Ui, ctl: &mut Control| {
        if first {
            ui.set_focus("search");
            first = false;
        }
        // Up, Down and Enter reach the list from the search field; Esc closes
        let mut moved = 0i32;
        let mut enter = false;
        let mut rest = Vec::new();
        for k in std::mem::take(&mut ui.input.keys) {
            if k.usage == keys::DOWN {
                moved += 1;
            } else if k.usage == keys::UP {
                moved -= 1;
            } else if k.usage == keys::ENTER || k.usage == keys::KP_ENTER {
                enter = true;
            } else if k.usage == keys::ESC {
                ctl.quit = true;
            } else {
                rest.push(k);
            }
        }
        ui.input.keys = rest;

        // What the right side lists: the search's hits, or a category's apps
        let shown: Vec<Hit> = if query.trim().is_empty() {
            let c = CATEGORIES[cat.unwrap_or(0)];
            list.iter().enumerate().filter(|(_, a)| c == "All" || a.category == c).map(|(i, _)| Hit::App(i)).collect()
        } else {
            hits.clone()
        };
        if moved != 0 && !shown.is_empty() {
            let cur = sel.map(|s| s as i32).unwrap_or(-1);
            sel = Some((cur + moved).clamp(0, shown.len() as i32 - 1) as usize);
        }
        if sel.map_or(false, |s| s >= shown.len()) {
            sel = None;
        }
        let mut go = Go::Nothing;
        ui.row(|ui| {
            // The side bar
            let side = ui.u(150.0);
            ui.column(side, |ui| {
                ui.dim_label("Favourites");
                for id in favs.clone() {
                    if let Some(a) = list.iter().find(|a| a.id == id) {
                        if ui.button_ex(&format!("fav-{id}"), &a.name, side, false, false).clicked {
                            spawn(&a.exec);
                            go = Go::Close;
                        }
                    }
                }
                ui.dim_label("Places");
                for (name, rel) in [("Home", ""), ("Documents", "Documents"), ("Downloads", "Downloads"), ("Pictures", "Pictures"), ("Trash", ".local/share/Trash/files")] {
                    if ui.button_ex(&format!("place-{name}"), name, side, false, false).clicked {
                        let p = home().join(rel);
                        let _ = std::fs::create_dir_all(&p);
                        spawn(&["/bin/files".into(), p.to_string_lossy().into_owned()]);
                        go = Go::Close;
                    }
                }
            });
            ui.column(0.0, |ui| {
                let (changed, submitted) = ui.text_field("search", &mut query, 0.0);
                if changed {
                    hits = if query.trim().is_empty() { Vec::new() } else { search(&list, &query) };
                    sel = if query.trim().is_empty() { None } else if hits.is_empty() { None } else { Some(0) };
                    if !query.trim().is_empty() {
                        println!("launcher: search \"{}\": {} found", query.trim(), hits.len());
                    }
                }
                enter |= submitted;
                let h = (ui.remaining_height() / ui.scale - 92.0).max(80.0);
                let labels: Vec<String> = shown.iter().map(|x| hit_label(&list, x)).collect();
                ui.row(|ui| {
                    if query.trim().is_empty() {
                        ui.column(ui.u(140.0), |ui| {
                            let before = cat;
                            ui.list("categories", &cat_names, &mut cat, h);
                            if cat != before {
                                sel = None;
                            }
                        });
                    }
                    ui.column(0.0, |ui| {
                        if labels.is_empty() {
                            ui.dim_label(if query.trim().is_empty() { "No apps here." } else { "Nothing found." });
                        } else {
                            ui.list("apps", &labels, &mut sel, h);
                        }
                    });
                });
                // What is chosen: its description and what can be done
                let cur = sel.and_then(|s| shown.get(s)).cloned();
                match &cur {
                    Some(Hit::App(i)) => {
                        let a = &list[*i];
                        ui.dim_label(if !status.is_empty() { &status } else if a.comment.is_empty() { " " } else { &a.comment });
                        let fav = favs.contains(&a.id);
                        ui.row(|ui| {
                            if ui.primary_button("Open") {
                                go = act(&list, &Hit::App(*i));
                            }
                            if ui.button(if fav { "Unfavourite" } else { "Favourite" }) {
                                if fav {
                                    favs.retain(|f| f != &a.id);
                                } else {
                                    favs.push(a.id.clone());
                                }
                                save_favourites(&favs);
                                println!("launcher: favourites {}", favs.join(","));
                            }
                            if ui.button("Pin to panel") {
                                pin_to_panel(&a.id);
                                status = format!("{} is on the panel", a.name);
                            }
                            if ui.button("To desktop") {
                                add_to_desktop(a);
                                status = format!("{} is on the desktop", a.name);
                            }
                        });
                    }
                    Some(h) => {
                        let h = h.clone();
                        ui.dim_label(" ");
                        if ui.primary_button("Open") {
                            go = act(&list, &h);
                        }
                    }
                    None => {
                        ui.dim_label(if status.is_empty() { "Type to search apps, settings and files" } else { &status });
                    }
                }
                // The session
                ui.row(|ui| {
                    for a in ACTIONS {
                        if ui.button(a) {
                            go = act(&list, &Hit::Action(a));
                        }
                    }
                });
            });
        });
        if enter {
            if let Some(h) = sel.and_then(|s| shown.get(s)) {
                go = act(&list, h);
            }
        }
        match go {
            Go::Nothing => {}
            Go::Close => ctl.quit = true,
            Go::Exit(c) => {
                exit_code = c;
                ctl.quit = true;
            }
        }
    });
    if let Err(e) = r {
        eprintln!("launcher: {e}");
        std::process::exit(1);
    }
    std::process::exit(exit_code);
}

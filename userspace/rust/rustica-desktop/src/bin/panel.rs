// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! panel: the desktop's panel (DESKTOP.md steps F6 and F10), docked at an
//! edge of the screen (bottom by default; top, left or right) and made of
//! applets in three zones, start, centre and end, as Cinnamon's:
//!
//! - menu: the fox button (the launcher; the Super key alone does the same)
//! - launchers: one button per pinned app (FoxDen, Files, Terminal)
//! - windows: a button per window (click: focus it, or minimise it if it
//!   has the focus); they share what room is left, their titles shortened,
//!   and past a minimum width a "+N" button stands for the rest, so the
//!   end of the panel never leaves the screen
//! - updates: "Updates (N)" when the installer service found some (package
//!   step E6), a notification when N grows
//! - keyboard (the layout), volume (not there yet), network (Net: the
//!   interface and its address; a click opens Settings > Network), clock,
//!   lock, logout, power (the power menu: Lock, Log out, Restart, Shut
//!   down, Power settings; the far end of the panel by default)
//!
//! Its settings are in ~/.config/rustica/panel.conf (else
//! /etc/rustica/panel.conf), read again when they change: position=
//! bottom|top|left|right, size= (the thickness of a bottom or top panel),
//! width= (of a side panel), autohide=yes|no, start=, centre=, end= (the
//! applets, in order), launchers= (names of /usr/share/applications/*.app,
//! or FoxDen app IDs). Settings > Panel writes them; a right click on the
//! panel opens it. The session lasts as long as the panel: Log out ends
//! it. Apps updated in the background, and app updates that wait because
//! they ask for more, are notified (FoxDen step FD6).
//!
//! It also starts the programs in /etc/autostart (one command a line).

use rustica_ui::rustica_window::{Event, EDGE_BOTTOM, EDGE_LEFT, EDGE_RIGHT, EDGE_TOP, ROLE_PANEL};
use rustica_ui::{Control, Request, Ui};
use std::time::{Duration, Instant, SystemTime};

/// The clock in the user's formats (Settings > Date & Time): the time,
/// and the date under it (2026-11-26, as Windows' taskbar)
fn clock() -> (String, Option<String>) {
    let f = rustica_ui::clock::Formats::current();
    f.panel_lines(&rustica_ui::clock::local(rustica_ui::clock::now()))
}

/// The clock's two lines share the buttons' height: a smaller size than
/// the panel's other text, so both fit
fn clock_px(ui: &mut Ui) -> f32 {
    let px = ui.font_px();
    let ratio = ui.face.line(px).2 / px;
    ((ui.row_h() + ui.u(6.0)) / 2.0 / ratio).min(px)
}

fn clock_width(ui: &mut Ui) -> f32 {
    match clock() {
        (t, None) => ui.text_width(&t),
        (t, Some(d)) => {
            let px = clock_px(ui);
            ui.face.width(&t, px).max(ui.face.width(&d, px))
        }
    }
}

/// The time above the date, each line centred
fn clock_applet(ui: &mut Ui, vertical: bool) {
    let (t, d) = clock();
    let Some(d) = d else {
        status(ui, &t, vertical);
        return;
    };
    let th = ui.theme;
    let px = clock_px(ui);
    let lh = ui.face.line(px).2;
    let (tw, dw) = (ui.face.width(&t, px), ui.face.width(&d, px));
    let w = tw.max(dw);
    let bh = if vertical { (lh * 2.0).ceil() } else { ui.row_h() };
    let r = ui.alloc(if vertical { 0.0 } else { w }, bh);
    let top = r.y + (bh - lh * 2.0) / 2.0;
    let cx = r.x + r.w / 2.0;
    ui.canvas.text(&mut ui.face, cx - tw / 2.0, top, &t, px, th.text);
    ui.canvas.text(&mut ui.face, cx - dw / 2.0, top + lh, &d, px, th.dim);
}

struct Win {
    id: i32,
    title: String,
    /// The app it belongs to (SET_APP): an app's ID or a .app file's name
    app: String,
    focused: bool,
    minimized: bool,
}

/// How many packages the installer service's last check found to upgrade
fn updates() -> usize {
    std::fs::read_to_string("/var/lib/rpg/updates").map(|t| t.lines().filter(|l| l.starts_with("step ")).count()).unwrap_or(0)
}

/// An installed app's name (its manifest's), or its ID
fn app_name(id: &str) -> String {
    std::fs::read_to_string(format!("/apps/{id}/manifest"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Name:").map(|v| v.trim().to_string())))
        .unwrap_or_else(|| id.to_string())
}

/// The installer service's app lines (FoxDen step FD6): apps updated in
/// the background (appupdated SECONDS ID FROM TO) and updates waiting
/// because they ask for more (appwaits ID FROM TO PERMISSIONS)
fn app_news() -> (Vec<(u64, String, String)>, Vec<(String, String, String)>) {
    let t = std::fs::read_to_string("/var/lib/rpg/updates").unwrap_or_default();
    let mut done = Vec::new();
    let mut waits = Vec::new();
    for l in t.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.first() {
            Some(&"appupdated") if w.len() >= 5 => done.push((w[1].parse().unwrap_or(0), w[2].to_string(), w[4].to_string())),
            Some(&"appwaits") if w.len() >= 5 => waits.push((w[1].to_string(), w[3].to_string(), w[4].replace(',', ", "))),
            _ => {}
        }
    }
    (done, waits)
}

/// The keyboard layout (/etc/keymap; Settings or localectl change it)
fn layout() -> String {
    let l = std::fs::read_to_string("/etc/keymap").map(|s| s.trim().to_string()).unwrap_or_default();
    if l.is_empty() { "us".to_string() } else { l }
}

/// RX_WINDOW_DESKTOP_LAUNCHER: the compositor saw the Super key alone
const LAUNCHER_KEY: u32 = 100;

/// The fox button and the Super key: open the menu, or close it if it is
/// open
fn toggle_launcher(launcher: &mut Option<std::process::Child>) {
    toggle(launcher, "/bin/launcher", &[], "launcher");
}

/// Start a program, or end it if it is running (a menu's button)
fn toggle(slot: &mut Option<std::process::Child>, prog: &str, args: &[&str], what: &str) {
    if let Some(mut c) = slot.take() {
        if matches!(c.try_wait(), Ok(None)) {
            println!("panel: {what} closed");
            let _ = c.kill();
            let _ = c.wait();
            return;
        }
    }
    println!("panel: {what}");
    *slot = std::process::Command::new(prog).args(args).spawn().ok();
}

/// The panel's settings
#[derive(Clone, PartialEq)]
struct Config {
    edge: u32,
    size: i32,
    width: i32,
    autohide: bool,
    zones: [Vec<String>; 3],
    launchers: Vec<String>,
}

fn config_paths() -> Vec<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    vec![std::path::Path::new(&home).join(".config/rustica/panel.conf"), "/etc/rustica/panel.conf".into()]
}

fn load_config() -> Config {
    let mut c = Config {
        edge: EDGE_BOTTOM,
        size: 36,
        width: 180,
        autohide: false,
        zones: [
            vec!["menu".into(), "launchers".into(), "windows".into()],
            vec![],
            // Lock and Log out are in the power menu (2026-11-26); they
            // can still be added as applets of their own
            vec!["updates".into(), "keyboard".into(), "volume".into(), "network".into(), "clock".into(), "power".into()],
        ],
        launchers: vec!["foxden".into(), "files".into(), "terminal".into()],
    };
    let text = config_paths().iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let words = |v: &str| v.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "position" => {
                c.edge = match v {
                    "top" => EDGE_TOP,
                    "left" => EDGE_LEFT,
                    "right" => EDGE_RIGHT,
                    _ => EDGE_BOTTOM,
                }
            }
            "size" => c.size = v.parse().unwrap_or(36).clamp(28, 96),
            "width" => c.width = v.parse().unwrap_or(180).clamp(48, 400),
            "autohide" => c.autohide = v == "yes" || v == "true" || v == "1",
            "start" => c.zones[0] = words(v),
            "centre" | "center" => c.zones[1] = words(v),
            "end" => c.zones[2] = words(v),
            "launchers" => c.launchers = words(v),
            _ => {}
        }
    }
    c
}

fn config_mtime() -> Option<SystemTime> {
    config_paths().iter().find_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// A pinned app: its key (a .app file's name or an app's ID), its name
/// and the command that starts it
struct Launcher {
    key: String,
    name: String,
    exec: Vec<String>,
}

/// Where an app's icon is: an installed app's manifest (Icon:, in its
/// folder), or a .app file's icon= (2026-11-27); FoxDen and programs
/// without one show their name
fn icon_path(key: &str) -> Option<String> {
    if key.is_empty() || key.contains('/') {
        return None;
    }
    if let Ok(t) = std::fs::read_to_string(format!("/apps/{key}/manifest")) {
        let icon = t.lines().find_map(|l| l.strip_prefix("Icon:").map(|v| v.trim().to_string()))?;
        if icon.is_empty() || icon.contains("..") {
            return None;
        }
        return Some(format!("/apps/{key}/{icon}"));
    }
    let t = std::fs::read_to_string(format!("/usr/share/applications/{key}.app")).ok()?;
    t.lines().find_map(|l| l.strip_prefix("icon=").map(|v| v.trim().to_string())).filter(|v| !v.is_empty())
}

/// Icons decoded once, then scaled to the panel's size (None: none)
#[derive(Default)]
struct Icons {
    decoded: std::collections::HashMap<String, Option<rustica_ui::gfx::Pixmap>>,
    scaled: std::collections::HashMap<(String, u32), Option<rustica_ui::gfx::Pixmap>>,
}

impl Icons {
    fn get(&mut self, key: &str, size: u32) -> Option<rustica_ui::gfx::Pixmap> {
        let k = (key.to_string(), size);
        if let Some(p) = self.scaled.get(&k) {
            return p.clone();
        }
        let img = self
            .decoded
            .entry(key.to_string())
            .or_insert_with(|| icon_path(key).and_then(|p| std::fs::read(p).ok()).and_then(|d| rustica_ui::gfx::decode_image(&d).ok()))
            .clone();
        let p = img.and_then(|i| rustica_ui::gfx::scale_square(&i, size));
        self.scaled.insert(k, p.clone());
        p
    }
}

/// The pinned apps into the user's panel settings (a right click's Pin
/// to taskbar or Unpin from taskbar): the launchers= line replaced, the
/// rest kept (the system's settings copied first if the user has none)
fn save_launchers(list: &[String]) {
    let paths = config_paths();
    let user = &paths[0];
    let old = std::fs::read_to_string(user).or_else(|_| std::fs::read_to_string(&paths[1])).unwrap_or_default();
    let mut out: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("launchers=")).map(str::to_string).collect();
    out.push(format!("launchers={}", list.join(",")));
    if let Some(dir) = user.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = user.with_extension("conf.new");
    if std::fs::write(&tmp, out.join("\n") + "\n").is_ok() {
        let _ = std::fs::rename(&tmp, user);
    }
}

fn launcher(id: &str) -> Option<Launcher> {
    if let Ok(t) = std::fs::read_to_string(format!("/usr/share/applications/{id}.app")) {
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|v| v.trim().to_string()));
        let exec = get("exec")?;
        return Some(Launcher { key: id.into(), name: get("name").unwrap_or_else(|| id.into()), exec: exec.split_whitespace().map(str::to_string).collect() });
    }
    if std::path::Path::new(&format!("/apps/{id}/manifest")).exists() {
        return Some(Launcher { key: id.into(), name: app_name(id), exec: vec!["/bin/appctl".into(), "run".into(), id.into()] });
    }
    None
}

fn start(cmd: &[String]) {
    if let Some((p, a)) = cmd.split_first() {
        println!("panel: start {}", cmd.join(" "));
        let _ = std::process::Command::new(p).args(a).spawn();
    }
}

fn main() {
    let user = std::env::var("USER").unwrap_or_default();
    if let Ok(list) = std::fs::read_to_string("/etc/autostart") {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut p = line.split_whitespace();
            if let Some(prog) = p.next() {
                let _ = std::process::Command::new(prog).args(p).spawn();
            }
        }
    }
    let mut wins: Vec<Win> = Vec::new();
    let mut first = true;
    let mut cfg = load_config();
    let mut cfg_mtime = config_mtime();
    let mut cfg_at = Instant::now();
    let mut launchers: Vec<Launcher> = cfg.launchers.iter().filter_map(|l| launcher(l)).collect();
    // The network's state, asked again every 3 seconds
    let mut net = String::new();
    let mut link = rustica_ui::net::Link::Off;
    // The sound service's volume, asked again every 2 seconds
    let mut sound: Option<rustica_ui::sound::Status> = None;
    let mut sound_at: Option<Instant> = None;
    let mut net_at: Option<Instant> = None;
    // The updates found, looked at again every 30 seconds
    let mut ups = 0usize;
    let mut ups_at: Option<Instant> = None;
    // What was said already of the apps (FoxDen step FD6): updates older
    // than the session are not news
    let (done0, waits0) = app_news();
    let mut seen_updated = done0.iter().map(|d| d.0).max().unwrap_or(0);
    let mut seen_waits: Vec<String> = waits0.iter().map(|w| format!("{} {}", w.0, w.1)).collect();
    // The "+N" button: which of the windows that did not fit comes next
    let mut more_next = 0usize;
    println!("panel: session of {user}");
    let mut launcher_proc: Option<std::process::Child> = None;
    let mut power_proc: Option<std::process::Child> = None;
    // A right click's menu on a button: the process, the app and whether
    // it is pinned
    let mut pin_proc: Option<(std::process::Child, String, bool)> = None;
    let mut icons = Icons::default();
    // The desktop's icons (step F12): started with the session, again if
    // they end with an error (at most 5 times)
    let start_desk = || std::process::Command::new("/bin/desk").spawn().ok();
    let mut desk = start_desk();
    let mut desk_restarts = 0;
    let r = rustica_ui::run("Panel", 800, 36, Some((0, 0)), |ui: &mut Ui, ctl: &mut Control| {
        if first {
            ui.padding = 3.0;
            ctl.requests.push(Request::SetRole(ROLE_PANEL));
            let size = if cfg.edge == EDGE_LEFT || cfg.edge == EDGE_RIGHT { cfg.width } else { cfg.size };
            ctl.requests.push(Request::SetPanel { edge: cfg.edge, size, autohide: cfg.autohide });
            first = false;
        }
        // Every second when the clock shows seconds
        ctl.wake_after = Some(Duration::from_secs(if rustica_ui::clock::Formats::current().seconds { 1 } else { 2 }));
        if let Some(d) = desk.as_mut() {
            if let Ok(Some(st)) = d.try_wait() {
                desk = None;
                if !st.success() && desk_restarts < 5 {
                    desk_restarts += 1;
                    println!("panel: the desktop's icons ended ({st}); starting them again");
                    desk = start_desk();
                }
            }
        }
        // The menu or the power menu ended: their Log out and Lock are the
        // panel's to do
        for proc_ in [&mut launcher_proc, &mut power_proc] {
            let Some(c) = proc_.as_mut() else { continue };
            ctl.wake_after = Some(Duration::from_millis(300));
            if let Ok(Some(st)) = c.try_wait() {
                *proc_ = None;
                match st.code() {
                    Some(10) => {
                        println!("panel: log out (menu)");
                        ctl.quit = true;
                    }
                    Some(11) => {
                        println!("panel: lock (menu)");
                        ctl.requests.push(Request::Lock);
                    }
                    _ => {}
                }
            }
        }
        // The right click's menu ended: 1 its item (pin or unpin)
        if let Some((c, key, pinned)) = pin_proc.as_mut() {
            ctl.wake_after = Some(Duration::from_millis(300));
            if let Ok(Some(st)) = c.try_wait() {
                if st.code() == Some(1) {
                    let mut list = cfg.launchers.clone();
                    if *pinned {
                        list.retain(|l| l != key);
                        println!("panel: unpin {key}");
                    } else if !list.contains(key) {
                        list.push(key.clone());
                        println!("panel: pin {key}");
                    }
                    save_launchers(&list);
                    cfg.launchers = list;
                    launchers = cfg.launchers.iter().filter_map(|l| launcher(l)).collect();
                    // Read again at the next look, without reloading
                    cfg_mtime = config_mtime();
                }
                pin_proc = None;
            }
        }
        // The settings changed (Settings > Panel)
        if cfg_at.elapsed() >= Duration::from_secs(1) {
            cfg_at = Instant::now();
            let m = config_mtime();
            if m != cfg_mtime {
                cfg_mtime = m;
                let n = load_config();
                if n != cfg {
                    let size = if n.edge == EDGE_LEFT || n.edge == EDGE_RIGHT { n.width } else { n.size };
                    ctl.requests.push(Request::SetPanel { edge: n.edge, size, autohide: n.autohide });
                    println!("panel: settings changed");
                    launchers = n.launchers.iter().filter_map(|l| launcher(l)).collect();
                    cfg = n;
                }
            }
        }
        for e in ctl.events.drain(..) {
            if let Event::WindowInfo { id, title, app, focused, minimized } = e {
                if id < 0 {
                    wins.clear();
                } else {
                    wins.push(Win { id, title, app, focused, minimized });
                }
            } else if let Event::Desktop(LAUNCHER_KEY) = e {
                // The Super key alone
                toggle_launcher(&mut launcher_proc);
            } else if let Event::Desktop(_) = e {
                // The desktop's menu (a right click on the wallpaper):
                // Change wallpaper and Appearance settings both open
                // Settings > Appearance
                println!("panel: desktop menu, Settings > Appearance");
                let _ = std::process::Command::new("/bin/settings").arg("appearance").spawn();
            }
        }
        if net_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(3)) {
            let ifs = rustica_ui::net::interfaces();
            let s = format!("Net: {}", rustica_ui::net::summary(ifs.as_deref()));
            link = rustica_ui::net::link(ifs.as_deref());
            if s != net {
                println!("panel: {s}");
                net = s;
            }
            net_at = Some(Instant::now());
        }
        if sound_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(2)) {
            sound = rustica_ui::sound::status();
            sound_at = Some(Instant::now());
        }
        if ups_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(30)) {
            let n = updates();
            if n != ups {
                println!("panel: updates {n}");
                if n > ups {
                    ctl.requests.push(Request::Notify(format!(
                        "{n} update{} for the system: Settings > Updates",
                        if n == 1 { "" } else { "s" }
                    )));
                }
                ups = n;
            }
            let (done, waits) = app_news();
            let since = seen_updated;
            for (t, id, to) in done.iter().filter(|d| d.0 > since) {
                println!("panel: app updated {id} {to}");
                ctl.requests.push(Request::Notify(format!("{} was updated to {to}", app_name(id))));
                seen_updated = seen_updated.max(*t);
            }
            for (id, to, perms) in &waits {
                let k = format!("{id} {to}");
                if !seen_waits.contains(&k) {
                    println!("panel: app update waits {id} {to}");
                    ctl.requests.push(Request::Notify(format!("{} {to} asks for more ({perms}): FoxDen > Updates", app_name(id))));
                    seen_waits.push(k);
                }
            }
            ups_at = Some(Instant::now());
        }

        let th = ui.theme;
        let w = ui.canvas.width();
        let h = ui.canvas.height();
        let vertical = cfg.edge == EDGE_LEFT || cfg.edge == EDGE_RIGHT;
        ui.canvas.fill_rect(rustica_ui::gfx::Rect::new(0.0, 0.0, w, h), th.taskbar);
        // The line on the side that faces the desktop
        let line = match cfg.edge {
            EDGE_TOP => rustica_ui::gfx::Rect::new(0.0, h - 1.0, w, 1.0),
            EDGE_LEFT => rustica_ui::gfx::Rect::new(w - 1.0, 0.0, 1.0, h),
            EDGE_RIGHT => rustica_ui::gfx::Rect::new(0.0, 0.0, 1.0, h),
            _ => rustica_ui::gfx::Rect::new(0.0, 0.0, w, 1.0),
        };
        ui.canvas.fill_rect(line, th.border);
        let mut actions: Vec<Action> = Vec::new();
        let menu_open = launcher_proc.is_some();
        // The icons of the pinned apps and of the windows, at this size
        let isz = ui.icon_size() as u32;
        let mut shown_icons = std::collections::HashMap::new();
        for k in launchers.iter().map(|l| l.key.as_str()).chain(wins.iter().map(|w| w.app.as_str())) {
            if !shown_icons.contains_key(k) {
                if let Some(p) = icons.get(k, isz) {
                    shown_icons.insert(k.to_string(), p);
                }
            }
        }
        let right_pressed = ui.input.right_pressed;
        let state = State { wins: &wins, launchers: &launchers, icons: &shown_icons, link, sound: sound.as_ref(), ups, vertical, menu_open };
        let gap = ui.u(8.0);
        if vertical {
            // One applet under the other; the window list takes the height left
            let fixed: f32 = cfg.zones.iter().flatten().filter(|a| *a != "windows").map(|a| applet_size(ui, a, &state, true)).sum();
            let room = ui.remaining_height() - fixed;
            ui.column(0.0, |ui| {
                for (zi, zone) in cfg.zones.iter().enumerate() {
                    if zi == 2 {
                        // The end zone at the bottom
                        let rest: f32 = cfg.zones[2].iter().map(|a| if a == "windows" { 0.0 } else { applet_size(ui, a, &state, true) }).sum();
                        let left = ui.remaining_height() - rest;
                        if left > 0.0 && !cfg.zones[2].iter().any(|a| a == "windows") {
                            ui.alloc(1.0, left - gap);
                        }
                    }
                    for a in zone {
                        applet(ui, a, &state, room, &mut more_next, &mut actions);
                    }
                }
            });
        } else {
            // Side by side: the end zone right-aligned, the window list
            // taking what is left between
            let total = ui.available_width();
            let fixed: f32 = cfg.zones.iter().flatten().filter(|a| *a != "windows").map(|a| applet_size(ui, a, &state, false)).sum();
            // (two gaps kept free, so the end zone can always be pushed flush right)
            let room = total - fixed - 2.0 * gap;
            ui.row(|ui| {
                for (zi, zone) in cfg.zones.iter().enumerate() {
                    if zi > 0 {
                        // Push this zone right: the centre one to the middle
                        let this: f32 = zone.iter().map(|a| if a == "windows" { room.max(0.0) } else { applet_size(ui, a, &state, false) }).sum();
                        let after: f32 = if zi == 1 {
                            cfg.zones[2].iter().map(|a| if a == "windows" { room.max(0.0) } else { applet_size(ui, a, &state, false) }).sum()
                        } else {
                            0.0
                        };
                        let free = ui.available_width() - this - after;
                        let push = if zi == 1 { ((total - this) / 2.0 - (total - ui.available_width())).min(free) } else { free };
                        if push > gap {
                            ui.alloc(push - gap, 1.0);
                        }
                    }
                    for a in zone {
                        applet(ui, a, &state, room, &mut more_next, &mut actions);
                    }
                }
            });
        }
        // A right click anywhere else on the panel: its settings
        if right_pressed && !actions.iter().any(|a| matches!(a, Action::PinMenu { .. })) {
            println!("panel: panel settings");
            let _ = std::process::Command::new("/bin/settings").arg("panel").spawn();
        }
        for a in actions {
            match a {
                Action::PinMenu { key, pinned, x, y } => {
                    if let Some((mut c, _, _)) = pin_proc.take() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    // Beside the button, on the desktop's side of the panel
                    let at = match cfg.edge {
                        EDGE_TOP => (x as i32, cfg.size + 4),
                        EDGE_LEFT => (cfg.width + 4, y as i32),
                        EDGE_RIGHT => (-2, y as i32),
                        _ => (x as i32, -2),
                    };
                    let item = if pinned { "Unpin from taskbar" } else { "Pin to taskbar" };
                    println!("panel: menu {item} ({key})");
                    if let Ok(c) = std::process::Command::new("/bin/popmenu").args([at.0.to_string(), at.1.to_string(), item.to_string()]).spawn() {
                        pin_proc = Some((c, key, pinned));
                    }
                }
                Action::Menu => toggle_launcher(&mut launcher_proc),
                Action::Power => {
                    // Beside the panel's power button
                    let at = match cfg.edge {
                        EDGE_TOP => (-2, cfg.size + 40),
                        EDGE_LEFT => (cfg.width + 12, -2),
                        _ => (-2, -2),
                    };
                    toggle(&mut power_proc, "/bin/powermenu", &[&at.0.to_string(), &at.1.to_string()], "power menu");
                }
                Action::Start(cmd) => start(&cmd),
                Action::Activate(id, title) => {
                    println!("panel: activate {title}");
                    ctl.requests.push(Request::Activate(id));
                }
                Action::Settings(page) => {
                    println!("panel: {page} settings");
                    let _ = std::process::Command::new("/bin/settings").arg(page).spawn();
                }
                Action::Volume(d) => {
                    if let Some(st) = &sound {
                        if let Some(n) = rustica_ui::sound::set_volume(st.volume + d) {
                            println!("panel: volume {}", n.volume);
                            sound = Some(n);
                        }
                    }
                }
                Action::Lock => {
                    println!("panel: lock");
                    ctl.requests.push(Request::Lock);
                }
                Action::Logout => {
                    println!("panel: log out");
                    ctl.quit = true;
                }
            }
        }
    });
    if let Err(e) = r {
        eprintln!("panel: {e}");
        std::process::exit(1);
    }
}

struct State<'a> {
    wins: &'a [Win],
    launchers: &'a [Launcher],
    /// The apps' icons, by key, at the panel's size
    icons: &'a std::collections::HashMap<String, rustica_ui::gfx::Pixmap>,
    /// The network icon's state
    link: rustica_ui::net::Link,
    /// The volume (None: no sound service)
    sound: Option<&'a rustica_ui::sound::Status>,
    ups: usize,
    vertical: bool,
    /// The menu is open (its button shows it)
    menu_open: bool,
}

enum Action {
    Menu,
    Power,
    Start(Vec<String>),
    Activate(i32, String),
    Settings(&'static str),
    /// The volume up or down this much (the wheel over the speaker)
    Volume(i32),
    Lock,
    Logout,
    /// A right click on a button: its app, whether it is pinned, where
    PinMenu { key: String, pinned: bool, x: f32, y: f32 },
}

/// The smallest a window's button gets before "+N" takes the rest
const WIN_MIN: f32 = 90.0;
const WIN_MAX: f32 = 220.0;

/// The room an applet takes (its width side by side, its height in a
/// column), the gap after it included; the window list's is what is left
fn applet_size(ui: &mut Ui, name: &str, s: &State, vertical: bool) -> f32 {
    let gap = ui.u(8.0);
    let bw = |ui: &mut Ui, t: &str| ui.text_width(t) + ui.u(32.0) + gap;
    let bh = ui.row_h() + gap;
    if vertical {
        return match name {
            "launchers" => s.launchers.len() as f32 * bh,
            "updates" if s.ups == 0 => 0.0,
            "keyboard" => ui.line_h() + gap,
            "volume" => bh,
            "clock" => ui.line_h() + gap,
            _ => bh,
        };
    }
    match name {
        "menu" => ui.row_h() * 1.4 + gap,
        "launchers" => s
            .launchers
            .iter()
            .map(|l| if s.icons.contains_key(&l.key) { ui.icon_button_width(true, "") } else { ui.icon_button_width(false, &l.name) } + gap)
            .sum(),
        "updates" => {
            if s.ups > 0 {
                bw(ui, &format!("Updates ({})", s.ups))
            } else {
                0.0
            }
        }
        "keyboard" => ui.text_width(&layout()) + gap,
        "volume" => ui.row_h() + ui.text_width("100%") + ui.u(6.0) + gap,
        "network" => ui.row_h() + gap,
        "clock" => clock_width(ui) + gap,
        "lock" => bw(ui, "Lock"),
        "logout" => bw(ui, "Log out"),
        "power" => ui.row_h() + gap,
        _ => 0.0,
    }
}

/// Status text, centred on the buttons' line (side by side) or a line of
/// its own (in a column)
fn status(ui: &mut Ui, text: &str, vertical: bool) {
    let th = ui.theme;
    let tw = ui.text_width(text);
    let bh = if vertical { ui.line_h() } else { ui.row_h() };
    let r = ui.alloc(if vertical { 0.0 } else { tw }, bh);
    let px = ui.font_px();
    let ty = r.y + (bh - ui.line_h()) / 2.0;
    let tx = if vertical { r.x + (r.w - tw) / 2.0 } else { r.x };
    ui.canvas.text(&mut ui.face, tx, ty, text, px, th.dim);
}

fn applet(ui: &mut Ui, name: &str, s: &State, room: f32, more_next: &mut usize, actions: &mut Vec<Action>) {
    let v = s.vertical;
    let full = if v { ui.available_width() } else { 0.0 };
    match name {
        "menu" => {
            // The fox (2026-11-25; FoxDen holds the apps, this is the menu)
            let side = ui.row_h();
            let r = ui.button_ex("menu", "", if v { full } else { side * 1.4 }, false, s.menu_open);
            let isz = side * 0.78;
            ui.icon(rustica_ui::Icon::Fox, r.rect.x + (r.rect.w - isz) / 2.0, r.rect.y + (r.rect.h - isz) / 2.0, isz);
            if r.clicked {
                actions.push(Action::Menu);
            }
        }
        "launchers" => {
            // A pinned app's windows are on its button (as Windows'): a
            // line under it while one is open, the accent's when focused
            for (i, l) in s.launchers.iter().enumerate() {
                let ws: Vec<&Win> = s.wins.iter().filter(|w| !w.app.is_empty() && w.app == l.key).collect();
                let focused = ws.iter().any(|w| w.focused && !w.minimized);
                let icon = s.icons.get(&l.key);
                let label = if icon.is_some() && !v { "" } else { l.name.as_str() };
                let r = ui.icon_button(&format!("launcher{i}"), icon, label, full, focused);
                if !ws.is_empty() && !focused {
                    let th = ui.theme;
                    let line = rustica_ui::gfx::Rect::new(r.rect.x + r.rect.w * 0.3, r.rect.y + r.rect.h - ui.u(3.0), r.rect.w * 0.4, ui.u(2.0));
                    ui.canvas.fill_rect(line, th.dim);
                }
                if r.clicked {
                    if ws.is_empty() {
                        actions.push(Action::Start(l.exec.clone()));
                    } else {
                        // One window: it (raised, or minimised if it has
                        // the focus); several: the one after the focused
                        let next = match ws.iter().position(|w| w.focused) {
                            Some(k) if ws.len() > 1 => ws[(k + 1) % ws.len()],
                            _ => ws[0],
                        };
                        actions.push(Action::Activate(next.id, next.title.clone()));
                    }
                }
                if r.right_clicked {
                    actions.push(Action::PinMenu { key: l.key.clone(), pinned: true, x: r.rect.x, y: r.rect.y });
                }
            }
        }
        "windows" => window_list(ui, s, room, more_next, actions),
        "updates" => {
            if s.ups > 0 && ui.button_ex("updates", &format!("Updates ({})", s.ups), full, true, false).clicked {
                actions.push(Action::Settings("updates"));
            }
        }
        "keyboard" => status(ui, &layout(), v),
        "volume" => {
            // A speaker and the volume; a click opens Settings > Sound,
            // the wheel over it changes the volume
            let side = ui.row_h();
            let text = match s.sound {
                Some(st) if st.devices.is_empty() => "\u{2014}".to_string(),
                Some(st) => format!("{}%", st.volume),
                None => "\u{2014}".to_string(),
            };
            let tw = if v { 0.0 } else { ui.text_width("100%") + ui.u(6.0) };
            let r = ui.button_ex("volume", "", if v { full } else { side + tw }, false, false);
            let isz = side * 0.62;
            let (waves, muted) = match s.sound {
                Some(st) => (rustica_ui::sound::waves(st.volume), st.muted || st.devices.is_empty()),
                None => (0, true),
            };
            let ix = if v { r.rect.x + (r.rect.w - isz) / 2.0 } else { r.rect.x + (side - isz) / 2.0 + ui.u(2.0) };
            ui.icon(rustica_ui::Icon::Speaker(waves, muted), ix, r.rect.y + (r.rect.h - isz) / 2.0, isz);
            if !v {
                let px = ui.font_px();
                let th = ui.theme;
                let ty = r.rect.y + (r.rect.h - ui.line_h()) / 2.0;
                ui.canvas.text(&mut ui.face, r.rect.x + side, ty, &text, px, th.text);
            }
            if r.clicked {
                actions.push(Action::Settings("sound"));
            }
            if ui.hover(r.rect) && ui.input.wheel != 0.0 {
                actions.push(Action::Volume(if ui.input.wheel > 0.0 { 5 } else { -5 }));
                ui.input.wheel = 0.0;
            }
        }
        "network" => {
            // An icon: the cable's boxes or the Wi-Fi bars (2026-11-26);
            // the details are in Settings > Network, a click away
            let side = ui.row_h();
            let r = ui.button_ex("network", "", if v { full } else { side }, false, false);
            let isz = side * 0.62;
            let icon = match s.link {
                rustica_ui::net::Link::Wireless(n) => rustica_ui::Icon::Wireless(n),
                rustica_ui::net::Link::Wired(on) => rustica_ui::Icon::Wired(on),
                rustica_ui::net::Link::Off => rustica_ui::Icon::Wired(false),
            };
            ui.icon(icon, r.rect.x + (r.rect.w - isz) / 2.0, r.rect.y + (r.rect.h - isz) / 2.0, isz);
            if r.clicked {
                actions.push(Action::Settings("network"));
            }
        }
        "clock" => clock_applet(ui, v),
        "lock" => {
            if ui.button_ex("lock", "Lock", full, false, false).clicked {
                actions.push(Action::Lock);
            }
        }
        "logout" => {
            if ui.button_ex("logout", "Log out", full, false, false).clicked {
                actions.push(Action::Logout);
            }
        }
        "power" => {
            let side = ui.row_h();
            let r = ui.button_ex("power", "", if v { full } else { side }, false, false);
            let isz = side * 0.62;
            ui.icon(rustica_ui::Icon::Power, r.rect.x + (r.rect.w - isz) / 2.0, r.rect.y + (r.rect.h - isz) / 2.0, isz);
            if r.clicked {
                actions.push(Action::Power);
            }
        }
        _ => {}
    }
}

/// The open windows: each button gets an equal share of `room` (at most
/// WIN_MAX, its title shortened to fit); when even WIN_MIN each does not
/// fit, the first ones that do are shown and "+N" activates the others in
/// turn
fn window_list(ui: &mut Ui, s: &State, room: f32, more_next: &mut usize, actions: &mut Vec<Action>) {
    // Windows of pinned apps are on their launchers' buttons
    let wins: Vec<&Win> = s.wins.iter().filter(|w| w.app.is_empty() || !s.launchers.iter().any(|l| l.key == w.app)).collect();
    let n = wins.len();
    if n == 0 {
        return;
    }
    let gap = ui.u(8.0);
    let v = s.vertical;
    let label = |w: &Win| if w.minimized { format!("({})", w.title) } else { w.title.clone() };
    // Each button as wide as its icon and title need (2026-11-27), up
    // to WIN_MAX; when they do not all fit, the same width each, down to
    // WIN_MIN, and "+N" for the rest
    let mut widths: Vec<f32> = Vec::new();
    let shown = if v {
        let bh = ui.row_h() + gap;
        let fits = ((room / bh).floor().max(0.0)) as usize;
        let shown = if fits >= n { n } else { fits.saturating_sub(1) };
        widths = vec![ui.available_width(); shown];
        shown
    } else {
        let natural: Vec<f32> = wins.iter().map(|w| ui.icon_button_width(s.icons.contains_key(&w.app), &label(w)).min(ui.u(WIN_MAX))).collect();
        let total: f32 = natural.iter().map(|w| w + gap).sum();
        if total <= room {
            widths = natural;
            n
        } else {
            let fits = ((room / (ui.u(WIN_MIN) + gap)).floor().max(0.0)) as usize;
            let shown = if fits >= n { n } else { fits.saturating_sub(1).max(1) };
            let more_w = if shown < n { ui.text_width(&format!("+{}", n - shown)) + ui.u(32.0) + gap } else { 0.0 };
            let each = ((room - more_w) / shown as f32 - gap).clamp(ui.u(40.0), ui.u(WIN_MAX));
            for k in 0..shown {
                widths.push(each.min(natural[k]));
            }
            shown
        }
    };
    for (k, win) in wins[..shown].iter().enumerate() {
        let r = ui.icon_button(&format!("win{}", win.id), s.icons.get(&win.app), &label(win), widths[k], win.focused);
        if r.clicked {
            actions.push(Action::Activate(win.id, win.title.clone()));
        }
        if r.right_clicked && launcher(&win.app).is_some() {
            actions.push(Action::PinMenu { key: win.app.clone(), pinned: false, x: r.rect.x, y: r.rect.y });
        }
    }
    if shown < n {
        let hidden = &wins[shown..];
        let label = format!("+{}", hidden.len());
        let each = if v { ui.available_width() } else { 0.0 };
        if ui.button_ex("more", &label, each, false, false).clicked {
            let w = hidden[*more_next % hidden.len()];
            *more_next += 1;
            actions.push(Action::Activate(w.id, w.title.clone()));
        }
    }
}

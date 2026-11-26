// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! settings: Rustica's Settings (DESKTOP.md step F7). Display (the scale,
//! ~/.config/rustica.conf), appearance (the theme and the wallpaper, the
//! same file; the compositor and every window follow at once), keyboard layout (localectl), mouse speed
//! (inputctl), network (the interfaces, DHCP or a fixed address, DNS
//! servers: netcfg; network step D6), date, time and time zone
//! (timedatectl), users (useradd, userdel, chpasswd, usermod), disks
//! (lsblk, read only), updates (the installer service /svc/packages,
//! through the updates command: what the last check found, Check now,
//! Install updates, a restart for a new system image; package step E6),
//! about.
//!
//!   settings [SECTION]     display, appearance, keyboard, mouse, network, time, users, disks, updates, about
//!
//! What is the system's (in /etc) is root's: another user is asked for
//! their password and the change goes through sudo -S. Prints what it
//! changes ("settings: ...") for the tests.

use rustica_apps::sys;
use rustica_ui::net::Iface;
use rustica_ui::{Column, Control, Icon, Row, Ui};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn say(s: &str) {
    println!("settings: {s}");
    let _ = std::io::stdout().flush();
}

const SECTIONS: [&str; 14] =
    ["Display", "Appearance", "Panel", "Desktop", "Keyboard", "Mouse", "Power", "Network", "Date & Time", "Users", "Disks", "Updates", "Apps", "About"];
const S_DISPLAY: usize = 0;
const S_APPEARANCE: usize = 1;
const S_PANEL: usize = 2;
const S_DESKTOP: usize = 3;
const S_KEYBOARD: usize = 4;
const S_MOUSE: usize = 5;
const S_POWER: usize = 6;
const S_NETWORK: usize = 7;
const S_TIME: usize = 8;
const S_USERS: usize = 9;
const S_DISKS: usize = 10;
const S_UPDATES: usize = 11;
const S_APPS: usize = 12;
const S_ABOUT: usize = 13;

/// The panel's applets (DESKTOP.md step F10), with what Settings calls them
const APPLETS: [(&str, &str); 11] = [
    ("power", "Power menu"),
    ("menu", "Menu (the fox)"),
    ("launchers", "Launchers"),
    ("windows", "Window list"),
    ("updates", "Updates"),
    ("keyboard", "Keyboard layout"),
    ("volume", "Volume"),
    ("network", "Network"),
    ("clock", "Clock"),
    ("lock", "Lock"),
    ("logout", "Log out"),
];

/// ~/.config/rustica/panel.conf as the panel reads it (the same defaults)
#[derive(Clone)]
struct PanelConf {
    position: String,
    size: i32,
    width: i32,
    autohide: bool,
    zones: [Vec<String>; 3],
    launchers: Vec<String>,
}

impl PanelConf {
    fn path() -> std::path::PathBuf {
        sys::config_dir().join("rustica").join("panel.conf")
    }

    fn load() -> Self {
        let mut c = PanelConf {
            position: "bottom".into(),
            size: 36,
            width: 180,
            autohide: false,
            zones: [
                vec!["menu".into(), "launchers".into(), "windows".into()],
                vec![],
                vec!["updates".into(), "keyboard".into(), "volume".into(), "network".into(), "clock".into(), "lock".into(), "logout".into(), "power".into()],
            ],
            launchers: vec!["foxden".into(), "files".into(), "terminal".into()],
        };
        let text = std::fs::read_to_string(Self::path()).or_else(|_| std::fs::read_to_string("/etc/rustica/panel.conf")).unwrap_or_default();
        let words = |v: &str| v.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "position" => c.position = v.into(),
                "size" => c.size = v.parse().unwrap_or(36),
                "width" => c.width = v.parse().unwrap_or(180),
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

    fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let text = format!(
            "# The panel (Settings > Panel; DESKTOP.md step F10)\nposition={}\nsize={}\nwidth={}\nautohide={}\nstart={}\ncentre={}\nend={}\nlaunchers={}\n",
            self.position,
            self.size,
            self.width,
            if self.autohide { "yes" } else { "no" },
            self.zones[0].join(" "),
            self.zones[1].join(" "),
            self.zones[2].join(" "),
            self.launchers.join(" ")
        );
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &p)
    }
}

/// The apps that can be pinned: /usr/share/applications/*.app (by name)
/// and installed FoxDen apps (by ID), with their names
fn pinnable() -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/usr/share/applications") {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("app") {
                continue;
            }
            let id = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
            let name = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| t.lines().find_map(|l| l.strip_prefix("name=").map(|v| v.trim().to_string())))
                .unwrap_or_else(|| id.clone());
            v.push((id, name));
        }
    }
    if let Ok(rd) = std::fs::read_dir("/apps") {
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if let Ok(t) = std::fs::read_to_string(e.path().join("manifest")) {
                let name = t.lines().find_map(|l| l.strip_prefix("Name:").map(|v| v.trim().to_string())).unwrap_or_else(|| id.clone());
                v.push((id, name));
            }
        }
    }
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

/// One key of ~/.config/rustica.conf, else of /etc/rustica/appearance
fn appearance_key(key: &str) -> String {
    let find = |p: &std::path::Path| -> Option<String> {
        let text = std::fs::read_to_string(p).ok()?;
        text.lines().find_map(|l| l.trim().strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string()))
    };
    find(&sys::config_dir().join("rustica.conf"))
        .or_else(|| find(std::path::Path::new("/etc/rustica/appearance")))
        .unwrap_or_else(|| "rustica".into())
}

/// The pictures in ~/Pictures (and ~), for the wallpaper
fn pictures() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in [sys::home().join("Pictures"), sys::home()] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                if ["png", "jpg", "jpeg", "qoi"].contains(&ext.as_str()) && p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// ~/x is the home folder's x
fn expand_home(p: &str) -> std::path::PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => sys::home().join(rest),
        None if p == "~" => sys::home(),
        None => p.into(),
    }
}

/// A picture made into the wallpaper: at most 3840 pixels on its long side,
/// as QOI (the compositor's format) in ~/.config/rustica/, a new name each
/// time so that the compositor sees the change; the older ones go
fn make_wallpaper(src: &std::path::Path) -> Result<String, String> {
    let pm = rustica_ui::gfx::load_image(&src.to_string_lossy())?;
    let (w, h) = (pm.width(), pm.height());
    let k = (3840.0 / w.max(h) as f32).min(1.0);
    let pm = if k < 1.0 {
        let (nw, nh) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let mut out = rustica_ui::gfx::Pixmap::new(nw, nh).ok_or("bad size")?;
        let paint = rustica_ui::gfx::PixmapPaint { quality: rustica_ui::gfx::FilterQuality::Bilinear, ..Default::default() };
        out.draw_pixmap(0, 0, pm.as_ref(), &paint, rustica_ui::gfx::Transform::from_scale(k, k), None);
        out
    } else {
        pm
    };
    let dir = sys::config_dir().join("rustica");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = dir.join(format!("wallpaper-{stamp}.qoi"));
    let tmp = dir.join(".wallpaper.tmp");
    std::fs::write(&tmp, rustica_ui::gfx::encode_qoi(&pm)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &name).map_err(|e| format!("{}: {e}", name.display()))?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if n.starts_with("wallpaper-") && n.ends_with(".qoi") && p != name {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(name.to_string_lossy().into_owned())
}

/// An installed app, for Settings > Apps (FoxDen step FD6)
#[derive(Clone)]
struct SetApp {
    id: String,
    name: String,
    version: String,
    perms: Vec<String>,
    folder: u64, // bytes: /apps/ID
    data: u64,   // bytes: the user's three folders for it
    /// Where it came from (rpg's Origin: foxden, system, file, "source
    /// URL"; "" before rpg kept it), FoxDen step FD7c
    origin: String,
}

/// Each installed app's Origin, from rpg's database (anyone may read it)
fn app_origins() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let text = std::fs::read_to_string("/var/lib/rpg/status").unwrap_or_default();
    for st in text.split("\n\n") {
        let get = |k: &str| st.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
        if let (Some(p), Some(o)) = (get("Package:"), get("Origin:")) {
            out.insert(p, o);
        }
    }
    out
}

/// /etc/rpg/install.conf: may anyone install apps from package files?
fn outside_anyone() -> bool {
    std::fs::read_to_string("/etc/rpg/install.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next())
        .filter_map(|l| l.trim().strip_prefix("outside"))
        .next_back()
        .is_some_and(|v| v.trim() == "anyone")
}

fn tree_size(p: &std::path::Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(p) else { return 0 };
    if !md.is_dir() {
        return md.len();
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum()).unwrap_or(0)
}

fn load_apps() -> Vec<SetApp> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    // Versions from rpg (ID VERSION NAME...)
    let list = Command::new("/bin/rpg").args(["list", "--apps"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let mut v = Vec::new();
    let origins = app_origins();
    for l in list.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() < 2 || !w[0].contains('.') {
            continue;
        }
        let id = w[0].to_string();
        let m = std::fs::read_to_string(format!("/apps/{id}/manifest")).unwrap_or_default();
        let field = |k: &str| m.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
        let data: u64 = [".local/share/apps", ".config/apps", ".cache/apps"].iter().map(|d| tree_size(&std::path::Path::new(&home).join(d).join(&id))).sum();
        v.push(SetApp {
            name: Some(field("Name:")).filter(|n| !n.is_empty()).unwrap_or(id.clone()),
            version: w[1].to_string(),
            perms: field("Permissions:").split_whitespace().map(str::to_string).collect(),
            folder: tree_size(&std::path::Path::new("/apps").join(&id)),
            data,
            origin: origins.get(&id).cloned().unwrap_or_default(),
            id,
        });
    }
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

fn bytes_text(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / 1048576.0)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}

fn perm_words(p: &str) -> &'static str {
    match p {
        "network" => "Use the network",
        "files:documents" => "Your Documents folder",
        "files:pictures" => "Your Pictures folder",
        "files:music" => "Your Music folder",
        "files:videos" => "Your Videos folder",
        "files:downloads" => "Your Downloads folder",
        "files:home" => "Your whole home folder",
        "removable" => "Removable drives",
        "audio" => "Sound",
        "camera" => "The camera",
        "location" => "Your location",
        "system" => "Outside the sandbox (never given)",
        _ => "Unknown",
    }
}

/// The permissions the user lets an app have (~/.config/apps/permissions,
/// appctl's), or None: all it asks for
fn app_granted(id: &str) -> Option<Vec<String>> {
    let home = std::env::var("HOME").ok()?;
    let t = std::fs::read_to_string(format!("{home}/.config/apps/permissions")).ok()?;
    t.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == id).then(|| v.split_whitespace().filter(|w| *w != "-").map(str::to_string).collect())
    })
}

fn app_set_granted(id: &str, perms: &[String]) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let dir = format!("{home}/.config/apps");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir}: {e}"))?;
    let path = format!("{dir}/permissions");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| l.split_once(':').map(|(k, _)| k.trim() != id).unwrap_or(true)).map(str::to_string).collect();
    lines.push(format!("{id}: {}", if perms.is_empty() { "-".to_string() } else { perms.join(" ") }));
    std::fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{path}: {e}"))
}

/// The installer service's last check (/var/lib/rpg/updates)
#[derive(Default, Clone)]
struct UpStatus {
    checked: i64,
    error: String,
    steps: Vec<(String, String, String, u64, String)>, // name, from ("" new), to, size, why
    kept: Vec<String>,
}

fn up_status() -> UpStatus {
    let mut u = UpStatus::default();
    for l in std::fs::read_to_string("/var/lib/rpg/updates").unwrap_or_default().lines() {
        if let Some(v) = l.strip_prefix("checked ") {
            u.checked = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("error ") {
            u.error = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("step ") {
            let f: Vec<&str> = v.split(' ').collect();
            if f.len() >= 5 {
                let dash = |t: &str| if t == "-" { String::new() } else { t.to_string() };
                u.steps.push((f[0].into(), dash(f[1]), f[2].into(), f[3].parse().unwrap_or(0), dash(f[4])));
            }
        } else if let Some(v) = l.strip_prefix("kept ") {
            u.kept.push(v.to_string());
        }
    }
    u
}

/// A check or an install running in the background
#[derive(Default)]
struct Job {
    lines: Vec<String>,
    done: Option<i32>,
}

fn start_job(argv: Vec<String>, input: String) -> Arc<Mutex<Job>> {
    let job = Arc::new(Mutex::new(Job::default()));
    let j = job.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let child = Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let mut g = j.lock().unwrap();
                g.lines.push(format!("{}: {e}", argv[0]));
                g.done = Some(-1);
                return;
            }
        };
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(input.as_bytes());
        }
        let err = child.stderr.take();
        let j2 = j.clone();
        let et = std::thread::spawn(move || {
            if let Some(e) = err {
                for l in std::io::BufReader::new(e).lines().map_while(Result::ok) {
                    j2.lock().unwrap().lines.push(l);
                }
            }
        });
        if let Some(o) = child.stdout.take() {
            for l in std::io::BufReader::new(o).lines().map_while(Result::ok) {
                j.lock().unwrap().lines.push(l);
            }
        }
        let _ = et.join();
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        j.lock().unwrap().done = Some(code);
    });
    job
}
const METHODS: [&str; 3] = ["Automatic (DHCP)", "Fixed address", "Off"];

/// A change that needs root, waiting for the password
struct Pending {
    what: String,
    cmds: Vec<(Vec<String>, String)>, // argv, stdin
    /// Installing updates: through the installer service, not sudo
    updates: bool,
}

/// The system's wallpapers (/usr/share/backgrounds), any with any theme
const WALLPAPERS: [(&str, &str); 6] =
    [("rustica", "Rustica fox"), ("dracula", "Dracula"), ("forest", "Forest"), ("arctic", "Arctic"), ("sakura", "Sakura"), ("gradient", "Gradient")];

/// What each token of a theme is, for people
const TOKEN_NAMES: [&str; 16] = [
    "Background", "Surface", "Raised surface", "Surface under the pointer", "Border", "Text", "Muted text", "Primary",
    "Primary under the pointer", "Accent", "Selection", "Success", "Warning", "Danger", "Information", "Panel",
];

/// A theme being made or changed (Settings > Appearance)
struct ThemeEdit {
    theme: rustica_ui::Theme,
    /// Its id when an existing theme of the user's is changed
    editing: Option<String>,
    name: String,
    hex: Vec<String>,
    error: String,
}

impl ThemeEdit {
    fn new(theme: rustica_ui::Theme, editing: Option<String>) -> ThemeEdit {
        let hex = rustica_ui::TOKENS.iter().map(|k| format!("#{:06X}", theme.get(k).unwrap_or(0))).collect();
        ThemeEdit { name: theme.label.to_string(), theme, editing, hex, error: String::new() }
    }
}

/// A theme's id from its name: lower case, a-z 0-9 and -
fn theme_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').chars().take(15).collect::<String>().trim_end_matches('-').to_string()
}

/// A small picture of a theme in r: its background and panel, a window
/// with a title bar, text, a primary button, the accent and a selection;
/// its name under it
fn theme_preview(ui: &mut Ui, r: rustica_ui::gfx::Rect, t: &rustica_ui::Theme, label: &str) {
    use rustica_ui::gfx::Rect;
    let sc = ui.u(1.0);
    let u = move |v: f32| v * sc;
    let (p, lh) = (u(8.0), ui.line_h());
    let a = Rect::new(r.x + p, r.y + p, r.w - 2.0 * p, r.h - 2.0 * p - lh - u(4.0));
    let (k6, k4, k3, k2) = (u(6.0), u(4.0), u(3.0), u(2.0));
    ui.canvas.fill_rounded(a, k4, t.background);
    // The panel, with the fox
    let ph = (a.h * 0.16).max(u(8.0));
    ui.canvas.fill_rect(Rect::new(a.x, a.y + a.h - ph, a.w, ph), t.taskbar);
    if let Some(f) = rustica_ui::ui::fox_path(a.x + k3, a.y + a.h - ph + ph * 0.15, ph * 0.7) {
        ui.canvas.fill_path_evenodd(&f, rustica_ui::ui::FOX_COLOR);
    }
    // A window: title bar, text lines, a selected row, a primary button, the accent
    let w = Rect::new(a.x + a.w * 0.12, a.y + a.h * 0.12, a.w * 0.76, a.h * 0.68 - ph * 0.5);
    ui.canvas.fill_rounded(w, k3, t.surface);
    ui.canvas.fill_rect(Rect::new(w.x, w.y, w.w, k6.max(w.h * 0.16)), t.surface_hi);
    ui.canvas.fill_rect(Rect::new(w.x + w.w - k6 - k2, w.y + k2 * 0.5, k6 * 0.8, k6 * 0.8), t.danger);
    let ty = w.y + k6.max(w.h * 0.16) + k4;
    ui.canvas.fill_rect(Rect::new(w.x + k4, ty, w.w * 0.55, k2), t.text);
    ui.canvas.fill_rect(Rect::new(w.x + k4, ty + k4, w.w * 0.4, k2), t.dim);
    ui.canvas.fill_rect(Rect::new(w.x + k2, ty + 2.0 * k4, w.w - 2.0 * k2, k6), t.selection);
    let by = w.y + w.h - k6 - k4;
    ui.canvas.fill_rounded(Rect::new(w.x + w.w - w.w * 0.34 - k4, by, w.w * 0.34, k6 + k2), k2, t.accent);
    ui.canvas.fill_rounded(Rect::new(w.x + k4, by + k2 * 0.5, k6, k6), k3, t.secondary);
    ui.canvas.fill_rounded(Rect::new(w.x + k4 + k6 + k2, by + k2 * 0.5, k6, k6), k3, t.good);
    ui.canvas.stroke_rounded(w, k3, 1.0, t.border);
    let px = ui.font_px();
    let shown = ui.elide(label, r.w - 2.0 * p, false);
    let c = ui.theme.text;
    ui.canvas.text(&mut ui.face, r.x + p, r.y + r.h - p - lh, &shown, px, c);
}

struct Settings {
    section: Option<usize>,
    theme: String,
    wallpaper: String,
    /// Settings > Appearance: the themes there are (id, name, the user's
    /// own), and the one being made or changed
    themes: Vec<(String, String, bool)>,
    theme_edit: Option<ThemeEdit>,
    pictures: Vec<std::path::PathBuf>,
    picture: Option<usize>,
    /// The picture file field, and the file chooser while it is open
    /// (with the file it writes the choice to)
    picture_path: String,
    /// Settings > Panel: the configuration, the applet chosen (zone,
    /// place), the launcher chosen, the apps that can be pinned
    panel: PanelConf,
    panel_sel: Option<(usize, usize)>,
    launcher_sel: Option<usize>,
    pinnable: Vec<(String, String)>,
    pin_sel: Option<usize>,
    chooser: Option<(std::process::Child, std::path::PathBuf)>,
    apps: Vec<SetApp>,
    scale: u32,
    keymaps: Vec<String>,
    keymap: Option<usize>,
    keymap_now: String,
    speed: u32,
    zones: Vec<String>,
    zone: Option<usize>,
    zone_now: String,
    zone_filter: String,
    date_text: String,
    users: Vec<(String, u32, String, bool)>, // name, uid, home, admin
    user_sel: Option<usize>,
    new_name: String,
    new_pw: String,
    new_admin: bool,
    disks: Vec<Row>,
    disk_sel: Option<usize>,
    about: Vec<(String, String)>,
    // Network: the interfaces as netd has them (None: netd not running),
    // when they were asked, the one selected; the editor's method (an
    // index into METHODS), address and gateway, for the interface named
    // in net_for; the DNS servers of /etc/network.conf
    ifaces: Option<Vec<Iface>>,
    net_at: Option<Instant>,
    iface_sel: Option<usize>,
    net_for: String,
    method: usize,
    addr_text: String,
    gw_text: String,
    dns_text: String,
    password: Option<String>, // the user's, once sudo took it
    pending: Option<Pending>,
    pw_text: String,
    message: Option<(String, String)>,
    confirm_delete: Option<String>,
    status: String,
    // Updates: the last check, what runs now (and what: "check" or
    // "install"), its output, whether a new system image went in
    ups: UpStatus,
    job: Option<(String, Arc<Mutex<Job>>)>,
    job_lines: Vec<String>,
    new_image: Option<String>,
    up_sel: Option<usize>,
}

/// /etc/network.conf's lines, as words (comments left out)
fn conf_lines() -> Vec<Vec<String>> {
    std::fs::read_to_string("/etc/network.conf")
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// What /etc/network.conf says for an interface: the method (an index
/// into METHODS), address and gateway; DHCP when it has no line
fn conf_method(name: &str) -> (usize, String, String) {
    for w in conf_lines() {
        if w.len() >= 2 && w[0] == name {
            match w[1].as_str() {
                "dhcp" => return (0, String::new(), String::new()),
                "static" => return (1, w.get(2).cloned().unwrap_or_default(), w.get(3).cloned().unwrap_or_default()),
                "off" | "manual" => return (2, String::new(), String::new()),
                _ => {}
            }
        }
    }
    (0, String::new(), String::new())
}

fn read_first_line(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default().lines().next().unwrap_or("").trim().to_string()
}

impl Settings {
    fn new() -> Settings {
        let mut s = Settings {
            section: Some(0),
            theme: appearance_key("theme"),
            wallpaper: appearance_key("wallpaper"),
            themes: rustica_ui::list_themes(),
            theme_edit: None,
            pictures: Vec::new(),
            picture: None,
            picture_path: String::new(),
            panel: PanelConf::load(),
            panel_sel: None,
            launcher_sel: None,
            pinnable: Vec::new(),
            pin_sel: None,
            chooser: None,
            apps: Vec::new(),
            scale: rustica_ui::app::scale_from_env() as u32,
            keymaps: Vec::new(),
            keymap: None,
            keymap_now: String::new(),
            speed: 100,
            zones: Vec::new(),
            zone: None,
            zone_now: String::new(),
            zone_filter: String::new(),
            date_text: String::new(),
            users: Vec::new(),
            user_sel: None,
            new_name: String::new(),
            new_pw: String::new(),
            new_admin: false,
            disks: Vec::new(),
            disk_sel: None,
            about: Vec::new(),
            ifaces: None,
            net_at: None,
            iface_sel: Some(0),
            net_for: String::new(),
            method: 0,
            addr_text: String::new(),
            gw_text: String::new(),
            dns_text: String::new(),
            password: None,
            pending: None,
            pw_text: String::new(),
            message: None,
            confirm_delete: None,
            status: String::new(),
            ups: UpStatus::default(),
            job: None,
            job_lines: Vec::new(),
            new_image: None,
            up_sel: None,
        };
        s.reload();
        s
    }

    fn reload(&mut self) {
        self.keymaps = sys::run("/bin/localectl", &["list-keymaps"]).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        self.keymaps.sort();
        self.keymaps.dedup();
        self.keymap_now = read_first_line("/etc/keymap");
        if self.keymap_now.is_empty() {
            self.keymap_now = "us".into();
        }
        self.keymap = self.keymaps.iter().position(|k| *k == self.keymap_now);
        self.speed = sys::run("/bin/inputctl", &[]).ok().and_then(|o| o.split(':').nth(1).and_then(|v| v.trim().trim_end_matches('%').parse().ok())).unwrap_or(100);
        self.zones = sys::run("/bin/timedatectl", &["list-timezones"]).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        self.zone_now = read_first_line("/etc/timezone");
        if self.zone_now.is_empty() {
            self.zone_now = "UTC".into();
        }
        self.zone = self.filtered_zones().iter().position(|z| *z == self.zone_now);
        let tm = sys::local(sys::now());
        self.date_text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.year + 1900, tm.mon + 1, tm.mday, tm.hour, tm.min, tm.sec);
        self.load_users();
        self.load_disks();
        self.load_about();
        self.ups = up_status();
        self.net_at = None;
        self.net_for.clear();
        self.dns_text = conf_lines().iter().filter(|w| w[0] == "dns").flat_map(|w| w[1..].to_vec()).collect::<Vec<_>>().join(" ");
    }

    fn filtered_zones(&self) -> Vec<String> {
        let f = self.zone_filter.to_lowercase();
        self.zones.iter().filter(|z| f.is_empty() || z.to_lowercase().contains(&f)).cloned().collect()
    }

    fn load_users(&mut self) {
        let admins: Vec<String> = std::fs::read_to_string("/etc/group")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("sudo:"))
            .map(|l| l.rsplit(':').next().unwrap_or("").split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();
        self.users = std::fs::read_to_string("/etc/passwd")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                let uid: u32 = f.get(2)?.parse().ok()?;
                (uid == 0 || (1000..60000).contains(&uid)).then(|| (f[0].to_string(), uid, f.get(5).unwrap_or(&"").to_string(), uid == 0 || admins.iter().any(|a| a == f[0])))
            })
            .collect();
    }

    fn load_disks(&mut self) {
        let out = sys::run("/bin/lsblk", &["-l", "-n", "-o", "NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS"]).unwrap_or_default();
        self.disks = out
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let disk = f.get(2) == Some(&"disk");
                // FSTYPE may be empty: a 4th field that starts with / is a mount point
                let (fs, mp) = match (f.get(3), f.get(4)) {
                    (Some(a), _) if a.starts_with('/') => (String::new(), a.to_string()),
                    (Some(a), Some(b)) => (a.to_string(), b.to_string()),
                    (Some(a), None) => (a.to_string(), String::new()),
                    _ => (String::new(), String::new()),
                };
                Row {
                    icon: if disk { Icon::Drive } else { Icon::None },
                    cells: vec![
                        if disk { f.first().unwrap_or(&"").to_string() } else { format!("   {}", f.first().unwrap_or(&"")) },
                        f.get(1).unwrap_or(&"").to_string(),
                        if disk { "disk".into() } else { "partition".into() },
                        fs,
                        mp,
                    ],
                }
            })
            .collect();
    }

    fn load_about(&mut self) {
        let os = sys::parse_kv(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
        let unq = |s: &str| s.trim_matches('"').to_string();
        let mut a = Vec::new();
        a.push(("Operating system".into(), unq(os.get("PRETTY_NAME").map(|s| s.as_str()).unwrap_or("Rustica"))));
        a.push(("Codename".into(), unq(os.get("VERSION_CODENAME").map(|s| s.as_str()).unwrap_or("redfox"))));
        let kernel = sys::run("/bin/uname", &["-srv"]).or_else(|_| sys::run("/bin/coreutils", &["uname", "-srv"])).unwrap_or_default();
        a.push(("Kernel".into(), kernel.trim().to_string()));
        a.push(("Computer name".into(), read_first_line("/etc/hostname")));
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        a.push(("Processors".into(), format!("{cpus} × x86-64")));
        // {"total":N,"used":N,...}
        let json_num = |text: &str, k: &str| -> Option<u64> {
            let i = text.find(&format!("\"{k}\":"))? + k.len() + 3;
            text[i..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        };
        if let Ok(free) = sys::run("/bin/free", &["--json"]) {
            if let (Some(t), Some(u)) = (json_num(&free, "total"), json_num(&free, "used")) {
                a.push(("Memory".into(), format!("{} ({} in use)", sys::human_size(t), sys::human_size(u))));
            }
        }
        let gpu = sys::run("/bin/lspci", &[]).unwrap_or_default().lines().find(|l| l.to_lowercase().contains("vga") || l.to_lowercase().contains("display")).map(|l| l.trim().to_string());
        if let Some(g) = gpu {
            a.push(("Graphics".into(), g));
        }
        if let Some(ns) = sys::run("/bin/uptime", &["--json"]).ok().and_then(|u| json_num(&u, "uptime_ns")) {
            let m = ns / 60_000_000_000;
            a.push(("Running for".into(), if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }));
        }
        self.about = a;
    }

    /// Run commands as root: now if we are, else after the password
    fn admin(&mut self, what: &str, cmds: Vec<(Vec<String>, String)>) {
        let p = Pending { what: what.to_string(), cmds, updates: false };
        if sys::euid() == 0 || self.password.is_some() {
            self.run_admin(p);
        } else {
            self.pw_text.clear();
            self.pending = Some(p);
        }
    }

    fn run_admin(&mut self, p: Pending) {
        let root = sys::euid() == 0;
        for (argv, input) in &p.cmds {
            let mut c = if root {
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            } else {
                let mut c = Command::new("/bin/sudo");
                c.arg("-S").args(argv);
                c
            };
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let r = c.spawn().and_then(|mut ch| {
                if let Some(mut si) = ch.stdin.take() {
                    if !root {
                        let _ = writeln!(si, "{}", self.password.clone().unwrap_or_default());
                    }
                    let _ = si.write_all(input.as_bytes());
                }
                ch.wait_with_output()
            });
            match r {
                Ok(o) if o.status.success() => {}
                Ok(o) => {
                    let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    if !root && (e.contains("password") || e.contains("not allowed") || e.contains("sudoers")) {
                        self.password = None;
                    }
                    self.message = Some((format!("Could not {}", p.what), if e.is_empty() { format!("{} failed", argv[0]) } else { e }));
                    self.reload();
                    return;
                }
                Err(e) => {
                    self.message = Some((format!("Could not {}", p.what), e.to_string()));
                    return;
                }
            }
        }
        say(&p.what);
        self.status = format!("Done: {}", p.what);
        self.reload();
    }

    // ---- sections ----

    fn display(&mut self, ui: &mut Ui) {
        ui.heading("Display");
        ui.dim_label("The size of everything in windows; it applies to windows opened from now on.");
        ui.space(6.0);
        let mut chosen = None;
        ui.row(|ui| {
            for s in 1..=2u32 {
                let label = if s == 1 { "Normal (100%)" } else { "Large (200%)" };
                let pressed = if self.scale == s { ui.primary_button(label) } else { ui.button(label) };
                if pressed {
                    chosen = Some(s);
                }
            }
        });
        if let Some(s) = chosen {
            if s != self.scale {
                let p = sys::config_dir().join("rustica.conf");
                match sys::write_kv(&p, "scale", &s.to_string()) {
                    Ok(()) => {
                        self.scale = s;
                        say(&format!("scale {s}"));
                        self.status = format!("Scale {}% for new windows", s * 100);
                    }
                    Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", p.display()))),
                }
            }
        }
        ui.space(8.0);
        ui.dim_label(&format!("Your settings: {}", sys::config_dir().join("rustica.conf").display()));
    }

    fn set_appearance(&mut self, key: &str, value: &str) {
        let p = sys::config_dir().join("rustica.conf");
        let _ = std::fs::create_dir_all(sys::config_dir());
        match sys::write_kv(&p, key, value) {
            Ok(()) => say(&format!("{key} {value}")),
            Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", p.display()))),
        }
    }

    fn appearance(&mut self, ui: &mut Ui) {
        ui.heading("Appearance");
        if self.theme_edit.is_some() {
            return self.theme_editor(ui);
        }
        ui.label("Themes");
        let mut chosen = None;
        let per_row = ((ui.available_width() + ui.u(8.0)) / ui.u(188.0)).floor().max(1.0) as usize;
        for row in self.themes.chunks(per_row) {
            ui.row(|ui| {
                for (id, label, mine) in row {
                    let t = rustica_ui::theme_named(id);
                    let r = ui.card(&format!("theme-{id}"), ui.u(180.0), ui.u(112.0), *id == self.theme);
                    theme_preview(ui, r.rect, &t, &if *mine { format!("{label} (yours)") } else { label.clone() });
                    if r.clicked {
                        chosen = Some(id.clone());
                    }
                }
            });
        }
        if let Some(t) = chosen {
            if t != self.theme {
                ui.theme = rustica_ui::theme_named(&t);
                self.set_appearance("theme", &t);
                self.status = format!("Theme {}", ui.theme.label);
                self.theme = t;
            }
        }
        ui.dim_label("Every window, the panel and the title bars change at once.");
        let mine = self.themes.iter().any(|t| t.0 == self.theme && t.2);
        let mut delete = false;
        ui.row(|ui| {
            if ui.button("Make your own from this one...") {
                let mut t = rustica_ui::theme_named(&self.theme);
                t.label = "";
                self.theme_edit = Some(ThemeEdit::new(t, None));
            }
            if mine && ui.button("Change it...") {
                let t = rustica_ui::theme_named(&self.theme);
                self.theme_edit = Some(ThemeEdit::new(t, Some(self.theme.clone())));
            }
            if mine && ui.button("Delete it") {
                delete = true;
            }
        });
        if delete {
            if let Some(d) = rustica_ui::user_theme_dir() {
                let _ = std::fs::remove_file(d.join(format!("{}.theme", self.theme)));
            }
            say(&format!("deleted the theme {}", self.theme));
            // Back to the system's theme of that name, if there is one, else Rustica
            let back = if rustica_ui::SYSTEM_THEMES.contains(&self.theme.as_str()) { self.theme.clone() } else { "rustica".to_string() };
            ui.theme = rustica_ui::theme_named(&back);
            self.set_appearance("theme", &back);
            self.theme = back;
            self.themes = rustica_ui::list_themes();
        }
        ui.space(8.0);
        ui.label("Wallpaper (any with any theme)");
        let mut wall = None;
        ui.row(|ui| {
            for (name, label) in WALLPAPERS {
                let pressed = if self.wallpaper == name { ui.primary_button(label) } else { ui.button(label) };
                if pressed {
                    wall = Some(name.to_string());
                }
            }
        });
        ui.dim_label("Or a picture of yours (PNG, JPEG or QOI), from any folder:");
        let mut use_path: Option<std::path::PathBuf> = None;
        ui.row(|ui| {
            let (_, enter) = ui.text_field("picture-path", &mut self.picture_path, 360.0);
            if (ui.button("Use") || enter) && !self.picture_path.trim().is_empty() {
                use_path = Some(expand_home(self.picture_path.trim()));
            }
            if self.chooser.is_none() && ui.button("Choose a file...") {
                let out = std::env::temp_dir().join(format!("settings-chooser-{}", std::process::id()));
                let _ = std::fs::remove_file(&out);
                let pics = sys::home().join("Pictures");
                let mut cmd = std::process::Command::new("/bin/chooser");
                cmd.args(["open", "--app", "Settings", "--title", "Wallpaper", "--patterns", "*.png *.jpg *.jpeg *.qoi"]);
                cmd.arg("--out").arg(&out).arg("--dir").arg(&pics);
                match cmd.spawn() {
                    Ok(c) => self.chooser = Some((c, out)),
                    Err(e) => self.message = Some(("Cannot open the file chooser".into(), e.to_string())),
                }
            }
        });
        // The chooser's answer
        if let Some((c, out)) = self.chooser.as_mut() {
            if let Ok(Some(st)) = c.try_wait() {
                if st.success() {
                    if let Ok(p) = std::fs::read_to_string(&*out) {
                        let p = p.trim().to_string();
                        self.picture_path = p.clone();
                        use_path = Some(p.into());
                    }
                }
                let _ = std::fs::remove_file(&*out);
                self.chooser = None;
            }
        }
        if !self.pictures.is_empty() {
            ui.dim_label("In Pictures and your home folder:");
            let names: Vec<String> = self.pictures.iter().map(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if p.parent() == Some(sys::home().join("Pictures").as_path()) { format!("Pictures/{n}") } else { n }
            }).collect();
            ui.list("pictures", &names, &mut self.picture, 100.0);
            if ui.button("Use this picture") {
                use_path = self.picture.and_then(|i| self.pictures.get(i)).cloned();
            }
        }
        if let Some(p) = use_path {
            match make_wallpaper(&p) {
                Ok(f) => wall = Some(f),
                Err(e) => self.message = Some(("Cannot use that picture".into(), e)),
            }
        }
        if let Some(w) = wall {
            self.wallpaper = w.clone();
            self.set_appearance("wallpaper", &w);
            self.status = "Wallpaper changed".into();
        }
    }

    /// Making or changing a theme of the user's own: its name, each token
    /// by hex code with a preview, then saved in
    /// ~/.config/rustica/themes/ID.theme and used
    fn theme_editor(&mut self, ui: &mut Ui) {
        let Some(e) = self.theme_edit.as_mut() else { return };
        ui.label(if e.editing.is_some() { "Change your theme" } else { "A theme of your own" });
        let mut done = None;
        ui.row(|ui| {
            let px = ui.font_px();
            let (nw, nh) = (ui.text_width("Name"), ui.row_h());
            let lr = ui.alloc(nw, nh);
            let (ty, tc) = (lr.y + (lr.h - ui.line_h()) / 2.0, ui.theme.text);
            ui.canvas.text(&mut ui.face, lr.x, ty, "Name", px, tc);
            ui.text_field("theme-name", &mut e.name, 260.0);
            if ui.primary_button("Save and use it") {
                done = Some(true);
            }
            if ui.button("Cancel") {
                done = Some(false);
            }
        });
        let r = ui.card("theme-edit-preview", ui.u(300.0), ui.u(170.0), true);
        let label = if e.name.trim().is_empty() { "(a name)".to_string() } else { e.name.trim().to_string() };
        theme_preview(ui, r.rect, &e.theme, &label);
        ui.dim_label("Each colour as a hex code (#RRGGBB); the preview follows.");
        let lw = ui.u(220.0);
        for (i, k) in rustica_ui::TOKENS.iter().enumerate() {
            ui.row(|ui| {
                let sw = ui.alloc(ui.row_h(), ui.row_h());
                let c = e.theme.get(k).unwrap_or(0);
                ui.canvas.fill_rounded(sw, ui.u(4.0), c);
                ui.canvas.stroke_rounded(sw, ui.u(4.0), 1.0, ui.theme.border);
                let lr = ui.alloc(lw, ui.row_h());
                let (px, tc) = (ui.font_px(), ui.theme.text);
                let ty = lr.y + (lr.h - ui.line_h()) / 2.0;
                ui.canvas.text(&mut ui.face, lr.x, ty, TOKEN_NAMES[i], px, tc);
                let (changed, _) = ui.text_field(&format!("tok-{k}"), &mut e.hex[i], 110.0);
                if changed {
                    let v = e.hex[i].trim().trim_start_matches('#');
                    if v.len() == 6 {
                        if let Ok(c) = u32::from_str_radix(v, 16) {
                            e.theme.set(k, c);
                        }
                    }
                }
                let v = e.hex[i].trim().trim_start_matches('#');
                if v.len() != 6 || u32::from_str_radix(v, 16).is_err() {
                    ui.dim_label("not a colour");
                }
            });
        }
        // Readability
        let t = e.theme;
        let body = rustica_ui::contrast(t.text, t.background);
        let on_surface = rustica_ui::contrast(t.text, t.surface);
        let on_primary = rustica_ui::contrast(t.accent_text, t.accent);
        if body < 4.5 || on_surface < 4.5 {
            ui.dim_label(&format!("The text may be hard to read (contrast {:.1}:1 on the background, {:.1}:1 on surfaces; 4.5 or more reads well)", body, on_surface));
        }
        if on_primary < 3.0 {
            ui.dim_label(&format!("Labels on primary buttons may be hard to read (contrast {on_primary:.1}:1)"));
        }
        if !e.error.is_empty() {
            let msg = e.error.clone();
            ui.dim_label(&msg);
        }
        match done {
            Some(false) => self.theme_edit = None,
            Some(true) => {
                let id = match &e.editing {
                    Some(id) => id.clone(),
                    None => theme_id(&e.name),
                };
                if e.name.trim().is_empty() || id.is_empty() {
                    e.error = "Give it a name (letters or digits)".into();
                    return;
                }
                if e.editing.is_none() && rustica_ui::SYSTEM_THEMES.contains(&id.as_str()) {
                    e.error = format!("{} is one of the system's themes: give yours another name", e.name.trim());
                    return;
                }
                if e.editing.is_none() && self.themes.iter().any(|t| t.0 == id) {
                    e.error = format!("You have a theme called {} already: give this one another name, or change that one", e.name.trim());
                    return;
                }
                let mut t = e.theme;
                t.label = Box::leak(e.name.trim().to_string().into_boxed_str());
                let Some(dir) = rustica_ui::user_theme_dir() else { return };
                let file = dir.join(format!("{id}.theme"));
                let text = format!("# A theme of {}'s, made in Settings > Appearance\n{}", std::env::var("USER").unwrap_or_default(), t.to_text());
                match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, text)) {
                    Ok(()) => {
                        say(&format!("saved the theme {id}"));
                        self.theme_edit = None;
                        self.themes = rustica_ui::list_themes();
                        ui.theme = rustica_ui::theme_named(&id);
                        // Written again even if it is the same: the compositor sees the change
                        self.set_appearance("theme", &id);
                        self.status = format!("Theme {}", ui.theme.label);
                        self.theme = id;
                    }
                    Err(err) => e.error = format!("Could not save {}: {err}", file.display()),
                }
            }
            None => {}
        }
    }

    /// Settings > Panel (DESKTOP.md step F10): where the panel is, how
    /// big, auto-hide, its applets and its launchers; saved at once, and
    /// the panel follows within a second
    fn panel_section(&mut self, ui: &mut Ui) {
        ui.heading("Panel");
        let mut changed = false;
        ui.label("Position");
        ui.row(|ui| {
            for (p, l) in [("bottom", "Bottom"), ("top", "Top"), ("left", "Left"), ("right", "Right")] {
                if ui.button_ex(&format!("pos-{p}"), l, 0.0, false, self.panel.position == p).clicked && self.panel.position != p {
                    self.panel.position = p.into();
                    changed = true;
                }
            }
        });
        let side = self.panel.position == "left" || self.panel.position == "right";
        ui.label(if side { "Width" } else { "Height" });
        ui.row(|ui| {
            let sizes: &[i32] = if side { &[120, 160, 180, 220, 280] } else { &[28, 32, 36, 44, 56] };
            for &n in sizes {
                let cur = if side { self.panel.width } else { self.panel.size };
                if ui.button_ex(&format!("size-{n}"), &n.to_string(), 0.0, false, cur == n).clicked && cur != n {
                    if side {
                        self.panel.width = n;
                    } else {
                        self.panel.size = n;
                    }
                    changed = true;
                }
            }
        });
        if ui.checkbox(&mut self.panel.autohide, "Hide the panel until the pointer reaches its edge") {
            changed = true;
        }
        ui.space(6.0);
        ui.label("Applets (choose one to move or remove it)");
        let zone_names = ["Start", "Centre", "End"];
        for z in 0..3 {
            ui.row(|ui| {
                ui.dim_label(zone_names[z]);
                if self.panel.zones[z].is_empty() {
                    ui.dim_label("(empty)");
                }
                for (i, a) in self.panel.zones[z].clone().iter().enumerate() {
                    let label = APPLETS.iter().find(|x| x.0 == a).map(|x| x.1).unwrap_or(a);
                    if ui.button_ex(&format!("applet-{z}-{i}"), label, 0.0, false, self.panel_sel == Some((z, i))).clicked {
                        self.panel_sel = if self.panel_sel == Some((z, i)) { None } else { Some((z, i)) };
                    }
                }
            });
        }
        if let Some((z, i)) = self.panel_sel.filter(|&(z, i)| i < self.panel.zones[z].len()) {
            let mut act = None;
            ui.row(|ui| {
                for (k, l) in [(0, "Earlier"), (1, "Later"), (2, "To start"), (3, "To centre"), (4, "To end"), (5, "Remove")] {
                    if ui.button(l) {
                        act = Some(k);
                    }
                }
            });
            if let Some(k) = act {
                let a = self.panel.zones[z][i].clone();
                match k {
                    0 if i > 0 => {
                        self.panel.zones[z].swap(i, i - 1);
                        self.panel_sel = Some((z, i - 1));
                    }
                    1 if i + 1 < self.panel.zones[z].len() => {
                        self.panel.zones[z].swap(i, i + 1);
                        self.panel_sel = Some((z, i + 1));
                    }
                    2..=4 => {
                        let to = k - 2;
                        if to != z {
                            self.panel.zones[z].remove(i);
                            self.panel.zones[to].push(a);
                            self.panel_sel = Some((to, self.panel.zones[to].len() - 1));
                        }
                    }
                    5 => {
                        self.panel.zones[z].remove(i);
                        self.panel_sel = None;
                    }
                    _ => {}
                }
                changed = true;
            }
        }
        let missing: Vec<(&str, &str)> = APPLETS
            .iter()
            .filter(|(id, _)| !self.panel.zones.iter().flatten().any(|a| a == id))
            .copied()
            .collect();
        if !missing.is_empty() {
            ui.row(|ui| {
                ui.dim_label("Add:");
                for (id, l) in &missing {
                    if ui.button_ex(&format!("add-{id}"), l, 0.0, false, false).clicked {
                        self.panel.zones[2].insert(0, id.to_string());
                        changed = true;
                    }
                }
            });
        }
        ui.space(6.0);
        ui.label("Launchers (pinned apps)");
        ui.row(|ui| {
            if self.panel.launchers.is_empty() {
                ui.dim_label("(none)");
            }
            for (i, l) in self.panel.launchers.clone().iter().enumerate() {
                let name = self.pinnable.iter().find(|p| &p.0 == l).map(|p| p.1.clone()).unwrap_or_else(|| l.clone());
                if ui.button_ex(&format!("pinned-{i}"), &name, 0.0, false, self.launcher_sel == Some(i)).clicked {
                    self.launcher_sel = if self.launcher_sel == Some(i) { None } else { Some(i) };
                }
            }
        });
        if let Some(i) = self.launcher_sel.filter(|&i| i < self.panel.launchers.len()) {
            ui.row(|ui| {
                if ui.button("Earlier") && i > 0 {
                    self.panel.launchers.swap(i, i - 1);
                    self.launcher_sel = Some(i - 1);
                    changed = true;
                }
                if ui.button("Later") && i + 1 < self.panel.launchers.len() {
                    self.panel.launchers.swap(i, i + 1);
                    self.launcher_sel = Some(i + 1);
                    changed = true;
                }
                if ui.button("Unpin") {
                    self.panel.launchers.remove(i);
                    self.launcher_sel = None;
                    changed = true;
                }
            });
        }
        let names: Vec<String> = self.pinnable.iter().map(|p| p.1.clone()).collect();
        if !names.is_empty() {
            ui.list("pinnable", &names, &mut self.pin_sel, 110.0);
            if ui.button("Pin to the panel") {
                if let Some((id, _)) = self.pin_sel.and_then(|i| self.pinnable.get(i)).cloned() {
                    if !self.panel.launchers.contains(&id) {
                        self.panel.launchers.push(id);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            match self.panel.save() {
                Ok(()) => {
                    say(&format!(
                        "panel {} {} {} start={} centre={} end={} launchers={}",
                        self.panel.position,
                        if side { self.panel.width } else { self.panel.size },
                        if self.panel.autohide { "autohide" } else { "shown" },
                        self.panel.zones[0].join(","),
                        self.panel.zones[1].join(","),
                        self.panel.zones[2].join(","),
                        self.panel.launchers.join(",")
                    ));
                    self.status = "Panel changed".into();
                }
                Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", PanelConf::path().display()))),
            }
        }
    }

    /// Settings > Desktop (DESKTOP.md step F12): which icons the desktop
    /// shows (~/.config/rustica/desktop.conf), and arranging them again
    fn desktop_section(&mut self, ui: &mut Ui) {
        ui.heading("Desktop");
        let p = sys::config_dir().join("rustica").join("desktop.conf");
        let kv = sys::parse_kv(&std::fs::read_to_string(&p).unwrap_or_default());
        let on = |k: &str| !matches!(kv.get(k).map(String::as_str), Some("no") | Some("false") | Some("0"));
        let mut changed = None;
        for (k, label) in [
            ("icons", "Show icons on the desktop"),
            ("home", "Home"),
            ("trash", "Trash"),
            ("drives", "Mounted drives"),
        ] {
            let mut v = on(k);
            if ui.checkbox(&mut v, label) {
                changed = Some((k, v));
            }
        }
        ui.dim_label("Files and folders in your Desktop folder are always shown with the icons.");
        if let Some((k, v)) = changed {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            match sys::write_kv(&p, k, if v { "yes" } else { "no" }) {
                Ok(()) => {
                    say(&format!("desktop {k} {}", if v { "yes" } else { "no" }));
                    self.status = "Desktop changed".into();
                }
                Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", p.display()))),
            }
        }
        ui.space(6.0);
        if ui.button("Arrange the icons again") {
            let _ = std::fs::remove_file(sys::config_dir().join("rustica").join("desktop-icons"));
            say("desktop icons arranged");
            self.status = "The icons are back in their columns".into();
        }
    }

    /// Settings > Power (DESKTOP.md step F15): the screensaver (blank
    /// after, how, lock then: ~/.config/rustica.conf, which the compositor
    /// reads), and what the lid and the power button do
    /// (~/.config/rustica/power.conf, for when the ACPI work lets Rustica
    /// see them)
    fn power_section(&mut self, ui: &mut Ui) {
        ui.heading("Power");
        let conf = sys::config_dir().join("rustica.conf");
        let kv = sys::parse_kv(&std::fs::read_to_string(&conf).unwrap_or_default());
        let blank: u32 = kv.get("blank").and_then(|v| v.parse().ok()).unwrap_or(15);
        let saver = kv.get("screensaver").cloned().unwrap_or_else(|| "blank".into());
        let mut lock = matches!(kv.get("lock_on_blank").map(String::as_str), Some("yes"));
        let mut set: Option<(&str, String)> = None;
        ui.label("Blank the screen when idle for");
        ui.row(|ui| {
            for (m, l) in [(0u32, "Never"), (1, "1 min"), (5, "5 min"), (10, "10 min"), (15, "15 min"), (30, "30 min"), (60, "1 hour")] {
                if ui.button_ex(&format!("blank-{m}"), l, 0.0, false, blank == m).clicked && blank != m {
                    set = Some(("blank", m.to_string()));
                }
            }
        });
        ui.label("Screensaver");
        ui.row(|ui| {
            for (k, l) in [("blank", "Black screen"), ("dim", "Dimmed wallpaper")] {
                if ui.button_ex(&format!("saver-{k}"), l, 0.0, false, saver == k).clicked && saver != k {
                    set = Some(("screensaver", k.to_string()));
                }
            }
        });
        if ui.checkbox(&mut lock, "Lock the screen when it blanks") {
            set = Some(("lock_on_blank", if lock { "yes" } else { "no" }.into()));
        }
        ui.dim_label("Locking needs a password: the live system's root has none (set one with passwd).");
        if let Some((k, v)) = set {
            let _ = std::fs::create_dir_all(sys::config_dir());
            match sys::write_kv(&conf, k, &v) {
                Ok(()) => {
                    say(&format!("power {k} {v}"));
                    self.status = "Power settings changed".into();
                }
                Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", conf.display()))),
            }
        }
        ui.space(8.0);
        let pconf = sys::config_dir().join("rustica").join("power.conf");
        let pk = sys::parse_kv(&std::fs::read_to_string(&pconf).unwrap_or_default());
        let lid = pk.get("lid").cloned().unwrap_or_else(|| "suspend".into());
        let button = pk.get("power_button").cloned().unwrap_or_else(|| "ask".into());
        let mut pset: Option<(&str, &str)> = None;
        ui.label("When the laptop's lid is closed");
        ui.row(|ui| {
            for (k, l) in [("suspend", "Suspend"), ("lock", "Lock the screen"), ("nothing", "Keep running")] {
                if ui.button_ex(&format!("lid-{k}"), l, 0.0, false, lid == k).clicked && lid != k {
                    pset = Some(("lid", k));
                }
            }
        });
        ui.label("When the power button is pressed");
        ui.row(|ui| {
            for (k, l) in [("ask", "Show the power menu"), ("poweroff", "Shut down"), ("suspend", "Suspend"), ("nothing", "Nothing")] {
                if ui.button_ex(&format!("pb-{k}"), l, 0.0, false, button == k).clicked && button != k {
                    pset = Some(("power_button", k));
                }
            }
        });
        if let Some((k, v)) = pset {
            let _ = std::fs::create_dir_all(pconf.parent().unwrap());
            match sys::write_kv(&pconf, k, v) {
                Ok(()) => {
                    say(&format!("power {k} {v}"));
                    self.status = "Power settings changed".into();
                }
                Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", pconf.display()))),
            }
        }
        ui.dim_label("The lid, the power button, the battery, Suspend and Hibernate need Rustica to read the");
        ui.dim_label("firmware's ACPI tables (planned): these choices are kept for then.");
    }

    fn keyboard(&mut self, ui: &mut Ui) {
        ui.heading("Keyboard");
        ui.label(&format!("Layout: {}", self.keymap_now));
        ui.dim_label("For the whole computer, the console too (/etc/keymap).");
        let h = (ui.remaining_height() - ui.line_h() * 4.0) / ui.scale;
        ui.list("keymaps", &self.keymaps, &mut self.keymap, h.clamp(60.0, 300.0));
        if ui.primary_button("Use this layout") {
            if let Some(k) = self.keymap.and_then(|i| self.keymaps.get(i)).cloned() {
                self.admin(&format!("keymap {k}"), vec![(vec!["/bin/localectl".into(), "set-keymap".into(), k], String::new())]);
            }
        }
    }

    fn mouse(&mut self, ui: &mut Ui) {
        ui.heading("Mouse");
        ui.label(&format!("Pointer speed: {}%", self.speed));
        ui.dim_label("How far the pointer moves for a movement of the mouse (/etc/input.conf).");
        ui.space(4.0);
        let steps = [25u32, 50, 75, 100, 150, 200, 300, 400];
        let mut chosen = None;
        ui.row(|ui| {
            for s in steps {
                let l = format!("{s}%");
                if if self.speed == s { ui.primary_button(&l) } else { ui.button(&l) } {
                    chosen = Some(s);
                }
            }
        });
        if let Some(s) = chosen {
            self.admin(&format!("pointer speed {s}"), vec![(vec!["/bin/inputctl".into(), "set-pointer-speed".into(), s.to_string()], String::new())]);
        }
    }

    fn network(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ctl.wake_after = Some(Duration::from_secs(2));
        if self.net_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(2)) {
            self.ifaces = rustica_ui::net::interfaces();
            self.net_at = Some(Instant::now());
        }
        ui.heading("Network");
        let Some(ifaces) = self.ifaces.clone() else {
            ui.label("The network service is not running.");
            ui.dim_label("Start it with: sudo svc start netd");
            return;
        };
        if ifaces.is_empty() {
            ui.label("No network card was found.");
            return;
        }
        let rows: Vec<Row> = ifaces
            .iter()
            .map(|i| Row {
                icon: Icon::None,
                cells: vec![
                    i.name.clone(),
                    match i.state.as_str() {
                        "up" => "connected".into(),
                        "no-carrier" => "no cable".into(),
                        s => s.to_string(),
                    },
                    i.v4.first().map(|(a, _)| a.clone()).unwrap_or_else(|| "\u{2014}".into()),
                    i.gateway.clone().unwrap_or_else(|| "\u{2014}".into()),
                ],
            })
            .collect();
        let cols = [
            Column { title: "Interface", width: 0.0, right: false },
            Column { title: "State", width: 110.0, right: false },
            Column { title: "Address", width: 150.0, right: false },
            Column { title: "Gateway", width: 120.0, right: false },
        ];
        if self.iface_sel.map_or(true, |k| k >= ifaces.len()) {
            self.iface_sel = Some(0);
        }
        ui.table("ifaces", &cols, &rows, &mut self.iface_sel, (ui.line_h() * 1.6 * (ifaces.len() as f32 + 1.5)) / ui.scale);
        let Some(i) = self.iface_sel.and_then(|k| ifaces.get(k)).cloned() else { return };
        // The editor follows the selection, from /etc/network.conf (a
        // fixed address starts as the one DHCP gave)
        if self.net_for != i.name {
            let (m, a, g) = conf_method(&i.name);
            self.method = m;
            self.addr_text = if a.is_empty() { i.v4.first().map(|(a, _)| a.clone()).unwrap_or_default() } else { a };
            self.gw_text = if g.is_empty() && m != 1 { i.gateway.clone().unwrap_or_default() } else { g };
            self.net_for = i.name.clone();
        }
        ui.dim_label(&format!("{} \u{b7} {} \u{b7} received {}, sent {}", i.driver, i.mac, sys::human_size(i.rx_bytes), sys::human_size(i.tx_bytes)));
        for (a, left) in &i.v4 {
            if let Some(s) = left {
                ui.dim_label(&format!("{a}: from DHCP, {} min left on the lease", s / 60));
            }
        }
        for a in i.v6.iter().filter(|a| !a.starts_with("fe80:")) {
            ui.dim_label(&format!("IPv6 {a}"));
        }
        ui.space(4.0);
        let mut chosen = None;
        ui.row(|ui| {
            for (k, m) in METHODS.iter().enumerate() {
                if if self.method == k { ui.primary_button(m) } else { ui.button(m) } {
                    chosen = Some(k);
                }
            }
        });
        if let Some(k) = chosen {
            self.method = k;
        }
        let mut enter = false;
        if self.method == 1 {
            let (_, e1) = ui.row(|ui| {
                ui.column(ui.u(90.0), |ui| ui.label("Address"));
                ui.text_field("addr", &mut self.addr_text, 180.0)
            });
            let (_, e2) = ui.row(|ui| {
                ui.column(ui.u(90.0), |ui| ui.label("Gateway"));
                ui.text_field("gw", &mut self.gw_text, 180.0)
            });
            ui.dim_label("The address with its prefix: 192.168.1.20/24. The gateway may be left empty.");
            enter = e1 || e2;
        }
        if ui.primary_button(&format!("Apply to {}", i.name)) || enter {
            let mut argv: Vec<String> = vec!["/bin/netcfg".into(), i.name.clone()];
            match self.method {
                0 => argv.push("dhcp".into()),
                1 => {
                    let a = self.addr_text.trim().to_string();
                    if !a.contains('/') {
                        self.message = Some(("Not an address".into(), "Give the address with its prefix, such as 192.168.1.20/24".into()));
                        return;
                    }
                    argv.push("static".into());
                    argv.push(a);
                    let g = self.gw_text.trim().to_string();
                    if !g.is_empty() {
                        argv.push(g);
                    }
                }
                _ => argv.push("off".into()),
            }
            let what = format!("network {}", argv[1..].join(" "));
            self.admin(&what, vec![(argv, String::new())]);
        }
        ui.space(8.0);
        ui.label("DNS servers");
        let now = rustica_ui::net::dns_servers();
        ui.dim_label(&format!("In use: {}", if now.is_empty() { "none".to_string() } else { now.join(", ") }));
        let (set, enter) = ui.row(|ui| {
            ui.column(ui.u(90.0), |ui| ui.label("Your own"));
            let (_, e) = ui.text_field("dns", &mut self.dns_text, 220.0);
            (ui.button("Use these servers"), e)
        });
        ui.dim_label("Tried before the ones DHCP gives; empty: DHCP's only. Addresses apart with spaces.");
        if set || enter {
            let mut argv: Vec<String> = vec!["/bin/netcfg".into(), "dns".into()];
            argv.extend(self.dns_text.split(|c: char| c == ' ' || c == ',').filter(|s| !s.is_empty()).map(str::to_string));
            let what = format!("dns {}", argv[2..].join(" "));
            self.admin(what.trim(), vec![(argv, String::new())]);
        }
    }

    fn time(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ctl.wake_after = Some(Duration::from_secs(1));
        ui.heading("Date & Time");
        use rustica_ui::clock::{self, Formats, DATE_FORMATS};
        let f = Formats::current();
        let tm = clock::local(clock::now());
        let mut full = f.clone();
        full.seconds = true;
        ui.label(&format!("{}   {}   ({})", f.date(&tm), full.time(&tm), self.zone_now));
        ui.space(4.0);
        // How dates and times are shown (2026-11-26): ~/.config/rustica.conf,
        // read by the panel's clock and Files
        ui.label("Time format");
        let mut set: Option<(&str, String)> = None;
        ui.row(|ui| {
            let mut t24 = f.clone();
            t24.h12 = false;
            let mut t12 = f.clone();
            t12.h12 = true;
            let a = format!("24-hour ({})", t24.time(&tm));
            let b = format!("12-hour ({})", t12.time(&tm));
            if if f.h12 { ui.button(&a) } else { ui.primary_button(&a) } {
                set = Some(("clock", "24".into()));
            }
            if if f.h12 { ui.primary_button(&b) } else { ui.button(&b) } {
                set = Some(("clock", "12".into()));
            }
        });
        let mut secs = f.seconds;
        if ui.checkbox(&mut secs, "Show seconds") {
            set = Some(("seconds", if secs { "yes" } else { "no" }.into()));
        }
        ui.label("Date format");
        for chunk in DATE_FORMATS.chunks(3) {
            ui.row(|ui| {
                for (name, _) in chunk {
                    let label = clock::date_as(name, &tm);
                    let pressed = if f.date == *name { ui.primary_button(&label) } else { ui.button(&label) };
                    if pressed {
                        set = Some(("date", name.to_string()));
                    }
                }
            });
        }
        let mut pd = f.panel_date;
        if ui.checkbox(&mut pd, "Show the date on the panel") {
            set = Some(("panel_date", if pd { "yes" } else { "no" }.into()));
        }
        if let Some((k, v)) = set {
            self.set_appearance(k, &v);
        }
        ui.space(4.0);
        let (set, enter) = ui.row(|ui| {
            ui.label("Set to");
            let (_, e) = ui.text_field("date", &mut self.date_text, 220.0);
            (ui.button("Set the clock"), e)
        });
        if set || enter {
            let t = self.date_text.trim().to_string();
            self.admin(&format!("time {t}"), vec![(vec!["/bin/timedatectl".into(), "set-time".into(), t], String::new())]);
        }
        ui.space(4.0);
        ui.label("Time zone");
        let (changed, _) = ui.row(|ui| {
            ui.label("Find");
            ui.text_field("zfilter", &mut self.zone_filter, 220.0)
        });
        let zones = self.filtered_zones();
        if changed {
            self.zone = zones.iter().position(|z| *z == self.zone_now);
        }
        let h = (ui.remaining_height() - ui.line_h() * 4.0) / ui.scale;
        ui.list("zones", &zones, &mut self.zone, h.clamp(60.0, 400.0));
        if ui.primary_button("Use this time zone") {
            if let Some(z) = self.zone.and_then(|i| zones.get(i)).cloned() {
                self.admin(&format!("timezone {z}"), vec![(vec!["/bin/timedatectl".into(), "set-timezone".into(), z], String::new())]);
            }
        }
    }

    fn users_section(&mut self, ui: &mut Ui) {
        ui.heading("Users");
        let rows: Vec<Row> = self
            .users
            .iter()
            .map(|(n, uid, home, admin)| Row { icon: Icon::None, cells: vec![n.clone(), uid.to_string(), home.clone(), if *admin { "administrator".into() } else { "user".into() }] })
            .collect();
        let cols = [
            Column { title: "Name", width: 0.0, right: false },
            Column { title: "Id", width: 70.0, right: true },
            Column { title: "Home", width: 170.0, right: false },
            Column { title: "Kind", width: 120.0, right: false },
        ];
        let h = (ui.remaining_height() - ui.line_h() * 13.0) / ui.scale;
        ui.table("users", &cols, &rows, &mut self.user_sel, h.max(80.0));
        let sel = self.user_sel.and_then(|i| self.users.get(i)).cloned();
        if let Some((name, uid, _, admin)) = &sel {
            let (rm, toggle) = ui.row(|ui| {
                let rm = *uid != 0 && ui.button(&format!("Remove {name}"));
                let toggle = *uid != 0 && ui.button(if *admin { "Make a normal user" } else { "Make an administrator" });
                (rm, toggle)
            });
            if rm {
                self.confirm_delete = Some(name.clone());
            }
            if toggle {
                let groups = if *admin {
                    // Every extra group but sudo
                    let g: Vec<String> = std::fs::read_to_string("/etc/group")
                        .unwrap_or_default()
                        .lines()
                        .filter(|l| !l.starts_with("sudo:"))
                        .filter(|l| l.rsplit(':').next().unwrap_or("").split(',').any(|m| m == name))
                        .map(|l| l.split(':').next().unwrap_or("").to_string())
                        .collect();
                    g.join(",")
                } else {
                    String::new()
                };
                let argv: Vec<String> = if *admin {
                    vec!["/bin/usermod".into(), "-G".into(), groups, name.clone()]
                } else {
                    vec!["/bin/usermod".into(), "-a".into(), "-G".into(), "sudo".into(), name.clone()]
                };
                self.admin(&format!("{} {name}", if *admin { "unadmin" } else { "admin" }), vec![(argv, String::new())]);
            }
        }
        ui.space(4.0);
        ui.label("A new user");
        let (_, _) = ui.row(|ui| {
            ui.label("Name");
            ui.text_field("uname", &mut self.new_name, 160.0)
        });
        let (_, enter) = ui.row(|ui| {
            ui.label("Password");
            ui.password_field("upw", &mut self.new_pw, 160.0)
        });
        ui.checkbox(&mut self.new_admin, "Administrator (may use sudo)");
        if ui.primary_button("Add the user") || enter {
            let name = self.new_name.trim().to_string();
            let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false);
            if !valid {
                self.message = Some(("Not a valid name".into(), "Lower-case letters, digits, - and _, starting with a letter".into()));
            } else if self.new_pw.is_empty() {
                self.message = Some(("No password".into(), "Give the new user a password".into()));
            } else {
                let mut add: Vec<String> = vec!["/bin/useradd".into(), "-m".into()];
                if self.new_admin {
                    add.push("-G".into());
                    add.push("sudo".into());
                }
                add.push(name.clone());
                let pw = format!("{name}:{}\n", self.new_pw);
                self.admin(&format!("useradd {name}"), vec![(add, String::new()), (vec!["/bin/chpasswd".into()], pw)]);
                self.new_name.clear();
                self.new_pw.clear();
                self.new_admin = false;
            }
        }
    }

    fn disks_section(&mut self, ui: &mut Ui) {
        ui.heading("Disks");
        ui.dim_label("The disks and their partitions, as lsblk shows them (Files mounts USB sticks).");
        let cols = [
            Column { title: "Name", width: 0.0, right: false },
            Column { title: "Size", width: 80.0, right: true },
            Column { title: "Kind", width: 90.0, right: false },
            Column { title: "Filesystem", width: 100.0, right: false },
            Column { title: "Mounted at", width: 140.0, right: false },
        ];
        let rows = self.disks.clone();
        ui.table("disks", &cols, &rows, &mut self.disk_sel, 0.0);
    }

    fn install_updates(&mut self) {
        let input = if sys::euid() == 0 { String::new() } else { format!("{}\n", self.password.clone().unwrap_or_default()) };
        say("updates install");
        self.job_lines.clear();
        self.new_image = None;
        self.job = Some(("install".into(), start_job(vec!["/bin/updates".into(), "install".into(), "-S".into()], input)));
    }

    fn updates_section(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ui.heading("Updates");
        // What runs, and what it said
        if let Some((what, job)) = self.job.clone() {
            ctl.wake_after = Some(Duration::from_millis(300));
            let (lines, done) = {
                let g = job.lock().unwrap();
                (g.lines.clone(), g.done)
            };
            self.job_lines = lines;
            if let Some(code) = done {
                self.job = None;
                self.ups = up_status();
                if what == "install" {
                    if code == 0 {
                        // A new system image (rustica-system-VERSION: N files installed)
                        self.new_image = self.job_lines.iter().find_map(|l| {
                            l.strip_prefix("rustica-system-").and_then(|r| r.split_once(' ')).filter(|(_, rest)| rest.contains("installed")).map(|(v, _)| v.trim_end_matches(':').to_string())
                        });
                        say(&format!("updates installed{}", self.new_image.as_ref().map(|v| format!(" (new system image {v})")).unwrap_or_default()));
                        self.status = "The updates are installed.".into();
                    } else {
                        let why = self.job_lines.iter().rev().find(|l| l.starts_with("updates:")).cloned().unwrap_or_else(|| format!("updates install ended with {code}"));
                        if why.contains("wrong password") || why.contains("not an administrator") {
                            self.password = None;
                        }
                        say(&format!("updates failed: {why}"));
                        self.message = Some(("The updates were not installed".into(), why));
                    }
                } else {
                    say(&format!("updates checked: {}", self.ups.steps.len()));
                }
            }
        }
        let running = self.job.is_some();
        if self.ups.checked == 0 {
            ui.label("Not checked yet.");
        } else {
            let tm = sys::local(self.ups.checked);
            ui.dim_label(&format!(
                "Last checked {:04}-{:02}-{:02} {:02}:{:02}{}",
                tm.year + 1900,
                tm.mon + 1,
                tm.mday,
                tm.hour,
                tm.min,
                if self.ups.error.is_empty() { String::new() } else { format!(": {}", self.ups.error) }
            ));
        }
        if self.ups.steps.is_empty() {
            if self.ups.checked != 0 && self.ups.error.is_empty() {
                ui.label("Everything is up to date.");
            }
        } else {
            let total: u64 = self.ups.steps.iter().map(|s| s.3).sum();
            ui.label(&format!("{} package{} to upgrade, {} to download", self.ups.steps.len(), if self.ups.steps.len() == 1 { "" } else { "s" }, sys::human_size(total)));
            let rows: Vec<Row> = self
                .ups
                .steps
                .iter()
                .map(|(n, from, to, size, why)| Row {
                    icon: Icon::None,
                    cells: vec![n.clone(), if from.is_empty() { format!("new{}", if why.is_empty() { String::new() } else { format!(", for {why}") }) } else { from.clone() }, to.clone(), sys::human_size(*size)],
                })
                .collect();
            let cols = [
                Column { title: "Package", width: 0.0, right: false },
                Column { title: "Installed", width: 170.0, right: false },
                Column { title: "New", width: 90.0, right: false },
                Column { title: "Size", width: 80.0, right: true },
            ];
            let h = (ui.line_h() * 1.6 * (rows.len().min(8) as f32 + 1.5)) / ui.scale;
            ui.table("updates", &cols, &rows, &mut self.up_sel, h);
            for k in &self.ups.kept {
                ui.dim_label(&format!("Kept back: {k}"));
            }
        }
        ui.space(4.0);
        let (check, install) = ui.row(|ui| {
            let c = !running && ui.button("Check now");
            let i = !running && !self.ups.steps.is_empty() && ui.primary_button("Install updates");
            (c, i)
        });
        if check {
            say("updates check");
            self.job_lines.clear();
            self.job = Some(("check".into(), start_job(vec!["/bin/updates".into(), "check".into()], String::new())));
        }
        if install {
            if sys::euid() == 0 || self.password.is_some() {
                self.install_updates();
            } else {
                self.pw_text.clear();
                self.pending = Some(Pending { what: "install the updates".into(), cmds: Vec::new(), updates: true });
            }
        }
        if running {
            ui.label(if self.job.as_ref().map(|j| j.0.as_str()) == Some("install") { "Installing the updates..." } else { "Checking..." });
        }
        if !self.job_lines.is_empty() {
            for l in self.job_lines.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                ui.dim_label(l);
            }
        }
        if let Some(v) = self.new_image.clone() {
            ui.space(4.0);
            ui.label(&format!("The new system image {v} starts at the next restart (the previous one stays in the boot menu)."));
            if ui.primary_button("Restart now") {
                self.admin("restart", vec![(vec!["/bin/reboot".into()], String::new())]);
            }
        }
        ui.space(8.0);
        ui.label("Sources");
        let src = std::fs::read_to_string("/etc/rpg/sources.list").unwrap_or_default();
        for l in src.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            ui.dim_label(l);
        }
        ui.dim_label("The system checks once a day (/etc/rpg/updates.conf); updates in a terminal: man updates.");
    }

    /// Settings > Apps (FoxDen step FD6): each installed app, what it
    /// takes up, what it may do (as FoxDen's Installed), Remove
    fn apps_section(&mut self, ui: &mut Ui) {
        ui.heading("Apps");
        // Apps from outside FoxDen (FoxDen step FD7c): an administrator's
        // choice whether anyone may install them from package files
        let mut anyone = outside_anyone();
        if ui.checkbox(&mut anyone, "Allow everyone to install apps from outside FoxDen (package files)") {
            let v = if anyone { "anyone" } else { "admin" };
            self.admin(&format!("apps from outside FoxDen: {v}"), vec![(vec!["/bin/rpg".into(), "config".into(), "outside".into(), v.into()], String::new())]);
        }
        ui.dim_label("They still run in their sandbox; files:home and system always need an administrator.");
        ui.separator();
        if self.apps.is_empty() {
            ui.dim_label("No apps are installed. FoxDen has them.");
            if ui.button("Open FoxDen") {
                let _ = Command::new("/bin/foxden").spawn();
            }
            return;
        }
        ui.dim_label("Turning a permission off takes effect the next time the app starts.");
        let h = (ui.remaining_height() - ui.line_h() * 2.0) / ui.scale;
        let mut reload = false;
        let apps = self.apps.clone();
        ui.scroll_area("apps", h, |ui| {
            for a in &apps {
                ui.push_id(&a.id);
                ui.label(&format!("{}  {}", a.name, a.version));
                ui.dim_label(&format!("    {} — the app {}, your data for it {}", a.id, bytes_text(a.folder), bytes_text(a.data)));
                match a.origin.as_str() {
                    "" | "foxden" | "system" => {}
                    "file" => ui.dim_label("    Not from FoxDen: installed from a package file. Nobody has checked it."),
                    o => ui.dim_label(&format!("    Not from FoxDen: from {}. Nobody has checked it.", o.strip_prefix("source ").map(|u| format!("the app source {u}")).unwrap_or(o.to_string()))),
                }
                let mut g = app_granted(&a.id).unwrap_or_else(|| a.perms.clone());
                let mut changed = false;
                if a.perms.is_empty() {
                    ui.dim_label("    Asks for no permissions");
                }
                for p in &a.perms {
                    let mut on = g.contains(p) && p != "system";
                    if ui.checkbox(&mut on, &format!("{} ({p})", perm_words(p))) {
                        changed = true;
                        if on {
                            g.push(p.clone());
                        } else {
                            g.retain(|x| x != p);
                        }
                    }
                }
                if changed {
                    g.retain(|p| a.perms.contains(p));
                    match app_set_granted(&a.id, &g) {
                        Ok(()) => say(&format!("app permissions {}: {}", a.id, if g.is_empty() { "-".to_string() } else { g.join(" ") })),
                        Err(e) => self.status = e,
                    }
                }
                ui.row(|ui| {
                    if ui.button("Open") {
                        say(&format!("open app {}", a.id));
                        let _ = Command::new("/bin/appctl").args(["run", &a.id]).spawn();
                    }
                    if ui.button("In FoxDen") {
                        let _ = Command::new("/bin/foxden").arg(&a.id).spawn();
                    }
                    if ui.button("Remove") {
                        say(&format!("remove app {}", a.id));
                        // Removing an app needs no administrator (the
                        // installer service's rule for rpg run by a user)
                        match Command::new("/bin/rpg").args(["remove", &a.id]).stdin(Stdio::null()).output() {
                            Ok(o) if o.status.success() => self.status = format!("{} is removed; your data for it stays.", a.name),
                            Ok(o) => self.status = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("rpg remove failed").to_string(),
                            Err(e) => self.status = format!("rpg: {e}"),
                        }
                        reload = true;
                    }
                });
                ui.separator();
                ui.pop_id();
            }
        });
        if reload {
            self.apps = load_apps();
        }
    }

    fn about_section(&mut self, ui: &mut Ui) {
        ui.heading("About");
        for (k, v) in self.about.clone() {
            ui.row(|ui| {
                ui.column(ui.u(150.0), |ui| ui.dim_label(&k));
                ui.column(0.0, |ui| ui.label(&v));
            });
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        if self.chooser.is_some() {
            // Look for the file chooser's answer
            ctl.wake_after = Some(Duration::from_millis(300));
        }
        let w = ui.available_width();
        let names: Vec<String> = SECTIONS.iter().map(|s| s.to_string()).collect();
        let before = self.section;
        ui.row(|ui| {
            ui.column(ui.u(170.0).min(w * 0.3), |ui| {
                let h = (ui.remaining_height() - ui.line_h() * 2.0) / ui.scale;
                ui.list("sections", &names, &mut self.section, h);
            });
            // The section's page scrolls when the window is too small
            ui.column(0.0, |ui| {
                let sec = self.section.unwrap_or(0);
                ui.scroll_page(&format!("page{sec}"), 30.0, |ui| match sec {
                    S_DISPLAY => self.display(ui),
                    S_APPEARANCE => self.appearance(ui),
                    S_PANEL => self.panel_section(ui),
                    S_DESKTOP => self.desktop_section(ui),
                    S_POWER => self.power_section(ui),
                    S_KEYBOARD => self.keyboard(ui),
                    S_MOUSE => self.mouse(ui),
                    S_NETWORK => self.network(ui, ctl),
                    S_TIME => self.time(ui, ctl),
                    S_USERS => self.users_section(ui),
                    S_DISKS => self.disks_section(ui),
                    S_UPDATES => self.updates_section(ui, ctl),
                    S_APPS => self.apps_section(ui),
                    _ => self.about_section(ui),
                })
            });
        });
        if self.section != before {
            self.status.clear();
            if let Some(s) = self.section {
                say(&format!("section {}", SECTIONS[s]));
            }
            if self.section == Some(S_DISKS) || self.section == Some(S_ABOUT) {
                self.load_disks();
                self.load_about();
            }
            if self.section == Some(S_UPDATES) {
                self.ups = up_status();
            }
            if self.section == Some(S_APPEARANCE) {
                self.pictures = pictures();
                self.themes = rustica_ui::list_themes();
            }
            if self.section == Some(S_APPS) {
                self.apps = load_apps();
            }
            if self.section == Some(S_PANEL) {
                self.panel = PanelConf::load();
                self.pinnable = pinnable();
            }
        }
        ui.focus_if_none("sections");
        if !self.status.is_empty() {
            ui.dim_label(&self.status.clone());
        }
        ctl.requests.push(rustica_ui::Request::Title(format!("{} — Settings", SECTIONS[self.section.unwrap_or(0)])));
        self.dialogs(ui);
    }

    fn dialogs(&mut self, ui: &mut Ui) {
        if self.pending.is_some() {
            // The password, in a bar over the bottom of the window
            let what = self.pending.as_ref().unwrap().what.clone();
            ui.separator();
            ui.label(&format!("Your password, to change the system ({what}):"));
            let (ok, cancel, enter) = ui.row(|ui| {
                let (_, e) = ui.password_field("sudo", &mut self.pw_text, 220.0);
                (ui.primary_button("OK"), ui.button("Cancel"), e)
            });
            if !ui.has_focus("sudo") && !cancel {
                ui.set_focus("sudo");
            }
            if ok || enter {
                self.password = Some(std::mem::take(&mut self.pw_text));
                let p = self.pending.take().unwrap();
                if p.updates {
                    self.install_updates();
                } else {
                    self.run_admin(p);
                }
            } else if cancel {
                self.pending = None;
            }
            return;
        }
        if let Some(name) = self.confirm_delete.clone() {
            match ui.dialog("Remove the user?", &format!("{name} and their home folder will be deleted."), &["Remove", "Cancel"]) {
                Some(0) => {
                    self.confirm_delete = None;
                    self.user_sel = None;
                    self.admin(&format!("userdel {name}"), vec![(vec!["/bin/userdel".into(), "-r".into(), name], String::new())]);
                }
                Some(_) => self.confirm_delete = None,
                None => {}
            }
            return;
        }
        if let Some((t, m)) = self.message.clone() {
            if ui.dialog(&t, &m, &["OK"]).is_some() {
                self.message = None;
            }
        }
    }
}

fn main() {
    // settings --wallpaper FILE (Files > Set as wallpaper): no window
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--wallpaper") {
        let Some(src) = args.get(2) else {
            eprintln!("usage: settings --wallpaper FILE");
            std::process::exit(2);
        };
        let notify = |text: &str| {
            let _ = rustica_ui::rustica_window::notify(text);
        };
        match make_wallpaper(&expand_home(src)) {
            Ok(f) => {
                let p = sys::config_dir().join("rustica.conf");
                let _ = std::fs::create_dir_all(sys::config_dir());
                if let Err(e) = sys::write_kv(&p, "wallpaper", &f) {
                    eprintln!("settings: {}: {e}", p.display());
                    notify("Could not save the wallpaper");
                    std::process::exit(1);
                }
                say(&format!("wallpaper {f}"));
            }
            Err(e) => {
                eprintln!("settings: {e}");
                notify(&format!("Not a picture the wallpaper can be: {e}"));
                std::process::exit(1);
            }
        }
        return;
    }
    let mut s = Settings::new();
    if let Some(a) = std::env::args().nth(1) {
        let a = a.to_lowercase();
        s.section = SECTIONS.iter().position(|n| n.to_lowercase().contains(&a)).or(Some(0));
        if s.section == Some(S_APPS) {
            s.apps = load_apps();
        }
        if s.section == Some(S_PANEL) {
            s.panel = PanelConf::load();
            s.pinnable = pinnable();
        }
        if s.section == Some(S_APPEARANCE) {
            s.pictures = pictures();
            s.themes = rustica_ui::list_themes();
        }
    }
    say("started");
    if let Err(e) = rustica_ui::run("Settings", 820, 560, None, |ui, ctl| s.frame(ui, ctl)) {
        eprintln!("settings: {e}");
        std::process::exit(1);
    }
}

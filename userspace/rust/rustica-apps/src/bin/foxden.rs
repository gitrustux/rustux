// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! foxden: FoxDen, Rustica's app store (FoxDen step FD5, docs/FOXDEN.md).
//!
//!   foxden [ID | --updates | FILE.rpg]   an app's page, the Updates
//!                              page, or a package file's app (FD7c)
//!
//! An app not from FoxDen (FoxDen step FD7c: from a package file, or an
//! app source no store key signs) says so on its card and page; its
//! developer is shown as claimed, not verified; installing it asks first.
//!
//! Pages: Home (every app the app sources have), Categories, Search,
//! Installed (each app's permissions, which the user may narrow:
//! ~/.config/apps/permissions, what appctl reads), Updates (apps, and the
//! system's from the installer service's last check), and an app's page
//! (icon, screenshots, description, developer, licence, sizes, what it
//! asks for in plain words; Install, Open, Update, Remove).
//!
//! What it shows comes from `rpg list --apps --available`; what it does
//! is rpg's: `rpg install -y ID`, `rpg remove ID`, `rpg upgrade -y`, `rpg
//! update`, run as a child whose output it shows. For a user other than
//! root rpg goes through the installer service (step FD4), which may need
//! an administrator's password: FoxDen asks for it in its own sheet and
//! hands it to rpg. Icons and screenshots are fetched once (curl, or a
//! file:// copy) into ~/.cache/foxden/ID/.
//!
//! It prints what happens ("foxden: ..."), for the tests.

use rustica_ui::gfx::{self, Rect};
use rustica_ui::{Control, Request, Ui};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

// ---- what the sources have ----

#[derive(Clone, Default, Debug)]
struct App {
    id: String,
    version: String,
    name: String,
    summary: String,
    description: Vec<String>,
    categories: Vec<String>,
    permissions: Vec<String>,
    icon: Option<String>,  // a URL, or a local file
    shots: Vec<String>,
    developer: String,
    source_code: String,
    license: String,
    homepage: String,
    size: u64,
    installed_size: u64, // KiB
    installed: Option<String>,
    upgradable: bool,
    source: String,
    /// rpg's From: foxden, "source URL" or file; Installed-From: where
    /// the installed one came from (FoxDen step FD7c)
    from: String,
    installed_from: String,
    /// A package file's app (foxden FILE.rpg): the file
    file: Option<String>,
}

/// How an origin reads
fn origin_words(o: &str) -> String {
    match o {
        "file" => "a package file".into(),
        o => o.strip_prefix("source ").map(|u| format!("the app source {u}")).unwrap_or_else(|| o.to_string()),
    }
}

impl App {
    /// Not from FoxDen: where it is from (None: FoxDen, the system's, or
    /// installed before rpg kept where from)
    fn outside(&self) -> Option<String> {
        let o = if self.installed.is_some() { &self.installed_from } else { &self.from };
        (!o.is_empty() && o != "foxden" && o != "system").then(|| origin_words(o))
    }
}

fn stanzas(text: &str) -> Vec<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = cur.last_mut() {
                let l = line.trim();
                last.1.push('\n');
                last.1.push_str(if l == "." { "" } else { l });
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            cur.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn load_apps() -> Result<Vec<App>, String> {
    load_from(&["list", "--apps", "--available"])
}

fn load_from(args: &[&str]) -> Result<Vec<App>, String> {
    let o = Command::new("/bin/rpg").args(args).output().map_err(|e| format!("rpg: {e}"))?;
    if !o.status.success() {
        return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let mut apps = Vec::new();
    for st in stanzas(&text) {
        let get = |k: &str| st.iter().find(|(f, _)| f.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()).unwrap_or_default();
        let words = |k: &str| get(k).split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let source = get("Source");
        let url = |rel: &str| if source == "-" || source.is_empty() { String::new() } else { format!("{}/{rel}", source.trim_end_matches('/')) };
        let desc = get("Description");
        let mut lines = desc.lines();
        let first = lines.next().unwrap_or("").to_string();
        let id = get("Package");
        let icon = if !get("Icon-File").is_empty() {
            Some(get("Icon-File"))
        } else if !get("Icon").is_empty() {
            Some(url(&get("Icon")))
        } else {
            None
        };
        apps.push(App {
            name: if get("App-Name").is_empty() { first.clone() } else { get("App-Name") },
            summary: get("App-Summary"),
            description: lines.map(str::to_string).collect(),
            categories: words("Categories"),
            permissions: words("Permissions"),
            icon: icon.filter(|s| !s.is_empty()),
            shots: words("Screenshots").iter().map(|s| url(s)).filter(|s| !s.is_empty()).collect(),
            // An outside developer's app (FoxDen step FD7a): its
            // developer's name and ID; the project's: the Maintainer
            developer: match (get("App-Developer-Name"), get("App-Developer")) {
                (n, d) if !n.is_empty() && !d.is_empty() => format!("{n} ({d})"),
                (_, d) if !d.is_empty() => d,
                _ => get("Maintainer"),
            },
            from: get("From"),
            installed_from: get("Installed-From"),
            file: None,
            source_code: get("App-Source"),
            license: get("License"),
            homepage: get("Homepage"),
            size: get("Size").parse().unwrap_or(0),
            installed_size: get("Installed-Size").parse().unwrap_or(0),
            installed: Some(get("Installed")).filter(|v| !v.is_empty()),
            upgradable: get("Upgradable") == "yes",
            version: get("Version"),
            source,
            id,
        });
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(apps)
}

/// The system's updates, from the installer service's last check
/// (/var/lib/rpg/updates: "step NAME FROM TO SIZE WHY"), apps left out
fn system_updates(apps: &[App]) -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string("/var/lib/rpg/updates").unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 4 && w[0] == "step" && !apps.iter().any(|a| a.id == w[1])).then(|| (w[1].to_string(), w[2].to_string(), w[3].to_string()))
        })
        .collect()
}

/// The installer service's app lines (FoxDen step FD6): updates that wait
/// because they ask for more (ID, version, permissions), and apps updated
/// in the background (ID, version), newest first
fn app_news() -> (Vec<(String, String, Vec<String>)>, Vec<(String, String)>) {
    let t = std::fs::read_to_string("/var/lib/rpg/updates").unwrap_or_default();
    let mut waits = Vec::new();
    let mut done = Vec::new();
    for l in t.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.first() {
            Some(&"appwaits") if w.len() >= 5 => waits.push((w[1].to_string(), w[3].to_string(), w[4].split(',').map(str::to_string).collect())),
            Some(&"appupdated") if w.len() >= 5 => done.push((w[2].to_string(), w[4].to_string())),
            _ => {}
        }
    }
    done.reverse();
    (waits, done)
}

fn size_text(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / 1048576.0)
    } else {
        format!("{} KB", b.div_ceil(1024).max(1))
    }
}

/// What a permission means, in words
fn permission_words(p: &str) -> &'static str {
    match p {
        "network" => "Use the network and the internet",
        "files:documents" => "Read and change your Documents folder",
        "files:pictures" => "Read and change your Pictures folder",
        "files:music" => "Read and change your Music folder",
        "files:videos" => "Read and change your Videos folder",
        "files:downloads" => "Read and change your Downloads folder",
        "files:home" => "Read and change everything in your home folder",
        "removable" => "Use USB sticks and other removable drives",
        "audio" => "Play and record sound",
        "camera" => "Use the camera",
        "location" => "Know where you are",
        "system" => "Run outside the sandbox, as your own programs do",
        _ => "Something this system does not know",
    }
}

// ---- the user ----

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

fn user() -> (String, u32) {
    extern "C" {
        fn getuid() -> u32;
    }
    let uid = unsafe { getuid() };
    let name = std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 3 && f[2].parse::<u32>().ok() == Some(uid)).then(|| f[0].to_string())
        })
        .unwrap_or_default();
    (name, uid)
}

/// In group sudo (rpg then asks for the user's own password, not a name)
fn is_admin(name: &str) -> bool {
    let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let Some(sudo) = groups.lines().find(|l| l.starts_with("sudo:")) else { return false };
    let f: Vec<&str> = sudo.split(':').collect();
    if f.len() > 3 && f[3].split(',').any(|m| m == name) {
        return true;
    }
    let gid = f.get(2).copied().unwrap_or("");
    passwd.lines().any(|l| {
        let p: Vec<&str> = l.split(':').collect();
        p.len() > 3 && p[0] == name && p[3] == gid
    })
}

/// The user's narrowing of an app's permissions (~/.config/apps/
/// permissions, appctl's): None when there is no line for it
fn granted(id: &str) -> Option<Vec<String>> {
    let t = std::fs::read_to_string(format!("{}/.config/apps/permissions", home())).ok()?;
    t.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == id).then(|| v.split_whitespace().filter(|w| *w != "-").map(str::to_string).collect())
    })
}

fn set_granted(id: &str, perms: &[String]) -> Result<(), String> {
    let dir = format!("{}/.config/apps", home());
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir}: {e}"))?;
    let path = format!("{dir}/permissions");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| l.split_once(':').map(|(k, _)| k.trim() != id).unwrap_or(true)).map(str::to_string).collect();
    lines.push(format!("{id}: {}", if perms.is_empty() { "-".to_string() } else { perms.join(" ") }));
    std::fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{path}: {e}"))
}

// ---- pictures, fetched once ----

fn cache_path(id: &str, url: &str) -> String {
    let name: String = url.rsplit('/').next().unwrap_or("x").chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    format!("{}/.cache/foxden/{id}/{name}", home())
}

/// The picture's bytes: from the cache, a file, or the network (curl)
fn fetch(id: &str, url: &str) -> Result<Vec<u8>, String> {
    if !url.contains("://") {
        return std::fs::read(url).map_err(|e| format!("{url}: {e}"));
    }
    let path = cache_path(id, url);
    if let Ok(d) = std::fs::read(&path) {
        return Ok(d);
    }
    let _ = std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap());
    if let Some(f) = url.strip_prefix("file://") {
        std::fs::copy(f, &path).map_err(|e| format!("{f}: {e}"))?;
    } else {
        let tmp = format!("{path}.part");
        let ok = Command::new("/bin/curl").args(["-fsS", "-o", &tmp, url]).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{url}: could not be fetched"));
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    std::fs::read(&path).map_err(|e| e.to_string())
}

fn scaled(p: &gfx::Pixmap, w: u32, h: u32) -> gfx::Pixmap {
    let mut out = gfx::Pixmap::new(w.max(1), h.max(1)).unwrap();
    let t = gfx::Transform::from_scale(w as f32 / p.width() as f32, h as f32 / p.height() as f32);
    out.draw_pixmap(0, 0, p.as_ref(), &gfx::PixmapPaint { quality: gfx::FilterQuality::Bilinear, ..Default::default() }, t, None);
    out
}

// ---- rpg, running ----

struct Job {
    what: String,  // "install", "remove", "upgrade", "update"
    id: Option<String>,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Arc<Mutex<Vec<String>>>,
    /// rpg says an administrator's password is needed: why
    auth: Arc<Mutex<Option<String>>>,
}

fn reader(mut r: impl Read + Send + 'static, lines: Arc<Mutex<Vec<String>>>, auth: Arc<Mutex<Option<String>>>, waker: Option<rustica_ui::rustica_window::Waker>) {
    std::thread::spawn(move || {
        let mut br = std::io::BufReader::new(&mut r);
        let mut line = String::new();
        while br.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
            let l = line.trim_end().to_string();
            println!("foxden: rpg: {l}");
            if let Some(why) = l.strip_suffix(": an administrator's password is needed") {
                *auth.lock().unwrap() = Some(why.to_string());
            }
            lines.lock().unwrap().push(l);
            line.clear();
            if let Some(w) = waker {
                w.wake();
            }
        }
        if let Some(w) = waker {
            w.wake();
        }
    });
}

fn start_job(what: &str, id: Option<&str>, args: &[&str], waker: Option<rustica_ui::rustica_window::Waker>) -> Result<Job, String> {
    let mut child = Command::new("/bin/rpg").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("rpg: {e}"))?;
    let lines = Arc::new(Mutex::new(Vec::new()));
    let auth = Arc::new(Mutex::new(None));
    reader(child.stdout.take().unwrap(), lines.clone(), auth.clone(), waker);
    reader(child.stderr.take().unwrap(), lines.clone(), auth.clone(), waker);
    println!("foxden: {what}{}", id.map(|i| format!(" {i}")).unwrap_or_default());
    Ok(Job { what: what.to_string(), id: id.map(str::to_string), stdin: child.stdin.take(), child, lines, auth })
}

// ---- the window ----

#[derive(Clone, PartialEq)]
enum Page {
    Home,
    Categories,
    Search,
    Installed,
    Updates,
    App(String),
}

const TABS: [&str; 5] = ["Home", "Categories", "Search", "Installed", "Updates"];

struct FoxDen {
    apps: Vec<App>,
    error: Option<String>,
    page: Page,
    tab: usize,
    category: Option<String>,
    query: String,
    pics: HashMap<String, Option<gfx::Pixmap>>,
    sizes: HashMap<(String, u32, u32), gfx::Pixmap>,
    wanted: HashSet<String>,
    pic_tx: mpsc::Sender<(String, Option<Vec<u8>>)>,
    pic_rx: mpsc::Receiver<(String, Option<Vec<u8>>)>,
    fetch_tx: Option<mpsc::Sender<(String, String)>>,
    job: Option<Job>,
    message: Option<(String, bool)>, // (text, an error)
    auth_name: String,
    auth_pw: String,
    me: String,
    admin: bool,
    focus_next: Option<String>,
    confirm_remove: Option<String>,
    /// Install an app not from FoxDen: asked first (FD7c)
    confirm_outside: Option<String>,
    /// The password sheet is up (it took the focus when it came)
    sheet_up: bool,
    waker: Option<rustica_ui::rustica_window::Waker>,
}

impl FoxDen {
    fn reload(&mut self) {
        match load_apps() {
            Ok(a) => {
                // A package file's app (foxden FILE.rpg) stays, unless it
                // is installed now (then rpg lists it)
                let files: Vec<App> = self.apps.iter().filter(|x| x.file.is_some()).cloned().collect();
                self.apps = a;
                for f in files {
                    if let Some(i) = self.apps.iter().position(|x| x.id == f.id) {
                        if self.apps[i].installed.is_some() && f.installed.is_none() {
                            continue;
                        }
                        self.apps[i] = f;
                    } else {
                        self.apps.push(f);
                    }
                }
                self.error = None;
                println!(
                    "foxden: {} apps ({} installed, {} to update)",
                    self.apps.len(),
                    self.apps.iter().filter(|a| a.installed.is_some()).count(),
                    self.apps.iter().filter(|a| a.upgradable).count()
                );
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn app(&self, id: &str) -> Option<&App> {
        self.apps.iter().find(|a| a.id == id)
    }

    /// A picture, asked for once (fetched by a thread); None until there
    fn pic(&mut self, id: &str, url: &str) -> Option<&gfx::Pixmap> {
        if !self.pics.contains_key(url) && self.wanted.insert(url.to_string()) {
            if self.fetch_tx.is_none() {
                let (tx, rx) = mpsc::channel::<(String, String)>();
                let out = self.pic_tx.clone();
                let waker = self.waker;
                std::thread::spawn(move || {
                    for (id, url) in rx {
                        let d = fetch(&id, &url).ok();
                        if out.send((url, d)).is_err() {
                            break;
                        }
                        if let Some(w) = waker {
                            w.wake();
                        }
                    }
                });
                self.fetch_tx = Some(tx);
            }
            let _ = self.fetch_tx.as_ref().unwrap().send((id.to_string(), url.to_string()));
        }
        self.pics.get(url).and_then(|p| p.as_ref())
    }

    fn pic_sized(&mut self, id: &str, url: &str, w: u32, h: u32) -> Option<gfx::Pixmap> {
        let key = (url.to_string(), w, h);
        if let Some(p) = self.sizes.get(&key) {
            return Some(p.clone());
        }
        let p = self.pic(id, url)?.clone();
        let s = scaled(&p, w, h);
        self.sizes.insert(key, s.clone());
        Some(s)
    }

    fn take_pictures(&mut self) {
        while let Ok((url, d)) = self.pic_rx.try_recv() {
            let p = d.and_then(|d| gfx::decode_image(&d).ok());
            self.pics.insert(url, p);
        }
    }

    fn open_page(&mut self, p: Page) {
        if let Page::App(id) = &p {
            println!("foxden: page {id}");
            let label = match self.app(id) {
                Some(a) if a.installed.is_some() => "Open",
                _ => "Install",
            };
            self.focus_next = Some(label.to_string());
        }
        self.page = p;
    }

    fn run(&mut self, what: &str, id: Option<&str>, args: &[&str]) {
        if self.job.is_some() {
            return;
        }
        self.message = None;
        self.auth_name.clear();
        self.auth_pw.clear();
        match start_job(what, id, args, self.waker) {
            Ok(j) => self.job = Some(j),
            Err(e) => self.message = Some((e, true)),
        }
    }

    /// The job's progress: done, it is reaped and the list read again
    fn poll_job(&mut self) {
        let Some(job) = self.job.as_mut() else { return };
        let Ok(Some(st)) = job.child.try_wait() else { return };
        // Let the readers finish
        std::thread::sleep(std::time::Duration::from_millis(50));
        let job = self.job.take().unwrap();
        let lines = job.lines.lock().unwrap().clone();
        let ok = st.success();
        let name = job.id.as_deref().and_then(|i| self.app(i)).map(|a| a.name.clone()).unwrap_or_default();
        let text = match (job.what.as_str(), ok) {
            ("install", true) => format!("{name} is installed."),
            ("remove", true) => format!("{name} is removed. Your data for it stays."),
            ("upgrade", true) => "The updates are installed.".to_string(),
            ("update", true) => "Checked for new apps and updates.".to_string(),
            (_, false) => lines
                .iter()
                .rev()
                .find(|l| l.starts_with("rpg:") || l.contains("refused") || l.contains("REFUSED"))
                .cloned()
                .unwrap_or_else(|| format!("rpg {} ended with {}", job.what, st.code().unwrap_or(-1)))
                .trim_start_matches("rpg: ")
                .to_string(),
            _ => String::new(),
        };
        println!("foxden: {} {}{}", job.what, if ok { "done" } else { "failed" }, job.id.as_deref().map(|i| format!(" {i}")).unwrap_or_default());
        self.message = Some((text, !ok));
        self.reload();
        if let (true, Some(id)) = (ok, &job.id) {
            if matches!(self.page, Page::App(_)) {
                let id = id.clone();
                self.open_page(Page::App(id));
            }
        }
    }

    fn send_auth(&mut self) {
        let Some(job) = self.job.as_mut() else { return };
        let mut answer = String::new();
        if !self.admin {
            answer.push_str(&format!("{}\n", self.auth_name.trim()));
        }
        answer.push_str(&format!("{}\n", self.auth_pw));
        if let Some(i) = job.stdin.as_mut() {
            let _ = i.write_all(answer.as_bytes());
            let _ = i.flush();
        }
        // zeroed as far as a String lets us
        unsafe { self.auth_pw.as_bytes_mut().iter_mut().for_each(|b| *b = 0) };
        self.auth_pw.clear();
        *job.auth.lock().unwrap() = None;
    }

    fn cancel_auth(&mut self) {
        if let Some(job) = self.job.as_mut() {
            job.stdin = None; // rpg reads the end: no administrator
            *job.auth.lock().unwrap() = None;
        }
    }

    // ---- pieces ----

    /// An app's card: icon, name, summary, a badge. True when clicked
    fn card(&mut self, ui: &mut Ui, a: &App, w: f32) -> bool {
        let h = ui.u(76.0);
        let r = ui.alloc(w, h);
        let (mx, my) = ui.input.mouse;
        let hot = r.contains(mx, my);
        let th = ui.theme;
        ui.canvas.fill_rounded(r, ui.u(8.0), if hot { th.surface_hi } else { th.surface });
        let isz = ui.u(48.0) as u32;
        let ix = r.x + ui.u(14.0);
        let iy = r.y + (h - isz as f32) / 2.0;
        match a.icon.clone().and_then(|u| self.pic_sized(&a.id, &u, isz, isz)) {
            Some(p) => ui.canvas.image(&p, ix, iy),
            None => ui.canvas.fill_rounded(Rect::new(ix, iy, isz as f32, isz as f32), ui.u(10.0), th.border),
        }
        let tx = ix + isz as f32 + ui.u(14.0);
        let px = ui.font_px();
        let maxw = r.right() - tx - ui.u(10.0);
        let name = fit(ui, &a.name, px, maxw, true);
        let (y1, y2) = (r.y + ui.u(14.0), r.y + ui.u(38.0));
        ui.canvas.text(&mut ui.bold, tx, y1, &name, px, th.text);
        let sum = fit(ui, &a.summary, px * 0.9, maxw, false);
        ui.canvas.text(&mut ui.face, tx, y2, &sum, px * 0.9, th.dim);
        let outside = a.outside().is_some();
        let badge = if a.upgradable { "Update" } else if a.installed.is_some() && !outside { "Installed" } else if outside { "Not from FoxDen" } else { "" };
        if !badge.is_empty() {
            let bw = ui.face.width(badge, px * 0.8) + ui.u(12.0);
            let br = Rect::new(r.right() - bw - ui.u(8.0), r.y + ui.u(8.0), bw, ui.u(20.0));
            ui.canvas.fill_rounded(br, ui.u(10.0), if a.upgradable { th.accent } else if outside { 0x8A5A1E } else { 0x3B5E45 });
            let (bx, by) = (br.x + ui.u(6.0), br.y + ui.u(2.0));
            ui.canvas.text(&mut ui.face, bx, by, badge, px * 0.8, if a.upgradable { th.accent_text } else { th.text });
        }
        hot && ui.input.released
    }

    /// Cards in a grid, in a scrolling area; the app clicked
    fn grid(&mut self, ui: &mut Ui, name: &str, list: &[App]) -> Option<String> {
        let top = ui.alloc(0.0, 0.0).y;
        let h = (ui.remaining_height() / ui.scale - 8.0).max(80.0);
        let mut clicked = None;
        let bottom = top + ui.u(h);
        ui.scroll_area(name, h, |ui| {
            let w = ui.available_width();
            let gap = ui.u(8.0);
            let cols = ((w + gap) / (ui.u(260.0) + gap)).floor().max(1.0) as usize;
            let cw = ((w - gap * (cols as f32 - 1.0)) / cols as f32).floor();
            for chunk in list.chunks(cols) {
                ui.row(|ui| {
                    for a in chunk {
                        let (_, my) = ui.input.mouse;
                        if self.card(ui, a, cw) && my >= top && my < bottom {
                            clicked = Some(a.id.clone());
                        }
                    }
                });
            }
            if list.is_empty() {
                ui.dim_label("No apps here.");
            }
        });
        clicked
    }

    // ---- pages ----

    fn home(&mut self, ui: &mut Ui) {
        let n = self.apps.len();
        ui.heading("Apps for Rustica");
        ui.dim_label(&format!("{n} app{} from the app sources. They run in a sandbox: each one sees only its own folder and what it asks for.", if n == 1 { "" } else { "s" }));
        let list = self.apps.clone();
        if let Some(id) = self.grid(ui, "home", &list) {
            self.open_page(Page::App(id));
        }
    }

    fn categories(&mut self, ui: &mut Ui) {
        let all: BTreeSet<String> = self.apps.iter().flat_map(|a| a.categories.clone()).collect();
        ui.row(|ui| {
            for c in &all {
                let on = self.category.as_deref() == Some(c.as_str());
                let clicked = if on { ui.primary_button(c) } else { ui.button(c) };
                if clicked {
                    self.category = Some(c.clone());
                }
            }
        });
        let Some(c) = self.category.clone().or_else(|| all.iter().next().cloned()) else {
            ui.dim_label("No apps yet.");
            return;
        };
        let list: Vec<App> = self.apps.iter().filter(|a| a.categories.contains(&c)).cloned().collect();
        if let Some(id) = self.grid(ui, "cat", &list) {
            self.open_page(Page::App(id));
        }
    }

    fn search(&mut self, ui: &mut Ui) {
        ui.focus_if_none("query");
        let w = ui.available_width();
        ui.text_field("query", &mut self.query, w);
        let q = self.query.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let list: Vec<App> = self
            .apps
            .iter()
            .filter(|a| {
                let hay = format!("{} {} {} {} {}", a.id, a.name, a.summary, a.description.join(" "), a.categories.join(" ")).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .cloned()
            .collect();
        if let Some(id) = self.grid(ui, "search", &list) {
            self.open_page(Page::App(id));
        }
    }

    fn installed(&mut self, ui: &mut Ui) {
        let list: Vec<App> = self.apps.iter().filter(|a| a.installed.is_some()).cloned().collect();
        if list.is_empty() {
            ui.dim_label("No apps are installed. Home and Search have them.");
            return;
        }
        ui.dim_label("What each app may do. Turning a permission off takes effect the next time the app starts.");
        let h = (ui.remaining_height() / ui.scale - 8.0).max(80.0);
        ui.scroll_area("installed", h, |ui| {
            for a in &list {
                ui.push_id(&a.id);
                ui.row(|ui| {
                    let isz = ui.u(32.0) as u32;
                    let r = ui.alloc(isz as f32, isz as f32);
                    if let Some(p) = a.icon.clone().and_then(|u| self.pic_sized(&a.id, &u, isz, isz)) {
                        ui.canvas.image(&p, r.x, r.y);
                    }
                    ui.label(&format!("{}  {}", a.name, a.installed.clone().unwrap_or_default()));
                    if ui.button("Open") {
                        println!("foxden: open {}", a.id);
                        let _ = Command::new("/bin/appctl").args(["run", &a.id]).spawn();
                    }
                    if ui.button("Details") {
                        self.open_page(Page::App(a.id.clone()));
                    }
                });
                if a.permissions.is_empty() {
                    ui.dim_label("    Asks for nothing beyond its own folder and a window");
                }
                let mut g = granted(&a.id).unwrap_or_else(|| a.permissions.clone());
                let mut changed = false;
                for p in &a.permissions {
                    let mut on = g.contains(p) && p != "system";
                    let label = format!("{} ({p})", permission_words(p));
                    if ui.checkbox(&mut on, &label) {
                        changed = true;
                        if on {
                            g.push(p.clone());
                        } else {
                            g.retain(|x| x != p);
                        }
                    }
                }
                if changed {
                    g.retain(|p| a.permissions.contains(p));
                    match set_granted(&a.id, &g) {
                        Ok(()) => println!("foxden: permissions {}: {}", a.id, if g.is_empty() { "-".to_string() } else { g.join(" ") }),
                        Err(e) => self.message = Some((e, true)),
                    }
                }
                ui.separator();
                ui.pop_id();
            }
        });
    }

    fn updates(&mut self, ui: &mut Ui) {
        let apps: Vec<App> = self.apps.iter().filter(|a| a.upgradable).cloned().collect();
        let sys = system_updates(&self.apps);
        ui.row(|ui| {
            if ui.button("Check for updates") {
                self.run("update", None, &["update"]);
            }
            if (!apps.is_empty() || !sys.is_empty()) && ui.primary_button("Install all updates") {
                self.run("upgrade", None, &["upgrade", "-y"]);
            }
        });
        if apps.is_empty() && sys.is_empty() {
            ui.dim_label("Everything is up to date (as of the last check).");
        }
        let (waits, done) = app_news();
        let waiting: Vec<&str> = waits.iter().map(|w| w.0.as_str()).collect();
        let plain: Vec<&App> = apps.iter().filter(|a| !waiting.contains(&a.id.as_str())).collect();
        if !waits.is_empty() {
            ui.label("Waiting for you: these new versions ask for more");
            for (id, to, perms) in &waits {
                let name = self.app(id).map(|a| a.name.clone()).unwrap_or(id.clone());
                let words: Vec<&str> = perms.iter().map(|p| permission_words(p)).collect();
                ui.push_id(id);
                ui.row(|ui| {
                    ui.dim_label(&format!("    {name} {to}: {}", words.join("; ")));
                    if ui.button("Allow and update") {
                        self.run("install", Some(id), &["install", "-y", id]);
                    }
                });
                ui.pop_id();
            }
        }
        if !plain.is_empty() {
            ui.label("Apps");
            for a in &plain {
                ui.dim_label(&format!("    {}  {} → {}", a.name, a.installed.clone().unwrap_or_default(), a.version));
            }
        }
        if !done.is_empty() {
            ui.label("Updated in the background");
            for (id, to) in done.iter().take(8) {
                let name = self.app(id).map(|a| a.name.clone()).unwrap_or(id.clone());
                ui.dim_label(&format!("    {name} {to}"));
            }
        }
        if !sys.is_empty() {
            ui.label("The system (an administrator's password is needed)");
            for (n, from, to) in &sys {
                ui.dim_label(&format!("    {n}  {} → {to}", if from == "-" { "new" } else { from }));
            }
        }
    }

    fn app_page(&mut self, ui: &mut Ui, id: &str) {
        let Some(a) = self.app(id).cloned() else {
            ui.label(&format!("{id}: no app source has it (Check for updates on the Updates page, or rpg update)."));
            return;
        };
        let busy = self.job.is_some();
        let isz = ui.u(72.0) as u32;
        let h = (ui.remaining_height() / ui.scale - 8.0).max(80.0);
        let mut act: Option<&str> = None;
        let focus = self.focus_next.take();
        ui.scroll_area("app", h, |ui| {
            // (inside: the buttons' IDs are the scroll area's)
            if let Some(f) = &focus {
                ui.set_focus(f);
            }
            ui.row(|ui| {
                let r = ui.alloc(isz as f32, isz as f32);
                match a.icon.clone().and_then(|u| self.pic_sized(&a.id, &u, isz, isz)) {
                    Some(p) => ui.canvas.image(&p, r.x, r.y),
                    None => ui.canvas.fill_rounded(r, ui.u(14.0), ui.theme.border),
                }
                ui.column(0.0, |ui| {
                    ui.heading(&a.name);
                    ui.dim_label(&a.summary);
                    // Not from FoxDen (FD7c): said, and asked about
                    if let Some(w) = a.outside() {
                        ui.label(&format!("Not from FoxDen: from {w}."));
                        ui.dim_label("Nobody has checked this app. It runs in its sandbox with the permissions below.");
                    }
                    let asking = self.confirm_outside.as_deref() == Some(a.id.as_str());
                    ui.row(|ui| {
                        if busy {
                            ui.dim_label("Working…");
                        } else if asking {
                            if ui.primary_button("Install anyway") {
                                act = Some("install-anyway");
                            }
                            if ui.button("Cancel") {
                                act = Some("cancel");
                            }
                        } else if a.installed.is_none() {
                            if ui.primary_button("Install") {
                                act = Some(if a.outside().is_some() { "ask" } else { "install" });
                            }
                        } else {
                            if ui.primary_button("Open") {
                                act = Some("open");
                            }
                            if a.upgradable && ui.button("Update") {
                                act = Some("update");
                            }
                            if ui.button("Remove") {
                                act = Some("remove");
                            }
                        }
                    });
                });
            });
            ui.separator();
            // Screenshots, as wide as the page allows
            let w = ui.available_width();
            for s in a.shots.clone() {
                if let Some(p) = self.pic(&a.id, &s).cloned() {
                    let k = (w / p.width() as f32).min(ui.u(360.0) / p.height() as f32).min(1.0);
                    let (sw, sh) = ((p.width() as f32 * k) as u32, (p.height() as f32 * k) as u32);
                    if let Some(sp) = self.pic_sized(&a.id, &s, sw, sh) {
                        ui.image(&sp);
                    }
                }
            }
            for l in &a.description {
                if l.is_empty() {
                    ui.space(4.0);
                } else {
                    ui.label(l);
                }
            }
            ui.space(6.0);
            ui.label("What it may do");
            if a.permissions.is_empty() {
                ui.dim_label("    Nothing beyond its own folder and a window: it asks for no permissions.");
            }
            for p in &a.permissions {
                let extra = if p == "files:home" || p == "system" { "  (an administrator says yes when it is installed)" } else { "" };
                ui.dim_label(&format!("    • {}{extra}", permission_words(p)));
            }
            ui.space(6.0);
            ui.label("Information");
            let mut rows: Vec<(String, String)> = vec![
                ("Version".into(), match (&a.installed, a.upgradable) {
                    (Some(i), true) => format!("{i} installed, {} available", a.version),
                    _ => a.version.clone(),
                }),
                ("Developer".into(), if a.outside().is_some() && !a.developer.is_empty() { format!("claims to be {}, not verified", a.developer) } else { a.developer.clone() }),
                ("Licence".into(), a.license.clone()),
                ("Download".into(), if a.size > 0 { size_text(a.size) } else { "-".into() }),
                ("Installed size".into(), size_text(a.installed_size * 1024)),
                ("Categories".into(), a.categories.join(", ")),
                ("ID".into(), a.id.clone()),
                ("From".into(), match (&a.file, a.outside()) {
                    (Some(f), _) => format!("the package file {f}"),
                    (None, Some(w)) => w,
                    _ if a.source == "-" => "a package file".into(),
                    _ => format!("FoxDen ({})", a.source),
                }),
            ];
            if !a.homepage.is_empty() {
                rows.push(("Homepage".into(), a.homepage.clone()));
            }
            if !a.source_code.is_empty() {
                rows.push(("Source code".into(), a.source_code.clone()));
            }
            for (k, v) in rows.into_iter().filter(|(_, v)| !v.is_empty()) {
                ui.dim_label(&format!("    {k}:  {v}"));
            }
        });
        let target = a.file.clone().unwrap_or_else(|| a.id.clone());
        match act {
            Some("ask") => {
                println!("foxden: {} is not from FoxDen ({}): asking first", a.id, a.outside().unwrap_or_default());
                self.confirm_outside = Some(a.id.clone());
            }
            Some("cancel") => {
                println!("foxden: not installed: {}", a.id);
                self.confirm_outside = None;
            }
            Some("install-anyway") => {
                println!("foxden: install anyway: {}", a.id);
                self.confirm_outside = None;
                self.run("install", Some(&a.id), &["install", "-y", &target]);
            }
            Some("install") => self.run("install", Some(&a.id), &["install", "-y", &target]),
            Some("update") => self.run("install", Some(&a.id), &["install", "-y", &a.id]),
            Some("remove") => self.confirm_remove = Some(a.id.clone()),
            Some("open") => {
                println!("foxden: open {}", a.id);
                if let Err(e) = Command::new("/bin/appctl").args(["run", &a.id]).spawn() {
                    self.message = Some((format!("appctl: {e}"), true));
                }
            }
            _ => {}
        }
    }

    /// The administrator's password, for the job waiting on it
    fn auth_sheet(&mut self, ui: &mut Ui, why: &str) {
        ui.heading("An administrator's password is needed");
        ui.label(&format!("{why}."));
        // The first field takes the focus when the sheet comes
        let first = !self.sheet_up;
        self.sheet_up = true;
        if self.admin {
            ui.dim_label(&format!("Your password ({}), as an administrator:", self.me));
        } else {
            ui.dim_label("You are not an administrator: someone who is types their name and password.");
            if first {
                ui.set_focus("name");
            }
            ui.row(|ui| {
                field_label(ui, "Name");
                ui.text_field("name", &mut self.auth_name, ui.u(220.0));
            });
        }
        if self.admin && first {
            ui.set_focus("password");
        }
        let mut go = false;
        ui.row(|ui| {
            field_label(ui, "Password");
            let (_, enter) = ui.password_field("password", &mut self.auth_pw, ui.u(220.0));
            go |= enter;
        });
        ui.row(|ui| {
            if ui.primary_button("Continue") {
                go = true;
            }
            if ui.button("Cancel") {
                self.cancel_auth();
            }
        });
        if go && (self.admin || !self.auth_name.trim().is_empty()) {
            self.send_auth();
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        if self.waker.is_none() {
            self.waker = ctl.waker;
        }
        self.take_pictures();
        self.poll_job();
        if self.job.is_some() {
            ctl.wake_after = Some(std::time::Duration::from_millis(250));
        }
        for k in &ui.input.keys {
            if k.ctrl() && k.usage == 0x14 {
                ctl.quit = true;
            }
        }
        // The confirmation for Remove
        if let Some(id) = self.confirm_remove.clone() {
            let name = self.app(&id).map(|a| a.name.clone()).unwrap_or(id.clone());
            self.base(ui, ctl);
            match ui.dialog(&format!("Remove {name}?"), "Its folder goes; your data for it stays.", &["Remove", "Cancel"]) {
                Some(0) => {
                    self.confirm_remove = None;
                    self.run("remove", Some(&id), &["remove", &id]);
                }
                Some(_) => self.confirm_remove = None,
                None => {}
            }
            return;
        }
        self.base(ui, ctl);
    }

    fn base(&mut self, ui: &mut Ui, ctl: &mut Control) {
        let title = match &self.page {
            Page::App(id) => format!("{} — FoxDen", self.app(id).map(|a| a.name.clone()).unwrap_or(id.clone())),
            _ => "FoxDen".to_string(),
        };
        ctl.requests.push(Request::Title(title));
        // A job that waits for an administrator
        let why = self.job.as_ref().and_then(|j| j.auth.lock().unwrap().clone());
        if let Some(why) = why {
            self.auth_sheet(ui, &why);
            return;
        }
        self.sheet_up = false;
        ui.row(|ui| {
            let mut t = self.tab;
            if ui.tabs("tabs", &TABS, &mut t) || (t != self.tab) {
                self.tab = t;
                self.page = match t {
                    0 => Page::Home,
                    1 => Page::Categories,
                    2 => Page::Search,
                    3 => Page::Installed,
                    _ => Page::Updates,
                };
            }
        });
        if matches!(self.page, Page::App(_)) && ui.button("← Back") {
            self.page = match self.tab {
                1 => Page::Categories,
                2 => Page::Search,
                3 => Page::Installed,
                4 => Page::Updates,
                _ => Page::Home,
            };
        }
        // What rpg says (while it runs), or how it ended
        if let Some(j) = &self.job {
            let last = j.lines.lock().unwrap().last().cloned().unwrap_or_else(|| format!("rpg {}…", j.what));
            ui.dim_label(&last);
        } else if let Some((m, err)) = self.message.clone() {
            ui.row(|ui| {
                if err {
                    let px = ui.font_px();
                    let w = ui.face.width(&m, px);
                    let r = ui.alloc(w, ui.line_h());
                    ui.canvas.text(&mut ui.face, r.x, r.y, &m, px, ui.theme.danger);
                } else {
                    ui.label(&m);
                }
                if ui.button("OK") {
                    self.message = None;
                }
            });
        }
        if let Some(e) = &self.error {
            ui.label(&format!("The app list could not be read: {e}"));
            return;
        }
        match self.page.clone() {
            Page::Home => self.home(ui),
            Page::Categories => self.categories(ui),
            Page::Search => self.search(ui),
            Page::Installed => self.installed(ui),
            Page::Updates => self.updates(ui),
            Page::App(id) => self.app_page(ui, &id),
        }
    }
}

/// A field's label in a column of its own, centred on the field's line
fn field_label(ui: &mut Ui, text: &str) {
    let h = ui.line_h() + ui.u(12.0);
    let r = ui.alloc(ui.u(90.0), h);
    let px = ui.font_px();
    let y = r.y + (h - ui.line_h()) / 2.0;
    let c = ui.theme.text;
    ui.canvas.text(&mut ui.face, r.x, y, text, px, c);
}

/// `text` shortened with … to fit `w`
fn fit(ui: &mut Ui, text: &str, px: f32, w: f32, bold: bool) -> String {
    let face = if bold { &mut ui.bold } else { &mut ui.face };
    if face.width(text, px) <= w {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && face.width(&format!("{s}…"), px) > w {
        s.pop();
    }
    format!("{}…", s.trim_end())
}

fn main() {
    let arg = std::env::args().nth(1);
    if arg.as_deref().is_some_and(|a| a == "-h" || a == "--help") {
        println!("usage: foxden [ID | --updates | FILE.rpg]");
        return;
    }
    let (me, uid) = user();
    let admin = uid == 0 || is_admin(&me);
    let (pic_tx, pic_rx) = mpsc::channel();
    let mut f = FoxDen {
        apps: Vec::new(),
        error: None,
        page: Page::Home,
        tab: 0,
        category: None,
        query: String::new(),
        pics: HashMap::new(),
        sizes: HashMap::new(),
        wanted: HashSet::new(),
        pic_tx,
        pic_rx,
        fetch_tx: None,
        job: None,
        message: None,
        auth_name: String::new(),
        auth_pw: String::new(),
        me,
        admin,
        focus_next: None,
        confirm_remove: None,
        confirm_outside: None,
        sheet_up: false,
        waker: None,
    };
    f.reload();
    match arg.as_deref() {
        Some("--updates") => {
            f.tab = 4;
            f.page = Page::Updates;
        }
        // A package file (FD7c): its app's page, from the file
        Some(path) if path.ends_with(".rpg") => {
            let abs = std::fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_string());
            match load_from(&["list", "--apps", "--file", &abs]) {
                Ok(mut v) if !v.is_empty() => {
                    let mut a = v.remove(0);
                    a.file = Some(abs.clone());
                    println!("foxden: package file {abs}: {} {}{}", a.id, a.version, if a.outside().is_some() { ", not from FoxDen" } else { "" });
                    let id = a.id.clone();
                    if let Some(i) = f.apps.iter().position(|x| x.id == id) {
                        f.apps[i] = a;
                    } else {
                        f.apps.push(a);
                    }
                    f.open_page(Page::App(id));
                }
                Ok(_) => f.message = Some((format!("{abs}: no app in it"), true)),
                Err(e) => f.message = Some((e, true)),
            }
        }
        Some(id) => f.open_page(Page::App(id.to_string())),
        None => {}
    }
    if let Err(e) = rustica_ui::run("FoxDen", 900, 640, None, |ui, ctl| f.frame(ui, ctl)) {
        eprintln!("foxden: {e}");
        std::process::exit(1);
    }
}

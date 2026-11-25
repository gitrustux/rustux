// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! files: Rustica's file manager (DESKTOP.md step F7). Browse folders;
//! copy, cut and paste, rename, move to the trash (or delete for good),
//! new folders; open a file with the application for its type
//! (/usr/share/applications, `types=`) or a folder in Terminal;
//! properties (size, times, owner, permissions); the trash (restore,
//! empty); USB sticks and other disks: mount them under /media and eject
//! them (mount and umount, step 14).
//!
//!   files [FOLDER]
//!
//! Keys: Enter opens, Backspace or Alt+Up goes up, Alt+Left back, F2
//! renames, Delete moves to the trash (Shift+Delete deletes), Ctrl+C,
//! Ctrl+X, Ctrl+V, Ctrl+Shift+N new folder, Ctrl+H hidden files, F5
//! reloads, Alt+Enter properties. Prints what it does ("files: ...") for
//! the tests.

use rustica_apps::sys;
use rustica_ui::{Column, Control, Icon, Row, Ui};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MOD_ALT: u32 = 0x44;

fn say(s: &str) {
    println!("files: {s}");
    let _ = std::io::stdout().flush();
}

#[derive(Clone)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    link: bool,
    size: u64,
    mtime: i64,
    mode: u32,
}

#[derive(Clone)]
struct Drive {
    dev: String,
    label: String,
    fstype: String,
    size: u64,
    removable: bool,
    mount: Option<PathBuf>,
}

enum Prompt {
    Rename(PathBuf),
    NewFolder,
}

enum Dialog {
    Delete(Vec<PathBuf>),
    EmptyTrash,
    Info(String, String),
    Properties(PathBuf),
}

struct Files {
    cwd: PathBuf,
    back: Vec<PathBuf>,
    entries: Vec<Entry>,
    rows: Vec<Row>,
    selected: Option<usize>,
    hidden: bool,
    clip: Option<(Vec<PathBuf>, bool)>, // paths, cut
    status: String,
    prompt: Option<(Prompt, String)>,
    dialog: Option<Dialog>,
    path_text: String,
    places: Vec<(String, PathBuf, Icon)>,
    place_sel: Option<usize>,
    drives: Vec<Drive>,
    scanned: Option<Instant>,
    scanner: std::sync::Arc<std::sync::Mutex<Option<Vec<Drive>>>>,
    scanning: bool,
    users: std::collections::HashMap<u32, String>,
    groups: std::collections::HashMap<u32, String>,
}

fn trash_dir() -> PathBuf {
    sys::home().join(".local/share/Trash")
}

fn is_trash(p: &Path) -> bool {
    p == trash_dir().join("files")
}

fn icon_for(e: &Entry) -> Icon {
    if e.dir {
        return Icon::Folder;
    }
    let ext = e.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => Icon::Image,
        _ if e.mode & 0o111 != 0 => Icon::Program,
        _ => Icon::File,
    }
}

/// A name that does not exist in dir yet: "a.txt", "a (copy).txt", "a (copy 2).txt"
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if std::fs::symlink_metadata(&p).is_err() {
        return p;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let cand = if n == 1 { format!("{stem} (copy){ext}") } else { format!("{stem} (copy {n}){ext}") };
        let p = dir.join(cand);
        if std::fs::symlink_metadata(&p).is_err() {
            return p;
        }
    }
    unreachable!()
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(from)?;
    if md.is_dir() {
        if to.starts_with(from) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a folder cannot be copied into itself"));
        }
        std::fs::create_dir(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        let _ = std::fs::set_permissions(to, md.permissions());
    } else if md.file_type().is_symlink() {
        let target = std::fs::read_link(from)?;
        std::os::unix::fs::symlink(target, to)?;
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

fn remove_tree(p: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(p)?;
    if md.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Another filesystem: copy, then remove
            copy_tree(from, to)?;
            remove_tree(from)
        }
    }
}

/// The size of a file, or of everything in a folder
fn total_size(p: &Path) -> (u64, u64) {
    let Ok(md) = std::fs::symlink_metadata(p) else { return (0, 0) };
    if !md.is_dir() {
        return (md.len(), 1);
    }
    let (mut size, mut n) = (0, 0);
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let (s, k) = total_size(&e.path());
            size += s;
            n += k;
        }
    }
    (size, n)
}

/// lsblk -J: the block devices (flat; a disk's partitions are listed
/// after it)
fn scan_drives() -> Vec<Drive> {
    let Ok(out) = sys::run("/bin/lsblk", &["-J", "-b", "-o", "NAME,RM,SIZE,TYPE,FSTYPE,LABEL,MOUNTPOINTS,DRIVER"]) else { return Vec::new() };
    let mut drives = Vec::new();
    // Each object holds "key": value pairs; children are objects too
    for obj in out.split('{').skip(1) {
        let obj = obj.split('}').next().unwrap_or("");
        let get = |k: &str| -> String {
            let pat = format!("\"{k}\":");
            let Some(i) = obj.find(&pat) else { return String::new() };
            let v = obj[i + pat.len()..].trim_start();
            if let Some(v) = v.strip_prefix('"') {
                v.split('"').next().unwrap_or("").to_string()
            } else if let Some(v) = v.strip_prefix('[') {
                v.split(']').next().unwrap_or("").replace('"', "").split(',').next().unwrap_or("").trim().to_string()
            } else {
                v.split([',', '\n', ']']).next().unwrap_or("").trim().to_string()
            }
        };
        let name = get("name");
        let fstype = get("fstype");
        if name.is_empty() || fstype.is_empty() || fstype == "null" {
            continue;
        }
        let mp = get("mountpoints");
        let rm = get("rm");
        drives.push(Drive {
            dev: name,
            label: { let l = get("label"); if l == "null" { String::new() } else { l } },
            fstype,
            size: get("size").parse().unwrap_or(0),
            // USB sticks do not all say they are removable: their driver does
            removable: rm == "true" || rm == "1" || get("driver") == "usb-storage",
            mount: if mp.is_empty() || mp == "null" { None } else { Some(PathBuf::from(mp)) },
        });
    }
    drives
}

impl Files {
    fn new(start: PathBuf) -> Files {
        let mut f = Files {
            cwd: start,
            back: Vec::new(),
            entries: Vec::new(),
            rows: Vec::new(),
            selected: None,
            hidden: false,
            clip: None,
            status: String::new(),
            prompt: None,
            dialog: None,
            path_text: String::new(),
            places: Vec::new(),
            place_sel: None,
            drives: Vec::new(),
            scanned: None,
            scanner: Default::default(),
            scanning: false,
            users: sys::id_names("/etc/passwd"),
            groups: sys::id_names("/etc/group"),
        };
        f.load();
        f
    }

    fn load(&mut self) {
        let keep = self.selected.and_then(|i| self.entries.get(i)).map(|e| e.name.clone());
        self.entries.clear();
        match std::fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !self.hidden && name.starts_with('.') {
                        continue;
                    }
                    let path = e.path();
                    let lmd = std::fs::symlink_metadata(&path).ok();
                    let md = std::fs::metadata(&path).ok().or(lmd.clone());
                    let Some(md) = md else { continue };
                    self.entries.push(Entry {
                        name,
                        path,
                        dir: md.is_dir(),
                        link: lmd.map(|m| m.file_type().is_symlink()).unwrap_or(false),
                        size: md.len(),
                        mtime: md.mtime(),
                        mode: md.mode(),
                    });
                }
            }
            Err(e) => self.status = format!("{}: {e}", self.cwd.display()),
        }
        self.entries.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        self.rows = self
            .entries
            .iter()
            .map(|e| Row {
                icon: icon_for(e),
                cells: vec![
                    if e.link { format!("{} →", e.name) } else { e.name.clone() },
                    if e.dir { "—".to_string() } else { sys::human_size(e.size) },
                    sys::format_time(e.mtime),
                ],
            })
            .collect();
        self.selected = keep.and_then(|n| self.entries.iter().position(|e| e.name == n));
        self.path_text = self.cwd.display().to_string();
    }

    fn go(&mut self, p: PathBuf) {
        let p = if p.is_relative() { self.cwd.join(p) } else { p };
        match std::fs::read_dir(&p) {
            Ok(_) => {
                if p != self.cwd {
                    self.back.push(std::mem::replace(&mut self.cwd, p));
                }
                self.selected = None;
                self.status.clear();
                self.load();
                say(&format!("folder {}", self.cwd.display()));
            }
            Err(e) => {
                self.status = format!("{}: {e}", p.display());
                self.path_text = self.cwd.display().to_string();
            }
        }
    }

    fn up(&mut self) {
        if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
            let child = self.cwd.file_name().map(|s| s.to_string_lossy().into_owned());
            self.go(parent);
            self.selected = child.and_then(|n| self.entries.iter().position(|e| e.name == n));
        }
    }

    fn sel(&self) -> Option<&Entry> {
        self.selected.and_then(|i| self.entries.get(i))
    }

    fn open(&mut self, i: usize) {
        let Some(e) = self.entries.get(i).cloned() else { return };
        if e.dir {
            return self.go(e.path);
        }
        match sys::app_for(&e.path) {
            Some(app) => {
                let p = e.path.display().to_string();
                match sys::launch(&app.exec, &[&p], Some(&self.cwd)) {
                    Ok(()) => {
                        self.status = format!("Opened {} with {}", e.name, app.name);
                        say(&format!("open {} with {}", e.path.display(), app.exec));
                    }
                    Err(err) => self.status = format!("{}: {err}", app.exec),
                }
            }
            None => self.status = format!("No application opens {}", e.name),
        }
    }

    fn open_terminal(&mut self) {
        let dir = match self.sel() {
            Some(e) if e.dir => e.path.clone(),
            _ => self.cwd.clone(),
        };
        match sys::launch("/bin/terminal", &[], Some(&dir)) {
            Ok(()) => say(&format!("terminal in {}", dir.display())),
            Err(e) => self.status = format!("terminal: {e}"),
        }
    }

    fn copy(&mut self, cut: bool) {
        if let Some(e) = self.sel() {
            let p = e.path.clone();
            self.status = format!("{} {}", if cut { "Cut" } else { "Copied" }, e.name);
            say(&format!("{} {}", if cut { "cut" } else { "copy" }, p.display()));
            self.clip = Some((vec![p], cut));
        }
    }

    fn paste(&mut self) {
        let Some((paths, cut)) = self.clip.clone() else { return };
        let mut last = None;
        for p in &paths {
            let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let r = if cut {
                let to = self.cwd.join(&name);
                if to == *p {
                    Ok(to)
                } else {
                    let to = free_name(&self.cwd, &name);
                    move_path(p, &to).map(|_| to)
                }
            } else {
                let to = free_name(&self.cwd, &name);
                copy_tree(p, &to).map(|_| to)
            };
            match r {
                Ok(to) => {
                    say(&format!("{} {} -> {}", if cut { "moved" } else { "copied" }, p.display(), to.display()));
                    last = to.file_name().map(|s| s.to_string_lossy().into_owned());
                }
                Err(e) => {
                    self.status = format!("{}: {e}", p.display());
                    self.dialog = Some(Dialog::Info("Could not paste".into(), format!("{}: {e}", name)));
                }
            }
        }
        if cut {
            self.clip = None;
        }
        self.load();
        if let Some(n) = last {
            self.selected = self.entries.iter().position(|e| e.name == n);
            self.status = format!("Pasted {n}");
        }
    }

    /// To ~/.local/share/Trash (files/ and info/NAME.trashinfo, as
    /// freedesktop.org's trash), or deleted for good from another disk
    fn trash(&mut self, p: PathBuf) {
        let t = trash_dir();
        let _ = std::fs::create_dir_all(t.join("files"));
        let _ = std::fs::create_dir_all(t.join("info"));
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let to = free_name(&t.join("files"), &name);
        let tname = to.file_name().unwrap().to_string_lossy().into_owned();
        match std::fs::rename(&p, &to) {
            Ok(()) => {
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
                let _ = std::fs::write(t.join("info").join(format!("{tname}.trashinfo")), info);
                self.status = format!("Moved {name} to the trash");
                say(&format!("trash {}", p.display()));
            }
            Err(_) => {
                // Another filesystem (a USB stick): ask to delete instead
                self.dialog = Some(Dialog::Delete(vec![p]));
                return;
            }
        }
        self.load();
    }

    fn restore(&mut self, p: PathBuf) {
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let info = trash_dir().join("info").join(format!("{name}.trashinfo"));
        let orig = std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Path=").map(PathBuf::from)));
        let Some(orig) = orig else {
            self.status = format!("{name}: where it came from is not known");
            return;
        };
        let dir = orig.parent().unwrap_or(Path::new("/")).to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        let to = free_name(&dir, &orig.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name.clone()));
        match move_path(&p, &to) {
            Ok(()) => {
                let _ = std::fs::remove_file(info);
                self.status = format!("Restored {}", to.display());
                say(&format!("restore {}", to.display()));
            }
            Err(e) => self.status = format!("{name}: {e}"),
        }
        self.load();
    }

    fn delete(&mut self, paths: &[PathBuf]) {
        for p in paths {
            match remove_tree(p) {
                Ok(()) => {
                    if p.parent() == Some(&trash_dir().join("files")) {
                        let n = p.file_name().unwrap().to_string_lossy().into_owned();
                        let _ = std::fs::remove_file(trash_dir().join("info").join(format!("{n}.trashinfo")));
                    }
                    say(&format!("delete {}", p.display()));
                    self.status = format!("Deleted {}", p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                }
                Err(e) => self.status = format!("{}: {e}", p.display()),
            }
        }
        self.load();
    }

    fn empty_trash(&mut self) {
        for sub in ["files", "info"] {
            let d = trash_dir().join(sub);
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let _ = remove_tree(&e.path());
                }
            }
        }
        say("empty trash");
        self.status = "The trash is empty".into();
        self.load();
    }

    fn finish_prompt(&mut self) {
        let Some((kind, text)) = self.prompt.take() else { return };
        let name = text.trim().to_string();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.status = "Not a valid name".into();
            return;
        }
        match kind {
            Prompt::Rename(from) => {
                let to = from.parent().unwrap_or(&self.cwd).join(&name);
                if to == from {
                    return;
                }
                if to.exists() {
                    self.status = format!("{name} already exists");
                    return;
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        say(&format!("rename {} -> {}", from.display(), to.display()));
                        self.load();
                        self.selected = self.entries.iter().position(|e| e.name == name);
                        self.status = format!("Renamed to {name}");
                    }
                    Err(e) => self.status = format!("{name}: {e}"),
                }
            }
            Prompt::NewFolder => {
                let p = self.cwd.join(&name);
                match std::fs::create_dir(&p) {
                    Ok(()) => {
                        say(&format!("new folder {}", p.display()));
                        self.load();
                        self.selected = self.entries.iter().position(|e| e.name == name);
                        self.status = format!("Made {name}");
                    }
                    Err(e) => self.status = format!("{name}: {e}"),
                }
            }
        }
    }

    // ---- disks ----

    fn rescan(&mut self) {
        self.set_drives(scan_drives());
    }

    fn set_drives(&mut self, drives: Vec<Drive>) {
        self.drives = drives;
        self.scanned = Some(Instant::now());
        let mut places = vec![
            ("Home".to_string(), sys::home(), Icon::Folder),
            ("Computer".to_string(), PathBuf::from("/"), Icon::Drive),
            ("Trash".to_string(), trash_dir().join("files"), Icon::Trash),
        ];
        for d in &self.drives {
            // The system's own disks stay out of the list (they are under /)
            if let Some(m) = &d.mount {
                if m == Path::new("/") || m.starts_with("/boot") {
                    continue;
                }
            } else if !d.removable {
                continue;
            }
            let name = if d.label.is_empty() { format!("{} ({}, {})", d.dev, sys::human_size(d.size), d.fstype) } else { d.label.clone() };
            places.push((name, d.mount.clone().unwrap_or_else(|| PathBuf::from(format!("dev:{}", d.dev))), Icon::Drive));
        }
        self.places = places;
    }

    fn drive_for(&self, p: &Path) -> Option<Drive> {
        if let Some(dev) = p.to_str().and_then(|s| s.strip_prefix("dev:")) {
            return self.drives.iter().find(|d| d.dev == dev).cloned();
        }
        self.drives.iter().find(|d| d.mount.as_deref() == Some(p)).cloned()
    }

    fn mount(&mut self, d: &Drive) {
        let base = if d.label.is_empty() { d.dev.clone() } else { d.label.replace('/', "_") };
        let dir = PathBuf::from("/media").join(&base);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.dialog = Some(Dialog::Info("Could not mount".into(), format!("{}: {e}", dir.display())));
            return;
        }
        let ds = dir.display().to_string();
        match sys::run_detached("/bin/mount", &[&d.dev, &ds]) {
            Ok(_) => {
                say(&format!("mount {} {}", d.dev, ds));
                self.status = format!("Mounted {} at {}", d.dev, ds);
                self.rescan();
                self.go(dir);
            }
            Err(e) => self.dialog = Some(Dialog::Info("Could not mount".into(), e)),
        }
    }

    fn eject(&mut self, d: &Drive) {
        let Some(m) = d.mount.clone() else { return };
        if self.cwd.starts_with(&m) {
            self.go(sys::home());
        }
        match sys::run_detached("/bin/umount", &[&m.display().to_string()]) {
            Ok(_) => {
                say(&format!("eject {}", d.dev));
                let _ = std::fs::remove_dir(&m);
                self.dialog = Some(Dialog::Info("Ejected".into(), format!("{} can be removed safely.", if d.label.is_empty() { &d.dev } else { &d.label })));
            }
            Err(e) => self.dialog = Some(Dialog::Info("Could not eject".into(), e)),
        }
        self.rescan();
    }

    fn properties(&self, p: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(md) = std::fs::symlink_metadata(p) else { return out };
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
        out.push(("Name".into(), name));
        out.push(("Where".into(), p.parent().map(|d| d.display().to_string()).unwrap_or_default()));
        let kind = if md.file_type().is_symlink() {
            format!("link to {}", std::fs::read_link(p).map(|t| t.display().to_string()).unwrap_or_default())
        } else if md.is_dir() {
            "folder".into()
        } else {
            match sys::app_for(p) {
                Some(a) => format!("file (opens with {})", a.name),
                None => "file".into(),
            }
        };
        out.push(("Kind".into(), kind));
        if md.is_dir() {
            let (s, n) = total_size(p);
            out.push(("Size".into(), format!("{} in {} files", sys::human_size(s), n)));
        } else {
            out.push(("Size".into(), format!("{} ({} bytes)", sys::human_size(md.len()), md.len())));
        }
        out.push(("Modified".into(), sys::format_time(md.mtime())));
        out.push(("Accessed".into(), sys::format_time(md.atime())));
        out.push(("Changed".into(), sys::format_time(md.ctime())));
        let user = self.users.get(&md.uid()).cloned().unwrap_or_else(|| md.uid().to_string());
        let group = self.groups.get(&md.gid()).cloned().unwrap_or_else(|| md.gid().to_string());
        out.push(("Owner".into(), format!("{user}, group {group}")));
        out.push(("Permissions".into(), format!("{} ({:o})", sys::mode_string(md.permissions().mode()), md.permissions().mode() & 0o7777)));
        out
    }

    // ---- the window ----

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        // The disks, from the thread that runs lsblk (it reads the disks:
        // slow on USB, so not in the window's thread)
        let fresh = self.scanner.lock().unwrap().take();
        if let Some(d) = fresh {
            self.set_drives(d);
        }
        if self.scanned.is_none() {
            self.rescan();
        }
        if !self.scanning {
            self.scanning = true;
            let slot = self.scanner.clone();
            let waker = ctl.waker;
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3));
                let d = scan_drives();
                *slot.lock().unwrap() = Some(d);
                if let Some(w) = waker {
                    w.wake();
                }
            });
        }
        let in_trash = is_trash(&self.cwd);
        // The window's shortcuts work wherever the focus is, except while
        // a name or the path is typed
        if self.prompt.is_none() && self.dialog.is_none() && !ui.has_focus("path") {
            let keys = std::mem::take(&mut ui.input.keys);
            let mut rest = Vec::new();
            for k in keys {
                if Self::shortcut(&k) {
                    self.key(&k, in_trash);
                } else {
                    rest.push(k);
                }
            }
            ui.input.keys = rest;
        }
        let menus: [(&str, &[&str]); 4] = [
            ("File", &["Open", "Open in Terminal", "New folder", "Properties", "Set as wallpaper", "Close"]),
            ("Edit", &["Copy", "Cut", "Paste", "Rename", "Move to the trash", "Delete"]),
            ("View", &["Show hidden files", "Reload"]),
            ("Go", &["Back", "Up", "Home", "Trash"]),
        ];
        if let Some((m, i)) = ui.menu_bar(&menus) {
            say(&format!("menu {} > {}", menus[m].0, menus[m].1[i]));
            match (m, i) {
                (0, 0) => {
                    if let Some(s) = self.selected {
                        self.open(s)
                    }
                }
                (0, 1) => self.open_terminal(),
                (0, 2) => self.prompt = Some((Prompt::NewFolder, "New folder".into())),
                (0, 3) => {
                    let p = self.sel().map(|e| e.path.clone()).unwrap_or(self.cwd.clone());
                    self.dialog = Some(Dialog::Properties(p));
                }
                (0, 4) => {
                    // Settings makes the picture the wallpaper (and says
                    // so in a notification if it is not a picture)
                    if let Some(e) = self.sel() {
                        let p = e.path.clone();
                        say(&format!("wallpaper {}", p.display()));
                        let _ = std::process::Command::new("/bin/settings").arg("--wallpaper").arg(&p).spawn();
                    }
                }
                (0, 5) => ctl.quit = true,
                (1, 0) => self.copy(false),
                (1, 1) => self.copy(true),
                (1, 2) => self.paste(),
                (1, 3) => {
                    if let Some(e) = self.sel() {
                        self.prompt = Some((Prompt::Rename(e.path.clone()), e.name.clone()));
                    }
                }
                (1, 4) => {
                    if let Some(e) = self.sel() {
                        let p = e.path.clone();
                        if in_trash {
                            self.dialog = Some(Dialog::Delete(vec![p]));
                        } else {
                            self.trash(p);
                        }
                    }
                }
                (1, 5) => {
                    if let Some(e) = self.sel() {
                        self.dialog = Some(Dialog::Delete(vec![e.path.clone()]));
                    }
                }
                (2, 0) => {
                    self.hidden = !self.hidden;
                    self.load();
                }
                (2, 1) => self.load(),
                (3, 0) => {
                    if let Some(p) = self.back.pop() {
                        self.go(p);
                        self.back.pop();
                    }
                }
                (3, 1) => self.up(),
                (3, 2) => self.go(sys::home()),
                (3, 3) => self.go(trash_dir().join("files")),
                _ => {}
            }
        }
        // The tool bar: back, up, home, the path
        let path_enter = ui.row(|ui| {
            if ui.button("Back") {
                if let Some(p) = self.back.pop() {
                    self.go(p);
                    self.back.pop();
                }
            }
            if ui.button("Up") {
                self.up();
            }
            if in_trash {
                if ui.button("Restore") {
                    if let Some(e) = self.sel() {
                        let p = e.path.clone();
                        self.restore(p);
                    }
                }
                if ui.button("Empty trash") {
                    self.dialog = Some(Dialog::EmptyTrash);
                }
            } else if ui.button("New folder") {
                self.prompt = Some((Prompt::NewFolder, "New folder".into()));
            }
            ui.text_field("path", &mut self.path_text, 0.0).1
        });
        if path_enter {
            let p = PathBuf::from(self.path_text.trim());
            self.go(p);
            ui.set_focus("table");
        }
        let w = ui.available_width();
        let mut table_keys = Vec::new();
        let mut activated = None;
        ui.row(|ui| {
            ui.column(ui.u(180.0).min(w * 0.3), |ui| {
                let names: Vec<String> = self.places.iter().map(|p| p.0.clone()).collect();
                let cur = self.places.iter().position(|p| p.1 == self.cwd);
                self.place_sel = cur;
                let h = (ui.remaining_height() - ui.line_h() * 4.0) / ui.scale;
                if ui.list("places", &names, &mut self.place_sel, h.max(60.0)) {
                    if let Some(i) = self.place_sel {
                        let p = self.places[i].1.clone();
                        if let Some(d) = self.drive_for(&p).filter(|d| d.mount.is_none()) {
                            self.mount(&d);
                        } else {
                            self.go(p);
                        }
                        ui.set_focus("table");
                    }
                }
                // Eject the disk shown
                if let Some(d) = self.drives.iter().find(|d| d.mount.as_ref().map(|m| self.cwd.starts_with(m) && m != Path::new("/")).unwrap_or(false)).cloned() {
                    if ui.button("Eject") {
                        self.eject(&d);
                    }
                }
            });
            ui.column(0.0, |ui| {
                let cols = [
                    Column { title: "Name", width: 0.0, right: false },
                    Column { title: "Size", width: 90.0, right: true },
                    Column { title: "Modified", width: 150.0, right: false },
                ];
                let h = (ui.remaining_height() - ui.line_h() * 2.5) / ui.scale;
                let r = ui.table("table", &cols, &self.rows, &mut self.selected, h.max(60.0));
                table_keys = r.keys;
                activated = r.activated;
            });
        });
        ui.focus_if_none("table");
        if let Some(i) = activated {
            self.open(i);
        }
        for k in table_keys {
            self.key(&k, in_trash);
        }
        // The bottom line: a name being typed, or the status
        if self.prompt.is_some() {
            let label = match &self.prompt {
                Some((Prompt::Rename(_), _)) => "New name:",
                _ => "Folder name:",
            };
            let mut text = std::mem::take(&mut self.prompt.as_mut().unwrap().1);
            let (ok, cancel, enter) = ui.row(|ui| {
                ui.label(label);
                let (_, enter) = ui.text_field("name", &mut text, 300.0);
                (ui.primary_button("OK"), ui.button("Cancel"), enter)
            });
            if let Some(p) = self.prompt.as_mut() {
                p.1 = text;
            }
            if !ui.has_focus("name") && !cancel {
                ui.set_focus("name");
            }
            if ok || enter {
                self.finish_prompt();
                ui.set_focus("table");
            } else if cancel || ui.input.keys.iter().any(|k| k.usage == 0x29) {
                self.prompt = None;
                ui.set_focus("table");
            }
        } else {
            let st = if self.status.is_empty() {
                let n = self.entries.len();
                format!("{} {}{}", n, if n == 1 { "item" } else { "items" }, match &self.clip {
                    Some((p, cut)) => format!(" · {} {} to paste", p.len(), if *cut { "cut" } else { "copied" }),
                    None => String::new(),
                })
            } else {
                self.status.clone()
            };
            ui.dim_label(&st);
        }
        self.dialogs(ui);
    }

    fn shortcut(k: &rustica_ui::Key) -> bool {
        let alt = k.mods & MOD_ALT != 0;
        match k.usage {
            0x2A | 0x3B | 0x3E | 0x4C => !k.ctrl() && !alt || k.usage == 0x4C,
            0x52 | 0x50 | 0x28 => alt,
            0x06 | 0x1B | 0x19 | 0x0B => k.ctrl(),
            0x11 => k.ctrl() && k.shift(),
            _ => false,
        }
    }

    fn key(&mut self, k: &rustica_ui::Key, in_trash: bool) {
        let alt = k.mods & MOD_ALT != 0;
        match k.usage {
            0x2A => self.up(),                                    // Backspace
            0x52 if alt => self.up(),                             // Alt+Up
            0x50 if alt => {
                if let Some(p) = self.back.pop() {
                    self.go(p);
                    self.back.pop();
                }
            }
            0x28 if alt => {
                let p = self.sel().map(|e| e.path.clone()).unwrap_or(self.cwd.clone());
                self.dialog = Some(Dialog::Properties(p));
            }
            0x3B => {
                // F2
                if let Some(e) = self.sel() {
                    self.prompt = Some((Prompt::Rename(e.path.clone()), e.name.clone()));
                }
            }
            0x3E => self.load(),                                  // F5
            0x4C => {
                // Delete, Shift+Delete
                if let Some(e) = self.sel() {
                    let p = e.path.clone();
                    if k.shift() || in_trash {
                        self.dialog = Some(Dialog::Delete(vec![p]));
                    } else {
                        self.trash(p);
                    }
                }
            }
            0x06 if k.ctrl() => self.copy(false),
            0x1B if k.ctrl() => self.copy(true),
            0x19 if k.ctrl() => self.paste(),
            0x11 if k.ctrl() && k.shift() => self.prompt = Some((Prompt::NewFolder, "New folder".into())),
            0x0B if k.ctrl() => {
                self.hidden = !self.hidden;
                self.load();
            }
            _ => {}
        }
    }

    fn dialogs(&mut self, ui: &mut Ui) {
        let Some(d) = self.dialog.take() else { return };
        let keep = match &d {
            Dialog::Delete(paths) => {
                let name = paths.first().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match ui.dialog("Delete for good?", &format!("{name} will be deleted and cannot be restored."), &["Delete", "Cancel"]) {
                    Some(0) => {
                        self.delete(paths);
                        false
                    }
                    Some(_) => false,
                    None => true,
                }
            }
            Dialog::EmptyTrash => match ui.dialog("Empty the trash?", "Everything in the trash will be deleted for good.", &["Empty", "Cancel"]) {
                Some(0) => {
                    self.empty_trash();
                    false
                }
                Some(_) => false,
                None => true,
            },
            Dialog::Info(title, msg) => ui.dialog(title, msg, &["OK"]).is_none(),
            Dialog::Properties(p) => {
                let msg = self.properties(p).iter().map(|(k, v)| format!("{k}:  {v}")).collect::<Vec<_>>().join("\n");
                ui.dialog("Properties", &msg, &["Close"]).is_none()
            }
        };
        if keep {
            self.dialog = Some(d);
        }
    }
}

fn main() {
    let start = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| sys::home()));
    let mut f = Files::new(start);
    say(&format!("started in {}", f.cwd.display()));
    let r = rustica_ui::run("Files", 860, 560, None, |ui, ctl| {
        let title = format!("{} — Files", f.cwd.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()));
        ctl.requests.push(rustica_ui::Request::Title(title));
        f.frame(ui, ctl)
    });
    if let Err(e) = r {
        eprintln!("files: {e}");
        std::process::exit(1);
    }
}

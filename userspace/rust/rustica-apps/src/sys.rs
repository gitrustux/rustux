// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What the applications share: the desktop's application list, sizes
//! and times for people, users and groups, settings files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// An application, from /usr/share/applications/NAME.app:
///
/// ```text
/// name=Files
/// exec=/bin/files
/// comment=Browse, copy and move files
/// types=txt,md,conf
/// ```
///
/// or an installed app (FoxDen) whose /apps/ID/manifest says which files
/// it opens (`Types: txt md`, FoxDen step FD6b): its exec is `appctl open
/// ID`, which gives it the file in its sandbox.
#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub exec: String,
    pub comment: String,
    /// File name extensions it opens (lower case)
    pub types: Vec<String>,
    /// An installed app's ID
    pub app_id: Option<String>,
}

pub fn applications() -> Vec<App> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/usr/share/applications") else { return out };
    for e in dir.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("app") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let kv = parse_kv(&text);
        let Some(exec) = kv.get("exec") else { continue };
        out.push(App {
            name: kv.get("name").cloned().unwrap_or_else(|| exec.clone()),
            exec: exec.clone(),
            comment: kv.get("comment").cloned().unwrap_or_default(),
            types: kv.get("types").map(|t| t.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
            app_id: None,
        });
    }
    out.extend(installed_apps());
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The installed apps that open files (a Types: line in /apps/ID/manifest)
fn installed_apps() -> Vec<App> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/apps") else { return out };
    for e in dir.flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let Ok(text) = std::fs::read_to_string(e.path().join("manifest")) else { continue };
        let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(':')).map(|v| v.trim().to_string()));
        if field("Id").as_deref() != Some(id.as_str()) {
            continue;
        }
        let types: Vec<String> = field("Types").unwrap_or_default().split([' ', ',']).map(|s| s.trim().trim_start_matches('.').to_lowercase()).filter(|s| !s.is_empty()).collect();
        if types.is_empty() {
            continue;
        }
        out.push(App {
            name: field("Name").unwrap_or_else(|| id.clone()),
            exec: format!("/bin/appctl open {id}"),
            comment: field("Summary").unwrap_or_default(),
            types,
            app_id: Some(id),
        });
    }
    out
}

/// The application that opens files with this name, if any
pub fn app_for(path: &Path) -> Option<App> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    applications().into_iter().find(|a| a.types.iter().any(|t| *t == ext))
}

/// Start an application's exec line with extra arguments
pub fn launch(exec: &str, args: &[&str], dir: Option<&Path>) -> std::io::Result<()> {
    let mut words = exec.split_whitespace();
    let prog = words.next().unwrap_or("");
    let mut c = Command::new(prog);
    c.args(words).args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    c.spawn().map(|_| ())
}

/// key=value lines (# comments)
pub fn parse_kv(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    m
}

/// Write key=value lines, keeping the other lines of the file
pub fn write_kv(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    let mut done = false;
    for line in old.lines() {
        if line.split_once('=').map(|(k, _)| k.trim() == key).unwrap_or(false) {
            if !done {
                out.push_str(&format!("{key}={value}\n"));
                done = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !done {
        out.push_str(&format!("{key}={value}\n"));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, out)
}

/// The user's settings directory, ~/.config
pub fn config_dir() -> PathBuf {
    home().join(".config")
}

pub fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/root"))
}

/// 1536 -> "1.5 KB"
pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} {}", if n == 1 { "byte" } else { "bytes" });
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", U[i])
    } else {
        format!("{v:.0} {}", U[i])
    }
}

#[repr(C)]
pub struct Tm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
    pub gmtoff: i64,
    pub zone: *const u8,
}

extern "C" {
    fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    fn time(t: *mut i64) -> i64;
    fn tzset();
}

/// Seconds since 1970 in local time
pub fn local(t: i64) -> Tm {
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    unsafe {
        tzset();
        localtime_r(&t, &mut tm);
    }
    tm
}

pub fn now() -> i64 {
    unsafe { time(std::ptr::null_mut()) }
}

/// "2026-11-15 09:33"
pub fn format_time(t: i64) -> String {
    let tm = local(t);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.year + 1900, tm.mon + 1, tm.mday, tm.hour, tm.min)
}

/// Names of users and groups by id (/etc/passwd, /etc/group)
pub fn id_names(file: &str) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() > 2 {
            if let Ok(id) = f[2].parse() {
                m.insert(id, f[0].to_string());
            }
        }
    }
    m
}

/// "rwxr-xr-x"
pub fn mode_string(mode: u32) -> String {
    let mut s = String::new();
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Run a command; its output, or its error output as the error
pub fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    match Command::new(prog).args(args).output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Err(if !e.is_empty() { e } else if !out.is_empty() { out } else { format!("{prog} failed") })
        }
        Err(e) => Err(format!("{prog}: {e}")),
    }
}

/// Run a command that may leave a program running with its output (mount
/// starts the filesystem's server): its error output goes to a file, so
/// nothing waits for the pipes to close
pub fn run_detached(prog: &str, args: &[&str]) -> Result<(), String> {
    let errf = std::env::temp_dir().join(format!("rustica-{}-{}.err", std::process::id(), now()));
    let file = std::fs::File::create(&errf).map_err(|e| format!("{}: {e}", errf.display()))?;
    let r = Command::new(prog).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(file).status();
    let msg = std::fs::read_to_string(&errf).unwrap_or_default().trim().to_string();
    let _ = std::fs::remove_file(&errf);
    match r {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(if msg.is_empty() { format!("{prog} failed") } else { msg }),
        Err(e) => Err(format!("{prog}: {e}")),
    }
}

extern "C" {
    fn geteuid() -> u32;
}

pub fn euid() -> u32 {
    unsafe { geteuid() }
}

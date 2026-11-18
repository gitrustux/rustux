// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The system images and rboot's entries (step E3). The system is
//! image-based: each `rustica-system-VERSION` package puts
//! /boot/efi/EFI/Rustica/rustux-VERSION.efi (the kernel with its
//! read-only /bin and /usr) on the EFI partition, and `rpg boot update`
//! (their postinst and postrm run it) writes rboot's entries for them:
//!
//!   rustica.conf            the newest image ("Rustica Server", or the
//!                           text console beside the desktop edition's)
//!   rustica-desktop.conf    the desktop edition: the newest image with
//!                           `desktop`, the default
//!   rustica-VERSION.conf    each older image, "(previous)"
//!
//! The command line comes from /etc/rustica/boot.conf (`options
//! root=UUID=... quiet`, written by the installer), the edition from
//! /var/lib/rustica/edition (`server` or `desktop`, the edition
//! packages'). Entries rpg wrote say so on their first line; rboot adds
//! each one's recovery entry itself (docs/BOOT.md).
//!
//! Boot counting (step E4), as systemd-boot's: a new image's entries are
//! written as `ID+3.conf`, three tries. rboot lowers the count before each
//! try (`ID+2-1.conf`); a boot that reaches its target renames the entry to
//! `ID.conf` (`bootctl bless`, init's boot-complete unit): the image is
//! good. At `+0` it failed to start, and rboot boots the previous image
//! (with `panic=10` on every command line, a kernel that panics restarts by
//! itself). Each image's state goes with it from entry to entry. The
//! running image is the command line's `BOOT_IMAGE=` (rboot adds it).

use crate::db::{self, Db};
use crate::version;
use std::path::PathBuf;

const MARK: &str = "# Written by rpg boot update: changes here are lost (man rpg)";
/// Tries a new image gets
const TRIES: u32 = 3;

/// What rboot's entries say about an image
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// Started and reached its target (an entry without a counter)
    Good,
    /// On trial: tries left and done
    Trying(u32, u32),
    /// Failed to start: the tries it had
    Failed(u32),
    /// No entry of rpg's for it yet
    New,
}

/// `ID+LEFT[-DONE]`
pub fn split_counter(stem: &str) -> (&str, Option<(u32, u32)>) {
    if let Some(k) = stem.rfind('+') {
        let rest = &stem[k + 1..];
        let (l, d) = rest.split_once('-').unwrap_or((rest, "0"));
        let num = |t: &str| if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) { t.parse::<u32>().ok() } else { None };
        if let (Some(l), Some(d)) = (num(l), num(d)) {
            return (&stem[..k], Some((l, d)));
        }
    }
    (stem, None)
}

/// The Rustica entries there are: (file name, id, counter, efi, rpg's)
fn existing(db: &Db) -> Vec<(String, String, Option<(u32, u32)>, String, bool)> {
    let dir = esp(db).join("loader/entries");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let Some(stem) = n.strip_suffix(".conf") else { continue };
        let (id, c) = split_counter(stem);
        if !id.starts_with("rustica") {
            continue;
        }
        let t = std::fs::read_to_string(e.path()).unwrap_or_default();
        let efi = t.lines().find_map(|l| l.trim().strip_prefix("efi")).map(|v| v.trim().replace('\\', "/")).unwrap_or_default();
        out.push((n.clone(), id.to_string(), c, efi, t.starts_with(MARK)));
    }
    out.sort();
    out
}

/// An image's state from the entries that start it
fn state_of(entries: &[(String, String, Option<(u32, u32)>, String, bool)], efi: &str) -> State {
    let mine: Vec<_> = entries.iter().filter(|e| e.3.eq_ignore_ascii_case(efi)).collect();
    if mine.is_empty() {
        return State::New;
    }
    if mine.iter().any(|e| e.2.is_none()) {
        return State::Good;
    }
    if let Some(e) = mine.iter().find(|e| e.2.is_some_and(|c| c.0 == 0)) {
        return State::Failed(e.2.unwrap().1);
    }
    let (l, d) = mine.iter().filter_map(|e| e.2).min_by_key(|c| c.0).unwrap();
    State::Trying(l, d)
}

/// The image this system started from (BOOT_IMAGE= on the command line,
/// which rboot adds), as rboot names it: /EFI/Rustica/...
pub fn running(db: &Db) -> Option<String> {
    let t = std::fs::read_to_string(db.path("/run/cmdline")).ok()?;
    let p = t.split_whitespace().find_map(|w| w.strip_prefix("BOOT_IMAGE="))?;
    Some(p.replace('\\', "/"))
}

pub struct Image {
    /// None: the unversioned rustux.efi of an installation from before
    /// step E3
    pub version: Option<String>,
    pub efi: String, // as rboot names it: /EFI/Rustica/...
}

fn esp(db: &Db) -> PathBuf {
    db.path("/boot/efi")
}

/// The images on the EFI partition, newest first
pub fn images(db: &Db) -> Vec<Image> {
    let dir = esp(db).join("EFI/Rustica");
    let mut v: Vec<Image> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(ver) = n.strip_prefix("rustux-").and_then(|r| r.strip_suffix(".efi")) {
                if version::valid(ver) {
                    v.push(Image { version: Some(ver.to_string()), efi: format!("/EFI/Rustica/{n}") });
                }
            }
        }
    }
    v.sort_by(|a, b| version::compare(b.version.as_deref().unwrap_or("0"), a.version.as_deref().unwrap_or("0")));
    if dir.join("rustux.efi").is_file() {
        v.push(Image { version: None, efi: "/EFI/Rustica/rustux.efi".into() });
    }
    v
}

pub fn edition(db: &Db) -> String {
    std::fs::read_to_string(db.path("/var/lib/rustica/edition")).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "server".into())
}

/// The command line every entry gets
fn options(db: &Db) -> Result<String, String> {
    let p = db.path("/etc/rustica/boot.conf");
    if let Ok(t) = std::fs::read_to_string(&p) {
        for l in t.lines() {
            if let Some(o) = l.trim().strip_prefix("options") {
                return Ok(o.trim().to_string());
            }
        }
        return Err(format!("{}: no \"options ...\" line", p.display()));
    }
    // An installation from before step E3: its own entry's
    let old = esp(db).join("loader/entries/rustica.conf");
    if let Ok(t) = std::fs::read_to_string(&old) {
        if let Some(o) = t.lines().find_map(|l| l.trim().strip_prefix("options")) {
            return Ok(o.trim().to_string());
        }
    }
    Err(format!("no {} (options root=UUID=... quiet): rpg cannot write the boot entries", p.display()))
}

/// rpg boot update
pub fn update(db: &Db) -> Result<(), String> {
    let imgs = images(db);
    if imgs.is_empty() {
        println!("rpg boot: no system image on the EFI partition ({}/EFI/Rustica)", esp(db).display());
        return Ok(());
    }
    let opts = options(db)?;
    let ed = edition(db);
    let desktop = ed == "desktop";
    let with = |extra: &str| if extra.is_empty() { opts.clone() } else { format!("{opts} {extra}") };
    let have = existing(db);
    let mut entries: Vec<(String, String, String)> = Vec::new(); // id, efi, text
    let opts_for = |extra: &str| {
        let o = with(extra);
        // A kernel that panics restarts, so that rboot can try again or fall back
        if o.split(' ').any(|w| w.starts_with("panic=")) { o } else { format!("{o} panic=10") }
    };
    let entry = |title: &str, efi: &str, options: &str, key: u32| format!("{MARK}\ntitle   {title}\nefi     {efi}\noptions {options}\nsort-key {key}\n");
    let newest = &imgs[0];
    let label = |i: &Image| i.version.clone().unwrap_or_else(|| "(before packages)".into());
    if desktop {
        entries.push(("rustica-desktop".into(), newest.efi.clone(), entry("Rustica", &newest.efi, &opts_for("desktop"), 1)));
        entries.push(("rustica".into(), newest.efi.clone(), entry("Rustica (text console)", &newest.efi, &opts_for(""), 2)));
    } else {
        entries.push(("rustica".into(), newest.efi.clone(), entry("Rustica Server", &newest.efi, &opts_for(""), 1)));
    }
    for (i, img) in imgs.iter().enumerate().skip(1) {
        let id = match &img.version {
            Some(v) => format!("rustica-{v}"),
            None => "rustica-legacy".into(),
        };
        let title = format!("Rustica {} (previous)", label(img));
        entries.push((id, img.efi.clone(), entry(&title, &img.efi, &opts_for(if desktop { "desktop" } else { "" }), 10 + i as u32)));
    }
    let dir = esp(db).join("loader/entries");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut written: Vec<String> = Vec::new();
    for (id, efi, text) in &entries {
        // The same entry for the same image keeps its file (and counter);
        // else the image's state decides
        let name = match have.iter().find(|h| &h.1 == id && h.3.eq_ignore_ascii_case(efi)) {
            Some(h) => h.0.clone(),
            None => match state_of(&have, efi) {
                State::Good => format!("{id}.conf"),
                State::Trying(l, d) => format!("{id}+{l}-{d}.conf"),
                State::Failed(d) => format!("{id}+0-{d}.conf"),
                // A new image beside an older one is tried first; the
                // first one there is has nothing to fall back to
                State::New if efi == &newest.efi && imgs.len() > 1 => format!("{id}+{TRIES}.conf"),
                State::New => format!("{id}.conf"),
            },
        };
        db::write_atomic(&dir.join(&name), text.as_bytes(), 0o644)?;
        written.push(name);
    }
    // Entries rpg wrote before (and the installer's two) that are gone now
    for (name, id, _, _, ours) in &have {
        if written.contains(name) {
            continue;
        }
        if *ours || id == "rustica-desktop" || (id == "rustica" && !written.iter().any(|w| split_counter(w.trim_end_matches(".conf")).0 == "rustica")) {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    // The default
    let lc = esp(db).join("loader/loader.conf");
    let want = if desktop { "rustica-desktop" } else { "rustica" };
    let old = std::fs::read_to_string(&lc).unwrap_or_else(|_| "timeout 3\n".into());
    let mut lines: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("default")).map(str::to_string).collect();
    lines.insert(0, format!("default {want}"));
    db::write_atomic(&lc, (lines.join("\n") + "\n").as_bytes(), 0o644)?;
    let now = existing(db);
    println!(
        "rpg boot: {} {} (the {ed} edition{}){}{}",
        if desktop { "Rustica" } else { "Rustica Server" },
        label(newest),
        if desktop { ", the graphical login" } else { "" },
        match imgs.len() {
            1 => String::new(),
            n => format!("; {} previous image{} in the menu", n - 1, if n == 2 { "" } else { "s" }),
        },
        match state_of(&now, &newest.efi) {
            State::Trying(l, 0) => format!("; {} starts next, {l} tries before the previous one is used again", label(newest)),
            State::Trying(l, _) => format!("; {} on trial, {l} tries left", label(newest)),
            State::Failed(d) => format!("; {} failed to start ({d} tries): the previous image starts", label(newest)),
            _ => String::new(),
        }
    );
    Ok(())
}

/// rpg boot status: which image runs, and how each one did
pub fn status(db: &Db) -> Result<(), String> {
    let imgs = images(db);
    let have = existing(db);
    let run = running(db);
    let label = |i: &Image| i.version.clone().unwrap_or_else(|| "(before packages)".into());
    match run.as_deref().and_then(|r| imgs.iter().find(|i| i.efi.eq_ignore_ascii_case(r))) {
        Some(i) => println!("Running: {} ({})", label(i), i.efi),
        None => println!("Running: {}", run.as_deref().map(|r| format!("{r} (not one of the images here)")).unwrap_or_else(|| "unknown (not started by rboot 0.1 of package step E4)".into())),
    }
    if imgs.is_empty() {
        println!("No system image on the EFI partition ({}/EFI/Rustica).", esp(db).display());
        return Ok(());
    }
    for (k, img) in imgs.iter().enumerate() {
        let st = match state_of(&have, &img.efi) {
            State::Good => "good: it started".to_string(),
            State::Trying(l, 0) => format!("new: starts next ({l} tries)"),
            State::Trying(l, d) => format!("on trial: {d} tries so far, {l} left"),
            State::Failed(d) => format!("FAILED to start ({d} tries): not started by default"),
            State::New => "no boot entry (sudo rpg boot update)".to_string(),
        };
        println!("{}  {:24} {}", if k == 0 { "newest  " } else { "previous" }, label(img), st);
    }
    if let State::Failed(_) = state_of(&have, &imgs[0].efi) {
        println!("The newest image failed to start; this one runs instead. A newer image comes with rpg upgrade (rpg remove {} takes the failed one away).", imgs[0].version.as_deref().map(|v| format!("rustica-system-{v}")).unwrap_or_default());
    }
    Ok(())
}

/// The image packages beyond two (step E4): the newest and the running
/// one stay (the two newest when it is not known which runs)
pub fn extra_images(db: &Db, installed: &[crate::control::Control]) -> Vec<String> {
    let mut imgs: Vec<&crate::control::Control> = installed.iter().filter(|c| c.get("Section") == Some("images") && c.name().starts_with("rustica-system-")).collect();
    imgs.sort_by(|a, b| version::compare(b.version(), a.version()));
    if imgs.len() <= 2 {
        return Vec::new();
    }
    let run = running(db).and_then(|r| r.rsplit('/').next().map(str::to_string));
    let mut keep: Vec<String> = vec![imgs[0].name().to_string()];
    if let Some(r) = run {
        if let Some(c) = imgs.iter().find(|c| r.eq_ignore_ascii_case(&format!("rustux-{}.efi", c.version()))) {
            if !keep.contains(&c.name().to_string()) {
                keep.push(c.name().to_string());
            }
        }
    }
    for c in &imgs {
        if keep.len() >= 2 {
            break;
        }
        if !keep.contains(&c.name().to_string()) {
            keep.push(c.name().to_string());
        }
    }
    imgs.iter().map(|c| c.name().to_string()).filter(|n| !keep.contains(n)).collect()
}

/// rpg boot list
pub fn list(db: &Db) -> Result<(), String> {
    let imgs = images(db);
    if imgs.is_empty() {
        println!("No system image on the EFI partition ({}/EFI/Rustica).", esp(db).display());
        return Ok(());
    }
    println!("Edition: {}", edition(db));
    for (i, img) in imgs.iter().enumerate() {
        let size = std::fs::metadata(esp(db).join(img.efi.trim_start_matches('/'))).map(|m| m.len()).unwrap_or(0);
        println!(
            "{}  {:24} {}  ({} MiB)",
            if i == 0 { "newest  " } else { "previous" },
            img.version.as_deref().unwrap_or("(before packages)"),
            img.efi,
            size / 1048576
        );
    }
    if let Ok(t) = std::fs::read_to_string(db.path("/run/cmdline")) {
        println!("Running with: {}", t.trim());
    }
    Ok(())
}

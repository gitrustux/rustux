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

use crate::db::{self, Db};
use crate::version;
use std::path::PathBuf;

const MARK: &str = "# Written by rpg boot update: changes here are lost (man rpg)";

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
    let mut entries: Vec<(String, String)> = Vec::new(); // id, text
    let entry = |title: &str, efi: &str, options: &str, key: u32| format!("{MARK}\ntitle   {title}\nefi     {efi}\noptions {options}\nsort-key {key}\n");
    let newest = &imgs[0];
    let label = |i: &Image| i.version.clone().unwrap_or_else(|| "(before packages)".into());
    if desktop {
        entries.push(("rustica-desktop".into(), entry("Rustica", &newest.efi, &with("desktop"), 1)));
        entries.push(("rustica".into(), entry("Rustica (text console)", &newest.efi, &with(""), 2)));
    } else {
        entries.push(("rustica".into(), entry("Rustica Server", &newest.efi, &with(""), 1)));
    }
    for (i, img) in imgs.iter().enumerate().skip(1) {
        let id = match &img.version {
            Some(v) => format!("rustica-{v}"),
            None => "rustica-legacy".into(),
        };
        let title = format!("Rustica {} (previous)", label(img));
        entries.push((id, entry(&title, &img.efi, &with(if desktop { "desktop" } else { "" }), 10 + i as u32)));
    }
    let dir = esp(db).join("loader/entries");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (id, text) in &entries {
        db::write_atomic(&dir.join(format!("{id}.conf")), text.as_bytes(), 0o644)?;
    }
    // Entries rpg wrote before (and the installer's two) that are gone now
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let Some(id) = n.strip_suffix(".conf") else { continue };
            if entries.iter().any(|(i, _)| i == id) || !id.starts_with("rustica") {
                continue;
            }
            let ours = std::fs::read_to_string(e.path()).map(|t| t.starts_with(MARK)).unwrap_or(false);
            if ours || id == "rustica-desktop" {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // The default
    let lc = esp(db).join("loader/loader.conf");
    let want = if desktop { "rustica-desktop" } else { "rustica" };
    let old = std::fs::read_to_string(&lc).unwrap_or_else(|_| "timeout 3\n".into());
    let mut lines: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("default")).map(str::to_string).collect();
    lines.insert(0, format!("default {want}"));
    db::write_atomic(&lc, (lines.join("\n") + "\n").as_bytes(), 0o644)?;
    println!(
        "rpg boot: {} {} (the {ed} edition{}){}",
        if desktop { "Rustica" } else { "Rustica Server" },
        label(newest),
        if desktop { ", the graphical login" } else { "" },
        match imgs.len() {
            1 => String::new(),
            n => format!("; {} previous image{} in the menu", n - 1, if n == 2 { "" } else { "s" }),
        }
    );
    Ok(())
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

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! App packages (FoxDen step FD1, FOXDEN.md): `Kind: app` in the control
//! file. An app is kept apart from the system: everything it has is in
//! /apps/ID/ (ID its package name, reverse DNS: org.rustica.calculator),
//! described by /apps/ID/manifest, in the control file's format:
//!
//!   Id: org.rustica.example        the package's name
//!   Name: Example                  what the launcher shows
//!   Summary: an example app        one line
//!   Exec: bin/example ARG...       relative to /apps/ID
//!   Icon: icon.png                 optional, relative to /apps/ID
//!   Categories: Utility Graphics   optional
//!   Permissions: network files:documents   optional (FD2 applies them)
//!   Platform: 1                    the platform level it is made for
//!
//! `rpg build DIR` makes one from DIR/RPG/control (Kind: app) and the
//! rest of DIR as the app's folder. rpg refuses an app with anything
//! outside its folder, scripts (they would run as root), set-user or
//! set-group bits, links that leave its folder, relations to other
//! packages (apps are self-contained), or a platform level this system
//! does not have. The users' data for it is in ~/.local/share/apps/ID,
//! ~/.config/apps/ID and ~/.cache/apps/ID (`rpg remove --purge` removes
//! them).

use crate::control::Control;
use crate::pkg::{FType, Package};

/// The platform level this system offers (FOXDEN.md: the C library's and
/// the toolkit's interfaces, the services an app may use)
pub const PLATFORM: u32 = 1;

/// What an app may ask for (FOXDEN.md, "Permissions")
pub const PERMISSIONS: [&str; 12] = [
    "network",
    "files:documents",
    "files:pictures",
    "files:music",
    "files:videos",
    "files:downloads",
    "files:home",
    "removable",
    "audio",
    "camera",
    "location",
    "system",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub exec: String,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    pub permissions: Vec<String>,
    pub platform: u32,
}

/// The app's folder
pub fn dir(id: &str) -> String {
    format!("/apps/{id}")
}

/// Reverse DNS: two or more parts of a-z, 0-9 and -, each starting with
/// a letter or digit
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && id.len() <= 128
        && parts.iter().all(|p| !p.is_empty() && p.as_bytes()[0].is_ascii_alphanumeric() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
}

/// A path inside the app's folder, given relative to it: no "..", no
/// leading "/"
fn inside(rel: &str) -> bool {
    !rel.is_empty() && !rel.starts_with('/') && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let c = Control::parse(text).map_err(|e| format!("the manifest: {e}"))?;
    let need = |k: &str| c.get(k).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).ok_or(format!("the manifest has no {k}"));
    let words = |k: &str| c.get(k).map(|v| v.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    let platform = need("Platform")?;
    Ok(Manifest {
        id: need("Id")?,
        name: need("Name")?,
        summary: need("Summary")?,
        exec: need("Exec")?,
        icon: c.get("Icon").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
        categories: words("Categories"),
        permissions: words("Permissions"),
        platform: platform.parse().map_err(|_| format!("Platform {platform}: a number (this system has {PLATFORM})"))?,
    })
}

/// The checks an app package must pass (rpg build and rpg install)
pub fn check(p: &Package) -> Result<Manifest, String> {
    let id = p.control.name();
    if !valid_id(id) {
        return Err(format!("{id}: an app's name is its ID, reverse DNS (org.rustica.example)"));
    }
    for f in ["Depends", "Pre-Depends", "Conflicts", "Breaks", "Provides", "Replaces"] {
        if p.control.get(f).is_some() {
            return Err(format!("{f}: an app is self-contained (no relations to other packages)"));
        }
    }
    if p.postinst.is_some() || p.prerm.is_some() || p.postrm.is_some() {
        return Err("an app has no scripts (they would run as root)".into());
    }
    let root = dir(id);
    let mut has_root = false;
    for f in &p.files {
        if f.path == "/apps" && f.typ == FType::Dir {
            continue;
        }
        if f.path == root {
            has_root = f.typ == FType::Dir;
            continue;
        }
        let Some(rel) = f.path.strip_prefix(&format!("{root}/")) else {
            return Err(format!("{}: outside the app's folder {root}", f.path));
        };
        if f.mode & 0o6000 != 0 {
            return Err(format!("{}: set-user or set-group bits in an app", f.path));
        }
        if f.typ == FType::Link {
            // Relative and inside: from the link's folder
            let base: Vec<&str> = rel.split('/').collect();
            let mut at: Vec<&str> = base[..base.len() - 1].to_vec();
            if f.hash.starts_with('/') {
                return Err(format!("{} -> {}: a link that leaves the app's folder", f.path, f.hash));
            }
            for c in f.hash.split('/') {
                match c {
                    "" | "." => {}
                    ".." => {
                        if at.pop().is_none() {
                            return Err(format!("{} -> {}: a link that leaves the app's folder", f.path, f.hash));
                        }
                    }
                    c => at.push(c),
                }
            }
        }
    }
    if !has_root {
        return Err(format!("no {root} in the package"));
    }
    let text = p.data.get(&format!("{root}/manifest")).ok_or(format!("no {root}/manifest"))?;
    let m = parse_manifest(&String::from_utf8_lossy(text))?;
    if m.id != id {
        return Err(format!("the manifest's Id {} is not the package's name {id}", m.id));
    }
    if m.platform == 0 || m.platform > PLATFORM {
        return Err(format!("made for platform {}; this system has platform {PLATFORM} (a newer Rustica runs it)", m.platform));
    }
    let prog = m.exec.split_whitespace().next().unwrap_or("");
    let exe = p.files.iter().find(|f| inside(prog) && f.path == format!("{root}/{prog}") && f.typ == FType::File);
    match exe {
        Some(f) if f.mode & 0o111 != 0 => {}
        Some(_) => return Err(format!("Exec {prog}: not executable")),
        None => return Err(format!("Exec {prog}: not a file in the app's folder")),
    }
    if let Some(i) = &m.icon {
        if !inside(i) || !p.files.iter().any(|f| f.path == format!("{root}/{i}") && f.typ == FType::File) {
            return Err(format!("Icon {i}: not a file in the app's folder"));
        }
    }
    if let Some(bad) = m.permissions.iter().find(|x| !PERMISSIONS.contains(&x.as_str())) {
        return Err(format!("Permissions: {bad}? (known: {})", PERMISSIONS.join(" ")));
    }
    Ok(m)
}

/// The users' data folders for an app (for --purge), as absolute paths
pub fn data_dirs(homes: &[String], id: &str) -> Vec<String> {
    let mut v = Vec::new();
    for h in homes {
        for sub in [".local/share/apps", ".config/apps", ".cache/apps"] {
            v.push(format!("{}/{sub}/{id}", h.trim_end_matches('/')));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{self, Entry, Kind};
    use crate::pkg::{files_text, sha256_hex, FileEntry};

    const MANIFEST: &str = "Id: org.rustica.example\nName: Example\nSummary: an example\nExec: bin/example -a\nPermissions: network\nPlatform: 1\n";

    fn pkg(control: &str, files: Vec<(&str, FType, u32, Vec<u8>)>, script: bool) -> Result<Package, String> {
        let mut list = Vec::new();
        let mut entries = vec![Entry { path: "control".into(), mode: 0o644, kind: Kind::File(control.as_bytes().to_vec()) }];
        let mut data = Vec::new();
        for (p, t, mode, d) in &files {
            let hash = match t {
                FType::File => sha256_hex(d),
                FType::Link => String::from_utf8(d.clone()).unwrap(),
                FType::Dir => String::new(),
            };
            list.push(FileEntry { typ: *t, mode: *mode, uid: 0, gid: 0, hash: hash.clone(), conf: false, path: p.to_string() });
            let kind = match t {
                FType::File => Kind::File(d.clone()),
                FType::Link => Kind::Symlink(hash),
                FType::Dir => Kind::Dir,
            };
            data.push(Entry { path: format!("data{p}"), mode: *mode, kind });
        }
        entries.push(Entry { path: "files".into(), mode: 0o644, kind: Kind::File(files_text(&list).into_bytes()) });
        if script {
            entries.push(Entry { path: "scripts/postinst".into(), mode: 0o755, kind: Kind::File(b"true".to_vec()) });
        }
        entries.extend(data);
        Package::read(&archive::write(&entries))
    }

    const CONTROL: &str = "Package: org.rustica.example\nVersion: 1.0\nArchitecture: all\nKind: app\nDescription: an example\n";

    fn good(extra: Vec<(&'static str, FType, u32, Vec<u8>)>) -> Vec<(&'static str, FType, u32, Vec<u8>)> {
        let mut v = vec![
            ("/apps", FType::Dir, 0o755, vec![]),
            ("/apps/org.rustica.example", FType::Dir, 0o755, vec![]),
            ("/apps/org.rustica.example/bin", FType::Dir, 0o755, vec![]),
            ("/apps/org.rustica.example/bin/example", FType::File, 0o755, b"\x7fELF".to_vec()),
            ("/apps/org.rustica.example/manifest", FType::File, 0o644, MANIFEST.as_bytes().to_vec()),
        ];
        v.extend(extra);
        v
    }

    #[test]
    fn a_good_app() {
        let p = pkg(CONTROL, good(vec![("/apps/org.rustica.example/lib", FType::Link, 0o777, b"bin".to_vec())]), false).unwrap();
        let m = check(&p).unwrap();
        assert_eq!(m.name, "Example");
        assert_eq!(m.permissions, vec!["network"]);
    }

    #[test]
    fn refused() {
        let cases: Vec<(String, Vec<(&'static str, FType, u32, Vec<u8>)>, bool, &str)> = vec![
            (CONTROL.into(), good(vec![("/etc", FType::Dir, 0o755, vec![]), ("/etc/x", FType::File, 0o644, b"x".to_vec())]), false, "outside the app's folder"),
            (CONTROL.into(), good(vec![]), true, "no scripts"),
            (CONTROL.into(), good(vec![("/apps/org.rustica.example/su", FType::File, 0o4755, b"x".to_vec())]), false, "set-user"),
            (CONTROL.into(), good(vec![("/apps/org.rustica.example/l", FType::Link, 0o777, b"/etc/shadow".to_vec())]), false, "leaves"),
            (CONTROL.into(), good(vec![("/apps/org.rustica.example/bin/l", FType::Link, 0o777, b"../../x".to_vec())]), false, "leaves"),
            (CONTROL.replace("Kind: app\n", "Kind: app\nDepends: libc\n"), good(vec![]), false, "self-contained"),
            (CONTROL.replace("org.rustica.example", "example"), good(vec![]), false, "reverse DNS"),
        ];
        for (c, files, script, want) in cases {
            let e = pkg(&c, files, script).and_then(|p| check(&p).map(|_| ())).unwrap_err();
            assert!(e.contains(want), "{e:?} should say {want:?}");
        }
        // The manifest itself
        for (m, want) in [
            (MANIFEST.replace("Platform: 1", "Platform: 2"), "platform 2"),
            (MANIFEST.replace("Id: org.rustica.example", "Id: org.other.app"), "not the package's name"),
            (MANIFEST.replace("Exec: bin/example -a", "Exec: bin/nothere"), "not a file"),
            (MANIFEST.replace("Exec: bin/example -a", "Exec: ../../bin/shell"), "not a file"),
            (MANIFEST.replace("Permissions: network", "Permissions: root"), "Permissions: root?"),
        ] {
            let mut files = good(vec![]);
            files[4].3 = m.into_bytes();
            let e = check(&pkg(CONTROL, files, false).unwrap()).unwrap_err();
            assert!(e.contains(want), "{e:?} should say {want:?}");
        }
        let mut files = good(vec![]);
        files[3].2 = 0o644;
        assert!(check(&pkg(CONTROL, files, false).unwrap()).unwrap_err().contains("not executable"));
    }

    #[test]
    fn ids() {
        assert!(valid_id("org.rustica.calculator"));
        assert!(valid_id("com.example-co.app2"));
        assert!(!valid_id("calculator"));
        assert!(!valid_id("org..x"));
        assert!(!valid_id("org.Rustica.x"));
        assert!(!valid_id("org.rustica.x+y"));
    }
}

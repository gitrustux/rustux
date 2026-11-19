// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! For apps from outside developers (FoxDen step FD7a, FOXDEN.md): a
//! developer signs their app's folder (its source, manifest, control file
//! and pictures) with their own Ed25519 key; the store's checks verify it
//! against the key the developer registered, and that a package built
//! from the folder holds exactly the folder's files and its program.
//!
//!   rpg dev digest DIR             what is signed: a line per file
//!   rpg dev verify DIR KEY.pub     DIR/SIGNATURE made by that key over it
//!   rpg dev check-package FILE.rpg DIR   FILE is DIR built: the same
//!                                  control fields, DIR's files (not its
//!                                  src/, RPG/, SIGNATURE) and its program
//!
//! The digest:
//!
//!   rpg-source 1
//!   file PATH SHA256               every file under DIR but SIGNATURE,
//!                                  sorted by path
//!
//! Signed with openssl (`openssl pkeyutl -sign -inkey dev.key -rawin -in
//! DIGEST -out DIR/SIGNATURE`): the raw 64 bytes, or as base64.

use crate::app;
use crate::control::Control;
use crate::keys;
use crate::pkg::{sha256_hex, FType, Package};
use std::path::Path;

/// The folder's files (relative paths, sorted) with their contents.
/// Only plain files and folders: a link or anything else is refused, as
/// is a name with a space or a control character (the digest is lines)
fn walk(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn go(base: &Path, d: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let rd = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd {
            let e = e.map_err(|e| format!("{}: {e}", d.display()))?;
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
            if rel.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!("{rel}: a name with a space or a control character"));
            }
            let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{rel}: {e}"))?;
            if m.is_dir() {
                go(base, &p, out)?;
            } else if m.is_file() {
                out.push((rel, std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
            } else {
                return Err(format!("{rel}: not a file or a folder (no links)"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn digest(dir: &Path) -> Result<String, String> {
    let mut s = String::from("rpg-source 1\n");
    for (rel, data) in walk(dir)? {
        if rel != "SIGNATURE" {
            s += &format!("file {rel} {}\n", sha256_hex(&data));
        }
    }
    Ok(s)
}

/// The signature's 64 bytes: raw, or base64 text
fn signature(dir: &Path) -> Result<Vec<u8>, String> {
    let f = dir.join("SIGNATURE");
    let raw = std::fs::read(&f).map_err(|e| format!("{}: {e} (rpg dev digest, then openssl pkeyutl -sign: README)", f.display()))?;
    if raw.len() == 64 {
        return Ok(raw);
    }
    let text = String::from_utf8_lossy(&raw);
    keys::unbase64(text.trim()).filter(|b| b.len() == 64).ok_or_else(|| format!("{}: not a signature (64 bytes, or their base64)", f.display()))
}

/// DIR/SIGNATURE by the key in `pubfile`; the key's ID
pub fn verify(dir: &Path, pubfile: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(pubfile).map_err(|e| format!("{}: {e}", pubfile.display()))?;
    let (key, name) = keys::parse(&text).map_err(|e| format!("{}: {e}", pubfile.display()))?;
    let k = keys::Key { name: name.unwrap_or_default(), key, file: pubfile.to_path_buf() };
    let d = digest(dir)?;
    let sig = signature(dir)?;
    if keys::verify(std::slice::from_ref(&k), d.as_bytes(), &sig).is_none() {
        return Err(format!("{}: SIGNATURE is not by the key {} over these files (changed since it was signed?)", dir.display(), k.id()));
    }
    Ok(k.id())
}

/// The package file is the folder built: its control fields as
/// DIR/RPG/control's, its files DIR's (but src/, RPG/ and SIGNATURE) and
/// the program the manifest's Exec names, nothing else
pub fn check_package(file: &Path, dir: &Path) -> Result<app::Manifest, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let p = Package::read(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    let m = app::check(&p).map_err(|e| format!("{}: {e}", file.display()))?;
    let ctext = std::fs::read_to_string(dir.join("RPG/control")).map_err(|e| format!("{}/RPG/control: {e}", dir.display()))?;
    let want = Control::parse(&ctext).map_err(|e| format!("{}/RPG/control: {e}", dir.display()))?;
    for (k, v) in &want.fields {
        if p.control.get(k) != Some(v.as_str()) {
            return Err(format!("{}: {k} is {:?}, the folder's {v:?}", file.display(), p.control.get(k).unwrap_or("(none)")));
        }
    }
    let root = app::dir(&m.id);
    let prog = format!("{root}/{}", m.exec.split_whitespace().next().unwrap_or(""));
    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    for (rel, data) in walk(dir)? {
        if rel == "SIGNATURE" || rel.starts_with("src/") || rel.starts_with("RPG/") {
            continue;
        }
        expected.push((format!("{root}/{rel}"), data));
    }
    for (path, data) in &expected {
        match p.data.get(path) {
            Some(d) if d == data => {}
            Some(_) => return Err(format!("{}: {path} differs from the folder's", file.display())),
            None => return Err(format!("{}: {path} is missing", file.display())),
        }
    }
    for f in &p.files {
        if f.typ == FType::Dir || f.path == prog || expected.iter().any(|(e, _)| *e == f.path) {
            continue;
        }
        return Err(format!("{}: {} is not in the folder (only the program {prog} may be added)", file.display(), f.path));
    }
    if !p.files.iter().any(|f| f.path == prog) {
        return Err(format!("{}: no program {prog}", file.display()));
    }
    Ok(m)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["digest", dir] => {
            print!("{}", digest(Path::new(dir))?);
            Ok(())
        }
        ["verify", dir, key] => {
            let id = verify(Path::new(dir), Path::new(key))?;
            println!("{dir}: signed by {id}");
            Ok(())
        }
        ["check-package", file, dir] => {
            let m = check_package(Path::new(file), Path::new(dir))?;
            println!("{file}: {} as in {dir}{}", m.id, m.developer.map(|d| format!(" (developer {d})")).unwrap_or_default());
            Ok(())
        }
        _ => Err("dev digest DIR | dev verify DIR KEY.pub | dev check-package FILE.rpg DIR".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rpg-dev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn folder(d: &Path) {
        std::fs::create_dir_all(d.join("RPG")).unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("RPG/control"), "Package: io.github.alice.dice\nVersion: 1.0.0\nArchitecture: x86_64\nKind: app\nDescription: Dice\n").unwrap();
        std::fs::write(d.join("manifest"), "Id: io.github.alice.dice\nName: Dice\nSummary: dice\nExec: bin/app\nPlatform: 1\nDeveloper: io.github.alice\nDeveloper-Name: Alice\n").unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {}\n").unwrap();
    }

    #[test]
    fn sign_and_verify() {
        let d = tmp("sign");
        folder(&d);
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let pubf = d.with_extension("pub");
        std::fs::write(&pubf, format!("rpg-ed25519 {} alice\n", keys::hex(&sk.verifying_key().to_bytes()))).unwrap();
        let text = digest(&d).unwrap();
        assert!(text.starts_with("rpg-source 1\nfile RPG/control "));
        assert!(text.contains("\nfile src/main.rs "));
        std::fs::write(d.join("SIGNATURE"), sk.sign(text.as_bytes()).to_bytes()).unwrap();
        assert!(verify(&d, &pubf).is_ok());
        // The signature itself is not signed; any change is seen
        std::fs::write(d.join("src/main.rs"), "fn main() { loop {} }\n").unwrap();
        assert!(verify(&d, &pubf).is_err());
        // Another key
        let other = SigningKey::from_bytes(&[8u8; 32]);
        std::fs::write(d.join("SIGNATURE"), other.sign(digest(&d).unwrap().as_bytes()).to_bytes()).unwrap();
        assert!(verify(&d, &pubf).is_err());
        // base64 is taken too
        std::fs::write(d.join("SIGNATURE"), crate::dev::tests::b64(&sk.sign(digest(&d).unwrap().as_bytes()).to_bytes())).unwrap();
        assert!(verify(&d, &pubf).is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", d.join("src/x")).unwrap();
            assert!(digest(&d).unwrap_err().contains("no links"));
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_file(&pubf);
    }

    fn b64(b: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    #[test]
    fn package_is_the_folder() {
        let d = tmp("pkg");
        folder(&d);
        // Built: the folder's files but src/ and RPG/, and the program
        // (rpg build: an app's DIR is its folder, with RPG/ beside)
        let b = tmp("pkgbuild");
        let root = b.clone();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(b.join("RPG")).unwrap();
        std::fs::copy(d.join("RPG/control"), b.join("RPG/control")).unwrap();
        std::fs::copy(d.join("manifest"), root.join("manifest")).unwrap();
        std::fs::write(root.join("bin/app"), b"\x7fELF program").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join("bin/app"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let (bytes, _) = crate::pkg::build(&b).unwrap();
        let f = b.with_extension("rpg");
        std::fs::write(&f, &bytes).unwrap();
        let m = check_package(&f, &d).unwrap();
        assert_eq!(m.developer.as_deref(), Some("io.github.alice"));
        // A file the folder does not have
        std::fs::write(root.join("extra"), b"x").unwrap();
        let (bytes, _) = crate::pkg::build(&b).unwrap();
        std::fs::write(&f, &bytes).unwrap();
        assert!(check_package(&f, &d).unwrap_err().contains("extra is not in the folder"));
        std::fs::remove_file(root.join("extra")).unwrap();
        // A changed manifest
        std::fs::write(root.join("manifest"), std::fs::read_to_string(d.join("manifest")).unwrap().replace("Name: Dice", "Name: Dice!")).unwrap();
        let (bytes, _) = crate::pkg::build(&b).unwrap();
        std::fs::write(&f, &bytes).unwrap();
        assert!(check_package(&f, &d).unwrap_err().contains("differs"));
        for x in [&d, &b] {
            let _ = std::fs::remove_dir_all(x);
        }
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn developer_namespace_and_updates() {
        let base = "Id: io.github.alice.dice\nName: Dice\nSummary: dice\nExec: bin/app\nPlatform: 1\n";
        let m = app::parse_manifest(&format!("{base}Developer: io.github.alice\n")).unwrap();
        let none = app::parse_manifest(base).unwrap();
        let bob = app::parse_manifest(&format!("{base}Developer: io.github.bob\n")).unwrap();
        assert!(app::same_developer(&m, &m).is_ok());
        assert!(app::same_developer(&none, &none).is_ok());
        assert!(app::same_developer(&m, &bob).unwrap_err().contains("does not pass to another developer"));
        assert!(app::same_developer(&none, &m).is_err());
        assert!(app::same_developer(&m, &none).is_err());
    }
}

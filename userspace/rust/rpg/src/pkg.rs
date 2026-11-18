// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! A package: the `.rpg` file's parts, and `rpg build`.
//!
//! In the gzipped tar:
//!   control              the control file (control.rs)
//!   files                a line per file, directory and symbolic link,
//!                        tab-separated: TYPE MODE UID GID HASH FLAGS PATH
//!                        TYPE f, d or l; HASH the file's SHA-256, a
//!                        link's target, "-" for a directory; FLAGS
//!                        "conf" (a configuration file: everything in
//!                        /etc) or "-"
//!   scripts/postinst     optional: run after installing (install, or
//!                        upgrade OLD-VERSION)
//!   scripts/prerm        optional: run before removing (remove)
//!   data/...             the files, rooted at /

use crate::archive::{self, Entry, Kind};
use crate::control::Control;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FType {
    File,
    Dir,
    Link,
}

/// A line of the files list
#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub typ: FType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// A file's SHA-256, a link's target ("" for a directory)
    pub hash: String,
    pub conf: bool,
    /// Absolute: /usr/bin/x
    pub path: String,
}

impl FileEntry {
    pub fn line(&self) -> String {
        format!(
            "{}\t{:04o}\t{}\t{}\t{}\t{}\t{}",
            match self.typ {
                FType::File => "f",
                FType::Dir => "d",
                FType::Link => "l",
            },
            self.mode & 0o7777,
            self.uid,
            self.gid,
            if self.hash.is_empty() { "-" } else { &self.hash },
            if self.conf { "conf" } else { "-" },
            self.path
        )
    }

    pub fn parse(line: &str) -> Result<FileEntry, String> {
        let f: Vec<&str> = line.splitn(7, '\t').collect();
        if f.len() != 7 {
            return Err(format!("a bad line in a files list: {line:?}"));
        }
        let typ = match f[0] {
            "f" => FType::File,
            "d" => FType::Dir,
            "l" => FType::Link,
            t => return Err(format!("a bad type in a files list: {t}")),
        };
        let num = |s: &str, radix| u32::from_str_radix(s, radix).map_err(|_| format!("a bad number in a files list: {line:?}"));
        let e = FileEntry {
            typ,
            mode: num(f[1], 8)?,
            uid: num(f[2], 10)?,
            gid: num(f[3], 10)?,
            hash: if f[4] == "-" && typ == FType::Dir { String::new() } else { f[4].to_string() },
            conf: f[5] == "conf",
            path: f[6].to_string(),
        };
        if !valid_path(&e.path) {
            return Err(format!("{}: not a clean absolute path", e.path));
        }
        if e.typ == FType::File && (e.hash.len() != 64 || !e.hash.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(format!("{}: a bad SHA-256", e.path));
        }
        Ok(e)
    }
}

pub fn parse_files(text: &str) -> Result<Vec<FileEntry>, String> {
    text.lines().filter(|l| !l.is_empty()).map(FileEntry::parse).collect()
}

pub fn files_text(files: &[FileEntry]) -> String {
    files.iter().map(|f| f.line() + "\n").collect()
}

/// /a/b: absolute, no empty, "." or ".." parts, no tabs or newlines
pub fn valid_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() > 1
        && !p.contains(['\t', '\n', '\r', '\0'])
        && p[1..].split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

pub fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// A package read into memory and checked
pub struct Package {
    pub control: Control,
    pub files: Vec<FileEntry>,
    pub postinst: Option<Vec<u8>>,
    pub prerm: Option<Vec<u8>>,
    /// The data: path -> content (files) or target (links)
    pub data: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn read(bytes: &[u8]) -> Result<Package, String> {
        let entries = archive::read(bytes)?;
        let mut control = None;
        let mut files = None;
        let (mut postinst, mut prerm) = (None, None);
        let mut data = BTreeMap::new();
        let mut data_kinds: BTreeMap<String, FType> = BTreeMap::new();
        for e in entries {
            match (e.path.as_str(), &e.kind) {
                ("control", Kind::File(d)) => control = Some(String::from_utf8(d.clone()).map_err(|_| "the control file is not UTF-8")?),
                ("files", Kind::File(d)) => files = Some(String::from_utf8(d.clone()).map_err(|_| "the files list is not UTF-8")?),
                ("scripts/postinst", Kind::File(d)) => postinst = Some(d.clone()),
                ("scripts/prerm", Kind::File(d)) => prerm = Some(d.clone()),
                ("scripts", Kind::Dir) | ("data", Kind::Dir) => {}
                (p, k) if p.starts_with("data/") => {
                    let path = format!("/{}", &p[5..]);
                    match k {
                        Kind::File(d) => {
                            data_kinds.insert(path.clone(), FType::File);
                            data.insert(path, d.clone());
                        }
                        Kind::Symlink(t) => {
                            data_kinds.insert(path.clone(), FType::Link);
                            data.insert(path, t.as_bytes().to_vec());
                        }
                        Kind::Dir => {
                            data_kinds.insert(path, FType::Dir);
                        }
                    }
                }
                (p, _) => return Err(format!("{p}: not part of a package")),
            }
        }
        let control = Control::parse(&control.ok_or("no control file: not a package")?)?;
        control.check()?;
        let files = parse_files(&files.ok_or("no files list: not a package")?)?;
        // Every listed thing is in the data, the same kind, with its hash;
        // nothing unlisted
        let mut seen = std::collections::BTreeSet::new();
        for f in &files {
            if !seen.insert(f.path.clone()) {
                return Err(format!("{}: listed twice", f.path));
            }
            match data_kinds.get(&f.path) {
                Some(k) if *k == f.typ => {}
                Some(_) => return Err(format!("{}: not the kind the files list says", f.path)),
                None if f.typ == FType::Dir => {}
                None => return Err(format!("{}: in the files list but not in the package", f.path)),
            }
            match f.typ {
                FType::File => {
                    if sha256_hex(&data[&f.path]) != f.hash {
                        return Err(format!("{}: damaged (its SHA-256 differs from the files list)", f.path));
                    }
                }
                FType::Link => {
                    if data[&f.path] != f.hash.as_bytes() {
                        return Err(format!("{}: the link's target differs from the files list", f.path));
                    }
                }
                FType::Dir => {}
            }
        }
        for p in data_kinds.keys() {
            if !seen.contains(p) {
                return Err(format!("{p}: in the package but not in its files list"));
            }
        }
        Ok(Package { control, files, postinst, prerm, data })
    }
}

/// rpg build DIR: DIR/RPG/control (and RPG/postinst, RPG/prerm), the
/// rest of DIR is the data, rooted at /. Everything belongs to root;
/// the modes are the files' own. Returns the package and its file name.
pub fn build(dir: &Path) -> Result<(Vec<u8>, String), String> {
    let rpgdir = dir.join("RPG");
    let ctext = std::fs::read_to_string(rpgdir.join("control")).map_err(|e| format!("{}: {e}", rpgdir.join("control").display()))?;
    let mut control = Control::parse(&ctext)?;
    control.check()?;
    for f in std::fs::read_dir(&rpgdir).map_err(|e| format!("{}: {e}", rpgdir.display()))?.flatten() {
        let n = f.file_name().to_string_lossy().to_string();
        if !["control", "postinst", "prerm"].contains(&n.as_str()) {
            return Err(format!("RPG/{n}: the RPG folder holds control, postinst and prerm"));
        }
    }
    let mut files = Vec::new();
    let mut data = Vec::new();
    let mut size: u64 = 0;
    walk(dir, dir, &mut files, &mut data, &mut size)?;
    if files.iter().all(|f| f.typ == FType::Dir) && control.get("Depends").is_none() {
        return Err(format!("{}: no files (and no Depends: an empty package)", dir.display()));
    }
    control.set("Installed-Size", &size.div_ceil(1024).max(1).to_string());
    let mut entries = vec![
        Entry { path: "control".into(), mode: 0o644, kind: Kind::File(control.text().into_bytes()) },
        Entry { path: "files".into(), mode: 0o644, kind: Kind::File(files_text(&files).into_bytes()) },
    ];
    for s in ["postinst", "prerm"] {
        if let Ok(d) = std::fs::read(rpgdir.join(s)) {
            entries.push(Entry { path: format!("scripts/{s}"), mode: 0o755, kind: Kind::File(d) });
        }
    }
    entries.extend(data);
    // The file name leaves the epoch out, as Debian's do
    let v = control.version();
    let v = v.split_once(':').map_or(v, |(_, rest)| rest);
    let name = format!("{}_{}_{}.rpg", control.name(), v, control.arch());
    Ok((archive::write(&entries), name))
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<FileEntry>, data: &mut Vec<Entry>, size: &mut u64) -> Result<(), String> {
    let mut names: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    names.sort();
    for p in names {
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
        if rel == "RPG" {
            continue;
        }
        let abs = format!("/{rel}");
        if !valid_path(&abs) {
            return Err(format!("{abs}: a name rpg cannot hold (tabs or newlines)"));
        }
        let md = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mode = md.permissions().mode() & 0o7777;
        let conf = abs.starts_with("/etc/");
        if md.file_type().is_symlink() {
            let t = std::fs::read_link(&p).map_err(|e| format!("{}: {e}", p.display()))?.to_string_lossy().to_string();
            files.push(FileEntry { typ: FType::Link, mode: 0o777, uid: 0, gid: 0, hash: t.clone(), conf: false, path: abs });
            data.push(Entry { path: format!("data/{rel}"), mode: 0o777, kind: Kind::Symlink(t) });
        } else if md.is_dir() {
            files.push(FileEntry { typ: FType::Dir, mode, uid: 0, gid: 0, hash: String::new(), conf: false, path: abs });
            data.push(Entry { path: format!("data/{rel}"), mode, kind: Kind::Dir });
            walk(root, &p, files, data, size)?;
        } else if md.is_file() {
            let d = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            *size += md.size();
            files.push(FileEntry { typ: FType::File, mode, uid: 0, gid: 0, hash: sha256_hex(&d), conf, path: abs });
            data.push(Entry { path: format!("data/{rel}"), mode, kind: Kind::File(d) });
        } else {
            return Err(format!("{}: only files, folders and symbolic links go into a package", p.display()));
        }
    }
    Ok(())
}

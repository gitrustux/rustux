// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The package database, /var/lib/rpg (under --root ROOT when given):
//!   status              a stanza per package: its control fields and
//!                       Status: installed (or config-files: removed,
//!                       its configuration files kept)
//!   info/NAME.files     the package's files list (pkg.rs's format)
//!   info/NAME.postinst, info/NAME.prerm   its scripts
//!   journal, pending/   the operation in progress (ops.rs): written
//!                       before anything changes, removed when done
//!   lock                the running rpg's process ID and when it started
//! Changes go to a new file renamed into place.

use crate::control::{self, Control};
use crate::pkg::{self, FileEntry};
use crate::sys;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Db {
    pub root: PathBuf,
}

impl Db {
    pub fn new(root: &Path) -> Db {
        Db { root: root.to_path_buf() }
    }

    /// An absolute path of the system rpg manages, on this computer
    pub fn path(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }

    pub fn dir(&self) -> PathBuf {
        self.path("/var/lib/rpg")
    }

    pub fn ensure(&self) -> Result<(), String> {
        for d in [self.dir(), self.dir().join("info")] {
            std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        Ok(())
    }

    pub fn status(&self) -> Result<Vec<Control>, String> {
        let p = self.dir().join("status");
        match std::fs::read_to_string(&p) {
            Ok(t) => control::parse_stanzas(&t).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    pub fn save_status(&self, all: &[Control]) -> Result<(), String> {
        let mut sorted: Vec<&Control> = all.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        let text: String = sorted.iter().map(|c| c.text()).collect::<Vec<_>>().join("\n");
        write_atomic(&self.dir().join("status"), text.as_bytes(), 0o644)
    }

    /// The installed packages (not those removed with their
    /// configuration files kept)
    pub fn installed(&self) -> Result<Vec<Control>, String> {
        Ok(self.status()?.into_iter().filter(|c| c.get("Status") == Some("installed")).collect())
    }

    pub fn entry(&self, name: &str) -> Result<Option<Control>, String> {
        Ok(self.status()?.into_iter().find(|c| c.name() == name))
    }

    pub fn files_of(&self, name: &str) -> Result<Vec<FileEntry>, String> {
        let p = self.dir().join("info").join(format!("{name}.files"));
        match std::fs::read_to_string(&p) {
            Ok(t) => pkg::parse_files(&t).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Every path of every package: path -> the packages listing it
    pub fn owners(&self) -> Result<BTreeMap<String, Vec<String>>, String> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in self.status()? {
            for f in self.files_of(c.name())? {
                m.entry(f.path).or_default().push(c.name().to_string());
            }
        }
        Ok(m)
    }

    pub fn script(&self, name: &str, which: &str) -> PathBuf {
        self.dir().join("info").join(format!("{name}.{which}"))
    }

    /// A package's files list and scripts into info/
    pub fn write_info(&self, name: &str, files: &[FileEntry], postinst: Option<&[u8]>, prerm: Option<&[u8]>) -> Result<(), String> {
        write_atomic(&self.dir().join("info").join(format!("{name}.files")), pkg::files_text(files).as_bytes(), 0o644)?;
        for (which, d) in [("postinst", postinst), ("prerm", prerm)] {
            let p = self.script(name, which);
            match d {
                Some(d) => write_atomic(&p, d, 0o755)?,
                None => remove_if_there(&p)?,
            }
        }
        Ok(())
    }

    pub fn remove_info(&self, name: &str) -> Result<(), String> {
        for w in ["files", "postinst", "prerm"] {
            remove_if_there(&self.dir().join("info").join(format!("{name}.{w}")))?;
        }
        Ok(())
    }
}

pub fn remove_if_there(p: &Path) -> Result<(), String> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Write a file under a temporary name, to the disk, then rename it
/// into place
pub fn write_atomic(p: &Path, data: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = p.with_file_name(format!("{}.new", p.file_name().unwrap().to_string_lossy()));
    let err = |e: std::io::Error| format!("{}: {e}", tmp.display());
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp).map_err(err)?;
        f.write_all(data).map_err(err)?;
        f.sync_all().map_err(err)?;
    }
    std::fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Only one rpg changes the system at a time. The lock holds the
/// process's ID and the computer's uptime then: a lock whose process is
/// gone, or from before a restart, is stale and taken over.
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn take(db: &Db) -> Result<Lock, String> {
        db.ensure()?;
        let path = db.dir().join("lock");
        let me = format!("{} {}\n", sys::pid(), sys::uptime());
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = f.write_all(me.as_bytes());
                    return Ok(Lock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let t = std::fs::read_to_string(&path).unwrap_or_default();
                    let mut w = t.split_whitespace();
                    let pid: i32 = w.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let up: i64 = w.next().and_then(|s| s.parse().ok()).unwrap_or(i64::MAX);
                    if pid != sys::pid() && sys::alive(pid) && up <= sys::uptime() {
                        return Err(format!("another rpg is changing the system (process {pid}); wait for it to finish"));
                    }
                    let _ = std::fs::remove_file(&path); // stale
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err(format!("{}: cannot take the lock", path.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

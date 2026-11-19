// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Installing and removing, safe against a power cut.
//!
//! Before anything changes, the whole plan goes to /var/lib/rpg/pending/
//! (the new control and files list, the scripts, and `actions`: a line
//! per path, what happens to it) and `journal` says which operation and
//! its phase:
//!
//!   begin       the new files are being written beside their places
//!               (PATH.rpg-tmp); nothing the system uses has changed.
//!               `rpg fix` undoes it: the .rpg-tmp files go, the old
//!               version stays as it was.
//!   staged      every new file is written and on the disk: from here
//!               the operation goes forward. Each PATH.rpg-tmp is
//!               renamed over PATH (one rename: a program sees the old
//!               file or the new one, never half of one), old files are
//!               deleted. `rpg fix` does what is left (a rename whose
//!               .rpg-tmp is gone was done).
//!   configure   the database says the new version; its postinst runs
//!               (again, after a power cut: scripts must be safe to run
//!               twice).
//!
//! Then journal and pending/ go. A removal starts at `staged` (its prerm
//! has run; the rest only deletes). Every rpg command that changes the
//! system finishes an interrupted operation first.
//!
//! Configuration files (everything in /etc) are the administrator's: one
//! changed since its package installed it is never overwritten; the new
//! one goes beside it as PATH.rpg-new. Removing keeps them (Status:
//! config-files) until --purge.

use crate::control::{Control, Rel};
use crate::db::{self, Db, Lock};
use crate::pkg::{sha256_hex, FType, FileEntry, Package};
use crate::{sys, version};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TMP: &str = ".rpg-tmp";
const NEW: &str = ".rpg-new";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// PATH.rpg-tmp renamed over PATH
    Rename,
    /// PATH.rpg-tmp renamed to PATH.rpg-new (a changed configuration file)
    New,
    /// The file is already as the package has it
    Skip,
    /// An old file deleted
    Delete,
    /// An old configuration file, changed here: left
    Keep,
    /// An old folder removed if empty
    Rmdir,
}

impl Act {
    fn word(self) -> &'static str {
        match self {
            Act::Rename => "rename",
            Act::New => "new",
            Act::Skip => "skip",
            Act::Delete => "delete",
            Act::Keep => "keep",
            Act::Rmdir => "rmdir",
        }
    }
    fn from(w: &str) -> Option<Act> {
        Some(match w {
            "rename" => Act::Rename,
            "new" => Act::New,
            "skip" => Act::Skip,
            "delete" => Act::Delete,
            "keep" => Act::Keep,
            "rmdir" => Act::Rmdir,
            _ => return None,
        })
    }
}

/// The operation in progress, as journal and pending/ hold it
struct Op {
    op: String, // install, remove
    name: String,
    version: String,
    old: Option<String>,
    phase: String,
    /// The package's status entry when done (None: removed entirely)
    control: Option<Control>,
    /// The files list when done
    files: Vec<FileEntry>,
    postinst: Option<Vec<u8>>,
    prerm: Option<Vec<u8>>,
    postrm: Option<Vec<u8>>,
    actions: Vec<(Act, String)>,
    /// The new files' content, while staging (not kept in pending/)
    data: BTreeMap<String, Vec<u8>>,
}

pub struct Ctx {
    pub db: Db,
}

fn err(p: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", p.display())
}

/// RPG_TEST_PAUSE=stage:N or rename:N (scripts/run-packages-test.sh):
/// stop after the N-th file of that phase, with everything on the disk,
/// so the test can cut the power there
fn test_pause(phase: &str, n: usize) {
    if let Ok(v) = std::env::var("RPG_TEST_PAUSE") {
        if v == format!("{phase}:{n}") {
            sys::sync_all();
            eprintln!("rpg: test pause ({phase} {n})");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    }
}

impl Ctx {
    fn pending(&self) -> PathBuf {
        self.db.dir().join("pending")
    }
    fn journal(&self) -> PathBuf {
        self.db.dir().join("journal")
    }

    pub fn interrupted(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.journal()).ok()?;
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k} "))).unwrap_or("?").to_string();
        Some(format!("{} of {} {}", get("op"), get("package"), get("version")))
    }

    fn write_journal(&self, op: &Op) -> Result<(), String> {
        let t = format!(
            "op {}\npackage {}\nversion {}\nold {}\nphase {}\n",
            op.op,
            op.name,
            op.version,
            op.old.as_deref().unwrap_or("-"),
            op.phase
        );
        db::write_atomic(&self.journal(), t.as_bytes(), 0o644)?;
        sys::sync_all();
        Ok(())
    }

    fn save_pending(&self, op: &Op) -> Result<(), String> {
        let p = self.pending();
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).map_err(|e| err(&p, e))?;
        if let Some(c) = &op.control {
            db::write_atomic(&p.join("control"), c.text().as_bytes(), 0o644)?;
        }
        db::write_atomic(&p.join("files"), crate::pkg::files_text(&op.files).as_bytes(), 0o644)?;
        let acts: String = op.actions.iter().map(|(a, path)| format!("{}\t{path}\n", a.word())).collect();
        db::write_atomic(&p.join("actions"), acts.as_bytes(), 0o644)?;
        for (w, d) in [("postinst", &op.postinst), ("prerm", &op.prerm), ("postrm", &op.postrm)] {
            if let Some(d) = d {
                db::write_atomic(&p.join(w), d, 0o755)?;
            }
        }
        Ok(())
    }

    fn load_op(&self) -> Result<Option<Op>, String> {
        let Ok(t) = std::fs::read_to_string(self.journal()) else { return Ok(None) };
        let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k} "))).map(str::to_string);
        let p = self.pending();
        let control = match std::fs::read_to_string(p.join("control")) {
            Ok(c) => Some(Control::parse(&c)?),
            Err(_) => None,
        };
        let files = crate::pkg::parse_files(&std::fs::read_to_string(p.join("files")).unwrap_or_default())?;
        let mut actions = Vec::new();
        for l in std::fs::read_to_string(p.join("actions")).unwrap_or_default().lines() {
            let (a, path) = l.split_once('\t').ok_or("pending/actions: a bad line")?;
            actions.push((Act::from(a).ok_or("pending/actions: a bad action")?, path.to_string()));
        }
        let old = get("old").filter(|o| o != "-");
        Ok(Some(Op {
            op: get("op").ok_or("journal: no op")?,
            name: get("package").ok_or("journal: no package")?,
            version: get("version").unwrap_or_default(),
            old,
            phase: get("phase").ok_or("journal: no phase")?,
            control,
            files,
            postinst: std::fs::read(p.join("postinst")).ok(),
            prerm: std::fs::read(p.join("prerm")).ok(),
            postrm: std::fs::read(p.join("postrm")).ok(),
            actions,
            data: BTreeMap::new(),
        }))
    }

    /// Finish (or undo) an interrupted operation. Ok(false): there was none.
    pub fn fix(&self) -> Result<bool, String> {
        let Some(mut op) = self.load_op()? else { return Ok(false) };
        match op.phase.as_str() {
            "begin" => {
                for (a, p) in &op.actions {
                    if matches!(a, Act::Rename | Act::New) {
                        let _ = std::fs::remove_file(self.tmp_of(p));
                    }
                }
                self.finish()?;
                println!(
                    "rpg: undid the interrupted {} of {} {}: {}",
                    op.op,
                    op.name,
                    op.version,
                    match &op.old {
                        Some(o) => format!("{} stays at {o}", op.name),
                        None => format!("{} is not installed", op.name),
                    }
                );
            }
            "staged" | "configure" => {
                println!("rpg: finishing the interrupted {} of {} {}", op.op, op.name, op.version);
                self.forward(&mut op)?;
            }
            p => return Err(format!("journal: an unknown phase {p}")),
        }
        Ok(true)
    }

    fn finish(&self) -> Result<(), String> {
        let _ = std::fs::remove_dir_all(self.pending());
        db::remove_if_there(&self.journal())?;
        sys::sync_all();
        Ok(())
    }

    fn tmp_of(&self, p: &str) -> PathBuf {
        let mut s = self.db.path(p).into_os_string();
        s.push(TMP);
        PathBuf::from(s)
    }
    fn new_of(&self, p: &str) -> PathBuf {
        let mut s = self.db.path(p).into_os_string();
        s.push(NEW);
        PathBuf::from(s)
    }

    /// From `staged` on: the renames and deletions, the database, the
    /// postinst
    fn forward(&self, op: &mut Op) -> Result<(), String> {
        if op.phase == "staged" {
            let mut n = 0;
            for (a, p) in &op.actions {
                let target = self.db.path(p);
                match a {
                    Act::Rename | Act::New => {
                        let tmp = self.tmp_of(p);
                        if tmp.symlink_metadata().is_ok() {
                            let to = if *a == Act::New { self.new_of(p) } else { target.clone() };
                            // A folder where a file goes: only an empty one
                            if to.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false) {
                                std::fs::remove_dir(&to).map_err(|e| err(&to, e))?;
                            }
                            std::fs::rename(&tmp, &to).map_err(|e| err(&to, e))?;
                            n += 1;
                            test_pause("rename", n);
                        }
                    }
                    Act::Delete => {
                        if target.symlink_metadata().map(|m| !m.is_dir()).unwrap_or(false) {
                            std::fs::remove_file(&target).map_err(|e| err(&target, e))?;
                        }
                    }
                    Act::Rmdir => {
                        let _ = std::fs::remove_dir(&target); // only if empty
                    }
                    Act::Skip | Act::Keep => {}
                }
            }
            sys::sync_all();
            // The database
            let mut all = self.db.status()?;
            all.retain(|c| c.name() != op.name);
            match &op.control {
                Some(c) => {
                    all.push(c.clone());
                    self.db.write_info(&op.name, &op.files, op.postinst.as_deref(), op.prerm.as_deref(), op.postrm.as_deref())?;
                }
                None => self.db.remove_info(&op.name)?,
            }
            self.db.save_status(&all)?;
            op.phase = "configure".into();
            self.write_journal(op)?;
        }
        if op.op == "install" {
            if let Some(s) = &op.postinst {
                let args: Vec<String> = match &op.old {
                    Some(o) => vec!["upgrade".into(), o.clone()],
                    None => vec!["install".into()],
                };
                self.run_script(&op.name, "postinst", s, &args);
            }
        } else if op.op == "remove" {
            // After the files are gone (the image packages update the boot
            // entries then)
            if let Some(s) = &op.postrm {
                self.run_script(&op.name, "postrm", s, &["remove".to_string()]);
            }
        }
        self.finish()
    }

    /// A package's script, by the system's shell (with --root: in the
    /// root, RPG_ROOT set)
    fn run_script(&self, name: &str, which: &str, script: &[u8], args: &[String]) {
        let p = self.pending().join(format!("run-{which}"));
        if std::fs::create_dir_all(self.pending()).is_err() || db::write_atomic(&p, script, 0o755).is_err() {
            eprintln!("rpg: {name}: cannot write its {which} script");
            return;
        }
        let shell = ["/bin/shell", "/bin/sh"].into_iter().find(|s| Path::new(s).exists()).unwrap_or("/bin/sh");
        let r = std::process::Command::new(shell)
            .arg(&p)
            .args(args)
            .current_dir(&self.db.root)
            .env("RPG_ROOT", &self.db.root)
            .env("RPG_PACKAGE", name)
            .status();
        match r {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("rpg: {name}: its {which} script failed ({s})"),
            Err(e) => eprintln!("rpg: {name}: its {which} script: {e}"),
        }
        let _ = std::fs::remove_file(&p);
    }

    /// Before changing anything: an interrupted operation finished first
    pub fn begin(&self) -> Result<Lock, String> {
        if !sys::is_root() && self.db.root == Path::new("/") {
            return Err("changing the system's packages needs root (sudo rpg ...)".into());
        }
        let lock = Lock::take(&self.db)?;
        self.fix()?;
        Ok(lock)
    }

    /// rpg install ./FILE.rpg ...
    /// Install packages read already (from files, or downloaded), each
    /// with its mark: Some(true) installed for another package,
    /// Some(false) asked for, None as the database has it
    pub fn install_packages(&self, items: Vec<(Package, Option<bool>)>) -> Result<(), String> {
        let (pkgs, marks): (Vec<Package>, Vec<Option<bool>>) = items.into_iter().unzip();
        // Dependencies and conflicts, with the whole set as installed
        let installed = self.db.installed()?;
        let names: BTreeSet<String> = pkgs.iter().map(|p| p.control.name().to_string()).collect();
        let mut world: Vec<Control> = installed.iter().filter(|c| !names.contains(c.name())).cloned().collect();
        world.extend(pkgs.iter().map(|p| p.control.clone()));
        for p in &pkgs {
            let c = &p.control;
            for clause in c.rels("Depends") {
                if !clause.iter().any(|r| satisfied(r, &world)) {
                    return Err(format!(
                        "{} {} needs {}{}",
                        c.name(),
                        c.version(),
                        clause.iter().map(Rel::text).collect::<Vec<_>>().join(" or "),
                        describe_have(&clause, &world)
                    ));
                }
            }
            for other in &world {
                if other.name() == c.name() {
                    continue;
                }
                for r in c.rels("Conflicts").iter().flatten() {
                    if matches(r, other) {
                        return Err(format!("{} conflicts with {} {}, which is installed", c.name(), other.name(), other.version()));
                    }
                }
                for r in other.rels("Conflicts").iter().flatten() {
                    if matches(r, c) {
                        return Err(format!("{} {} conflicts with {}", other.name(), other.version(), c.name()));
                    }
                }
            }
        }
        // Dependencies within the set go first
        let mut order: Vec<usize> = Vec::new();
        let mut placed = BTreeSet::new();
        while order.len() < pkgs.len() {
            let before = order.len();
            for (i, p) in pkgs.iter().enumerate() {
                if placed.contains(&i) {
                    continue;
                }
                let waits = p.control.rels("Depends").iter().flatten().any(|r| {
                    pkgs.iter().enumerate().any(|(j, q)| j != i && !placed.contains(&j) && q.control.name() == r.name)
                });
                if !waits {
                    order.push(i);
                    placed.insert(i);
                }
            }
            if order.len() == before {
                // A cycle: the rest as given
                for i in 0..pkgs.len() {
                    if placed.insert(i) {
                        order.push(i);
                    }
                }
            }
        }
        for i in order {
            self.install_one(&pkgs[i], marks[i])?;
        }
        Ok(())
    }

    fn install_one(&self, p: &Package, auto: Option<bool>) -> Result<(), String> {
        let c = &p.control;
        let name = c.name().to_string();
        let old_entry = self.db.entry(&name)?;
        let old = old_entry.as_ref().filter(|e| e.get("Status") == Some("installed")).map(|e| e.version().to_string());
        let old_files = self.db.files_of(&name)?;
        let old_map: BTreeMap<&str, &FileEntry> = old_files.iter().map(|f| (f.path.as_str(), f)).collect();
        let owners = self.db.owners()?;
        // Files of other packages
        for f in &p.files {
            if f.typ == FType::Dir {
                continue;
            }
            if let Some(o) = owners.get(&f.path) {
                let others: Vec<&String> = o.iter().filter(|n| **n != name).collect();
                if !others.is_empty() {
                    return Err(format!("{}: {} belongs to {} too; nothing was changed", name, f.path, others.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
                }
            }
            let on_disk = self.db.path(&f.path);
            if let Ok(m) = on_disk.symlink_metadata() {
                if m.is_dir() && std::fs::read_dir(&on_disk).map(|mut d| d.next().is_some()).unwrap_or(true) {
                    return Err(format!("{name}: {} is a folder with files in it here; the package has a file there", f.path));
                }
            }
        }
        // What happens to each path
        let mut actions = Vec::new();
        let mut kept_notes = Vec::new();
        let new_paths: BTreeSet<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
        for f in &p.files {
            match f.typ {
                FType::Dir => {}
                FType::Link => actions.push((Act::Rename, f.path.clone())),
                FType::File if !f.conf => actions.push((Act::Rename, f.path.clone())),
                FType::File => {
                    let here = std::fs::read(self.db.path(&f.path)).ok().map(|d| sha256_hex(&d));
                    let was = old_map.get(f.path.as_str()).map(|o| o.hash.clone());
                    let a = match here {
                        None => Act::Rename,
                        Some(h) if h == f.hash => Act::Skip,
                        Some(h) if was.as_deref() == Some(h.as_str()) => Act::Rename,
                        Some(_) => {
                            kept_notes.push(f.path.clone());
                            Act::New
                        }
                    };
                    actions.push((a, f.path.clone()));
                }
            }
        }
        // Old files the new version does not have; folders deepest first
        let mut old_dirs = Vec::new();
        for f in &old_files {
            if new_paths.contains(f.path.as_str()) {
                continue;
            }
            match f.typ {
                FType::Dir => old_dirs.push(f.path.clone()),
                _ if f.conf && self.changed(f) => actions.push((Act::Keep, f.path.clone())),
                _ => actions.push((Act::Delete, f.path.clone())),
            }
        }
        old_dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        for d in old_dirs {
            if top_level(&d) {
                continue;
            }
            if owners.get(&d).map_or(true, |o| o.iter().all(|n| *n == name)) {
                actions.push((Act::Rmdir, d));
            }
        }
        let mut control = c.clone();
        control.set("Status", "installed");
        // Installed for another package (rpg autoremove), held (rpg hold):
        // kept across upgrades
        let was = |f: &str| old_entry.as_ref().filter(|_| old.is_some()).and_then(|e| e.get(f)).map(str::to_string);
        match auto.unwrap_or(was("Auto").as_deref() == Some("yes")) {
            true => control.set("Auto", "yes"),
            false => control.remove("Auto"),
        }
        if was("Hold").as_deref() == Some("yes") {
            control.set("Hold", "yes");
        }
        let mut op = Op {
            op: "install".into(),
            name: name.clone(),
            version: c.version().to_string(),
            old: old.clone(),
            phase: "begin".into(),
            control: Some(control),
            files: p.files.clone(),
            postinst: p.postinst.clone(),
            prerm: p.prerm.clone(),
            postrm: p.postrm.clone(),
            actions,
            data: p.data.clone(),
        };
        self.save_pending(&op)?;
        self.write_journal(&op)?;
        let mut made = Vec::new();
        if let Err(e) = self.stage(&op, p, &mut made) {
            // Nothing the system uses has changed yet: undo at once
            for (a, path) in &op.actions {
                if matches!(a, Act::Rename | Act::New) {
                    let _ = std::fs::remove_file(self.tmp_of(path));
                }
            }
            for d in made.iter().rev() {
                let _ = std::fs::remove_dir(d);
            }
            self.finish()?;
            let hint = if e.contains("os error 30") && self.db.root == Path::new("/") {
                " (on Rustux, /bin, /usr and /boot are the boot image, read-only: packages go in /opt, /etc and /var, the system image in /boot/efi)"
            } else {
                ""
            };
            return Err(format!("{name}: {e}{hint}; nothing was changed"));
        }
        sys::sync_all();
        op.phase = "staged".into();
        self.write_journal(&op)?;
        self.forward(&mut op)?;
        let n = p.files.iter().filter(|f| f.typ != FType::Dir).count();
        match &old {
            Some(o) if o == c.version() => println!("{name} {}: installed again ({n} files)", c.version()),
            Some(o) => println!("{name}: {o} -> {} ({n} files)", c.version()),
            None => println!("{name} {}: installed ({n} files)", c.version()),
        }
        for k in kept_notes {
            println!("  {k}: changed here, so kept; the package's new one is {k}{NEW}");
        }
        for (a, k) in &op.actions {
            if *a == Act::Keep {
                println!("  {k}: changed here, so kept (no longer in the package)");
            }
        }
        Ok(())
    }

    /// A configuration file changed since its package installed it?
    fn changed(&self, f: &FileEntry) -> bool {
        match std::fs::read(self.db.path(&f.path)) {
            Ok(d) => sha256_hex(&d) != f.hash,
            Err(_) => false,
        }
    }

    /// The folders, then each new file beside its place (PATH.rpg-tmp)
    fn stage(&self, op: &Op, p: &Package, made: &mut Vec<PathBuf>) -> Result<(), String> {
        let root = sys::is_root();
        for f in p.files.iter().filter(|f| f.typ == FType::Dir) {
            let d = self.db.path(&f.path);
            match d.symlink_metadata() {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(format!("{}: there is a file where the package has a folder", f.path)),
                Err(_) => {
                    // The folders made, outermost first, to undo
                    let mut missing = Vec::new();
                    let mut a = d.as_path();
                    while a.symlink_metadata().is_err() {
                        missing.push(a.to_path_buf());
                        match a.parent() {
                            Some(p) => a = p,
                            None => break,
                        }
                    }
                    std::fs::create_dir_all(&d).map_err(|e| err(&d, e))?;
                    made.extend(missing.into_iter().rev());
                    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(f.mode));
                    if root {
                        let _ = sys::chown(&d, f.uid, f.gid);
                    }
                }
            }
        }
        let byp: BTreeMap<&str, &FileEntry> = p.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut n = 0;
        for (a, path) in &op.actions {
            if !matches!(a, Act::Rename | Act::New) {
                continue;
            }
            let f = byp[path.as_str()];
            let tmp = self.tmp_of(path);
            if let Some(parent) = tmp.parent() {
                if parent.symlink_metadata().is_err() {
                    std::fs::create_dir_all(parent).map_err(|e| err(parent, e))?;
                    made.push(parent.to_path_buf());
                }
            }
            let _ = std::fs::remove_file(&tmp);
            match f.typ {
                FType::Link => std::os::unix::fs::symlink(&f.hash, &tmp).map_err(|e| err(&tmp, e))?,
                _ => {
                    use std::io::Write;
                    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| err(&tmp, e))?;
                    out.write_all(&op.data[path.as_str()]).map_err(|e| err(&tmp, e))?;
                    out.sync_all().map_err(|e| err(&tmp, e))?;
                    drop(out);
                    if root {
                        let _ = sys::chown(&tmp, f.uid, f.gid);
                    }
                    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(f.mode)).map_err(|e| err(&tmp, e))?;
                }
            }
            n += 1;
            test_pause("stage", n);
        }
        Ok(())
    }

    /// Change one field of an installed package's status entry (Hold,
    /// Auto); None removes it. Under the lock.
    pub fn set_field(&self, name: &str, field: &str, value: Option<&str>) -> Result<(), String> {
        let mut all = self.db.status()?;
        let Some(e) = all.iter_mut().find(|c| c.name() == name && c.get("Status") == Some("installed")) else {
            return Err(format!("{name} is not installed"));
        };
        match value {
            Some(v) => e.set(field, v),
            None => e.remove(field),
        }
        self.db.save_status(&all)
    }

    /// rpg remove [--purge] NAME...
    pub fn remove(&self, names: &[String], purge: bool) -> Result<(), String> {
        let status = self.db.status()?;
        let set: BTreeSet<&str> = names.iter().map(|s| s.as_str()).collect();
        for n in names {
            match status.iter().find(|c| c.name() == n) {
                Some(c) if c.get("Status") == Some("installed") => {}
                Some(_) if purge => {}
                Some(_) => return Err(format!("{n} is not installed (its configuration files are: rpg remove --purge {n})")),
                None => return Err(format!("{n} is not installed")),
            }
        }
        // Packages that need them
        let remaining: Vec<Control> = status.iter().filter(|c| c.get("Status") == Some("installed") && !set.contains(c.name())).cloned().collect();
        for c in &remaining {
            for clause in c.rels("Depends") {
                if !clause.iter().any(|r| satisfied(r, &remaining)) {
                    let all: Vec<Control> = status.iter().filter(|c| c.get("Status") == Some("installed")).cloned().collect();
                    if clause.iter().any(|r| satisfied(r, &all)) {
                        return Err(format!(
                            "{} {} needs {}; remove it too (rpg remove {} {})",
                            c.name(),
                            c.version(),
                            clause.iter().map(Rel::text).collect::<Vec<_>>().join(" or "),
                            names.join(" "),
                            c.name()
                        ));
                    }
                }
            }
        }
        for n in names {
            self.remove_one(n, purge)?;
        }
        Ok(())
    }

    fn remove_one(&self, name: &str, purge: bool) -> Result<(), String> {
        let entry = self.db.entry(name)?.ok_or(format!("{name} is not installed"))?;
        let was_installed = entry.get("Status") == Some("installed");
        let files = self.db.files_of(name)?;
        if was_installed {
            if let Ok(s) = std::fs::read(self.db.script(name, "prerm")) {
                self.run_script(name, "prerm", &s, &["remove".to_string()]);
            }
        }
        let owners = self.db.owners()?;
        let mut actions = Vec::new();
        let mut kept = Vec::new();
        let mut dirs = Vec::new();
        for f in &files {
            match f.typ {
                FType::Dir => dirs.push(f.path.clone()),
                _ if f.conf && !purge => {
                    actions.push((Act::Keep, f.path.clone()));
                    kept.push(f.clone());
                }
                _ => {
                    actions.push((Act::Delete, f.path.clone()));
                    if f.conf {
                        // and what rpg put beside it
                        actions.push((Act::Delete, format!("{}{NEW}", f.path)));
                    }
                }
            }
        }
        // The folders holding kept files stay the package's too, so that
        // --purge removes them later
        for f in &files {
            if f.typ == FType::Dir && kept.iter().any(|k| k.typ != FType::Dir && k.path.starts_with(&format!("{}/", f.path))) {
                kept.push(f.clone());
            }
        }
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        for d in dirs {
            if top_level(&d) {
                continue;
            }
            if owners.get(&d).map_or(true, |o| o.iter().all(|n| n == name)) {
                actions.push((Act::Rmdir, d));
            }
        }
        // Kept configuration files stay the package's (Status: config-files)
        let control = if kept.iter().all(|k| k.typ == FType::Dir) {
            None
        } else {
            let mut c = entry.clone();
            c.set("Status", "config-files");
            Some(c)
        };
        let mut op = Op {
            op: "remove".into(),
            name: name.to_string(),
            version: entry.version().to_string(),
            old: Some(entry.version().to_string()),
            phase: "staged".into(),
            control,
            files: kept.clone(),
            postinst: None,
            prerm: None,
            // Its postrm runs after the removal (kept with the
            // configuration files' entry, for --purge)
            postrm: if was_installed { std::fs::read(self.db.script(name, "postrm")).ok() } else { None },
            actions,
            data: BTreeMap::new(),
        };
        self.save_pending(&op)?;
        self.write_journal(&op)?;
        self.forward(&mut op)?;
        // An app's data in the users' homes (FoxDen step FD1)
        let is_app = entry.kind() == "app";
        if is_app && purge {
            let homes: Vec<String> = std::fs::read_to_string(self.db.path("/etc/passwd"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split(':').nth(5).map(str::to_string))
                .filter(|h| h.starts_with('/') && h != "/")
                .collect();
            for d in crate::app::data_dirs(&homes, name) {
                let p = self.db.path(&d);
                if p.exists() {
                    std::fs::remove_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    println!("{name}: removed {d}");
                }
            }
        }
        if !was_installed {
            println!("{name}: its configuration files removed");
        } else if kept.iter().all(|k| k.typ == FType::Dir) {
            println!("{name} {}: removed{}", entry.version(), if is_app && !purge { " (the users' data for it stays: rpg remove --purge removes it)" } else { "" });
        } else {
            println!("{name} {}: removed; its configuration files are kept (rpg remove --purge {name} removes them)", entry.version());
        }
        Ok(())
    }
}

/// A package file, read and checked: whole, for this computer, not an app
pub fn read_file(f: &str) -> Result<Package, String> {
    let bytes = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
    let p = Package::read(&bytes).map_err(|e| format!("{f}: {e}"))?;
    check_installable(&p).map_err(|e| format!("{f}: {e}"))?;
    Ok(p)
}

pub fn check_installable(p: &Package) -> Result<(), String> {
    let a = p.control.arch();
    if a != "all" && a != std::env::consts::ARCH {
        return Err(format!("a package for {a}; this computer is {}", std::env::consts::ARCH));
    }
    if p.control.kind() == "app" {
        // FoxDen step FD1: everything in its own folder (app.rs)
        crate::app::check(p)?;
    } else if let Some(f) = p.files.iter().find(|f| f.path == "/apps" && f.typ != FType::Dir || f.path.starts_with("/apps/")) {
        return Err(format!("{}: /apps is for apps (Kind: app); a system package has nothing there", f.path));
    }
    Ok(())
}

/// /etc, /opt, /var...: never removed with a package, even if empty
fn top_level(d: &str) -> bool {
    !d.trim_start_matches('/').contains('/')
}

/// Does an installed (or about to be) package satisfy this relation, by
/// name and version or by Provides?
pub fn satisfied(r: &Rel, world: &[Control]) -> bool {
    world.iter().any(|c| matches(r, c))
}

fn matches(r: &Rel, c: &Control) -> bool {
    if c.name() == r.name {
        return match &r.op {
            Some((op, v)) => version::satisfies(c.version(), op, v),
            None => true,
        };
    }
    // Provides: names only (no versions)
    r.op.is_none() && c.rels("Provides").iter().flatten().any(|p| p.name == r.name)
}

fn describe_have(clause: &[Rel], world: &[Control]) -> String {
    for r in clause {
        if let Some(c) = world.iter().find(|c| c.name() == r.name) {
            return format!("; {} {} is installed", c.name(), c.version());
        }
    }
    ", which is not installed".into()
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Repositories (step E2): sources, `rpg update`, the package lists, and
//! `rpg index` (the publishing side).
//!
//! A repository is static files (any web server, mirror or disk):
//!
//!   dists/SUITE/Release        Origin, Suite, Date, Architectures,
//!                              Sections, and SHA256: a line per index
//!                              ("HASH SIZE SECTION/ARCH/Packages.gz")
//!   dists/SUITE/Release.sig    the Ed25519 signature of Release
//!   dists/SUITE/SECTION/ARCH/Packages.gz
//!                              a stanza per package: its control
//!                              fields and Filename, Size, SHA256
//!   pool/SECTION/L/NAME/NAME_VERSION_ARCH.rpg
//!
//! One signature covers everything: Release names each index's hash,
//! each index each package's. Sources are lines of
//! /etc/rpg/sources.list and /etc/rpg/sources.list.d/*.list:
//!
//!   https://rustux.com/download/rustica  redfox  main
//!   apps https://rustux.com/download/apps platform 1     (FoxDen's, FD3)
//!
//! `rpg update` keeps the lists in /var/lib/rpg/lists/; a source that
//! fails (unsigned, a wrong signature, a hash that differs, a server
//! that cannot be reached) keeps its old lists.

use crate::control::{self, Control};
use crate::db::{self, Db};
use crate::fetch::{self, Fetcher};
use crate::keys::{self, Kind};
use crate::pkg::{sha256_hex, Package};
use crate::version;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const ARCH: &str = std::env::consts::ARCH;

#[derive(Clone, Debug)]
pub struct Source {
    pub kind: Kind,
    pub url: String,
    pub suite: String,
    pub sections: Vec<String>,
    /// The file it is in
    pub file: PathBuf,
}

impl Source {
    pub fn base(&self) -> &str {
        self.url.trim_end_matches('/')
    }
    pub fn text(&self) -> String {
        format!(
            "{}{}  {}  {}",
            if self.kind == Kind::App { "apps " } else { "" },
            self.url,
            self.suite,
            self.sections.join(" ")
        )
    }
    /// The lists' file names start with this
    pub fn prefix(&self) -> String {
        let u = self.base().split_once("://").map_or(self.base(), |(_, r)| r);
        let mut s: String = u.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        s.push('_');
        s.push_str(&self.suite);
        s
    }
}

pub fn parse_source(line: &str, file: &Path) -> Result<Option<Source>, String> {
    let line = line.split('#').next().unwrap_or("");
    let mut w: Vec<&str> = line.split_whitespace().collect();
    if w.is_empty() {
        return Ok(None);
    }
    let kind = if w[0] == "apps" {
        w.remove(0);
        Kind::App
    } else {
        Kind::System
    };
    if w.len() < 3 {
        return Err(format!("{line:?}: a source is URL SUITE SECTION... (https://rustux.com/download/rustica redfox main)"));
    }
    let url = w[0];
    if !["https://", "http://", "file://"].iter().any(|p| url.starts_with(p)) {
        return Err(format!("{url}: a source's address starts with https://, http:// or file://"));
    }
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if !w[1..].iter().all(|s| ok(s)) {
        return Err(format!("{line:?}: a suite and sections are letters, digits and - _ ."));
    }
    Ok(Some(Source { kind, url: url.to_string(), suite: w[1].to_string(), sections: w[2..].iter().map(|s| s.to_string()).collect(), file: file.to_path_buf() }))
}

pub fn list_file(db: &Db) -> PathBuf {
    db.path("/etc/rpg/sources.list")
}

/// Every source, sources.list first, then sources.list.d/*.list
pub fn sources(db: &Db) -> Result<Vec<Source>, String> {
    let mut files = vec![list_file(db)];
    if let Ok(rd) = std::fs::read_dir(db.path("/etc/rpg/sources.list.d")) {
        let mut more: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "list")).collect();
        more.sort();
        files.extend(more);
    }
    let mut out = Vec::new();
    for f in files {
        let Ok(t) = std::fs::read_to_string(&f) else { continue };
        for (n, l) in t.lines().enumerate() {
            match parse_source(l, &f) {
                Ok(Some(s)) => out.push(s),
                Ok(None) => {}
                Err(e) => return Err(format!("{}:{}: {e}", f.display(), n + 1)),
            }
        }
    }
    Ok(out)
}

/// What Release says
pub struct Release {
    pub fields: Control,
    /// path (SECTION/ARCH/Packages.gz) -> (SHA-256, size)
    pub files: Vec<(String, String, u64)>,
}

pub fn parse_release(text: &str) -> Result<Release, String> {
    let c = control::parse_stanzas(text)?.into_iter().next().ok_or("an empty Release file")?;
    let mut files = Vec::new();
    for l in c.get("SHA256").unwrap_or("").lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() != 3 {
            continue;
        }
        let size = w[1].parse().map_err(|_| format!("Release: a bad size in {l:?}"))?;
        files.push((w[2].to_string(), w[0].to_lowercase(), size));
    }
    Ok(Release { fields: c, files })
}

pub fn lists_dir(db: &Db) -> PathBuf {
    db.path("/var/lib/rpg/lists")
}

/// A package a source has
#[derive(Clone, Debug)]
pub struct Avail {
    pub c: Control,
    pub source: Source,
    pub filename: String,
    pub size: u64,
    pub sha: String,
}

impl Avail {
    pub fn url(&self) -> String {
        format!("{}/{}", self.source.base(), self.filename)
    }
    pub fn name(&self) -> &str {
        self.c.name()
    }
    pub fn version(&self) -> &str {
        self.c.version()
    }
}

/// rpg update: each source's Release, its signature, its indexes.
/// Returns how many sources failed.
pub fn update(db: &Db, f: &mut Fetcher) -> Result<usize, String> {
    let srcs = sources(db)?;
    let dir = lists_dir(db);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if srcs.is_empty() {
        println!("No sources: rpg source add URL SUITE SECTION (/etc/rpg/sources.list)");
    }
    let mut failed = 0;
    let mut keep: Vec<String> = Vec::new();
    for s in &srcs {
        let label = format!("{} {}", s.base(), s.suite);
        keep.push(s.prefix());
        if s.kind == Kind::App {
            println!("{label}: an app source: apps come with FoxDen (FOXDEN.md step FD3), not yet");
            continue;
        }
        match update_one(db, f, s) {
            Ok(n) => println!("{label}: {n} packages"),
            Err(e) => {
                failed += 1;
                println!("{label}: REFUSED: {e}");
                let had = std::fs::read_dir(&dir).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with(&s.prefix()))).unwrap_or(false);
                if had {
                    println!("  (its lists from before are kept)");
                }
            }
        }
    }
    // Lists of sources no longer there
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !keep.iter().any(|p| n.starts_with(&format!("{p}_"))) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(failed)
}

fn update_one(db: &Db, f: &mut Fetcher, s: &Source) -> Result<usize, String> {
    let base = format!("{}/dists/{}", s.base(), s.suite);
    let release = f.get(&format!("{base}/Release")).map_err(|e| match e {
        fetch::Error::NotFound => format!("no such repository ({base}/Release: {})", e.text()),
        e => format!("{base}/Release: {}", e.text()),
    })?;
    let sig = f.get(&format!("{base}/Release.sig")).map_err(|e| format!("not signed ({base}/Release.sig: {})", e.text()))?;
    let trusted = keys::load(db, s.kind);
    if trusted.is_empty() {
        return Err("no trusted keys (/etc/rpg/keys; rpg key add FILE)".into());
    }
    let signer = keys::verify(&trusted, &release, &sig)
        .ok_or("the signature of Release is not by a trusted key (or Release was changed after signing)")?;
    let text = String::from_utf8(release.clone()).map_err(|_| "Release is not text")?;
    let rel = parse_release(&text)?;
    if rel.fields.get("Suite") != Some(s.suite.as_str()) {
        return Err(format!("Release is for {:?}, not {}", rel.fields.get("Suite").unwrap_or("?"), s.suite));
    }
    let mut got: Vec<(String, Vec<u8>)> = Vec::new();
    let mut count = 0;
    for sec in &s.sections {
        let path = format!("{sec}/{ARCH}/Packages.gz");
        let Some((_, hash, size)) = rel.files.iter().find(|(p, _, _)| *p == path) else {
            return Err(format!("Release has no {path} (sections: {})", rel.fields.get("Sections").unwrap_or("?")));
        };
        let gz = f.get(&format!("{base}/{path}")).map_err(|e| format!("{base}/{path}: {}", e.text()))?;
        if gz.len() as u64 != *size || sha256_hex(&gz) != *hash {
            return Err(format!("{path}: its SHA-256 differs from Release's (changed after signing?)"));
        }
        let mut plain = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut plain).map_err(|e| format!("{path}: not gzip ({e})"))?;
        let text = String::from_utf8(plain).map_err(|_| format!("{path}: not text"))?;
        let st = control::parse_stanzas(&text).map_err(|e| format!("{path}: {e}"))?;
        for c in &st {
            index_entry(c, s).map_err(|e| format!("{path}: {e}"))?;
        }
        count += st.len();
        got.push((format!("{}_{sec}_{ARCH}_Packages", s.prefix()), text.into_bytes()));
    }
    // All of it checked: now into the lists
    let dir = lists_dir(db);
    db::write_atomic(&dir.join(format!("{}_Release", s.prefix())), &release, 0o644)?;
    db::write_atomic(&dir.join(format!("{}_Release.sig", s.prefix())), &sig, 0o644)?;
    for (n, d) in got {
        db::write_atomic(&dir.join(n), &d, 0o644)?;
    }
    let _ = signer;
    Ok(count)
}

fn index_entry(c: &Control, s: &Source) -> Result<Avail, String> {
    c.check().map_err(|e| format!("{}: {e}", c.name()))?;
    let filename = c.get("Filename").ok_or(format!("{}: no Filename", c.name()))?.to_string();
    if filename.starts_with('/') || filename.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err(format!("{}: a bad Filename", c.name()));
    }
    let size = c.get("Size").and_then(|v| v.parse().ok()).ok_or(format!("{}: no Size", c.name()))?;
    let sha = c.get("SHA256").filter(|h| h.len() == 64).ok_or(format!("{}: no SHA256", c.name()))?.to_lowercase();
    Ok(Avail { c: c.clone(), source: s.clone(), filename, size, sha })
}

/// Every package the sources have (from the lists rpg update made)
pub fn available(db: &Db) -> Result<Vec<Avail>, String> {
    let mut out = Vec::new();
    let dir = lists_dir(db);
    for s in sources(db)? {
        if s.kind != Kind::System {
            continue;
        }
        for sec in &s.sections {
            let p = dir.join(format!("{}_{sec}_{ARCH}_Packages", s.prefix()));
            let Ok(t) = std::fs::read_to_string(&p) else { continue };
            for c in control::parse_stanzas(&t).map_err(|e| format!("{}: {e}", p.display()))? {
                if c.arch() == ARCH || c.arch() == "all" {
                    out.push(index_entry(&c, &s)?);
                }
            }
        }
    }
    Ok(out)
}

/// The version rpg would install: the highest any source has (the first
/// source's when two have the same)
pub fn candidate<'a>(av: &'a [Avail], name: &str) -> Option<&'a Avail> {
    let mut best: Option<&Avail> = None;
    for a in av.iter().filter(|a| a.name() == name) {
        if best.is_none_or(|b| version::compare(a.version(), b.version()) == std::cmp::Ordering::Greater) {
            best = Some(a);
        }
    }
    best
}

/// Were the lists ever made?
pub fn have_lists(db: &Db) -> bool {
    std::fs::read_dir(lists_dir(db)).map(|mut d| d.next().is_some()).unwrap_or(false)
}

// ---- The publishing side: rpg index ----

pub struct IndexOpts {
    pub suite: String,
    pub section: String,
    pub archs: Vec<String>,
    pub origin: String,
}

/// rpg index REPO: every .rpg under REPO/pool (checked whole), into
/// REPO/dists/SUITE/SECTION/ARCH/Packages.gz (packages for all in every
/// architecture's) and REPO/dists/SUITE/Release. Signing is apart.
pub fn index(repo: &Path, o: &IndexOpts) -> Result<(), String> {
    let pool = repo.join("pool");
    let mut files = Vec::new();
    walk_rpg(&pool, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("{}: no .rpg files under it", pool.display()));
    }
    let mut entries: Vec<Control> = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let p = Package::read(&bytes).map_err(|e| format!("{}: {e}", f.display()))?;
        let mut c = p.control.clone();
        if entries.iter().any(|e| e.name() == c.name() && e.version() == c.version() && e.arch() == c.arch()) {
            return Err(format!("{} {} ({}) is in the pool twice", c.name(), c.version(), c.arch()));
        }
        let rel = f.strip_prefix(repo).unwrap().to_string_lossy().to_string();
        c.set("Filename", &rel);
        c.set("Size", &bytes.len().to_string());
        c.set("SHA256", &sha256_hex(&bytes));
        entries.push(c);
    }
    let mut archs = o.archs.clone();
    if archs.is_empty() {
        archs = entries.iter().map(|c| c.arch().to_string()).filter(|a| a != "all").collect();
        archs.sort();
        archs.dedup();
        if archs.is_empty() {
            archs.push("x86_64".into());
        }
    }
    let dists = repo.join("dists").join(&o.suite);
    let mut sums = Vec::new();
    for a in &archs {
        let mut mine: Vec<&Control> = entries.iter().filter(|c| c.arch() == a || c.arch() == "all").collect();
        mine.sort_by(|x, y| x.name().cmp(y.name()).then(version::compare(x.version(), y.version())));
        let text: String = mine.iter().map(|c| c.text()).collect::<Vec<_>>().join("\n");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut gz, text.as_bytes()).map_err(|e| e.to_string())?;
        let gz = gz.finish().map_err(|e| e.to_string())?;
        let rel = format!("{}/{a}/Packages.gz", o.section);
        let p = dists.join(&rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| format!("{}: {e}", p.display()))?;
        std::fs::write(&p, &gz).map_err(|e| format!("{}: {e}", p.display()))?;
        sums.push(format!(" {} {} {rel}", sha256_hex(&gz), gz.len()));
        println!("{}: {} packages", p.display(), mine.len());
    }
    let release = format!(
        "Origin: {}\nSuite: {}\nDate: {}\nArchitectures: {}\nSections: {}\nSHA256:\n{}\n",
        o.origin,
        o.suite,
        now_text(),
        archs.join(" "),
        o.section,
        sums.join("\n")
    );
    let rp = dists.join("Release");
    std::fs::write(&rp, release).map_err(|e| format!("{}: {e}", rp.display()))?;
    let _ = std::fs::remove_file(dists.join("Release.sig"));
    println!("{}: written; now sign it:", rp.display());
    println!("  openssl pkeyutl -sign -inkey KEY.pem -rawin -in {} -out {}.sig", rp.display(), rp.display());
    Ok(())
}

fn walk_rpg(d: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let rd = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_rpg(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "rpg") {
            out.push(p);
        }
    }
    Ok(())
}

/// The pool path for a package: pool/SECTION/L/NAME/FILE
pub fn pool_path(section: &str, name: &str, file: &str) -> String {
    let l = if name.starts_with("lib") && name.len() > 3 { &name[..4] } else { &name[..1] };
    format!("pool/{section}/{l}/{name}/{file}")
}

/// UTC now, as "2026-11-18 12:34:56 UTC"
fn now_text() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, secs) = ((t / 86400) as i64, t % 86400);
    // Days to a civil date (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", secs / 3600, secs % 3600 / 60, secs % 60)
}

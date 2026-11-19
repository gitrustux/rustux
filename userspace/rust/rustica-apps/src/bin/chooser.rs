// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! chooser: the file chooser an app's request shows (FoxDen step FD6,
//! docs/FOXDEN.md "The file chooser"). appctl runs it, as the user, when
//! a sandboxed app asks to open or save a file:
//!
//!   chooser open|save --app NAME --out FILE [--title T] [--patterns "*.txt *.md"] [--name N]
//!
//! The user goes through their folders (the places: Home, Documents,
//! Pictures, ...), picks a file or types its name, and says Open (Save),
//! or Cancel. The path chosen is written to FILE and chooser ends with 0;
//! 1 when the user said no. It is a window of its own, drawn by the
//! session, so an app cannot pick for the user or see the folders.

use rustica_ui::ui::{keys, Column, Icon, Row};
use rustica_ui::{Control, Request, Ui};
use std::path::{Path, PathBuf};

struct Entry {
    name: String,
    dir: bool,
    size: u64,
}

struct Chooser {
    save: bool,
    app: String,
    title: String,
    patterns: Vec<String>,
    all: bool,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    name: String,
    out: String,
    error: Option<String>,
    replace: Option<PathBuf>,
    first: bool,
}

/// "*.txt" matching "note.txt"; "*" anything (one star at most per pattern)
fn matches(pat: &str, name: &str) -> bool {
    let (p, n) = (pat.to_lowercase(), name.to_lowercase());
    match p.split_once('*') {
        Some((a, b)) => n.len() >= a.len() + b.len() && n.starts_with(a) && n.ends_with(b),
        None => p == n,
    }
}

impl Chooser {
    fn read(&mut self) {
        self.entries.clear();
        self.selected = None;
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let md = std::fs::metadata(e.path());
                    let dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    if !dir && !self.all && !self.patterns.is_empty() && !self.patterns.iter().any(|p| matches(p, &name)) {
                        continue;
                    }
                    self.entries.push(Entry { name, dir, size: md.map(|m| m.len()).unwrap_or(0) });
                }
                self.entries.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{}: {e}", self.dir.display())),
        }
    }

    fn go(&mut self, d: PathBuf) {
        self.dir = d;
        self.read();
    }

    fn done(&self, p: &Path) -> ! {
        if std::fs::write(&self.out, format!("{}\n", p.display())).is_err() {
            eprintln!("chooser: {}: cannot write", self.out);
            std::process::exit(2);
        }
        println!("chooser: {} {}", if self.save { "save" } else { "open" }, p.display());
        std::process::exit(0);
    }

    /// The name typed (or picked), chosen
    fn choose(&mut self) {
        let n = self.name.trim().to_string();
        if n.is_empty() {
            return;
        }
        if n.contains('/') {
            self.error = Some("a name, not a path: go to the folder first".into());
            return;
        }
        let p = self.dir.join(&n);
        let md = std::fs::metadata(&p);
        if md.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
            self.name.clear();
            self.go(p);
            return;
        }
        if self.save {
            if md.is_ok() && self.replace.as_deref() != Some(p.as_path()) {
                self.replace = Some(p);
                return;
            }
            self.done(&p);
        }
        if md.is_err() {
            self.error = Some(format!("{n}: there is no such file here"));
            return;
        }
        self.done(&p);
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ctl.requests.push(Request::Title(format!("{} — {}", self.title, self.app)));
        for k in &ui.input.keys {
            if k.usage == keys::ESC {
                std::process::exit(1);
            }
        }
        ui.heading(&format!("{} wants to {} a file", self.app, if self.save { "save" } else { "open" }));
        ui.dim_label(&format!(
            "{}{} It gets this one file only, not its folder.",
            self.title,
            if self.title.ends_with('.') || self.title.is_empty() { "" } else { "." }
        ));
        // The places
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        ui.row(|ui| {
            for (label, p) in [("Home", home.clone()), ("Documents", home.join("Documents")), ("Pictures", home.join("Pictures")), ("Music", home.join("Music")), ("Videos", home.join("Videos")), ("Downloads", home.join("Downloads"))] {
                if p.is_dir() && ui.button(label) {
                    self.go(p);
                }
            }
            if self.dir.parent().is_some() && ui.button("Up") {
                let up = self.dir.parent().unwrap().to_path_buf();
                self.go(up);
            }
        });
        ui.dim_label(&self.dir.display().to_string());
        let rows: Vec<Row> = self
            .entries
            .iter()
            .map(|e| Row { icon: if e.dir { Icon::Folder } else { Icon::File }, cells: vec![e.name.clone(), if e.dir { String::new() } else { format!("{} KB", e.size.div_ceil(1024)) }] })
            .collect();
        let h = (ui.remaining_height() / ui.scale - 110.0).max(80.0);
        let cols = [Column { title: "Name", width: 0.0, right: false }, Column { title: "Size", width: 90.0, right: true }];
        let r = ui.table("files", &cols, &rows, &mut self.selected, h);
        if r.changed {
            if let Some(e) = self.selected.and_then(|i| self.entries.get(i)) {
                if !e.dir {
                    self.name = e.name.clone();
                }
            }
        }
        if let Some(i) = r.activated {
            if let Some(e) = self.entries.get(i) {
                let p = self.dir.join(&e.name);
                if e.dir {
                    self.go(p);
                } else {
                    self.name = e.name.clone();
                    self.choose();
                }
            }
        }
        if !self.patterns.is_empty() {
            let mut all = self.all;
            if ui.checkbox(&mut all, &format!("All files (not only {})", self.patterns.join(" "))) {
                self.all = all;
                self.read();
            }
        }
        // The name, then the buttons
        if self.first {
            ui.set_focus("name");
            self.first = false;
        }
        let mut go = false;
        ui.row(|ui| {
            // The label centred on the field's line
            let h = ui.line_h() + ui.u(12.0);
            let r = ui.alloc(ui.u(56.0), h);
            let (px, c) = (ui.font_px(), ui.theme.text);
            let y = r.y + (h - ui.line_h()) / 2.0;
            ui.canvas.text(&mut ui.face, r.x, y, "Name", px, c);
            let w = ui.available_width();
            let (changed, enter) = ui.text_field("name", &mut self.name, w);
            if changed {
                self.replace = None;
            }
            go |= enter;
        });
        if let Some(p) = &self.replace {
            ui.label(&format!("{} is there already: Save again to replace it.", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
        }
        if let Some(e) = &self.error {
            ui.label(e);
        }
        ui.row(|ui| {
            if ui.primary_button(if self.save { "Save" } else { "Open" }) {
                go = true;
            }
            if ui.button("Cancel") {
                std::process::exit(1);
            }
        });
        if go {
            self.choose();
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let save = match a.first().map(String::as_str) {
        Some("open") => false,
        Some("save") => true,
        _ => {
            eprintln!("usage: chooser open|save --app NAME --out FILE [--title T] [--patterns P] [--name N]");
            std::process::exit(2);
        }
    };
    let val = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned().unwrap_or_default();
    let out = val("--out");
    if out.is_empty() {
        eprintln!("chooser: --out FILE");
        std::process::exit(2);
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let docs = home.join("Documents");
    let mut c = Chooser {
        save,
        app: Some(val("--app")).filter(|s| !s.is_empty()).unwrap_or_else(|| "An app".into()),
        title: val("--title"),
        patterns: val("--patterns").split_whitespace().map(str::to_string).collect(),
        all: false,
        dir: if docs.is_dir() { docs } else { home },
        entries: Vec::new(),
        selected: None,
        name: val("--name"),
        out,
        error: None,
        replace: None,
        first: true,
    };
    c.read();
    if let Err(e) = rustica_ui::run("Choose a file", 620, 480, None, |ui, ctl| c.frame(ui, ctl)) {
        eprintln!("chooser: {e}");
        std::process::exit(2);
    }
    std::process::exit(1); // the window closed: no
}

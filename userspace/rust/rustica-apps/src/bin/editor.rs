// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! editor: Rustica's Text Editor (DESKTOP.md step F7, the first of the
//! applications "next in line"): one UTF-8 text file in the toolkit's
//! text editor field.
//!
//!   editor [FILE]
//!
//! Ctrl+S saves (a new file asks for its name), Ctrl+Q quits (asking
//! first if there are changes). Prints "editor: ..." lines for the tests.

use rustica_ui::{Control, Ui};
use std::io::Write;
use std::path::PathBuf;

fn say(s: &str) {
    println!("editor: {s}");
    let _ = std::io::stdout().flush();
}

enum Dialog {
    Quit,
    Error(String),
}

struct Editor {
    path: Option<PathBuf>,
    text: String,
    saved: String,
    naming: Option<String>,
    dialog: Option<Dialog>,
    status: String,
}

impl Editor {
    fn save(&mut self) -> bool {
        let Some(p) = self.path.clone() else {
            self.naming = Some(String::new());
            return false;
        };
        match std::fs::write(&p, &self.text) {
            Ok(()) => {
                self.saved = self.text.clone();
                self.status = format!("Saved {}", p.display());
                say(&format!("saved {} ({} bytes)", p.display(), self.text.len()));
                true
            }
            Err(e) => {
                self.dialog = Some(Dialog::Error(format!("{}: {e}", p.display())));
                false
            }
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "New file".into());
        let changed = self.text != self.saved;
        ctl.requests.push(rustica_ui::Request::Title(format!("{}{name} — Text Editor", if changed { "• " } else { "" })));
        let menus: [(&str, &[&str]); 2] = [("File", &["Save", "Quit"]), ("Edit", &["Select all"])];
        let mut quit = false;
        if let Some((m, i)) = ui.menu_bar(&menus) {
            match (m, i) {
                (0, 0) => {
                    self.save();
                }
                (0, 1) => quit = true,
                _ => {}
            }
        }
        for k in &ui.input.keys {
            if k.ctrl() && k.usage == 0x16 {
                self.save();
            } else if k.ctrl() && k.usage == 0x14 {
                quit = true;
            }
        }
        if quit {
            if changed {
                self.dialog = Some(Dialog::Quit);
            } else {
                ctl.quit = true;
            }
        }
        let h = (ui.remaining_height() - ui.line_h() * 2.5) / ui.scale;
        if ui.text_area("text", &mut self.text, 0.0, h.max(60.0)) {
            self.status.clear();
        }
        ui.focus_if_none("text");
        if let Some(n) = self.naming.as_mut() {
            let mut n2 = std::mem::take(n);
            let (ok, cancel, enter) = ui.row(|ui| {
                ui.label("Save as:");
                let (_, e) = ui.text_field("name", &mut n2, 360.0);
                (ui.primary_button("Save"), ui.button("Cancel"), e)
            });
            if !ui.has_focus("name") && !cancel {
                ui.set_focus("name");
            }
            if (ok || enter) && !n2.trim().is_empty() {
                let p = PathBuf::from(n2.trim());
                self.path = Some(if p.is_relative() { std::env::current_dir().unwrap_or_default().join(p) } else { p });
                self.naming = None;
                self.save();
                ui.set_focus("text");
            } else if cancel {
                self.naming = None;
                ui.set_focus("text");
            } else {
                self.naming = Some(n2);
            }
        } else {
            let lines = self.text.lines().count();
            let st = if self.status.is_empty() { format!("{} lines, {} characters", lines, self.text.chars().count()) } else { self.status.clone() };
            ui.dim_label(&st);
        }
        match &self.dialog {
            Some(Dialog::Quit) => match ui.dialog("Save the changes?", &format!("{name} has changes that are not saved."), &["Save", "Don't save", "Cancel"]) {
                Some(0) => {
                    self.dialog = None;
                    if self.save() {
                        ctl.quit = true;
                    }
                }
                Some(1) => ctl.quit = true,
                Some(_) => self.dialog = None,
                None => {}
            },
            Some(Dialog::Error(e)) => {
                let e = e.clone();
                if ui.dialog("Could not save", &e, &["OK"]).is_some() {
                    self.dialog = None;
                }
            }
            None => {}
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);
    let text = match &path {
        Some(p) => match std::fs::read(p) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                eprintln!("editor: {}: {e}", p.display());
                std::process::exit(1);
            }
        },
        None => String::new(),
    };
    say(&format!("open {}", path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(new)".into())));
    let mut ed = Editor { path, saved: text.clone(), text, naming: None, dialog: None, status: String::new() };
    if let Err(e) = rustica_ui::run("Text Editor", 760, 540, None, |ui, ctl| ed.frame(ui, ctl)) {
        eprintln!("editor: {e}");
        std::process::exit(1);
    }
}

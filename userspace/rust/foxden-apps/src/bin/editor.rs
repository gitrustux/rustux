// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! editor: Rustica's Text Editor (DESKTOP.md step F7, the first of the
//! applications "next in line"; an app since FoxDen step FD6b,
//! org.rustica.texteditor): one UTF-8 text file in the toolkit's text
//! editor field.
//!
//!   editor [FILE]
//!
//! An app in its sandbox: Files opens a file with it (appctl open), which
//! puts that file alone in its view; Open and Save as ask the user with
//! the file chooser (<rustux/portal.h>) and get the file chosen, not its
//! folder. Run outside a sandbox (no chooser to ask), Save as asks for a
//! path instead.
//!
//! Ctrl+O opens, Ctrl+S saves (a new file asks where), Ctrl+Q quits
//! (asking first if there are changes). Prints "editor: ..." lines for
//! the tests.

use rustica_ui::{Control, Ui};
use std::ffi::{c_char, CStr, CString};
use std::io::{Read, Seek, Write};
use std::os::fd::FromRawFd;
use std::path::PathBuf;

fn say(s: &str) {
    println!("editor: {s}");
    let _ = std::io::stdout().flush();
}

extern "C" {
    fn rx_portal_open(title: *const c_char, patterns: *const c_char, name: *mut c_char, cap: usize) -> i32;
    fn rx_portal_save(title: *const c_char, suggested: *const c_char, name: *mut c_char, cap: usize) -> i32;
}

const ENOSYS: i32 = 38;
const ECANCELED: i32 = 125;
const PATTERNS: &str = "*.txt *.md *.conf *.cfg *.ini *.sh *.c *.h *.rs *.toml *.json *.csv *.log";

/// Ask the file chooser: the file and its name, or the error (-ECANCELED:
/// the user said no; -ENOSYS: not started by appctl)
fn portal(save: bool, title: &str, extra: &str) -> Result<(std::fs::File, String), i32> {
    let t = CString::new(title).unwrap();
    let x = CString::new(extra).unwrap();
    let mut name = [0 as c_char; 256];
    let fd = unsafe {
        if save {
            rx_portal_save(t.as_ptr(), x.as_ptr(), name.as_mut_ptr(), name.len())
        } else {
            rx_portal_open(t.as_ptr(), x.as_ptr(), name.as_mut_ptr(), name.len())
        }
    };
    if fd < 0 {
        return Err(-fd);
    }
    let n = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned();
    Ok((unsafe { std::fs::File::from_raw_fd(fd) }, n))
}

fn errno_text(e: i32) -> String {
    std::io::Error::from_raw_os_error(e).to_string()
}

/// Where the text goes
enum Target {
    /// A new file: Save asks where
    New,
    /// A path it may open (its argument, or typed outside a sandbox)
    Path(PathBuf),
    /// A file the chooser gave to write (Save as)
    Chosen(std::fs::File, String),
    /// A file the chooser gave to read (Open): saving asks again
    ReadOnly(String),
}

enum Dialog {
    /// Changes not saved, then: quit (false) or open another (true)
    Unsaved(bool),
    Error(String),
}

struct Editor {
    target: Target,
    text: String,
    saved: String,
    naming: Option<String>,
    dialog: Option<Dialog>,
    status: String,
}

impl Editor {
    fn name(&self) -> String {
        match &self.target {
            Target::New => "New file".into(),
            Target::Path(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            Target::Chosen(_, n) | Target::ReadOnly(n) => n.clone(),
        }
    }

    fn saved_ok(&mut self, what: String) -> bool {
        self.saved = self.text.clone();
        self.status = format!("Saved {what}");
        say(&format!("saved {what} ({} bytes)", self.text.len()));
        true
    }

    fn save(&mut self) -> bool {
        match &mut self.target {
            Target::Path(p) => {
                let p = p.clone();
                match std::fs::write(&p, &self.text) {
                    Ok(()) => self.saved_ok(p.display().to_string()),
                    Err(e) => {
                        self.dialog = Some(Dialog::Error(format!("{}: {e}", p.display())));
                        false
                    }
                }
            }
            Target::Chosen(f, n) => {
                let n = n.clone();
                let r = f.rewind().and_then(|_| f.write_all(self.text.as_bytes())).and_then(|_| f.set_len(self.text.len() as u64));
                match r {
                    Ok(()) => self.saved_ok(format!("{n} (chosen)")),
                    Err(e) => {
                        self.dialog = Some(Dialog::Error(format!("{n}: {e}")));
                        false
                    }
                }
            }
            Target::New | Target::ReadOnly(_) => self.save_as(),
        }
    }

    /// Ask where (the chooser; a typed path outside a sandbox)
    fn save_as(&mut self) -> bool {
        let suggest = match &self.target {
            Target::New => "Untitled.txt".to_string(),
            _ => self.name(),
        };
        match portal(true, "Save the text", &suggest) {
            Ok((f, n)) => {
                self.target = Target::Chosen(f, n);
                self.save()
            }
            Err(ENOSYS) => {
                self.naming = Some(String::new());
                false
            }
            Err(ECANCELED) => {
                self.status = "Not saved".into();
                false
            }
            Err(e) => {
                self.dialog = Some(Dialog::Error(format!("Save as: {}", errno_text(e))));
                false
            }
        }
    }

    /// Open another file, from the chooser
    fn open(&mut self) {
        match portal(false, "Open a text file", PATTERNS) {
            Ok((mut f, n)) => {
                let mut b = Vec::new();
                match f.read_to_end(&mut b) {
                    Ok(_) => {
                        self.text = String::from_utf8_lossy(&b).into_owned();
                        self.saved = self.text.clone();
                        self.status = format!("Opened {n}");
                        say(&format!("open {n} (chosen, {} bytes)", b.len()));
                        self.target = Target::ReadOnly(n);
                    }
                    Err(e) => self.dialog = Some(Dialog::Error(format!("{n}: {e}"))),
                }
            }
            Err(ECANCELED) => {}
            Err(ENOSYS) => self.status = "Open: start the Text Editor from Applications or Files".into(),
            Err(e) => self.dialog = Some(Dialog::Error(format!("Open: {}", errno_text(e)))),
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        let name = self.name();
        let changed = self.text != self.saved;
        ctl.requests.push(rustica_ui::Request::Title(format!("{}{name} — Text Editor", if changed { "• " } else { "" })));
        let menus: [(&str, &[&str]); 2] = [("File", &["Open…", "Save", "Save as…", "Quit"]), ("Edit", &["Select all"])];
        let (mut quit, mut open) = (false, false);
        if let Some((m, i)) = ui.menu_bar(&menus) {
            match (m, i) {
                (0, 0) => open = true,
                (0, 1) => {
                    self.save();
                }
                (0, 2) => {
                    self.save_as();
                }
                (0, 3) => quit = true,
                _ => {}
            }
        }
        for k in &ui.input.keys {
            if k.ctrl() && k.usage == 0x16 {
                self.save();
            } else if k.ctrl() && k.usage == 0x14 {
                quit = true;
            } else if k.ctrl() && k.usage == 0x12 {
                open = true;
            }
        }
        if quit || open {
            if changed {
                self.dialog = Some(Dialog::Unsaved(open));
            } else if open {
                self.open();
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
                self.target = Target::Path(if p.is_relative() { std::env::current_dir().unwrap_or_default().join(p) } else { p });
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
            Some(Dialog::Unsaved(open)) => {
                let open = *open;
                match ui.dialog("Save the changes?", &format!("{name} has changes that are not saved."), &["Save", "Don't save", "Cancel"]) {
                    Some(c @ (0 | 1)) => {
                        self.dialog = None;
                        if c == 1 || self.save() {
                            if open {
                                self.open();
                            } else {
                                ctl.quit = true;
                            }
                        }
                    }
                    Some(_) => self.dialog = None,
                    None => {}
                }
            }
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
    let target = path.map(Target::Path).unwrap_or(Target::New);
    let mut ed = Editor { target, saved: text.clone(), text, naming: None, dialog: None, status: String::new() };
    if let Err(e) = rustica_ui::run("Text Editor", 760, 540, None, |ui, ctl| ed.frame(ui, ctl)) {
        eprintln!("editor: {e}");
        std::process::exit(1);
    }
}

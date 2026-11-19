// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! launcher: the applications (DESKTOP.md step F6), from the panel's
//! Applications button. The list comes from /usr/share/applications/*.app
//! (lines "name=...", "exec=..." and an optional "comment=...") and the
//! apps installed in /apps (FoxDen step FD1: each one's manifest, Name,
//! Summary and Exec relative to its folder). Choosing one starts it and
//! closes the launcher; an app is started in its sandbox by `appctl run
//! ID` (step FD2).

use rustica_ui::{Control, Ui};

pub struct App {
    pub name: String,
    pub exec: String,
    pub comment: String,
}

fn apps() -> Vec<App> {
    let mut v = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/usr/share/applications") {
        for e in dir.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("app") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let mut a = App { name: String::new(), exec: String::new(), comment: String::new() };
            for line in text.lines() {
                if let Some((k, val)) = line.split_once('=') {
                    match k.trim() {
                        "name" => a.name = val.trim().into(),
                        "exec" => a.exec = val.trim().into(),
                        "comment" => a.comment = val.trim().into(),
                        _ => {}
                    }
                }
            }
            if !a.name.is_empty() && !a.exec.is_empty() {
                v.push(a);
            }
        }
    }
    let system = v.len();
    if let Ok(dir) = std::fs::read_dir("/apps") {
        for e in dir.flatten() {
            let folder = e.path();
            let Ok(text) = std::fs::read_to_string(folder.join("manifest")) else { continue };
            let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(':'))).map(|v| v.trim().to_string()).unwrap_or_default();
            let (name, exec) = (field("Name"), field("Exec"));
            // Exec is inside the folder: no absolute path, no ..
            if name.is_empty() || exec.is_empty() || exec.starts_with('/') || exec.split_whitespace().next().unwrap_or("").split('/').any(|c| c == "..") {
                continue;
            }
            // Started in its sandbox (FoxDen step FD2): appctl reads the
            // manifest again and runs Exec with the app's view and job
            let id = e.file_name().to_string_lossy().to_string();
            v.push(App { name, exec: format!("/bin/appctl run {id}"), comment: field("Summary") });
        }
    }
    println!("launcher: {} applications, {} apps", system, v.len() - system);
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn start(a: &App) {
    let mut parts = a.exec.split_whitespace();
    if let Some(prog) = parts.next() {
        match std::process::Command::new(prog).args(parts).spawn() {
            Ok(_) => println!("launcher: started {}", a.exec),
            Err(e) => eprintln!("launcher: {}: {e}", a.exec),
        }
    }
}

fn main() {
    let list = apps();
    let names: Vec<String> = list.iter().map(|a| a.name.clone()).collect();
    let mut sel: Option<usize> = if list.is_empty() { None } else { Some(0) };
    let mut first = true;
    let r = rustica_ui::run("Applications", 340, 360, Some((12, 60)), |ui: &mut Ui, ctl: &mut Control| {
        if first {
            ui.set_focus("apps");
            first = false;
        }
        ui.heading("Applications");
        ui.list("apps", &names, &mut sel, 200.0);
        if let Some(i) = sel {
            let c = list[i].comment.clone();
            ui.dim_label(if c.is_empty() { " " } else { &c });
        }
        let (open, close) = ui.row(|ui| (ui.primary_button("Open"), ui.button("Close")));
        let enter = ui.input.keys.iter().any(|k| k.usage == 0x28);
        if (open || enter) && sel.is_some() {
            start(&list[sel.unwrap()]);
            ctl.quit = true;
        }
        if close || ui.input.keys.iter().any(|k| k.usage == 0x29) {
            ctl.quit = true;
        }
    });
    if let Err(e) = r {
        eprintln!("launcher: {e}");
        std::process::exit(1);
    }
}

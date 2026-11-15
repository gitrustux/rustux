// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! greeter: the graphical login (DESKTOP.md step F6). Full screen, a name
//! and a password; desktop --run starts it (as root) and checks them: the
//! greeter writes "login\tNAME\tPASSWORD" on its stdout and reads "ok" or
//! "fail" on its stdin. It ends when someone has logged in.

use rustica_ui::{Control, Request, Ui};
use std::io::{BufRead, Write};

struct Greeter {
    name: String,
    password: String,
    message: String,
    first: bool,
    host: String,
}

fn check(name: &str, password: &str) -> bool {
    let mut out = std::io::stdout();
    if writeln!(out, "login\t{name}\t{password}").is_err() || out.flush().is_err() {
        return false;
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim() == "ok"
}

fn frame(g: &mut Greeter, ui: &mut Ui, ctl: &mut Control) {
    if g.first {
        ctl.requests.push(Request::Fullscreen(true));
        ui.set_focus("name");
        g.first = false;
    }
    // The form, in the middle of the screen
    let (w, h) = (ui.canvas.width(), ui.canvas.height());
    let fw = ui.u(360.0);
    ui.space(((h / ui.scale) / 2.0 - 190.0).max(0.0));
    ui.row(|ui| {
        ui.alloc(((w - fw) / 2.0 - ui.u(12.0)).max(0.0), 1.0);
        ui.column(fw, |ui| {
            ui.heading("Rustica");
            ui.dim_label(&format!("Log in to {}", g.host));
            ui.space(10.0);
            ui.label("Name");
            let (_, e1) = ui.text_field("name", &mut g.name, 360.0);
            ui.label("Password");
            let (_, e2) = ui.password_field("password", &mut g.password, 360.0);
            ui.space(4.0);
            let go = ui.primary_button("Log in");
            if (go || e1 || e2) && !g.name.is_empty() {
                if check(&g.name, &g.password) {
                    ctl.quit = true;
                } else {
                    g.message = "Wrong name or password".into();
                    g.password.clear();
                    ui.set_focus("password");
                }
            }
            if !g.message.is_empty() {
                let m = g.message.clone();
                ui.dim_label(&m);
            }
        });
    });
}

fn main() {
    let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "rustica".into());
    let mut g = Greeter { name: String::new(), password: String::new(), message: String::new(), first: true, host };
    if let Err(e) = rustica_ui::run("Log in", 640, 480, None, |ui, ctl| frame(&mut g, ui, ctl)) {
        eprintln!("greeter: {e}");
        std::process::exit(1);
    }
}

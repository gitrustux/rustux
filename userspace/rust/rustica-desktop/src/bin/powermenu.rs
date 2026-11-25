// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! powermenu: the panel's power button (DESKTOP.md step F15), at the far
//! end of the panel: Lock screen, Log out, Suspend, Hibernate, Restart,
//! Shut down, and Power settings. Lock and Log out are the panel's: the
//! menu ends with status 11 or 10 for them. Restart and Shut down ask init
//! (`reboot`, `poweroff`), which lets the desktop session's user do it.
//! Suspend and Hibernate need the ACPI interpreter (CHECKLIST.md, phase P)
//! and are shown as not there yet.
//!
//!   powermenu [X Y]      where its window goes (-2: against the right or
//!                        bottom edge of the work area)

use rustica_ui::ui::keys;
use rustica_ui::{Control, Ui};

fn main() {
    let a: Vec<i32> = std::env::args().skip(1).filter_map(|v| v.parse().ok()).collect();
    let at = if a.len() == 2 { (a[0], a[1]) } else { (-2, -2) };
    let mut code = 0;
    let mut message = String::new();
    let r = rustica_ui::run("Power", 250, 296, Some(at), |ui: &mut Ui, ctl: &mut Control| {
        if ui.input.keys.iter().any(|k| k.usage == keys::ESC) {
            ctl.quit = true;
        }
        let w = ui.available_width();
        let run = |ui: &mut Ui, label: &str| ui.button_ex(label, label, w, false, false).clicked;
        if run(ui, "Lock screen") {
            println!("powermenu: lock");
            code = 11;
            ctl.quit = true;
        }
        if run(ui, "Log out") {
            println!("powermenu: log out");
            code = 10;
            ctl.quit = true;
        }
        for (what, cmd) in [("Restart", "/bin/reboot"), ("Shut down", "/bin/poweroff")] {
            if run(ui, what) {
                println!("powermenu: {}", what.to_lowercase());
                match std::process::Command::new(cmd).output() {
                    // It does not come back when it works
                    Ok(o) => {
                        let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        message = if e.is_empty() { format!("{what} failed") } else { e };
                    }
                    Err(e) => message = format!("{cmd}: {e}"),
                }
            }
        }
        ui.dim_label("Suspend, Hibernate: not yet");
        if run(ui, "Power settings…") {
            let _ = std::process::Command::new("/bin/settings").arg("power").spawn();
            ctl.quit = true;
        }
        if !message.is_empty() {
            ui.dim_label(&message);
        }
    });
    if let Err(e) = r {
        eprintln!("powermenu: {e}");
        std::process::exit(1);
    }
    std::process::exit(code);
}

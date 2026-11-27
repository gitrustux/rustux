// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! popmenu: a small menu at a place on the screen, for the panel's right
//! clicks (2026-11-27: Pin to taskbar, Unpin from taskbar). It ends with
//! the number of the item chosen (1 for the first), or 0 (Esc, or a click
//! elsewhere, which takes its focus).
//!
//!   popmenu X Y ITEM...      X or Y -2: against the work area's right or
//!                            bottom edge

use rustica_ui::ui::keys;
use rustica_ui::{Control, Ui};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: popmenu X Y ITEM...");
        std::process::exit(2);
    }
    let at = (args[0].parse().unwrap_or(-2), args[1].parse().unwrap_or(-2));
    let items: Vec<String> = args[2..].to_vec();
    let w = items.iter().map(|i| i.chars().count()).max().unwrap_or(10) as u32 * 9 + 60;
    let h = items.len() as u32 * 40 + 12;
    let mut code = 0;
    let mut had_focus = false;
    let r = rustica_ui::run("Menu", w.max(160), h, Some(at), |ui: &mut Ui, ctl: &mut Control| {
        if ui.input.keys.iter().any(|k| k.usage == keys::ESC) {
            ctl.quit = true;
        }
        // A click elsewhere: the menu loses the focus and goes
        for e in &ctl.events {
            if let rustica_ui::rustica_window::Event::Focus(f) = e {
                if *f {
                    had_focus = true;
                } else if had_focus {
                    ctl.quit = true;
                }
            }
        }
        let bw = ui.available_width();
        for (i, item) in items.iter().enumerate() {
            if ui.button_ex(&format!("item{i}"), item, bw, false, false).clicked {
                code = i as i32 + 1;
                ctl.quit = true;
            }
        }
    });
    if let Err(e) = r {
        eprintln!("popmenu: {e}");
        std::process::exit(0);
    }
    std::process::exit(code);
}

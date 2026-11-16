// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! panel: the desktop's panel (DESKTOP.md step F6), docked at the bottom
//! of the screen: Applications (the launcher), a button per window (click:
//! focus it, or minimise it if it has the focus), the keyboard layout,
//! volume (not there yet), the network (Net: the interface and its
//! address, network step D6; a click opens Settings > Network), the
//! clock, Lock and Log out. The session lasts as long as the panel: Log
//! out ends it.
//!
//! It also starts the programs in /etc/autostart (one command a line).

use rustica_ui::rustica_window::{Event, ROLE_PANEL};
use rustica_ui::{Control, Request, Ui};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Default)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
    _rest: [i64; 4],
}

extern "C" {
    fn time(t: *mut i64) -> i64;
    fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
}

fn clock() -> String {
    let mut tm = Tm::default();
    unsafe {
        let t = time(std::ptr::null_mut());
        localtime_r(&t, &mut tm);
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{} {:02}:{:02}", DAYS[(tm.wday.rem_euclid(7)) as usize], tm.hour, tm.min)
}

struct Win {
    id: i32,
    title: String,
    focused: bool,
    minimized: bool,
}

/// The keyboard layout (/etc/keymap; Settings or localectl change it)
fn layout() -> String {
    let l = std::fs::read_to_string("/etc/keymap").map(|s| s.trim().to_string()).unwrap_or_default();
    if l.is_empty() { "us".to_string() } else { l }
}

fn main() {
    let user = std::env::var("USER").unwrap_or_default();
    if let Ok(list) = std::fs::read_to_string("/etc/autostart") {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut p = line.split_whitespace();
            if let Some(prog) = p.next() {
                let _ = std::process::Command::new(prog).args(p).spawn();
            }
        }
    }
    let mut wins: Vec<Win> = Vec::new();
    let mut first = true;
    // The network's state, asked again every 3 seconds
    let mut net = String::new();
    let mut net_at: Option<Instant> = None;
    println!("panel: session of {user}");
    let r = rustica_ui::run("Panel", 800, 36, Some((0, 0)), |ui: &mut Ui, ctl: &mut Control| {
        if first {
            ui.padding = 3.0;
            ctl.requests.push(Request::SetRole(ROLE_PANEL));
            first = false;
        }
        ctl.wake_after = Some(Duration::from_secs(5));
        for e in ctl.events.drain(..) {
            if let Event::WindowInfo { id, title, focused, minimized } = e {
                if id < 0 {
                    wins.clear();
                } else {
                    wins.push(Win { id, title, focused, minimized });
                }
            }
        }
        let th = ui.theme;
        let w = ui.canvas.width();
        let h = ui.canvas.height();
        ui.canvas.fill_rect(rustica_ui::gfx::Rect::new(0.0, 0.0, w, h), th.surface);
        ui.canvas.fill_rect(rustica_ui::gfx::Rect::new(0.0, 0.0, w, 1.0), th.border);
        if net_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(3)) {
            let s = format!("Net: {}", rustica_ui::net::summary(rustica_ui::net::interfaces().as_deref()));
            if s != net {
                println!("panel: {s}");
                net = s;
            }
            net_at = Some(Instant::now());
        }
        let left_text = format!("{}    Vol \u{2014}", layout());
        let right_text = clock();
        let right_w = ui.text_width(&left_text)
            + ui.text_width(&net)
            + ui.text_width(&right_text)
            + ui.text_width("Lock")
            + ui.text_width("Log out")
            + ui.u(32.0 * 3.0 + 8.0 * 6.0 + 16.0);
        ui.row(|ui| {
            if ui.primary_button("Applications") {
                println!("panel: launcher");
                let _ = std::process::Command::new("/bin/launcher").spawn();
            }
            for win in &wins {
                let mut t = win.title.clone();
                if win.minimized {
                    t = format!("({t})");
                }
                ui.push_id(&win.id.to_string());
                let clicked = if win.focused { ui.primary_button(&t) } else { ui.button(&t) };
                ui.pop_id();
                if clicked {
                    println!("panel: activate {}", win.title);
                    ctl.requests.push(Request::Activate(win.id));
                }
            }
            let left = ui.available_width() - right_w;
            if left > 0.0 {
                ui.alloc(left, 1.0);
            }
            // The status text, centred on the buttons' line
            let status = |ui: &mut Ui, text: &str| {
                let tw = ui.text_width(text);
                let bh = ui.line_h() + ui.u(12.0);
                let r = ui.alloc(tw, bh);
                let px = ui.font_px();
                let ty = r.y + (bh - ui.line_h()) / 2.0;
                ui.canvas.text(&mut ui.face, r.x, ty, text, px, th.dim);
            };
            status(ui, &left_text);
            if ui.button(&net) {
                println!("panel: network settings");
                let _ = std::process::Command::new("/bin/settings").arg("network").spawn();
            }
            status(ui, &right_text);
            if ui.button("Lock") {
                println!("panel: lock");
                ctl.requests.push(Request::Lock);
            }
            if ui.button("Log out") {
                println!("panel: log out");
                ctl.quit = true;
            }
        });
    });
    if let Err(e) = r {
        eprintln!("panel: {e}");
        std::process::exit(1);
    }
}

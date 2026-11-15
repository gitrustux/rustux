// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! settings: Rustica's Settings (DESKTOP.md step F7). Display (the scale,
//! ~/.config/rustica.conf), keyboard layout (localectl), mouse speed
//! (inputctl), date, time and time zone (timedatectl), users (useradd,
//! userdel, chpasswd, usermod), disks (lsblk, read only), about.
//!
//!   settings [SECTION]     display, keyboard, mouse, time, users, disks, about
//!
//! What is the system's (in /etc) is root's: another user is asked for
//! their password and the change goes through sudo -S. Prints what it
//! changes ("settings: ...") for the tests.

use rustica_apps::sys;
use rustica_ui::{Column, Control, Icon, Row, Ui};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn say(s: &str) {
    println!("settings: {s}");
    let _ = std::io::stdout().flush();
}

const SECTIONS: [&str; 7] = ["Display", "Keyboard", "Mouse", "Date & Time", "Users", "Disks", "About"];

/// A change that needs root, waiting for the password
struct Pending {
    what: String,
    cmds: Vec<(Vec<String>, String)>, // argv, stdin
}

struct Settings {
    section: Option<usize>,
    scale: u32,
    keymaps: Vec<String>,
    keymap: Option<usize>,
    keymap_now: String,
    speed: u32,
    zones: Vec<String>,
    zone: Option<usize>,
    zone_now: String,
    zone_filter: String,
    date_text: String,
    users: Vec<(String, u32, String, bool)>, // name, uid, home, admin
    user_sel: Option<usize>,
    new_name: String,
    new_pw: String,
    new_admin: bool,
    disks: Vec<Row>,
    disk_sel: Option<usize>,
    about: Vec<(String, String)>,
    password: Option<String>, // the user's, once sudo took it
    pending: Option<Pending>,
    pw_text: String,
    message: Option<(String, String)>,
    confirm_delete: Option<String>,
    status: String,
}

fn read_first_line(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default().lines().next().unwrap_or("").trim().to_string()
}

impl Settings {
    fn new() -> Settings {
        let mut s = Settings {
            section: Some(0),
            scale: rustica_ui::app::scale_from_env() as u32,
            keymaps: Vec::new(),
            keymap: None,
            keymap_now: String::new(),
            speed: 100,
            zones: Vec::new(),
            zone: None,
            zone_now: String::new(),
            zone_filter: String::new(),
            date_text: String::new(),
            users: Vec::new(),
            user_sel: None,
            new_name: String::new(),
            new_pw: String::new(),
            new_admin: false,
            disks: Vec::new(),
            disk_sel: None,
            about: Vec::new(),
            password: None,
            pending: None,
            pw_text: String::new(),
            message: None,
            confirm_delete: None,
            status: String::new(),
        };
        s.reload();
        s
    }

    fn reload(&mut self) {
        self.keymaps = sys::run("/bin/localectl", &["list-keymaps"]).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        self.keymaps.sort();
        self.keymaps.dedup();
        self.keymap_now = read_first_line("/etc/keymap");
        if self.keymap_now.is_empty() {
            self.keymap_now = "us".into();
        }
        self.keymap = self.keymaps.iter().position(|k| *k == self.keymap_now);
        self.speed = sys::run("/bin/inputctl", &[]).ok().and_then(|o| o.split(':').nth(1).and_then(|v| v.trim().trim_end_matches('%').parse().ok())).unwrap_or(100);
        self.zones = sys::run("/bin/timedatectl", &["list-timezones"]).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        self.zone_now = read_first_line("/etc/timezone");
        if self.zone_now.is_empty() {
            self.zone_now = "UTC".into();
        }
        self.zone = self.filtered_zones().iter().position(|z| *z == self.zone_now);
        let tm = sys::local(sys::now());
        self.date_text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.year + 1900, tm.mon + 1, tm.mday, tm.hour, tm.min, tm.sec);
        self.load_users();
        self.load_disks();
        self.load_about();
    }

    fn filtered_zones(&self) -> Vec<String> {
        let f = self.zone_filter.to_lowercase();
        self.zones.iter().filter(|z| f.is_empty() || z.to_lowercase().contains(&f)).cloned().collect()
    }

    fn load_users(&mut self) {
        let admins: Vec<String> = std::fs::read_to_string("/etc/group")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("sudo:"))
            .map(|l| l.rsplit(':').next().unwrap_or("").split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();
        self.users = std::fs::read_to_string("/etc/passwd")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                let uid: u32 = f.get(2)?.parse().ok()?;
                (uid == 0 || (1000..60000).contains(&uid)).then(|| (f[0].to_string(), uid, f.get(5).unwrap_or(&"").to_string(), uid == 0 || admins.iter().any(|a| a == f[0])))
            })
            .collect();
    }

    fn load_disks(&mut self) {
        let out = sys::run("/bin/lsblk", &["-l", "-n", "-o", "NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS"]).unwrap_or_default();
        self.disks = out
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let disk = f.get(2) == Some(&"disk");
                // FSTYPE may be empty: a 4th field that starts with / is a mount point
                let (fs, mp) = match (f.get(3), f.get(4)) {
                    (Some(a), _) if a.starts_with('/') => (String::new(), a.to_string()),
                    (Some(a), Some(b)) => (a.to_string(), b.to_string()),
                    (Some(a), None) => (a.to_string(), String::new()),
                    _ => (String::new(), String::new()),
                };
                Row {
                    icon: if disk { Icon::Drive } else { Icon::None },
                    cells: vec![
                        if disk { f.first().unwrap_or(&"").to_string() } else { format!("   {}", f.first().unwrap_or(&"")) },
                        f.get(1).unwrap_or(&"").to_string(),
                        if disk { "disk".into() } else { "partition".into() },
                        fs,
                        mp,
                    ],
                }
            })
            .collect();
    }

    fn load_about(&mut self) {
        let os = sys::parse_kv(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
        let unq = |s: &str| s.trim_matches('"').to_string();
        let mut a = Vec::new();
        a.push(("Operating system".into(), unq(os.get("PRETTY_NAME").map(|s| s.as_str()).unwrap_or("Rustica"))));
        a.push(("Codename".into(), unq(os.get("VERSION_CODENAME").map(|s| s.as_str()).unwrap_or("redfox"))));
        let kernel = sys::run("/bin/uname", &["-srv"]).or_else(|_| sys::run("/bin/coreutils", &["uname", "-srv"])).unwrap_or_default();
        a.push(("Kernel".into(), kernel.trim().to_string()));
        a.push(("Computer name".into(), read_first_line("/etc/hostname")));
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        a.push(("Processors".into(), format!("{cpus} × x86-64")));
        // {"total":N,"used":N,...}
        let json_num = |text: &str, k: &str| -> Option<u64> {
            let i = text.find(&format!("\"{k}\":"))? + k.len() + 3;
            text[i..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        };
        if let Ok(free) = sys::run("/bin/free", &["--json"]) {
            if let (Some(t), Some(u)) = (json_num(&free, "total"), json_num(&free, "used")) {
                a.push(("Memory".into(), format!("{} ({} in use)", sys::human_size(t), sys::human_size(u))));
            }
        }
        let gpu = sys::run("/bin/lspci", &[]).unwrap_or_default().lines().find(|l| l.to_lowercase().contains("vga") || l.to_lowercase().contains("display")).map(|l| l.trim().to_string());
        if let Some(g) = gpu {
            a.push(("Graphics".into(), g));
        }
        if let Some(ns) = sys::run("/bin/uptime", &["--json"]).ok().and_then(|u| json_num(&u, "uptime_ns")) {
            let m = ns / 60_000_000_000;
            a.push(("Running for".into(), if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }));
        }
        self.about = a;
    }

    /// Run commands as root: now if we are, else after the password
    fn admin(&mut self, what: &str, cmds: Vec<(Vec<String>, String)>) {
        let p = Pending { what: what.to_string(), cmds };
        if sys::euid() == 0 || self.password.is_some() {
            self.run_admin(p);
        } else {
            self.pw_text.clear();
            self.pending = Some(p);
        }
    }

    fn run_admin(&mut self, p: Pending) {
        let root = sys::euid() == 0;
        for (argv, input) in &p.cmds {
            let mut c = if root {
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            } else {
                let mut c = Command::new("/bin/sudo");
                c.arg("-S").args(argv);
                c
            };
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let r = c.spawn().and_then(|mut ch| {
                if let Some(mut si) = ch.stdin.take() {
                    if !root {
                        let _ = writeln!(si, "{}", self.password.clone().unwrap_or_default());
                    }
                    let _ = si.write_all(input.as_bytes());
                }
                ch.wait_with_output()
            });
            match r {
                Ok(o) if o.status.success() => {}
                Ok(o) => {
                    let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    if !root && (e.contains("password") || e.contains("not allowed") || e.contains("sudoers")) {
                        self.password = None;
                    }
                    self.message = Some((format!("Could not {}", p.what), if e.is_empty() { format!("{} failed", argv[0]) } else { e }));
                    self.reload();
                    return;
                }
                Err(e) => {
                    self.message = Some((format!("Could not {}", p.what), e.to_string()));
                    return;
                }
            }
        }
        say(&p.what);
        self.status = format!("Done: {}", p.what);
        self.reload();
    }

    // ---- sections ----

    fn display(&mut self, ui: &mut Ui) {
        ui.heading("Display");
        ui.dim_label("The size of everything in windows; it applies to windows opened from now on.");
        ui.space(6.0);
        let mut chosen = None;
        ui.row(|ui| {
            for s in 1..=2u32 {
                let label = if s == 1 { "Normal (100%)" } else { "Large (200%)" };
                let pressed = if self.scale == s { ui.primary_button(label) } else { ui.button(label) };
                if pressed {
                    chosen = Some(s);
                }
            }
        });
        if let Some(s) = chosen {
            if s != self.scale {
                let p = sys::config_dir().join("rustica.conf");
                match sys::write_kv(&p, "scale", &s.to_string()) {
                    Ok(()) => {
                        self.scale = s;
                        say(&format!("scale {s}"));
                        self.status = format!("Scale {}% for new windows", s * 100);
                    }
                    Err(e) => self.message = Some(("Could not save".into(), format!("{}: {e}", p.display()))),
                }
            }
        }
        ui.space(8.0);
        ui.dim_label(&format!("Your settings: {}", sys::config_dir().join("rustica.conf").display()));
    }

    fn keyboard(&mut self, ui: &mut Ui) {
        ui.heading("Keyboard");
        ui.label(&format!("Layout: {}", self.keymap_now));
        ui.dim_label("For the whole computer, the console too (/etc/keymap).");
        let h = (ui.remaining_height() - ui.line_h() * 4.0) / ui.scale;
        ui.list("keymaps", &self.keymaps, &mut self.keymap, h.clamp(60.0, 300.0));
        if ui.primary_button("Use this layout") {
            if let Some(k) = self.keymap.and_then(|i| self.keymaps.get(i)).cloned() {
                self.admin(&format!("keymap {k}"), vec![(vec!["/bin/localectl".into(), "set-keymap".into(), k], String::new())]);
            }
        }
    }

    fn mouse(&mut self, ui: &mut Ui) {
        ui.heading("Mouse");
        ui.label(&format!("Pointer speed: {}%", self.speed));
        ui.dim_label("How far the pointer moves for a movement of the mouse (/etc/input.conf).");
        ui.space(4.0);
        let steps = [25u32, 50, 75, 100, 150, 200, 300, 400];
        let mut chosen = None;
        ui.row(|ui| {
            for s in steps {
                let l = format!("{s}%");
                if if self.speed == s { ui.primary_button(&l) } else { ui.button(&l) } {
                    chosen = Some(s);
                }
            }
        });
        if let Some(s) = chosen {
            self.admin(&format!("pointer speed {s}"), vec![(vec!["/bin/inputctl".into(), "set-pointer-speed".into(), s.to_string()], String::new())]);
        }
    }

    fn time(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ctl.wake_after = Some(Duration::from_secs(1));
        ui.heading("Date & Time");
        let tm = sys::local(sys::now());
        ui.label(&format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}   ({})", tm.year + 1900, tm.mon + 1, tm.mday, tm.hour, tm.min, tm.sec, self.zone_now));
        ui.space(4.0);
        let (set, enter) = ui.row(|ui| {
            ui.label("Set to");
            let (_, e) = ui.text_field("date", &mut self.date_text, 220.0);
            (ui.button("Set the clock"), e)
        });
        if set || enter {
            let t = self.date_text.trim().to_string();
            self.admin(&format!("time {t}"), vec![(vec!["/bin/timedatectl".into(), "set-time".into(), t], String::new())]);
        }
        ui.space(4.0);
        ui.label("Time zone");
        let (changed, _) = ui.row(|ui| {
            ui.label("Find");
            ui.text_field("zfilter", &mut self.zone_filter, 220.0)
        });
        let zones = self.filtered_zones();
        if changed {
            self.zone = zones.iter().position(|z| *z == self.zone_now);
        }
        let h = (ui.remaining_height() - ui.line_h() * 4.0) / ui.scale;
        ui.list("zones", &zones, &mut self.zone, h.clamp(60.0, 400.0));
        if ui.primary_button("Use this time zone") {
            if let Some(z) = self.zone.and_then(|i| zones.get(i)).cloned() {
                self.admin(&format!("timezone {z}"), vec![(vec!["/bin/timedatectl".into(), "set-timezone".into(), z], String::new())]);
            }
        }
    }

    fn users_section(&mut self, ui: &mut Ui) {
        ui.heading("Users");
        let rows: Vec<Row> = self
            .users
            .iter()
            .map(|(n, uid, home, admin)| Row { icon: Icon::None, cells: vec![n.clone(), uid.to_string(), home.clone(), if *admin { "administrator".into() } else { "user".into() }] })
            .collect();
        let cols = [
            Column { title: "Name", width: 0.0, right: false },
            Column { title: "Id", width: 70.0, right: true },
            Column { title: "Home", width: 170.0, right: false },
            Column { title: "Kind", width: 120.0, right: false },
        ];
        let h = (ui.remaining_height() - ui.line_h() * 13.0) / ui.scale;
        ui.table("users", &cols, &rows, &mut self.user_sel, h.max(80.0));
        let sel = self.user_sel.and_then(|i| self.users.get(i)).cloned();
        if let Some((name, uid, _, admin)) = &sel {
            let (rm, toggle) = ui.row(|ui| {
                let rm = *uid != 0 && ui.button(&format!("Remove {name}"));
                let toggle = *uid != 0 && ui.button(if *admin { "Make a normal user" } else { "Make an administrator" });
                (rm, toggle)
            });
            if rm {
                self.confirm_delete = Some(name.clone());
            }
            if toggle {
                let groups = if *admin {
                    // Every extra group but sudo
                    let g: Vec<String> = std::fs::read_to_string("/etc/group")
                        .unwrap_or_default()
                        .lines()
                        .filter(|l| !l.starts_with("sudo:"))
                        .filter(|l| l.rsplit(':').next().unwrap_or("").split(',').any(|m| m == name))
                        .map(|l| l.split(':').next().unwrap_or("").to_string())
                        .collect();
                    g.join(",")
                } else {
                    String::new()
                };
                let argv: Vec<String> = if *admin {
                    vec!["/bin/usermod".into(), "-G".into(), groups, name.clone()]
                } else {
                    vec!["/bin/usermod".into(), "-a".into(), "-G".into(), "sudo".into(), name.clone()]
                };
                self.admin(&format!("{} {name}", if *admin { "unadmin" } else { "admin" }), vec![(argv, String::new())]);
            }
        }
        ui.space(4.0);
        ui.label("A new user");
        let (_, _) = ui.row(|ui| {
            ui.label("Name");
            ui.text_field("uname", &mut self.new_name, 160.0)
        });
        let (_, enter) = ui.row(|ui| {
            ui.label("Password");
            ui.password_field("upw", &mut self.new_pw, 160.0)
        });
        ui.checkbox(&mut self.new_admin, "Administrator (may use sudo)");
        if ui.primary_button("Add the user") || enter {
            let name = self.new_name.trim().to_string();
            let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false);
            if !valid {
                self.message = Some(("Not a valid name".into(), "Lower-case letters, digits, - and _, starting with a letter".into()));
            } else if self.new_pw.is_empty() {
                self.message = Some(("No password".into(), "Give the new user a password".into()));
            } else {
                let mut add: Vec<String> = vec!["/bin/useradd".into(), "-m".into()];
                if self.new_admin {
                    add.push("-G".into());
                    add.push("sudo".into());
                }
                add.push(name.clone());
                let pw = format!("{name}:{}\n", self.new_pw);
                self.admin(&format!("useradd {name}"), vec![(add, String::new()), (vec!["/bin/chpasswd".into()], pw)]);
                self.new_name.clear();
                self.new_pw.clear();
                self.new_admin = false;
            }
        }
    }

    fn disks_section(&mut self, ui: &mut Ui) {
        ui.heading("Disks");
        ui.dim_label("The disks and their partitions, as lsblk shows them (Files mounts USB sticks).");
        let cols = [
            Column { title: "Name", width: 0.0, right: false },
            Column { title: "Size", width: 80.0, right: true },
            Column { title: "Kind", width: 90.0, right: false },
            Column { title: "Filesystem", width: 100.0, right: false },
            Column { title: "Mounted at", width: 140.0, right: false },
        ];
        let rows = self.disks.clone();
        ui.table("disks", &cols, &rows, &mut self.disk_sel, 0.0);
    }

    fn about_section(&mut self, ui: &mut Ui) {
        ui.heading("About");
        for (k, v) in self.about.clone() {
            ui.row(|ui| {
                ui.column(ui.u(150.0), |ui| ui.dim_label(&k));
                ui.column(0.0, |ui| ui.label(&v));
            });
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        let w = ui.available_width();
        let names: Vec<String> = SECTIONS.iter().map(|s| s.to_string()).collect();
        let before = self.section;
        ui.row(|ui| {
            ui.column(ui.u(170.0).min(w * 0.3), |ui| {
                let h = (ui.remaining_height() - ui.line_h() * 2.0) / ui.scale;
                ui.list("sections", &names, &mut self.section, h);
            });
            ui.column(0.0, |ui| match self.section.unwrap_or(0) {
                0 => self.display(ui),
                1 => self.keyboard(ui),
                2 => self.mouse(ui),
                3 => self.time(ui, ctl),
                4 => self.users_section(ui),
                5 => self.disks_section(ui),
                _ => self.about_section(ui),
            });
        });
        if self.section != before {
            self.status.clear();
            if let Some(s) = self.section {
                say(&format!("section {}", SECTIONS[s]));
            }
            if self.section == Some(5) || self.section == Some(6) {
                self.load_disks();
                self.load_about();
            }
        }
        ui.focus_if_none("sections");
        if !self.status.is_empty() {
            ui.dim_label(&self.status.clone());
        }
        ctl.requests.push(rustica_ui::Request::Title(format!("{} — Settings", SECTIONS[self.section.unwrap_or(0)])));
        self.dialogs(ui);
    }

    fn dialogs(&mut self, ui: &mut Ui) {
        if self.pending.is_some() {
            // The password, in a bar over the bottom of the window
            let what = self.pending.as_ref().unwrap().what.clone();
            ui.separator();
            ui.label(&format!("Your password, to change the system ({what}):"));
            let (ok, cancel, enter) = ui.row(|ui| {
                let (_, e) = ui.password_field("sudo", &mut self.pw_text, 220.0);
                (ui.primary_button("OK"), ui.button("Cancel"), e)
            });
            if !ui.has_focus("sudo") && !cancel {
                ui.set_focus("sudo");
            }
            if ok || enter {
                self.password = Some(std::mem::take(&mut self.pw_text));
                let p = self.pending.take().unwrap();
                self.run_admin(p);
            } else if cancel {
                self.pending = None;
            }
            return;
        }
        if let Some(name) = self.confirm_delete.clone() {
            match ui.dialog("Remove the user?", &format!("{name} and their home folder will be deleted."), &["Remove", "Cancel"]) {
                Some(0) => {
                    self.confirm_delete = None;
                    self.user_sel = None;
                    self.admin(&format!("userdel {name}"), vec![(vec!["/bin/userdel".into(), "-r".into(), name], String::new())]);
                }
                Some(_) => self.confirm_delete = None,
                None => {}
            }
            return;
        }
        if let Some((t, m)) = self.message.clone() {
            if ui.dialog(&t, &m, &["OK"]).is_some() {
                self.message = None;
            }
        }
    }
}

fn main() {
    let mut s = Settings::new();
    if let Some(a) = std::env::args().nth(1) {
        let a = a.to_lowercase();
        s.section = SECTIONS.iter().position(|n| n.to_lowercase().contains(&a)).or(Some(0));
    }
    say("started");
    if let Err(e) = rustica_ui::run("Settings", 820, 560, None, |ui, ctl| s.frame(ui, ctl)) {
        eprintln!("settings: {e}");
        std::process::exit(1);
    }
}

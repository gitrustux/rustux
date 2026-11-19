// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! settings: Rustica's Settings (DESKTOP.md step F7). Display (the scale,
//! ~/.config/rustica.conf), keyboard layout (localectl), mouse speed
//! (inputctl), network (the interfaces, DHCP or a fixed address, DNS
//! servers: netcfg; network step D6), date, time and time zone
//! (timedatectl), users (useradd, userdel, chpasswd, usermod), disks
//! (lsblk, read only), updates (the installer service /svc/packages,
//! through the updates command: what the last check found, Check now,
//! Install updates, a restart for a new system image; package step E6),
//! about.
//!
//!   settings [SECTION]     display, keyboard, mouse, network, time, users, disks, updates, about
//!
//! What is the system's (in /etc) is root's: another user is asked for
//! their password and the change goes through sudo -S. Prints what it
//! changes ("settings: ...") for the tests.

use rustica_apps::sys;
use rustica_ui::net::Iface;
use rustica_ui::{Column, Control, Icon, Row, Ui};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn say(s: &str) {
    println!("settings: {s}");
    let _ = std::io::stdout().flush();
}

const SECTIONS: [&str; 9] = ["Display", "Keyboard", "Mouse", "Network", "Date & Time", "Users", "Disks", "Updates", "About"];
const S_DISKS: usize = 6;
const S_UPDATES: usize = 7;
const S_ABOUT: usize = 8;

/// The installer service's last check (/var/lib/rpg/updates)
#[derive(Default, Clone)]
struct UpStatus {
    checked: i64,
    error: String,
    steps: Vec<(String, String, String, u64, String)>, // name, from ("" new), to, size, why
    kept: Vec<String>,
}

fn up_status() -> UpStatus {
    let mut u = UpStatus::default();
    for l in std::fs::read_to_string("/var/lib/rpg/updates").unwrap_or_default().lines() {
        if let Some(v) = l.strip_prefix("checked ") {
            u.checked = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("error ") {
            u.error = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("step ") {
            let f: Vec<&str> = v.split(' ').collect();
            if f.len() >= 5 {
                let dash = |t: &str| if t == "-" { String::new() } else { t.to_string() };
                u.steps.push((f[0].into(), dash(f[1]), f[2].into(), f[3].parse().unwrap_or(0), dash(f[4])));
            }
        } else if let Some(v) = l.strip_prefix("kept ") {
            u.kept.push(v.to_string());
        }
    }
    u
}

/// A check or an install running in the background
#[derive(Default)]
struct Job {
    lines: Vec<String>,
    done: Option<i32>,
}

fn start_job(argv: Vec<String>, input: String) -> Arc<Mutex<Job>> {
    let job = Arc::new(Mutex::new(Job::default()));
    let j = job.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let child = Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let mut g = j.lock().unwrap();
                g.lines.push(format!("{}: {e}", argv[0]));
                g.done = Some(-1);
                return;
            }
        };
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(input.as_bytes());
        }
        let err = child.stderr.take();
        let j2 = j.clone();
        let et = std::thread::spawn(move || {
            if let Some(e) = err {
                for l in std::io::BufReader::new(e).lines().map_while(Result::ok) {
                    j2.lock().unwrap().lines.push(l);
                }
            }
        });
        if let Some(o) = child.stdout.take() {
            for l in std::io::BufReader::new(o).lines().map_while(Result::ok) {
                j.lock().unwrap().lines.push(l);
            }
        }
        let _ = et.join();
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        j.lock().unwrap().done = Some(code);
    });
    job
}
const METHODS: [&str; 3] = ["Automatic (DHCP)", "Fixed address", "Off"];

/// A change that needs root, waiting for the password
struct Pending {
    what: String,
    cmds: Vec<(Vec<String>, String)>, // argv, stdin
    /// Installing updates: through the installer service, not sudo
    updates: bool,
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
    // Network: the interfaces as netd has them (None: netd not running),
    // when they were asked, the one selected; the editor's method (an
    // index into METHODS), address and gateway, for the interface named
    // in net_for; the DNS servers of /etc/network.conf
    ifaces: Option<Vec<Iface>>,
    net_at: Option<Instant>,
    iface_sel: Option<usize>,
    net_for: String,
    method: usize,
    addr_text: String,
    gw_text: String,
    dns_text: String,
    password: Option<String>, // the user's, once sudo took it
    pending: Option<Pending>,
    pw_text: String,
    message: Option<(String, String)>,
    confirm_delete: Option<String>,
    status: String,
    // Updates: the last check, what runs now (and what: "check" or
    // "install"), its output, whether a new system image went in
    ups: UpStatus,
    job: Option<(String, Arc<Mutex<Job>>)>,
    job_lines: Vec<String>,
    new_image: Option<String>,
    up_sel: Option<usize>,
}

/// /etc/network.conf's lines, as words (comments left out)
fn conf_lines() -> Vec<Vec<String>> {
    std::fs::read_to_string("/etc/network.conf")
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// What /etc/network.conf says for an interface: the method (an index
/// into METHODS), address and gateway; DHCP when it has no line
fn conf_method(name: &str) -> (usize, String, String) {
    for w in conf_lines() {
        if w.len() >= 2 && w[0] == name {
            match w[1].as_str() {
                "dhcp" => return (0, String::new(), String::new()),
                "static" => return (1, w.get(2).cloned().unwrap_or_default(), w.get(3).cloned().unwrap_or_default()),
                "off" | "manual" => return (2, String::new(), String::new()),
                _ => {}
            }
        }
    }
    (0, String::new(), String::new())
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
            ifaces: None,
            net_at: None,
            iface_sel: Some(0),
            net_for: String::new(),
            method: 0,
            addr_text: String::new(),
            gw_text: String::new(),
            dns_text: String::new(),
            password: None,
            pending: None,
            pw_text: String::new(),
            message: None,
            confirm_delete: None,
            status: String::new(),
            ups: UpStatus::default(),
            job: None,
            job_lines: Vec::new(),
            new_image: None,
            up_sel: None,
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
        self.ups = up_status();
        self.net_at = None;
        self.net_for.clear();
        self.dns_text = conf_lines().iter().filter(|w| w[0] == "dns").flat_map(|w| w[1..].to_vec()).collect::<Vec<_>>().join(" ");
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
        let p = Pending { what: what.to_string(), cmds, updates: false };
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

    fn network(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ctl.wake_after = Some(Duration::from_secs(2));
        if self.net_at.map_or(true, |t| t.elapsed() >= Duration::from_secs(2)) {
            self.ifaces = rustica_ui::net::interfaces();
            self.net_at = Some(Instant::now());
        }
        ui.heading("Network");
        let Some(ifaces) = self.ifaces.clone() else {
            ui.label("The network service is not running.");
            ui.dim_label("Start it with: sudo svc start netd");
            return;
        };
        if ifaces.is_empty() {
            ui.label("No network card was found.");
            return;
        }
        let rows: Vec<Row> = ifaces
            .iter()
            .map(|i| Row {
                icon: Icon::None,
                cells: vec![
                    i.name.clone(),
                    match i.state.as_str() {
                        "up" => "connected".into(),
                        "no-carrier" => "no cable".into(),
                        s => s.to_string(),
                    },
                    i.v4.first().map(|(a, _)| a.clone()).unwrap_or_else(|| "\u{2014}".into()),
                    i.gateway.clone().unwrap_or_else(|| "\u{2014}".into()),
                ],
            })
            .collect();
        let cols = [
            Column { title: "Interface", width: 0.0, right: false },
            Column { title: "State", width: 110.0, right: false },
            Column { title: "Address", width: 150.0, right: false },
            Column { title: "Gateway", width: 120.0, right: false },
        ];
        if self.iface_sel.map_or(true, |k| k >= ifaces.len()) {
            self.iface_sel = Some(0);
        }
        ui.table("ifaces", &cols, &rows, &mut self.iface_sel, (ui.line_h() * 1.6 * (ifaces.len() as f32 + 1.5)) / ui.scale);
        let Some(i) = self.iface_sel.and_then(|k| ifaces.get(k)).cloned() else { return };
        // The editor follows the selection, from /etc/network.conf (a
        // fixed address starts as the one DHCP gave)
        if self.net_for != i.name {
            let (m, a, g) = conf_method(&i.name);
            self.method = m;
            self.addr_text = if a.is_empty() { i.v4.first().map(|(a, _)| a.clone()).unwrap_or_default() } else { a };
            self.gw_text = if g.is_empty() && m != 1 { i.gateway.clone().unwrap_or_default() } else { g };
            self.net_for = i.name.clone();
        }
        ui.dim_label(&format!("{} \u{b7} {} \u{b7} received {}, sent {}", i.driver, i.mac, sys::human_size(i.rx_bytes), sys::human_size(i.tx_bytes)));
        for (a, left) in &i.v4 {
            if let Some(s) = left {
                ui.dim_label(&format!("{a}: from DHCP, {} min left on the lease", s / 60));
            }
        }
        for a in i.v6.iter().filter(|a| !a.starts_with("fe80:")) {
            ui.dim_label(&format!("IPv6 {a}"));
        }
        ui.space(4.0);
        let mut chosen = None;
        ui.row(|ui| {
            for (k, m) in METHODS.iter().enumerate() {
                if if self.method == k { ui.primary_button(m) } else { ui.button(m) } {
                    chosen = Some(k);
                }
            }
        });
        if let Some(k) = chosen {
            self.method = k;
        }
        let mut enter = false;
        if self.method == 1 {
            let (_, e1) = ui.row(|ui| {
                ui.column(ui.u(90.0), |ui| ui.label("Address"));
                ui.text_field("addr", &mut self.addr_text, 180.0)
            });
            let (_, e2) = ui.row(|ui| {
                ui.column(ui.u(90.0), |ui| ui.label("Gateway"));
                ui.text_field("gw", &mut self.gw_text, 180.0)
            });
            ui.dim_label("The address with its prefix: 192.168.1.20/24. The gateway may be left empty.");
            enter = e1 || e2;
        }
        if ui.primary_button(&format!("Apply to {}", i.name)) || enter {
            let mut argv: Vec<String> = vec!["/bin/netcfg".into(), i.name.clone()];
            match self.method {
                0 => argv.push("dhcp".into()),
                1 => {
                    let a = self.addr_text.trim().to_string();
                    if !a.contains('/') {
                        self.message = Some(("Not an address".into(), "Give the address with its prefix, such as 192.168.1.20/24".into()));
                        return;
                    }
                    argv.push("static".into());
                    argv.push(a);
                    let g = self.gw_text.trim().to_string();
                    if !g.is_empty() {
                        argv.push(g);
                    }
                }
                _ => argv.push("off".into()),
            }
            let what = format!("network {}", argv[1..].join(" "));
            self.admin(&what, vec![(argv, String::new())]);
        }
        ui.space(8.0);
        ui.label("DNS servers");
        let now = rustica_ui::net::dns_servers();
        ui.dim_label(&format!("In use: {}", if now.is_empty() { "none".to_string() } else { now.join(", ") }));
        let (set, enter) = ui.row(|ui| {
            ui.column(ui.u(90.0), |ui| ui.label("Your own"));
            let (_, e) = ui.text_field("dns", &mut self.dns_text, 220.0);
            (ui.button("Use these servers"), e)
        });
        ui.dim_label("Tried before the ones DHCP gives; empty: DHCP's only. Addresses apart with spaces.");
        if set || enter {
            let mut argv: Vec<String> = vec!["/bin/netcfg".into(), "dns".into()];
            argv.extend(self.dns_text.split(|c: char| c == ' ' || c == ',').filter(|s| !s.is_empty()).map(str::to_string));
            let what = format!("dns {}", argv[2..].join(" "));
            self.admin(what.trim(), vec![(argv, String::new())]);
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

    fn install_updates(&mut self) {
        let input = if sys::euid() == 0 { String::new() } else { format!("{}\n", self.password.clone().unwrap_or_default()) };
        say("updates install");
        self.job_lines.clear();
        self.new_image = None;
        self.job = Some(("install".into(), start_job(vec!["/bin/updates".into(), "install".into(), "-S".into()], input)));
    }

    fn updates_section(&mut self, ui: &mut Ui, ctl: &mut Control) {
        ui.heading("Updates");
        // What runs, and what it said
        if let Some((what, job)) = self.job.clone() {
            ctl.wake_after = Some(Duration::from_millis(300));
            let (lines, done) = {
                let g = job.lock().unwrap();
                (g.lines.clone(), g.done)
            };
            self.job_lines = lines;
            if let Some(code) = done {
                self.job = None;
                self.ups = up_status();
                if what == "install" {
                    if code == 0 {
                        // A new system image (rustica-system-VERSION: N files installed)
                        self.new_image = self.job_lines.iter().find_map(|l| {
                            l.strip_prefix("rustica-system-").and_then(|r| r.split_once(' ')).filter(|(_, rest)| rest.contains("installed")).map(|(v, _)| v.trim_end_matches(':').to_string())
                        });
                        say(&format!("updates installed{}", self.new_image.as_ref().map(|v| format!(" (new system image {v})")).unwrap_or_default()));
                        self.status = "The updates are installed.".into();
                    } else {
                        let why = self.job_lines.iter().rev().find(|l| l.starts_with("updates:")).cloned().unwrap_or_else(|| format!("updates install ended with {code}"));
                        if why.contains("wrong password") || why.contains("not an administrator") {
                            self.password = None;
                        }
                        say(&format!("updates failed: {why}"));
                        self.message = Some(("The updates were not installed".into(), why));
                    }
                } else {
                    say(&format!("updates checked: {}", self.ups.steps.len()));
                }
            }
        }
        let running = self.job.is_some();
        if self.ups.checked == 0 {
            ui.label("Not checked yet.");
        } else {
            let tm = sys::local(self.ups.checked);
            ui.dim_label(&format!(
                "Last checked {:04}-{:02}-{:02} {:02}:{:02}{}",
                tm.year + 1900,
                tm.mon + 1,
                tm.mday,
                tm.hour,
                tm.min,
                if self.ups.error.is_empty() { String::new() } else { format!(": {}", self.ups.error) }
            ));
        }
        if self.ups.steps.is_empty() {
            if self.ups.checked != 0 && self.ups.error.is_empty() {
                ui.label("Everything is up to date.");
            }
        } else {
            let total: u64 = self.ups.steps.iter().map(|s| s.3).sum();
            ui.label(&format!("{} package{} to upgrade, {} to download", self.ups.steps.len(), if self.ups.steps.len() == 1 { "" } else { "s" }, sys::human_size(total)));
            let rows: Vec<Row> = self
                .ups
                .steps
                .iter()
                .map(|(n, from, to, size, why)| Row {
                    icon: Icon::None,
                    cells: vec![n.clone(), if from.is_empty() { format!("new{}", if why.is_empty() { String::new() } else { format!(", for {why}") }) } else { from.clone() }, to.clone(), sys::human_size(*size)],
                })
                .collect();
            let cols = [
                Column { title: "Package", width: 0.0, right: false },
                Column { title: "Installed", width: 170.0, right: false },
                Column { title: "New", width: 90.0, right: false },
                Column { title: "Size", width: 80.0, right: true },
            ];
            let h = (ui.line_h() * 1.6 * (rows.len().min(8) as f32 + 1.5)) / ui.scale;
            ui.table("updates", &cols, &rows, &mut self.up_sel, h);
            for k in &self.ups.kept {
                ui.dim_label(&format!("Kept back: {k}"));
            }
        }
        ui.space(4.0);
        let (check, install) = ui.row(|ui| {
            let c = !running && ui.button("Check now");
            let i = !running && !self.ups.steps.is_empty() && ui.primary_button("Install updates");
            (c, i)
        });
        if check {
            say("updates check");
            self.job_lines.clear();
            self.job = Some(("check".into(), start_job(vec!["/bin/updates".into(), "check".into()], String::new())));
        }
        if install {
            if sys::euid() == 0 || self.password.is_some() {
                self.install_updates();
            } else {
                self.pw_text.clear();
                self.pending = Some(Pending { what: "install the updates".into(), cmds: Vec::new(), updates: true });
            }
        }
        if running {
            ui.label(if self.job.as_ref().map(|j| j.0.as_str()) == Some("install") { "Installing the updates..." } else { "Checking..." });
        }
        if !self.job_lines.is_empty() {
            for l in self.job_lines.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                ui.dim_label(l);
            }
        }
        if let Some(v) = self.new_image.clone() {
            ui.space(4.0);
            ui.label(&format!("The new system image {v} starts at the next restart (the previous one stays in the boot menu)."));
            if ui.primary_button("Restart now") {
                self.admin("restart", vec![(vec!["/bin/reboot".into()], String::new())]);
            }
        }
        ui.space(8.0);
        ui.label("Sources");
        let src = std::fs::read_to_string("/etc/rpg/sources.list").unwrap_or_default();
        for l in src.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            ui.dim_label(l);
        }
        ui.dim_label("The system checks once a day (/etc/rpg/updates.conf); updates in a terminal: man updates.");
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
                3 => self.network(ui, ctl),
                4 => self.time(ui, ctl),
                5 => self.users_section(ui),
                S_DISKS => self.disks_section(ui),
                S_UPDATES => self.updates_section(ui, ctl),
                _ => self.about_section(ui),
            });
        });
        if self.section != before {
            self.status.clear();
            if let Some(s) = self.section {
                say(&format!("section {}", SECTIONS[s]));
            }
            if self.section == Some(S_DISKS) || self.section == Some(S_ABOUT) {
                self.load_disks();
                self.load_about();
            }
            if self.section == Some(S_UPDATES) {
                self.ups = up_status();
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
                if p.updates {
                    self.install_updates();
                } else {
                    self.run_admin(p);
                }
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

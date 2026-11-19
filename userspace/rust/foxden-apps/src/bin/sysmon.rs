// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! sysmon: Rustica's System Monitor, an app (FoxDen step FD3, the apps
//! repository's org.rustica.sysmon): how busy the processors are and how
//! much memory is in use, over the last minute, with the uptime, threads
//! and the kernel's version. It asks for no permissions: all of it comes
//! from system_get_stats (0x91) and system_get_version (0x90), which any
//! process may call. The list of processes needs the system's job, which
//! a sandboxed app is not given (it comes with the `system` permission,
//! FoxDen step FD4). At start it prints one line ("sysmon: 4 CPUs, ...").

use rustica_ui::gfx::Rect;
use rustica_ui::{Control, Ui};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Stats {
    total_bytes: u64,
    free_bytes: u64,
    boot_time: u64,
    uptime: u64,
    cpus: u64,
    idle_time: u64,
    threads: u64,
    console_cols: u64,
    console_rows: u64,
    kernel_heap: u64,
    kernel_heap_used: u64,
}

fn syscall(n: i64, a: i64, b: i64) -> i64 {
    let ret: i64;
    // Rustux's system call: int 0x80, number in rax, arguments in rdi, rsi
    unsafe {
        std::arch::asm!("int 0x80", inlateout("rax") n => ret, in("rdi") a, in("rsi") b, in("rdx") 0i64,
                        in("r10") 0i64, in("r8") 0i64, in("r9") 0i64, lateout("rcx") _, lateout("r11") _);
    }
    ret
}

fn stats() -> Option<Stats> {
    let mut s = Stats::default();
    (syscall(0x91, &mut s as *mut Stats as i64, std::mem::size_of::<Stats>() as i64) == 0).then_some(s)
}

fn version() -> String {
    let mut b = [0u8; 256];
    if syscall(0x90, b.as_mut_ptr() as i64, b.len() as i64) < 0 {
        return String::new();
    }
    let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).trim().to_string()
}

fn mib(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MiB", b >> 20)
    }
}

fn uptime(ns: u64) -> String {
    let s = ns / 1_000_000_000;
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", s % 60)
    }
}

const POINTS: usize = 60;

struct Mon {
    last: Option<(Stats, Instant)>,
    now: Stats,
    cpu: Vec<f32>, // 0..1, a point a second
    mem: Vec<f32>,
    version: String,
}

impl Mon {
    fn sample(&mut self) {
        let Some(s) = stats() else { return };
        if let Some((prev, _)) = self.last {
            let wall = s.uptime.saturating_sub(prev.uptime) as f64 * s.cpus.max(1) as f64;
            let idle = s.idle_time.saturating_sub(prev.idle_time) as f64;
            let busy = if wall > 0.0 { (1.0 - idle / wall).clamp(0.0, 1.0) } else { 0.0 };
            self.cpu.push(busy as f32);
        }
        let used = s.total_bytes.saturating_sub(s.free_bytes);
        self.mem.push(if s.total_bytes > 0 { used as f32 / s.total_bytes as f32 } else { 0.0 });
        for v in [&mut self.cpu, &mut self.mem] {
            if v.len() > POINTS {
                v.remove(0);
            }
        }
        self.now = s;
        self.last = Some((s, Instant::now()));
    }

    fn graph(ui: &mut Ui, title: &str, now: &str, values: &[f32], color: u32) {
        ui.row(|ui| {
            ui.label(title);
            ui.dim_label(now);
        });
        let w = ui.available_width();
        let h = ui.u(110.0);
        let r = ui.alloc(w, h);
        let th = ui.theme;
        ui.canvas.fill_rounded(r, ui.u(6.0), th.surface);
        let inner = r.shrink(ui.u(6.0));
        // Grid lines at 25 %, 50 %, 75 %
        for q in 1..4 {
            let y = (inner.y + inner.h * q as f32 / 4.0).round();
            ui.canvas.fill_rect(Rect::new(inner.x, y, inner.w, 1.0), th.border);
        }
        // The newest at the right edge, each bar a POINTS-th of the width
        let step = inner.w / POINTS as f32;
        let n = values.len();
        for (i, v) in values.iter().enumerate() {
            let bh = (inner.h * v.clamp(0.0, 1.0)).max(1.0);
            let x0 = (inner.x + inner.w - step * (n - i) as f32).round();
            let x1 = (inner.x + inner.w - step * (n - i - 1) as f32).round();
            ui.canvas.fill_rect(Rect::new(x0, inner.y + inner.h - bh, (x1 - x0 - 1.0).max(1.0), bh), color);
        }
    }

    fn frame(&mut self, ui: &mut Ui, ctl: &mut Control) {
        for k in std::mem::take(&mut ui.input.keys) {
            if k.usage == rustica_ui::ui::keys::ESC || (k.ctrl() && k.usage == 0x14) {
                ctl.quit = true;
            }
        }
        if self.last.is_none_or(|(_, t)| t.elapsed() >= Duration::from_millis(950)) {
            self.sample();
        }
        ctl.wake_after = Some(Duration::from_secs(1));
        let s = self.now;
        let used = s.total_bytes.saturating_sub(s.free_bytes);
        let cpu_now = self.cpu.last().map(|v| format!("{:.0} %", v * 100.0)).unwrap_or_else(|| "…".into());
        ui.heading("System Monitor");
        ui.dim_label(&self.version);
        ui.space(4.0);
        let cpus = format!("Processors ({})", s.cpus);
        Self::graph(ui, &cpus, &cpu_now, &self.cpu, 0x50FA7B);
        let mem_now = format!("{} of {}", mib(used), mib(s.total_bytes));
        Self::graph(ui, "Memory", &mem_now, &self.mem, 0xBD93F9);
        ui.row(|ui| {
            ui.label(&format!("Up {}", uptime(s.uptime)));
            ui.dim_label(&format!("{} threads", s.threads));
            ui.dim_label(&format!("kernel heap {} of {}", mib(s.kernel_heap_used), mib(s.kernel_heap)));
        });
    }
}

fn main() {
    let Some(s) = stats() else {
        eprintln!("sysmon: system_get_stats failed");
        std::process::exit(1);
    };
    println!(
        "sysmon: {} CPUs, {} of {} memory in use, up {}",
        s.cpus,
        mib(s.total_bytes.saturating_sub(s.free_bytes)),
        mib(s.total_bytes),
        uptime(s.uptime)
    );
    let mut m = Mon { last: None, now: s, cpu: Vec::new(), mem: Vec::new(), version: version() };
    if let Err(e) = rustica_ui::run("System Monitor", 520, 440, None, |ui, ctl| m.frame(ui, ctl)) {
        eprintln!("sysmon: {e}");
        std::process::exit(1);
    }
}

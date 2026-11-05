// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! System information, clocks and power. Signatures are in
//! docs/SYSCALL.md.

use super::driver::{require_resource, require_resource_kind};
use super::object::{ret, R};
use super::{user, SyscallArgs, SyscallRet};
use crate::arch::amd64::mm::RxStatus;
use crate::arch::amd64::platform;
use core::sync::atomic::Ordering;

/// Clocks for clock_get
pub const CLOCK_MONOTONIC: u32 = 0;
pub const CLOCK_UTC: u32 = 1;

/// Kernel name, version and build (for `uname`)
pub const VERSION: &str = concat!("Rustux ", env!("CARGO_PKG_VERSION"), " (", env!("RUSTUX_BUILD_INFO"), ") x86_64");

/// clock_get(clock_id) — ns: monotonic since power-on (0) or UTC since
/// 1970 (1, from the real-time clock at boot)
pub fn sys_clock_get(args: SyscallArgs) -> SyscallRet {
    match args.arg_u32(0) {
        CLOCK_MONOTONIC => crate::kobj::now_ns() as SyscallRet,
        CLOCK_UTC => platform::utc_now() as SyscallRet,
        _ => super::err_to_ret(RxStatus::ERR_INVALID_ARGS),
    }
}

/// clock_set_utc(resource, utc_ns) — set the wall clock (root resource)
pub fn sys_clock_set_utc(args: SyscallArgs) -> SyscallRet {
    let (res, utc) = (args.arg_u32(0), args.arg_i64(1));
    ret((|| {
        require_resource(res)?;
        if utc < 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        platform::set_utc(utc);
        Ok(0)
    })())
}

/// system_get_version(*buf, len) — the version string, NUL-terminated;
/// returns its length
pub fn sys_system_get_version(args: SyscallArgs) -> SyscallRet {
    let (buf, len) = (args.arg(0), args.arg(1));
    ret((|| {
        let v = VERSION.as_bytes();
        if len < v.len() + 1 {
            return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
        }
        user::write_bytes(buf, v)?;
        user::write_bytes(buf + v.len(), &[0])?;
        Ok(v.len())
    })())
}

/// system_get_stats(*buf, len) — `rx_system_stats_t`: memory, CPUs,
/// uptime, idle time
pub fn sys_system_get_stats(args: SyscallArgs) -> SyscallRet {
    let (buf, len) = (args.arg(0), args.arg(1));
    ret((|| -> R<usize> {
        use crate::arch::amd64::percpu;
        let page = 4096u64;
        let total = crate::mm::pmm::pmm_count_total_pages() * page;
        let free = crate::mm::pmm::pmm_count_free_pages() * page;
        let mut idle = 0u64;
        for c in percpu::all() {
            idle += c.idle_ns.load(Ordering::Relaxed);
        }
        let (cols, rows) = crate::drivers::display::size();
        let fields: [u64; 9] = [
            total,
            free,
            platform::boot_ns(),
            crate::kobj::now_ns().saturating_sub(platform::boot_ns()),
            percpu::online_count() as u64,
            idle,
            crate::process::table::PROCESS_TABLE.lock().count() as u64,
            cols as u64,
            rows as u64,
        ];
        let bytes: alloc::vec::Vec<u8> = fields.iter().flat_map(|v| v.to_le_bytes()).collect();
        // Programs built before the console size was added pass 7 fields
        if len < 7 * 8 {
            return Err(RxStatus::ERR_BUFFER_TOO_SMALL);
        }
        let n = core::cmp::min(len, bytes.len()) / 8 * 8;
        user::write_bytes(buf, &bytes[..n])?;
        Ok(0)
    })())
}

/// Commands for system_powerctl
pub const POWER_REBOOT: u32 = 1;
pub const POWER_OFF: u32 = 2;

/// system_powerctl(resource, cmd) — restart (1) or power off (2); needs
/// the root or the power resource. Returns only if power-off failed.
pub fn sys_system_powerctl(args: SyscallArgs) -> SyscallRet {
    let (res, cmd) = (args.arg_u32(0), args.arg_u32(1));
    ret((|| {
        require_resource_kind(res, Some(crate::kobj::interrupt::kind::POWER))?;
        match cmd {
            POWER_REBOOT => {
                crate::klog::write(b"[POWER] restarting\n");
                platform::reboot()
            }
            POWER_OFF => {
                crate::klog::write(b"[POWER] powering off\n");
                platform::power_off();
                crate::klog::write(b"[POWER] ACPI power-off did not work\n");
                Err(RxStatus::ERR_NOT_SUPPORTED)
            }
            _ => Err(RxStatus::ERR_INVALID_ARGS),
        }
    })())
}

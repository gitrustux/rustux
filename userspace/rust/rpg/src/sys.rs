// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The few C library calls rpg needs beyond std: the same on Rustux's C
//! library and on Linux (the publishing side runs rpg build there).

extern "C" {
    fn geteuid() -> u32;
    fn getpid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn sync();
    fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
    fn lchown(path: *const u8, uid: u32, gid: u32) -> i32;
}

pub fn is_root() -> bool {
    unsafe { geteuid() == 0 }
}

pub fn pid() -> i32 {
    unsafe { getpid() }
}

/// Is there a process with this ID?
pub fn alive(pid: i32) -> bool {
    pid > 0 && unsafe { kill(pid, 0) } == 0
}

/// Everything written so far on its way to the disks
pub fn sync_all() {
    unsafe { sync() }
}

/// Seconds since this computer started (CLOCK_MONOTONIC): smaller than
/// a lock's means the computer was restarted since
pub fn uptime() -> i64 {
    let mut ts = [0i64; 2];
    unsafe { clock_gettime(1, &mut ts) };
    ts[0]
}

pub fn chown(path: &std::path::Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let mut p = path.as_os_str().as_bytes().to_vec();
    p.push(0);
    if unsafe { lchown(p.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

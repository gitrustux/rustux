// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rpg for users who are not root (FoxDen step FD4): install, remove,
//! update and upgrade go to the installer service, /svc/packages
//! (packagesd, which runs rpg as root), and what it says is printed here.
//! It decides who may do what: apps from app sources that ask for neither
//! files:home nor system, anyone; anything else an administrator's
//! password, asked for here ("auth" from the service), the user's own if
//! they are one, else an administrator's name and password.
//!
//! Only on Rustux (the build for Rustux links against its C library);
//! the Linux build of rpg, for publishing, has no service to ask.

use std::ffi::{CStr, CString};

extern "C" {
    fn rx_service_connect(path: *const u8, out: *mut u32) -> i32;
    fn getpass(prompt: *const u8) -> *mut u8;
    fn getgrouplist(user: *const u8, group: u32, groups: *mut u32, n: *mut i32) -> i32;
}

const SHOULD_WAIT: i64 = -13;
const READABLE: u32 = 1;
const PEER_CLOSED: u32 = 4;

extern "C" {
    // The C library's system call (each architecture's in <rustux/arch.h>)
    fn __rx_syscall(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64;
}

fn syscall6(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64 {
    unsafe { __rx_syscall(n, a, b, c, d, e, f) }
}

struct Channel(u32);

impl Drop for Channel {
    fn drop(&mut self) {
        syscall6(0x07, self.0 as i64, 0, 0, 0, 0, 0);
    }
}

impl Channel {
    fn connect(path: &str) -> Result<Channel, String> {
        let p = CString::new(path).unwrap();
        let mut h = 0u32;
        let r = unsafe { rx_service_connect(p.as_ptr() as *const u8, &mut h) };
        if r < 0 {
            return Err(format!("the installer service {path} is not running (sudo svc start packagesd), or ask root: sudo rpg ..."));
        }
        Ok(Channel(h))
    }

    fn write(&self, msg: &[u8]) -> Result<(), String> {
        let r = syscall6(0x21, self.0 as i64, 0, msg.as_ptr() as i64, msg.len() as i64, 0, 0);
        if r < 0 {
            return Err("the installer service went away".into());
        }
        Ok(())
    }

    /// The next message (waiting for it); None when the service closed
    fn read(&self) -> Option<String> {
        let mut buf = vec![0u8; 65536];
        loop {
            let mut actual = [0u32; 2];
            let caps = buf.len() as i64; // no handles
            let r = syscall6(0x22, self.0 as i64, 0, buf.as_mut_ptr() as i64, 0, caps, actual.as_mut_ptr() as i64);
            if r == SHOULD_WAIT {
                let mut seen = 0u32;
                syscall6(0x26, self.0 as i64, (READABLE | PEER_CLOSED) as i64, -1i64, &mut seen as *mut u32 as i64, 0, 0);
                if seen & READABLE == 0 {
                    return None;
                }
                continue;
            }
            if r < 0 {
                return None;
            }
            return Some(String::from_utf8_lossy(&buf[..actual[0] as usize]).into_owned());
        }
    }
}

fn user_name() -> Option<String> {
    let uid = unsafe { libc_getuid() };
    std::fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 3 && f[2].parse::<u32>().ok() == Some(uid)).then(|| f[0].to_string())
    })
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// Is `name` in group sudo (an administrator)?
fn is_admin(name: &str) -> bool {
    let Some(gid) = std::fs::read_to_string("/etc/group").ok().and_then(|t| {
        t.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 2 && f[0] == "sudo").then(|| f[2].parse::<u32>().ok()).flatten()
        })
    }) else {
        return false;
    };
    let c = CString::new(name).unwrap();
    let mut groups = [0u32; 64];
    let mut n = groups.len() as i32;
    unsafe { getgrouplist(c.as_ptr() as *const u8, u32::MAX, groups.as_mut_ptr(), &mut n) };
    groups[..n.clamp(0, 64) as usize].contains(&gid)
}

fn ask(prompt: &str) -> Option<String> {
    let p = CString::new(prompt).unwrap();
    let r = unsafe { getpass(p.as_ptr() as *const u8) };
    if r.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(r as *const std::ffi::c_char) }.to_string_lossy().into_owned())
}

extern "C" {
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
}

/// A line from standard input, a byte at a time (as getpass reads it:
/// a buffered read would take the password's line too)
fn ask_line(prompt: &str) -> Option<String> {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut l = Vec::new();
    loop {
        let mut b = 0u8;
        let r = unsafe { read(0, &mut b, 1) };
        if r != 1 {
            if l.is_empty() {
                return None;
            }
            break;
        }
        if b == b'\n' || b == b'\r' {
            break;
        }
        l.push(b);
    }
    Some(String::from_utf8_lossy(&l).trim().to_string())
}

/// Ask the installer service to run `request` (its first line: "install
/// ID...", "remove ID...", "update", "upgrade"); its output printed.
/// The exit code rpg should end with.
pub fn run(request: &str) -> Result<i32, String> {
    let ch = Channel::connect("/svc/packages")?;
    let mut auth = String::new();
    for _ in 0..3 {
        let msg = if auth.is_empty() { request.to_string() } else { format!("{request}\n{auth}") };
        ch.write(msg.as_bytes())?;
        loop {
            let Some(m) = ch.read() else { return Err("the installer service went away".into()) };
            let (kind, rest) = m.split_once(' ').unwrap_or((m.as_str(), ""));
            match kind {
                "out" => println!("{rest}"),
                "done" => return Ok(rest.trim().parse().unwrap_or(1)),
                "denied" => return Err(format!("refused: {rest}")),
                "auth" => {
                    // An administrator's say is needed: why, then a password
                    println!("{rest}: an administrator's password is needed");
                    let me = user_name().unwrap_or_default();
                    auth = if is_admin(&me) {
                        ask(&format!("[rpg] password for {me}: ")).ok_or("no password")?
                    } else {
                        let who = ask_line("An administrator's name: ").filter(|s| !s.is_empty()).ok_or("no administrator named")?;
                        let pw = ask(&format!("[rpg] password for {who}: ")).ok_or("no password")?;
                        format!("as {who} {pw}")
                    };
                    break;
                }
                "error" => return Err(rest.to_string()),
                _ => println!("{m}"),
            }
        }
    }
    Err("three wrong passwords".into())
}

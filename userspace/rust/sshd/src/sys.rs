// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The C library's calls sshd needs: pseudo-terminals (as rustica-apps'
//! pty.rs), a wake-up pipe, poll, random bytes, chmod.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};

extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn ptsname(fd: i32) -> *const std::ffi::c_char;
    fn open(path: *const std::ffi::c_char, flags: i32, ...) -> i32;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn dup(fd: i32) -> i32;
    #[link_name = "pipe"]
    fn c_pipe(fds: *mut i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn poll(fds: *mut PollFd, n: u64, timeout: i32) -> i32;
    fn getrandom(buf: *mut u8, n: usize, flags: u32) -> isize;
    #[link_name = "chmod"]
    fn c_chmod(path: *const std::ffi::c_char, mode: u32) -> i32;
}

const O_RDWR: i32 = 2;
const O_NONBLOCK: i32 = 0o4000;
const F_SETFL: i32 = 4;
const TIOCSWINSZ: u64 = 0x5414;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct Winsize {
    rows: u16,
    cols: u16,
    xpixel: u16,
    ypixel: u16,
}

/// The master side of a pseudo-terminal
pub struct Pty {
    fd: i32,
}

impl Pty {
    pub fn open() -> io::Result<Pty> {
        let fd = unsafe { posix_openpt(O_RDWR) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Pty { fd })
    }

    /// Run a program with the slave as its stdin, stdout and stderr
    pub fn spawn(&self, cmd: &mut Command) -> io::Result<Child> {
        let name = unsafe { CStr::from_ptr(ptsname(self.fd)) }.to_owned();
        let slave = unsafe { open(CString::from(name).as_ptr(), O_RDWR) };
        if slave < 0 {
            return Err(io::Error::last_os_error());
        }
        let copy = |fd: i32| -> io::Result<Stdio> {
            let d = unsafe { dup(fd) };
            if d < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Stdio::from(unsafe { OwnedFd::from_raw_fd(d) }))
        };
        let r = (|| cmd.stdin(copy(slave)?).stdout(copy(slave)?).stderr(copy(slave)?).spawn())();
        unsafe { close(slave) };
        r
    }

    pub fn set_size(&self, cols: usize, rows: usize) {
        let w = Winsize { rows: rows as u16, cols: cols as u16, xpixel: 0, ypixel: 0 };
        unsafe { ioctl(self.fd, TIOCSWINSZ, &w as *const Winsize) };
    }

    /// Type: bytes into the line discipline
    pub fn write(&self, b: &[u8]) {
        let mut done = 0;
        while done < b.len() {
            let n = unsafe { write(self.fd, b[done..].as_ptr(), b.len() - done) };
            if n <= 0 {
                break;
            }
            done += n as usize;
        }
    }

    /// A copy to read the programs' output from (another thread)
    pub fn reader(&self) -> io::Result<File> {
        let d = unsafe { dup(self.fd) };
        if d < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(d) })
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe { close(self.fd) };
    }
}

/// A pipe (read end non-blocking): to wake the connection's loop
pub fn pipe() -> (i32, i32) {
    let mut f = [0i32; 2];
    unsafe {
        c_pipe(f.as_mut_ptr());
        fcntl(f[0], F_SETFL, O_NONBLOCK);
    }
    (f[0], f[1])
}

pub fn wake(fd: i32) {
    unsafe { write(fd, b"x".as_ptr(), 1) };
}

pub fn drain(fd: i32) {
    let mut b = [0u8; 256];
    while unsafe { read(fd, b.as_mut_ptr(), b.len()) } > 0 {}
}

pub fn close_pipe(fd: i32) {
    unsafe { close(fd) };
}


/// Until the socket is readable (or writable, if `out`), the pipe has
/// something, or the time is up
pub fn wait(sock: i32, out: bool, pipe: i32, ms: i32) {
    let mut p = [
        PollFd { fd: sock, events: POLLIN | if out { POLLOUT } else { 0 }, revents: 0 },
        PollFd { fd: pipe, events: POLLIN, revents: 0 },
    ];
    unsafe { poll(p.as_mut_ptr(), 2, ms) };
}

pub fn random(b: &mut [u8]) {
    unsafe { getrandom(b.as_mut_ptr(), b.len(), 0) };
}

pub fn chmod(path: &str, mode: u32) {
    if let Ok(c) = CString::new(path) {
        unsafe { c_chmod(c.as_ptr(), mode) };
    }
}

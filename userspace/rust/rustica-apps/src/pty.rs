// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pseudo-terminals from Rust: the C library's posix_openpt (libc pty.c,
//! with the line discipline of <rustux/ldisc.h>).

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
    fn close(fd: i32) -> i32;
    fn dup(fd: i32) -> i32;
}

const O_RDWR: i32 = 2;
const TIOCSWINSZ: u64 = 0x5414;

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
        // The copies in cmd are closed when it is dropped
        r
    }

    /// The terminal's size, which the programs read with TIOCGWINSZ
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

    /// A copy to read from (in another thread): what the programs print
    /// and the echo; an error (EIO) once every program has closed the slave
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

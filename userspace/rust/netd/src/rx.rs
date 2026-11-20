// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The few Rustux system calls netd needs (<rustux/syscalls.h>), and the
//! C library's services (/svc/...).


pub type Handle = u32;

pub const OK: i64 = 0;
pub const ERR_SHOULD_WAIT: i64 = -13;
pub const CHANNEL_READABLE: u32 = 1 << 0;
pub const WRITABLE: u32 = 1 << 1;
pub const PEER_CLOSED: u32 = 1 << 2;
pub const TIME_INFINITE: u64 = u64::MAX;

extern "C" {
    // The C library's system call (each architecture's in <rustux/arch.h>)
    fn __rx_syscall(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64;
}

#[inline]
unsafe fn syscall(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64 {
    __rx_syscall(n, a, b, c, d, e, f)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WaitItem {
    pub handle: Handle,
    pub waitfor: u32,
    pub pending: u32,
}

/// Who wrote a message (rx_sender_t)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Sender {
    pub pid: u64,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub ngroups: u32,
    pub _pad: u32,
    pub groups: [u32; 16],
}

pub fn close(h: Handle) {
    unsafe { syscall(0x07, h as i64, 0, 0, 0, 0, 0) };
}

pub fn channel_create() -> Option<(Handle, Handle)> {
    let (mut a, mut b) = (0u32, 0u32);
    let r = unsafe { syscall(0x20, 0, &mut a as *mut u32 as i64, &mut b as *mut u32 as i64, 0, 0, 0) };
    (r == OK).then_some((a, b))
}

pub fn channel_write(h: Handle, data: &[u8], handles: &[Handle]) -> i64 {
    unsafe {
        syscall(0x21, h as i64, 0, data.as_ptr() as i64, data.len() as i64, handles.as_ptr() as i64, handles.len() as i64)
    }
}

/// One message: Ok((bytes, handle)), Err(status) (SHOULD_WAIT: none)
pub fn channel_read(h: Handle, buf: &mut [u8], sender: Option<&mut Sender>) -> Result<(usize, Option<Handle>), i64> {
    #[repr(C)]
    struct Actual {
        n: [u32; 2],
        s: Sender,
    }
    let mut a = Actual { n: [0, 0], s: Sender::default() };
    let mut hs = [0u32; 1];
    let caps = buf.len() as u64 | (1u64 << 32);
    let opts = if sender.is_some() { 1 } else { 0 };
    let r = unsafe { syscall(0x22, h as i64, opts, buf.as_mut_ptr() as i64, hs.as_mut_ptr() as i64, caps as i64, &mut a as *mut Actual as i64) };
    if r != OK {
        return Err(r);
    }
    if let Some(s) = sender {
        *s = a.s;
    }
    Ok((a.n[0] as usize, (a.n[1] > 0).then_some(hs[0])))
}

pub fn socket_create() -> Option<(Handle, Handle)> {
    let (mut a, mut b) = (0u32, 0u32);
    let r = unsafe { syscall(0x2A, 0, &mut a as *mut u32 as i64, &mut b as *mut u32 as i64, 0, 0, 0) };
    (r == OK).then_some((a, b))
}

/// Bytes written (Err SHOULD_WAIT when full, PEER_CLOSED when the other end is gone)
pub fn socket_write(h: Handle, data: &[u8]) -> Result<usize, i64> {
    let mut n = 0u64;
    let r = unsafe { syscall(0x2B, h as i64, 0, data.as_ptr() as i64, data.len() as i64, &mut n as *mut u64 as i64, 0) };
    if r == OK { Ok(n as usize) } else { Err(r) }
}

/// Write no more on this end (the other end reads end-of-stream)
pub fn socket_shutdown_write(h: Handle) {
    let mut n = 0u64;
    unsafe { syscall(0x2B, h as i64, 1, 0, 0, &mut n as *mut u64 as i64, 0) };
}

/// Bytes read (Err SHOULD_WAIT when none yet, PEER_CLOSED at the end)
pub fn socket_read(h: Handle, buf: &mut [u8]) -> Result<usize, i64> {
    let mut n = 0u64;
    let r = unsafe { syscall(0x2C, h as i64, 0, buf.as_mut_ptr() as i64, buf.len() as i64, &mut n as *mut u64 as i64, 0) };
    if r == OK { Ok(n as usize) } else { Err(r) }
}

pub fn wait_many(items: &mut [WaitItem], deadline: u64) -> i64 {
    unsafe { syscall(0x27, items.as_mut_ptr() as i64, items.len() as i64, deadline as i64, 0, 0, 0) }
}

pub fn monotonic() -> u64 {
    unsafe { syscall(0x40, 0, 0, 0, 0, 0, 0) as u64 }
}

extern "C" {
    fn rx_service_publish(path: *const u8, h: Handle) -> i32;
}

/// Publish a service: the handle to read new connections from
pub fn publish(path: &str) -> Option<Handle> {
    let (ours, theirs) = channel_create()?;
    let p = std::ffi::CString::new(path).ok()?;
    if unsafe { rx_service_publish(p.as_ptr() as *const u8, theirs) } < 0 {
        close(ours);
        return None;
    }
    Some(ours)
}

/// New connections to a published service (each a channel)
pub fn accept_all(listener: Handle) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    while let Ok((_, h)) = channel_read(listener, &mut buf, None) {
        if let Some(h) = h {
            out.push(h);
        }
    }
    out
}

extern "C" {
    fn rx_devmgr_connect(out: *mut Handle) -> i32;
    fn rx_dev_call(ch: Handle, op: u32, req: *const u8, len: usize, h: Handle, out: *mut u8, cap: usize, outlen: *mut usize, hout: *mut Handle) -> i32;
}

const PCI_INFO_SIZE: usize = 224; // sizeof(rx_pci_info_t), <rustux/device.h>

/// The network controllers (PCI class 2) devmgr knows, as "BB:DD.F", in
/// bus order: what names the interfaces (eth0 is the first)
pub fn network_pci_slots() -> Vec<String> {
    let mut dm: Handle = 0;
    if unsafe { rx_devmgr_connect(&mut dm) } < 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; 65536];
    let mut got = 0usize;
    let r = unsafe { rx_dev_call(dm, 1, std::ptr::null(), 0, 0, buf.as_mut_ptr(), buf.len(), &mut got, std::ptr::null_mut()) };
    close(dm);
    if r < 0 || got < 4 {
        return Vec::new();
    }
    let n = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    let mut v: Vec<(u8, u8, u8)> = Vec::new();
    for k in 0..n {
        let o = 4 + k * PCI_INFO_SIZE;
        if o + PCI_INFO_SIZE > got {
            break;
        }
        let d = &buf[o..o + PCI_INFO_SIZE];
        if d[14] == 0x02 {
            v.push((d[2], d[3], d[4]));
        }
    }
    v.sort();
    v.into_iter().map(|(b, d, f)| format!("{b:02x}:{d:02x}.{f:x}")).collect()
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Checked access to user memory from syscalls.
//!
//! Every page of a user buffer is checked against the current address
//! space (present, user-accessible, writable when written) before the
//! kernel touches it, so a bad pointer gives the program an error instead
//! of a kernel page fault.

use crate::arch::amd64::mm::kernel_map;
use crate::arch::amd64::mm::RxStatus;

fn current_pml4() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack)) };
    cr3 & 0x000f_ffff_ffff_f000
}

/// Check `len` bytes at `addr` in the current user address space
pub fn check(addr: usize, len: usize, write: bool) -> Result<(), RxStatus> {
    if len == 0 {
        return Ok(());
    }
    let end = addr.checked_add(len).ok_or(RxStatus::ERR_INVALID_ARGS)?;
    if addr == 0 || end > 0x0000_8000_0000_0000 {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    let pml4 = current_pml4();
    let mut page = addr & !0xFFF;
    while page < end {
        if !unsafe { kernel_map::user_access_ok(pml4, page as u64, write) } {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        page += 0x1000;
    }
    Ok(())
}

pub fn write_u32(addr: usize, v: u32) -> Result<(), RxStatus> {
    check(addr, 4, true)?;
    unsafe { core::ptr::write_unaligned(addr as *mut u32, v) };
    Ok(())
}

pub fn write_u64(addr: usize, v: u64) -> Result<(), RxStatus> {
    write_bytes(addr, &v.to_le_bytes())
}

pub fn read_u32(addr: usize) -> Result<u32, RxStatus> {
    check(addr, 4, false)?;
    Ok(unsafe { core::ptr::read_unaligned(addr as *const u32) })
}

/// Copy `len` bytes from user memory
pub fn read_bytes(addr: usize, len: usize) -> Result<alloc::vec::Vec<u8>, RxStatus> {
    check(addr, len, false)?;
    let mut v = alloc::vec::Vec::with_capacity(len);
    unsafe {
        core::ptr::copy_nonoverlapping(addr as *const u8, v.as_mut_ptr(), len);
        v.set_len(len);
    }
    Ok(v)
}

/// Copy `data` to user memory
pub fn write_bytes(addr: usize, data: &[u8]) -> Result<(), RxStatus> {
    check(addr, data.len(), true)?;
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len()) };
    Ok(())
}

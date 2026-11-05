// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The kernel log: everything the kernel prints for debugging goes to the
//! debug port (0xE9, QEMU's debug console; ignored on hardware) and into a
//! 64 KiB ring buffer that `dmesg` reads through a debug log handle
//! (`debuglog_read`). Lock-free, so it works in interrupt handlers and in
//! the panic path: a writer reserves a position with one atomic add and
//! stores its byte there.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

pub const RING: usize = 64 * 1024;

static BUF: [AtomicU8; RING] = [const { AtomicU8::new(0) }; RING];
/// Bytes ever written (the next byte goes to `WRITTEN % RING`)
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Write one byte to the debug port only
#[inline(always)]
pub fn port_put(b: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags)) };
}

/// Log one byte
#[inline]
pub fn put(b: u8) {
    port_put(b);
    let at = WRITTEN.fetch_add(1, Ordering::Relaxed);
    BUF[(at % RING as u64) as usize].store(b, Ordering::Relaxed);
}

/// Log a string
pub fn write(s: &[u8]) {
    for &b in s {
        put(b);
    }
}

/// Copy log bytes from position `*pos` into `out`; returns how many and
/// advances `*pos`. A position older than what the ring still holds jumps
/// to the oldest byte kept.
pub fn read(pos: &mut u64, out: &mut [u8]) -> usize {
    let end = WRITTEN.load(Ordering::Acquire);
    let oldest = end.saturating_sub(RING as u64);
    if *pos < oldest {
        *pos = oldest;
    }
    let n = ((end - *pos) as usize).min(out.len());
    for (i, o) in out[..n].iter_mut().enumerate() {
        *o = BUF[((*pos + i as u64) % RING as u64) as usize].load(Ordering::Relaxed);
    }
    *pos += n as u64;
    n
}

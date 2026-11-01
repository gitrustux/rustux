// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel heap allocator.
//!
//! The heap is one region handed over at boot (16 MiB from the PMM).
//!
//! - Small requests (size and alignment both at most 2 KiB) are rounded up
//!   to a power of two from 16 to 2048 and served from a free list per
//!   size. An empty list is refilled by carving one 4 KiB page into equal
//!   blocks; freed blocks go back on their list.
//! - Larger requests take whole pages from a list of free page runs kept in
//!   address order; freeing merges a run with its neighbours.
//!
//! The free-list nodes live in the free memory itself. Every operation runs
//! under a spin lock with interrupts disabled, so interrupt handlers may
//! allocate too. (The previous allocator used the same links for the free
//! list and for physical neighbours, and corrupted itself under load.)

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

/// Size of the kernel heap set up at boot
pub const DEFAULT_HEAP_SIZE: usize = 16 * 1024 * 1024;

const PAGE: usize = 4096;
const MIN_CLASS: usize = 16;
const MAX_CLASS: usize = 2048;
const CLASSES: usize = 8; // 16, 32, ..., 2048

struct FreeBlock {
    next: *mut FreeBlock,
}

/// A run of free pages; the header sits in its first page
struct Run {
    size: usize,
    next: *mut Run,
}

struct Heap {
    start: usize,
    size: usize,
    /// Free page runs, sorted by address
    runs: *mut Run,
    small: [*mut FreeBlock; CLASSES],
    /// Bytes currently handed out (after rounding)
    used: usize,
}

fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

/// Size class index for a small request
fn class_of(size: usize, align: usize) -> Option<usize> {
    let need = size.max(align).max(MIN_CLASS).next_power_of_two();
    if need > MAX_CLASS {
        return None;
    }
    Some(need.trailing_zeros() as usize - MIN_CLASS.trailing_zeros() as usize)
}

impl Heap {
    const fn new() -> Self {
        Self { start: 0, size: 0, runs: null_mut(), small: [null_mut(); CLASSES], used: 0 }
    }

    unsafe fn init(&mut self, start: usize, size: usize) {
        let s = align_up(start, PAGE);
        let size = (size - (s - start)) & !(PAGE - 1);
        self.start = s;
        self.size = size;
        self.small = [null_mut(); CLASSES];
        self.used = 0;
        if size == 0 {
            self.runs = null_mut();
            return;
        }
        let r = s as *mut Run;
        (*r).size = size;
        (*r).next = null_mut();
        self.runs = r;
    }

    /// First fit: `bytes` (a multiple of PAGE) aligned to `align` (>= PAGE)
    unsafe fn alloc_pages(&mut self, bytes: usize, align: usize) -> *mut u8 {
        let mut link: *mut *mut Run = &mut self.runs;
        while !(*link).is_null() {
            let r = *link;
            let (rstart, rend) = (r as usize, r as usize + (*r).size);
            let a = align_up(rstart, align);
            if a.checked_add(bytes).map_or(false, |e| e <= rend) {
                // Replace run r by what is left before and after [a, a+bytes)
                let mut rest = (*r).next;
                let tail = rend - (a + bytes);
                if tail > 0 {
                    let t = (a + bytes) as *mut Run;
                    (*t).size = tail;
                    (*t).next = rest;
                    rest = t;
                }
                if a > rstart {
                    (*r).size = a - rstart;
                    (*r).next = rest;
                    rest = r;
                }
                *link = rest;
                return a as *mut u8;
            }
            link = &mut (*r).next;
        }
        null_mut()
    }

    unsafe fn free_pages(&mut self, addr: usize, bytes: usize) {
        let mut prev: *mut Run = null_mut();
        let mut cur = self.runs;
        while !cur.is_null() && (cur as usize) < addr {
            prev = cur;
            cur = (*cur).next;
        }
        let node = addr as *mut Run;
        (*node).size = bytes;
        (*node).next = cur;
        if prev.is_null() {
            self.runs = node;
        } else {
            (*prev).next = node;
        }
        if !cur.is_null() && addr + bytes == cur as usize {
            (*node).size += (*cur).size;
            (*node).next = (*cur).next;
        }
        if !prev.is_null() && prev as usize + (*prev).size == addr {
            (*prev).size += (*node).size;
            (*prev).next = (*node).next;
        }
    }

    unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        let size = size.max(1);
        match class_of(size, align) {
            Some(c) => {
                let block = MIN_CLASS << c;
                if self.small[c].is_null() {
                    let page = self.alloc_pages(PAGE, PAGE);
                    if page.is_null() {
                        return null_mut();
                    }
                    // Carve the page, lowest block first in the list
                    let mut head = null_mut();
                    let mut off = PAGE;
                    while off >= block {
                        off -= block;
                        let b = page.add(off) as *mut FreeBlock;
                        (*b).next = head;
                        head = b;
                    }
                    self.small[c] = head;
                }
                let b = self.small[c];
                self.small[c] = (*b).next;
                self.used += block;
                b as *mut u8
            }
            None => {
                let bytes = align_up(size, PAGE);
                let p = self.alloc_pages(bytes, align.max(PAGE));
                if !p.is_null() {
                    self.used += bytes;
                }
                p
            }
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, size: usize, align: usize) {
        let addr = ptr as usize;
        if ptr.is_null() || addr < self.start || addr >= self.start + self.size {
            return;
        }
        let size = size.max(1);
        match class_of(size, align) {
            Some(c) => {
                let b = ptr as *mut FreeBlock;
                (*b).next = self.small[c];
                self.small[c] = b;
                self.used -= MIN_CLASS << c;
            }
            None => {
                let bytes = align_up(size, PAGE);
                self.free_pages(addr, bytes);
                self.used -= bytes;
            }
        }
    }

    unsafe fn free_page_bytes(&self) -> usize {
        let mut n = 0;
        let mut r = self.runs;
        while !r.is_null() {
            n += (*r).size;
            r = (*r).next;
        }
        n
    }
}

/// The heap behind a spin lock taken with interrupts disabled
pub struct KernelHeap {
    lock: AtomicBool,
    heap: UnsafeCell<Heap>,
}

unsafe impl Sync for KernelHeap {}

impl KernelHeap {
    pub const fn new() -> Self {
        Self { lock: AtomicBool::new(false), heap: UnsafeCell::new(Heap::new()) }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Heap) -> R) -> R {
        let flags: u64;
        unsafe { core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem)) };
        while self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        let r = f(unsafe { &mut *self.heap.get() });
        self.lock.store(false, Ordering::Release);
        if flags & (1 << 9) != 0 {
            unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
        }
        r
    }
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with(|h| h.alloc(layout.size(), layout.align()))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.with(|h| h.dealloc(ptr, layout.size(), layout.align()))
    }
}

/// The global allocator (only in the kernel binary)
#[cfg_attr(feature = "standalone", global_allocator)]
static HEAP: KernelHeap = KernelHeap::new();

/// Hand the heap its memory. Call once, before the first allocation.
///
/// # Safety
/// The region must be mapped, writable, and used for nothing else.
pub unsafe fn init(heap_start: usize, heap_size: usize) {
    HEAP.with(|h| h.init(heap_start, heap_size));
}

/// Like [`init`], rounding the start and size up to whole pages; returns
/// the start used.
///
/// # Safety
/// As for [`init`].
pub unsafe fn init_aligned(start_addr: usize, size: usize) -> usize {
    let s = align_up(start_addr, PAGE);
    init(s, align_up(size, PAGE));
    s
}

/// Allocate `size` bytes aligned to `align` (null on failure)
///
/// # Safety
/// Free with [`deallocate`] and the same size and alignment.
pub unsafe fn allocate(size: usize, align: usize) -> *mut u8 {
    HEAP.with(|h| h.alloc(size, align))
}

/// Free memory from [`allocate`]
///
/// # Safety
/// `ptr`, `size` and `align` must match the allocation.
pub unsafe fn deallocate(ptr: *mut u8, size: usize, align: usize) {
    HEAP.with(|h| h.dealloc(ptr, size, align))
}

/// Bytes currently allocated (after rounding)
pub fn heap_usage() -> usize {
    HEAP.with(|h| h.used)
}

/// Total heap size in bytes
pub fn heap_size() -> usize {
    HEAP.with(|h| h.size)
}

/// Bytes in free page runs (not counting free small blocks)
pub fn heap_available() -> usize {
    HEAP.with(|h| unsafe { h.free_page_bytes() })
}

/// Print usage to the debug console
pub fn heap_print_summary() {
    use core::fmt::Write;
    struct Out;
    impl Write for Out {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            crate::kobj::debugcon(s);
            Ok(())
        }
    }
    let _ = writeln!(Out, "[HEAP] {} KiB used, {} KiB in free page runs, {} KiB total",
                     heap_usage() / 1024, heap_available() / 1024, heap_size() / 1024);
}

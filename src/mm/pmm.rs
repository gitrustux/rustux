// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Physical Memory Manager (PMM)
//!
//! # Design
//!
//! - Physical memory is split into arenas. At boot they are built from the
//!   free (`EfiConventionalMemory`) regions of the UEFI memory map, which
//!   `main.rs` records with [`note_usable_memory`] after ExitBootServices.
//!   Arenas never cross 4 GiB, so callers can ask for memory below 4 GiB
//!   (DMA for 32-bit devices).
//! - Every page has a [`Page`] record. The records for all arenas are
//!   carved out of RAM (24 bytes per 4 KiB page, about 0.6%) and accessed
//!   through the kernel's direct map, so they are valid in every address
//!   space.
//! - Allocation scans from a per-arena hint (O(1) for the common case), and
//!   prefers memory above 4 GiB so low memory stays available for DMA.
//! - Without a recorded memory map (e.g. the `kernel_test` entry point) the
//!   old fixed zones are used via [`pmm_add_arena`].
//!
//! # Usage
//!
//! ```rust
//! // Allocate a single page
//! let page = pmm::pmm_alloc_page(0)?;
//!
//! // Free a page
//! pmm::pmm_free_page(page);
//!
//! // Count free pages
//! let free = pmm::pmm_count_free_pages();
//! ```

use crate::arch::amd64::mm::{
    PAddr,
    VAddr,
    RxStatus,
    RxResult,
    page_tables::PAGE_SIZE
};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Global PMM allocation call counter
static ALLOC_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Page size shift for quick division/multiplication
pub const PAGE_SIZE_SHIFT: u8 = 12;

/// Mask for page-aligned addresses
pub const PAGE_MASK: usize = PAGE_SIZE - 1;


/// Check if an address is page-aligned
#[inline]
pub const fn is_page_aligned(addr: usize) -> bool {
    (addr & PAGE_MASK) == 0
}

/// Align an address down to page boundary
#[inline]
pub const fn align_page_down(addr: usize) -> usize {
    addr & !PAGE_MASK
}

/// Align an address up to page boundary
#[inline]
pub const fn align_page_up(addr: usize) -> usize {
    (addr + PAGE_MASK) & !PAGE_MASK
}

/// Convert bytes to number of pages (rounding up)
#[inline]
pub const fn bytes_to_pages(bytes: usize) -> usize {
    (bytes + PAGE_MASK) / PAGE_SIZE
}

/// Convert pages to bytes
#[inline]
pub const fn pages_to_bytes(pages: usize) -> usize {
    pages << PAGE_SIZE_SHIFT
}

/// Page state enumeration
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// Page is free and can be allocated
    Free = 0,

    /// Page is allocated (general purpose)
    Allocated = 1,

    /// Page is reserved (cannot be allocated)
    Reserved = 2,
}

impl PageState {
    /// Check if this page is free
    pub fn is_free(&self) -> bool {
        *self == PageState::Free
    }
}

/// Physical memory arena information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArenaInfo {
    /// Arena name (e.g., "low_mem", "high_mem")
    pub name: [u8; 16],

    /// Arena flags (e.g., LOW_MEM, HIGH_MEM)
    pub flags: u32,

    /// Arena allocation priority (lower = higher priority)
    pub priority: u32,

    /// Base physical address
    pub base: PAddr,

    /// Size in bytes
    pub size: usize,
}

impl ArenaInfo {
    /// Create a new arena info structure
    pub const fn new(name: &[u8], flags: u32, priority: u32, base: PAddr, size: usize) -> Self {
        let mut name_bytes = [0u8; 16];
        let mut i = 0;
        while i < 16 && i < name.len() {
            name_bytes[i] = name[i];
            i += 1;
        }

        Self {
            name: name_bytes,
            flags,
            priority,
            base,
            size,
        }
    }

    /// Get the number of pages in this arena
    pub fn page_count(&self) -> usize {
        self.size / PAGE_SIZE
    }

    /// Get the end physical address (exclusive)
    pub fn end(&self) -> PAddr {
        self.base + self.size as PAddr
    }
}

/// Arena allocation flags
pub const ARENA_FLAG_LOW_MEM: u32 = 0x1;   // Arena is in "low memory" (< 4GB)
pub const ARENA_FLAG_HIGH_MEM: u32 = 0x2;  // Arena is in "high memory" (>= 4GB)
pub const ARENA_FLAG_KERNEL: u32 = 0x4;   // Arena is for kernel metadata (page tables, heap)
pub const ARENA_FLAG_USER: u32 = 0x8;     // Arena is for user memory (VMO backing pages)

/// PMM allocation flags
pub const PMM_ALLOC_FLAG_ANY: u32 = 0x0;       // Allocate from any arena
pub const PMM_ALLOC_FLAG_LOW_MEM: u32 = 0x1;   // Allocate only from low memory arenas
pub const PMM_ALLOC_FLAG_KERNEL: u32 = 0x4;    // Allocate from kernel zone only
pub const PMM_ALLOC_FLAG_USER: u32 = 0x8;      // Allocate from user zone only

/// Memory zone definitions
///
/// These define the physical memory regions for kernel vs user allocations.
/// This prevents VMO clone operations from corrupting kernel heap metadata.
///
/// # Important Invariant
///
/// Kernel metadata (heap, page tables, allocator structures) must NEVER
/// live in the same physical region as user-visible memory (VMO backing pages).
///
/// Kernel Zone: 0x00100000 - 0x00FFFFFF (16 MB)
///   - Kernel heap
///   - Page tables
///   - Kernel metadata structures
///
/// User Zone: 0x01000000 - 0x7FFFFFFF (2 GB+)
///   - VMO backing pages
///   - User data
///   - Clone destinations
pub const KERNEL_ZONE_START: PAddr = 0x0010_0000;  // 1 MB
pub const KERNEL_ZONE_END: PAddr = 0x00FF_FFFF;    // 16 MB
pub const USER_ZONE_START: PAddr = 0x0100_0000;    // 16 MB
pub const USER_ZONE_END: PAddr = 0x7FFF_FFFF;      // 2 GB

/// Page structure tracking a single physical page
#[repr(C)]
#[derive(Debug)]
pub struct Page {
    /// Physical address of this page
    pub paddr: PAddr,

    /// Current state of the page
    pub state: PageState,

    /// Reference count for shared pages
    pub ref_count: u32,

    /// Arena index this page belongs to
    pub arena_index: u8,

    /// Page index within arena
    pub page_index: u32,
}

impl Page {
    /// Create a new page structure
    pub const fn new(paddr: PAddr, arena_index: u8, page_index: u32) -> Self {
        Self {
            paddr,
            state: PageState::Free,
            ref_count: 0,
            arena_index,
            page_index,
        }
    }

    /// Check if this page is free
    pub fn is_free(&self) -> bool {
        self.state.is_free()
    }
}

/// Maximum number of arenas
const MAX_ARENAS: usize = 32;

/// 4 GiB: arenas are split here
const FOUR_GIB: PAddr = 0x1_0000_0000;

/// Memory arena structure
struct Arena {
    /// Arena information
    info: ArenaInfo,

    /// Page records: physical address (accessed through the direct map)
    /// when `pages_phys != 0`, otherwise a kernel-image pointer.
    pages_phys: PAddr,
    pages_ptr: *mut Page,

    /// Total number of pages
    total_count: u64,

    /// Number of free pages
    free_count: u64,

    /// Index to start the next single-page search from
    next_free: usize,
}

impl Arena {
    const fn empty() -> Self {
        Self {
            info: ArenaInfo::new(b"empty", 0, 0, 0, 0),
            pages_phys: 0,
            pages_ptr: core::ptr::null_mut(),
            total_count: 0,
            free_count: 0,
            next_free: 0,
        }
    }

    /// The page records of this arena
    fn pages(&mut self) -> &mut [Page] {
        let ptr = if self.pages_phys != 0 {
            crate::arch::amd64::mm::kernel_map::phys_to_virt(self.pages_phys) as *mut Page
        } else {
            self.pages_ptr
        };
        if ptr.is_null() {
            return &mut [];
        }
        unsafe { core::slice::from_raw_parts_mut(ptr, self.total_count as usize) }
    }

    fn index_of(&self, paddr: PAddr) -> Option<usize> {
        if !self.address_in_arena(paddr) {
            return None;
        }
        let index = ((paddr - self.info.base) / PAGE_SIZE as PAddr) as usize;
        if index < self.total_count as usize { Some(index) } else { None }
    }

    /// Allocate a single page from this arena
    fn alloc_page(&mut self) -> Option<PAddr> {
        if self.free_count == 0 {
            return None;
        }
        let total = self.total_count as usize;
        let start = if self.next_free < total { self.next_free } else { 0 };
        let pages = self.pages();
        let mut found = None;
        for k in 0..total {
            let i = (start + k) % total;
            if pages[i].is_free() {
                pages[i].state = PageState::Allocated;
                pages[i].ref_count = 1;
                found = Some((i, pages[i].paddr));
                break;
            }
        }
        let (i, paddr) = found?;
        self.free_count -= 1;
        self.next_free = i + 1;
        Some(paddr)
    }

    /// Allocate `count` contiguous pages whose start is aligned to
    /// `align_pages` pages
    fn alloc_contiguous(&mut self, count: usize, align_pages: usize) -> Option<PAddr> {
        if (self.free_count as usize) < count {
            return None;
        }
        let total = self.total_count as usize;
        let base_page = (self.info.base / PAGE_SIZE as PAddr) as usize;
        let align = core::cmp::max(align_pages, 1);
        let pages = self.pages();

        // First index whose physical page number is a multiple of `align`
        let mut start = (align - base_page % align) % align;
        while start + count <= total {
            match (start..start + count).rev().find(|&j| !pages[j].is_free()) {
                Some(busy) => {
                    // Skip past the busy page, keeping the alignment
                    let next = busy + 1;
                    start = next + (align - (base_page + next) % align) % align;
                }
                None => {
                    for p in &mut pages[start..start + count] {
                        p.state = PageState::Allocated;
                        p.ref_count = 1;
                    }
                    let paddr = pages[start].paddr;
                    self.free_count -= count as u64;
                    return Some(paddr);
                }
            }
        }
        None
    }

    /// Free a page back to this arena
    fn free_page(&mut self, paddr: PAddr) -> RxStatus {
        if (paddr - self.info.base) % PAGE_SIZE as PAddr != 0 {
            return RxStatus::ERR_INVALID_ARGS;
        }
        let index = match self.index_of(paddr) {
            Some(i) => i,
            None => return RxStatus::ERR_INVALID_ARGS,
        };
        let pages = self.pages();
        if pages[index].state != PageState::Allocated {
            // Double free or freeing a reserved page: ignore
            return RxStatus::ERR_INVALID_ARGS;
        }
        pages[index].state = PageState::Free;
        pages[index].ref_count = 0;
        self.free_count += 1;
        if index < self.next_free {
            self.next_free = index;
        }
        RxStatus::OK
    }

    /// Mark a page reserved (never allocated)
    fn reserve_page(&mut self, paddr: PAddr) {
        if let Some(index) = self.index_of(paddr) {
            let pages = self.pages();
            let was_free = pages[index].is_free();
            pages[index].state = PageState::Reserved;
            if was_free {
                self.free_count -= 1;
            }
        }
    }

    /// Check if a physical address is within this arena
    fn address_in_arena(&self, addr: PAddr) -> bool {
        addr >= self.info.base && addr < (self.info.base + self.info.size as PAddr)
    }

    /// Does this arena satisfy the allocation flags?
    fn matches(&self, flags: u32) -> bool {
        if flags & PMM_ALLOC_FLAG_LOW_MEM != 0 && self.info.flags & ARENA_FLAG_LOW_MEM == 0 {
            return false;
        }
        if flags & PMM_ALLOC_FLAG_KERNEL != 0 && self.info.flags & ARENA_FLAG_KERNEL == 0 {
            return false;
        }
        if flags & PMM_ALLOC_FLAG_USER != 0 && self.info.flags & ARENA_FLAG_USER == 0 {
            return false;
        }
        true
    }
}

/// All PMM state, behind one lock taken with interrupts off (pages are
/// allocated and freed from interrupt handlers too, e.g. USB hot-plug on
/// the timer tick, and from several CPUs).
struct PmmState {
    arenas: [Arena; MAX_ARENAS],
    num_arenas: usize,
    /// Free regions from the firmware memory map (before the arenas exist)
    regions: [Region; MAX_REGIONS],
    region_count: usize,
    regions_dropped: usize,
    /// Set once the arenas were built from the firmware memory map
    from_memory_map: bool,
    /// Allocator for page records before the heap exists (fallback path)
    boot_alloc: Option<BootAllocFn>,
}

// The page-record pointers inside the arenas point at memory owned by the
// PMM; access goes through the lock
unsafe impl Send for PmmState {}

static PMM: crate::sync::IrqMutex<PmmState> = crate::sync::IrqMutex::new(PmmState {
    arenas: [const { Arena::empty() }; MAX_ARENAS],
    num_arenas: 0,
    regions: [Region { base: 0, end: 0 }; MAX_REGIONS],
    region_count: 0,
    regions_dropped: 0,
    from_memory_map: false,
    boot_alloc: None,
});

impl PmmState {
    /// Arena indices in allocation order: memory above 4 GiB first, so
    /// that low memory is left for devices that can only address 32 bits
    fn order(&self) -> ([usize; MAX_ARENAS], usize) {
        let mut idx = [0usize; MAX_ARENAS];
        let mut n = 0;
        for high in [true, false] {
            for i in 0..self.num_arenas {
                if (self.arenas[i].info.flags & ARENA_FLAG_HIGH_MEM != 0) == high {
                    idx[n] = i;
                    n += 1;
                }
            }
        }
        (idx, n)
    }

    fn arena_for(&mut self, paddr: PAddr) -> Option<&mut Arena> {
        let n = self.num_arenas;
        self.arenas[..n].iter_mut().find(|a| a.address_in_arena(paddr))
    }

    fn free_pages(&self) -> u64 {
        self.arenas[..self.num_arenas].iter().map(|a| a.free_count).sum()
    }

    fn total_pages(&self) -> u64 {
        self.arenas[..self.num_arenas].iter().map(|a| a.total_count).sum()
    }
}

// ============================================================================
// Building the PMM from the UEFI memory map
// ============================================================================

/// A free physical memory region (from the firmware memory map)
#[derive(Clone, Copy, Debug)]
struct Region {
    base: PAddr,
    end: PAddr,
}

/// Maximum regions recorded from the memory map
const MAX_REGIONS: usize = 256;


/// Take `count` contiguous pages below 1 MiB (not page 0) out of the
/// recorded free regions, for the SMP start-up trampoline. Call after the
/// memory map was recorded and before the PMM is built from it; the pages
/// are then never handed out.
pub fn take_low_pages(count: usize) -> Option<PAddr> {
    const ONE_MIB: PAddr = 0x10_0000;
    let need = (count * PAGE_SIZE) as PAddr;
    let mut g = PMM.lock();
    let st = &mut *g;
    for r in st.regions[..st.region_count].iter_mut() {
        let start = r.base.max(PAGE_SIZE as PAddr);
        if start + need <= r.end.min(ONE_MIB) {
            if start == r.base {
                r.base += need;
            } else {
                // Region starts at page 0: keep page 0 out, use what follows
                r.base = start + need;
            }
            return Some(start);
        }
    }
    None
}

/// True if the PMM was built from the firmware memory map (as opposed to
/// the fixed fallback zones)
pub fn pmm_from_memory_map() -> bool {
    PMM.lock().from_memory_map
}

/// Record a free region of physical memory (call for every
/// `EfiConventionalMemory` descriptor after ExitBootServices, before
/// [`pmm_init_from_memory_map`]).
pub fn note_usable_memory(base: PAddr, page_count: u64) {
    let mut g = PMM.lock();
    let st = &mut *g;
    {
        if st.region_count >= MAX_REGIONS {
            st.regions_dropped += 1;
            return;
        }
        st.regions[st.region_count] = Region { base, end: base + page_count * PAGE_SIZE as PAddr };
        st.region_count += 1;
    }
}

/// Every memory-map range that is not device memory (free RAM, firmware
/// and ACPI data, the kernel image, reserved areas), merged where they
/// touch. Physical VMOs must not overlap them.
static RAM_MAP: crate::sync::SpinMutex<([Region; 128], usize)> = crate::sync::SpinMutex::new(([Region { base: 0, end: 0 }; 128], 0));

/// Record a non-device range of the firmware memory map
pub fn note_ram(base: PAddr, page_count: u64) {
    let end = base + page_count * PAGE_SIZE as PAddr;
    let mut g = RAM_MAP.lock();
    let (regions, n) = &mut *g;
    if *n > 0 {
        let full = *n == regions.len();
        let last = &mut regions[*n - 1];
        if base <= last.end && end >= last.base {
            last.base = last.base.min(base);
            last.end = last.end.max(end);
            return;
        }
        if full {
            // Full: grow the last range over the gap (refuses more, never less)
            last.base = last.base.min(base);
            last.end = last.end.max(end);
            return;
        }
    }
    regions[*n] = Region { base, end };
    *n += 1;
}

/// Whether [base, end) touches RAM or firmware memory (true if no memory
/// map was recorded: then nothing can be proven to be device memory)
pub fn overlaps_ram(base: PAddr, end: PAddr) -> bool {
    let g = RAM_MAP.lock();
    let (regions, n) = &*g;
    if *n == 0 {
        return true;
    }
    regions[..*n].iter().any(|r| base < r.end && r.base < end)
}

/// Lowest physical address handed out: the first 1 MiB holds real-mode
/// structures and firmware data on PCs.
const MIN_USABLE: PAddr = 0x10_0000;

fn dbg(s: &str) {
    for &b in s.as_bytes() {
        unsafe { crate::klog::put(b) };
    }
}

fn dbg_hex(v: u64) {
    let h = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    let mut n = v;
    let mut i = 0;
    loop {
        buf[i] = h[(n & 0xF) as usize];
        n >>= 4;
        i += 1;
        if n == 0 {
            break;
        }
    }
    dbg("0x");
    while i > 0 {
        i -= 1;
        unsafe { crate::klog::put(buf[i]) };
    }
}

fn dbg_dec(v: u64) {
    let mut buf = [0u8; 20];
    let mut n = v;
    let mut i = 0;
    loop {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
        if n == 0 {
            break;
        }
    }
    while i > 0 {
        i -= 1;
        unsafe { crate::klog::put(buf[i]) };
    }
}

/// Build the arenas from the regions recorded with [`note_usable_memory`].
///
/// Returns false (and does nothing) if no regions were recorded.
///
/// # Safety
/// Call once, before any allocation, while all physical memory is
/// reachable at `phys_to_virt` (UEFI identity map or the direct map).
pub unsafe fn pmm_init_from_memory_map() -> bool {
    let mut g = PMM.lock();
    let st = &mut *g;
    let count = st.region_count;
    if count == 0 {
        return false;
    }

    // Copy, page-align, clip the first 1 MiB
    let mut regions = [Region { base: 0, end: 0 }; MAX_REGIONS];
    let mut n = 0;
    for r in &st.regions[..count] {
        let base = core::cmp::max(align_up(r.base), MIN_USABLE);
        let end = align_down(r.end);
        if end > base {
            regions[n] = Region { base, end };
            n += 1;
        }
    }

    // Sort by base (insertion sort: n is small)
    for i in 1..n {
        let mut j = i;
        while j > 0 && regions[j - 1].base > regions[j].base {
            regions.swap(j - 1, j);
            j -= 1;
        }
    }

    // Merge touching or overlapping regions
    let mut m = 0;
    for i in 0..n {
        if m > 0 && regions[i].base <= regions[m - 1].end {
            regions[m - 1].end = core::cmp::max(regions[m - 1].end, regions[i].end);
        } else {
            regions[m] = regions[i];
            m += 1;
        }
    }
    let mut n = m;

    // Split at 4 GiB so every arena is entirely below or above it
    let mut i = 0;
    while i < n {
        let r = regions[i];
        if r.base < FOUR_GIB && r.end > FOUR_GIB && n < MAX_REGIONS {
            for k in (i + 1..n).rev() {
                regions[k + 1] = regions[k];
            }
            regions[i] = Region { base: r.base, end: FOUR_GIB };
            regions[i + 1] = Region { base: FOUR_GIB, end: r.end };
            n += 1;
        }
        i += 1;
    }

    // Too many regions: drop the smallest
    let mut dropped_bytes: u64 = 0;
    while n > MAX_ARENAS {
        let (idx, _) = regions[..n]
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| r.end - r.base)
            .unwrap();
        dropped_bytes += regions[idx].end - regions[idx].base;
        for k in idx..n - 1 {
            regions[k] = regions[k + 1];
        }
        n -= 1;
    }

    // Page records for every page of every region
    let total_pages: u64 = regions[..n].iter().map(|r| (r.end - r.base) / PAGE_SIZE as PAddr).sum();
    let meta_bytes = total_pages * core::mem::size_of::<Page>() as u64;
    let meta_pages = (meta_bytes + PAGE_SIZE as u64 - 1) / PAGE_SIZE as u64;

    // Carve them from the end of the largest region (above 4 GiB if possible)
    let (largest, _) = match regions[..n]
        .iter()
        .enumerate()
        .max_by_key(|(_, r)| (r.base >= FOUR_GIB, r.end - r.base))
    {
        Some(x) => x,
        None => return false,
    };
    if (regions[largest].end - regions[largest].base) / (PAGE_SIZE as u64) < meta_pages + 1 {
        dbg("[PMM] FATAL: no region large enough for page records\n");
        return false;
    }
    let meta_base = regions[largest].end - meta_pages * PAGE_SIZE as u64;
    regions[largest].end = meta_base;

    // Create the arenas, each with its slice of the page records
    let mut meta_next = meta_base;
    for r in &regions[..n] {
        let size = (r.end - r.base) as usize;
        if size == 0 {
            continue;
        }
        let page_count = size / PAGE_SIZE;
        let idx = st.num_arenas;
        let flags = ARENA_FLAG_KERNEL
            | ARENA_FLAG_USER
            | if r.end <= FOUR_GIB { ARENA_FLAG_LOW_MEM } else { ARENA_FLAG_HIGH_MEM };
        let name: &[u8] = if r.end <= FOUR_GIB { b"ram<4G" } else { b"ram>4G" };

        let pages = core::slice::from_raw_parts_mut(
            crate::arch::amd64::mm::kernel_map::phys_to_virt(meta_next) as *mut Page,
            page_count,
        );
        for (i, p) in pages.iter_mut().enumerate() {
            core::ptr::write(p, Page::new(r.base + (i * PAGE_SIZE) as PAddr, idx as u8, i as u32));
        }

        let arena = &mut st.arenas[idx];
        arena.info = ArenaInfo::new(name, flags, idx as u32, r.base, size);
        arena.pages_phys = meta_next;
        arena.pages_ptr = core::ptr::null_mut();
        arena.total_count = page_count as u64;
        arena.free_count = page_count as u64;
        arena.next_free = 0;
        st.num_arenas += 1;

        meta_next += (page_count * core::mem::size_of::<Page>()) as u64;
    }

    st.from_memory_map = true;

    // Report
    dbg("[PMM] UEFI memory map: ");
    dbg_dec(count as u64);
    dbg(" free regions -> ");
    dbg_dec(st.num_arenas as u64);
    dbg(" arenas\n");
    for a in &st.arenas[..st.num_arenas] {
        dbg("[PMM]   ");
        dbg_hex(a.info.base);
        dbg(" - ");
        dbg_hex(a.info.end());
        dbg("  ");
        dbg_dec((a.info.size / (1024 * 1024)) as u64);
        dbg(" MiB\n");
    }
    dbg("[PMM] Usable: ");
    dbg_dec((st.total_pages() * PAGE_SIZE as u64) / (1024 * 1024));
    dbg(" MiB, page records: ");
    dbg_dec(meta_pages * PAGE_SIZE as u64 / 1024);
    dbg(" KiB at ");
    dbg_hex(meta_base);
    dbg("\n");
    if st.regions_dropped > 0 || dropped_bytes > 0 {
        dbg("[PMM] WARNING: ignored ");
        dbg_dec(st.regions_dropped as u64);
        dbg(" map entries and ");
        dbg_dec(dropped_bytes / 1024);
        dbg(" KiB in small regions (arena limit)\n");
    }
    true
}

const fn align_up(v: PAddr) -> PAddr {
    (v + PAGE_SIZE as PAddr - 1) & !(PAGE_SIZE as PAddr - 1)
}

const fn align_down(v: PAddr) -> PAddr {
    v & !(PAGE_SIZE as PAddr - 1)
}

/// Early PMM initialization with explicit low/high ranges
///
/// # Safety
///
/// Must be called exactly once during kernel boot, before any memory allocation.
pub unsafe fn pmm_init_early(
    low_base: PAddr,
    low_size: usize,
    high_base: Option<PAddr>,
    high_size: Option<usize>,
) {
    let low_info = ArenaInfo::new(b"low_mem\0", ARENA_FLAG_LOW_MEM, 0, low_base, low_size);
    let _ = pmm_add_arena(low_info);

    if let (Some(base), Some(size)) = (high_base, high_size) {
        let high_info = ArenaInfo::new(b"high_mem\0", ARENA_FLAG_HIGH_MEM, 1, base, size);
        let _ = pmm_add_arena(high_info);
    }
}

/// Add a memory arena whose page records come from the boot allocator (or
/// the heap). Used when no UEFI memory map is available.
///
/// # Returns
///
/// `RxStatus::OK` on success, or an error code
pub unsafe fn pmm_add_arena(info: ArenaInfo) -> RxStatus {
    let mut g = PMM.lock();
    let st = &mut *g;
    if st.num_arenas >= MAX_ARENAS {
        return RxStatus::ERR_NO_MEMORY;
    }

    let page_count = info.page_count();
    if page_count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }

    let pages_layout = core::alloc::Layout::array::<Page>(page_count).unwrap();
    let pages_ptr = if let Some(boot_alloc) = st.boot_alloc {
        boot_alloc(pages_layout.size(), pages_layout.align())
    } else {
        crate::mm::allocator::allocate(pages_layout.size(), pages_layout.align())
    };
    if pages_ptr.is_null() {
        return RxStatus::ERR_NO_MEMORY;
    }

    let pages = core::slice::from_raw_parts_mut(pages_ptr as *mut Page, page_count);
    for (i, p) in pages.iter_mut().enumerate() {
        core::ptr::write(p, Page::new(info.base + (i * PAGE_SIZE) as PAddr, st.num_arenas as u8, i as u32));
    }

    let arena = &mut st.arenas[st.num_arenas];
    arena.info = info;
    arena.pages_phys = 0;
    arena.pages_ptr = pages_ptr as *mut Page;
    arena.total_count = page_count as u64;
    arena.free_count = page_count as u64;
    arena.next_free = 0;

    st.num_arenas += 1;
    RxStatus::OK
}

/// Boot allocator function type
type BootAllocFn = unsafe extern "C" fn(size: usize, align: usize) -> *mut u8;

/// Set the boot allocator used by [`pmm_add_arena`] for page records
///
/// # Safety
///
/// Must be called before pmm_add_arena.
pub unsafe fn set_boot_allocator(alloc: BootAllocFn) {
    PMM.lock().boot_alloc = Some(alloc);
}

fn report_exhausted(st: &PmmState, what: &str, count: usize, flags: u32) {
    let call = ALLOC_CALL_COUNT.load(Ordering::Relaxed);
    dbg("[PMM] OUT OF MEMORY: ");
    dbg(what);
    dbg(" count=");
    dbg_dec(count as u64);
    dbg(" flags=");
    dbg_hex(flags as u64);
    dbg(" after ");
    dbg_dec(call as u64);
    dbg(" allocations, free pages=");
    dbg_dec(st.free_pages());
    dbg("\n");
}

/// Allocate a single physical page
///
/// `flags`: `PMM_ALLOC_FLAG_LOW_MEM` restricts the page to below 4 GiB;
/// KERNEL/USER select arenas with those flags (all map arenas have both).
pub fn pmm_alloc_page(flags: u32) -> RxResult<PAddr> {
    ALLOC_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut g = PMM.lock();
    let (order, n) = g.order();
    for &i in &order[..n] {
        let arena = &mut g.arenas[i];
        if !arena.matches(flags) {
            continue;
        }
        if let Some(paddr) = arena.alloc_page() {
            return Ok(paddr);
        }
    }
    report_exhausted(&g, "page", 1, flags);
    Err(RxStatus::ERR_NO_MEMORY)
}

/// Allocate a page for kernel use (page tables, kernel stacks, ...)
pub fn pmm_alloc_kernel_page() -> RxResult<PAddr> {
    pmm_alloc_page(PMM_ALLOC_FLAG_KERNEL)
}

/// Allocate a page for user memory (VMO backing pages)
pub fn pmm_alloc_user_page() -> RxResult<PAddr> {
    pmm_alloc_page(PMM_ALLOC_FLAG_USER)
}

/// Allocate `count` physically contiguous pages.
///
/// `align_log2` is the log2 of the required byte alignment of the first
/// page (values up to 12 mean page alignment).
pub fn pmm_alloc_contiguous(count: usize, flags: u32, align_log2: u8) -> RxResult<PAddr> {
    if count == 0 {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    let align_pages = if align_log2 > PAGE_SIZE_SHIFT {
        1usize << (align_log2 - PAGE_SIZE_SHIFT)
    } else {
        1
    };
    if count == 1 && align_pages == 1 {
        return pmm_alloc_page(flags);
    }
    ALLOC_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut g = PMM.lock();
    let (order, n) = g.order();
    for &i in &order[..n] {
        let arena = &mut g.arenas[i];
        if !arena.matches(flags) {
            continue;
        }
        if let Some(paddr) = arena.alloc_contiguous(count, align_pages) {
            return Ok(paddr);
        }
    }
    report_exhausted(&g, "contiguous", count, flags);
    Err(RxStatus::ERR_NO_MEMORY)
}

/// Free a single physical page
pub fn pmm_free_page(paddr: PAddr) -> RxStatus {
    match PMM.lock().arena_for(paddr) {
        Some(arena) => arena.free_page(paddr),
        None => RxStatus::ERR_INVALID_ARGS,
    }
}

/// Free `count` contiguous pages starting at `paddr`
pub fn pmm_free_contiguous(paddr: PAddr, count: usize) -> RxStatus {
    if count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }
    let end_addr = paddr + (count * PAGE_SIZE) as PAddr;
    let mut g = PMM.lock();
    match g.arena_for(paddr) {
        Some(arena) if arena.address_in_arena(end_addr - 1) => {
            for i in 0..count {
                let _ = arena.free_page(paddr + (i * PAGE_SIZE) as PAddr);
            }
            RxStatus::OK
        }
        _ => RxStatus::ERR_INVALID_ARGS,
    }
}

/// Count free pages across all arenas
pub fn pmm_count_free_pages() -> u64 {
    PMM.lock().free_pages()
}

/// Mark pages as reserved so they are never allocated.
/// Pages outside every arena are ignored (they are not managed anyway).
pub fn pmm_reserve_pages(paddr: PAddr, count: usize) -> RxStatus {
    if count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }
    let mut g = PMM.lock();
    for i in 0..count {
        let page = paddr + (i * PAGE_SIZE) as PAddr;
        if let Some(arena) = g.arena_for(page) {
            arena.reserve_page(page);
        }
    }
    RxStatus::OK
}

/// Count total pages across all arenas
pub fn pmm_count_total_pages() -> u64 {
    PMM.lock().total_pages()
}

/// Total bytes managed by the PMM
pub fn pmm_count_total_bytes() -> u64 {
    pmm_count_total_pages() * PAGE_SIZE as u64
}

/// Number of arenas (0 before the PMM is initialised)
pub fn pmm_arena_count() -> usize {
    PMM.lock().num_arenas
}

/// Page record for a physical address, or null if the PMM does not manage it
pub fn paddr_to_page(paddr: PAddr) -> *mut Page {
    match PMM.lock().arena_for(paddr) {
        Some(arena) => match arena.index_of(paddr) {
            Some(i) => &mut arena.pages()[i] as *mut Page,
            None => core::ptr::null_mut(),
        },
        None => core::ptr::null_mut(),
    }
}

/// Kernel virtual address of a physical address (direct map)
pub fn paddr_to_vaddr(paddr: PAddr) -> VAddr {
    crate::arch::amd64::mm::kernel_map::phys_to_virt(paddr) as VAddr
}

/// Same as [`paddr_to_vaddr`] (kept for existing callers)
pub fn paddr_to_vaddr_user_zone(paddr: PAddr) -> VAddr {
    crate::arch::amd64::mm::kernel_map::phys_to_virt(paddr) as VAddr
}

/// Allocate a page from any arena
pub fn alloc_page() -> RxResult<PAddr> {
    pmm_alloc_page(0)
}

/// Free a page
pub fn free_page(paddr: PAddr) {
    let _ = pmm_free_page(paddr);
}

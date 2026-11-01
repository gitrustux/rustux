// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel address space for x86-64.
//!
//! The kernel builds its own page tables at boot instead of running on the
//! ones UEFI left behind:
//!
//! - **Higher-half direct map (HHDM)** at [`HHDM_BASE`] (PML4 entry 256):
//!   all physical memory and MMIO up to the highest address in the UEFI
//!   memory map (at least 4 GiB). The kernel reaches heap, stacks, page
//!   tables, user pages and devices through this range, so they stay
//!   reachable whichever address space is active.
//! - **Boot identity map** in PML4 entry 0 of the kernel's own page tables
//!   only, because the kernel image still runs at the physical address UEFI
//!   loaded it to.
//!
//! User address spaces ([`new_user_pml4`]) share PML4 entries 256–511 with the
//! kernel and get a private lower half that contains only the kernel image,
//! mapped supervisor-only. User programs can therefore use any lower-half
//! address that does not overlap the kernel image.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Base of the higher-half direct map of physical memory.
pub const HHDM_BASE: u64 = 0xffff_8000_0000_0000;

const PAGE_SIZE: u64 = 4096;
const ENTRIES: usize = 512;

// Page-table entry flags
pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
const PTE_HUGE: u64 = 1 << 7;
pub const PTE_NO_EXECUTE: u64 = 1 << 63;
/// Page-level write-through / cache-disable (uncached when both set)
pub const PTE_PWT: u64 = 1 << 3;
pub const PTE_PCD: u64 = 1 << 4;

/// Device register (MMIO) window: PML4 entry 257. Its PDPT is created by
/// [`init`], so every address space (which copies PML4 256-511 when it is
/// created) shares the mappings added later by [`map_mmio`].
pub const MMIO_BASE: u64 = 0xffff_8080_0000_0000;
const MMIO_SIZE: u64 = 1 << 39;
static MMIO_NEXT: AtomicU64 = AtomicU64::new(MMIO_BASE);
const PTE_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

static HHDM_READY: AtomicBool = AtomicBool::new(false);
static KERNEL_PML4: AtomicU64 = AtomicU64::new(0);
static KERNEL_IMAGE_BASE: AtomicU64 = AtomicU64::new(0);
static KERNEL_IMAGE_SIZE: AtomicU64 = AtomicU64::new(0);
static PHYS_LIMIT: AtomicU64 = AtomicU64::new(0);

/// Record where UEFI loaded the kernel image (call before ExitBootServices).
pub fn set_kernel_image(base: u64, size: u64) {
    KERNEL_IMAGE_BASE.store(base, Ordering::Relaxed);
    KERNEL_IMAGE_SIZE.store(size, Ordering::Relaxed);
}

/// Record the highest physical address that must be reachable (end of the
/// highest UEFI memory map entry, framebuffer, ...). Can be called several
/// times; the maximum wins.
pub fn note_phys_limit(end: u64) {
    PHYS_LIMIT.fetch_max(end, Ordering::Relaxed);
}

/// Kernel image range `(base, size)` as reported by UEFI.
pub fn kernel_image() -> (u64, u64) {
    (
        KERNEL_IMAGE_BASE.load(Ordering::Relaxed),
        KERNEL_IMAGE_SIZE.load(Ordering::Relaxed),
    )
}

/// Physical address of the kernel's PML4 (0 before [`init`]).
pub fn kernel_pml4() -> u64 {
    KERNEL_PML4.load(Ordering::Acquire)
}

/// True once the HHDM is active.
pub fn hhdm_ready() -> bool {
    HHDM_READY.load(Ordering::Acquire)
}

/// Convert a physical address to the kernel virtual address used to access it.
///
/// Before [`init`] this is the identity (UEFI's 1:1 map); afterwards it is the
/// higher-half direct map, which is present in every address space.
#[inline]
pub fn phys_to_virt(paddr: u64) -> u64 {
    if hhdm_ready() {
        HHDM_BASE + paddr
    } else {
        paddr
    }
}

/// Convert a kernel virtual address back to physical. Accepts HHDM addresses
/// and (identity-mapped) kernel image addresses.
#[inline]
pub fn virt_to_phys(vaddr: u64) -> u64 {
    if vaddr >= HHDM_BASE {
        vaddr - HHDM_BASE
    } else {
        vaddr
    }
}

/// Allocates a zeroed 4 KiB page for page tables. Returns its physical address.
type AllocFn = fn() -> Option<u64>;

unsafe fn zero_page(paddr: u64) {
    core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, PAGE_SIZE as usize);
}

unsafe fn table(paddr: u64) -> *mut u64 {
    phys_to_virt(paddr) as *mut u64
}

fn cpu_has_1g_pages() -> bool {
    // CPUID.80000001H:EDX[26] = Page1GB
    let max_ext = unsafe { core::arch::x86_64::__cpuid(0x8000_0000) }.eax;
    if max_ext < 0x8000_0001 {
        return false;
    }
    let r = unsafe { core::arch::x86_64::__cpuid(0x8000_0001) };
    r.edx & (1 << 26) != 0
}

/// Build the kernel page tables and switch to them.
///
/// `alloc` must return zero-initialisable pages that are identity-mapped at
/// the time of the call (the PMM's kernel zone).
///
/// # Safety
/// Must be called once, early in boot, while UEFI's identity map is active.
pub unsafe fn init(alloc: AllocFn) -> Result<(), &'static str> {
    // Cover at least the 4 GiB below which most MMIO (APIC, IOAPIC,
    // framebuffer) lives, and at most 512 GiB (one PDPT).
    let gib = 1u64 << 30;
    let limit = PHYS_LIMIT.load(Ordering::Relaxed).max(4 * gib);
    let limit = ((limit + gib - 1) / gib * gib).min(512 * gib);
    let gigabytes = (limit / gib) as usize;

    let pml4 = alloc().ok_or("kernel_map: out of pages (PML4)")?;
    let pdpt = alloc().ok_or("kernel_map: out of pages (PDPT)")?;
    zero_page(pml4);
    zero_page(pdpt);

    let flags = PTE_PRESENT | PTE_WRITABLE;
    let pdpt_t = table(pdpt);
    if cpu_has_1g_pages() {
        for g in 0..gigabytes {
            *pdpt_t.add(g) = (g as u64 * gib) | flags | PTE_HUGE;
        }
    } else {
        for g in 0..gigabytes {
            let pd = alloc().ok_or("kernel_map: out of pages (PD)")?;
            let pd_t = table(pd);
            for e in 0..ENTRIES {
                let phys = g as u64 * gib + e as u64 * (2 << 20);
                *pd_t.add(e) = phys | flags | PTE_HUGE;
            }
            *pdpt_t.add(g) = pd | flags;
        }
    }

    // The same PDPT serves both the higher-half direct map and the boot
    // identity map of the kernel's own address space.
    let pml4_t = table(pml4);
    *pml4_t.add(0) = pdpt | flags;
    *pml4_t.add(256) = pdpt | flags;

    // Empty PDPT for the device register window (see MMIO_BASE)
    let mmio_pdpt = alloc().ok_or("kernel_map: out of pages (MMIO PDPT)")?;
    zero_page(mmio_pdpt);
    *pml4_t.add(257) = mmio_pdpt | flags;

    core::arch::asm!("mov cr3, {}", in(reg) pml4, options(nostack));
    KERNEL_PML4.store(pml4, Ordering::Release);
    HHDM_READY.store(true, Ordering::Release);
    Ok(())
}

/// Create the top-level table for a new user address space: kernel half
/// (PML4 256–511) shared with the kernel, and a private lower half that
/// contains only the kernel image, mapped supervisor-only.
///
/// # Safety
/// [`init`] must have run.
pub unsafe fn new_user_pml4(alloc: AllocFn) -> Result<u64, &'static str> {
    let kernel = kernel_pml4();
    if kernel == 0 {
        return Err("kernel_map: not initialised");
    }
    let pml4 = alloc().ok_or("kernel_map: out of pages (user PML4)")?;
    zero_page(pml4);
    let src = table(kernel);
    let dst = table(pml4);
    for i in 256..ENTRIES {
        *dst.add(i) = *src.add(i);
    }

    // Kernel code, data and statics (GDT, IDT, TSS, ramdisk) keep running at
    // their load address while a process is active.
    let (base, size) = kernel_image();
    if size == 0 {
        return Err("kernel_map: kernel image range unknown");
    }
    let start = base & !(PAGE_SIZE - 1);
    let end = (base + size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let mut va = start;
    while va < end {
        map_4k(pml4, va, va, PTE_WRITABLE, alloc)?;
        va += PAGE_SIZE;
    }
    Ok(pml4)
}

/// Map one 4 KiB page in the lower half of `pml4`.
///
/// `flags` may contain [`PTE_WRITABLE`], [`PTE_USER`] and [`PTE_NO_EXECUTE`].
/// Intermediate tables are created as needed (writable, and user-accessible
/// so that user leaves below them work; the leaf decides the real access).
/// Never modifies tables shared with the kernel half.
///
/// # Safety
/// `pml4` must be a user PML4 from [`new_user_pml4`].
pub unsafe fn map_4k(pml4: u64, vaddr: u64, paddr: u64, flags: u64, alloc: AllocFn)
    -> Result<(), &'static str>
{
    if vaddr >= 0x0000_8000_0000_0000 {
        return Err("kernel_map: map_4k only maps the lower half");
    }
    if vaddr & (PAGE_SIZE - 1) != 0 || paddr & (PAGE_SIZE - 1) != 0 {
        return Err("kernel_map: unaligned address");
    }
    let indices = [
        ((vaddr >> 39) & 0x1ff) as usize,
        ((vaddr >> 30) & 0x1ff) as usize,
        ((vaddr >> 21) & 0x1ff) as usize,
    ];
    let mut t = table(pml4);
    for &idx in &indices {
        let entry = *t.add(idx);
        let next = if entry & PTE_PRESENT == 0 {
            let p = alloc().ok_or("kernel_map: out of pages (page table)")?;
            zero_page(p);
            *t.add(idx) = p | PTE_PRESENT | PTE_WRITABLE | PTE_USER;
            p
        } else if entry & PTE_HUGE != 0 {
            return Err("kernel_map: address covered by a large page");
        } else {
            entry & PTE_ADDR_MASK
        };
        t = table(next);
    }
    let pt_idx = ((vaddr >> 12) & 0x1ff) as usize;
    let old = *t.add(pt_idx);
    if old & PTE_PRESENT != 0 && old & PTE_ADDR_MASK != paddr {
        return Err("kernel_map: address already mapped");
    }
    *t.add(pt_idx) = paddr | PTE_PRESENT | (flags & (PTE_WRITABLE | PTE_USER | PTE_NO_EXECUTE));
    Ok(())
}

/// Map `size` bytes of device registers at physical `paddr` uncached
/// (PCD|PWT, no execute) into the MMIO window and return the kernel virtual
/// address of `paddr`. Mappings are never removed.
///
/// # Safety
/// [`init`] must have run. `paddr` must be device memory, not RAM.
pub unsafe fn map_mmio(paddr: u64, size: u64, alloc: AllocFn) -> Result<u64, &'static str> {
    let kernel = kernel_pml4();
    if kernel == 0 {
        return Err("kernel_map: not initialised");
    }
    let first = paddr & !(PAGE_SIZE - 1);
    let last = (paddr + size.max(1) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let pages = (last - first) / PAGE_SIZE;
    let va_base = MMIO_NEXT.fetch_add(pages * PAGE_SIZE, Ordering::AcqRel);
    if va_base + pages * PAGE_SIZE > MMIO_BASE + MMIO_SIZE {
        return Err("kernel_map: MMIO window full");
    }
    let flags = PTE_PRESENT | PTE_WRITABLE | PTE_PCD | PTE_PWT | PTE_NO_EXECUTE;
    for i in 0..pages {
        let vaddr = va_base + i * PAGE_SIZE;
        // PML4[257] exists (init); walk/create PDPT -> PD -> PT
        let mut t = table(*table(kernel).add(257) & PTE_ADDR_MASK);
        for shift in [30u64, 21] {
            let idx = ((vaddr >> shift) & 0x1ff) as usize;
            let entry = *t.add(idx);
            let next = if entry & PTE_PRESENT == 0 {
                let p = alloc().ok_or("kernel_map: out of pages (MMIO table)")?;
                zero_page(p);
                *t.add(idx) = p | PTE_PRESENT | PTE_WRITABLE;
                p
            } else {
                entry & PTE_ADDR_MASK
            };
            t = table(next);
        }
        *t.add(((vaddr >> 12) & 0x1ff) as usize) = (first + i * PAGE_SIZE) | flags;
        core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack));
    }
    Ok(va_base + (paddr - first))
}

/// Translate a lower-half address through `pml4` (debugging aid).
pub unsafe fn translate(pml4: u64, vaddr: u64) -> Option<u64> {
    let mut t = table(pml4);
    let shifts = [39u64, 30, 21, 12];
    for (level, &s) in shifts.iter().enumerate() {
        let e = *t.add(((vaddr >> s) & 0x1ff) as usize);
        if e & PTE_PRESENT == 0 {
            return None;
        }
        if level == 3 || e & PTE_HUGE != 0 {
            let page = 1u64 << s;
            return Some((e & PTE_ADDR_MASK & !(page - 1)) + (vaddr & (page - 1)));
        }
        t = table(e & PTE_ADDR_MASK);
    }
    None
}

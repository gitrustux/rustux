// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Starting the other CPU cores (application processors, APs).
//!
//! 1. The CPUs are listed in the ACPI MADT (found through the RSDP that
//!    UEFI gives the kernel).
//! 2. A small trampoline is copied to a page below 1 MiB, reserved from the
//!    firmware memory map before the PMM was built. An AP starts there in
//!    16-bit real mode, switches to 32-bit protected mode, turns on PAE,
//!    long mode and paging with a temporary PML4 (below 4 GiB, since CR3
//!    is loaded in 32-bit mode; it identity-maps the trampoline and shares
//!    the kernel half), and jumps to [`ap_entry`] on its own stack.
//! 3. The boot CPU wakes each AP in turn with INIT, SIPI, SIPI and waits
//!    for it to report in.
//! 4. Each AP copies the boot CPU's CR0/CR4/EFER settings (SSE, NX, ...),
//!    loads the kernel page tables, its own GDT and TSS and the shared IDT,
//!    enables its local APIC, starts its timer and enters the scheduler's
//!    idle loop.
//!
//! Also here: inter-processor interrupts to wake idle CPUs, and TLB
//! shootdowns when a mapping changes in an address space another CPU is
//! using.

use super::mm::kernel_map;
use super::percpu;
use core::sync::atomic::{AtomicU64, Ordering};

/// Physical address of the RSDP (from the UEFI configuration table)
static RSDP: AtomicU64 = AtomicU64::new(0);
/// Two pages below 1 MiB: trampoline code, then the temporary PML4
static LOW_PAGES: AtomicU64 = AtomicU64::new(0);
/// LAPIC timer initial count for a 10 ms tick (calibrated on the boot CPU)
static TIMER_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// IPI vector that only wakes a halted CPU
pub const WAKE_VECTOR: u8 = 0xF0;
/// IPI vector asking a CPU to flush its TLB
pub const FLUSH_VECTOR: u8 = 0xF1;

/// Record the boot CPU's timer calibration for the other CPUs
pub fn set_timer_count(count: u32) {
    TIMER_COUNT.store(count, Ordering::Release);
}

/// Record the RSDP address (before ExitBootServices)
pub fn set_rsdp(paddr: u64) {
    RSDP.store(paddr, Ordering::Release);
}

/// Reserve the trampoline pages (after the memory map was recorded,
/// before the PMM is built)
pub fn reserve_low_memory() {
    if let Some(p) = crate::mm::pmm::take_low_pages(2) {
        LOW_PAGES.store(p, Ordering::Release);
    }
}

// ---------------------------------------------------------------------
// ACPI: list the local APICs
// ---------------------------------------------------------------------

unsafe fn phys<T>(p: u64) -> *const T {
    kernel_map::phys_to_virt(p) as *const T
}

unsafe fn rd32(p: u64) -> u32 {
    core::ptr::read_unaligned(phys::<u32>(p))
}

unsafe fn rd64(p: u64) -> u64 {
    core::ptr::read_unaligned(phys::<u64>(p))
}

unsafe fn checksum_ok(p: u64, len: u32) -> bool {
    let b = core::slice::from_raw_parts(phys::<u8>(p), len as usize);
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
}

/// Find a table by signature through the XSDT (or the RSDT)
pub(crate) unsafe fn find_table(sig: &[u8; 4]) -> Option<u64> {
    let rsdp = RSDP.load(Ordering::Acquire);
    if rsdp == 0 || core::slice::from_raw_parts(phys::<u8>(rsdp), 8) != b"RSD PTR " {
        return None;
    }
    let revision = *phys::<u8>(rsdp + 15);
    let (root, entry_size) = if revision >= 2 && rd64(rsdp + 24) != 0 {
        (rd64(rsdp + 24), 8)
    } else {
        (rd32(rsdp + 16) as u64, 4)
    };
    let len = rd32(root + 4);
    if !checksum_ok(root, len) {
        return None;
    }
    let mut off = 36;
    while off + entry_size <= len as u64 {
        let t = if entry_size == 8 { rd64(root + off) } else { rd32(root + off) as u64 };
        if t != 0 && core::slice::from_raw_parts(phys::<u8>(t), 4) == sig && checksum_ok(t, rd32(t + 4)) {
            return Some(t);
        }
        off += entry_size;
    }
    None
}

/// LAPIC IDs of the usable CPUs in the MADT (boot CPU included)
fn madt_apic_ids() -> ([u32; percpu::MAX_CPUS * 4], usize) {
    let mut ids = [0u32; percpu::MAX_CPUS * 4];
    let mut n = 0;
    unsafe {
        let Some(madt) = find_table(b"APIC") else { return (ids, 0) };
        let len = rd32(madt + 4) as u64;
        let mut off = 44;
        while off + 2 <= len {
            let p = madt + off;
            let (ty, elen) = (*phys::<u8>(p), *phys::<u8>(p + 1) as u64);
            if elen < 2 {
                break;
            }
            // Flags bit 0: enabled. (Bit 1, "online capable", marks CPUs
            // that may be added later; there may be nothing behind them.)
            let id = match ty {
                0 if rd32(p + 4) & 1 != 0 => Some(*phys::<u8>(p + 3) as u32),
                9 if rd32(p + 8) & 1 != 0 => Some(rd32(p + 4)), // x2APIC
                _ => None,
            };
            if let Some(id) = id {
                if id < 255 && n < ids.len() && !ids[..n].contains(&id) {
                    ids[n] = id;
                    n += 1;
                }
            }
            off += elen;
        }
    }
    (ids, n)
}

// ---------------------------------------------------------------------
// Trampoline
// ---------------------------------------------------------------------

core::arch::global_asm!(
    ".att_syntax prefix",
    ".section .rdata$ap_tramp, \"dr\"",
    ".p2align 12",
    ".global rustux_ap_tramp_start",
    "rustux_ap_tramp_start:",
    ".code16",
    "cli",
    "cld",
    "movw %cs, %ax",
    "movw %ax, %ds",
    "xorl %ebx, %ebx",
    "movw %cs, %bx",
    "shll $4, %ebx",                       // ebx = physical base of this page
    "lgdtl (rustux_ap_gdt_ptr - rustux_ap_tramp_start)",
    "movl %cr0, %eax",
    "orl $1, %eax",
    "movl %eax, %cr0",
    "ljmpl *(rustux_ap_far32 - rustux_ap_tramp_start)",
    ".code32",
    "rustux_ap_pm32:",
    "movw $0x10, %ax",
    "movw %ax, %ds",
    "movw %ax, %es",
    "movw %ax, %ss",
    "movl %cr4, %eax",
    "orl $0x20, %eax",                     // PAE
    "movl %eax, %cr4",
    "movl (rustux_ap_pml4 - rustux_ap_tramp_start)(%ebx), %eax",
    "movl %eax, %cr3",
    "movl $0xC0000080, %ecx",              // EFER
    "rdmsr",
    "orl (rustux_ap_efer - rustux_ap_tramp_start)(%ebx), %eax",
    "wrmsr",
    "movl %cr0, %eax",
    "orl $0x80000000, %eax",               // paging -> long mode
    "movl %eax, %cr0",
    "ljmpl *(rustux_ap_far64 - rustux_ap_tramp_start)(%ebx)",
    ".code64",
    "rustux_ap_long64:",
    "movq (rustux_ap_cr4 - rustux_ap_tramp_start)(%rbx), %rax",
    "movq %rax, %cr4",
    "movq (rustux_ap_cr0 - rustux_ap_tramp_start)(%rbx), %rax",
    "movq %rax, %cr0",
    "fninit",
    "movq (rustux_ap_stack - rustux_ap_tramp_start)(%rbx), %rsp",
    "movq (rustux_ap_arg - rustux_ap_tramp_start)(%rbx), %rdi",
    "movq (rustux_ap_entry - rustux_ap_tramp_start)(%rbx), %rax",
    "jmpq *%rax",
    ".p2align 3",
    "rustux_ap_gdt:",
    ".quad 0",
    ".quad 0x00CF9A000000FFFF",            // 0x08: 32-bit code
    ".quad 0x00CF92000000FFFF",            // 0x10: data
    ".quad 0x00AF9A000000FFFF",            // 0x18: 64-bit code
    "rustux_ap_gdt_end:",
    ".p2align 3",
    ".global rustux_ap_gdt_ptr",
    "rustux_ap_gdt_ptr:",
    ".word rustux_ap_gdt_end - rustux_ap_gdt - 1",
    ".long 0",                             // patched: base + gdt offset
    ".p2align 3",
    "rustux_ap_far32:",
    ".long 0",                             // patched: base + pm32 offset
    ".word 0x08",
    ".p2align 3",
    "rustux_ap_far64:",
    ".long 0",                             // patched: base + long64 offset
    ".word 0x18",
    ".p2align 3",
    "rustux_ap_pml4: .long 0",
    "rustux_ap_efer: .long 0",
    "rustux_ap_cr4: .quad 0",
    "rustux_ap_cr0: .quad 0",
    "rustux_ap_stack: .quad 0",
    "rustux_ap_arg: .quad 0",
    "rustux_ap_entry: .quad 0",
    ".global rustux_ap_tramp_end",
    "rustux_ap_tramp_end:",
    // Offsets for the Rust side
    ".global rustux_ap_off_table",
    ".p2align 3",
    "rustux_ap_off_table:",
    ".quad rustux_ap_gdt - rustux_ap_tramp_start",
    ".quad rustux_ap_gdt_ptr - rustux_ap_tramp_start",
    ".quad rustux_ap_far32 - rustux_ap_tramp_start",
    ".quad rustux_ap_pm32 - rustux_ap_tramp_start",
    ".quad rustux_ap_far64 - rustux_ap_tramp_start",
    ".quad rustux_ap_long64 - rustux_ap_tramp_start",
    ".quad rustux_ap_pml4 - rustux_ap_tramp_start",
    ".quad rustux_ap_efer - rustux_ap_tramp_start",
    ".quad rustux_ap_cr4 - rustux_ap_tramp_start",
    ".quad rustux_ap_cr0 - rustux_ap_tramp_start",
    ".quad rustux_ap_stack - rustux_ap_tramp_start",
    ".quad rustux_ap_arg - rustux_ap_tramp_start",
    ".quad rustux_ap_entry - rustux_ap_tramp_start",
    ".quad rustux_ap_tramp_end - rustux_ap_tramp_start",
    ".text",
    ".intel_syntax noprefix",
);

extern "C" {
    static rustux_ap_tramp_start: u8;
    static rustux_ap_off_table: [u64; 14];
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Off {
    Gdt, GdtPtr, Far32, Pm32, Far64, Long64, Pml4, Efer, Cr4, Cr0, Stack, Arg, Entry, End,
}

fn off(o: Off) -> u64 {
    unsafe { rustux_ap_off_table[o as usize] }
}

// ---------------------------------------------------------------------
// Local APIC inter-processor interrupts
// ---------------------------------------------------------------------

fn lapic() -> u64 {
    kernel_map::phys_to_virt(0xFEE0_0000)
}

unsafe fn lapic_write(reg: u64, v: u32) {
    core::ptr::write_volatile((lapic() + reg) as *mut u32, v);
}

unsafe fn lapic_read(reg: u64) -> u32 {
    core::ptr::read_volatile((lapic() + reg) as *const u32)
}

/// Send an IPI (ICR low word `low`) to `apic_id` and wait until delivered
unsafe fn send_ipi(apic_id: u32, low: u32) {
    lapic_write(0x310, apic_id << 24);
    lapic_write(0x300, low);
    let deadline = super::tsc::Deadline::after_ms(100);
    while lapic_read(0x300) & (1 << 12) != 0 && !deadline.expired() {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------
// Bring-up
// ---------------------------------------------------------------------

/// Stack for each AP (parked APs need little)
const AP_STACK_PAGES: usize = 4;

/// Start every other CPU listed in the MADT; returns how many came up.
/// Built with the `no_smp` feature, does nothing (escape hatch for
/// hardware where bring-up misbehaves).
pub fn start_aps() -> usize {
    if cfg!(feature = "no_smp") {
        crate::kobj::debugcon("[SMP] disabled at build time (no_smp)\n");
        return 0;
    }
    use crate::mm::pmm;
    let base = LOW_PAGES.load(Ordering::Acquire);
    let (ids, n) = madt_apic_ids();
    if n <= 1 {
        return 0;
    }
    if base == 0 {
        crate::kobj::debugcon("[SMP] no free page below 1 MiB for the trampoline\n");
        return 0;
    }
    percpu::register_boot_cpu();
    let me = percpu::lapic_id();
    unsafe {
        // Trampoline code and data
        let len = off(Off::End) as usize;
        let tramp = kernel_map::phys_to_virt(base) as *mut u8;
        core::ptr::copy_nonoverlapping(&rustux_ap_tramp_start as *const u8, tramp, len);
        let put32 = |o: Off, v: u32| core::ptr::write_unaligned(tramp.add(off(o) as usize) as *mut u32, v);
        let put64 = |o: Off, v: u64| core::ptr::write_unaligned(tramp.add(off(o) as usize) as *mut u64, v);
        core::ptr::write_unaligned(tramp.add(off(Off::GdtPtr) as usize) as *mut u16, 4 * 8 - 1);
        core::ptr::write_unaligned(tramp.add(off(Off::GdtPtr) as usize + 2) as *mut u32, (base + off(Off::Gdt)) as u32);
        put32(Off::Far32, (base + off(Off::Pm32)) as u32);
        put32(Off::Far64, (base + off(Off::Long64)) as u32);

        // Temporary PML4 below 1 MiB: the kernel half, plus PML4[0] ->
        // PDPT -> PD with one 2 MiB page identity-mapping 0..2 MiB
        let pml4 = base + 4096;
        let (Ok(pdpt), Ok(pd)) = (pmm::pmm_alloc_kernel_page(), pmm::pmm_alloc_kernel_page()) else {
            return 0;
        };
        let t = |p: u64| kernel_map::phys_to_virt(p) as *mut u64;
        core::ptr::copy_nonoverlapping(t(kernel_map::kernel_pml4()), t(pml4), 512);
        core::ptr::write_bytes(t(pdpt), 0, 512);
        core::ptr::write_bytes(t(pd), 0, 512);
        *t(pml4) = pdpt | 0x3;
        *t(pdpt) = pd | 0x3;
        *t(pd) = 0x83; // present, writable, 2 MiB page at 0
        put32(Off::Pml4, pml4 as u32);

        // The boot CPU's control registers
        let (cr0, cr4): (u64, u64);
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        let efer_lo: u32;
        core::arch::asm!("rdmsr", in("ecx") 0xC000_0080u32, out("eax") efer_lo, out("edx") _, options(nomem, nostack));
        put32(Off::Efer, (1 << 8) | (efer_lo & (1 << 11))); // LME, and NXE if on
        put64(Off::Cr4, cr4);
        put64(Off::Cr0, cr0);
        put64(Off::Entry, ap_entry as usize as u64);

        let mut started = 0;
        for &id in &ids[..n] {
            if id == me {
                continue;
            }
            let Some(cpu) = percpu::reserve(id) else { break };
            let Ok(stack) = pmm::pmm_alloc_contiguous(AP_STACK_PAGES, pmm::PMM_ALLOC_FLAG_KERNEL, 0) else { break };
            let top = kernel_map::phys_to_virt(stack) + (AP_STACK_PAGES * 4096) as u64;
            put64(Off::Stack, top - 8);
            put64(Off::Arg, cpu.index as u64);
            core::sync::atomic::fence(Ordering::SeqCst);

            // INIT, then two start-up IPIs pointing at the trampoline page
            send_ipi(id, 0x0000_4500);
            super::tsc::delay_ms(10);
            let vector = (base >> 12) as u32;
            for _ in 0..2 {
                if cpu.is_online() {
                    break;
                }
                send_ipi(id, 0x0000_4600 | vector);
                let d = super::tsc::Deadline::after_ms(1);
                while !cpu.is_online() && !d.expired() {
                    core::hint::spin_loop();
                }
            }
            let d = super::tsc::Deadline::after_ms(100);
            while !cpu.is_online() && !d.expired() {
                core::hint::spin_loop();
            }
            if cpu.is_online() {
                started += 1;
            } else {
                // Its slot stays reserved and offline; try the next one
                crate::kobj::debugcon("[SMP] a CPU did not start\n");
            }
        }
        let _ = pmm::pmm_free_page(pdpt);
        let _ = pmm::pmm_free_page(pd);
        started
    }
}

/// Send the wake IPI to the CPUs whose bits are set in `mask` (by index)
pub fn wake_cpus(mask: u32) {
    let me = percpu::this().index;
    for cpu in percpu::all() {
        if mask & (1 << cpu.index) != 0 && cpu.index != me && cpu.is_online() {
            unsafe { send_ipi(cpu.apic_id(), WAKE_VECTOR as u32) };
        }
    }
}

/// Flush this CPU's TLB if another CPU asked (IPI handler, and CPUs waiting
/// for their own shootdown)
pub fn service_flush() {
    let cpu = percpu::this();
    let req = cpu.flush_req.load(Ordering::Acquire);
    if cpu.flush_ack.load(Ordering::Acquire) < req {
        unsafe {
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack));
        }
        cpu.flush_ack.store(req, Ordering::Release);
    }
}

/// After page table entries of `pml4` were removed or restricted: make
/// every other CPU using those page tables flush its TLB, and wait until
/// they have. Must be called with no locks held (a CPU we wait for may
/// need them first).
pub fn tlb_shootdown(pml4: u64) {
    if percpu::online_count() <= 1 {
        return;
    }
    let me = percpu::this().index;
    // The PTE changes are visible before we look at who uses the tables
    core::sync::atomic::fence(Ordering::SeqCst);
    let mut wait = [0u64; percpu::MAX_CPUS];
    let mut any = false;
    for cpu in percpu::all() {
        if cpu.index != me && cpu.is_online() && cpu.cr3.load(Ordering::SeqCst) == pml4 {
            wait[cpu.index] = cpu.flush_req.fetch_add(1, Ordering::AcqRel) + 1;
            unsafe { send_ipi(cpu.apic_id(), FLUSH_VECTOR as u32) };
            any = true;
        }
    }
    if !any {
        return;
    }
    for cpu in percpu::all() {
        let want = wait[cpu.index];
        while want != 0 && cpu.flush_ack.load(Ordering::Acquire) < want {
            // Another CPU may be waiting for us at the same time
            service_flush();
            core::hint::spin_loop();
        }
    }
}

/// Program this CPU's LAPIC timer: 10 ms periodic, vector 32
unsafe fn start_timer() {
    let count = TIMER_COUNT.load(Ordering::Acquire);
    if count == 0 {
        return;
    }
    lapic_write(0x3E0, 0x03); // divide by 16, as on the boot CPU
    lapic_write(0x320, 32 | (1 << 17)); // periodic
    lapic_write(0x380, count);
}

/// Where an AP arrives in 64-bit mode, on its own stack, with the
/// temporary page tables
extern "sysv64" fn ap_entry(index: u64) -> ! {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) kernel_map::kernel_pml4(), options(nostack));
        let cpu = percpu::get(index as usize).expect("bad CPU index");
        super::descriptor::load_cpu_tables(cpu.tables());
        // (load_cpu_tables also reloads CS: the trampoline's 0x18 is the
        // kernel GDT's user code segment)
        super::descriptor::idt_load_shared();
        // Enable this CPU's local APIC (spurious vector 0xFF); accept all
        // interrupt priorities
        lapic_write(0xF0, lapic_read(0xF0) | 0x1FF);
        lapic_write(0x80, 0);
        start_timer();
        percpu::mark_online(cpu);
    }
    crate::process::run::idle_loop()
}

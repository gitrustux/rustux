// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86 Descriptor Tables
//!
//! This module provides GDT and IDT setup functions.

// ============================================================================
// GDT (Global Descriptor Table) Structures
// ============================================================================

/// GDT Entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GdtEntry {
    pub limit_low: u16,
    pub base_low: u16,
    pub base_mid: u8,
    pub access: u8,
    pub flags_limit_high: u8,
    pub base_high: u8,
}

/// GDT Pointer (used with lgdt instruction)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct GdtPointer {
    pub limit: u16,
    pub base: u64,
}

/// Task State Segment (TSS) for x86-64
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TaskStateSegment {
    pub reserved0: u32,
    pub rsp0: u64,     // Ring 0 stack pointer
    pub rsp1: u64,     // Ring 1 stack pointer
    pub rsp2: u64,     // Ring 2 stack pointer
    pub reserved1: u32,
    pub reserved2: u32,
    pub ist1: u64,     // Interrupt Stack Table 1
    pub ist2: u64,     // Interrupt Stack Table 2
    pub ist3: u64,     // Interrupt Stack Table 3
    pub ist4: u64,     // Interrupt Stack Table 4
    pub ist5: u64,     // Interrupt Stack Table 5
    pub ist6: u64,     // Interrupt Stack Table 6
    pub ist7: u64,     // Interrupt Stack Table 7
    pub reserved3: u64,
    pub reserved4: u16,
    pub iomap_base: u16, // I/O map base address (offset 102)
}

// The hardware layout is 104 bytes, with the I/O map base at offset 102.
// (This struct used to lack `reserved3: u64`: it was 96 bytes and put the
// field at offset 92, so the CPU read the bitmap offset from past the
// end of the TSS and refused every user port access.)
const _: () = assert!(core::mem::size_of::<TaskStateSegment>() == 104);
const _: () = assert!(core::mem::offset_of!(TaskStateSegment, iomap_base) == 102);

/// GDT Entry types
pub const GDT_NULL: usize = 0;
pub const GDT_KERNEL_CODE: usize = 1;
pub const GDT_KERNEL_DATA: usize = 2;
pub const GDT_USER_CODE: usize = 3;
pub const GDT_USER_DATA: usize = 4;
pub const GDT_TSS_LOW: usize = 5;
pub const GDT_TSS_HIGH: usize = 6;
pub const GDT_ENTRIES: usize = 7;

// Access byte flags
pub const ACC_PRESENT: u8 = 0x80;
pub const ACC_SYSTEM: u8 = 0x00;
pub const ACC_CODE_DATA: u8 = 0x10;
pub const ACC_CODE: u8 = 0x0A;
pub const ACC_DATA: u8 = 0x02;
pub const ACC_DPL0: u8 = 0x00;
pub const ACC_DPL3: u8 = 0x60;

// Flags byte flags
pub const FLAG_GRANULARITY_4K: u8 = 0x80;
pub const FLAG_SIZE_64BIT: u8 = 0x20;

/// One CPU's GDT and TSS (each CPU needs its own TSS: it holds that CPU's
/// kernel stack pointer and I/O bitmap, and a TSS descriptor is marked busy
/// while loaded). Stored in `percpu`.
#[repr(C)]
pub struct CpuTables {
    gdt: [GdtEntry; GDT_ENTRIES],
    gdt_ptr: GdtPointer,
    tss: TssWithIoMap,
    /// Koid of the process whose bitmap is in the TSS (0 = all denied)
    io_map_owner: u64,
}

impl CpuTables {
    pub const fn new() -> Self {
        Self {
            gdt: [GdtEntry::null(); GDT_ENTRIES],
            gdt_ptr: GdtPointer { limit: 0, base: 0 },
            tss: TssWithIoMap { tss: TaskStateSegment::null(), io_map: [0xFF; IO_MAP_BYTES], io_map_end: 0xFF },
            io_map_owner: 0,
        }
    }
}

/// This CPU's tables
fn my_tables() -> *mut CpuTables {
    super::percpu::this().tables()
}

/// One bit per I/O port (65536 ports); a set bit denies user access
pub const IO_MAP_BYTES: usize = 8192;

/// The TSS followed by its I/O permission bitmap. Ring-3 `in`/`out` to a
/// port whose bit is set (or beyond the TSS limit) raises #GP; every bit
/// starts set, and a process gets ports through `ioports_request`.
#[repr(C, packed)]
struct TssWithIoMap {
    tss: TaskStateSegment,
    io_map: [u8; IO_MAP_BYTES],
    /// Required all-ones byte after the bitmap
    io_map_end: u8,
}

/// Load a process's I/O permission bitmap into the TSS (`None` = deny all).
/// `owner` identifies the bitmap so switching between threads of the same
/// process does not copy 8 KiB each time; pass `force` after a change.
///
/// # Safety
/// Interrupts must be disabled (single CPU, no concurrent TSS writes).
pub unsafe fn load_io_map(owner: u64, map: Option<&[u8; IO_MAP_BYTES]>, force: bool) {
    let t = my_tables();
    let owner = if map.is_some() { owner } else { 0 };
    if owner == (*t).io_map_owner && !force {
        return;
    }
    let dst = core::ptr::addr_of_mut!((*t).tss.io_map) as *mut u8;
    match map {
        Some(m) => core::ptr::copy_nonoverlapping(m.as_ptr(), dst, IO_MAP_BYTES),
        None => core::ptr::write_bytes(dst, 0xFF, IO_MAP_BYTES),
    }
    (*t).io_map_owner = owner;
}

/// Set the ring-0 stack the CPU switches to on an interrupt, exception or
/// `int 0x80` taken while running in ring 3 (TSS.RSP0).
///
/// # Safety
/// `rsp0` must be the top of a mapped kernel stack in every address space
/// that can run in ring 3.
pub unsafe fn set_tss_rsp0(rsp0: u64) {
    // TSS is packed; write through a raw pointer to avoid an unaligned reference
    core::ptr::addr_of_mut!((*my_tables()).tss.tss.rsp0).write_unaligned(rsp0);
}

impl GdtEntry {
    pub const fn null() -> Self {
        Self {
            limit_low: 0,
            base_low: 0,
            base_mid: 0,
            access: 0,
            flags_limit_high: 0,
            base_high: 0,
        }
    }

    pub fn set_gate(base: u64, limit: u32, access: u8, flags: u8) -> Self {
        Self {
            limit_low: limit as u16,
            base_low: (base & 0xFFFF) as u16,
            base_mid: ((base >> 16) & 0xFF) as u8,
            access,
            flags_limit_high: ((limit >> 16) & 0x0F) as u8 | flags,
            base_high: ((base >> 24) & 0xFF) as u8,
        }
    }

    pub fn set_tss_low(base: u64, limit: u32, access: u8) -> Self {
        Self {
            limit_low: limit as u16,
            base_low: (base & 0xFFFF) as u16,
            base_mid: ((base >> 16) & 0xFF) as u8,
            access,
            flags_limit_high: ((limit >> 16) & 0x0F) as u8,
            base_high: ((base >> 24) & 0xFF) as u8,
        }
    }

    /// Upper half of a 16-byte TSS descriptor: bytes 8-11 hold base bits
    /// 63:32, bytes 12-15 are reserved (zero). In this struct's layout that
    /// is `limit_low` (bits 47:32) and `base_low` (bits 63:48).
    /// (Bits 63:48 used to land in the reserved bytes; harmless while the
    /// TSS was below 4 GiB, fatal once the kernel runs at 0xffffffff8....)
    pub fn set_tss_high(base: u64) -> Self {
        Self {
            limit_low: ((base >> 32) & 0xFFFF) as u16,
            base_low: ((base >> 48) & 0xFFFF) as u16,
            base_mid: 0,
            access: 0,
            flags_limit_high: 0,
            base_high: 0,
        }
    }
}

impl TaskStateSegment {
    pub const fn null() -> Self {
        Self {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            reserved2: 0,
            ist1: 0,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            reserved3: 0,
            reserved4: 0,
            iomap_base: 0,
        }
    }
}

/// Set up and load this CPU's GDT and TSS
pub fn gdt_setup() {
    unsafe { load_cpu_tables(my_tables()) }
}

/// Fill in a CPU's GDT and TSS and load them on the CPU executing this
///
/// # Safety
/// `t` must be the executing CPU's own tables.
pub unsafe fn load_cpu_tables(t: *mut CpuTables) {
    let gdt = &mut (*t).gdt;
    gdt[GDT_NULL] = GdtEntry::null();
    gdt[GDT_KERNEL_CODE] = GdtEntry::set_gate(0, 0xFFFFF,
        ACC_PRESENT | ACC_CODE_DATA | ACC_CODE | ACC_DPL0, FLAG_GRANULARITY_4K | FLAG_SIZE_64BIT);
    gdt[GDT_KERNEL_DATA] = GdtEntry::set_gate(0, 0xFFFFF,
        ACC_PRESENT | ACC_CODE_DATA | ACC_DATA | ACC_DPL0, FLAG_GRANULARITY_4K);
    gdt[GDT_USER_CODE] = GdtEntry::set_gate(0, 0xFFFFF,
        ACC_PRESENT | ACC_CODE_DATA | ACC_CODE | ACC_DPL3, FLAG_GRANULARITY_4K | FLAG_SIZE_64BIT);
    gdt[GDT_USER_DATA] = GdtEntry::set_gate(0, 0xFFFFF,
        ACC_PRESENT | ACC_CODE_DATA | ACC_DATA | ACC_DPL3, FLAG_GRANULARITY_4K);

    // TSS (two entries); the I/O bitmap follows the 104-byte TSS
    core::ptr::addr_of_mut!((*t).tss.tss.iomap_base)
        .write_unaligned(core::mem::size_of::<TaskStateSegment>() as u16);
    let tss_base = core::ptr::addr_of!((*t).tss) as u64;
    let tss_limit = core::mem::size_of::<TssWithIoMap>() as u32 - 1;
    gdt[GDT_TSS_LOW] = GdtEntry::set_tss_low(tss_base, tss_limit, ACC_PRESENT | 0x09);
    gdt[GDT_TSS_HIGH] = GdtEntry::set_tss_high(tss_base);

    (*t).gdt_ptr.limit = ((core::mem::size_of::<GdtEntry>() * GDT_ENTRIES) - 1) as u16;
    (*t).gdt_ptr.base = gdt.as_ptr() as u64;
    gdt_load(&(*t).gdt_ptr);
    // Switch to this GDT's kernel code and data selectors. The CPU may
    // still hold the firmware's (UEFI uses 0x38, beyond this GDT) or the
    // AP trampoline's (0x18, this GDT's user code): an IRETQ to a context
    // running with such a CS faults.
    core::arch::asm!(
        "push 0x08",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        "mov {tmp:e}, 0x10",
        "mov ss, {tmp:e}",
        "mov ds, {tmp:e}",
        "mov es, {tmp:e}",
        tmp = out(reg) _,
    );
    tss_load((GDT_TSS_LOW * 8) as u16);
}

/// Setup the IDT (Interrupt Descriptor Table)
///
/// This initializes the IDT with empty entries. Use `idt_set_gate` to install
/// specific interrupt handlers.
pub fn idt_setup_readonly() {
    unsafe {
        // Code segment selector for kernel (index 1 * 8 = 8, for ring 0)
        const KERNEL_CS: u16 = GDT_KERNEL_CODE as u16 * 8;

        // Initialize IDT entries with a default handler (all zeros = points to null)
        // The caller should use idt_set_gate to install actual handlers
        let idt = IDT.boot_mut();
        for i in 0..IDT_ENTRIES {
            idt[i] = IdtEntry {
                offset_low: 0,
                selector: KERNEL_CS,
                ist: 0,  // IST0 (no special stack)
                type_attr: IDT_INTERRUPT_GATE,
                offset_mid: 0,
                offset_high: 0,
                reserved: 0,
            };
        }

        let p = IDT_POINTER.boot_mut();
        p.limit = ((core::mem::size_of::<IdtEntry>() * IDT_ENTRIES) - 1) as u16;
        p.base = IDT.as_ptr() as u64;
        idt_load(IDT_POINTER.get());
    }
}

/// Extract the Requested Privilege Level (RPL) from a selector
///
/// # Arguments
///
/// * `selector` - Segment selector value
///
/// # Returns
///
/// The RPL value (0-3)
pub const fn SELECTOR_PL(selector: u16) -> u16 {
    selector & 3
}

/// Make a selector from RPL
///
/// # Arguments
///
/// * `rpl` - Requested Privilege Level (0-3)
///
/// # Returns
///
/// A selector value with the specified RPL
pub const fn SELECTOR_FROM_RPL(rpl: u16) -> u16 {
    rpl & 3
}

// ============================================================================
// IDT (Interrupt Descriptor Table) Structures
// ============================================================================

/// IDT Entry (16 bytes for x86-64)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IdtEntry {
    pub offset_low: u16,
    pub selector: u16,
    pub ist: u8,
    pub type_attr: u8,
    pub offset_mid: u16,
    pub offset_high: u32,
    pub reserved: u32,
}

/// IDT Pointer (used with lidt instruction)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IdtPointer {
    pub limit: u16,
    pub base: u64,
}

// IDT Entry types
pub const IDT_INTERRUPT_GATE: u8 = 0x8E;
pub const IDT_TRAP_GATE: u8 = 0x8F;
pub const IDT_TASK_GATE: u8 = 0x85;

// Standard x86-64 exception vectors (0-31)
pub const X86_INT_DIVIDE_ERROR: u8 = 0;
pub const X86_INT_DEBUG: u8 = 1;
pub const X86_INT_NMI: u8 = 2;
pub const X86_INT_BREAKPOINT: u8 = 3;
pub const X86_INT_OVERFLOW: u8 = 4;
pub const X86_INT_BOUND_RANGE: u8 = 5;
pub const X86_INT_INVALID_OP: u8 = 6;
pub const X86_INT_DEVICE_NA: u8 = 7;
pub const X86_INT_DOUBLE_FAULT: u8 = 8;
pub const X86_INT_INVALID_TSS: u8 = 10;
pub const X86_INT_SEGMENT_NP: u8 = 11;
pub const X86_INT_STACK_FAULT: u8 = 12;
pub const X86_INT_GP_FAULT: u8 = 13;
pub const X86_INT_PAGE_FAULT: u8 = 14;
pub const X86_INT_X87_FPU_ERROR: u8 = 16;
pub const X86_INT_ALIGNMENT_CHECK: u8 = 17;
pub const X86_INT_MACHINE_CHECK: u8 = 18;
pub const X86_INT_SIMD_FP_ERROR: u8 = 19;
pub const X86_INT_VIRTUALIZATION: u8 = 20;
pub const X86_INT_SECURITY: u8 = 30;

// APIC interrupts
pub const X86_INT_APIC_SPURIOUS: u8 = 0xFF;
pub const X86_INT_APIC_TIMER: u8 = 0x20;
pub const X86_INT_APIC_ERROR: u8 = 0x21;

pub const IDT_ENTRIES: usize = 256;

// The IDT, shared by all CPUs: filled in during boot, read-only afterwards
pub static IDT: crate::sync::BootCell<[IdtEntry; IDT_ENTRIES]> = crate::sync::BootCell::new([IdtEntry::null(); IDT_ENTRIES]);
pub static IDT_POINTER: crate::sync::BootCell<IdtPointer> = crate::sync::BootCell::new(IdtPointer { limit: 0, base: 0 });

/// Load the shared IDT on the CPU executing this (other CPUs at start-up)
pub fn idt_load_shared() {
    unsafe { idt_load(IDT_POINTER.get()) }
}

impl IdtEntry {
    pub const fn null() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    pub fn set_gate(offset: u64, selector: u16, type_attr: u8, ist: u8) -> Self {
        Self {
            offset_low: (offset & 0xFFFF) as u16,
            selector,
            ist,
            type_attr,
            offset_mid: ((offset >> 16) & 0xFFFF) as u16,
            offset_high: ((offset >> 32) & 0xFFFFFFFF) as u32,
            reserved: 0,
        }
    }
}

// ============================================================================
// Assembly Functions
// ============================================================================

/// Load GDT
///
/// # Safety
///
/// Must be called with a valid GDT pointer
#[inline]
pub unsafe fn gdt_load(gdt_ptr: &GdtPointer) {
    core::arch::asm!("lgdt [{}]", in(reg) gdt_ptr, options(nostack));
}

/// Load IDT
///
/// # Safety
///
/// Must be called with a valid IDT pointer
#[inline]
pub unsafe fn idt_load(idt_ptr: &IdtPointer) {
    core::arch::asm!("lidt [{}]", in(reg) idt_ptr, options(nostack, readonly));
}

/// Load TSS
///
/// # Safety
///
/// Must be called with a valid TSS selector
#[inline]
pub unsafe fn tss_load(selector: u16) {
    core::arch::asm!("ltr {0:x}", in(reg) selector, options(nostack));
}

/// Get TSS reference for modification
///
/// # Safety
///
/// Caller must ensure TSS has been initialized
pub unsafe fn get_tss() -> &'static mut TaskStateSegment {
    &mut *core::ptr::addr_of_mut!((*my_tables()).tss.tss)
}

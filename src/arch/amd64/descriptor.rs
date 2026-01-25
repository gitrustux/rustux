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
    pub reserved3: u16,
    pub iomap_base: u16, // I/O map base address
}

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

// Global GDT storage
static mut GDT: [GdtEntry; GDT_ENTRIES] = [GdtEntry::null(); GDT_ENTRIES];
static mut GDT_POINTER: GdtPointer = GdtPointer { limit: 0, base: 0 };
static mut TSS: TaskStateSegment = TaskStateSegment::null();

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

    pub fn set_tss_high(base: u64) -> Self {
        Self {
            limit_low: ((base >> 32) & 0xFFFF) as u16,
            base_low: 0,
            base_mid: 0,
            access: 0,
            flags_limit_high: ((base >> 48) as u8) & 0xFF,
            base_high: ((base >> 56) as u8) & 0xFF,
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
            iomap_base: 0,
        }
    }
}

/// Setup the GDT (Global Descriptor Table)
pub fn gdt_setup() {
    unsafe {
        // Null descriptor (required)
        GDT[GDT_NULL] = GdtEntry::null();

        // Kernel code segment (64-bit)
        GDT[GDT_KERNEL_CODE] = GdtEntry::set_gate(
            0,                      // Base (ignored in long mode)
            0xFFFFF,                // Limit (ignored in long mode)
            ACC_PRESENT | ACC_CODE_DATA | ACC_CODE | ACC_DPL0, // Present, Code, DPL0
            FLAG_GRANULARITY_4K | FLAG_SIZE_64BIT,               // 4KB pages, 64-bit
        );

        // Kernel data segment
        GDT[GDT_KERNEL_DATA] = GdtEntry::set_gate(
            0,                      // Base (ignored in long mode)
            0xFFFFF,                // Limit (ignored in long mode)
            ACC_PRESENT | ACC_CODE_DATA | ACC_DATA | ACC_DPL0, // Present, Data, DPL0
            FLAG_GRANULARITY_4K,                                      // 4KB pages
        );

        // User code segment (64-bit)
        GDT[GDT_USER_CODE] = GdtEntry::set_gate(
            0,                      // Base (ignored in long mode)
            0xFFFFF,                // Limit (ignored in long mode)
            ACC_PRESENT | ACC_CODE_DATA | ACC_CODE | ACC_DPL3, // Present, Code, DPL3
            FLAG_GRANULARITY_4K | FLAG_SIZE_64BIT,               // 4KB pages, 64-bit
        );

        // User data segment
        GDT[GDT_USER_DATA] = GdtEntry::set_gate(
            0,                      // Base (ignored in long mode)
            0xFFFFF,                // Limit (ignored in long mode)
            ACC_PRESENT | ACC_CODE_DATA | ACC_DATA | ACC_DPL3, // Present, Data, DPL3
            FLAG_GRANULARITY_4K,                                      // 4KB pages
        );

        // TSS entry (needs two entries)
        let tss_base = &TSS as *const TaskStateSegment as u64;
        let tss_limit = core::mem::size_of::<TaskStateSegment>() as u32;
        let tss_access = ACC_PRESENT | 0x09; // Present, TSS, DPL0

        GDT[GDT_TSS_LOW] = GdtEntry::set_tss_low(tss_base, tss_limit, tss_access);
        GDT[GDT_TSS_HIGH] = GdtEntry::set_tss_high(tss_base);

        // Setup GDT pointer
        GDT_POINTER.limit = ((core::mem::size_of::<GdtEntry>() * GDT_ENTRIES) - 1) as u16;
        GDT_POINTER.base = &GDT as *const GdtEntry as u64;

        // Load GDT
        gdt_load(&GDT_POINTER);

        // Load TSS
        let tss_selector = (GDT_TSS_LOW * 8) as u16;
        tss_load(tss_selector);
    }
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
        for i in 0..IDT_ENTRIES {
            IDT[i] = IdtEntry {
                offset_low: 0,
                selector: KERNEL_CS,
                ist: 0,  // IST0 (no special stack)
                type_attr: IDT_INTERRUPT_GATE,
                offset_mid: 0,
                offset_high: 0,
                reserved: 0,
            };
        }

        // Setup IDT pointer
        IDT_POINTER.limit = ((core::mem::size_of::<IdtEntry>() * IDT_ENTRIES) - 1) as u16;
        IDT_POINTER.base = &IDT as *const IdtEntry as u64;

        // Load IDT
        idt_load(&IDT_POINTER);
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

// Global IDT storage
pub static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::null(); IDT_ENTRIES];
pub static mut IDT_POINTER: IdtPointer = IdtPointer { limit: 0, base: 0 };

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
    &mut TSS
}

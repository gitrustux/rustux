// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 Interrupt Descriptor Table (IDT)

/// IDT entry structure
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

/// IDT pointer structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IdtPointer {
    pub limit: u16,
    pub base: u64,
}

/// Interrupt frame for x86_64
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86Iframe {
    // General registers
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // Segment registers
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    // Special registers
    pub rflags: u64,
    pub rip: u64,
    pub user_ss: u64,
    pub user_cs: u64,
}

// IDT Entry types
pub const IDT_INTERRUPT_GATE: u8 = 0x8E;
pub const IDT_TRAP_GATE: u8 = 0x8F;
pub const IDT_TASK_GATE: u8 = 0x85;

/// Initialize the IDT
///
/// This function initializes the IDT with proper exception handlers.
pub fn idt_init() {
    unsafe {
        // Use the IDT setup from descriptor module
        super::descriptor::idt_setup_readonly();
    }
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

/// Set an IDT gate (interrupt handler)
///
/// This function sets an entry in the IDT to point to an interrupt handler.
///
/// # Arguments
/// * `vector` - The interrupt vector (0-255)
/// * `handler` - The handler function address
/// * `selector` - The code segment selector
/// * `type_attr` - The gate type attributes
///
/// # Safety
///
/// This function modifies the IDT and must only be called with valid pointers.
pub unsafe fn idt_set_gate(vector: u8, handler: u64, selector: u16, type_attr: u8) {
    use super::descriptor::{IDT, IDT_ENTRIES, IdtEntry as DescriptorIdtEntry};

    if vector as usize >= IDT_ENTRIES {
        return;
    }

    IDT[vector as usize] = DescriptorIdtEntry::set_gate(handler, selector, type_attr, 0);
}

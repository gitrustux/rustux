// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86 architecture-specific initialization and core functions
//!
//! This module provides the main architecture initialization for x86_64 systems.

// ============================================================================
// MSR and CR register constants
// ============================================================================

/// IA32_GS_BASE MSR - Kernel GS Base
pub const X86_MSR_IA32_GS_BASE: u32 = 0xC000_0101;

/// IA32_FS_BASE MSR - FS Base
pub const X86_MSR_IA32_FS_BASE: u32 = 0xC000_0100;

/// IA32_KERNEL_GS_BASE MSR - Kernel GS Base
pub const X86_MSR_IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// CR0 flags
pub const X86_CR0_CD: u64 = 1 << 30; // Cache disable
pub const X86_CR0_NW: u64 = 1 << 29; // Not-write-through

// ============================================================================
// Interrupt control
// ============================================================================

/// Disable interrupts
#[inline]
pub fn arch_disable_ints() -> u64 {
    unsafe { x86_cli() };
    // Return interrupt state (to be implemented)
    0
}

/// Enable interrupts
#[inline]
pub fn arch_enable_ints() {
    unsafe { x86_sti() };
}

/// Check if interrupts are disabled
#[inline]
pub fn arch_ints_disabled() -> bool {
    // Read RFLAGS and check IF bit
    let rflags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) rflags, options(nostack, nomem));
    }
    rflags & (1 << 9) == 0
}

/// CLI instruction (disable interrupts)
#[inline]
unsafe fn x86_cli() {
    core::arch::asm!("cli", options(nostack));
}

/// STI instruction (enable interrupts)
#[inline]
unsafe fn x86_sti() {
    core::arch::asm!("sti", options(nostack));
}

// ============================================================================
// MSR access
// ============================================================================

/// Read an MSR (Model Specific Register)
///
/// # Safety
///
/// The caller must ensure the MSR index is valid.
#[inline]
pub unsafe fn x86_read_msr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    core::arch::asm!("rdmsr",
                     in("ecx") msr,
                     out("eax") low,
                     out("edx") high,
                     options(nostack, nomem, preserves_flags));
    ((high as u64) << 32) | (low as u64)
}

/// Write to an MSR (Model Specific Register)
///
/// # Safety
///
/// The caller must ensure the MSR index is valid and the value is appropriate.
#[inline]
pub unsafe fn x86_write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!("wrmsr",
                     in("ecx") msr,
                     in("eax") low,
                     in("edx") high,
                     options(nostack, nomem, preserves_flags));
}

// ============================================================================
// Memory Management Unit (MMU) integration
// ============================================================================

use super::mmu;

/// Early architecture initialization
///
/// This is called very early in the boot process, before the VM subsystem
/// is fully initialized.
pub fn arch_early_init() {
    mmu::x86_mmu_early_init();
}

/// Main architecture initialization
///
/// Called after the VM subsystem is up. Prints processor info and
/// initializes core architecture features.
pub fn arch_init() {
    // MMU initialization is already done in arch_early_init
    mmu::x86_mmu_init();

    // GDT and IDT setup
    super::descriptor::gdt_setup();
    super::descriptor::idt_setup_readonly();

    // TODO: Add CPU feature detection and debug output
    // println!("x86_64 architecture initialized");
}

/// Enter userspace at the given entry point
///
/// # Safety
///
/// Caller must ensure all pointers are valid and the system is in a proper state
pub unsafe fn arch_enter_uspace(_entry_point: usize, _sp: usize, _arg1: usize, _arg2: usize) -> ! {
    // TODO: Implement full userspace entry
    loop {
        unsafe { core::arch::asm!("hlt") }
    }
}

/// Read CR0 register
pub fn x86_get_cr0() -> u64 {
    let cr0_value: u64;
    unsafe {
        core::arch::asm!("mov %cr0, {}", out(reg) cr0_value);
    }
    cr0_value
}

/// Write CR0 register
///
/// # Safety
///
/// This function modifies a critical system register.
pub unsafe fn x86_write_cr0(cr0_value: u64) {
    core::arch::asm!("mov {}, %cr0", in(reg) cr0_value, options(nostack, nomem));
}

/// Read CR3 register (page table base)
pub fn x86_read_cr3() -> u64 {
    let cr3_value: u64;
    unsafe {
        core::arch::asm!("mov %cr3, {}", out(reg) cr3_value);
    }
    cr3_value
}

/// Write CR3 register (page table base)
///
/// # Safety
///
/// This function modifies a critical system register.
pub unsafe fn x86_write_cr3(cr3_value: u64) {
    core::arch::asm!("mov {}, %cr3", in(reg) cr3_value, options(nostack, nomem));
}

// ============================================================================
// HLT instruction
// ============================================================================

/// Halt the CPU
///
/// # Safety
///
/// This will halt the CPU permanently.
#[inline]
pub unsafe fn x86_hlt() {
    core::arch::asm!("hlt", options(nostack));
}

/// Halt and loop forever
///
/// Used for stopping execution (panic, halt_and_loop)
pub fn halt_and_loop() -> ! {
    loop {
        unsafe { x86_hlt() }
    }
}

// Export the iframe for use by other modules
pub use super::idt::X86Iframe;

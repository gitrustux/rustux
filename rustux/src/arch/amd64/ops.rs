// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 Low-Level Operations
//!
//! This module provides functions for idle, MSR access,
//! and monitor/mwait support.

/// Put the CPU to sleep if interrupts are enabled
///
/// This function checks if interrupts are enabled and if so,
/// executes the HLT instruction to wait for an interrupt.
pub fn x86_idle() {
    // Get RFLAGS and check IF bit
    let rflags = x86_get_rflags();

    // Check if IF (interrupt enable) bit is set (bit 9)
    if rflags & (1 << 9) != 0 {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Read RFLAGS register
///
/// # Returns
///
/// The current value of RFLAGS
#[inline]
pub fn x86_get_rflags() -> u64 {
    unsafe {
        let rflags: u64;
        core::arch::asm!(
            "pushfq",
            "pop {0}",
            out(reg) rflags,
            options(nomem, nostack)
        );
        rflags
    }
}

/// Read an MSR (Model Specific Register) with validation
///
/// This function checks if the MSR is valid before reading.
///
/// # Arguments
///
/// * `msr_id` - The MSR index to read
/// * `val_out` - Pointer to store the read value
///
/// # Returns
///
/// true on success, false if the MSR doesn't exist
///
/// # Safety
///
/// val_out must point to valid memory
pub unsafe fn read_msr_safe(msr_id: u32, val_out: *mut u64) -> bool {
    // Check if the MSR is in the list of known MSRs
    let is_valid = match msr_id {
        // IA32_APIC_BASE
        0x1B => true,
        // IA32_BIOS_SIGN_ID
        0x8B => true,
        // IA32_MTRRCAP
        0xFE => true,
        // IA32_SYSENTER_CS, ESP, EIP
        0x174 | 0x175 | 0x176 | 0x177 => true,
        // IA32_EFER, STAR, LSTAR, CSTAR, SFMASK, FS_BASE, GS_BASE, KERNEL_GS_BASE
        0xC000_0080..=0xC000_0084 => true,
        0xC000_0100..=0xC000_0102 => true,
        // Local APIC registers
        0x800..=0x8FF => true,
        // IO APIC registers
        0xFEC0_0000..=0xFEC0_0040 => false, // Memory-mapped, not MSR
        // Unknown MSR
        _ => false,
    };

    if !is_valid {
        return false;
    }

    // Read the MSR
    let low: u32;
    let high: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr_id,
        lateout("eax") low,
        lateout("edx") high,
        options(nomem, nostack, preserves_flags)
    );

    // Combine the result
    *val_out = ((high as u64) << 32) | (low as u64);
    true
}

/// Write an MSR (Model Specific Register) with validation
///
/// This function checks if the MSR is valid before writing.
///
/// # Arguments
///
/// * `msr_id` - The MSR index to write
/// * `value` - Value to write
///
/// # Returns
///
/// true on success, false if the MSR doesn't exist or is read-only
///
/// # Safety
///
/// Writing to invalid MSRs can cause undefined behavior
pub unsafe fn write_msr_safe(msr_id: u32, value: u64) -> bool {
    // Check if the MSR is writable
    let is_writable = match msr_id {
        // IA32_EFER, STAR, LSTAR, SFMASK, FMASK
        0xC000_0080 | 0xC000_0081 | 0xC000_0082 | 0xC000_0084 => true,
        // FS_BASE, GS_BASE, KERNEL_GS_BASE
        0xC000_0100..=0xC000_0102 => true,
        // Local APIC registers (most are writable)
        0x800..=0x8FF => true,
        // Read-only MSRs
        _ => false,
    };

    if !is_writable {
        return false;
    }

    // Write the MSR
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr_id,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );

    true
}

/// Wait for a memory address to change (MONITOR/MWAIT)
///
/// This function sets up a monitor and then waits using MWAIT.
/// The CPU enters a low-power state until the monitored address
/// is written to or an interrupt occurs.
///
/// # Arguments
///
/// * `addr` - Address to monitor
/// * `extensions` - Monitor extensions (typically 0)
/// * `hints` - MWAIT hints (typically 0)
///
/// # Safety
///
/// addr must be a valid memory address
pub unsafe fn x86_mwait<T>(addr: *const T, extensions: u32, hints: u32) {
    // Check if interrupts are enabled
    let rflags = x86_get_rflags();
    if rflags & (1 << 9) == 0 {
        // Don't wait if interrupts disabled
        return;
    }

    // Set up monitor
    core::arch::asm!(
        "monitor",
        in("rax") addr,
        in("rcx") extensions,
        in("rdx") 0u32, // optional hints
        options(nomem, nostack)
    );

    // Wait
    core::arch::asm!(
        "mwait",
        in("eax") hints,
        in("ecx") extensions,
        options(nomem, nostack)
    );
}

/// Monitor a memory address for changes
///
/// Sets up the hardware to monitor the specified cache line
/// for writes. Used in conjunction with MWAIT.
///
/// # Arguments
///
/// * `addr` - Address to monitor
/// * `extensions` - Monitor extensions (typically 0)
/// * `hints` - Optional hints (typically 0)
///
/// # Safety
///
/// addr must be a valid memory address
pub unsafe fn x86_monitor<T>(addr: *const T, extensions: u32, hints: u32) {
    core::arch::asm!(
        "monitor",
        in("rax") addr,
        in("rcx") extensions,
        in("rdx") hints,
        options(nomem, nostack)
    );
}

/// Simple MWAIT without separate monitor setup
///
/// This is a convenience function that combines monitor and mwait
/// for common use cases.
///
/// # Arguments
///
/// * `addr` - Address to monitor
///
/// # Safety
///
/// addr must be a valid memory address
pub unsafe fn x86_mwait_simple<T>(addr: *const T) {
    x86_monitor(addr, 0, 0);
    x86_mwait(addr, 0, 0);
}

/// Check if MONITOR/MWAIT is supported
///
/// # Returns
///
/// true if the CPU supports MONITOR/MWAIT
pub fn x86_has_mwait() -> bool {
    unsafe {
        let ecx: u32;
        core::arch::asm!(
            "cpuid",
            in("eax") 1u32,
            lateout("ecx") ecx,
            options(nostack, nomem)
        );

        // Check bit 3 (MONITOR/MWAIT)
        ecx & (1 << 3) != 0
    }
}

/// NOP instruction - does nothing
#[inline]
pub fn nop() {
    unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
}

/// PAUSE instruction - hint to CPU that we're spinning
#[inline]
pub fn pause() {
    unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
}

/// WBINVD instruction - write-back and invalidate cache
///
/// # Safety
///
/// This is a privileged instruction that should only be used
/// in kernel mode
#[inline]
pub unsafe fn wbinvd() {
    core::arch::asm!("wbinvd", options(nomem, nostack));
}

/// HLT instruction - halt CPU until interrupt
///
/// # Safety
///
/// This is a privileged instruction that should only be used
/// in kernel mode
#[inline]
pub unsafe fn hlt() {
    core::arch::asm!("hlt", options(nomem, nostack));
}

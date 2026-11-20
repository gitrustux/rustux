// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64's answers to what the rest of the kernel asks of the processor
//! (crate::arch::cpu, docs/PORTING.md). Another architecture's port gives
//! the same functions.

/// Disable interrupts; whether they were enabled
#[inline]
pub fn irq_save() -> bool {
    let flags: u64;
    unsafe { core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem)) };
    flags & (1 << 9) != 0
}

/// Re-enable interrupts if `enabled` (from irq_save)
#[inline]
pub fn irq_restore(enabled: bool) {
    if enabled {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
}

/// Disable interrupts
#[inline]
pub fn irq_disable() {
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// Enable interrupts
#[inline]
pub fn irq_enable() {
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}

/// Wait for the next interrupt, with interrupts enabled only while
/// waiting (`sti; hlt` takes a pending interrupt only once `hlt` has
/// started, so a wake-up cannot be missed); disabled again after
#[inline]
pub fn wait_for_interrupt() {
    unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
}

/// Wait for the next interrupt as they are (enabled or not)
#[inline]
pub fn halt() {
    unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
}

/// Stop this processor for good
#[inline]
pub fn halt_forever() -> ! {
    loop {
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// The address a page fault was for (CR2)
#[inline]
pub fn fault_address() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) v, options(nomem, nostack)) };
    v
}

/// The physical address of the page tables in use (CR3, without flags)
#[inline]
pub fn page_table_root() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) v, options(nomem, nostack)) };
    v & 0x000f_ffff_ffff_f000
}

/// A save area for the floating-point and vector registers
#[repr(C, align(16))]
pub struct FpuArea(pub [u8; 512]);

impl FpuArea {
    pub const fn new() -> FpuArea {
        FpuArea([0; 512])
    }

    /// Save this processor's floating-point and vector registers
    #[inline]
    pub fn save(&mut self) {
        unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) self.0.as_mut_ptr(), options(nostack)) };
    }

    /// Load them back
    #[inline]
    pub fn restore(&self) {
        unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) self.0.as_ptr(), options(nostack)) };
    }
}

/// The thread pointer user mode sees (the FS base)
#[inline]
pub fn set_user_thread_pointer(v: u64) {
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") 0xC000_0100u32, in("eax") v as u32, in("edx") (v >> 32) as u32,
                         options(nomem, nostack, preserves_flags));
    }
}

/// A byte to the emulator's debug console (QEMU's port 0xE9)
#[inline(always)]
pub fn debug_out(b: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags)) };
}

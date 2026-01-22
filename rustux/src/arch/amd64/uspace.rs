// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 Userspace Transition
//!
//! This module provides functionality to transition from kernel mode
//! to user mode for process execution.

#![allow(dead_code)]

use core::arch::asm;

/// User code segment selector (RPL=3)
const USER_CS: u64 = 0x1B;

/// User data segment selector (RPL=3)
const USER_DS: u64 = 0x23;

/// Execute a loaded process image
///
/// This function transitions from kernel mode to user mode and
/// begins execution of the loaded process.
///
/// # Arguments
///
/// * `entry` - Entry point address
/// * `stack_top` - Stack top address
/// * `cr3` - Page table base address
///
/// # Safety
///
/// This function never returns. The caller must ensure that:
/// - The entry point points to valid executable code
/// - The stack is properly mapped
/// - The page tables are correctly configured
/// - All segments are mapped at the correct addresses
///
/// # Note
///
/// This function performs the following steps:
/// 1. Loads the new CR3 (page table base)
/// 2. Sets up user mode segment selectors
/// 3. Uses IRETQ to switch to user mode at the entry point
pub unsafe fn execute_process(entry: u64, stack_top: u64, _cr3: u64) -> ! {
    // NOTE: CR3 switch is disabled for now - we use kernel page table
    // TODO: Implement proper address space switching with kernel PML4 template

    // Debug: Entry point
    {
        let msg = b"[USPACE] entry=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = entry;
        let mut buf = [0u8; 16];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b" stack=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = stack_top;
        let mut buf = [0u8; 16];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b"\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    // Load process CR3 to switch to process page table
    {
        let msg = b"[USPACE] Loading process CR3=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = _cr3;
        let mut buf = [0u8; 16];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'A', options(nomem, nostack));
    }

    // Load the process's CR3 (switch to process page table)
    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'B', options(nomem, nostack));
    asm!(
        "mov cr3, {cr3}",
        cr3 = in(reg) _cr3,
        options(nostack)
    );
    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'C', options(nomem, nostack));

    // SANITY CHECK: Read first byte of userspace entry after CR3 switch
    // This verifies the mapping exists and is accessible
    {
        let first_byte = unsafe { *(entry as *const u8) };
        let msg = b"[USPACE] First userspace byte = 0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = first_byte as u64;
        let mut buf = [0u8; 4];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
    }

    // Step 3: Set up user data segments (DS, ES, FS, GS) but NOT SS
    // SS will be set by IRETQ
    {
        let msg = b"[USPACE] Setting up user data segments...\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }
    asm!(
        "mov ds, {ds}",
        "mov es, {ds}",
        "mov fs, {ds}",
        "mov gs, {ds}",
        // NOTE: Don't set SS here - IRETQ will do it
        ds = in(reg) USER_DS as u16,
        options(nostack)
    );
    {
        let msg = b"[USPACE] Data segments set, preparing IRETQ...\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    // Step 4: Use IRETQ to switch to user mode
    // IRETQ pops: RIP, CS, RFLAGS, RSP, SS (in that order from stack)
    // We push in reverse order: SS, RSP, RFLAGS, CS, RIP
    {
        let msg = b"[USPACE] IRETQ frame: CS=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = USER_CS;
        let mut buf = [0u8; 4];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b" SS=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = USER_DS;
        let mut buf = [0u8; 4];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b"\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let msg = b"[USPACE] About to execute IRETQ to user mode...\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }
    // RFLAGS: IF=1 (interrupts enabled), IOPL=3 (allows userspace I/O)
    // 0x3202 = (3 << 12) | (1 << 9) | 1
    asm!(
        "push {ss}",          // Stack selector (will be loaded into SS by IRETQ)
        "push {rsp_val}",     // Stack pointer (will be loaded into RSP by IRETQ)
        "push {rflags}",      // RFLAGS (IF=1, IOPL=3)
        "push {cs}",          // Code selector (will be loaded into CS by IRETQ)
        "push {entry}",       // Entry point (will be loaded into RIP by IRETQ)
        "iretq",              // Interrupt return to user mode
        ss = in(reg) USER_DS as u64,
        rsp_val = in(reg) stack_top,
        rflags = in(reg) 0x3202u64, // IF=1, IOPL=3
        cs = in(reg) USER_CS,
        entry = in(reg) entry,
        options(noreturn)
    );
}

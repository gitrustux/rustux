// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 Context Switch Assembly
//!
//! This file contains the low-level assembly code for context switching
//! between processes. It saves all CPU state from the current process
//! and restores the saved state of the next process.
//!
//!

#ifdef __ELF__
.section .text
#endif

.global context_switch
.type context_switch, @function

/// void context_switch(SavedState *prev, SavedState *next, u64 next_cr3)
///
/// Context switch from one process to another.
///
/// Arguments:
///   RDI = prev - pointer to SavedState to save current state
///   RSI = next - pointer to SavedState to restore next state
///   RDX = next_cr3 - CR3 value for the next process
///
/// This function:
/// 1. Saves all general-purpose registers to prev
/// 2. Saves CR3, RFLAGS, RIP, CS, SS
/// 3. Saves FPU state with FXSAVE
/// 4. Loads the next CR3 (switches page tables)
/// 5. Restores all registers from next
/// 6. Restores FPU state with FXRSTOR
/// 7. Returns to the next process's RIP
///
/// Calling convention: System V AMD64 ABI
context_switch:
    // ============================================================
    // Save current state to prev (pointed to by RDI)
    // ============================================================

    // Save general-purpose registers
    movq    %rax, (%rdi)          // prev->rax = RAX
    movq    %rbx, 8(%rdi)         // prev->rbx = RBX
    movq    %rcx, 16(%rdi)        // prev->rcx = RCX
    movq    %rdx, 24(%rdi)        // prev->rdx = RDX
    movq    %rsi, 32(%rdi)        // prev->rsi = RSI
    movq    %rbp, 40(%rdi)        // prev->rbp = RBP

    // Save RSP - this is the stack pointer where we'll return
    movq    %rsp, 48(%rdi)        // prev->rsp = RSP

    movq    %r8,  56(%rdi)        // prev->r8  = R8
    movq    %r9,  64(%rdi)        // prev->r9  = R9
    movq    %r10, 72(%rdi)        // prev->r10 = R10
    movq    %r11, 80(%rdi)        // prev->r11 = R11
    movq    %r12, 88(%rdi)        // prev->r12 = R12
    movq    %r13, 96(%rdi)        // prev->r13 = R13
    movq    %r14, 104(%rdi)       // prev->r14 = R14
    movq    %r15, 112(%rdi)       // prev->r15 = R15

    // Save CR3 (current page table)
    movq    %cr3, %rax
    movq    %rax, 120(%rdi)       // prev->cr3 = CR3

    // Save RFLAGS
    pushfq
    popq    %rax
    movq    %rax, 128(%rdi)       // prev->rflags = RFLAGS

    // Save RIP (where we'll return to)
    // We use a trick: push the return address, then read it
    // The return address is already on the stack from the call
    movq    (%rsp), %rax
    movq    %rax, 136(%rdi)       // prev->rip = return address

    // Save segment selectors
    movw    %cs, %ax
    movzwq  %ax, %rax
    movq    %rax, 144(%rdi)       // prev->cs = CS

    movw    %ss, %ax
    movzwq  %ax, %rax
    movq    %rax, 152(%rdi)       // prev->ss = SS

    // Save FPU state (512 bytes at offset 160)
    fxsave  160(%rdi)

    // ============================================================
    // Switch to next process's page table
    // ============================================================

    // Load next CR3 (RDX contains next_cr3)
    movq    %rdx, %cr3

    // ============================================================
    // Restore next state from next (pointed to by RSI)
    // ============================================================

    // Restore general-purpose registers
    movq    (%rsi), %rax          // RAX = next->rax
    movq    8(%rsi), %rbx         // RBX = next->rbx
    movq    16(%rsi), %rcx        // RCX = next->rcx
    movq    24(%rsi), %rdx        // RDX = next->rdx
    // Note: RSI is restored after we finish using it as a pointer
    movq    40(%rsi), %rbp        // RBP = next->rbp
    movq    48(%rsi), %rsp        // RSP = next->rsp

    movq    56(%rsi), %r8         // R8  = next->r8
    movq    64(%rsi), %r9         // R9  = next->r9
    movq    72(%rsi), %r10        // R10 = next->r10
    movq    80(%rsi), %r11        // R11 = next->r11
    movq    88(%rsi), %r12        // R12 = next->r12
    movq    96(%rsi), %r13        // R13 = next->r13
    movq    104(%rsi), %r14       // R14 = next->r14
    movq    112(%rsi), %r15       // R15 = next->r15

    // Save RSI value temporarily before restoring it
    movq    %rsi, %rcx            // RCX = pointer to next

    // Restore RFLAGS
    movq    128(%rcx), %rax
    pushq   %rax
    popfq

    // Restore segment selectors
    movq    144(%rcx), %rax
    movw    %ax, %cs              // CS = next->cs

    movq    152(%rcx), %rax
    movw    %ax, %ss              // SS = next->ss

    // Restore FPU state (512 bytes at offset 160)
    fxrstor 160(%rcx)

    // Restore RDI and RSI last (they're our pointer registers)
    // We've already saved RSI in RCX
    movq    32(%rcx), %rsi        // RSI = next->rsi
    movq    40(%rcx), %rdi        // RDI = next->rdi (overwrites our prev pointer)

    // Note: RAX, RBX, RDX, RBP, R8-R15 were already restored
    // RCX still has our temporary pointer, restore it now
    movq    16(%rcx), %rcx        // RCX = next->rcx

    // Restore RSP was done earlier
    // RSP is already set to next->rsp

    // Jump to the next instruction
    // The RIP is at offset 136
    // We use a special trick: read the RIP and jump to it
    movq    136(%rcx), %rax
    jmp     *%rax

    // ============================================================
    // Return point after context switch
    // ============================================================
    // When we return here, the stack has been restored to the
    // previous process's stack and we just need to return
    // Actually, we jump directly to the saved RIP, so we never
    // reach here. The "ret" instruction below is never executed.

    ret

.size context_switch, .-context_switch

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Running processes: kernel-stack switching between user processes.
//!
//! Every process has its own kernel stack. While a process runs in user mode
//! its kernel stack is empty and TSS.RSP0 points at its top, so interrupts
//! and `int 0x80` from that process land there. To switch processes the
//! kernel saves the callee-saved registers on the current kernel stack,
//! records the stack pointer in the process, loads the next process's page
//! tables and stack pointer, and returns into wherever that process left off
//! (inside a syscall, an interrupt handler, or, for a new process, a
//! trampoline that enters user mode with `iretq`).
//!
//! Switches happen on `yield`, on `exit`, and on timer interrupts taken in
//! user mode. Kernel code is never preempted, so spin locks held in the
//! kernel cannot be contended by another process on this single CPU.

use crate::process::table::{ProcessState, PROCESS_TABLE};

core::arch::global_asm!(
    // rustux_switch_stacks(save_rsp: *mut u64, new_rsp: u64, new_cr3: u64)  [sysv64]
    ".global rustux_switch_stacks",
    "rustux_switch_stacks:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov cr3, rdx",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    // First entry of a new process: the IRETQ frame is on the stack.
    ".global rustux_enter_user",
    "rustux_enter_user:",
    "mov ax, 0x23",
    "mov ds, ax",
    "mov es, ax",
    "xor eax, eax",
    "iretq",
);

extern "sysv64" {
    fn rustux_switch_stacks(save_rsp: *mut u64, new_rsp: u64, new_cr3: u64);
    fn rustux_enter_user();
}

/// User code selector (GDT entry 3, RPL 3)
const USER_CS: u64 = 0x1B;
/// User data/stack selector (GDT entry 4, RPL 3)
const USER_DS: u64 = 0x23;
/// RFLAGS for new user processes: IF set, reserved bit 1 set
const USER_RFLAGS: u64 = 0x202;

/// Lay out a new process's kernel stack so that the first switch to it
/// "returns" into [`rustux_enter_user`], which IRETQs to `entry`.
/// Returns the value to store as the process's saved kernel RSP.
///
/// # Safety
/// `kernel_stack_top` must be the 16-byte aligned top of a mapped, writable
/// kernel stack of at least 128 bytes.
pub unsafe fn prepare_kernel_stack(kernel_stack_top: u64, entry: u64, user_stack_top: u64) -> u64 {
    // Lowest address first: r15 r14 r13 r12 rbx rbp | ret | rip cs rflags rsp ss
    let frame: [u64; 12] = [
        0, 0, 0, 0, 0, 0,
        rustux_enter_user as usize as u64,
        entry,
        USER_CS,
        USER_RFLAGS,
        user_stack_top,
        USER_DS,
    ];
    // Keep the IRETQ frame 16-byte aligned like a CPU-pushed one
    let rsp = (kernel_stack_top - 8 * 12 - 8) & !0xF;
    let p = rsp as *mut u64;
    for (i, v) in frame.iter().enumerate() {
        p.add(i).write(*v);
    }
    rsp
}

/// Size of each process's kernel stack. Syscall paths (ELF loading in
/// `spawn`, for example) need well over 16 KiB.
pub const KERNEL_STACK_PAGES: usize = 32; // 128 KiB

/// Allocate a physically contiguous kernel stack; returns its top (an HHDM
/// address valid in every address space).
pub fn alloc_kernel_stack() -> Result<u64, &'static str> {
    use crate::mm::pmm;
    let base = pmm::pmm_alloc_contiguous(KERNEL_STACK_PAGES, pmm::PMM_ALLOC_FLAG_KERNEL, 0)
        .map_err(|_| "out of memory for kernel stack")?;
    Ok(pmm::paddr_to_vaddr(base) as u64 + (KERNEL_STACK_PAGES * 4096) as u64)
}

fn save_and_disable_interrupts() -> u64 {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem));
    }
    flags
}

fn restore_interrupts(flags: u64) {
    if flags & 0x200 != 0 {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
}

/// Where the boot context's stack pointer goes when the first process starts.
/// The boot context is never resumed.
static mut BOOT_RSP: u64 = 0;

/// Start running process `pid` from the boot context. Never returns.
pub fn start(pid: u32) -> ! {
    let _ = save_and_disable_interrupts();
    let (rsp, cr3, kstack) = {
        let mut table = PROCESS_TABLE.lock();
        let p = table.get_mut(pid).expect("run::start: no such process");
        p.state = ProcessState::Running;
        let v = (p.kernel_rsp, p.page_table, p.kernel_stack);
        table.set_current(pid);
        v
    };
    unsafe {
        crate::arch::amd64::descriptor::set_tss_rsp0(kstack);
        rustux_switch_stacks(core::ptr::addr_of_mut!(BOOT_RSP), rsp, cr3);
    }
    unreachable!("boot context resumed");
}

/// Give up the CPU to the next runnable process, if any. The current process
/// stays runnable and continues later from this call.
pub fn yield_now() {
    switch_away(ProcessState::Ready);
}

/// End the current process and run the next one. Never returns.
pub fn exit_current() -> ! {
    switch_away(ProcessState::Zombie);
    // No other process can run: idle in the kernel's own address space.
    unsafe {
        let pml4 = crate::arch::amd64::mm::kernel_map::kernel_pml4();
        core::arch::asm!("mov cr3, {}", in(reg) pml4, options(nostack));
        loop {
            core::arch::asm!("sti", "hlt", options(nomem, nostack));
        }
    }
}

/// Switch to the next runnable process, leaving the current one in `state`.
/// Returns when the current process is scheduled again (or immediately if
/// nothing else can run and the current process is still runnable). For
/// non-runnable states it returns only if no other process exists.
fn switch_away(state: ProcessState) {
    let flags = save_and_disable_interrupts();
    let switch = {
        let mut table = PROCESS_TABLE.lock();
        let cur = match table.current_pid() {
            Some(c) => c,
            None => {
                drop(table);
                restore_interrupts(flags);
                return;
            }
        };
        if let Some(p) = table.get_mut(cur) {
            p.state = state;
        }
        match table.find_next_runnable(Some(cur)) {
            Some(next) if next != cur => {
                let save: *mut u64 = match table.get_mut(cur) {
                    Some(p) => core::ptr::addr_of_mut!(p.kernel_rsp),
                    None => core::ptr::addr_of_mut!(BOOT_RSP),
                };
                let n = table.get_mut(next).expect("runnable process vanished");
                n.state = ProcessState::Running;
                let v = (save, n.kernel_rsp, n.page_table, n.kernel_stack);
                table.set_current(next);
                Some(v)
            }
            _ => {
                if state == ProcessState::Ready {
                    if let Some(p) = table.get_mut(cur) {
                        p.state = ProcessState::Running;
                    }
                }
                None
            }
        }
    };
    if let Some((save, rsp, cr3, kstack)) = switch {
        unsafe {
            crate::arch::amd64::descriptor::set_tss_rsp0(kstack);
            rustux_switch_stacks(save, rsp, cr3);
        }
    }
    restore_interrupts(flags);
}

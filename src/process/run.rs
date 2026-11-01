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
//! Switches happen on `yield`, on `exit`, when a process blocks, and on
//! timer interrupts taken in user mode. Kernel code is never preempted, so
//! spin locks held in the kernel cannot be contended by another process on
//! this single CPU.
//!
//! Blocking: [`block_current`] marks the process `Blocked` with an optional
//! deadline. [`wake`] only sets an atomic flag, so it is safe from any
//! context, interrupt handlers included. The scheduler treats a blocked
//! process as runnable once its flag is set or its deadline has passed. When
//! nothing can run, the CPU halts until the next interrupt (timer tick,
//! keyboard) and looks again.

use crate::process::table::{ProcessState, PROCESS_TABLE};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Process ids are below this (the process table size)
const MAX_PIDS: usize = 256;

#[allow(clippy::declare_interior_mutable_const)]
const WAKE_INIT: AtomicBool = AtomicBool::new(false);
#[allow(clippy::declare_interior_mutable_const)]
const DEADLINE_INIT: AtomicU64 = AtomicU64::new(u64::MAX);
/// Set by [`wake`]; consumed when the process is scheduled
static WAKE: [AtomicBool; MAX_PIDS] = [WAKE_INIT; MAX_PIDS];
/// Monotonic ns at which a blocked process becomes runnable anyway
static DEADLINE: [AtomicU64; MAX_PIDS] = [DEADLINE_INIT; MAX_PIDS];

/// Make `pid` runnable if it is blocked (or make its next block return at
/// once). Safe from interrupt handlers.
pub fn wake(pid: u32) {
    if (pid as usize) < MAX_PIDS {
        WAKE[pid as usize].store(true, Ordering::Release);
    }
}

/// Block the current process until [`wake`] is called for it or the
/// monotonic clock reaches `deadline` (ns; `u64::MAX` = no deadline).
/// Returns at once if a wake-up is already pending. Callers re-check their
/// condition afterwards: wake-ups can be spurious.
pub fn block_current(deadline: u64) {
    let flags = save_and_disable_interrupts();
    let cur = PROCESS_TABLE.lock().current_pid();
    if let Some(cur) = cur {
        let i = cur as usize;
        if i < MAX_PIDS && !WAKE[i].swap(false, Ordering::AcqRel) {
            DEADLINE[i].store(deadline, Ordering::Release);
            switch_away(ProcessState::Blocked);
            WAKE[i].store(false, Ordering::Release);
            DEADLINE[i].store(u64::MAX, Ordering::Release);
        }
    }
    restore_interrupts(flags);
}

/// Next process to run after `cur` (round robin), `cur` itself last.
/// Blocked processes whose wake flag is set or deadline passed become Ready.
fn pick_next(table: &mut crate::process::table::ProcessTable, cur: u32) -> Option<u32> {
    let mut now: Option<u64> = None;
    for k in 1..=MAX_PIDS as u32 {
        let pid = (cur + k) % MAX_PIDS as u32;
        let p = match table.get_mut(pid) {
            Some(p) => p,
            None => continue,
        };
        match p.state {
            ProcessState::Ready | ProcessState::Running => return Some(pid),
            ProcessState::Blocked => {
                let i = pid as usize;
                let deadline = DEADLINE[i].load(Ordering::Acquire);
                let expired = deadline != u64::MAX && {
                    let n = *now.get_or_insert_with(crate::kobj::now_ns);
                    n >= deadline
                };
                if WAKE[i].load(Ordering::Acquire) || expired {
                    p.state = ProcessState::Ready;
                    return Some(pid);
                }
            }
            _ => {}
        }
    }
    None
}

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
    // RBX and R12 (restored from the prepared frame) hold the two start
    // arguments; every other register is cleared so no kernel value leaks.
    "rustux_enter_user:",
    "mov ax, 0x23",
    "mov ds, ax",
    "mov es, ax",
    "mov rdi, rbx",
    "mov rsi, r12",
    "xor eax, eax",
    "xor ebx, ebx",
    "xor ecx, ecx",
    "xor edx, edx",
    "xor ebp, ebp",
    "xor r8d, r8d",
    "xor r9d, r9d",
    "xor r10d, r10d",
    "xor r11d, r11d",
    "xor r12d, r12d",
    "xor r13d, r13d",
    "xor r14d, r14d",
    "xor r15d, r15d",
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

/// Set the two values a new process finds in RDI and RSI when it first
/// enters user mode (`kernel_rsp` as returned by [`prepare_kernel_stack`]).
///
/// # Safety
/// The process must not have run yet.
pub unsafe fn set_start_args(kernel_rsp: u64, arg1: u64, arg2: u64) {
    let p = kernel_rsp as *mut u64;
    p.add(4).write(arg1); // restored into RBX
    p.add(3).write(arg2); // restored into R12
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
/// Its handles are closed first (peers see PEER_CLOSED, waiters wake).
pub fn exit_current() -> ! {
    let (objects, mappings) = {
        let mut table = PROCESS_TABLE.lock();
        match table.current_pid().and_then(|c| table.get_mut(c)) {
            Some(p) => (p.handles.take_all(), Some(p.aspace.destroy())),
            None => (alloc::vec::Vec::new(), None),
        }
    };
    drop(objects);
    drop(mappings);
    switch_away(ProcessState::Zombie);
    // Not reached: switch_away idles until another process can run, and a
    // zombie is never scheduled again.
    loop {
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Switch to the next runnable process, leaving the current one in `state`.
/// Returns when the current process is scheduled again (at once if it is
/// still runnable and nothing else is). If nothing at all can run, halts
/// until an interrupt and looks again.
fn switch_away(state: ProcessState) {
    let flags = save_and_disable_interrupts();
    let mut first = true;
    loop {
        let mut table = PROCESS_TABLE.lock();
        let cur = match table.current_pid() {
            Some(c) => c,
            None => {
                drop(table);
                restore_interrupts(flags);
                return;
            }
        };
        if first {
            if let Some(p) = table.get_mut(cur) {
                p.state = state;
            }
            first = false;
        }
        match pick_next(&mut table, cur) {
            Some(next) if next != cur => {
                let save: *mut u64 = match table.get_mut(cur) {
                    Some(p) => core::ptr::addr_of_mut!(p.kernel_rsp),
                    None => core::ptr::addr_of_mut!(BOOT_RSP),
                };
                let n = table.get_mut(next).expect("runnable process vanished");
                n.state = ProcessState::Running;
                let (rsp, cr3, kstack) = (n.kernel_rsp, n.page_table, n.kernel_stack);
                table.set_current(next);
                drop(table);
                unsafe {
                    crate::arch::amd64::descriptor::set_tss_rsp0(kstack);
                    rustux_switch_stacks(save, rsp, cr3);
                }
                // Scheduled again
                restore_interrupts(flags);
                return;
            }
            Some(_) => {
                // The current process itself is runnable (yield with nothing
                // else ready, or woken while looking)
                if let Some(p) = table.get_mut(cur) {
                    p.state = ProcessState::Running;
                }
                drop(table);
                restore_interrupts(flags);
                return;
            }
            None => {
                // Nothing can run: wait for an interrupt (timer tick,
                // keyboard) that may wake someone or pass a deadline
                drop(table);
                unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
            }
        }
    }
}

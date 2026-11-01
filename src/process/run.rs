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
//!
//! Killing: table entries are threads (see `kobj::task`). A thread is never
//! removed while it may hold references on its kernel stack; [`kill`] sets
//! a flag and wakes it, and the thread exits itself the next time it would
//! return to user mode (end of a syscall, timer preemption), unwinding
//! whatever it was doing first (waits return early once killed). The
//! process is finished off when its last thread is gone. Exited entries
//! are reaped (kernel stack freed, id reused) when a new thread starts.

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
/// Set by [`kill`]: the thread must exit before returning to user mode
static KILLED: [AtomicBool; MAX_PIDS] = [WAKE_INIT; MAX_PIDS];

/// Ask thread `tid` to exit (it does so at its next return to user mode)
pub fn kill(tid: u32) {
    if (tid as usize) < MAX_PIDS {
        KILLED[tid as usize].store(true, Ordering::Release);
        wake(tid);
    }
}

/// Whether the current thread has been killed (long waits check this and
/// return early)
pub fn is_killed() -> bool {
    match PROCESS_TABLE.lock().current_pid() {
        Some(c) if (c as usize) < MAX_PIDS => KILLED[c as usize].load(Ordering::Acquire),
        _ => false,
    }
}

/// Exit the current thread if it was killed. Called on the way back to
/// user mode.
pub fn exit_if_killed() {
    if is_killed() {
        exit_thread();
    }
}

/// Clear the per-id flags of a newly started thread
pub fn reset_flags(tid: u32) {
    if (tid as usize) < MAX_PIDS {
        let i = tid as usize;
        WAKE[i].store(false, Ordering::Release);
        KILLED[i].store(false, Ordering::Release);
        DEADLINE[i].store(u64::MAX, Ordering::Release);
    }
}

/// Put `process`'s I/O port permissions in the TSS (interrupts disabled)
fn load_ports(process: &crate::kobj::task::Process) {
    let m = process.io_map.lock();
    unsafe { crate::arch::amd64::descriptor::load_io_map(process.koid, m.as_deref(), false) };
}

/// The current thread's object
pub fn current_thread() -> Option<(u32, alloc::sync::Arc<crate::kobj::task::Thread>)> {
    let table = PROCESS_TABLE.lock();
    let cur = table.current_pid()?;
    table.get(cur).map(|p| (cur, p.thread.clone()))
}

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
        if i < MAX_PIDS && !KILLED[i].load(Ordering::Acquire) && !WAKE[i].swap(false, Ordering::AcqRel) {
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
        load_ports(&p.thread.process);
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

/// End the current thread and run the next one. Never returns. If it was
/// the last thread of its process, the process is finished: its handles
/// are closed (peers see PEER_CLOSED), its memory is freed, and it is
/// signalled TERMINATED.
pub fn exit_thread() -> ! {
    if let Some((tid, thread)) = current_thread() {
        thread.mark_dead();
        if thread.process.thread_exited(tid) {
            let remains = thread.process.finalize();
            drop(remains);
        }
        drop(thread);
    }
    switch_away(ProcessState::Zombie);
    // Not reached: a zombie is never scheduled again
    loop {
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Kill a process with return code `code`: every thread is asked to exit
/// and the process is finished when the last one has. If the current
/// thread belongs to it, this does not return. Takes the reference by value
/// so nothing stays behind on a kernel stack that is never unwound.
pub fn kill_process(process: alloc::sync::Arc<crate::kobj::task::Process>, code: i64) {
    if let Some(threads) = process.begin_kill(code) {
        if threads.is_empty() {
            drop(process.finalize());
        }
        for tid in threads {
            kill(tid);
        }
    }
    let mine = match current_thread() {
        Some((_, t)) => alloc::sync::Arc::ptr_eq(&t.process, &process),
        None => false,
    };
    drop(process);
    if mine {
        exit_thread();
    }
}

/// End the current process with `code` (process_exit, fatal CPU exception).
/// Never returns.
pub fn exit_current(code: i64) -> ! {
    if let Some(p) = current_thread().map(|(_, t)| t.process.clone()) {
        kill_process(p, code);
    }
    exit_thread();
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
                load_ports(&n.thread.process);
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

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Running threads on every CPU: kernel-stack switching.
//!
//! Every thread has its own kernel stack. While a thread runs in user mode
//! its kernel stack is empty and the CPU's TSS.RSP0 points at its top, so
//! interrupts and `int 0x80` land there. To switch, the kernel saves the
//! callee-saved registers on the current kernel stack, records the stack
//! pointer, loads the next thread's page tables and stack pointer, and
//! returns into wherever that thread left off (inside a syscall, the timer
//! handler, or, for a new thread, a trampoline that enters user mode).
//!
//! **CPUs.** Each CPU has a current thread (in `percpu`) and an idle
//! context on its own boot stack, which runs [`idle_loop`]: pick a runnable
//! thread and switch to it, or halt until an interrupt. A thread that stops
//! running with nothing else to do switches to its CPU's idle context; it
//! never idles on its own stack, which another CPU may want to resume.
//!
//! **On-CPU flag.** A thread is marked on-CPU from the moment a CPU picks
//! it until that CPU has finished switching *away* from it: the next
//! context on that CPU clears the flag ([`finish_switch`]). Other CPUs never
//! pick, and the reaper never frees, a thread whose flag is set, so no stack
//! is ever used by two CPUs.
//!
//! **Interrupts.** Kernel code runs with interrupts disabled (system calls
//! enter through an interrupt gate and stay that way); interrupts are taken
//! in user mode and while a CPU idles. Kernel code is never preempted;
//! the timer preempts user mode on every CPU.
//!
//! **Blocking.** [`block_current`] marks the thread `Blocked` with an
//! optional deadline. [`wake`] sets an atomic flag (safe from any context,
//! interrupt handlers included) and sends an IPI to idle CPUs; a blocked
//! thread becomes runnable once its flag is set or its deadline passed.
//!
//! **Killing.** [`kill`] sets a flag and wakes the thread; it exits itself
//! the next time it would return to user mode, unwinding first (waits
//! return early once killed). The process is finished when its last thread
//! is gone. Exited entries are reaped when a new thread starts.

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
/// The thread is running on a CPU, or a CPU is still switching away from
/// it (see module docs)
static ON_CPU: [AtomicBool; MAX_PIDS] = [WAKE_INIT; MAX_PIDS];
/// One bit per CPU index: halted in its idle loop
static IDLE_CPUS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Where the next idle CPU starts looking (round robin)
static PICK_FROM: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Whether thread `tid` is on a CPU (the reaper must leave it alone)
pub fn on_cpu(tid: u32) -> bool {
    (tid as usize) < MAX_PIDS && ON_CPU[tid as usize].load(Ordering::Acquire)
}

/// Clear the on-CPU flag of the thread this CPU just switched away from.
/// Called by whatever runs next on the CPU, once the old stack is unused.
fn finish_switch() {
    let prev = crate::arch::amd64::percpu::this().prev.swap(0, Ordering::AcqRel);
    if prev != 0 && (prev as usize) < MAX_PIDS {
        ON_CPU[prev as usize].store(false, Ordering::Release);
    }
}

#[no_mangle]
extern "sysv64" fn rustux_finish_switch() {
    finish_switch();
}

/// Wake idle CPUs so they look for work
fn kick_idle_cpus() {
    let idle = IDLE_CPUS.load(Ordering::Acquire);
    if idle != 0 {
        crate::arch::amd64::smp::wake_cpus(idle);
    }
}

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
        ON_CPU[i].store(false, Ordering::Release);
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
        kick_idle_cpus();
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

/// Next thread to run on this CPU: round robin from `cur` (or a shared
/// cursor for an idle CPU), `cur` itself last. Threads on other CPUs are
/// skipped. Blocked threads whose wake flag is set or deadline passed
/// become Ready.
fn pick_next(table: &mut crate::process::table::ProcessTable, cur: Option<u32>) -> Option<u32> {
    let mut now: Option<u64> = None;
    let start = cur.unwrap_or_else(|| PICK_FROM.fetch_add(1, Ordering::Relaxed) % MAX_PIDS as u32);
    for k in 1..=MAX_PIDS as u32 {
        let pid = (start + k) % MAX_PIDS as u32;
        if Some(pid) != cur && ON_CPU[pid as usize].load(Ordering::Acquire) {
            continue;
        }
        let p = match table.get_mut(pid) {
            Some(p) => p,
            None => continue,
        };
        match p.state {
            ProcessState::Ready => return Some(pid),
            ProcessState::Running if Some(pid) == cur => return Some(pid),
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

/// Make `next` this CPU's running thread (table locked); returns the stack
/// pointer and page tables to switch to
fn take_cpu(table: &mut crate::process::table::ProcessTable, next: u32) -> (u64, u64) {
    let cpu = crate::arch::amd64::percpu::this();
    ON_CPU[next as usize].store(true, Ordering::Release);
    let n = table.get_mut(next).expect("runnable thread vanished");
    n.state = ProcessState::Running;
    let (rsp, cr3, kstack) = (n.kernel_rsp, n.page_table, n.kernel_stack);
    load_ports(&n.thread.process);
    cpu.set_current(Some(next));
    // Published before CR3 is loaded (TLB shootdowns rely on the order)
    cpu.cr3.store(cr3, Ordering::SeqCst);
    unsafe { crate::arch::amd64::descriptor::set_tss_rsp0(kstack) };
    (rsp, cr3)
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
    // Clear the on-CPU flag of the thread this CPU left (RBX and R12 are
    // callee-saved, so the arguments survive the call)
    "sub rsp, 8",
    "call rustux_finish_switch",
    "add rsp, 8",
    // Fresh FPU/SSE state: no values from other threads
    "fninit",
    "push 0x1F80",
    "ldmxcsr [rsp]",
    "add rsp, 8",
    "xorps xmm0, xmm0",
    "xorps xmm1, xmm1",
    "xorps xmm2, xmm2",
    "xorps xmm3, xmm3",
    "xorps xmm4, xmm4",
    "xorps xmm5, xmm5",
    "xorps xmm6, xmm6",
    "xorps xmm7, xmm7",
    "xorps xmm8, xmm8",
    "xorps xmm9, xmm9",
    "xorps xmm10, xmm10",
    "xorps xmm11, xmm11",
    "xorps xmm12, xmm12",
    "xorps xmm13, xmm13",
    "xorps xmm14, xmm14",
    "xorps xmm15, xmm15",
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

/// A CPU's idle context: run whatever is runnable, halt when nothing is.
/// The boot CPU enters it once `init` exists; every other CPU after its
/// set-up. Never returns. Threads that stop running with nothing else to do
/// switch back here.
pub fn idle_loop() -> ! {
    let _ = save_and_disable_interrupts();
    loop {
        finish_switch();
        let cpu = crate::arch::amd64::percpu::this();
        let bit = 1u32 << cpu.index;
        let mut table = PROCESS_TABLE.lock();
        // Marked idle before looking, so a wake after the look sends an IPI
        IDLE_CPUS.fetch_or(bit, Ordering::AcqRel);
        match pick_next(&mut table, None) {
            Some(next) => {
                IDLE_CPUS.fetch_and(!bit, Ordering::AcqRel);
                cpu.prev.store(0, Ordering::Release);
                let (rsp, cr3) = take_cpu(&mut table, next);
                drop(table);
                unsafe { rustux_switch_stacks(cpu.boot_rsp(), rsp, cr3) };
                // A thread on this CPU had nothing left to run: back here
            }
            None => {
                drop(table);
                // `sti; hlt` takes a pending interrupt only after `hlt`
                // has started, so a wake IPI cannot be missed
                unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
                IDLE_CPUS.fetch_and(!bit, Ordering::AcqRel);
            }
        }
    }
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

/// Switch to the next runnable thread, leaving the current one in `state`.
/// Returns when the current thread runs again, possibly on another CPU (at
/// once if it is still runnable and nothing else is). With nothing to run,
/// switches to this CPU's idle context.
fn switch_away(state: ProcessState) {
    let flags = save_and_disable_interrupts();
    {
        let cpu = crate::arch::amd64::percpu::this();
        let mut table = PROCESS_TABLE.lock();
        let Some(cur) = cpu.current() else {
            drop(table);
            restore_interrupts(flags);
            return;
        };
        let save: *mut u64 = match table.get_mut(cur) {
            Some(p) => {
                p.state = state;
                core::ptr::addr_of_mut!(p.kernel_rsp)
            }
            None => {
                drop(table);
                restore_interrupts(flags);
                return;
            }
        };
        match pick_next(&mut table, Some(cur)) {
            Some(next) if next == cur => {
                // Still runnable and nothing else is waiting
                if let Some(p) = table.get_mut(cur) {
                    p.state = ProcessState::Running;
                }
                drop(table);
                restore_interrupts(flags);
                return;
            }
            Some(next) => {
                cpu.prev.store(cur, Ordering::Release);
                let (rsp, cr3) = take_cpu(&mut table, next);
                drop(table);
                unsafe { rustux_switch_stacks(save, rsp, cr3) };
            }
            None => {
                // Nothing else can run: go to this CPU's idle context
                cpu.set_current(None);
                cpu.prev.store(cur, Ordering::Release);
                let kernel = crate::arch::amd64::mm::kernel_map::kernel_pml4();
                cpu.cr3.store(kernel, Ordering::SeqCst);
                let idle_rsp = unsafe { *cpu.boot_rsp() };
                drop(table);
                unsafe { rustux_switch_stacks(save, idle_rsp, kernel) };
            }
        }
    }
    // Running again, perhaps on a different CPU
    finish_switch();
    restore_interrupts(flags);
}

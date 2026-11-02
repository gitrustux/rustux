# Concurrency: globals, CPUs and locks

Since checklist step 5 (November 2026) the kernel has no `static mut`.
`build.rs` fails the build if one appears under `src/`, and the crates use
`#![deny(static_mut_refs)]`.

## How global state is kept

| Kind | Use | Examples |
|------|-----|----------|
| Locked, also touched by interrupt handlers | `sync::IrqMutex` (spin lock; interrupts off while held) | PMM, console, COM1, keyboard input and kernel input channel, USB driver (`USB_LOCK`) |
| Locked, process context only | `sync::SpinMutex` | process table, object state |
| Written once during boot, read afterwards | `sync::BootCell` | IDT, framebuffer info, boot allocator buffer |
| Counters and flags | atomics | ticks, syscall counts, debug flag |
| One per CPU | `arch::amd64::percpu` | GDT, TSS with I/O bitmap, boot-context stack pointer |

The USB driver is entered only through `usb::init` (boot) and `usb::poll`
(timer tick), which take `USB_LOCK`; its globals are `UsbGlobal` cells that
assert the lock is held. The panic handler never waits for the console
lock (its CPU may hold it).

Kernel code runs with interrupts disabled (system calls enter through an
interrupt gate and stay that way; before step 6 the syscall stub turned
them back on, which let the timer tick deadlock on a channel lock held by
the code it interrupted). Interrupts arrive only in user mode or while a
CPU idles, so a handler never interrupts a lock holder on its own CPU;
another CPU holding the lock just makes it wait.

## CPUs

- `arch::amd64::percpu`: up to 16 CPUs. A CPU finds its slot from its
  local APIC ID, which user code cannot change. (A GS-based pointer would
  be faster but needs `swapgs` in every entry path, and the interrupt entry
  points are compiler-generated.) Per CPU: GDT, TSS with I/O bitmap,
  idle-context stack pointer, current thread, the thread it just left,
  loaded CR3, TLB-flush counters.
- `arch::amd64::smp`: the other cores are found in the ACPI MADT (XSDT or
  RSDT, enabled entries only) and started with INIT-SIPI-SIPI through a
  16/32/64-bit trampoline in a page below 1 MiB that is reserved before the
  PMM is built. Each AP copies the boot CPU's CR0/CR4/EFER, loads its own
  GDT/TSS (and reloads CS) and the shared IDT, enables its local APIC,
  starts a 10 ms LAPIC timer and enters the scheduler's idle loop.
  `[SMP] CPUs online: N` on the screen. Build with
  `--features uefi_kernel,no_smp` to skip AP start-up.

## Scheduling on every CPU (step 6)

`process::run` has the details. In short:

- Every CPU has a current thread and an **idle context** on its own stack
  (`idle_loop`). A thread with nothing to switch to goes to its CPU's idle
  context; it never waits on its own stack.
- One process table under one lock; each CPU picks round robin from it,
  skipping threads that are **on a CPU**. That flag is set when a CPU picks
  a thread and cleared by whatever runs next on that CPU once the old stack
  is unused (`finish_switch`). The reaper skips such threads too.
- `wake` sets an atomic flag and sends an IPI (vector 0xF0) to halted CPUs.
  `wait_on` registers as a waiter before it looks at the signals, so a
  signal raised on another CPU in between is not lost.
- **TLB shootdown** (vector 0xF1): after `vmar_unmap`/`vmar_protect`, the
  CPUs whose CR3 is that address space flush and acknowledge; the caller
  waits with no locks held and serves flush requests aimed at itself while
  waiting, so two CPUs shooting at each other cannot deadlock.
- **FPU/SSE**: user registers are saved on the thread's kernel stack in the
  syscall stub and in the timer handler (when it interrupted user mode), so
  they survive kernel SSE use, switches and moving to another CPU. New
  threads start with a clean FPU state.
- USB keyboards are polled on CPU 0's tick only; IRQ 1 goes to CPU 0.
- `smptest` checks parallel speed-up, SSE state across migrations, the
  shootdown (it fails if shootdowns are disabled) and cross-CPU wake-ups.

## Not yet

- One global run queue and process-table lock (fine for a dozen CPUs;
  per-CPU queues with stealing later).
- Device interrupts all go to CPU 0.
- Debug calls `debug_cpu` (0x51) and `debug_cpu_count` (0x52) exist for
  tests only.

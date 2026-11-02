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
interrupt gate); interrupts arrive only in user mode or while a CPU idles.
That is why handlers may take locks that process context also takes.

## CPUs

- `arch::amd64::percpu`: up to 16 CPUs. A CPU finds its slot from its
  local APIC ID, which user code cannot change. (A GS-based pointer would
  be faster but needs `swapgs` in every entry path, and the interrupt entry
  points are compiler-generated.)
- `arch::amd64::smp`: the other cores are found in the ACPI MADT (XSDT or
  RSDT, enabled entries only) and started with INIT-SIPI-SIPI through a
  16/32/64-bit trampoline in a page below 1 MiB that is reserved before the
  PMM is built. Each AP copies the boot CPU's CR0/CR4/EFER, loads its own
  GDT/TSS and the shared IDT, enables its local APIC and **parks** (`cli;
  hlt`). The boot log and the screen show `[SMP] CPUs online: N`.
- Escape hatch: build with `--features uefi_kernel,no_smp` to skip AP
  start-up.

## Not yet

Threads run only on the boot CPU. Using the other cores needs a scheduler
that keeps a current thread per CPU (today the process table has one
`current`), per-CPU run queues or a shared one with a lock, a LAPIC timer
on every CPU, IPIs to wake idle CPUs, and TLB shootdowns when mappings
change. That is checklist step 6.

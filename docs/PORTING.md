# Porting Rustux to other architectures

Rustux runs on x86-64 today. The next architectures are **aarch64**
(first in QEMU's `virt` machine, then real boards) and **riscv64**. This
page says what is ready for them, the rules that keep it ready, and what
a port still has to do.

## What is ready

### The C library

Everything the library needs from the CPU is in one header per
architecture, `userspace/libc/include/rustux/arch/ARCH.h`, picked by
`<rustux/arch.h>`:

| | x86-64 | aarch64 | riscv64 |
|---|---|---|---|
| `rx_syscall()` | `int 0x80`: number in `rax`, arguments `rdi rsi rdx r10 r8 r9`, result `rax` | `svc #0`: number in `x8`, arguments `x0`–`x5`, result `x0` | `ecall`: number in `a7`, arguments `a0`–`a5`, result `a0` |
| `rx_arch_thread_pointer()` (the thread's `struct __pthread`) | `fs:0` | `tpidr_el0` | `tp` |
| `rx_arch_cycles()` | `rdtsc` | `cntvct_el0` | `rdtime` |
| `rx_arch_hw_random()` | `rdrand` | none yet (returns 0) | none yet (returns 0) |
| `rx_arch_barrier()` (DMA) | `mfence` | `dmb sy` | `fence iorw, iorw` |
| `rx_cpu_relax()` | `pause` | `yield` | `nop` |
| I/O ports (`rx_inb()` ...) | yes, `RX_ARCH_HAS_PORT_IO` | none | none |

`docs/SYSCALL.md` has the full calling conventions. Every architecture
keeps all registers but the result unchanged across a system call.

A new thread starts in `__thread_entry`, a few instructions in
`userspace/libc/src/arch/ARCH.c` that pass the thread to `__thread_start()`
in C. `string.c` uses `rep movsb`/`stosb` on x86-64 and plain loops
elsewhere. `math.c` uses the x87 unit on x86-64 and plain C elsewhere
(`log`, `exp`, `pow`, `fmod`, `sin`, `cos`, `atan2`, within about 3 units
in the last place; testable on x86 with `-DRX_PORTABLE_MATH`).
`userspace/build.sh` builds `libc/src/arch/x86_64.c`; a port's build
takes its own.

### The Rust programs

No Rust program has assembly. They make system calls through the C
library's `__rx_syscall(n, a, b, c, d, e, f)` (`rpg`, `netd`, the System
Monitor), so the instruction is written once per architecture, in C. All
of them (and the toolkit, the desktop, the apps, FoxDen) pass `cargo
check` for `aarch64-unknown-linux-musl` and `riscv64gc-unknown-linux-musl`.

### The C programs

All build for aarch64 and riscv64 except `ps2kbd` (the PS/2 controller is
a PC device): `userspace/X86-ONLY`. `devmgr` reaches PCI through ECAM
there (I/O port configuration is x86-only); `drvtest` skips its I/O port
check; `smptest`'s register test uses the FP registers of each
architecture.

### Packages and FoxDen

`rpg` knows `x86_64`, `aarch64` and `riscv64` (`ARCHS` in
`userspace/rust/rpg/src/control.rs`); a package is for one of them or
`all`. A repository has an index for each, so a computer of any
architecture finds its own (an empty one lists the packages for `all`).
A computer installs only its own architecture's packages. Apps are
built for x86-64 only for now: `scripts/build-outside-app.sh` and the
foxden-apps checks accept `Architecture: x86_64`; when a port runs, the
apps repository builds each app once per architecture.

### The kernel

The kernel calls the CPU through `crate::arch::cpu` (`src/arch/cpu.rs`):
interrupts on and off, waiting and halting, the fault address, the page
table root, the FPU save area, the user thread pointer and the debug
console. On x86-64 that is `src/arch/amd64/cpu.rs`; a port adds its own
file and one `pub use` line. `src/arch/arm64` and `src/arch/riscv64` have
the beginnings of the interrupt controllers (GIC, PLIC) and page tables.

## The rules

These are checked, so the work above stays done:

- **No assembly in the kernel outside `src/arch`.** `build.rs` counts
  `asm!` and `global_asm!` in every other file and fails the build on one
  more than `src/arch/NOT-YET-MOVED` allows. That list holds what is left
  (main.rs's stack switch and interrupt stubs, process/run.rs's user entry
  and context switch, the PS/2 driver); its numbers may only go down.
- **No assembly in the Rust programs**, and **every C program builds for
  aarch64 and riscv64** unless `userspace/X86-ONLY` lists it:
  `scripts/check-ports.sh` (in the regression run; `--quick` skips Rust).
- New CPU-specific code goes in `src/arch/amd64` (kernel) or
  `rustux/arch/x86_64.h` / `libc/src/arch/x86_64.c` (library), behind a
  portable name, with the other architectures' versions or an `#error`
  that names what is missing.

## What a port still has to do

In the order of the aarch64 step:

1. **Build the kernel for the target.** `cargo check --target
   aarch64-unknown-uefi` stops at the `x86_64` crate today (39 errors, all
   inside it): make it a dependency for x86-64 only and put `arch::amd64`
   and its users behind `#[cfg(target_arch = "x86_64")]`.
2. **Move what `NOT-YET-MOVED` lists** behind `arch::cpu` or into
   `src/arch/amd64`: the interrupt stubs and the context switch become one
   file per architecture.
3. **The boot path.** UEFI on both (`aarch64-unknown-uefi`, QEMU `virt`
   with EDK2), so the boot manager and the image layout stay the same;
   the device tree or ACPI for memory, the GIC/PLIC and the timer.
4. **Exceptions and system calls**: `svc`/`ecall` entry, saving the
   registers `docs/SYSCALL.md` promises to keep.
5. **Page tables, the timer, SMP** (PSCI on aarch64, SBI on riscv64).
6. **Userspace**: a linker script and `build.sh` for the target, the
   Rust programs built for it, and the system and apps repositories
   published for it. Drivers: virtio over MMIO or PCI (ECAM), PL011 or
   the SBI console.
7. **Tests**: the QEMU scripts run with `qemu-system-aarch64`.

`rx_arch_hw_random()` returns 0 on aarch64 and riscv64 until the port
reads the CPU's generator (`RNDR`, `Zkr`) or asks the kernel; `getrandom`
then mixes only the cycle counter and the clocks, which is not enough
for keys.

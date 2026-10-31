# Rustux

A capability-based microkernel written in Rust, booting directly from UEFI on x86_64.

Rustux is the kernel of **Rustica OS**. The full OS (live USB image, shell, apps, GUI plans) is in [gitrustux/rustica](https://github.com/gitrustux/rustica), which includes this repository as a submodule at `repo/kernel`.

## Status

**Phases 4–6 complete. Phase 7A in progress.**

| Area | State |
|------|-------|
| UEFI boot | Kernel is a standalone UEFI application (`BOOTX64.EFI`), no separate bootloader |
| Interrupts | GDT/IDT, Local APIC + IOAPIC (discovered via ACPI MADT), timer |
| Memory | Physical memory manager, kernel heap, per-process page tables |
| Processes | ELF loading, process table (256 slots), round-robin scheduler, `int 0x80` syscalls |
| Objects | Handles, channels, events, VMOs, timers, jobs (Zircon-style capabilities) |
| Filesystem | VFS + embedded ramdisk |
| Drivers | 16550 UART, PS/2 keyboard, framebuffer console |
| Other architectures | ARM64 (GIC) and RISC-V (PLIC) code present; build targets x86_64 only |

**Boot flow:**
```
UEFI Firmware → BOOTX64.EFI (rustux.efi) → Kernel → Init (PID 1) → Shell (PID 2)
```

### Known Issues

- **Userspace does not run yet.** The kernel boots through all subsystems in QEMU and enters ring 3, but user programs are linked at `0x400000`, which every process still maps through the firmware's identity mapping (copied into the lower half of each address space). `init` therefore faults on its first instruction. Fix: move the kernel out of the lower half.
- **No USB keyboard support.** Only PS/2 is implemented, so most modern hardware has no keyboard input. USB HID (xHCI) is the next driver; see [`docs/polling.txt`](docs/polling.txt).
- **Local APIC register map** in `src/arch/amd64/apic.rs` does not match the Intel layout (EOI is correct). See [`docs/README.md`](docs/README.md#known-issues).

## Building

### Prerequisites

```bash
rustup toolchain install nightly          # the kernel uses #![feature(abi_x86_interrupt)]
rustup target add x86_64-unknown-uefi
apt install dosfstools mtools qemu-system-x86 ovmf
apt install gcc-x86-64-linux-gnu          # for the C userspace programs
```

### Build

```bash
# Kernel + bootable disk image (rustux.img)
./build.sh

# Kernel only
cargo build --release --bin rustux --features uefi_kernel --target x86_64-unknown-uefi
# → target/x86_64-unknown-uefi/release/rustux.efi
```

The `uefi_kernel` feature is required. A plain `cargo build` builds only the library.

### Run in QEMU

```bash
./test-qemu.sh        # builds rustux.img, boots it, checks for timer ticks
cat /tmp/rustux-qemu-debug.log
```

### Live USB image

```bash
./build-live-image.sh
```

Writing and booting the image is covered in the [Rustica README](https://github.com/gitrustux/rustica#quick-start).

## Documentation

| File | Contents |
|------|----------|
| [`docs/README.md`](docs/README.md) | Repository layout, component status, file reference |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Kernel architecture |
| [`docs/OBJECTS.md`](docs/OBJECTS.md) | Kernel objects |
| [`docs/SYSCALL.md`](docs/SYSCALL.md) | System calls |
| [`docs/TESTS.md`](docs/TESTS.md) | Tests |
| [`docs/design/capability-system.md`](docs/design/capability-system.md) | Capability system design |
| [`PLAN.md`](PLAN.md) | Development plan |

## Related Repositories

- [gitrustux/rustica](https://github.com/gitrustux/rustica) – Rustica OS distribution
- [gitrustux/apps](https://github.com/gitrustux/apps) – userspace applications
- [gitrustux/loader](https://github.com/gitrustux/loader) – legacy UEFI kernel and USB/xHCI work

## License

MIT – see [LICENSE](LICENSE).

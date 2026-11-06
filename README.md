# Rustux

A capability-based microkernel written in Rust, booting directly from UEFI on x86_64.

Rustux is the kernel of **Rustica OS**. The full OS (live USB image, shell, apps, GUI plans) is in [gitrustux/rustica](https://github.com/gitrustux/rustica), which includes this repository as a submodule at `repo/kernel`.

## Status

**Boots to an interactive shell in QEMU and on the test laptop.** Checklist steps 1-13 are done (phase A: a usable command line; userspace PCI drivers; disk drivers: virtio-blk, NVMe read-only, USB storage; GPT partitions); step 14 (FAT12/16/32 read and write, `mount`, `mkfs.fat`, `fsck.fat`, the EFI partition at `/boot/efi`) passes on QEMU. See [`docs/ROADMAP.md`](docs/ROADMAP.md) and the checklist in the Rustux project.

| Area | State |
|------|-------|
| UEFI boot | Kernel is a standalone UEFI application (`BOOTX64.EFI`), no separate bootloader |
| Interrupts | GDT/IDT, Local APIC + IOAPIC (discovered via ACPI MADT), timer |
| Memory | Kernel-owned page tables; all physical memory mapped in the upper half (`0xffff_8000_0000_0000`); each process has a private lower half; kernel heap |
| Processes | ELF loading, process table (256 slots), kernel-stack switching on yield, exit and timer (user mode only), `int 0x80` syscalls; faults kill only the offending process |
| Objects | Handles, channels, events, VMOs, timers, jobs (Zircon-style capabilities) |
| Filesystem | VFS + embedded ramdisk |
| Drivers | 16550 UART, PS/2 keyboard, framebuffer console (ANSI colours, Dracula theme) |
| Other architectures | ARM64 (GIC) and RISC-V (PLIC) code present; build targets x86_64 only |

**Boot flow:**
```
UEFI Firmware → BOOTX64.EFI (rustux.efi) → Kernel → Init (PID 1) → Shell (PID 2)
```

### Known Issues

- **USB is keyboards only** (xHCI, USB 2.0 hubs, HID boot keyboards; polled). Root-port hot-plug works (KVM switching); a device unplugged from a hub's own port is not noticed, and a keyboard endpoint that halts is not recovered. See `src/drivers/usb/`.

- **No `wait`.** The shell starts a program and immediately shows its prompt, so program output can appear after the next prompt.
- **Boot services memory is not reclaimed.** The PMM uses only the free (`EfiConventionalMemory`) regions of the UEFI memory map; memory the firmware used during boot (`EfiBootServicesCode/Data`) stays unused.
- **Local APIC register struct** in `src/arch/amd64/apic.rs` does not match the Intel layout (the EOI and TPR writes the kernel uses are correct). See [`docs/README.md`](docs/README.md#known-issues).

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
| [`docs/USERSPACE.md`](docs/USERSPACE.md) | Userspace: C library, programs, shell |
| [`docs/FS.md`](docs/FS.md) | Filesystem service, protocol and boot image |
| [`docs/RUST.md`](docs/RUST.md) | Rust programs (std on Rustux) |
| [`docs/COMMANDS.md`](docs/COMMANDS.md) | The commands: where each comes from, differences from GNU |
| [`docs/DRIVERS.md`](docs/DRIVERS.md) | Drivers in userspace: devmgr, PCI, DMA, interrupts, block devices |
| [`docs/TESTS.md`](docs/TESTS.md) | Build, QEMU testing and debugging |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Architecture checklist |
| [`docs/HISTORY.md`](docs/HISTORY.md) | Project history and decisions |
| [`docs/usb-keyboard.md`](docs/usb-keyboard.md) | USB keyboard status and debugging history |
| [`docs/design/`](docs/design/) | Design documents: capabilities, scheduler and IPC, syscall ABI, HAL traits |

## Related Repositories

- [gitrustux/rustica](https://github.com/gitrustux/rustica) – Rustica OS distribution
- [gitrustux/apps](https://github.com/gitrustux/apps) – userspace applications
- [gitrustux/loader](https://github.com/gitrustux/loader) – legacy UEFI kernel and USB/xHCI work

## License

MIT – see [LICENSE](LICENSE).

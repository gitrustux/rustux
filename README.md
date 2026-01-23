# Rustux OS

A hobby operating system written in Rust, featuring a native UEFI kernel with an interactive shell and Dracula-themed interface.

## Current Status

**Phase 6 COMPLETE: Interactive Shell** 🟢

The system boots to a fully interactive command-line shell with:
- PS/2 keyboard input
- Framebuffer text console
- Process management with round-robin scheduler
- Embedded ramdisk filesystem
- Dracula theme (mandatory invariant)

**Boot Flow:**
```
UEFI Firmware → BOOTX64.EFI → Kernel → Init (PID 1) → Shell (PID 2)
```

## Quick Start

### Download Live Image

Visit https://rustux.com/rustica/ to download the latest live USB image.

### Write to USB

```bash
# Identify your USB device
lsblk

# Write the image (replace /dev/sdX with your device)
sudo dd if=rustica-live-amd64-0.1.0.img of=/dev/sdX bs=4M status=progress conv=fsync
sudo sync
```

### Boot

1. Insert USB and restart your computer
2. Enter boot menu (F12, F2, F10, Del, or Esc key)
3. Select the USB drive (look for "UEFI: USB...")
4. System boots directly to the Rustux shell

## Project Structure

This is a monorepo containing the complete Rustux OS:

```
/var/www/rustux.com/prod/
├── rustux/                 # Kernel (UEFI application)
│   ├── src/               # Kernel source code
│   ├── test-userspace/    # C programs (shell, init, tests)
│   ├── build.rs           # Embed ramdisk with userspace binaries
│   ├── build-live-image.sh# Live USB build script
│   └── README.md          # Kernel documentation
├── rustica/               # Userspace OS distribution
│   ├── docs/             # Documentation (BUILD.md, IMAGE.md, PLAN.md)
│   └── shell/            # Rust shell implementation (reference)
└── apps/                  # Userspace applications
    └── cli/              # Command-line tools
```

## What's Working (Phase 6)

| Component | Description | Status |
|-----------|-------------|--------|
| **Direct UEFI Boot** | No GRUB, no Linux kernel - standalone UEFI application | ✅ |
| **PS/2 Keyboard** | Scancode set 1 to ASCII conversion, modifier tracking | ✅ |
| **Framebuffer Console** | PSF2 font (8x16), scrolling, Dracula theme colors | ✅ |
| **Process Management** | Process table (256 slots), round-robin scheduler | ✅ |
| **Syscall Interface** | read, write, open, close, lseek, spawn, exit, getpid, getppid, yield | ✅ |
| **VFS + Ramdisk** | Virtual filesystem abstraction, embedded ELF binaries | ✅ |
| **Interactive Shell** | C shell with built-in commands, Dracula theme | ✅ |

### Shell Commands

```
rustux> help
rustux> clear
rustux> echo hello world
rustux> ps
rustux> hello
rustux> counter
rustux> exit
```

## Planned Features (Phase 7)

| Component | Description | Timeline |
|-----------|-------------|----------|
| **USB HID Driver** | Keyboard + mouse support via USB | Phase 7A |
| **Framebuffer Mapping** | Map framebuffer into userspace for direct drawing | Phase 7A |
| **GUI Server** | Single-process window manager (early Mac OS style) | Phase 7B |
| **GUI Client Library** | librustica_gui for building GUI applications | Phase 7C |

## System Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| **Architecture** | x86_64 (AMD64) | x86_64 (AMD64) |
| **Boot** | UEFI 2.0 | UEFI 2.3+ |
| **RAM** | 512 MB | 1 GB |
| **Storage** | 128 MB (USB) | 4 GB |
| **Input** | PS/2 Keyboard | PS/2 or USB HID* |

\* USB HID support planned for Phase 7

## Dracula Theme (MANDATORY INVARIANT)

The Dracula color palette is the default system theme and must survive:

- Kernel rebuilds
- CLI refactors
- Framebuffer rewrites
- GUI introduction later

**Canonical Dracula Colors:**
```
FG_DEFAULT = #F8F8F2  (r: 248, g: 248, b: 242)
BG_DEFAULT = #282A36  (r: 40, g: 42, b: 54)
CYAN       = #8BE9FD  (r: 139, g: 233, b: 253)
PURPLE     = #BD93F9  (r: 189, g: 147, b: 249)
GREEN      = #50FA7B  (r: 80, g: 250, b: 123)
RED        = #FF5555  (r: 255, g: 85, b: 85)
ORANGE     = #FFB86C  (r: 255, g: 184, b: 108)
YELLOW     = #F1FA8C  (r: 241, g: 250, b: 140)
```

## Documentation

- **[BUILD.md](rustica/docs/BUILD.md)** - Live USB build instructions
- **[IMAGE.md](rustica/docs/IMAGE.md)** - System architecture and boot flow
- **[PLAN.md](rustica/docs/PLAN.md)** - Development roadmap with detailed phase specs
- **[rustux/README.md](rustux/README.md)** - Kernel-specific documentation
- **[rustica/README.md](rustica/README.md)** - OS distribution documentation

## Contributing

See PLAN.md for:
- Coding standards
- Development workflow
- Phase specifications
- Technical decisions

## License

MIT License - See LICENSE file for details.

## Links

- **Repository:** https://github.com/gitrustux/rustux
- **Website:** https://rustux.com
- **Issue Tracker:** https://github.com/gitrustux/rustux/issues

---

**Last Updated:** January 23, 2025
**Status:** Phase 6 COMPLETE - Interactive shell running with Dracula theme

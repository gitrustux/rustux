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

### Build Live USB Image

```bash
cd /var/www/rustux.com/prod/rustux
./build-live-image.sh
```

Output: `/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img`

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

**GUI Architecture:**
```
┌─────────────────────────────────────┐
│           Application (Rust)          │
│         (uses librustica_gui)        │
├─────────────────────────────────────┤
│          GUI Server (rustica-gui)    │
│    (owns framebuffer, input events)   │
├─────────────────────────────────────┤
│              Rustux Kernel            │
│  (syscalls, scheduler, drivers)      │
├─────────────────────────────────────┤
│              UEFI Firmware            │
│         (BOOTX64.EFI)                 │
└─────────────────────────────────────┘
```

## Project Structure

```
/var/www/rustux.com/prod/
├── rustux/                 # Kernel (UEFI application)
│   ├── src/
│   │   ├── arch/amd64/    # Architecture-specific code (x86_64)
│   │   ├── drivers/       # Device drivers (keyboard, display)
│   │   ├── exec/          # ELF loading, process creation
│   │   ├── fs/            # VFS, ramdisk
│   │   ├── process/       # Process table, context switching
│   │   ├── sched/         # Round-robin scheduler
│   │   ├── syscall/       # System call handlers
│   │   └── main.rs        # Kernel entry point
│   ├── test-userspace/    # C programs (shell, init, hello, counter)
│   ├── build.rs           # Embed ramdisk with userspace binaries
│   ├── build-live-image.sh# Live USB build script
│   └── PLAN.md            # Development roadmap
└── rustica/                # Userspace OS distribution
    ├── docs/              # Documentation (IMAGE.md, PLAN.md, BUILD.md)
    └── shell/             # Rust shell implementation (reference)
```

## Build Requirements

### Prerequisites

```bash
# Rust toolchain (UEFI target)
rustup target add x86_64-unknown-uefi

# GCC for cross-compiling userspace C programs
apt install gcc-x86-64-linux-gnu

# Image creation tools
apt install parted dosfstools coreutils
```

### Build Commands

```bash
cd /var/www/rustux.com/prod/rustux

# Build kernel (UEFI application)
cargo build --release --target x86_64-unknown-uefi

# Build userspace C programs
cd test-userspace
x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector \
    shell.c -o shell.elf
x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector \
    init.c -o init.elf

# Build live USB image
./build-live-image.sh
```

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

## Development Roadmap

### Phase 4: Userspace & Process Execution ✅ COMPLETE
- ELF loading with segment mapping
- Per-process address spaces
- Page table isolation
- int 0x80 syscall interface
- First userspace instruction execution

### Phase 5: Process Management & Essential Syscalls ✅ COMPLETE
- Process table with 256 slots
- Round-robin scheduler with context switching
- Ramdisk for embedded files
- sys_spawn() for spawning from paths
- Init process (PID 1) auto-loads on boot

### Phase 6: Input, Display, Interactive Shell ✅ COMPLETE
- PS/2 keyboard driver (IRQ1, ports 0x60/0x64)
- Scancode to ASCII conversion with modifier tracking
- Framebuffer driver with PSF2 fonts
- Text console with scrolling
- Interactive C shell with Dracula theme

### Phase 7: Minimal GUI 🚧 PLANNED
- USB HID driver (keyboard + mouse)
- Framebuffer mapping syscall
- GUI server process (rustica-gui)
- GUI client library (librustica_gui)

## Documentation

- **BUILD.md** - Live USB build instructions
- **IMAGE.md** - System architecture and boot flow
- **PLAN.md** - Development roadmap with detailed phase specs

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
- **Documentation:** https://rustux.com
- **Issue Tracker:** https://github.com/gitrustux/rustux/issues

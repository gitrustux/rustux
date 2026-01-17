# TREE.md - Directory Structure of /var/www/rustux.com/prod

This document shows the directory tree structure of the production directory.

## Directory Structure (Top Level)

```
/var/www/rustux.com/prod/
├── .cache/                    # Cache directories (huggingface, chroma, etc.)
├── .claude/                   # Claude Code configuration
├── .config/                   # Configuration files
├── .config/psysh/              # Psysh PHP REPL config
├── .npm/                      # NPM cache
├── ERROR.md                   # Error documentation
├── FIXED.md                   # Fixed issues documentation
├── TODO.md                    # Todo items (deleted)
├── claude_code_prod_zai.sh    # Claude Code production script
├── append_adult_content.php    # PHP script
├── .bash_history
├── guacable.com/             # Guacable marketplace (separate project)
│   ├── backup/
│   ├── examine/
│   ├── html/                  # Laravel application
│   └── ...
├── liberanus.com/             # Liberanus project (separate project)
│   ├── backups/
│   ├── data/
│   ├── html/
│   ├── rag-app/
│   └── ...
├── resources/                 # Resources
│   └── views/
├── rustux.com/                # Rustux main website
│   └── html/
├── rustux.com/prod/           # THIS - Production directory
│   ├── kernel/                # RUSTUX kernel source
│   ├── html/                  # Web content (downloads, images)
│   └── rustica/               # Rustica OS documentation
├── project/                   # Project files
│   └── latin/
├── .stfolder
├── .npm/
└── .cache/
```

## Key Directory: kernel/

```
/var/www/rustux.com/prod/kernel/
├── build-live-image.sh        # Build script for bootable UEFI image
├── Cargo.toml                 # Workspace configuration
├── build.rs                   # Build script
├── uefi-loader/               # UEFI bootloader source
│   ├── Cargo.toml
│   ├── src/
│   └── target/
├── kernel-efi/                # UEFI kernel (main focus)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── framebuffer.rs    # VGA/text output
│   │   ├── keyboard.rs        # PS/2 keyboard driver + input buffer
│   │   ├── main.rs            # UEFI entry point
│   │   ├── mouse.rs           # PS/2 mouse driver
│   │   ├── native_console.rs  # Console abstraction
│   │   ├── runtime.rs         # ⚠️ KEY FILE - Interrupt handling (APIC fix)
│   │   ├── shell.rs           # Interactive shell
│   │   ├── syscall.rs         # System call handling
│   │   └── ...
│   └── target/
├── src/                       # Shared kernel library
│   ├── kernel/
│   │   ├── arch/
│   │   │   ├── amd64/          # x86_64 architecture
│   │   │   │   ├── apic.rs     # Local/IO APIC support
│   │   │   │   ├── interrupts.rs
│   │   │   │   └── ...
│   │   │   ├── arm64/          # ARM64 architecture
│   │   │   │   ├── interrupts.rs
│   │   │   │   └── ...
│   │   │   └── riscv64/        # RISC-V architecture
│   │   │       ├── plic.rs     # Platform-Level Interrupt Controller
│   │   │       └── ...
│   │   └── ...
│   └── ...
├── userspace/                 # Userspace tools
│   └── scripts/
│       └── build-all.sh
└── target/                    # Build artifacts
```

## Key Directory: rustux.com/html/

```
/var/www/rustux.com/prod/rustux.com/html/
├── rustica/                   # Rustica OS documentation
│   └── docs/
│       ├── documentation/
│       ├── architecture/
│       └── ...
└── rustica-live-amd64-0.1.0.img           # ⚠️ BOOTABLE UEFI IMAGE
└── rustica-live-amd64.img                       # Symlink
```

## Key File: kernel-efi/src/runtime.rs

This is the critical file for interrupt handling. Current state after APIC fix:

```rust
pub unsafe fn init_keyboard_interrupts() -> Result<(), &'static str> {
    // --- 1️⃣ Initialize IOAPIC ---
    const IOAPIC_BASE: u64 = 0xFEC0_0000;
    const IOAPIC_IOREGSEL: u64 = 0x00;
    const IOAPIC_IOWIN: u64 = 0x10;
    const IRQ1_REDIR_OFFSET: u32 = 0x12;

    let ioapic_sel = (IOAPIC_BASE + IOAPIC_IOREGSEL) as *mut u32;
    let ioapic_win = (IOAPIC_BASE + IOAPIC_IOWIN) as *mut u32;

    const IRQ1_VECTOR: u32 = 33;
    let low_dword = IRQ1_VECTOR;
    let high_dword = 0;

    ioapic_sel.write_volatile(IRQ1_REDIR_OFFSET);
    ioapic_win.write_volatile(low_dword);
    ioapic_sel.write_volatile(IRQ1_REDIR_OFFSET + 1);
    ioapic_win.write_volatile(high_dword);

    // --- 2️⃣ IDT entry for IRQ1 ---
    let kernel_cs: u16;
    core::arch::asm!("mov {0:x}, cs", out(reg) kernel_cs, ...);
    IDT[33] = IdtEntry::interrupt_gate(keyboard_irq_stub as *const () as u64, kernel_cs);

    // --- 3️⃣ Initialize keyboard driver ---
    crate::keyboard::init();

    Ok(())
}
```

**Current interrupt path:**
```
Keyboard → IOAPIC (0xFEC00000) → Local APIC (UEFI enabled) → CPU → IDT[33] → handler
```

## Architecture-Specific Interrupt Controllers

| Architecture | Controller | Location | Status |
|--------------|------------|----------|--------|
| **x86_64 (amd64)** | Local APIC + IOAPIC | `src/kernel/arch/amd64/apic.rs` | ✅ Implemented |
| **ARM64** | GIC (Generic Interrupt Controller) | `src/kernel/arch/arm64/interrupts.rs` | ✅ Stub exists |
| **RISC-V** | PLIC (Platform-Level Interrupt Controller) | `src/kernel/arch/riscv64/plic.rs` | ✅ Implemented |

## Build Artifacts

```
target/
├── x86_64-unknown-uefi/
│   └── release/
│       ├── rustux-kernel-efi.efi    # UEFI kernel binary
│       └── ...
└── ...
```

## Bootable Image

```
rustica-live-amd64-0.1.0.img    (128MB UEFI bootable disk image)
├── EFI/
│   ├── BOOT/
│   │   └── BOOTX64.EFI          # UEFI bootloader
│   └── Rustux/
│       └── kernel.efi          # RUSTUX kernel
```

## Key Files for IRQ1 Debugging

| File | Purpose |
|------|---------|
| `kernel-efi/src/runtime.rs` | IOAPIC init, LAPIC EOI, IDT setup |
| `kernel-efi/src/keyboard.rs` | Input buffer, IRQ handler, polling fallback |
| `kernel-efi/src/main.rs` | UEFI entry point, sti, init ordering |
| `kernel-efi/src/framebuffer.rs` | VGA output, diagnostic markers |

## Diagnostic Markers

When keyboard IRQ works, you should see on VGA:
- **Column 1**: Red `!` - IRQ1 entered CPU (first instruction of handler)
- **Column 58**: Yellow `IOAPIC!` - IOAPIC initialized
- **Column 79**: White `K` - IRQ handler executed (incrementing counter)

## Generated Files

- `build-live-image.sh` - Creates UEFI bootable disk image
- `FIXED.md` - Documents the APIC fix
- `rustica-live-amd64-0.1.0.img` - Bootable UEFI image with kernel

---
*Generated: 2025-01-17*

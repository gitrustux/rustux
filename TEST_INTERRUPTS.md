# Rustux Kernel - Interrupt System Test Guide

This document explains how to test the migrated boot infrastructure (GDT, IDT, APIC, Timer) using QEMU.

## Overview

The interrupt system test validates that the following components work correctly:

1. **GDT** (Global Descriptor Table) - Memory segmentation and privilege levels
2. **IDT** (Interrupt Descriptor Table) - Interrupt handler routing
3. **APIC** (Advanced Programmable Interrupt Controller) - Local APIC + I/O APIC
4. **Timer Interrupt** - Periodic LAPIC timer for kernel scheduling

## Quick Start

### Prerequisites

Install QEMU:

```bash
# Ubuntu/Debian
sudo apt install qemu-system-x86

# Fedora
sudo dnf install qemu-system-x86

# macOS
brew install qemu
```

### Running the Test

#### Option 1: Using the test script (Recommended)

```bash
cd /var/www/rustux.com/prod/rustux
chmod +x test-qemu.sh
./test-qemu.sh
```

#### Option 2: Manual QEMU invocation

```bash
# Build the kernel
cargo build --release --tests

# Run in QEMU with debug console
qemu-system-x86_64 \
    -kernel target/x86_64-unknown-none/release/librustux.a \
    -nographic \
    -device isa-debugcon,iobase=0xE9 \
    -serial mon:stdio \
    -m 512M \
    -smp 1
```

### Expected Output

```
╔══════════════════════════════════════════════════════════╗
║           RUSTUX KERNEL - INTERRUPT TEST                 ║
║           Testing Migrated Boot Infrastructure           ║
╚══════════════════════════════════════════════════════════╝

=== Rustux Interrupt System Test ===
Testing migrated boot infrastructure

[1/5] Setting up GDT... OK
[2/5] Setting up IDT... OK
[3/5] Installing timer handler... OK
[4/5] Initializing APIC... OK
[5/5] Configuring timer... OK

Interrupt system configured!
Enabling interrupts and waiting for timer ticks...

Waiting for timer interrupts (100 ticks max)...
[TICK 1]
[TICK 2]

=== TEST PASSED ===
Received 50 timer ticks successfully!

Test complete. Halting CPU.
```

## Understanding the Test

### What the Test Does

1. **GDT Setup** (`src/arch/amd64/descriptor.rs:gdt_setup`)
   - Creates kernel and user code/data segments
   - Sets up Task State Segment (TSS)
   - Loads GDT via `lgdt` instruction

2. **IDT Setup** (`src/arch/amd64/descriptor.rs:idt_setup_readonly`)
   - Creates 256 interrupt gate entries
   - Loads IDT via `lidt` instruction
   - Installs exception handlers (divide by zero, page fault, etc.)

3. **Timer Handler Installation** (`src/arch/amd64/test.rs:timer_handler`)
   - Sets IDT entry 32 to point to `timer_handler`
   - Configured as interrupt gate (DPL=0)

4. **APIC Initialization** (`src/arch/amd64/apic.rs:apic_local_init`)
   - Enables Local APIC via Spurious Interrupt Vector Register
   - Uses default MMIO base address: `0xFEE00000`

5. **Timer Configuration** (`src/arch/amd64/test.rs:configure_lapic_timer`)
   - Sets timer divide to 1
   - Configures periodic mode (bit 16)
   - Sets interrupt vector to 32
   - Sets initial count to 1,000,000

### Why QEMU Debug Console?

The test uses QEMU's debug console (port 0xE9) instead of a full VGA/serial console because:

1. **Simplicity** - No need for framebuffer or complex console driver
2. **Early Boot** - Works immediately after kernel entry
3. **Debugging** - Output appears in QEMU's terminal window

### Interrupt Flow

```
1. Timer fires → LAPIC asserts IRQ 0
2. CPU reads vector from LAPIC (32)
3. CPU pushes interrupt frame to stack
4. CPU jumps to IDT[32] → timer_handler
5. timer_handler increments TICK counter
6. timer_handler sends EOI to LAPIC (port 0xE0)
7. timer_handler executes `iretq` → return to halted state
```

## Troubleshooting

### No Output

**Symptoms**: QEMU starts but nothing is printed

**Solutions**:
- Verify QEMU debug console is enabled: `-device isa-debugcon,iobase=0xE9`
- Check that the test is being built: `cargo build --release --tests`
- Ensure `test_entry` module is included in lib.rs

### "No Timer Ticks"

**Symptoms**: Test prints "Waiting for timer interrupts..." but no `[TICK]` messages

**Possible Causes**:
1. APIC not enabled (check APIC base address)
2. Timer configured incorrectly (wrong divide or initial count)
3. Interrupts not enabled (check `sti` instruction)
4. Running on real hardware without APIC

**Debugging**:
- Add `qemu_print` statements in `timer_handler`
- Verify LAPIC MMIO base address (`0xFEE00000`)
- Check that SVR register has bit 8 set (APIC enable)

### Build Errors

**Symptoms**: `cargo build` fails with undefined references

**Solutions**:
- Ensure all modules are declared in mod.rs files
- Check that `test_entry` is compiled with `#[cfg(test)]`
- Verify that `idt_set_gate` is exported from idt.rs

### Triple Fault

**Symptoms**: QEMU exits immediately with "qemu: terminating on signal 15"

**Causes**:
1. IDT entry points to invalid address
2. Stack pointer is invalid
3. GDT is not set up correctly

**Debugging**:
- Use QEMU monitor: `-monitor stdio`
- Add `info registers` command to QEMU monitor
- Check GDT/IDT setup with GDB

## QEMU Commands Reference

### Basic QEMU invocation

```bash
# No GUI (text mode)
qemu-system-x86_64 -nographic -kernel kernel.elf

# With GUI (VGA)
qemu-system-x86_64 -kernel kernel.elf

# With debug console
qemu-system-x86_64 -nographic -device isa-debugcon,iobase=0xE9 -kernel kernel.elf
```

### Debugging with GDB

```bash
# Terminal 1: Start QEMU with GDB server
qemu-system-x86_64 -nographic -kernel kernel.elf -s -S

# Terminal 2: Connect GDB
gdb kernel.elf
(gdb) target remote :1234
(gdb) break timer_handler
(gdb) continue
```

### QEMU Monitor Commands

Press `Ctrl+A, C` to open QEMU monitor (when using `-nographic`):

```
(qemu) info registers    # Show CPU registers
(qemu) x/10i $pc        # Disassemble instructions
(qemu) xp/512x 0xFEE00000  # Display LAPIC registers
(qemu) quit              # Exit QEMU
```

## Next Steps After Testing

If the test passes, the interrupt system is working correctly. You can now:

1. **Migrate Phase 2 Components**:
   - ACPI/MADT parsing (discover APIC base dynamically)
   - Shell/CLI (polling → interrupt-driven input)
   - Memory Manager (PMM/VMM integration)

2. **Add More Interrupt Handlers**:
   - Keyboard (IRQ1, vector 33)
   - Serial port (IRQ4, vector 36)
   - Spurious interrupt (vector 255)

3. **Implement Scheduler**:
   - Use timer ticks for thread preemption
   - Add thread switch in timer_handler

4. **Fix Shell Polling Issue**:
   - Replace `syscall::read_line()` with interrupt-driven input
   - Use keyboard handler to buffer input
   - Wake shell when Enter is pressed

## Files Created for Testing

| File | Purpose |
|------|---------|
| `src/arch/amd64/test.rs` | Interrupt test functions and handlers |
| `src/test_entry.rs` | Kernel entry point for testing |
| `src/arch/amd64/idt.rs` | Added `idt_set_gate` function |
| `test-qemu.sh` | Build and test script |
| `TEST_INTERRUPTS.md` | This document |

## Exit QEMU

Press `Ctrl+A, X` to exit QEMU (when using `-nographic` mode).

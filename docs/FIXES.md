# Rustux Kernel Fixes Summary

## Date: 2025-01-16

## Problem Statement
The UEFI kernel booted successfully and reached the shell prompt, but keyboard input never worked. The shell would hang at `rustux>` prompt indefinitely.

## Root Causes Identified

### 1. **Missing I/O Delays in PIC Initialization** (CRITICAL)
The 8259 Programmable Interrupt Controller (PIC) requires I/O delays between writes. Without delays, the PIC cannot properly process Initialization Control Words (ICWs), resulting in incorrect or incomplete IRQ routing.

**Fix Applied:** Added `io_delay()` function using `outb(0x80, 0)` - the canonical, architecturally serializing I/O delay.

**Why port 0x80:** Port 0x80 is the legacy "debug" port used for POST codes. Writing to it guarantees I/O serialization on all x86 CPUs. This is the industry-standard PIC delay mechanism.

**Incorrect approaches:**
- ❌ NOP loops - not reliable on modern CPUs
- ❌ `pause` instruction - weaker serialization
- ✅ `outb(0x80, 0)` - correct, architecturally serializing

**Location:** `/var/www/rustux.com/prod/kernel/kernel-efi/src/runtime.rs`

**Note:** VGA visual confirmation was moved outside of `init_pic()` to `init_keyboard_interrupts()` for better separation of concerns. The `init_pic()` function is now a pure hardware initialization function with no side effects.

### 2. **Mouse Driver Conflict**
The PS/2 mouse and keyboard share the same hardware controller (data port 0x60). Having both drivers initialized caused:
- Mouse IRQ12 consuming keyboard scan codes
- Corrupted input buffer
- Shell blocking forever on invalid data

**Fix Applied:** Completely removed mouse driver from kernel-efi
- Removed `mod mouse;` from main.rs
- Removed `mouse_irq_stub()` from runtime.rs
- Removed `init_mouse_interrupts()` from runtime.rs
- Removed `cmd_mouse` from shell.rs
- Set PIC2 mask to 0xFF (all IRQ2 IRQs disabled)

**Lesson:** **Kernel Rule #1: One device at a time.** Get keyboard working before adding mouse.

### 3. **No Visibility Into Interrupt State**
No way to verify if IRQ1 was actually firing or if the PIC was configured correctly.

**Fix Applied:** Added comprehensive debug infrastructure:
- Visual VGA counter (top-right corner) that increments on every IRQ1
- Debug commands: `irq`, `kbd`, `pic`
- PIC register read functions: `pic_get_masks()`, `pic_get_irr()`, `pic_get_isr()`
- Non-blocking keyboard API: `try_read_char_direct()`, `read_line_timeout()`

## PIC Configuration Reference

### Correct PIC Remap Sequence

```
Port          | Write | Purpose
--------------|-------|--------------------------------------------------
0x20, 0xA0    | 0x11  | ICW1: Initialize PIC, requires ICW4
0x80          | 0x00  | I/O delay (architecturally serializing)
0x21          | 0x20  | ICW2: PIC1 vector offset → IRQ0-7 = vectors 32-39
0xA1          | 0x28  | ICW2: PIC2 vector offset → IRQ8-15 = vectors 40-47
0x80          | 0x00  | I/O delay
0x21          | 0x04  | ICW3: PIC2 at IRQ2 on PIC1 (cascade)
0xA1          | 0x02  | ICW3: Cascade identity
0x80          | 0x00  | I/O delay
0x21, 0xA1    | 0x01  | ICW4: 8086 mode
0x80          | 0x00  | I/O delay
0x21          | 0xF9  | IRQ mask: IRQ1 ENABLED, IRQ2 ENABLED, others disabled
0xA1          | 0xFF  | IRQ mask: all disabled
0x80          | 0x00  | I/O delay
```

### IRQ Masks Explained

```
PIC1 mask 0xF9 = 11111001 (binary)
  bit 0 (IRQ0 timer)   = 1 → DISABLED
  bit 1 (IRQ1 keyboard) = 0 → ENABLED ✓
  bit 2 (IRQ2 cascade)  = 0 → ENABLED (required for PIC2)
  bits 3-7             = 1 → DISABLED

PIC2 mask 0xFF = 11111111 (binary)
  all IRQs DISABLED (no mouse/other devices)
```

### Vector Mapping After Remap

```
IRQ  | Vector | Handler
-----|--------|--------------------------
0    | 32     | Timer (disabled)
1    | 33     | Keyboard ✓ ENABLED
2    | 34     | Cascade (PIC2)
8-15 | 40-47  | PIC2 IRQs (all disabled)
```

## Critical Initialization Order

```
1. init_exception_handlers()
   - Sets up IDT entries 0-31 (CPU exceptions)
   - Executes lidt instruction

2. init_keyboard_interrupts()
   - Calls init_pic() to remap PIC
   - Sets up IDT[33] for IRQ1 (keyboard)
   - Executes lidt again
   - Calls keyboard::init()

3. sti (Enable Interrupts)
   - CPU begins delivering interrupts

4. run_shell()
   - Enters command loop
```

**CRITICAL:** `sti` MUST be after:
- GDT loaded (UEFI provides this)
- IDT loaded (lidt executed)
- PIC remapped and IRQ1 unmasked

## Debug Commands Added

| Command | Purpose |
|---------|---------|
| `help`  | List all commands |
| `irq`   | Show IRQ count, test keyboard hardware directly |
| `kbd`   | 5-second keyboard test (IRQ vs polling) |
| `pic`   | Show PIC masks, IRR, ISR registers |
| `info`  | Show system information |
| `mem`   | Show memory/allocator status |
| `clear` | Clear screen |

## VGA Debug Markers

| Position | Marker | Indicates |
|----------|--------|-----------|
| Column 40 | "IDT OK!" | IDT successfully loaded |
| Column 60 | "PIC!" | PIC successfully remapped |
| Column 79 | 0-F | IRQ1 counter (increments on each keypress) |

## Key Lessons Learned

### 1. Hardware Timing Matters
The 8259 PIC is ancient hardware (1980s) and requires explicit delays between I/O operations. Modern CPUs execute billions of instructions per second - without delays, the PIC cannot keep up.

### 2. Shared Hardware Resources
PS/2 keyboard and mouse share the same I/O ports (0x60/0x64). Both drivers reading from these ports will corrupt each other's data.

### 3. Debug Visibility is Essential
Cannot debug interrupt-driven code without visibility into:
- Whether interrupts are firing at all
- What the PIC state actually is
- What hardware is reporting

### 4. One Device at a Time
Initialize and test devices independently. Adding multiple complex drivers simultaneously makes debugging nearly impossible.

### 5. Visual Confirmation Works
Simple VGA text markers are more reliable than complex logging for early kernel debugging.

## Files Modified

| File | Changes |
|------|---------|
| `kernel-efi/src/main.rs` | Removed `mod mouse;` |
| `kernel-efi/src/runtime.rs` | Added I/O delays to `init_pic()`, added PIC debug functions, removed mouse IRQ stub |
| `kernel-efi/src/keyboard.rs` | Added IRQ1 visual counter, added `try_read_char_direct()`, `get_irq_count()`, `read_line_timeout()` |
| `kernel-efi/src/shell.rs` | Removed mouse command, added `irq`, `kbd`, `pic` debug commands |

## Testing Checklist

When booting the kernel, verify in order:

1. ✓ "IDT OK!" appears on VGA (column 40)
2. ✓ "PIC!" appears on VGA (column 60)
3. ✓ "Interrupts enabled" message appears
4. ✓ "Starting shell..." message appears
5. ✓ "> rustux>" prompt appears
6. ✓ Type a key → counter increments in top-right (column 79)
7. ✓ Type `pic` → shows "IRQ1 enabled"
8. ✓ Type `irq` → shows IRQ count > 0
9. ✓ Type `kbd` and press key → shows "SUCCESS - IRQ driver working!"
10. ✓ Type `help` → command list appears

## References

- 8259 PIC Datasheet: Intel 8259A Programmable Interrupt Controller
- PS/2 Keyboard/Mouse Controller: IBM PS/2 Hardware Technical Reference
- x86_64 Interrupt Descriptor Table: AMD64 Architecture Programmer's Manual Volume 2

# IRQ1 Fix - UEFI x86_64 Interrupt Delivery

## Problem
Under UEFI on real hardware, IRQ1 (keyboard interrupt) was never firing. The kernel was stuck in `[POLLING]` mode because:
1. Local APIC was disabled by runtime.rs:543 (`apic_svr.write_volatile(0xFF)`)
2. IOAPIC was never initialized
3. IDT was never reloaded after IOAPIC configuration

## Solution
Implemented proper UEFI interrupt routing:

1. **Disable LAPIC Disable** - Removed the line that was disabling LAPIC
2. **Initialize IOAPIC** - Configure IOAPIC at 0xFEC0_0000 for IRQ1 → Vector 33
3. **Enable Local APIC** - Let UEFI firmware handle LAPIC initialization
4. **Send EOI to Local APIC** - Changed from PIC to Local APIC for EOI

## Interrupt Path (Fixed)
```
Keyboard → IOAPIC (0xFEC00000) → Local APIC (UEFI enabled) → CPU → IDT[33] → Handler
```

## Files Modified
- `kernel-efi/src/runtime.rs` - IOAPIC initialization, LAPIC EOI, IDT setup
- `kernel-efi/src/main.rs` - sti instruction with proper asm options
- `kernel-efi/src/keyboard.rs` - IRQ1 handler, input buffer, polling fallback

## Diagnostic Markers
- **VGA column 1**: Red `!` - IRQ1 entered CPU (first instruction of handler)
- **VGA column 58**: Yellow "IOAPIC!" - IOAPIC initialized
- **VGA column 79**: White `K` - IRQ handler execution counter

## Expected Behavior After Fix
- ✅ Red `!` marker appears
- ✅ `K` counter increments on each keypress
- ✅ `[POLLING]` disappears
- ✅ Keyboard input works via interrupts

## Status
**Still investigating** - IRQ1 not firing on real hardware even with fix applied.
Next step: Investigate IOAPIC memory-mapped writes and ensure proper MMIO setup.

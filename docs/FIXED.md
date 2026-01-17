# FIXED.md - Kernel Bug Fixes and Solutions

## UEFI x86_64 Keyboard IRQ1 Not Firing (APIC Routing Issue)

### Problem Description
Under UEFI, the keyboard was stuck in `[POLLING]` mode because IRQ1 interrupts never reached the CPU. The kernel was attempting to use legacy 8259 PIC interrupt routing, but UEFI firmware routes all hardware interrupts through IOAPIC → Local APIC instead.

### Root Cause
1. **Local APIC was disabled** - The code wrote `0xFF` to the LAPIC Spurious Vector Register (SVR), clearing bit 8 (APIC enable)
2. **IOAPIC was never initialized** - No code configured the IOAPIC to route IRQ1 to a LAPIC vector
3. **PIC-only approach incompatible with UEFI** - Modern UEFI firmware (OVMF, etc.) ignores legacy PIC for hardware IRQ routing

### Solution
Implemented proper UEFI APIC interrupt routing in `kernel-efi/src/runtime.rs`:

#### 1. Enable Local APIC
```rust
// File: kernel-efi/src/runtime.rs
const LOCAL_APIC_BASE: u64 = 0xFEE0_0000;
const LAPIC_SVR_OFFSET: usize = 0x70; // Spurious Vector Register
const LAPIC_TPR_OFFSET: usize = 0x30; // Task Priority Register

let lapic_svr = (LOCAL_APIC_BASE + LAPIC_SVR_OFFSET as u64) as *mut u32;
let lapic_tpr = (LOCAL_APIC_BASE + LAPIC_TPR_OFFSET as u64) as *mut u32;

// Enable Local APIC (set bit 8) and set spurious vector to 0xFF
lapic_svr.write_volatile(0x100 | 0xFF);
// Allow all interrupts (TPR = 0)
lapic_tpr.write_volatile(0);
```

#### 2. Initialize IOAPIC for IRQ1
```rust
// File: kernel-efi/src/runtime.rs
const IOAPIC_BASE: u64 = 0xFEC0_0000;
const IOAPIC_IOREGSEL: u64 = 0x00;
const IOAPIC_IOWIN: u64 = 0x10;
const IRQ1_REDIR_OFFSET: u32 = 0x12; // Redirection table for IRQ1 (low dword)

let ioapic_sel = (IOAPIC_BASE + IOAPIC_IOREGSEL) as *mut u32;
let ioapic_win = (IOAPIC_BASE + IOAPIC_IOWIN) as *mut u32;

const IRQ1_VECTOR: u32 = 33; // Vector assigned to keyboard IRQ1
let low_dword = IRQ1_VECTOR; // Edge-triggered, active-high, not masked
let high_dword = 0; // Destination CPU 0 (BSP)

// Write low dword of IRQ1 redirection entry
ioapic_sel.write_volatile(IRQ1_REDIR_OFFSET);
ioapic_win.write_volatile(low_dword);
// Write high dword of IRQ1 redirection entry
ioapic_sel.write_volatile(IRQ1_REDIR_OFFSET + 1);
ioapic_win.write_volatile(high_dword);
```

#### 3. Update IRQ Handler EOI
Changed from PIC EOI to Local APIC EOI in `keyboard_irq_stub()`:
```asm
// OLD (PIC EOI - wrong for UEFI):
mov al, 0x20
out 0x20, al

// NEW (Local APIC EOI):
push rax
mov eax, 0xFEE00040  ; Local APIC EOI register
mov dword ptr [rax], 0
pop rax
```

### Interrupt Path (Corrected)
```
Keyboard → IOAPIC (IRQ1 → Vector 33) → Local APIC → CPU → IDT[33] → keyboard_irq_stub
           0xFEC00000                    0xFEE00000
```

### Files Modified
- `kernel-efi/src/runtime.rs`:
  - `init_keyboard_interrupts()` - Completely rewritten for APIC mode
  - `keyboard_irq_stub()` - Changed EOI from PIC to Local APIC

### Expected Behavior After Fix
- ✅ Red `!` marker appears at VGA column 1 (IRQ1 entry proof)
- ✅ White `K` counter appears/increments at VGA column 79 (handler execution proof)
- ✅ `[POLLING]` disappears
- ✅ Keyboard input works via interrupts (no polling)
- ✅ VGA shows "IOAPIC!" at column 58 (initialization confirmation)

### Architecture-Specific Notes

#### ARM64 (aarch64)
ARM64 uses the GIC (Generic Interrupt Controller) instead of APIC:
- **GICv2/v3**: Different register layout and initialization
- **Interrupt routing**: Uses ITS (Interrupt Translation Service) for MSI
- **Vector numbering**: Different from x86 (typically SGI/PPI/SPI ranges)
- **Implementation needed**: Similar approach but with GIC registers

#### RISC-V
RISC-V uses the AIA (Advanced Interrupt Architecture) with CLINT:
- **PLIC**: Platform-Level Interrupt Controller (external interrupts)
- **CLINT**: Core-Local Interrupt Controller (timer/software)
- **Interrupt routing**: Different priority and enable mechanisms
- **Implementation needed**: PLIC configuration for keyboard IRQ

### Verification
To verify the fix works:
1. Boot the UEFI kernel image
2. Press any key
3. Observe:
   - Red `!` at VGA column 1 appears
   - `K` counter at column 79 increments
   - `[POLLING]` does NOT appear
   - Keyboard input works immediately

### Date Fixed
2025-01-17

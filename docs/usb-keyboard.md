# USB Keyboard Support: Status and Debugging History

This document collects everything learned while trying to get keyboard input working on real hardware. It merges `docs/polling.txt` (August 2026 summary) and `html/rustica/DEBUG.md` (May 2026 debugging log, previously published on the website).

## Current status (2026-10-31)

- The kernel only has a PS/2 (8042) keyboard driver. Most modern laptops and desktops have no working 8042 (or only firmware emulation of one), so USB HID is required.
- The USB/xHCI driver lives in the **legacy** `kernel-efi` (repository `gitrustux/loader`, branch `wip/xhci-2026-08`), not in this kernel yet. It must be ported to `src/drivers/` once it works.
- Branch `wip/xhci-2026-08` (commit `3b4af59`) fixes the problems found in the October review of a Comet Lake laptop log:
  - the PCI scan only checked devices 0–7 on bus 0, so the chipset xHCI at `00:14.0` was never found and the Thunderbolt controller was used instead;
  - PORTSC was read with an off-by-one port index and only ports 1–4 were checked;
  - USBSTS, interrupter (event ring) and PORTSC bit positions were wrong;
  - no BIOS/OS handoff, no scratchpad buffers, reset without halting first, CRCR written after Run;
  - producer rings had no Link TRB (the keyboard endpoint stopped after ~15 reports);
  - enumeration assumed the first connected device was a keyboard.
- What to look for on screen with that build: `PCI: SELECTED 00:14.0 8086:…`, `xHCI: BIOS handoff... released by BIOS`, `Reset OK`, one line per connected port, then `boot keyboard: interface …` and `keyboard ready on port N`.

## August 2026 summary (from `polling.txt`)

### 1. Original Symptom
Rustica successfully booted to its CLI, but keyboard input did not work.
The kernel displayed:

    [POLLING]

The system appeared to be polling for keyboard input rather than receiving
keyboard interrupts.

The initial investigation focused heavily on the IRQ1 -> IOAPIC -> Local APIC
interrupt path.

### 2. Initial Interrupt / APIC Investigation
Several interrupt-routing changes were made while investigating the problem:

- ACPI MADT parsing was added to determine whether legacy IRQ1 was remapped
  to another GSI.
- IRQ1 defaults to GSI 1 when no ACPI Interrupt Source Override exists.
- The kernel displayed:
      IRQ1 Override: None (default GSI=1)

- IOAPIC redirection for IRQ1 was configured.
- The Local APIC destination was changed to the actual BSP APIC ID rather than
  assuming APIC ID 0.
- The IRQ vector was tested using vector 0x41.
- The keyboard IDT handler was installed at both 0x21 and 0x41 as a diagnostic.
- IOAPIC masking was checked.
- The Local APIC TPR was set to zero.
- The Local APIC SVR was enabled.
- The EOI register was corrected from 0x40 to the correct 0xB0 offset.
- x2APIC detection and disabling was added using IA32_APIC_BASE MSR.
- The kernel verified the APIC base address.
- Diagnostic pixels and messages were added to confirm APIC/IOAPIC state.
- A software interrupt / halt-style diagnostic was considered to prove that
  the IDT/interrupt path itself could execute independently of keyboard input.

These changes did not make physical keyboard input work.

### 3. Important Discovery: the Keyboard Was USB
The critical discovery was that testing had been performed with:

- A USB keyboard.
- Previously, a laptop keyboard.

The kernel's existing keyboard driver was a PS/2 (8042) driver.

Therefore, the kernel was waiting for PS/2 IRQ1/input even though the actual
keyboard was USB.

This explains why extensive IRQ1/APIC debugging did not produce keyboard
input: there was no PS/2 keyboard generating IRQ1.

### 4. PS/2 Controller Test
The kernel checked the traditional PS/2 controller ports:

    0x64  — 8042 status
    0x60  — keyboard data

The diagnostic result on the tested environment was effectively:

    8042 status: 0xFF
    8042 self-test response: 0xFF

This indicates that an 8042/PS/2 controller is not available/responding in
the environment being tested.

The conclusion was that the PS/2 path is not useful for the target hardware.

### 5. UEFI Input Experiment
A UEFI SimpleTextInput fallback was added/tested.

The kernel could detect the saved UEFI input protocol and displayed:

    [UEFI]

However, after ExitBootServices, UEFI Boot Services console input cannot
normally be relied upon. The kernel did not receive usable keyboard input
through this mechanism.

The [UEFI] message therefore did NOT prove that keyboard input was working.
It only showed that the UEFI input protocol pointer had been detected.

### 6. QEMU Testing
QEMU was used to test the kernel.

The goal was to provide PS/2 emulation and determine whether the existing
PS/2 interrupt implementation worked in a controlled environment.

QEMU configurations involving the PC machine type and attempted i8042
configuration were tried/discussed.

The result was still not a successful keyboard-input test, and the diagnostic
output continued to indicate that the PS/2 controller was unavailable or that
the kernel was not reaching the expected working input path.

A later recommendation was to use the standard QEMU "pc" machine rather than
overcomplicating the machine configuration, since the PC machine normally
provides legacy PS/2 devices.

### 7. [NO KEYBOARD] Diagnostic
A stronger diagnostic was then introduced:

- Check whether a PS/2 controller is actually present.
- If present, initialize the PS/2 path and display [PS/2].
- If absent, display [NO KEYBOARD] and halt.

This was intended to prevent the kernel from silently falling into an
unproductive polling loop when no PS/2 device exists.

The source was updated so the keyboard initialization checks
controller_present() before continuing.

### 8. USB Keyboard Direction
The correct long-term solution for modern physical hardware is USB HID
keyboard support.

The planned architecture is:

    PCI discovery
        |
        v
    USB host controller
        |
        +--> xHCI (primary target for modern hardware)
        |
        v
    USB HID keyboard
        |
        v
    HID Boot Protocol reports
        |
        v
    Keyboard event/scancode translation
        |
        v
    Rustica shell / CLI

The first implementation should be intentionally minimal:

- Detect xHCI via PCI.
- Map the xHCI MMIO BAR.
- Initialize the controller.
- Detect the USB root hub/port.
- Enumerate the keyboard.
- Configure the USB HID keyboard interface.
- Use HID Boot Protocol.
- Receive the standard keyboard reports.
- Translate HID usage codes into Rustica key events.
- Initially polling the USB controller is acceptable as a diagnostic.
- USB interrupts can be added after basic input works.

This is preferable to continuing to debug PS/2 IRQ1 on USB-only hardware.

### 9. Source Tree / Build Confusion

> **Resolved (2026-10-31).** `gitrustux/rustux` (the `prod/` folder on the server) is now the only kernel source. The copy in `loader/rustux` was merged and removed; `rustica` includes the kernel as a submodule at `repo/kernel`.
There was also a major source-tree problem during the debugging process.

Relevant directories included:

    /var/www/rustux.com/prod/kernel-efi
    /var/www/rustux.com/prod/loader/kernel-efi
    /var/www/rustux.com/prod/loader/rustux
    /var/www/rustux.com/prod/uefi-loader

Some of these contained overlapping or detached copies of kernel/UEFI code.

At one point the detached copy was mistakenly considered a duplicate and
deleted. It was later determined that the /loader tree was part of the
working/canonical build path.

The current intended canonical source is associated with:

    /var/www/rustux.com/prod/loader/rustux

Do NOT blindly copy an entire kernel-efi directory from one tree to another.
Changes need to be reconciled file-by-file against the canonical source.

### 10. APIC Changes That Should Be Preserved
The following APIC-related fixes were considered valid and should not be
discarded merely because the keyboard turned out to be USB:

- Correct Local APIC EOI offset:
      0xB0

- IA32_APIC_BASE MSR handling.

- x2APIC detection and controlled transition to xAPIC when MMIO access is
  required.

- APIC global enable verification.

- Reading the actual BSP Local APIC ID.

- Routing IOAPIC interrupts to the actual BSP APIC ID.

- ACPI MADT parsing for IRQ1 Interrupt Source Overrides.

These are useful infrastructure for future interrupt-driven devices.

### 11. Build Regression / "EFI OK" Problem

> **Solved (2026-10-31).** The tiny EFI binary was not a linking problem. The entry path called `mem::zeroed()` for a `&mut [u8]`, which Rust compiles into a guaranteed panic, so the optimizer removed everything after "EFI OK". See commit `b8d2e0e` and `docs/TESTS.md`.
After source reconciliation, the build process became unstable.

A later build produced only:

    EFI OK

instead of the previous full kernel boot output.

Investigation showed a major difference in EFI binary sizes:

    Old working BOOTX64.EFI: approximately 28 KB
    New EFI: approximately 2.5 KB

The new EFI appeared to contain only the UEFI stub and was not correctly
linking the actual kernel implementation.

The Rust library itself produced a large rlib (approximately 6.4 MB), but
the final EFI executable was not incorporating the kernel code correctly.

Claude began investigating Cargo.toml / rlib vs final EFI linking and was
about to copy the old working EFI as a temporary workaround. That action was
stopped.

IMPORTANT:
Do NOT replace the working build with an old EFI binary as a "temporary fix"
without first understanding the build/linking problem. That can hide the
source/build regression.

### 12. Priorities (August 2026)
The project should now be treated as two separate problems:

A. INPUT HARDWARE SUPPORT
   The original [POLLING] problem was caused by the kernel's PS/2 assumption
   while testing with USB keyboards.

   The correct next driver is a minimal USB HID keyboard driver, preferably
   xHCI for modern hardware.

B. BUILD / SOURCE INTEGRITY
   The UEFI build must first be restored so that the actual Rustica kernel is
   linked into BOOTX64.EFI.

   The fact that the image previously booted to the CLI but now only says
   "EFI OK" indicates a build/linking/source-tree regression, not a keyboard
   hardware problem.

### 13. Recommended Order of Operations
1. Stop making additional APIC/IRQ1 changes.
2. Restore the known-good canonical source/build structure.
3. Verify that BOOTX64.EFI contains the actual kernel and boots to the CLI.
4. Verify the exact source directory used by the build.
5. Remove obsolete duplicate/debug source trees only after confirming they are
   not build inputs.
6. Keep the useful APIC fixes, but do not use them as the primary keyboard
   debugging target.
7. Implement minimal xHCI USB HID keyboard support.
8. Initially use USB polling to prove enumeration and HID reports work.
9. Once USB keyboard input works, add xHCI interrupt support.
10. Remove temporary debug messages such as [UEFI], [POLLING], APIC pixel
    markers, and halt-test code after the corresponding subsystem is verified.

### 14. Key Lesson
The original diagnosis was backwards:

The kernel was not necessarily failing to deliver IRQ1.

It was attempting to use a PS/2 keyboard interface while the test keyboard
was USB.

Therefore:

    USB keyboard
        X
        |
    PS/2 8042 / IRQ1

is the fundamental hardware mismatch.

The next-generation Rustica design should not assume POSIX or legacy PC
input interfaces. Device drivers should be built around explicit hardware
abstractions, with USB HID support treated as a first-class input path.

## May 2026 debugging log (from the website `DEBUG.md`)

> **Corrections (October 2026).** Some conclusions below turned out to be wrong:
> - "Fix 8: PORTSC bit positions" used the EHCI layout. In xHCI, PED is bit 1 and PR is bit 4 (see the xHCI spec, section 5.4.8).
> - "Fix 9: CRCR readback MISMATCH" was not a bug. The Command Ring Pointer always reads back as 0 (xHCI 5.4.5).
> - Scanning only devices 0–7 on bus 0 was introduced around this time and hid the chipset USB controller.

**Status:** CRITICAL BUG FOUND - CRCR invalid, xHCI command path broken
**Last Updated:** 2026-05-12 (Fix 9: CRCR calculation and ERST setup)
**Image:** rustica-live-amd64-0.1.0.img
**SHA256:** 4f4cd803fb603275076b65e83c065573bfcbe764fb9e90f1c1a92c2015f08726
**Test Hardware:** Laptop with USB keyboard (no PS/2 port)

---

#### CRITICAL BUG DISCOVERED: xHCI Command Path Broken (Fix 9)

##### Test Results (2026-05-12 - After Fix 8)

**What worked:**
- xHCI controller detected ✅
- USB port detects device (CCS=1 PED=1 Low) ✅
- Port reset executes ✅
- Device descriptor read attempted ✅

**What failed:**
```
xHCI: Enable Slot timeout
```

##### Root Cause: Invalid CRCR and Missing ERST Setup

**The Bug:**

1. **CRCR value was invalid:** `0x0000E839` - Only ~59KB, clearly wrong for a kernel memory address
   - CRCR must use physical address for DMA
   - Must be 64-byte aligned
   - Was using unaligned virtual address

2. **No Event Ring Segment Table (ERST) setup:**
   - Controller didn't know where to send completion events
   - No interrupter enable
   - No DCBAA (Device Context Base Address Array) setup

##### Fix Applied (Fix 9)

**1. Fixed CRCR Calculation:**
```rust
// Calculate CRCR: Command Ring Control Register
// [63:6] Physical address (64-byte aligned)
// [5:1] Reserved
// [0] RCS (Ring Cycle State)
let cmd_ptr = cmd_base + (enqueue_idx as u64 * 16);
let aligned_ptr = cmd_ptr & !0x3F;  // 64-byte align
let rcs_bit = (cycle & 1) as u64;
let crcr = aligned_ptr | rcs_bit;
```

**2. Added 64-byte alignment:**
```rust
#[repr(C, align(64))]
pub struct CommandRing { ... }

#[repr(C, align(64))]
pub struct EventRing { ... }
```

**3. Added ERST setup:**
```rust
pub struct ErstEntry {
    pub segment_base: u64,   // Physical address
    pub segment_size: u32,   // Number of TRBs
    _reserved: u32,
}
```

**4. Added DCBAA setup:**
```rust
static mut DCBAA: [u64; 256] = [0; 256]; // Device Context Base Address Array
```

**5. Enabled interrupter 0:**
```rust
self.write_runtime_reg(REG_IMAN, iman | IMAN_IE);
```

**6. Added comprehensive debugging:**
- Command ring base address
- Event ring base address
- ERST entry address
- DCBAA address
- CRCR value with readback verification
- TRB contents before enqueue

##### Expected Output with Fix

```
xHCI: Setting up DCBAA and ERST...
xHCI: Command ring base = 0x... (64-byte aligned)
xHCI: Event ring base = 0x... (64-byte aligned)
xHCI: ERST entry addr = 0x...
xHCI: DCBAA = 0x...
xHCI: ERST configured, interrupter enabled
xHCI: Enable Slot command...
xHCI: CMD ring base=0x... idx=0 cycle=1
xHCI: CRCR=0x... (properly aligned 64-bit address)
xHCI: CRCR readback=0x... (should match)
xHCI: Command complete Slot=1
```

---

#### PREVIOUS STATUS: PORTSC Bit Positions Fixed (Fix 8)

##### Test Results (2026-05-12 - After Fix 7)

**What worked:**
- xHCI controller detected ✅
- USB port detects device (CCS=1 PED=1 Low) ✅
- Device shows as Low Speed keyboard ✅

**What failed:**
```
PORTSC[2] BEFORE: 0x00221603 CCS=1 PED=1 PR=0 Speed=Low
PORTSC[2] DURING: 0x00020200 CCS=0 PED=0 PR=0 Speed=Full
```

Device disconnects during port reset (CCS=1→0, PED=1→0) and PR never shows as set.

##### Root Cause: Wrong PORTSC Bit Positions

**The Bug (duplicate bit values):**
```rust
const PORTSC_CSC: u32 = 0x0000_0002; // bit 1
pub const PORTSC_PED: u32 = 0x0000_0002; // bit 1 - DUPLICATE!
const PORTSC_PEDC: u32 = 0x0000_0010; // bit 4
pub const PORTSC_PR: u32 = 0x0000_0010; // bit 4 - DUPLICATE!
```

**xHCI Spec (section 5.4.8) defines:**
- Bit 0: CCS = 0x0001 (Current Connect Status)
- Bit 1: CSC = 0x0002 (Connect Status Change)
- Bit 2: **PED = 0x0004** (Port Enabled/Disabled) ← Was wrong!
- Bit 3: **PEDC = 0x0008** (Port Enabled/Disabled Change) ← Was wrong!
- Bit 4: OCA = 0x0010 (Over-current Active)
- Bit 5: **PR = 0x0020** (Port Reset) ← Was wrong!
- Bit 9: PP = 0x0200 (Port Power)
- Bits 13:10: Port Speed

**The problem:** PR was at bit 4 (0x0010) instead of bit 5 (0x0020), so we were setting/clearing the wrong bit (OCA instead of PR).

##### Fix Applied (Fix 8)
```rust
pub const PORTSC_PED: u32 = 0x0000_0004; // Bit 2 - FIXED
const PORTSC_PEDC: u32 = 0x0000_0008; // Bit 3 - FIXED
pub const PORTSC_PR: u32 = 0x0000_0020; // Bit 5 - FIXED
```

Also added:
- More debug output showing exact PORTSC write value
- Continue enumeration even if reset fails (device was already enabled)
- Added print statements to track PORTSC before/during/after writes

---

#### PREVIOUS STATUS: USB DEVICE DETECTED, ENUMERATION IN PROGRESS
5. Device descriptor dump
6. PS/2 fallback disabled when xHCI succeeds + CCS=1

---

#### CRITICAL BUG DISCOVERED: PCI Config Field Extraction (FIXED)

##### Bug Identified (2026-05-12)

**Root Cause:** PCI configuration space fields were being read from incorrect offsets/positions.

##### Broken Output (Before Fix)

```
vendor=8086 device=8086 class=02 sub=02 if=02 hdr=00 [INTEL]
vendor=8086 device=8086 class=02 sub=02 if=02 hdr=10 [INTEL]
vendor=10DE device=10DE class=A1 sub=A1 if=A1 hdr=10
```

**Evidence of Bug:**
- `vendor=8086 device=8086` - Impossible (same value for both)
- `vendor=10DE device=10DE` - Impossible (same value for both)
- `class=02 sub=02 if=02` - Impossible (all fields identical)

##### Bug: Incorrect PCI Field Extraction

**The Problem:**
```rust
// WRONG - Reading from byte offsets instead of DWORD-aligned offsets
let vendor_id = pci_read_config(bus, device, function, 0x00) as u16;  // Offset 0x00 as u16
let device_id = pci_read_config(bus, device, function, 0x02) as u16;    // Offset 0x02 as u16
let class = pci_read_config(bus, device, function, 0x08) as u8;       // Offset 0x08 as u8
let subclass = pci_read_config(bus, device, function, 0x09) as u8;    // Offset 0x09 as u8
```

**Why This Failed:**
- `pci_read_config()` returns a 32-bit DWORD
- PCI config space must be read at DWORD-aligned offsets (0x00, 0x04, 0x08, 0x0C)
- Fields must be extracted from specific bit positions within each DWORD

##### Fix Applied: Correct PCI Field Extraction

```rust
// CORRECT - Read DWORDs and extract fields
// Offset 0x00: Vendor ID (bits 0-15) + Device ID (bits 16-31)
let vendor_device = pci_read_config(bus, device, function, 0x00);
let vendor_id = (vendor_device & 0xFFFF) as u16;
let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

// Offset 0x08: Class (bits 24-31) + Subclass (bits 16-23) + Prog IF (bits 8-15) + Revision (bits 0-7)
let class_reg = pci_read_config(bus, device, function, 0x08);
let class = ((class_reg >> 24) & 0xFF) as u8;
let subclass = ((class_reg >> 16) & 0xFF) as u8;
let prog_if = ((class_reg >> 8) & 0xFF) as u8;

// Offset 0x0C: Header Type (bits 16-23)
let header_reg = pci_read_config(bus, device, function, 0x0C);
let header_type = ((header_reg >> 16) & 0xFF) as u8;
```

##### Expected Output After Fix

```
PCI: 00:00.0 vendor=8086 device=9B54 class=00 sub=00 if=00 hdr=00 [INTEL]
PCI: 00:01.0 vendor=8086 device=A0D1 class=01 sub=01 if=01 hdr=80 [INTEL]
PCI: 00:14.0 vendor=8086 device=A36D class=0C sub=03 if=30 hdr=80 [USB] [INTEL]
PCI: 00:1F.0 vendor=8086 device=AAAA class=06 sub=04 if=20 hdr=01 [BRIDGE] buses=0-1A-1A
```

**Key Indicators:**
- Vendor ID ≠ Device ID (e.g., `vendor=8086 device=A36D`)
- Different class/subclass values per device type
- USB controllers: `class=0C sub=03 if=30` (xHCI) or `if=20` (EHCI)

---

#### Problem Description

Keyboard input is not working when booting from live USB on laptop hardware.

##### Observed Behavior

1. System boots successfully to shell
2. USB keyboard detection fails
3. Falls back to PS/2 (which doesn't exist on laptop)
4. No keyboard input accepted
5. `[POLLING]` message was shown (fixed in later build)

##### Boot Messages (Before Fix)

```
Step 1: Scanning PCI bus...
Step 2: Trying xHCI (USB 3.0)...
PCI: Scanning for USB controllers...
PCI: No compatible USB controller found
USB: No xHCI controller found
Step 2b: Trying EHCI (USB 2.0)...
PCI: Scanning for USB controllers...
PCI: No compatible USB controller found
USB: EHCI init failed
[PS/2]
Interrupts initialized
Keyboard initialized
Interrupts enabled
Starting shell
> rustux>
```

##### Expected Behavior

```
PCI: Found xHCI (USB 3.0) controller
PCI: MMIO base = 0xXXXXXXXX
USB: xHCI initialized
[USB KBD]
```

---

#### Architecture

##### Keyboard Initialization Flow

```
keyboard::init()
    │
    ├─> usb::init()  // Tries USB first (Tier-1)
    │       │
    │       ├─> xhci::init()
    │       │       └─> pci::scan_for_usb_controller()
    │       │               └─> PCI config I/O reads (0xCF8/0xCFC)
    │       │
    │       └─> ehci::init()  // Fallback
    │               └─> pci::scan_for_usb_controller()
    │
    └─> ps2::controller_present()  // Fallback
            └─> PS/2 port reads (0x60/0x64)
```

##### Current Code Location

**PCI Scan:** `/var/www/rustux.com/prod/loader/kernel-efi/src/usb/pci.rs`
- Uses legacy PCI Configuration Space Access Mechanism #1 (CAM1)
- I/O ports: `PCI_CONFIG_ADDRESS = 0xCF8`, `PCI_CONFIG_DATA = 0xCFC`
- **NOW:** Scans buses 0-255 (was 0-15)
- **NOW:** Includes debug output for devices found and buses with devices

**USB Controllers:** `/var/www/rustux.com/prod/loader/kernel-efi/src/usb/`
- `xhci.rs` - USB 3.0 (xHCI) controller driver
- `ehci.rs` - USB 2.0 (EHCI) controller driver
- `device.rs` - USB device enumeration
- `hid.rs` - USB HID keyboard polling

---

#### Fixes Attempted

##### Fix 7: Comprehensive xHCI Enumeration Debugging ✅ **(LATEST FIX - 2026-05-12)**
- **Issue:** USB device detected but enumeration failing, no debugging visibility
- **Evidence:** "USB: Port 2... CONNECTED" but "No device found on ports 1-4"
- **Root Cause:** Insufficient debugging to identify where enumeration fails
- **Action:** Added comprehensive debugging:
  - PORTSC dumps before/during/after reset with CCS, PED, PR, Port Speed
  - Enable Slot command with CRCR and completion events
  - Control transfer debugging (GET_DESCRIPTOR, SET_ADDRESS)
  - Event ring debugging with completion codes
  - Device descriptor dump
  - PS/2 fallback disabled when xHCI succeeds + CCS=1
- **Code:** See "New xHCI Enumeration Debugging" section
- **Result:** Awaiting test - should provide detailed enumeration trace

##### Fix 6: CRITICAL - Fixed PCI Config Field Extraction ✅
- **Issue:** PCI fields read from incorrect offsets/bit positions
- **Evidence:** `vendor=8086 device=8086` (identical values - impossible)
- **Root Cause:** Reading byte offsets instead of DWORD-aligned offsets
- **Action:** Corrected field extraction from DWORDs at proper offsets
- **Result:** ✅ FIXED - xHCI controller now detected, USB device found

##### Fix 5: Removed bus 15 limit ✅
- **Issue:** Only scanning buses 0-15, USB controller on higher bus
- **Action:** Removed `if bus > 15 { break; }` - now scans all 255 buses
- **Result:** ✅ FIXED - Combined with Fix 6, now finds USB controllers

##### Fix 4: Deleted old kernel directory ✅
- **Issue:** `/var/www/rustux.com/prod/kernel-efi/` was old unused code
- **Action:** Deleted directory
- **Result:** Confirmed build uses correct `/var/www/rustux.com/prod/loader/kernel-efi/`

##### Fix 3: Removed device 0 check ✅
- **Issue:** Only scanned bus 0
- **Code:** `if bus == 0 { break; }` after first bus
- **Action:** Changed to `if bus > 15 { break; }` to scan buses 0-15
- **Result:** Still no USB controllers found

##### Fix 3: Removed device 0 check ✅
- **Issue:** Skipped entire buses if device 0 didn't exist
- **Code:** `if vendor_id == 0xFFFF { continue; }` for bus check
- **Action:** Removed the bus-level device 0 check
- **Result:** Still no USB controllers found (needed more buses)

##### Fix 4: Added debug output ✅
- **Action:** Added test PCI read and device counter
- **Result:** CONFIRMED PCI I/O ports work (see Test Results below)

##### Fix 5: Removed bus 15 limit ✅ **(LATEST FIX)**
- **Issue:** Only scanned buses 0-15, USB controller likely on higher bus
- **Evidence:** Test showed 16 devices on buses 0-15, but no USB controller
- **Action:** Removed `if bus > 15 { break; }` - now scans all 255 buses
- **Result:** Awaiting test

---

#### Test Results (2026-05-12)

##### Debug Output Results

```
PCI: Test read bus0:dev0:fn0 vendor = 0x9B548086
PCI: Total devices found = 16
```

**Analysis:**
- `0x9B548086` = 32-bit PCI config read
  - Device ID: `0x9B54` (upper 16 bits)
  - Vendor ID: `0x8086` = **Intel** (lower 16 bits) ✅
- 16 PCI devices found on buses 0-15
- PCI I/O ports **ARE WORKING** under UEFI

##### Key Finding

**PCI I/O ports work correctly**, but the USB controller is on a bus number > 15.
Modern PCIe hierarchies often place USB controllers on higher bus numbers.

---

#### Root Cause Analysis

##### Hypothesis 1: PCI I/O Ports Don't Work Under UEFI ❌

**Probability:** LOW (DISPROVEN)

**Test Result:** PCI read returned `0x9B548086` (valid Intel device ID)
**Conclusion:** PCI I/O ports work under UEFI on this hardware

---

##### Hypothesis 2: USB Controllers on Higher Bus Numbers ✅ **CONFIRMED**

**Probability:** HIGH (CONFIRMED)

**Evidence:**
- 16 PCI devices found on buses 0-15
- No USB controllers in that range
- USB controller must be on bus > 15
- PCIe topology can place devices on any bus number

**Fix Applied:** Removed bus 15 limit, now scans buses 0-255

**Expected Result:** Should find USB controller and initialize keyboard

---

##### Hypothesis 3: PCI Class/Subclass Codes Wrong

**Probability:** LOW

**Reasoning:** Still possible but unlikely. USB controllers should use standard Class 0x0C, Subclass 0x03.

**Test:** After full bus scan, if still not found, will print ALL PCI devices to check class codes.

---

#### Debug Output (Current)

```rust
// Test if PCI reads work at all
let test_vendor = pci_read_config(0, 0, 0, PCI_VENDOR_ID);
// Output: "PCI: Test read bus0:dev0:fn0 vendor = 0xXXXX"

// Count total PCI devices found
let mut devices_found = 0u32;
let mut buses_with_devices = 0u8;

// Report total
// Output: "PCI: Total devices found = X"
// Output: "PCI: Buses with devices = Y"
```

**Expected Results (After Fix):**
- Total devices: Should be > 16 (more devices on higher buses)
- Buses with devices: Should show how many buses have PCI devices
- Should see "PCI: Found xHCI" or "PCI: Found EHCI"

---

#### Next Steps

##### Immediate: Test Full Bus Scan

1. Test new image with all 255 buses scanned
2. Look for "PCI: Found xHCI/EHCI" message
3. Check if "[USB KBD]" appears instead of "[PS/2]"
4. Test keyboard input

##### If Still Not Found:

**Option A: Print All PCI Devices**
- List every PCI device found with class/subclass
- Find USB controller with non-standard class codes
- May need to check vendor-specific class codes

**Option B: Check for Hidden USB Controllers**
- Some systems hide USB behind PCIe bridges
- May need recursive PCIe scanning
- Check device type (bridge vs endpoint)

---

#### Workarounds (While Fixing)

##### Option 1: External USB Keyboard
- Some external keyboards may use different USB controllers
- Test with known-working USB keyboard

##### Option 2: PS/2 Keyboard Adapter
- USB-to-PS/2 adapter may work
- Only if laptop has PS/2 port via dock

##### Option 3: Serial Console
- Use serial port for shell interaction
- Requires serial cable and another computer

---

#### Reference Information

##### PCI Configuration Space Access

**Mechanism 1 (CAM1) - Current (WORKING):**
- I/O ports: 0xCF8 (CONFIG_ADDRESS), 0xCFC (CONFIG_DATA)
- Legacy standard from ISA/PCI days
- **STATUS:** Working on this hardware (Intel vendor ID confirmed)

**Mechanism 2 (ECAM) - Alternative:**
- Memory-mapped config space
- Base address from ACPI MCFG table
- Standard for PCIe systems
- Not needed for this hardware (CAM1 works)

##### USB Controller PCI IDs

**xHCI (USB 3.0):**
- Class: 0x0C (Serial Bus)
- Subclass: 0x03 (USB)
- Prog IF: 0x30 (xHCI)

**EHCI (USB 2.0):**
- Class: 0x0C (Serial Bus)
- Subclass: 0x03 (USB)
- Prog IF: 0x20 (EHCI)

##### Common USB Controller Vendors

- Intel: 0x8086 ✅ (confirmed present)
- AMD: 0x1022
- VIA: 0x1106
- NEC: 0x1033

##### PCIe Bus Numbering

PCIe can have up to 256 buses (0-255):
- Bus 0: Typically chipset/root devices
- Buses 1-15: Common devices
- Buses 16-255: PCIe devices, hot-plug devices, etc.

USB controllers commonly appear on higher bus numbers on modern systems.

---

#### Files Modified

1. `/var/www/rustux.com/prod/loader/kernel-efi/src/usb/pci.rs`
   - **FIXED:** Corrected PCI config field extraction (Fix 6 - CRITICAL)
   - Removed bus 0 device 0 check (Fix 3)
   - Removed bus 15 limit - scans all 255 buses (Fix 5)
   - Added full PCI device diagnostic dump (Fix 4)
   - Added debug output for devices/buses found (Fix 4)
   - Unknown USB controller prog_if now attempted (Fix 4)
   - Added print_hex_byte helper function (Fix 4)

2. `/var/www/rustux.com/prod/loader/kernel-efi/src/usb/device.rs` (Fix 7 - LATEST)
   - Added comprehensive PORTSC dumping (before/during/after reset)
   - Added port speed name extraction
   - Added event TRB debugging with completion codes
   - Added device descriptor dump
   - Added Enable Slot command debugging
   - Added Address Device debugging
   - Added xHCI_succeeded_with_device() flag to prevent PS/2 fallback
   - Added DEVICE_DESC_BUFFER for DMA

3. `/var/www/rustux.com/prod/loader/kernel-efi/src/usb/xhci.rs` (Fix 7)
   - Added CommandRing struct and implementation
   - Added command_ring field to XhciController
   - Made PORTSC_PED, PORTSC_PR, PORTSC_PORT_SPEED_MASK public
   - Made write_port_sc() public
   - Added issue_enable_slot_command() method
   - Added issue_control_get() method (GET_DESCRIPTOR)
   - Added issue_control_set() method (SET_ADDRESS, etc.)
   - Fixed CRCR write as two 32-bit registers

4. `/var/www/rustux.com/prod/loader/kernel-efi/src/keyboard/mod.rs` (Fix 7)
   - Added check for xhci_succeeded_with_device()
   - Prevents PS/2 fallback when USB device detected but enumeration fails
   - Shows `[USB DETECTED - ENUM PENDING]` in that case

5. Deleted: `/var/www/rustux.com/prod/kernel-efi/` (old duplicate) (Fix 1)
6. Deleted: `/var/www/rustux.com/prod/loader/rustica/` (empty directory)

---

#### Build Command

```bash
cd /var/www/rustux.com/prod/loader
./build-live-image.sh
```

**Output:** `/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img`

---

#### Test Procedure

1. Download latest image (SHA256: 62322a3acabc7c1f2f78f95361f8c87bedfa7eedbe7bf1535a265d8349963ad2)
2. Write to USB drive
3. Boot laptop from USB
4. **CRITICAL:** Record ALL output including:
   - PCI device dump (every line starting with "PCI:")
   - PORTSC dumps (before/during/after reset)
   - xHCI enumeration debugging messages
   - Event ring debugging (completion codes)
   - Device descriptor dump (if enumeration succeeds)
5. Look for `[USB KBD]` or `[USB DETECTED - ENUM PENDING]` instead of `[PS/2]`
6. Test keyboard input

---

#### New xHCI Enumeration Debugging (Fix 7)

##### Debug Features Added

1. **PORTSC Dumping:**
   - Before port reset: Shows CCS, PED, PR, Port Speed
   - During port reset: Shows PR bit set
   - After port reset: Shows PR cleared, PED set, device speed

2. **Enable Slot Command Debugging:**
   - Command TRB enqueue
   - CRCR register value
   - Command completion event with slot ID
   - Completion code verification

3. **Control Transfer Debugging:**
   - Setup packet breakdown (bmRequestType, bRequest, wValue, wLength)
   - Data stage TRB debugging
   - Status stage debugging
   - Event ring completion codes

4. **Device Descriptor Dump:**
   - bLength, bDescriptorType
   - bcdUSB, bDeviceClass, bDeviceSubClass
   - idVendor, idProduct
   - bNumConfigurations

5. **PS/2 Fallback Prevention:**
   - System keeps USB backend when xHCI succeeds and CCS=1
   - Shows `[USB DETECTED - ENUM PENDING]` if enumeration fails but device detected
   - Prevents incorrect PS/2 fallback on USB-only systems

##### Expected Debug Output

```
USB: Port 2... CCS=1 PED=1 High
USB: === PORT RESET DEBUG ===
USB: Port 2 reset sequence:
PORTSC[2] BEFORE: 0x00200200 CCS=1 PED=0 PR=0 Speed=Full
USB: Setting PR bit...
PORTSC[2] DURING: 0x00210202 CCS=1 PED=1 PR=1 Speed=High
USB: Waiting for PR to clear...
USB: PR cleared after <10ms
PORTSC[2] AFTER: 0x00200205 CCS=1 PED=1 PR=0 Speed=High
USB: Port enabled OK (PED=1)
USB: Device still connected (CCS=1)
USB: === ENABLE SLOT COMMAND ===
USB: Issuing Enable Slot command...
xHCI: CRCR=0x...
xHCI: Command complete Slot=1
USB: Enable Slot OK - Slot ID=01
USB: === GET DEVICE DESCRIPTOR ===
USB: Setup: bmRequestType=0x80 bRequest=0x06 wValue=0x0100 wLength=0x0012
USB: Get Descriptor OK
  Device Descriptor:
    bLength=12
    bDescriptorType=01
    bcdUSB=0x0110
    bDeviceClass=00
    bDeviceSubClass=00
    ...
USB: === SET DEVICE ADDRESS ===
USB: Setting device address to 02...
USB: Set Address OK
USB: === ASSUMING HID KEYBOARD ===
USB: Slot=01 Endpoint=0x81 MaxPacket=08
[USB KBD]
```

---

#### Expected Results (After Enumeration Debugging)

##### Correct PCI Device Output:
```
PCI: 00:00.0 vendor=8086 device=9B54 class=00 sub=00 if=00 hdr=00 [INTEL]
PCI: 00:01.0 vendor=8086 device=A0D1 class=01 sub=01 if=01 hdr=80 [INTEL]
PCI: 00:14.0 vendor=8086 device=A36D class=0C sub=03 if=30 hdr=80 [USB] [INTEL]
  BAR0 = 0xXXXXXXXX [MEM]
```

##### Boot Sequence:
```
PCI: Scanning for USB controllers...
PCI: Test read bus0:dev0:fn0 vendor = 0x9B548086
PCI: 00:14.0 vendor=8086 device=A36D class=0C sub=03 if=30 hdr=80 [USB] [INTEL]
PCI: Found xHCI (USB 3.0) controller
PCI: MMIO base = 0xXXXXXXXX
PCI: Using xHCI controller
USB: xHCI initialized
[USB KBD]
Interrupts initialized
Keyboard initialized
Interrupts enabled
Starting shell
> rustux> _
```

**Key Success Indicators:**
1. ✅ Vendor ID ≠ Device ID (e.g., `vendor=8086 device=A36D`)
2. ✅ Different class/subclass values per device type
3. ✅ Device with `class=0C sub=03 if=30` (xHCI) or `if=20` (EHCI)
4. ✅ `[USB KBD]` message appears
5. ✅ Keyboard input works

---

**Previous Broken Output (For Comparison):**
```
vendor=8086 device=8086 class=02 sub=02 if=02 hdr=00 [INTEL]
vendor=10DE device=10DE class=A1 sub=A1 if=A1 hdr=10
```

---

**Next Action:** Test with comprehensive xHCI enumeration debugging. Expected outputs:

1. **PCI Detection:** Should see correct PCI device output with vendor/device mismatch
2. **xHCI Init:** Should see "xHCI initialized" with MMIO base
3. **Port Detection:** Should see "Port X... CCS=1 PED=1 Speed"
4. **PORTSC Debug:** Should see PORTSC dumps before/during/after reset
5. **Enable Slot:** Should see "Enable Slot OK - Slot ID=X" or error code
6. **Device Descriptor:** Should see descriptor dump with vendor/product IDs
7. **Keyboard Backend:** Should see `[USB KBD]` or `[USB DETECTED - ENUM PENDING]`
8. **NO PS/2:** Should NOT see `[PS/2]` when USB device is detected

Please share the complete boot output from "=== USB INIT DEBUG ===" onwards.

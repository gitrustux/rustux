//! Rustux Kernel - UEFI Entry Point with Simple Keyboard Test

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;
extern crate rustux;

use uefi::prelude::*;
use core::arch::asm;
use core::ptr::write_volatile;

use rustux::arch::amd64::{descriptor, idt, apic};

// Note: Global allocator is now in src/mm/allocator.rs (LinkedListAllocator)
// The UEFI allocator is no longer used as the global allocator after exit_boot_services()

// Simple keyboard scancode counter
static mut KEYBOARD_COUNT: u32 = 0;

/// Initialize the 8042 Keyboard Controller
///
/// The keyboard controller must be initialized to generate IRQ1 interrupts.
/// This function:
/// 1. Disables the keyboard
/// 2. Flushes the output buffer
/// 3. Reads the current command byte
/// 4. Enables interrupt generation (bit 0)
/// 5. Re-enables the keyboard
fn keyboard_controller_init() {
    const KB_DATA_PORT: u16 = 0x60;
    const KB_CMD_PORT: u16 = 0x64;
    const CMD_READ_CB: u8 = 0x20;   // Read command byte
    const CMD_WRITE_CB: u8 = 0x60;  // Write command byte
    const CMD_DISABLE: u8 = 0xAD;   // Disable keyboard
    const CMD_ENABLE: u8 = 0xAE;    // Enable keyboard

    unsafe {
        debug_print("[KBD] Initializing 8042 keyboard controller...\n");

        // Disable keyboard
        asm!("out dx, al", in("dx") KB_CMD_PORT, in("al") CMD_DISABLE, options(nostack));

        // Flush output buffer
        let mut status: u8;
        asm!("in al, dx", out("al") status, in("dx") KB_CMD_PORT, options(nomem, nostack));
        if status & 0x01 != 0 {
            let _dummy: u8;
            asm!("in al, dx", out("al") _dummy, in("dx") KB_DATA_PORT, options(nomem, nostack));
        }

        // Read current command byte
        asm!("out dx, al", in("dx") KB_CMD_PORT, in("al") CMD_READ_CB, options(nostack));
        // Wait for data ready
        loop {
            asm!("in al, dx", out("al") status, in("dx") KB_CMD_PORT, options(nomem, nostack));
            if status & 0x01 != 0 { break; }
        }
        let mut cmd_byte: u8;
        asm!("in al, dx", out("al") cmd_byte, in("dx") KB_DATA_PORT, options(nomem, nostack));

        // Set bit 0 (enable keyboard interrupt) and bit 6 (translate scancode to set 2)
        cmd_byte |= 0x41;  // Enable interrupt + translate scancode

        // Write modified command byte
        asm!("out dx, al", in("dx") KB_CMD_PORT, in("al") CMD_WRITE_CB, options(nostack));
        // Wait for input buffer empty
        loop {
            asm!("in al, dx", out("al") status, in("dx") KB_CMD_PORT, options(nomem, nostack));
            if status & 0x02 == 0 { break; }
        }
        asm!("out dx, al", in("dx") KB_DATA_PORT, in("al") cmd_byte, options(nostack));

        // Re-enable keyboard
        asm!("out dx, al", in("dx") KB_CMD_PORT, in("al") CMD_ENABLE, options(nostack));

        debug_print("[KBD] Keyboard controller initialized\n");
    }
}

#[entry]
fn main() -> Status {
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║           RUSTUX KERNEL - KEYBOARD INTERRUPT TEST       ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    let acpi_rsdp = find_acpi_rsdp();
    if let Some(rsdp) = acpi_rsdp {
        debug_print("[OK] ACPI RSDP found: 0x");
        print_hex(rsdp);
        debug_print("\n");
    }

    debug_print("[PHASE] Skipping pre-exit_boot_services userspace test...\n");
    debug_print("        (PMM not initialized yet, test moved to post-boot)\n\n");

    // NOTE: Userspace test moved to after PMM initialization in init_late()
    // The PMM must be initialized before we can allocate pages for address spaces

    debug_print("[PHASE] Exiting boot services...\n\n");
    let _memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    kernel_main();
}

fn kernel_main() -> ! {
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  KERNEL MODE - Testing Interrupts                       ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    // Initialize kernel subsystems (including heap)
    rustux::init::kernel_init();

    // Setup GDT
    debug_print("[1/5] Setting up GDT...\n");
    unsafe { descriptor::gdt_setup(); }
    debug_print("      ✓ GDT configured\n");

    // Setup IDT
    debug_print("[2/5] Setting up IDT...\n");
    unsafe { descriptor::idt_setup_readonly(); }
    debug_print("      ✓ IDT configured\n");

    // Install timer handler
    debug_print("[3/5] Installing timer handler...\n");
    unsafe { idt::idt_set_gate(32, timer_handler as u64, 0x08, 0x8E); }
    debug_print("      ✓ Timer handler at vector 32\n");

    // Install keyboard handler
    debug_print("[3.5/5] Installing keyboard handler...\n");
    unsafe { idt::idt_set_gate(33, keyboard_handler as u64, 0x08, 0x8E); }
    debug_print("      ✓ Keyboard handler at vector 33\n");

    // Initialize APIC
    debug_print("[4/5] Initializing APIC...\n");
    unsafe { apic::apic_local_init(); }
    debug_print("      ✓ APIC initialized\n");

    // Configure keyboard IRQ
    debug_print("[4.5/5] Configuring keyboard IRQ...\n");
    unsafe { apic::apic_io_init(1, 33); }
    debug_print("      ✓ IRQ1 → Vector 33\n");

    // Initialize keyboard controller
    debug_print("[4.6/5] Initializing keyboard controller...\n");
    keyboard_controller_init();
    debug_print("      ✓ Keyboard controller initialized\n");

    // Configure timer
    debug_print("[5/5] Configuring timer...\n");
    unsafe {
        let lapic = 0xFEE00000usize;
        write_volatile((lapic + 0x3E0) as *mut u32, 0x03);
        write_volatile((lapic + 0x320) as *mut u32, 32 | (1 << 17));
        write_volatile((lapic + 0x380) as *mut u32, 10_000_000);
    }
    debug_print("      ✓ Timer configured\n\n");

    // Enable interrupts
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  PHASE 4A: Testing Userspace Execution                  ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    unsafe { asm!("sti"); }

    // TEST: Userspace execution (Phase 4A) - MOVED BEFORE exit_boot_services
    // Load and execute the userspace ELF binary
    // NOTE: This is now done in main() before exiting boot services
    // because the UEFI allocator is needed for heap allocations

    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  Userspace test moved to UEFI mode                   ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    // Never reached
    loop { unsafe { asm!("hlt"); } }
}

// Keyboard handler (IRQ1 = Vector 33)
#[no_mangle]
pub extern "x86-interrupt" fn keyboard_handler(_sf: idt::X86Iframe) {
    unsafe {
        // Read scancode
        let scancode: u8;
        asm!("in al, dx", in("dx") 0x60u16, out("al") scancode, options(nomem, nostack, preserves_flags));

        // Print it
        debug_print("[KEY:");
        print_hex(scancode as u64);
        debug_print("]\n");

        KEYBOARD_COUNT += 1;

        // Send EOI to LAPIC (write 0 to EOI register at offset 0x40)
        let lapic = 0xFEE00000usize;
        write_volatile((lapic + 0x40) as *mut u32, 0);
    }
}

// Timer handler (Vector 32)
#[no_mangle]
pub extern "x86-interrupt" fn timer_handler(_sf: idt::X86Iframe) {
    unsafe {
        let msg = b"[TICK]\n";
        for &b in msg {
            asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags));
        }
        let lapic = 0xFEE00000usize;
        write_volatile((lapic + 0xB0) as *mut u32, 0);
    }
}

fn find_acpi_rsdp() -> Option<u64> {
    use uefi::table::cfg::ConfigTableEntry;
    let mut result = None;
    uefi::system::with_config_table(|slice| {
        for entry in slice {
            if entry.guid == ConfigTableEntry::ACPI2_GUID && !entry.address.is_null() {
                result = Some(entry.address as u64);
                break;
            }
        }
    });
    result
}

const QEMU_DEBUGCON_PORT: u16 = 0xE9;

fn qemu_debugcon_write_byte(b: u8) {
    unsafe {
        asm!("out dx, al", in("dx") QEMU_DEBUGCON_PORT, in("al") b, options(nostack, nomem));
    }
}

fn debug_print(s: &str) {
    for byte in s.bytes() {
        qemu_debugcon_write_byte(byte);
    }
}

fn print_hex(mut n: u64) {
    if n == 0 {
        qemu_debugcon_write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 16];
    let mut i = 0;
    while n > 0 {
        let digit = (n % 16) as u8;
        buf[i] = if digit < 10 { b'0' + digit } else { b'A' + digit - 10 };
        n /= 16;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        qemu_debugcon_write_byte(buf[i]);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("hlt", options(nostack, nomem)) }; }
}
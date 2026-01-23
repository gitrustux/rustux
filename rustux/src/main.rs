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
use rustux::drivers::keyboard;

// Note: Global allocator is now in src/mm/allocator.rs (LinkedListAllocator)
// The UEFI allocator is no longer used as the global allocator after exit_boot_services()

// Simple keyboard scancode counter (legacy, for compatibility)
static mut KEYBOARD_COUNT: u32 = 0;

/// Initialize the 8042 Keyboard Controller
///
/// This is now a wrapper around the new keyboard::init() function.
/// The keyboard controller must be initialized to generate IRQ1 interrupts.
fn keyboard_controller_init() {
    unsafe {
        debug_print("[KBD] Initializing PS/2 keyboard driver...\n");

        // Use the new keyboard driver module
        keyboard::init();

        debug_print("[KBD] Keyboard driver initialized\n");
    }
}

#[entry]
fn main() -> Status {
    use uefi::system;
    use uefi::cstr16;

    // Simple single message - NO special characters, NO reset
    system::with_stdout(|stdout| {
        let msg = cstr16!("EFI OK");
        let _ = stdout.output_string(msg);
    });

    // PROGRESS MARKER: Entry point reached (RED framebuffer)
    fb_red();

    let _acpi_rsdp = find_acpi_rsdp();
    let _memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    // PROGRESS MARKER: ExitBootServices succeeded
    // This confirms kernel is fully in control of hardware
    system::with_stdout(|stdout| {
        let msg = cstr16!("EBS OK");
        let _ = stdout.output_string(msg);
    });

    // PROGRESS MARKER: ExitBootServices succeeded (GREEN framebuffer)
    fb_green();

    // SILENT BOOT PHASE ENDS: Now safe to enable debug output
    unsafe { DEBUG_ENABLED = true; }

    kernel_main();
}

fn kernel_main() -> ! {
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  KERNEL MODE - Testing Interrupts                       ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    // CRITICAL: Initialize PMM first (needed for stack allocation)
    rustux::init::pmm_init();

    // CRITICAL: Switch to proper kernel stack BEFORE any deep operations
    // The firmware stack is too small and causes corruption during ELF loading.
    // This function does NOT return - it jumps to kernel_main_on_new_stack()
    unsafe {
        rustux::arch::amd64::init::init_kernel_stack(kernel_main_on_new_stack as usize);
    }
}

/// Continuation of kernel_main() - runs on the new kernel stack
/// This function is jumped to directly by init_kernel_stack(), it is never called normally.
fn kernel_main_on_new_stack() -> ! {
    debug_print("[STACK] Now running on new kernel stack!\n");

    // Complete the rest of kernel initialization on the new stack
    debug_print("[INIT] Calling kernel_init_rest()...\n");
    rustux::init::kernel_init_rest();
    debug_print("[INIT] kernel_init_rest() returned!\n");

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

    // Install syscall handler (int 0x80)
    debug_print("[3.6/5] Installing syscall handler...\n");
    unsafe { idt::idt_set_gate(0x80, syscall_handler as u64, 0x08, 0x8E); }
    debug_print("      ✓ Syscall handler at vector 0x80\n");

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

    // Initialize display console (Phase 6B)
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  PHASE 6B: Initializing Display Console                   ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");
    unsafe {
        init_display_console();
    }
    debug_print("      ✓ Display console initialized\n\n");

    // Initialize ramdisk (Phase 5C)
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  PHASE 5C: Initializing Ramdisk                          ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");
    unsafe {
        rustux::fs::ramdisk::init_ramdisk(include_bytes!(concat!(env!("OUT_DIR"), "/ramdisk.bin")));
    }
    debug_print("      ✓ Ramdisk initialized\n\n");

    // Try to load and execute init.elf from ramdisk (Phase 5D)
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  PHASE 5D: Loading Init Process                         ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    let init_loaded = unsafe {
        use rustux::fs::ramdisk;
        use rustux::exec::load_elf_process;
        use rustux::process::table::{Process, PROCESS_TABLE};

        // Get the ramdisk
        let ramdisk = match ramdisk::get_ramdisk() {
            Ok(r) => r,
            Err(_) => {
                debug_print("[INIT] Ramdisk not available, skipping init load\n\n");
                false
            }
        };

        // Look for init.elf in ramdisk
        let init_file = match ramdisk.find_file("bin/init") {
            Some(f) => f,
            None => {
                debug_print("[INIT] init.elf not found in ramdisk, skipping\n\n");
                false
            }
        };

        debug_print("[INIT] Found init.elf in ramdisk\n");
        debug_print("[INIT] File size: ");
        print_hex(init_file.size as u64);
        debug_print(" bytes\n");

        // Read the ELF data from ramdisk
        let elf_data_ptr = ramdisk.data.as_ptr().add(init_file.data_offset as usize);
        let elf_data = core::slice::from_raw_parts(elf_data_ptr, init_file.size as usize);

        debug_print("[INIT] Loading ELF binary...\n");

        // Load the ELF binary
        let process_image = match load_elf_process(elf_data) {
            Ok(img) => img,
            Err(e) => {
                debug_print("[INIT] Failed to load ELF: ");
                for &b in e.as_bytes() {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
                debug_print("\n");
                false
            }
        };

        debug_print("[INIT] ELF loaded successfully\n");
        debug_print("[INIT] Entry point: 0x");
        print_hex(process_image.entry);
        debug_print("\n");

        // Allocate kernel stack (4 pages)
        let kernel_stack_paddrs = [
            match rustux::mm::pmm::pmm_alloc_kernel_page() {
                Ok(p) => p,
                Err(_) => {
                    debug_print("[INIT] Failed to allocate kernel stack\n");
                    false
                }
            },
            match rustux::mm::pmm::pmm_alloc_kernel_page() {
                Ok(p) => p,
                Err(_) => {
                    debug_print("[INIT] Failed to allocate kernel stack\n");
                    false
                }
            },
            match rustux::mm::pmm::pmm_alloc_kernel_page() {
                Ok(p) => p,
                Err(_) => {
                    debug_print("[INIT] Failed to allocate kernel stack\n");
                    false
                }
            },
            match rustux::mm::pmm::pmm_alloc_kernel_page() {
                Ok(p) => p,
                Err(_) => {
                    debug_print("[INIT] Failed to allocate kernel stack\n");
                    false
                }
            },
        ];

        // Get the kernel stack virtual addresses
        let kernel_stack_vaddrs = [
            rustux::mm::pmm::paddr_to_vaddr(kernel_stack_paddrs[0]),
            rustux::mm::pmm::paddr_to_vaddr(kernel_stack_paddrs[1]),
            rustux::mm::pmm::paddr_to_vaddr(kernel_stack_paddrs[2]),
            rustux::mm::pmm::paddr_to_vaddr(kernel_stack_paddrs[3]),
        ];

        // Stack grows down, so top is at the highest address
        let kernel_stack_top = (kernel_stack_vaddrs[3] + 4096) as u64;

        // Get page table physical address
        let page_table_phys = process_image.address_space.page_table.phys;

        // Create process with PID 1
        let process = Process::new(
            1,  // PID 1 (init)
            0,  // PPID 0 (kernel)
            page_table_phys,
            kernel_stack_top,
            process_image.stack_top,
            process_image.entry,
        );

        // Set process name
        let mut name_owned = alloc::string::String::from("init");
        process.set_name(name_owned);

        // Add to process table
        PROCESS_TABLE.lock().insert(process);
        PROCESS_TABLE.lock().set_current(1);

        debug_print("[INIT] Process created with PID 1\n");
        debug_print("[INIT] Kernel stack: 0x");
        print_hex(kernel_stack_top);
        debug_print("\n");
        debug_print("[INIT] User stack: 0x");
        print_hex(process_image.stack_top);
        debug_print("\n");
        debug_print("[INIT] Page table: 0x");
        print_hex(page_table_phys);
        debug_print("\n\n");

        debug_print("╔══════════════════════════════════════════════════════════╗\n");
        debug_print("║  Jumping to Init Process (Userspace)                   ║\n");
        debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

        // Execute the init process - never returns
        rustux::arch::amd64::uspace::execute_process(
            process_image.entry,
            process_image.stack_top,
            page_table_phys,
        );

        // Unreachable
        false
    };

    if !init_loaded {
        debug_print("[INIT] Failed to load init process, halting...\n");
        loop { unsafe { asm!("hlt"); } }
    }

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
    use rustux::drivers::keyboard;

    unsafe {
        // Use the new keyboard driver module to handle the IRQ
        keyboard::handle_irq();

        // Debug: show we received an interrupt
        // debug_print("[K]\n");

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

// Syscall handler (int 0x80 = Vector 0x80)
//
// This handler is invoked when userspace executes `int 0x80`.
// It extracts the syscall number and arguments from the interrupt frame
// and dispatches to the appropriate syscall implementation.
#[no_mangle]
pub extern "x86-interrupt" fn syscall_handler(sf: idt::X86Iframe) {
    use rustux::syscall::{SyscallArgs, syscall_dispatch};

    // PROOF: Syscall reached - fill top half CYAN to verify
    unsafe {
        if FRAMEBUFFER_ADDR != 0 {
            let fb_ptr = FRAMEBUFFER_ADDR as *mut u16;
            let pixel_count = FRAMEBUFFER_SIZE as usize / 2;
            // Fill top half with CYAN (RGB565: 0x07FF)
            for i in 0..(pixel_count / 2) {
                *(fb_ptr.add(i)) = 0x07FF;
            }
        }
    }

    let syscall_num = sf.rax as u32;

    // Note: The int 0x80 ABI uses ebx/ecx/edx for args 0/1/2
    let syscall_args = SyscallArgs::new(
        syscall_num,
        [
            sf.rbx as usize,  // arg0 (ebx)
            sf.rcx as usize,  // arg1 (ecx)
            sf.rdx as usize,  // arg2 (edx)
            sf.r10 as usize,  // arg3 (not used by int 0x80, but we have it in the frame)
            sf.r8  as usize,  // arg4 (not used by int 0x80)
            sf.r9  as usize,  // arg5 (not used by int 0x80)
        ],
    );

    // Call the syscall dispatcher
    let _ret = syscall_dispatch(syscall_args);
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

/// Fill the framebuffer with a solid color for progress indication
///
/// Color format: RGB565
/// - Red:   0xF800
/// - Green: 0x07E0
/// - Blue:  0x001F
/// - White: 0xFFFF
/// - Black: 0x0000
fn fill_framebuffer_color(color_rgb565: u32) {
    use uefi::boot;
    use uefi::proto::console::gop::GraphicsOutput;
    use core::mem::transmute;

    unsafe {
        // Get GOP handle using the boot services API
        let gop_handle = boot::get_handle_for_protocol::<GraphicsOutput>()
            .expect("Failed to get GOP handle");

        // Open GOP protocol
        let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle)
            .expect("Failed to open GOP protocol");

        let mode = gop.current_mode_info();
        let fb = gop.frame_buffer();

        // Use transmute to convert FrameBuffer to a mutable u8 slice
        // This is unsafe but necessary because the FrameBuffer type doesn't expose the slice directly
        let fb_slice: &mut [u8] = transmute_copy(&fb);

        // Fill the framebuffer with the color
        let pixel_count = mode.resolution().0 * mode.resolution().1;
        let color_bytes = [
            (color_rgb565 & 0xFF) as u8,
            ((color_rgb565 >> 8) & 0xFF) as u8,
        ];

        for i in 0..pixel_count {
            let offset = i * 2;
            if offset + 1 < fb_slice.len() {
                fb_slice[offset] = color_bytes[0];
                fb_slice[offset + 1] = color_bytes[1];
            }
        }
    }
}

// Helper function for transmuting references
unsafe fn transmute_copy<T, U>(src: &T) -> U {
    let mut dst: U = core::mem::zeroed();
    core::ptr::copy_nonoverlapping(
        src as *const T as *const u8,
        &mut dst as *mut U as *mut u8,
        core::mem::size_of::<T>(),
    );
    dst
}

/// Fill framebuffer red - EFI entry point reached
fn fb_red() {
    fill_framebuffer_color(0xF800);
}

/// Fill framebuffer green - ExitBootServices succeeded
/// Also saves framebuffer info for post-ExitBootServices use
fn fb_green() {
    use uefi::boot;
    use uefi::proto::console::gop::GraphicsOutput;

    unsafe {
        let gop_handle = boot::get_handle_for_protocol::<GraphicsOutput>()
            .expect("Failed to get GOP handle");

        let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(gop_handle)
            .expect("Failed to open GOP protocol");

        let mode = gop.current_mode_info();
        let fb = gop.frame_buffer();

        // Use transmute_copy to convert FrameBuffer to a mutable u8 slice
        let fb_slice: &mut [u8] = transmute_copy(&fb);
        let fb_addr = fb_slice.as_mut_ptr() as u64;
        let pixel_count = mode.resolution().0 * mode.resolution().1;

        // Save framebuffer info for later use
        FRAMEBUFFER_ADDR = fb_addr;
        FRAMEBUFFER_SIZE = (pixel_count * 2) as u64; // 2 bytes per pixel (RGB565)
        FRAMEBUFFER_WIDTH = mode.resolution().0;
        FRAMEBUFFER_HEIGHT = mode.resolution().1;

        // Fill with green (0x07E0 in RGB565)
        let color_bytes = [0xE0, 0x07]; // Little-endian RGB565
        for i in 0..pixel_count {
            let offset = i * 2;
            if offset + 1 < fb_slice.len() {
                fb_slice[offset] = color_bytes[0];
                fb_slice[offset + 1] = color_bytes[1];
            }
        }
    }
}

// Save framebuffer info for use after ExitBootServices
static mut FRAMEBUFFER_ADDR: u64 = 0;
static mut FRAMEBUFFER_SIZE: u64 = 0;
static mut FRAMEBUFFER_WIDTH: usize = 0;
static mut FRAMEBUFFER_HEIGHT: usize = 0;

/// Fill framebuffer blue - CR3 load succeeded (works after ExitBootServices)
/// NOTE: Must be called after fb_green() to capture framebuffer address
pub extern "C" fn fb_blue() {
    unsafe {
        if FRAMEBUFFER_ADDR == 0 {
            return; // No framebuffer available
        }
        let fb_ptr = FRAMEBUFFER_ADDR as *mut u16;
        let pixel_count = FRAMEBUFFER_SIZE as usize / 2;
        for i in 0..pixel_count {
            *(fb_ptr.add(i)) = 0x001F; // Blue (RGB565)
        }
    }
}

/// Fill framebuffer white - About to IRETQ to userspace (works after ExitBootServices)
pub extern "C" fn fb_white() {
    unsafe {
        if FRAMEBUFFER_ADDR == 0 {
            return; // No framebuffer available
        }
        let fb_ptr = FRAMEBUFFER_ADDR as *mut u16;
        let pixel_count = FRAMEBUFFER_SIZE as usize / 2;
        for i in 0..pixel_count {
            *(fb_ptr.add(i)) = 0xFFFF; // White (RGB565)
        }
    }
}

/// Get the framebuffer address (for passing to userspace)
pub fn get_framebuffer_addr() -> u64 {
    unsafe { FRAMEBUFFER_ADDR }
}

/// Get the framebuffer size (for passing to userspace)
pub fn get_framebuffer_size() -> u64 {
    unsafe { FRAMEBUFFER_SIZE }
}

/// Get the framebuffer width (for display console)
pub fn get_framebuffer_width() -> usize {
    unsafe { FRAMEBUFFER_WIDTH }
}

/// Get the framebuffer height (for display console)
pub fn get_framebuffer_height() -> usize {
    unsafe { FRAMEBUFFER_HEIGHT }
}

/// Initialize the display console
///
/// This function should be called after fb_green() to initialize
/// the text console using the framebuffer information.
pub unsafe fn init_display_console() {
    use rustux::drivers::display::{Framebuffer, PixelFormat, init as display_init};

    if FRAMEBUFFER_ADDR == 0 {
        debug_print("[DISPLAY] No framebuffer available, skipping console init\n");
        return;
    }

    // Calculate pitch (stride) from width and bytes per pixel
    let bpp = 16; // RGB565
    let pitch = FRAMEBUFFER_WIDTH * (bpp / 8);

    let framebuffer = Framebuffer::new(
        FRAMEBUFFER_ADDR,
        FRAMEBUFFER_WIDTH,
        FRAMEBUFFER_HEIGHT,
        pitch,
        bpp,
        PixelFormat::RGB,
    );

    display_init(framebuffer);

    debug_print("[DISPLAY] Text console initialized\n");
    debug_print("[DISPLAY] Resolution: ");
    print_hex(FRAMEBUFFER_WIDTH as u64);
    debug_print("x");
    print_hex(FRAMEBUFFER_HEIGHT as u64);
    debug_print("\n");
}

/// Fill framebuffer yellow - Process exited
/// Works after ExitBootServices
pub extern "C" fn fb_yellow() {
    unsafe {
        if FRAMEBUFFER_ADDR == 0 {
            return; // No framebuffer available
        }
        let fb_ptr = FRAMEBUFFER_ADDR as *mut u16;
        let pixel_count = FRAMEBUFFER_SIZE as usize / 2;
        for i in 0..pixel_count {
            *(fb_ptr.add(i)) = 0xFFE0; // Yellow (RGB565)
        }
    }
}

const QEMU_DEBUGCON_PORT: u16 = 0xE9;

fn qemu_debugcon_write_byte(b: u8) {
    unsafe {
        asm!("out dx, al", in("dx") QEMU_DEBUGCON_PORT, in("al") b, options(nostack, nomem));
    }
}

// UEFI-safe debug functions (no-op before exit_boot_services)
static mut DEBUG_ENABLED: bool = false;

#[inline(always)]
fn debug_print(s: &str) {
    unsafe {
        if !DEBUG_ENABLED {
            return;
        }
    }
    for &b in s.as_bytes() {
        unsafe { asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack)); }
    }
}

#[inline(always)]
fn print_hex(n: u64) {
    unsafe {
        if !DEBUG_ENABLED {
            return;
        }
    }
    let mut digits = [0u8; 16];
    let mut i = 0;
    let mut n = n;
    loop {
        let d = (n & 0xF) as u8;
        digits[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        n >>= 4;
        i += 1;
        if n == 0 { break; }
    }
    while i > 0 {
        i -= 1;
        unsafe { asm!("out dx, al", in("dx") 0xE9u16, in("al") digits[i], options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("hlt", options(nostack, nomem)) }; }
}
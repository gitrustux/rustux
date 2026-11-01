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

    // Record the GOP framebuffer while boot services are still available;
    // everything after ExitBootServices draws from the saved values.
    capture_framebuffer();

    // PROGRESS MARKER: Entry point reached (RED framebuffer)
    fb_red();

    let _acpi_rsdp = find_acpi_rsdp();
    record_kernel_image();
    let memory_map = unsafe { uefi::boot::exit_boot_services(None) };
    record_phys_limit(&memory_map);

    // PROGRESS MARKER: ExitBootServices succeeded.
    // UEFI console output is no longer available (uefi::system::with_stdout
    // panics after ExitBootServices), so report on the debug port instead.
    for &b in b"EBS OK\n" {
        qemu_debugcon_write_byte(b);
    }

    // PROGRESS MARKER: ExitBootServices succeeded (GREEN framebuffer)
    fb_green();

    // SILENT BOOT PHASE ENDS: Now safe to enable debug output
    unsafe { DEBUG_ENABLED = true; }

    kernel_main();
}

fn kernel_main() -> ! {
    // ================================================================
    // PRIMITIVE DEBUG PROOF - Kernel entry executes
    // ================================================================
    // This is the FIRST thing that runs in kernel mode.
    // Uses port 0xE9 which works without framebuffer, stack, or Rust runtime.
    // If you see 'K' in QEMU debug console, kernel entry was reached.
    // ================================================================
    unsafe {
        // Step 1: Send 'K' to prove we're in kernel mode
        core::arch::asm!(
            "mov dx, 0xE9",
            "mov al, 0x4B",  // 'K' in ASCII
            "out dx, al",
            options(nomem, nostack)
        );

        // Step 2: Send '1' to prove we can execute sequential instructions
        core::arch::asm!(
            "mov dx, 0xE9",
            "mov al, 0x31",  // '1' in ASCII
            "out dx, al",
            options(nomem, nostack)
        );
    }

    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  KERNEL MODE - Testing Interrupts                       ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    // CRITICAL: Initialize PMM first (needed for stack allocation)
    rustux::init::pmm_init();

    // Build the kernel's own page tables (higher-half direct map of physical
    // memory) before anything allocates the kernel stack or heap, so those
    // live at addresses that are valid in every address space.
    unsafe {
        use rustux::arch::amd64::mm::kernel_map;
        match kernel_map::init(|| rustux::mm::pmm::pmm_alloc_kernel_page().ok()) {
            Ok(()) => {
                debug_print("[VM] Kernel page tables active, physical memory mapped at 0x");
                print_hex(kernel_map::HHDM_BASE);
                debug_print("\n");
            }
            Err(e) => {
                debug_print("[VM] FATAL: ");
                debug_print(e);
                debug_print("\n");
                loop { asm!("hlt"); }
            }
        }
    }

    // Move the kernel image to the top 2 GiB (KERNEL_VBASE). Code keeps
    // running at the load address until the jump below; from then on
    // nothing returns into the old frames.
    let delta = unsafe {
        use rustux::arch::amd64::mm::kernel_map;
        match kernel_map::relocate_kernel(|| rustux::mm::pmm::pmm_alloc_kernel_page().ok()) {
            Ok(d) => {
                debug_print("[VM] Kernel image relocated to 0x");
                print_hex(kernel_map::KERNEL_VBASE);
                debug_print("\n");
                d
            }
            Err(e) => {
                debug_print("[VM] FATAL: ");
                debug_print(e);
                debug_print("\n");
                loop { asm!("hlt"); }
            }
        }
    };

    // The continuation's address at the new location. Taken RIP-relative
    // (the code is still at the load address) and moved by the delta.
    let continuation: u64;
    unsafe {
        asm!("lea {}, [rip + {f}]", out(reg) continuation, f = sym kernel_main_on_new_stack, options(nomem, nostack));
    }
    let continuation = continuation.wrapping_add(delta);

    // CRITICAL: Switch to proper kernel stack BEFORE any deep operations
    // The firmware stack is too small and causes corruption during ELF loading.
    // This function does NOT return - it jumps to kernel_main_on_new_stack()
    // in the relocated image.
    unsafe {
        rustux::arch::amd64::init::init_kernel_stack(continuation as usize);
    }
}

/// Continuation of kernel_main() - runs on the new kernel stack
/// This function is jumped to directly by init_kernel_stack(), it is never called normally.
extern "C" fn kernel_main_on_new_stack() -> ! {
    debug_print("[STACK] Now running on new kernel stack!\n");
    {
        let rip: u64;
        unsafe { asm!("lea {}, [rip]", out(reg) rip, options(nomem, nostack)) };
        debug_print("[VM] Running at 0x");
        print_hex(rip);
        debug_print("\n");
    }
    // Pointers the boot code stored at runtime before the move
    rustux::init::after_relocation();

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
    unsafe { install_exception_handlers(); }

    // The kernel's own GDT and IDT are loaded (the firmware's were in low
    // memory); nothing needs the boot identity map any more. Dropping it
    // makes any stray low-address access fault instead of silently working.
    unsafe { rustux::arch::amd64::mm::kernel_map::drop_identity_map(); }
    debug_print("[VM] Boot identity map removed\n");
    debug_print("      ✓ IDT configured\n");

    // Install timer handler
    debug_print("[3/5] Installing timer handler...\n");
    unsafe { idt::idt_set_gate(32, timer_handler as u64, 0x08, 0x8E); }
    debug_print("      ✓ Timer handler at vector 32\n");

    // Install keyboard handler
    debug_print("[3.5/5] Installing keyboard handler...\n");
    unsafe { idt::idt_set_gate(33, keyboard_handler as u64, 0x08, 0x8E); }
    debug_print("      ✓ Keyboard handler at vector 33\n");

    // Install syscall handler (int 0x80). Type 0xEE = present, DPL 3,
    // 64-bit interrupt gate: DPL 3 lets ring-3 code execute `int 0x80`.
    debug_print("[3.6/5] Installing syscall handler...\n");
    unsafe { idt::idt_set_gate(0x80, rustux_int80_entry as u64, 0x08, 0xEE); }
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
        let lapic = rustux::arch::amd64::mm::kernel_map::phys_to_virt(0xFEE0_0000) as usize;
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

    // USB keyboards (xHCI, hubs), after the console so the log is visible.
    // Timeouts need a calibrated TSC.
    let tsc_hz = rustux::arch::amd64::tsc::x86_calibrate_tsc();
    debug_print("[TSC] ");
    print_hex(tsc_hz);
    debug_print(" Hz\n");
    rustux::drivers::usb::init();

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

    // Try to load and execute init.elf from ramdisk (Phase 5D)
    debug_print("╔══════════════════════════════════════════════════════════╗\n");
    debug_print("║  PHASE 5D: Loading Init Process                         ║\n");
    debug_print("╚══════════════════════════════════════════════════════════╝\n\n");

    unsafe {
        use rustux::fs::ramdisk;
        use rustux::exec::load_elf_process;
        use rustux::process::table::{Process, PROCESS_TABLE};

        // Get the ramdisk
        let ramdisk = match ramdisk::get_ramdisk() {
            Ok(r) => r,
            Err(_) => {
                debug_print("[INIT] Ramdisk not available, skipping init load\n\n");
                // Continue without init process - halt
                loop { core::arch::asm!("hlt"); }
            }
        };

        // Look for init.elf in ramdisk
        let init_file = match ramdisk.find_file("bin/init") {
            Some(f) => f,
            None => {
                debug_print("[INIT] init.elf not found in ramdisk, skipping\n\n");
                // Continue without init process - halt
                loop { core::arch::asm!("hlt"); }
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
                // Continue without init process - halt
                loop { core::arch::asm!("hlt"); }
            }
        };

        debug_print("[INIT] ELF loaded successfully\n");
        debug_print("[INIT] Entry point: 0x");
        print_hex(process_image.entry);
        debug_print("\n");

        // Allocate init's kernel stack
        let kernel_stack_top = match rustux::process::run::alloc_kernel_stack() {
            Ok(top) => top,
            Err(e) => {
                debug_print("[INIT] ");
                debug_print(e);
                debug_print("\n");
                loop { core::arch::asm!("hlt"); }
            }
        };

        // Get page table physical address
        let page_table_phys = process_image.address_space.page_table.phys;

        // Create process with PID 1
        let mut process = Process::new(
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
        // Hand the CPU to the scheduler; init runs first. The boot context
        // is never resumed.
        rustux::process::run::start(1);

        // Unreachable
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

        // Send EOI to LAPIC (write 0 to EOI register at offset 0xB0)
        let lapic = rustux::arch::amd64::mm::kernel_map::phys_to_virt(0xFEE0_0000) as usize;
        write_volatile((lapic + 0xB0) as *mut u32, 0);
    }
}

/// Frame pushed by the CPU on an interrupt (what the x86-interrupt ABI
/// passes to the handler)
#[repr(C)]
pub struct InterruptFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

// ---------------------------------------------------------------------------
// CPU exceptions (vectors 0-31)
// ---------------------------------------------------------------------------

const EXCEPTION_NAMES: [&str; 21] = [
    "divide error", "debug", "NMI", "breakpoint", "overflow", "bound range",
    "invalid opcode", "device not available", "double fault", "coprocessor overrun",
    "invalid TSS", "segment not present", "stack fault", "general protection",
    "page fault", "reserved", "x87 error", "alignment check", "machine check",
    "SIMD error", "virtualization",
];

/// Report a CPU exception. User-mode faults kill the current process;
/// kernel-mode faults halt the machine.
fn handle_exception(vector: u64, code: Option<u64>, frame: &InterruptFrame) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    let name = EXCEPTION_NAMES.get(vector as usize).copied().unwrap_or("exception");
    let user = frame.cs & 3 == 3;
    let pid = rustux::process::table::PROCESS_TABLE.lock().current_pid().unwrap_or(0);

    // Debug port: full detail
    unsafe { DEBUG_ENABLED = true; }
    debug_print("\n[FAULT] ");
    debug_print(name);
    debug_print(" (vector 0x");
    print_hex(vector);
    debug_print(") in ");
    debug_print(if user { "user mode, pid 0x" } else { "KERNEL mode, pid 0x" });
    print_hex(pid as u64);
    debug_print(" rip=0x");
    print_hex(frame.rip);
    debug_print(" rsp=0x");
    print_hex(frame.rsp);
    if let Some(c) = code {
        debug_print(" error=0x");
        print_hex(c);
    }
    if vector == 14 {
        debug_print(" addr=0x");
        print_hex(cr2);
    }
    debug_print("\n");

    // Screen: one line for the user
    if rustux::drivers::display::is_initialized() {
        rustux::drivers::display::write_str("\n[");
        rustux::drivers::display::write_str(name);
        rustux::drivers::display::write_str(if user { ": process killed]\n" } else { ": kernel halted]\n" });
    }

    if user {
        rustux::process::run::exit_current();
    }
    loop {
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

macro_rules! exception_handler {
    ($name:ident, $vector:expr) => {
        extern "x86-interrupt" fn $name(frame: InterruptFrame) {
            handle_exception($vector, None, &frame);
        }
    };
    ($name:ident, $vector:expr, error_code) => {
        extern "x86-interrupt" fn $name(frame: InterruptFrame, code: u64) {
            handle_exception($vector, Some(code), &frame);
        }
    };
}

exception_handler!(exc_divide, 0);
exception_handler!(exc_debug, 1);
exception_handler!(exc_nmi, 2);
exception_handler!(exc_breakpoint, 3);
exception_handler!(exc_overflow, 4);
exception_handler!(exc_bound, 5);
exception_handler!(exc_invalid_opcode, 6);
exception_handler!(exc_no_device, 7);
exception_handler!(exc_double_fault, 8, error_code);
exception_handler!(exc_invalid_tss, 10, error_code);
exception_handler!(exc_segment, 11, error_code);
exception_handler!(exc_stack, 12, error_code);
exception_handler!(exc_gp, 13, error_code);
exception_handler!(exc_page_fault, 14, error_code);
exception_handler!(exc_x87, 16);
exception_handler!(exc_alignment, 17, error_code);
exception_handler!(exc_machine_check, 18);
exception_handler!(exc_simd, 19);

/// Install handlers for CPU exceptions 0-19
unsafe fn install_exception_handlers() {
    let handlers: [(u8, u64); 18] = [
        (0, exc_divide as u64),
        (1, exc_debug as u64),
        (2, exc_nmi as u64),
        (3, exc_breakpoint as u64),
        (4, exc_overflow as u64),
        (5, exc_bound as u64),
        (6, exc_invalid_opcode as u64),
        (7, exc_no_device as u64),
        (8, exc_double_fault as u64),
        (10, exc_invalid_tss as u64),
        (11, exc_segment as u64),
        (12, exc_stack as u64),
        (13, exc_gp as u64),
        (14, exc_page_fault as u64),
        (16, exc_x87 as u64),
        (17, exc_alignment as u64),
        (18, exc_machine_check as u64),
        (19, exc_simd as u64),
    ];
    for (vector, handler) in handlers {
        idt::idt_set_gate(vector, handler, 0x08, 0x8E);
    }
}

// Timer handler (Vector 32)
#[no_mangle]
pub extern "x86-interrupt" fn timer_handler(frame: InterruptFrame) {
    unsafe {
        let msg = b"[TICK]\n";
        for &b in msg {
            asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags));
        }
        let lapic = rustux::arch::amd64::mm::kernel_map::phys_to_virt(0xFEE0_0000) as usize;
        write_volatile((lapic + 0xB0) as *mut u32, 0);
    }
    // Preempt only user mode: kernel code may hold spin locks that the next
    // process would need.
    if frame.cs & 3 == 3 {
        rustux::process::run::yield_now();
    }
}

// int 0x80 system call entry
//
// ABI: syscall number in RAX, arguments in RDI, RSI, RDX, R10, R8, R9
// (the System V order documented in docs/SYSCALL.md);
// result returned in RAX. The CPU pushes SS/RSP/RFLAGS/CS/RIP (switching to
// TSS.RSP0 when coming from ring 3); the stub saves every general register,
// calls the Rust dispatcher with a pointer to them, and restores them with
// RAX replaced by the result. Interrupts are re-enabled while the syscall
// runs so that blocking calls (e.g. read on stdin) can receive keyboard IRQs.
core::arch::global_asm!(
    ".global rustux_int80_entry",
    "rustux_int80_entry:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "cld",
    // 5 CPU-pushed + 15 saved qwords keep RSP 16-byte aligned for the call
    "mov rdi, rsp",
    "sti",
    "call rustux_int80_dispatch",
    "cli",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
);

extern "C" {
    fn rustux_int80_entry();
}

/// Registers saved by `rustux_int80_entry` (lowest address first)
#[repr(C)]
struct Int80Regs {
    r15: u64, r14: u64, r13: u64, r12: u64, r11: u64, r10: u64, r9: u64, r8: u64,
    rbp: u64, rdi: u64, rsi: u64, rdx: u64, rcx: u64, rbx: u64, rax: u64,
}

#[no_mangle]
extern "sysv64" fn rustux_int80_dispatch(regs: &mut Int80Regs) {
    use rustux::syscall::{SyscallArgs, syscall_dispatch};

    let args = SyscallArgs::new(
        regs.rax as u32,
        [
            regs.rdi as usize,
            regs.rsi as usize,
            regs.rdx as usize,
            regs.r10 as usize,
            regs.r8 as usize,
            regs.r9 as usize,
        ],
    );
    regs.rax = syscall_dispatch(args) as u64;
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

/// Framebuffer information saved from GOP before ExitBootServices
static mut FRAMEBUFFER_ADDR: u64 = 0;
static mut FRAMEBUFFER_SIZE: u64 = 0; // bytes
static mut FRAMEBUFFER_WIDTH: usize = 0; // visible pixels
static mut FRAMEBUFFER_HEIGHT: usize = 0;
static mut FRAMEBUFFER_STRIDE: usize = 0; // pixels per scan line (>= width)
/// true when byte 0 of a pixel is blue (UEFI PixelBlueGreenRedReserved)
static mut FRAMEBUFFER_BLUE_FIRST: bool = true;

/// Query GOP once, while boot services are active, and save the
/// framebuffer address, geometry and pixel layout.
fn capture_framebuffer() {
    use uefi::boot;
    use uefi::proto::console::gop::{GraphicsOutput, PixelFormat as GopFormat};

    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        return;
    };
    let Ok(mut gop) = boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        return;
    };
    let mode = gop.current_mode_info();
    let blue_first = match mode.pixel_format() {
        GopFormat::Bgr => true,
        GopFormat::Rgb => false,
        // Bitmask / BltOnly: no linear 32-bit framebuffer we can draw to
        _ => return,
    };
    let (width, height) = mode.resolution();
    let mut fb = gop.frame_buffer();
    unsafe {
        FRAMEBUFFER_ADDR = fb.as_mut_ptr() as u64;
        FRAMEBUFFER_SIZE = fb.size() as u64;
        FRAMEBUFFER_WIDTH = width;
        FRAMEBUFFER_HEIGHT = height;
        FRAMEBUFFER_STRIDE = mode.stride();
        FRAMEBUFFER_BLUE_FIRST = blue_first;
    }
}

/// Record where UEFI loaded this image, so user address spaces can keep the
/// kernel mapped (call before ExitBootServices).
fn record_kernel_image() {
    use uefi::boot;
    use uefi::proto::loaded_image::LoadedImage;

    if let Ok(image) = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()) {
        let (base, size) = image.info();
        rustux::arch::amd64::mm::kernel_map::set_kernel_image(base as u64, size);
    }
}

/// Record what the kernel needs from the UEFI memory map:
/// - the highest physical address (RAM and MMIO) and the framebuffer, so
///   the direct map covers all of it;
/// - every free (`EfiConventionalMemory`) region, for the PMM.
///
/// Boot services code/data is not reclaimed yet: the firmware stack the
/// kernel is still running on lives there until the stack switch.
fn record_phys_limit(map: &impl uefi::mem::memory_map::MemoryMap) {
    use rustux::arch::amd64::mm::kernel_map::note_phys_limit;
    use uefi::mem::memory_map::MemoryType;
    for desc in map.entries() {
        note_phys_limit(desc.phys_start + desc.page_count * 4096);
        if desc.ty == MemoryType::CONVENTIONAL {
            rustux::mm::pmm::note_usable_memory(desc.phys_start, desc.page_count);
        }
    }
    unsafe {
        if FRAMEBUFFER_ADDR != 0 {
            note_phys_limit(FRAMEBUFFER_ADDR + FRAMEBUFFER_SIZE);
        }
    }
}

/// Fill the visible screen with one colour (32 bits per pixel).
/// Works before and after ExitBootServices; does nothing without a framebuffer.
fn fill_screen(r: u8, g: u8, b: u8) {
    unsafe {
        if FRAMEBUFFER_ADDR == 0 {
            return;
        }
        let pixel: u32 = if FRAMEBUFFER_BLUE_FIRST {
            (b as u32) | ((g as u32) << 8) | ((r as u32) << 16)
        } else {
            (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
        };
        let base = rustux::arch::amd64::mm::kernel_map::phys_to_virt(FRAMEBUFFER_ADDR) as *mut u32;
        let limit = FRAMEBUFFER_SIZE as usize / 4;
        for y in 0..FRAMEBUFFER_HEIGHT {
            for x in 0..FRAMEBUFFER_WIDTH {
                let i = y * FRAMEBUFFER_STRIDE + x;
                if i < limit {
                    write_volatile(base.add(i), pixel);
                }
            }
        }
    }
}

/// Fill framebuffer red - EFI entry point reached
fn fb_red() {
    fill_screen(0xFF, 0x00, 0x00);
}

/// Fill framebuffer green - ExitBootServices succeeded
fn fb_green() {
    fill_screen(0x00, 0xFF, 0x00);
}

/// Fill framebuffer blue - CR3 load succeeded (works after ExitBootServices)
#[no_mangle]
pub extern "C" fn fb_blue() {
    fill_screen(0x00, 0x00, 0xFF);
}

/// Get framebuffer address (for use after ExitBootServices)
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

    // GOP framebuffers are 32 bits per pixel; rows are `stride` pixels apart.
    // The display driver's PixelFormat::RGB stores blue in byte 0 (UEFI Bgr).
    let bpp = 32;
    let pitch = FRAMEBUFFER_STRIDE * 4;
    let format = if FRAMEBUFFER_BLUE_FIRST { PixelFormat::RGB } else { PixelFormat::BGR };

    // Draw through the direct map so the console also works while a user
    // address space is active (e.g. during sys_write).
    let framebuffer = Framebuffer::new(
        rustux::arch::amd64::mm::kernel_map::phys_to_virt(FRAMEBUFFER_ADDR),
        FRAMEBUFFER_WIDTH,
        FRAMEBUFFER_HEIGHT,
        pitch,
        bpp,
        format,
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
    fill_screen(0xFF, 0xFF, 0x00);
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
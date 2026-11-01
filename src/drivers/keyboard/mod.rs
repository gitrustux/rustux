// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! PS/2 Keyboard Driver
//!
//! This module provides a complete PS/2 keyboard driver with:
//! - Controller and device initialization
//! - Scancode to ASCII conversion
//! - Modifier key tracking (Shift, Ctrl, Alt, Caps Lock)
//! - Special key support (arrows, home, end, etc.)
//! - Circular buffer for keyboard events
//!
//! ## Hardware
//! - Data port: 0x60
//! - Command/status port: 0x64
//! - IRQ: IRQ1 (interrupt 33)
//!
//! ## Usage
//! ```rust
//! use rustux::drivers::keyboard;
//!
//! // Initialize keyboard (call from kernel init)
//! keyboard::init();
//!
//! // Read a character (blocking - returns None if no data)
//! if let Some(ch) = keyboard::read_char() {
//!     // Process character
//! }
//!
//! // Check for available data
//! if keyboard::has_data() {
//!     let ch = keyboard::read_char().unwrap();
//! }
//! ```

pub mod ps2;
pub mod layout;

use core::sync::atomic::{AtomicBool, Ordering};

// Re-exports
pub use layout::{
    KeyEvent, ModifierState, SpecialKey,
    scancode_to_keyevent,
};
pub use ps2::{
    CircularBuffer, INPUT_BUFFER_SIZE,
    PS2_DATA_PORT, PS2_CMD_PORT,
    controller_status, read_data_port,
};

/// Global input buffer for keyboard events
static mut INPUT_BUFFER: CircularBuffer<u8, INPUT_BUFFER_SIZE> = CircularBuffer::new();

/// Current modifier state
static mut MODIFIER_STATE: ModifierState = ModifierState::new();

/// Extended scancode flag (0xE0 prefix)
static mut EXTENDED_SCANCODE: bool = false;

/// Flag to track if keyboard has been initialized
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// An 8042 controller was found and set up (polling must not touch the
/// ports otherwise: a missing controller reads as 0xFF, "data waiting")
static PRESENT: AtomicBool = AtomicBool::new(false);

/// Initialize the PS/2 keyboard driver
///
/// This function performs full PS/2 controller and keyboard initialization:
/// 1. Checks if PS/2 controller is present
/// 2. Resets input buffer and modifier state
/// 3. Initializes PS/2 controller (enables IRQ1)
/// 4. Initializes keyboard device (enable scanning)
/// 5. Flushes stale keyboard data
///
/// If no PS/2 controller is present, prints [NO KEYBOARD] and halts.
///
/// # Safety
/// This function must be called only once during kernel initialization.
/// It should be called before enabling interrupts.
pub unsafe fn init() {
    // Check if PS/2 controller is present
    if !ps2::controller_present() {
        // No keyboard controller - print [NO KEYBOARD] and continue
        // PS/2 is optional - USB HID will be the path for modern systems
        crate::drivers::display::write_str("[NO KEYBOARD] - PS/2 absent, continuing without keyboard\n");
        // Mark as initialized but with no backend (allowing graceful degradation)
        INITIALIZED.store(true, Ordering::Release);
        return;
    }

    // PS/2 controller present - print [PS/2]
    crate::drivers::display::write_str("[PS/2] ");

    // Reset state
    INPUT_BUFFER = CircularBuffer::new();
    MODIFIER_STATE = ModifierState::new();
    EXTENDED_SCANCODE = false;

    // Set up the 8042 and the keyboard (prints a diagnostic line)
    ps2::ps2_controller_init();
    PRESENT.store(true, Ordering::Release);

    // CRITICAL: Flush any stale scan codes from keyboard buffer
    ps2::flush_output_buffer();

    // Additional thorough flush - clear any remaining stale data
    for _ in 0..256 {
        if controller_status() & ps2::STATUS_OBF == 0 {
            break; // Buffer is empty
        }
        let _ = read_data_port();
    }

    INITIALIZED.store(true, Ordering::Release);
}

/// Handle a keyboard interrupt (IRQ1)
///
/// This function is called from the IRQ1 interrupt handler.
/// It reads the scancode from the keyboard, converts it to ASCII,
/// and updates the modifier state.
///
/// # Safety
/// This function must only be called from an interrupt handler.
pub unsafe fn handle_irq() {
    // Read every byte waiting: the controller raises IRQ1 again only once
    // its output buffer has been emptied
    for _ in 0..64 {
        let status = controller_status();
        if status & ps2::STATUS_OBF == 0 {
            break;
        }
        let byte = read_data_port();
        if status & (ps2::STATUS_AUXDATA | ps2::STATUS_ERRORS) != 0 {
            continue; // mouse data or a transmission error
        }
        process_scancode(byte);
    }
    if INPUT_BUFFER.has_data() {
        wake_waiter();
    }
}

/// Process waiting for keyboard input (0 = none); woken by IRQ1
static WAITER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Register the process to wake when a key arrives
pub fn set_waiter(pid: u32) {
    WAITER.store(pid, Ordering::Release);
}

fn wake_waiter() {
    let pid = WAITER.load(Ordering::Acquire);
    if pid != 0 {
        crate::process::run::wake(pid);
    }
}

/// Polling fallback for when IRQ1 is not delivered (e.g. a firmware
/// routing the kernel does not handle): decode pending bytes directly.
pub fn poll() {
    unsafe {
        if !PRESENT.load(Ordering::Acquire) || controller_status() & ps2::STATUS_OBF == 0 {
            return;
        }
        let flags: u64;
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem));
        handle_irq();
        if flags & 0x200 != 0 {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

/// Decode one scancode (set 1, as delivered with translation on)
unsafe fn process_scancode(scancode: u8) {
    // Check for 0xE0 prefix (extended scancode)
    if scancode == 0xE0 {
        EXTENDED_SCANCODE = true;
        return;
    }

    let extended = EXTENDED_SCANCODE;
    EXTENDED_SCANCODE = false;

    // Process the scancode
    let keyevent = scancode_to_keyevent(scancode, &MODIFIER_STATE, extended);

    // Update modifier state and write to buffer
    match keyevent {
        KeyEvent::Ascii(ascii) => {
            // Regular ASCII character - write to buffer
            INPUT_BUFFER.write(ascii);
        }
        KeyEvent::Special(special) => {
            match special {
                // Modifier keys - update state
                SpecialKey::LeftShift => MODIFIER_STATE.left_shift = true,
                SpecialKey::RightShift => MODIFIER_STATE.right_shift = true,
                SpecialKey::LeftCtrl => MODIFIER_STATE.left_ctrl = true,
                SpecialKey::RightCtrl => MODIFIER_STATE.right_ctrl = true,
                SpecialKey::LeftAlt => MODIFIER_STATE.left_alt = true,
                SpecialKey::RightAlt => MODIFIER_STATE.right_alt = true,
                SpecialKey::CapsLock => {
                    MODIFIER_STATE.caps_lock = !MODIFIER_STATE.caps_lock;
                }
                // Backspace - write as control character
                SpecialKey::Backspace => {
                    INPUT_BUFFER.write(0x08);
                }
                // Enter - write as newline
                SpecialKey::Enter => {
                    INPUT_BUFFER.write(b'\n');
                }
                // Tab - write as tab character
                SpecialKey::Tab => {
                    INPUT_BUFFER.write(b'\t');
                }
                // Other special keys - for future use (arrows, etc.)
                _ => {
                    // Arrow keys and other special keys could be handled here
                    // For now, we ignore them or could write special escape sequences
                }
            }
        }
        KeyEvent::Release(code) => {
            // Key release - update modifier state
            match code {
                0x2A => MODIFIER_STATE.left_shift = false,
                0x36 => MODIFIER_STATE.right_shift = false,
                0x1D => {
                    // Need to distinguish left/right ctrl based on extended flag
                    if extended {
                        MODIFIER_STATE.right_ctrl = false;
                    } else {
                        MODIFIER_STATE.left_ctrl = false;
                    }
                }
                0x38 => {
                    // Need to distinguish left/right alt based on extended flag
                    if extended {
                        MODIFIER_STATE.right_alt = false;
                    } else {
                        MODIFIER_STATE.left_alt = false;
                    }
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------
// Kernel input channel
//
// Since step 4 the PS/2 keyboard is driven by a userspace program
// (`/bin/ps2kbd`). The USB keyboard driver still runs in the kernel: it is
// polled on the timer tick and its characters go out over a channel whose
// other end belongs to `init`, which forwards them to the shell.
// ---------------------------------------------------------------------

static KERNEL_INPUT: crate::sync::SpinMutex<Option<alloc::sync::Arc<crate::kobj::Channel>>> =
    crate::sync::SpinMutex::new(None);
static POLLING: AtomicBool = AtomicBool::new(false);

/// Create the kernel input channel and return the end for `init`
pub fn take_input_channel() -> alloc::sync::Arc<crate::kobj::Channel> {
    let (ours, theirs) = crate::kobj::Channel::new_pair();
    *KERNEL_INPUT.lock() = Some(ours);
    theirs
}

/// Start polling USB on the timer tick (call once the scheduler runs)
pub fn start_polling() {
    POLLING.store(true, Ordering::Release);
}

/// Timer tick: poll the USB keyboards and send what they typed. Runs in
/// the interrupt handler, which only fires in user mode or while the CPU
/// idles, so no kernel lock is held by the code it interrupted.
pub fn tick() {
    if !POLLING.load(Ordering::Acquire) {
        return;
    }
    crate::drivers::usb::poll();
    let mut data = alloc::vec::Vec::new();
    unsafe {
        while let Some(b) = INPUT_BUFFER.read() {
            data.push(b);
        }
    }
    if data.is_empty() {
        return;
    }
    if let Some(ch) = KERNEL_INPUT.lock().as_ref() {
        let _ = ch.write(crate::kobj::channel::Message { data, handles: alloc::vec::Vec::new() });
    }
}

/// Read a single character from the keyboard buffer
///
/// # Returns
/// * `Some(char)` - Character if available
/// * `None` - No character available
///
/// # Note
/// This function is non-blocking. Returns immediately if no data is available.
pub fn read_char() -> Option<char> {
    unsafe {
        INPUT_BUFFER.read().map(|b| b as char)
    }
}

/// Add a character from another keyboard (USB) to the input buffer.
/// Interrupts are disabled around the write: the IRQ1 handler writes the
/// same buffer.
pub fn push_input(c: u8) {
    unsafe {
        let flags: u64;
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem));
        INPUT_BUFFER.write(c);
        if flags & 0x200 != 0 {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

/// Check if keyboard data is available
///
/// # Returns
/// * `true` - At least one character is available
/// * `false` - Buffer is empty
pub fn has_data() -> bool {
    unsafe {
        INPUT_BUFFER.has_data()
    }
}

/// Get the current modifier state
///
/// # Returns
/// Current modifier state (shift, ctrl, alt, caps lock)
pub fn get_modifiers() -> ModifierState {
    unsafe {
        MODIFIER_STATE
    }
}

/// Flush the input buffer (discard all pending characters)
pub fn flush() {
    unsafe {
        while INPUT_BUFFER.read().is_some() {}
    }
}

/// Get the number of characters available in the buffer
pub fn available() -> usize {
    unsafe {
        INPUT_BUFFER.available()
    }
}

/// Check if the buffer is full
pub fn is_full() -> bool {
    unsafe {
        INPUT_BUFFER.is_full()
    }
}

/// Check if keyboard driver has been initialized
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialized_flag() {
        assert!(!is_initialized());
    }

    #[test]
    fn test_modifiers_initial_state() {
        unsafe {
            assert!(!MODIFIER_STATE.shift());
            assert!(!MODIFIER_STATE.ctrl());
            assert!(!MODIFIER_STATE.alt());
        }
    }
}

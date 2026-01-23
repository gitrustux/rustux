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

/// Initialize the PS/2 keyboard driver
///
/// This function performs full PS/2 controller and keyboard initialization:
/// 1. Resets input buffer and modifier state
/// 2. Initializes PS/2 controller (enables IRQ1)
/// 3. Initializes keyboard device (enable scanning)
/// 4. Flushes stale keyboard data
///
/// # Safety
/// This function must be called only once during kernel initialization.
/// It should be called before enabling interrupts.
pub unsafe fn init() {
    // Reset state
    INPUT_BUFFER = CircularBuffer::new();
    MODIFIER_STATE = ModifierState::new();
    EXTENDED_SCANCODE = false;

    // Initialize PS/2 controller
    ps2::ps2_controller_init();

    // Initialize keyboard device
    ps2::ps2_keyboard_init();

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
    // Check controller status first
    let status = controller_status();

    // Bit 0: output buffer full
    // Bit 5: mouse data (ignore)
    if status & ps2::STATUS_OBF == 0 {
        return; // No data available
    }

    // Ignore mouse data (bit 5 set)
    if status & ps2::STATUS_AUXDATA != 0 {
        let _ = read_data_port(); // Flush and ignore
        return;
    }

    // Read scancode from data port
    let scancode = read_data_port();

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

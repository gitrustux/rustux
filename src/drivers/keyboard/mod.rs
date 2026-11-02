// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Keyboard support left in the kernel.
//!
//! Since step 4 the PS/2 keyboard is driven by a userspace program
//! (`/bin/ps2kbd`); the kernel only sets up the 8042 controller at boot.
//! The USB keyboard driver is still in the kernel: it is polled on the
//! timer tick, its characters collect in a buffer here, and they go out
//! over the kernel input channel, whose other end belongs to `init`.
//! The scancode tables in `layout` are kept for reference and tests.
//!
//! ## Hardware
//! - Data port: 0x60
//! - Command/status port: 0x64
//! - IRQ: IRQ1 (interrupt 33)
//!
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

/// USB keystrokes waiting to be sent, and the kernel's end of the input
/// channel. Locked with interrupts off: the timer tick uses it.
struct Input {
    buffer: CircularBuffer<u8, INPUT_BUFFER_SIZE>,
    channel: Option<alloc::sync::Arc<crate::kobj::Channel>>,
}

static INPUT: crate::sync::IrqMutex<Input> =
    crate::sync::IrqMutex::new(Input { buffer: CircularBuffer::new(), channel: None });

/// Flag to track if keyboard has been initialized
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// An 8042 controller was found and set up
static PRESENT: AtomicBool = AtomicBool::new(false);

/// Set once the scheduler runs: from then on the timer tick polls USB
static POLLING: AtomicBool = AtomicBool::new(false);

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
    INPUT.lock().buffer = CircularBuffer::new();

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

/// Create the kernel input channel and return the end for `init`
pub fn take_input_channel() -> alloc::sync::Arc<crate::kobj::Channel> {
    let (ours, theirs) = crate::kobj::Channel::new_pair();
    INPUT.lock().channel = Some(ours);
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
    let mut input = INPUT.lock();
    let mut data = alloc::vec::Vec::new();
    while let Some(b) = input.buffer.read() {
        data.push(b);
    }
    if data.is_empty() {
        return;
    }
    if let Some(ch) = input.channel.as_ref() {
        let _ = ch.write(crate::kobj::channel::Message { data, handles: alloc::vec::Vec::new() });
    }
}

/// Add a character from a USB keyboard
pub fn push_input(c: u8) {
    INPUT.lock().buffer.write(c);
}

/// Check if keyboard driver has been initialized
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// Whether an 8042 controller was found at boot
pub fn ps2_present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialized_flag() {
        assert!(!is_initialized());
    }
}

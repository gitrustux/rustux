// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! PS/2 Keyboard Driver
//!
//! This module provides a PS/2 keyboard driver with proper controller
//! and device initialization. It handles scan codes from the keyboard
//! and converts them to ASCII characters.
//!
//! ## Hardware
//! - Data port: 0x60
//! - Command/status port: 0x64
//! - IRQ: IRQ1 (interrupt 33)
//!
//! ## Initialization
//! 1. PS/2 Controller initialization (port 0x64)
//!    - Disable keyboard (0xAD)
//!    - Flush output buffer
//!    - Read config (0x20)
//!    - Enable IRQ1 (bit 0)
//!    - Write config (0x60)
//!    - Enable keyboard (0xAE)
//!
//! 2. Keyboard device initialization (port 0x60)
//!    - Disable scanning (0xF5)
//!    - Wait for ACK (0xFA)
//!    - Enable scanning (0xF4)
//!    - Wait for ACK (0xFA)

/// Keyboard data port (for reading data and sending device commands)
const KEYBOARD_DATA_PORT: u16 = 0x60;

/// Keyboard command/status port (for controller commands and status)
const KEYBOARD_COMMAND_PORT: u16 = 0x64;

/// PS/2 Controller commands
const CMD_DISABLE_KEYBOARD: u8 = 0xAD;
const CMD_ENABLE_KEYBOARD: u8 = 0xAE;
const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;

/// Keyboard device commands
const KBD_DISABLE_SCANNING: u8 = 0xF5;
const KBD_ENABLE_SCANNING: u8 = 0xF4;
const KBD_ACK: u8 = 0xFA;

/// Input buffer size (fixed, no heap)
const INPUT_BUFFER_SIZE: usize = 256;

/// Circular input buffer
struct InputBuffer {
    data: [u8; INPUT_BUFFER_SIZE],
    read_pos: usize,
    write_pos: usize,
}

impl InputBuffer {
    const fn new() -> Self {
        Self {
            data: [0; INPUT_BUFFER_SIZE],
            read_pos: 0,
            write_pos: 0,
        }
    }

    /// Write a byte to the buffer
    fn write(&mut self, byte: u8) -> bool {
        let next_pos = (self.write_pos + 1) % INPUT_BUFFER_SIZE;

        // Check if buffer is full
        if next_pos == self.read_pos {
            return false; // Buffer full
        }

        self.data[self.write_pos] = byte;
        self.write_pos = next_pos;
        true
    }

    /// Read a byte from the buffer
    fn read(&mut self) -> Option<u8> {
        if self.read_pos == self.write_pos {
            return None; // Buffer empty
        }

        let byte = self.data[self.read_pos];
        self.read_pos = (self.read_pos + 1) % INPUT_BUFFER_SIZE;
        Some(byte)
    }

    /// Check if buffer has data
    fn has_data(&self) -> bool {
        self.read_pos != self.write_pos
    }
}

/// Global input buffer
static mut INPUT_BUFFER: InputBuffer = InputBuffer::new();

/// Shift state
static mut SHIFT_PRESSED: bool = false;

/// US QWERTY scan code set 1 to ASCII translation table
const SCAN_CODE_TO_ASCII: &[u8; 128] = &[
    0,      // 0x00: Unknown
    0,      // 0x01: Esc (ignored for now)
    b'1',   // 0x02: 1
    b'2',   // 0x03: 2
    b'3',   // 0x04: 3
    b'4',   // 0x05: 4
    b'5',   // 0x06: 5
    b'6',   // 0x07: 6
    b'7',   // 0x08: 7
    b'8',   // 0x09: 8
    b'9',   // 0x0A: 9
    b'0',   // 0x0B: 0
    b'-',   // 0x0C: -
    b'=',   // 0x0D: =
    b'\x08', // 0x0E: Backspace
    b'\t',  // 0x0F: Tab
    b'q',   // 0x10: Q
    b'w',   // 0x11: W
    b'e',   // 0x12: E
    b'r',   // 0x13: R
    b't',   // 0x14: T
    b'y',   // 0x15: Y
    b'u',   // 0x16: U
    b'i',   // 0x17: I
    b'o',   // 0x18: O
    b'p',   // 0x19: P
    b'[',   // 0x1A: [
    b']',   // 0x1B: ]
    b'\n',  // 0x1C: Enter
    0,      // 0x1D: Left Ctrl (ignored)
    b'a',   // 0x1E: A
    b's',   // 0x1F: S
    b'd',   // 0x20: D
    b'f',   // 0x21: F
    b'g',   // 0x22: G
    b'h',   // 0x23: H
    b'j',   // 0x24: J
    b'k',   // 0x25: K
    b'l',   // 0x26: L
    b';',   // 0x27: ;
    b'\'',  // 0x28: '
    b'`',   // 0x29: `
    0,      // 0x2A: Left Shift (ignored)
    b'\\',  // 0x2B: \
    b'z',   // 0x2C: Z
    b'x',   // 0x2D: X
    b'c',   // 0x2E: C
    b'v',   // 0x2F: V
    b'b',   // 0x30: B
    b'n',   // 0x31: N
    b'm',   // 0x32: M
    b',',   // 0x33: ,
    b'.',   // 0x34: .
    b'/',   // 0x35: /
    0,      // 0x36: Right Shift (ignored)
    0,      // 0x37: Print Screen (ignored)
    0,      // 0x38: Alt (ignored)
    b' ',   // 0x39: Space
    0,      // 0x3A: Caps Lock (ignored)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x3B-0x44: F1-F10 (ignored)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x45-0x4E: Various (ignored)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x4F-0x58: Various (ignored)
    0,      // 0x59
    0,      // 0x5A
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x5B-0x64: Various (ignored)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x65-0x6E: Various (ignored)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x6F-0x78: Various (ignored)
    0, 0, 0, 0, 0, 0, 0, // 0x79-0x7F: Various (ignored)
];

// =============================================================
// PS/2 CONTROLLER HELPER FUNCTIONS
// =============================================================

/// Read controller status register (port 0x64)
unsafe fn controller_status() -> u8 {
    let status: u8;
    core::arch::asm!(
        "in al, dx",
        inlateout("dx") KEYBOARD_COMMAND_PORT => _,
        out("al") status,
        options(nomem, nostack)
    );
    status
}

/// Write byte to controller command port (0x64)
unsafe fn controller_write(cmd: u8) {
    // Wait for input buffer to be empty (bit 1 of status)
    let mut timeout = 100000;
    while timeout > 0 {
        let status = controller_status();
        if status & 0x02 == 0 {
            break; // Input buffer empty
        }
        timeout -= 1;
        for _ in 0..100 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }

    // Write command
    core::arch::asm!(
        "out dx, al",
        in("al") cmd,
        in("dx") KEYBOARD_COMMAND_PORT,
        options(nomem, nostack)
    );
}

/// Read byte from keyboard data port (0x60)
unsafe fn read_data_port() -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        inlateout("dx") KEYBOARD_DATA_PORT => _,
        out("al") value,
        options(nomem, nostack)
    );
    value
}

/// Write byte to keyboard data port (for device commands)
unsafe fn keyboard_write(cmd: u8) {
    // Wait for input buffer to be empty (bit 1 of status)
    let mut timeout = 100000;
    while timeout > 0 {
        let status = controller_status();
        if status & 0x02 == 0 {
            break; // Input buffer empty
        }
        timeout -= 1;
        for _ in 0..100 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }

    // Write command to data port
    core::arch::asm!(
        "out dx, al",
        in("al") cmd,
        in("dx") KEYBOARD_DATA_PORT,
        options(nomem, nostack)
    );
}

/// Flush output buffer (read any pending data)
///
/// CRITICAL: This flushes stale scan codes from the keyboard buffer.
/// If buffer has stale data (like repeated 0x20), all keys will show 'd'.
unsafe fn flush_output_buffer() {
    let mut count = 0;
    // Increased from 16 to 128 to handle more stale data
    while controller_status() & 0x01 != 0 && count < 128 {
        let _ = read_data_port();
        count += 1;
    }
}

/// Read byte from keyboard with timeout (returns None on timeout)
unsafe fn keyboard_read_timeout() -> Option<u8> {
    let mut timeout = 100000;
    while timeout > 0 {
        let status = controller_status();
        if status & 0x01 != 0 {
            return Some(read_data_port());
        }
        timeout -= 1;
        for _ in 0..100 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
    None
}

// =============================================================
// PS/2 CONTROLLER INITIALIZATION
// =============================================================

/// Initialize PS/2 controller for keyboard operation
unsafe fn ps2_controller_init() {
    // 1. Disable keyboard port
    controller_write(CMD_DISABLE_KEYBOARD);

    // 2. Flush any pending data from output buffer
    flush_output_buffer();

    // 3. Read controller configuration byte
    controller_write(CMD_READ_CONFIG);
    let config = if let Some(c) = keyboard_read_timeout() {
        c
    } else {
        // Default config if read fails
        0b0100_0001 // IRQ1 disabled, system flag set
    };

    // 4. Enable IRQ1 (bit 0) and clear IRQ2 (bit 1) for keyboard
    let new_config = config | 0x01; // Set bit 0 to enable IRQ1
    controller_write(CMD_WRITE_CONFIG);
    keyboard_write(new_config);

    // 5. Enable keyboard port
    controller_write(CMD_ENABLE_KEYBOARD);

    // Small delay to let commands take effect
    for _ in 0..10000 {
        core::arch::asm!("nop", options(nomem, nostack));
    }
}

/// Initialize PS/2 keyboard device
///
/// This performs the standard PS/2 keyboard initialization sequence:
/// 1. Disable scanning
/// 2. Wait for ACK
/// 3. Flush buffer
/// 4. Enable scanning
/// 5. Wait for ACK
unsafe fn ps2_keyboard_init() {
    // 1. Disable scanning
    keyboard_write(KBD_DISABLE_SCANNING);

    // 2. Wait for ACK (0xFA)
    if let Some(resp) = keyboard_read_timeout() {
        if resp != KBD_ACK {
            // No ACK, but continue anyway - keyboard may not respond properly
        }
    }

    // 3. Flush any remaining data AND clear any stale scan codes
    // This is CRITICAL - stale scan codes cause the "all keys show 'd'" bug
    flush_output_buffer();

    // Additional flush - read up to 256 times to ensure buffer is completely clear
    for _ in 0..256 {
        if controller_status() & 0x01 == 0 {
            break; // Buffer is empty
        }
        let _ = read_data_port();
    }

    // 4. Enable scanning
    keyboard_write(KBD_ENABLE_SCANNING);

    // 5. Wait for ACK (0xFA)
    if let Some(resp) = keyboard_read_timeout() {
        if resp != KBD_ACK {
            // No ACK, but continue anyway - keyboard may still work
        }
    }

    // 6. Give keyboard time to stabilize after enabling scanning
    // This ensures IRQs start working properly
    for _ in 0..100000 {
        core::arch::asm!("nop", options(nomem, nostack));
    }
}

// =============================================================
// PUBLIC API
// =============================================================

/// Initialize the PS/2 keyboard driver
///
/// This function performs full PS/2 controller and keyboard initialization:
/// 1. Resets input buffer and shift state
/// 2. Initializes PS/2 controller (enables IRQ1)
/// 3. Initializes keyboard device (enables scanning)
/// 4. Flushes stale keyboard data
pub fn init() {
    unsafe {
        // Reset state
        INPUT_BUFFER = InputBuffer::new();
        SHIFT_PRESSED = false;

        // Initialize PS/2 controller
        ps2_controller_init();

        // Initialize keyboard device
        ps2_keyboard_init();

        // CRITICAL: Flush any stale scan codes from keyboard buffer
        // This prevents the "all keys show 'd'" bug on boot
        flush_output_buffer();

        // Additional thorough flush - clear any remaining stale data
        for _ in 0..256 {
            if controller_status() & 0x01 == 0 {
                break; // Buffer is empty
            }
            let _ = read_data_port();
        }
    }
}

/// Handle a scan code (unified decode path for IRQ and polling)
///
/// This function processes a single scan code and converts it to ASCII.
/// It handles both make codes (key press) and break codes (key release).
unsafe fn handle_scancode(scan_code: u8) {
    // Check if this is a release code (0x80 prefix)
    if scan_code & 0x80 != 0 {
        // Release code - extract the actual scan code
        let make_code = scan_code & 0x7F;

        // Check if shift is being released
        if make_code == 0x2A || make_code == 0x36 {
            SHIFT_PRESSED = false;
        }
    } else {
        // Make code - key press
        // Check if shift is being pressed
        if scan_code == 0x2A || scan_code == 0x36 {
            SHIFT_PRESSED = true;
            return;
        }

        // Convert scan code to ASCII
        if (scan_code as usize) < SCAN_CODE_TO_ASCII.len() {
            let mut ascii = SCAN_CODE_TO_ASCII[scan_code as usize];

            // Apply shift for letters (convert to uppercase)
            if SHIFT_PRESSED && ascii >= b'a' && ascii <= b'z' {
                ascii -= 32; // Convert to uppercase
            }

            // Ignore null bytes (unsupported keys)
            if ascii != 0 {
                // Write to input buffer for shell
                INPUT_BUFFER.write(ascii);
            }
        }
    }
}

/// IRQ1 keyboard interrupt handler
///
/// This function is called by the IDT interrupt handler for IRQ1.
/// It reads the scan code from the keyboard and converts it to ASCII.
#[no_mangle]
pub extern "C" fn keyboard_irq_handler() {
    unsafe {
        // === IRQ1 PROOF: Write visible marker to VGA ===
        // This proves the IRQ handler is being called even if shell is blocked
        const VGA_BUFFER: u64 = 0xB8000;
        let vga = VGA_BUFFER as *mut u16;
        // Write 'K' (Keyboard) in green on black at column 79 (far right of top line)
        // Increment position each time to show activity
        static mut IRQ_COUNT: u8 = 0;
        IRQ_COUNT = IRQ_COUNT.wrapping_add(1);
        // Display count as hex digit 0-F at top-right corner
        let hex_digit = if IRQ_COUNT < 10 { b'0' + IRQ_COUNT } else { b'A' + IRQ_COUNT - 10 };
        *vga.add(79) = 0x0F00 | (hex_digit as u16); // White on black
        // === END IRQ1 PROOF ===

        // Check controller status first
        let status = controller_status();

        // Bit 0: output buffer full
        // Bit 5: mouse data (ignore)
        // Bit 6: timeout error (ignore)
        // Bit 7: parity error (ignore)
        if status & 0x01 == 0 {
            // No data available - shouldn't happen in IRQ handler
            pic_send_eoi();
            return;
        }

        // Ignore mouse data (bit 5 set)
        if status & 0x20 != 0 {
            pic_send_eoi();
            return;
        }

        // Read scan code from data port
        let scan_code = read_data_port();

        // Handle the scan code
        handle_scancode(scan_code);

        // Send EOI to PIC (CRITICAL!)
        pic_send_eoi();
    }
}

/// Send End of Interrupt (EOI) to the PIC
///
/// This is CRITICAL - without EOI, no further IRQs will be delivered.
unsafe fn pic_send_eoi() {
    // Send EOI to PIC1
    core::arch::asm!(
        "mov al, 0x20",
        "out 0x20, al",
        options(nomem, nostack)
    );
}

/// Read a single character from the keyboard buffer
///
/// # Returns
/// * `Some(char)` - Character if available
/// * `None` - No character available
pub fn read_char() -> Option<char> {
    unsafe {
        INPUT_BUFFER.read().map(|b| b as char)
    }
}

/// Read a line from the keyboard until Enter is pressed
///
/// This function blocks until Enter is pressed and returns the
/// input string (excluding the newline character).
///
/// It first tries IRQ-driven input, then falls back to polling
/// after a timeout if no IRQs are being received.
///
/// # Arguments
/// * `buffer` - Buffer to store the input string
///
/// # Returns
/// * Number of characters read (excluding newline)
pub fn read_line(buffer: &mut [u8]) -> usize {
    let mut count = 0;
    let mut empty_iterations = 0;
    let mut polling_mode = false;

    loop {
        if let Some(c) = read_char() {
            match c {
                '\n' => {
                    // Enter key - end of line
                    break;
                }
                '\x08' => {
                    // Backspace
                    if count > 0 {
                        count -= 1;
                    }
                }
                _ if count < buffer.len() => {
                    // Regular character
                    buffer[count] = c as u8;
                    count += 1;
                }
                _ => {
                    // Buffer full - ignore
                }
            }
            empty_iterations = 0;
        } else {
            empty_iterations += 1;

            // If we haven't received any input via IRQ after 50000 iterations,
            // switch to polling mode
            if !polling_mode && empty_iterations > 50000 {
                polling_mode = true;

                // Notify user that we're in polling mode
                crate::framebuffer::write_str_color("[POLLING] ", crate::framebuffer::colors::encode(crate::framebuffer::colors::YELLOW));
            }

            // In polling mode, directly read from keyboard hardware
            if polling_mode {
                if let Some(c) = unsafe { poll_read() } {
                    match c {
                        '\n' => break,
                        '\x08' => { if count > 0 { count -= 1; } }
                        _ if count < buffer.len() => {
                            buffer[count] = c as u8;
                            count += 1;
                        }
                        _ => {}
                    }
                    empty_iterations = 0;
                }
            }
        }

        // Small delay to prevent busy-waiting
        for _ in 0..1000 {
            unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
        }
    }

    count
}

/// Poll the keyboard directly (IRQ bypass)
///
/// This function polls the keyboard hardware for input, checking
/// the controller status and filtering out invalid data.
///
/// IMPORTANT: We do NOT flush on every read to avoid clearing data we need.
/// Only flush if we're seeing obviously invalid status.
unsafe fn poll_read() -> Option<char> {
    // Check controller status
    let status = controller_status();

    // Check if data is available (bit 0)
    if status & 0x01 == 0 {
        return None; // No data available yet
    }

    // Ignore mouse data (bit 5)
    if status & 0x20 != 0 {
        // This is mouse data, not keyboard - flush and ignore
        let _ = read_data_port();
        return None;
    }

    // Ignore error conditions (timeout or parity error)
    if status & 0xC0 != 0 {
        // Error - flush and ignore
        let _ = read_data_port();
        return None;
    }

    // Read scan code from data port
    let scan_code = read_data_port();

    // Check if this is a release code (bit 7 set)
    if scan_code & 0x80 == 0 {
        // Make code (key press) - convert to ASCII
        if (scan_code as usize) < SCAN_CODE_TO_ASCII.len() {
            let ascii = SCAN_CODE_TO_ASCII[scan_code as usize];

            // No shift handling in polling (keep it simple)
            if ascii != 0 {
                return Some(ascii as char);
            }
        }
    }

    // Break codes are ignored for polling (already handled above)

    None
}

/// Flush the input buffer
pub fn flush() {
    unsafe {
        while INPUT_BUFFER.read().is_some() {}
    }
}

// =============================================================
// DEBUGGING / NON-BLOCKING API
// =============================================================

/// Try to read a character from keyboard with direct hardware polling
///
/// This function bypasses the IRQ buffer and polls the hardware directly.
/// It returns immediately with None if no data is available.
///
/// This is useful for debugging IRQ issues - if this works but read_char()
/// doesn't, then IRQ1 is not firing.
pub fn try_read_char_direct() -> Option<char> {
    unsafe {
        // First, flush any stale data that might be stuck in the buffer
        flush_output_buffer();

        // Check controller status
        let status = controller_status();

        // Check if data is available (bit 0)
        if status & 0x01 == 0 {
            return None;
        }

        // Ignore mouse data (bit 5)
        if status & 0x20 != 0 {
            let _ = read_data_port(); // Flush and ignore
            return None;
        }

        // Ignore error conditions
        if status & 0xC0 != 0 {
            let _ = read_data_port(); // Flush and ignore
            return None;
        }

        // Read scan code
        let scan_code = read_data_port();

        // Only handle make codes (not break codes with 0x80 bit)
        if scan_code & 0x80 != 0 {
            return None;
        }

        // Convert to ASCII
        if (scan_code as usize) < SCAN_CODE_TO_ASCII.len() {
            let ascii = SCAN_CODE_TO_ASCII[scan_code as usize];
            if ascii != 0 {
                return Some(ascii as char);
            }
        }

        None
    }
}

/// Check if IRQ1 has ever fired (by checking the VGA counter)
///
/// This reads back the IRQ counter written by the keyboard IRQ handler.
/// Returns the number of IRQ1 interrupts that have fired.
pub fn get_irq_count() -> u8 {
    unsafe {
        const VGA_BUFFER: u64 = 0xB8000;
        let vga = VGA_BUFFER as *mut u16;
        let char_attr = *vga.add(79);
        (char_attr & 0xFF) as u8
    }
}

/// Read a line with a timeout (non-blocking)
///
/// This function reads keyboard input but returns after `max_iterations`
/// even if Enter is not pressed.
///
/// # Returns
/// * `Some(count)` - Number of characters read (may be 0)
/// * `None` - Timeout reached before Enter
pub fn read_line_timeout(buffer: &mut [u8], max_iterations: u64) -> Option<usize> {
    let mut count = 0;
    let mut iterations = 0;

    loop {
        iterations += 1;
        if iterations > max_iterations {
            // Timeout - return partial line
            return if count > 0 { Some(count) } else { None };
        }

        // Try IRQ buffer first
        if let Some(c) = read_char() {
            match c {
                '\n' => return Some(count),
                '\x08' => { if count > 0 { count -= 1; } }
                _ if count < buffer.len() => {
                    buffer[count] = c as u8;
                    count += 1;
                }
                _ => {}
            }
        } else {
            // Try direct hardware polling (IRQ might not be working)
            if let Some(c) = try_read_char_direct() {
                match c {
                    '\n' => return Some(count),
                    '\x08' => { if count > 0 { count -= 1; } }
                    _ if count < buffer.len() => {
                        buffer[count] = c as u8;
                        count += 1;
                    }
                    _ => {}
                }
            }
        }

        // Small delay
        for _ in 0..1000 {
            unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(KEYBOARD_DATA_PORT, 0x60);
        assert_eq!(KEYBOARD_COMMAND_PORT, 0x64);
        assert_eq!(INPUT_BUFFER_SIZE, 256);
    }

    #[test]
    fn test_scan_code_table_size() {
        assert_eq!(SCAN_CODE_TO_ASCII.len(), 128);
    }

    #[test]
    fn test_input_buffer_new() {
        let buffer = InputBuffer::new();
        assert!(!buffer.has_data());
        assert_eq!(buffer.available(), 0);
    }

    #[test]
    fn test_input_buffer_write_read() {
        let mut buffer = InputBuffer::new();
        assert!(buffer.write(b'A'));
        assert!(buffer.has_data());
        assert_eq!(buffer.available(), 1);
        assert_eq!(buffer.read(), Some(b'A'));
        assert!(!buffer.has_data());
    }
}

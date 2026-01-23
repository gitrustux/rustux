// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! PS/2 Keyboard Controller Driver
//!
//! This module provides low-level PS/2 keyboard controller support,
//! including initialization, port I/O, and interrupt handling.

/// PS/2 Keyboard Data Port (read scancodes, send device commands)
pub const PS2_DATA_PORT: u16 = 0x60;

/// PS/2 Keyboard Command/Status Port (controller commands, status)
pub const PS2_CMD_PORT: u16 = 0x64;

/// PS/2 Controller Commands
pub const CMD_DISABLE_KEYBOARD: u8 = 0xAD;
pub const CMD_ENABLE_KEYBOARD: u8 = 0xAE;
pub const CMD_READ_CONFIG: u8 = 0x20;
pub const CMD_WRITE_CONFIG: u8 = 0x60;

/// Keyboard Device Commands
pub const KBD_DISABLE_SCANNING: u8 = 0xF5;
pub const KBD_ENABLE_SCANNING: u8 = 0xF4;
pub const KBD_ACK: u8 = 0xFA;

/// Status register bits
pub const STATUS_OBF: u8 = 0x01; // Output buffer full
pub const STATUS_IBF: u8 = 0x02; // Input buffer full
pub const STATUS_AUXDATA: u8 = 0x20; // Mouse data

/// Input buffer size for circular buffer
pub const INPUT_BUFFER_SIZE: usize = 256;

/// Read controller status register (port 0x64)
#[inline]
pub unsafe fn controller_status() -> u8 {
    let status: u8;
    core::arch::asm!(
        "in al, dx",
        inlateout("dx") PS2_CMD_PORT => _,
        out("al") status,
        options(nomem, nostack)
    );
    status
}

/// Write command to controller command port (0x64)
pub unsafe fn controller_write(cmd: u8) {
    // Wait for input buffer to be empty (bit 1 of status)
    let mut timeout = 100_000;
    while timeout > 0 {
        let status = controller_status();
        if status & STATUS_IBF == 0 {
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
        in("dx") PS2_CMD_PORT,
        options(nomem, nostack)
    );
}

/// Read byte from keyboard data port (0x60)
#[inline]
pub unsafe fn read_data_port() -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        inlateout("dx") PS2_DATA_PORT => _,
        out("al") value,
        options(nomem, nostack)
    );
    value
}

/// Write byte to keyboard data port (for device commands)
pub unsafe fn write_data_port(cmd: u8) {
    // Wait for input buffer to be empty (bit 1 of status)
    let mut timeout = 100_000;
    while timeout > 0 {
        let status = controller_status();
        if status & STATUS_IBF == 0 {
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
        in("dx") PS2_DATA_PORT,
        options(nomem, nostack)
    );
}

/// Flush output buffer (read any pending data)
///
/// This is CRITICAL for flushing stale scan codes from the keyboard buffer.
/// If the buffer has stale data, all keys may show the same character.
pub unsafe fn flush_output_buffer() {
    let mut count = 0;
    while controller_status() & STATUS_OBF != 0 && count < 128 {
        let _ = read_data_port();
        count += 1;
    }
}

/// Read byte from keyboard with timeout (returns None on timeout)
pub unsafe fn keyboard_read_timeout() -> Option<u8> {
    let mut timeout = 100_000;
    while timeout > 0 {
        let status = controller_status();
        if status & STATUS_OBF != 0 {
            return Some(read_data_port());
        }
        timeout -= 1;
        for _ in 0..100 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }
    None
}

/// Initialize PS/2 controller for keyboard operation
pub unsafe fn ps2_controller_init() {
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

    // 4. Enable IRQ1 (bit 0) for keyboard interrupt generation
    let new_config = config | 0x01; // Set bit 0 to enable IRQ1
    controller_write(CMD_WRITE_CONFIG);
    write_data_port(new_config);

    // 5. Enable keyboard port
    controller_write(CMD_ENABLE_KEYBOARD);

    // Small delay to let commands take effect
    for _ in 0..10_000 {
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
pub unsafe fn ps2_keyboard_init() {
    // 1. Disable scanning
    write_data_port(KBD_DISABLE_SCANNING);

    // 2. Wait for ACK (0xFA)
    if let Some(resp) = keyboard_read_timeout() {
        if resp != KBD_ACK {
            // No ACK, but continue anyway - keyboard may not respond properly
        }
    }

    // 3. Flush any remaining data AND clear any stale scan codes
    // This is CRITICAL - stale scan codes cause keyboard bugs
    flush_output_buffer();

    // Additional flush - read up to 256 times to ensure buffer is completely clear
    for _ in 0..256 {
        if controller_status() & STATUS_OBF == 0 {
            break; // Buffer is empty
        }
        let _ = read_data_port();
    }

    // 4. Enable scanning
    write_data_port(KBD_ENABLE_SCANNING);

    // 5. Wait for ACK (0xFA)
    if let Some(resp) = keyboard_read_timeout() {
        if resp != KBD_ACK {
            // No ACK, but continue anyway - keyboard may still work
        }
    }

    // 6. Give keyboard time to stabilize after enabling scanning
    // This ensures IRQs start working properly
    for _ in 0..100_000 {
        core::arch::asm!("nop", options(nomem, nostack));
    }
}

/// Circular input buffer for keyboard events
pub struct CircularBuffer<T, const N: usize> {
    data: [T; N],
    read_pos: usize,
    write_pos: usize,
}

impl<T: Copy, const N: usize> CircularBuffer<T, N> {
    pub const fn new() -> Self {
        Self {
            data: [unsafe { core::mem::zeroed() }; N],
            read_pos: 0,
            write_pos: 0,
        }
    }

    /// Write a value to the buffer
    pub fn write(&mut self, value: T) -> bool {
        let next_pos = (self.write_pos + 1) % N;

        // Check if buffer is full
        if next_pos == self.read_pos {
            return false; // Buffer full
        }

        self.data[self.write_pos] = value;
        self.write_pos = next_pos;
        true
    }

    /// Read a value from the buffer
    pub fn read(&mut self) -> Option<T> {
        if self.read_pos == self.write_pos {
            return None; // Buffer empty
        }

        let value = self.data[self.read_pos];
        self.read_pos = (self.read_pos + 1) % N;
        Some(value)
    }

    /// Check if buffer has data
    pub fn has_data(&self) -> bool {
        self.read_pos != self.write_pos
    }

    /// Get number of available items
    pub fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            N - self.read_pos + self.write_pos
        }
    }

    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        (self.write_pos + 1) % N == self.read_pos
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(PS2_DATA_PORT, 0x60);
        assert_eq!(PS2_CMD_PORT, 0x64);
        assert_eq!(INPUT_BUFFER_SIZE, 256);
    }

    #[test]
    fn test_circular_buffer_new() {
        let buf: CircularBuffer<u8, 16> = CircularBuffer::new();
        assert!(!buf.has_data());
        assert_eq!(buf.available(), 0);
        assert!(!buf.is_full());
    }

    #[test]
    fn test_circular_buffer_write_read() {
        let mut buf: CircularBuffer<u8, 16> = CircularBuffer::new();
        assert!(buf.write(42));
        assert!(buf.has_data());
        assert_eq!(buf.available(), 1);
        assert_eq!(buf.read(), Some(42));
        assert!(!buf.has_data());
    }

    #[test]
    fn test_circular_buffer_wraparound() {
        let mut buf: CircularBuffer<u8, 4> = CircularBuffer::new();
        assert!(buf.write(1));
        assert!(buf.write(2));
        assert!(buf.write(3));
        assert_eq!(buf.read(), Some(1));
        assert_eq!(buf.read(), Some(2));
        assert!(buf.write(4));
        assert!(buf.write(5));
        assert_eq!(buf.read(), Some(3));
        assert_eq!(buf.read(), Some(4));
        assert_eq!(buf.read(), Some(5));
        assert!(!buf.has_data());
    }

    #[test]
    fn test_circular_buffer_full() {
        let mut buf: CircularBuffer<u8, 4> = CircularBuffer::new();
        assert!(buf.write(1));
        assert!(buf.write(2));
        assert!(buf.write(3));
        // Fourth write should fail because buffer is full
        assert!(!buf.is_full());
        // Actually, with size 4, we can write 3 items before full
        assert!(buf.write(4));
        // Now it's full
        assert!(buf.is_full());
        // This write should fail
        assert!(!buf.write(5));
    }

    #[test]
    fn test_circular_buffer_clear() {
        let mut buf: CircularBuffer<u8, 16> = CircularBuffer::new();
        assert!(buf.write(1));
        assert!(buf.write(2));
        assert!(buf.write(3));
        assert_eq!(buf.available(), 3);
        buf.clear();
        assert!(!buf.has_data());
        assert_eq!(buf.available(), 0);
    }
}

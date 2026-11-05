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
pub const STATUS_ERRORS: u8 = 0xC0; // Timeout / parity error

/// Controller commands used during setup
pub const CMD_DISABLE_AUX: u8 = 0xA7;
pub const CMD_SELF_TEST: u8 = 0xAA;
pub const CMD_TEST_KEYBOARD_PORT: u8 = 0xAB;

/// Configuration byte bits
const CFG_KBD_IRQ: u8 = 0x01;
const CFG_AUX_IRQ: u8 = 0x02;
const CFG_KBD_CLOCK_OFF: u8 = 0x10;
const CFG_AUX_CLOCK_OFF: u8 = 0x20;
const CFG_TRANSLATE: u8 = 0x40;

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

/// Check if PS/2 controller is present
///
/// Returns true if the 8042 controller responds.
/// A status of 0xFF indicates no controller is present.
pub unsafe fn controller_present() -> bool {
    let status = controller_status();
    status != 0xFF
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

fn hex2(v: Option<u8>) -> [u8; 2] {
    let h = b"0123456789ABCDEF";
    match v {
        Some(b) => [h[(b >> 4) as usize], h[(b & 0xF) as usize]],
        None => [b'-', b'-'],
    }
}

/// Send a controller command that returns one byte
unsafe fn command_with_reply(cmd: u8) -> Option<u8> {
    controller_write(cmd);
    keyboard_read_timeout()
}

/// Initialize the 8042 controller and keyboard.
///
/// Does not rely on the state the firmware left behind (on the test
/// laptop it leaves the keyboard clock disabled, configuration byte 0x75):
/// both ports off, flush, self-test, configuration byte with IRQ1 and
/// scancode translation on and the mouse port off, keyboard port test,
/// keyboard port on, scanning on. Prints one diagnostic line:
///
///   PS/2: test=55 cfg=75->65 port=00 kbd=FA
///
/// (55 = self-test passed, 00 = port test passed, FA = keyboard ACK; "--"
/// means no reply.) Call with interrupts disabled: the replies arrive on
/// the data port the IRQ handler reads.
pub unsafe fn ps2_controller_init() {
    controller_write(CMD_DISABLE_KEYBOARD);
    controller_write(CMD_DISABLE_AUX);
    flush_output_buffer();

    let cfg_before = command_with_reply(CMD_READ_CONFIG);

    // Some controllers reset the configuration byte during the self-test,
    // so the configuration is written afterwards
    let self_test = command_with_reply(CMD_SELF_TEST);

    let cfg = (cfg_before.unwrap_or(0) | CFG_KBD_IRQ | CFG_TRANSLATE | CFG_AUX_CLOCK_OFF)
        & !(CFG_AUX_IRQ | CFG_KBD_CLOCK_OFF);
    controller_write(CMD_WRITE_CONFIG);
    write_data_port(cfg);
    let cfg_after = command_with_reply(CMD_READ_CONFIG);

    let port_test = command_with_reply(CMD_TEST_KEYBOARD_PORT);

    controller_write(CMD_ENABLE_KEYBOARD);

    // Enable scanning; retry on "resend" (0xFE)
    let mut kbd = None;
    for _ in 0..3 {
        flush_output_buffer();
        write_data_port(KBD_ENABLE_SCANNING);
        kbd = keyboard_read_timeout();
        if kbd != Some(0xFE) {
            break;
        }
    }
    flush_output_buffer();

    let mut line = *b"PS/2: test=.. cfg=..->.. port=.. kbd=..\n";
    let fields = [(11, self_test), (18, cfg_before), (22, cfg_after), (30, port_test), (37, kbd)];
    for (pos, v) in fields {
        let h = hex2(v);
        line[pos] = h[0];
        line[pos + 1] = h[1];
    }
    crate::drivers::display::write_str(core::str::from_utf8_unchecked(&line));
    for &b in line.iter() {
        crate::klog::put(b);
    }
}

/// Keyboard device setup is part of [`ps2_controller_init`] (enable
/// scanning); kept for existing callers.
pub unsafe fn ps2_keyboard_init() {}

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
    /// Positions and data are shared between the IRQ handler and readers,
    /// so they are accessed with volatile reads/writes (otherwise a read
    /// loop may never see the handler's writes).
    pub fn write(&mut self, value: T) -> bool {
        unsafe {
            let write_pos = core::ptr::read_volatile(&self.write_pos);
            let next_pos = (write_pos + 1) % N;
            if next_pos == core::ptr::read_volatile(&self.read_pos) {
                return false; // Buffer full
            }
            core::ptr::write_volatile(&mut self.data[write_pos], value);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
            core::ptr::write_volatile(&mut self.write_pos, next_pos);
        }
        true
    }

    /// Read a value from the buffer
    pub fn read(&mut self) -> Option<T> {
        unsafe {
            let read_pos = core::ptr::read_volatile(&self.read_pos);
            if read_pos == core::ptr::read_volatile(&self.write_pos) {
                return None; // Buffer empty
            }
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);
            let value = core::ptr::read_volatile(&self.data[read_pos]);
            core::ptr::write_volatile(&mut self.read_pos, (read_pos + 1) % N);
            Some(value)
        }
    }

    /// Check if buffer has data
    pub fn has_data(&self) -> bool {
        unsafe { core::ptr::read_volatile(&self.read_pos) != core::ptr::read_volatile(&self.write_pos) }
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

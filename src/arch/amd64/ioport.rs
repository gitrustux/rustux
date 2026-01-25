// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86 I/O Port Management
//!
//! This module provides I/O port bitmap management for x86.
//!
//! On x86 systems, I/O ports are used to communicate with hardware devices.
//! The I/O permission bitmap is used by the kernel to control which ports
//! user-space processes can access.

/// I/O port bitmap size (8K ports = 1KB bitmap)
pub const IO_BITMAP_SIZE: usize = 0x1000;

/// Number of I/O ports
pub const IO_PORT_COUNT: usize = 0x1000;

/// I/O port bitmap
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoBitmap {
    /// Bitmap data - each bit represents one I/O port
    /// Bit set to 1 means port is allowed for user mode
    /// Bit set to 0 means port is kernel-only
    pub bitmap: [u8; IO_BITMAP_SIZE],
}

impl IoBitmap {
    /// Create a new I/O bitmap with all ports disabled (kernel-only)
    pub const fn new() -> Self {
        Self {
            bitmap: [0; IO_BITMAP_SIZE],
        }
    }

    /// Create a new I/O bitmap with all ports enabled (user-accessible)
    pub fn new_all_allowed() -> Self {
        Self {
            bitmap: [0xFF; IO_BITMAP_SIZE],
        }
    }

    /// Enable access to a specific I/O port
    ///
    /// # Arguments
    ///
    /// * `port` - The I/O port number (0-0xFFF)
    pub fn enable_port(&mut self, port: u16) {
        if port < IO_PORT_COUNT as u16 {
            let byte_index = port as usize / 8;
            let bit_index = port as usize % 8;
            self.bitmap[byte_index] |= 1 << bit_index;
        }
    }

    /// Disable access to a specific I/O port
    ///
    /// # Arguments
    ///
    /// * `port` - The I/O port number (0-0xFFF)
    pub fn disable_port(&mut self, port: u16) {
        if port < IO_PORT_COUNT as u16 {
            let byte_index = port as usize / 8;
            let bit_index = port as usize % 8;
            self.bitmap[byte_index] &= !(1 << bit_index);
        }
    }

    /// Check if access to a specific I/O port is enabled
    ///
    /// # Arguments
    ///
    /// * `port` - The I/O port number (0-0xFFF)
    ///
    /// # Returns
    ///
    /// true if the port is enabled for user access, false otherwise
    pub fn is_port_enabled(&self, port: u16) -> bool {
        if port >= IO_PORT_COUNT as u16 {
            return false;
        }
        let byte_index = port as usize / 8;
        let bit_index = port as usize % 8;
        (self.bitmap[byte_index] & (1 << bit_index)) != 0
    }

    /// Enable access to a range of I/O ports
    ///
    /// # Arguments
    ///
    /// * `start` - Starting port number
    /// * `end` - Ending port number (inclusive)
    pub fn enable_port_range(&mut self, start: u16, end: u16) {
        for port in start..=end {
            self.enable_port(port);
        }
    }

    /// Disable access to a range of I/O ports
    ///
    /// # Arguments
    ///
    /// * `start` - Starting port number
    /// * `end` - Ending port number (inclusive)
    pub fn disable_port_range(&mut self, start: u16, end: u16) {
        for port in start..=end {
            self.disable_port(port);
        }
    }

    /// Clear all port permissions (make all ports kernel-only)
    pub fn clear_all(&mut self) {
        self.bitmap = [0; IO_BITMAP_SIZE];
    }

    /// Enable all port permissions (make all ports user-accessible)
    pub fn enable_all(&mut self) {
        self.bitmap = [0xFF; IO_BITMAP_SIZE];
    }
}

impl Default for IoBitmap {
    fn default() -> Self {
        Self::new()
    }
}

/// ============================================================================
/// I/O Port Access Functions
/// ============================================================================

/// Read a byte from an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
///
/// # Safety
///
/// The port must be valid for the current hardware.
/// This function should typically only be called by drivers.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nomem, nostack)
    );
    value
}

/// Read a word (16 bits) from an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
///
/// # Safety
///
/// The port must be valid for the current hardware.
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    core::arch::asm!(
        "in ax, dx",
        out("ax") value,
        in("dx") port,
        options(nomem, nostack)
    );
    value
}

/// Read a double word (32 bits) from an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
///
/// # Safety
///
/// The port must be valid for the current hardware.
#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!(
        "in eax, dx",
        out("eax") value,
        in("dx") port,
        options(nomem, nostack)
    );
    value
}

/// Write a byte to an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
/// * `value` - The byte to write
///
/// # Safety
///
/// The port must be valid for the current hardware.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack)
    );
}

/// Write a word (16 bits) to an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
/// * `value` - The word to write
///
/// # Safety
///
/// The port must be valid for the current hardware.
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") value,
        options(nomem, nostack)
    );
}

/// Write a double word (32 bits) to an I/O port
///
/// # Arguments
///
/// * `port` - The I/O port address
/// * `value` - The double word to write
///
/// # Safety
///
/// The port must be valid for the current hardware.
#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack)
    );
}

/// ============================================================================
/// Common I/O Port Addresses
/// ============================================================================

/// PIC (Programmable Interrupt Controller) ports
pub mod pic {
    /// Master PIC command port
    pub const PIC1_CMD: u16 = 0x20;
    /// Master PIC data port
    pub const PIC1_DATA: u16 = 0x21;
    /// Slave PIC command port
    pub const PIC2_CMD: u16 = 0xA0;
    /// Slave PIC data port
    pub const PIC2_DATA: u16 = 0xA1;
}

/// PIT (Programmable Interval Timer) ports
pub mod pit {
    /// Channel 0 data port (system timer)
    pub const CHANNEL0: u16 = 0x40;
    /// Channel 1 data port (memory refresh)
    pub const CHANNEL1: u16 = 0x41;
    /// Channel 2 data port (PC speaker)
    pub const CHANNEL2: u16 = 0x42;
    /// Mode/Command register
    pub const MODE: u16 = 0x43;
}

/// COM (Serial port) ports
pub mod com {
    /// COM1 base address
    pub const COM1_BASE: u16 = 0x3F8;
    /// COM2 base address
    pub const COM2_BASE: u16 = 0x2F8;
    /// COM3 base address
    pub const COM3_BASE: u16 = 0x3E8;
    /// COM4 base address
    pub const COM4_BASE: u16 = 0x2E8;
}

/// Keyboard controller ports
pub mod keyboard {
    /// Keyboard data port
    pub const DATA: u16 = 0x60;
    /// Keyboard status/command register
    pub const STATUS: u16 = 0x64;
}

/// CMOS/RTC ports
pub mod cmos {
    /// CMOS index register
    pub const INDEX: u16 = 0x70;
    /// CMOS data register
    pub const DATA: u16 = 0x71;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_bitmap_new() {
        let bitmap = IoBitmap::new();
        // All ports should be disabled
        assert!(!bitmap.is_port_enabled(0x60));
        assert!(!bitmap.is_port_enabled(0x3F8));
    }

    #[test]
    fn test_io_bitmap_enable() {
        let mut bitmap = IoBitmap::new();
        bitmap.enable_port(0x60);
        assert!(bitmap.is_port_enabled(0x60));
        assert!(!bitmap.is_port_enabled(0x61));
    }

    #[test]
    fn test_io_bitmap_range() {
        let mut bitmap = IoBitmap::new();
        bitmap.enable_port_range(0x60, 0x64);
        for port in 0x60..=0x64 {
            assert!(bitmap.is_port_enabled(port));
        }
        assert!(!bitmap.is_port_enabled(0x65));
    }

    #[test]
    fn test_io_bitmap_disable() {
        let mut bitmap = IoBitmap::new_all_allowed();
        bitmap.disable_port(0x60);
        assert!(!bitmap.is_port_enabled(0x60));
        assert!(bitmap.is_port_enabled(0x61));
    }
}

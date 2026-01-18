// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! UART Driver for x86_64 (16550)
//!
//! This module provides a driver for the 16550 UART (and compatible variants)
//! commonly used on x86_64 systems for serial console I/O.
//!
//! # Usage
//!
//! ```ignore
//! use rustux::drivers::uart::Uart16550;
//!
//! // Initialize UART at COM1 (0x3F8)
//! let uart = unsafe { Uart16550::new(0x3F8) };
//! uart.init();
//!
//! // Write a string
//! uart.write_str("Hello, World!\n");
//! ```

use crate::arch::amd64::ioport::{inb, outb};

/// Base I/O port for COM1
pub const COM1_PORT: u16 = 0x3F8;

/// Base I/O port for COM2
pub const COM2_PORT: u16 = 0x2F8;

/// Base I/O port for COM3
pub const COM3_PORT: u16 = 0x3E8;

/// Base I/O port for COM4
pub const COM4_PORT: u16 = 0x2E8;

/// 16550 UART register offsets
mod reg {
    /// Receive buffer (read) / Transmit hold (write)
    pub const RBR_THR: u16 = 0;

    /// Interrupt enable
    pub const IER: u16 = 1;

    /// FIFO control
    pub const FCR: u16 = 2;

    /// Line control
    pub const LCR: u16 = 3;

    /// Modem control
    pub const MCR: u16 = 4;

    /// Line status
    pub const LSR: u16 = 5;

    /// Modem status
    pub const MSR: u16 = 6;

    /// Scratch register
    pub const SCR: u16 = 7;
}

/// Line Control Register bits
mod lcr {
    /// 8 bits per word
    pub const WLEN8: u8 = 0x03;

    /// Enable DLAB (Divisor Latch Access Bit)
    pub const DLAB: u8 = 0x80;
}

/// Line Status Register bits
mod lsr {
    /// Data ready (received byte available)
    pub const DR: u8 = 0x01;

    /// Transmitter hold register empty (ready to send)
    pub const THRE: u8 = 0x20;
}

/// FIFO Control Register bits
mod fcr {
    /// Enable FIFO
    pub const ENABLE: u8 = 0x01;

    /// Clear receive FIFO
    pub const CLEAR_RX: u8 = 0x02;

    /// Clear transmit FIFO
    pub const CLEAR_TX: u8 = 0x04;
}

/// 16550 UART driver
#[derive(Debug)]
pub struct Uart16550 {
    /// Base I/O port
    base_port: u16,
}

impl Uart16550 {
    /// Create a new UART driver for the given base port
    ///
    /// # Safety
    ///
    /// The base port must be valid and accessible.
    pub const unsafe fn new(base_port: u16) -> Self {
        Self { base_port }
    }

    /// Initialize the UART
    ///
    /// Configures the UART for:
    /// - 115200 baud
    /// - 8 data bits
    /// - No parity
    /// - 1 stop bit
    /// - FIFO enabled
    pub fn init(&self) {
        // Disable interrupts
        unsafe {
            outb(self.base_port + reg::IER, 0);
        }

        // Enable DLAB to set baud rate
        unsafe {
            outb(self.base_port + reg::LCR, lcr::DLAB);
        }

        // Set divisor for 115200 baud (assuming 1.8432 MHz clock)
        // Divisor = 1843200 / (16 * 115200) = 1
        unsafe {
            outb(self.base_port + reg::RBR_THR, 1); // Low byte
            outb(self.base_port + reg::IER, 0);     // High byte
        }

        // Configure: 8 bits, no parity, 1 stop bit, disable DLAB
        unsafe {
            outb(self.base_port + reg::LCR, lcr::WLEN8);
        }

        // Enable FIFO, clear buffers
        unsafe {
            outb(self.base_port + reg::FCR, 0x07); // Enable + clear RX + clear TX
        }

        // Set modem control (RTS + DTR)
        unsafe {
            outb(self.base_port + reg::MCR, 0x03);
        }
    }

    /// Write a single byte
    pub fn write_byte(&self, byte: u8) {
        // Wait for transmitter to be ready
        loop {
            let lsr = unsafe { inb(self.base_port + reg::LSR) };
            if lsr & lsr::THRE != 0 {
                break;
            }
        }

        // Write the byte
        unsafe {
            outb(self.base_port + reg::RBR_THR, byte);
        }
    }

    /// Read a single byte (blocking)
    pub fn read_byte(&self) -> u8 {
        // Wait for data to be available
        loop {
            let lsr = unsafe { inb(self.base_port + reg::LSR) };
            if lsr & lsr::DR != 0 {
                break;
            }
        }

        // Read the byte
        unsafe { inb(self.base_port + reg::RBR_THR) }
    }

    /// Check if data is available to read
    pub fn has_data(&self) -> bool {
        let lsr = unsafe { inb(self.base_port + reg::LSR) };
        lsr & lsr::DR != 0
    }

    /// Write a string
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }

    /// Get the base port
    pub const fn base_port(&self) -> u16 {
        self.base_port
    }
}

/// Global COM1 UART instance
///
/// This is initialized during kernel startup and used for console I/O.
static mut COM1: Option<Uart16550> = None;

/// Initialize the global COM1 UART
///
/// # Safety
///
/// Should only be called once during kernel initialization.
pub unsafe fn init_com1() {
    COM1 = Some(Uart16550::new(COM1_PORT));
    if let Some(ref uart) = COM1 {
        uart.init();
    }
}

/// Get a reference to the global COM1 UART
///
/// # Safety
///
/// Returns a mutable reference to global state.
pub unsafe fn com1() -> Option<&'static mut Uart16550> {
    COM1.as_mut()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uart_constants() {
        assert_eq!(COM1_PORT, 0x3F8);
        assert_eq!(COM2_PORT, 0x2F8);
    }

    #[test]
    fn test_uart_create() {
        // This test just verifies the struct can be created
        // Actual I/O would require hardware access
        let uart = unsafe { Uart16550::new(0x3F8) };
        assert_eq!(uart.base_port(), 0x3F8);
    }
}

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Generic interrupt handling
//!
//! This module provides architecture-independent interrupt handling,
//! using the architecture-specific InterruptController implementations.

use crate::traits::InterruptController;

/// Generic interrupt handler that can use any InterruptController implementation
pub struct InterruptHandler<C: InterruptController> {
    controller: C,
}

impl<C: InterruptController> InterruptHandler<C> {
    /// Create a new interrupt handler with the given controller
    pub fn new(controller: C) -> Self {
        Self {
            controller,
        }
    }

    /// Initialize the interrupt controller
    pub fn init(&mut self) -> Result<(), &'static str> {
        self.controller.init()
    }

    /// Enable an interrupt
    pub fn enable_irq(&mut self, irq: u64, vector: u64) {
        self.controller.enable_irq(irq, vector);
    }

    /// Disable an interrupt
    pub fn disable_irq(&mut self, irq: u64) {
        self.controller.disable_irq(irq);
    }

    /// Send end-of-interrupt signal
    pub fn send_eoi(&self, irq: u64) {
        self.controller.send_eoi(irq);
    }
}

/// Default interrupt handler type for x86_64
pub type X86_64InterruptHandler = InterruptHandler<crate::arch::X86_64InterruptController>;

/// Default interrupt handler type for ARM64
pub type Arm64InterruptHandler = InterruptHandler<crate::arch::Arm64InterruptController>;

/// Default interrupt handler type for RISC-V
pub type Riscv64InterruptHandler = InterruptHandler<crate::arch::Riscv64InterruptController>;

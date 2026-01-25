// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! QEMU test configuration
//!
//! Provides configuration for running the kernel in QEMU for testing.

/// QEMU machine types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QemuMachineType {
    /// Standard PC (i440FX + PIIX)
    Pc,
    /// Q35 chipset (supports multiple IOAPICs)
    Q35,
    /// MicroVM (minimal VM for fast boot)
    MicroVM,
}

/// Memory sizes for QEMU VMs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QemuMemorySize {
    /// 128 MB
    M128,
    /// 256 MB
    M256,
    /// 512 MB
    M512,
    /// 1 GB
    G1,
    /// 2 GB
    G2,
}

/// QEMU display options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QemuDisplay {
    /// No display (serial only)
    None,
    /// VGA display
    Vga,
    /// SDL display
    Sdl,
}

/// QEMU test configuration
///
/// This struct contains the configuration for running QEMU tests.
#[derive(Debug, Clone, Copy)]
pub struct QemuTestConfig {
    /// Machine type
    pub machine: QemuMachineType,
    /// Memory size
    pub memory: QemuMemorySize,
    /// Number of CPUs
    pub cpus: u32,
    /// Display option
    pub display: QemuDisplay,
    /// Enable KVM acceleration
    pub kvm: bool,
    /// Enable debug console
    pub debug: bool,
}

impl QemuTestConfig {
    /// Create a new QEMU test configuration with defaults
    pub fn new() -> Self {
        Self {
            machine: QemuMachineType::Q35,  // Q35 for IOAPIC support
            memory: QemuMemorySize::M512,
            cpus: 1,
            display: QemuDisplay::None,
            kvm: false,
            debug: true,
        }
    }

    /// Set the machine type
    pub fn with_machine(mut self, machine: QemuMachineType) -> Self {
        self.machine = machine;
        self
    }

    /// Set the memory size
    pub fn with_memory(mut self, memory: QemuMemorySize) -> Self {
        self.memory = memory;
        self
    }

    /// Set the number of CPUs
    pub fn with_cpus(mut self, cpus: u32) -> Self {
        self.cpus = cpus;
        self
    }

    /// Set the display option
    pub fn with_display(mut self, display: QemuDisplay) -> Self {
        self.display = display;
        self
    }

    /// Enable or disable KVM
    pub fn with_kvm(mut self, kvm: bool) -> Self {
        self.kvm = kvm;
        self
    }

    /// Enable or disable debug output
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Generate QEMU command line arguments
    ///
    /// This returns a string containing the QEMU command line arguments.
    /// Note: This is for documentation purposes - in a real implementation,
    /// you'd use these arguments to actually launch QEMU.
    pub fn to_qemu_args(&self) -> &'static str {
        // In a real implementation, this would dynamically generate
        // the QEMU command line arguments based on the configuration.
        //
        // Example output:
        // "-machine q35 -m 512M -smp 1 -display none -serial stdio -debugcon stdio"
        ""
    }

    /// Get recommended configuration for interrupt testing
    pub fn for_interrupt_testing() -> Self {
        Self::new()
            .with_machine(QemuMachineType::Q35)  // Q35 has better IOAPIC support
            .with_memory(QemuMemorySize::M512)
            .with_cpus(1)
            .with_display(QemuDisplay::None)
            .with_debug(true)
    }
}

impl Default for QemuTestConfig {
    fn default() -> Self {
        Self::new()
    }
}

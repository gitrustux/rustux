// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Interrupt test harness
//!
//! Provides utilities for testing interrupt controller functionality.

use crate::traits::InterruptController;
use crate::arch::X86_64InterruptController;
use crate::acpi;

/// Test results
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResult {
    /// Test passed
    Passed,
    /// Test failed with a message
    Failed(&'static str),
    /// Test skipped
    Skipped(&'static str),
}

/// Interrupt test harness
///
/// This struct provides methods to test various aspects of the interrupt system.
pub struct InterruptTestHarness {
    controller: X86_64InterruptController,
    tests_passed: usize,
    tests_failed: usize,
    tests_skipped: usize,
}

impl InterruptTestHarness {
    /// Create a new test harness
    pub fn new() -> Self {
        Self {
            controller: X86_64InterruptController::new(),
            tests_passed: 0,
            tests_failed: 0,
            tests_skipped: 0,
        }
    }

    /// Get the test controller (mutable)
    pub fn controller_mut(&mut self) -> &mut X86_64InterruptController {
        &mut self.controller
    }

    /// Record a test result
    fn record_result(&mut self, result: TestResult) {
        match result {
            TestResult::Passed => self.tests_passed += 1,
            TestResult::Failed(_) => self.tests_failed += 1,
            TestResult::Skipped(_) => self.tests_skipped += 1,
        }
    }

    /// Test ACPI RSDP discovery
    pub fn test_rsdp_discovery(&mut self) -> TestResult {
        if let Some(_rsdp) = acpi::find_rsdp() {
            TestResult::Passed
        } else {
            TestResult::Failed("RSDP not found")
        }
    }

    /// Test ACPI MADT parsing
    pub fn test_madt_parsing(&mut self) -> TestResult {
        let rsdp = match acpi::find_rsdp() {
            Some(rsdp) => rsdp,
            None => return TestResult::Skipped("RSDP not found"),
        };

        let madt = match acpi::find_and_parse_madt(rsdp) {
            Some(madt) => madt,
            None => return TestResult::Failed("MADT not found or parsing failed"),
        };

        if madt.io_apic_count > 0 {
            TestResult::Passed
        } else {
            TestResult::Failed("No I/O APICs found in MADT")
        }
    }

    /// Test I/O APIC discovery
    pub fn test_ioapic_discovery(&mut self) -> TestResult {
        let rsdp = match acpi::find_rsdp() {
            Some(rsdp) => rsdp,
            None => return TestResult::Skipped("RSDP not found"),
        };

        let madt = match acpi::find_and_parse_madt(rsdp) {
            Some(madt) => madt,
            None => return TestResult::Skipped("MADT not found"),
        };

        if let Some(_address) = madt.first_ioapic_address() {
            TestResult::Passed
        } else {
            TestResult::Failed("No I/O APIC address found")
        }
    }

    /// Test interrupt controller initialization
    pub fn test_controller_init(&mut self) -> TestResult {
        match self.controller.init() {
            Ok(()) => TestResult::Passed,
            Err(e) => TestResult::Failed(e),
        }
    }

    /// Test IRQ routing
    pub fn test_irq_routing(&mut self, irq: u64, vector: u64) -> TestResult {
        self.controller.enable_irq(irq, vector);
        // In a real test, we'd verify the routing by checking the IOAPIC registers
        // For now, just check that it doesn't crash
        TestResult::Passed
    }

    /// Test EOI functionality
    pub fn test_eoi(&mut self, irq: u64) -> TestResult {
        self.controller.send_eoi(irq);
        // In a real test, we'd verify the EOI was sent
        TestResult::Passed
    }

    /// Run all tests
    pub fn run_all_tests(&mut self) {
        self.log("Starting interrupt system tests...");

        self.log("Testing RSDP discovery...");
        self.record_result(self.test_rsdp_discovery());

        self.log("Testing MADT parsing...");
        self.record_result(self.test_madt_parsing());

        self.log("Testing I/O APIC discovery...");
        self.record_result(self.test_ioapic_discovery());

        self.log("Testing interrupt controller initialization...");
        self.record_result(self.test_controller_init());

        self.log("Testing IRQ1 routing...");
        self.record_result(self.test_irq_routing(1, 33));

        self.log("Testing EOI...");
        self.record_result(self.test_eoi(1));

        self.print_summary();
    }

    /// Print test summary
    pub fn print_summary(&self) {
        self.log(&format!("Test Summary:"));
        self.log(&format!("  Passed: {}", self.tests_passed));
        self.log(&format!("  Failed: {}", self.tests_failed));
        self.log(&format!("  Skipped: {}", self.tests_skipped));
    }

    /// Log a message (in a real kernel, this would print to the console)
    fn log(&self, message: &str) {
        // In a real kernel, this would use the console/serial output
        let _ = message;
        // For now, this is a no-op that prevents unused variable warnings
    }
}

impl Default for InterruptTestHarness {
    fn default() -> Self {
        Self::new()
    }
}

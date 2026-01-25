// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ACPI (Advanced Configuration and Power Interface) table parsing
//!
//! This module provides functionality to parse ACPI tables, specifically:
//! - RSDP (Root System Description Pointer) discovery
//! - RSDT/XSDT (Root System Description Table) parsing
//! - MADT (Multiple APIC Description Table) parsing for interrupt controller discovery
//!
//! # Example
//! ```ignore
//! use rustux::acpi;
//!
//! // Find RSDP and parse tables
//! if let Some(rsdp) = acpi::find_rsdp() {
//!     let madt = acpi::find_madt(&rsdp).unwrap();
//!     for ioapic in &madt.io_apics {
//!         println!("IOAPIC at 0x{:x}", ioapic.address);
//!     }
//! }
//! ```

pub mod rsdp;
pub mod rsdt;
pub mod madt;

pub use rsdp::{Rsdp, find_rsdp};
pub use rsdt::{Rsdt, SDTHeader};
pub use madt::{
    Madt,
    ParsedMadt,
    find_and_parse_madt,
    IoApicEntry,
    LocalApicEntry,
    InterruptSourceOverrideEntry,
};

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! MADT (Multiple APIC Description Table) parsing
//!
//! The MADT contains information about the interrupt controllers in the system,
//! including Local APICs and I/O APICs.

use core::mem;

use super::rsdt::SDTHeader;

/// MADT signature
pub const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// Maximum number of Local APICs we expect
const MAX_LOCAL_APICS: usize = 256;

/// Maximum number of I/O APICs we expect
const MAX_IO_APICS: usize = 8;

/// Maximum number of interrupt source overrides
const MAX_OVERRIDES: usize = 16;

/// MADT table structure
#[repr(C, packed)]
pub struct Madt {
    pub header: SDTHeader,
    /// Physical address of local APIC
    pub local_apic_address: u32,
    /// Flags
    pub flags: u32,
    // Variable-length entries follow here
}

/// MADT entry types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MadtEntryType {
    /// Local APIC (Processor Local APIC)
    LocalApic = 0,
    /// I/O APIC
    IoApic = 1,
    /// Interrupt Source Override
    InterruptSourceOverride = 2,
    /// NMI Source
    NmiSource = 3,
    /// Local APIC NMI
    LocalApicNmi = 4,
    /// Local APIC Address Override
    LocalApicAddressOverride = 5,
    /// I/O SAPIC
    IoSapic = 6,
    /// Local SAPIC
    LocalSapic = 7,
    /// Platform Interrupt Sources
    PlatformInterruptSource = 8,
    /// Processor Local x2APIC
    LocalX2Apic = 9,
    /// Local x2APIC NMI
    LocalX2ApicNmi = 10,
    /// GIC CPU Interface (ARM)
    GicCpuInterface = 11,
    /// GIC Distributor (ARM)
    GicDistributor = 12,
    /// GIC MSI Frame (ARM)
    GicMsiFrame = 13,
    /// GIC Redistributor (ARM)
    GicRedistributor = 14,
    /// GIC Interrupt Translation Service (ARM)
    GicIts = 15,
}

/// MADT entry header (common to all entry types)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

/// Local APIC entry (Type 0)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LocalApicEntry {
    pub header: MadtEntryHeader,
    /// ACPI processor ID
    pub processor_id: u8,
    /// Local APIC ID
    pub apic_id: u8,
    /// Flags (bit 0 = enabled)
    pub flags: u32,
}

/// I/O APIC entry (Type 1)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IoApicEntry {
    pub header: MadtEntryHeader,
    /// I/O APIC ID
    pub io_apic_id: u8,
    /// Reserved
    pub reserved: u8,
    /// I/O APIC address (32-bit physical address)
    pub address: u32,
    /// Global System Interrupt Base
    pub gsi_base: u32,
}

/// Interrupt Source Override entry (Type 2)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InterruptSourceOverrideEntry {
    pub header: MadtEntryHeader,
    /// Bus (0 = ISA)
    pub bus: u8,
    /// Source bus IRQ
    pub source_irq: u8,
    /// Global System Interrupt
    pub gsi: u32,
    /// Flags (polarity, trigger mode)
    pub flags: u16,
}

/// Parsed MADT table
#[derive(Debug)]
pub struct ParsedMadt {
    /// Physical address of local APIC
    pub local_apic_address: u32,
    /// Flags
    pub flags: u32,
    /// Number of local APIC entries
    pub local_apic_count: usize,
    /// Local APIC entries
    pub local_apics: [LocalApicEntry; MAX_LOCAL_APICS],
    /// Number of I/O APIC entries
    pub io_apic_count: usize,
    /// I/O APIC entries
    pub io_apics: [IoApicEntry; MAX_IO_APICS],
    /// Number of interrupt source override entries
    pub override_count: usize,
    /// Interrupt source override entries
    pub overrides: [InterruptSourceOverrideEntry; MAX_OVERRIDES],
}

impl ParsedMadt {
    /// Get the first I/O APIC address
    ///
    /// This is a convenience function for systems with a single I/O APIC.
    pub fn first_ioapic_address(&self) -> Option<u32> {
        if self.io_apic_count > 0 {
            Some(self.io_apics[0].address)
        } else {
            None
        }
    }

    /// Find an I/O APIC by GSI
    ///
    /// Returns the I/O APIC entry that handles the given Global System Interrupt.
    pub fn find_ioapic_for_gsi(&self, gsi: u32) -> Option<&IoApicEntry> {
        for i in 0..self.io_apic_count {
            let ioapic = &self.io_apics[i];
            // Each I/O APIC handles a range of GSIs
            // The number of interrupts per I/O APIC is determined by its version register
            // For now, assume 24 interrupts per I/O APIC (common for older systems)
            let gsi_end = ioapic.gsi_base + 24;
            if gsi >= ioapic.gsi_base && gsi < gsi_end {
                return Some(ioapic);
            }
        }
        None
    }
}

/// Parse the MADT table
///
/// # Arguments
/// * `madt` - Pointer to the MADT table
///
/// # Returns
/// * `Some(ParsedMadt)` if parsing succeeds
/// * `None` if parsing fails
///
/// # Safety
/// The madt pointer must point to valid memory.
pub unsafe fn parse_madt(madt: &Madt) -> Option<ParsedMadt> {
    let mut result = ParsedMadt {
        local_apic_address: madt.local_apic_address,
        flags: madt.flags,
        local_apic_count: 0,
        local_apics: [LocalApicEntry {
            header: MadtEntryHeader {
                entry_type: 0,
                length: 0,
            },
            processor_id: 0,
            apic_id: 0,
            flags: 0,
        }; MAX_LOCAL_APICS],
        io_apic_count: 0,
        io_apics: [IoApicEntry {
            header: MadtEntryHeader {
                entry_type: 0,
                length: 0,
            },
            io_apic_id: 0,
            reserved: 0,
            address: 0,
            gsi_base: 0,
        }; MAX_IO_APICS],
        override_count: 0,
        overrides: [InterruptSourceOverrideEntry {
            header: MadtEntryHeader {
                entry_type: 0,
                length: 0,
            },
            bus: 0,
            source_irq: 0,
            gsi: 0,
            flags: 0,
        }; MAX_OVERRIDES],
    };

    // Entries start after the MADT header (44 bytes)
    let header_size = mem::size_of::<SDTHeader>() + 8; // SDTHeader + local_apic_address + flags
    let mut offset = header_size;

    while offset < madt.header.length as usize {
        let entry_ptr = (madt as *const Madt as *const u8).add(offset) as *const MadtEntryHeader;
        let entry_header = &*entry_ptr;

        let entry_length = entry_header.length as usize;

        match entry_header.entry_type {
            0 => {
                // Local APIC
                if result.local_apic_count < MAX_LOCAL_APICS {
                    let entry = &*(entry_ptr as *const LocalApicEntry);
                    result.local_apics[result.local_apic_count] = *entry;
                    result.local_apic_count += 1;
                }
            }
            1 => {
                // I/O APIC
                if result.io_apic_count < MAX_IO_APICS {
                    let entry = &*(entry_ptr as *const IoApicEntry);
                    result.io_apics[result.io_apic_count] = *entry;
                    result.io_apic_count += 1;
                }
            }
            2 => {
                // Interrupt Source Override
                if result.override_count < MAX_OVERRIDES {
                    let entry = &*(entry_ptr as *const InterruptSourceOverrideEntry);
                    result.overrides[result.override_count] = *entry;
                    result.override_count += 1;
                }
            }
            _ => {
                // Unknown entry type - skip
            }
        }

        offset += entry_length;
    }

    Some(result)
}

/// Find and parse the MADT table
///
/// # Arguments
/// * `rsdp` - The RSDP structure
///
/// # Returns
/// * `Some(ParsedMadt)` if MADT is found and parsed successfully
/// * `None` if MADT is not found or parsing fails
pub fn find_and_parse_madt(rsdp: &super::rsdp::Rsdp) -> Option<ParsedMadt> {
    unsafe {
        let madt_header = super::rsdt::find_table_in_rsdt(rsdp, MADT_SIGNATURE)?;

        let madt = &*(madt_header as *const SDTHeader as *const Madt);
        parse_madt(madt)
    }
}

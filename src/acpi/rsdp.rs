// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RSDP (Root System Description Pointer) discovery
//!
//! The RSDP is the entry point for ACPI tables. It can be found in:
//! 1. The first 1KB of the EBDA (Extended BIOS Data Area)
//! 2. The BIOS ROM address space 0xE0000-0xFFFFF

use core::mem;

/// RSDP signature (for ACPI 1.0)
pub const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// RSDP structure (ACPI 1.0)
#[repr(C, packed)]
#[derive(Debug)]
pub struct Rsdp {
    /// Signature "RSD PTR " (8 bytes)
    pub signature: [u8; 8],
    /// Checksum of the entire RSDP structure
    pub checksum: u8,
    /// OEM identifier (6 bytes)
    pub oem_id: [u8; 6],
    /// Revision
    pub revision: u8,
    /// Physical address of RSDT
    pub rsdt_physical_address: u32,
}

/// RSDP structure (ACPI 2.0+)
#[repr(C, packed)]
pub struct RsdpV2 {
    /// ACPI 1.0 RSDP portion
    pub base: Rsdp,
    /// Length of the entire RSDP structure
    pub length: u32,
    /// Physical address of XSDT
    pub xsdt_physical_address: u64,
    /// Checksum of entire structure (including extended fields)
    pub extended_checksum: u8,
    /// Reserved (must be 0)
    pub reserved: [u8; 3],
}

/// Verify RSDP checksum
///
/// Returns true if the checksum is valid
pub fn verify_rsdp_checksum(rsdp: &Rsdp) -> bool {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            rsdp as *const Rsdp as *const u8,
            mem::size_of::<Rsdp>(),
        )
    };

    let sum: u8 = bytes.iter().fold(0, |acc, &b| acc.wrapping_add(b));
    sum == 0
}

/// Search for RSDP in a given memory range
///
/// # Arguments
/// * `start` - Start of the memory range to search
/// * `end` - End of the memory range to search
///
/// # Returns
/// * `Some(&Rsdp)` if a valid RSDP is found
/// * `None` if no valid RSDP is found
///
/// # Safety
/// This function reads from physical memory addresses.
/// The caller must ensure the memory range is valid and accessible.
unsafe fn search_rsdp_in_range(start: u64, end: u64) -> Option<&'static Rsdp> {
    let mut addr = start;

    while addr < end {
        let rsdp_ptr = addr as *const Rsdp;
        let rsdp = &*rsdp_ptr;

        // Check for signature
        if &rsdp.signature != RSDP_SIGNATURE {
            addr += 16; // RSDP is 16-byte aligned
            continue;
        }

        // Verify checksum
        if verify_rsdp_checksum(rsdp) {
            return Some(rsdp);
        }

        addr += 16;
    }

    None
}

/// Find the RSDP in system memory
///
/// This searches for the RSDP in the standard locations:
/// 1. First 1KB of the EBDA (pointed to by 0x40E)
/// 2. BIOS ROM area 0xE0000-0xFFFFF
///
/// # Returns
/// * `Some(&Rsdp)` if a valid RSDP is found
/// * `None` if no valid RSDP is found
///
/// # Safety
/// This function reads from physical memory addresses.
/// In UEFI systems, ACPI tables are typically provided via the
/// ACPI configuration table in the EFI system table.
pub fn find_rsdp() -> Option<&'static Rsdp> {
    // TODO: In UEFI, we should get this from the EFI system table
    // For now, search the legacy BIOS locations

    unsafe {
        // Try BIOS ROM area first (0xE0000-0xFFFFF)
        if let Some(rsdp) = search_rsdp_in_range(0xE0000, 0xFFFFF) {
            return Some(rsdp);
        }

        // Try EBDA (Extended BIOS Data Area)
        // EBDA address is at 0x40E, first 1KB
        let ebda_ptr = 0x40E as *const u16;
        let ebda_address = (*ebda_ptr) as u64 * 16; // Convert segment to linear address

        if ebda_address != 0 {
            if let Some(rsdp) = search_rsdp_in_range(ebda_address, ebda_address + 0x400) {
                return Some(rsdp);
            }
        }
    }

    None
}

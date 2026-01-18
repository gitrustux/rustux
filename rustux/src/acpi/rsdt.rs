// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RSDT (Root System Description Table) parsing
//!
//! The RSDT contains pointers to all other ACPI tables.

use core::mem;

/// Standard ACPI System Description Table header
#[repr(C, packed)]
#[derive(Debug)]
pub struct SDTHeader {
    /// Table signature (4 bytes)
    pub signature: [u8; 4],
    /// Length of the table including header
    pub length: u32,
    /// Revision
    pub revision: u8,
    /// Checksum of entire table
    pub checksum: u8,
    /// OEM identifier (6 bytes)
    pub oem_id: [u8; 6],
    /// OEM table identifier (8 bytes)
    pub oem_table_id: [u8; 8],
    /// OEM revision
    pub oem_revision: u32,
    /// Creator ID
    pub creator_id: u32,
    /// Creator revision
    pub creator_revision: u32,
}

/// Verify SDT checksum
///
/// Returns true if the checksum is valid
pub fn verify_sdt_checksum(header: &SDTHeader, length: u32) -> bool {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            header as *const SDTHeader as *const u8,
            length as usize,
        )
    };

    let sum: u8 = bytes.iter().fold(0, |acc, &b| acc.wrapping_add(b));
    sum == 0
}

/// RSDT (Root System Description Table)
///
/// Contains an array of 32-bit physical addresses pointing to other ACPI tables.
#[repr(C, packed)]
pub struct Rsdt {
    pub header: SDTHeader,
    // Array of physical addresses follows here
}

/// XSDT (Extended System Description Table)
///
/// Contains an array of 64-bit physical addresses pointing to other ACPI tables.
/// Used in ACPI 2.0+ systems.
#[repr(C, packed)]
pub struct Xsdt {
    pub header: SDTHeader,
    // Array of 64-bit physical addresses follows here
}

/// Iterator over SDT entries in an RSDT
pub struct RsdtIterator<'a> {
    current: *const u32,
    end: *const u32,
    _phantom: core::marker::PhantomData<&'a u32>,
}

impl<'a> RsdtIterator<'a> {
    /// Create a new iterator from an RSDT
    ///
    /// # Safety
    /// The rsdt must point to valid memory and the length must be correct.
    pub unsafe fn new(rsdt: &'a Rsdt) -> Self {
        let entry_count = (rsdt.header.length as usize - mem::size_of::<SDTHeader>()) / 4;
        let start = (rsdt as *const Rsdt as *const u8).add(mem::size_of::<SDTHeader>()) as *const u32;
        let end = start.add(entry_count);

        Self {
            current: start,
            end,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<'a> Iterator for RsdtIterator<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.end {
            let value = unsafe { *self.current };
            self.current = unsafe { self.current.offset(1) };
            Some(value)
        } else {
            None
        }
    }
}

/// Find a table with a specific signature in the RSDT
///
/// # Arguments
/// * `rsdp` - The RSDP structure
/// * `signature` - 4-byte signature to search for
///
/// # Returns
/// * `Some(&SDTHeader)` if the table is found
/// * `None` if the table is not found
///
/// # Safety
/// This function dereferences physical memory addresses.
pub fn find_table_in_rsdt(rsdp: &super::rsdp::Rsdp, signature: &[u8; 4]) -> Option<&'static SDTHeader> {
    unsafe {
        let rsdt_address = rsdp.rsdt_physical_address as u64;
        let rsdt = &*(rsdt_address as *const Rsdt);

        // Verify RSDT signature
        if &rsdt.header.signature != b"RSDT" {
            return None;
        }

        // Verify RSDT checksum
        if !verify_sdt_checksum(&rsdt.header, rsdt.header.length) {
            return None;
        }

        // Search through entries
        for entry_addr in RsdtIterator::new(rsdt) {
            let header = &*(entry_addr as u64 as *const SDTHeader);

            if &header.signature == signature {
                // Verify checksum of the found table
                if verify_sdt_checksum(header, header.length) {
                    return Some(header);
                }
            }
        }
    }

    None
}

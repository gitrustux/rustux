// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ELF Loader
//!
//! This module provides ELF (Executable and Linkable Format) binary loading
//! for x86_64 executables. It parses ELF files and maps them into process
//! address spaces using Virtual Memory Objects (VMOs).

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::object::{Vmo, VmoFlags};

// ============================================================================
// ELF Constants
// ============================================================================

/// ELF magic number
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF class (32-bit vs 64-bit)
pub const ELFCLASS64: u8 = 2;

/// ELF data encoding (little-endian vs big-endian)
pub const ELFDATA2LSB: u8 = 1;

/// ELF version
pub const EV_CURRENT: u8 = 1;

/// x86_64 machine architecture
pub const EM_X86_64: u16 = 62;

/// ELF file type: Executable
pub const ET_EXEC: u16 = 2;

/// Program header type: Load
pub const PT_LOAD: u32 = 1;

// Segment permissions
pub const PF_X: u32 = 0x1; // Execute
pub const PF_W: u32 = 0x2; // Write
pub const PF_R: u32 = 0x4; // Read

// ============================================================================
// ELF File Structures
// ============================================================================

/// ELF identifier (first 16 bytes of ELF file)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ElfIdent {
    pub magic: [u8; 4],      // 0x7F 'ELF'
    pub class_: u8,            // 1 = 32-bit, 2 = 64-bit
    pub endianness: u8,        // 1 = little-endian
    pub version: u8,           // ELF version (must be 1)
    pub os_abi: u8,            // System ABI (often 0)
    pub abi_version: u8,       // ABI version
    pub pad: [u8; 7],          // Padding
}

/// ELF header (first 64 bytes for 64-bit ELF)
#[repr(C)]
#[derive(Debug)]
pub struct ElfHeader {
    pub e_ident: [u8; 16],    // ELF identification
    pub e_type: u16,           // File type (relocatable, executable, etc.)
    pub e_machine: u16,        // Architecture (x86_64 = 62)
    pub e_version: u32,         // ELF version (must be 1)
    pub e_entry: u64,          // Entry point virtual address
    pub e_phoff: u64,          // Program header table file offset
    pub e_shoff: u64,          // Section header table file offset
    pub e_flags: u32,          // Processor-specific flags
    pub e_ehsize: u16,         // ELF header size
    pub e_phentsize: u16,      // Program header entry size
    pub e_phnum: u16,          // Number of program headers
    pub e_shentsize: u16,      // Section header entry size
    pub e_shnum: u16,          // Number of section headers
    pub e_shstrndx: u16,       // Section header string table index
}

/// Program header (describes a segment to load)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ProgramHeader {
    pub p_type: u32,           // Segment type (LOAD, DYNAMIC, INTERP, etc.)
    pub p_flags: u32,          // Segment flags (R, W, X permissions)
    pub p_offset: u64,         // Segment file offset
    pub p_vaddr: u64,          // Segment virtual address
    pub p_paddr: u64,          // Segment physical address (usually = vaddr)
    pub p_filesz: u64,        // Segment size in file
    pub p_memsz: u64,          // Segment size in memory (can be > filesz for BSS)
    pub p_align: u64,          // Segment alignment (power of 2)
}

/// Loaded ELF segment information
pub struct LoadedSegment {
    pub vaddr: u64,           // Virtual address
    pub size: u64,             // Size in memory
    pub vmo: Box<Vmo>,         // VMO containing the segment data (boxed for stable address)
    pub flags: u32,            // PF_R | PF_W | PF_X
}

/// Loaded ELF binary information
pub struct LoadedElf {
    pub entry: u64,             // Entry point address
    pub segments: Vec<LoadedSegment>, // Loaded segments
    pub stack_addr: u64,        // Stack top address
    pub stack_size: u64,        // Stack size
}

// ============================================================================
// ELF Parsing
// ============================================================================

/// Parse ELF header from raw data
///
/// # Arguments
///
/// * `data` - Raw ELF file data
///
/// # Returns
///
/// * `Ok(ElfHeader)` - Parsed ELF header
/// * `Err(&str)` - Error message if ELF is invalid
pub fn parse_elf_header(data: &[u8]) -> Result<ElfHeader, &'static str> {
    // Minimum size check
    if data.len() < 64 {
        return Err("ELF file too small");
    }

    // Validate magic
    if &data[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic (not an ELF file)");
    }

    // Must be 64-bit
    if data[4] != ELFCLASS64 {
        return Err("Not a 64-bit ELF (class must be 2)");
    }

    // Must be little-endian
    if data[5] != ELFDATA2LSB {
        return Err("Not little-endian (endianness must be 1)");
    }

    // Read remaining header fields
    let e_type = u16::from_le_bytes([data[16], data[17]]);
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    let e_entry = u64::from_le_bytes([
        data[24], data[25], data[26], data[27],
        data[28], data[29], data[30], data[31],
    ]);
    let e_phoff = u64::from_le_bytes([
        data[32], data[33], data[34], data[35],
        data[36], data[37], data[38], data[39],
    ]);
    let e_shoff = u64::from_le_bytes([
        data[40], data[41], data[42], data[43],
        data[44], data[45], data[46], data[47],
    ]);
    let e_phentsize = u16::from_le_bytes([data[54], data[55]]);
    let e_phnum = u16::from_le_bytes([data[56], data[57]]);

    // Build e_ident array
    let mut e_ident = [0u8; 16];
    e_ident[0..4].copy_from_slice(&ELF_MAGIC);
    e_ident[4] = data[4];     // class
    e_ident[5] = data[5];     // endianness
    e_ident[6] = data[6];     // version
    e_ident[7] = data[7];     // os_abi
    e_ident[8] = data[8];     // abi_version
    // rest remains 0

    Ok(ElfHeader {
        e_ident,
        e_type,
        e_machine,
        e_version: 1,
        e_entry,
        e_phoff,
        e_shoff,
        e_flags: 0,
        e_ehsize: 64,
        e_phentsize,
        e_phnum,
        e_shentsize: 0,
        e_shnum: 0,
        e_shstrndx: 0,
    })
}

/// Parse program headers from raw data
///
/// # Arguments
///
/// * `data` - Raw ELF file data
/// * `phoff` - Program header offset from ELF header
/// * `phentsize` - Size of each program header entry
/// * `phnum` - Number of program headers
///
/// # Returns
///
/// * `Vec<ProgramHeader>` - Parsed program headers
pub fn parse_program_headers(
    data: &[u8],
    phoff: u64,
    phentsize: u16,
    phnum: u16,
) -> Vec<ProgramHeader> {
    let mut headers = Vec::new();

    for i in 0..phnum {
        let offset = phoff as usize + (i as usize * phentsize as usize);

        if offset + phentsize as usize > data.len() {
            break; // Don't read past end of file
        }

        let ph_data = &data[offset..offset + phentsize as usize];

        let p_type = u32::from_le_bytes([
            ph_data[0], ph_data[1], ph_data[2], ph_data[3],
        ]);

        let p_flags = u32::from_le_bytes([
            ph_data[4], ph_data[5], ph_data[6], ph_data[7],
        ]);

        let p_offset = u64::from_le_bytes([
            ph_data[8], ph_data[9], ph_data[10], ph_data[11],
            ph_data[12], ph_data[13], ph_data[14], ph_data[15],
        ]);

        let p_vaddr = u64::from_le_bytes([
            ph_data[16], ph_data[17], ph_data[18], ph_data[19],
            ph_data[20], ph_data[21], ph_data[22], ph_data[23],
        ]);

        let p_paddr = u64::from_le_bytes([
            ph_data[24], ph_data[25], ph_data[26], ph_data[27],
            ph_data[28], ph_data[29], ph_data[30], ph_data[31],
        ]);

        let p_filesz = u64::from_le_bytes([
            ph_data[32], ph_data[33], ph_data[34], ph_data[35],
            ph_data[36], ph_data[37], ph_data[38], ph_data[39],
        ]);

        let p_memsz = u64::from_le_bytes([
            ph_data[40], ph_data[41], ph_data[42], ph_data[43],
            ph_data[44], ph_data[45], ph_data[46], ph_data[47],
        ]);

        let p_align = u64::from_le_bytes([
            ph_data[48], ph_data[49], ph_data[50], ph_data[51],
            ph_data[52], ph_data[53], ph_data[54], ph_data[55],
        ]);


        headers.push(ProgramHeader {
            p_type,
            p_flags,
            p_offset,
            p_vaddr,
            p_paddr,
            p_filesz,
            p_memsz,
            p_align,
        });
    }

    headers
}

// ============================================================================
// ELF Validation
// ============================================================================

/// Validate ELF header for x86_64 executable
///
/// # Arguments
///
/// * `header` - Parsed ELF header
///
/// # Returns
///
/// * `Ok(())` - ELF is valid for loading
/// * `Err(&str)` - ELF is invalid or not supported
pub fn validate_elf_header(header: &ElfHeader) -> Result<(), &'static str> {
    // Must be executable
    if header.e_type != ET_EXEC {
        return Err("Not an executable (wrong e_type)");
    }

    // Must be x86_64
    if header.e_machine != EM_X86_64 {
        return Err("Not x86_64 (wrong e_machine)");
    }

    // Must have program headers
    if header.e_phnum == 0 {
        return Err("No program headers");
    }

    // Must have program header entries
    if header.e_phoff == 0 || header.e_phentsize < 56 {
        return Err("Invalid program header table");
    }

    Ok(())
}

// ============================================================================
// ELF Loading
// ============================================================================

/// Convert ELF PF_* flags to VMO flags
fn elf_flags_to_vmo_flags(p_flags: u32) -> VmoFlags {
    // For now, VMOs don't have execute/write flags in their flags
    // Those are managed at the mapping level
    VmoFlags::empty
}

/// Load an ELF binary into memory
///
/// This function parses an ELF file and creates VMOs for each LOAD segment,
/// then returns information needed to execute the binary.
///
/// # Arguments
///
/// * `elf_data` - Raw ELF file contents
///
/// # Returns
///
/// * `Ok(Box<LoadedElf>)` - Loaded ELF with segments mapped to VMOs (boxed to protect from stack corruption)
/// * `Err(&str)` - Error loading ELF
pub fn load_elf(elf_data: &[u8]) -> Result<Box<LoadedElf>, &'static str> {
    // Parse ELF header
    let header = parse_elf_header(elf_data)?;

    // Validate ELF header
    validate_elf_header(&header)?;

    // Parse program headers
    let phentsize = header.e_phentsize;
    let phoff = header.e_phoff;
    let phnum = header.e_phnum;

    let prog_headers = parse_program_headers(elf_data, phoff, phentsize, phnum);

    // Filter for LOAD segments and clone them to avoid reference issues
    // We need to own the data because heap allocations during VMO creation
    // can corrupt the references in the Vec.
    let load_segments: Vec<ProgramHeader> = prog_headers
        .iter()
        .filter(|ph| ph.p_type == PT_LOAD)
        .cloned()
        .collect();

    // Convert to array to prevent Vec corruption during heap allocations
    // This is critical because VMO operations trigger heap allocations
    // that can corrupt the Vec's internal storage.
    let mut segments_array = [None; 16]; // Support up to 16 LOAD segments
    let mut segment_count = 0;
    for ph in load_segments {
        if segment_count < 16 {
            segments_array[segment_count] = Some(ph);
            segment_count += 1;
        }
    }

    // Load each segment from the array
    let mut segments = Vec::with_capacity(segment_count);

    for idx in 0..segment_count {
        let ph = segments_array[idx].unwrap();

        // Copy all necessary fields BEFORE any heap allocations
        // This prevents corruption from Vec reallocations during VMO creation
        let p_offset = ph.p_offset;
        let p_filesz = ph.p_filesz;
        let p_memsz = ph.p_memsz;
        let p_vaddr = ph.p_vaddr;
        let p_flags = ph.p_flags;

        // Get segment data from file (using copied values)
        let file_start = p_offset as usize;
        let file_end = (p_offset + p_filesz) as usize;

        // Check bounds before accessing slice
        if p_filesz > 0 && file_end > elf_data.len() {
            return Err("Segment extends beyond file size");
        }

        let segment_data = if p_filesz > 0 {
            &elf_data[file_start..file_end]
        } else {
            &[]
        };

        // Create VMO for this segment
        let mem_size = p_memsz.max(p_filesz); // Handle BSS (filesz < memsz)

        // Skip zero-size segments (alignment segments, etc.)
        if mem_size == 0 {
            continue;
        }

        // Align up to page size
        let aligned_size = (mem_size + 0xFFF) & !0xFFF;

        // Create VMO
        let vmo_flags = elf_flags_to_vmo_flags(p_flags);

        // Create VMO with size
        let vmo = Vmo::create(aligned_size as usize, vmo_flags)
            .map_err(|_| "Failed to create VMO")?;

        // CRITICAL: Immediately box the VMO before any operations
        // This prevents stack corruption from overwriting the VMO
        let boxed_vmo = Box::new(vmo);

        // Write segment data to VMO (this allocates physical pages)
        if p_filesz > 0 {
            boxed_vmo.write(0, segment_data)
                .map_err(|_| "Failed to write segment data to VMO")?;
        }

        // Zero the BSS portion (if any) - use smaller chunks to avoid stack overflow
        if p_memsz > p_filesz {
            let bss_offset = p_filesz as usize;
            let bss_size = (p_memsz - p_filesz) as usize;

            // Zero in 256-byte chunks to avoid large stack allocations
            let mut bytes_written = 0;
            while bytes_written < bss_size {
                let chunk_size = core::cmp::min(bss_size - bytes_written, 256);
                let zero_chunk = [0u8; 256]; // Much smaller!

                boxed_vmo.write(bss_offset + bytes_written, &zero_chunk[..chunk_size])
                    .map_err(|_| "Failed to zero BSS")?;
                bytes_written += chunk_size;
            }
        }

        // Store segment in Vec
        segments.push(LoadedSegment {
            vaddr: p_vaddr,
            size: mem_size,
            vmo: boxed_vmo,
            flags: p_flags,
        });
    }

    // Set up user stack
    let stack_addr = 0x7fff_ffff_f000u64;
    let stack_size = 8 * 1024 * 1024; // 8 MB stack

    Ok(Box::new(LoadedElf {
        entry: header.e_entry,
        segments,
        stack_addr,
        stack_size,
    }))
}

/// Check if data looks like an ELF file
///
/// # Arguments
///
/// * `data` - File data to check
///
/// # Returns
///
/// * `true` if data appears to be a valid ELF file
pub fn is_elf_file(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }

    // Check magic
    &data[0..4] == ELF_MAGIC
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elf_magic() {
        assert_eq!(ELF_MAGIC, [0x7f, b'E', b'L', b'F']);
    }

    #[test]
    fn test_elf_constants() {
        assert_eq!(ELFCLASS64, 2);
        assert_eq!(ELFDATA2LSB, 1);
        assert_eq!(EM_X86_64, 62);
        assert_eq!(ET_EXEC, 2);
        assert_eq!(PT_LOAD, 1);
    }

    #[test]
    fn test_ident_size() {
        assert_eq!(core::mem::size_of::<ElfIdent>(), 16);
    }

    #[test]
    fn test_header_size() {
        assert_eq!(core::mem::size_of::<ElfHeader>(), 64);
    }

    #[test]
    fn test_program_header_size() {
        assert_eq!(core::mem::size_of::<ProgramHeader>(), 56);
    }

    #[test]
    fn test_simple_64bit_elf() {
        // Minimal valid 64-bit ELF header
        let data: [u8; 64] = [
            // e_ident
            0x7F, b'E', b'L', b'F',    // magic
            2,                      // class (64-bit)
            1,                      // endianness (little-endian)
            1,                      // version
            0,                      // os_abi
            0,                      // abi_version
            // pad
            0, 0, 0, 0, 0, 0, 0,
            // e_type
            0x02, 0x00,              // ET_EXEC
            // e_machine
            0x3E, 0x00,              // EM_X86_64
            // e_version
            0x01, 0x00, 0x00, 0x00,
            // e_entry
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x10,
            // e_phoff
            0x40, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            // e_shoff
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            // e_flags
            0x00, 0x00, 0x00, 0x00,
            // e_ehsize
            0x40, 0x00,
            // e_phentsize
            0x38, 0x00,
            // e_phnum
            0x02, 0x00,
            // e_shentsize
            0x00, 0x00,
            // e_shnum
            0x00, 0x00,
            // e_shstrndx
            0x00, 0x00,
            // remaining padding
            0, 0, 0, 0, 0, 0,
        ];

        let result = parse_elf_header(&data);
        assert!(result.is_ok(), "Failed to parse ELF header: {:?}", result);

        let header = result.unwrap();
        assert_eq!(header.e_type, ET_EXEC);
        assert_eq!(header.e_machine, EM_X86_64);
        assert_eq!(header.e_phnum, 2);
        assert_eq!(header.e_entry, 0x100000000);
    }

    #[test]
    fn test_is_elf_file() {
        // Valid ELF
        let elf_data = [
            0x7F, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00, // ident...
            0x3E, 0x00,                                      // x86_64
            // ... rest of header
        ];
        assert!(is_elf_file(&elf_data));

        // Not ELF
        assert!(!is_elf_file(b"#!/bin/bash"));
        assert!(!is_elf_file(b"Plain text"));
    }

    #[test]
    fn test_validate_executable() {
        let data: [u8; 64] = [
            0x7F, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00,
            0x3E, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
            0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x40, 0x00,
            0x38, 0x00,
            0x02, 0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x00, 0x00,
            0, 0, 0, 0, 0, 0,
        ];

        let header = parse_elf_header(&data).unwrap();
        assert!(validate_elf_header(&header).is_ok());

        // Wrong type
        let mut bad_data = data;
        bad_data[0x10] = 0x01; // e_type = 1 (relocatable)
        let header = parse_elf_header(&bad_data).unwrap();
        assert!(validate_elf_header(&header).is_err());

        // Wrong architecture
        let mut bad_data = data;
        bad_data[0x12] = 0x03; // e_machine = 3 (x86)
        let header = parse_elf_header(&bad_data).unwrap();
        assert!(validate_elf_header(&header).is_err());
    }
}

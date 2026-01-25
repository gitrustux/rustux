// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Build script for Rustux kernel
//!
//! This build script:
//! 1. Compiles assembly files (context switch)
//! 2. Embeds files into the kernel as a ramdisk
//! 3. Generates ramdisk.bin at build time

use std::env;
use std::fs;
use std::path::PathBuf;
use std::io::Write;

fn main() {
    // Tell cargo to rerun this script if source files change
    println!("cargo:rerun-if-changed=src/arch/amd64/switch.S");
    println!("cargo:rerun-if-changed=test-userspace/");
    println!("cargo:rerun-if-changed=test-userspace/shell/");
    println!("cargo:rerun-if-changed=files/");
    println!("cargo:rerun-if-changed=target/hello.elf");
    println!("cargo:rerun-if-changed=target/counter.elf");
    println!("cargo:rerun-if-changed=target/init.elf");
    println!("cargo:rerun-if-changed=target/shell.elf");

    // Get the output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // ============================================================================
    // Part 1: Compile context switch assembly
    // ============================================================================

    let asm_file = PathBuf::from("src/arch/amd64/switch.S");
    let obj_file = out_dir.join("switch.o");

    if asm_file.exists() {
        let status = std::process::Command::new("cc")
            .arg("-c")
            .arg("-o")
            .arg(&obj_file)
            .arg(&asm_file)
            .status()
            .expect("Failed to execute cc command");

        if !status.success() {
            panic!("Failed to compile assembly file: {:?}", asm_file);
        }

        println!("cargo:rustc-link-arg={}", obj_file.display());
    }

    // ============================================================================
    // Part 2: Create embedded ramdisk
    // ============================================================================

    // Ramdisk structures (matching src/fs/ramdisk.rs)
    #[repr(C)]
    struct RamdiskFile {
        name_offset: u32,
        data_offset: u32,
        size: u32,
        _pad: u32,
    }

    #[repr(C)]
    struct RamdiskSuperblock {
        magic: u32,           // 0x52555458 ("RUTX")
        num_files: u32,
        files_offset: u32,
    }

    let ramdisk_output = out_dir.join("ramdisk.bin");
    let mut ramdisk = fs::File::create(&ramdisk_output)
        .expect("Failed to create ramdisk.bin");

    // Collect files to embed
    let mut files_to_embed = vec![
        // Test files
        ("files/test.txt", "test.txt"),
    ];

    // Check for ELF binaries in target directory
    let potential_elf_files = vec![
        ("target/hello.elf", "bin/hello"),
        ("target/counter.elf", "bin/counter"),
        ("target/init.elf", "bin/init"),
        ("target/shell.elf", "bin/shell"),
    ];

    // Add ELF files if they exist
    for (src_path, name) in potential_elf_files {
        if PathBuf::from(src_path).exists() {
            files_to_embed.push((src_path, name));
            println!("cargo:warning=Embedding ELF: {} -> {}", src_path, name);
        }
    }

    // Check which test userspace programs exist
    let userspace_progs = vec![
        "test-userspace/hello.c",
        "test-userspace/counter.c",
        "test-userspace/spinner.c",
        "test-userspace/init.c",
    ];

    // Check for ELF binaries that were compiled
    let elf_binaries = vec![
        "test-userspace/hello.elf",
        "test-userspace/counter.elf",
        "test-userspace/spinner.elf",
        "test-userspace/init.elf",
    ];

    // Calculate offsets
    let superblock_size = std::mem::size_of::<RamdiskSuperblock>() as u32;
    let file_entry_size = std::mem::size_of::<RamdiskFile>() as u32;

    // Start offsets
    let mut files_offset = superblock_size;
    let mut names_offset = files_offset + file_entry_size * files_to_embed.len() as u32;
    let mut data_offset = names_offset;
    let mut file_entries = Vec::new();

    // First pass: calculate all offsets
    for (src_path, _name) in &files_to_embed {
        let contents = fs::read(src_path)
            .expect(&format!("Failed to read file: {}", src_path));

        let name_bytes = _name.as_bytes();

        // Calculate offsets
        let name_len = name_bytes.len() as u32;
        file_entries.push(RamdiskFile {
            name_offset: data_offset,
            data_offset: data_offset + name_len + 1, // +1 for null terminator
            size: contents.len() as u32,
            _pad: 0,
        });

        // Update data offset (after name + null terminator)
        data_offset = data_offset + name_len + 1;
    }

    // Second pass: write the ramdisk
    // Write superblock
    let superblock = RamdiskSuperblock {
        magic: 0x52555458,
        num_files: file_entries.len() as u32,
        files_offset: files_offset,
    };

    unsafe {
        let superblock_bytes = std::slice::from_raw_parts(
            &superblock as *const _ as *const u8,
            std::mem::size_of::<RamdiskSuperblock>(),
        );
        ramdisk.write_all(superblock_bytes).unwrap();
    }

    // Write file headers (at files_offset)
    // First pad to files_offset
    let mut current_size = superblock_size as usize;
    while current_size < files_offset as usize {
        ramdisk.write_all(&[0]).unwrap();
        current_size += 1;
    }

    // Write file entries
    for entry in &file_entries {
        unsafe {
            let entry_bytes = std::slice::from_raw_parts(
                entry as *const _ as *const u8,
                std::mem::size_of::<RamdiskFile>(),
            );
            ramdisk.write_all(entry_bytes).unwrap();
        }
    }

    // Write names and data
    for (src_path, _name) in &files_to_embed {
        // Write name (with null terminator)
        ramdisk.write_all(_name.as_bytes()).unwrap();
        ramdisk.write_all(&[0u8]).unwrap(); // null terminator

        // Write file contents
        let contents = fs::read(src_path)
            .expect(&format!("Failed to read file: {}", src_path));
        ramdisk.write_all(&contents).unwrap();
    }

    // Tell cargo where to find the ramdisk
    println!("cargo:rustc-env=RAMDISK_PATH={}", ramdisk_output.display());
    println!("cargo:rerun-if-changed={}", ramdisk_output.display());

    // Print ramdisk info for debugging
    println!("cargo:warning=Ramdisk: {} files, {} bytes total",
        file_entries.len(),
        ramdisk.metadata().unwrap().len()
    );

    // ============================================================================
    // Part 3: Link search path
    // ============================================================================

    println!("cargo:rustc-link-search={}", out_dir.display());
}

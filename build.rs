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

/// Fail the build if a `static mut` declaration appears in the kernel
/// sources (see sync::irq for what to use instead). Only files git tracks
/// are checked, so stray copies (e.g. old files a sync tool brought back)
/// cannot break the build; without git, every .rs file under src/ is.
fn forbid_static_mut() {
    let tracked = std::process::Command::new("git")
        .args(["ls-files", "--", "src"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(std::path::PathBuf::from).collect::<Vec<_>>());
    let files = match tracked {
        Some(f) if !f.is_empty() => f,
        _ => {
            let mut all = Vec::new();
            walk(std::path::Path::new("src"), &mut all);
            all
        }
    };
    for path in files.iter().filter(|p| p.extension().map_or(false, |x| x == "rs")) {
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            // A declaration, not a `&'static mut T` type
            if code.match_indices("static mut ").any(|(i, _)| !code[..i].ends_with('\'')) {
                panic!("{}:{}: `static mut` is not allowed; use IrqMutex, SpinMutex, BootCell, an atomic or per-CPU data", path.display(), n + 1);
            }
        }
    }
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    forbid_static_mut();
    // Tell cargo to rerun this script if source files change
    println!("cargo:rerun-if-changed=src/arch/amd64/switch.S");
    println!("cargo:rerun-if-changed=test-userspace/");
    println!("cargo:rerun-if-changed=test-userspace/shell/");
    println!("cargo:rerun-if-changed=userspace/c-progs/");
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
        // Determine the target
        let target = env::var("TARGET").unwrap_or_default();

        // For UEFI targets, skip assembly compilation for now
        // The assembly file needs COFF format for UEFI which requires special tooling
        // The kernel can boot without context switching for initial testing
        if target.contains("uefi") {
            println!("cargo:warning=Skipping switch.S compilation for UEFI target (context switching unavailable)");
        } else {
            // For normal targets, use gcc for GAS syntax
            let status = std::process::Command::new("gcc")
                .arg("-c")
                .arg("-o")
                .arg(&obj_file)
                .arg(&asm_file)
                .status();

            let status = match status {
                Ok(s) => s,
                Err(_) => {
                    // If gcc fails, try with clang
                    std::process::Command::new("clang")
                        .arg("-c")
                        .arg("-o")
                        .arg(&obj_file)
                        .arg(&asm_file)
                        .status()
                        .expect("Failed to execute compiler command")
                }
            };

            if !status.success() {
                panic!("Failed to compile assembly file: {:?}", asm_file);
            } else {
                println!("cargo:rustc-link-arg={}", obj_file.display());
            }
        }
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

    // ELF binaries to embed. A freshly compiled copy in target/ wins; otherwise
    // fall back to the prebuilt binaries tracked in the repo (test-userspace/
    // and userspace/c-progs/), so a clean checkout still boots to the shell.
    let potential_elf_files: Vec<(&[&'static str], &'static str)> = vec![
        (&["target/hello.elf", "userspace/c-progs/hello.elf"], "bin/hello"),
        (&["target/counter.elf", "userspace/c-progs/counter.elf"], "bin/counter"),
        (&["target/objtest.elf", "userspace/c-progs/objtest.elf"], "bin/objtest"),
        (&["target/ipctest.elf", "userspace/c-progs/ipctest.elf"], "bin/ipctest"),
        (&["target/spawntest.elf", "userspace/c-progs/spawntest.elf"], "bin/spawntest"),
        (&["target/true.elf", "userspace/c-progs/true.elf"], "bin/true"),
        (&["target/ps2kbd.elf", "userspace/c-progs/ps2kbd.elf"], "bin/ps2kbd"),
        (&["target/drvtest.elf", "userspace/c-progs/drvtest.elf"], "bin/drvtest"),
        (&["target/init.elf", "test-userspace/init.elf"], "bin/init"),
        (&["target/shell.elf", "test-userspace/shell/shell.elf"], "bin/shell"),
    ];

    // Add the first existing candidate for each ELF
    for (candidates, name) in potential_elf_files {
        if let Some(src_path) = candidates.iter().find(|p| PathBuf::from(p).exists()) {
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

        // Next entry starts after this name, its null terminator and its contents
        // (matches the write order below: name, NUL, data)
        data_offset = data_offset + name_len + 1 + contents.len() as u32;
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

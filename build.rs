// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Build script for Rustux kernel
//!
//! This build script:
//! 1. Compiles assembly files (context switch)
//! 2. Builds the boot image (bootfs) embedded in the kernel, from
//!    files/test.txt and the programs in userspace/bin/

use std::env;
use std::fs;
use std::path::PathBuf;

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
    println!("cargo:rerun-if-changed=userspace/bin/");
    println!("cargo:rerun-if-changed=files/");

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
    // Part 2: Build the boot image (bootfs)
    // ============================================================================
    //
    // Layout (docs/FS.md): header { u32 magic 0x52555458 "RUTX", u32 count,
    // u32 table offset (12) }, then `count` entries { u32 name offset,
    // u32 data offset, u32 size, u32 0 }, then each file's NUL-terminated
    // name followed by its data. The kernel only looks up bin/init in it;
    // the filesystem service (fsd) serves it at /boot, with /bin = /boot/bin.

    let image_path = out_dir.join("bootfs.img");

    // Files: files/test.txt as /boot/test.txt, and every program in
    // userspace/bin/ (built by userspace/build.sh and committed) as /bin/<name>
    let mut files: Vec<(String, String)> = vec![("files/test.txt".into(), "test.txt".into())];
    let mut programs: Vec<(String, String)> = fs::read_dir("userspace/bin")
        .expect("userspace/bin/ is missing (run userspace/build.sh)")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (format!("userspace/bin/{}", name), format!("bin/{}", name))
        })
        .collect();
    programs.sort();
    files.extend(programs);

    let table = 12usize;
    let mut next = table + 16 * files.len();
    let mut entries = Vec::new();
    let mut body = Vec::new();
    for (src, name) in &files {
        let contents = fs::read(src).unwrap_or_else(|e| panic!("cannot read {}: {}", src, e));
        let name_off = next;
        let data_off = name_off + name.len() + 1;
        entries.push((name_off as u32, data_off as u32, contents.len() as u32));
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&contents);
        next = data_off + contents.len();
    }
    let mut image = Vec::with_capacity(next);
    for v in [0x5255_5458u32, files.len() as u32, table as u32] {
        image.extend_from_slice(&v.to_le_bytes());
    }
    for (n, d, len) in &entries {
        for v in [*n, *d, *len, 0] {
            image.extend_from_slice(&v.to_le_bytes());
        }
    }
    image.extend_from_slice(&body);
    fs::write(&image_path, &image).expect("cannot write bootfs.img");
    println!("cargo:rerun-if-changed=files/test.txt");
    println!("cargo:warning=Boot image: {} files, {} bytes", files.len(), image.len());

    // ============================================================================
    // Part 3: Link search path
    // ============================================================================

    println!("cargo:rustc-link-search={}", out_dir.display());
}

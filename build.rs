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

/// "git <hash>, built <date>" for the kernel's version string
fn build_info() -> String {
    let git = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    // Date from the clock, as YYYY-MM-DD (UTC)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("git {}, built {:04}-{:02}-{:02}", git, y, m, d)
}

fn main() {
    forbid_static_mut();
    println!("cargo:rustc-env=RUSTUX_BUILD_INFO={}", build_info());
    println!("cargo:rerun-if-changed=.git/HEAD");
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

    // Files: everything under files/ at the same path (files/etc/hostname
    // as /boot/etc/hostname, copied to /etc by fsd), and every program in
    // userspace/bin/ (built by userspace/build.sh and committed) as /bin/<name>
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).collect()).unwrap_or_default();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.push((p.to_string_lossy().into_owned(), rel));
            }
        }
    }
    let mut files: Vec<(String, String)> = Vec::new();
    walk(std::path::Path::new("files"), std::path::Path::new("files"), &mut files);
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

    // userspace/links: more names for multi-call programs ("NAME PROGRAM"),
    // stored as entries that share the program's data
    println!("cargo:rerun-if-changed=userspace/links");
    let mut links: Vec<(String, String)> = Vec::new();
    for line in fs::read_to_string("userspace/links").unwrap_or_default().lines() {
        let mut w = line.split_whitespace();
        if let (Some(name), Some(target)) = (w.next(), w.next()) {
            if !name.starts_with('#') {
                links.push((format!("bin/{}", name), format!("bin/{}", target)));
            }
        }
    }
    links.retain(|(name, target)| {
        let ok = files.iter().any(|(_, n)| n == target) && !files.iter().any(|(_, n)| n == name);
        if !ok {
            println!("cargo:warning=userspace/links: skipping {} ({} missing, or a program of that name exists)", name, target);
        }
        ok
    });

    let table = 12usize;
    let count = files.len() + links.len();
    let mut next = table + 16 * count;
    let mut entries = Vec::new();
    let mut body = Vec::new();
    let mut placed: std::collections::HashMap<String, (u32, u32)> = std::collections::HashMap::new();
    for (src, name) in &files {
        let contents = fs::read(src).unwrap_or_else(|e| panic!("cannot read {}: {}", src, e));
        let name_off = next;
        let mut data_off = name_off + name.len() + 1;
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        // Programs start on a page boundary: the loader maps their code
        // straight from the image (docs/FS.md)
        if name.starts_with("bin/") {
            let pad = (4096 - data_off % 4096) % 4096;
            body.resize(body.len() + pad, 0);
            data_off += pad;
        }
        entries.push((name_off as u32, data_off as u32, contents.len() as u32));
        placed.insert(name.clone(), (data_off as u32, contents.len() as u32));
        body.extend_from_slice(&contents);
        next = data_off + contents.len();
    }
    for (name, target) in &links {
        let (data_off, len) = placed[target];
        entries.push((next as u32, data_off, len));
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        next += name.len() + 1;
    }
    let mut image = Vec::with_capacity(next);
    for v in [0x5255_5458u32, count as u32, table as u32] {
        image.extend_from_slice(&v.to_le_bytes());
    }
    for (n, d, len) in &entries {
        for v in [*n, *d, *len, 0] {
            image.extend_from_slice(&v.to_le_bytes());
        }
    }
    image.extend_from_slice(&body);
    fs::write(&image_path, &image).expect("cannot write bootfs.img");
    println!("cargo:rerun-if-changed=files/");
    println!("cargo:warning=Boot image: {} files, {} links, {} bytes", files.len(), links.len(), image.len());

    // ============================================================================
    // Part 3: Link search path
    // ============================================================================

    println!("cargo:rustc-link-search={}", out_dir.display());
}

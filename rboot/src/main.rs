// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! rboot: Rustica's boot manager (checklist step 19), a UEFI application
//! in the style of systemd-boot. It lives at `\EFI\Rustica\rboot.efi` (or
//! `\EFI\BOOT\BOOTX64.EFI`) on an EFI system partition and offers:
//!
//! - the entries in `\loader\entries\*.conf` on its own partition:
//!   `title`, `efi` (the program), `options` (its command line), `bootfs`
//!   (a boot image for the Rustux kernel), `sort-key`;
//! - for each Rustica entry (its program is `rustux.efi` or `rustux-VERSION.efi`) a recovery entry
//!   (the rescue target) and, for the first one with `root=` set, a live
//!   entry (`root=live`: the system from memory, disks untouched);
//! - Windows Boot Manager and Linux loaders (shim, GRUB, systemd-boot)
//!   found on any EFI partition, and the firmware setup;
//! - boot counting (package step E4), as systemd-boot's: an entry file
//!   named `ID+LEFT[-DONE].conf` has LEFT tries; rboot lowers it
//!   (`ID+LEFT-1-DONE+1.conf`) before starting it and names the file in
//!   the LoaderBootCountPath variable, and the system, once up, renames it
//!   to `ID.conf` (`bootctl bless`). An entry at +0 failed to start: every
//!   entry of its image goes to the end of the menu, marked so, and is
//!   not chosen by default: the next one is (the previous image). A
//!   Rustica entry's command line also gets `BOOT_IMAGE=PATH`;
//! - `\loader\loader.conf`: `default` (an entry id, `*` at the end matches
//!   any rest), `timeout` (seconds; 0 boots at once unless a key is held),
//!   `editor` (no: no `e`), `auto-entries`, `auto-firmware` (no: none).
//!
//! Keys: a number (1-9) starts that entry, Up/Down (Home/End) and Enter, `e` edits the command line once,
//! `d` makes the selection the default (the LoaderEntryDefault variable),
//! any key stops the countdown. It speaks systemd-boot's loader variables
//! (LoaderInfo, LoaderEntries, LoaderEntryDefault, LoaderEntryOneShot,
//! LoaderEntrySelected, LoaderConfigTimeout, LoaderDevicePartUUID), so
//! `bootctl` works with it. What it shows is also written to the debug
//! port (QEMU's debugcon) for the tests.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::prelude::*;
use uefi::proto::console::text::{Color, Key, ScanCode};
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::{DevicePath, DeviceSubType, DeviceType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, ResetType, VariableAttributes, VariableVendor};
use uefi::{guid, CString16, Guid};

const VERSION: &str = "rboot 0.1";
/// systemd's boot loader interface
const LOADER_GUID: Guid = guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f");

#[derive(Clone, PartialEq)]
enum Kind {
    Rustica,
    Other,
    Firmware,
}

#[derive(Clone)]
struct Entry {
    id: String,
    title: String,
    kind: Kind,
    fs: Option<Handle>,
    path: String,
    options: String,
    sort: String,
    /// The .conf file of an entry with a boot counter (not for the
    /// recovery and live entries made from it)
    file: Option<String>,
    /// Tries left and done (`+LEFT-DONE` in the file name)
    tries: Option<(u32, u32)>,
    /// Its image failed to start (an entry of it at +0)
    bad: bool,
}

struct Config {
    default: String,
    timeout: Option<u32>,
    editor: bool,
    auto_entries: bool,
    auto_firmware: bool,
}

// ---- output ----

fn debugcon(s: &str) {
    for b in s.bytes() {
        unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags)) };
    }
}

fn put(s: &str) {
    if let Ok(c) = CString16::try_from(s) {
        system::with_stdout(|o| {
            let _ = o.output_string(&c);
        });
    }
}

fn color(fg: Color, bg: Color) {
    system::with_stdout(|o| {
        let _ = o.set_color(fg, bg);
    });
}

fn at(col: usize, row: usize) {
    system::with_stdout(|o| {
        let _ = o.set_cursor_position(col, row);
    });
}

fn screen_size() -> (usize, usize) {
    system::with_stdout(|o| o.current_mode().ok().flatten().map(|m| (m.columns(), m.rows())).unwrap_or((80, 25)))
}

fn read_key() -> Option<Key> {
    system::with_stdin(|i| i.read_key().ok().flatten())
}

fn wait_key() -> Key {
    loop {
        if let Some(k) = read_key() {
            return k;
        }
        boot::stall(Duration::from_millis(20));
    }
}

// ---- files ----

fn wide(path: &str) -> Option<CString16> {
    CString16::try_from(path.replace('/', "\\").as_str()).ok()
}

fn open_root(fs: Handle) -> Option<Directory> {
    let mut sfs = boot::open_protocol_exclusive::<SimpleFileSystem>(fs).ok()?;
    sfs.open_volume().ok()
}

fn read_file(fs: Handle, path: &str) -> Option<Vec<u8>> {
    let mut root = open_root(fs)?;
    let h = root.open(&wide(path)?, FileMode::Read, FileAttribute::empty()).ok()?;
    let mut f = h.into_regular_file()?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16384];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(out)
}

fn exists(fs: Handle, path: &str) -> bool {
    let Some(mut root) = open_root(fs) else { return false };
    let Some(p) = wide(path) else { return false };
    root.open(&p, FileMode::Read, FileAttribute::empty()).is_ok()
}

/// Names in a directory (not . and ..), with whether each is a directory
fn list_dir(fs: Handle, path: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let Some(mut root) = open_root(fs) else { return out };
    let Some(p) = wide(path) else { return out };
    let Ok(h) = root.open(&p, FileMode::Read, FileAttribute::empty()) else { return out };
    let Some(mut d) = h.into_directory() else { return out };
    while let Ok(Some(info)) = d.read_entry_boxed() {
        let name = info.file_name().to_string();
        if name == "." || name == ".." {
            continue;
        }
        out.push((name, info.attribute().contains(FileAttribute::DIRECTORY)));
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

// ---- variables ----

fn set_var(name: &str, data: &[u8], persistent: bool) {
    let Ok(n) = CString16::try_from(name) else { return };
    let mut attrs = VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    if persistent {
        attrs |= VariableAttributes::NON_VOLATILE;
    }
    let _ = runtime::set_variable(&n, &VariableVendor(LOADER_GUID), attrs, data);
}

fn utf16z(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for c in s.encode_utf16().chain(core::iter::once(0)) {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v
}

fn get_var_bytes(vendor: &VariableVendor, name: &str) -> Option<Vec<u8>> {
    let n = CString16::try_from(name).ok()?;
    let mut buf = vec![0u8; 4096];
    let (data, _) = runtime::get_variable(&n, vendor, &mut buf).ok()?;
    Some(data.to_vec())
}

fn get_var_str(name: &str) -> Option<String> {
    let b = get_var_bytes(&VariableVendor(LOADER_GUID), name)?;
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16(&u).ok()
}

fn delete_var(name: &str) {
    if let Ok(n) = CString16::try_from(name) {
        let _ = runtime::set_variable(&n, &VariableVendor(LOADER_GUID), VariableAttributes::empty(), &[]);
    }
}

// ---- configuration and entries ----

fn key_value(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (k, v) = line.split_once(|c: char| c == ' ' || c == '\t')?;
    Some((k, v.trim()))
}

fn yes(v: &str) -> bool {
    matches!(v, "1" | "yes" | "y" | "true" | "on")
}

fn read_config(esp: Handle) -> Config {
    let mut c = Config { default: String::new(), timeout: None, editor: true, auto_entries: true, auto_firmware: true };
    if let Some(b) = read_file(esp, "\\loader\\loader.conf") {
        for line in text(&b).lines() {
            match key_value(line) {
                Some(("default", v)) => c.default = v.to_string(),
                Some(("timeout", v)) => c.timeout = if v == "menu-force" { Some(u32::MAX) } else { v.parse().ok() },
                Some(("editor", v)) => c.editor = yes(v),
                Some(("auto-entries", v)) => c.auto_entries = yes(v),
                Some(("auto-firmware", v)) => c.auto_firmware = yes(v),
                _ => {}
            }
        }
    }
    c
}

/// rustux.efi, or a system image rpg installed: rustux-VERSION.efi
/// (package step E3)
fn is_rustica(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit(['\\', '/']).next().unwrap_or("");
    name == "rustux.efi" || (name.starts_with("rustux-") && name.ends_with(".efi"))
}

/// `ID+LEFT[-DONE]`: the id and the counter
fn split_counter(stem: &str) -> (&str, Option<(u32, u32)>) {
    if let Some(k) = stem.rfind('+') {
        let rest = &stem[k + 1..];
        let (l, d) = rest.split_once('-').unwrap_or((rest, "0"));
        let num = |t: &str| if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) { t.parse::<u32>().ok() } else { None };
        if let (Some(l), Some(d)) = (num(l), num(d)) {
            return (&stem[..k], Some((l, d)));
        }
    }
    (stem, None)
}

/// Rename a file in \loader\entries (FAT renames through SetInfo)
fn rename_entry(fs: Handle, from: &str, to: &str) -> bool {
    let Some(mut root) = open_root(fs) else { return false };
    let Some(p) = wide(&format!("\\loader\\entries\\{}", from)) else { return false };
    let Ok(h) = root.open(&p, FileMode::ReadWrite, FileAttribute::empty()) else { return false };
    let Some(mut f) = h.into_regular_file() else { return false };
    let Ok(info) = f.get_boxed_info::<FileInfo>() else { return false };
    let Ok(name) = CString16::try_from(to) else { return false };
    let mut storage = vec![0u64; 128];
    let buf = unsafe { core::slice::from_raw_parts_mut(storage.as_mut_ptr() as *mut u8, storage.len() * 8) };
    let Ok(new) = FileInfo::new(buf, info.file_size(), info.physical_size(), *info.create_time(), *info.last_access_time(), *info.modification_time(), info.attribute(), &name) else {
        return false;
    };
    let ok = f.set_info(new).is_ok();
    let _ = f.flush();
    ok
}

fn conf_entries(esp: Handle) -> Vec<Entry> {
    let mut out = Vec::new();
    for (name, dir) in list_dir(esp, "\\loader\\entries") {
        if dir || !name.to_ascii_lowercase().ends_with(".conf") {
            continue;
        }
        let Some(b) = read_file(esp, &format!("\\loader\\entries\\{}", name)) else { continue };
        let (id, tries) = split_counter(&name[..name.len() - 5]);
        let id = id.to_string();
        let mut e = Entry {
            id: id.clone(),
            title: id.clone(),
            kind: Kind::Other,
            fs: Some(esp),
            path: String::new(),
            options: String::new(),
            sort: String::new(),
            file: tries.map(|_| name.clone()),
            tries,
            bad: false,
        };
        let mut bootfs = String::new();
        for line in text(&b).lines() {
            match key_value(line) {
                Some(("title", v)) => e.title = v.to_string(),
                Some(("efi", v)) | Some(("linux", v)) => e.path = v.replace('/', "\\"),
                Some(("options", v)) => {
                    if !e.options.is_empty() {
                        e.options.push(' ');
                    }
                    e.options.push_str(v);
                }
                Some(("bootfs", v)) => bootfs = v.to_string(),
                Some(("sort-key", v)) => e.sort = v.to_string(),
                _ => {}
            }
        }
        if e.path.is_empty() {
            continue;
        }
        if !e.path.starts_with('\\') {
            e.path.insert(0, '\\');
        }
        if !bootfs.is_empty() {
            e.options = format!("{} bootfs={}", e.options, bootfs).trim().to_string();
        }
        if is_rustica(&e.path) {
            e.kind = Kind::Rustica;
        }
        out.push(e);
    }
    // sort-key, then id (as systemd-boot)
    out.sort_by(|a, b| {
        let ka = if a.sort.is_empty() { "~" } else { a.sort.as_str() };
        let kb = if b.sort.is_empty() { "~" } else { b.sort.as_str() };
        ka.cmp(kb).then(a.id.cmp(&b.id))
    });
    out
}

fn without_word(options: &str, key: &str) -> String {
    options.split(' ').filter(|w| !w.is_empty() && *w != key && !w.starts_with(&format!("{}=", key))).collect::<Vec<_>>().join(" ")
}

/// Recovery and live entries next to each Rustica entry
fn add_rustica_extras(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut live_done = entries.iter().any(|e| e.kind == Kind::Rustica && e.options.split(' ').any(|w| w == "root=live"));
    for e in entries {
        let rustica = e.kind == Kind::Rustica;
        let opts = e.options.clone();
        out.push(e.clone());
        if !rustica {
            continue;
        }
        let words: Vec<&str> = opts.split(' ').collect();
        // (not for the installer's entry, nor one that is a rescue entry
        // already, nor the desktop's: its recovery is the other's)
        if !words.iter().any(|w| *w == "rescue" || *w == "single" || *w == "installer" || *w == "desktop") {
            let mut r = e.clone();
            r.file = None;
            r.tries = None;
            r.id = format!("{}-recovery", e.id);
            r.title = format!("{} (recovery)", e.title);
            r.options = format!("{} rescue", without_word(&without_word(&opts, "autologin"), "quiet")).trim().to_string();
            out.push(r);
        }
        let has_root = words.iter().any(|w| w.starts_with("root=") && *w != "root=live");
        if has_root && !live_done {
            let mut l = e.clone();
            l.file = None;
            l.tries = None;
            l.id = format!("{}-live", e.id);
            l.title = format!("{} (live, in memory)", e.title);
            l.options = format!("{} root=live", without_word(&opts, "root")).trim().to_string();
            out.push(l);
            live_done = true;
        }
    }
    out
}

fn fs_handles() -> Vec<Handle> {
    boot::find_handles::<SimpleFileSystem>().unwrap_or_default()
}

/// Windows and Linux on any EFI partition
fn auto_entries(esp: Handle, have: &[Entry]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut n_win = 0;
    for fs in fs_handles() {
        let mut found: Vec<(String, String, String)> = Vec::new(); // id, title, path
        if exists(fs, "\\EFI\\Microsoft\\Boot\\bootmgfw.efi") {
            n_win += 1;
            let id = if n_win == 1 { "auto-windows".to_string() } else { format!("auto-windows-{}", n_win) };
            found.push((id, "Windows Boot Manager".into(), "\\EFI\\Microsoft\\Boot\\bootmgfw.efi".into()));
        }
        for (dir, is_dir) in list_dir(fs, "\\EFI") {
            let low = dir.to_ascii_lowercase();
            if !is_dir || low == "boot" || low == "microsoft" || low == "rustica" {
                continue;
            }
            for prog in ["shimx64.efi", "grubx64.efi", "systemd-bootx64.efi"] {
                let path = format!("\\EFI\\{}\\{}", dir, prog);
                if exists(fs, &path) {
                    let mut name = dir.clone();
                    if let Some(c) = name.get_mut(0..1) {
                        c.make_ascii_uppercase();
                    }
                    found.push((format!("auto-{}", low), format!("Linux ({})", name), path));
                    break;
                }
            }
        }
        for (id, title, path) in found {
            // Not if an entry of the configuration starts it already
            if have.iter().any(|e| e.path.eq_ignore_ascii_case(&path) && e.fs == Some(fs)) {
                continue;
            }
            let mut id = id;
            if out.iter().any(|e| e.id == id) {
                id = format!("{}-{}", id, out.len() + 1);
            }
            out.push(Entry { id, title, kind: Kind::Other, fs: Some(fs), path, options: String::new(), sort: String::new(), file: None, tries: None, bad: false });
        }
    }
    let _ = esp;
    out
}

fn firmware_setup_supported() -> bool {
    let Some(b) = get_var_bytes(&VariableVendor::GLOBAL_VARIABLE, "OsIndicationsSupported") else { return false };
    b.len() >= 8 && u64::from_le_bytes(b[..8].try_into().unwrap()) & 1 != 0
}

fn all_entries(esp: Handle, cfg: &Config) -> Vec<Entry> {
    let mut entries = conf_entries(esp);
    // No configuration: the kernel next to us
    if !entries.iter().any(|e| e.kind == Kind::Rustica) && exists(esp, "\\EFI\\Rustica\\rustux.efi") {
        entries.insert(0, Entry {
            id: "rustica".into(),
            title: "Rustica".into(),
            kind: Kind::Rustica,
            fs: Some(esp),
            path: "\\EFI\\Rustica\\rustux.efi".into(),
            options: String::new(),
            sort: String::new(),
            file: None,
            tries: None,
            bad: false,
        });
    }
    let mut entries = add_rustica_extras(entries);
    // Images that failed to start: all their entries last
    let bad: Vec<String> = entries.iter().filter(|e| e.kind == Kind::Rustica && e.tries.is_some_and(|t| t.0 == 0)).map(|e| e.path.to_ascii_lowercase()).collect();
    if !bad.is_empty() {
        for e in entries.iter_mut() {
            if e.kind == Kind::Rustica && bad.contains(&e.path.to_ascii_lowercase()) {
                e.bad = true;
                e.title = format!("{} (failed to start)", e.title);
            }
        }
    }
    let (good, failed): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|e| !e.bad);
    let mut entries = good;
    if cfg.auto_entries {
        let mut have = entries.clone();
        have.extend(failed.iter().cloned());
        let auto = auto_entries(esp, &have);
        entries.extend(auto);
    }
    if cfg.auto_firmware && firmware_setup_supported() {
        entries.push(Entry {
            id: "auto-reboot-to-firmware-setup".into(),
            title: "Firmware setup".into(),
            kind: Kind::Firmware,
            fs: None,
            path: String::new(),
            options: String::new(),
            sort: String::new(),
            file: None,
            tries: None,
            bad: false,
        });
    }
    entries.extend(failed);
    entries
}

fn pattern_match(pat: &str, id: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => pat == id || pat.strip_suffix(".conf") == Some(id),
    }
}

// ---- booting ----

fn device_path_of(fs: Handle) -> Option<ScopedProtocol<DevicePath>> {
    unsafe {
        boot::open_protocol::<DevicePath>(
            OpenProtocolParams { handle: fs, agent: boot::image_handle(), controller: None },
            OpenProtocolAttributes::GetProtocol,
        )
        .ok()
    }
}

/// The ESP's partition GUID (from its device path), as text
fn partition_uuid(fs: Handle) -> Option<String> {
    let dp = device_path_of(fs)?;
    for node in dp.node_iter() {
        if node.device_type() == DeviceType::MEDIA && node.sub_type() == DeviceSubType::MEDIA_HARD_DRIVE {
            let d = node.data();
            // partition number (4), start (8), size (8), signature (16), format (1), type (1)
            if d.len() >= 38 && d[37] == 2 {
                let g = Guid::from_bytes(d[20..36].try_into().ok()?);
                return Some(format!("{}", g));
            }
        }
    }
    None
}

fn boot_entry(e: &Entry, options: &str) -> Result<(), String> {
    if e.kind == Kind::Firmware {
        let mut ind = get_var_bytes(&VariableVendor::GLOBAL_VARIABLE, "OsIndications").unwrap_or_default();
        ind.resize(8, 0);
        let v = u64::from_le_bytes(ind[..8].try_into().unwrap()) | 1;
        let name = CString16::try_from("OsIndications").unwrap();
        let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
        runtime::set_variable(&name, &VariableVendor::GLOBAL_VARIABLE, attrs, &v.to_le_bytes()).map_err(|e| format!("cannot set OsIndications: {:?}", e.status()))?;
        debugcon("rboot: rebooting into the firmware setup\n");
        runtime::reset(ResetType::COLD, Status::SUCCESS, None);
    }
    let fs = e.fs.ok_or("no partition")?;
    let dp = device_path_of(fs).ok_or("no device path for the partition")?;
    let file = wide(&e.path).ok_or("bad path")?;
    let mut buf = Vec::new();
    let mut b = DevicePathBuilder::with_vec(&mut buf);
    for node in dp.node_iter() {
        b = b.push(&node).map_err(|_| "device path too long")?;
    }
    b = b.push(&build::media::FilePath { path_name: &file }).map_err(|_| "device path too long")?;
    let full = b.finalize().map_err(|_| "device path")?;
    drop(dp);
    // Boot counting: one try fewer, before it starts
    let mut count_path = String::new();
    if let (Some(file), Some((left, done))) = (&e.file, e.tries) {
        if left > 0 {
            let to = format!("{}+{}-{}.conf", e.id, left - 1, done + 1);
            if rename_entry(fs, file, &to) {
                count_path = format!("\\loader\\entries\\{}", to);
                debugcon(&format!("rboot: {}: try {} of {} ({})\n", e.id, done + 1, left + done, to));
            } else {
                debugcon(&format!("rboot: {}: could not count this try (renaming {} failed)\n", e.id, file));
            }
        }
    }
    if count_path.is_empty() {
        delete_var("LoaderBootCountPath");
    } else {
        set_var("LoaderBootCountPath", &utf16z(&count_path), false);
    }
    let image = boot::load_image(boot::image_handle(), LoadImageSource::FromDevicePath { device_path: full, boot_policy: uefi::proto::BootPolicy::ExactMatch })
        .map_err(|err| format!("cannot load {}: {:?}", e.path, err.status()))?;
    // The command line: UTF-16 with its NUL; it must live until the program has read it.
    // The kernel learns which image it is (as GRUB's BOOT_IMAGE)
    let full_opts = if e.kind == Kind::Rustica { format!("{} BOOT_IMAGE={}", options, e.path).trim().to_string() } else { options.to_string() };
    let opts: Vec<u16> = full_opts.encode_utf16().chain(core::iter::once(0)).collect();
    let opts = alloc::boxed::Box::leak(opts.into_boxed_slice());
    if !full_opts.is_empty() {
        let mut li = boot::open_protocol_exclusive::<LoadedImage>(image).map_err(|_| "no loaded image protocol")?;
        unsafe { li.set_load_options(opts.as_ptr() as *const u8, (opts.len() * 2) as u32) };
    }
    set_var("LoaderEntrySelected", &utf16z(&e.id), false);
    debugcon(&format!("rboot: booting {} ({}{}{})\n", e.id, e.path, if options.is_empty() { "" } else { " " }, options));
    system::with_stdout(|o| {
        let _ = o.set_color(Color::LightGray, Color::Black);
        let _ = o.clear();
    });
    let r = boot::start_image(image);
    let _ = boot::unload_image(image);
    match r {
        Ok(()) => Err(format!("{} returned", e.title)),
        Err(err) => Err(format!("{} failed: {:?}", e.title, err.status())),
    }
}

// ---- the menu ----

/// The menu's column: centred, at most 76 wide
fn layout(cols: usize) -> (usize, usize) {
    let w = cols.saturating_sub(4).min(76).max(20);
    ((cols.saturating_sub(w)) / 2, w)
}

fn pad(s: &str, w: usize) -> String {
    let mut line: String = s.chars().take(w).collect();
    while line.chars().count() < w {
        line.push(' ');
    }
    line
}

// The first row of the list, below the header
const LIST_TOP: usize = 7;
// Rows of the whole block: header, list, a gap, help (2), countdown (2)
static MENU_Y: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The menu's first row: the block centred on the screen
fn set_menu_y(n: usize, rows: usize) -> usize {
    let list = n.min(rows.saturating_sub(LIST_TOP + 6).max(1));
    let need = LIST_TOP + list + 6;
    let y = rows.saturating_sub(need) / 2;
    MENU_Y.store(y, core::sync::atomic::Ordering::Relaxed);
    y
}

fn draw(entries: &[Entry], sel: usize, top: usize, rows: usize, cols: usize, status: &str) {
    color(Color::LightGray, Color::Black);
    system::with_stdout(|o| {
        let _ = o.clear();
        let _ = o.enable_cursor(false);
    });
    let (x, w) = layout(cols);
    let y = set_menu_y(entries.len(), rows);
    // The header: a frame with the name, the release and the version
    let inner = w.saturating_sub(2);
    let centre = |t: &str| {
        let n = t.chars().count().min(inner);
        let l = (inner - n) / 2;
        format!("{}{}", " ".repeat(l), pad(t, inner - l))
    };
    color(Color::Red, Color::Black);
    at(x, y + 1);
    put(&format!("\u{250C}{}\u{2510}", "\u{2500}".repeat(inner)));
    at(x, y + 2);
    put("\u{2502}");
    color(Color::White, Color::Black);
    put(&centre("R U S T I C A"));
    color(Color::Red, Color::Black);
    put("\u{2502}");
    at(x, y + 3);
    put("\u{2502}");
    color(Color::DarkGray, Color::Black);
    put(&centre(&format!("RedFox  \u{00B7}  {}", VERSION)));
    color(Color::Red, Color::Black);
    put("\u{2502}");
    at(x, y + 4);
    put(&format!("\u{2514}{}\u{2518}", "\u{2500}".repeat(inner)));
    color(Color::LightGray, Color::Black);
    at(x + 1, y + 5);
    put("Choose how to start:");
    // The entries, numbered (1-9 start one at once)
    let list_rows = rows.saturating_sub(LIST_TOP + 6).max(1);
    for (k, e) in entries.iter().enumerate().skip(top).take(list_rows) {
        at(x, y + LIST_TOP + k - top);
        let num = if k < 9 { format!("{}", k + 1) } else { " ".to_string() };
        let line = pad(&format!("  {}   {}", num, e.title), w);
        if k == sel {
            color(Color::White, Color::Red);
        } else if e.bad {
            color(Color::DarkGray, Color::Black);
        } else {
            color(Color::LightGray, Color::Black);
        }
        put(&line);
    }
    let shown = entries.len().min(list_rows);
    let foot = y + LIST_TOP + shown + 1;
    color(Color::LightGray, Color::Black);
    at(x + 1, foot);
    put(&pad("Press a number, or use \u{2191} \u{2193} and Enter.", w.saturating_sub(1)));
    color(Color::DarkGray, Color::Black);
    at(x + 1, foot + 1);
    put(&pad("e: edit the command line   d: make it the default", w.saturating_sub(1)));
    STATUS_ROW.store((foot + 4).min(rows.saturating_sub(2)), core::sync::atomic::Ordering::Relaxed);
    status_line(status, rows, cols);
}

// The countdown's two rows (and the status) below the help
fn bottom_row(rows: usize) -> usize {
    let _ = rows;
    STATUS_ROW.load(core::sync::atomic::Ordering::Relaxed)
}
static STATUS_ROW: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(23);

fn status_line(text: &str, rows: usize, cols: usize) {
    let (x, w) = layout(cols);
    color(Color::LightGray, Color::Black);
    at(0, bottom_row(rows));
    put(&pad("", cols.saturating_sub(1)));
    at(x + 1, bottom_row(rows));
    put(&pad(text, w.saturating_sub(1)));
}

/// The countdown: what starts by itself, and when, with a bar
fn countdown_line(title: &str, left: u32, total: u32, rows: usize, cols: usize) {
    let (x, w) = layout(cols);
    let row = bottom_row(rows);
    color(Color::LightGray, Color::Black);
    at(0, row.saturating_sub(1));
    put(&pad("", cols.saturating_sub(1)));
    at(x + 1, row.saturating_sub(1));
    color(Color::Yellow, Color::Black);
    put(&pad(
        &format!("No choice in {} second{}: {} starts by itself.", left, if left == 1 { "" } else { "s" }, title),
        w.saturating_sub(1),
    ));
    let bar = w.saturating_sub(2);
    let full = if total == 0 { 0 } else { (bar * left as usize) / total as usize };
    at(0, row);
    color(Color::LightGray, Color::Black);
    put(&pad("", cols.saturating_sub(1)));
    at(x + 1, row);
    color(Color::Red, Color::Black);
    put(&"\u{2588}".repeat(full));
    color(Color::DarkGray, Color::Black);
    put(&"\u{2591}".repeat(bar - full));
}

/// Edit the command line on the status line: Enter boots, Esc cancels
fn edit(initial: &str, rows: usize, cols: usize) -> Option<String> {
    let mut s = String::from(initial);
    loop {
        let shown = format!("options: {}_", s);
        let tail: String = if shown.chars().count() > cols.saturating_sub(4) {
            shown.chars().skip(shown.chars().count() - cols.saturating_sub(4)).collect()
        } else {
            shown
        };
        status_line(&tail, rows, cols);
        match wait_key() {
            Key::Special(ScanCode::ESCAPE) => return None,
            Key::Printable(c) => {
                let ch: char = c.into();
                match ch {
                    '\r' | '\n' => return Some(s),
                    '\u{8}' => {
                        s.pop();
                    }
                    c if (' '..='~').contains(&c) && s.len() < 500 => s.push(c),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[entry]
fn main() -> Status {
    let me = boot::image_handle();
    let esp = match boot::open_protocol_exclusive::<LoadedImage>(me).ok().and_then(|li| li.device()) {
        Some(h) => h,
        None => return Status::LOAD_ERROR,
    };
    system::with_stdin(|i| {
        let _ = i.reset(false);
    });
    let cfg = read_config(esp);
    let entries = all_entries(esp, &cfg);
    // The loader interface, for bootctl
    set_var("LoaderInfo", &utf16z(VERSION), false);
    let mut list = Vec::new();
    for e in &entries {
        list.extend(utf16z(&e.id));
    }
    set_var("LoaderEntries", &list, false);
    if let Some(u) = partition_uuid(esp) {
        set_var("LoaderDevicePartUUID", &utf16z(&u.to_ascii_uppercase()), false);
    }
    debugcon(&format!("rboot: {} entries:", entries.len()));
    for e in &entries {
        debugcon(&format!(" {}", e.id));
    }
    debugcon("\n");
    if entries.is_empty() {
        put("rboot: nothing to boot (no \\loader\\entries, no \\EFI\\Rustica\\rustux.efi)\r\n");
        boot::stall(Duration::from_secs(10));
        return Status::NOT_FOUND;
    }
    // The default: a one-shot choice, the saved one, the configuration's, the first
    let oneshot = get_var_str("LoaderEntryOneShot");
    if oneshot.is_some() {
        delete_var("LoaderEntryOneShot");
    }
    let saved = get_var_str("LoaderEntryDefault");
    let mut sel = 0;
    for (i, pat) in [oneshot.clone(), saved, Some(cfg.default.clone())].into_iter().enumerate() {
        let Some(pat) = pat else { continue };
        if pat.is_empty() {
            continue;
        }
        if let Some(k) = entries.iter().position(|e| pattern_match(&pat, &e.id)) {
            // A failed image is not the default (a one-shot choice is kept)
            if entries[k].bad && i > 0 {
                let next = entries.iter().position(|e| !e.bad && e.kind != Kind::Firmware).unwrap_or(k);
                debugcon(&format!("rboot: {} failed to start: {} instead\n", entries[k].id, entries[next].id));
                sel = next;
            } else {
                sel = k;
            }
            break;
        }
    }
    // The timeout: bootctl's (one-shot, then saved), the configuration's, 5 s
    let oneshot_timeout = get_var_str("LoaderConfigTimeoutOneShot").and_then(|v| v.parse().ok());
    if oneshot_timeout.is_some() {
        delete_var("LoaderConfigTimeoutOneShot");
    }
    let timeout = oneshot_timeout
        .or_else(|| get_var_str("LoaderConfigTimeout").and_then(|v| if v == "menu-force" { Some(u32::MAX) } else { v.parse().ok() }))
        .or(cfg.timeout)
        .unwrap_or(5);
    // timeout 0: straight to the default, unless a key is held
    if (timeout == 0 || oneshot.is_some()) && read_key().is_none() {
        let e = entries[sel].clone();
        let opts = e.options.clone();
        if let Err(msg) = boot_entry(&e, &opts) {
            debugcon(&format!("rboot: {}\n", msg));
        }
    }
    let (cols, rows) = screen_size();
    let mut top = 0;
    let list_rows = rows.saturating_sub(LIST_TOP + 6).max(1);
    let mut left: Option<u32> = if timeout == u32::MAX || timeout == 0 { None } else { Some(timeout * 10) };
    let mut status = String::new();
    let mut redraw = true;
    loop {
        if sel < top {
            top = sel;
        }
        if sel >= top + list_rows {
            top = sel + 1 - list_rows;
        }
        if redraw {
            draw(&entries, sel, top, rows, cols, &status);
            redraw = false;
        }
        if let Some(t) = left {
            if t % 10 == 0 {
                countdown_line(&entries[sel].title, t / 10, timeout, rows, cols);
                debugcon(&format!("rboot: booting {} in {} s\n", entries[sel].id, t / 10));
            }
        }
        let key = match read_key() {
            Some(k) => k,
            None => {
                boot::stall(Duration::from_millis(100));
                if let Some(t) = left {
                    if t == 0 {
                        left = None;
                        let e = entries[sel].clone();
                        let o = e.options.clone();
                        status = boot_entry(&e, &o).err().unwrap_or_default();
                        redraw = true;
                    } else {
                        left = Some(t - 1);
                    }
                }
                continue;
            }
        };
        if left.take().is_some() {
            redraw = true; // the countdown's lines away
        }
        match key {
            Key::Special(ScanCode::UP) => {
                sel = sel.saturating_sub(1);
                redraw = true;
            }
            Key::Special(ScanCode::DOWN) => {
                if sel + 1 < entries.len() {
                    sel += 1;
                }
                redraw = true;
            }
            Key::Special(ScanCode::HOME) => {
                sel = 0;
                redraw = true;
            }
            Key::Special(ScanCode::END) => {
                sel = entries.len() - 1;
                redraw = true;
            }
            Key::Printable(c) => {
                let ch: char = c.into();
                match ch {
                    '\r' | '\n' => {
                        let e = entries[sel].clone();
                        let o = e.options.clone();
                        status = boot_entry(&e, &o).err().unwrap_or_default();
                        debugcon(&format!("rboot: {}\n", status));
                        redraw = true;
                    }
                    'e' if cfg.editor && entries[sel].kind != Kind::Firmware => {
                        let e = entries[sel].clone();
                        if let Some(o) = edit(&e.options, rows, cols) {
                            status = boot_entry(&e, &o).err().unwrap_or_default();
                        }
                        redraw = true;
                    }
                    // A number starts that entry at once
                    '1'..='9' if ((ch as usize) - ('1' as usize)) < entries.len() => {
                        sel = (ch as usize) - ('1' as usize);
                        let e = entries[sel].clone();
                        let o = e.options.clone();
                        draw(&entries, sel, top.min(sel), rows, cols, "");
                        status = boot_entry(&e, &o).err().unwrap_or_default();
                        debugcon(&format!("rboot: {}\n", status));
                        redraw = true;
                    }
                    'd' => {
                        set_var("LoaderEntryDefault", &utf16z(&entries[sel].id), true);
                        status = format!("{} is the default now", entries[sel].title);
                        debugcon(&format!("rboot: default {}\n", entries[sel].id));
                        redraw = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

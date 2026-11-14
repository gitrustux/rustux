// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fakeos: a stand-in for another system's boot loader in the boot manager
//! tests (scripts/run-rboot-test.sh). Placed where Windows or Linux keep
//! theirs, it writes "FAKEOS <its path> [<its options>]" to the debug port
//! and on the screen, then powers the machine off. Not shipped.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::runtime::ResetType;

#[entry]
fn main() -> Status {
    let (path, opts) = match boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()) {
        Ok(li) => {
            let path = li
                .file_path()
                .and_then(|p| p.to_string(uefi::proto::device_path::text::DisplayOnly(true), uefi::proto::device_path::text::AllowShortcuts(true)).ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let opts = li.load_options_as_cstr16().map(|s| s.to_string()).unwrap_or_default();
            (path, opts)
        }
        Err(_) => (String::new(), String::new()),
    };
    let line = format!("FAKEOS {} [{}]\n", path, opts);
    for b in line.bytes() {
        unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags)) };
    }
    if let Ok(s) = uefi::CString16::try_from(line.as_str()) {
        system::with_stdout(|o| {
            let _ = o.output_string(&s);
        });
    }
    boot::stall(core::time::Duration::from_millis(500));
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None);
}

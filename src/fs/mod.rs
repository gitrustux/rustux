// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Filesystem Layer
//!
//! This module provides filesystem functionality for the Rustux kernel.
//! It includes:
//! - Ramdisk (embedded read-only filesystem)
//! - VFS (Virtual File System) abstraction
//! - File operations for reading/writing files

pub mod ramdisk;
pub mod vfs;

// Re-export commonly used types
pub use ramdisk::{
    Ramdisk, RamdiskFile, RamdiskSuperblock,
    RAMDISK, init_ramdisk, get_ramdisk, FileOffset,
    Errno,
    rxstatus_to_errno, ENOSYS,
};

pub use vfs::{
    FileOps, RamdiskFileOps,
    Whence,
    open_ramdisk_file,
};

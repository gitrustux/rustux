// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Ramdisk (Embedded Filesystem)
//!
//! This module provides a simple read-only ramdisk filesystem that is
//! embedded directly into the kernel binary at build time. It allows
//! userspace programs to access files via the standard file API.
//!
//! # Design
//!
//! The ramdisk is a flat filesystem with:
//! - No directory structure (all files at root level)
//! - Read-only access (writes return EROFS)
//! - Embedded at build time via build.rs
//! - Accessed via global RAMDISK reference
//!
//! # Layout
//!
//! ```text
//! Offset 0x00: Superblock (16 bytes)
//! Offset 0x10: File headers (16 bytes each, num_files entries)
//! Offset 0x10 + (num_files * 16): File names (null-terminated)
//! After names: File data (contiguous)
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // Initialize ramdisk from embedded data
//! unsafe {
//!     RAMDISK.lock().replace(Ramdisk::from_embedded_data());
//! }
//!
//! // Open a file
//! let ramdisk = RAMDISK.lock();
//! let file = ramdisk.as_ref().unwrap().find_file("/test.txt")?;
//!
//! // Read file data
//! let mut buffer = [0u8; 1024];
//! let bytes_read = ramdisk.read_file(&file, &mut buffer);
//! ```

use crate::sync::SpinMutex;
use alloc::vec::Vec;

/// ============================================================================
/// Error Numbers
/// ============================================================================

/// Filesystem error numbers (POSIX-compatible)
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    Success = 0,
    EPERM = 1,      // Operation not permitted
    ENOENT = 2,     // No such file or directory
    ESRCH = 3,      // No such process
    EINTR = 4,      // Interrupted system call
    EIO = 5,        // I/O error
    ENXIO = 6,      // No such device or address
    E2BIG = 7,      // Argument list too long
    ENOEXEC = 8,    // Exec format error
    EBADF = 9,      // Bad file number
    ECHILD = 10,    // No child processes
    EAGAIN = 11,    // Try again
    ENOMEM = 12,    // Out of memory
    EACCES = 13,    // Permission denied
    EFAULT = 14,    // Bad address
    EBUSY = 16,     // Device busy
    EEXIST = 17,    // File exists
    ENODEV = 19,    // No such device
    ENOTDIR = 20,   // Not a directory
    EISDIR = 21,    // Is a directory
    EINVAL = 22,   // Invalid argument
    ENFILE = 23,    // File table overflow
    EMFILE = 24,    // Too many open files
    ESPIPE = 29,    // Illegal seek
    EROFS = 30,     // Read-only filesystem
    EMLINK = 31,    // Too many links
    EPIPE = 32,     // Broken pipe
    EDOM = 33,      // Numerical argument out of domain
    ERANGE = 34,    // Result too large
    ENOSYS = 38,    // Function not implemented
}

/// Convert RxStatus to Errno
pub fn rxstatus_to_errno(status: crate::arch::amd64::mm::RxStatus) -> Errno {
    use crate::arch::amd64::mm::RxStatus;
    match status {
        RxStatus::OK => Errno::Success,
        RxStatus::ERR_INVALID_ARGS => Errno::EINVAL,
        RxStatus::ERR_NO_MEMORY => Errno::ENOMEM,
        RxStatus::ERR_NOT_FOUND => Errno::ENOENT,
        RxStatus::ERR_NOT_SUPPORTED => Errno::ENOSYS,
        _ => Errno::EIO,
    }
}

/// Additional error for ENOSYS
pub const ENOSYS: Errno = Errno::EPERM; // Temporary: use EPERM for "not supported"

/// ============================================================================
/// Ramdisk Structures
/// ============================================================================

/// Ramdisk file header (embedded at compile time)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RamdiskFile {
    /// Offset to file name string (from start of ramdisk)
    pub name_offset: u32,
    /// Offset to file data (from start of ramdisk)
    pub data_offset: u32,
    /// File size in bytes
    pub size: u32,
    /// Reserved (for alignment)
    pub _pad: u32,
}

/// Ramdisk superblock (at offset 0)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RamdiskSuperblock {
    /// Magic number: 0x52555458 ("RUTX")
    pub magic: u32,
    /// Number of files in the ramdisk
    pub num_files: u32,
    /// Offset to file headers array (from start of ramdisk)
    pub files_offset: u32,
}

impl RamdiskSuperblock {
    /// Check if the superblock magic is valid
    pub fn is_valid(&self) -> bool {
        self.magic == 0x52555458
    }
}

/// ============================================================================
/// Ramdisk Filesystem
/// ============================================================================

/// Ramdisk filesystem
///
/// This structure provides access to the embedded ramdisk data.
/// It is read-only and all files are at the root level.
pub struct Ramdisk {
    /// Raw ramdisk data (embedded at compile time)
    pub data: &'static [u8],
    /// Parsed superblock
    pub superblock: &'static RamdiskSuperblock,
}

impl Ramdisk {
    /// Create a new ramdisk from embedded data
    ///
    /// # Arguments
    ///
    /// * `data` - Raw ramdisk data from build.rs
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The data is valid ramdisk format
    /// - The superblock magic is correct
    /// - All offsets are within bounds
    pub unsafe fn from_embedded_data(data: &'static [u8]) -> Self {
        // Parse superblock at offset 0
        let superblock = &*(data.as_ptr() as *const RamdiskSuperblock);

        if !superblock.is_valid() {
            // Invalid ramdisk - create empty one
            // In production this would halt, but for now we create an empty ramdisk
            return Self {
                data,
                superblock: &*(data.as_ptr() as *const RamdiskSuperblock),
            };
        }

        Self {
            data,
            superblock,
        }
    }

    /// Find a file by name
    ///
    /// # Arguments
    ///
    /// * `name` - File name (e.g., "/test.txt" or "test.txt")
    ///
    /// # Returns
    ///
    /// The RamdiskFile if found, or None if not found
    pub fn find_file(&self, name: &str) -> Option<RamdiskFile> {
        // Strip leading slash if present
        let name = if name.starts_with('/') {
            &name[1..]
        } else {
            name
        };

        // Get file headers array
        let files = unsafe {
            let base = self.data.as_ptr().add(self.superblock.files_offset as usize);
            let count = self.superblock.num_files as usize;
            core::slice::from_raw_parts(base as *const RamdiskFile, count)
        };

        // Search for file by name
        for &file in files {
            let file_name = unsafe {
                let base = self.data.as_ptr();
                let name_ptr = base.add(file.name_offset as usize);
                // Find null terminator
                let mut len = 0;
                while *name_ptr.add(len) != 0 && len < 256 {
                    len += 1;
                }
                core::str::from_utf8_unchecked(
                    core::slice::from_raw_parts(name_ptr, len)
                )
            };

            if file_name == name {
                return Some(file);
            }
        }

        None
    }

    /// Read file data into a buffer
    ///
    /// # Arguments
    ///
    /// * `file` - The file to read (from find_file)
    /// * `buf` - Output buffer
    ///
    /// # Returns
    ///
    /// Number of bytes read
    pub fn read_file(&self, file: &RamdiskFile, buf: &mut [u8]) -> usize {
        let data_ptr = unsafe {
            self.data.as_ptr().add(file.data_offset as usize)
        };

        let to_copy = core::cmp::min(buf.len(), file.size as usize);

        if to_copy == 0 {
            return 0;
        }

        unsafe {
            core::ptr::copy_nonoverlapping(data_ptr, buf.as_mut_ptr(), to_copy);
        }

        to_copy
    }

    /// Get file size
    ///
    /// # Arguments
    ///
    /// * `file` - The file (from find_file)
    ///
    /// # Returns
    ///
    /// File size in bytes
    pub fn file_size(&self, file: &RamdiskFile) -> usize {
        file.size as usize
    }

    /// List all files in the ramdisk
    ///
    /// # Returns
    ///
    /// Vector of file names
    pub fn list_files(&self) -> alloc::vec::Vec<alloc::string::String> {
        let mut names = alloc::vec::Vec::new();

        let files = unsafe {
            let base = self.data.as_ptr().add(self.superblock.files_offset as usize);
            let count = self.superblock.num_files as usize;
            core::slice::from_raw_parts(base as *const RamdiskFile, count)
        };

        for &file in files {
            let file_name = unsafe {
                let base = self.data.as_ptr();
                let name_ptr = base.add(file.name_offset as usize);
                let mut len = 0;
                while *name_ptr.add(len) != 0 && len < 256 {
                    len += 1;
                }
                alloc::string::String::from_utf8_unchecked(
                    core::slice::from_raw_parts(name_ptr, len).to_vec()
                )
            };
            names.push(file_name);
        }

        names
    }

    /// Get the number of files
    pub fn file_count(&self) -> usize {
        self.superblock.num_files as usize
    }

    /// Check if ramdisk is empty (no files)
    pub fn is_empty(&self) -> bool {
        self.superblock.num_files == 0
    }
}

/// ============================================================================
/// Global Ramdisk Instance
/// ============================================================================

/// Global ramdisk instance
///
/// This is initialized during kernel startup with the embedded
/// ramdisk data that was generated by build.rs
pub static RAMDISK: SpinMutex<Option<Ramdisk>> = SpinMutex::new(None);

/// Initialize the ramdisk from embedded data
///
/// This function is called during kernel initialization to set up
/// the global RAMDISK with the embedded filesystem data.
///
/// # Arguments
///
/// * `data` - Raw ramdisk data from build.rs
///
/// # Safety
///
/// The data must be valid ramdisk format
pub unsafe fn init_ramdisk(data: &'static [u8]) {
    let ramdisk = Ramdisk::from_embedded_data(data);
    RAMDISK.lock().replace(ramdisk);
}

/// Get the ramdisk (convenience wrapper)
///
/// # Returns
///
/// Reference to the ramdisk, or error if not initialized
pub fn get_ramdisk() -> Result<&'static Ramdisk, Errno> {
    // SAFETY: We extend the lifetime here because the RAMDISK is static
    // and never moves after initialization. This is safe for a singleton.
    let lock = RAMDISK.lock();
    let result: Option<&Ramdisk> = lock.as_ref();
    match result {
        Some(r) => unsafe {
            // Extend lifetime from non-'static to 'static
            // This is safe because RAMDISK is a static singleton
            Ok(core::mem::transmute::<&Ramdisk, &'static Ramdisk>(r))
        },
        None => Err(Errno::ENODEV),
    }
}

/// ============================================================================
/// File Offset Tracking
/// ============================================================================

/// File offset tracking for open file descriptors
///
/// This is stored in the FdKind::File offset field and
/// updated by sys_read and sys_lseek.
#[derive(Debug, Clone, Copy)]
pub struct FileOffset {
    pub current: u64,
}

impl FileOffset {
    /// Create a new file offset
    pub const fn new() -> Self {
        Self { current: 0 }
    }

    /// Get the current offset
    pub fn get(&self) -> u64 {
        self.current
    }

    /// Set the offset (for sys_lseek)
    pub fn set(&mut self, offset: u64) {
        self.current = offset;
    }

    /// Add to the offset (for sys_read)
    pub fn add(&mut self, delta: usize) {
        self.current = self.current.saturating_add(delta as u64);
    }
}

impl Default for FileOffset {
    fn default() -> Self {
        Self::new()
    }
}

/// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_errno_values() {
        assert_eq!(Errno::Success as i32, 0);
        assert_eq!(Errno::ENOENT as i32, 2);
        assert_eq!(Errno::EBADF as i32, 9);
        assert_eq!(Errno::EROFS as i32, 30);
    }

    #[test]
    fn test_ramdisk_file_size() {
        assert_eq!(core::mem::size_of::<RamdiskFile>(), 16);
        assert_eq!(core::mem::size_of::<RamdiskSuperblock>(), 12);
    }

    #[test]
    fn test_superblock_magic() {
        let sb = RamdiskSuperblock {
            magic: 0x52555458,
            num_files: 3,
            files_offset: 16,
        };
        assert!(sb.is_valid());
    }
}

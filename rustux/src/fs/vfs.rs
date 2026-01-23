// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Virtual File System (VFS) Layer
//!
//! This module provides the VFS abstraction for file I/O operations.
//! It defines the FileOps trait that must be implemented by different
//! file types (ramdisk files, pipes, etc.).

use crate::fs::ramdisk::{Ramdisk, RamdiskFile, Errno};

/// ============================================================================
/// Seek Origin (Whence)
/// ============================================================================

/// Seek origin for sys_lseek
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whence {
    /// Seek from beginning of file
    Set = 0,
    /// Seek from current position
    Cur = 1,
    /// Seek from end of file
    End = 2,
}

/// ============================================================================
/// File Operations Trait
/// ============================================================================

/// File operations trait
///
/// This trait must be implemented by all file types that can be
/// accessed via file descriptors (ramdisk files, pipes, etc.)
pub trait FileOps {
    /// Read data from the file
    ///
    /// # Arguments
    ///
    /// * `buf` - Output buffer
    ///
    /// # Returns
    ///
    /// Number of bytes read, or error code
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Errno>;

    /// Write data to the file
    ///
    /// # Arguments
    ///
    /// * `buf` - Input data
    ///
    /// # Returns
    ///
    /// Number of bytes written, or error code
    fn write(&mut self, buf: &[u8]) -> Result<usize, Errno>;

    /// Seek to a position in the file
    ///
    /// # Arguments
    ///
    /// * `offset` - Offset in bytes
    /// * `whence` - Seek origin (Set, Cur, End)
    ///
    /// # Returns
    ///
    /// New file position, or error code
    fn seek(&mut self, offset: i64, whence: Whence) -> Result<u64, Errno>;
}

/// ============================================================================
/// Ramdisk File Operations
/// ============================================================================

/// Ramdisk file operations implementation
///
/// This implements FileOps for read-only ramdisk files.
/// The file maintains a current offset that is updated by reads and seeks.
pub struct RamdiskFileOps {
    /// The file (from ramdisk)
    pub file: RamdiskFile,
    /// Current read offset
    pub offset: u64,
    /// Size of the file (cached)
    pub size: u64,
}

impl RamdiskFileOps {
    /// Create a new ramdisk file operations object
    ///
    /// # Arguments
    ///
    /// * `file` - The ramdisk file entry
    pub fn new(file: RamdiskFile) -> Self {
        Self {
            file,
            offset: 0,
            size: file.size as u64,
        }
    }

    /// Get the current file offset
    pub fn get_offset(&self) -> u64 {
        self.offset
    }
}

impl FileOps for RamdiskFileOps {
    /// Read from the ramdisk file
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Errno> {
        let ramdisk = crate::fs::ramdisk::get_ramdisk()
            .map_err(|_| Errno::ENODEV)?;

        // Calculate remaining bytes from current offset
        let remaining = (self.size - self.offset) as usize;

        if remaining == 0 {
            return Ok(0); // EOF
        }

        let to_read = core::cmp::min(buf.len(), remaining);

        // Read from the file at current offset
        let data_offset = self.file.data_offset as usize + self.offset as usize;
        let data_ptr = unsafe {
            ramdisk.data.as_ptr().add(data_offset)
        };

        unsafe {
            core::ptr::copy_nonoverlapping(data_ptr, buf.as_mut_ptr(), to_read);
        }

        // Update offset
        self.offset += to_read as u64;

        Ok(to_read)
    }

    /// Write to the ramdisk file (always fails - read-only)
    fn write(&mut self, _buf: &[u8]) -> Result<usize, Errno> {
        Err(Errno::EROFS)
    }

    /// Seek within the ramdisk file
    fn seek(&mut self, offset: i64, whence: Whence) -> Result<u64, Errno> {
        let mut new_offset = match whence {
            Whence::Set => {
                if offset < 0 {
                    return Err(Errno::EINVAL);
                }
                offset as u64
            }
            Whence::Cur => {
                let current = self.offset as i64;
                let new = current + offset;
                if new < 0 {
                    return Err(Errno::EINVAL);
                }
                new as u64
            }
            Whence::End => {
                let end = self.size as i64;
                let new = end + offset;
                if new < 0 {
                    return Err(Errno::EINVAL);
                }
                new as u64
            }
        };

        // Clamp to file size
        if new_offset > self.size {
            new_offset = self.size;
        }

        self.offset = new_offset;
        Ok(new_offset)
    }
}

/// ============================================================================
/// VFS Integration
/// ============================================================================

/// Open a file from the ramdisk
///
/// This is a helper function used by sys_open to create
/// FileOps objects for ramdisk files.
///
/// # Arguments
///
/// * `path` - File name (with or without leading slash)
///
/// # Returns
///
/// RamdiskFileOps for the file, or error code
pub fn open_ramdisk_file(path: &str) -> Result<RamdiskFileOps, Errno> {
    let ramdisk = crate::fs::ramdisk::get_ramdisk()
        .map_err(|_| Errno::ENODEV)?;

    let file = ramdisk.find_file(path)
        .ok_or(Errno::ENOENT)?;

    Ok(RamdiskFileOps::new(file))
}

/// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whence_values() {
        assert_eq!(Whence::Set as i32, 0);
        assert_eq!(Whence::Cur as i32, 1);
        assert_eq!(Whence::End as i32, 2);
    }

    #[test]
    fn test_ramdisk_file_ops() {
        let file = RamdiskFile {
            name_offset: 0,
            data_offset: 32,
            size: 100,
            _pad: 0,
        };

        let mut ops = RamdiskFileOps::new(file);

        // Initial offset
        assert_eq!(ops.get_offset(), 0);

        // Seek to end
        let result = ops.seek(0, Whence::End);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 100);
    }
}

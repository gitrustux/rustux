// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! File Descriptor Abstraction
//!
//! This module provides the file descriptor abstraction for I/O syscalls.
//!
//! # Design
//!
//! - File descriptors (fd) are small integers (0-255)
//! - fd 0: stdin (keyboard input, future)
//! - fd 1: stdout (kernel debug console, port 0xE9)
//! - fd 2: stderr (same as stdout for now)
//! - fd 3+: files, pipes, etc. (Phase 5C)

/// File descriptor kinds
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FdKind {
    /// Standard input (fd 0) - Keyboard input (future)
    Stdin,

    /// Standard output (fd 1) - Kernel debug console (port 0xE9)
    Stdout,

    /// Standard error (fd 2) - Same as stdout for now
    Stderr,

    /// File descriptor (fd 3+) - For ramdisk files (Phase 5C)
    File {
        /// Inode number
        inode: u32,
        /// Current file offset
        offset: u64,
    },

    /// Pipe descriptor (future)
    Pipe {
        /// True if this is the read end
        read_end: bool,
        /// Pipe ID
        pipe_id: u32,
    },
}

/// File descriptor entry
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    /// Kind of file descriptor
    pub kind: FdKind,

    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
    pub flags: u32,
}

impl FileDescriptor {
    /// Create a new file descriptor
    pub const fn new(kind: FdKind, flags: u32) -> Self {
        Self { kind, flags }
    }

    /// Create a stdin file descriptor
    pub const fn stdin() -> Self {
        Self {
            kind: FdKind::Stdin,
            flags: 0, // O_RDONLY
        }
    }

    /// Create a stdout file descriptor
    pub const fn stdout() -> Self {
        Self {
            kind: FdKind::Stdout,
            flags: 1, // O_WRONLY
        }
    }

    /// Create a stderr file descriptor
    pub const fn stderr() -> Self {
        Self {
            kind: FdKind::Stderr,
            flags: 1, // O_WRONLY
        }
    }
}

/// Per-process file descriptor table
///
/// Manages file descriptors for a single process.
/// FD 0, 1, 2 are pre-allocated as stdin, stdout, stderr.
pub struct FileDescriptorTable {
    /// File descriptors (indexed by fd number)
    fds: [Option<FileDescriptor>; 256],

    /// Next fd to allocate (starts at 3, after stdin/stdout/stderr)
    next_fd: u8,
}

impl FileDescriptorTable {
    /// Create a new file descriptor table with stdin/stdout/stderr pre-allocated
    pub const fn new() -> Self {
        const NONE: Option<FileDescriptor> = None;

        // Initialize array with None values
        let mut fds = [NONE; 256];

        // Pre-allocate stdin, stdout, stderr (will be set up in init below)
        // For now, we use const fn, so we can't actually set them here

        Self {
            fds,
            next_fd: 3,
        }
    }

    /// Initialize the standard file descriptors (0, 1, 2)
    ///
    /// This must be called after creating the table to set up stdin/stdout/stderr.
    /// Note: This can't be done in `new()` because it requires mutable access.
    pub fn init(&mut self) {
        self.fds[0] = Some(FileDescriptor::stdin());
        self.fds[1] = Some(FileDescriptor::stdout());
        self.fds[2] = Some(FileDescriptor::stderr());
    }

    /// Allocate a new file descriptor
    ///
    /// Returns the fd number, or None if the table is full.
    pub fn alloc(&mut self, kind: FdKind, flags: u32) -> Option<u8> {
        // Check if we've wrapped around (table is full)
        if self.next_fd == 0 {
            return None;
        }

        let fd = self.next_fd;
        self.fds[fd as usize] = Some(FileDescriptor::new(kind, flags));

        // Increment and check for wrap
        self.next_fd = self.next_fd.wrapping_add(1);
        if self.next_fd == 0 {
            // Table is full after this allocation
        }

        Some(fd)
    }

    /// Get a file descriptor by number
    ///
    /// Returns None if the fd is not allocated.
    pub fn get(&self, fd: u8) -> Option<&FileDescriptor> {
        self.fds.get(fd as usize)?.as_ref()
    }

    /// Get a mutable file descriptor by number
    ///
    /// Returns None if the fd is not allocated.
    pub fn get_mut(&mut self, fd: u8) -> Option<&mut FileDescriptor> {
        self.fds.get_mut(fd as usize)?.as_mut()
    }

    /// Close a file descriptor
    ///
    /// Returns the closed descriptor, or None if the fd was not allocated.
    pub fn close(&mut self, fd: u8) -> Option<FileDescriptor> {
        // u8 can't be >= 256, so we skip that check

        // Don't allow closing stdin/stdout/stderr
        if fd < 3 {
            return None;
        }

        self.fds.get_mut(fd as usize)?.take()
    }

    /// Get the number of active file descriptors
    pub fn count(&self) -> usize {
        self.fds.iter().filter(|f| f.is_some()).count()
    }
}

// ============================================================================
// Open Flags (for future use with sys_open)
// ============================================================================

/// Open flags (for sys_open)
pub mod flags {
    /// Read-only
    pub const O_RDONLY: u32 = 0;

    /// Write-only
    pub const O_WRONLY: u32 = 1;

    /// Read-write
    pub const O_RDWR: u32 = 2;

    /// Create file if it doesn't exist
    pub const O_CREAT: u32 = 1 << 3;

    /// Exclusive create
    pub const O_EXCL: u32 = 1 << 4;

    /// Truncate file
    pub const O_TRUNC: u32 = 1 << 5;

    /// Append mode
    pub const O_APPEND: u32 = 1 << 6;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fd_table_new() {
        let mut table = FileDescriptorTable::new();
        table.init();

        // Check that stdin, stdout, stderr are allocated
        assert!(table.get(0).is_some());
        assert!(table.get(1).is_some());
        assert!(table.get(2).is_some());

        // Check that they have the right kind
        assert!(matches!(table.get(0).unwrap().kind, FdKind::Stdin));
        assert!(matches!(table.get(1).unwrap().kind, FdKind::Stdout));
        assert!(matches!(table.get(2).unwrap().kind, FdKind::Stderr));
    }

    #[test]
    fn test_fd_alloc() {
        let mut table = FileDescriptorTable::new();
        table.init();

        // Allocate a new fd
        let fd = table.alloc(FdKind::File { inode: 1, offset: 0 }, flags::O_RDONLY);
        assert!(fd.is_some());
        assert_eq!(fd.unwrap(), 3);

        // Check that it exists
        assert!(table.get(3).is_some());
        assert_eq!(table.count(), 4); // stdin, stdout, stderr, plus the new one
    }

    #[test]
    fn test_fd_close() {
        let mut table = FileDescriptorTable::new();
        table.init();

        // Allocate a new fd
        let fd = table.alloc(FdKind::File { inode: 1, offset: 0 }, flags::O_RDONLY).unwrap();

        // Close it
        let closed = table.close(fd);
        assert!(closed.is_some());
        assert!(table.get(fd).is_none());

        // Can't close stdin
        assert!(table.close(0).is_none());
        assert!(table.get(0).is_some());
    }

    #[test]
    fn test_fd_kind() {
        let stdin = FileDescriptor::stdin();
        assert!(matches!(stdin.kind, FdKind::Stdin));

        let stdout = FileDescriptor::stdout();
        assert!(matches!(stdout.kind, FdKind::Stdout));

        let stderr = FileDescriptor::stderr();
        assert!(matches!(stderr.kind, FdKind::Stderr));
    }
}

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Execution and ELF Loading
//!
//! This module provides functionality for loading and executing
//! ELF binaries in userspace.

pub mod elf;
pub mod process_loader;
pub mod userspace_exec_test;

// Re-export ELF types
pub use elf::{
    ElfIdent,
    ElfHeader,
    ProgramHeader,
    LoadedSegment,
    LoadedElf,
    parse_elf_header,
    parse_program_headers,
    validate_elf_header,
    load_elf,
    is_elf_file,
};

// Re-export process loader types
pub use process_loader::{ProcessImage, load_elf_process};

// Re-export userspace test
pub use userspace_exec_test::test_userspace_execution;

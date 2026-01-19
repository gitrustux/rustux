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
pub mod userspace_test;

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

// Re-export userspace test types
pub use userspace_test::{
    execute_userspace_test,
    test_mexec_minimal,
};

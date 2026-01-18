// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 page table constants

#![no_std]

pub const PAGE_SIZE: usize = 1 << 12; // 4096 bytes

/// Number of entries per page table
pub const ENTRIES_PER_PAGE_TABLE: usize = 512;

pub const PAGE_SIZE_SHIFT: usize = 12;

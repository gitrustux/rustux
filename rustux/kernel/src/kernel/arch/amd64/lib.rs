// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! AMD64/x86_64 architecture-specific kernel code

#![no_std]

// Export interrupt controller
pub mod interrupt;

// Export APIC module
pub mod apic;

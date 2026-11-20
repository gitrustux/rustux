// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! What the rest of the kernel asks of the processor (docs/PORTING.md):
//! interrupts on and off, waiting, the fault address, the page tables in
//! use, the floating-point registers, the user thread pointer, the debug
//! console. Code outside src/arch/ uses these, not assembly of its own
//! (build.rs checks it).

#[cfg(target_arch = "x86_64")]
pub use super::amd64::cpu::*;

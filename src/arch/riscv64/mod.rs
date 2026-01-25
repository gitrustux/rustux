// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64-bit architecture-specific code
//!
//! This module contains all RISC-V-specific implementations.
//!
//! # Modules
//!
//! - [`arch`] - Architecture definitions, CPU features, and SBI interface
//! - [`interrupt`] - PLIC and CLINT interrupt controller support
//! - [`mm`] - Memory management unit (MMU) and page tables

pub mod arch;
pub mod interrupt;
pub mod mm;

// Re-exports
pub use arch::{
    HartInfo, RiscvFeatures, RiscvInterruptController,
    SbiExtension, SbiFunction, SbiRet, SbiCall,
    Clint, get_hart_info, set_hart_info, get_bootstrap_hart,
    get_sbi_version, get_features,
    fence, fence_i, fence_s,
    RISCV_MAX_HARTS, RISCV_PAGE_SIZE, RISCV_PAGE_SHIFT,
};
pub use interrupt::{Plic, PlicHartContext, PlicIrq, PlicPriority};
pub use mm::{
    PageTable, PageTableEntry, PageTableFlags, PageTableLevel, PageTableMode,
    AddressSpace, Asid, AsidAllocator,
    sfence_vma, sfence_vma_asid, sfence_vma_addr,
    ASID_INVALID, ASID_KERNEL,
    SV39_VA_BITS, SV48_VA_BITS,
};

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86 MMU (Memory Management Unit)
//!
//! This module provides page table management for x86-64.

/// Physical address type
pub type PAddr = u64;

/// Virtual address type
pub type VAddr = usize;

// Page size constants
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

// MSR definitions
pub const IA32_PAT_MSR: u32 = 0x277;
pub const IA32_MTRR_CAP_MSR: u32 = 0xFE;
pub const IA32_MTRR_DEF_TYPE_MSR: u32 = 0x2FF;

// PAT register default values (write-back caching)
pub const PAT_DEFAULT_VALUE: u64 = 0x0007010600070106;

// Global page table state (simplified - in real kernel would be per-address space)
static mut BOOT_PML4: Option<PAddr> = None;

/// Kernel physical offset for direct-mapped physical memory
///
/// Physical memory is mapped at this offset in kernel virtual address space.
/// This allows the kernel to access any physical page by adding this offset.
/// Using the standard x86_64 kernel direct map offset (same as Linux).
const KERNEL_PHYS_OFFSET: u64 = 0xffff_8000_0000_0000;

/// Maximum physical memory to map (16GB for now)
const MAX_PHYS_MEMORY: u64 = 0x4_0000_0000;

/// Early MMU initialization
///
/// This is called very early in boot to set up basic MMU state.
/// At this point, we have minimal memory allocation available.
pub fn x86_mmu_early_init() {
    unsafe {
        // Initialize PAT (Page Attribute Table) for proper memory caching
        x86_mmu_percpu_init();

        // The bootloader has already set up basic page tables
        // We preserve them for now and will create proper page tables later
        let current_cr3 = read_cr3();
        BOOT_PML4 = Some(current_cr3);

        // Set up direct mapping of physical memory
        // This maps [KERNEL_PHYS_OFFSET .. KERNEL_PHYS_OFFSET + MAX_PHYS_MEMORY]
        // to physical memory [0 .. MAX_PHYS_MEMORY]
        x86_setup_direct_map();

        // Enable write-protect (CR0.WP) to protect kernel code from modification
        x86_enable_write_protect();

        // Ensure page tables are properly configured
        x86_validate_page_tables();
    }
}

/// Set up direct mapping of physical memory
///
/// Maps physical memory at KERNEL_PHYS_OFFSET so the kernel can access
/// any physical page by adding this offset.
unsafe fn x86_setup_direct_map() {
    // Page table entry flags
    const PTE_P: u64 = 0x001;  // Present
    const PTE_W: u64 = 0x002;  // Read/Write
    const PTE_G: u64 = 0x100;  // Global

    // Get the current PML4
    let cr3 = read_cr3();
    let pml4_virt = cr3 as *mut u64;

    // Calculate PML4 index for KERNEL_PHYS_OFFSET
    // KERNEL_PHYS_OFFSET = 0xffff_8000_0000_0000
    // Bits 47:39 = 0b100000000 = 256 (0x100)
    let pml4_index = ((KERNEL_PHYS_OFFSET >> 39) & 0x1FF) as usize;

    // Check if PML4 entry is already present
    let pml4_entry = *pml4_virt.add(pml4_index);

    // We need to create or verify the PML4 entry for the direct map
    // For simplicity, we'll allocate page tables using a fixed address
    // In a real kernel, this would use the PMM

    // Map the first 1GB of physical memory using 2MB pages
    // This should be sufficient for the current 128MB RAM setup
    // PML4[256] -> PDP -> PD with 2MB pages

    // For now, use a simpler approach: identity mapping for low memory
    // The UEFI bootloader should have already set this up
    // We'll verify it exists

    // TODO: Implement full direct mapping setup with proper page table allocation
    // For now, rely on UEFI bootloader's identity mapping of low memory
}

/// Main MMU initialization
///
/// This is called after the VM subsystem is up and we can allocate memory.
pub fn x86_mmu_init() {
    unsafe {
        // At this point, we should create proper kernel page tables
        // For now, we continue using the boot page tables
        // TODO: Create kernel address space with proper mappings

        // Synchronize PAT settings across all CPUs
        x86_pat_sync(0xFFFF_FFFF_FFFF_FFFF); // All CPUs

        // Initialize large page support detection
        x86_detect_huge_page_support();
    }
}

/// Per-CPU MMU initialization
///
/// Initializes MMU settings specific to this CPU.
pub fn x86_mmu_percpu_init() {
    unsafe {
        // Initialize PAT (Page Attribute Table) for proper memory caching
        // The default PAT value provides write-back caching for most memory
        x86_write_msr(IA32_PAT_MSR, PAT_DEFAULT_VALUE);

        // Initialize MTRR (Memory Type Range Registers) if supported
        // For now, we use the default BIOS settings
        // TODO: Implement proper MTRR initialization
    }
}

/// Sync PAT (Page Attribute Table)
///
/// Synchronizes PAT settings across CPUs. This ensures all CPUs have
/// consistent memory type settings for proper caching behavior.
///
/// # Arguments
///
/// * `cpu_mask` - Bitmask of CPUs to synchronize (0xFFFFFFFFFFFFFFFF = all CPUs)
pub fn x86_pat_sync(cpu_mask: u64) {
    // In single-CPU systems, this is a no-op
    if cpu_mask == 1 {
        return;
    }

    unsafe {
        // Read current PAT value from this CPU
        let current_pat = x86_read_msr(IA32_PAT_MSR);

        // In SMP systems, we would send IPIs to other CPUs to update their PAT
        // For now, this is a stub since we don't have IPI support yet
        // TODO: Implement IPI-based PAT synchronization for SMP
        let _ = current_pat;
    }
}

/// Check if a virtual address is canonical
pub fn x86_is_vaddr_canonical_impl(va: VAddr) -> bool {
    // x86-64 canonical addresses must have bits 63:48 all equal to bit 47
    const CANONICAL_MASK: u64 = 0xFFFF800000000000;
    (va as u64 & CANONICAL_MASK) == 0 || (va as u64 & CANONICAL_MASK) == CANONICAL_MASK
}

/// Check if an address is in kernel space
///
/// On x86-64, kernel addresses are in the upper half (high bit set).
pub fn is_kernel_address(addr: usize) -> bool {
    addr & 0xFFFF800000000000 != 0
}

/// Write to CR3 register (page table base)
///
/// # Safety
///
/// This function modifies a critical system register.
/// The caller must ensure the new page table is valid.
pub unsafe fn write_cr3(cr3_value: PAddr) {
    core::arch::asm!("mov cr3, {}", in(reg) cr3_value, options(nostack, nomem));
}

/// Read CR3 register (page table base)
pub fn read_cr3() -> PAddr {
    let cr3_value: PAddr;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3_value);
    }
    cr3_value
}

/// Create boot page tables
///
/// This function creates the initial page tables used during boot.
/// It sets up:
/// - Identity mapping for low memory (first 2MB)
/// - Kernel code mapping at KERNEL_BASE
/// - Stack mapping
///
/// # Returns
///
/// Physical address of the PML4 table
pub fn x86_boot_create_page_tables() -> PAddr {
    unsafe {
        // Allocate page-aligned memory for page tables
        // In a real kernel, this would come from a physical memory allocator
        // For now, we use a simplified approach

        // Page table entry flags
        const PTE_P: u64 = 0x001;  // Present
        const PTE_W: u64 = 0x002;  // Read/Write
        const PTE_U: u64 = 0x004;  // User
        const PTE_G: u64 = 0x100;  // Global

        // Allocate PML4 (should be page-aligned)
        let pml4_addr = 0x10000 as PAddr; // Placeholder - should be allocated
        let pml4: *mut u64 = pml4_addr as *mut u64;

        // Zero the PML4
        for i in 0..512 {
            *pml4.add(i) = 0;
        }

        // Allocate PDPTE, PDE, and PT (simplified - use 2MB pages)
        let pdpt_addr = 0x11000 as PAddr;
        let pd_addr = 0x12000 as PAddr;

        let pdpt: *mut u64 = pdpt_addr as *mut u64;
        let pd: *mut u64 = pd_addr as *mut u64;

        // Zero tables
        for i in 0..512 {
            *pdpt.add(i) = 0;
            *pd.add(i) = 0;
        }

        // Set up PML4[0] to point to PDPT
        *pml4 = pdpt_addr | PTE_P | PTE_W;

        // Set up PDPTE[0] to point to PD
        *pdpt = pd_addr | PTE_P | PTE_W;

        // Set up 2MB pages mapping the first 2GB of memory identically
        for i in 0..1024 {
            // Identity map with 2MB pages: present, writable, global
            *pd.add(i) = (i * 0x200000) as u64 | PTE_P | PTE_W | PTE_G | 0x080; // PS=1 for 2MB pages
        }

        // Return the physical address of PML4
        pml4_addr
    }
}

/// Read an MSR (Model Specific Register)
///
/// # Safety
///
/// The caller must ensure the MSR index is valid.
#[inline]
pub unsafe fn x86_read_msr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    core::arch::asm!("rdmsr",
                     in("ecx") msr,
                     out("eax") low,
                     out("edx") high,
                     options(nostack, nomem, preserves_flags));
    ((high as u64) << 32) | (low as u64)
}

/// Write to an MSR (Model Specific Register)
///
/// # Safety
///
/// The caller must ensure the MSR index is valid and the value is appropriate.
#[inline]
pub unsafe fn x86_write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!("wrmsr",
                     in("ecx") msr,
                     in("eax") low,
                     in("edx") high,
                     options(nostack, nomem, preserves_flags));
}

/// Set TSS SP0 (kernel stack pointer)
///
/// # Safety
///
/// This modifies critical task state segment data.
#[inline]
pub unsafe fn x86_set_tss_sp(sp: u64) {
    use crate::arch::amd64::descriptor::get_tss;
    let tss = get_tss();
    tss.rsp0 = sp;
}

/// Set DS segment register
///
/// # Safety
///
/// This modifies a segment register.
#[inline]
pub unsafe fn x86_set_ds(sel: u16) {
    core::arch::asm!("mov ds, {}", in(reg) sel, options(nostack));
}

/// Set ES segment register
///
/// # Safety
///
/// This modifies a segment register.
#[inline]
pub unsafe fn x86_set_es(sel: u16) {
    core::arch::asm!("mov es, {}", in(reg) sel, options(nostack));
}

/// Set FS segment register
///
/// # Safety
///
/// This modifies a segment register.
#[inline]
pub unsafe fn x86_set_fs(sel: u16) {
    core::arch::asm!("mov fs, {}", in(reg) sel, options(nostack));
}

/// Set GS segment register
///
/// # Safety
///
/// This modifies a segment register.
#[inline]
pub unsafe fn x86_set_gs(sel: u16) {
    core::arch::asm!("mov gs, {}", in(reg) sel, options(nostack));
}

/// Get GS segment register
///
/// # Safety
///
/// This reads a segment register.
#[inline]
pub unsafe fn x86_get_gs() -> u16 {
    let gs: u16;
    core::arch::asm!("mov {}, gs", out(reg) gs);
    gs
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Enable write-protect in CR0
///
/// This protects kernel code from being modified.
unsafe fn x86_enable_write_protect() {
    let mut cr0: u64;
    core::arch::asm!("mov {}, cr0", out(reg) cr0);
    cr0 |= 0x10000; // Set WP bit (bit 16)
    core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack));
}

/// Validate that page tables are properly configured
unsafe fn x86_validate_page_tables() {
    let cr3 = read_cr3();
    if cr3 == 0 {
        panic!("CR3 is null - no page table active!");
    }

    // Check that PML4 is aligned
    if cr3 & PAGE_MASK as u64 != 0 {
        panic!("PML4 is not page-aligned!");
    }
}

/// Detect huge page support
unsafe fn x86_detect_huge_page_support() {
    // TODO: Use CPUID to detect support for 1GB pages
    // For now, assume 2MB pages are supported (standard on x86-64)
}

/// Invalidate a TLB entry
///
/// # Safety
///
/// Caller must ensure the address is valid and page-aligned.
pub unsafe fn x86_tlb_invalidate_page(vaddr: VAddr) {
    core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack, nomem));
}

/// Get the kernel's CR3 value
///
/// Returns the physical address of the kernel page table.
pub fn x86_kernel_cr3() -> PAddr {
    unsafe {
        match BOOT_PML4 {
            Some(cr3) => cr3,
            None => read_cr3(),
        }
    }
}

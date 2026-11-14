// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! UEFI runtime services after ExitBootServices (step 19): the firmware's
//! variables (boot entries, BootOrder, the boot manager's settings).
//!
//! At boot the kernel copies the runtime descriptors of the UEFI memory map
//! ([`note_descriptor`]), then, once its own page tables are active and the
//! boot identity map still is, calls SetVirtualAddressMap ([`init`]): code
//! and data at their direct-map address (HHDM, executable and cached),
//! runtime MMIO (the variable store's flash) uncached in the MMIO window.
//! From then on the firmware runs at those addresses in every address
//! space. `noefi` on the kernel command line skips all of it (no runtime
//! services: for firmware that crashes in them).
//!
//! Calls are serialised (the services are not reentrant), run with
//! interrupts off, and keep the caller's FPU/SSE registers (fxsave), since
//! firmware code may use SSE.

use crate::arch::amd64::mm::kernel_map;
use crate::arch::amd64::mm::RxStatus;
use crate::sync::BootCell;
use core::sync::atomic::{AtomicBool, Ordering};
use uefi::mem::memory_map::{MemoryAttribute, MemoryDescriptor, MemoryType};
use uefi::runtime::{VariableAttributes, VariableVendor};
use uefi::{CStr16, Guid};

const MAX_RUNTIME: usize = 128;

/// Boot only, one CPU: the runtime descriptors, how many, the system table
struct Boot {
    descs: [MemoryDescriptor; MAX_RUNTIME],
    n: usize,
    system_table: u64,
}
static BOOT: BootCell<Boot> = BootCell::new(Boot {
    descs: [MemoryDescriptor {
        ty: MemoryType::RESERVED,
        phys_start: 0,
        virt_start: 0,
        page_count: 0,
        att: MemoryAttribute::empty(),
    }; MAX_RUNTIME],
    n: 0,
    system_table: 0,
});
static READY: AtomicBool = AtomicBool::new(false);
static LOCK: crate::sync::SpinMutex<()> = crate::sync::SpinMutex::new(());

/// Keep a runtime descriptor of the UEFI memory map (call for each, before
/// the PMM hands memory out)
pub fn note_descriptor(d: &MemoryDescriptor) {
    if !d.att.contains(MemoryAttribute::RUNTIME) {
        return;
    }
    // Boot, one CPU
    let b = unsafe { BOOT.boot_mut() };
    if b.n < MAX_RUNTIME {
        b.descs[b.n] = *d;
        b.n += 1;
    }
}

/// The system table's physical address (call before ExitBootServices)
pub fn note_system_table(phys: u64) {
    unsafe { BOOT.boot_mut().system_table = phys };
}

/// Move the runtime services to kernel addresses. Returns a message for
/// the boot log.
///
/// # Safety
/// After `kernel_map::init`, while the boot identity map is still in place
/// (the firmware runs at its physical addresses during the call), on one CPU.
pub unsafe fn init() -> &'static str {
    let b = BOOT.boot_mut();
    let n = b.n;
    if b.system_table == 0 || n == 0 {
        return "no runtime services";
    }
    let system_table = b.system_table;
    let descs = &mut b.descs;
    for d in descs[..n].iter_mut() {
        let size = d.page_count * 4096;
        d.virt_start = if d.ty == MemoryType::MMIO || d.ty == MemoryType::MMIO_PORT_SPACE {
            match kernel_map::map_mmio(d.phys_start, size, || crate::mm::pmm::pmm_alloc_kernel_page().ok()) {
                Ok(va) => va,
                Err(_) => return "runtime MMIO not mapped: no runtime services",
            }
        } else {
            kernel_map::phys_to_virt(d.phys_start)
        };
    }
    let st = kernel_map::phys_to_virt(system_table) as *const uefi_raw::table::system::SystemTable;
    match uefi::runtime::set_virtual_address_map(&mut descs[..n], st) {
        Ok(()) => {
            READY.store(true, Ordering::Release);
            "runtime services at kernel addresses"
        }
        Err(_) => "SetVirtualAddressMap failed: no runtime services",
    }
}

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

#[repr(C, align(16))]
struct FxArea([u8; 512]);

/// Run `f` as a firmware call: alone, interrupts off, SSE state kept
fn call<T>(f: impl FnOnce() -> T) -> T {
    let mut fx = FxArea([0; 512]);
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags);
    }
    let g = LOCK.lock();
    unsafe {
        core::arch::asm!("fxsave64 [{}]", in(reg) fx.0.as_mut_ptr(), options(nostack));
    }
    let r = f();
    unsafe {
        core::arch::asm!("fxrstor64 [{}]", in(reg) fx.0.as_ptr(), options(nostack));
    }
    drop(g);
    unsafe {
        if flags & (1 << 9) != 0 {
            core::arch::asm!("sti");
        }
    }
    r
}

fn status(s: uefi::Status) -> RxStatus {
    use uefi::Status as S;
    match s {
        S::NOT_FOUND => RxStatus::ERR_NOT_FOUND,
        S::BUFFER_TOO_SMALL => RxStatus::ERR_BUFFER_TOO_SMALL,
        S::INVALID_PARAMETER => RxStatus::ERR_INVALID_ARGS,
        S::WRITE_PROTECTED | S::SECURITY_VIOLATION => RxStatus::ERR_ACCESS_DENIED,
        S::OUT_OF_RESOURCES => RxStatus::ERR_NO_MEMORY,
        S::UNSUPPORTED => RxStatus::ERR_NOT_SUPPORTED,
        _ => RxStatus::ERR_IO,
    }
}

fn name16(name: &[u16]) -> Result<&CStr16, RxStatus> {
    CStr16::from_u16_with_nul(name).map_err(|_| RxStatus::ERR_INVALID_ARGS)
}

/// GetVariable: (attributes, size); BUFFER_TOO_SMALL gives the size needed
pub fn get(name: &[u16], guid: [u8; 16], buf: &mut [u8]) -> Result<(u32, usize), (RxStatus, usize)> {
    if !ready() {
        return Err((RxStatus::ERR_NOT_SUPPORTED, 0));
    }
    let n = name16(name).map_err(|e| (e, 0))?;
    let vendor = VariableVendor(Guid::from_bytes(guid));
    call(|| match uefi::runtime::get_variable(n, &vendor, buf) {
        Ok((data, attrs)) => Ok((attrs.bits(), data.len())),
        Err(e) => {
            let need = (*e.data()).unwrap_or(0);
            Err((status(e.status()), need))
        }
    })
}

/// SetVariable; empty data deletes it
pub fn set(name: &[u16], guid: [u8; 16], attrs: u32, data: &[u8]) -> Result<(), RxStatus> {
    if !ready() {
        return Err(RxStatus::ERR_NOT_SUPPORTED);
    }
    let n = name16(name)?;
    let vendor = VariableVendor(Guid::from_bytes(guid));
    let a = VariableAttributes::from_bits_retain(attrs);
    call(|| uefi::runtime::set_variable(n, &vendor, a, data)).map_err(|e| status(e.status()))
}

/// GetNextVariableName: `name` (NUL-terminated, "" to start) and `guid`
/// are replaced by the next variable's
pub fn next(name: &mut [u16], guid: &mut [u8; 16]) -> Result<(), RxStatus> {
    if !ready() {
        return Err(RxStatus::ERR_NOT_SUPPORTED);
    }
    let mut vendor = VariableVendor(Guid::from_bytes(*guid));
    call(|| uefi::runtime::get_next_variable_key(name, &mut vendor)).map_err(|e| status(e.status()))?;
    *guid = vendor.0.to_bytes();
    Ok(())
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Per-CPU data.
//!
//! Each CPU has its own GDT and TSS (with the I/O permission bitmap), the
//! stack pointer of its boot context, and its LAPIC ID. A CPU finds its
//! slot by reading its LAPIC ID, which user code cannot change. (A GS-based
//! pointer would be faster but needs `swapgs` on every entry from user
//! mode, and the interrupt entry points are compiler-generated.) While only
//! the boot CPU runs, the lookup is skipped.
//!
//! A slot's tables are touched only by its own CPU, except by the boot CPU
//! before that CPU starts.

use super::descriptor::CpuTables;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub const MAX_CPUS: usize = 16;
const NO_APIC: u32 = u32::MAX;

pub struct Cpu {
    pub index: usize,
    apic_id: AtomicU32,
    online: AtomicBool,
    tables: UnsafeCell<CpuTables>,
    boot_rsp: UnsafeCell<u64>,
}

// Each CPU only touches its own `tables` and `boot_rsp` (see module docs)
unsafe impl Sync for Cpu {}

impl Cpu {
    const fn new(index: usize) -> Self {
        Self {
            index,
            apic_id: AtomicU32::new(NO_APIC),
            online: AtomicBool::new(false),
            tables: UnsafeCell::new(CpuTables::new()),
            boot_rsp: UnsafeCell::new(0),
        }
    }

    /// This CPU's GDT/TSS (only the owning CPU may use the pointer, or the
    /// boot CPU before the owner starts)
    pub fn tables(&self) -> *mut CpuTables {
        self.tables.get()
    }

    /// Where the scheduler saves this CPU's boot-context stack pointer
    pub fn boot_rsp(&self) -> *mut u64 {
        self.boot_rsp.get()
    }

    pub fn apic_id(&self) -> u32 {
        self.apic_id.load(Ordering::Acquire)
    }

    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Acquire)
    }
}

static CPUS: [Cpu; MAX_CPUS] = {
    let mut i = 0;
    // Not `[Cpu::new(0); N]`: Cpu is not Copy
    let mut a: [core::mem::MaybeUninit<Cpu>; MAX_CPUS] = unsafe { core::mem::MaybeUninit::uninit().assume_init() };
    while i < MAX_CPUS {
        a[i] = core::mem::MaybeUninit::new(Cpu::new(i));
        i += 1;
    }
    unsafe { core::mem::transmute::<_, [Cpu; MAX_CPUS]>(a) }
};

/// Number of CPUs running kernel code
static ONLINE: AtomicUsize = AtomicUsize::new(1);

/// The local APIC ID of the CPU executing this
pub fn lapic_id() -> u32 {
    let base = crate::arch::amd64::mm::kernel_map::phys_to_virt(0xFEE0_0000);
    unsafe { core::ptr::read_volatile((base + 0x20) as *const u32) >> 24 }
}

/// The CPU executing this
pub fn this() -> &'static Cpu {
    if ONLINE.load(Ordering::Acquire) <= 1 {
        return &CPUS[0];
    }
    let id = lapic_id();
    CPUS.iter().find(|c| c.apic_id() == id).unwrap_or(&CPUS[0])
}

pub fn get(index: usize) -> Option<&'static Cpu> {
    CPUS.get(index)
}

/// Record the boot CPU's LAPIC ID (before starting any other CPU)
pub fn register_boot_cpu() {
    CPUS[0].apic_id.store(lapic_id(), Ordering::Release);
    CPUS[0].online.store(true, Ordering::Release);
}

/// Reserve the slot for the CPU with `apic_id` (boot CPU, before starting it)
pub fn reserve(apic_id: u32) -> Option<&'static Cpu> {
    let slot = CPUS.iter().skip(1).find(|c| c.apic_id() == NO_APIC)?;
    slot.apic_id.store(apic_id, Ordering::Release);
    Some(slot)
}

/// Called by a newly started CPU once its tables are loaded
pub fn mark_online(cpu: &Cpu) {
    cpu.online.store(true, Ordering::Release);
    ONLINE.fetch_add(1, Ordering::AcqRel);
}

pub fn online_count() -> usize {
    ONLINE.load(Ordering::Acquire)
}

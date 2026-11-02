// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Locks and cells for kernel globals (replacing `static mut`).
//!
//! How global state is classified:
//!
//! - **Locked, used by interrupt handlers too** → [`IrqMutex`]: a spin lock
//!   that keeps interrupts disabled on this CPU while held, so a handler
//!   can never interrupt the holder and deadlock on the same lock.
//! - **Locked, process context only** → [`SpinMutex`](super::SpinMutex).
//!   (Kernel code runs with interrupts disabled, so this is also safe in
//!   practice; `IrqMutex` is used wherever a handler might touch the data.)
//! - **Written once during boot, read afterwards** → [`BootCell`].
//! - **Counters and flags** → atomics.
//! - **One per CPU** → `arch::amd64::percpu`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Disable interrupts; returns whether they were enabled
#[inline]
pub fn irq_save() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let flags: u64;
        unsafe { core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem)) };
        flags & (1 << 9) != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Re-enable interrupts if `enabled` (the value from [`irq_save`])
#[inline]
pub fn irq_restore(enabled: bool) {
    #[cfg(target_arch = "x86_64")]
    if enabled {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = enabled;
}

/// A spin lock that disables interrupts while held
pub struct IrqMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for IrqMutex<T> {}
unsafe impl<T: Send> Sync for IrqMutex<T> {}

impl<T> IrqMutex<T> {
    pub const fn new(data: T) -> Self {
        Self { locked: AtomicBool::new(false), data: UnsafeCell::new(data) }
    }

    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let irq = irq_save();
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        IrqMutexGuard { mutex: self, irq }
    }

    /// Like `lock`, but gives up instead of spinning (for the panic and
    /// fault paths, which must not deadlock on a lock their own CPU holds)
    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let irq = irq_save();
        if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(IrqMutexGuard { mutex: self, irq })
        } else {
            irq_restore(irq);
            None
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

pub struct IrqMutexGuard<'a, T> {
    mutex: &'a IrqMutex<T>,
    irq: bool,
}

impl<T> Drop for IrqMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
        irq_restore(self.irq);
    }
}

impl<T> Deref for IrqMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

/// Data set up during boot, while one CPU runs and before anything reads
/// it concurrently; read-only afterwards.
pub struct BootCell<T> {
    data: UnsafeCell<T>,
}

unsafe impl<T: Sync> Sync for BootCell<T> {}

impl<T> BootCell<T> {
    pub const fn new(data: T) -> Self {
        Self { data: UnsafeCell::new(data) }
    }

    pub fn get(&self) -> &T {
        unsafe { &*self.data.get() }
    }

    /// Mutable access during boot.
    ///
    /// # Safety
    /// Only while a single CPU runs and nothing else holds a reference
    /// (boot, before other CPUs start and before user programs run).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn boot_mut(&self) -> &mut T {
        &mut *self.data.get()
    }

    pub fn as_ptr(&self) -> *mut T {
        self.data.get()
    }
}

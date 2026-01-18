// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Spinlock Implementation
//!
//! This module provides a simple spinlock for kernel use.
//! Spinlocks are used when the expected wait time is very short.

use core::sync::atomic::{AtomicBool, Ordering};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

/// A simple spinlock
pub struct SpinMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinMutex<T> {}
unsafe impl<T: Send> Sync for SpinMutex<T> {}

impl<T> SpinMutex<T> {
    /// Create a new spinlock
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock, spinning until it becomes available
    pub fn lock(&self) -> SpinMutexGuard<'_, T> {
        while self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            // Spin with pause to reduce bus contention
            core::hint::spin_loop();
        }
        SpinMutexGuard { mutex: self }
    }

    /// Try to acquire the lock without spinning
    pub fn try_lock(&self) -> Option<SpinMutexGuard<'_, T>> {
        if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(SpinMutexGuard { mutex: self })
        } else {
            None
        }
    }

    /// Get a raw pointer to the inner data
    ///
    /// # Safety
    ///
    /// This function is unsafe because it returns a raw pointer without
    /// any synchronization guarantees. The caller must ensure proper access.
    pub unsafe fn as_ptr(&self) -> *mut T {
        self.data.get()
    }

    /// Check if the mutex is currently locked
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

/// RAII guard for a SpinMutex
pub struct SpinMutexGuard<'a, T> {
    mutex: &'a SpinMutex<T>,
}

impl<'a, T> Drop for SpinMutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl<'a, T> Deref for SpinMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> DerefMut for SpinMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

/// Type alias for SpinMutex as SpinLock for compatibility
pub type SpinLock<T> = SpinMutex<T>;

/// Type alias for SpinMutexGuard as SpinLockGuard for compatibility
pub type SpinLockGuard<'a, T> = SpinMutexGuard<'a, T>;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinlock_basic() {
        let mutex = SpinMutex::new(42);
        {
            let guard = mutex.lock();
            assert_eq!(*guard, 42);
            *guard = 100;
        }
        assert_eq!(*mutex.lock(), 100);
    }

    #[test]
    fn test_spinlock_try_lock() {
        let mutex = SpinMutex::new(42);

        {
            let _guard = mutex.lock();
            // Lock is held, try_lock should fail
            assert!(mutex.try_lock().is_none());
        }
        // Lock is released, try_lock should succeed
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn test_spinlock_is_locked() {
        let mutex = SpinMutex::new(42);
        assert!(!mutex.is_locked());

        {
            let _guard = mutex.lock();
            assert!(mutex.is_locked());
        }

        assert!(!mutex.is_locked());
    }
}

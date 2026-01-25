// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 TSC (Time Stamp Counter)
//!
//! This module provides access to the TSC for timing and performance measurements.
//!
//! The TSC is a 64-bit register that counts processor cycles since reset.
//! It provides a high-resolution timestamp for performance measurement.

use core::sync::atomic::{AtomicU64, Ordering};

/// Cached TSC frequency in Hz
static mut TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Default TSC frequency (2 GHz) when not calibrated
const DEFAULT_TSC_FREQUENCY: u64 = 2_000_000_000;

/// Read the Time Stamp Counter
///
/// # Safety
///
/// This function uses inline assembly to read the TSC.
#[inline]
pub unsafe fn rdtsc() -> u64 {
    let mut high: u32;
    let mut low: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Read the Time Stamp Counter with serialization
///
/// This version serializes the instruction stream to prevent
/// out-of-order execution from affecting the timing.
///
/// # Safety
///
/// This function uses inline assembly to read the TSC.
#[inline]
pub unsafe fn rdtsc_serialized() -> u64 {
    let mut high: u32;
    let mut low: u32;
    core::arch::asm!(
        "lfence",
        "rdtsc",
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Get the TSC frequency in Hz
///
/// Returns the cached TSC frequency if available, otherwise
/// returns a default frequency of 2 GHz.
pub fn x86_tsc_frequency() -> u64 {
    unsafe {
        let freq = TSC_FREQUENCY.load(Ordering::Relaxed);
        if freq == 0 {
            DEFAULT_TSC_FREQUENCY
        } else {
            freq
        }
    }
}

/// Set the TSC frequency in Hz
///
/// This should be called during kernel initialization to calibrate
/// the TSC frequency based on platform information.
///
/// # Safety
///
/// This function modifies a global static variable and should only
/// be called once during initialization.
pub unsafe fn x86_set_tsc_frequency(freq: u64) {
    TSC_FREQUENCY.store(freq, Ordering::Release);
}

/// Calibrate the TSC frequency
///
/// Attempts to calibrate the TSC frequency using a known time source
/// (such as the PIT or HPET). Returns the calibrated frequency in Hz.
///
/// # Returns
///
/// The calibrated TSC frequency in Hz, or the default frequency if
/// calibration fails.
pub fn x86_calibrate_tsc() -> u64 {
    // TODO: Implement proper TSC calibration using PIT/HPET/ACPI
    // For now, return a default frequency
    let default_freq = DEFAULT_TSC_FREQUENCY;
    unsafe {
        x86_set_tsc_frequency(default_freq);
    }
    default_freq
}

/// Store the TSC adjustment for suspend/resume
///
/// This function should be called before system suspend to store
/// the current TSC value, allowing for adjustment on resume.
pub fn x86_tsc_store_adjustment() {
    // TODO: Implement TSC adjustment storage for suspend/resume
    // This would typically:
    // 1. Read the current TSC value
    // 2. Store it in a persistent location (e.g., ACPI NVS)
    // 3. On resume, calculate the adjustment needed
}

/// Get TSC ticks since boot
///
/// Returns the raw TSC value, which represents the number of
/// processor cycles since reset.
#[inline]
pub fn tsc_ticks() -> u64 {
    unsafe { rdtsc() }
}

/// Convert TSC ticks to nanoseconds
///
/// # Arguments
///
/// * `ticks` - Number of TSC ticks
///
/// # Returns
///
/// Approximate number of nanoseconds
#[inline]
pub fn tsc_to_ns(ticks: u64) -> u64 {
    let freq = x86_tsc_frequency();
    if freq == 0 {
        return 0;
    }
    // ticks * 1_000_000_000 / freq
    // Use 128-bit arithmetic to avoid overflow
    let ticks = ticks as u128;
    let freq = freq as u128;
    ((ticks * 1_000_000_000) / freq) as u64
}

/// Convert nanoseconds to TSC ticks
///
/// # Arguments
///
/// * `ns` - Number of nanoseconds
///
/// # Returns
///
/// Approximate number of TSC ticks
#[inline]
pub fn ns_to_tsc(ns: u64) -> u64 {
    let freq = x86_tsc_frequency();
    if freq == 0 {
        return 0;
    }
    // ns * freq / 1_000_000_000
    let ns = ns as u128;
    let freq = freq as u128;
    ((ns * freq) / 1_000_000_000) as u64
}

/// TSC-based delay for a specified number of microseconds
///
/// # Arguments
///
/// * `us` - Number of microseconds to delay
pub fn tsc_delay_us(us: u64) {
    let start = tsc_ticks();
    let target_ns = us * 1000;
    let target_ticks = ns_to_tsc(target_ns);

    loop {
        let elapsed = tsc_ticks().wrapping_sub(start);
        if elapsed >= target_ticks {
            break;
        }
        // Use pause to reduce power consumption in spin loop
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
}

/// TSC-based delay for a specified number of milliseconds
///
/// # Arguments
///
/// * `ms` - Number of milliseconds to delay
pub fn tsc_delay_ms(ms: u64) {
    let start = tsc_ticks();
    let target_ns = ms * 1_000_000;
    let target_ticks = ns_to_tsc(target_ns);

    loop {
        let elapsed = tsc_ticks().wrapping_sub(start);
        if elapsed >= target_ticks {
            break;
        }
        // Use pause to reduce power consumption in spin loop
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tsc_read() {
        let tsc1 = unsafe { rdtsc() };
        let tsc2 = unsafe { rdtsc() };

        // TSC should be monotonically increasing
        assert!(tsc2 >= tsc1);
    }

    #[test]
    fn test_tsc_frequency() {
        let freq = x86_tsc_frequency();
        // Frequency should be non-zero (either set or default)
        assert!(freq > 0);
    }

    #[test]
    fn test_tsc_conversion() {
        // Test round-trip conversion
        let ticks = 1_000_000;
        let ns = tsc_to_ns(ticks);
        let back = ns_to_tsc(ns);

        // Allow some tolerance due to integer division
        let diff = if back > ticks {
            back - ticks
        } else {
            ticks - back
        };
        assert!(diff < 1000); // Less than 0.1% error
    }
}

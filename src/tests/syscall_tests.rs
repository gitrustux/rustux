// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! System Call Integration Tests
//!
//! This module tests the system call interface from within the kernel.
//! These tests run in kernel mode and verify the syscall dispatcher
//! and handler implementations.
//!
//! For userspace syscall testing, see the userspace test suite.

use crate::syscall::{self, number, SyscallArgs, SyscallRet};

/// Test syscall dispatch with unknown syscall number
#[test]
fn test_syscall_unknown() {
    let args = SyscallArgs::new(0xFF, [0, 0, 0, 0, 0, 0]);
    let result = syscall::syscall_dispatch(args);

    // Should return negative error code
    assert!(result < 0, "Unknown syscall should return error");
}

/// Test syscall dispatch with CLOCK_GET (only implemented syscall)
#[test]
fn test_syscall_clock_get() {
    let args = SyscallArgs::new(
        number::CLOCK_GET,
        [0, 0, 0, 0, 0, 0], // clock_id = 0 (MONOTONIC)
    );
    let result = syscall::syscall_dispatch(args);

    // Should return positive or zero (time in nanoseconds)
    assert!(result >= 0, "CLOCK_GET should return valid time");
}

/// Test syscall args access methods
#[test]
fn test_syscall_args_access() {
    let args = SyscallArgs::new(
        0x10,
        [0x1111, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666],
    );

    assert_eq!(args.number, 0x10);
    assert_eq!(args.arg(0), 0x1111);
    assert_eq!(args.arg(1), 0x2222);
    assert_eq!(args.arg(2), 0x3333);
    assert_eq!(args.arg(3), 0x4444);
    assert_eq!(args.arg(4), 0x5555);
    assert_eq!(args.arg(5), 0x6666);

    // Test out of range
    assert_eq!(args.arg(10), 0);

    // Test typed access
    assert_eq!(args.arg_u32(0), 0x1111);
    assert_eq!(args.arg_u64(1), 0x2222);
}

/// Test all syscall stubs return NOT_SUPPORTED
#[test]
fn test_syscall_stubs() {
    let test_cases = vec![
        (number::PROCESS_CREATE, "PROCESS_CREATE"),
        (number::PROCESS_START, "PROCESS_START"),
        (number::THREAD_CREATE, "THREAD_CREATE"),
        (number::THREAD_START, "THREAD_START"),
        (number::THREAD_EXIT, "THREAD_EXIT"),
        (number::PROCESS_EXIT, "PROCESS_EXIT"),
        (number::VMO_CREATE, "VMO_CREATE"),
        (number::VMO_READ, "VMO_READ"),
        (number::VMO_WRITE, "VMO_WRITE"),
        (number::VMO_CLONE, "VMO_CLONE"),
        (number::VMAR_MAP, "VMAR_MAP"),
        (number::VMAR_UNMAP, "VMAR_UNMAP"),
        (number::VMAR_PROTECT, "VMAR_PROTECT"),
        (number::CHANNEL_CREATE, "CHANNEL_CREATE"),
        (number::CHANNEL_WRITE, "CHANNEL_WRITE"),
        (number::CHANNEL_READ, "CHANNEL_READ"),
        (number::EVENT_CREATE, "EVENT_CREATE"),
        (number::EVENTPAIR_CREATE, "EVENTPAIR_CREATE"),
        (number::OBJECT_SIGNAL, "OBJECT_SIGNAL"),
        (number::OBJECT_WAIT_ONE, "OBJECT_WAIT_ONE"),
        (number::OBJECT_WAIT_MANY, "OBJECT_WAIT_MANY"),
        (number::JOB_CREATE, "JOB_CREATE"),
        (number::HANDLE_DUPLICATE, "HANDLE_DUPLICATE"),
        (number::HANDLE_TRANSFER, "HANDLE_TRANSFER"),
        (number::TIMER_CREATE, "TIMER_CREATE"),
        (number::TIMER_SET, "TIMER_SET"),
        (number::TIMER_CANCEL, "TIMER_CANCEL"),
    ];

    for (syscall_num, name) in test_cases {
        let args = SyscallArgs::new(syscall_num, [0, 0, 0, 0, 0, 0]);
        let result = syscall::syscall_dispatch(args);

        // Most stubs should return ERR_NOT_SUPPORTED (-1)
        // HANDLE_CLOSE returns 0 (success)
        if syscall_num == number::HANDLE_CLOSE {
            assert_eq!(
                result, 0,
                "{} should return 0 (currently always succeeds)",
                name
            );
        } else if syscall_num == number::CLOCK_GET {
            assert!(
                result >= 0,
                "{} (CLOCK_GET) should return valid time, got {}",
                name,
                result
            );
        } else {
            assert_eq!(
                result,
                -1,
                "{} should return ERR_NOT_SUPPORTED (-1), got {}",
                name,
                result
            );
        }
    }
}

/// Test syscall number constants
#[test]
fn test_syscall_numbers() {
    // Process & Thread (0x01-0x0F)
    assert_eq!(number::PROCESS_CREATE, 0x01);
    assert_eq!(number::PROCESS_START, 0x02);
    assert_eq!(number::THREAD_CREATE, 0x03);
    assert_eq!(number::THREAD_START, 0x04);
    assert_eq!(number::THREAD_EXIT, 0x05);
    assert_eq!(number::PROCESS_EXIT, 0x06);
    assert_eq!(number::HANDLE_CLOSE, 0x07);

    // Memory / VMO (0x10-0x1F)
    assert_eq!(number::VMO_CREATE, 0x10);
    assert_eq!(number::VMO_READ, 0x11);
    assert_eq!(number::VMO_WRITE, 0x12);
    assert_eq!(number::VMO_CLONE, 0x13);
    assert_eq!(number::VMAR_MAP, 0x14);
    assert_eq!(number::VMAR_UNMAP, 0x15);
    assert_eq!(number::VMAR_PROTECT, 0x16);

    // IPC & Sync (0x20-0x2F)
    assert_eq!(number::CHANNEL_CREATE, 0x20);
    assert_eq!(number::CHANNEL_WRITE, 0x21);
    assert_eq!(number::CHANNEL_READ, 0x22);
    assert_eq!(number::EVENT_CREATE, 0x23);
    assert_eq!(number::EVENTPAIR_CREATE, 0x24);
    assert_eq!(number::OBJECT_SIGNAL, 0x25);
    assert_eq!(number::OBJECT_WAIT_ONE, 0x26);
    assert_eq!(number::OBJECT_WAIT_MANY, 0x27);

    // Jobs & Handles (0x30-0x3F)
    assert_eq!(number::JOB_CREATE, 0x30);
    assert_eq!(number::HANDLE_DUPLICATE, 0x31);
    assert_eq!(number::HANDLE_TRANSFER, 0x32);

    // Time (0x40-0x4F)
    assert_eq!(number::CLOCK_GET, 0x40);
    assert_eq!(number::TIMER_CREATE, 0x41);
    assert_eq!(number::TIMER_SET, 0x42);
    assert_eq!(number::TIMER_CANCEL, 0x43);
}

/// Test clock monotonic returns increasing values
#[test]
fn test_clock_monotonic_increases() {
    let args1 = SyscallArgs::new(number::CLOCK_GET, [0, 0, 0, 0, 0, 0]);
    let time1 = syscall::syscall_dispatch(args1);

    // Simulate some work
    let mut sum = 0u64;
    for i in 0..1000 {
        sum = sum.wrapping_add(i);
    }
    let _ = sum; // Use the value to avoid optimization

    let args2 = SyscallArgs::new(number::CLOCK_GET, [0, 0, 0, 0, 0, 0]);
    let time2 = syscall::syscall_dispatch(args2);

    // Time should have increased (or stayed same on fast systems)
    assert!(time2 >= time1, "Monotonic clock should not go backwards");
}

/// Test syscall with maximum syscall number
#[test]
fn test_syscall_max_number() {
    let args = SyscallArgs::new(number::MAX_SYSCALL, [0, 0, 0, 0, 0, 0]);
    let result = syscall::syscall_dispatch(args);

    // TIMER_CANCEL is the highest syscall (0x43)
    // It's a stub, so should return NOT_SUPPORTED
    assert_eq!(result, -1, "MAX_SYSCALL should be a stub returning -1");
}

/// Test syscall with number beyond max
#[test]
fn test_syscall_beyond_max() {
    let args = SyscallArgs::new(number::MAX_SYSCALL + 1, [0, 0, 0, 0, 0, 0]);
    let result = syscall::syscall_dispatch(args);

    // Should return NOT_SUPPORTED for unknown syscalls
    assert_eq!(result, -1, "Syscall beyond MAX should return NOT_SUPPORTED");
}

/// Test HANDLE_CLOSE syscall
#[test]
fn test_handle_close() {
    // HANDLE_CLOSE is currently a stub that always returns 0
    let args = SyscallArgs::new(number::HANDLE_CLOSE, [12345, 0, 0, 0, 0, 0]);
    let result = syscall::syscall_dispatch(args);

    // Should succeed (currently always returns 0)
    assert_eq!(result, 0, "HANDLE_CLOSE should return 0");
}

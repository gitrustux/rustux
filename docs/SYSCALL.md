# Rustux Kernel - System Call Reference

**Version:** 0.2.0
**ABI Version:** Stable v1
**Date:** 2025-01-18

---

## Table of Contents

1. [Overview](#overview)
2. [Calling Convention](#calling-convention)
3. [Error Handling](#error-handling)
4. [System Call Reference](#system-call-reference)
5. [Implementation Status](#implementation-status)
6. [Usage Examples](#usage-examples)
7. [Future Additions](#future-additions)

---

## Overview

The Rustux system call interface provides a stable, cross-architecture ABI for userspace programs to interact with the kernel. The design follows these principles:

- **Stability:** Syscall numbers and semantics are frozen across architectures
- **Object-based:** All operations work on handles with associated rights
- **Deterministic:** Same inputs → same outputs → same errors
- **No arch leakage:** CPU differences are hidden below the ABI

### Architecture Support

| Architecture | Syscall Instruction | Status |
|--------------|---------------------|--------|
| AMD64 (x86_64) | `syscall` | ✅ Implemented |
| ARM64 (AArch64) | `svc #0` | 🔶 ABI defined |
| RISC-V | `ecall` | 🔶 ABI defined |

---

## Calling Convention

### AMD64 (x86_64)

```asm
; System call via `syscall` instruction
mov rax, <syscall_number>  ; Syscall number
mov rdi, <arg1>             ; First argument
mov rsi, <arg2>             ; Second argument
mov rdx, <arg3>             ; Third argument
mov r10, <arg4>             ; Fourth argument
mov r8,  <arg5>             ; Fifth argument
mov r9,  <arg6>             ; Sixth argument
syscall                     ; Enter kernel
; Result in rax
```

**Register Usage:**
- `rax`: Syscall number (in) / Return value (out)
- `rdi, rsi, rdx, r10, r8, r9`: Arguments (in order)
- `rcx`: Return address (saved by `syscall`)
- `r11`: RFLAGS (saved by `syscall`)

### ARM64 (AArch64)

```asm
; System call via `svc #0` instruction
mov x8,  <syscall_number>   ; Syscall number
mov x0,  <arg1>             ; First argument
mov x1,  <arg2>             ; Second argument
mov x2,  <arg3>             ; Third argument
mov x3,  <arg4>             ; Fourth argument
mov x4,  <arg5>             ; Fifth argument
mov x5,  <arg6>             ; Sixth argument
svc #0                      ; Enter kernel
; Result in x0
```

**Register Usage:**
- `x8`: Syscall number (in) / Return value (out)
- `x0-x5`: Arguments (in order)

### RISC-V

```asm
; System call via `ecall` instruction
li a7,  <syscall_number>    ; Syscall number
mv a0,  <arg1>              ; First argument
mv a1,  <arg2>              ; Second argument
mv a2,  <arg3>              ; Third argument
mv a3,  <arg4>              ; Fourth argument
mv a4,  <arg5>              ; Fifth argument
mv a5,  <arg6>              ; Sixth argument
ecall                       ; Enter kernel
; Result in a0
```

**Register Usage:**
- `a7`: Syscall number (in) / Return value (out)
- `a0-a5`: Arguments (in order)

---

## Error Handling

### Return Value Convention

All system calls use a unified return value convention:

```text
Success: Return value in rax/x0/a0 (positive or zero)
Failure: Return negative error code
```

### Error Codes

| Error Code | Value | Description |
|------------|-------|-------------|
| `ERR_NOT_SUPPORTED` | -1 | Syscall not implemented |
| `ERR_NO_MEMORY` | -2 | Out of memory |
| `ERR_INVALID_ARGS` | -3 | Invalid arguments |
| `ERR_BAD_HANDLE` | -4 | Invalid handle |
| `ERR_ACCESS_DENIED` | -5 | Insufficient rights |
| `ERR_IO` | -6 | I/O error |
| `ERR_TIMED_OUT` | -7 | Operation timed out |
| `ERR_INTERRUPTED` | -8 | Operation interrupted |
| `ERR_NOT_FOUND` | -9 | Resource not found |
| `ERR_ALREADY_EXISTS` | -10 | Resource already exists |

### Checking for Errors (C)

```c
ssize_t result = syscall(SYS_PROCESS_CREATE, arg1, arg2);
if (result < 0) {
    // Error occurred
    int error_code = -result;
    switch (error_code) {
        case 1:  // ERR_NOT_SUPPORTED
            fprintf(stderr, "Syscall not implemented\n");
            break;
        case 2:  // ERR_NO_MEMORY
            fprintf(stderr, "Out of memory\n");
            break;
        // ... other error codes
    }
} else {
    // Success
    handle_t handle = (handle_t)result;
}
```

### Checking for Errors (Rust)

```rust
let result = unsafe { syscall_process_create(arg1, arg2) };
if result < 0 {
    let error_code = -result;
    match error_code {
        1 => eprintln!("Syscall not implemented"),
        2 => eprintln!("Out of memory"),
        // ... other error codes
        _ => eprintln!("Unknown error: {}", error_code),
    }
} else {
    let handle = result as u32;
    // Success
}
```

---

## System Call Reference

### Process & Thread (0x01-0x0F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `PROCESS_CREATE` | 0x01 | Create a new process | 🔶 Stub |
| `PROCESS_START` | 0x02 | Start a created process | 🔶 Stub |
| `THREAD_CREATE` | 0x03 | Create a new thread | 🔶 Stub |
| `THREAD_START` | 0x04 | Start a created thread | 🔶 Stub |
| `THREAD_EXIT` | 0x05 | Exit current thread | 🔶 Stub |
| `PROCESS_EXIT` | 0x06 | Exit current process | 🔶 Stub |
| `HANDLE_CLOSE` | 0x07 | Close a handle | 🔶 Stub |

#### PROCESS_CREATE (0x01)

Create a new process object.

**Arguments:**
- `arg0`: Process creation flags (reserved, set to 0)
- `arg1`: Name pointer (userspace virtual address)
- `arg2`: Name length

**Returns:**
- Success: Handle to the new process
- Failure: Negative error code

**Example:**
```c
const char* name = "my_process";
handle_t process = syscall(SYS_PROCESS_CREATE, 0, name, strlen(name));
if (process < 0) {
    perror("Failed to create process");
}
```

#### THREAD_EXIT (0x05)

Exit the current thread.

**Arguments:**
- `arg0`: Exit code

**Returns:**
- Does not return (thread terminates)

#### PROCESS_EXIT (0x06)

Exit the current process and all its threads.

**Arguments:**
- `arg0`: Exit code

**Returns:**
- Does not return (process terminates)

#### HANDLE_CLOSE (0x07)

Close a handle, releasing the reference to the kernel object.

**Arguments:**
- `arg0`: Handle to close

**Returns:**
- Success: 0
- Failure: Negative error code

---

### Memory / VMO (0x10-0x1F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `VMO_CREATE` | 0x10 | Create a Virtual Memory Object | 🔶 Stub |
| `VMO_READ` | 0x11 | Read from a VMO | 🔶 Stub |
| `VMO_WRITE` | 0x12 | Write to a VMO | 🔶 Stub |
| `VMO_CLONE` | 0x13 | Clone a VMO | 🔶 Stub |
| `VMAR_MAP` | 0x14 | Map a VMO into address space | 🔶 Stub |
| `VMAR_UNMAP` | 0x15 | Unmap a VMO from address space | 🔶 Stub |
| `VMAR_PROTECT` | 0x16 | Change memory protection | 🔶 Stub |

#### VMO_CREATE (0x10)

Create a Virtual Memory Object (VMO) - a contiguous region of memory that can be mapped into process address spaces.

**Arguments:**
- `arg0`: Size of VMO in bytes
- `arg1`: VMO flags (reserved, set to 0)

**Returns:**
- Success: Handle to the new VMO
- Failure: Negative error code

**Example:**
```c
// Create a 4KB VMO
handle_t vmo = syscall(SYS_VMO_CREATE, 4096, 0);
if (vmo < 0) {
    perror("Failed to create VMO");
}
```

#### VMAR_MAP (0x14)

Map a VMO into the current process's address space.

**Arguments:**
- `arg0`: VMO handle to map
- `arg1`: Virtual address hint (0 for any)
- `arg2`: Size to map
- `arg3`: Protection flags (READ=1, WRITE=2, EXEC=4)
- `arg4`: Mapping flags (reserved, set to 0)

**Returns:**
- Success: Mapped virtual address
- Failure: Negative error code

---

### IPC & Sync (0x20-0x2F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `CHANNEL_CREATE` | 0x20 | Create an IPC channel | 🔶 Stub |
| `CHANNEL_WRITE` | 0x21 | Write to a channel | 🔶 Stub |
| `CHANNEL_READ` | 0x22 | Read from a channel | 🔶 Stub |
| `EVENT_CREATE` | 0x23 | Create an event object | 🔶 Stub |
| `EVENTPAIR_CREATE` | 0x24 | Create an event pair | 🔶 Stub |
| `OBJECT_SIGNAL` | 0x25 | Signal an object | 🔶 Stub |
| `OBJECT_WAIT_ONE` | 0x26 | Wait on one object | 🔶 Stub |
| `OBJECT_WAIT_MANY` | 0x27 | Wait on multiple objects | 🔶 Stub |

#### CHANNEL_CREATE (0x20)

Create a bidirectional IPC channel for message passing between processes.

**Arguments:**
- `arg0`: Channel options (reserved, set to 0)

**Returns:**
- Success: Handle to the new channel
- Failure: Negative error code

**Example:**
```c
handle_t channel = syscall(SYS_CHANNEL_CREATE, 0);
if (channel < 0) {
    perror("Failed to create channel");
}
```

#### CHANNEL_WRITE (0x21)

Write a message to a channel.

**Arguments:**
- `arg0`: Channel handle
- `arg1`: Message buffer pointer
- `arg2`: Message size
- `arg3`: Handle array pointer (optional)
- `arg4`: Handle count

**Returns:**
- Success: Number of bytes written
- Failure: Negative error code

#### CHANNEL_READ (0x22)

Read a message from a channel.

**Arguments:**
- `arg0`: Channel handle
- `arg1`: Buffer pointer
- `arg2`: Buffer size
- `arg3`: Handle array pointer (optional)
- `arg4`: Handle array capacity

**Returns:**
- Success: Number of bytes read
- Failure: Negative error code

---

### Jobs & Handles (0x30-0x3F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `JOB_CREATE` | 0x30 | Create a job object | 🔶 Stub |
| `HANDLE_DUPLICATE` | 0x31 | Duplicate a handle | 🔶 Stub |
| `HANDLE_TRANSFER` | 0x32 | Transfer a handle | 🔶 Stub |

#### HANDLE_DUPLICATE (0x31)

Duplicate a handle, potentially with reduced rights.

**Arguments:**
- `arg0`: Handle to duplicate
- `arg1`: Rights mask (optional, 0 for same rights)

**Returns:**
- Success: New handle
- Failure: Negative error code

---

### Time (0x40-0x4F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `CLOCK_GET` | 0x40 | Get current time | ✅ Working |
| `TIMER_CREATE` | 0x41 | Create a timer object | 🔶 Stub |
| `TIMER_SET` | 0x42 | Set a timer | 🔶 Stub |
| `TIMER_CANCEL` | 0x43 | Cancel a timer | 🔶 Stub |

#### CLOCK_GET (0x40)

Get the current time from the specified clock.

**Arguments:**
- `arg0`: Clock ID (0=MONOTONIC, 1=UTC, 2=THREAD_TIME)

**Returns:**
- Success: Time in nanoseconds
- Failure: Negative error code

**Example:**
```c
// Get monotonic time (always increases, not affected by system time changes)
int64_t time_ns = syscall(SYS_CLOCK_GET, 0);
if (time_ns < 0) {
    perror("Failed to get time");
} else {
    printf("Current time: %lld ns\n", time_ns);
}
```

**Implementation Note:** Currently uses TSC (Time Stamp Counter) converted to nanoseconds.

---

## Implementation Status

### Summary

| Category | Total | Implemented | Stub |
|----------|-------|-------------|------|
| Process & Thread | 7 | 0 | 7 |
| Memory / VMO | 7 | 0 | 7 |
| IPC & Sync | 8 | 0 | 8 |
| Jobs & Handles | 3 | 0 | 3 |
| Time | 4 | 1 | 3 |
| **Total** | **29** | **1** | **28** |

### Priority Implementation Order

For basic userspace execution, the following syscalls should be implemented first:

1. **PROCESS_EXIT** (0x06) - Required for process termination
2. **HANDLE_CLOSE** (0x07) - Required for cleanup
3. **VMO_CREATE** (0x10) - Required for memory allocation
4. **VMAR_MAP** (0x14) - Required for memory mapping
5. **PROCESS_CREATE** (0x01) - Required for spawning processes

---

## Usage Examples

### Example 1: Creating a Process

```c
#include <stdint.h>
#include <stddef.h>

// Syscall numbers
#define SYS_PROCESS_CREATE  0x01
#define SYS_PROCESS_START   0x02
#define SYS_PROCESS_EXIT    0x06
#define SYS_HANDLE_CLOSE    0x07

typedef uint32_t handle_t;

// Inline syscall wrapper for AMD64
static inline long syscall1(long num, long arg1) {
    long result;
    asm volatile (
        "syscall"
        : "=a"(result)
        : "a"(num), "D"(arg1)
        : "rcx", "r11", "memory"
    );
    return result;
}

static inline long syscall3(long num, long arg1, long arg2, long arg3) {
    long result;
    asm volatile (
        "syscall"
        : "=a"(result)
        : "a"(num), "D"(arg1), "S"(arg2), "d"(arg3)
        : "rcx", "r11", "memory"
    );
    return result;
}

int main() {
    // Create a new process
    const char* name = "test_process";
    handle_t process = (handle_t)syscall3(
        SYS_PROCESS_CREATE,
        0,              // flags
        (long)name,     // name pointer
        12              // name length
    );

    if (process < 0) {
        return -1;  // Error
    }

    // Start the process (implementation-specific)
    // ...

    // Close the handle when done
    syscall1(SYS_HANDLE_CLOSE, process);

    // Exit current process
    syscall1(SYS_PROCESS_EXIT, 0);

    return 0;  // Never reached
}
```

### Example 2: Memory Allocation

```c
#define SYS_VMO_CREATE    0x10
#define SYS_VMAR_MAP      0x14

// Allocate 4KB of memory
handle_t vmo = (handle_t)syscall1(SYS_VMO_CREATE, 4096);

if (vmo < 0) {
    return -1;  // Error
}

// Map the VMO into our address space with read/write permissions
void* ptr = (void*)syscall5(
    SYS_VMAR_MAP,
    vmo,        // VMO handle
    0,          // Virtual address hint (any)
    4096,       // Size
    3,          // Protection (1=READ, 2=WRITE, 3=RW)
    0           // Flags
);

if (ptr < 0) {
    syscall1(SYS_HANDLE_CLOSE, vmo);
    return -1;  // Error
}

// Use the memory
char* buffer = (char*)ptr;
buffer[0] = 'H';
buffer[1] = 'e';
buffer[2] = 'l';
buffer[3] = 'l';
buffer[4] = 'o';

// Cleanup would involve unmapping and closing handles
```

---

## Future Additions

### Planned Syscalls (ABI v2)

The following syscalls are planned for future additions but are NOT part of the stable v1 ABI:

| Syscall | Description | Priority |
|---------|-------------|----------|
| `PROCESS_GET_INFO` | Get process information | High |
| `THREAD_GET_INFO` | Get thread information | High |
| `VMO_SET_SIZE` | Resize a VMO | Medium |
| `CHANNEL_QUERY` | Query channel state | Medium |
| `EVENT_RESET` | Reset an event object | Low |
| `FDIO_READ` | Read from file descriptor | High |
| `FDIO_WRITE` | Write to file descriptor | High |

### File I/O Syscalls

File I/O will be handled through the VMO and channel syscalls, with dedicated file descriptor syscalls planned:

- `FDIO_OPEN` - Open a file
- `FDIO_CLOSE` - Close a file descriptor
- `FDIO_READ` - Read from a file descriptor
- `FDIO_WRITE` - Write to a file descriptor
- `FDIO_SEEK` - Seek in a file
- `FDIO_STAT` - Get file information

---

## References

- **Zircon Syscalls:** https://fuchsia.dev/fuchsia-src/reference/syscalls
- **Linux Syscalls:** https://man7.org/linux/man-pages/man2/syscalls.2.html
- **AMD64 ABI:** https://gitlab.com/x86-psABIs/x86-64-ABI

---

*Last Updated: 2025-01-18*
*Author: Rustux Kernel Team*
*License: MIT*

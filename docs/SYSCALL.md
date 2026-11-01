# Rustux Kernel - System Call Reference

**Version:** 0.3.0
**ABI Version:** Stable v1
**Date:** 2026-11-01

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

> **Current implementation (2026-10-31):** the kernel enters system calls through `int 0x80` (vector 0x80, DPL 3), not the `syscall` instruction yet. The registers are the same: number in `rax`, arguments in `rdi, rsi, rdx, r10, r8, r9`, result in `rax`; all other registers are preserved. Programs built with the `syscall` instruction fault with "invalid opcode".

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

Errors are returned as the negative of these values (`-10` = `ERR_BAD_HANDLE`).
The C header `userspace/c-progs/rx.h` defines them as `RX_ERR_*`.

| Error Code | Value | Description |
|------------|-------|-------------|
| `ERR_INVALID_ARGS` | 1 | Invalid argument, or a user pointer that is unmapped, read-only when written, or not in the lower half |
| `ERR_NO_MEMORY` | 2 | Out of memory, or the handle table is full (1024 handles) |
| `ERR_NOT_IMPLEMENTED` | 3 | Syscall is still a stub |
| `ERR_ACCESS_DENIED` | 4 | The handle lacks a required right |
| `ERR_NOT_FOUND` | 5 | Resource not found |
| `ERR_BUSY` | 6 | Resource busy |
| `ERR_IO` | 7 | I/O error |
| `ERR_INTERNAL` | 8 | Internal kernel error |
| `ERR_NOT_SUPPORTED` | 9 | Operation not supported (unknown options or clock) |
| `ERR_BAD_HANDLE` | 10 | Handle is not valid in this process (never existed, closed, or stale) |
| `ERR_WRONG_TYPE` | 11 | Handle refers to an object of the wrong type |
| `ERR_TIMED_OUT` | 12 | Deadline passed before the signals were seen |
| `ERR_SHOULD_WAIT` | 13 | Operation would block |
| `ERR_PEER_CLOSED` | 14 | The other end of a pair was closed |
| `ERR_BUFFER_TOO_SMALL` | 15 | Output buffer too small |
| `ERR_OUT_OF_RANGE` | 16 | Value out of range |
| `ERR_BAD_STATE` | 17 | Object or process in the wrong state |

Checks happen in this order: handle valid (`BAD_HANDLE`), object type (`WRONG_TYPE`), rights (`ACCESS_DENIED`), then arguments.

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

#### HANDLE_CLOSE (0x07) ✅

Close a handle. The object is destroyed when its last handle is closed
(closing one end of an event pair sets `PEER_CLOSED` on the other end).
All handles of a process are closed when it exits.

**Arguments:**
- `arg0`: Handle to close (`0` is accepted and does nothing)

**Returns:** 0, or `ERR_BAD_HANDLE`

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

### Handles, Rights and Signals

Every kernel object is reached through a **handle**, a 32-bit value that is
only meaningful in the process that owns it. A handle carries **rights**;
closing a handle never affects other handles to the same object. Handle
values encode a slot and a generation, so a closed handle value is not
reused straight away and stale values fail with `ERR_BAD_HANDLE`.

**Rights** (Zircon values):

| Right | Bit | Allows |
|-------|-----|--------|
| `DUPLICATE` | 0 | `handle_duplicate` |
| `TRANSFER` | 1 | Sending the handle over a channel (planned) |
| `READ` | 2 | Reading state or data |
| `WRITE` | 3 | Changing state (e.g. `timer_set`) |
| `SIGNAL` | 12 | `object_signal` |
| `SIGNAL_PEER` | 13 | `object_signal_peer` |
| `WAIT` | 14 | `object_wait_one` / `object_wait_many` |
| `INSPECT` | 15 | Querying information |
| `SAME_RIGHTS` | 31 | In `handle_duplicate`: keep the original rights |

Default rights: event = BASIC + SIGNAL; event pair = BASIC + SIGNAL + SIGNAL_PEER;
timer = BASIC + WRITE + SIGNAL, where BASIC = DUPLICATE + TRANSFER + WAIT + INSPECT.

**Signals** are 32 bits of state on every object. Bits 24–31 are user signals
that any holder with `SIGNAL` may set or clear; the lower bits belong to the
kernel:

| Signal | Bit | Meaning |
|--------|-----|---------|
| `CHANNEL_READABLE` | 0 | A message is waiting (channels, planned) |
| `CHANNEL_WRITABLE` | 1 | The channel can accept a message (planned) |
| `PEER_CLOSED` | 2 | The other end of a pair was closed |
| `EVENT_SIGNALED` / `TIMER_SIGNALED` | 3 | Event set by a user, or timer deadline reached |
| `USER_SIGNAL_0..7` | 24–31 | Free for programs |

### Waiting

A wait blocks the calling process until any of the requested signals is set
on the object, or the **deadline** passes. Deadlines are absolute times in
nanoseconds on the monotonic clock (`clock_get(0)`); `0` means poll and
`UINT64_MAX` means wait forever. While a process is blocked it uses no CPU;
the scheduler runs other processes or halts the CPU until the next interrupt.
Deadline resolution is the 10 ms scheduler tick.

---

### IPC & Sync (0x20-0x2F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `CHANNEL_CREATE` | 0x20 | Create an IPC channel | 🔶 Stub (next) |
| `CHANNEL_WRITE` | 0x21 | Write to a channel | 🔶 Stub (next) |
| `CHANNEL_READ` | 0x22 | Read from a channel | 🔶 Stub (next) |
| `EVENT_CREATE` | 0x23 | Create an event | ✅ |
| `EVENTPAIR_CREATE` | 0x24 | Create an event pair | ✅ |
| `OBJECT_SIGNAL` | 0x25 | Set/clear signals on an object | ✅ |
| `OBJECT_WAIT_ONE` | 0x26 | Wait on one object | ✅ |
| `OBJECT_WAIT_MANY` | 0x27 | Wait on up to 64 objects | ✅ |
| `OBJECT_SIGNAL_PEER` | 0x28 | Set/clear signals on the peer of a pair | ✅ |
| `OBJECT_GET_TYPE` | 0x29 | Object type of a handle | ✅ |

#### EVENT_CREATE (0x23)

`event_create(uint32_t options, rx_handle_t *out)`

- `options` must be 0 (`ERR_INVALID_ARGS` otherwise)
- Writes the new handle to `*out`. Returns 0.

#### EVENTPAIR_CREATE (0x24)

`eventpair_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1)`

Creates two linked events. Each end can signal the other with
`object_signal_peer`; closing one end sets `PEER_CLOSED` on the other.

#### OBJECT_SIGNAL (0x25)

`object_signal(rx_handle_t h, uint32_t clear_mask, uint32_t set_mask)`

Clears then sets signals on the object. Needs `SIGNAL`. Only user signals
and `EVENT_SIGNALED` (on events and event pairs) may be changed; any other
bit gives `ERR_INVALID_ARGS`. Waiters are woken if the signals change.

#### OBJECT_SIGNAL_PEER (0x28)

`object_signal_peer(rx_handle_t h, uint32_t clear_mask, uint32_t set_mask)`

Same as `object_signal`, applied to the other end of an event pair. Needs
`SIGNAL_PEER`. `ERR_WRONG_TYPE` on anything but an event pair,
`ERR_PEER_CLOSED` if the other end is gone.

#### OBJECT_WAIT_ONE (0x26)

`object_wait_one(rx_handle_t h, uint32_t signals, uint64_t deadline, uint32_t *observed)`

Needs `WAIT`. Returns 0 when any bit of `signals` is set, or
`ERR_TIMED_OUT` when the deadline passes. In both cases the object's
current signals are written to `*observed` (which may be NULL).

#### OBJECT_WAIT_MANY (0x27)

`object_wait_many(rx_wait_item_t *items, uint64_t count, uint64_t deadline)`

```c
typedef struct {
    rx_handle_t handle;   // in
    uint32_t waitfor;     // in: signals to wait for
    uint32_t pending;     // out: the object's signals
} rx_wait_item_t;
```

Waits until any item has one of its `waitfor` signals. `count` is 1–64.
Every item's `pending` is filled in on return (including on `ERR_TIMED_OUT`).

#### OBJECT_GET_TYPE (0x29)

`object_get_type(rx_handle_t h)` returns the object type: 5 = event,
16 = event pair, 22 = timer (Zircon values). Needs no rights.

#### CHANNEL_CREATE / WRITE / READ (0x20-0x22) — planned

Bidirectional message pipes that carry bytes and handles. Being implemented
next; see the checklist.

---

### Jobs & Handles (0x30-0x3F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `JOB_CREATE` | 0x30 | Create a job object | 🔶 Stub |
| `HANDLE_DUPLICATE` | 0x31 | Duplicate a handle | ✅ |
| `HANDLE_TRANSFER` | 0x32 | Transfer a handle | 🔶 Stub |

#### HANDLE_DUPLICATE (0x31)

`handle_duplicate(rx_handle_t h, uint32_t rights, rx_handle_t *out)`

Creates a second handle to the same object. Needs `DUPLICATE`. `rights` is
`SAME_RIGHTS` or a subset of the original rights; asking for a right the
original lacks gives `ERR_INVALID_ARGS`. Use this to hand out a weaker
handle, e.g. one that can only wait.

---

### Time (0x40-0x4F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `CLOCK_GET` | 0x40 | Get current time | ✅ |
| `TIMER_CREATE` | 0x41 | Create a timer object | ✅ |
| `TIMER_SET` | 0x42 | Arm a timer | ✅ |
| `TIMER_CANCEL` | 0x43 | Disarm a timer | ✅ |

#### CLOCK_GET (0x40)

`clock_get(uint32_t clock_id)` returns nanoseconds. Clock 0 is monotonic
(from the calibrated TSC); it is the clock used for all deadlines.

#### TIMER_CREATE (0x41)

`timer_create(uint32_t options, uint32_t clock_id, rx_handle_t *out)`

`options` must be 0 and `clock_id` 0 (monotonic).

#### TIMER_SET (0x42)

`timer_set(rx_handle_t h, uint64_t deadline, uint64_t slack)`

Needs `WRITE`. Clears `TIMER_SIGNALED` and arms the timer;
`TIMER_SIGNALED` is set once the monotonic clock reaches `deadline`.
`slack` is accepted and ignored. Setting an armed timer re-arms it.

#### TIMER_CANCEL (0x43)

`timer_cancel(rx_handle_t h)` — needs `WRITE`. Disarms the timer;
`TIMER_SIGNALED` is left as it was.

---

## Implementation Status

### Summary

| Category | Total | Implemented | Stub |
|----------|-------|-------------|------|
| Process & Thread | 7 | 3 (spawn, process_exit, handle_close) | 4 |
| Memory / VMO | 7 | 0 | 7 |
| IPC & Sync | 10 | 7 | 3 (channels) |
| Jobs & Handles | 3 | 1 | 2 |
| Time | 4 | 4 | 0 |

The test program `objtest` (in the ramdisk as `/bin/objtest`, source in
`userspace/c-progs/objtest.c`) exercises everything above; run it from the
shell and it prints one line per check.

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

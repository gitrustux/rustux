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
6. [Example](#example-start-a-child-process-and-talk-to-it)

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
| AMD64 (x86_64) | `int 0x80` (see note below) | ✅ Implemented |
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
| `ERR_ALREADY_EXISTS` | 18 | Something is already there (e.g. a mapping at a specific address) |
| `ERR_CANCELED` | 19 | The wait was cancelled (object destroyed, thread killed) |

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
| `PROCESS_CREATE` | 0x01 | Create an empty process in a job | ✅ |
| `PROCESS_START` | 0x02 | Start a process's first thread | ✅ |
| `SPAWN` | 0x03 | Start `/bin/<name>` from the ramdisk (legacy) | ✅ frozen |
| `THREAD_START` | 0x04 | Start another thread in a running process | ✅ |
| `THREAD_EXIT` | 0x05 | Exit the current thread | ✅ |
| `PROCESS_EXIT` | 0x06 | Exit the current process | ✅ |
| `HANDLE_CLOSE` | 0x07 | Close a handle | ✅ |
| `THREAD_CREATE` | 0x08 | Create a thread object in a process | ✅ |
| `TASK_KILL` | 0x09 | Kill a job, process or thread | ✅ |
| `OBJECT_GET_INFO` | 0x0A | Query an object (`ZX_INFO_PROCESS` only) | ✅ |
| `SPAWN_PROCESS` | 0x0B | Like `SPAWN`, and return a process handle | ✅ |

**Model** (Zircon): a **job** holds processes and child jobs. A **process**
owns a handle table and an address space and starts with no threads;
`process_start` runs its first thread. A **thread** is what the scheduler
runs. A process ends when its last thread exits, when it calls
`process_exit`, or when it or its job is killed; then its handles are
closed, its memory is freed, and its handle gets `PROCESS_TERMINATED`
(bit 3). A thread's handle gets `THREAD_TERMINATED` (bit 3) when it stops.
A killed thread that is blocked in the kernel wakes up, abandons the call
and exits before it would return to user mode.

Rights: process = BASIC + READ + WRITE + PROPERTY + ENUMERATE + DESTROY +
SIGNAL + MANAGE_PROCESS + MANAGE_THREAD; thread = BASIC + READ + WRITE +
PROPERTY + DESTROY + SIGNAL + MANAGE_THREAD; job = the process rights +
MANAGE_JOB.

**Program start.** A thread starts with its first two arguments in RDI and
RSI and every other register zero. Programs started by the kernel (`init`,
`spawn`) get a **bootstrap channel** in RDI holding one message: one u32
tag per handle, with handles to the process itself (`PA_PROC_SELF` 1), its
first thread (`PA_THREAD_SELF` 2), its job (`PA_JOB_DEFAULT` 3) and its
root VMAR (`PA_VMAR_ROOT` 4), as in Fuchsia's processargs, followed by
any handles the parent passed. `init` also gets the root resource
(`PA_ROOT_RESOURCE` 0x20) and the kernel's input channel
(`PA_KERNEL_INPUT` 0x21); these two tags are Rustux's own.
`rx_startup()` in `rx.h` reads it. The kernel launcher's stack is 64 KiB
ending at `0x7fff_ffff_f000`, and RSP starts 8 below a 16-byte boundary,
as if `_start` had been called; pass a stack pointer aligned the same way
to `process_start` and `thread_start`.

#### PROCESS_CREATE (0x01)

`process_create(rx_handle_t job, const char *name, size_t name_len, uint32_t options, rx_handle_t *proc_out, rx_handle_t *vmar_out)`

Needs `MANAGE_PROCESS` on the job. `options` must be 0. Returns handles to
the new, empty process and to its root VMAR, through which the creator
maps code and a stack into it. `ERR_BAD_STATE` if the job was killed.

#### PROCESS_START (0x02)

`process_start(rx_handle_t proc, rx_handle_t thread, uint64_t entry, uint64_t stack, rx_handle_t arg1, uint64_t arg2)`

Runs `thread` (from `thread_create` on this process; otherwise
`ERR_ACCESS_DENIED`) at `entry` with RSP = `stack`. `arg1` (0 for none) is
**moved** into the new process, also on failure; the thread finds its new
value in RDI and `arg2` in RSI. Needs `WRITE` on both handles; `arg1` needs
`TRANSFER`. `ERR_BAD_STATE` if the process was already started or has
ended.

#### THREAD_CREATE (0x08)

`thread_create(rx_handle_t proc, const char *name, size_t name_len, uint32_t options, rx_handle_t *out)`

Needs `MANAGE_THREAD` on the process. The thread does not run until
`process_start` or `thread_start`.

#### THREAD_START (0x04)

`thread_start(rx_handle_t thread, uint64_t entry, uint64_t stack, uint64_t arg1, uint64_t arg2)`

Starts another thread in a process that is already running (RDI = `arg1`,
RSI = `arg2`). Needs `MANAGE_THREAD`. `ERR_BAD_STATE` if the thread was
started before.

#### THREAD_EXIT (0x05)

`thread_exit()` — does not return. If it was the process's last thread,
the process ends with return code 0.

#### PROCESS_EXIT (0x06)

`process_exit(int64_t return_code)` — does not return. Other threads of
the process are killed. The return code can be read with
`object_get_info`.

#### TASK_KILL (0x09)

`task_kill(rx_handle_t task)` — a job (all its processes and child jobs;
no new processes can be created in it), a process (return code `-1024`,
`ZX_TASK_RETCODE_SYSCALL_KILL`) or a thread. Needs `DESTROY`. Killing a
task that has already ended does nothing. A process killed by a CPU
exception ends with `-1025`.

#### OBJECT_GET_INFO (0x0A)

`object_get_info(rx_handle_t h, uint32_t topic, void *buf, size_t size, size_t *actual, size_t *avail)`

Only topic `ZX_INFO_PROCESS` (3) so far; needs `INSPECT`. Writes

```c
typedef struct {
    int64_t return_code;
    int64_t start_time;   // monotonic ns
    uint32_t flags;       // 1 = STARTED, 2 = EXITED
    uint32_t padding;
} rx_info_process_t;
```

`actual` and `avail` (may be NULL) are set to 1.

#### SPAWN (0x03) / SPAWN_PROCESS (0x0B)

`spawn(const char *path)` and `spawn_process(const char *path, rx_handle_t *proc_out, const rx_handle_t *handles, const uint32_t *tags, uint32_t count)`

If `handles` is not NULL, `count` (at most 8) handles are **moved** into
the child's bootstrap message after the standard four, each with its tag
(use `PA_USER0` = 0xF0 and up for your own); they need `TRANSFER` and leave
the caller even on failure. `init` uses this to give the keyboard driver
the root resource and a channel, and the shell its input channel.

Kernel shortcut used by `init` and the shell until a userspace loader
exists: loads `/bin/<name>` from the ramdisk into a new process in the
caller's job and starts it with a bootstrap channel. Returns the new
thread's id. `spawn_process` also returns a process handle, which the shell
waits on for `PROCESS_TERMINATED`. **Frozen**: no new features.

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
| `VMO_CREATE` | 0x10 | Create a virtual memory object | ✅ |
| `VMO_READ` | 0x11 | Copy bytes out of a VMO | ✅ |
| `VMO_WRITE` | 0x12 | Copy bytes into a VMO | ✅ |
| `VMO_CLONE` | 0x13 | Create a snapshot child VMO | ✅ |
| `VMAR_MAP` | 0x14 | Map a VMO into an address space | ✅ |
| `VMAR_UNMAP` | 0x15 | Remove mappings | ✅ |
| `VMAR_PROTECT` | 0x16 | Change mapping permissions | ✅ |
| `VMO_GET_SIZE` | 0x17 | Size of a VMO | ✅ |
| `VMO_REPLACE_AS_EXECUTABLE` | 0x18 | Add `EXECUTE` to a VMO handle | ✅ |

A **VMO** is a piece of memory, a whole number of pages long (at most 1 GiB).
Pages are allocated on first write or when mapped; unwritten memory reads
as zero. Rights: BASIC + READ + WRITE + MAP + GET_PROPERTY + SET_PROPERTY
(no EXECUTE: use `vmo_replace_as_executable` to get a handle whose memory
can be mapped executable).

A **VMAR** is a range of a process's address space. A process's root VMAR
is `[0x1_0000, 0x7fff_ffff_f000)`: all of user space except the first
64 KiB, so null-pointer accesses fault. Programs started by the kernel get
a handle to it in their bootstrap message; `process_create` returns one for
the new process. The program image and stack are ordinary mappings in it.
Mappings without a fixed address go at or above 4 GiB when there is room.
Root VMAR rights: BASIC + READ + WRITE + EXECUTE. A mapping keeps its VMO
alive after the VMO's handles are closed.

#### VMO_CREATE (0x10)

`vmo_create(uint64_t size, uint32_t options, rx_handle_t *out)` — `size`
is rounded up to whole pages; `options` must be 0. `ERR_OUT_OF_RANGE`
above 1 GiB.

#### VMO_READ (0x11) / VMO_WRITE (0x12)

`vmo_read(rx_handle_t h, void *buf, uint64_t offset, size_t len)` (needs `READ`)
`vmo_write(rx_handle_t h, const void *buf, uint64_t offset, size_t len)` (needs `WRITE`)

`ERR_OUT_OF_RANGE` if `offset + len` is past the end of the VMO.

#### VMO_CLONE (0x13)

`vmo_clone(rx_handle_t h, uint32_t options, uint64_t offset, uint64_t size, rx_handle_t *out)`

Like `zx_vmo_create_child` with `ZX_VMO_CHILD_SNAPSHOT` (`options` 0 or 1):
the child starts as a copy of `[offset, offset + size)` (page-aligned
`offset`; bytes past the parent's end are zero) and the two are
independent afterwards. Needs `DUPLICATE` and `READ`. The copy is made at
once (no copy-on-write yet). C: `rx_vmo_create_child()`.

#### VMO_GET_SIZE (0x17)

`vmo_get_size(rx_handle_t h, uint64_t *size)`

#### VMO_REPLACE_AS_EXECUTABLE (0x18)

`vmo_replace_as_executable(rx_handle_t h, rx_handle_t vmex, rx_handle_t *out)`

Replaces the VMO handle by one with the same rights plus `EXECUTE` (the old
handle is closed, also on failure). `vmex` is not checked yet: any process
may do this.

#### VMAR_MAP (0x14)

`vmar_map(rx_handle_t vmar, uint64_t options | (vmo << 32), uint64_t vmar_offset, uint64_t vmo_offset, uint64_t len, uint64_t *mapped_addr)`

The C wrapper `rx_vmar_map()` has the Zircon signature
(`vmar, options, vmar_offset, vmo, vmo_offset, len, *mapped_addr`).

Options (Zircon values): `PERM_READ` 1, `PERM_WRITE` 2, `PERM_EXECUTE` 4,
`SPECIFIC` 16. Without `SPECIFIC` the kernel picks the lowest free address
and `vmar_offset` must be 0; with it the mapping goes at
`vmar base + vmar_offset` (`ERR_ALREADY_EXISTS` if something is there).
`len` is rounded up to pages; `vmo_offset` must be page aligned and the
range must lie inside the VMO (`ERR_OUT_OF_RANGE`). Write or execute
without read is `ERR_INVALID_ARGS` (x86 cannot express it).

The VMO handle needs `MAP`; each permission needs the matching right
(`READ`, `WRITE`, `EXECUTE`) on both the VMO and the VMAR handle. Those
rights also cap what `vmar_protect` can grant later.

#### VMAR_UNMAP (0x15)

`vmar_unmap(rx_handle_t vmar, uint64_t addr, uint64_t len)` — removes
whatever is mapped in the page-aligned range, splitting mappings that cross
its edges. `ERR_NOT_FOUND` if nothing was mapped there, `ERR_INVALID_ARGS`
if the range is not inside the VMAR.

#### VMAR_PROTECT (0x16)

`vmar_protect(rx_handle_t vmar, uint32_t options, uint64_t addr, uint64_t len)`

Sets the permissions (options as for `vmar_map`) of a range that must be
completely mapped (`ERR_NOT_FOUND` otherwise). `ERR_ACCESS_DENIED` if a
mapping was made through handles that lacked the right.

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
| `INSPECT` | 15 | Querying information (`object_get_info`) |
| `ENUMERATE` | 8 | Listing children (planned) |
| `DESTROY` | 9 | `task_kill` |
| `MANAGE_JOB` | 16 | `job_create` on a job |
| `MANAGE_PROCESS` | 17 | `process_create` on a job |
| `MANAGE_THREAD` | 18 | `thread_create`, `thread_start` |
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
| `CHANNEL_CREATE` | 0x20 | Create a channel (two endpoints) | ✅ |
| `CHANNEL_WRITE` | 0x21 | Send bytes and handles | ✅ |
| `CHANNEL_READ` | 0x22 | Receive bytes and handles | ✅ |
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

Same as `object_signal`, applied to the other end of an event pair or a
channel (on a channel only user signals may be changed). Needs
`SIGNAL_PEER`. `ERR_WRONG_TYPE` on any other object,
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

#### CHANNEL_CREATE (0x20)

`channel_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1)`

Creates the two endpoints of a channel. A message written to one endpoint
is read from the other. Rights: BASIC + READ + WRITE + SIGNAL + SIGNAL_PEER.
New endpoints have `CHANNEL_WRITABLE` set. When one endpoint is closed the
other gets `PEER_CLOSED` and loses `CHANNEL_WRITABLE`; messages still queued
on it can be read, and handles in messages queued on the closed endpoint
are closed.

#### CHANNEL_WRITE (0x21)

`channel_write(rx_handle_t h, uint32_t options, const void *bytes, uint32_t num_bytes, const rx_handle_t *handles, uint32_t num_handles)`

Needs `WRITE`. At most 65536 bytes and 64 handles (`ERR_OUT_OF_RANGE`).
Each handle sent needs `TRANSFER`; it leaves the caller's handle table and
arrives in the reader's table with the same rights. As in Zircon, the
listed handles are **always consumed**, even when the write fails.
Sending the channel's own handle or its peer gives `ERR_NOT_SUPPORTED`.
Errors: `ERR_PEER_CLOSED` (other end gone), `ERR_SHOULD_WAIT` (1024
messages already queued), `ERR_BAD_HANDLE` (a listed handle is invalid).

#### CHANNEL_READ (0x22)

`channel_read(rx_handle_t h, uint32_t options, void *bytes, rx_handle_t *handles, uint64_t capacity, uint32_t actual[2])`

Needs `READ`. `capacity` packs the buffer sizes: `num_bytes | (num_handles << 32)`.
`actual` (may be NULL) receives the message's byte and handle counts.
The C wrapper `rx_channel_read()` in `rx.h` has the Zircon signature
(`num_bytes, num_handles, *actual_bytes, *actual_handles`) and does the
packing.

- `ERR_SHOULD_WAIT`: nothing queued (wait for `CHANNEL_READABLE`)
- `ERR_PEER_CLOSED`: nothing queued and the other end is gone
- `ERR_BUFFER_TOO_SMALL`: the next message does not fit; `actual` is
  filled in and the message stays queued

`CHANNEL_READABLE` is set while at least one message is queued.

---

### Jobs & Handles (0x30-0x3F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `JOB_CREATE` | 0x30 | Create a child job | ✅ |
| `HANDLE_DUPLICATE` | 0x31 | Duplicate a handle | ✅ |
| `HANDLE_REPLACE` | 0x32 | Replace a handle with one with fewer rights | ✅ |

#### JOB_CREATE (0x30)

`job_create(rx_handle_t parent, uint32_t options, rx_handle_t *out)` —
needs `MANAGE_JOB` on the parent. `ERR_BAD_STATE` if the parent was killed.

#### HANDLE_REPLACE (0x32)

`handle_replace(rx_handle_t h, uint32_t rights, rx_handle_t *out)` — like
`handle_duplicate`, but the old handle is closed (also on failure) and no
`DUPLICATE` right is needed. (0x32 was a planned `handle_transfer`; handles
move between processes over channels instead.)

#### HANDLE_DUPLICATE (0x31)

`handle_duplicate(rx_handle_t h, uint32_t rights, rx_handle_t *out)`

Creates a second handle to the same object. Needs `DUPLICATE`. `rights` is
`SAME_RIGHTS` or a subset of the original rights; asking for a right the
original lacks gives `ERR_INVALID_ARGS`. Use this to hand out a weaker
handle, e.g. one that can only wait.

---

### Drivers (0x80-0x8F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `INTERRUPT_CREATE` | 0x80 | Bind a legacy ISA IRQ to a handle | ✅ (IRQ 1 only) |
| `INTERRUPT_WAIT` | 0x81 | Block until the IRQ fires | ✅ |
| `INTERRUPT_ACK` | 0x82 | Re-arm the IRQ | ✅ |
| `INTERRUPT_DESTROY` | 0x83 | Cancel waits on the interrupt | ✅ |
| `IOPORTS_REQUEST` | 0x84 | Let the calling process use I/O ports | ✅ |
| `IOPORTS_RELEASE` | 0x85 | Give the ports back | ✅ |

Drivers run as ordinary processes. Both calls that reach hardware need the
**root resource** (object type 15, rights DUPLICATE + TRANSFER + WRITE +
INSPECT), which the kernel gives only to `init`; `init` passes copies to
the drivers it starts. Without it, ports stay closed: a user `in`/`out` to a
port the process was not granted raises #GP and the process is killed
(exit code -1025). The first driver is the PS/2 keyboard (`/bin/ps2kbd`,
source `userspace/c-progs/ps2kbd.c`).

#### INTERRUPT_CREATE (0x80)

`interrupt_create(rx_handle_t resource, uint32_t irq, uint32_t options, rx_handle_t *out)`

Binds legacy ISA `irq` (only 1, the PS/2 keyboard, is wired up so far;
others give `ERR_NOT_SUPPORTED`) to a new interrupt object (type 9).
`ERR_ALREADY_EXISTS` if another interrupt object holds the IRQ. While
bound, the kernel's interrupt handler only records the IRQ and wakes the
waiting thread; the driver reads the device itself. Closing the last handle
unbinds it (for example when the driver is killed).

#### INTERRUPT_WAIT (0x81)

`interrupt_wait(rx_handle_t h, uint64_t *timestamp)` — needs `WAIT`.
Returns at once if the IRQ fired since the last wait, otherwise blocks
until it does; `timestamp` (may be NULL) receives the monotonic time of
the firing. One thread at a time (`ERR_BAD_STATE` for a second).
`ERR_CANCELED` after `interrupt_destroy` or when the thread is killed.

#### INTERRUPT_ACK (0x82) / INTERRUPT_DESTROY (0x83)

`interrupt_ack(rx_handle_t h)` and `interrupt_destroy(rx_handle_t h)` —
need `WRITE`. Edge-triggered ISA IRQs are never masked, so `ack` has
nothing to re-enable yet. `destroy` makes the current and later waits
return `ERR_CANCELED`.

#### IOPORTS_REQUEST (0x84) / IOPORTS_RELEASE (0x85)

`ioports_request(rx_handle_t resource, uint16_t port, uint32_t len)` and
`ioports_release(...)` — grant or take back `in`/`out` on ports
`[port, port + len)` for the **calling process** (all its threads). The
kernel keeps a 64 Ki-bit permission bitmap per process and loads it into
the TSS when one of its threads runs.

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

All the object syscalls are implemented (checklist step 3). Test programs
in the ramdisk, run from the shell:

| Program | Covers | Checks |
|---------|--------|--------|
| `objtest` | handles, rights, events, event pairs, waiting, timers, channels, VMOs, VMARs | 94 |
| `ipctest` | processes, threads, jobs, `task_kill`, two processes exchanging a message and a handle over a channel | 31 |
| `spawntest` | starts `/bin/true` 300 times; process clean-up | 1 |
| `drvtest` | driver calls refused without the root resource; a direct port read kills the program (exit code -1025 expected) | 5 |
| `smptest` | threads on several CPUs: speed-up, SSE registers across migrations, TLB shootdown, cross-CPU wake-ups | 6 |

Sources are in `userspace/c-progs/`; `rx.h` there has C wrappers for every
call above with Zircon-style signatures.

### Debugging (tests only)

`debug_cpu()` (0x51) returns the index of the CPU running the caller;
`debug_cpu_count()` (0x52) the number of CPUs running the scheduler. Used
by `smptest`.

### Keyboard input

Since step 4 the kernel does not hand out keystrokes. The PS/2 keyboard is
driven by `/bin/ps2kbd`; the USB keyboard driver is still in the kernel,
polled on the timer tick, and sends characters over the kernel input
channel. `init` forwards both to the shell over the shell's channel
(`PA_USER0`). `read` on fd 0 returns `ERR_NOT_SUPPORTED`. The shell's
`restart-kbd` command asks `init` to kill the driver, which `init` then
restarts.

### Unix-style calls (frozen)

`write` (0x60), `read` (0x61), `open` (0x62), `close` (0x63), `lseek`
(0x64), `getpid` (0x70), `getppid` (0x71), `yield` (0x72), `spawn` (0x03)
and `spawn_process` (0x0B) keep working for the existing programs (`init`,
the shell) but get no new features. A userspace library will provide POSIX
file descriptors on top of handles and channels, like Fuchsia's `fdio`.

---

## Example: start a child process and talk to it

Abridged from `userspace/c-progs/ipctest.c`, which runs a copy of its own
image in the child:

```c
#include "rx.h"

void _start(rx_handle_t bootstrap) {
    rx_startup(bootstrap);               // rx_job_default, rx_vmar_root, ...

    rx_handle_t proc, vmar, thread, c0, c1;
    rx_process_create(rx_job_default, "child", 5, 0, &proc, &vmar);
    // ... map code (a VMO made executable with rx_vmo_replace_as_executable)
    //     and a stack into the child through `vmar` ...
    rx_thread_create(proc, "main", 4, 0, &thread);
    rx_channel_create(0, &c0, &c1);
    rx_process_start(proc, thread, entry, stack_top - 8, c1, 0);  // c1 moves

    rx_object_wait_one(c0, RX_CHANNEL_READABLE, RX_TIME_INFINITE, 0);  // blocks
    char buf[64];
    rx_handle_t handles[4];
    uint32_t nb, nh;
    rx_channel_read(c0, 0, buf, handles, sizeof buf, 4, &nb, &nh);

    rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
    rx_info_process_t info;
    rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_exit(info.return_code);
}
```

---

## References

- **Zircon Syscalls:** https://fuchsia.dev/fuchsia-src/reference/syscalls
- **Linux Syscalls:** https://man7.org/linux/man-pages/man2/syscalls.2.html
- **AMD64 ABI:** https://gitlab.com/x86-psABIs/x86-64-ABI

---

*Last Updated: 2025-01-18*
*Author: Rustux Kernel Team*
*License: MIT*

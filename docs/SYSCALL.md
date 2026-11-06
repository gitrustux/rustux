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
The C header `<rustux/syscalls.h>` (`userspace/libc/include/rustux/`) defines them as `RX_ERR_*`.

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
| `ERR_NO_RESOURCES` | 20 | A limited resource is used up (all 64 MSI vectors taken) |

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
| ~~`SPAWN`~~ | 0x03 | Removed in step 8 (programs are loaded by the C library) | — |
| `THREAD_START` | 0x04 | Start another thread in a running process | ✅ |
| `THREAD_EXIT` | 0x05 | Exit the current thread | ✅ |
| `PROCESS_EXIT` | 0x06 | Exit the current process | ✅ |
| `HANDLE_CLOSE` | 0x07 | Close a handle | ✅ |
| `THREAD_CREATE` | 0x08 | Create a thread object in a process | ✅ |
| `TASK_KILL` | 0x09 | Kill a job, process or thread | ✅ |
| `OBJECT_GET_INFO` | 0x0A | Query an object (see topics below) | ✅ |
| ~~`SPAWN_PROCESS`~~ | 0x0B | Removed in step 8 | — |
| `OBJECT_GET_CHILD` | 0x0C | A job's child job or process, or a process's thread, by koid | ✅ |
| `OBJECT_GET_PROPERTY` | 0x0D | Read a property (only `NAME`) | ✅ |
| `OBJECT_SET_PROPERTY` | 0x0E | Set a property (only a thread's `REGISTER_FS`) | ✅ |

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
RSI and every other register zero. Every program (`init`, started by the
kernel, and all others, started by the C library's loader) gets a
**bootstrap channel** in RDI holding one message in Fuchsia's processargs
format (`<rustux/processargs.h>`):

| Offset | Contents |
|--------|----------|
| 0 | header: `protocol` 0x4150585d, `version` 0x00010000, `handle_info_off`, `args_off`, `args_num`, `environ_off`, `environ_num`, `names_off`, `names_num` (9 × u32) |
| `handle_info_off` | one u32 info word per handle: type in bits 0-7, argument in bits 16-31 |
| `args_off` | `args_num` NUL-terminated arguments (argv[0] first) |
| `environ_off` | `environ_num` NUL-terminated `NAME=value` strings |

The handles travel with the message in the same order as the info words:
the process itself (`PA_PROC_SELF` 1), its first thread
(`PA_THREAD_SELF` 2), its job (`PA_JOB_DEFAULT` 3) and its root VMAR
(`PA_VMAR_ROOT` 4), then any the parent passed. Other types:
`PA_VMO_BOOTFS` 0x1B (the boot image, `init` only), `PA_NS_DIR` 0x20 (the
namespace root: a connection to the filesystem service, `docs/FS.md`),
`PA_FD` 0x30 (a file descriptor; the argument is the fd number in bits
0-11 and its kind in bits 12-15: 1 console, 2 debug log, 3 file, 4 pipe),
`PA_ROOT_RESOURCE` 0x3F (Fuchsia's `PA_RESOURCE`; `init` and drivers),
`PA_KERNEL_INPUT` 0xE0 (the kernel's input channel, `init` only; Rustux's
own), `PA_USER0` 0xF0 and up for a parent's own handles. (Before step 7 the
message was only a u32 tag per handle, and the resource and input tags were
0x20 and 0x21.) The C library reads the message before `main` runs
(`userspace/libc/src/start.c`) and builds it when it starts a program
(`spawn.c`). The kernel's launcher for init gives a 64 KiB stack (the C
library 256 KiB), ending at `0x7fff_ffff_f000`; RSP starts 8 below a
16-byte boundary,
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

Topics (Zircon numbers; structs in `<rustux/syscalls.h>`):

| Topic | On | Right | Writes |
|-------|----|-------|--------|
| `HANDLE_BASIC` 2 | any | — | `{ u64 koid; u32 rights; u32 type; u64 related_koid; }` (koid 0 for objects other than tasks; related: a process's job, a thread's process, a job's parent) |
| `PROCESS` 3 | process | `INSPECT` | `{ i64 return_code; i64 start_time; u32 flags (1 STARTED, 2 EXITED); u32 pad; }` |
| `PROCESS_THREADS` 4 | process | `ENUMERATE` | koids (u64) of its running threads |
| `JOB_CHILDREN` 6 | job | `ENUMERATE` | koids of its child jobs |
| `JOB_PROCESSES` 7 | job | `ENUMERATE` | koids of its processes that have not exited |
| `TASK_STATS` 11 | process | `INSPECT` | `{ u64 mem_mapped_bytes; u64 mem_private_bytes; u64 mem_shared_bytes; u64 scaled_shared; }`: memory committed in the VMOs it maps, each counted once, split into private (VMOs only this process holds) and shared (VMOs also held elsewhere, such as the boot image); `scaled_shared` is 0 |
| `TASK_RUNTIME` 25 | process, thread | `INSPECT` | `{ i64 cpu_time; i64 queue_time; i64 page_fault_time; i64 lock_contention_time; }` ns (only `cpu_time` is measured) |
| `INTERRUPT_MSI` 0x1000 | MSI interrupt | `INSPECT` | `{ u64 address; u32 data; u32 vector; }`: what to program into the device (`ERR_BAD_STATE` for an ISA interrupt) |
| `BTI` 0x1001 | BTI | `INSPECT` | `{ u64 minimum_contiguity; u64 aspace_size; u64 pmo_count; u64 quarantine_count; }`: 4096, all addresses, ranges pinned now, 0 |

For the koid lists, as many koids as fit in `size` are written; `actual`
gets how many, `avail` how many exist. For the others both are 1. A buffer
smaller than the struct gives `ERR_BUFFER_TOO_SMALL`.

CPU time is charged when a thread leaves the CPU and on every timer tick
while it keeps it.

#### OBJECT_GET_CHILD (0x0C)

`object_get_child(rx_handle_t h, uint64_t koid, uint32_t rights, rx_handle_t *out)`
— needs `ENUMERATE`. From a job: its child job or process with that koid;
from a process: its thread. `rights` is `RX_RIGHT_SAME_RIGHTS` (the
default rights of that object type) or a subset of them. `ERR_NOT_FOUND`
if no such child. `ps` and `kill` walk the job tree with it.

#### OBJECT_GET_PROPERTY (0x0D)

`object_get_property(rx_handle_t h, uint32_t property, void *buf, size_t len)`
— needs `GET_PROPERTY`. Only `PROP_NAME` (3): a process's name (a thread
gives its process's), NUL-terminated, at most 31 bytes.

#### OBJECT_SET_PROPERTY (0x0E)

`object_set_property(rx_handle_t h, uint32_t property, const void *value, size_t len)`
— needs `SET_PROPERTY`. Only `PROP_REGISTER_FS` (4) on a thread, as in
Zircon: `value` points to a `uint64_t`, the thread's FS base (`len` 8). The
C library points it at the thread's control block, below which the
thread's copy of the program's thread-local storage lies (x86-64 TLS
variant II, `%fs:0` holding the block's own address). It must be a user
address (below `0x7fff_ffff_f000`), else `ERR_INVALID_ARGS`. The kernel
keeps it with the thread and loads it (`IA32_FS_BASE`) whenever the thread
is scheduled; for the calling thread, at once. Threads start with 0.

#### SPAWN (0x03) / SPAWN_PROCESS (0x0B) — removed

Until step 8 the kernel loaded `/bin/<name>` from its ramdisk for these
calls. The kernel now holds no files: it starts only `init`, from the boot
image built into it (`boot_image_find` in `src/process/launch.rs`, which
looks up `bin/init`), and hands `init` the whole image as a VMO. Programs
start others with `rx_spawn()` (`<rustux/process.h>`): the C library reads
the ELF file through the filesystem service, loads it with
`process_create`, `vmo_write`, `vmo_replace_as_executable` and `vmar_map`,
and starts it with `process_start` and a processargs message.

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
| `VMO_CREATE_PHYSICAL` | 0x19 | A VMO over device memory (drivers) | ✅ |
| `VMO_SET_CACHE_POLICY` | 0x1A | Cached or uncached mappings | ✅ |
| `VMO_CREATE_CONTIGUOUS` | 0x1B | Physically contiguous memory for DMA | ✅ |

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

#### VMO_CREATE_PHYSICAL (0x19)

`vmo_create_physical(rx_handle_t resource, uint64_t paddr, uint64_t size, rx_handle_t *out)`
— needs the root resource. A VMO over `[paddr, paddr + size)` (page
aligned): device registers, a frame buffer, PCI configuration space. The
range must not touch RAM, firmware memory, ACPI tables or the kernel
(everything in the UEFI memory map except memory-mapped I/O and reserved
ranges): `ERR_ACCESS_DENIED`. Its pages are never freed or committed;
`vmo_read`/`vmo_write` give `ERR_NOT_SUPPORTED` (map it); it cannot be
cloned or pinned. Mapped uncached (`UNCACHED_DEVICE`) unless changed.

#### VMO_SET_CACHE_POLICY (0x1A)

`vmo_set_cache_policy(rx_handle_t vmo, uint32_t policy)` — needs
`SET_PROPERTY`. `CACHED` 0, `UNCACHED` 1, `UNCACHED_DEVICE` 2 (both
uncached: PCD and PWT in the page tables), `WRITE_COMBINING` 3 gives
`ERR_NOT_SUPPORTED` (needs the PAT). Applies to mappings made afterwards.

#### VMO_CREATE_CONTIGUOUS (0x1B)

`vmo_create_contiguous(rx_handle_t bti, uint64_t size, uint32_t options, rx_handle_t *out)`
— needs a BTI with `MAP`. Zeroed memory in one physically contiguous run.
`options`: bits 0-7 the log2 of the alignment (0: a page), `0x100`
(`RX_VMO_CONTIGUOUS_LOW`) keeps it below 4 GiB. `ERR_NO_MEMORY` when no
run that long is free. Its device address comes from `bti_pin`.

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
| `OBJECT_WAIT_MANY` | 0x27 | Wait on up to 1024 objects | ✅ |
| `OBJECT_SIGNAL_PEER` | 0x28 | Set/clear signals on the peer of a pair | ✅ |
| `OBJECT_GET_TYPE` | 0x29 | Object type of a handle | ✅ |
| `SOCKET_CREATE` | 0x2A | Create a socket (byte stream, two endpoints) | ✅ |
| `SOCKET_WRITE` | 0x2B | Write bytes | ✅ |
| `SOCKET_READ` | 0x2C | Read bytes | ✅ |

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

Waits until any item has one of its `waitfor` signals. `count` is 1–1024
(the filesystem service waits on every open connection at once).
Every item's `pending` is filled in on return (including on `ERR_TIMED_OUT`).

#### OBJECT_GET_TYPE (0x29)

`object_get_type(rx_handle_t h)` returns the object type: 5 = event,
16 = event pair, 22 = timer (Zircon values). Needs no rights.

#### SOCKET_CREATE (0x2A) / SOCKET_WRITE (0x2B) / SOCKET_READ (0x2C)

`socket_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1)`
`socket_write(rx_handle_t h, uint32_t options, const void *buf, size_t len, size_t *actual)`
`socket_read(rx_handle_t h, uint32_t options, void *buf, size_t len, size_t *actual)`

A socket is a pair of endpoints carrying a byte stream each way (Zircon's
stream sockets); pipes are sockets. Up to 64 KiB wait in each direction:
`socket_write` writes as much as fits and reports it in `*actual`, or
returns `ERR_SHOULD_WAIT` when nothing fits; `socket_read` returns up to
`len` bytes, `ERR_SHOULD_WAIT` when none are there yet, `ERR_PEER_CLOSED`
when none will come. Writing to a closed peer gives `ERR_PEER_CLOSED`.
Signals: `READABLE` (bit 0), `WRITABLE` (bit 1, room in the peer's
buffer), `PEER_CLOSED` (bit 2), user signals (also through
`object_signal_peer`). Type 14; rights BASIC + READ + WRITE + SIGNAL +
SIGNAL_PEER. `options` must be 0.

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

### Drivers and resources (0x80-0x8F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `INTERRUPT_CREATE` | 0x80 | Bind a legacy ISA IRQ to a handle | ✅ (IRQ 1 only) |
| `INTERRUPT_WAIT` | 0x81 | Block until the IRQ fires | ✅ |
| `INTERRUPT_ACK` | 0x82 | Re-arm the IRQ | ✅ |
| `INTERRUPT_DESTROY` | 0x83 | Cancel waits on the interrupt | ✅ |
| `IOPORTS_REQUEST` | 0x84 | Let the calling process use I/O ports | ✅ |
| `IOPORTS_RELEASE` | 0x85 | Give the ports back | ✅ |
| `RESOURCE_CREATE` | 0x86 | Make a narrower resource from the root resource | ✅ |
| `BTI_CREATE` | 0x87 | A bus transaction initiator (DMA for one device) | ✅ |
| `BTI_PIN` | 0x88 | Pin VMO pages, get their device addresses | ✅ |
| `PMT_UNPIN` | 0x89 | Release pinned pages | ✅ |

Drivers run as ordinary processes. Both calls that reach hardware need the
**root resource** (object type 15, rights DUPLICATE + TRANSFER + WRITE +
INSPECT), which the kernel gives only to `init`; `init` passes copies to
the drivers it starts. Without it, ports stay closed: a user `in`/`out` to a
port the process was not granted raises #GP and the process is killed
(exit code -1025). The first driver is the PS/2 keyboard (`/bin/ps2kbd`,
source `userspace/c-progs/ps2kbd.c`).

From step 12 the device manager (`devmgr`) holds the root resource and
gives each PCI driver only what its device needs: physical VMOs for its
BARs, an IOPORT resource for an I/O BAR, MSI interrupt objects and a BTI
(`docs/DRIVERS.md`).

#### RESOURCE_CREATE (0x86)

`resource_create(rx_handle_t parent, uint32_t kind, rx_handle_t *out, uint64_t base, uint64_t size)` —
`parent` must be the root resource. Kinds: 1, **power** (restart and
power off with `system_powerctl`; `init` makes one for the shell, which
passes it on to the programs it runs, `PA_POWER_RESOURCE` 0xE1), and 2,
**I/O ports** `[base, base + size)` (at most 65536), which
`ioports_request` accepts for ports in that range (devmgr gives one to a
driver whose device has an I/O BAR). `base` and `size` are ignored for
power.

#### INTERRUPT_CREATE (0x80)

`interrupt_create(rx_handle_t resource, uint32_t irq, uint32_t options, rx_handle_t *out)`

With `options` 0, binds legacy ISA `irq` (only 1, the PS/2 keyboard, is
wired up so far; others give `ERR_NOT_SUPPORTED`) to a new interrupt
object (type 9). With `options` `0x100` (`RX_INTERRUPT_MSI`, `irq` 0) it
takes a free **MSI vector** (64 of them, 0x40-0x7F; `ERR_NO_RESOURCES`
when all are taken): `object_get_info(INTERRUPT_MSI)` tells the address
and data to write into the device's MSI capability or MSI-X table (a
fixed, edge-triggered message to the boot CPU). Closing the last handle
frees the vector.
`ERR_ALREADY_EXISTS` if another interrupt object holds the IRQ. While
bound, the kernel's interrupt handler only records the IRQ and wakes the
waiting thread; the driver reads the device itself. Closing the last handle
unbinds it (for example when the driver is killed).

#### INTERRUPT_WAIT (0x81)

`interrupt_wait(rx_handle_t h, uint64_t *timestamp, uint64_t deadline)` — needs `WAIT`.
Returns at once if the IRQ fired since the last wait, otherwise blocks
until it does; `timestamp` (may be NULL) receives the monotonic time of
the firing. One thread at a time (`ERR_BAD_STATE` for a second).
`ERR_CANCELED` after `interrupt_destroy` or when the thread is killed.
`deadline` (step 13): monotonic nanoseconds after which the wait fails
with `ERR_TIMED_OUT`; 0 waits for ever (Zircon's call has no deadline;
the C library's `rx_interrupt_wait` passes 0, `rx_interrupt_wait_until`
a deadline). Disk drivers use it so a lost interrupt cannot hang them.

#### INTERRUPT_ACK (0x82) / INTERRUPT_DESTROY (0x83)

`interrupt_ack(rx_handle_t h)` and `interrupt_destroy(rx_handle_t h)` —
need `WRITE`. ISA IRQs and MSIs are edge-triggered and never masked, so
`ack` has nothing to re-enable yet. `destroy` makes the current and later waits
return `ERR_CANCELED`.

#### IOPORTS_REQUEST (0x84) / IOPORTS_RELEASE (0x85)

`ioports_request(rx_handle_t resource, uint16_t port, uint32_t len)` and
`ioports_release(...)` — grant or take back `in`/`out` on ports
`[port, port + len)` for the **calling process** (all its threads). The
kernel keeps a 64 Ki-bit permission bitmap per process and loads it into
the TSS when one of its threads runs. `resource` is the root resource or
an I/O port resource covering the range.

#### BTI_CREATE (0x87)

`bti_create(rx_handle_t resource, uint32_t options, uint64_t bti_id, rx_handle_t *out)`
— needs the root resource; `options` 0. A BTI (object type 24, rights
BASIC + READ + WRITE + MAP) stands for one device's DMA (devmgr uses
bus:device.function as the id). Without an IOMMU the device sees
physical addresses.

#### BTI_PIN (0x88)

`bti_pin(rx_handle_t bti, uint32_t options, rx_handle_t vmo, uint64_t offset, uint64_t size, uint64_t *addrs, rx_handle_t *pmt)`
— the kernel takes `bti | options << 32` in the first register (six
registers in all). Needs `MAP` on both handles, and `READ`/`WRITE` on the
VMO for `RX_BTI_PERM_READ` (1) / `RX_BTI_PERM_WRITE` (2), one of which is
required. Commits and pins `[offset, offset + size)` (page multiples) and
writes each page's device address to `addrs` (`size / 4096` entries), or
just the first with `RX_BTI_CONTIGUOUS` (8, contiguous VMOs only). The
pages stay put until `pmt_unpin` or the PMT handle (object type 26) is
closed. Physical VMOs cannot be pinned (`ERR_NOT_SUPPORTED`).

#### PMT_UNPIN (0x89)

`pmt_unpin(rx_handle_t pmt)` — releases the pages and closes the handle.

---

### Time (0x40-0x4F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `CLOCK_GET` | 0x40 | Get current time | ✅ |
| `TIMER_CREATE` | 0x41 | Create a timer object | ✅ |
| `TIMER_SET` | 0x42 | Arm a timer | ✅ |
| `TIMER_CANCEL` | 0x43 | Disarm a timer | ✅ |
| `CLOCK_SET_UTC` | 0x44 | Set the wall clock | ✅ |

#### CLOCK_GET (0x40)

`clock_get(uint32_t clock_id)` returns nanoseconds. Clock 0 is monotonic
(from the calibrated TSC, counting from power-on); it is the clock used for
all deadlines. Clock 1 is UTC: ns since 1970, set at boot from the CMOS
real-time clock (read as UTC; the century from the FADT's century register
when there is one).

#### CLOCK_SET_UTC (0x44)

`clock_set_utc(rx_handle_t resource, int64_t utc_ns)` — needs the root
resource. `init` uses it when `/etc/adjtime` says the hardware clock keeps
local time (as Windows does), correcting by the offset of the zone in
`/etc/timezone`. The real-time clock itself is never written.

### System (0x90-0x9F)

| Syscall | Number | Description | Status |
|---------|--------|-------------|--------|
| `SYSTEM_GET_VERSION` | 0x90 | Kernel version string | ✅ |
| `SYSTEM_GET_STATS` | 0x91 | Memory, CPUs, uptime | ✅ |
| `SYSTEM_POWERCTL` | 0x92 | Restart or power off | ✅ |
| `ACPI_TABLE_READ` | 0x93 | Copy an ACPI table (drivers) | ✅ |

`system_get_version(char *buf, size_t len)` writes
`Rustux <version> (git <hash>, built <date>) x86_64` with a NUL and
returns its length (`ERR_BUFFER_TOO_SMALL` if it does not fit).

`system_get_stats(rx_system_stats_t *s)` writes `{ u64 total_bytes;
u64 free_bytes; u64 boot_time; u64 uptime; u64 cpus; u64 idle_time;
u64 threads; u64 console_cols; u64 console_rows; }`: memory the kernel
manages and how much is free, the monotonic time the kernel started, ns
since then, CPUs online, ns all CPUs spent halted, threads running, and
the size of the screen console in characters (0 without a screen; step
10). Needs no handle. The second argument is the buffer's size: programs
built before step 10 pass 56 bytes and get the first seven fields.

`system_powerctl(rx_handle_t resource, uint32_t cmd)` — needs the root or
the power resource. `cmd` 1 restarts: the FADT reset register, then the
8042 keyboard controller, then port 0xCF9, then a triple fault. `cmd` 2
powers off through ACPI S5 (the `_S5_` values from the DSDT written with
SLP_EN to the PM1 control registers); it returns `ERR_NOT_SUPPORTED` only
if that did not work. Nothing is synced first (there are no disks yet).

`acpi_table_read(rx_handle_t resource, uint32_t signature, uint32_t index, void *buf, size_t len)`
— needs the root resource. Copies the `index`th ACPI table with this
signature (four ASCII bytes, little endian: `MCFG` is 0x4746434D; the
C wrapper takes the string) into `buf` and returns the table's length;
`len` 0 just returns the length. The DSDT is found through the FADT.
`ERR_NOT_FOUND` if there is none. devmgr reads MCFG for PCI Express
configuration space.

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
in `/bin`, run from the shell:

| Program | Covers | Checks |
|---------|--------|--------|
| `objtest` | handles, rights, events, event pairs, waiting, timers, channels, VMOs, VMARs | 94 |
| `ipctest` | processes, threads, jobs, `task_kill`, two processes exchanging a message and a handle over a channel | 31 |
| `spawntest` | starts `/bin/true` 300 times; process clean-up | 1 |
| `drvtest` | driver calls refused without the root resource; a direct port read kills the program (exit code -1025 expected) | 5 |
| `smptest` | threads on several CPUs: speed-up, SSE registers across migrations, TLB shootdown, cross-CPU wake-ups | 6 |

`libctest` (50 checks) covers the C library, `fstest` (64) the filesystem
service, file descriptors and pipes, `systest` (39) process and system
information, clocks and time zones. Sources are in
`userspace/c-progs/`; `<rustux/syscalls.h>` has C wrappers for every call
above with Zircon-style signatures (`docs/USERSPACE.md`).

### Debugging

`debug_cpu()` (0x51) returns the index of the CPU running the caller;
`debug_cpu_count()` (0x52) the number of CPUs running the scheduler. Used
by `smptest`.

`debuglog_create(rx_handle_t resource, uint32_t options, rx_handle_t *out)`
(0x53) returns a debug log handle (type `LOG` 12). With `options` 0 it
needs the root resource and the handle writes (rights BASIC + READ + WRITE
+ SIGNAL): `debuglog_write(rx_handle_t h, uint32_t options, const void
*buf, size_t len)` (0x54, needs `WRITE`, `len` at most 4096) prints the
bytes on the kernel console and the debug port. `init`, the console
service, prints everything the programs write through it. With
`RX_LOG_FLAG_READABLE` (0x40000000) it needs no resource and the handle
only reads the **kernel log**: `debuglog_read(rx_handle_t h, uint32_t
options, void *buf, size_t len)` (0x55, needs `READ`) returns the next
bytes not read through that handle (the kernel keeps the last 64 KiB of
what it printed for debugging) and `ERR_SHOULD_WAIT` when there are none
(`dmesg`).

### Console and keyboard input

Since step 4 the kernel does not hand out keystrokes. The PS/2 keyboard is
driven by `/bin/ps2kbd`; the USB keyboard driver is still in the kernel,
polled on the timer tick. Since step 10 both send **key events**, not
characters: 4 bytes each, `{ u8 usage; u8 modifiers; u8 flags; u8 0 }`
with the HID usage (page 7, the same for every layout), the HID modifier
byte (bit 0 left Ctrl, 1 left Shift, 2 left Alt, 3 left GUI, 4-7 the right
ones), and flags bit 0 = pressed (else released), bit 1 = the device
repeats held keys itself (PS/2 typematic). `ps2kbd` writes them to `init`
over a channel; the kernel's USB driver over the kernel input channel
(`PA_KERNEL_INPUT`). Modifier keys come as their own events (usages
0xE0-0xE7) too.

Since step 7 `init` is the **console service**. It turns key events into
bytes with the keyboard layout (built-in US; others in
`/usr/share/keymaps/NAME`, chosen by `/etc/keymap`, reloaded on request
from the shell's `loadkeys`), as xterm does: UTF-8 characters, Ctrl codes,
Alt as an ESC prefix, `ESC [ A`-`D` for the arrows, `ESC [ n ~` for
Insert/Delete/PgUp/PgDn/F5-F12, `ESC O P`-`S` for F1-F4, xterm modifier
parameters (`ESC [ 1 ; 5 C` for Ctrl+Right). It repeats held USB keys
itself (after 500 ms, every 33 ms). Shift+PgUp/PgDn scroll the screen
console's history (500 lines).

In **cooked** mode (the default) `init` edits the typed line (echo,
Backspace, Ctrl+U, Ctrl+W), sends it to the program reading the console on
Enter, and prints what programs write. In **raw** mode the bytes of each
key go to the program as they are typed. A program's fds 0-2 are handles
to the console channel; the protocol (`<rustux/console.h>`) is a 32-bit op
per message: `WRITE` (1, program → console), `INPUT` (2, console →
program; a line, or a key's bytes in raw mode; no bytes = end of input from
Ctrl+D) and `SET_MODE` (3, program → console: `u32 lflag, u32 iflag` with
the termios bits `ICANON`, `ECHO`, `ISIG` and `ICRNL`; `tcsetattr()` sends
it). With `ISIG` Ctrl+C raises `USER_SIGNAL_0` on the programs' end of the
channel; the shell then kills the program in the foreground and clears it.
The shell's `restart-kbd` command asks `init` (over a control channel,
`PA_USER0`) to kill the keyboard driver, which `init` then restarts.

The screen console (`src/drivers/display/console.rs`) is a VT100/xterm
terminal: cursor movement and positioning, erasing, insert/delete of
characters and lines, scroll regions, 16, 256 and 24-bit colours, bold,
underline and reverse, the alternate screen (`ESC [ ? 1049 h`), cursor
visibility, autowrap, save/restore cursor, UTF-8 with box drawing, blocks,
accented Latin letters and a few symbols. A line feed also returns to
column 0 (newline mode, `ESC [ 20 l` turns it off), since there is no tty
layer adding the carriage return. Programs ask for its size with
`ioctl(TIOCGWINSZ)` (from `system_get_stats`).

### Unix-style calls (removed in step 8)

`write` (0x60), `read` (0x61), `open` (0x62), `close` (0x63), `lseek`
(0x64), `getpid` (0x70), `getppid` (0x71), `spawn` (0x03) and
`spawn_process` (0x0B) are gone, with the kernel's ramdisk, its VFS and its
per-process fd table: the C library provides file descriptors on top of
handles (console channels, filesystem connections, sockets), like
Fuchsia's `fdio`, and the filesystem service holds the files. `yield`
(0x72) stays (tests, and the C library waits with it when the console's
queue is full).

---

## Example: start a child process and talk to it

Abridged from `userspace/c-progs/ipctest.c`, which runs a copy of its own
image in the child:

```c
#include <rustux/process.h>             // rx_job_default, rx_vmar_root, ...

int main(void) {

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

# Rustux Kernel - Kernel Objects & Capability Security

**Version:** 0.2.0
**Date:** 2025-01-18

---

## Table of Contents

1. [Overview](#overview)
2. [Capability Security Model](#capability-security-model)
3. [Handle & Rights System](#handle--rights-system)
4. [Kernel Object Types](#kernel-object-types)
5. [Handle Operations](#handle-operations)
6. [Handle Table](#handle-table)
7. [Object Lifecycle](#object-lifecycle)
8. [Usage Examples](#usage-examples)
9. [Implementation Reference](#implementation-reference)

---

## Overview

The Rustux kernel uses a **capability-based security model** where all kernel resources are represented as **objects** accessed through **handles** with specific **rights**. This design is inspired by Zircon (Fuchsia OS) and provides:

- **Fine-grained access control** - Each operation requires specific rights
- **No global root** - No all-powerful user, only what handles grant
- **Composability** - Rights can be reduced when duplicating/transferring
- **Safety** - Type-safe object references prevent use-after-free bugs

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Kernel Object** | A resource managed by the kernel (process, VMO, channel, etc.) |
| **Handle** | A capability token that references an object |
| **Rights** | Bitmask specifying permitted operations on a handle |
| **Handle Table** | Per-process table mapping handle values to objects |

---

## Capability Security Model

### Traditional vs Capability-Based

**Traditional (UID/GID):**
```
Process running as "root" → Can access everything
Process running as "user" → Can access user's files
```

**Capability-Based:**
```
Process has handle to File A with READ rights → Can read File A
Process has NO handle to File B → Cannot access File B at all
Process has handle to File C with WRITE rights → Can write to File C
```

### Principles

1. **No Ambient Authority** - Having a process doesn't grant access to anything
2. **Least Privilege** - Handles only grant what's explicitly needed
3. **Delegation** - Processes can send handles via IPC with reduced rights
4. **Revocation** - Closing a handle immediately revokes access

### Example: File Access

```c
// Traditional Linux
int fd = open("/etc/passwd", O_RDONLY);  // Check UID/GID
read(fd, buffer, size);                   // No further checks

// Capability-Based (Rustux)
handle_t vmo = sys_vmo_create(4096);        // Create memory object
handle_t file = sys_file_open(vmo, READ);   // Get handle with READ
sys_vmo_write(file, ...);                   // Fails - no WRITE right
```

---

## Handle & Rights System

### Handle Structure

```c
typedef struct {
    uint32_t value;     // Handle value (index into handle table)
    uint32_t padding;   // Alignment
} handle_t;
```

### Rights Bitmask

| Right | Value | Hex | Description |
|-------|-------|-----|-------------|
| `NONE` | 0 | 0x00 | No rights |
| `READ` | 1 | 0x01 | Read state |
| `WRITE` | 2 | 0x02 | Modify state |
| `EXECUTE` | 4 | 0x04 | Execute code |
| `SIGNAL` | 8 | 0x08 | Signal object |
| `WAIT` | 8 | 0x08 | Wait on object |
| `MAP` | 16 | 0x10 | Map into address space |
| `DUPLICATE` | 32 | 0x20 | Duplicate handle |
| `TRANSFER` | 64 | 0x40 | Transfer to another process |
| `MANAGE` | 128 | 0x80 | Admin control |
| `APPLY_PROFILE` | 256 | 0x100 | Apply CPU profile |
| `SAME_RIGHTS` | 2147483648 | 0x80000000 | Keep same rights on dup |

### Rights Combinations

| Name | Rights | Usage |
|------|--------|-------|
| `BASIC` | READ \| WRITE | Basic read/write access |
| `DEFAULT` | BASIC \| SIGNAL \| MAP \| DUPLICATE | Default handle rights |

### Checking Rights

Before any operation, the kernel checks if the handle has the required rights:

```rust
// Example: VMO write operation
fn vmo_write(handle: &Handle, data: &[u8]) -> Result<(), Error> {
    handle.require(Rights::WRITE)?;  // Fails if no WRITE right
    // ... perform write operation
}
```

---

## Kernel Object Types

### Object Type Enumeration

| Type | Value | Name | Description | Default Rights |
|------|-------|------|-------------|----------------|
| `PROCESS` | 1 | "process" | Process object | MANAGE |
| `THREAD` | 2 | "thread" | Thread object | MANAGE |
| `VMO` | 3 | "vmo" | Virtual Memory Object | DEFAULT |
| `VMAR` | 4 | "vmar" | VM Address Region | MAP \| READ \| WRITE |
| `CHANNEL` | 5 | "channel" | IPC channel endpoint | READ \| WRITE |
| `EVENT` | 6 | "event" | Event object | SIGNAL \| WAIT |
| `EVENTPAIR` | 7 | "eventpair" | Event pair object | SIGNAL \| WAIT |
| `TIMER` | 8 | "timer" | Timer object | SIGNAL \| WRITE |
| `JOB` | 9 | "job" | Job (process group) | MANAGE |
| `PORT` | 10 | "port" | Port (waitset) | READ \| WRITE |
| `PROFILE` | 11 | "profile" | CPU profile | READ |

### Object Descriptions

#### Process (1)

A process object represents a userspace process.

**Operations:**
- Create: `sys_process_create()`
- Start: `sys_process_start()`
- Exit: `sys_process_exit()`

**Rights Required:**
- MANAGE: Create/terminate process, read info

#### Thread (2)

A thread object represents a thread of execution.

**Operations:**
- Create: `sys_thread_create()`
- Start: `sys_thread_start()`
- Exit: `sys_thread_exit()`

**Rights Required:**
- MANAGE: Control thread lifecycle

#### VMO - Virtual Memory Object (3)

A VMO represents a contiguous region of memory that can be mapped into process address spaces.

**Operations:**
- Create: `sys_vmo_create(size, flags)`
- Read: `sys_vmo_read(handle, offset, buffer, size)`
- Write: `sys_vmo_write(handle, offset, data, size)`
- Map: `sys_vmar_map(vmo, addr, size, flags)`

**Rights Required:**
- READ: Read from VMO
- WRITE: Write to VMO
- MAP: Map into address space
- DUPLICATE: Duplicate the VMO handle

**Example:**
```c
// Create a 4KB VMO
handle_t vmo = sys_vmo_create(4096, 0);

// Map it into process address space
void* addr = sys_vmar_map(vmo, 0, 4096, READ | WRITE);
```

#### VMAR - Virtual Memory Address Region (4)

A VMAR represents a region of virtual address space that can contain mappings.

**Operations:**
- Map: `sys_vmar_map(vmo, addr, size, flags)`
- Unmap: `sys_vmar_unmap(addr, size)`
- Protect: `sys_vmar_protect(addr, size, flags)`

**Rights Required:**
- MAP: Create new mappings
- READ: Read memory in region
- WRITE: Write to memory in region

#### Channel (5)

A channel is a bidirectional message queue for inter-process communication.

**Operations:**
- Create: `sys_channel_create(options)`
- Write: `sys_channel_write(handle, data, size, handles, num_handles)`
- Read: `sys_channel_read(handle, buffer, size, handles, num_handles)`

**Rights Required:**
- READ: Read messages from channel
- WRITE: Write messages to channel

**Example:**
```c
// Create a channel
handle_t ch = sys_channel_create(0);

// Send a message with a handle
char data[] = "Hello";
handle_t handles[] = {vmo_handle};
sys_channel_write(ch, data, sizeof(data), handles, 1);
```

#### Event (6)

An event is a synchronization primitive that can be signaled and waited on.

**Operations:**
- Create: `sys_event_create(options)`
- Signal: `sys_object_signal(handle)`
- Wait: `sys_object_wait_one(handle)`

**Rights Required:**
- SIGNAL: Signal the event
- WAIT: Wait for the event

**Example:**
```c
handle_t event = sys_event_create(0);

// In thread A:
sys_object_wait_one(event);  // Blocks until signaled

// In thread B:
sys_object_signal(event);    // Wakes thread A
```

#### Timer (8)

A timer object provides timed notifications.

**Operations:**
- Create: `sys_timer_create(options)`
- Set: `sys_timer_set(handle, deadline, slack)`
- Cancel: `sys_timer_cancel(handle)`
- Wait: `sys_object_wait_one(handle)`

**Rights Required:**
- WRITE: Set/cancel timer
- SIGNAL/WAIT: Wait for timer expiration

**Example:**
```c
handle_t timer = sys_timer_create(0);

// Set timer to fire in 1 second (1,000,000,000 nanoseconds)
sys_timer_set(timer, 1000000000, 0);

// Wait for timer
sys_object_wait_one(timer);
```

---

## Handle Operations

### Creating Handles

Handles are created when kernel objects are created:

```c
// Create a VMO - returns a handle
handle_t vmo = sys_vmo_create(4096, 0);
if (vmo < 0) {
    // Error: negative value indicates error
}
```

### Duplicating Handles

Handles can be duplicated with the same or reduced rights:

```c
// Duplicate with same rights
handle_t vmo2 = sys_handle_duplicate(vmo, SAME_RIGHTS);

// Duplicate with reduced rights (read-only)
handle_t vmo_ro = sys_handle_duplicate(vmo, READ);
```

### Closing Handles

Handles must be closed when no longer needed:

```c
// Close a handle
sys_handle_close(vmo);
```

### Transferring Handles

Handles can be transferred to other processes via IPC:

```c
// Send a handle through a channel
handle_t handles[] = {vmo_handle};
sys_channel_write(channel, data, data_size, handles, 1);

// Receive a handle through a channel
handle_t received_handles[1];
sys_channel_read(channel, buffer, buf_size, received_handles, 1);
```

---

## Handle Table

### Structure

Each process has a handle table that maps handle values to objects:

```
Process Handle Table (max 256 entries)
┌─────┬───────────┬───────────┬─────────┐
│ Idx │ Object    │ Rights    │ RefCnt  │
├─────┼───────────┼───────────┼─────────┤
│ 0   │ VMO #42   │ RW        │ 2       │
│ 1   │ Channel#7 │ RW        │ 1       │
│ 2   │ Process#1 │ MANAGE    │ 3       │
│ ... │ ...       │ ...       │ ...     │
└─────┴───────────┴───────────┴─────────┘
```

### Limits

- **Maximum handles per process:** 256
- **Handle value range:** 0-255 (index into table)
- **Handle value 0:** Reserved (NULL handle equivalent)

### Operations

| Operation | Syscall | Description |
|-----------|---------|-------------|
| Add | Implicit (on object create) | Add handle to table |
| Get | Implicit (in syscall handler) | Look up handle by value |
| Remove | `sys_handle_close` | Remove handle from table |
| Duplicate | `sys_handle_duplicate` | Copy handle to new slot |

---

## Object Lifecycle

### Reference Counting

All kernel objects use reference counting:

```
Object Creation
    ↓
Ref Count = 1
    ↓
Handle Created → Ref Count = 2
    ↓
Handle Duplicated → Ref Count = 3
    ↓
Handle Closed → Ref Count = 2
    ↓
Handle Closed → Ref Count = 1
    ↓
Final Handle Closed → Ref Count = 0
    ↓
Object Destroyed
```

### States

| State | Description |
|-------|-------------|
| **Initializing** | Object being created |
| **Alive** | Object is usable |
| **Destroying** | Object being destroyed (no new operations) |
| **Destroyed** | Object memory freed |

### Cleanup

When reference count reaches zero:
1. Object marked as "destroying"
2. Associated resources freed (memory, mappings, etc.)
3. Object memory deallocated

---

## Usage Examples

### Example 1: Shared Memory

```c
// Process A: Create and share memory
handle_t vmo = sys_vmo_create(4096, 0);
void* addr = sys_vmar_map(vmo, 0, 4096, READ | WRITE);

// Write data
strcpy((char*)addr, "Hello from Process A");

// Create channel for IPC
handle_t channel = sys_channel_create(0);

// Send VMO handle to Process B
handle_t handles[] = {vmo};
sys_channel_write(channel, "data", 4, handles, 1);

// Process B: Receive and access shared memory
handle_t received_vmo;
sys_channel_read(channel, buffer, buf_size, &received_vmo, 1);

// Map the received VMO
void* addr_b = sys_vmar_map(received_vmo, 0, 4096, READ | WRITE);

// Read data written by Process A
printf("Process B reads: %s\n", (char*)addr_b);
// Output: "Process B reads: Hello from Process A"
```

### Example 2: Rights Reduction

```c
// Create a VMO with full rights
handle_t vmo = sys_vmo_create(4096, 0);

// Duplicate with reduced rights (read-only)
handle_t vmo_readonly = sys_handle_duplicate(vmo, READ);

// This succeeds
sys_vmo_write(vmo, data, size);  // Original has WRITE

// This fails
sys_vmo_write(vmo_readonly, data, size);  // No WRITE right

// This succeeds
sys_vmo_read(vmo_readonly, buffer, size);  // Has READ right
```

### Example 3: Event Synchronization

```c
// Create event for synchronization
handle_t event = sys_event_create(0);

// Thread A: Worker
void* worker_thread(void* arg) {
    handle_t event = *(handle_t*)arg;

    // Do work
    process_data();

    // Signal completion
    sys_object_signal(event);
    sys_handle_close(event);
    return NULL;
}

// Main thread: Wait for worker
handle_t event = sys_event_create(0);
pthread_create(&thread, NULL, worker_thread, &event);

// Wait for worker to complete
sys_object_wait_one(event);

printf("Worker thread completed!\n");
sys_handle_close(event);
```

---

## Implementation Reference

### Data Structures

#### Handle (src/object/handle.rs)

```rust
pub struct Handle {
    pub id: HandleId,              // Unique handle ID
    pub base: *const KernelObjectBase,  // Pointer to object
    pub rights: Rights,            // Rights bitmask
}
```

#### Rights (src/object/handle.rs)

```rust
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Self = Self(0x00);
    pub const READ: Self = Self(0x01);
    pub const WRITE: Self = Self(0x02);
    pub const EXECUTE: Self = Self(0x04);
    pub const SIGNAL: Self = Self(0x08);
    pub const MAP: Self = Self(0x10);
    pub const DUPLICATE: Self = Self(0x20);
    pub const TRANSFER: Self = Self(0x40);
    pub const MANAGE: Self = Self(0x80);
    pub const APPLY_PROFILE: Self = Self(0x100);
    pub const SAME_RIGHTS: Self = Self(0x8000_0000);
}
```

#### KernelObjectBase (src/object/handle.rs)

```rust
pub struct KernelObjectBase {
    pub obj_type: ObjectType,          // Object type
    pub ref_count: AtomicUsize,        // Reference count
    pub destroying: AtomicBool,        // Being destroyed flag
}
```

#### HandleTable (src/object/handle.rs)

```rust
pub struct HandleTable {
    slots: [SpinMutex<Option<HandleEntry>>; MAX_HANDLES],
    count: SpinMutex<usize>,
}

pub const MAX_HANDLES: usize = 256;
```

### Module Locations

| Module | Location |
|--------|----------|
| Handle & Rights | `src/object/handle.rs` |
| Event | `src/object/event.rs` |
| Timer | `src/object/timer.rs` |
| Channel | `src/object/channel.rs` |
| VMO | `src/object/vmo.rs` |
| Job | `src/object/job.rs` |
| Process | `src/process/process.rs` |

---

## References

- **Zircon Handles:** https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts#objects
- **Capability-Based Security:** https://en.wikipedia.org/wiki/Capability-based_security
- **KeyKOS:** Early capability-based OS

---

*Last Updated: 2025-01-18*
*Author: Rustux Kernel Team*
*License: MIT*

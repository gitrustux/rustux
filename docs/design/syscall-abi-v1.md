# Rustux Syscall ABI Specification (v1)

> **Design target.** Moved from `gitrustux/loader` (`docs/syscall_abi_spec.md`). This is the planned object/handle syscall ABI. The syscalls actually implemented today, and their numbers, are documented in [`../SYSCALL.md`](../SYSCALL.md); most object syscalls are still stubs.

**Status:** Draft v1, not yet implemented (intended to be identical on ARM64, x86-64 and RISC-V)

---

## Core Principles

1. **Identical semantics across all architectures**
2. **Purely object- and handle-based**
3. **Validate:** argument ranges, rights masks, object state
4. **Never expose architecture-specific behavior**
5. **Return only documented error codes**
6. **Be deterministic and reorder-safe**

---

## Calling Convention

### Architecture-Specific Entry

| Architecture | Instruction | Syscall Num | Args | Return |
|--------------|-------------|-------------|------|--------|
| ARM64 | `svc #0` | x8 | x0-x5 | x0 |
| x86-64 | `syscall` | rax | rdi,rsi,rdx,r10,r8,r9 | rax |
| RISC-V | `ecall` | a7 | a0-a5 | a0 |

### Return Convention

```
Success: return value (>= 0)
Failure: return negative error code
```

---

## Handle & Rights Model

### Handle Structure

```
handle_id: u32    // Process-local handle identifier
rights: bitmask   // Capability rights
```

### Rights Categories

| Right | Description |
|-------|-------------|
| `RIGHT_READ` | Read object state |
| `RIGHT_WRITE` | Write/modify object |
| `RIGHT_EXECUTE` | Execute code |
| `RIGHT_SIGNAL` | Signal/wait on object |
| `RIGHT_MAP` | Map into address space |
| `RIGHT_DUP` | Duplicate handle |
| `RIGHT_TRANSFER` | Transfer to another process |
| `RIGHT_MANAGE` | Administrative control |

### Enforcement

If required right is missing → return `RX_ERR_ACCESS_DENIED`

| Operation | Required Right |
|-----------|----------------|
| read object state | `RIGHT_READ` |
| write / mutate | `RIGHT_WRITE` |
| signal object | `RIGHT_SIGNAL` |
| map memory | `RIGHT_MAP` |
| duplicate handle | `RIGHT_DUP` |
| transfer handle | `RIGHT_TRANSFER` |
| control process/thread/job | `RIGHT_MANAGE` |

---

## Error Codes (Stable)

```rust
pub enum RxError {
    OK                 = 0,
    INVALID_ARGS       = -1,
    NO_MEMORY          = -2,
    ACCESS_DENIED      = -3,
    NOT_FOUND          = -4,
    ALREADY_EXISTS     = -5,
    TIMED_OUT          = -6,
    SHOULD_WAIT        = -7,
    UNAVAILABLE        = -8,
    OUT_OF_RANGE       = -9,
    NOT_SUPPORTED      = -10,
    BAD_STATE          = -11,
    CANCELED           = -12,
    PEER_CLOSED        = -13,
    BAD_HANDLE         = -14,
}
```

---

## Syscall Catalog

### Process & Thread

#### `rx_process_create(parent_job, name, flags) -> handle`

Creates a new process object under a job.

**Requires:** `RIGHT_MANAGE` on `parent_job`

**Returns:** handle to new process with default rights

**Errors:**
- `INVALID_ARGS` - name too long, invalid flags
- `BAD_STATE` - parent job terminated
- `NO_MEMORY` - insufficient memory
- `ACCESS_DENIED` - insufficient rights

---

#### `rx_process_start(proc, entry, stack, arg) -> status`

Transitions a created process to **running** state.

**Requires:** `RIGHT_MANAGE` on `proc`

**Behavior:**
- Creates initial thread
- Loads entry + stack into address space
- After start, further mapping is user responsibility

**Errors:**
- `BAD_STATE` - process already running
- `INVALID_ARGS` - invalid entry/stack range
- `ACCESS_DENIED` - insufficient rights

---

#### `rx_thread_create(proc, name, flags) -> handle`

Creates a thread within a process.

**Requires:** `RIGHT_MANAGE` on `proc`

Thread exists in **created but not running** state.

---

#### `rx_thread_start(thread, entry, stack, arg) -> status`

Begins thread execution.

**Errors:**
- `BAD_STATE` - already started
- `INVALID_ARGS` - invalid mapping
- `ACCESS_DENIED` - insufficient rights

---

#### `rx_thread_exit(status) -> !`
#### `rx_process_exit(status) -> !`

Terminates thread or process.

**Behavior:**
- All handles closed
- Peer objects notified (`SIG_TERMINATED`)

---

#### `rx_handle_close(handle) -> status`

Idempotent close.

**Errors:**
- `BAD_HANDLE` - invalid handle

---

### Memory / VMO

#### `rx_vmo_create(size, flags) -> handle`

Creates a **virtual memory object** backing pages.

**Flags:**
- `VMO_COW` - Copy-on-write
- `VMO_ZERO_ON_COMMIT` - Zero pages on fault

**Size Rules:**
- Rounded up to page size
- Zero size → `INVALID_ARGS`

---

#### `rx_vmo_read(vmo, offset, buf, len) -> bytes_read`
#### `rx_vmo_write(vmo, offset, buf, len) -> bytes_written`

**Requires:** `RIGHT_READ` / `RIGHT_WRITE`

**Behavior:**
- Partial reads/writes permitted at EOF
- Out-of-range → `OUT_OF_RANGE`

---

#### `rx_vmo_clone(vmo, flags) -> handle`

COW copy.

**Edge Cases:**
- Deep clones allowed
- Write to original or clone triggers COW split

---

#### `rx_vmar_map(proc, vmo, offset, addr_hint, len, flags) -> addr`

Maps VMO pages into process address space.

**Requires:** `RIGHT_MAP` on VMO and `RIGHT_MANAGE` on process

**Flags:**
- `VMAR_READ`, `VMAR_WRITE`, `VMAR_EXECUTE`
- `VMAR_FIXED` - reject if region unavailable

**Errors:**
- `ALREADY_EXISTS` - overlap with existing mapping
- `INVALID_ARGS` - invalid alignment

---

#### `rx_vmar_unmap(proc, addr, len) -> status`

Fully removes mappings. Partial region allowed.

---

#### `rx_vmar_protect(proc, addr, len, flags) -> status`

Changes protection on existing mapping.

---

### IPC & Sync

#### `rx_channel_create(options) -> (handle1, handle2)`

Creates two-endpoint message channel.

**Guarantees:**
- Messages delivered in FIFO order
- Capacity-bounded
- Backpressure via `SHOULD_WAIT`

---

#### `rx_channel_write(ch, bytes, handles[]) -> status`

**Requires:** `RIGHT_WRITE`

**Behavior:**
- Blocks or returns `SHOULD_WAIT` if full
- Handle transfer clears source handle
- `existing_rights ∧ mask` = transferred rights

**Edge Case:**
- Writing to closed peer → `PEER_CLOSED`

---

#### `rx_channel_read(ch, out_bytes, out_handles) -> status`

**Requires:** `RIGHT_READ`

**Behavior:**
- Reads one full message
- Insufficient buffer → `OUT_OF_RANGE`
- Empty + open → `SHOULD_WAIT`
- Empty + peer closed → `PEER_CLOSED`

---

#### `rx_event_create() -> handle`
#### `rx_eventpair_create() -> (handle1, handle2)`

Manual signaling or paired signaling primitives.

---

#### `rx_object_signal(obj, clear_mask, set_mask) -> status`

Atomically modifies signal bits.

**Requires:** `RIGHT_SIGNAL`

Invalid bitmask → `INVALID_ARGS`.

---

#### `rx_object_wait_one(obj, signals, deadline) -> signals_observed`
#### `rx_object_wait_many(list[], deadline) -> signals_observed`

**Blocking Semantics:**
- Wake when *any* requested signal becomes active
- `deadline == 0` → nonblocking poll
- Timeout → `TIMED_OUT`

---

### Jobs & Handles

#### `rx_job_create(parent, flags) -> handle`

Creates job under parent job hierarchy.

Jobs propagate termination downward.

---

#### `rx_handle_duplicate(h, rights_mask) -> new_handle`

Returns new handle with `new_rights = old_rights ∧ rights_mask`.

Trying to add rights → `ACCESS_DENIED`.

---

#### `rx_handle_transfer(h, target_proc, rights_mask) -> status`

Moves handle to target process.

Source handle becomes invalid.

---

### Time

#### `rx_clock_get(which) -> nanoseconds`

- `CLOCK_MONOTONIC` - never decreases
- `CLOCK_REALTIME` - may adjust (slew permitted)

---

#### `rx_timer_create() -> handle`
#### `rx_timer_set(timer, deadline, slack) -> status`
#### `rx_timer_cancel(timer) -> status`

**Signals:**
- Completion sets `SIG_TIMER_DONE`

Slack allows batching for power efficiency.

---

## Signal Bits

| Signal | Description |
|--------|-------------|
| `SIG_READABLE` | Object can be read |
| `SIG_WRITABLE` | Object can be written |
| `SIG_PEER_CLOSED` | Peer endpoint closed |
| `SIG_SIGNALED` | Object was signaled |
| `SIG_TERMINATED` | Process/thread exited |
| `SIG_TIMER_DONE` | Timer fired |

---

## Object Types

| Type | Description |
|------|-------------|
| `PROCESS` | Address space + thread container |
| `THREAD` | Executable execution context |
| `VMO` | Virtual memory object backing |
| `VMAR` | VMO region mapped into a process |
| `CHANNEL` | Bidirectional IPC message queue |
| `EVENT` / `EVENTPAIR` | Synchronization primitives |
| `JOB` | Process tree supervisor |
| `TIMER` | One-shot or repeating timer |
| `PORT` | Waitset / async dispatch target |

---

## Versioning

- Syscalls identified by **stable numeric IDs**
- Removing or altering behavior is **forbidden**
- Deprecations require capability flags
- Each release documents syscall digest

---

*Syscall ABI v1 - Frozen 2025-01-03*

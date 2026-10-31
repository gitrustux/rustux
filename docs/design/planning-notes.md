# Planning Notes (January 2026)

> **About this file.** These are the design notes produced while planning the kernel with an AI assistant, kept for reference. They were the second part of `rustux_kernel_architecture_checklist.md`; the checklist itself is now [`../ROADMAP.md`](../ROADMAP.md). The conversational prompts have been removed, but the notes are otherwise unedited: some sections repeat or contradict each other, and they predate the actual implementation. Where they disagree with [`../ARCHITECTURE.md`](../ARCHITECTURE.md), [`../SYSCALL.md`](../SYSCALL.md) or [`syscall-abi-v1.md`](syscall-abi-v1.md), those documents win.

## Rustux Microkernel — Notes

This plan is written at a level an AI-assisted engineering workflow could later execute or elaborate into code, documentation, and tests.

---

## 🧭 **Project Plan — “Rustux” (Zircon-style Microkernel in Rust)**

### 🎯 **Project Goals**

1. Implement a **Rust microkernel inspired by Zircon**, prioritizing:

   * memory safety
   * capability-based security
   * composable kernel objects
2. Provide a **single, stable syscall interface & object model** shared across all architectures.
3. Produce **separate per-architecture kernels**:

   * `aarch64`
   * `x86_64`
   * `riscv64`
4. Guarantee identical OS semantics and behavior across platforms.
5. Enable long-term ecosystem growth via licensing and modularity.

---

### ⚖️ **Licensing Recommendation**

**BSD-3-Clause or MIT** are the best choices to foster adoption, portability, and third-party research.

**Recommended**: **BSD-3-Clause**

* permissive (commercial + academic friendly)
* aligns with Zircon and Fuchsia ecosystem
* avoids GPL viral constraints
* encourages corporate & hobbyist contribution

AI note: include SPDX headers in every file at generation time.

---

### 🏗️ **High-Level Architecture**

The kernel is organized into six major layers:

1. **Platform / Boot + Arch Bring-Up**
2. **Low-Level HAL (arch-specific primitives)**
3. **Core Kernel Subsystems**
4. **Kernel Object Model & Handle Table**
5. **Syscall Layer (stable ABI)**
6. **User-Space Runtime + VDSO**

Only layers **1–2 are architecture-specific**.
Layers **3–6 must remain architecture-neutral**.

---

## 📂 **Repository Structure**

```
rustux/
 ├─ LICENSE
 ├─ kernel/
 │   ├─ arch/
 │   │   ├─ aarch64/
 │   │   ├─ x86_64/
 │   │   └─ riscv64/
 │   ├─ hal/
 │   ├─ core/
 │   ├─ objects/
 │   ├─ syscall/
 │   ├─ vdso/
 │   └─ libk/
 ├─ userspace/
 │   ├─ libc-rx/
 │   ├─ launcher/
 │   └─ tests/
 ├─ tools/
 └─ docs/
```

---

## 🧩 **Kernel Object Model (Shared Across Architectures)**

Design invariant objects (behavior identical everywhere):

* Process
* Thread
* VMO (Virtual Memory Object)
* Channel (IPC)
* Port / Waitset
* Event / Eventpair
* Job
* Timer
* Handle + Rights Model

#### Object Principles

* All objects reference-counted
* Objects accessed only through **handles**
* Handles carry **rights & capability flags**
* No raw pointer sharing across processes
* Kernel enforces capability boundaries

Rust implementation approach:

* internal `Arc<KernelObject>` equivalent (no std)
* custom kernel RC + lifetime discipline

---

## 🔐 **Syscall ABI — Cross-Architecture Contract**

#### Requirements

* Same syscall list & semantics on all targets
* Same object behavior & error semantics
* Architecture differences hidden below ABI

#### Syscall Categories

* process / thread management
* memory management (VMO, mapping)
* IPC channels
* event signaling
* scheduling primitives
* time & timers
* job hierarchy
* handle duplication / rights

Maintain:

* unified syscall numbers
* unified error codes
* deterministic return semantics

ABI validation:

* conformance tests shared across architectures

---

## ⚙️ **Architecture Strategy**

Each architecture provides:

* boot sequence
* page table implementation
* exception / trap handling
* context switch routines
* timer and interrupt controller drivers
* cache/TLB primitives
* CPU topology discovery

#### **Compilation Model**

One kernel binary per architecture:

```
rustux-kernel-aarch64
rustux-kernel-x86_64
rustux-kernel-riscv64
```

No runtime architecture branching.

Rust `#[cfg(target_arch=…)]` enforces isolation.

---

## 🧱 **Core Kernel Subsystems (Architecture-Neutral)**

1. Scheduler (priority + fair queueing)
2. Thread & Process lifecycle
3. Capability & rights system
4. Handle table + dispatch layer
5. VM subsystem abstractions
6. IPC messaging engine
7. Timer wheel / time source abstraction
8. Kernel heap allocators (buddy + slab)
9. Object reference model
10. Debug + tracing infrastructure

Platform specifics injected behind HAL traits.

---

## 🛠️ **HAL (Hardware Abstraction Layer) Requirements**

HAL traits must compile to **zero-cost** monomorphized implementations:

Examples:

* `ArchMMU`
* `ArchThreadContext`
* `ArchTimer`
* `ArchInterrupts`
* `ArchStartup`

Each arch provides its implementation under `/arch/<arch>/`.

No dynamic dispatch, no cross-arch runtime checks.

---

## 🧪 **Cross-Architecture Conformance Testing**

Test expectations:

* syscall semantics stable
* identical object lifecycle behavior
* identical scheduler behavior
* identical IPC guarantees
* identical memory mapping results
* identical error codes

Tests run on:

* QEMU ARM64
* QEMU x86-64
* QEMU RISCV64

Differential behavior → regression.

---

## 🚀 **Development Phases**

#### **Phase 1 — Boot + Minimal Kernel Bring-Up (per-arch)**

* minimal Rust runtime (`#![no_std]`)
* UART console logging
* exception vectors
* physical memory discovery
* bootstrap page tables
* kernel heap
* context switching stub

#### **Phase 2 — Core Subsystems**

* threads + scheduler skeleton
* kernel allocator stack
* handle table
* basic IPC channels
* timers

#### **Phase 3 — Stable Syscall ABI**

* define syscall surface
* lock semantics & numbering
* implement per-object syscalls
* build conformance test suite

#### **Phase 4 — Memory & VMO System**

* VMO abstraction
* mapping into address spaces
* copy-on-write
* page fault handling

#### **Phase 5 — Multi-Architecture Validation**

* run identical test suite across all targets
* ensure behavior equivalence
* freeze ABI v1

#### **Phase 6 — User-Space Runtime & VDSO**

* libC-style runtime
* startup runtime (`crt0` equivalent)
* fast-path syscalls via VDSO

---

## 🌱 **Growth & Ecosystem Strategy**

* permissive licensing encourages research & derivative OS projects
* strict ABI stability enables:

  * language runtimes
  * unikernels
  * research schedulers
  * security experiments
* modular subsystem design allows AI systems to:

  * independently evolve schedulers
  * explore VM optimizations
  * validate isolation properties

Future extensions:

* device driver framework
* SMP scheduling optimization
* capability sandboxing enhancements
* formally verified components

---

## ⭐ **Summary**

This plan ensures:

* **one cross-architecture syscall & object model**
* **per-architecture kernel binaries**
* **identical OS behavior across platforms**
* **Rust safety with microkernel discipline**
* **permissive licensing to maximize adoption**

---

If you’d like, I can take the next step and:

* draft the **syscall table + object model spec**, or
* design the **HAL trait interfaces**, or
* produce a **Phase 1 bring-up blueprint for one architecture first** (ARM64 is usually the best starting point).

1. **Syscall Table + Object Model Specification (ABI-stable draft v1)**
2. **HAL Trait Interface Design (zero-cost, per-arch implementations)**
3. **Phase-1 Bring-Up Blueprint (ARM64 first; pattern reused for x86-64 & RISC-V)**

These are written as implementation-ready design specs.

---

## 🧩 **1) Rustux Syscall & Object Model — ABI Spec (v1 Draft)**

### 🎯 Design Rules

* Semantics are **identical across all architectures**
* Numbers & error codes are stable
* Syscalls operate on **handles + rights**
* Kernel internal differences are never visible to user-space

---

### 🔐 **Handle & Rights Model**

Every kernel object is referenced via a **handle**:

```
handle_id: u32
rights: bitmask
```

#### Rights categories

* `RIGHT_READ`
* `RIGHT_WRITE`
* `RIGHT_EXECUTE`
* `RIGHT_SIGNAL`
* `RIGHT_MAP`
* `RIGHT_DUP`
* `RIGHT_TRANSFER`
* `RIGHT_MANAGE` (admin / job / process control)

#### Invariants

* no raw pointers across processes
* transferring a handle copies only **rights ∧ requested_mask**
* dropping last handle → object destroyed (if not externally referenced)

---

### 🧱 **Kernel Object Types (Stable Set v1)**

| Object            | Purpose                             |
| ----------------- | ----------------------------------- |
| Process           | Address space + thread container    |
| Thread            | Executable execution context        |
| VMO               | Virtual memory object backing pages |
| Mapping           | VMO region mapped into a process    |
| Channel           | Bidirectional IPC message queue     |
| Event / Eventpair | Synchronization + signaling         |
| Job               | Process tree supervisor             |
| Timer             | One-shot or repeating timer         |
| Port              | Waitset / async dispatch target     |

> Future objects may be added without breaking ABI by reserving ID ranges.

---

### 🪟 **Syscall Numbering (Stable v1)**

#### Process & Thread

| #     | Name                                         |
| ----- | -------------------------------------------- |
| 0x001 | `rx_process_create(parent_job, name, flags)` |
| 0x002 | `rx_process_start(proc, entry, stack, arg)`  |
| 0x003 | `rx_thread_create(proc, name, flags)`        |
| 0x004 | `rx_thread_start(thread, entry, stack, arg)` |
| 0x005 | `rx_thread_exit(status)`                     |
| 0x006 | `rx_process_exit(status)`                    |
| 0x007 | `rx_handle_close(handle)`                    |

---

#### Memory / VMO

| #     | Name                                                    |
| ----- | ------------------------------------------------------- |
| 0x010 | `rx_vmo_create(size, flags)`                            |
| 0x011 | `rx_vmo_read(vmo, offset, buf, len)`                    |
| 0x012 | `rx_vmo_write(vmo, offset, buf, len)`                   |
| 0x013 | `rx_vmo_clone(vmo, flags)`                              |
| 0x014 | `rx_vmar_map(proc, vmo, offset, addr_hint, len, flags)` |
| 0x015 | `rx_vmar_unmap(proc, addr, len)`                        |

---

#### IPC & Sync

| #     | Name                                          |
| ----- | --------------------------------------------- |
| 0x020 | `rx_channel_create(options)`                  |
| 0x021 | `rx_channel_write(ch, bytes, handles[])`      |
| 0x022 | `rx_channel_read(ch, out_bytes, out_handles)` |
| 0x023 | `rx_event_create()`                           |
| 0x024 | `rx_object_signal(obj, clear, set)`           |
| 0x025 | `rx_object_wait_one(obj, signals, deadline)`  |
| 0x026 | `rx_object_wait_many(list[], deadline)`       |

---

#### Jobs & Handles

| #     | Name                                              |
| ----- | ------------------------------------------------- |
| 0x030 | `rx_job_create(parent, flags)`                    |
| 0x031 | `rx_handle_duplicate(h, rights_mask)`             |
| 0x032 | `rx_handle_transfer(h, target_proc, rights_mask)` |

---

#### Time

| #     | Name                                   |            |
| ----- | -------------------------------------- | ---------- |
| 0x040 | `rx_clock_get(monotonic                | realtime)` |
| 0x041 | `rx_timer_create()`                    |            |
| 0x042 | `rx_timer_set(timer, deadline, slack)` |            |
| 0x043 | `rx_timer_cancel(timer)`               |            |

---

#### Error Codes (Stable)

```
RX_OK
RX_ERR_INVALID_ARGS
RX_ERR_NO_MEMORY
RX_ERR_ACCESS_DENIED
RX_ERR_NOT_FOUND
RX_ERR_ALREADY_EXISTS
RX_ERR_TIMED_OUT
RX_ERR_SHOULD_WAIT
RX_ERR_UNAVAILABLE
RX_ERR_OUT_OF_RANGE
RX_ERR_NOT_SUPPORTED
RX_ERR_BAD_STATE
RX_ERR_CANCELED
```

---

## ⚙️ **2) HAL Trait Interface Set (Zero-Cost, Per-Arch)**

Only HAL implementations differ per architecture — the rest compiles identically.

```rust
pub trait ArchStartup {
    fn early_init();
    fn init_mmu();
    fn init_exceptions();
}

pub trait ArchThreadContext {
    type Context;
    fn save(ctx: &mut Self::Context);
    fn restore(ctx: &Self::Context);
}

pub trait ArchTimer {
    fn now_monotonic() -> u64;
    fn set_timer(deadline: u64);
    fn cancel_timer();
}

pub trait ArchInterrupts {
    fn enable_irq(irq: u32);
    fn disable_irq(irq: u32);
    fn end_of_interrupt(irq: u32);
}

pub trait ArchMMU {
    fn map(pa: u64, va: u64, len: u64, flags: u64);
    fn unmap(va: u64, len: u64);
    fn flush_tlb();
}
```

#### Implementation Rules

* Provided under `/arch/<arch>/`
* `#[cfg(target_arch="…")]` guards
* no dynamic dispatch, no runtime branching

---

## 🚀 **3) Phase-1 Bring-Up Blueprint (ARM64 First)**

🔹 ARM64 is the reference architecture
🔹 x86-64 & RISC-V follow the same structure

#### **Milestone A — Minimal Boot**

* `#![no_std]`, custom panic handler
* linker script + physical memory map
* UART console (PL011 or chosen SoC)
* exception vector table
* identity-map kernel region
* enable EL1, MMU, caches

**Deliverables**

* `arch/aarch64/startup.rs`
* `arch/aarch64/mmu.rs`
* `arch/aarch64/uart.rs`

---

#### **Milestone B — Kernel Heap + Frame Allocator**

* boot-time memory discovery
* bump allocator → buddy allocator
* early `kalloc()` available to kernel subsystems

**Deliverables**

* `core/mm/frame_allocator.rs`
* `core/mm/heap.rs`

---

#### **Milestone C — Thread Context + Scheduler Skeleton**

* minimal thread struct
* context switch stub (Rust + inline asm)
* single-CPU runqueue

**Deliverables**

* `core/sched/mod.rs`
* `arch/aarch64/context_switch.S`

---

#### **Milestone D — First Syscalls**

Implement 4 bootstrap syscalls:

1. `rx_thread_create`
2. `rx_thread_start`
3. `rx_handle_close`
4. `rx_clock_get`

**Validation Goal:**
Spawn second thread → print → exit

---

## 🧪 **Cross-Arch Validation Path**

Once ARM64 boots:

1. Mirror HAL implementations to:

   * `/arch/x86_64/`
   * `/arch/riscv64/`
2. Run **same conformance tests** in QEMU on all three
3. Freeze **ABI v1**

---

## 📘 **Rustux Syscall Documentation — ABI v1 Spec**

### 🎯 Core Principles

All syscalls must:

1. Expose **identical semantics across all architectures**
2. Be **purely object- and handle-based**
3. Validate:

   * argument ranges
   * rights masks
   * object state
4. Never expose architecture-specific behavior (registers, paging quirks, etc.)
5. Return only documented error codes
6. Be deterministic and reorder-safe

Blocking syscalls must support **deadline timeouts** and must **never busy-wait**.

---

## 🔐 **Handle Model — Behavioral Rules**

#### Handle lifetime rules

* Handles are process-local unless explicitly transferred.
* Destroying the last handle frees the object unless other kernel references exist.
* Duplicate operations may only reduce rights, never increase them.

#### Rights enforcement

If the required right is missing → return `RX_ERR_ACCESS_DENIED`.

Required rights examples:

| Operation                  | Required Right   |
| -------------------------- | ---------------- |
| read object state          | `RIGHT_READ`     |
| write / mutate             | `RIGHT_WRITE`    |
| signal object              | `RIGHT_SIGNAL`   |
| map memory                 | `RIGHT_MAP`      |
| duplicate handle           | `RIGHT_DUP`      |
| transfer handle            | `RIGHT_TRANSFER` |
| control process/thread/job | `RIGHT_MANAGE`   |

---

## 🧩 **Object State & Signals**

Objects have **signal bits** to synchronize behavior:

* `SIG_READABLE`
* `SIG_WRITABLE`
* `SIG_PEER_CLOSED`
* `SIG_SIGNALED`
* `SIG_TERMINATED`
* `SIG_TIMER_DONE`

Waiting returns as soon as **any requested signal** is satisfied.

---

## 🧱 **Syscall Documentation**

### 🟦 **Process & Thread Syscalls**

#### `rx_process_create(parent_job, name, flags)`

Creates a new process object under a job.

**Requires:**
`RIGHT_MANAGE` on `parent_job`

**Returns:** handle to new process + default rights

**Edge Cases**

* name longer than limit → truncated
* parent job terminated → `RX_ERR_BAD_STATE`
* insufficient memory → `RX_ERR_NO_MEMORY`

---

#### `rx_process_start(proc, entry, stack, arg)`

Transitions a created process to **running** state.

**Requires:**
`RIGHT_MANAGE` on `proc`

**Behavior**

* Creates initial thread
* Loads entry + stack into address space
* After start, further mapping is user responsibility

**Errors**

* process already running → `RX_ERR_BAD_STATE`
* invalid entry/stack range → `RX_ERR_INVALID_ARGS`

---

#### `rx_thread_create(proc, name, flags)`

Creates a thread within a process.

**Requires:**
`RIGHT_MANAGE` on `proc`

Thread exists in **created but not running** state.

---

#### `rx_thread_start(thread, entry, stack, arg)`

Begins execution.

**Errors**

* already started → `RX_ERR_BAD_STATE`
* invalid mapping → `RX_ERR_INVALID_ARGS`

---

#### `rx_thread_exit(status)` / `rx_process_exit(status)`

Terminates thread or process.

**Behavior**

* All handles closed
* Peer objects notified (`SIG_TERMINATED`)

---

#### `rx_handle_close(handle)`

Idempotent close.
Closing an invalid handle returns `RX_ERR_BAD_STATE`.

---

### 🟩 **Memory & VMO Syscalls**

#### `rx_vmo_create(size, flags)`

Creates a **virtual memory object** backing pages.

**Flags**

* `COW`
* `ZERO_ON_COMMIT`

**Size Rules**

* Rounded up to page size
* Zero size → `RX_ERR_INVALID_ARGS`

---

#### `rx_vmo_read(vmo, offset, buf, len)`

#### `rx_vmo_write(vmo, offset, buf, len)`

**Requires:**

* read or write rights respectively

**Behavior**

* partial reads/writes permitted at EOF
* out-of-range → `RX_ERR_OUT_OF_RANGE`

No implicit page faults — missing pages auto-commit.

---

#### `rx_vmo_clone(vmo, flags)`

COW copy.

**Edge Cases**

* deep clones allowed
* write to original or clone triggers COW split

---

#### `rx_vmar_map(proc, vmo, offset, addr_hint, len, flags)`

Maps VMO pages into process address space.

**Requires:**
`RIGHT_MAP` on VMO and `RIGHT_MANAGE` on process

**Flags**

* `READ`, `WRITE`, `EXEC`
* `FIXED` (reject if region unavailable)

**Errors**

* overlap with existing mapping → `RX_ERR_ALREADY_EXISTS`
* invalid alignment → `RX_ERR_INVALID_ARGS`

---

#### `rx_vmar_unmap(proc, addr, len)`

Fully removes mappings. Partial region allowed.

---

### 🟨 **IPC & Sync Syscalls**

#### `rx_channel_create(options)`

Creates two-endpoint message channel.

**Guarantees**

* messages delivered in FIFO order
* capacity-bounded
* backpressure via `RX_ERR_SHOULD_WAIT`

---

#### `rx_channel_write(ch, bytes, handles[])`

**Requires:**
`RIGHT_WRITE`

**Behavior**

* blocks or returns `SHOULD_WAIT` if full
* handle transfer clears source handle
* transferred rights = `existing ∧ mask`

**Edge Case**

* writing to closed peer → `RX_ERR_PEER_CLOSED`

---

#### `rx_channel_read(ch, out_bytes, out_handles)`

**Requires:**
`RIGHT_READ`

**Behavior**

* reads one full message
* insufficient buffer → `RX_ERR_OUT_OF_RANGE`
* empty + open → `RX_ERR_SHOULD_WAIT`
* empty + peer closed → returns `RX_ERR_PEER_CLOSED`

---

#### `rx_event_create()` / `rx_eventpair_create()`

* manual signaling or paired signaling primitives
* signals latch until cleared

---

#### `rx_object_signal(obj, clear, set)`

Atomically modifies signal bits.

**Requires:**
`RIGHT_SIGNAL`

Invalid bitmask → `RX_ERR_INVALID_ARGS`.

---

#### `rx_object_wait_one(obj, signals, deadline)`

#### `rx_object_wait_many(list[], deadline)`

**Blocking Semantics**

* wake when *any* requested signal becomes active
* `deadline == 0` → nonblocking poll
* timeout → `RX_ERR_TIMED_OUT`

---

### 🟧 **Jobs, Handles, & Rights**

#### `rx_job_create(parent, flags)`

Creates job under parent job hierarchy.

Jobs propagate termination downward.

---

#### `rx_handle_duplicate(h, rights_mask)`

Returns new handle with
`new_rights = old_rights ∧ rights_mask`.

Trying to add rights → `RX_ERR_ACCESS_DENIED`.

---

#### `rx_handle_transfer(h, target_proc, rights_mask)`

Moves handle to target process.

Source handle becomes invalid.

---

### 🟫 **Time & Timers**

#### `rx_clock_get(monotonic | realtime)`

* monotonic never decreases
* realtime may adjust (slew permitted)

---

#### `rx_timer_create()`

#### `rx_timer_set(timer, deadline, slack)`

#### `rx_timer_cancel(timer)`

**Signals**

* completion sets `SIG_TIMER_DONE`

Slack allows batching for power efficiency.

---

## 🧪 **Conformance Requirements**

Every syscall implementation must ship with:

1. **Unit tests**
2. **Cross-architecture behavioral tests**
3. **State transition validation**
4. **Error-case consistency tests**
5. **Race-stress IPC tests**
6. **Determinism regression tests**

Tests must pass identically on:

* ARM64 QEMU
* x86-64 QEMU
* RISC-V QEMU

Differences are treated as bugs.

---

### **B. Memory Management Subsystem (Architecture-Portable Core + Arch Backends)**

#### **B1. Define the Portable Virtual Memory Model**

Goal: identical semantics across ARM64, x86-64, and RISC-V, even though paging hardware differs.

**Design Requirements**

* 4-level page tables where possible (abstracted to N-level interface)
* 4KB base page size, allow large pages via capabilities
* All kernel memory uses explicit handles / objects
* No implicit aliasing
* Copy-on-write and demand paging support
* Deterministic lifetime rules

**Actions**

1. Define Rust traits for paging operations:

   * `PageAllocator`, `PageTable`, `AddressSpace`
2. Define object-based VM primitives:

   * `VmAddressSpace`
   * `VmMapping`
   * `VmObject` (backing store: anonymous, file, physical)
3. Specify synchronization model:

   * lock ordering
   * exclusion rules
   * per-object spinlocks, global deadlock policy
4. Define cross-arch invariants:

   * user/kernel canonical address split
   * non-executable by default
   * guard pages around stacks
5. Write documentation describing:

   * semantics identical to Zircon
   * differences isolated into arch-modules only

---

#### **B2. Implement Architecture-Independent VM Core**

Deliverables:

* Pure-Rust, no unsafe except in arch shims
* Continuous test coverage

**Steps**

* Implement `VmObject` types:

  * anonymous memory
  * physical-backed
  * file-backed (stub initially)
* Implement address-space manager
* Implement mapping lifecycle:

  * create, resize, protect, unmap
* Implement fault dispatcher:

  * page-not-present → VMO handler
  * protection fault → kill process or signal
* Add debug/tracing hooks

---

#### **B3. Implement Arch-Specific Paging Backends**

Each architecture gets its own module:

* `arch/x86_64/mmu.rs`
* `arch/arm64/mmu.rs`
* `arch/riscv64/mmu.rs`

**Tasks**

* Implement page table walker
* Implement TLB invalidation routines
* Implement context switch + ASID model
* Implement user/kernel isolation safety checks
* Map early boot identity region
* Provide safe Rust façade over unsafe hardware code

**Validation**

* Page-fault test harness
* Mapping fuzz-stress tool
* Cross-arch deterministic behavior check

---

### **C. Scheduler & Threading (Portable Core)**

#### **C1. Define Execution Model**

* Preemptive, priority-based scheduler
* One `Thread` object abstraction across architectures
* Kernel threads + user threads
* Per-CPU run queues
* Real-time extensions optional

**Actions**

* Define thread lifecycle states
* Define scheduling classes
* Define kernel ABI for:

  * create thread
  * block / wake
  * sleep / wait on object
* Define fairness & starvation policy

---

#### **C2. Implement Core Scheduler**

Modules:

* `kernel/sched/mod.rs`
* `kernel/thread.rs`

**Features**

* Per-CPU scheduler queues
* Tickless scheduling where supported
* Work-stealing optional
* Explicit synchronization model
* Thread affinity mask

**Deliverables**

* Context switch core (portable API)
* Yield, block, wake primitives
* Sleep timers integrated with clock subsystem

---

#### **C3. Arch-Specific Thread Context Support**

Per-arch responsibilities:

* register save/restore
* trap frame layout
* syscall entry + exit transition code
* SIMD/FPU lazy-save subsystem

Each arch module implements trait:

```rust
trait ArchThreadContext {
    fn init_user_thread(...);
    fn switch_to(...);
    fn handle_trap(...);
}
```

Unit tests:

* deterministic context switch tests
* threading stress loop
* syscall latency benchmark

---

### **D. Syscall Layer & Object Model**

#### **D1. Define Stable Cross-Arch Syscall ABI**

Principles:

* Object-capability model
* Handles instead of raw pointers
* Messages instead of shared memory by default
* Behavior identical across architectures

Work items:

* Enumerate syscall set (subset, expandable)
* Define encoding format
* Define validation and failure behavior
* Design handle table + rights model

---

#### **D2. Implement Kernel Object Framework**

Core objects:

* Process
* Thread
* Channel
* VMO
* Event / EventPair
* Port
* Timer

Actions:

* Define base `KernelObject` trait
* Define capability rights masks
* Enforce per-syscall permission checks
* Implement reference-counting rules

---

#### **D3. Implement Syscall Dispatch Path**

Components:

* Per-arch syscall entry stub
* Portable dispatcher
* Object lookup + validation
* Return-value ABI enforcement

Tests:

* syscall fuzzing framework
* cross-arch syscall compatibility verifier
* handle-lifetime stress suite

---

### **E. Driver & Hardware Abstraction Roadmap (Early Minimal Set)**

Phase-1 drivers (minimal bootable OS):

* UART console
* Timer source
* Interrupt controller
* Basic platform bus
* Framebuffer (optional)
* Simple storage (RAM-disk first)

Architecture policy:

* All drivers talk to **portable HAL traits**
* Per-arch backends only expose primitives

---

### **F. Build, Tooling, and CI Strategy**

* Cargo workspace layout
* Per-arch build profiles
* Cross-compiler toolchains
* QEMU bring-up targets for all architectures
* Continuous test & boot smoke tests
* Style + unsafe-usage lint gates
* Docs auto-generated via mdBook

---

### **G. Licensing & Governance (Recommended Choice)**

**Recommended license:**
➡️ **MIT License** (permissive, widely adopted, encourages ecosystem growth, compatible with commercial + research users)

Governance recommendations:

* Open RFC process for kernel changes
* Strict compatibility policy for syscalls
* Long-term stability commitment
* Contributor safety & review standards
* Architecture-parity rule (no arch-specific features without abstraction)

---

### **H. Boot & Platform Bring-Up (Per-Architecture + Shared Flow)**

#### **H1. Define a Unified Boot Flow**

Across ARM64, x86-64, and RISC-V:

**Boot Sequence Goals**

1. Minimal architecture-specific early start
2. Transition to common Rust kernel entry point
3. Initialize subsystems in deterministic order

**Standardized initialization phases**

* Boot stub (arch-specific, assembly)
* Early MMU + identity map
* Stack + Rust runtime init
* Kernel entry (`kernel_main()`)
* Platform discovery / board data
* Core system bring-up:

  * heap allocator
  * scheduler bootstrap
  * interrupt system
  * first kernel thread
* Spawn `init` userspace process

Document:

* boot invariants
* expected registers at entry
* memory layout assumptions

---

#### **H2. Architecture-Specific Boot Stubs**

Per-arch responsibilities:

**x86-64**

* Long mode transition
* GDT/IDT bootstrap
* APIC discovery
* Early identity mapping

**ARM64**

* EL2/EL1 entry configuration
* Exception vector install
* TTBR page-table setup

**RISC-V**

* SATP MMU activation
* SBI or bare-metal mode support
* Trap vector initialization

Deliverables:

* `arch/*/boot/entry.S`
* `arch/*/boot/early_init.rs`
* QEMU bootable images

Tests:

* boot trace log
* early memory probe validation
* panic-safe fallback checks

---

### **I. Interrupts, Timers, and Synchronization**

#### **I1. Cross-Arch Interrupt Abstraction**

Define traits:

```rust
trait InterruptController {
    fn register_handler(...);
    fn enable_irq(...);
    fn disable_irq(...);
    fn eoi(...);
}
```

Arch backends:

* x86-64 → APIC / IOAPIC
* ARM64 → GICv2/v3
* RISC-V → PLIC + local interrupts

Policies:

* Interrupt handlers cannot block
* Deferred work via scheduler queues
* Strict reentrancy rules

Validation:

* synthetic IRQ generator tests
* latency measurement harness

---

#### **I2. Timer Subsystem**

Core features:

* monotonic kernel clock
* per-CPU timers
* sleep / timeout integration
* scheduler tick hooks (optional tickless)

Arch backends:

* HPET / TSC (x86-64)
* ARM generic timer
* RISC-V timer CSR

Tests:

* drift consistency across CPUs
* timer storm resilience
* suspend/resume correctness (future)

---

#### **I3. Kernel Synchronization Primitives**

Implement:

* spinlocks (arch-safe)
* mutex (sleepable)
* rwlocks
* wait queues
* atomic reference counted handles

Rules:

* lock-ordering policy
* lock-dependency tracking in debug builds
* lock misuse detector

---

### **J. IPC & Capability Model Deep-Implementation**

#### **J1. Channel Messaging System**

Features:

* bi-directional message passing
* attach handles + VMOs
* bounded message sizes
* deterministic delivery semantics

Security:

* no implicit sharing
* handle rights validated at send/recv
* message queue backpressure rules

Tests:

* round-trip latency benchmark
* panic-safe failure paths
* fuzzing of malformed payloads

---

#### **J2. Wait / Event / Port Model**

Provide primitives:

* `Event`, `EventPair`
* `WaitOn(object, signals…)`
* `Port` for async object notification

Use cases:

* thread blocking
* I/O completion queues
* process lifecycle events

---

### **K. Process Model & Userspace Runtime**

#### **K1. Process & Address Space Lifecycle**

Implement:

* process creation from VMO executable image
* handle table initialization
* argument / stack bootstrap
* process termination policy

Security:

* no implicit global state
* capabilities define all authority
* deterministic resource cleanup

---

#### **K2. Minimal Userspace Runtime (Stage-0)**

Purpose:

* test kernel syscall layer
* bootstrap later libc/libsystem

Deliverables:

* tiny runtime in Rust or C
* channel, VMO, process syscalls
* basic loader stub

Target tools:

* QEMU user mode testing
* cross-arch parity validation

---

### **L. Early Driver Model & Platform Bus**

#### **L1. Driver Framework (Minimal First Pass)**

Principles:

* drivers are kernel objects
* attach to devices via bus enumeration
* no architecture-specific logic in drivers

Core drivers to implement:

* UART console
* platform bus
* interrupt routing glue
* RAM-disk block device
* basic framebuffer (optional)

Add async I/O model via ports.

---

#### **L2. Device Discovery Strategy**

Per-platform options:

* ACPI (x86-64 capable platforms)
* Device tree (ARM / RISC-V embedded)
* Static board description for early stage

Portable enumeration API for drivers.

---

### **M. Security, Safety, and Reliability Baselines**

#### **M1. Memory Safety Rules**

* `unsafe` only in arch + HAL layers
* every unsafe block documented
* build mode with runtime guards
* slab allocators with canaries in debug mode

#### **M2. Capability Integrity**

* no ambient authority
* handles must be unforgeable
* all syscalls validate rights + lifetimes

#### **M3. Crash & Diagnostics**

* structured crash dumps
* kernel tracing ring buffer
* deterministic panic handling
* cross-arch symbol resolver tools

---

### **N. Testing, Simulation, + Continuous Validation**

#### **N1. Multi-Architecture CI Matrix**

For each commit:

* build all 3 architectures
* run unit tests
* run QEMU boot smoke tests
* run syscall parity tests

#### **N2. Determinism & Cross-Arch Behavioral Tests**

Create test suites that:

* run identical syscalls
* compare outcomes across archs
* fail if semantics diverge

#### **N3. Performance Baseline Benchmarks**

Measure:

* syscall latency
* context switch time
* page-fault handling cost
* IPC round-trip

Same workload, all architectures.

---

### **O. Roadmap for Phase-2 Expansion (After MVP Kernel)**

Once boot + userspace + IPC + VM + scheduling are stable:

* Filesystem layer (object-based)
* Networking stack
* Storage drivers (NVMe, virtio-blk)
* User-space driver model (optional)
* SMP scaling & NUMA awareness
* Capability sandboxed services
* POSIX-compatibility shim (optional layer)

---

## **P. Phase-1 Implementation: Boot + Early MMU + Kernel Core**

### **P1. Repository + Workspace Layout (Executable Project Skeleton)**

Create a Cargo workspace with clean separation between
portable core, architecture backends, and platform layers.

```
/kernel
  /arch
    /x86_64
    /arm64
    /riscv64
  /core
  /mm
  /sched
  /sys
  /ipc
  /hal
  /drivers
  /platform
  /tools
```

**Workspace `Cargo.toml`**

* `kernel-core` → arch-agnostic logic
* `kernel-arch-*` → per-arch crates
* `kernel-bin` → final architecture-linked image
* Enforce `#![forbid(unsafe_op_in_unsafe_fn)]`

Add `xtask/` utility crate for:

* image building
* QEMU boot automation
* test orchestration

---

### **P2. Common Kernel Entry Contract**

Define a **single shared Rust entry point**:

```rust
pub fn kernel_main(boot_info: BootInfo) -> ! {
    init_heap();
    init_logging();
    mm::init(boot_info.memory_map);
    sched::bootstrap();
    platform::init_devices();
    start_init_process();
}
```

Each architecture must guarantee:

* stack is valid
* minimal identity map is active
* interrupts disabled
* boot memory map provided

All divergence happens **before** this call.

---

### **P3. x86-64 Early Boot Scaffold (First Target Architecture)**

#### **P3.1 Early Boot Responsibilities**

* transition to long mode
* set up temporary identity page table
* initialize stack & BSS
* jump into Rust safe world

Artifacts:

* `arch/x86_64/boot/entry.S`
* `arch/x86_64/boot/early_init.rs`

Key invariants:

* identity-map kernel load region
* map physical memory used so far
* MMU must be active before calling Rust entry

#### **P3.2 Safe Wrapper Boundary**

Expose only a minimal Rust surface:

```rust
pub fn arch_early_init() -> BootInfo;
pub fn arch_enable_mmu(pt_root: PhysAddr);
pub fn arch_jump_to_kernel(entry: fn(BootInfo) -> !) -> !;
```

All unsafe details stay confined.

Add debug traces for:

* page table roots
* CR3 value
* early mapping coverage region

---

### **P4. Portable Early Memory Allocator (“Boot Alloc”)**

Goal: allow mapping + struct creation before full VM starts.

Features:

* bump allocator over boot-reserved region
* never reused after handoff
* debug-instrumented for leaks

Used by:

* early page table creation
* CPU + trap structures
* boot logs

Later replaced by full kernel heap.

---

### **P5. Portable MMU Interface + Arch Backends**

Define architecture-agnostic traits:

```rust
pub trait ArchPageTable {
    fn map(&mut self, virt: VAddr, phys: PAddr, flags: MapFlags);
    fn unmap(&mut self, virt: VAddr);
    fn resolve(&self, virt: VAddr) -> Option<PAddr>;
    fn flush_tlb(&self);
}
```

Backends implement:

* x86-64 → 4-level page tables
* ARM64 → TTBR + 4-level translation tables
* RISC-V → SV39/SV48 abstraction layer

Policy:

* portable API presents **N-level paging**
* architecture differences hidden

Tests:

* identity-map validation
* guard-page correctness
* double-map prevention
* cross-arch parity behavior

---

### **P6. Kernel Virtual Memory Layout (Unified Spec)**

Define a **fixed logical layout across architectures**:

* Lower half: user space
* Upper half: kernel space
* Guard pages around stacks and heaps
* Per-CPU regions
* Direct-map physical window (optional phase-2)

Documented as invariant contract.

This ensures:

> binaries differ per-arch,
> **behavior remains identical.**

---

### **P7. First Bring-Up Milestone (“Stage-0 Boot Success”)**

Target outcome:

* QEMU boots
* kernel enters `kernel_main()`
* initializes heap + logging
* prints boot banner via UART/console
* halts cleanly

Deliverables:

* deterministic boot log
* panic-safe fallback path
* minimal developer debugging tools

This is the **first measurable milestone**.

---

## **Q. Phase-1 Parallel Prep for ARM64 + RISC-V**

Once x86-64 scaffolding stabilizes:

#### **Q1. ARM64 Boot Port**

* EL1 entry rules
* GIC timer + interrupts stubbed
* page tables via TTBR0/1
* init vectors + exception frames

#### **Q2. RISC-V Boot Port**

* SBI or bare-metal mode toggles
* SATP MMU enable
* trap vector + scratch CSRs
* PLIC + timer stubs

Each must call the same:
`kernel_main(boot_info)`

and share VM / scheduler / syscall semantics.

---

## **R. Engineering Quality Gates for This Phase**

Before continuing to scheduler or syscalls:

* [ ] boots successfully on all 3 architectures in QEMU
* [ ] passes early memory unit tests
* [ ] identical panic semantics cross-arch
* [ ] MMU + guard-page behavior verified
* [ ] no `unsafe` outside sanctioned layers
* [ ] CI runs multi-arch smoke test automatically

Only when these pass do we proceed.

---

## **S. Trap, Exception, and Syscall Entry Model (Unified Cross-Arch Design)**

### **S1. Define a Single Portable Exception Frame Model**

Goal: every trap, interrupt, and syscall must produce a **standardized kernel frame object** visible to core code.

Design a canonical structure:

```rust
pub struct TrapFrame {
    pub regs: [u64; N_ARCH_REGS],   // canonical register set
    pub pc: u64,
    pub sp: u64,
    pub flags: u64,
    pub cause: TrapCause,
}
```

Where `TrapCause` is portable:

```rust
pub enum TrapCause {
    Syscall,
    PageFault { addr: u64, access: AccessType },
    IllegalInstruction,
    DivideByZero,
    Breakpoint,
    TimerInterrupt,
    ExternalInterrupt(u32),
    SoftwareInterrupt,
    Unknown,
}
```

Architecture backends perform:

* register save/restore
* architecture translation → portable `TrapCause`

Core kernel never sees raw CPU formats.

Deliverables:

* `arch/*/trap/frame.S`
* `arch/*/trap/translate.rs`

Tests:

* synthetic traps triggered and compared cross-arch
* frame round-trip restore validation

---

### **S2. Trap Handling Pipeline (Deterministic Order)**

Standard phases applied to every trap:

1. Save CPU state → TrapFrame
2. Classify cause
3. Route to handler table
4. Decide kernel-thread continuation policy:

   * resume thread
   * deliver signal/event to user
   * kill process
   * panic kernel (fatal only)

Provide central dispatcher:

```rust
pub fn handle_trap(tf: &mut TrapFrame) -> TrapAction;
```

Where:

```rust
pub enum TrapAction {
    Resume,      // return to user/kernel
    RetryFault,  // after demand-paging
    Terminate,   // user-process exit
    Panic,       // kernel fatal condition
}
```

---

## **T. Interrupt Subsystem Integration**

### **T1. Interrupt Dispatch Contract**

All architecture IRQs are normalized into:

```rust
pub struct IrqContext<'a> {
    pub tf: &'a mut TrapFrame,
    pub irq: u32,
}
```

Global registry:

```rust
type IrqHandler = fn(&mut IrqContext);

pub fn register_irq_handler(irq: u32, h: IrqHandler);
```

Rules:

* IRQ handlers must be **non-blocking**
* scheduling deferral happens via wait-queues / work queues
* nested IRQ reentrancy disabled by default

Backends:

* x86-64 → APIC/IOAPIC vector routing
* ARM64 → GICv2/v3
* RISC-V → PLIC + local timer/software IRQs

Tests:

* synthetic IRQ storm
* latency histogram capture
* handler misbehavior guard tests

---

### **T2. Timer Interrupt & Kernel Clock**

Timer source responsibilities:

* provide monotonic kernel timebase
* support per-CPU timers
* drive scheduler tick (or tickless idle)
* integrate with timeouts & sleeps

Portable interface:

```rust
pub trait ArchTimer {
    fn now_ns() -> u64;
    fn set_deadline_ns(t: u64);
    fn cancel();
}
```

Architecture mappings:

* x86-64 → TSC/HPET/APIC timer
* ARM64 → CNTVCT_EL0
* RISC-V → time CSR + timer compare

Validation:

* drift and skew tests across CPUs
* deadline accuracy tolerance bounds

---

## **U. Syscall Entry / Exit Semantics**

### **U1. Unified Syscall ABI Contract**

Requirement: **same syscall numbers, names, semantics across all architectures**.

Registers may differ per-arch — translation bridges normalize them.

Portable syscall dispatch:

```rust
pub fn syscall_dispatch(tf: &mut TrapFrame) -> SysResult;
```

Return protocol:

* success → return value in `r0`/`rax`/`a0`
* failure → negative errno code
* no exceptions for syscall error flow

Design early syscall core set:

* process/thread lifecycle
* VMO / memory objects
* channel IPC
* event/wait
* handle ops
* time & sleep

Documented as stable ABI.

---

### **U2. Syscall Safety & Capability Validation**

Before executing each syscall:

* validate handle
* validate rights mask
* validate user pointer ranges (if/when introduced)
* ensure no ambient authority exists

Misuse behaviors:

* invalid handle → `ERR_BAD_HANDLE`
* insufficient rights → `ERR_ACCESS_DENIED`
* undefined syscall → `ERR_NOT_SUPPORTED`

Tests:

* syscall fuzz harness
* cross-arch parity tests (identical results required)
* capability misuse regression cases

---

## **V. Fault Handling & Diagnostics Policy**

### **V1. Page Fault Handling Path**

Portable lifecycle:

1. classify fault:

   * user vs kernel
   * read/write/execute
2. attempt resolution:

   * demand-paging / VMO mapping (future phase)
   * guard-page hit → deterministic kill
3. escalate decision:

   * user fault → terminate process w/ reason object
   * kernel fault → panic with structured crash dump

Add configurable debug mode:

* kernel stack trace
* symbolized PC addresses
* dump of TrapFrame

---

### **V2. Breakpoint & Debug Trap Support**

Provide:

* software breakpoint trap
* single-step stub (future)
* kernel debug hooks
* optional GDB remote stub (later)

Must not diverge by architecture in semantics.

---

## **W. Quality Gates Before Scheduling Layer**

Before implementing threads & scheduler:

* [ ] traps validated on all architectures
* [ ] timer & IRQ round-trip tests pass
* [ ] syscall ABI parity verified cross-arch
* [ ] fault behavior identical across platforms
* [ ] panic diagnostics stable & reproducible
* [ ] CI runs trap tests in QEMU for all three targets

Only when this layer stabilizes should we proceed.

---

## **X. Threading Model & Kernel Thread Objects**

### **X1. Define Core Thread Abstractions**

Primary kernel objects:

* `Process`
* `Thread`
* `AddressSpace`
* `HandleTable`

Thread lifecycle states:

```rust
pub enum ThreadState {
    New,
    Ready,
    Running,
    Blocked,
    Sleeping,
    Dying,
    Dead,
}
```

Thread attributes:

* priority (integer, fixed + RT classes later)
* CPU affinity mask
* scheduling class
* handle table reference (for user threads)
* reference-counted kernel object backing

Rust struct skeleton:

```rust
pub struct Thread {
    pub tid: ThreadId,
    pub state: ThreadState,
    pub priority: u8,
    pub cpu_affinity: CpuMask,
    pub context: ArchThreadContext,
    pub stack: KernelStack,
    pub process: Arc<Process>,
}
```

Rules:

* **no implicit blocking**
* blocking must happen through wait-queues
* thread destruction requires deterministic teardown

---

### **X2. Kernel Stack Model**

Policy (portable across architectures):

* dedicated kernel stack per thread
* guard page at base
* stacks allocated via VM subsystem
* interrupt entry switches to per-CPU interrupt stack

Stack invariants are documented and tested.

---

## **Y. Context Switching (Arch-Portable API + Arch Backends)**

### **Y1. Portable Context Interface**

Trait implemented by each architecture:

```rust
pub trait ArchThreadContext {
    fn init_user(entry: u64, stack: u64, aspace: &AddressSpace) -> Self;
    fn init_kernel(entry: fn() -> !, stack: &KernelStack) -> Self;
    fn save_current(tf: &TrapFrame, current: &mut Self);
    fn switch_to(next: &Self) -> !;
}
```

Responsibilities:

* register save/restore
* PC / SP / flags preservation
* FPU/SIMD lazy-save policy
* per-arch trap frame compatibility

Tests:

* round-trip state preservation
* high-frequency switch stress
* cross-arch deterministic semantics

---

### **Y2. First Runnable Threads**

Bootstrap sequence:

1. Create **idle thread** per CPU
2. Create **bootstrap kernel thread**
3. Migrate from boot context into real scheduler context
4. Enable preemption

Deliverable milestone:

> The kernel runs real threads instead of executing in boot context.

---

## **Z. Scheduler Design & Bootstrap**

### **Z1. Scheduling Policy (Phase-1)**

Initial policy:

* preemptive, priority-based
* per-CPU run queue
* simple O(1) ready queues
* no fairness decay (added later)
* optional affinity enforcement

Core operations:

```rust
fn enqueue(thread: Arc<Thread>);
fn dequeue(thread: &Thread);
fn pick_next(cpu: CpuId) -> Arc<Thread>;
```

Scheduler entry points:

* `sched::on_timer_tick()`
* `sched::yield_current()`
* `sched::wake(thread)`
* `sched::block(wait_queue)`

---

### **Z2. Wait Queues & Blocking Primitives**

Portable wait-queue API:

```rust
pub struct WaitQueue { ... }

impl WaitQueue {
    pub fn block_current(&self, reason: BlockReason);
    pub fn wake_one(&self);
    pub fn wake_all(&self);
}
```

Rules:

* blocking always explicitly declared
* IRQ context may only queue deferred wakeups
* wakeups never directly context-switch in IRQ path

---

### **Z3. Timer-Integrated Sleep**

Syscall path:

* `sleep_until(deadline)`
* scheduler converts to per-CPU timer event
* wake enters ready queue

Tests:

* long wait accuracy
* cancellation behavior
* stress w/ frequent deadline churn

---

### **Z4. Multiprocessor Support (Phase-1 Bring-Up)**

Per-CPU structures:

* run queue
* current thread pointer
* idle thread
* CPU-local timer state

Cross-CPU operations:

* IPI for remote preemption requests
* TLB shootdown hooks reserved (future paging integration)

Initial SMP milestone:

* kernel boots with ≥2 virtual CPUs in QEMU
* threads schedule independently per CPU
* preemption validated under load

---

## **AA. First User-Mode Thread Launch**

### **AA1. Minimal Userspace Bring-Up Path**

Process creation syscall:

* create process object
* create address space
* map program image (stub phase: test stub)
* create main thread
* start thread in user mode

Transition requires:

* validated TrapFrame for user entry
* context switch into user mode
* syscall return path correctness

Deliverables:

* “hello-syscall-world” test process
* runs on all 3 architectures
* prints via IPC to kernel debug console or channel

---

## **AB. Quality Gates Before IPC + VM Expansion**

Proceed only after:

* [ ] context switching stable across architectures
* [ ] scheduler correctness fuzz tests pass
* [ ] blocking / wake semantics verified
* [ ] SMP boot works reliably
* [ ] first user thread runs on all targets
* [ ] CI runs multi-arch scheduler + threading tests

---

### **D. Kernel Core Services (continued)**

#### **D4. Capability & Handle System (Security + Object Access Control)**

**Goals**

* Enforce fine-grained access control to kernel objects
* Preserve Zircon-style handle semantics
* Provide per-process capability tables
* Ensure deterministic, portable behavior across architectures

**Design Requirements**

1. Handles are unforgeable references managed by the kernel
2. Capability rights are immutable unless explicitly modified via syscalls
3. Handle duplication and transfer semantics remain identical on all architectures
4. No userland pointer exposure — handles are opaque integers
5. Reference lifecycle always tied to object lifetime

**Implementation Plan**

* Implement `HandleId` as a 64-bit value (per-process namespace)
* Handle metadata stored in per-process capability tables
* Rights expressed as bitmasks
* Reference counting tied to KernelObject traits
* Enforce revocation semantics (closing the last handle destroys the object)

**Rust Design Sketch (Conceptual, not implementation code)**

* Define trait `KernelObject` with standardized lifecycle hooks
* Define `Handle` wrapper containing object pointer + rights mask
* Use Rust strong typing + borrowing rules to prevent accidental aliasing
* Enforce handle transfer rules via syscall boundaries

**Architecture-Independence**

* Capability enforcement lives in core kernel
* No arch conditionals allowed inside capability logic
* Makes security behavior deterministic across CPU flavors

---

#### **D5. Scheduler & Threading Model**

**Goals**

* Provide fully preemptive, priority-based scheduling
* Support identical scheduling semantics across architectures
* Abstract CPU-specific interrupt/timer differences cleanly

**Scheduling Requirements**

* Threads own:

  * Kernel stack
  * Register state (per-arch struct)
  * Scheduling priority + policy
* Processes own:

  * Handle table
  * VM address space
* CPU affinity & SMP support mandatory

**Scheduling Policies**

* Default:

  * Priority-based round-robin with time slices
* Future Extensions:

  * EDF (optional)
  * Real-time class hooks
* Deterministic starvation protection across platforms

**Implementation Plan**

* Define `ThreadStateArch` variants via trait binding
* Store scheduler metadata in arch-agnostic structs
* Clock tick handled via per-arch timer interrupt -> unified scheduler hook
* Include per-CPU run queues with work-stealing support

**Architectural boundary rule**

> Only context-save/restore code differs between architectures — policies and behavior must not.

---

#### **D6. Inter-Process Communication (IPC) Primitives**

**Goals**

* Preserve Zircon-style message-passing architecture
* Make IPC the foundation for subsystem composability
* Ensure IPC behavior remains consistent across architectures

**Core IPC Objects**

* Channels
* Event pairs
* Ports
* Futexes (strictly portable semantics)
* Signals

**Design Requirements**

* Messages travel via bounded queues with ownership semantics
* Handles may optionally transfer ownership between processes
* Timeouts behave identically across architectures
* Use of shared memory requires explicit VM object mapping

**Implementation Plan**

* Base IPC primitives implemented in core kernel
* Architecture only affects low-level atomic & futex primitives
* Provide optional zero-copy message support where safe
* Enforce signal masking rules uniformly

**Security Constraints**

* Handle transfer requires explicit rights
* No raw pointer sharing across processes
* Message boundaries always explicit & validated

---

### **E. Platform Abstraction Layers (Ensuring Portability)**

To ensure one OS design works across architectures, the kernel must enforce a strict split between:

* **Portable Core**
* **Architecture-Specific Layers**

#### **E1. Architecture Abstraction Boundary**

**Define mandatory separation layers**

1. **Arch-Independent Kernel**

   * Scheduler policy
   * Handle & capability system
   * IPC system
   * Process & thread management
   * VMM rules & semantics
   * Syscall object model

2. **Arch-Specific Layer**

   * Boot sequence
   * Page table layout
   * Trap/interrupt entry
   * Context switching
   * Timer primitives
   * Hardware MMU setup

**Rule**

> No architecture-dependent logic is allowed to bleed into core services.

This ensures binary differences between platforms do not affect OS semantics.

---

#### **E2. HAL (Hardware Abstraction Layer)**

**Goals**

* Provide a thin but explicit HAL contract
* Prevent per-architecture drift
* Allow future portability (POWER, LoongArch, etc.)

**HAL Responsibilities**

* CPU enumeration
* Timer primitives
* Interrupt controller interface (APIC/GIC/CLINT)
* Atomic & memory barrier wrappers
* Cache/TLB maintenance hooks

**Design Strategy**

* HAL exposed through traits + function tables
* Implementations per-arch
* Core never references CPU instructions directly

This preserves the invariant system behavior across platforms while still allowing hardware optimization internally.

---

### **F. Virtual Memory System Architecture**

The virtual memory subsystem is one of the most architecture-sensitive parts of the kernel — but our goal is to ensure **identical semantics and behavior across ARM64, x86-64, and RISC-V**, even if the underlying MMU implementations differ.

To achieve that, we define:

* A **portable VM object model**
* A **strict abstraction boundary between VM policy and page-table mechanics**
* Architecture modules that only implement *translation details*, not behavior.

---

#### **F1. VM Design Goals**

1. **Uniform semantics across architectures**

   * Same rules for mapping, permissions, sharing, COW, paging, etc.
   * Same syscall behavior and error conditions.

2. **Object-based memory management**

   * Memory represented as **VM Objects (VMOs)**
   * Processes map VMOs into address spaces

3. **Deterministic behavior**

   * No implicit over-commit without policy choice
   * Same alignment and rounding rules everywhere

4. **Explicit operations**

   * Mapping, unmapping, protecting, committing pages, zeroing, duplication
   * No hidden mappings or kernel-magic ownership changes

---

#### **F2. Core VM Concepts (Architecture-Independent)**

##### **Key Kernel Objects**

* **VMO (Virtual Memory Object)**

  * Backing store abstraction (anonymous memory, file-backed, device memory)
  * Supports:

    * Copy-on-write
    * Snapshotting
    * Resizing
    * Page commitment management

* **Address Space (Process VM Root)**

  * Per-process virtual address map
  * Contains a tree of regions

* **VMAR (Virtual Memory Address Region)**

  * Defines *where* and *how* VMOs may be mapped
  * Supports:

    * Sub-regions
    * Guard regions
    * Permission inheritance
    * Fixed vs flexible placement

**Rule:** All mapping policy lives in VMO/VMAR — **not** in arch code.

---

#### **F3. Page Management Semantics (Portable Rules)**

Across all architectures:

* Page size: **4 KiB base page** (with optional large page support)
* Mapping permissions:

  * `READ`, `WRITE`, `EXECUTE`
* Write-xor-execute enforced kernel-wide
* User vs kernel separation strictly enforced
* Memory barriers & coherence semantics standardized

##### **Copy-on-Write Behavior**

* First write triggers page fault
* Kernel allocates new page
* Updates mapping reference
* Behavior must be identical on all architectures

##### **Shared Memory Semantics**

* Two mappings of same VMO page must see identical bytes
* Synchronization via IPC/futex — never implied side effects

---

#### **F4. Architecture Abstraction Boundary for VM**

We enforce a **two-layer VM design**:

##### **Portable Layer (Core VM Logic)**

Implements:

* Page allocation & accounting
* VMO tree & snapshot rules
* Region placement & conflict checks
* Mapping lifecycle
* Permission checks
* Fault classification & resolution flow

##### **Architecture Layer (MMU Backend)**

Responsible only for:

* Page-table format & layout
* TLB shootdown & flush operations
* Address translation mechanisms
* Attribute encoding (UXN, PXN, PAT, etc.)

**Rule:**
The portable VM layer decides *what should happen*.
The arch layer only handles *how to encode it in hardware*.

---

#### **F5. Fault Handling Pipeline (Cross-Arch Invariant)**

1. CPU raises page fault trap
2. Arch layer decodes trap → produces a canonical fault event:

   * Address
   * Access type (read/write/exec)
   * User/kernel mode
3. Core VM logic:

   * Resolves mapping
   * Applies COW if needed
   * Validates permissions
   * Returns standardized result
4. Arch layer installs hardware mapping
5. Thread resumes

This keeps **fault semantics identical** on all platforms.

---

#### **F6. Memory Attributes & Caching Policy**

Portable policy definitions:

* `NORMAL`
* `DEVICE`
* `UNCACHED`
* `WRITE_COMBINING`

Architecture layer maps these to hardware encodings:

* ARM64 → MAIR, TCR, PXN/UXN flags
* x86-64 → PAT, PCD, PWT, NX bit
* RISC-V → Svpbmt / memory types

**Policy stability is guaranteed**, even if hardware encodings change.

---

#### **F7. Large Pages & Performance Extensions**

Support is **optional per-platform**, but rules are unified:

* Transparent huge pages are **never automatic**
* Large pages require explicit mapping policy
* If not supported by arch → syscall fails consistently

This ensures predictable, portable behavior.

---

### **G. Boot & Early Bring-Up (Per Architecture)**

Next we define:

* Early boot flow
* CPU initialization
* Memory discovery
* Transition into the shared kernel

Planned subsections:

* **G1. Common Boot Goals & Constraints**
* **G2. ARM64 Bring-Up Path (UEFI/GIC/EL2 options)**
* **G3. x86-64 Bring-Up Path (UEFI/APIC/SMEP/SMAP)**
* **G4. RISC-V Bring-Up Path (S-Mode, SBI, CLINT)**
* **G5. Unified Entry to Portable Kernel Runtime**

---

### **G. Boot & Early Bring-Up (Per Architecture)**

The goal of the boot process is to:

* Initialize the minimum required hardware context
* Establish a temporary early memory allocator
* Bring up one bootstrap CPU to kernel entry
* Enumerate additional CPUs and start them
* Transition cleanly into **architecture-independent kernel code**

All architectures must converge into the same **portable kernel entry contract**.

---

#### **G1. Common Boot Goals & Constraints**

**Invariant Boot Contract (Required for All Architectures)**

When the portable kernel entry function runs:

* Paging is enabled
* Kernel is executing in the **highest available privilege level**
* The kernel has:

  * A linear mapping of physical memory (boot-time only)
  * A stack for the current CPU
* Interrupts are disabled
* A minimal platform descriptor is available containing:

  * Memory map
  * CPU/cores list (or discoverable API)
  * Timer source
  * Bootloader-provided kernel image metadata

**Boot-Time Rules**

* No syscalls, no userland visible behavior
* No architectural policy decisions in portable code
* Boot differences must be isolated in `arch/*/boot/`

---

### **G2. ARM64 Bring-Up Path**

**Boot Environments Supported**

* UEFI (preferred)
* Bootloaders such as TF-A + U-Boot
* EL2 optional; primary mode: **EL1 with MMU enabled**

**ARM64 Boot Steps**

1. **Reset Entry**

   * CPU starts in EL3/EL2/EL1 depending on platform
   * If above EL1:

     * Drop to EL1 with correct exception vectors

2. **Paging & Memory Setup**

   * Establish identity-mapped bootstrap page tables
   * Enable MMU & caches
   * Map kernel virtual region

3. **Stack + CPU Context Init**

   * Allocate per-CPU bootstrap stack
   * Initialize early CPU data structure

4. **Timer + GIC Minimal Init**

   * Initialize architectural timer
   * Configure GIC distributor + CPU interface (mask interrupts)

5. **Secondary CPU Holding Pen**

   * Secondary cores spin waiting for release signal

6. **Jump to Portable Kernel Entry**

   * Pass platform descriptor + CPU id

**Later SMP Bring-Up**

* Release secondary CPUs one-by-one
* Each CPU repeats context + stack init
* Joins scheduler CPU list

---

### **G3. x86-64 Bring-Up Path**

**Boot Environment**

* UEFI (preferred, replaces legacy BIOS)
* Long mode required before entry

**x86-64 Boot Steps**

1. **Early Real-Mode → Long-Mode Transition (Bootloader)**

   * Bootloader loads kernel and enters 64-bit mode
   * Kernel never handles real-mode

2. **Bootstrap Processor (BSP) Entry**

   * BSP starts at kernel entry point
   * Interrupts disabled

3. **Paging & Memory Setup**

   * Create temporary identity PML4
   * Map kernel image + linear memory window
   * Enable CR0/CR4/Paging flags

4. **Stack & CPU Data Init**

   * Initialize BSP per-CPU structure
   * Load GS-base for CPU-local storage (if used)

5. **APIC & Timer Minimal Init**

   * Mask LAPIC interrupts
   * Initialize TSC + LAPIC timer backend

6. **Enter Portable Kernel Runtime**

   * Provide ACPI/UEFI memory + CPU enumeration hooks

**Secondary CPU Bring-Up**

* BSP sends INIT/SIPI signals
* APs execute AP trampoline
* APs initialize stack + CPU data
* APs join scheduler

---

### **G4. RISC-V Bring-Up Path**

**Boot Context**

* Bootloader enters **S-mode**
* SBI (Supervisor Binary Interface) used for:

  * Console
  * Timer
  * IPI
  * Basic power functions

**RISC-V Boot Steps**

1. **Reset Entry**

   * Execution begins on Hart0 in S-mode
   * Interrupts disabled

2. **Paging Initialization**

   * Create Sv39 bootstrap tables (Sv48 optional later)
   * Map kernel region + temporary physical mapping
   * Enable `satp`

3. **Stack & Hart Local State**

   * Allocate per-hart bootstrap stack
   * Initialize hart data structure

4. **Timer + IPI Prep**

   * Configure SBI timer fence
   * Disable interrupt delivery until scheduler ready

5. **Secondary Hart Parking**

   * Other harts spin in SBI wait loop

6. **Enter Portable Kernel Runtime**

   * Pass hart-id + platform descriptor

**SMP Bring-Up**

* Primary hart signals SBI hart-start
* Each hart repeats init sequence
* Registers with scheduler

---

### **G5. Unified Entry to Portable Kernel Runtime**

Once any bootstrap CPU finishes architecture-specific work, it calls:

> `kernel_main(platform_descriptor, cpu_id)`

At this point, execution becomes **architecture-agnostic**.

**Portable Startup Sequence**

1. Initialize global allocator + slab caches
2. Initialize kernel object system
3. Bring up root scheduler + idle thread
4. Initialize VM subsystem full mode
5. Initialize IPC primitives
6. Initialize process manager
7. Start first userland bootstrap process

**Secondary CPUs**

* Enter via `kernel_secondary_init(cpu_id)`
* Register with scheduler
* Begin accepting runnable threads

---

### **H. Syscall Layer & Object Model Specification**

The syscall and object model form the foundation of API compatibility. The rule is:

> **Architectures may differ in ABI details, but syscall semantics and object behavior must be identical.**

To guarantee this, we define:

* A **stable handle-based object system**
* A **small, orthogonal syscall surface**
* Deterministic error semantics
* Explicit actions — no hidden side effects
* A compatibility commitment across kernel builds

---

#### **H1. Syscall Design Principles**

1. **Object-centric rather than POSIX-style**

   * Processes interact through **handles to kernel objects**
   * No global FD namespace

2. **Explicit Resource Ownership**

   * Handle rights define capabilities
   * Creation, duplication, transfer must be explicit

3. **Deterministic & Portable**

   * Same inputs → same results → same errors
   * No architecture-dependent branching in semantics

4. **Minimal but Composable API**

   * Fewer, orthogonal syscalls instead of special-case APIs

5. **Forward-Compatible**

   * New objects or rights may be added
   * Existing behavior may not change

---

#### **H2. Syscall Calling Convention Policy**

* **User ABI differs per architecture**, but:

  * Argument ordering
  * Width rules
  * Return conventions
  * Error mapping
    are normalized inside the syscall layer.

* Syscalls enter via:

  * ARM64: `svc`
  * x86-64: `syscall/sysenter`
  * RISC-V: `ecall`

Each arch stub converts registers → **canonical syscall frame**, then dispatches to shared logic.

---

#### **H3. Canonical Syscall Object Model**

Each kernel object implements:

* **Type identifier**
* **Rights mask**
* **Signal set**
* **Reference lifecycle**

Core object classes:

* Process
* Thread
* VMO
* VMAR
* Channel
* Event / Event Pair
* Port / Waitset
* Socket / Datagram endpoint (optional)
* Timer
* Job (hierarchical process grouping)

**Rule:**
All objects support a uniform baseline of operations:

* `create`
* `duplicate`
* `transfer`
* `close`
* `signal`
* `query-info`

Specialized ops exist per-object but must follow the same capability semantics.

---

#### **H4. Error & Status Semantics (Cross-Arch Invariant)**

All syscalls return:

* Success → zero
* Failure → negative error code from a **stable table**

Examples:

* `INVALID_ARGS`
* `ACCESS_DENIED`
* `NOT_SUPPORTED`
* `NO_MEMORY`
* `TIMED_OUT`
* `SHOULD_WAIT`

**Rules**

* No errno-style ambient state
* No architecture-specific error codes
* Timeouts must behave identically across platforms

---

#### **H5. Blocking, Cancellation, and Signals**

* Blocking syscalls must support:

  * Deadlock-safe wake semantics
  * Optional deadline timeouts
* Cancellation points trigger via:

  * Thread kill
  * Handle closure
* Signals are:

  * Edge-triggered
  * Explicitly consumed by wait operations

This keeps concurrency behavior portable and predictable.

---

#### **H6. Syscall Catalog (Initial Set)**

##### **Process & Thread**

* `process_create`
* `process_start`
* `thread_create`
* `thread_start`
* `task_kill`
* `task_get_info`
* `task_suspend` (optional)

##### **Memory / VMO / VMAR**

* `vmo_create`
* `vmo_read`
* `vmo_write`
* `vmar_map`
* `vmar_unmap`
* `vmar_protect`
* `vmo_clone` (COW)
* `vmo_resize`

##### **IPC**

* `channel_create`
* `channel_write`
* `channel_read`
* `event_create`
* `eventpair_create`
* `port_create`
* `port_wait`

##### **Synchronization & Timing**

* `futex_wait`
* `futex_wake`
* `timer_create`
* `timer_set`
* `timer_cancel`
* `clock_get`

##### **Handle & Capability Ops**

* `handle_duplicate`
* `handle_replace`
* `object_signal`
* `object_get_info`

**All semantics are architecture-independent.**

---

#### **H7. ABI Stability & Versioning**

To ensure long-term compatibility:

* Syscalls are identified by **stable numeric IDs**
* Removing or altering behavior is forbidden
* Deprecations require:

  * Capability flags
  * Opt-in semantics
* Userland libraries abstract per-arch ABI differences

A published **Syscall Specification Document** accompanies each release.

---

### **I. Driver Framework & Device Model**

The driver model must support:

* Hardware diversity across platforms
* Strict security and isolation guarantees
* Stable behavior across architectures
* Predictable lifecycle and power-state transitions
* The ability for user-mode drivers where appropriate

The guiding principle:

> **Drivers integrate through a stable device-object and bus-framework API — never by reaching into kernel internals or architecture details.**

---

#### **I1. Driver Execution Model**

We adopt a **hybrid kernel + user-mode driver architecture**:

* **Kernel-mode drivers** only for:

  * Interrupt controllers
  * Timers / schedulers
  * Early boot console
  * Physical memory + MMU–critical devices
  * Storage block root
* **User-mode drivers by default** for:

  * USB
  * Networking
  * Filesystems
  * Display / GPU stacks
  * HID / audio / sensors
  * Platform services

**Benefits**

* Fault isolation
* Easier debugging
* Cross-arch portability
* Lower kernel attack surface

User-mode drivers communicate with the kernel through:

* **device objects**
* **interrupt and IO port abstractions**
* **shared memory VMOs**
* **channels for RPC**

---

#### **I2. Canonical Device Object Model**

Every device is represented by a **Device Object (DevObj)** with:

* Type & class (block, net, HID, GPU, bus, virtual, etc.)
* Parent device (bus / controller relationship)
* Driver binding rules
* Lifetime & power state
* Set of published operations / properties
* Reference-counted handle ownership

Device objects support:

* `bind_driver`
* `add_child`
* `remove`
* `suspend`
* `resume`
* `ioctl / method dispatch`
* `get_properties`

Architecture-specific devices (APIC, GIC, SBI timer, etc.)
are wrapped behind **portable abstractions**.

---

#### **I3. Bus & Controller Frameworks**

To ensure portability, each bus framework defines a **stable, cross-arch contract**, while low-level register & MMIO handling lives in the device-specific backend.

Initial supported frameworks:

* **Platform bus (SoC / ACPI / firmware-enumerated devices)**
* **PCIe bus framework**
* **VirtIO framework**
* **USB host framework (user-mode first)**
* **Simple I²C / SPI transport abstractions**

##### **Bus Responsibilities**

* Device discovery / enumeration
* Resource assignment
* Interrupt routing abstraction
* Power domain awareness
* Binding compatible drivers

##### **Driver Responsibilities**

* Program device registers via MMIO VMOs
* Respond to interrupts using kernel-provided handles
* Implement published device protocol

---

#### **I4. Interrupt & DMA Abstraction**

To maintain cross-arch compatibility:

##### **Interrupts**

* Drivers receive **interrupt objects**, not hardware vectors
* Architecture layer maps:

  * ARM64 → GIC
  * x86-64 → APIC/MSI/MSI-X
  * RISC-V → APLIC/IMSIC/SBI
* Delivery semantics are identical:

  * Level vs edge-triggered
  * Mask/unmask operations
  * Wait-on-signal dispatch model

##### **DMA & Memory Coherency**

Portable semantics:

* Driver requests DMA buffers as VMOs
* Kernel provides:

  * Physical address windows
  * Cache-coherency rules
* Architecture backend handles:

  * IOMMU mappings
  * Cache flush / invalidate barriers

Drivers do **not** perform arch-specific cache ops.

---

#### **I5. Driver Lifecycle & Power Management**

Every driver follows a deterministic lifecycle:

1. **Probe / Bind**
2. **Init**
3. **Start / Online**
4. **Suspend (runtime or system)**
5. **Resume**
6. **Remove / Unbind**

Power states are portable:

* `D0` active
* `D1/D2` low-power (optional)
* `D3` off / quiesced

Policy lives in the **device framework**, not in drivers.

---

#### **I6. Driver Communication Patterns**

Supported IPC & data exchange patterns:

* **RPC via channels** (control operations)
* **Shared VMOs** (queues, rings, buffers)
* **Signal-driven eventing** (interrupts, completion)
* **Ports / waitsets** for asynchronous batching

This enables:

* VirtIO-style queues
* Network ring buffers
* USB transfer scheduling
* Modern, lock-minimal concurrency

---

#### **I7. Driver Portability Rules**

To guarantee cross-architecture growth:

* Drivers may not read CPU ID registers directly
* Drivers may not assume page-table layouts
* No inline assembly except through approved HAL hooks
* Endianness and alignment rules must be explicit
* Timing must use kernel clocks — not TSC or cycle counters

Violations are caught in:

* static analysis
* cross-arch CI builds
* conformance test suites

---

### **J. Testing, Portability Validation & Performance Profiling**

The objective of this phase is to build a **rigorous, automation-first validation pipeline** that guarantees:

* Architectural behavior parity
* Stable syscall and object semantics
* Deterministic concurrency and timing behavior
* Regression-free performance characteristics
* Strong correctness guarantees before hardware deployment

We design testing as a **first-class subsystem** — not an afterthought.

---

#### **J1. Multi-Architecture CI Strategy**

All commits must pass automated builds and tests for:

* **ARM64**
* **x86-64**
* **RISC-V (S-mode, SBI-based)**

Build matrix includes:

* Debug kernel
* Release kernel
* Instrumented / sanitizers build (UBSan, KASAN-like where feasible)

**CI Environments**

* QEMU for fast emulation & deterministic behavior
* Hardware-in-the-loop runners for:

  * Selected ARM64 SBCs
  * RISC-V development boards
  * x86-64 reference platform

**Rule:**
A feature is not “done” until it passes **all architectures**.

---

#### **J2. Conformance Test Suites**

We define a formal **Kernel Conformance Test Suite (KCTS)** to validate:

##### **Object Model Semantics**

* Handle lifecycle
* Rights enforcement
* Duplication / transfer rules
* Signal edge semantics
* Blocking + timeout behavior

##### **Syscall Contract Tests**

* Argument validation
* Error code determinism
* Cross-process effects
* ABI alignment + structure layout

##### **VM & Memory Model**

* VMO cloning & COW correctness
* Mapping alignment & protection
* Page-fault behavior invariants
* Memory accounting tests

##### **Scheduling & Threading**

* Priority fairness
* Preemption model
* Futex edge cases
* Cancellation & teardown behaviors

**Goal:** guarantee **identical outcomes** across architectures.

---

#### **J3. SMP, Timing & Concurrency Validation**

To ensure deterministic behavior under load:

* Stress tests for:

  * Thread storms
  * IPC flood patterns
  * High-frequency timer workloads
* Randomized schedule fuzzing
* Priority inversion & futex race simulations
* Inter-CPU IPI ordering validation

Timing tests verify:

* Deadline semantics
* Monotonic clock drift bounds
* Timer wake precision

Results compared across platforms to flag divergence.

---

#### **J4. Driver & Hardware Abstraction Tests**

Each device framework includes:

* Fake / virtual device harnesses
* Loopback transport tests
* Interrupt delivery conformance checks
* DMA boundary-condition validation

Drivers must pass tests in:

* Emulated mode
* Real hardware mode (where available)

No driver may depend on architecture-specific side effects.

---

#### **J5. Security & Isolation Testing**

Security validation includes:

* Handle capability-escape tests
* Memory isolation probes
* Kernel object lifetime misuse fuzzing
* Privilege escalation attempts
* Syscall parameter mutation fuzzers
* UAF / race-targeted stress harnesses

Optional future integrations:

* Formal verification of selected primitives
* Model-checked object lifecycle logic

---

#### **J6. Performance Baseline & Regression Tracking**

We define **portable performance baselines**:

* Syscall latency microbenchmarks
* Channel / IPC throughput
* VMO clone + page-fault overhead
* Context-switch latency
* Timer & futex round-trip overhead
* Scheduler fairness metrics
* Driver I/O queue latency

Profiles must be:

* Comparable across architectures
* Repeatable under deterministic load
* Tracked over time via CI dashboards

**Rule:**
Performance improvements on one architecture must not break parity semantics on others.

---

#### **J7. Benchmark Methodology**

* Warm-cache and cold-cache passes
* Single-CPU and SMP scaling curves
* Power-aware modes (ARM SoCs especially)
* QEMU vs hardware deltas recorded

Instrumentation tools:

* Tracepoints in scheduler, IPC, VM
* Event-driven profiling buffers (user-accessible)
* Optional per-arch cycle counters abstracted behind a common API

---

#### **J8. Release Qualification Gates**

A release is approved only when:

1. **All architectural CI builds pass**
2. **Conformance suite reports zero semantic divergences**
3. **No correctness regressions**
4. **Performance regression budget respected**
5. **Driver framework conformance passes**
6. **Security harness shows no critical leaks or escapes**

Only then is the release promoted to **stable syscall compatibility**.

---

### **Section J — Virtual Memory Manager (VMM) & Pager Interface**

#### **J.1 — Architecture-Neutral VMM Abstractions**

Define a common **memory object model** that works identically across all supported architectures:

* `VmObject` (VMA-backed memory container)
* `VmMapping` (region mapped into a process address space)
* `AddressSpace` (per-process virtual address layout)
* `Pager` (optional external memory provider)

Design goals:

* Keep the *semantic contract identical* across architectures.
* Allow architecture layers to plug in MMU / page table implementations.
* Separate **policy (kernel object model)** from **mechanism (arch page tables)**.

Interfaces:

* `create_vmo(size)`
* `map_vmo(vmar, vmo, rights, flags, addr)`
* `protect(vmar, addr, size, rights)`
* `page_fault_handler(context)` — delegates to pager or zero-fill

---

#### **J.2 — Pager & Demand Paging Framework**

Provide optional pager support similar to Zircon's:

* Backing sources:

  * anonymous / zero-fill
  * file-backed (through userspace FS server later)
  * shared memory objects
* Paging behavior is **policy-free in kernel** — userspace controls policy.

Pager callbacks:

* `supply_pages(vmo, offset, pages)`
* `op_commit` / `op_decommit`
* `op_zero_range`
* `op_clone_cow` (copy-on-write cloning)

Kernel must:

* Provide deterministic paging semantics
* Prevent priority inversion and deadlocks
* Use per-VMO locks — avoid global memory locks

---

#### **J.3 — Memory Protection & Security Guarantees**

Mandatory guarantees:

* W^X enforcement (never writable + executable simultaneously)
* NX support where architecture allows
* Capability-gated mapping:

  * Rights separated for READ / WRITE / EXECUTE / MAP
* Guard pages around stacks / key VM ranges
* Deterministic handling of invalid memory access:

  * SIG-style exceptions replaced with object-based exception messages

---

### **Section K — Threading, Scheduling, & CPU Topology**

#### **K.1 — Scheduler Requirements**

Cross-architecture identical scheduling semantics:

* Priority-based, round-robin within priority class
* Optional real-time scheduling class
* Deterministic behavior under identical workload

Design constraints:

* No implicit over-commit assumptions
* Preemption points well-defined
* Avoid global run queue bottlenecks

Implementation:

* Per-CPU run queues
* Work stealing for load balancing
* Avoid architecture-specific optimizations leaking into policy

---

#### **K.2 — Thread Object Model**

Kernel thread object properties:

* handle-based
* belongs to a process
* has exception port
* has scheduling profile / priority
* owns TLS base (but architecture supplies hardware mechanism)

Operations:

* `thread_create(process)`
* `thread_start(entry, arg)`
* `thread_suspend / resume`
* `thread_set_profile`
* `thread_wait`

---

#### **K.3 — CPU & Core Abstraction Layer**

Expose consistent topology representation:

* cpu_id
* numa_node (optional / future-safe)
* core / SMT sibling awareness

Architecture plugin responsibilities:

* bring-up sequence per arch
* interrupt routing rules
* TLB shootdown mechanism
* architecture-specific idle loop

Kernel policy must remain architecture-neutral.

---

### **Section L — Inter-Process Communication (IPC) Model**

#### **L.1 — Channel Object Semantics**

Channels provide:

* bidirectional message passing
* optional handles embedded in messages
* capability transfer (move semantics only)
* fixed maximum message size policy

Behavioral guarantees:

* Deterministic delivery ordering
* Back-pressure (no infinite buffering)
* Deadlock-free send / receive path

---

#### **L.2 — Signals & Wait Objects**

Unified Wait API:

* wait on channels, threads, events, timers
* batch wait (wait_sets / ports)
* edge-triggered signaling model

Signals include:

* READABLE / WRITABLE
* PEER_CLOSED
* EXCEPTION_PENDING
* TIMER_FIRED

Must behave identically on all architectures.

---

### **Section M — Kernel Object Security & Capability Model**

#### **M.1 — Handle Rights Enforcement**

Each handle carries:

* object reference
* rights mask
* optional policy flags

Rights categories:

* DUPLICATE
* TRANSFER
* READ / WRITE
* EXECUTE / MAP (memory objects)
* SIGNAL / WAIT (events and channels)
* MANAGE (restricted)

Kernel requirements:

* enforce least-privilege
* prevent confused-deputy conditions
* block unauthorized mapping / execution

---

#### **M.2 — Namespace Isolation**

No global process IDs or global access.

All interactions require:

* valid handle
* appropriate capability rights

Processes cannot:

* enumerate global objects
* access any resource without explicit delegation

This ensures deterministic security behavior across architectures.

---

### **Section N — Platform Abstraction Boundaries (Final Kernel Layering)**

Layers must remain strict:

1. **Core Kernel (arch-agnostic)**
2. **Architecture Layer (MMU, traps, interrupts, cache ops)**
3. **Platform Layer (board / SoC / firmware glue)**
4. **Userland HAL Services (drivers in userspace later)**

No crossing upward shortcuts allowed.

All behaviors defined **in object model contracts**, not per-arch behavior.

---

### **Section O — Boot Process & Early Bring-Up**

#### **O.1 — Unified Boot Semantics**

Even though each architecture has different firmware and entry methods, the **post-bring-up state must converge** to the same kernel invariants:

Boot sequence contract:

1. Minimal architecture entry stub runs
2. Establish temporary identity-mapped page table
3. Initialize stack + per-CPU bootstrap structure
4. Transition into `arch_init_early()`
5. Hand off to `kernel_init_stage1()` (architecture-neutral)

Kernel invariant at transition:

* Paging enabled
* Interrupts disabled
* Single bootstrap CPU active
* Boot memory allocator available
* Kernel image mapped read-only + executable
* Data sections mapped appropriately (W^X preserved)

---

#### **O.2 — Per-Architecture Early Bring-Up Responsibilities**

Each architecture layer must implement:

* page-table bootstrap + mapping rules
* early trap / exception table installation
* CPU mode transition to privileged 64-bit mode
* cache / TLB sanity configuration
* architectural feature discovery (extensions, timers, etc.)

But these **must not influence policy behavior** — only capability exposure.

---

#### **O.3 — Secondary CPU Bring-Up**

SMP startup process:

* BSP initializes scheduler + run queues
* Secondary CPUs wait in low-power loop
* Kernel broadcasts startup signal
* Secondary enters `arch_cpu_init_secondary()`
* Joins scheduler under identical semantics

TLB shootdown, IPI routing, and topology handoff occur after all CPUs register.

---

### **Section P — Exception Handling & Debug Interfaces**

#### **P.1 — Unified Exception Object Model**

Instead of POSIX-style signals, exceptions become **observable kernel events** routed through:

* thread exception port
* process exception port
* debugger exception port

Exception classes must map consistently:

* page fault
* general protection / illegal instruction
* divide-by-zero / arithmetic fault
* syscall violations
* hardware breakpoint / single-step

Behavior must be deterministic across architectures:

* same ordering rules
* same resume semantics
* same rights model for attaching handlers

---

#### **P.2 — Debugger & Introspection Semantics**

Provide object-based debugging controls:

* suspend / resume threads
* read / write registers (abstracted field model)
* inspect address space mappings
* inject synthetic faults (capability-restricted)

Architectures supply register‐layout adapters, but **debug flow must not differ by CPU type**.

---

#### **P.3 — Fault Reporting & Policy Guarantees**

Design guarantees:

* all unhandled exceptions → process-kill via policy
* no silent traps
* exception metadata includes:

  * faulting address
  * instruction pointer
  * access type (read/write/exec)
  * architectural syndrome code (normalized)

---

### **Section Q — Timer, Clock & Timekeeping Model**

#### **Q.1 — Cross-Arch Time Abstraction**

Expose uniform kernel time primitives:

* monotonic clock
* boottime clock
* high-resolution timer API
* tickless scheduling behavior

Architecture layer provides:

* hardware clock source driver
* timer interrupt routing
* stable tick conversion units

Kernel time semantics must remain identical regardless of platform.

---

#### **Q.2 — Timer Object Semantics**

Timer object behavior:

* one-shot or repeating
* cancellation is explicit
* firing generates signal or wake event
* ordering guarantees for near-simultaneous expirations

Timers integrate with wait-sets and ports.

---

#### **Q.3 — Time Security & Drift Handling**

Requirements:

* monotonic clock must never step backward
* NTP / userspace sync affects wall clock only
* support for future TSC / counter skew correction
* no arch-specific timing quirks visible to userspace

---

### **Section R — Testing, Equivalence & Certification Harness**

#### **R.1 — Architecture Equivalence Tests**

Create a **conformance suite** that validates:

* syscall semantics
* scheduling behavior under workload
* paging + COW correctness
* exception and IPC ordering
* timer edge-case consistency

Tests must run on:

* ARM64 hardware / QEMU
* x86-64 hardware / QEMU
* RISC-V hardware / QEMU

Passing the suite = **behavioral equivalence certification**.

---

#### **R.2 — Kernel Micro-bench Harness**

Include:

* latency + throughput metrics for IPC, threads, paging
* deterministic replay tests
* regression baselines per release

Performance may differ per-architecture, **but semantics must not.**

---

#### **R.3 — Fuzzing & Formal Invariants**

* syscall contract fuzzing
* VM + scheduler invariants via model checking
* handle-capability misuse stress tests

Goal: prevent divergence and emergent undefined states.

---

### **Section S — Licensing, Governance & Growth Model**

#### **S.1 — Recommended License**

**MIT License** — selected to maximize:

* ecosystem growth
* commercial adoption
* research use
* permissive derivative development

Rationale:

* encourages driver and runtime stack participation
* avoids GPL contamination concerns
* compatible with academic and industry collaboration

Future governance option:

* require contribution sign-off (DCO)
* stewardship model with technical steering committee

---

#### **S.2 — Contribution & Architecture Policy**

Rules to preserve design integrity:

* no policy-leaking optimizations in arch layers
* semantic-change proposals require **arch-equivalence validation**
* every new feature must include:

  * spec update
  * cross-arch test
  * security review

---

### **Section T — Driver & I/O Architecture**

#### **T.1 — Userspace-First Driver Model**

Adopt a **microkernel-inspired, capability-scoped driver model**:

* Most drivers run in **userspace driver hosts**
* Drivers communicate via:

  * channels / ports
  * shared VMOs with ring buffers
* Kernel exposure limited to:

  * interrupts
  * MMIO mapping
  * DMA pinning primitives

Benefits:

* fault isolation
* improved reliability
* crash-containment per driver
* simplified security model

Kernel includes only:

* scheduler
* VM subsystem
* IPC layer
* minimal device dispatch veneer

---

#### **T.2 — Driver Framework Semantics**

Driver capabilities provided as **kernel objects**:

* `Interrupt`
* `PciDevice` / `MmioRegion`
* `DmaBuffer`
* `DeviceNode`
* `DriverHost`

All driver framework semantics must be **identical across architectures**, including:

* enumeration lifecycle
* power / suspend semantics
* hot-plug behavior
* error propagation rules

Architectures only differ in *how* device discovery occurs, never in *policy semantics*.

---

#### **T.3 — Supported Driver Classes (Initial Target Set)**

Stage-1 drivers:

* UART / serial console
* VirtIO block / net
* PCI root + basic enumeration
* Generic storage abstraction
* Timer + RTC sources
* Simple framebuffer / debug console

Stage-2 expansion:

* NVMe
* USB (host stack userspace)
* Network stack runtime in userspace
* GPIO / I2C / SPI for ARM SoCs

---

#### **T.4 — DMA, Memory Safety & Zero-Copy**

Rules:

* DMA buffers must be explicitly pinned
* zero-copy messaging via shared VMOs
* drivers may not map executable kernel memory
* W^X upheld at all times
* bounce buffers required for alignment-restricted hardware

These guarantees must remain identical per-architecture.

---

### **Section U — Userspace Runtime & ABI Stability**

#### **U.1 — Syscall ABI Freezing Policy**

* ABI evolves conservatively
* existing syscalls remain stable
* new syscalls use **object-semantic compatibility rules**
* architecture calling conventions differ — semantics do not

Each release documents:

* syscall table digest
* object model invariants
* rights & capability contract

---

#### **U.2 — Core Runtime Libraries**

Provide a **portable userspace foundation**:

* `libsys` — syscall + object wrappers
* `libipc` — channels, ports, waitsets
* `libvmo` — memory & paging abstractions
* `librt` — threads, futexes, timers
* libc-compat shim for POSIX portability layer (optional tier)

This ensures that **applications behave the same across all supported CPUs** when rebuilt.

---

#### **U.3 — Process Model & Launch Semantics**

Process startup contract:

* immutable VDSO mapping for fast syscalls
* initial handle table includes:

  * process handle
  * root namespace / job
  * stdio / debug ports as policy-controlled
* ASLR uniform semantics across architectures

---

### **Section V — Build, Toolchain & Cross-Compile Pipeline**

#### **V.1 — Toolchain Baseline**

* Rust stable + nightly for kernel features where necessary
* LLVM backend targeting:

  * `aarch64-unknown-none`
  * `x86_64-unknown-none`
  * `riscv64-unknown-none`

Every build output includes:

* kernel binary
* symbol + debug mapping
* conformance test image
* QEMU bootable image

---

#### **V.2 — Multi-Target Build Matrix**

Build modes:

* `debug`
* `release`
* `instrumented`
* `profile-guided` (later)

For each architecture:

* kernel core shared sources
* arch directories gated behind traits
* driver packages built per-arch where needed

Rule: **No architecture-specific code leaks into core modules**.

---

#### **V.3 — Reproducible Build Guarantee**

* hermetic toolchain containers
* build ID hashing
* deterministic linking
* symbol table reproducibility requirements

---

### **Section W — Documentation, Specs & Developer Workflow**

#### **W.1 — Spec-First Change Process**

Every subsystem maintains:

* high-level design document
* behavioral contract spec
* invariants & failure semantics
* cross-architecture equivalence notes

Changes require:

* RFC proposal
* test coverage plan
* compatibility impact review

---

#### **W.2 — Developer Workflow Standards**

* Rust style + safety gate enforcement
* forbid `unsafe` except in audited modules
* each unsafe block requires rationale comment
* architectural code paths isolated & reviewed separately

---

#### **W.3 — Public Documentation Set**

Deliverables:

* Architecture & Object Model Reference
* Syscall & ABI Manual
* Driver Framework Guide
* Porting Guide (per CPU family)
* Kernel Internals Book (Rustdoc-style)

Documentation is part of the release artifact.

---

### **Section X — Phased Implementation Roadmap**

The goal of the roadmap is to bring the system up **incrementally**, preserving architectural parity from the beginning while avoiding premature complexity.

Each phase ends only when **tests + equivalence guarantees pass**.

---

#### **Phase 1 — Core Kernel Skeleton (All Architectures Planned, ARM64 First)**

Objectives:

* Establish repository + module structure
* Define object model traits & syscall surfaces
* Implement minimal boot + panic path
* Bring up ARM64 QEMU single-core environment

Deliverables:

* `kernel_core` crate compiles
* early allocator + paging bootstrap
* debug UART console output
* first unit tests running in QEMU

Success criteria:

* deterministic boot to “kernel ok” state
* ARM64 bring-up validated
* x86-64 + RISC-V stubs compile but do not run yet

---

#### **Phase 2 — Scheduler, Threads, and IPC Foundations**

Objectives:

* implement basic scheduler
* create thread + process object lifecycle
* add channels + waitset IPC
* ensure behavior conforms to object model spec

Deliverables:

* preemption + context switch
* per-thread handle tables
* channel message passing (copy mode)
* blocking/wake semantics verified

Success criteria:

* conformance tests pass on ARM64
* architecture-agnostic behavior frozen
* x86-64 bring-up begins using same contracts

---

#### **Phase 3 — Virtual Memory & VMO Semantics**

Objectives:

* add VMO create/clone/map rules
* handle COW + zero-page cloning
* implement page-fault path + mapping permissions

Deliverables:

* userspace test harness
* deterministic COW behavior
* paging invariants documented + enforced

Success criteria:

* mem + IPC tests pass
* identical semantic results across ARM64 + x86-64
* RISC-V platform boot starts with same VM contract

---

#### **Phase 4 — SMP, Timers, and Exception Model**

Objectives:

* multi-CPU bring-up
* IPI + TLB shootdown support
* unified exception routing
* timer + monotonic clock APIs

Deliverables:

* SMP stress tests
* determinism validation tools
* equivalence test suite expansion

Success criteria:

* identical exception + timer semantics across

  * ARM64 hardware + QEMU
  * x86-64 QEMU
  * RISC-V QEMU

---

#### **Phase 5 — Userspace Driver Framework & Device IO**

Objectives:

* implement driver hosts
* MMIO + DMA primitives
* initial VirtIO + UART drivers
* bring-up bootable minimal userspace

Deliverables:

* userspace console shell
* driver isolation failure-tests
* PCI discovery abstraction

Success criteria:

* kernel never panics on driver crash
* behavior identical across architectures
* conformance suite “driver mode” passes

---

#### **Phase 6 — Stabilization & ABI Freeze v1**

Objectives:

* syscall/ABI checksum published
* performance baseline defined
* hard compatibility rules enforced

Deliverables:

* first **architecture-compatible kernel family release**
* documentation set complete
* stable toolchain + CI pipeline

Success criteria:

* same behavior, three binaries:

  * ARM64
  * x86-64
  * RISC-V
* all architecture-equivalence tests pass

---

### **Section Y — Repository Layout & Module Structure**

Proposed structure:

```
/kernel
  /core
    scheduler/
    ipc/
    vmo/
    object/
    syscall/
    mm/
    sync/
  /arch
    /arm64
      boot/
      traps/
      mmu/
      smp/
    /x86_64
      boot/
      traps/
      gdt_idt/
      mmu/
    /riscv64
      boot/
      traps/
      sv39/
  /drivers
    framework/
    virtio/
    uart/
  /lib
    ktrace/
    logging/
    utils/
  /tests
    conformance/
    fuzz/
    microbench/
```

Rules:

* `kernel/core/**` must never depend on `/arch/*`
* architecture modules implement trait contracts only

---

### **Section Z — Initial Trait & Object Interface Scaffolding**

These interfaces anchor portability boundaries (illustrative skeletons):

```rust
pub trait ArchCpu {
    fn init_primary();
    fn init_secondary();
    fn context_switch(from: &ThreadCtx, to: &ThreadCtx);
}

pub trait ArchMmu {
    fn map(vaddr: usize, paddr: usize, flags: MmuFlags) -> Result<()>;
    fn unmap(vaddr: usize);
    fn flush_tlb(vaddr: Option<usize>);
}

pub trait ArchTimer {
    fn now_ticks() -> u64;
    fn schedule_interrupt(deadline: u64);
}

pub trait ExceptionAdapter {
    fn normalize_trap(info: RawTrapInfo) -> NormalizedException;
}
```

Kernel core depends only on these traits — architecture layers provide implementations.

---

### **Section AA — Scheduler & Threading: Core Rust Scaffolding**

#### **AA.1 — Thread Object & Context Model**

```rust
pub struct Thread {
    pub tid: ThreadId,
    pub state: ThreadState,
    pub cpu_affinity: CpuMask,
    pub ctx: ThreadCtx,
    pub handles: HandleTable,
    pub priority: u8,
}

pub enum ThreadState {
    New,
    Runnable,
    Blocked(BlockReason),
    Dying,
    Dead,
}

pub enum BlockReason {
    WaitChannel(ChannelId),
    WaitTimer(TimerId),
    WaitFutex(FutexKey),
}
```

Thread context is **opaque to core** and implemented per-arch:

```rust
pub struct ThreadCtx {
    pub arch: ArchThreadCtx,
}
```

---

#### **AA.2 — Scheduler Trait Boundary**

```rust
pub trait Scheduler {
    fn enqueue(thread: ThreadId);
    fn dequeue(thread: ThreadId);
    fn pick_next(cpu: CpuId) -> Option<ThreadId>;
    fn yield_now();
    fn block(reason: BlockReason);
    fn wake(thread: ThreadId);
}
```

Policy rules:

* strictly priority-biased
* no starvation allowed
* preemption via timer + IPI
* semantics identical on all architectures

---

#### **AA.3 — Context Switch Integration**

Scheduler calls into architecture layer:

```rust
pub fn switch_to(next: &Thread) {
    ArchCpu::context_switch(
        &CURRENT_THREAD.ctx.arch,
        &next.ctx.arch
    );
}
```

Architecture must *not* alter semantics — only mechanism.

---

### **Section AB — IPC Channels & Wait-Set Semantics**

#### **AB.1 — Channel Object Skeleton**

```rust
pub struct Channel {
    pub id: ChannelId,
    pub peer: ChannelId,
    pub queue: RingBuf<Message>,
}

pub struct Message {
    pub bytes: Vec<u8>,
    pub handles: Vec<Handle>,
}
```

Guarantees:

* bounded queue capacity
* deterministic wake ordering
* copy-by-default, zero-copy via shared VMO handles

---

#### **AB.2 — Channel Operations**

```rust
impl Channel {
    pub fn send(&mut self, msg: Message) -> Result<()> {
        if self.queue.is_full() {
            return Err(Error::WouldBlock);
        }
        self.queue.push(msg);
        Scheduler::wake_waiters(self.id);
        Ok(())
    }

    pub fn recv(&mut self) -> Result<Message> {
        self.queue.pop().ok_or(Error::Empty)
    }
}
```

---

#### **AB.3 — Wait-Set Contract**

Wait-sets allow a thread to block on multiple objects:

```rust
pub enum WaitObject {
    Channel(ChannelId),
    Timer(TimerId),
    Futex(FutexKey),
}

pub struct WaitSet {
    pub objects: Vec<WaitObject>,
}

pub trait Waitable {
    fn ready(&self) -> bool;
}
```

Rules:

* wake ordering = **time then object index**
* cancellation must be explicit
* identical semantics across architectures

---

### **Section AC — VMO, Paging & COW Semantics**

#### **AC.1 — VMO Core Structure**

```rust
pub struct Vmo {
    pub id: VmoId,
    pub len: usize,
    pub pages: PageMap,
    pub cow_parent: Option<VmoId>,
}
```

---

#### **AC.2 — Mapping & Fault Rules**

```rust
pub trait AddressSpace {
    fn map_vmo(
        &mut self,
        vmo: &Vmo,
        vaddr: usize,
        rights: MapRights
    ) -> Result<()>;

    fn unmap(&mut self, vaddr: usize, len: usize);
}
```

COW Policy:

* clone is lazy
* first write → private page split
* behavior identical across architectures
* mechanism differs in MMU backend only

---

#### **AC.3 — Page Fault Handling Skeleton**

```rust
pub fn handle_page_fault(addr: usize, access: FaultAccess) -> Result<()> {
    let mapping = VM.find_mapping(addr)?;
    match mapping.resolve_fault(addr, access) {
        FaultAction::CopyOnWrite => perform_cow(addr),
        FaultAction::DemandAlloc => alloc_zero_page(addr),
        FaultAction::ProtectionFail => Err(Error::AccessViolation),
    }
}
```

Architecture delivers the trap →
core resolves → architecture maps page via `ArchMmu`.

---

### **Section AD — AI-Executable Task Breakdown**

These are **work units** designed for autonomous or semi-autonomous implementation agents.

#### **Phase-1 Tasks (Boot + Core Skeleton)**

* [ ] Create repository + directory layout
* [ ] Define core traits for `ArchCpu`, `ArchMmu`, `ArchTimer`
* [ ] Implement minimal ARM64 boot stub for QEMU
* [ ] Add early allocator + identity map bootstrap
* [ ] Implement UART debug console
* [ ] Add panic + logging pathway
* [ ] Validate boot to “kernel ok”

---

#### **Phase-2 Tasks (Threads + IPC Foundations)**

* [ ] Implement `Thread`, `ThreadCtx`, `ThreadState`
* [ ] Implement basic run queue scheduler
* [ ] Add context-switch hooks via `ArchCpu`
* [ ] Implement channel object + message queue
* [ ] Implement wait-sets + wake logic
* [ ] Create conformance tests for:

  * send/recv ordering
  * blocking + timeout semantics
  * scheduler fairness cases

---

#### **Phase-3 Tasks (VMO + Paging)**

* [ ] Implement VMO core + clone semantics
* [ ] Implement address-space mapping API
* [ ] Integrate page-fault handler
* [ ] Add COW clone unit tests
* [ ] Validate identical behavior in ARM64 + x86-64 QEMU

---

### **Section AE — ARM64 QEMU Bring-Up Checklist (First Boot Path)**

Target platform: **QEMU virt ARM64**, EL1/EL2 depending on design (start EL1 for simplicity).

#### **AE.1 — Boot Entry Flow**

Boot sequence requirements:

1. Firmware loads kernel at configured physical base
2. CPU starts at `_start` with:

   * MMU **disabled**
   * caches **disabled**
   * flat physical addressing
3. Kernel must:

   * set stack for primary CPU
   * establish temporary identity map
   * enable MMU + caches
   * jump to Rust entry

Checklist:

* [ ] `_start` assembly stub
* [ ] zero `.bss`
* [ ] init stack pointer
* [ ] install temporary page tables
* [ ] enable SCTLR.M + caches
* [ ] jump to `kmain()`

---

#### **AE.2 — Early Console Confirmation Path**

To validate bring-up:

* [ ] Map UART MMIO region
* [ ] Provide `putc()` polling function
* [ ] Print boot banners at milestone steps:

  * `[boot] entry ok`
  * `[boot] mmu on`
  * `[boot] rust entry`
  * `[boot] scheduler init`

This allows precise binary-search debugging during bring-up.

---

#### **AE.3 — Early Memory Rules**

* physical memory discovery via QEMU DT (or hardcoded first pass)
* before full allocator, use **bump allocator**
* page table identity-map only:

  * kernel image region
  * UART page
  * bootstrap stack

Failure modes to test:

* misaligned TTBR entries
* wrong MAIR cache attrs
* forgetting TLB invalidate after enabling MMU

---

### **Section AF — SMP Bring-Up & CPU Topology**

#### **AF.1 — CPU Startup Model**

Primary CPU responsibilities:

* initialize kernel core subsystems
* place APs (secondary CPUs) in **wait loop**
* release APs after scheduler is ready

Secondary CPU path:

1. receive spin-table entry address
2. jump to `secondary_cpu_entry`
3. configure:

   * stack
   * per-CPU state
   * local timer
4. signal `cpu_online()`
5. join scheduler

Checklist:

* [ ] per-CPU struct with:

  * run queue pointer
  * current thread
  * local timer state
* [ ] CPU online barrier
* [ ] AP startup timeout protection

---

#### **AF.2 — TLB Shootdown Semantics**

Cross-architecture invariant:

* page table changes **must not become visible inconsistently**

Behavior contract:

* writer CPU:

  * update page tables
  * enqueue shootdown message
  * wait for ACKs
* target CPUs:

  * flush local TLB
  * ACK
  * resume

Scaffolding concept:

```
Shootdown(msg: FlushRange) ->
  send to cpuset
  await barrier
```

Rules:

* ordering guarantees documented
* identical semantics ARM/x86/RISC-V
* only mechanism differs

---

### **Section AG — Exception & Interrupt Normalization Layer**

Goal: architecture traps → **normalized exception enum** → core policy.

#### **AG.1 — Normalized Exception Types**

Examples:

* `PageFault { addr, access }`
* `Syscall`
* `TimerInterrupt`
* `DeviceInterrupt(irq)`
* `IllegalInstruction`
* `UnalignedAccess`

Architecture layer responsibilities:

* decode raw trap state
* populate normalized structure **without applying policy**
* forward to core dispatcher

---

#### **AG.2 — Core Exception Dispatcher**

Flow:

1. arch trap handler enters
2. normalize → `NormalizedException`
3. dispatch by class:

   * page fault → VM subsystem
   * syscall → syscall table
   * timer → scheduler tick
   * irq → IRQ broker → driver host
4. return disposition to arch layer

Invariant: **dispatch semantics identical across architectures**.

---

### **Section AH — Determinism & Equivalence Testing Foundations**

To ensure “same behavior, different binaries”:

#### **AH.1 — Architectural Equivalence Tests**

Test classes:

* IPC ordering sequences
* scheduler fairness traces
* VMO COW decision logs
* page fault → resolution outcomes
* timeout & wake race cases

Approach:

* produce **trace artifacts**
* compare across:

  * ARM64 QEMU
  * x86-64 QEMU
  * RISC-V QEMU

Rule: differences must be explainable only by timing, never by semantics.

---

### **Section AI — Next Actionable Task Blocks**

#### **SMP & Exceptions Implementation Tasks**

* [ ] Implement per-CPU structures & bootstrap
* [ ] Implement secondary CPU bring-up routine
* [ ] Add IPI mechanism wrapper per architecture
* [ ] Implement TLB shootdown coordinator + ACK protocol
* [ ] Implement normalized exception enum
* [ ] Implement exception dispatcher
* [ ] Add conformance tests for:

  * page fault semantic equivalence
  * timeout + wake ordering under SMP
  * shootdown correctness

---

### **Step A-14 — Implement the ARM64 Exception Vector Table**

The ARM64 CPU jumps into your kernel through the **exception vectors** when interrupts, faults, or traps occur. You need to:

1. Define the 4 exception levels × 4 stack states vectors
2. Point VBAR_EL1 at the table
3. Route all exceptions into Rust handlers

Create a file such as:

```
src/kernel/arch/arm64/exceptions.rs
```

Design choices:

* Write the vector table in **inline assembly**
* Immediately branch to Rust stub functions
* Avoid doing real work in assembly if possible

Example structure (conceptual):

* Synchronous Exception
* IRQ
* FIQ (rarely used in Linux/Zircon-style kernels)
* SError

Once the table exists, add:

* Basic panic logging
* Register dump support
* Trap type classification

The goal here is correctness first — **performance later**.

---

### **Step A-15 — Implement Exception Entry Stubs → Rust**

Next, for each vector entry, create a Rust wrapper function like:

* `handle_sync_exception()`
* `handle_irq()`
* `handle_serror()`

Responsibility of the stub:

* Save register state into a struct
* Decode ESR_EL1 (exception syndrome)
* Forward to subsystem dispatcher

This step gives your kernel the ability to:

* Fault gracefully
* Print a trace
* Not silently reboot or wedge

This is essential before enabling more features.

---

### **Step A-16 — Add Basic MMIO Access Helpers**

Kernel code will soon need to touch:

* UART
* GIC
* Timer registers

Create safe-ish wrappers for:

* `readl() / writel()`
* Strongly typed IO register structs
* Memory barriers

Rust advantage here:

* Prevents type errors
* Encodes register semantics
* Avoids aliasing UB that C kernels constantly fight

This layer becomes the foundation for **device drivers**.

---

### **Step A-17 — Bring Up the Generic Interrupt Controller (GIC)**

Next milestone: enable interrupts in EL1.

Sub-steps:

1. Map GIC distributor & redistributor regions
2. Probe the CPU interface
3. Route IRQs to core 0 initially
4. Mask everything except UART + timer

Rust structure suggestion:

```
gic/
 ├── mod.rs
 ├── dist.rs
 ├── redist.rs
 ├── cpuif.rs
```

Do NOT enable interrupts globally until:

* Exception handlers are stable
* Stack switching works reliably
* UART logging is operational

Once stable, enable interrupts one source at a time.

---

### **Step A-18 — Implement a Kernel Timer Source**

Now that interrupts work, add:

* ARM Generic Timer (CNTV)
* Timer interrupt handler
* Ticks→nsec conversion
* Early scheduler timebase hooks

This unlocks:

* Timekeeping
* Sleep / delay primitives
* Preemptive scheduling later

For now:

* Just print a message every tick
* Confirm interrupt latency & stability

---

### **Step A-19 — Implement Early Physical Memory Manager**

Before virtual memory multitasking, build:

* Region allocator
* Boot-time physical memory tracking
* Early page-frame allocator

Rust fits this well using:

* Lifetime discipline
* No raw pointers except inside tiny unsafe blocks

Keep it simple:

* Page-aligned blocks only
* No fragmentation handling yet

This supports:

* Mapping new memory regions
* Kernel heap initialization

---

### **Step A-20 — Initialize the Kernel Heap (Rust Alloc Support)**

Once page allocation works:

* Stand up a kernel heap allocator
* Hook it into `alloc::` global allocator
* Replace temp bump allocator

At this point Rust features unlock:

* Vec
* String
* Box
* HashMap (eventually)

Now your kernel can scale beyond toy prototypes.

---

#### **Phase B — Virtual Memory & Address Space Model**

Includes:

* Translation tables
* Per-process address spaces
* Copy-on-write foundations
* Kernel/user separation
* Safe syscalls

If you want, I can continue with:

👉 **“Continue — Start Phase B”**

or

👉 **Dive deeper into any step above** (exception handlers, GIC, timers, memory allocator, etc.)

We’re now entering **Phase B — Virtual Memory & Address Space Model**. This is where rustux stops being “bare-metal runtime” and starts becoming a **true microkernel platform**.

---

### **Step B-1 — Define the Kernel Virtual Memory Layout**

First, you need a **clear virtual address map** for ARM64 (and later x86-64 / RISC-V).

Decide:

* Where the **kernel code/data** lives
* Where the **physical memory window** is mapped
* Where **device MMIO regions** go
* Whether userspace and kernel share one address space or are separate

Typical design (inspired by Zircon & Linux):

* High half → kernel space
* Low half → userspace per-process
* Direct-map window for physical memory
* Guard pages around stacks

Rust advantage here:

* Enforce invariants at type level
* Reduce invalid pointer cases

Once the layout is pinned, **do not change it lightly** — lots of later code will depend on it.

---

### **Step B-2 — Implement Page Table Abstractions (ARM64)**

ARM64 page tables are:

* 4-level radix tree
* 4 KB pages
* Block mappings for large regions

You’ll build:

1. A `PageTable` struct
2. Page table entry type
3. Helpers for:

   * map / unmap
   * set attributes (RW/XN/Device/Normal)
   * TLB maintenance

Start by supporting only:

* 4 KB pages
* Kernel mappings only
* Identity + higher-half mapping as needed

Later you’ll add:

* Huge pages
* Copy-on-write
* User permissions

---

### **Step B-3 — Create the Kernel Address Space**

Now instantiate the **kernel’s own VA space**.

Steps:

1. Allocate root translation table
2. Map:

   * Kernel text/data as RX / RW
   * Device regions as Device-nGnRnE
   * Physical memory window
3. Switch TTBR1_EL1 to new table
4. Invalidate TLBs

If this step works:

* Kernel keeps running
* UART still prints
* Timer interrupt still fires

If it fails:

* Panic in exception handler
* Debug via ESR_EL1 + far/pc registers

This is one of the **hardest bring-up points** — expect iteration.

---

### **Step B-4 — Introduce the Concept of an Address Space Object**

Now move toward **microkernel semantics**.

Define something like:

* `AddressSpace`
* Reference-counted
* Backed by a root page table

Responsibilities:

* Manage mappings
* Provide isolateable memory views
* Prepare foundation for processes / threads

Do NOT yet create real processes — just ensure separate address spaces can exist.

Rust helps here because:

* Ownership semantics mirror mapping lifetime
* Less risk of dangling mappings

---

### **Step B-5 — Add Per-Thread Kernel Stack Mapping**

To safely switch between threads later:

* Each thread gets its own kernel stack
* Stack lives in kernel space
* Guard page below stack to detect overflow

Steps:

1. Allocate stack frames
2. Map them into kernel VA
3. Store pointer in thread struct
4. Configure SP_EL1 on context switch

Even before a scheduler exists, you can:

* Create dummy threads
* Switch between them manually

This validates stack isolation early.

---

### **Step B-6 — Implement Early Virtual Memory Debug Tools**

Before complexity grows, add:

* VA→PA translation dump
* Page table walker
* Mapping audit logging

These tools will save you enormous time later.

Example use cases:

* Fault happens at VA X
* Walk page tables
* Report why mapping failed

Rust makes it easier to guarantee output correctness and avoid UB while inspecting memory.

---

### **Step B-7 — Add User/Supervisor Memory Attribute Support**

Now extend mappings with:

* User-accessible vs kernel-only
* Execute-never rules
* Read-only enforcement

This is what enables:

* User processes soon
* Syscall boundary enforcement
* Sandboxing guarantees

At this point, you’ll have:

* A working kernel address space
* The ability to create new address spaces
* Proper mapping rules
* The foundations of userspace isolation

This is roughly equivalent to **early Zircon VM + Linux mm initialization**.

---

### **You’re Approaching the Next Major Milestone**

Once Phase B completes, the kernel will be ready for:

#### **Phase C — Syscalls, Threads, and Scheduler Bring-Up**

Which will include:

* Thread context structures
* Context switching
* Syscall entry trampolines
* First userspace program
* IPC primitives foundation

---

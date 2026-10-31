# Rustux Roadmap

> **About this file.** This is the architecture checklist written in January 2026 (moved here from the server root). The checkboxes have **not** been kept up to date, and the plan assumed ARM64 first, whereas development has so far targeted x86-64. For where things actually stand, see the status table in the [README](../README.md) and [`HISTORY.md`](HISTORY.md). The long design notes behind this checklist are in [`design/planning-notes.md`](design/planning-notes.md).
>
> **Rough status (2026-10-31):** Phase A (boot, PMM, logging) works on x86-64 under UEFI. Phase B is the current blocker: the kernel still runs in the firmware's lower-half identity mapping. Phase C has a round-robin scheduler and `int 0x80` syscalls but no working userspace yet. Phase D objects exist in code but their syscalls are stubs. Phases E–I have not started.

The original step-by-step plan for a Zircon‑style microkernel in Rust with a stable syscall/object model across **ARM64, x86‑64, and RISC‑V (RV64)**. User binaries are compiled per‑architecture; kernel behavior and ABI remain identical.

---

## **Phase A — Boot, Platform Bring‑Up & Core Services**

### **A‑1. Repository & Build System**
- [ ] Create mono‑repo structure: `/kernel`, `/arch/{arm64,x86_64,riscv64}`, `/lib`, `/sysroot`, `/docs`.
- [ ] Select license (use **MIT** to encourage adoption & contributions).
- [ ] Initialize Rust workspace with `no_std`, nightly features gated behind cfg.
- [ ] Add cross‑compile targets for ARM64, x86‑64, RV64.
- [ ] Establish style, unsafe‑code policy, lint gates.

### **A‑2. Architecture Abstraction Layer (AAL)**
- [ ] Define `arch/` traits for CPU init, MMU, traps, timers, IPI, context switch.
- [ ] Provide per‑arch implementations w/ identical semantics.
- [ ] Keep arch‑specific code isolated from kernel core.

### **A‑3. Early Boot & Exception Vectors (ARM64 first)**
- [ ] Bring up EL1, stack, BSS, and relocate kernel if required.
- [ ] Install exception vectors; implement panic + crash dump path.
- [ ] Enable timer + UART for debug output.

### **A‑4. Physical Memory Manager (PMM)**
- [ ] Parse boot memory map.
- [ ] Implement page‑frame allocator (buddy or bitmap).
- [ ] Add guard rails & invariants.

### **A‑5. Kernel Logging & Diagnostics**
- [ ] Early UART logger.
- [ ] Structured logs + crash reason + register dump.

---

## **Phase B — Virtual Memory & Address Space Model**

### **B‑1. Kernel Virtual Address Layout**
- [ ] Define fixed VA map: user low‑half, kernel high‑half.
- [ ] Reserve regions: text/data, per‑CPU, stacks + guards, phys‑map window, MMIO.

### **B‑2. Page Table Abstractions (ARM64 baseline)**
- [ ] Implement PageTable + PTE types.
- [ ] Support map/unmap, attributes, TLB maintenance.
- [ ] Start with 4‑KB pages; add large pages later.

### **B‑3. Kernel Address Space Init**
- [ ] Allocate kernel root table.
- [ ] Map kernel text RX / data RW / stacks + guards.
- [ ] Map MMIO + phys‑map with proper attributes.
- [ ] Switch TTBR + invalidate TLBs; verify stability.

### **B‑4. AddressSpace Object**
- [ ] Implement ref‑counted `AddressSpace` handle.
- [ ] Manage mappings independent of processes.

### **B‑5. Per‑Thread Kernel Stacks**
- [ ] Allocate stack frames per thread.
- [ ] Map with guard pages; store SP in thread state.

### **B‑6. VM Debugging Utilities**
- [ ] VA→PA walker and table dump.
- [ ] Mapping audit + fault diagnostics.

### **B‑7. Protection Attributes**
- [ ] Support user/kernel access, XN, RO/RW enforcement.

---

## **Phase C — Threads, Syscalls, and Scheduling (Cross‑Arch ABI‑Stable)**

### **C‑1. Unified Syscall ABI Spec**
- [ ] Freeze syscall numbers & semantics across arches.
- [ ] Define calling convention adapter per arch.
- [ ] Document object model + handle semantics.

### **C‑2. Trap/Syscall Entry Paths**
- [ ] Implement arch trampolines → common dispatcher.
- [ ] Validate user pointers & memory ranges.

### **C‑3. Thread Object & Context**
- [ ] Define `Thread` struct (regs, TLS, state, kernel stack).
- [ ] Implement save/restore for each arch.

### **C‑4. Scheduler (Minimal Round‑Robin)**
- [ ] Per‑CPU run queue.
- [ ] Timer tick → preemption.
- [ ] Context switch + accounting.

### **C‑5. Process / Task Skeleton**
- [ ] `Process` binds `AddressSpace` + handle table.
- [ ] Create first user task via loader stub.

### **C‑6. User/Kernel Boundary Safety**
- [ ] Copy‑to/from‑user helpers.
- [ ] Fault isolation + precise crash reporting.

---

## **Phase D — Kernel Objects & IPC (Zircon‑style)**

### **D‑1. Handle & Rights Model**
- [ ] Global handle table with reference counting.
- [ ] Rights mask checked on each operation.

### **D‑2. Core Objects**
- [ ] VM Objects (VMO) — paged memory backing.
- [ ] Channels — bidirectional IPC with message queues.
- [ ] Events & Eventpairs.
- [ ] Ports / Wait sets.

### **D‑3. Capability Transfer via IPC**
- [ ] Allow handle passing with rights reduction.

---

## **Phase E — Memory Management Features**

### **E‑1. Demand Paging & COW**
- [ ] Pager interface; lazy faults; copy‑on‑write.

### **E‑2. Shared Memory**
- [ ] Map VMOs into multiple AddressSpaces.

### **E‑3. VDSO Support**
- [ ] Map per‑process VDSO with stable API surface.

---

## **Phase F — Multiplatform Enablement**

### **F‑1. Port to x86‑64**
- [ ] Implement arch layer (GDT/IDT, APIC, PTs).
- [ ] Reuse common VM + scheduler + objects unchanged.

### **F‑2. Port to RISC‑V RV64**
- [ ] Implement S‑mode traps, Sv39/48 page tables, PLIC/ACLINT.

### **F‑3. Conformance Tests (ABI & Behavior)**
- [ ] Run syscall compatibility suite across all arches.

---

## **Phase G — Userspace SDK & Toolchain**

### **G‑1. LibSystem / Runtime**
- [ ] Provide user‑level syscall wrappers + handle API.

### **G‑2. Build & Package**
- [ ] Per‑arch rootfs build; identical sources, separate binaries.

---

## **Phase H — Reliability, Security, & QA**

- [ ] Fuzz syscall layer + IPC.
- [ ] Property tests for VM + handles.
- [ ] Kernel invariants + static analysis.

---

## **Phase I — Documentation & Governance**

- [ ] Publish spec for syscall ABI and object model.
- [ ] Contribution guide, code of conduct, review policy.
- [ ] Versioning & stability guarantees.

---

### **Done Criteria (Milestone M1)**
- [ ] Boots on ARM64 + launches first user task.
- [ ] Passes ABI conformance on x86‑64 and RV64.
- [ ] Same syscall/object semantics on all architectures.

---

> This checklist is intentionally deterministic and arch‑portable so an automated CLI or AI agent can execute it step‑by‑step while keeping the kernel’s ABI, syscall interface, and object model consistent across platforms.

# Design: Kernel Services, IPC and Scheduling

Target design for the scheduler and IPC objects, adapted from the Zircon model. Taken from the project's first design note ("Let us begin"). The current kernel implements only a single-CPU round-robin scheduler (`src/sched/`) and stubs for most object syscalls; this document describes where it is heading.

## Kernel services

The kernel provides: system calls, memory management, task and process management, IPC and synchronization, exception handling, and hardware support services (clocks, entropy, device I/O). IPC, memory management, the scheduler and process management are kernel services; everything else should live in userspace.

User programs reach system calls through a **vDSO**: a shared library in ELF format that the kernel maps into every new process. It is "virtual" because it comes from the kernel image rather than a file.

Most system calls take one or more **handles**: process-local 32-bit references to kernel objects (`rx_handle_t`). Each handle carries **rights** that say what its holder may do with the handle and the object.

## Jobs, processes and threads

- **Thread:** a thread of execution within an address space.
- **Process:** executable code running in a private, isolated address space. Processes are the unit of capability: a process can do exactly what the handles it holds allow.
- **Job:** a group of related processes and jobs. All jobs form a single rooted tree, so an application made of several processes can be controlled as one unit.

Every process starts with one **VMAR** (virtual memory address region) spanning the whole user address space; it can be used directly or subdivided into child VMARs. Memory itself is held in **VMOs**.

## IPC objects

Processes are isolated by default. The kernel provides these objects for communication:

| Object | Purpose |
|--------|---------|
| Event | Signalling between processes |
| Socket | Streaming data transport, like a pipe |
| Stream | Seekable streaming transport, like a file |
| Channel | Message transport that carries data **and handles** |
| FIFO | Control plane for shared memory, small payloads |

**Channels** have exactly two endpoints, each normally owned by a different process. Only the owner of an endpoint can read or write it, and endpoint ownership can itself be transferred. Writing handles into a channel removes them from the sender; reading the message adds them to the receiver. This makes channels the way capabilities move between processes, and the natural way to launch new processes. Service protocols on top of channels are described by the **Rustica Interface Definition Language (RIDL)**.

## Scheduler

### Per-CPU run queues

Each logical CPU runs its own scheduler, and schedulers coordinate through inter-processor interrupts (IPIs). Each CPU has one FIFO queue per priority level (32 levels, 0 lowest to 31 highest). To pick the next thread, the scheduler takes the head of the highest non-empty queue.

### Timeslices

Every thread gets the same timeslice when it starts running. A thread that uses its whole timeslice goes to the **back** of its queue. A thread that blocked with time left goes to the **front** when it wakes and runs only for the remainder. The scheduler programs the CPU's preemption timer for a full slice or the remainder; when it fires, the current thread is requeued and another is chosen.

A thread that blocks on a shared resource leaves its run queue for that resource's wait queue. When unblocked it is placed on an eligible CPU (see below), at the front of the queue if it had time left.

### Effective priority

Three factors decide which queue a thread is in:

1. **Base priority:** the thread's requested priority (0–31).
2. **Boost:** an offset in `[-MAX_PRIORITY_ADJ, MAX_PRIORITY_ADJ]`:
   - +1 when the thread is unblocked (after waiting or sleeping);
   - −1 when it yields or voluntarily reschedules, but not below 0;
   - −1 when it is preempted after using its whole timeslice, and this can go negative.
3. **Inherited priority:** a thread holding a resource that blocks a higher-priority thread temporarily runs at that thread's priority.

Effective priority is the inherited priority if there is one, otherwise base + boost. When it changes, the thread moves queues and the CPU reschedules. The effect: interactive threads, which mostly wait for input and do little work, stay responsive, while CPU-bound background threads drift down.

### CPU assignment and migration

Threads have a 32-bit CPU affinity mask (bit 0 = CPU 1, and so on). When placing a thread the scheduler chooses, in order:

1. the selecting CPU, if idle and in the mask;
2. the CPU the thread last ran on, if idle and in the mask;
3. any idle CPU in the mask;
4. the last CPU, if active and in the mask;
5. the selecting CPU, if it is the only CPU in the mask or none of the masked CPUs are active;
6. any active CPU in the mask.

A thread running outside its mask (case 5) is moved back when it is next preempted, yields or reschedules. Placement is re-evaluated every time a thread wakes.

### CPU activation

When a CPU is taken offline its threads move to other CPUs, except threads **pinned** to only that CPU, which wait until it returns. There is no active rebalancing when a CPU comes back; it picks up work through the placement rules above.

### Idle and real-time threads

- Each CPU has an **idle thread** that runs when nothing else can, effectively at priority −1. It measures idle time and can enter a low-power wait.
- **Real-time threads** (`THREAD_FLAG_REAL_TIME`) are not preempted; they run until they block, yield or reschedule.

### Future: fair scheduler and tracing

A fair (weighted) scheduler is a later goal, together with trace events for analysis: `sched_block`, `sched_unblock`, `sched_unblock_list`, `sched_yield`, `sched_preempt`, `sched_reschedule`, and a `sched_latency` flow from "entered run queue" to "context switched in".

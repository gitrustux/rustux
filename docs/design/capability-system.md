# Capability System Design for Rustux

**Date:** 2025-01-17
**Status:** Design Document
**Inspired by:** Zircon (Fuchsia) Kernel

---

## Overview

Rustux implements a **capability-based security model** inspired by Zircon. In this model, all access to kernel resources is mediated through *capabilities* (also called *handles*), which are unforgeable references to kernel objects.

### Key Principles

1. **All access is through capabilities** - No global namespaces or direct object access
2. **Capabilities are unforgeable** - Cannot be forged, only received from kernel
3. **Capabilities have rights** - Each capability specifies what operations are allowed
4. **Capabilities are revocable** - Kernel can revoke capabilities at any time
5. **Least privilege** - Objects are created with minimal rights

---

## Core Concepts

### KernelObject Trait

All kernel objects implement the `KernelObject` trait:

```rust
pub trait KernelObject {
    /// Get the object type
    fn type_id(&self) -> ObjectType;

    /// Get associated handles (for bookkeeping)
    fn handle_count(&self) -> usize;

    /// Called when last handle is closed
    fn on_last_handle_closed(&mut self);
}
```

### ObjectType Enum

```rust
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Process = 1,
    Thread = 2,
    VmObject = 3,
    VmAddressSpace = 4,
    Channel = 5,
    Event = 6,
    Interrupt = 7,
    Port = 8,
    Fifo = 9,
    // ... more types
}
```

### Handle Rights

Each capability has associated *rights* that specify what operations are allowed:

```rust
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u64);

impl Rights {
    pub const NONE: Rights = Rights(0);

    // Basic rights
    pub const DUPLICATE: Rights = Rights(1 << 0);
    pub const TRANSFER: Rights = Rights(1 << 1);
    pub const READ: Rights = Rights(1 << 2);
    pub const WRITE: Rights = Rights(1 << 3);
    pub const EXECUTE: Rights = Rights(1 << 4);
    pub const MAP: Rights = Rights(1 << 5);
    pub const GET_PROPERTY: Rights = Rights(1 << 6);
    pub const SET_PROPERTY: Rights = Rights(1 << 7);
    pub const ENUMERATE: Rights = Rights(1 << 8);
    pub const DESTROY: Rights = Rights(1 << 9);
    pub const SIGNAL: Rights = Rights(1 << 10);
    pub const WAIT: Rights = Rights(1 << 11);
    pub const SIGNAL_PEER: Rights = Rights(1 << 12);
    pub const BIND_INTERRUPT: Rights = Rights(1 << 13);

    pub fn contains(&self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn remove(&mut self, other: Rights) {
        self.0 &= !other.0;
    }
}
```

### Handle Values

A *handle* is a 32-bit integer that represents a capability:

```rust
pub type Handle = u32;

// Special handle values
pub const INVALID_HANDLE: Handle = 0;
pub const HANDLE_SELF: Handle = u32::MAX;  // Refers to current process
```

---

## Object Types

### Process Object

Represents a user-space process with its own address space.

```rust
pub struct ProcessObject {
    id: ProcessId,
    address_space: VmAddressSpace,
    handle_table: HandleTable,
    state: ProcessState,
    // ...
}

// Process-specific rights
impl Rights {
    pub const PROCESS_TERMINATE: Rights = Rights(1 << 16);
    pub const PROCESS_DEBUG: Rights = Rights(1 << 17);
    pub const PROCESS_MEMORY: Rights = Rights(1 << 18);
    pub const PROCESS_THREADS: Rights = Rights(1 << 19);
    pub const PROCESS_VMAR_ROOT: Rights = Rights(1 << 20);
}
```

### Thread Object

Represents a thread of execution within a process.

```rust
pub struct ThreadObject {
    id: ThreadId,
    process: Weak<ProcessObject>,
    state: ThreadState,
    registers: Registers,
    // ...
}

// Thread-specific rights
impl Rights {
    pub const THREAD_TERMINATE: Rights = Rights(1 << 21);
    pub const THREAD_STATE: Rights = Rights(1 << 22);
    pub const THREAD_READ_STATE: Rights = Rights(1 << 23);
    pub const THREAD_WRITE_STATE: Rights = Rights(1 << 24);
}
```

### Interrupt Object

Represents a hardware interrupt source.

```rust
pub struct InterruptObject {
    irq: u32,
    vector: u8,
    port: Option<Handle>,      // Port to signal when interrupt fires
    threshold: u64,            // Signal threshold (0 = disable)
    timestamp: u64,            // Last interrupt timestamp
    // ...
}

// Interrupt-specific rights
impl Rights {
    pub const INTERRUPT_BIND: Rights = Rights(1 << 25);
    pub const INTERRUPT_SIGNAL: Rights = Rights(1 << 26);
    pub const INTERRUPT_WAIT: Rights = Rights(1 << 27);
    pub const INTERRUPT_GET_TIMESTAMP: Rights = Rights(1 << 28);
}

// Example: Creating an interrupt object
pub fn create_interrupt_object(
    irq: u32,
    vector: u8,
    port: Handle,
) -> Result<Handle, Error> {
    let interrupt = InterruptObject {
        irq,
        vector,
        port: Some(port),
        threshold: 1,
        timestamp: 0,
    };

    // Create handle with appropriate rights
    let rights = Rights::INTERRUPT_BIND |
                 Rights::INTERRUPT_SIGNAL |
                 Rights::INTERRUPT_GET_TIMESTAMP;

    create_handle(interrupt, rights)
}
```

### Channel Object

Bidirectional messaging channel (like Zircon channels).

```rust
pub struct ChannelObject {
    endpoints: [ChannelEndpoint; 2],
    // ...
}

pub struct ChannelEndpoint {
    packets: Vec<Packet>,
    waiting: Vec<Handle>,  // Threads waiting for messages
    // ...
}

// Channel-specific rights
impl Rights {
    pub const CHANNEL_READ: Rights = Rights(READ);
    pub const CHANNEL_WRITE: Rights = Rights(WRITE);
}
```

### Port Object

Unidirectional packet queue (like Zircon ports).

```rust
pub struct PortObject {
    packets: Vec<Packet>,
    waiting: Vec<Handle>,
    // ...

    pub struct Packet {
        kind: PacketKind,
        status: i32,
        data: [u8; 32],
        // ...
    }

    pub enum PacketKind {
        User(usize),           // User packet
        Interrupt(Handle),     // Interrupt packet
        Exception(Handle),     // Exception packet
        // ...
    }
}

// Port-specific rights
impl Rights {
    pub const PORT_READ: Rights = Rights(READ);
    pub const PORT_WRITE: Rights = Rights(WRITE);
}
```

---

## Handle Table

Each process maintains a *handle table* that maps handle values to objects:

```rust
pub struct HandleTable {
    table: Vec<Option<HandleEntry>>,
    next_free: usize,
}

pub struct HandleEntry {
    object: Arc<dyn KernelObject>,
    rights: Rights,
    process_id: ProcessId,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            table: Vec::new(),
            next_free: 1,  // 0 is reserved (INVALID_HANDLE)
        }
    }

    /// Add a new handle to the table
    pub fn add_handle(
        &mut self,
        object: Arc<dyn KernelObject>,
        rights: Rights,
    ) -> Handle {
        let handle_value = self.next_free;

        // Grow table if needed
        if handle_value >= self.table.len() {
            self.table.resize(handle_value + 1, None);
        }

        self.table[handle_value] = Some(HandleEntry {
            object,
            rights,
            process_id: current_process_id(),
        });

        self.next_free = handle_value + 1;
        handle_value as Handle
    }

    /// Get an object from a handle
    pub fn get_handle(
        &self,
        handle: Handle,
        required_rights: Rights,
    ) -> Option<Arc<dyn KernelObject>> {
        if handle == INVALID_HANDLE {
            return None;
        }

        let entry = self.table.get(handle as usize)?.as_ref()?;

        // Check rights
        if !entry.rights.contains(required_rights) {
            return None;  // Access denied
        }

        Some(Arc::clone(&entry.object))
    }

    /// Remove a handle from the table
    pub fn remove_handle(&mut self, handle: Handle) -> Option<Arc<dyn KernelObject>> {
        if handle == INVALID_HANDLE {
            return None;
        }

        let entry = self.table.get_mut(handle as usize)?.take()?;
        Some(entry.object)
    }
}
```

---

## System Calls

System calls operate on handles and respect capability rights:

```rust
// Example system call: zx_handle_duplicate
pub fn sys_handle_duplicate(
    handle: Handle,
    rights: Rights,
) -> Result<Handle, Error> {
    let current = current_process();
    let entry = current.handle_table.get_handle(handle, Rights::DUPLICATE)?;

    // Reduce rights if requested
    let new_rights = entry.rights.intersect(rights);

    Ok(current.handle_table.add_handle(entry.object, new_rights))
}

// Example system call: zx_handle_close
pub fn sys_handle_close(handle: Handle) -> Result<(), Error> {
    let current = current_process();
    let object = current.handle_table.remove_handle(handle)?;

    // Check if this was the last handle
    if Arc::strong_count(&object) == 1 {
        object.on_last_handle_closed();
    }

    Ok(())
}

// Example system call: zx_interrupt_bind
pub fn sys_interrupt_bind(
    interrupt_handle: Handle,
    port_handle: Handle,
    key: u64,
    options: u32,
) -> Result<(), Error> {
    let current = current_process();

    // Get interrupt object with BIND right
    let interrupt = current.handle_table.get_handle(
        interrupt_handle,
        Rights::INTERRUPT_BIND
    )?;

    // Get port object with WRITE right
    let port = current.handle_table.get_handle(
        port_handle,
        Rights::PORT_WRITE
    )?;

    // Bind interrupt to port
    interrupt.downcast::<InterruptObject>()?.bind(
        port.downcast::<PortObject>()?,
        key,
        options,
    );

    Ok(())
}
```

---

## Security Properties

### Unforgeability

Handles are allocated from a per-process table and are:
- **Not globally unique** - Same handle value can refer to different objects in different processes
- **Not predictable** - Sequential allocation within process, but process-local
- **Cannot be forged** - Only kernel can add entries to handle tables

### Principle of Least Privilege

Objects are created with minimal rights:

```rust
// Creating a channel: endpoints start with basic read/write rights
let (h1, h2) = create_channel()?;
// h1: READ | WRITE
// h2: READ | WRITE

// Can duplicate with fewer rights
let h1_readonly = sys_handle_duplicate(h1, Rights::READ)?;
// h1_readonly: READ only

// Cannot upgrade rights
let _ = sys_handle_duplicate(h1_readonly, Rights::WRITE);  // Error!
```

### Revocation

Kernel can revoke capabilities:

```rust
// Process termination
fn terminate_process(process: Handle) {
    let process_obj = get_process(process);
    // Invalidate all handles in the process's handle table
    process_obj.handle_table.clear();
}
```

---

## Interrupt Integration

### Interrupt Objects as Capabilities

Interrupt sources are represented as interrupt objects:

```rust
// Driver requests IRQ
pub fn sys_interrupt_create(
    irq: u32,
    vector: u8,
    options: u32,
) -> Result<Handle, Error> {
    // Check if driver has permission to bind this IRQ
    let current = current_process();
    if !current.can_bind_interrupt(irq) {
        return Err(Error::ACCESS_DENIED);
    }

    // Create interrupt object
    let interrupt = InterruptObject {
        irq,
        vector,
        port: None,
        threshold: 0,
        timestamp: 0,
    };

    let rights = Rights::INTERRUPT_BIND |
                 Rights::INTERRUPT_SIGNAL |
                 Rights::INTERRUPT_GET_TIMESTAMP;

    Ok(current.handle_table.add_handle(interrupt, rights))
}

// Driver binds interrupt to port
pub fn sys_interrupt_bind(
    interrupt_handle: Handle,
    port_handle: Handle,
    key: u64,
    options: u32,
) -> Result<(), Error> {
    let current = current_process();

    let interrupt_obj = current.handle_table.get_handle(
        interrupt_handle,
        Rights::INTERRUPT_BIND
    )?;

    let port_obj = current.handle_table.get_handle(
        port_handle,
        Rights::PORT_WRITE
    )?;

    interrupt_obj.downcast::<InterruptObject>()?.bind(
        port_obj.downcast::<PortObject>()?,
        key,
        options,
    );

    // Enable the IRQ in the interrupt controller
    enable_irq(interrupt_obj.irq, interrupt_obj.vector);

    Ok(())
}
```

### Interrupt Delivery

When an interrupt fires:

```rust
extern "x86-interrupt" fn keyboard_interrupt_handler(frame: InterruptFrame) {
    // Get interrupt object for this IRQ
    let interrupt_obj = get_interrupt_object_for_irq(1);

    // Update timestamp
    interrupt_obj.timestamp = read_tsc();

    // Signal the bound port
    if let Some(port) = &interrupt_obj.port {
        port.write(Packet {
            kind: PacketKind::Interrupt(interrupt_obj.handle),
            status: 0,
            key: interrupt_obj.port_key,
            timestamp: interrupt_obj.timestamp,
        });
    }

    // Send EOI
    apic_send_eoi(1);
}
```

---

## Comparison with Zircon

| Feature | Zircon | Rustux |
|---------|--------|--------|
| Handle size | 32-bit | 32-bit |
| Rights system | ✅ | ✅ |
| Object types | 30+ | Starting with core types |
| Channel communication | ✅ | Planned |
| Port-based interrupts | ✅ | ✅ |
| Capability-based security | ✅ | ✅ |
| Handle duplication | ✅ | ✅ |
| Rights reduction | ✅ | ✅ |

---

## Next Steps

1. **Implement core objects** - Process, Thread, Channel, Port, Interrupt
2. **Implement handle table** - Per-process handle management
3. **Implement system calls** - Handle operations, object creation
4. **Implement interrupt binding** - Connect interrupt objects to ports
5. **Add more object types** - VMAR, VMO, FIFO, etc.
6. **Testing** - Comprehensive tests of capability system

---

*This design document is a work in progress and will evolve as the implementation progresses.*

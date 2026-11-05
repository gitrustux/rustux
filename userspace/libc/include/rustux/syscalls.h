// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Rustux system calls for C programs.
// ABI: int 0x80, number in RAX, arguments in RDI, RSI, RDX, R10, R8, R9,
// result in RAX (negative = error). See docs/SYSCALL.md.

#ifndef RUSTUX_SYSCALLS_H
#define RUSTUX_SYSCALLS_H

#include <stdint.h>

typedef uint32_t rx_handle_t;
typedef int64_t rx_status_t;

#define RX_OK                   0
#define RX_ERR_INVALID_ARGS    -1
#define RX_ERR_NO_MEMORY       -2
#define RX_ERR_ACCESS_DENIED   -4
#define RX_ERR_NOT_SUPPORTED   -9
#define RX_ERR_BAD_HANDLE     -10
#define RX_ERR_WRONG_TYPE     -11
#define RX_ERR_TIMED_OUT      -12
#define RX_ERR_SHOULD_WAIT    -13
#define RX_ERR_PEER_CLOSED    -14
#define RX_ERR_BUFFER_TOO_SMALL -15
#define RX_ERR_OUT_OF_RANGE   -16
#define RX_ERR_NOT_FOUND       -5
#define RX_ERR_BAD_STATE      -17
#define RX_ERR_ALREADY_EXISTS -18
#define RX_ERR_CANCELED       -19

#define RX_TASK_RETCODE_SYSCALL_KILL   -1024
#define RX_TASK_RETCODE_EXCEPTION_KILL -1025

#define RX_TIME_INFINITE  UINT64_MAX

// Signals
#define RX_CHANNEL_READABLE   (1u << 0)
#define RX_CHANNEL_WRITABLE   (1u << 1)
#define RX_PEER_CLOSED        (1u << 2)
#define RX_EVENT_SIGNALED     (1u << 3)
#define RX_TIMER_SIGNALED     (1u << 3)
#define RX_PROCESS_TERMINATED (1u << 3)
#define RX_THREAD_TERMINATED  (1u << 3)
#define RX_USER_SIGNAL_0      (1u << 24)

// Rights
#define RX_RIGHT_DUPLICATE   (1u << 0)
#define RX_RIGHT_TRANSFER    (1u << 1)
#define RX_RIGHT_READ        (1u << 2)
#define RX_RIGHT_WRITE       (1u << 3)
#define RX_RIGHT_EXECUTE     (1u << 4)
#define RX_RIGHT_MAP         (1u << 5)
#define RX_RIGHT_DESTROY     (1u << 9)
#define RX_RIGHT_SIGNAL      (1u << 12)
#define RX_RIGHT_SIGNAL_PEER (1u << 13)
#define RX_RIGHT_WAIT        (1u << 14)
#define RX_RIGHT_SAME_RIGHTS (1u << 31)

// Object types
#define RX_OBJ_TYPE_PROCESS   1
#define RX_OBJ_TYPE_THREAD    2
#define RX_OBJ_TYPE_VMO       3
#define RX_OBJ_TYPE_INTERRUPT 9
#define RX_OBJ_TYPE_RESOURCE  15
#define RX_OBJ_TYPE_CHANNEL   4
#define RX_OBJ_TYPE_JOB       17
#define RX_OBJ_TYPE_VMAR      18
#define RX_OBJ_TYPE_EVENT     5
#define RX_OBJ_TYPE_EVENTPAIR 16
#define RX_OBJ_TYPE_TIMER     22
#define RX_OBJ_TYPE_LOG       12
#define RX_OBJ_TYPE_SOCKET    14

// vmar_map / vmar_protect options
#define RX_VM_PERM_READ    (1u << 0)
#define RX_VM_PERM_WRITE   (1u << 1)
#define RX_VM_PERM_EXECUTE (1u << 2)
#define RX_VM_SPECIFIC     (1u << 4)

// Every process's root VMAR is [0x10000, 0x7ffffffff000). Mappings
// without RX_VM_SPECIFIC go at or above 4 GiB when there is room.
#define RX_ROOT_VMAR_BASE  0x10000ull
#define RX_VM_ALLOC_HINT   0x100000000ull

// Bootstrap message handle info words (Fuchsia PA_* values): the type in
// bits 0-7, an argument in bits 16-31
#define RX_PA_PROC_SELF   0x01
#define RX_PA_THREAD_SELF 0x02
#define RX_PA_JOB_DEFAULT 0x03
#define RX_PA_VMAR_ROOT   0x04
#define RX_PA_VMO_BOOTFS  0x1B    // the boot image (init only)
#define RX_PA_NS_DIR      0x20    // root directory of the namespace (filesystem)
#define RX_PA_FD          0x30    // a file descriptor; argument = fd | kind << 12
#define RX_PA_ROOT_RESOURCE 0x3F  // the root resource (init, drivers)
#define RX_PA_KERNEL_INPUT  0xE0  // Rustux: kernel input channel (init)
#define RX_PA_POWER_RESOURCE 0xE1 // Rustux: the power resource (reboot, poweroff)
#define RX_PA_JOB_SYSTEM    0xE2  // Rustux: the job holding the whole system (ps, kill)
#define RX_PA_USER0       0xF0    // handles a parent passes to a child
#define RX_PA_USER1       0xF1
#define RX_PA_USER2       0xF2
#define RX_PA_HND(type, arg) (((uint32_t)(type) & 0xFF) | (((uint32_t)(arg) & 0xFFFF) << 16))

// object_get_info topics (Zircon values)
#define RX_INFO_HANDLE_BASIC     2
#define RX_INFO_PROCESS          3
#define RX_INFO_PROCESS_THREADS  4   // u64 koids
#define RX_INFO_JOB_CHILDREN     6   // u64 koids
#define RX_INFO_JOB_PROCESSES    7   // u64 koids
#define RX_INFO_TASK_STATS      11
#define RX_INFO_TASK_RUNTIME    25

typedef struct {
    uint64_t koid;          // 0 for objects other than jobs, processes, threads
    uint32_t rights;
    uint32_t type;          // RX_OBJ_TYPE_*
    uint64_t related_koid;  // a process's job, a thread's process, a job's parent
} rx_info_handle_basic_t;

typedef struct {
    uint64_t mem_mapped_bytes;
    uint64_t mem_private_bytes;   // memory committed in the VMOs it maps
    uint64_t mem_shared_bytes;
    uint64_t mem_scaled_shared_bytes;
} rx_info_task_stats_t;

typedef struct {
    int64_t cpu_time;   // ns
    int64_t queue_time;
    int64_t page_fault_time;
    int64_t lock_contention_time;
} rx_info_task_runtime_t;

#define RX_PROP_NAME 3
#define RX_MAX_NAME_LEN 32
typedef struct {
    int64_t return_code;
    int64_t start_time;
    uint32_t flags;      // RX_INFO_PROCESS_FLAG_*
    uint32_t padding;
} rx_info_process_t;
#define RX_INFO_PROCESS_FLAG_STARTED (1u << 0)
#define RX_INFO_PROCESS_FLAG_EXITED  (1u << 1)

typedef struct {
    rx_handle_t handle;
    uint32_t waitfor;
    uint32_t pending;
} rx_wait_item_t;

static inline int64_t rx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
    register int64_t r10 __asm__("r10") = d;
    register int64_t r8 __asm__("r8") = e;
    register int64_t r9 __asm__("r9") = f;
    int64_t ret;
    __asm__ volatile("int $0x80"
                     : "=a"(ret)
                     : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "memory", "rcx", "r11");
    return ret;
}

#define RX_SYS(n, a, b, c, d) rx_syscall((n), (int64_t)(a), (int64_t)(b), (int64_t)(c), (int64_t)(d), 0, 0)
#define RX_SYS6(n, a, b, c, d, e, f) rx_syscall((n), (int64_t)(a), (int64_t)(b), (int64_t)(c), (int64_t)(d), (int64_t)(e), (int64_t)(f))

static inline rx_status_t rx_handle_close(rx_handle_t h) { return RX_SYS(0x07, h, 0, 0, 0); }
static inline rx_status_t rx_handle_duplicate(rx_handle_t h, uint32_t rights, rx_handle_t *out) { return RX_SYS(0x31, h, rights, out, 0); }
static inline rx_status_t rx_event_create(uint32_t options, rx_handle_t *out) { return RX_SYS(0x23, options, out, 0, 0); }
static inline rx_status_t rx_eventpair_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1) { return RX_SYS(0x24, options, out0, out1, 0); }
static inline rx_status_t rx_object_signal(rx_handle_t h, uint32_t clear, uint32_t set) { return RX_SYS(0x25, h, clear, set, 0); }
static inline rx_status_t rx_object_signal_peer(rx_handle_t h, uint32_t clear, uint32_t set) { return RX_SYS(0x28, h, clear, set, 0); }
static inline rx_status_t rx_object_wait_one(rx_handle_t h, uint32_t signals, uint64_t deadline, uint32_t *observed) { return RX_SYS(0x26, h, signals, deadline, observed); }
static inline rx_status_t rx_object_wait_many(rx_wait_item_t *items, uint64_t count, uint64_t deadline) { return RX_SYS(0x27, items, count, deadline, 0); }
static inline rx_status_t rx_object_get_type(rx_handle_t h) { return RX_SYS(0x29, h, 0, 0, 0); }
static inline rx_status_t rx_channel_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1) { return RX_SYS(0x20, options, out0, out1, 0); }
static inline rx_status_t rx_channel_write(rx_handle_t h, uint32_t options, const void *bytes, uint32_t num_bytes,
                                           const rx_handle_t *handles, uint32_t num_handles) {
    return RX_SYS6(0x21, h, options, bytes, num_bytes, handles, num_handles);
}
// Same signature as zx_channel_read; the kernel takes the two capacities
// packed into one register and the two counts through one pointer.
static inline rx_status_t rx_channel_read(rx_handle_t h, uint32_t options, void *bytes, rx_handle_t *handles,
                                          uint32_t num_bytes, uint32_t num_handles,
                                          uint32_t *actual_bytes, uint32_t *actual_handles) {
    uint32_t actual[2] = {0, 0};
    rx_status_t r = RX_SYS6(0x22, h, options, bytes, handles,
                            (uint64_t)num_bytes | ((uint64_t)num_handles << 32), actual);
    if (actual_bytes) *actual_bytes = actual[0];
    if (actual_handles) *actual_handles = actual[1];
    return r;
}
// Sockets: byte streams (pipes). Write takes as much as fits (64 KiB wait
// in each direction); both return SHOULD_WAIT when they can't proceed and
// PEER_CLOSED when the other end is gone (for read: and nothing is left).
static inline rx_status_t rx_socket_create(uint32_t options, rx_handle_t *out0, rx_handle_t *out1) {
    return RX_SYS(0x2A, options, out0, out1, 0);
}
static inline rx_status_t rx_socket_write(rx_handle_t h, uint32_t options, const void *buf, uint64_t len, uint64_t *actual) {
    return RX_SYS6(0x2B, h, options, buf, len, actual, 0);
}
static inline rx_status_t rx_socket_read(rx_handle_t h, uint32_t options, void *buf, uint64_t len, uint64_t *actual) {
    return RX_SYS6(0x2C, h, options, buf, len, actual, 0);
}

static inline rx_status_t rx_vmo_create(uint64_t size, uint32_t options, rx_handle_t *out) { return RX_SYS(0x10, size, options, out, 0); }
static inline rx_status_t rx_vmo_read(rx_handle_t h, void *buf, uint64_t offset, uint64_t len) { return RX_SYS(0x11, h, buf, offset, len); }
static inline rx_status_t rx_vmo_write(rx_handle_t h, const void *buf, uint64_t offset, uint64_t len) { return RX_SYS(0x12, h, buf, offset, len); }
static inline rx_status_t rx_vmo_create_child(rx_handle_t h, uint32_t options, uint64_t offset, uint64_t size, rx_handle_t *out) {
    return RX_SYS6(0x13, h, options, offset, size, out, 0);
}
static inline rx_status_t rx_vmo_get_size(rx_handle_t h, uint64_t *size) { return RX_SYS(0x17, h, size, 0, 0); }
// Same signature as zx_vmar_map; the kernel takes options and the VMO
// handle packed into one register.
static inline rx_status_t rx_vmar_map(rx_handle_t vmar, uint32_t options, uint64_t vmar_offset, rx_handle_t vmo,
                                      uint64_t vmo_offset, uint64_t len, uint64_t *mapped_addr) {
    return RX_SYS6(0x14, vmar, (uint64_t)options | ((uint64_t)vmo << 32), vmar_offset, vmo_offset, len, mapped_addr);
}
static inline rx_status_t rx_vmar_unmap(rx_handle_t vmar, uint64_t addr, uint64_t len) { return RX_SYS(0x15, vmar, addr, len, 0); }
static inline rx_status_t rx_vmar_protect(rx_handle_t vmar, uint32_t options, uint64_t addr, uint64_t len) { return RX_SYS(0x16, vmar, options, addr, len); }
static inline rx_status_t rx_vmo_replace_as_executable(rx_handle_t h, rx_handle_t vmex, rx_handle_t *out) { return RX_SYS(0x18, h, vmex, out, 0); }

// Tasks
static inline rx_status_t rx_process_create(rx_handle_t job, const char *name, uint64_t name_len, uint32_t options,
                                            rx_handle_t *proc_out, rx_handle_t *vmar_out) {
    return RX_SYS6(0x01, job, name, name_len, options, proc_out, vmar_out);
}
static inline rx_status_t rx_process_start(rx_handle_t proc, rx_handle_t thread, uint64_t entry, uint64_t stack,
                                           rx_handle_t arg1, uint64_t arg2) {
    return RX_SYS6(0x02, proc, thread, entry, stack, arg1, arg2);
}
static inline rx_status_t rx_thread_create(rx_handle_t proc, const char *name, uint64_t name_len, uint32_t options, rx_handle_t *out) {
    return RX_SYS6(0x08, proc, name, name_len, options, out, 0);
}
static inline rx_status_t rx_thread_start(rx_handle_t thread, uint64_t entry, uint64_t stack, uint64_t arg1, uint64_t arg2) {
    return RX_SYS6(0x04, thread, entry, stack, arg1, arg2, 0);
}
static inline void rx_thread_exit(void) { RX_SYS(0x05, 0, 0, 0, 0); for (;;) {} }
static inline rx_status_t rx_task_kill(rx_handle_t h) { return RX_SYS(0x09, h, 0, 0, 0); }
static inline rx_status_t rx_object_get_info(rx_handle_t h, uint32_t topic, void *buf, uint64_t size, uint64_t *actual, uint64_t *avail) {
    return RX_SYS6(0x0A, h, topic, buf, size, actual, avail);
}
static inline rx_status_t rx_object_get_child(rx_handle_t h, uint64_t koid, uint32_t rights, rx_handle_t *out) {
    return RX_SYS(0x0C, h, koid, rights, out);
}
static inline rx_status_t rx_object_get_property(rx_handle_t h, uint32_t prop, void *buf, uint64_t len) {
    return RX_SYS(0x0D, h, prop, buf, len);
}
static inline rx_status_t rx_job_create(rx_handle_t parent, uint32_t options, rx_handle_t *out) { return RX_SYS(0x30, parent, options, out, 0); }
static inline rx_status_t rx_handle_replace(rx_handle_t h, uint32_t rights, rx_handle_t *out) { return RX_SYS(0x32, h, rights, out, 0); }

// Drivers (need the root resource)
static inline rx_status_t rx_interrupt_create(rx_handle_t resource, uint32_t irq, uint32_t options, rx_handle_t *out) {
    return RX_SYS(0x80, resource, irq, options, out);
}
static inline rx_status_t rx_interrupt_wait(rx_handle_t h, uint64_t *timestamp) { return RX_SYS(0x81, h, timestamp, 0, 0); }
static inline rx_status_t rx_interrupt_ack(rx_handle_t h) { return RX_SYS(0x82, h, 0, 0, 0); }
static inline rx_status_t rx_interrupt_destroy(rx_handle_t h) { return RX_SYS(0x83, h, 0, 0, 0); }
static inline rx_status_t rx_ioports_request(rx_handle_t resource, uint16_t port, uint32_t len) { return RX_SYS(0x84, resource, port, len, 0); }
static inline rx_status_t rx_ioports_release(rx_handle_t resource, uint16_t port, uint32_t len) { return RX_SYS(0x85, resource, port, len, 0); }
static inline uint8_t rx_inb(uint16_t port) {
    uint8_t v;
    __asm__ volatile("inb %1, %0" : "=a"(v) : "Nd"(port));
    return v;
}
static inline void rx_outb(uint16_t port, uint8_t v) { __asm__ volatile("outb %0, %1" : : "a"(v), "Nd"(port)); }

// Debugging / tests
static inline int64_t rx_debug_cpu(void) { return RX_SYS(0x51, 0, 0, 0, 0); }
static inline int64_t rx_debug_cpu_count(void) { return RX_SYS(0x52, 0, 0, 0, 0); }
static inline void rx_yield(void) { RX_SYS(0x72, 0, 0, 0, 0); }

// Debug log (needs the root resource): prints on the kernel console
static inline rx_status_t rx_debuglog_create(rx_handle_t resource, uint32_t options, rx_handle_t *out) {
    return RX_SYS(0x53, resource, options, out, 0);
}
static inline rx_status_t rx_debuglog_write(rx_handle_t h, uint32_t options, const void *buf, uint64_t len) {
    return RX_SYS(0x54, h, options, buf, len);
}
// With RX_LOG_FLAG_READABLE, debuglog_create needs no resource and the
// handle reads the kernel log: debuglog_read returns the byte count, or
// RX_ERR_SHOULD_WAIT when everything has been read
#define RX_LOG_FLAG_READABLE 0x40000000u
static inline rx_status_t rx_debuglog_read(rx_handle_t h, uint32_t options, void *buf, uint64_t len) {
    return RX_SYS(0x55, h, options, buf, len);
}

// Resources: a narrower resource from the root resource
#define RX_RSRC_KIND_POWER 1
static inline rx_status_t rx_resource_create(rx_handle_t parent, uint32_t kind, rx_handle_t *out) {
    return RX_SYS(0x86, parent, kind, out, 0);
}

// System information and power
typedef struct {
    uint64_t total_bytes;     // memory the kernel manages
    uint64_t free_bytes;
    uint64_t boot_time;       // monotonic ns when the kernel started its clock
    uint64_t uptime;          // ns
    uint64_t cpus;            // online
    uint64_t idle_time;       // ns all CPUs spent halted
    uint64_t threads;         // running threads
} rx_system_stats_t;
static inline rx_status_t rx_system_get_version(char *buf, uint64_t len) { return RX_SYS(0x90, buf, len, 0, 0); }
static inline rx_status_t rx_system_get_stats(rx_system_stats_t *s) { return RX_SYS(0x91, s, sizeof *s, 0, 0); }
#define RX_SYSTEM_POWERCTL_REBOOT   1u
#define RX_SYSTEM_POWERCTL_POWEROFF 2u
// Needs the root or the power resource; returns only on failure
static inline rx_status_t rx_system_powerctl(rx_handle_t resource, uint32_t cmd) { return RX_SYS(0x92, resource, cmd, 0, 0); }

#define RX_CLOCK_MONOTONIC 0
#define RX_CLOCK_UTC       1
static inline uint64_t rx_clock_get_monotonic(void) { return (uint64_t)RX_SYS(0x40, RX_CLOCK_MONOTONIC, 0, 0, 0); }
// ns since 1970 (UTC), from the real-time clock at boot
static inline int64_t rx_clock_get_utc(void) { return RX_SYS(0x40, RX_CLOCK_UTC, 0, 0, 0); }
// Needs the root resource
static inline rx_status_t rx_clock_set_utc(rx_handle_t resource, int64_t utc_ns) { return RX_SYS(0x44, resource, utc_ns, 0, 0); }
static inline rx_status_t rx_timer_create(uint32_t options, uint32_t clock_id, rx_handle_t *out) { return RX_SYS(0x41, options, clock_id, out, 0); }
static inline rx_status_t rx_timer_set(rx_handle_t h, uint64_t deadline, uint64_t slack) { return RX_SYS(0x42, h, deadline, slack, 0); }
static inline rx_status_t rx_timer_cancel(rx_handle_t h) { return RX_SYS(0x43, h, 0, 0, 0); }

// Ends the process at once (no stdio flush; use exit() for that)
__attribute__((noreturn)) static inline void rx_exit(int64_t code) {
    RX_SYS(0x06, code, 0, 0, 0);
    __builtin_unreachable();
}

#endif


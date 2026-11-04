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
#define RX_PA_FD          0x30    // a file descriptor; argument = fd number
#define RX_PA_ROOT_RESOURCE 0x3F  // the root resource (init, drivers)
#define RX_PA_KERNEL_INPUT  0xE0  // Rustux: kernel input channel (init)
#define RX_PA_USER0       0xF0    // handles a parent passes to a child
#define RX_PA_USER1       0xF1
#define RX_PA_USER2       0xF2
#define RX_PA_HND(type, arg) (((uint32_t)(type) & 0xFF) | (((uint32_t)(arg) & 0xFFFF) << 16))

#define RX_INFO_PROCESS 3
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

// Arguments and environment for spawn_process: NUL-terminated strings
// back to back (at most 32 KiB together)
typedef struct {
    const char *args;
    uint32_t args_len;
    uint32_t argc;
    const char *env;
    uint32_t env_len;
    uint32_t envc;
} rx_spawn_strings_t;

// Start /bin/<name> from the ramdisk; `handles` (moved, may be NULL, at
// most 16) go into the child's bootstrap message with the given info
// words. Most programs use rx_spawn() from <rustux/process.h> instead.
static inline int64_t rx_spawn_process(const char *path, rx_handle_t *proc_out,
                                       const rx_handle_t *handles, const uint32_t *tags, uint32_t count,
                                       const rx_spawn_strings_t *strings) {
    return RX_SYS6(0x0B, path, proc_out, handles, tags, count, strings);
}

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

static inline uint64_t rx_clock_get_monotonic(void) { return (uint64_t)RX_SYS(0x40, 0, 0, 0, 0); }
static inline rx_status_t rx_timer_create(uint32_t options, uint32_t clock_id, rx_handle_t *out) { return RX_SYS(0x41, options, clock_id, out, 0); }
static inline rx_status_t rx_timer_set(rx_handle_t h, uint64_t deadline, uint64_t slack) { return RX_SYS(0x42, h, deadline, slack, 0); }
static inline rx_status_t rx_timer_cancel(rx_handle_t h) { return RX_SYS(0x43, h, 0, 0, 0); }

// Ends the process at once (no stdio flush; use exit() for that)
__attribute__((noreturn)) static inline void rx_exit(int64_t code) {
    RX_SYS(0x06, code, 0, 0, 0);
    __builtin_unreachable();
}

#endif


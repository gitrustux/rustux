// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ipctest: processes, threads and jobs, and two processes talking over a
// channel (the "done when" test of checklist step 3).
//
// Child processes run a copy of this program's own image: the parent copies
// its code and data into a VMO, maps it at the same address in the child,
// gives the child a stack, and starts it at one of the child_* functions
// below with a channel handle as its argument.

#include "rx.h"

extern char __image_start[], __image_end[];

static int passed, failed;
#define MS 1000000ull

static void check(const char *name, int ok, int64_t got) {
    rx_puts(ok ? "  ok   " : "  FAIL ");
    rx_puts(name);
    if (!ok) {
        rx_puts(" (got ");
        rx_putd(got);
        rx_puts(")");
    }
    rx_puts("\n");
    if (ok) passed++; else failed++;
}

static int same(const char *a, const char *b, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) if (a[i] != b[i]) return 0;
    return 1;
}

static void sleep_ms(uint64_t ms) {
    rx_handle_t t;
    rx_timer_create(0, 0, &t);
    rx_timer_set(t, rx_clock_get_monotonic() + ms * MS, 0);
    rx_object_wait_one(t, RX_TIMER_SIGNALED, RX_TIME_INFINITE, 0);
    rx_handle_close(t);
}

// A fresh 64 KiB stack mapped in `vmar`; returns the initial stack pointer
static uint64_t new_stack(rx_handle_t vmar) {
    rx_handle_t vmo;
    uint64_t addr = 0;
    rx_vmo_create(65536, 0, &vmo);
    rx_vmar_map(vmar, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 65536, &addr);
    rx_handle_close(vmo);
    return addr + 65536 - 8; // as if called: 8 below a 16-byte boundary
}

typedef void (*entry_fn)(rx_handle_t, uint64_t);

// Create a process in `job` running a copy of this image, starting at
// `entry` with `arg1` (moved into the child) in RDI
static rx_status_t make_child(rx_handle_t job, const char *name, entry_fn entry, rx_handle_t arg1,
                              rx_handle_t *proc_out, rx_handle_t *thread_out) {
    rx_handle_t proc, vmar, thread, img, exec;
    uint64_t lo = (uint64_t)__image_start & ~0xFFFull;
    uint64_t hi = ((uint64_t)__image_end + 0xFFF) & ~0xFFFull;
    uint64_t addr;
    rx_status_t r = rx_process_create(job, name, rx_strlen(name), 0, &proc, &vmar);
    if (r != RX_OK) return r;
    rx_vmo_create(hi - lo, 0, &img);
    rx_vmo_write(img, (const void *)lo, 0, hi - lo);
    rx_vmo_replace_as_executable(img, 0, &exec);
    r = rx_vmar_map(vmar, RX_VM_PERM_READ | RX_VM_PERM_WRITE | RX_VM_PERM_EXECUTE | RX_VM_SPECIFIC,
                    lo - RX_ROOT_VMAR_BASE, exec, 0, hi - lo, &addr);
    rx_handle_close(exec);
    if (r != RX_OK) return r;
    uint64_t sp = new_stack(vmar);
    rx_handle_close(vmar);
    rx_thread_create(proc, "main", 4, 0, &thread);
    r = rx_process_start(proc, thread, (uint64_t)entry, sp, arg1, 0);
    *proc_out = proc;
    *thread_out = thread;
    return r;
}

// ---- code that runs in the children ----

// Waits 50 ms (so the parent is already blocked), sends a message with an
// event handle, waits until the parent signals that event, says bye, exits 42
static void child_talker(rx_handle_t ch, uint64_t arg) {
    (void)arg;
    rx_handle_t ev, dup;
    sleep_ms(50);
    rx_event_create(0, &ev);
    rx_handle_duplicate(ev, RX_RIGHT_SAME_RIGHTS, &dup);
    rx_channel_write(ch, 0, "hello parent", 12, &dup, 1);
    rx_object_wait_one(ev, RX_EVENT_SIGNALED, RX_TIME_INFINITE, 0);
    rx_channel_write(ch, 0, "bye", 3, 0, 0);
    rx_exit(42);
}

// Blocks forever on its channel (until killed)
static void child_sleeper(rx_handle_t ch, uint64_t arg) {
    (void)arg;
    rx_object_wait_one(ch, RX_CHANNEL_READABLE, RX_TIME_INFINITE, 0);
    rx_exit(0);
}

// A second thread in our own process
static volatile uint64_t shared_value;
static void thread_fn(rx_handle_t ev, uint64_t value) {
    shared_value = value;
    rx_object_signal(ev, 0, RX_EVENT_SIGNALED);
    rx_thread_exit();
}

// ---- the tests ----

static void two_processes(void) {
    rx_handle_t c0, c1, proc, thread, got[2];
    uint32_t observed = 0, nb = 0, nh = 0;
    char buf[32];
    rx_info_process_t info;
    int64_t r;
    rx_puts("two processes over a channel:\n");

    rx_channel_create(0, &c0, &c1);
    // Timed from before the child exists: on several CPUs it starts (and
    // its 50 ms sleep begins) while we are still returning from the call
    uint64_t t0 = rx_clock_get_monotonic();
    r = make_child(rx_job_default, "talker", child_talker, c1, &proc, &thread);
    check("process_create + process_start (child gets one channel end)", r == RX_OK, r);
    check("  our end of the moved handle is gone", rx_object_get_type(c1) == RX_ERR_BAD_HANDLE, 0);

    r = rx_object_wait_one(c0, RX_CHANNEL_READABLE, RX_TIME_INFINITE, &observed);
    uint64_t waited = (rx_clock_get_monotonic() - t0) / MS;
    check("wait_one blocks until the child writes", r == RX_OK && (observed & RX_CHANNEL_READABLE), r);
    check("  (blocked for about 50 ms)", waited >= 40, (int64_t)waited);
    r = rx_channel_read(c0, 0, buf, got, sizeof buf, 2, &nb, &nh);
    check("message from the child", r == RX_OK && nb == 12 && same(buf, "hello parent", 12), r);
    check("  carrying one handle: an event", nh == 1 && rx_object_get_type(got[0]) == RX_OBJ_TYPE_EVENT, nh);
    r = rx_object_signal(got[0], 0, RX_EVENT_SIGNALED);
    check("signal the child's event through our handle", r == RX_OK, r);
    r = rx_object_wait_one(c0, RX_CHANNEL_READABLE, rx_clock_get_monotonic() + 1000 * MS, 0);
    rx_channel_read(c0, 0, buf, 0, sizeof buf, 0, &nb, 0);
    check("child saw the signal and replied", r == RX_OK && nb == 3 && same(buf, "bye", 3), r);

    r = rx_object_wait_one(proc, RX_PROCESS_TERMINATED, rx_clock_get_monotonic() + 1000 * MS, 0);
    check("process handle gets TERMINATED", r == RX_OK, r);
    r = rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    check("  exit code 42", r == RX_OK && info.return_code == 42, info.return_code);
    check("  flags STARTED | EXITED", info.flags == (RX_INFO_PROCESS_FLAG_STARTED | RX_INFO_PROCESS_FLAG_EXITED), info.flags);
    r = rx_object_wait_one(thread, RX_THREAD_TERMINATED, 0, 0);
    check("  its thread is TERMINATED too", r == RX_OK, r);
    r = rx_object_wait_one(c0, RX_PEER_CLOSED, 0, 0);
    check("  and its channel end was closed", r == RX_OK, r);
    r = rx_process_start(proc, thread, (uint64_t)child_talker, 0, 0, 0);
    check("process_start on a finished process -> BAD_STATE", r == RX_ERR_BAD_STATE, r);
    rx_handle_close(proc);
    rx_handle_close(thread);
    rx_handle_close(got[0]);
    rx_handle_close(c0);
}

static void threads(void) {
    rx_handle_t ev, t2;
    rx_puts("threads:\n");
    check("bootstrap gave us our process, thread, job and root VMAR",
          rx_object_get_type(rx_proc_self) == RX_OBJ_TYPE_PROCESS &&
          rx_object_get_type(rx_thread_self) == RX_OBJ_TYPE_THREAD &&
          rx_object_get_type(rx_job_default) == RX_OBJ_TYPE_JOB &&
          rx_object_get_type(rx_vmar_root) == RX_OBJ_TYPE_VMAR, 0);
    rx_event_create(0, &ev);
    int64_t r = rx_thread_create(rx_proc_self, "worker", 6, 0, &t2);
    check("thread_create in our own process", r == RX_OK, r);
    r = rx_thread_start(t2, (uint64_t)thread_fn, new_stack(rx_vmar_root), ev, 1234);
    check("thread_start", r == RX_OK, r);
    r = rx_object_wait_one(ev, RX_EVENT_SIGNALED, rx_clock_get_monotonic() + 1000 * MS, 0);
    check("second thread ran and signalled", r == RX_OK && shared_value == 1234, (int64_t)shared_value);
    r = rx_object_wait_one(t2, RX_THREAD_TERMINATED, rx_clock_get_monotonic() + 1000 * MS, 0);
    check("  and exited (thread TERMINATED, process still running)", r == RX_OK, r);
    r = rx_thread_start(t2, (uint64_t)thread_fn, 0, ev, 0);
    check("thread_start twice -> BAD_STATE", r == RX_ERR_BAD_STATE, r);
    rx_handle_close(t2);
    rx_handle_close(ev);
}

static void jobs_and_kill(void) {
    rx_handle_t job, c0, c1, proc, thread, ev, p2, v2;
    rx_info_process_t info;
    int64_t r;
    rx_puts("jobs and task_kill:\n");

    r = rx_job_create(rx_job_default, 0, &job);
    check("job_create", r == RX_OK && rx_object_get_type(job) == RX_OBJ_TYPE_JOB, r);
    rx_channel_create(0, &c0, &c1);
    r = make_child(job, "sleeper", child_sleeper, c1, &proc, &thread);
    check("start a child in the new job", r == RX_OK, r);
    sleep_ms(20); // let it block inside the kernel
    r = rx_task_kill(job);
    check("task_kill(job)", r == RX_OK, r);
    r = rx_object_wait_one(proc, RX_PROCESS_TERMINATED, rx_clock_get_monotonic() + 1000 * MS, 0);
    check("blocked child is killed", r == RX_OK, r);
    rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    check("  exit code -1024 (killed)", info.return_code == RX_TASK_RETCODE_SYSCALL_KILL, info.return_code);
    r = rx_object_wait_one(c0, RX_PEER_CLOSED, 0, 0);
    check("  the handle it was waiting on got closed", r == RX_OK, r);
    r = rx_process_create(job, "late", 4, 0, &p2, &v2);
    check("process_create in a killed job -> BAD_STATE", r == RX_ERR_BAD_STATE, r);
    r = rx_task_kill(proc);
    check("killing a dead process is harmless", r == RX_OK, r);

    rx_event_create(0, &ev);
    r = rx_process_create(ev, "x", 1, 0, &p2, &v2);
    check("process_create with an event as job -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_process_create(rx_job_default, "unstarted", 9, 0, &p2, &v2);
    r = rx_process_start(p2, rx_thread_self, 0, 0, 0, 0);
    check("process_start with another process's thread -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_task_kill(p2);
    r = rx_object_wait_one(p2, RX_PROCESS_TERMINATED, 0, 0);
    check("killing a never-started process terminates it", r == RX_OK, r);
    rx_handle_close(p2);
    rx_handle_close(v2);
    rx_handle_close(ev);
    rx_handle_close(proc);
    rx_handle_close(thread);
    rx_handle_close(c0);
    rx_handle_close(job);
}

int main(void) {
    rx_puts("ipctest: processes, threads, jobs, channels between processes\n");
    two_processes();
    threads();
    jobs_and_kill();
    rx_puts("ipctest: ");
    rx_putd(passed);
    rx_puts(" passed, ");
    rx_putd(failed);
    rx_puts(failed ? " FAILED\n" : " failed\n");
    rx_exit(failed);
}

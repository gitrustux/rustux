// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// smptest: threads running on several CPUs at once (checklist step 6).
//
// 1. Four CPU-bound threads land on different CPUs and finish faster
//    together than one after another.
// 2. SSE registers survive syscalls, preemption and moving to another CPU.
// 3. TLB shootdown: a thread on another CPU sees a remapped page at once.
// 4. Ping-pong over channels between two threads (wake-ups across CPUs).

#include "rx.h"

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

static void note(const char *name, int64_t v, const char *unit) {
    rx_puts("  info ");
    rx_puts(name);
    rx_putd(v);
    rx_puts(unit);
    rx_puts("\n");
}

static uint64_t new_stack(void) {
    rx_handle_t vmo;
    uint64_t addr = 0;
    rx_vmo_create(65536, 0, &vmo);
    rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 65536, &addr);
    rx_handle_close(vmo);
    return addr + 65536 - 8;
}

typedef void (*thread_fn)(rx_handle_t, uint64_t);

static rx_handle_t start_thread(thread_fn fn, rx_handle_t arg1, uint64_t arg2) {
    rx_handle_t t = 0;
    rx_thread_create(rx_proc_self, "w", 1, 0, &t);
    rx_thread_start(t, (uint64_t)fn, new_stack(), arg1, arg2);
    return t;
}

static void wait_thread(rx_handle_t t) {
    rx_object_wait_one(t, RX_THREAD_TERMINATED, RX_TIME_INFINITE, 0);
    rx_handle_close(t);
}

// ---- 1. parallel work ----

#define WORKERS 4
static volatile uint64_t work_units;
static volatile uint32_t cpus_seen[WORKERS];

static void spin(uint64_t units) {
    volatile uint64_t x = 0;
    for (uint64_t i = 0; i < units; i++) x += i;
}

static void worker(rx_handle_t done, uint64_t idx) {
    uint32_t seen = 0;
    for (int part = 0; part < 16; part++) {
        seen |= 1u << rx_debug_cpu();
        spin(work_units / 16);
    }
    cpus_seen[idx] = seen;
    rx_object_signal(done, 0, RX_EVENT_SIGNALED);
    rx_thread_exit();
}

static void parallel(int cpus) {
    rx_handle_t ev[WORKERS], th[WORKERS];
    rx_puts("parallel work:\n");
    // Calibrate: about 150 ms of work for one thread
    uint64_t t0 = rx_clock_get_monotonic();
    spin(1000000);
    uint64_t per_million = rx_clock_get_monotonic() - t0;
    work_units = per_million ? (150 * MS * 1000000ull) / per_million : 10000000;

    t0 = rx_clock_get_monotonic();
    for (int i = 0; i < WORKERS; i++) {
        rx_event_create(0, &ev[i]);
        th[i] = start_thread(worker, ev[i], i);
    }
    for (int i = 0; i < WORKERS; i++) {
        rx_object_wait_one(ev[i], RX_EVENT_SIGNALED, RX_TIME_INFINITE, 0);
        rx_handle_close(ev[i]);
        wait_thread(th[i]);
    }
    uint64_t together = rx_clock_get_monotonic() - t0;

    uint32_t all = 0;
    for (int i = 0; i < WORKERS; i++) all |= cpus_seen[i];
    int distinct = 0;
    for (int b = 0; b < 32; b++) if (all & (1u << b)) distinct++;
    note("CPUs online: ", cpus, "");
    note("CPUs the 4 workers ran on: ", distinct, "");
    note("4 x 150 ms of work took ", (int64_t)(together / MS), " ms");
    if (cpus >= 2) {
        check("workers ran on more than one CPU", distinct >= 2, distinct);
        // One CPU would need about 600 ms; allow generous slack
        check("4 workers finish faster than one CPU could", together < 450 * MS, (int64_t)(together / MS));
    } else {
        check("one CPU: all workers on CPU 0", all == 1, all);
    }
}

// ---- 2. SSE registers ----

static void fill_xmm(uint64_t seed) {
    __asm__ volatile(
        "movq %0, %%rax\n"
        "movq %%rax, %%xmm0\n inc %%rax\n movq %%rax, %%xmm1\n inc %%rax\n"
        "movq %%rax, %%xmm2\n inc %%rax\n movq %%rax, %%xmm3\n inc %%rax\n"
        "movq %%rax, %%xmm4\n inc %%rax\n movq %%rax, %%xmm5\n inc %%rax\n"
        "movq %%rax, %%xmm6\n inc %%rax\n movq %%rax, %%xmm7\n inc %%rax\n"
        "movq %%rax, %%xmm8\n inc %%rax\n movq %%rax, %%xmm9\n inc %%rax\n"
        "movq %%rax, %%xmm10\n inc %%rax\n movq %%rax, %%xmm11\n inc %%rax\n"
        "movq %%rax, %%xmm12\n inc %%rax\n movq %%rax, %%xmm13\n inc %%rax\n"
        "movq %%rax, %%xmm14\n inc %%rax\n movq %%rax, %%xmm15\n"
        : : "r"(seed) : "rax");
}

// Number of XMM registers that do not hold their expected value
static uint64_t check_xmm(uint64_t seed) {
    uint64_t bad;
    __asm__ volatile(
        "xorq %0, %0\n"
        "movq %1, %%rcx\n"
        "movq %%xmm0, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm1, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm2, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm3, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm4, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm5, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm6, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm7, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm8, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm9, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm10, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm11, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm12, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm13, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm14, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n inc %%rcx\n"
        "movq %%xmm15, %%rdx\n cmpq %%rcx, %%rdx\n setne %%dl\n movzbq %%dl, %%rdx\n addq %%rdx, %0\n"
        : "=&r"(bad) : "r"(seed) : "rcx", "rdx", "cc");
    return bad;
}

static volatile uint64_t sse_bad[WORKERS], sse_moves[WORKERS];

static void sse_worker(rx_handle_t done, uint64_t idx) {
    uint64_t bad = 0, moves = 0;
    for (int round = 0; round < 200; round++) {
        uint64_t seed = (idx + 1) * 0x0101010100000000ull + (uint64_t)round * 32;
        fill_xmm(seed);
        int64_t before = rx_debug_cpu();      // syscall
        spin(work_units / 400);                // long enough to be preempted
        rx_yield();                            // syscall that switches
        int64_t after = rx_debug_cpu();
        bad += check_xmm(seed);
        if (before != after) moves++;
    }
    sse_bad[idx] = bad;
    sse_moves[idx] = moves;
    rx_object_signal(done, 0, RX_EVENT_SIGNALED);
    rx_thread_exit();
}

static void sse(int cpus) {
    rx_handle_t ev[WORKERS], th[WORKERS];
    rx_puts("SSE registers across switches:\n");
    for (int i = 0; i < WORKERS; i++) {
        rx_event_create(0, &ev[i]);
        th[i] = start_thread(sse_worker, ev[i], i);
    }
    uint64_t bad = 0, moves = 0;
    for (int i = 0; i < WORKERS; i++) {
        rx_object_wait_one(ev[i], RX_EVENT_SIGNALED, RX_TIME_INFINITE, 0);
        rx_handle_close(ev[i]);
        wait_thread(th[i]);
        bad += sse_bad[i];
        moves += sse_moves[i];
    }
    note("rounds where the thread changed CPU: ", (int64_t)moves, " of 800");
    check("all 16 XMM registers intact after every round", bad == 0, (int64_t)bad);
    if (cpus >= 2) check("threads moved between CPUs mid-round", moves > 0, (int64_t)moves);
}

// ---- 3. TLB shootdown ----

#define TLB_ADDR 0x50000000ull
// 0 start, 1 reader is reading, 2 main asks to pause, 3 reader paused
// (the translation stays in its TLB), 4 remapped: read again, 5 done
static volatile int tlb_phase;
static volatile uint64_t tlb_seen, tlb_reader_cpu;

static void tlb_reader(rx_handle_t done, uint64_t arg) {
    (void)arg;
    volatile uint64_t *p = (volatile uint64_t *)(RX_ROOT_VMAR_BASE + TLB_ADDR);
    while (tlb_phase < 2) {           // keep the translation hot
        tlb_seen = *p;
        tlb_reader_cpu = rx_debug_cpu();
        tlb_seen = *p;
        if (tlb_phase == 0) tlb_phase = 1;
    }
    tlb_phase = 3;                     // stop touching the page
    while (tlb_phase < 4) {}
    tlb_seen = *p;                     // after the remap: must be the new page
    tlb_phase = 5;
    rx_object_signal(done, 0, RX_EVENT_SIGNALED);
    rx_thread_exit();
}

static void tlb(int cpus) {
    rx_handle_t a, b, ev, th;
    uint64_t addr;
    rx_puts("TLB shootdown:\n");
    rx_vmo_create(4096, 0, &a);
    rx_vmo_create(4096, 0, &b);
    uint64_t one = 1, two = 2;
    rx_vmo_write(a, &one, 0, 8);
    rx_vmo_write(b, &two, 0, 8);
    rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_SPECIFIC, TLB_ADDR, a, 0, 4096, &addr);
    rx_event_create(0, &ev);
    th = start_thread(tlb_reader, ev, 0);
    // Spin (no yield) until the reader is reading on another CPU, so its
    // TLB, not ours, holds the old translation
    uint64_t give_up = rx_clock_get_monotonic() + 1000 * MS;
    int64_t me = rx_debug_cpu();
    while ((tlb_phase < 1 || (int64_t)tlb_reader_cpu == me) && rx_clock_get_monotonic() < give_up) {
        me = rx_debug_cpu();
    }
    tlb_phase = 2;
    while (tlb_phase < 3) {}
    rx_vmar_unmap(rx_vmar_root, addr, 4096);
    rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_SPECIFIC, TLB_ADDR, b, 0, 4096, &addr);
    tlb_phase = 4;
    rx_object_wait_one(ev, RX_EVENT_SIGNALED, rx_clock_get_monotonic() + 2000 * MS, 0);
    wait_thread(th);
    if (cpus >= 2 && (int64_t)tlb_reader_cpu == me) note("(reader shared our CPU; result not conclusive) cpu ", me, "");
    check("reader on another CPU sees the new page", tlb_seen == 2, (int64_t)tlb_seen);
    rx_vmar_unmap(rx_vmar_root, addr, 4096);
    rx_handle_close(a);
    rx_handle_close(b);
    rx_handle_close(ev);
}

// ---- 4. ping-pong ----

#define ROUNDS 2000
static void ponger(rx_handle_t ch, uint64_t arg) {
    (void)arg;
    uint32_t v, n;
    for (int i = 0; i < ROUNDS; i++) {
        rx_object_wait_one(ch, RX_CHANNEL_READABLE, RX_TIME_INFINITE, 0);
        rx_channel_read(ch, 0, &v, 0, 4, 0, &n, 0);
        v++;
        rx_channel_write(ch, 0, &v, 4, 0, 0);
    }
    rx_handle_close(ch);
    rx_thread_exit();
}

static void pingpong(void) {
    rx_handle_t c0, c1, th;
    uint32_t v = 0, n;
    rx_puts("ping-pong between threads:\n");
    rx_channel_create(0, &c0, &c1);
    th = start_thread(ponger, c1, 0);
    uint64_t t0 = rx_clock_get_monotonic();
    int ok = 1;
    for (int i = 0; i < ROUNDS && ok; i++) {
        uint32_t sent = v;
        rx_channel_write(c0, 0, &v, 4, 0, 0);
        if (rx_object_wait_one(c0, RX_CHANNEL_READABLE, rx_clock_get_monotonic() + 1000 * MS, 0) != RX_OK) ok = 0;
        else rx_channel_read(c0, 0, &v, 0, 4, 0, &n, 0);
        if (v != sent + 1) ok = 0;
    }
    uint64_t took = rx_clock_get_monotonic() - t0;
    wait_thread(th);
    note("round trips per second: ", took ? (int64_t)(ROUNDS * 1000000000ull / took) : 0, "");
    check("2000 round trips, none lost", ok, v);
    rx_handle_close(c0);
}

void _start(rx_handle_t bootstrap) {
    rx_startup(bootstrap);
    int cpus = (int)rx_debug_cpu_count();
    rx_puts("smptest: threads on several CPUs\n");
    parallel(cpus);
    sse(cpus);
    tlb(cpus);
    pingpong();
    rx_puts("smptest: ");
    rx_putd(passed);
    rx_puts(" passed, ");
    rx_putd(failed);
    rx_puts(failed ? " FAILED\n" : " failed\n");
    rx_exit(failed);
}

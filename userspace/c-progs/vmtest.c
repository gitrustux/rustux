// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// vmtest: demand paging (DESKTOP.md step F1). Prints "vmtest: N passed,
// M failed" (the command tests run it).
//
//   - a 1 GiB mapping costs only the pages touched (the process's private
//     memory, the system's free memory)
//   - the kernel copies into and out of pages not touched yet (vmo_read,
//     vmo_write, channel reads, file reads)
//   - threads touching the same pages at once all see the same memory
//   - 100 threads (256 KiB stacks each: 25 MiB) start and cost little
//     more than their kernel stacks (128 KiB each); the task table holds
//     256 threads and processes in all
//   - a stack overflow hits the guard page: the thread's process is killed
//     (vmtest --overflow, in a child), not something below the stack
//   - running out of memory kills the process that touches too much
//     (vmtest --hog, in a child), and the system goes on
//
// Run as root or not; it needs a few MiB.

#include <fcntl.h>
#include <pthread.h>
#include <rustux/process.h>
#include <rustux/syscalls.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int passed, failed;

static void check(int ok, const char *what) {
    printf("  %s %s\n", ok ? "ok  " : "FAIL", what);
    if (ok) passed++;
    else failed++;
}

static uint64_t private_bytes(void) {
    rx_info_task_stats_t st;
    memset(&st, 0, sizeof st);
    if (rx_object_get_info(rx_proc_self, RX_INFO_TASK_STATS, &st, sizeof st, 0, 0) != RX_OK) return 0;
    return st.mem_private_bytes;
}

static uint64_t free_bytes(void) {
    rx_system_stats_t s;
    memset(&s, 0, sizeof s);
    rx_system_get_stats(&s);
    return s.free_bytes;
}

static char *map_new(uint64_t size, rx_handle_t *vmo_out) {
    rx_handle_t vmo;
    uint64_t addr = 0;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK) {
        rx_handle_close(vmo);
        return 0;
    }
    if (vmo_out) *vmo_out = vmo;
    else rx_handle_close(vmo);
    return (char *)addr;
}

static void test_sparse(void) {
    uint64_t before = private_bytes(), fb = free_bytes();
    char *m = map_new(1ull << 30, 0);
    check(m != 0, "a 1 GiB mapping");
    if (!m) return;
    uint64_t mapped = private_bytes();
    for (int i = 0; i < 10; i++) m[(uint64_t)i * (100ull << 20)] = (char)(i + 1);
    uint64_t touched = private_bytes(), fa = free_bytes();
    printf("       private memory: %llu KiB before, %llu KiB mapped, %llu KiB with 10 pages touched; free: %lld KiB less\n",
           (unsigned long long)before / 1024, (unsigned long long)mapped / 1024, (unsigned long long)touched / 1024,
           (long long)(fb - fa) / 1024);
    check(mapped - before < 64 * 1024, "mapping 1 GiB commits nothing");
    check(touched - mapped == 10 * 4096, "touching 10 pages commits 10 pages (40 KiB)");
    check(fb - fa < 2 * 1024 * 1024, "the system's free memory drops by little");
    int ok = 1;
    for (int i = 0; i < 10; i++) ok &= m[(uint64_t)i * (100ull << 20)] == (char)(i + 1) && m[(uint64_t)i * (100ull << 20) + 4095] == 0;
    check(ok && m[(1ull << 30) - 1] == 0, "touched pages keep their data; untouched ones read as zero");
    rx_vmar_unmap(rx_vmar_root, (uint64_t)m, 1ull << 30);
    check(private_bytes() <= mapped, "unmapping frees them");
}

static void test_kernel_copies(void) {
    rx_handle_t vmo;
    char *m = map_new(16 * 4096, &vmo);
    if (!m) {
        check(0, "a 64 KiB mapping");
        return;
    }
    // vmo_write: the kernel commits the page; the mapping then sees it
    rx_vmo_write(vmo, "from the kernel", 2 * 4096, 16);
    check(!strcmp(m + 2 * 4096, "from the kernel"), "vmo_write into a page not touched yet shows in the mapping");
    // vmo_read into a buffer page not touched yet
    rx_handle_t v2;
    rx_vmo_create(4096, 0, &v2);
    rx_vmo_write(v2, "hello", 0, 6);
    check(rx_vmo_read(v2, m + 5 * 4096, 0, 6) == RX_OK && !strcmp(m + 5 * 4096, "hello"), "vmo_read into a page not touched yet");
    // A channel message read into one
    rx_handle_t a, b;
    rx_channel_create(0, &a, &b);
    rx_channel_write(a, 0, "message", 8, 0, 0);
    uint32_t nb = 0;
    check(rx_channel_read(b, 0, m + 7 * 4096, 0, 64, 0, &nb, 0) == RX_OK && nb == 8 && !strcmp(m + 7 * 4096, "message"),
          "channel_read into a page not touched yet");
    // write() from a page not touched yet (zeros), read() into another
    int fd = open("/tmp/vmtest.tmp", O_RDWR | O_CREAT | O_TRUNC, 0600);
    int w = fd >= 0 ? (int)write(fd, m + 9 * 4096, 4096) : -1;
    lseek(fd, 0, SEEK_SET);
    write(fd, "file", 5);
    lseek(fd, 0, SEEK_SET);
    int r = fd >= 0 ? (int)read(fd, m + 11 * 4096, 4096) : -1;
    close(fd);
    unlink("/tmp/vmtest.tmp");
    check(w == 4096 && r == 4096 && !strcmp(m + 11 * 4096, "file"), "write() from and read() into pages not touched yet");
    rx_handle_close(a);
    rx_handle_close(b);
    rx_handle_close(v2);
    rx_handle_close(vmo);
    rx_vmar_unmap(rx_vmar_root, (uint64_t)m, 16 * 4096);
}

static char *shared;
static void *toucher(void *arg) {
    int me = (int)(long)arg;
    // Every thread writes its byte into each of 64 pages, in a different order
    for (int k = 0; k < 64; k++) {
        int page = (k * 7 + me * 13) % 64;
        shared[page * 4096 + me] = (char)(me + 1);
    }
    return 0;
}

static void test_threads_same_pages(void) {
    shared = map_new(64 * 4096, 0);
    pthread_t t[8];
    for (long i = 0; i < 8; i++) pthread_create(&t[i], 0, toucher, (void *)i);
    for (int i = 0; i < 8; i++) pthread_join(t[i], 0);
    int ok = 1;
    for (int p = 0; p < 64; p++)
        for (int me = 0; me < 8; me++) ok &= shared[p * 4096 + me] == (char)(me + 1);
    check(ok, "8 threads faulting the same 64 pages: every write kept");
}

static volatile int go;
static void *sleeper(void *arg) {
    (void)arg;
    while (!go) usleep(1000);
    return 0;
}

static void test_many_threads(void) {
    // 100 threads of 256 KiB stacks would be 25 MiB committed (plus 12.5 MiB
    // of kernel stacks, which are not demand paged)
    pthread_t t[100];
    int n = 0;
    uint64_t fb = free_bytes();
    for (; n < 100; n++)
        if (pthread_create(&t[n], 0, sleeper, 0)) break;
    uint64_t fa = free_bytes();
    go = 1;
    for (int i = 0; i < n; i++) pthread_join(t[i], 0);
    printf("       %d threads started; free memory %lld KiB less while they ran\n", n, (long long)(fb - fa) / 1024);
    check(n == 100, "100 threads with 256 KiB stacks start");
    check(fb - fa < 100ull * 192 * 1024, "their stacks are not committed (less than 192 KiB a thread with the kernel stack)");
}

static int child(const char *arg, int64_t *code) {
    char *argv[] = { "vmtest", (char *)arg, 0 };
    rx_handle_t proc;
    if (rx_spawn("/bin/vmtest", argv, 0, RX_SPAWN_CLONE_ALL, 0, 0, &proc) != RX_OK) return -1;
    *code = rx_wait_process(proc);
    return 0;
}

static int depth(int n) {
    volatile char pad[1024];
    pad[0] = (char)n;
    return n ? depth(n - 1) + pad[0] : 0;
}
static void *overflow(void *arg) {
    (void)arg;
    depth(1 << 20); // 1 GiB of frames: far past the 256 KiB stack
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "--overflow")) {
        pthread_t t;
        pthread_create(&t, 0, overflow, 0);
        pthread_join(t, 0);
        printf("vmtest --overflow: still running (no guard page?)\n");
        return 0;
    }
    if (argc == 2 && !strcmp(argv[1], "--hog")) {
        // Touch 1 GiB mappings until memory runs out
        for (int k = 0; k < 64; k++) {
            char *m = map_new(1ull << 30, 0);
            if (!m) return 3;
            for (uint64_t off = 0; off < (1ull << 30); off += 4096) m[off] = 1;
        }
        return 0;
    }
    printf("vmtest: demand paging\n");
    test_sparse();
    test_kernel_copies();
    test_threads_same_pages();
    test_many_threads();
    int64_t code = 0;
    int r = child("--overflow", &code);
    check(r == 0 && code == RX_TASK_RETCODE_EXCEPTION_KILL, "a stack overflow hits the guard page: the process is killed");
    uint64_t fb = free_bytes();
    r = child("--hog", &code);
    uint64_t fa = free_bytes();
    printf("       the memory hog ended with %lld; free memory before %llu MiB, after %llu MiB\n", (long long)code,
           (unsigned long long)fb >> 20, (unsigned long long)fa >> 20);
    check(r == 0 && code == RX_TASK_RETCODE_EXCEPTION_KILL, "running out of memory kills the process that touches too much");
    check(fa + (8 << 20) >= fb, "its memory is all given back");
    printf("vmtest: %d passed, %d failed\n", passed, failed);
    return failed ? 1 : 0;
}

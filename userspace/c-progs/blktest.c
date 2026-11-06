// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// blktest: checks of the block device protocol and devmgr's block list,
// against RAM disks it starts itself. Prints each check (-q: only
// failures) and "blktest: N passed, M failed"; exit code = failures.

#include <errno.h>
#include <rustux/device.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static int passed, failed, quiet;

static void check(int ok, const char *what) {
    if (!quiet || !ok) printf("  %s %s\n", ok ? "ok  " : "FAIL", what);
    if (ok) passed++;
    else failed++;
}

static void nap(long ms) {
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000 };
    nanosleep(&ts, 0);
}

// devmgr's block list; returns the count (entries in out)
static int list(rx_block_info_t *out, int max) {
    rx_handle_t dm;
    if (rx_devmgr_connect(&dm) < 0) return -1;
    static char buf[65536];
    size_t got = 0;
    int r = rx_dev_call(dm, RX_DM_OP_LIST_BLOCK, 0, 0, 0, buf, sizeof buf, &got, 0);
    rx_handle_close(dm);
    if (r < 0 || got < 4) return -1;
    uint32_t n;
    memcpy(&n, buf, 4);
    int k = 0;
    for (; k < (int)n && k < max; k++) memcpy(&out[k], buf + 4 + k * sizeof *out, sizeof *out);
    return k;
}

// Start a RAM disk and find its name (the entry that was not there before)
static rx_handle_t start_ramdisk(char *const args[], char *name) {
    rx_block_info_t before[32], after[32];
    int nb = list(before, 32);
    char *argv[8] = { "ramdisk", "-q" };
    int a = 2;
    for (int k = 0; args[k] && a < 7; k++) argv[a++] = args[k];
    argv[a] = 0;
    rx_handle_t proc;
    if (rx_spawn("/bin/ramdisk", argv, 0, RX_SPAWN_CLONE_ALL, 0, 0, &proc) != RX_OK) return 0;
    for (int tries = 0; tries < 100; tries++) {
        int na = list(after, 32);
        for (int k = 0; k < na; k++) {
            int old = 0;
            for (int j = 0; j < nb; j++) old |= !strcmp(before[j].name, after[k].name);
            if (!old) {
                strcpy(name, after[k].name);
                return proc;
            }
        }
        nap(20);
    }
    rx_task_kill(proc);
    rx_handle_close(proc);
    return 0;
}

static int in_list(const char *name, rx_block_info_t *found) {
    rx_block_info_t l[32];
    int n = list(l, 32);
    for (int k = 0; k < n; k++) {
        if (!strcmp(l[k].name, name)) {
            if (found) *found = l[k];
            return 1;
        }
    }
    return 0;
}

static int req(rx_handle_t ch, uint32_t op, uint64_t block, uint32_t count, const void *data, size_t len,
               void *out, size_t cap) {
    rx_block_req_t q = { 0, op, block, count, 0, 0 };
    return rx_block_call(ch, &q, data, len, 0, out, cap, 0);
}

int main(int argc, char **argv) {
    quiet = argc > 1 && !strcmp(argv[1], "-q");
    char name[32], line[128];
    char *rw_args[] = { "1M", 0 };
    rx_handle_t proc = start_ramdisk(rw_args, name);
    check(proc != 0, "a 1 MiB RAM disk starts and appears in devmgr's block list");
    if (!proc) goto done;

    rx_handle_t ch;
    int r = rx_block_open(name, &ch);
    snprintf(line, sizeof line, "connect to %s through devmgr", name);
    check(r == 0, line);
    if (r < 0) goto done;

    rx_block_info_t info;
    r = req(ch, RX_BLOCK_OP_GET_INFO, 0, 0, 0, 0, &info, sizeof info);
    check(r == 0 && info.block_size == 512 && info.block_count == 2048 && !(info.flags & RX_BLOCK_FLAG_READONLY),
          "GET_INFO: 2048 blocks of 512 bytes, writable");

    static uint8_t a[8192], b[8192];
    for (int k = 0; k < 8192; k++) a[k] = (uint8_t)(k * 13 + 1);
    int w = req(ch, RX_BLOCK_OP_WRITE, 10, 16, a, 8192, 0, 0);
    int rd = req(ch, RX_BLOCK_OP_READ, 10, 16, 0, 0, b, sizeof b);
    check(w == 0 && rd == 0 && !memcmp(a, b, 8192), "WRITE 16 blocks, READ them back");
    check(req(ch, RX_BLOCK_OP_READ, 2040, 16, 0, 0, b, sizeof b) == -ERANGE, "READ past the end: ERANGE");
    check(req(ch, RX_BLOCK_OP_WRITE, 0, 2, a, 100, 0, 0) == -EINVAL, "WRITE with the wrong amount of data: EINVAL");
    check(req(ch, RX_BLOCK_OP_FLUSH, 0, 0, 0, 0, 0, 0) == 0, "FLUSH");

    // Through VMOs: 64 KiB in one request
    rx_handle_t v1, v2, dup;
    uint64_t m1 = 0, m2 = 0;
    rx_vmo_create(65536, 0, &v1);
    rx_vmo_create(65536, 0, &v2);
    rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, v1, 0, 65536, &m1);
    rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, v2, 0, 65536, &m2);
    for (int k = 0; k < 65536; k++) ((uint8_t *)m1)[k] = (uint8_t)(k ^ (k >> 8));
    uint32_t id1 = 0, id2 = 0;
    rx_block_req_t q = { 0, RX_BLOCK_OP_ATTACH_VMO, 0, 0, 0, 0 };
    rx_handle_duplicate(v1, RX_RIGHT_SAME_RIGHTS, &dup);
    int at1 = rx_block_call(ch, &q, 0, 0, dup, &id1, 4, 0);
    rx_handle_duplicate(v2, RX_RIGHT_SAME_RIGHTS, &dup);
    int at2 = rx_block_call(ch, &q, 0, 0, dup, &id2, 4, 0);
    check(at1 == 0 && at2 == 0 && id1 && id2 && id1 != id2, "ATTACH_VMO twice: two VMO ids");
    rx_block_req_t wv = { 0, RX_BLOCK_OP_WRITE_VMO, 100, 128, id1, 0 };
    rx_block_req_t rv = { 0, RX_BLOCK_OP_READ_VMO, 100, 128, id2, 0 };
    int a1 = rx_block_call(ch, &wv, 0, 0, 0, 0, 0, 0), a2 = rx_block_call(ch, &rv, 0, 0, 0, 0, 0, 0);
    check(a1 == 0 && a2 == 0 && !memcmp((void *)m1, (void *)m2, 65536), "WRITE_VMO then READ_VMO 64 KiB");
    rd = req(ch, RX_BLOCK_OP_READ, 100, 64, 0, 0, b, sizeof b);
    check(rd == 0 && !memcmp(b, (void *)m1, 8192), "data written from a VMO reads back through the channel");
    rx_block_req_t dv = { 0, RX_BLOCK_OP_DETACH_VMO, 0, 0, id1, 0 };
    int d1 = rx_block_call(ch, &dv, 0, 0, 0, 0, 0, 0);
    int d2 = rx_block_call(ch, &wv, 0, 0, 0, 0, 0, 0);
    check(d1 == 0 && d2 == -EINVAL, "DETACH_VMO; the id is then refused");
    rx_handle_close(ch);

    // Read-only
    char ro_name[32];
    char *ro_args[] = { "-r", "-b", "4096", "64K", 0 };
    rx_handle_t proc2 = start_ramdisk(ro_args, ro_name);
    rx_block_info_t li;
    check(proc2 && in_list(ro_name, &li) && (li.flags & RX_BLOCK_FLAG_READONLY) && li.block_size == 4096 && li.block_count == 16,
          "a read-only RAM disk with 4 KiB blocks is listed as such");
    if (proc2 && rx_block_open(ro_name, &ch) == 0) {
        check(req(ch, RX_BLOCK_OP_WRITE, 0, 1, a, 4096, 0, 0) == -EROFS, "WRITE to a read-only device: EROFS");
        check(req(ch, RX_BLOCK_OP_READ, 0, 1, 0, 0, b, sizeof b) == 0, "READ from it works");
        rx_handle_close(ch);
    }

    // A device goes away with its server
    rx_task_kill(proc);
    rx_wait_process(proc);
    if (proc2) {
        rx_task_kill(proc2);
        rx_wait_process(proc2);
    }
    int gone = 0;
    for (int tries = 0; tries < 50 && !gone; tries++) {
        gone = !in_list(name, 0) && !in_list(ro_name, 0);
        if (!gone) nap(20);
    }
    check(gone, "the devices leave the list when their server ends");
    check(rx_block_open(name, &ch) == -ENOENT, "connecting to a removed device: ENOENT");
done:
    printf("blktest: %d passed, %d failed\n", passed, failed);
    return failed;
}

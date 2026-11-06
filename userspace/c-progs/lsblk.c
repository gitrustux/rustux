// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// lsblk: list block devices (from devmgr, /svc/devmgr).
//
//   lsblk [-b] [-n] [-J|--json]

#include <rustux/device.h>
#include <stdio.h>
#include <string.h>

// lsblk's sizes: 512 bytes -> "512B", 16 MiB -> "16M", 1.5 GiB -> "1.5G"
static void human(uint64_t n, char *buf, size_t cap) {
    const char *u = "BKMGTP";
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    if (v >= 10 || v == (double)(uint64_t)v) snprintf(buf, cap, "%.0f%c", v, u[k]);
    else snprintf(buf, cap, "%.1f%c", v, u[k]);
}

int main(int argc, char **argv) {
    int bytes = 0, noheader = 0, json = 0;
    for (int a = 1; a < argc; a++) {
        if (!strcmp(argv[a], "-b") || !strcmp(argv[a], "--bytes")) bytes = 1;
        else if (!strcmp(argv[a], "-n") || !strcmp(argv[a], "--noheadings")) noheader = 1;
        else if (!strcmp(argv[a], "-J") || !strcmp(argv[a], "--json")) json = 1;
        else if (!strcmp(argv[a], "-d") || !strcmp(argv[a], "--nodeps")) {}
        else {
            FILE *f = strcmp(argv[a], "--help") ? stderr : stdout;
            fprintf(f, "usage: lsblk [-b] [-n] [-J|--json]\nList block devices.\n"
                       "  -b  sizes in bytes\n  -n  no header line\n  -J  JSON (--json)\n");
            return f == stdout ? 0 : 2;
        }
    }
    rx_handle_t dm;
    int r = rx_devmgr_connect(&dm);
    if (r < 0) {
        fprintf(stderr, "lsblk: cannot reach the device manager (%s)\n", strerror(-r));
        return 1;
    }
    static char buf[65536];
    size_t got = 0;
    r = rx_dev_call(dm, RX_DM_OP_LIST_BLOCK, 0, 0, 0, buf, sizeof buf, &got, 0);
    if (r < 0 || got < 4) {
        fprintf(stderr, "lsblk: the device manager did not answer\n");
        return 1;
    }
    uint32_t n;
    memcpy(&n, buf, 4);
    if (json) printf("{\n   \"blockdevices\": [");
    else if (!noheader) printf("NAME     RM %10s RO TYPE MOUNTPOINTS\n", "SIZE");
    for (uint32_t k = 0; k < n && 4 + (k + 1) * sizeof(rx_block_info_t) <= got; k++) {
        rx_block_info_t b;
        memcpy(&b, buf + 4 + k * sizeof b, sizeof b);
        uint64_t size = b.block_count * b.block_size;
        int rm = (b.flags & RX_BLOCK_FLAG_REMOVABLE) != 0, ro = (b.flags & RX_BLOCK_FLAG_READONLY) != 0;
        char sz[32];
        if (bytes) snprintf(sz, sizeof sz, "%llu", (unsigned long long)size);
        else human(size, sz, sizeof sz);
        if (json) {
            printf("%s\n      {\"name\":\"%s\", \"rm\":%s, \"size\":", k ? "," : "", b.name, rm ? "true" : "false");
            if (bytes) printf("%llu", (unsigned long long)size);
            else printf("\"%s\"", sz);
            printf(", \"ro\":%s, \"type\":\"disk\", \"mountpoints\":[null], \"block_size\":%u, \"driver\":\"%s\", \"model\":\"%s\"}",
                   ro ? "true" : "false", b.block_size, b.driver, b.model);
        } else {
            printf("%-8s %2d %10s %2d disk\n", b.name, rm, sz, ro);
        }
    }
    if (json) printf("\n   ]\n}\n");
    return 0;
}

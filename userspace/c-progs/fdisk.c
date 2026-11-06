// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fdisk -l: list disks and their partition tables (step 13), read from
// the disks themselves. Changing tables is gpt's job.
//
//   fdisk -l [DISK...]

#include <errno.h>
#include <rustux/device.h>
#include <rustux/gpt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static rx_handle_t ch;
static uint32_t bs;

static int rd(void *c, uint64_t lba, uint32_t count, void *buf) { (void)c; return rx_block_read(ch, bs, lba, count, buf); }

static void human(uint64_t n, char *buf, size_t cap) {
    const char *u[] = { "B", "KiB", "MiB", "GiB", "TiB", "PiB" };
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    if (v >= 10 || v == (double)(uint64_t)v) snprintf(buf, cap, "%.0f %s", v, u[k]);
    else snprintf(buf, cap, "%.2f %s", v, u[k]);
}

static void short_size(uint64_t n, char *buf, size_t cap) {
    const char *u = "BKMGTP";
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    if (v >= 10 || v == (double)(uint64_t)v) snprintf(buf, cap, "%.0f%c", v, u[k]);
    else snprintf(buf, cap, "%.1f%c", v, u[k]);
}

static int list(const char *name) {
    int r = rx_block_open(name, &ch);
    rx_block_info_t info;
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "fdisk: cannot open %s: %s\n", name, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    bs = info.block_size;
    char sz[32], s2[16];
    uint64_t bytes = info.block_count * bs;
    human(bytes, sz, sizeof sz);
    printf("Disk %s: %s, %llu bytes, %llu sectors\n", name, sz, (unsigned long long)bytes, (unsigned long long)info.block_count);
    if (info.model[0]) printf("Disk model: %s\n", info.model);
    printf("Units: sectors of 1 * %u = %u bytes\n", bs, bs);
    printf("Sector size (logical/physical): %u bytes / %u bytes\n", bs, bs);
    static rx_gpt_t gpt;
    r = rx_gpt_read(&gpt, bs, info.block_count, rd, 0);
    if (r == 0) {
        char g[37];
        rx_guid_format(gpt.disk_guid, g);
        printf("Disklabel type: gpt\nDisk identifier: %s\n", g);
        if (gpt.from_backup) printf("The primary GPT table is corrupt, but the backup appears OK, so that will be used.\n");
        int any = 0;
        for (uint32_t i = 0; i < gpt.entries; i++) {
            rx_gpt_part_t *p = &gpt.parts[i];
            if (rx_guid_is_zero(p->type)) continue;
            if (!any) printf("\n%-12s %12s %12s %12s %6s %s\n", "Device", "Start", "End", "Sectors", "Size", "Type");
            any = 1;
            uint64_t n = p->last - p->first + 1;
            char dev[48];
            size_t len = strlen(name);
            snprintf(dev, sizeof dev, "%s%s%u", name, len && name[len - 1] >= '0' && name[len - 1] <= '9' ? "p" : "", i + 1);
            short_size(n * bs, s2, sizeof s2);
            const char *t = rx_gpt_type_name(p->type);
            char tg[37];
            if (!strcmp(t, "unknown")) { rx_guid_format(p->type, tg); t = tg; }
            printf("%-12s %12llu %12llu %12llu %6s %s\n", dev, (unsigned long long)p->first, (unsigned long long)p->last,
                   (unsigned long long)n, s2, t);
        }
    } else if (r == -EBADMSG) {
        printf("The GPT is damaged: both the primary and the backup table fail their checks.\n");
    } else {
        rx_mbr_part_t mp[4];
        if (rx_mbr_read(mp, bs, rd, 0) > 0) {
            printf("Disklabel type: dos\n\n%-12s %4s %12s %12s %12s %6s %3s %s\n", "Device", "Boot", "Start", "End", "Sectors", "Size", "Id", "Type");
            for (int i = 0; i < 4; i++) {
                if (!mp[i].type) continue;
                char dev[48];
                size_t len = strlen(name);
                snprintf(dev, sizeof dev, "%s%s%d", name, len && name[len - 1] >= '0' && name[len - 1] <= '9' ? "p" : "", i + 1);
                short_size((uint64_t)mp[i].count * bs, s2, sizeof s2);
                printf("%-12s %4s %12u %12llu %12u %6s %3x %s\n", dev, mp[i].bootable ? "*" : "", mp[i].first,
                       (unsigned long long)mp[i].first + mp[i].count - 1, mp[i].count, s2, mp[i].type, rx_mbr_type_name(mp[i].type));
            }
        }
    }
    rx_handle_close(ch);
    return 0;
}

static int by_name(const void *a, const void *b) {
    return strcmp(((const rx_block_info_t *)a)->name, ((const rx_block_info_t *)b)->name);
}

int main(int argc, char **argv) {
    int l = 0, first = 1;
    const char *names[64];
    int nnames = 0;
    for (int a = 1; a < argc; a++) {
        if (!strcmp(argv[a], "-l") || !strcmp(argv[a], "--list")) l = 1;
        else if (!strcmp(argv[a], "--help") || !strcmp(argv[a], "-h")) {
            printf("usage: fdisk -l [DISK...]\nList disks and their partition tables (GPT or MBR).\n"
                   "To change a table, use gpt (gpt --help).\n");
            return 0;
        } else if (argv[a][0] == '-') { fprintf(stderr, "usage: fdisk -l [DISK...]\n"); return 2; }
        else if (nnames < 64) names[nnames++] = argv[a];
    }
    if (!l) {
        fprintf(stderr, "fdisk: only listing (fdisk -l) is supported; change tables with gpt\n");
        return 2;
    }
    int status = 0;
    if (nnames) {
        for (int i = 0; i < nnames; i++) {
            if (!first) printf("\n\n");
            first = 0;
            status |= list(names[i]);
        }
        return status;
    }
    rx_block_info_t *all;
    int n = rx_block_list(&all);
    if (n < 0) {
        fprintf(stderr, "fdisk: cannot reach the device manager (%s)\n", strerror(-n));
        return 1;
    }
    qsort(all, (size_t)n, sizeof *all, by_name);
    for (int k = 0; k < n; k++) {
        if (all[k].flags & RX_BLOCK_FLAG_PARTITION) continue;
        if (!first) printf("\n\n");
        first = 0;
        status |= list(all[k].name);
    }
    return status;
}

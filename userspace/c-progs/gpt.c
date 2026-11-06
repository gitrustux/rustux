// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// gpt: show and change GPT partition tables (step 13).
//
//   gpt [show] DISK
//   gpt create [-f] DISK
//   gpt add [-i N] [-b START] [-s SIZE] [-t TYPE] [-l LABEL] DISK
//   gpt remove -i N DISK
//   gpt label -i N -l LABEL DISK
//   gpt type -i N -t TYPE DISK
//   gpt destroy DISK
//   gpt types
//
// Works on whole disks through the block protocol, then asks the driver
// to read the table again (RX_BLOCK_OP_RESCAN), so the partitions appear
// in lsblk at once. Read-only disks (the NVMe driver until step 20) are
// refused by their drivers.

#include <errno.h>
#include <rustux/device.h>
#include <rustux/gpt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static rx_handle_t ch;
static rx_block_info_t info;
static rx_gpt_t gpt;
static const char *disk;

static int rd(void *c, uint64_t lba, uint32_t count, void *buf) { (void)c; return rx_block_read(ch, info.block_size, lba, count, buf); }
static int wr(void *c, uint64_t lba, uint32_t count, const void *buf) { (void)c; return rx_block_write(ch, info.block_size, lba, count, buf); }

static void human(uint64_t n, char *buf, size_t cap) {
    const char *u = "BKMGTP";
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    if (v >= 10 || v == (double)(uint64_t)v) snprintf(buf, cap, "%.0f%c", v, u[k]);
    else snprintf(buf, cap, "%.1f%c", v, u[k]);
}

static int usage(FILE *f, int code) {
    fprintf(f, "usage: gpt [show] DISK\n"
               "       gpt create [-f] DISK          a new, empty table (-f: over an existing one)\n"
               "       gpt add [-i N] [-b START] [-s SIZE] [-t TYPE] [-l LABEL] DISK\n"
               "       gpt remove -i N DISK\n"
               "       gpt label -i N -l LABEL DISK\n"
               "       gpt type -i N -t TYPE DISK\n"
               "       gpt destroy DISK              erase the table (both copies and the MBR)\n"
               "       gpt types                     the TYPE names\n"
               "START is in blocks; SIZE in blocks, or bytes with K, M, G or T (default: the\n"
               "rest of the free space). New partitions start on a 1 MiB boundary. TYPE: a\n"
               "name from 'gpt types' or a GUID (default linux).\n");
    return code;
}

static int open_disk(void) {
    int r = rx_block_open(disk, &ch);
    if (r < 0) {
        fprintf(stderr, "gpt: %s: %s\n", disk, r == -ENOENT ? "no such block device" : strerror(-r));
        return -1;
    }
    if ((r = rx_block_info(ch, &info)) < 0) {
        fprintf(stderr, "gpt: %s: %s\n", disk, strerror(-r));
        return -1;
    }
    if (info.flags & RX_BLOCK_FLAG_PARTITION) {
        fprintf(stderr, "gpt: %s is a partition of %s; give the disk\n", disk, info.parent);
        return -1;
    }
    return 0;
}

static int load(void) {
    int r = rx_gpt_read(&gpt, info.block_size, info.block_count, rd, 0);
    if (r == -ENOENT) fprintf(stderr, "gpt: %s has no GPT (gpt create %s makes one)\n", disk, disk);
    else if (r == -EBADMSG) fprintf(stderr, "gpt: %s: both copies of the GPT are damaged\n", disk);
    else if (r < 0) fprintf(stderr, "gpt: %s: %s\n", disk, strerror(-r));
    else if (gpt.from_backup) fprintf(stderr, "gpt: %s: the primary table is damaged; using the backup (saving repairs it)\n", disk);
    return r < 0 ? -1 : 0;
}

static int save(void) {
    int r = rx_gpt_write(&gpt, info.block_size, info.block_count, wr, 0);
    if (r == 0) {
        rx_block_req_t q = { 0 };
        q.op = RX_BLOCK_OP_FLUSH;
        r = rx_block_call(ch, &q, 0, 0, 0, 0, 0, 0);
    }
    if (r < 0) {
        fprintf(stderr, "gpt: %s: cannot write the table: %s\n", disk, strerror(-r));
        return 1;
    }
    r = rx_block_simple(ch, RX_BLOCK_OP_RESCAN);
    if (r == -EBUSY) fprintf(stderr, "gpt: %s: a partition is in use; lsblk shows the new table once it is closed and the disk rescanned\n", disk);
    else if (r < 0 && r != -ENOTSUP) fprintf(stderr, "gpt: %s: the driver did not re-read the table: %s\n", disk, strerror(-r));
    return 0;
}

static int show(void) {
    if (load() < 0) return 1;
    char g[37], sz[16];
    rx_guid_format(gpt.disk_guid, g);
    human(info.block_count * info.block_size, sz, sizeof sz);
    printf("%s: GPT, %llu blocks of %u bytes (%s), disk GUID %s\n", disk, (unsigned long long)info.block_count,
           info.block_size, sz, g);
    printf("usable blocks %llu-%llu\n", (unsigned long long)gpt.first_usable, (unsigned long long)gpt.last_usable);
    printf("%4s %12s %12s %12s %7s  %-22s %s\n", "#", "START", "END", "BLOCKS", "SIZE", "TYPE", "NAME");
    for (uint32_t i = 0; i < gpt.entries; i++) {
        rx_gpt_part_t *p = &gpt.parts[i];
        if (rx_guid_is_zero(p->type)) continue;
        uint64_t n = p->last - p->first + 1;
        human(n * info.block_size, sz, sizeof sz);
        const char *t = rx_gpt_type_name(p->type);
        char tg[37];
        if (!strcmp(t, "unknown")) { rx_guid_format(p->type, tg); t = tg; }
        printf("%4u %12llu %12llu %12llu %7s  %-22s %s\n", i + 1, (unsigned long long)p->first, (unsigned long long)p->last,
               (unsigned long long)n, sz, t, p->name);
    }
    return 0;
}

// Blocks, or bytes with a suffix
static int parse_size(const char *s, uint64_t *out) {
    char *end;
    uint64_t v = strtoull(s, &end, 10);
    uint64_t mul = 0;
    switch (*end) {
    case 0: *out = v; return 0;
    case 'k': case 'K': mul = 1ull << 10; break;
    case 'm': case 'M': mul = 1ull << 20; break;
    case 'g': case 'G': mul = 1ull << 30; break;
    case 't': case 'T': mul = 1ull << 40; break;
    default: return -1;
    }
    end++;
    if (*end == 'i') end++;
    if (*end == 'B') end++;
    if (*end || !v) return -1;
    *out = (v * mul + info.block_size - 1) / info.block_size;
    return 0;
}

static int overlaps(uint64_t first, uint64_t last, int skip) {
    for (uint32_t i = 0; i < RX_GPT_MAX_PARTS; i++) {
        rx_gpt_part_t *p = &gpt.parts[i];
        if ((int)i == skip || rx_guid_is_zero(p->type)) continue;
        if (first <= p->last && p->first <= last) return 1;
    }
    return 0;
}

static int add(int index, uint64_t start, uint64_t size, const uint8_t type[16], const char *label) {
    if (load() < 0) return 1;
    if (index == 0) {
        for (int i = 0; i < RX_GPT_MAX_PARTS && !index; i++) if (rx_guid_is_zero(gpt.parts[i].type)) index = i + 1;
        if (!index) { fprintf(stderr, "gpt: %s: the table is full\n", disk); return 1; }
    } else if (index < 1 || index > RX_GPT_MAX_PARTS || !rx_guid_is_zero(gpt.parts[index - 1].type)) {
        fprintf(stderr, "gpt: %s: partition %d is %s\n", disk, index, index < 1 || index > RX_GPT_MAX_PARTS ? "out of range" : "in use");
        return 1;
    }
    uint64_t align = (1u << 20) / info.block_size;
    if (!align) align = 1;
    // A table written for a smaller disk (an image copied onto a larger
    // stick) ends early: writing it moves the backup to the real end, so
    // the space after it is usable
    uint64_t eblk = (RX_GPT_MAX_PARTS * 128 + info.block_size - 1) / info.block_size;
    if (info.block_count > 2 + eblk && info.block_count - 2 - eblk > gpt.last_usable) {
        printf("gpt: %s: the table covered %llu of %llu blocks; it now covers the whole disk\n", disk,
               (unsigned long long)gpt.last_usable + eblk + 2, (unsigned long long)info.block_count);
        gpt.last_usable = info.block_count - 2 - eblk;
    }
    uint64_t first = 0, last = 0;
    if (start) {
        first = start;
        if (!size) {
            // up to the next partition or the end
            last = gpt.last_usable;
            for (int i = 0; i < RX_GPT_MAX_PARTS; i++) {
                rx_gpt_part_t *p = &gpt.parts[i];
                if (!rx_guid_is_zero(p->type) && p->first > first && p->first - 1 < last) last = p->first - 1;
            }
        } else {
            last = first + size - 1;
        }
    } else {
        // The first aligned gap that fits (or, without a size, the first gap)
        uint64_t b = (gpt.first_usable + align - 1) / align * align;
        while (b <= gpt.last_usable) {
            int moved = 0;
            for (int i = 0; i < RX_GPT_MAX_PARTS; i++) {
                rx_gpt_part_t *p = &gpt.parts[i];
                if (!rx_guid_is_zero(p->type) && b >= p->first && b <= p->last) {
                    b = (p->last + 1 + align - 1) / align * align;
                    moved = 1;
                }
            }
            if (moved) continue;
            uint64_t end = gpt.last_usable;
            for (int i = 0; i < RX_GPT_MAX_PARTS; i++) {
                rx_gpt_part_t *p = &gpt.parts[i];
                if (!rx_guid_is_zero(p->type) && p->first > b && p->first - 1 < end) end = p->first - 1;
            }
            if (!size || end - b + 1 >= size) {
                first = b;
                last = size ? b + size - 1 : end;
                break;
            }
            b = (end + 1 + align - 1) / align * align;
        }
        if (!first) { fprintf(stderr, "gpt: %s: no free space%s\n", disk, size ? " that large" : ""); return 1; }
    }
    if (first < gpt.first_usable || last > gpt.last_usable || last < first) {
        fprintf(stderr, "gpt: %s: blocks %llu-%llu are outside the usable %llu-%llu\n", disk, (unsigned long long)first,
                (unsigned long long)last, (unsigned long long)gpt.first_usable, (unsigned long long)gpt.last_usable);
        return 1;
    }
    if (overlaps(first, last, -1)) { fprintf(stderr, "gpt: %s: blocks %llu-%llu overlap a partition\n", disk, (unsigned long long)first, (unsigned long long)last); return 1; }
    rx_gpt_part_t *p = &gpt.parts[index - 1];
    memset(p, 0, sizeof *p);
    memcpy(p->type, type, 16);
    rx_guid_random(p->guid);
    p->first = first;
    p->last = last;
    snprintf(p->name, sizeof p->name, "%s", label ? label : "");
    if (save()) return 1;
    char sz[16];
    human((last - first + 1) * info.block_size, sz, sizeof sz);
    printf("%s: partition %d added, blocks %llu-%llu (%s)\n", disk, index, (unsigned long long)first, (unsigned long long)last, sz);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) return usage(stderr, 2);
    const char *cmd = argv[1];
    int a = 2;
    if (!strcmp(cmd, "--help") || !strcmp(cmd, "-h")) return usage(stdout, 0);
    if (!strcmp(cmd, "types")) {
        static const char *names[] = { "efi", "bios", "msreserved", "msdata", "winre", "msldm", "msldmdata", "linux", "linuxroot",
                                       "linuxhome", "swap", "lvm", "raid", "xbootldr", "hfs", "apfs", "freebsd" };
        for (size_t i = 0; i < sizeof names / sizeof names[0]; i++) {
            uint8_t g[16];
            char s[37];
            rx_gpt_type_parse(names[i], g);
            rx_guid_format(g, s);
            printf("%-11s %s  %s\n", names[i], s, rx_gpt_type_name(g));
        }
        return 0;
    }
    if (strcmp(cmd, "show") && strcmp(cmd, "create") && strcmp(cmd, "add") && strcmp(cmd, "remove") && strcmp(cmd, "label") &&
        strcmp(cmd, "type") && strcmp(cmd, "destroy")) {
        cmd = "show";
        a = 1;
    }
    int force = 0, index = 0;
    uint64_t start = 0, size = 0;
    const char *tname = "linux", *label = 0, *size_arg = 0;
    for (; a < argc && argv[a][0] == '-'; a++) {
        const char *o = argv[a];
        if (!strcmp(o, "-f")) force = 1;
        else if (!strcmp(o, "-i") && a + 1 < argc) index = atoi(argv[++a]);
        else if (!strcmp(o, "-b") && a + 1 < argc) start = strtoull(argv[++a], 0, 10);
        else if (!strcmp(o, "-s") && a + 1 < argc) size_arg = argv[++a];
        else if (!strcmp(o, "-t") && a + 1 < argc) tname = argv[++a];
        else if (!strcmp(o, "-l") && a + 1 < argc) label = argv[++a];
        else return usage(stderr, 2);
    }
    if (a != argc - 1) return usage(stderr, 2);
    disk = argv[a];
    if (open_disk() < 0) return 1;
    if (size_arg && parse_size(size_arg, &size) < 0) { fprintf(stderr, "gpt: bad size: %s\n", size_arg); return 2; }
    uint8_t type[16];
    if (rx_gpt_type_parse(tname, type) < 0) { fprintf(stderr, "gpt: unknown type: %s (see gpt types)\n", tname); return 2; }

    if (!strcmp(cmd, "show")) return show();
    if (!strcmp(cmd, "create")) {
        rx_gpt_t old;
        if (!force && rx_gpt_read(&old, info.block_size, info.block_count, rd, 0) != -ENOENT) {
            fprintf(stderr, "gpt: %s already has a GPT (gpt create -f replaces it)\n", disk);
            return 1;
        }
        rx_gpt_init(&gpt, info.block_size, info.block_count);
        if (save()) return 1;
        printf("%s: new GPT, usable blocks %llu-%llu\n", disk, (unsigned long long)gpt.first_usable, (unsigned long long)gpt.last_usable);
        return 0;
    }
    if (!strcmp(cmd, "add")) return add(index, start, size, type, label);
    if (!strcmp(cmd, "destroy")) {
        uint32_t eblk = (RX_GPT_MAX_PARTS * 128 + info.block_size - 1) / info.block_size;
        uint8_t *z = calloc(eblk + 1, info.block_size);
        int r = z ? 0 : -ENOMEM;
        if (!r) r = wr(0, 0, eblk + 2 > info.block_count ? 1 : 2, z);                       // MBR and header
        if (!r && info.block_count > eblk + 2) r = wr(0, info.block_count - 1 - eblk, eblk + 1, z); // backup
        free(z);
        if (r < 0) { fprintf(stderr, "gpt: %s: %s\n", disk, strerror(-r)); return 1; }
        rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
        rx_block_simple(ch, RX_BLOCK_OP_RESCAN);
        printf("%s: partition table erased\n", disk);
        return 0;
    }
    // remove, label, type: an existing partition
    if (load() < 0) return 1;
    if (index < 1 || index > RX_GPT_MAX_PARTS || rx_guid_is_zero(gpt.parts[index - 1].type)) {
        fprintf(stderr, "gpt: %s: no partition %d (-i N)\n", disk, index);
        return 1;
    }
    rx_gpt_part_t *p = &gpt.parts[index - 1];
    if (!strcmp(cmd, "remove")) memset(p, 0, sizeof *p);
    else if (!strcmp(cmd, "label")) {
        if (!label) return usage(stderr, 2);
        snprintf(p->name, sizeof p->name, "%s", label);
    } else memcpy(p->type, type, 16);
    if (save()) return 1;
    printf("%s: partition %d %s\n", disk, index, !strcmp(cmd, "remove") ? "removed" : "changed");
    return 0;
}

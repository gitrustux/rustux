// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// blkid: what each block device holds (step 13): partition tables on
// disks, filesystems and GPT names/GUIDs on partitions.
//
//   blkid [-o full|value|export] [-s TAG]... [NAME...]
//
// Names are devmgr's (vda1), not /dev paths: block devices are not in
// /dev yet.

#include <errno.h>
#include <rustux/device.h>
#include <rustux/fsprobe.h>
#include <rustux/gpt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *tags[16];
static int ntags;
static const char *mode = "full";

static int wanted(const char *tag) {
    if (!ntags) return 1;
    for (int i = 0; i < ntags; i++) if (!strcmp(tags[i], tag)) return 1;
    return 0;
}

static void lower(char *s) { for (; *s; s++) if (*s >= 'A' && *s <= 'Z') *s = (char)(*s + 32); }

static int printed;

static void tag(const char *name, const char *t, const char *v) {
    if (!*v || !wanted(t)) return;
    if (!strcmp(mode, "value")) printf("%s\n", v);
    else if (!strcmp(mode, "export")) {
        if (!printed) printf("DEVNAME=%s\n", name);
        printf("%s=%s\n", t, v);
    } else {
        if (!printed) printf("%s:", name);
        printf(" %s=\"%s\"", t, v);
    }
    printed++;
}

static int show(const rx_block_info_t *b) {
    printed = 0;
    char g[40];
    if (b->flags & RX_BLOCK_FLAG_PARTITION) {
        rx_fsprobe_t fs;
        if (rx_block_probe(b->name, &fs) == 0) {
            tag(b->name, "LABEL", fs.label);
            tag(b->name, "UUID", fs.uuid);
            tag(b->name, "TYPE", fs.type);
        }
        if (b->flags & RX_BLOCK_FLAG_GPT) {
            tag(b->name, "PARTLABEL", b->label);
            rx_guid_format(b->guid, g);
            lower(g);
            tag(b->name, "PARTUUID", g);
        } else {
            snprintf(g, sizeof g, "%u", b->number);
            tag(b->name, "PART_ENTRY_NUMBER", g);
        }
    } else {
        rx_fsprobe_t fs;
        if (rx_block_probe(b->name, &fs) == 0) {   // a filesystem on the whole disk
            tag(b->name, "LABEL", fs.label);
            tag(b->name, "UUID", fs.uuid);
            tag(b->name, "TYPE", fs.type);
        }
        if (b->flags & RX_BLOCK_FLAG_GPT) {
            rx_guid_format(b->guid, g);
            lower(g);
            tag(b->name, "PTUUID", g);
            tag(b->name, "PTTYPE", "gpt");
        } else if (b->flags & RX_BLOCK_FLAG_MBR) {
            tag(b->name, "PTTYPE", "dos");
        }
    }
    if (printed && !strcmp(mode, "full")) printf("\n");
    if (printed && !strcmp(mode, "export")) printf("\n");
    return printed;
}

static int by_name(const void *a, const void *b) {
    return strcmp(((const rx_block_info_t *)a)->name, ((const rx_block_info_t *)b)->name);
}

int main(int argc, char **argv) {
    const char *names[64], *find_label = 0, *find_uuid = 0;
    int nnames = 0;
    for (int a = 1; a < argc; a++) {
        if (!strcmp(argv[a], "-o") && a + 1 < argc) mode = argv[++a];
        else if (!strcmp(argv[a], "-L") && a + 1 < argc) find_label = argv[++a];
        else if (!strcmp(argv[a], "-U") && a + 1 < argc) find_uuid = argv[++a];
        else if (!strcmp(argv[a], "-s") && a + 1 < argc) { if (ntags < 16) tags[ntags++] = argv[++a]; }
        else if (!strcmp(argv[a], "--help") || !strcmp(argv[a], "-h")) {
            printf("usage: blkid [-o full|value|export] [-s TAG]... [NAME...]\n"
                   "       blkid -L LABEL | -U UUID   the device with that filesystem\n"
                   "What block devices hold: partition tables (PTTYPE, PTUUID), filesystems\n"
                   "(TYPE, LABEL, UUID) and GPT partition names and GUIDs (PARTLABEL, PARTUUID).\n");
            return 0;
        } else if (argv[a][0] == '-') {
            fprintf(stderr, "usage: blkid [-o full|value|export] [-s TAG]... [NAME...]\n");
            return 2;
        } else if (nnames < 64) names[nnames++] = argv[a];
    }
    if (strcmp(mode, "full") && strcmp(mode, "value") && strcmp(mode, "export")) {
        fprintf(stderr, "blkid: unknown output format: %s\n", mode);
        return 2;
    }
    rx_block_info_t *all;
    int n = rx_block_list(&all);
    if (n < 0) {
        fprintf(stderr, "blkid: cannot reach the device manager (%s)\n", strerror(-n));
        return 4;
    }
    // Disks: their own, current info (the table may have changed)
    for (int k = 0; k < n; k++) {
        if (all[k].flags & RX_BLOCK_FLAG_PARTITION) continue;
        rx_handle_t ch;
        if (rx_block_open(all[k].name, &ch) == 0) {
            rx_block_info(ch, &all[k]);
            rx_handle_close(ch);
        }
    }
    qsort(all, (size_t)n, sizeof *all, by_name);
    if (find_label || find_uuid) {
        // As findfs: the first device whose filesystem has it
        for (int k = 0; k < n; k++) {
            rx_fsprobe_t fs;
            if (rx_block_probe(all[k].name, &fs) < 0) continue;
            if (find_label ? !strcmp(fs.label, find_label) : !strcasecmp(fs.uuid, find_uuid)) {
                printf("%s\n", all[k].name);
                return 0;
            }
        }
        return 2;
    }
    int any = 0, status = 0;
    if (nnames) {
        for (int i = 0; i < nnames; i++) {
            int found = 0;
            for (int k = 0; k < n; k++) {
                if (!strcmp(all[k].name, names[i])) { found = 1; any |= show(&all[k]); }
            }
            if (!found) status = 2;
        }
    } else {
        for (int k = 0; k < n; k++) any |= show(&all[k]);
    }
    return status ? status : any ? 0 : 2;
}

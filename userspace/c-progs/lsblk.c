// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// lsblk: list block devices (from devmgr, /svc/devmgr) as a tree of
// disks and their partitions.
//
//   lsblk [-b] [-n] [-l] [-d] [-f] [-o COLUMNS] [-J] [NAME...]

#include <errno.h>
#include <stdarg.h>
#include <rustux/device.h>
#include <rustux/fsprobe.h>
#include <rustux/gpt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

enum { C_NAME, C_RM, C_SIZE, C_RO, C_TYPE, C_MOUNT, C_FSTYPE, C_LABEL, C_UUID, C_PARTLABEL, C_PARTUUID,
       C_PARTTYPE, C_PARTTYPENAME, C_PARTN, C_PTTYPE, C_PTUUID, C_START, C_LOGSEC, C_MODEL, C_DRIVER, NCOLS };

static const struct { const char *name; int width; int right; const char *help; } cols[NCOLS] = {
    { "NAME", 10, 0, "device name" },
    { "RM", 2, 1, "removable" },
    { "SIZE", 7, 1, "size" },
    { "RO", 2, 1, "read-only" },
    { "TYPE", 4, 0, "disk or part" },
    { "MOUNTPOINTS", 11, 0, "where it is mounted (none yet)" },
    { "FSTYPE", 9, 0, "filesystem (ntfs, vfat, ext4, ...)" },
    { "LABEL", 12, 0, "filesystem label" },
    { "UUID", 36, 0, "filesystem UUID" },
    { "PARTLABEL", 16, 0, "partition name (GPT)" },
    { "PARTUUID", 36, 0, "partition GUID (GPT)" },
    { "PARTTYPE", 36, 0, "partition type GUID (GPT) or 0xNN (MBR)" },
    { "PARTTYPENAME", 20, 0, "partition type name" },
    { "PARTN", 5, 1, "partition number" },
    { "PTTYPE", 6, 0, "partition table: gpt or dos" },
    { "PTUUID", 36, 0, "GPT disk GUID" },
    { "START", 10, 1, "first block on the disk" },
    { "LOG-SEC", 7, 1, "block size" },
    { "MODEL", 20, 0, "device model" },
    { "DRIVER", 10, 0, "driver program" },
};

struct dev {
    rx_block_info_t info;
    rx_fsprobe_t fs;
};

static struct dev *devs;
static int ndevs, bytes, json;
static int show[NCOLS], nshow;

static void human(uint64_t n, char *buf, size_t cap) {
    const char *u = "BKMGTP";
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    if (v >= 10 || v == (double)(uint64_t)v) snprintf(buf, cap, "%.0f%c", v, u[k]);
    else snprintf(buf, cap, "%.1f%c", v, u[k]);
}

static int needs_probe(void) {
    for (int i = 0; i < nshow; i++) if (show[i] == C_FSTYPE || show[i] == C_LABEL || show[i] == C_UUID) return 1;
    return 0;
}

// The value of column c for device d ("" when empty); *num set for numbers
static const char *value(struct dev *d, int c, int *num) {
    static char v[128];
    rx_block_info_t *b = &d->info;
    int part = (b->flags & RX_BLOCK_FLAG_PARTITION) != 0;
    *num = 0;
    v[0] = 0;
    switch (c) {
    case C_NAME: return b->name;
    case C_RM: *num = 1; return b->flags & RX_BLOCK_FLAG_REMOVABLE ? "1" : "0";
    case C_RO: *num = 1; return b->flags & RX_BLOCK_FLAG_READONLY ? "1" : "0";
    case C_SIZE:
        if (bytes) { snprintf(v, sizeof v, "%llu", (unsigned long long)(b->block_count * b->block_size)); *num = 1; }
        else human(b->block_count * b->block_size, v, sizeof v);
        return v;
    case C_TYPE: return part ? "part" : "disk";
    case C_MOUNT: return "";
    case C_FSTYPE: return d->fs.type;
    case C_LABEL: return d->fs.label;
    case C_UUID: return d->fs.uuid;
    case C_PARTLABEL: return part ? b->label : "";
    case C_PARTUUID:
        if (part && (b->flags & RX_BLOCK_FLAG_GPT)) rx_guid_format(b->guid, v);
        break;
    case C_PARTTYPE:
        if (part && (b->flags & RX_BLOCK_FLAG_GPT)) rx_guid_format(b->type_guid, v);
        else if (part) snprintf(v, sizeof v, "0x%02x", b->type_guid[0]);
        break;
    case C_PARTTYPENAME:
        if (part && (b->flags & RX_BLOCK_FLAG_GPT)) return rx_gpt_type_name(b->type_guid);
        if (part) return rx_mbr_type_name(b->type_guid[0]);
        return "";
    case C_PARTN:
        if (part) { snprintf(v, sizeof v, "%u", b->number); *num = 1; }
        return v;
    case C_PTTYPE: return b->flags & RX_BLOCK_FLAG_GPT ? "gpt" : b->flags & RX_BLOCK_FLAG_MBR ? "dos" : "";
    case C_PTUUID:
        if (!part && (b->flags & RX_BLOCK_FLAG_GPT)) rx_guid_format(b->guid, v);
        break;
    case C_START: snprintf(v, sizeof v, "%llu", (unsigned long long)b->start); *num = 1; return v;
    case C_LOGSEC: snprintf(v, sizeof v, "%u", b->block_size); *num = 1; return v;
    case C_MODEL: return b->model;
    case C_DRIVER: return b->driver;
    }
    // GUIDs in lower case, as Linux's lsblk prints them
    for (char *p = v; *p; p++) if (*p >= 'A' && *p <= 'F') *p = (char)(*p + 32);
    return v;
}

static void json_str(const char *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((unsigned char)*s < 0x20) printf("\\u%04x", *s);
        else putchar(*s);
    }
    putchar('"');
}

static void print_json(struct dev *d, int indent) {
    printf("%*s{", indent, "");
    for (int i = 0; i < nshow; i++) {
        int num;
        const char *s = value(d, show[i], &num);
        char key[32];
        int k = 0;
        for (const char *p = cols[show[i]].name; *p && k < 31; p++) key[k++] = (char)(*p >= 'A' && *p <= 'Z' ? *p + 32 : *p);
        key[k] = 0;
        printf("%s\"%s\":", i ? ", " : "", key);
        if (show[i] == C_RM || show[i] == C_RO) printf("%s", s[0] == '1' ? "true" : "false");
        else if (show[i] == C_MOUNT) printf("[null]");
        else if (!*s) printf("null");
        else if (num) printf("%s", s);
        else json_str(s);
    }
}

// Rows are built here and printed without trailing spaces
static char line[1024];
static size_t llen;

static void out(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(line + llen, sizeof line - llen, fmt, ap);
    va_end(ap);
    if (n > 0) llen += (size_t)n < sizeof line - llen ? (size_t)n : sizeof line - llen - 1;
}

static void end_line(void) {
    while (llen && line[llen - 1] == ' ') llen--;
    line[llen] = 0;
    puts(line);
    llen = 0;
}

static void print_row(struct dev *d, const char *prefix, int list) {
    for (int i = 0; i < nshow; i++) {
        int num, c = show[i];
        const char *s = value(d, c, &num);
        int last = i == nshow - 1;
        if (c == C_NAME && !list) {
            char n[96];
            snprintf(n, sizeof n, "%s%s", prefix, s);
            // box-drawing characters take 3 bytes but one column
            int extra = 0;
            for (const char *p = prefix; *p; p++) if (((unsigned char)*p & 0xC0) == 0x80) extra++;
            if (last) out("%s", n);
            else out("%-*s", cols[c].width + extra, n);
        } else if (last && !cols[c].right) {
            out("%s", s);
        } else {
            out(cols[c].right ? "%*s" : "%-*s", cols[c].width, s);
        }
        if (!last) out(" ");
    }
    end_line();
}

static int by_name(const void *a, const void *b) {
    const struct dev *x = a, *y = b;
    int px = (x->info.flags & RX_BLOCK_FLAG_PARTITION) != 0, py = (y->info.flags & RX_BLOCK_FLAG_PARTITION) != 0;
    if (px && py && !strcmp(x->info.parent, y->info.parent)) return (int)x->info.number - (int)y->info.number;
    return strcmp(x->info.name, y->info.name);
}

static int set_columns(const char *list) {
    nshow = 0;
    char buf[512];
    snprintf(buf, sizeof buf, "%s", list);
    for (char *t = strtok(buf, ","); t; t = strtok(0, ",")) {
        int found = -1;
        for (int c = 0; c < NCOLS; c++) if (!strcasecmp(cols[c].name, t)) found = c;
        if (found < 0) {
            fprintf(stderr, "lsblk: unknown column: %s\n", t);
            return -1;
        }
        if (nshow < NCOLS) show[nshow++] = found;
    }
    return nshow ? 0 : -1;
}

static void usage(FILE *f) {
    fprintf(f, "usage: lsblk [-b] [-n] [-l] [-d] [-f] [-o COLUMNS] [-J] [NAME...]\n"
               "List block devices: disks, and their partitions below them.\n"
               "  -b  sizes in bytes        -n  no header line\n"
               "  -l  a list, not a tree    -d  disks only, no partitions\n"
               "  -f  filesystems (FSTYPE, LABEL, UUID)\n"
               "  -o  columns, separated by commas (+COL adds to the default)\n"
               "  -J  JSON (--json)\n"
               "Columns:\n");
    for (int c = 0; c < NCOLS; c++) fprintf(f, "  %-13s %s\n", cols[c].name, cols[c].help);
}

static int wanted(const struct dev *d, const char **names, int nnames) {
    if (!nnames) return 1;
    for (int i = 0; i < nnames; i++) {
        if (!strcmp(names[i], d->info.name)) return 1;
        if ((d->info.flags & RX_BLOCK_FLAG_PARTITION) && !strcmp(names[i], d->info.parent)) return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    int noheader = 0, list = 0, nodeps = 0;
    const char *names[64];
    int nnames = 0;
    set_columns("NAME,RM,SIZE,RO,TYPE,MOUNTPOINTS");
    for (int a = 1; a < argc; a++) {
        const char *s = argv[a];
        if (!strcmp(s, "-b") || !strcmp(s, "--bytes")) bytes = 1;
        else if (!strcmp(s, "-n") || !strcmp(s, "--noheadings")) noheader = 1;
        else if (!strcmp(s, "-J") || !strcmp(s, "--json")) json = 1;
        else if (!strcmp(s, "-l") || !strcmp(s, "--list")) list = 1;
        else if (!strcmp(s, "-d") || !strcmp(s, "--nodeps")) nodeps = 1;
        else if (!strcmp(s, "-f") || !strcmp(s, "--fs")) set_columns("NAME,FSTYPE,LABEL,UUID,MOUNTPOINTS");
        else if ((!strcmp(s, "-o") || !strcmp(s, "--output")) && a + 1 < argc) {
            const char *o = argv[++a];
            char buf[512];
            if (o[0] == '+') snprintf(buf, sizeof buf, "NAME,RM,SIZE,RO,TYPE,MOUNTPOINTS,%s", o + 1);
            else snprintf(buf, sizeof buf, "%s", o);
            if (set_columns(buf) < 0) return 2;
        } else if (!strcmp(s, "--help") || !strcmp(s, "-h")) { usage(stdout); return 0; }
        else if (s[0] == '-') { usage(stderr); return 2; }
        else if (nnames < 64) names[nnames++] = s;
    }
    rx_block_info_t *all;
    int n = rx_block_list(&all);
    if (n < 0) {
        fprintf(stderr, "lsblk: cannot reach the device manager (%s)\n", strerror(-n));
        return 1;
    }
    devs = calloc(n ? (size_t)n : 1, sizeof *devs);
    for (int k = 0; k < n; k++) devs[k].info = all[k];
    ndevs = n;
    free(all);
    // A disk's own GET_INFO has its table type after a rescan (devmgr
    // keeps what was published)
    for (int k = 0; k < ndevs; k++) {
        if (devs[k].info.flags & RX_BLOCK_FLAG_PARTITION) continue;
        rx_handle_t ch;
        rx_block_info_t fresh;
        if (rx_block_open(devs[k].info.name, &ch) == 0) {
            if (rx_block_info(ch, &fresh) == 0) devs[k].info = fresh;
            rx_handle_close(ch);
        }
    }
    qsort(devs, (size_t)ndevs, sizeof *devs, by_name);
    int status = 0;
    for (int i = 0; i < nnames; i++) {
        int found = 0;
        for (int k = 0; k < ndevs; k++) found |= !strcmp(devs[k].info.name, names[i]);
        if (!found) {
            fprintf(stderr, "lsblk: %s: not a block device\n", names[i]);
            status = 32;
        }
    }
    if (needs_probe()) {
        for (int k = 0; k < ndevs; k++) {
            if (wanted(&devs[k], names, nnames)) rx_block_probe(devs[k].info.name, &devs[k].fs);
        }
    }
    if (json) printf("{\n   \"blockdevices\": [");
    else if (!noheader) {
        for (int i = 0; i < nshow; i++) {
            int c = show[i], last = i == nshow - 1;
            if (last && !cols[c].right) out("%s", cols[c].name);
            else out(cols[c].right ? "%*s" : "%-*s", cols[c].width, cols[c].name);
            if (!last) out(" ");
        }
        end_line();
    }
    int first = 1;
    for (int k = 0; k < ndevs; k++) {
        struct dev *d = &devs[k];
        int part = (d->info.flags & RX_BLOCK_FLAG_PARTITION) != 0;
        if (part && nodeps) continue;
        if (!list) {
            // In a tree, a partition whose disk is shown goes below it
            int below = 0;
            for (int j = 0; part && j < ndevs; j++) {
                if (!strcmp(devs[j].info.name, d->info.parent) && wanted(&devs[j], names, nnames)) below = 1;
            }
            if (below) continue;
            if (nnames) {
                int named = 0;
                for (int i = 0; i < nnames; i++) named |= !strcmp(names[i], d->info.name);
                if (!named) continue;
            }
        } else if (!wanted(d, names, nnames)) {
            continue;
        }
        if (json) {
            printf("%s\n", first ? "" : ",");
            print_json(d, 6);
        } else {
            print_row(d, "", list);
        }
        first = 0;
        if (part || nodeps || list) {
            if (json) printf("}");
            continue;
        }
        // Its partitions
        int kids[RX_GPT_MAX_PARTS], nk = 0;
        for (int j = 0; j < ndevs && nk < RX_GPT_MAX_PARTS; j++) {
            if ((devs[j].info.flags & RX_BLOCK_FLAG_PARTITION) && !strcmp(devs[j].info.parent, d->info.name)) kids[nk++] = j;
        }
        if (json && nk) printf(",\n         \"children\": [");
        for (int i = 0; i < nk; i++) {
            if (json) {
                printf("%s\n", i ? "," : "");
                print_json(&devs[kids[i]], 12);
                printf("}");
            } else {
                print_row(&devs[kids[i]], i == nk - 1 ? "\xe2\x94\x94\xe2\x94\x80" : "\xe2\x94\x9c\xe2\x94\x80", 0);
            }
        }
        if (json && nk) printf("\n         ]");
        if (json) printf("}");
    }
    if (json) printf("\n   ]\n}\n");
    return status;
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// efibootmgr: the firmware's boot entries (step 19), as Linux's.
//
//   efibootmgr [-v]                      list BootCurrent, BootNext, Timeout, BootOrder, BootNNNN
//   efibootmgr -o XXXX,YYYY,...          set BootOrder
//   efibootmgr -n XXXX | -N              set / delete BootNext (once)
//   efibootmgr -b XXXX -B                delete an entry (and take it out of BootOrder)
//   efibootmgr -b XXXX -a | -A           make it active / inactive
//   efibootmgr -c [-d DISK [-p PART]] [-l LOADER] [-L LABEL]
//                                        a new entry, first in BootOrder; without
//                                        -d on the partition rboot started from
//                                        (LoaderDevicePartUUID); LOADER defaults
//                                        to \EFI\Rustica\rboot.efi, LABEL to Rustica
//   efibootmgr -t SECONDS | -T           set / delete Timeout
//
// Changes need root. The new entry's path is HD(PART)/File(LOADER), the
// short form (the firmware finds the partition by its GUID).

#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "efivar.h"

static int quiet, verbose;

static int fail(const char *what, int r) {
    fprintf(stderr, "efibootmgr: %s: %s\n", what, ev_strerror(r));
    return 1;
}

static int hex4(const char *s, uint16_t *out) {
    char *e;
    unsigned long v = strtoul(s, &e, 16);
    if (!*s || *e || v > 0xFFFF) return -1;
    *out = (uint16_t)v;
    return 0;
}

static int list(void) {
    uint8_t b[4096];
    char name[16];
    int n = ev_get(EFI_GLOBAL_GUID, "BootCurrent", b, sizeof b, 0);
    if (n == -ENOSYS) return fail("cannot read the variables", n);
    if (n >= 2) printf("BootCurrent: %04X\n", rd16(b));
    if ((n = ev_get(EFI_GLOBAL_GUID, "BootNext", b, sizeof b, 0)) >= 2) printf("BootNext: %04X\n", rd16(b));
    if ((n = ev_get(EFI_GLOBAL_GUID, "Timeout", b, sizeof b, 0)) >= 2) printf("Timeout: %u seconds\n", rd16(b));
    uint16_t order[128];
    int no = ev_boot_order(order, 128);
    printf("BootOrder: ");
    for (int i = 0; i < no; i++) printf("%s%04X", i ? "," : "", order[i]);
    printf("\n");
    // Every BootNNNN, in number order
    char vname[512] = "";
    uint8_t guid[16] = { 0 }, global[16];
    rx_guid_parse(EFI_GLOBAL_GUID, global);
    uint16_t nums[256];
    int nn = 0;
    while (ev_next(vname, sizeof vname, guid) == 0) {
        uint16_t x;
        if (memcmp(guid, global, 16) || strlen(vname) != 8 || strncmp(vname, "Boot", 4) || hex4(vname + 4, &x) < 0) continue;
        if (nn < 256) nums[nn++] = x;
    }
    for (int i = 1; i < nn; i++)
        for (int j = i; j > 0 && nums[j - 1] > nums[j]; j--) { uint16_t t = nums[j]; nums[j] = nums[j - 1]; nums[j - 1] = t; }
    for (int i = 0; i < nn; i++) {
        snprintf(name, sizeof name, "Boot%04X", nums[i]);
        if ((n = ev_get(EFI_GLOBAL_GUID, name, b, sizeof b, 0)) < 0) continue;
        load_option_t lo;
        if (lo_parse(b, (size_t)n, &lo) < 0) continue;
        char path[1024];
        dp_text(lo.dp, lo.dp_len, path, sizeof path);
        printf("%s%c %s\t%s", name, lo.attrs & LOAD_OPTION_ACTIVE ? '*' : ' ', lo.desc, path);
        if (verbose && lo.opt_len) printf(" (%zu bytes of options)", lo.opt_len);
        printf("\n");
    }
    return 0;
}

// The partition whose GUID rboot recorded (LoaderDevicePartUUID)
static int boot_partition(rx_block_info_t *info) {
    char s[64];
    uint8_t want[16];
    if (ev_get_str(EFI_LOADER_GUID, "LoaderDevicePartUUID", s, sizeof s) < 0 || rx_guid_parse(s, want) < 0) {
        fprintf(stderr, "efibootmgr: this boot did not go through rboot: give the partition with -d DISK [-p PART]\n");
        return -1;
    }
    rx_block_info_t *all = 0;
    int n = rx_block_list(&all);
    for (int i = 0; i < n; i++)
        if ((all[i].flags & RX_BLOCK_FLAG_PARTITION) && !memcmp(all[i].guid, want, 16)) {
            *info = all[i];
            free(all);
            return 0;
        }
    free(all);
    fprintf(stderr, "efibootmgr: no partition %s (rboot's) among the block devices\n", s);
    return -1;
}

// The partition: DISK + N, or DISK + pN (nvme0n1p1)
static int find_partition(const char *disk, int part, rx_block_info_t *info) {
    char name[64];
    const char *forms[] = { "%s%d", "%sp%d" };
    for (int k = 0; k < 2; k++) {
        snprintf(name, sizeof name, forms[k], disk, part);
        rx_handle_t ch;
        if (rx_block_open(name, &ch) < 0) continue;
        int r = rx_block_info(ch, info);
        rx_handle_close(ch);
        if (r == 0 && (info->flags & RX_BLOCK_FLAG_PARTITION) && (info->flags & RX_BLOCK_FLAG_GPT)) return 0;
    }
    fprintf(stderr, "efibootmgr: %s has no GPT partition %d (lsblk lists them)\n", disk, part);
    return -1;
}

static int create(const char *disk, int part, const char *loader, const char *label) {
    rx_block_info_t info;
    if (disk ? find_partition(disk, part, &info) < 0 : boot_partition(&info) < 0) return 1;
    uint8_t dp[600], opt[1200];
    size_t dl = dp_build_hd_file(dp, sizeof dp, info.number, info.start, info.block_count, info.guid, loader);
    size_t ol = dl ? lo_build(opt, sizeof opt, LOAD_OPTION_ACTIVE, label, dp, dl) : 0;
    if (!ol) {
        fprintf(stderr, "efibootmgr: the loader path or label is too long\n");
        return 1;
    }
    // The first free number
    uint16_t num = 0;
    char name[16];
    uint8_t tmp[8];
    for (;; num++) {
        snprintf(name, sizeof name, "Boot%04X", num);
        int r = ev_get(EFI_GLOBAL_GUID, name, tmp, sizeof tmp, 0);
        if (r == -ENOENT) break;
        if (r == -ENOSYS) return fail("cannot read the variables", r);
        if (num == 0xFFFF) return 1;
    }
    int r = ev_set(EFI_GLOBAL_GUID, name, EV_DEFAULT, opt, ol);
    if (r < 0) return fail(name, r);
    uint16_t order[128];
    int n = ev_boot_order(order, 127);
    if (n < 0) n = 0;
    memmove(order + 1, order, (size_t)n * 2);
    order[0] = num;
    if ((r = ev_set_boot_order(order, n + 1)) < 0) return fail("BootOrder", r);
    if (!quiet) printf("Created %s: %s\n", name, label);
    return 0;
}

static int set_order(const char *list_s) {
    uint16_t order[128];
    int n = 0;
    char buf[1024];
    snprintf(buf, sizeof buf, "%s", list_s);
    for (char *save = 0, *t = strtok_r(buf, ",", &save); t; t = strtok_r(0, ",", &save)) {
        if (n == 128 || hex4(t, &order[n]) < 0) {
            fprintf(stderr, "efibootmgr: bad entry number %s (hex, as 0001)\n", t);
            return 1;
        }
        n++;
    }
    int r = ev_set_boot_order(order, n);
    return r < 0 ? fail("BootOrder", r) : 0;
}

static int delete_entry(uint16_t num) {
    char name[16];
    snprintf(name, sizeof name, "Boot%04X", num);
    int r = ev_set(EFI_GLOBAL_GUID, name, 0, 0, 0);
    if (r < 0) return fail(name, r);
    uint16_t order[128];
    int n = ev_boot_order(order, 128), k = 0;
    for (int i = 0; i < n; i++) if (order[i] != num) order[k++] = order[i];
    if (k != n && (r = ev_set_boot_order(order, k)) < 0) return fail("BootOrder", r);
    return 0;
}

static int set_active(uint16_t num, int on) {
    char name[16];
    uint8_t b[4096];
    uint32_t attrs = 0;
    snprintf(name, sizeof name, "Boot%04X", num);
    int n = ev_get(EFI_GLOBAL_GUID, name, b, sizeof b, &attrs);
    if (n < 4) return fail(name, n < 0 ? n : -EINVAL);
    if (on) b[0] |= LOAD_OPTION_ACTIVE;
    else b[0] &= (uint8_t)~LOAD_OPTION_ACTIVE;
    int r = ev_set(EFI_GLOBAL_GUID, name, attrs ? attrs : EV_DEFAULT, b, (size_t)n);
    return r < 0 ? fail(name, r) : 0;
}

static int usage(int code) {
    fprintf(code ? stderr : stdout,
            "usage: efibootmgr [-v]\n"
            "       efibootmgr -o XXXX,YYYY,...    set BootOrder\n"
            "       efibootmgr -n XXXX | -N        set / delete BootNext\n"
            "       efibootmgr -b XXXX -B          delete an entry\n"
            "       efibootmgr -b XXXX -a | -A     make it active / inactive\n"
            "       efibootmgr -c [-d DISK [-p PART]] [-l LOADER] [-L LABEL]\n"
            "       efibootmgr -t SECONDS | -T     set / delete Timeout\n"
            "Changes need root. -q: quiet. See man efibootmgr.\n");
    return code;
}

int main(int argc, char **argv) {
    int create_it = 0, del = 0, act = -1, del_next = 0, del_timeout = 0, part = 1;
    const char *order = 0, *next = 0, *num_s = 0, *disk = 0, *loader = "\\EFI\\Rustica\\rboot.efi", *label = "Rustica", *timeout = 0;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
#define NEXTARG() (i + 1 < argc ? argv[++i] : (fprintf(stderr, "efibootmgr: %s needs a value\n", a), exit(2), ""))
        if (!strcmp(a, "-v") || !strcmp(a, "--verbose")) verbose = 1;
        else if (!strcmp(a, "-q") || !strcmp(a, "--quiet")) quiet = 1;
        else if (!strcmp(a, "-o") || !strcmp(a, "--bootorder")) order = NEXTARG();
        else if (!strcmp(a, "-n") || !strcmp(a, "--bootnext")) next = NEXTARG();
        else if (!strcmp(a, "-N") || !strcmp(a, "--delete-bootnext")) del_next = 1;
        else if (!strcmp(a, "-b") || !strcmp(a, "--bootnum")) num_s = NEXTARG();
        else if (!strcmp(a, "-B") || !strcmp(a, "--delete-bootnum")) del = 1;
        else if (!strcmp(a, "-a") || !strcmp(a, "--active")) act = 1;
        else if (!strcmp(a, "-A") || !strcmp(a, "--inactive")) act = 0;
        else if (!strcmp(a, "-c") || !strcmp(a, "--create")) create_it = 1;
        else if (!strcmp(a, "-d") || !strcmp(a, "--disk")) disk = NEXTARG();
        else if (!strcmp(a, "-p") || !strcmp(a, "--part")) part = atoi(NEXTARG());
        else if (!strcmp(a, "-l") || !strcmp(a, "--loader")) loader = NEXTARG();
        else if (!strcmp(a, "-L") || !strcmp(a, "--label")) label = NEXTARG();
        else if (!strcmp(a, "-t") || !strcmp(a, "--timeout")) timeout = NEXTARG();
        else if (!strcmp(a, "-T") || !strcmp(a, "--delete-timeout")) del_timeout = 1;
        else if (!strcmp(a, "-h") || !strcmp(a, "--help")) return usage(0);
        else return usage(2);
    }
    int changed = 0, rc = 0;
    uint16_t num = 0;
    if (num_s && hex4(num_s, &num) < 0) {
        fprintf(stderr, "efibootmgr: bad entry number %s\n", num_s);
        return 2;
    }
    if (create_it) {
        rc |= create(disk, part, loader, label), changed = 1;
    }
    if (del) {
        if (!num_s) return usage(2);
        rc |= delete_entry(num), changed = 1;
    }
    if (act >= 0) {
        if (!num_s) return usage(2);
        rc |= set_active(num, act), changed = 1;
    }
    if (order) rc |= set_order(order), changed = 1;
    if (next) {
        uint16_t x;
        if (hex4(next, &x) < 0) return usage(2);
        int r = ev_set(EFI_GLOBAL_GUID, "BootNext", EV_DEFAULT, &x, 2);
        if (r < 0) rc |= fail("BootNext", r);
        changed = 1;
    }
    if (del_next) {
        int r = ev_delete(EFI_GLOBAL_GUID, "BootNext");
        if (r < 0) rc |= fail("BootNext", r);
        changed = 1;
    }
    if (timeout) {
        uint16_t t = (uint16_t)atoi(timeout);
        int r = ev_set(EFI_GLOBAL_GUID, "Timeout", EV_DEFAULT, &t, 2);
        if (r < 0) rc |= fail("Timeout", r);
        changed = 1;
    }
    if (del_timeout) {
        int r = ev_delete(EFI_GLOBAL_GUID, "Timeout");
        if (r < 0) rc |= fail("Timeout", r);
        changed = 1;
    }
    if (rc) return 1;
    if (changed && quiet) return 0;
    return list();
}

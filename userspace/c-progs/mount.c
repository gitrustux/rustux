// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mount: attach a filesystem on a block device to a directory (step 14).
//
//   mount                               list what is mounted (/etc/mtab)
//   mount [-t TYPE] [-o OPTS] DEV DIR   mount DEV at DIR
//   mount DIR | DEV                     mount as /etc/fstab says
//   mount -a [-w SECONDS]               mount everything in /etc/fstab
//                                       (not noauto, not mounted yet)
//
// DEV is a block device name (sda1; /dev/sda1 works too), LABEL=name,
// UUID=XXXX-XXXX, PARTLABEL=name, or EFI: the EFI system partition, on a
// removable disk if there is one (the stick Rustux started from). An EFI
// partition on a fixed disk (Windows' own) is mounted read-only unless
// the options say "rw!".
// TYPE: vfat (fat, msdos), ext2 (ext3 and ext4 without their new
// features: read-only or refused, step 16), or auto (default): what blkid
// finds.
// OPTS: ro, rw, rw!, noauto, nofail, defaults (comma separated).
// With -a, devices that are not there yet are waited for (-w, default 10
// seconds: USB sticks take a moment; EFI waits that long for one on a
// removable disk before it takes a fixed disk's); nofail entries that never come are
// skipped quietly.
//
// The filesystem itself is served by a program per type (/bin/fatfs,
// /bin/ext2fs),
// which mount starts and hands to fsd.

#include <errno.h>
#include <fcntl.h>
#include <rustux/device.h>
#include <rustux/fs.h>
#include <rustux/fsprobe.h>
#include <rustux/gpt.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *prog = "mount";

static void usage(void) {
    fprintf(stderr,
            "usage: mount                              list mounted filesystems\n"
            "       mount [-t TYPE] [-o OPTS] DEV DIR  mount DEV (sda1, LABEL=x, UUID=x, EFI) at DIR\n"
            "       mount DIR|DEV                      as /etc/fstab says\n"
            "       mount -a [-w SECONDS]              everything in /etc/fstab\n"
            "TYPE: vfat, ext2 or auto; OPTS: ro, rw, noauto, nofail, defaults\n");
}

// Split a line into whitespace-separated fields (in place)
static int fields(char *s, char **f, int max) {
    int n = 0;
    while (n < max) {
        while (*s == ' ' || *s == '\t' || *s == '\n') s++;
        if (!*s || *s == '#') break;
        f[n++] = s;
        while (*s && *s != ' ' && *s != '\t' && *s != '\n') s++;
        if (*s) *s++ = 0;
    }
    return n;
}

static int list(void) {
    FILE *f = fopen("/etc/mtab", "r");
    if (!f) {
        fprintf(stderr, "mount: /etc/mtab: %s\n", strerror(errno));
        return 1;
    }
    char line[600];
    while (fgets(line, sizeof line, f)) {
        char *f[4];
        if (fields(line, f, 4) == 4) printf("%s on %s type %s (%s)\n", f[0], f[1], f[2], f[3]);
    }
    fclose(f);
    return 0;
}

// Is DIR in /etc/mtab? (*src gets what is mounted there)
static int mounted_at(const char *dir, char *src, size_t cap) {
    FILE *f = fopen("/etc/mtab", "r");
    if (!f) return 0;
    char line[600], *fl[2];
    int found = 0;
    while (!found && fgets(line, sizeof line, f)) {
        if (fields(line, fl, 2) == 2 && !strcmp(fl[1], dir)) {
            found = 1;
            if (src) snprintf(src, cap, "%s", fl[0]);
        }
    }
    fclose(f);
    return found;
}

static int has_opt(const char *opts, const char *o) {
    size_t n = strlen(o);
    for (const char *p = opts; p && *p;) {
        const char *e = strchr(p, ',');
        size_t l = e ? (size_t)(e - p) : strlen(p);
        if (l == n && !strncmp(p, o, n)) return 1;
        p = e ? e + 1 : 0;
    }
    return 0;
}

static int is_esp(const rx_block_info_t *b) {
    static const uint8_t esp[16] = { 0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
                                     0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B };
    if (!(b->flags & RX_BLOCK_FLAG_PARTITION)) return 0;
    if (b->flags & RX_BLOCK_FLAG_GPT) return !memcmp(b->type_guid, esp, 16);
    if (b->flags & RX_BLOCK_FLAG_MBR) return b->type_guid[0] == 0xEF;
    return 0;
}

static int removable(const rx_block_info_t *all, int n, const rx_block_info_t *b) {
    if (b->flags & RX_BLOCK_FLAG_REMOVABLE) return 1;
    for (int i = 0; i < n; i++)
        if (!strcmp(all[i].name, b->parent)) return (all[i].flags & RX_BLOCK_FLAG_REMOVABLE) != 0;
    return 0;
}

// While set (mount -a, before its deadline), EFI does not settle for a
// fixed disk's partition: the USB stick Rustux started from may still be
// on its way (USB devices appear seconds after NVMe ones)
static int want_removable;

// The device name for a source; *fixed_esp is set for EFI on a fixed disk.
// -ENOENT if there is none (yet).
static int find_device(const char *src, char *out, size_t cap, int *fixed_esp) {
    *fixed_esp = 0;
    if (!strncmp(src, "/dev/", 5)) src += 5;
    int by_label = !strncmp(src, "LABEL=", 6), by_uuid = !strncmp(src, "UUID=", 5);
    int by_plabel = !strncmp(src, "PARTLABEL=", 10), efi = !strcmp(src, "EFI");
    rx_block_info_t *all;
    int n = rx_block_list(&all);
    if (n < 0) return n;
    int r = -ENOENT, best = -1;
    for (int i = 0; i < n && r == -ENOENT; i++) {
        rx_block_info_t *b = &all[i];
        if (by_label || by_uuid) {
            rx_fsprobe_t fs;
            if (!(b->flags & RX_BLOCK_FLAG_PARTITION) && (b->flags & (RX_BLOCK_FLAG_GPT | RX_BLOCK_FLAG_MBR))) continue;
            if (rx_block_probe(b->name, &fs) < 0) continue;
            if (by_label ? strcmp(fs.label, src + 6) : strcasecmp(fs.uuid, src + 5)) continue;
            r = 0;
        } else if (by_plabel) {
            if (!(b->flags & RX_BLOCK_FLAG_PARTITION) || strcmp(b->label, src + 10)) continue;
            r = 0;
        } else if (efi) {
            if (!is_esp(b)) continue;
            if (!removable(all, n, b)) {
                if (best < 0) best = i;
                continue;
            }
            r = 0;
        } else {
            if (strcmp(b->name, src)) continue;
            r = 0;
        }
        snprintf(out, cap, "%s", b->name);
    }
    if (r == -ENOENT && best >= 0 && !want_removable) {
        snprintf(out, cap, "%s", all[best].name);
        *fixed_esp = 1;
        r = 0;
    }
    free(all);
    return r;
}

// Start the server for `type` on `dev` and mount it at `dir`
static int do_mount(const char *src, const char *type, const char *opts, const char *dir, int quiet) {
    char dev[64];
    int fixed_esp = 0;
    int r = find_device(src, dev, sizeof dev, &fixed_esp);
    if (r < 0) {
        if (!quiet) fprintf(stderr, "%s: %s: %s\n", prog, src, r == -ENOENT ? "no such block device" : strerror(-r));
        return r;
    }
    int ro = has_opt(opts, "ro");
    if (fixed_esp && !has_opt(opts, "rw!")) ro = 1;
    char probed[16] = "";
    if (!type || !strcmp(type, "auto")) {
        rx_fsprobe_t fs;
        if (rx_block_probe(dev, &fs) == 0) snprintf(probed, sizeof probed, "%s", fs.type);
        if (!*probed) {
            fprintf(stderr, "%s: %s: no filesystem found (try -t vfat)\n", prog, dev);
            return -EINVAL;
        }
        type = probed;
    }
    if (!strcmp(type, "fat") || !strcmp(type, "msdos") || !strcmp(type, "fat32") || !strcmp(type, "fat16")) type = "vfat";
    const char *server_path, *server_name;
    if (!strcmp(type, "vfat")) {
        server_path = "/bin/fatfs";
        server_name = "fatfs";
    } else if (!strcmp(type, "ext2") || !strcmp(type, "ext3") || !strcmp(type, "ext4")) {
        server_path = "/bin/ext2fs";
        server_name = "ext2fs";
    } else {
        fprintf(stderr, "%s: %s: cannot mount %s filesystems (vfat and ext2)\n", prog, dev, type);
        return -ENOTSUP;
    }
    struct stat st;
    if (stat(dir, &st) < 0 || !S_ISDIR(st.st_mode)) {
        fprintf(stderr, "%s: %s: %s\n", prog, dir, stat(dir, &st) < 0 ? strerror(errno) : "not a directory");
        return -ENOTDIR;
    }
    char was[256];
    if (mounted_at(dir, was, sizeof was) && strcmp(dir, "/")) {
        fprintf(stderr, "%s: %s: %s is mounted there already\n", prog, dir, was);
        return -EBUSY;
    }
    rx_handle_t client, server, proc;
    if (rx_channel_create(0, &client, &server) != RX_OK) return -ENOMEM;
    char *argv[] = { (char *)server_name, ro ? "-r" : dev, ro ? dev : 0, 0 };
    rx_spawn_handle_t hs[] = { { RX_PA_USER0, server } };
    // In the default job, so the server outlives this command (and Ctrl+C)
    if (rx_spawn_etc(rx_job_default, server_path, argv, 0, RX_SPAWN_CLONE_ALL, hs, 1, &proc) != RX_OK) {
        fprintf(stderr, "%s: cannot start %s: %s\n", prog, server_path, strerror(errno));
        rx_handle_close(client);
        return -ENOENT;
    }
    // The server answers a STAT once the filesystem is loaded, or exits
    rx_fs_hdr_t h = { 1, RX_FS_OP_STAT };
    rx_channel_write(client, 0, &h, sizeof h, 0, 0);
    rx_wait_item_t w[2] = { { client, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 }, { proc, RX_PROCESS_TERMINATED, 0 } };
    rx_object_wait_many(w, 2, rx_clock_get_monotonic() + 30000000000ull);
    char rep[sizeof(rx_fs_reply_t) + sizeof(rx_fs_attr_t)];
    uint32_t nb = 0, nh = 0;
    rx_handle_t hh;
    if (!(w[0].pending & RX_CHANNEL_READABLE) ||
        rx_channel_read(client, 0, rep, &hh, sizeof rep, 1, &nb, &nh) != RX_OK || nb < sizeof(rx_fs_reply_t)) {
        if (!(w[1].pending & RX_PROCESS_TERMINATED)) {
            fprintf(stderr, "%s: %s: the filesystem server does not answer\n", prog, dev);
            rx_task_kill(proc);
        }
        rx_handle_close(proc);
        rx_handle_close(client);
        return -EIO;
    }
    rx_handle_close(proc);
    // The server may serve it read-only (features it does not keep up)
    rx_fs_reply_t rp;
    memcpy(&rp, rep, sizeof rp);
    if (rp.status == 0 && nb >= sizeof rp + sizeof(rx_fs_attr_t)) {
        rx_fs_attr_t a;
        memcpy(&a, rep + sizeof rp, sizeof a);
        if (a.read_only) ro = 1;
    }
    char source[80];
    snprintf(source, sizeof source, "/dev/%s", dev);
    r = rx_mount(source, dir, type, ro ? MS_RDONLY : 0, client);
    if (r < 0) {
        fprintf(stderr, "%s: %s: %s\n", prog, dir, strerror(-r));
        return r;
    }
    if (fixed_esp && !has_opt(opts, "ro") && !quiet)
        printf("%s: %s is on a fixed disk: mounted read-only (rw! in the options to write)\n", prog, dev);
    return 0;
}

struct fsent {
    char src[256], dir[256], type[32], opts[64];
};

static int read_fstab(struct fsent *e, int max) {
    FILE *f = fopen("/etc/fstab", "r");
    if (!f) return -1;
    char line[700];
    int n = 0;
    while (n < max && fgets(line, sizeof line, f)) {
        char *fl[4];
        int k = fields(line, fl, 4);
        if (k < 2) continue;
        snprintf(e[n].src, sizeof e[n].src, "%s", fl[0]);
        snprintf(e[n].dir, sizeof e[n].dir, "%s", fl[1]);
        snprintf(e[n].type, sizeof e[n].type, "%s", k > 2 ? fl[2] : "auto");
        snprintf(e[n].opts, sizeof e[n].opts, "%s", k > 3 ? fl[3] : "defaults");
        n++;
    }
    fclose(f);
    return n;
}

int main(int argc, char **argv) {
    const char *type = 0, *opts = "", *root_spec = 0;
    int all = 0, wait_s = 10, c;
    // init at boot: mount --root SPEC [-t TYPE] [-o OPTS] (the root= of
    // the kernel command line; waits up to 30 s for the disk)
    if (argc >= 3 && !strcmp(argv[1], "--root")) {
        root_spec = argv[2];
        optind = 3;
        wait_s = 30;
    }
    while ((c = getopt(argc, argv, "t:o:aw:h")) != -1) {
        switch (c) {
        case 't': type = optarg; break;
        case 'o': opts = optarg; break;
        case 'a': all = 1; break;
        case 'w': wait_s = atoi(optarg); break;
        default: usage(); return 2;
        }
    }
    int rest = argc - optind;
    if (root_spec) {
        if (rest) { usage(); return 2; }
        uint64_t until = rx_clock_get_monotonic() + (uint64_t)wait_s * 1000000000ull;
        int r;
        for (;;) {
            r = do_mount(root_spec, type, opts, "/", 1);
            if (r != -ENOENT || rx_clock_get_monotonic() >= until) break;
            usleep(250000);
        }
        if (r == -ENOENT) fprintf(stderr, "mount: root %s: no such block device\n", root_spec);
        else if (r == 0) printf("mount: root filesystem: %s\n", root_spec);
        return r < 0;
    }
    if (!all && rest == 0) return list();
    struct fsent tab[32];
    if (all) {
        if (rest) { usage(); return 2; }
        int n = read_fstab(tab, 32);
        if (n < 0) {
            fprintf(stderr, "mount: /etc/fstab: %s\n", strerror(errno));
            return 1;
        }
        int failed = 0;
        uint64_t until = rx_clock_get_monotonic() + (uint64_t)wait_s * 1000000000ull;
        for (int i = 0; i < n; i++) {
            if (has_opt(tab[i].opts, "noauto") || !strcmp(tab[i].type, "swap") || mounted_at(tab[i].dir, 0, 0)) continue;
            int r;
            for (;;) {
                want_removable = rx_clock_get_monotonic() < until;
                r = do_mount(tab[i].src, tab[i].type, tab[i].opts, tab[i].dir, 1);
                if (r != -ENOENT || !want_removable) break;
                usleep(250000);
            }
            if (r == -ENOENT && !has_opt(tab[i].opts, "nofail")) {
                fprintf(stderr, "mount: %s: no such block device\n", tab[i].src);
            }
            if (r < 0 && !(r == -ENOENT && has_opt(tab[i].opts, "nofail"))) failed = 1;
        }
        return failed;
    }
    if (rest == 1) {
        // From /etc/fstab, by directory or source
        int n = read_fstab(tab, 32);
        for (int i = 0; i < n; i++) {
            if (!strcmp(tab[i].dir, argv[optind]) || !strcmp(tab[i].src, argv[optind]))
                return do_mount(tab[i].src, type ? type : tab[i].type, *opts ? opts : tab[i].opts, tab[i].dir, 0) < 0;
        }
        fprintf(stderr, "mount: %s: not in /etc/fstab\n", argv[optind]);
        return 1;
    }
    if (rest != 2) { usage(); return 2; }
    return do_mount(argv[optind], type, opts, argv[optind + 1], 0) < 0;
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// rustica-install: install Rustica Server on a disk (checklist step 20,
// docs/INSTALL.md). Run from the live system ("Install Rustica Server" in
// the boot menu starts it on the console; root may start it from a live
// session too). It asks, shows every change, and writes only after the
// user types "install":
//
//   language and keyboard, the disk (size, model, partitions and the
//   systems on it), then how: erase the disk / alongside the systems in
//   its free space / replace an earlier Rustica / manual (choose the EFI
//   and root partitions), the host name, the time zone, the first user
//   (in group sudo) and root (locked, or a password).
//
// Then: the partition table (new partitions only, or a new table when
// erasing), a FAT32 EFI partition (512 MiB) when there is none to reuse,
// ext4 for the root; the root's /etc from the boot image with the
// settings and the user; rboot and the kernel on the EFI partition with
// an entry (Windows and Linux are found by rboot at boot); a firmware boot
// entry first in BootOrder. The other systems' partitions and files are
// not touched. The firmware's order changes last, so a power cut before
// the end leaves the computer starting what it started before.
//
// Options (for scripts and tests; without them it asks):
//   --disk NAME|auto --mode erase|alongside|replace|manual [--esp PART|new] [--root PART|new]
//   --keymap NAME --hostname NAME --timezone ZONE
//   --user NAME [--fullname TEXT] --password PASS | --password-file FILE
//   --lock-root | --root-password PASS
//   --yes (no questions; the summary is still shown) --dry-run (write nothing)
//   --reboot | --no-reboot
// The log: /var/log/install.log (copied to the installed system).

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <crypt.h>
#include <rustux/device.h>
#include <rustux/fsprobe.h>
#include <spawn.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "efivar.h"

extern char **environ;

#define MIB (1024ull * 1024ull)
#define ESP_SIZE (512 * MIB)
#define MIN_ROOT (512 * MIB)   // the system itself is in the kernel's boot image; the root holds settings and data
#define LOG_FILE "/var/log/install.log"
#define TGT "/run/install/root"
#define TGT_ESP "/run/install/esp"

static const uint8_t ESP_TYPE[16] = { 0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B };

// ---- what was chosen ----

static struct {
    char disk[32], mode[16], esp_arg[32], root_arg[32];
    char keymap[64], hostname[64], timezone[64];
    char user[32], fullname[64], password[128], root_password[128];
    int lock_root, yes, dry_run, reboot; // reboot: 1 yes, 0 no, -1 ask
    int interactive;
} opt = { .lock_root = -1, .reboot = -1 };

static FILE *logf;

static void logmsg(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void logmsg(const char *fmt, ...) {
    if (!logf) return;
    time_t t = time(0);
    struct tm tm;
    localtime_r(&t, &tm);
    char when[24];
    strftime(when, sizeof when, "%Y-%m-%d %H:%M:%S", &tm);
    va_list ap;
    va_start(ap, fmt);
    fprintf(logf, "%s ", when);
    vfprintf(logf, fmt, ap);
    fputc('\n', logf);
    fflush(logf);
    va_end(ap);
}

// On the screen and in the log
static void say(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    printf("%s\n", buf);
    fflush(stdout);
    logmsg("%s", buf);
}

static __attribute__((noreturn)) void die(const char *fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    printf("\nrustica-install: %s\n", buf);
    logmsg("ERROR: %s", buf);
    printf("Nothing more was changed. The log is %s.\n", LOG_FILE);
    exit(1);
}

static void human(uint64_t n, char *buf, size_t cap) {
    const char *u = "BKMGTP";
    double v = (double)n;
    int k = 0;
    while (v >= 1024 && k < 5) { v /= 1024; k++; }
    snprintf(buf, cap, v >= 100 || k == 0 ? "%.0f%c" : "%.1f%c", v, u[k]);
}

// ---- asking ----

static void ask(const char *q, const char *def, char *out, size_t cap) {
    for (;;) {
        if (def && *def) printf("%s [%s]: ", q, def);
        else printf("%s: ", q);
        fflush(stdout);
        char line[256];
        if (!fgets(line, sizeof line, stdin)) die("no more input");
        line[strcspn(line, "\r\n")] = 0;
        char *s = line;
        while (*s == ' ') s++;
        if (!*s && def) s = (char *)def;
        if (*s) {
            snprintf(out, cap, "%s", s);
            return;
        }
    }
}

static int ask_yes(const char *q, int def) {
    char a[16];
    ask(q, def ? "Y/n" : "y/N", a, sizeof a);
    if (!strcmp(a, "Y/n")) return 1;
    if (!strcmp(a, "y/N")) return 0;
    return a[0] == 'y' || a[0] == 'Y';
}

static void ask_password(const char *who, char *out, size_t cap) {
    for (;;) {
        char p1[128], p2[128];
        char q[96];
        snprintf(q, sizeof q, "Password for %s: ", who);
        char *a = getpass(q);
        snprintf(p1, sizeof p1, "%s", a ? a : "");
        snprintf(q, sizeof q, "The same again: ");
        a = getpass(q);
        snprintf(p2, sizeof p2, "%s", a ? a : "");
        if (!p1[0]) {
            printf("An empty password is not allowed.\n");
            continue;
        }
        if (strcmp(p1, p2)) {
            printf("They differ; once more.\n");
            continue;
        }
        snprintf(out, cap, "%s", p1);
        memset(p1, 0, sizeof p1);
        memset(p2, 0, sizeof p2);
        return;
    }
}

// ---- running programs ----

// Run argv (output to the log); the exit code
static int run(char *const argv[]) {
    char line[512] = "";
    size_t k = 0;
    for (int i = 0; argv[i] && k < sizeof line - 2; i++) k += (size_t)snprintf(line + k, sizeof line - k, "%s%s", i ? " " : "", argv[i]);
    logmsg("run: %s", line);
    if (opt.dry_run) return 0;
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_addopen(&fa, 1, LOG_FILE, O_WRONLY | O_APPEND | O_CREAT, 0600);
    posix_spawn_file_actions_adddup2(&fa, 1, 2);
    pid_t pid;
    int r = posix_spawn(&pid, argv[0], &fa, 0, argv, environ);
    posix_spawn_file_actions_destroy(&fa);
    if (r != 0) {
        logmsg("cannot start %s: %s", argv[0], strerror(r));
        return 127;
    }
    int st = 0;
    if (waitpid(pid, &st, 0) < 0) return 126;
    int code = WIFEXITED(st) ? WEXITSTATUS(st) : 128;
    if (code) logmsg("exit code %d", code);
    return code;
}

#define RUN(...) run((char *const[]){ __VA_ARGS__, 0 })

// ---- disks ----

typedef struct {
    rx_block_info_t info;   // the partition
    rx_fsprobe_t fs;
    int has_fs;
    char systems[128];      // what is on it (an EFI partition's loaders, Rustica's root)
} part_t;

typedef struct {
    rx_block_info_t info;
    rx_gpt_t *gpt;          // NULL: no GPT
    int has_mbr;
    part_t parts[RX_GPT_MAX_PARTS];
    int nparts;
    uint64_t free_first, free_last;  // the largest free region (blocks, 1 MiB aligned), 0/0 if none
    char systems[256];
    int live;               // holds the system we run from
} disk_t;

static disk_t disks[16];
static int ndisks;

static rx_handle_t disk_ch;
static uint32_t disk_bs;
static int drd(void *c, uint64_t lba, uint32_t count, void *buf) { (void)c; return rx_block_read(disk_ch, disk_bs, lba, count, buf); }
static int dwr(void *c, uint64_t lba, uint32_t count, const void *buf) { (void)c; return rx_block_write(disk_ch, disk_bs, lba, count, buf); }

static int is_esp(const part_t *p) {
    return (p->info.flags & RX_BLOCK_FLAG_GPT) ? !memcmp(p->info.type_guid, ESP_TYPE, 16) : p->info.type_guid[0] == 0xEF;
}

static int is_rustica_root(const part_t *p) {
    return !strcmp(p->info.label, "Rustica root") || (p->has_fs && !strcmp(p->fs.label, "rustica-root"));
}

static int exists(const char *p) {
    struct stat st;
    return stat(p, &st) == 0;
}

// The loaders on an EFI partition (mounted read-only for a moment)
static void esp_systems(part_t *p) {
    mkdir("/run/install", 0755);
    mkdir("/run/install/look", 0755);
    char dev[48];
    snprintf(dev, sizeof dev, "%s", p->info.name);
    int saved = opt.dry_run;
    opt.dry_run = 0;
    int r = RUN("/bin/mount", "-o", "ro", dev, "/run/install/look");
    opt.dry_run = saved;
    if (r != 0) return;
    char *s = p->systems;
    size_t cap = sizeof p->systems, k = strlen(s);
#define ADD(name) do { k += (size_t)snprintf(s + k, k < cap ? cap - k : 0, "%s%s", k ? ", " : "", name); } while (0)
    if (exists("/run/install/look/EFI/Microsoft/Boot/bootmgfw.efi")) ADD("Windows Boot Manager");
    if (exists("/run/install/look/EFI/Rustica/rustux.efi")) ADD("Rustica");
    DIR *d = opendir("/run/install/look/EFI");
    struct dirent *e;
    while (d && (e = readdir(d))) {
        if (e->d_name[0] == '.' || !strcasecmp(e->d_name, "BOOT") || !strcasecmp(e->d_name, "Microsoft") || !strcasecmp(e->d_name, "Rustica")) continue;
        char path[300];
        const char *progs[] = { "shimx64.efi", "grubx64.efi", "systemd-bootx64.efi" };
        for (int i = 0; i < 3; i++) {
            snprintf(path, sizeof path, "/run/install/look/EFI/%s/%s", e->d_name, progs[i]);
            if (exists(path)) {
                char n[96];
                snprintf(n, sizeof n, "Linux (%s)", e->d_name);
                ADD(n);
                break;
            }
        }
    }
    if (d) closedir(d);
#undef ADD
    RUN("/bin/umount", "/run/install/look");
}

// The live medium: the partition rboot started from, or what /boot/efi is
static void live_partition(char *out, size_t cap) {
    out[0] = 0;
    char s[64];
    uint8_t g[16];
    if (ev_get_str(EFI_LOADER_GUID, "LoaderDevicePartUUID", s, sizeof s) == 0 && rx_guid_parse(s, g) == 0) {
        rx_block_info_t *all = 0;
        int n = rx_block_list(&all);
        for (int i = 0; i < n; i++)
            if ((all[i].flags & RX_BLOCK_FLAG_PARTITION) && !memcmp(all[i].guid, g, 16)) snprintf(out, cap, "%s", all[i].name);
        free(all);
        if (out[0]) return;
    }
    FILE *f = fopen("/etc/mtab", "r");
    char line[256];
    while (f && fgets(line, sizeof line, f)) {
        char *save = 0, *dev = strtok_r(line, " \t\n", &save), *dir = strtok_r(0, " \t\n", &save);
        if (dev && dir && !strcmp(dir, "/boot/efi")) snprintf(out, cap, "%s", !strncmp(dev, "/dev/", 5) ? dev + 5 : dev);
    }
    if (f) fclose(f);
}

static void free_space(disk_t *d) {
    d->free_first = d->free_last = 0;
    if (!d->gpt) return;
    uint64_t per_mib = MIB / d->info.block_size;
    uint64_t lo = d->gpt->first_usable, hi = d->gpt->last_usable;
    // Sort the used ranges, then look at the gaps
    uint64_t used[RX_GPT_MAX_PARTS][2];
    int n = 0;
    for (uint32_t i = 0; i < d->gpt->entries && i < RX_GPT_MAX_PARTS; i++) {
        rx_gpt_part_t *p = &d->gpt->parts[i];
        if (rx_guid_is_zero(p->type)) continue;
        used[n][0] = p->first;
        used[n][1] = p->last;
        n++;
    }
    for (int i = 1; i < n; i++)
        for (int j = i; j > 0 && used[j - 1][0] > used[j][0]; j--) {
            uint64_t a = used[j][0], b = used[j][1];
            used[j][0] = used[j - 1][0]; used[j][1] = used[j - 1][1];
            used[j - 1][0] = a; used[j - 1][1] = b;
        }
    uint64_t cur = lo;
    for (int i = 0; i <= n; i++) {
        uint64_t end = i < n ? used[i][0] : hi + 1;  // the gap is [cur, end)
        uint64_t a = (cur + per_mib - 1) / per_mib * per_mib;
        uint64_t b = end / per_mib * per_mib;      // aligned end (exclusive)
        if (b > a && b - a > d->free_last - d->free_first + (d->free_last ? 1 : 0)) {
            d->free_first = a;
            d->free_last = b - 1;
        }
        if (i < n && used[i][1] + 1 > cur) cur = used[i][1] + 1;
    }
}

static uint64_t free_bytes(const disk_t *d) {
    return d->free_last ? (d->free_last - d->free_first + 1) * d->info.block_size : 0;
}

static void scan_disks(void) {
    char live[48];
    live_partition(live, sizeof live);
    rx_block_info_t *all = 0;
    int n = rx_block_list(&all);
    if (n < 0) die("cannot list the disks: %s", strerror(-n));
    char live_disk[32] = "";
    for (int i = 0; i < n; i++)
        if (!strcmp(all[i].name, live)) snprintf(live_disk, sizeof live_disk, "%s", all[i].parent);
    ndisks = 0;
    for (int i = 0; i < n && ndisks < 16; i++) {
        rx_block_info_t *b = &all[i];
        if (b->flags & RX_BLOCK_FLAG_PARTITION) continue;
        if (!strcmp(b->driver, "ramdisk")) continue;
        disk_t *d = &disks[ndisks++];
        memset(d, 0, sizeof *d);
        d->info = *b;
        d->live = live_disk[0] && !strcmp(b->name, live_disk);
        for (int j = 0; j < n; j++) {
            if (!(all[j].flags & RX_BLOCK_FLAG_PARTITION) || strcmp(all[j].parent, b->name) || d->nparts == RX_GPT_MAX_PARTS) continue;
            part_t *p = &d->parts[d->nparts++];
            memset(p, 0, sizeof *p);
            p->info = all[j];
            p->has_fs = rx_block_probe(p->info.name, &p->fs) == 0;
            if (is_esp(p) && !d->live) esp_systems(p);
            if (is_rustica_root(p)) snprintf(p->systems + strlen(p->systems), sizeof p->systems - strlen(p->systems), "%sRustica root", p->systems[0] ? ", " : "");
            if (p->has_fs && !strcmp(p->fs.type, "ntfs") && !strstr(d->systems, "Windows data"))
                snprintf(d->systems + strlen(d->systems), sizeof d->systems - strlen(d->systems), "%sWindows data (NTFS)", d->systems[0] ? ", " : "");
            if (p->systems[0])
                snprintf(d->systems + strlen(d->systems), sizeof d->systems - strlen(d->systems), "%s%s", d->systems[0] ? ", " : "", p->systems);
        }
        // Sort the partitions by number
        for (int x = 1; x < d->nparts; x++)
            for (int y = x; y > 0 && d->parts[y - 1].info.number > d->parts[y].info.number; y--) {
                part_t t = d->parts[y];
                d->parts[y] = d->parts[y - 1];
                d->parts[y - 1] = t;
            }
        if (b->flags & RX_BLOCK_FLAG_GPT) {
            if (rx_block_open(b->name, &disk_ch) == 0) {
                disk_bs = b->block_size;
                d->gpt = malloc(sizeof *d->gpt);
                if (d->gpt && rx_gpt_read(d->gpt, b->block_size, b->block_count, drd, 0) != 0) {
                    free(d->gpt);
                    d->gpt = 0;
                }
                rx_handle_close(disk_ch);
            }
        } else if (b->flags & RX_BLOCK_FLAG_MBR) {
            d->has_mbr = 1;
        }
        free_space(d);
    }
    free(all);
}

static void show_disk(int k, const disk_t *d) {
    char sz[16];
    human(d->info.block_count * d->info.block_size, sz, sizeof sz);
    printf("  %d) %-10s %7s  %s%s%s\n", k, d->info.name, sz, d->info.model[0] ? d->info.model : d->info.driver,
           d->live ? "  (the installation medium: not offered)" : "",
           (d->info.flags & RX_BLOCK_FLAG_READONLY) && !strcmp(d->info.driver, "nvme") ? "  (internal NVMe disk)" : "");
    printf("      %s", d->gpt ? "GPT" : d->has_mbr ? "MBR (not supported for installing: GPT only)" : "no partition table");
    if (d->systems[0]) printf("; holds %s", d->systems);
    printf("\n");
    for (int i = 0; i < d->nparts; i++) {
        const part_t *p = &d->parts[i];
        human(p->info.block_count * p->info.block_size, sz, sizeof sz);
        const char *t = (p->info.flags & RX_BLOCK_FLAG_GPT) ? rx_gpt_type_name(p->info.type_guid) : "MBR partition";
        printf("      %-12s %7s  %-20s %-6s %s%s%s\n", p->info.name, sz, t, p->has_fs ? p->fs.type : "", p->info.label,
               p->systems[0] ? "  - " : "", p->systems);
    }
    if (d->free_last) {
        human(free_bytes(d), sz, sizeof sz);
        printf("      free space   %7s\n", sz);
    }
}

static disk_t *find_disk(const char *name) {
    for (int i = 0; i < ndisks; i++) if (!strcmp(disks[i].info.name, name)) return &disks[i];
    return 0;
}

static part_t *find_part(disk_t *d, const char *name) {
    for (int i = 0; i < d->nparts; i++) {
        char alt[48];
        snprintf(alt, sizeof alt, "%u", d->parts[i].info.number);
        if (!strcmp(d->parts[i].info.name, name) || !strcmp(alt, name)) return &d->parts[i];
    }
    return 0;
}

// ---- the plan ----

static struct {
    disk_t *d;
    int new_table;       // erase: a new GPT
    part_t *esp;         // an existing EFI partition to reuse (NULL: make one)
    part_t *root;        // an existing partition to format (NULL: make one)
    uint64_t esp_first, esp_last, root_first, root_last;  // new partitions (blocks)
    int esp_num, root_num;
    int windows;         // Windows on the disk: the hardware clock keeps local time
    int nvme;            // the internal NVMe disk: unlock writes, nvme=rw
} plan;

static void plan_new_parts(uint64_t first, uint64_t last, int need_esp) {
    uint64_t per_mib = MIB / plan.d->info.block_size;
    if (need_esp) {
        plan.esp_first = first;
        plan.esp_last = first + ESP_SIZE / plan.d->info.block_size - 1;
        first = (plan.esp_last + 1 + per_mib - 1) / per_mib * per_mib;
    }
    plan.root_first = first;
    plan.root_last = last;
}

static void choose(void) {
    scan_disks();
    int offered = 0;
    for (int i = 0; i < ndisks; i++) if (!disks[i].live) offered++;
    if (!offered) die("no disk to install on (the installation medium itself is not offered)");
    if (opt.interactive && !opt.disk[0]) {
        printf("\nDisks:\n");
        for (int i = 0; i < ndisks; i++) show_disk(i + 1, &disks[i]);
        printf("\n");
        for (;;) {
            char a[32], def[8];
            int first = 0;
            for (int i = 0; i < ndisks; i++) if (!disks[i].live) { first = i + 1; break; }
            snprintf(def, sizeof def, "%d", first);
            ask("The disk to install on (its number)", def, a, sizeof a);
            int k = atoi(a);
            if (k >= 1 && k <= ndisks && !disks[k - 1].live) {
                snprintf(opt.disk, sizeof opt.disk, "%s", disks[k - 1].info.name);
                break;
            }
            disk_t *d = find_disk(a);
            if (d && !d->live) {
                snprintf(opt.disk, sizeof opt.disk, "%s", a);
                break;
            }
            printf("Not one of the disks above.\n");
        }
    }
    // auto: the one disk there is besides the installation medium
    if (!strcmp(opt.disk, "auto")) {
        disk_t *only = 0;
        for (int i = 0; i < ndisks; i++)
            if (!disks[i].live) {
                if (only) die("--disk auto: more than one disk (%s, %s): name it", only->info.name, disks[i].info.name);
                only = &disks[i];
            }
        snprintf(opt.disk, sizeof opt.disk, "%s", only->info.name);
    }
    plan.d = find_disk(opt.disk);
    if (!plan.d) die("no disk %s (lsblk lists them)", opt.disk);
    if (plan.d->live) die("%s holds the installation medium", opt.disk);
    disk_t *d = plan.d;
    plan.nvme = !strcmp(d->info.driver, "nvme");
    plan.windows = strstr(d->systems, "Windows") != 0;
    part_t *esp = 0, *rroot = 0;
    for (int i = 0; i < d->nparts; i++) {
        if (is_esp(&d->parts[i]) && !esp) esp = &d->parts[i];
        if (is_rustica_root(&d->parts[i]) && !rroot) rroot = &d->parts[i];
    }
    int can_alongside = d->gpt && free_bytes(d) >= (esp ? 0 : ESP_SIZE) + MIN_ROOT;
    int can_replace = d->gpt && esp && rroot;
    if (opt.interactive && !opt.mode[0]) {
        char sz[16];
        human(free_bytes(d), sz, sizeof sz);
        printf("\nHow to install on %s:\n", d->info.name);
        printf("  1) erase the whole disk: everything on %s is lost%s\n", d->info.name, d->systems[0] ? " (including the systems on it)" : "");
        if (can_alongside) printf("  2) alongside: in the free space (%s); the other partitions stay as they are\n", sz);
        else printf("  2) alongside: not possible (%s)\n", !d->gpt ? "no GPT" : "too little free space; shrink a partition in its own system first");
        if (can_replace) printf("  3) replace the Rustica on %s (formatted; its EFI files replaced)\n", rroot->info.name);
        printf("  4) manual: choose the EFI and root partitions\n");
        for (;;) {
            char a[8];
            ask("Choice", can_replace ? "3" : can_alongside ? "2" : "1", a, sizeof a);
            if (a[0] == '1') { strcpy(opt.mode, "erase"); break; }
            if (a[0] == '2' && can_alongside) { strcpy(opt.mode, "alongside"); break; }
            if (a[0] == '3' && can_replace) { strcpy(opt.mode, "replace"); break; }
            if (a[0] == '4') { strcpy(opt.mode, "manual"); break; }
        }
    }
    if (!strcmp(opt.mode, "erase")) {
        plan.new_table = 1;
        uint64_t per_mib = MIB / d->info.block_size;
        uint64_t last = (d->info.block_count - 34) / per_mib * per_mib - 1;  // the backup table at the end
        if ((last + 1 - per_mib) * d->info.block_size < ESP_SIZE + MIN_ROOT) die("%s is too small", d->info.name);
        plan_new_parts(per_mib, last, 1);
        plan.windows = 0;
    } else if (!strcmp(opt.mode, "alongside")) {
        if (!can_alongside) die("%s: not enough free space (or no GPT) to install alongside", d->info.name);
        plan.esp = esp;
        plan_new_parts(d->free_first, d->free_last, esp == 0);
    } else if (!strcmp(opt.mode, "replace")) {
        if (!can_replace) die("%s has no earlier Rustica (an EFI partition and a partition labelled \"Rustica root\")", d->info.name);
        plan.esp = esp;
        plan.root = rroot;
    } else if (!strcmp(opt.mode, "manual")) {
        if (!d->gpt) die("%s has no GPT: erase it, or make one with gpt create", d->info.name);
        if (opt.interactive && !opt.esp_arg[0]) {
            printf("\n");
            show_disk(1, d);
            ask("The EFI partition: an existing one (its name) or new (512 MiB of the free space)", esp ? esp->info.name : "new", opt.esp_arg, sizeof opt.esp_arg);
        }
        if (!strcmp(opt.esp_arg, "new") || !opt.esp_arg[0]) plan.esp = 0;
        else if (!(plan.esp = find_part(d, opt.esp_arg)) || !is_esp(plan.esp)) die("%s is not an EFI system partition on %s", opt.esp_arg, d->info.name);
        if (opt.interactive && !opt.root_arg[0])
            ask("The root partition: one to format (its name; what is on it is lost) or new (the free space)", "new", opt.root_arg, sizeof opt.root_arg);
        if (!strcmp(opt.root_arg, "new") || !opt.root_arg[0]) {
            if (free_bytes(d) < (plan.esp ? 0 : ESP_SIZE) + MIN_ROOT) die("not enough free space on %s", d->info.name);
            plan_new_parts(d->free_first, d->free_last, plan.esp == 0);
        } else {
            if (!(plan.root = find_part(d, opt.root_arg))) die("no partition %s on %s", opt.root_arg, d->info.name);
            if (plan.root == plan.esp || is_esp(plan.root)) die("the root cannot be an EFI partition");
            if (plan.root->info.block_count * plan.root->info.block_size < MIN_ROOT) die("%s is too small (at least 512 MiB)", plan.root->info.name);
            if (!plan.esp) {
                if (free_bytes(d) < ESP_SIZE) die("no room for a new EFI partition on %s", d->info.name);
                plan.esp_first = d->free_first;
                plan.esp_last = d->free_first + ESP_SIZE / d->info.block_size - 1;
            }
        }
    } else {
        die("--mode must be erase, alongside, replace or manual");
    }
    // Partition numbers for what is new
    int used[RX_GPT_MAX_PARTS + 1] = { 0 };
    if (!plan.new_table && d->gpt)
        for (uint32_t i = 0; i < d->gpt->entries && i < RX_GPT_MAX_PARTS; i++)
            if (!rx_guid_is_zero(d->gpt->parts[i].type)) used[i + 1] = 1;
    int next = 1;
    if (!plan.esp && plan.esp_last) {
        while (used[next]) next++;
        plan.esp_num = next;
        used[next] = 1;
    }
    if (!plan.root) {
        while (used[next]) next++;
        plan.root_num = next;
    }
}

static void choose_settings(void) {
    if (!opt.keymap[0]) {
        if (opt.interactive) {
            printf("\nKeyboard layouts: us");
            DIR *dd = opendir("/usr/share/keymaps");
            struct dirent *e;
            while (dd && (e = readdir(dd))) if (e->d_name[0] != '.') printf(" %s", e->d_name);
            if (dd) closedir(dd);
            printf("\n");
            ask("Keyboard layout", "us", opt.keymap, sizeof opt.keymap);
        } else {
            strcpy(opt.keymap, "us");
        }
    }
    if (strcmp(opt.keymap, "us")) {
        char p[128];
        snprintf(p, sizeof p, "/usr/share/keymaps/%s", opt.keymap);
        if (!exists(p)) die("no keyboard layout %s", opt.keymap);
    }
    // Use it now, for the passwords
    if (opt.interactive && !opt.dry_run) RUN("/bin/localectl", "set-keymap", opt.keymap);
    if (!opt.hostname[0]) {
        if (opt.interactive) ask("Host name", "rustica", opt.hostname, sizeof opt.hostname);
        else strcpy(opt.hostname, "rustica");
    }
    for (const char *c = opt.hostname; *c; c++)
        if (!((*c >= 'a' && *c <= 'z') || (*c >= 'A' && *c <= 'Z') || (*c >= '0' && *c <= '9') || *c == '-')) die("host name: letters, digits and - only");
    if (!opt.timezone[0]) {
        if (opt.interactive) {
            for (;;) {
                ask("Time zone (as Europe/Berlin; ? lists them)", "UTC", opt.timezone, sizeof opt.timezone);
                if (strcmp(opt.timezone, "?")) break;
                RUN("/bin/timedatectl", "list-timezones");
                FILE *f = fopen("/usr/share/zoneinfo/zones", "r");
                char line[128];
                int col = 0;
                while (f && fgets(line, sizeof line, f)) {
                    if (line[0] == '#') continue;
                    line[strcspn(line, " \n")] = 0;
                    printf("%-24s%s", line, ++col % 3 ? "" : "\n");
                }
                if (f) fclose(f);
                printf("\n");
            }
        } else {
            strcpy(opt.timezone, "UTC");
        }
    }
    {
        FILE *f = fopen("/usr/share/zoneinfo/zones", "r");
        char line[128];
        int ok = 0;
        size_t l = strlen(opt.timezone);
        while (f && fgets(line, sizeof line, f)) if (!strncmp(line, opt.timezone, l) && line[l] == ' ') ok = 1;
        if (f) fclose(f);
        if (!ok) die("no time zone %s (timedatectl list-timezones)", opt.timezone);
    }
    if (!opt.user[0]) {
        if (!opt.interactive) die("--user NAME is needed");
        printf("\nThe first user (an administrator: group sudo)\n");
        ask("Full name", "", opt.fullname, sizeof opt.fullname);
        ask("User name", "", opt.user, sizeof opt.user);
    }
    if (!(opt.user[0] >= 'a' && opt.user[0] <= 'z')) die("user name: starts with a small letter");
    for (const char *c = opt.user; *c; c++)
        if (!((*c >= 'a' && *c <= 'z') || (*c >= '0' && *c <= '9') || *c == '-' || *c == '_')) die("user name: small letters, digits, - and _ only");
    if (!strcmp(opt.user, "root") || !strcmp(opt.user, "nobody")) die("user name: %s is taken", opt.user);
    if (!opt.password[0]) {
        if (!opt.interactive) die("--password or --password-file is needed");
        ask_password(opt.user, opt.password, sizeof opt.password);
    }
    if (opt.lock_root < 0) {
        if (opt.interactive) {
            opt.lock_root = ask_yes("Lock root's password (administer with sudo)?", 1);
            if (!opt.lock_root && !opt.root_password[0]) ask_password("root", opt.root_password, sizeof opt.root_password);
        } else {
            opt.lock_root = !opt.root_password[0];
        }
    }
}

static void summary(void) {
    disk_t *d = plan.d;
    char sz[16], sz2[16];
    human(d->info.block_count * d->info.block_size, sz, sizeof sz);
    printf("\n================ The installation ================\n");
    printf("Disk %s (%s, %s)%s:\n", d->info.name, d->info.model[0] ? d->info.model : d->info.driver, sz,
           plan.nvme ? " - the computer's internal NVMe disk" : "");
    logmsg("plan: disk %s mode %s", d->info.name, opt.mode);
    if (plan.new_table) {
        printf("  - ERASE the disk: a new partition table; everything on it is lost\n");
        for (int i = 0; i < d->nparts; i++) printf("      lost: %s %s %s\n", d->parts[i].info.name, d->parts[i].has_fs ? d->parts[i].fs.type : "", d->parts[i].systems);
    } else {
        for (int i = 0; i < d->nparts; i++) {
            part_t *p = &d->parts[i];
            human(p->info.block_count * p->info.block_size, sz2, sizeof sz2);
            if (p == plan.esp) printf("  - keep %s (EFI, %s%s%s): add \\EFI\\Rustica and \\loader; nothing else on it changes\n", p->info.name, sz2,
                                      p->systems[0] ? ", holds " : "", p->systems);
            else if (p == plan.root) printf("  - FORMAT %s (%s, %s) as ext4 \"rustica-root\": what is on it is lost\n", p->info.name, p->has_fs ? p->fs.type : "no filesystem", sz2);
            else printf("  - %s (%s%s%s): untouched\n", p->info.name, p->has_fs ? p->fs.type : "no filesystem", p->systems[0] ? ", " : "", p->systems);
        }
    }
    char name[48];
    const char *pfx = d->info.name[strlen(d->info.name) - 1] >= '0' && d->info.name[strlen(d->info.name) - 1] <= '9' ? "p" : "";
    if (!plan.esp && plan.esp_last) {
        human((plan.esp_last - plan.esp_first + 1) * d->info.block_size, sz2, sizeof sz2);
        snprintf(name, sizeof name, "%s%s%d", d->info.name, pfx, plan.esp_num);
        printf("  - create %s: EFI system partition, %s, FAT32\n", name, sz2);
    }
    if (!plan.root) {
        human((plan.root_last - plan.root_first + 1) * d->info.block_size, sz2, sizeof sz2);
        snprintf(name, sizeof name, "%s%s%d", d->info.name, pfx, plan.root_num);
        printf("  - create %s: \"Rustica root\", %s, ext4\n", name, sz2);
    }
    printf("Boot: rboot and the kernel in \\EFI\\Rustica; a firmware boot entry \"Rustica\" first in the boot order%s\n",
           plan.windows ? "\n      (Windows stays in rboot's menu; the hardware clock keeps local time, as Windows wants)" : "");
    printf("System: host %s, time zone %s, keyboard %s\n", opt.hostname, opt.timezone, opt.keymap);
    printf("        user %s%s%s (administrator: group sudo), root %s\n", opt.user, opt.fullname[0] ? ", " : "", opt.fullname,
           opt.lock_root ? "locked (use sudo)" : "with a password");
    printf("===================================================\n");
}

// ---- doing it ----

static char esp_dev[48], root_dev[48];
static char root_uuid[40], esp_serial[16];

static void wait_device(const char *name) {
    for (int i = 0; i < 100; i++) {
        rx_handle_t ch;
        if (rx_block_open(name, &ch) == 0) {
            rx_handle_close(ch);
            return;
        }
        usleep(100000);
    }
    die("%s did not appear after the partition table was written", name);
}

static void write_table(void) {
    disk_t *d = plan.d;
    if (!plan.new_table && plan.esp_last == 0 && plan.root) return; // nothing new
    int r = rx_block_open(d->info.name, &disk_ch);
    if (r < 0) die("%s: %s", d->info.name, strerror(-r));
    disk_bs = d->info.block_size;
    rx_gpt_t *g = malloc(sizeof *g);
    if (!g) die("out of memory");
    if (plan.new_table) {
        rx_gpt_init(g, d->info.block_size, d->info.block_count);
    } else {
        *g = *d->gpt;
    }
    if (!plan.esp && plan.esp_last) {
        rx_gpt_part_t *p = &g->parts[plan.esp_num - 1];
        memset(p, 0, sizeof *p);
        memcpy(p->type, ESP_TYPE, 16);
        rx_guid_random(p->guid);
        p->first = plan.esp_first;
        p->last = plan.esp_last;
        snprintf(p->name, sizeof p->name, "EFI system partition");
    }
    if (!plan.root) {
        rx_gpt_part_t *p = &g->parts[plan.root_num - 1];
        memset(p, 0, sizeof *p);
        rx_gpt_type_parse("linux", p->type);
        rx_guid_random(p->guid);
        p->first = plan.root_first;
        p->last = plan.root_last;
        snprintf(p->name, sizeof p->name, "Rustica root");
    }
    say("Writing the partition table of %s...", d->info.name);
    if (!opt.dry_run) {
        if ((r = rx_gpt_write(g, d->info.block_size, d->info.block_count, dwr, 0)) < 0) die("cannot write the partition table: %s", strerror(-r));
        rx_block_simple(disk_ch, RX_BLOCK_OP_FLUSH);
        r = rx_block_simple(disk_ch, RX_BLOCK_OP_RESCAN);
        if (r < 0) die("%s did not read its new table (%s): restart and run the installer again (the table is written)", d->info.name, strerror(-r));
    }
    rx_handle_close(disk_ch);
    free(g);
}

static void unlock_nvme(void) {
    if (!plan.nvme || !(plan.d->info.flags & RX_BLOCK_FLAG_READONLY)) return;
    say("Allowing writes to %s (the internal disk), as you confirmed...", plan.d->info.name);
    if (opt.dry_run) return;
    rx_handle_t ch;
    if (rx_block_open(plan.d->info.name, &ch) < 0) die("cannot open %s", plan.d->info.name);
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_ALLOW_WRITE;
    const char *w = RX_BLOCK_ALLOW_WRITE_WORDS;
    int r = rx_block_call(ch, &q, w, strlen(w), 0, 0, 0, 0);
    rx_handle_close(ch);
    if (r < 0) die("%s refused writes: %s", plan.d->info.name, strerror(-r));
}

static void part_name(int num, char *out, size_t cap) {
    const char *n = plan.d->info.name;
    size_t l = strlen(n);
    snprintf(out, cap, "%s%s%d", n, l && n[l - 1] >= '0' && n[l - 1] <= '9' ? "p" : "", num);
}

static void make_filesystems(void) {
    if (plan.esp) snprintf(esp_dev, sizeof esp_dev, "%s", plan.esp->info.name);
    else part_name(plan.esp_num, esp_dev, sizeof esp_dev);
    if (plan.root) snprintf(root_dev, sizeof root_dev, "%s", plan.root->info.name);
    else part_name(plan.root_num, root_dev, sizeof root_dev);
    if (!opt.dry_run) {
        wait_device(esp_dev);
        wait_device(root_dev);
    }
    uint8_t g[16];
    rx_guid_random(g);
    rx_guid_format(g, root_uuid);
    for (char *c = root_uuid; *c; c++) if (*c >= 'A' && *c <= 'Z') *c += 32;
    if (!plan.esp) {
        uint8_t s[4];
        rx_guid_random(g);
        memcpy(s, g, 4);
        snprintf(esp_serial, sizeof esp_serial, "%02X%02X%02X%02X", s[0], s[1], s[2], s[3]);
        say("Making the EFI partition %s (FAT32)...", esp_dev);
        if (RUN("/bin/mkfs.fat", "-F", "32", "-n", "ESP", "-i", esp_serial, esp_dev)) die("mkfs.fat %s failed", esp_dev);
    }
    say("Making the root filesystem on %s (ext4)...", root_dev);
    if (RUN("/bin/mkfs.ext4", "-q", "-L", "rustica-root", "-U", root_uuid, root_dev) && RUN("/bin/mkfs.ext4", "-L", "rustica-root", "-U", root_uuid, root_dev))
        die("mkfs.ext4 %s failed", root_dev);
}

// ---- files ----

static void wfile(const char *path, const char *text, mode_t mode) {
    logmsg("write %s", path);
    if (opt.dry_run) return;
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (fd < 0) die("%s: %s", path, strerror(errno));
    size_t n = strlen(text);
    if (write(fd, text, n) != (ssize_t)n) die("%s: %s", path, strerror(errno));
    close(fd);
    chmod(path, mode);
}

static void copy_file(const char *from, const char *to, mode_t mode) {
    logmsg("copy %s -> %s", from, to);
    if (opt.dry_run) return;
    int in = open(from, O_RDONLY);
    if (in < 0) die("%s: %s", from, strerror(errno));
    int out = open(to, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (out < 0) die("%s: %s", to, strerror(errno));
    static char buf[64 * 1024];
    ssize_t n;
    while ((n = read(in, buf, sizeof buf)) > 0)
        if (write(out, buf, (size_t)n) != n) die("%s: %s", to, strerror(errno));
    if (n < 0) die("%s: %s", from, strerror(errno));
    close(in);
    if (close(out) < 0) die("%s: %s", to, strerror(errno));
}

static void copy_tree(const char *from, const char *to) {
    if (!opt.dry_run) mkdir(to, 0755);
    DIR *d = opendir(from);
    if (!d) die("%s: %s", from, strerror(errno));
    struct dirent *e;
    while ((e = readdir(d))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..") || !strcmp(e->d_name, "mtab")) continue;
        char a[512], b[512];
        snprintf(a, sizeof a, "%s/%s", from, e->d_name);
        snprintf(b, sizeof b, "%s/%s", to, e->d_name);
        struct stat st;
        if (lstat(a, &st) < 0) continue;
        if (S_ISDIR(st.st_mode)) copy_tree(a, b);
        else if (S_ISREG(st.st_mode)) copy_file(a, b, (st.st_mode & 0777) ? st.st_mode & 0777 : 0644);
    }
    closedir(d);
}

static void append(const char *path, const char *line) {
    logmsg("append to %s", path);
    if (opt.dry_run) return;
    FILE *f = fopen(path, "a");
    if (!f) die("%s: %s", path, strerror(errno));
    fputs(line, f);
    fclose(f);
}

// group: add `user` to `grp`'s members
static void add_to_group(const char *path, const char *grp, const char *user) {
    if (opt.dry_run) return;
    FILE *f = fopen(path, "r");
    if (!f) die("%s: %s", path, strerror(errno));
    char out[8192] = "", line[512];
    size_t k = 0, gl = strlen(grp);
    while (fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        if (!strncmp(line, grp, gl) && line[gl] == ':') {
            size_t l = strlen(line);
            int empty = l && line[l - 1] == ':';
            k += (size_t)snprintf(out + k, sizeof out - k, "%s%s%s\n", line, empty ? "" : ",", user);
        } else {
            k += (size_t)snprintf(out + k, sizeof out - k, "%s\n", line);
        }
    }
    fclose(f);
    wfile(path, out, 0644);
}

static void set_shadow(const char *path, const char *user, const char *hash) {
    if (opt.dry_run) return;
    FILE *f = fopen(path, "r");
    if (!f) die("%s: %s", path, strerror(errno));
    char out[8192] = "", line[512];
    size_t k = 0, ul = strlen(user);
    int found = 0;
    long days = (long)(time(0) / 86400);
    while (fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        if (!strncmp(line, user, ul) && line[ul] == ':') {
            k += (size_t)snprintf(out + k, sizeof out - k, "%s:%s:%ld:0:99999:7:::\n", user, hash, days);
            found = 1;
        } else {
            k += (size_t)snprintf(out + k, sizeof out - k, "%s\n", line);
        }
    }
    fclose(f);
    if (!found) k += (size_t)snprintf(out + k, sizeof out - k, "%s:%s:%ld:0:99999:7:::\n", user, hash, days);
    wfile(path, out, 0600);
}

static const char *hash_of(const char *pass) {
    static char h[160];
    const char *salt = crypt_gensalt("$6$", 0, 0, 0);
    const char *c = salt ? crypt(pass, salt) : 0;
    if (!c || c[0] == '*') die("cannot hash the password");
    snprintf(h, sizeof h, "%s", c);
    return h;
}

static void install_root(void) {
    say("Setting up the root filesystem...");
    mkdir("/run/install", 0755);
    mkdir(TGT, 0755);
    if (RUN("/bin/mount", "-t", "ext4", root_dev, TGT)) die("cannot mount %s", root_dev);
    const char *dirs[] = { "etc", "home", "root", "tmp", "var", "var/log", "var/lib", "mnt", "boot", "run", 0 };
    for (int i = 0; dirs[i]; i++) {
        char p[256];
        snprintf(p, sizeof p, TGT "/%s", dirs[i]);
        if (!opt.dry_run) mkdir(p, 0755);
    }
    if (!opt.dry_run) {
        chmod(TGT "/root", 0700);
        chmod(TGT "/tmp", 01777);
    }
    // /etc: the boot image's (as fsd would seed it), then the settings
    copy_tree("/boot/etc", TGT "/etc");
    if (!opt.dry_run) {
        unlink(TGT "/etc/autologin");               // the installed system asks who logs in
        unlink(TGT "/etc/svc/enabled/installer");
        chmod(TGT "/etc/shadow", 0600);
        chmod(TGT "/etc/gshadow", 0600);
        chmod(TGT "/etc/sudoers", 0440);
    }
    char buf[512];
    snprintf(buf, sizeof buf, "%s\n", opt.hostname);
    wfile(TGT "/etc/hostname", buf, 0644);
    snprintf(buf, sizeof buf, "%s\n", opt.timezone);
    wfile(TGT "/etc/timezone", buf, 0644);
    snprintf(buf, sizeof buf, "%s\n", opt.keymap);
    if (strcmp(opt.keymap, "us")) wfile(TGT "/etc/keymap", buf, 0644);
    if (plan.windows) wfile(TGT "/etc/adjtime", "0.0 0 0\n0\nLOCAL\n", 0644);
    wfile(TGT "/etc/fstab",
          "# /etc/fstab: what `mount -a` mounts (written by rustica-install).\n"
          "# The root filesystem is root= on the kernel command line (rboot's entry).\n"
          "# SOURCE      DIRECTORY   TYPE   OPTIONS\n"
          "# EFI: the EFI system partition rboot started from\n"
          "EFI           /boot/efi   vfat   rw,nofail\n", 0644);
    wfile(TGT "/etc/motd", "Rustica Server. man intro, man rustux; sudo for administration.\n", 0644);
    // The user: uid and gid 1000, group of its own, in sudo
    snprintf(buf, sizeof buf, "%s:x:1000:1000:%s:/home/%s:/bin/shell\n", opt.user, opt.fullname, opt.user);
    append(TGT "/etc/passwd", buf);
    snprintf(buf, sizeof buf, "%s:x:1000:\n", opt.user);
    append(TGT "/etc/group", buf);
    add_to_group(TGT "/etc/group", "sudo", opt.user);
    set_shadow(TGT "/etc/shadow", opt.user, hash_of(opt.password));
    set_shadow(TGT "/etc/shadow", "root", opt.lock_root ? "!" : hash_of(opt.root_password));
    char home[128];
    snprintf(home, sizeof home, TGT "/home/%s", opt.user);
    if (!opt.dry_run) {
        mkdir(home, 0750);
        if (chown(home, 1000, 1000) < 0) logmsg("chown %s: %s", home, strerror(errno));
    }
    memset(opt.password, 0, sizeof opt.password);
    memset(opt.root_password, 0, sizeof opt.root_password);
}

// The kernel and rboot from the medium we run from
static void sources(char *kernel, char *rboot, size_t cap) {
    struct stat st;
    kernel[0] = rboot[0] = 0;
    if (stat("/boot/efi/EFI/Rustica/rustux.efi", &st) == 0) snprintf(kernel, cap, "/boot/efi/EFI/Rustica/rustux.efi");
    // A stick without rboot (RBOOT=0): BOOTX64.EFI is the kernel itself (it
    // carries its boot image: far larger than rboot)
    else if (stat("/boot/efi/EFI/BOOT/BOOTX64.EFI", &st) == 0 && st.st_size > 4 * (off_t)MIB) snprintf(kernel, cap, "/boot/efi/EFI/BOOT/BOOTX64.EFI");
    if (stat("/boot/efi/EFI/Rustica/rboot.efi", &st) == 0) snprintf(rboot, cap, "/boot/efi/EFI/Rustica/rboot.efi");
    else if (stat("/usr/lib/rboot/rboot.efi", &st) == 0) snprintf(rboot, cap, "/usr/lib/rboot/rboot.efi");
}

static void install_esp(void) {
    char kernel[128], rboot[128];
    sources(kernel, rboot, sizeof kernel);
    say("Installing rboot and the kernel on %s...", esp_dev);
    mkdir(TGT_ESP, 0755);
    // An EFI partition on a fixed disk is mounted read-only unless rw!
    if (RUN("/bin/mount", "-t", "vfat", "-o", "rw!", esp_dev, TGT_ESP)) die("cannot mount %s", esp_dev);
    const char *dirs[] = { TGT_ESP "/EFI", TGT_ESP "/EFI/Rustica", TGT_ESP "/EFI/BOOT", TGT_ESP "/loader", TGT_ESP "/loader/entries", 0 };
    for (int i = 0; dirs[i]; i++) if (!opt.dry_run) mkdir(dirs[i], 0755);
    // New files under other names first, then in place: a power cut leaves
    // the old ones (an earlier Rustica's) or none
    copy_file(kernel, TGT_ESP "/EFI/Rustica/rustux.new", 0644);
    copy_file(rboot, TGT_ESP "/EFI/Rustica/rboot.new", 0644);
    if (!opt.dry_run) {
        sync();
        unlink(TGT_ESP "/EFI/Rustica/rustux.efi");
        if (rename(TGT_ESP "/EFI/Rustica/rustux.new", TGT_ESP "/EFI/Rustica/rustux.efi") < 0) die("rename: %s", strerror(errno));
        unlink(TGT_ESP "/EFI/Rustica/rboot.efi");
        if (rename(TGT_ESP "/EFI/Rustica/rboot.new", TGT_ESP "/EFI/Rustica/rboot.efi") < 0) die("rename: %s", strerror(errno));
    }
    // The firmware's fallback, only where there is none (never Windows')
    if (!exists(TGT_ESP "/EFI/BOOT/BOOTX64.EFI")) copy_file(rboot, TGT_ESP "/EFI/BOOT/BOOTX64.EFI", 0644);
    char buf[512];
    snprintf(buf, sizeof buf, "title   Rustica Server\nefi     /EFI/Rustica/rustux.efi\noptions root=UUID=%s quiet%s\nsort-key 1\n", root_uuid,
             plan.nvme ? " nvme=rw" : "");
    wfile(TGT_ESP "/loader/entries/rustica.conf", buf, 0644);
    // The same with the desktop (DESKTOP.md step F6): the graphical login
    snprintf(buf, sizeof buf, "title   Rustica (desktop)\nefi     /EFI/Rustica/rustux.efi\noptions root=UUID=%s quiet desktop%s\nsort-key 2\n",
             root_uuid, plan.nvme ? " nvme=rw" : "");
    wfile(TGT_ESP "/loader/entries/rustica-desktop.conf", buf, 0644);
    wfile(TGT_ESP "/loader/loader.conf", plan.windows ? "default rustica\ntimeout 5\n" : "default rustica\ntimeout 3\n", 0644);
}

// A firmware boot entry for rboot, first in BootOrder (last of all)
static void boot_entry(void) {
    rx_block_info_t info;
    rx_handle_t ch;
    if (opt.dry_run) {
        say("Would add the firmware boot entry \"Rustica\" (\\EFI\\Rustica\\rboot.efi on %s) first in BootOrder.", esp_dev);
        return;
    }
    if (rx_block_open(esp_dev, &ch) < 0 || rx_block_info(ch, &info) < 0) die("cannot read %s's partition data", esp_dev);
    rx_handle_close(ch);
    uint8_t dp[600], lo[1200];
    size_t dl = dp_build_hd_file(dp, sizeof dp, info.number, info.start, info.block_count, info.guid, "\\EFI\\Rustica\\rboot.efi");
    size_t ll = dl ? lo_build(lo, sizeof lo, LOAD_OPTION_ACTIVE, "Rustica", dp, dl) : 0;
    if (!ll) die("cannot build the boot entry");
    // Reuse an entry for the same loader on the same partition, else a free number
    uint16_t order[128];
    int n = ev_boot_order(order, 127);
    if (n == -ENOSYS) {
        say("No UEFI runtime services: the firmware's boot order is unchanged (\\EFI\\BOOT\\BOOTX64.EFI starts rboot if the firmware finds nothing else).");
        return;
    }
    if (n < 0) n = 0;
    int num = -1, first_free = -1;
    char name[16];
    uint8_t b[4096];
    for (int x = 0; x < 0x200 && num < 0; x++) {
        snprintf(name, sizeof name, "Boot%04X", x);
        int l = ev_get(EFI_GLOBAL_GUID, name, b, sizeof b, 0);
        if (l == -ENOENT) {
            if (first_free < 0) first_free = x;
            continue;
        }
        load_option_t o;
        if (l > 0 && lo_parse(b, (size_t)l, &o) == 0 && o.dp_len == dl && !memcmp(o.dp, dp, dl)) num = x;
    }
    if (num < 0) num = first_free;
    if (num < 0) die("no free firmware boot entry number");
    snprintf(name, sizeof name, "Boot%04X", num);
    int r = ev_set(EFI_GLOBAL_GUID, name, EV_DEFAULT, lo, ll);
    if (r < 0) die("cannot write %s: %s", name, ev_strerror(r));
    int k = 0;
    uint16_t out[128];
    out[k++] = (uint16_t)num;
    for (int i = 0; i < n && k < 128; i++) if (order[i] != num) out[k++] = order[i];
    if ((r = ev_set_boot_order(out, k)) < 0) die("cannot write BootOrder: %s", ev_strerror(r));
    say("Firmware boot entry %s \"Rustica\" is first in the boot order.", name);
}

static void finish(void) {
    say("Writing everything out...");
    if (!opt.dry_run) {
        sync();
        // The log goes with the system
        fflush(logf);
        copy_file(LOG_FILE, TGT "/var/log/install.log", 0600);
        RUN("/bin/umount", TGT_ESP);
        RUN("/bin/umount", TGT);
        sync();
    }
}

// Started by init for the install target (SERVICE=installer): when it
// ends without a restart, the live system takes the console
static int restart;
static void after(void) {
    const char *svc = getenv("SERVICE");
    if (restart || !svc || strcmp(svc, "installer")) return;
    printf("\nStarting the live system (the installer again: rustica-install)...\n");
    fflush(stdout);
    int saved = opt.dry_run;
    opt.dry_run = 0;
    RUN("/bin/svc", "isolate", "multi-user");
    opt.dry_run = saved;
}

static int usage(int code) {
    fprintf(code ? stderr : stdout,
            "usage: rustica-install [--dry-run]                     (asks)\n"
            "       rustica-install --disk NAME --mode erase|alongside|replace|manual [--esp PART|new] [--root PART|new]\n"
            "                       --user NAME (--password PASS | --password-file FILE) [--fullname TEXT]\n"
            "                       [--lock-root | --root-password PASS] [--hostname NAME] [--timezone ZONE] [--keymap NAME]\n"
            "                       [--yes] [--dry-run] [--reboot | --no-reboot]\n"
            "Install Rustica Server on a disk (man rustica-install). The log: " LOG_FILE ".\n");
    return code;
}

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
#define VAL(field) do { if (i + 1 >= argc) return usage(2); snprintf(opt.field, sizeof opt.field, "%s", argv[++i]); } while (0)
        if (!strcmp(a, "--disk")) VAL(disk);
        else if (!strcmp(a, "--mode")) VAL(mode);
        else if (!strcmp(a, "--esp")) VAL(esp_arg);
        else if (!strcmp(a, "--root")) VAL(root_arg);
        else if (!strcmp(a, "--keymap")) VAL(keymap);
        else if (!strcmp(a, "--hostname")) VAL(hostname);
        else if (!strcmp(a, "--timezone")) VAL(timezone);
        else if (!strcmp(a, "--user")) VAL(user);
        else if (!strcmp(a, "--fullname")) VAL(fullname);
        else if (!strcmp(a, "--password")) VAL(password);
        else if (!strcmp(a, "--root-password")) { VAL(root_password); opt.lock_root = 0; }
        else if (!strcmp(a, "--password-file")) {
            if (i + 1 >= argc) return usage(2);
            FILE *f = fopen(argv[++i], "r");
            if (!f || !fgets(opt.password, sizeof opt.password, f)) { fprintf(stderr, "rustica-install: cannot read %s\n", argv[i]); return 2; }
            fclose(f);
            opt.password[strcspn(opt.password, "\r\n")] = 0;
        } else if (!strcmp(a, "--lock-root")) opt.lock_root = 1;
        else if (!strcmp(a, "--yes")) opt.yes = 1;
        else if (!strcmp(a, "--dry-run")) opt.dry_run = 1;
        else if (!strcmp(a, "--reboot")) opt.reboot = 1;
        else if (!strcmp(a, "--no-reboot")) opt.reboot = 0;
        else if (!strcmp(a, "-h") || !strcmp(a, "--help")) return usage(0);
        else return usage(2);
    }
    if (geteuid() != 0) {
        fprintf(stderr, "rustica-install: run it as root (sudo rustica-install)\n");
        return 1;
    }
    opt.interactive = !opt.yes;
    atexit(after);
    mkdir("/var/log", 0755);
    logf = fopen(LOG_FILE, "a");
    chmod(LOG_FILE, 0600);
    printf("\n  Rustica Server installer%s\n\n", opt.dry_run ? " (dry run: nothing is written)" : "");
    logmsg("rustica-install started%s", opt.dry_run ? " (dry run)" : "");
    if (opt.interactive) printf("Language: English (the only one for now)\n");
    choose_settings();
    choose();
    summary();
    if (opt.dry_run) {
        say("Dry run: nothing was written.");
        return 0;
    }
    if (opt.interactive) {
        char a[32];
        ask("Type install to write these changes (anything else stops)", "", a, sizeof a);
        if (strcmp(a, "install")) {
            say("Stopped: nothing was written.");
            return 1;
        }
        if (plan.nvme && (plan.d->info.flags & RX_BLOCK_FLAG_READONLY)) {
            printf("%s is the computer's internal disk, which Rustica does not write unless you say so here.\n", plan.d->info.name);
            ask("Type its name to allow writing to it", "", a, sizeof a);
            if (strcmp(a, plan.d->info.name)) {
                say("Stopped: nothing was written.");
                return 1;
            }
        }
    } else {
        say("--yes: installing as shown.");
    }
    // In order: what can be undone least goes last
    unlock_nvme();
    // Partitions of this disk must not be in use (mounted) for the new table
    write_table();
    make_filesystems();
    install_root();
    install_esp();
    finish();     // the root and the EFI files are on the disk
    boot_entry(); // and only now does the firmware start them
    say("Rustica Server is installed on %s.", plan.d->info.name);
    printf("\nRemove the installation medium and restart.\n");
    int reboot = opt.reboot;
    if (reboot < 0) reboot = opt.interactive ? ask_yes("Restart now?", 1) : 0;
    fclose(logf);
    logf = 0;
    if (reboot) {
        restart = 1;
        RUN("/bin/reboot");
    }
    return 0;
}

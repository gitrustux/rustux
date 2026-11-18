// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// bootctl: the boot manager (rboot) from the running system (step 19),
// through systemd's boot loader variables, which rboot speaks.
//
//   bootctl [status]                 the boot manager, the entries' defaults, the firmware's order
//   bootctl list                     the menu's entries (as rboot found them at this boot)
//   bootctl set-default ID           the entry rboot starts (LoaderEntryDefault; "" removes it)
//   bootctl set-oneshot ID           the entry for the next boot only
//   bootctl set-timeout SECONDS      overrides loader.conf ("" removes it)
//   bootctl reboot-to-firmware [yes|no]   the next boot goes into the firmware setup
//   bootctl is-installed             rboot is on the EFI partition
//   bootctl bless                    this boot is good: the entry's boot counter goes
//                                    (package step E4: ID+LEFT-DONE.conf becomes ID.conf)
//   --esp-path=DIR                   the EFI partition's mount point (default /boot/efi)
//
// Changing needs root.

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "efivar.h"

static const char *esp = "/boot/efi";

static int fail(const char *what, int r) {
    fprintf(stderr, "bootctl: %s: %s\n", what, ev_strerror(r));
    return 1;
}

// key value from a .conf file ("" if not there)
static void conf_value(const char *path, const char *key, char *out, size_t cap) {
    out[0] = 0;
    FILE *f = fopen(path, "r");
    if (!f) return;
    char line[512];
    size_t kl = strlen(key);
    while (fgets(line, sizeof line, f)) {
        char *s = line;
        while (*s == ' ' || *s == '\t') s++;
        if (strncmp(s, key, kl) || (s[kl] != ' ' && s[kl] != '\t')) continue;
        s += kl;
        while (*s == ' ' || *s == '\t') s++;
        s[strcspn(s, "\r\n")] = 0;
        snprintf(out, cap, "%s", s);
        break;
    }
    fclose(f);
}

// "ID+LEFT[-DONE]": the length of ID (the whole if there is no counter);
// *left and *done set when there is one
static size_t counter_split(const char *stem, size_t len, int *left, int *done) {
    for (size_t k = len; k-- > 0;) {
        if (stem[k] != '+') continue;
        int l = 0, d = 0, nl = 0, nd = 0, dash = 0;
        size_t i;
        for (i = k + 1; i < len; i++) {
            char c = stem[i];
            if (c >= '0' && c <= '9') {
                if (dash) d = d * 10 + (c - '0'), nd++;
                else l = l * 10 + (c - '0'), nl++;
            } else if (c == '-' && !dash && nl) dash = 1;
            else break;
        }
        if (i == len && nl && (!dash || nd)) {
            if (left) *left = l;
            if (done) *done = d;
            return k;
        }
        break;
    }
    if (left) *left = -1;
    return len;
}

// The .conf file of entry id (ID.conf or ID+LEFT-DONE.conf) into path
static int entry_file(const char *id, char *path, size_t cap, int *left, int *done) {
    char dir[300];
    snprintf(dir, sizeof dir, "%s/loader/entries", esp);
    snprintf(path, cap, "%s/%s.conf", dir, id);
    struct stat st;
    if (left) *left = -1;
    if (stat(path, &st) == 0) return 0;
    DIR *d = opendir(dir);
    if (!d) return -1;
    struct dirent *e;
    int found = -1;
    while ((e = readdir(d))) {
        size_t l = strlen(e->d_name);
        if (l <= 5 || strcmp(e->d_name + l - 5, ".conf")) continue;
        int lf, dn;
        size_t k = counter_split(e->d_name, l - 5, &lf, &dn);
        if (k != l - 5 && k == strlen(id) && !strncmp(e->d_name, id, k)) {
            snprintf(path, cap, "%s/%s", dir, e->d_name);
            if (left) *left = lf;
            if (done) *done = dn;
            found = 0;
            break;
        }
    }
    closedir(d);
    return found;
}

// The menu's entry ids: LoaderEntries from rboot, else the .conf files
static int entry_ids(char ids[][96], int max, int *from_loader) {
    uint16_t buf[4096];
    int n = ev_get(EFI_LOADER_GUID, "LoaderEntries", buf, sizeof buf, 0), k = 0;
    *from_loader = n > 0;
    if (n > 0) {
        size_t units = (size_t)n / 2, i = 0;
        while (i < units && k < max) {
            size_t j = i;
            while (j < units && buf[j]) j++;
            if (j > i) ev_utf8(buf + i, j - i, ids[k++], 96);
            i = j + 1;
        }
        return k;
    }
    char dir[300];
    snprintf(dir, sizeof dir, "%s/loader/entries", esp);
    DIR *d = opendir(dir);
    if (!d) return 0;
    struct dirent *e;
    while ((e = readdir(d)) && k < max) {
        size_t l = strlen(e->d_name);
        if (l > 5 && !strcmp(e->d_name + l - 5, ".conf")) snprintf(ids[k++], 96, "%.*s", (int)counter_split(e->d_name, l - 5, 0, 0), e->d_name);
    }
    closedir(d);
    return k;
}

static void title_of(const char *id, char *out, size_t cap) {
    char path[400], base[96];
    snprintf(base, sizeof base, "%s", id);
    const char *suffix = "";
    size_t l = strlen(base);
    if (l > 9 && !strcmp(base + l - 9, "-recovery")) base[l - 9] = 0, suffix = " (recovery)";
    else if (l > 5 && !strcmp(base + l - 5, "-live")) base[l - 5] = 0, suffix = " (live, in memory)";
    int left = -1;
    entry_file(base, path, sizeof path, &left, 0);
    if (left == 0) suffix = !strcmp(suffix, "") ? " (failed to start)" : suffix;
    char t[200];
    conf_value(path, "title", t, sizeof t);
    if (!t[0]) {
        if (!strcmp(base, "rustica")) snprintf(t, sizeof t, "Rustica");
        else if (!strncmp(base, "auto-windows", 12)) snprintf(t, sizeof t, "Windows Boot Manager");
        else if (!strcmp(base, "auto-reboot-to-firmware-setup")) snprintf(t, sizeof t, "Firmware setup");
        else if (!strncmp(base, "auto-", 5)) {
            snprintf(t, sizeof t, "Linux (%s)", base + 5);
            if (t[7] >= 'a' && t[7] <= 'z') t[7] = (char)(t[7] - 32); // as rboot shows it
        }
        else snprintf(t, sizeof t, "%s", base);
    }
    snprintf(out, cap, "%s%s", t, suffix);
}

static void default_entry(char *out, size_t cap, int *from_var) {
    *from_var = ev_get_str(EFI_LOADER_GUID, "LoaderEntryDefault", out, cap) == 0 && out[0];
    if (*from_var) return;
    char path[300];
    snprintf(path, sizeof path, "%s/loader/loader.conf", esp);
    conf_value(path, "default", out, cap);
}

static int cmd_list(void) {
    char ids[64][96], def[96], sel[96] = "", title[256];
    int from_loader, from_var;
    int n = entry_ids(ids, 64, &from_loader);
    default_entry(def, sizeof def, &from_var);
    ev_get_str(EFI_LOADER_GUID, "LoaderEntrySelected", sel, sizeof sel);
    if (!n) {
        printf("No boot entries (the boot manager did not start this system, and there is no %s/loader/entries).\n", esp);
        return 1;
    }
    if (!from_loader) printf("(from %s/loader/entries: this boot did not go through rboot)\n", esp);
    for (int i = 0; i < n; i++) {
        title_of(ids[i], title, sizeof title);
        int is_def = def[0] && (!strcmp(def, ids[i]) || (def[strlen(def) - 1] == '*' && !strncmp(def, ids[i], strlen(def) - 1)));
        printf("%-28s %s%s%s\n", ids[i], title, is_def ? "  (default)" : "", !strcmp(sel, ids[i]) ? "  (selected)" : "");
    }
    return 0;
}

static int cmd_status(void) {
    char info[128] = "", sel[96] = "", def[96], timeout[32] = "", part[64] = "";
    int from_var;
    int r = ev_get_str(EFI_LOADER_GUID, "LoaderInfo", info, sizeof info);
    if (r == -ENOSYS) {
        printf("No UEFI runtime services: %s\n", ev_strerror(r));
        return 1;
    }
    ev_get_str(EFI_LOADER_GUID, "LoaderEntrySelected", sel, sizeof sel);
    ev_get_str(EFI_LOADER_GUID, "LoaderDevicePartUUID", part, sizeof part);
    default_entry(def, sizeof def, &from_var);
    if (ev_get_str(EFI_LOADER_GUID, "LoaderConfigTimeout", timeout, sizeof timeout) < 0 || !timeout[0]) {
        char path[300];
        snprintf(path, sizeof path, "%s/loader/loader.conf", esp);
        conf_value(path, "timeout", timeout, sizeof timeout);
    }
    printf("Boot manager:\n");
    printf("      Product: %s\n", info[0] ? info : "(not rboot: this boot did not go through it)");
    if (part[0]) printf("    Partition: %s\n", part);
    printf("Current entry: %s\n", sel[0] ? sel : "-");
    char cp[300] = "";
    if (ev_get_str(EFI_LOADER_GUID, "LoaderBootCountPath", cp, sizeof cp) == 0 && cp[0]) {
        const char *n = strrchr(cp, '\\');
        n = n ? n + 1 : cp;
        char path[400];
        struct stat st;
        snprintf(path, sizeof path, "%s/loader/entries/%s", esp, n);
        printf(" Boot counted: %s (%s)\n", n, stat(path, &st) == 0 ? "not yet blessed: bootctl bless" : "blessed");
    }
    printf("Default entry: %s%s\n", def[0] ? def : "(the first)", from_var ? " (set with bootctl set-default)" : "");
    printf("      Timeout: %s\n", timeout[0] ? timeout : "5 (default)");
    struct stat st;
    char p[300];
    snprintf(p, sizeof p, "%s/EFI/Rustica/rboot.efi", esp);
    printf("    Installed: %s\n", stat(p, &st) == 0 ? p : "no rboot on the EFI partition (or it is not mounted)");
    // The firmware's side
    uint8_t b[4096];
    uint16_t order[64], cur = 0xFFFF;
    if (ev_get(EFI_GLOBAL_GUID, "BootCurrent", b, sizeof b, 0) >= 2) cur = rd16(b);
    int n = ev_boot_order(order, 64);
    printf("\nFirmware boot order:\n");
    for (int i = 0; i < n; i++) {
        char name[16];
        snprintf(name, sizeof name, "Boot%04X", order[i]);
        int l = ev_get(EFI_GLOBAL_GUID, name, b, sizeof b, 0);
        load_option_t lo;
        if (l < 0 || lo_parse(b, (size_t)l, &lo) < 0) continue;
        printf("  %s %s%s\n", name, lo.desc, order[i] == cur ? "  (this boot)" : "");
    }
    return 0;
}

static int set_entry_var(const char *var, const char *id) {
    int r;
    if (!*id) {
        r = ev_delete(EFI_LOADER_GUID, var);
        return r < 0 ? fail(var, r) : 0;
    }
    char ids[64][96];
    int from_loader, n = entry_ids(ids, 64, &from_loader), ok = !n;
    for (int i = 0; i < n; i++) if (!strcmp(ids[i], id)) ok = 1;
    if (!ok && !strchr(id, '*')) {
        fprintf(stderr, "bootctl: no entry %s (bootctl list shows them)\n", id);
        return 1;
    }
    r = ev_set_str(EFI_LOADER_GUID, var, EV_DEFAULT, id);
    return r < 0 ? fail(var, r) : 0;
}

// This boot reached the multi-user target: the entry rboot counted
// (LoaderBootCountPath) loses its counter, so it is good from now on.
// The boot-complete unit runs it.
static int cmd_bless(void) {
    char cp[300] = "";
    if (ev_get_str(EFI_LOADER_GUID, "LoaderBootCountPath", cp, sizeof cp) < 0 || !cp[0]) {
        printf("bootctl: this boot was not counted: nothing to do\n");
        return 0;
    }
    // \loader\entries\ID+LEFT-DONE.conf
    const char *name = strrchr(cp, '\\');
    name = name ? name + 1 : cp;
    size_t l = strlen(name);
    if (l <= 5 || strcmp(name + l - 5, ".conf")) {
        fprintf(stderr, "bootctl: LoaderBootCountPath is not an entry: %s\n", cp);
        return 1;
    }
    size_t k = counter_split(name, l - 5, 0, 0);
    char from[400], to[400];
    snprintf(from, sizeof from, "%s/loader/entries/%s", esp, name);
    snprintf(to, sizeof to, "%s/loader/entries/%.*s.conf", esp, (int)k, name);
    struct stat st;
    if (k == l - 5 || stat(from, &st) != 0) {
        printf("bootctl: %s: no longer counted (good already, or written again)\n", name);
        return 0;
    }
    if (rename(from, to) != 0) {
        perror(to);
        return 1;
    }
    sync();
    printf("bootctl: this boot is good: %s is now %.*s.conf\n", name, (int)k, name);
    return 0;
}

static int cmd_firmware(const char *arg) {
    uint8_t b[16];
    int n = ev_get(EFI_GLOBAL_GUID, "OsIndicationsSupported", b, sizeof b, 0);
    if (n < 8 || !(b[0] & 1)) {
        printf("The firmware cannot be asked to start its setup.\n");
        return 1;
    }
    uint64_t v = 0;
    if ((n = ev_get(EFI_GLOBAL_GUID, "OsIndications", b, sizeof b, 0)) >= 8) memcpy(&v, b, 8);
    if (!arg) {
        printf("%s\n", v & 1 ? "yes" : "no");
        return 0;
    }
    if (!strcmp(arg, "yes") || !strcmp(arg, "true") || !strcmp(arg, "1")) v |= 1;
    else v &= ~1ull;
    int r = ev_set(EFI_GLOBAL_GUID, "OsIndications", EV_DEFAULT, &v, 8);
    return r < 0 ? fail("OsIndications", r) : 0;
}

static int usage(int code) {
    fprintf(code ? stderr : stdout,
            "usage: bootctl [status] | list | set-default ID | set-oneshot ID | set-timeout SECONDS\n"
            "               | set-timeout-oneshot SECONDS | reboot-to-firmware [yes|no] | is-installed | bless\n"
            "               [--esp-path=DIR]\n"
            "The boot manager (rboot) and its entries; see man bootctl. Changes need root.\n");
    return code;
}

int main(int argc, char **argv) {
    char *args[8];
    int na = 0;
    for (int i = 1; i < argc; i++) {
        if (!strncmp(argv[i], "--esp-path=", 11)) esp = argv[i] + 11;
        else if (!strcmp(argv[i], "--no-pager") || !strcmp(argv[i], "-q")) {}
        else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) return usage(0);
        else if (na < 8) args[na++] = argv[i];
    }
    const char *cmd = na ? args[0] : "status";
    if (!strcmp(cmd, "status")) return cmd_status();
    if (!strcmp(cmd, "list")) return cmd_list();
    if (!strcmp(cmd, "set-default") && na == 2) return set_entry_var("LoaderEntryDefault", args[1]);
    if (!strcmp(cmd, "set-oneshot") && na == 2) return set_entry_var("LoaderEntryOneShot", args[1]);
    if ((!strcmp(cmd, "set-timeout") || !strcmp(cmd, "set-timeout-oneshot")) && na == 2) {
        const char *var = cmd[11] ? "LoaderConfigTimeoutOneShot" : "LoaderConfigTimeout";
        int r = args[1][0] ? ev_set_str(EFI_LOADER_GUID, var, EV_DEFAULT, args[1]) : ev_delete(EFI_LOADER_GUID, var);
        return r < 0 ? fail(var, r) : 0;
    }
    if (!strcmp(cmd, "bless")) return cmd_bless();
    if (!strcmp(cmd, "reboot-to-firmware")) return cmd_firmware(na > 1 ? args[1] : 0);
    if (!strcmp(cmd, "is-installed")) {
        char p[300];
        struct stat st;
        snprintf(p, sizeof p, "%s/EFI/Rustica/rboot.efi", esp);
        int yes = stat(p, &st) == 0;
        printf("%s\n", yes ? "yes" : "no");
        return !yes;
    }
    return usage(2);
}

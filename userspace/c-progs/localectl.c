// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// localectl: the keyboard layout of the console (step 17), as systemd's.
//
//   localectl [status]            the layout (/etc/keymap) and the language
//   localectl list-keymaps        us and those in /usr/share/keymaps
//   localectl set-keymap NAME     /etc/keymap, and the console uses it now
//
// Changes are root's (sudo localectl set-keymap de).

#include <dirent.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "sysreq.h"

static void current(char *buf, size_t cap) {
    FILE *f = fopen("/etc/keymap", "r");
    buf[0] = 0;
    if (f) {
        if (!fgets(buf, (int)cap, f)) buf[0] = 0;
        fclose(f);
    }
    buf[strcspn(buf, " \t\r\n")] = 0;
    if (!buf[0]) snprintf(buf, cap, "us");
}

static int known(const char *name) {
    if (!strcmp(name, "us")) return 1;
    if (strchr(name, '/') || name[0] == '.') return 0;
    char path[160];
    snprintf(path, sizeof path, "/usr/share/keymaps/%s", name);
    return access(path, R_OK) == 0;
}

int main(int argc, char **argv) {
    char km[64];
    if (argc == 1 || !strcmp(argv[1], "status")) {
        current(km, sizeof km);
        printf("   System Locale: LANG=C.UTF-8\n       VC Keymap: %s\n", km);
        return 0;
    }
    if (!strcmp(argv[1], "list-keymaps")) {
        puts("us");
        DIR *d = opendir("/usr/share/keymaps");
        struct dirent *e;
        while (d && (e = readdir(d)))
            if (e->d_name[0] != '.') puts(e->d_name);
        if (d) closedir(d);
        return 0;
    }
    if (!strcmp(argv[1], "set-keymap") && argc == 3) {
        if (!known(argv[2])) {
            fprintf(stderr, "localectl: no keymap %s (see localectl list-keymaps)\n", argv[2]);
            return 1;
        }
        FILE *f = fopen("/etc/keymap", "w");
        if (!f) {
            fprintf(stderr, "localectl: /etc/keymap: %s%s\n", strerror(errno), errno == EACCES ? " (try sudo)" : "");
            return 1;
        }
        fprintf(f, "%s\n", argv[2]);
        fclose(f);
        int r = sysreq(RX_SYSTEM_OP_RELOAD_KEYMAP, 0, 0);
        if (r < 0) fprintf(stderr, "localectl: the console will use it after a reboot (%s)\n", strerror(-r));
        return 0;
    }
    fprintf(stderr, "usage: localectl [status | list-keymaps | set-keymap NAME]\n");
    return 2;
}

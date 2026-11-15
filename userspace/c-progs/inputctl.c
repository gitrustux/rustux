// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// inputctl: the input service's settings (DESKTOP.md step F7), in
// /etc/input.conf; Settings uses it.
//
//   inputctl [status]                  the settings
//   inputctl set-pointer-speed PERCENT relative mouse motion scaled by
//                                      PERCENT (25-400, 100 as it comes)
//
// Changes are root's (sudo inputctl ...); the input service takes them
// at once.

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "sysreq.h"

#define CONF "/etc/input.conf"

static int speed(void) {
    FILE *f = fopen(CONF, "r");
    int s = 100;
    char line[128];
    if (!f) return s;
    while (fgets(line, sizeof line, f)) {
        const char *p = line;
        while (*p == ' ' || *p == '\t') p++;
        if (strncmp(p, "pointer_speed", 13)) continue;
        p += 13;
        while (*p == ' ' || *p == '=') p++;
        int v = atoi(p);
        if (v >= 25 && v <= 400) s = v;
    }
    fclose(f);
    return s;
}

int main(int argc, char **argv) {
    if (argc == 1 || !strcmp(argv[1], "status")) {
        printf("Pointer speed: %d%%\n", speed());
        return 0;
    }
    if (!strcmp(argv[1], "set-pointer-speed") && argc == 3) {
        char *end;
        long v = strtol(argv[2], &end, 10);
        if (*end == '%') end++;
        if (*end || v < 25 || v > 400) {
            fprintf(stderr, "inputctl: the speed is a percentage from 25 to 400\n");
            return 1;
        }
        FILE *f = fopen(CONF, "w");
        if (!f) {
            fprintf(stderr, "inputctl: %s: %s%s\n", CONF, strerror(errno), errno == EACCES ? " (try sudo)" : "");
            return 1;
        }
        fprintf(f, "# The input service's settings (inputctl)\npointer_speed=%ld\n", v);
        fclose(f);
        // The input service reads it with the keyboard layout
        int r = sysreq(RX_SYSTEM_OP_RELOAD_KEYMAP, 0, 0);
        if (r < 0) fprintf(stderr, "inputctl: it applies after a reboot (%s)\n", strerror(-r));
        return 0;
    }
    fprintf(stderr, "usage: inputctl [status | set-pointer-speed PERCENT]\n");
    return 2;
}

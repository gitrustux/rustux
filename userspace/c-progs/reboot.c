// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// reboot: restart the computer (root, or the desktop session's user; others: sudo reboot). init (/svc/system)
// ends the sessions, writes every disk out and marks it clean, then does
// it (step 17; before that the shell held a power resource).

#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <errno.h>
#include "sysreq.h"

int main(int argc, char **argv) {
    if (argc > 1) {
        printf("Usage: reboot\nTo restart the computer (as root: sudo reboot).\n");
        return strcmp(argv[1], "--help") ? 2 : 0;
    }
    // init decides: root, or the graphical session's user
    if (geteuid() == 0) {
        printf("reboot: writing out the disks...\n");
        fflush(stdout);
    }
    int r = sysreq(RX_SYSTEM_OP_REBOOT, 0, 0);
    if (r == -EPERM)
        fprintf(stderr, "reboot: only root or the desktop session's user may (try: sudo reboot)\n");
    else
        fprintf(stderr, "reboot: failed: %s\n", strerror(-r));
    return 1;
}

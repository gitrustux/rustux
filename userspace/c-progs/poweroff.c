// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// poweroff: turn the computer off (root only; others: sudo poweroff). init (/svc/system)
// ends the sessions, writes every disk out and marks it clean, then does
// it (step 17; before that the shell held a power resource).

#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "sysreq.h"

int main(int argc, char **argv) {
    if (argc > 1) {
        printf("Usage: poweroff\nTo turn the computer off (as root: sudo poweroff).\n");
        return strcmp(argv[1], "--help") ? 2 : 0;
    }
    if (geteuid() != 0) {
        fprintf(stderr, "poweroff: must be root (try: sudo poweroff)\n");
        return 1;
    }
    printf("poweroff: writing out the disks...\n");
    fflush(stdout);
    int r = sysreq(RX_SYSTEM_OP_POWEROFF, 0, 0);
    fprintf(stderr, "poweroff: failed: %s\n", strerror(-r));
    return 1;
}

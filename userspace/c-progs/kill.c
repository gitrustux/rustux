// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// kill PID...: end processes (PIDs as shown by ps). Rustux has no
// signals: a signal option such as -9 is accepted and ignored; the process
// always ends at once with exit code -1024.

#include "tasks.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct find { uint64_t koid; int found; rx_status_t r; };

static void match(rx_handle_t p, uint64_t koid, uint64_t job, void *ctx) {
    (void)job;
    struct find *f = ctx;
    if (koid != f->koid || f->found) return;
    f->found = 1;
    f->r = rx_task_kill(p);
}

int main(int argc, char **argv) {
    int status = 0, any = 0;
    uint64_t self = rx_koid_of(rx_proc_self);
    if (argc == 2 && !strcmp(argv[1], "--help")) {
        printf("Usage: kill PID...\nEnd processes (PIDs as ps shows them). Signal options (-9, -TERM) are\n"
               "accepted and ignored: Rustux has no signals, a process always ends at once.\n");
        return 0;
    }
    for (int i = 1; i < argc; i++) {
        if (argv[i][0] == '-') continue; // -9, -TERM, ...: no signals
        char *end;
        struct find f = { strtoull(argv[i], &end, 10), 0, 0 };
        any = 1;
        if (*end || !f.koid) {
            fprintf(stderr, "kill: %s: not a process id\n", argv[i]);
            status = 1;
            continue;
        }
        if (f.koid == self) {
            fprintf(stderr, "kill: %s: that is kill itself\n", argv[i]);
            status = 1;
            continue;
        }
        rx_walk_job(rx_system_job(), 0, match, &f);
        if (!f.found) {
            fprintf(stderr, "kill: %s: no such process\n", argv[i]);
            status = 1;
        } else if (f.r != RX_OK) {
            fprintf(stderr, "kill: %s: error %lld\n", argv[i], (long long)f.r);
            status = 1;
        }
    }
    if (!any) {
        fprintf(stderr, "usage: kill PID...\n");
        return 2;
    }
    return status;
}

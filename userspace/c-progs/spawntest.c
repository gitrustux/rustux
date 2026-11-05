// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// spawntest: start /bin/true 300 times, one after another, waiting for
// each. Checks that exited processes are cleaned up (thread ids are reused
// past the 255-entry table, memory is given back).

#include "rx.h"

#define RUNS 300

// Programs are loaded by the C library (from /bin through the filesystem
// service) since step 8
static inline int64_t spawn_process(const char *path, rx_handle_t *proc) {
    return rx_spawn(path, 0, 0, RX_SPAWN_CLONE_NS, 0, 0, proc);
}

int main(void) {
    int ok = 0;
    int64_t first_err = 0;
    for (int i = 0; i < RUNS; i++) {
        rx_handle_t proc = 0;
        rx_info_process_t info;
        int64_t r = spawn_process("/bin/true", &proc);
        if (r < 0) {
            if (!first_err) first_err = r;
            continue;
        }
        rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
        rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
        rx_handle_close(proc);
        if (info.return_code == 0) ok++;
    }
    rx_puts("spawntest: ");
    rx_putd(ok);
    rx_puts(" of ");
    rx_putd(RUNS);
    rx_puts(" processes ran and exited");
    if (first_err) {
        rx_puts("; first error ");
        rx_putd(first_err);
    }
    rx_puts("\n");
    rx_exit(ok == RUNS ? 0 : 1);
}

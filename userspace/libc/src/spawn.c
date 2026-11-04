// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Starting programs. The kernel loads the ELF from the ramdisk for now;
// step 8 moves loading into this library (programs read from files).

#include "libc.h"
#include <errno.h>
#include <stdlib.h>
#include <string.h>

#define MAX_SPAWN_HANDLES 16

// Pack strings into one NUL-separated block
static char *pack(char *const v[], uint32_t *len, uint32_t *count) {
    size_t total = 0;
    uint32_t n = 0;
    for (; v && v[n]; n++) total += strlen(v[n]) + 1;
    char *b = malloc(total ? total : 1);
    if (!b) return 0;
    char *p = b;
    for (uint32_t i = 0; i < n; i++) {
        size_t k = strlen(v[i]) + 1;
        memcpy(p, v[i], k);
        p += k;
    }
    *len = (uint32_t)total;
    *count = n;
    return b;
}

rx_status_t rx_spawn(const char *path, char *const argv[], char *const envp[], uint32_t flags,
                     const rx_spawn_handle_t *handles, uint32_t nhandles, rx_handle_t *proc_out) {
    rx_handle_t hs[MAX_SPAWN_HANDLES];
    uint32_t info[MAX_SPAWN_HANDLES];
    uint32_t n = 0;
    rx_status_t r = RX_OK;

    if (flags & RX_SPAWN_CLONE_STDIO) {
        for (int fd = 0; fd < 3; fd++) {
            if (__fds[fd].kind != RX_FD_CONSOLE) continue; // only console fds can be passed so far
            rx_handle_t dup;
            if (rx_handle_duplicate(__fds[fd].handle, RX_RIGHT_SAME_RIGHTS, &dup) == RX_OK) {
                hs[n] = dup;
                info[n++] = RX_PA_HND(RX_PA_FD, fd);
            }
        }
    }
    for (uint32_t i = 0; i < nhandles; i++) {
        if (n < MAX_SPAWN_HANDLES) {
            hs[n] = handles[i].handle;
            info[n++] = handles[i].info;
        } else {
            rx_handle_close(handles[i].handle);
            r = RX_ERR_OUT_OF_RANGE;
        }
    }

    char *const default_argv[] = { (char *)path, 0 };
    rx_spawn_strings_t s;
    char *a = pack(argv ? argv : default_argv, &s.args_len, &s.argc);
    char *e = pack(envp ? envp : environ, &s.env_len, &s.envc);
    if (!a || !e) r = RX_ERR_NO_MEMORY;
    if (r != RX_OK) {
        for (uint32_t i = 0; i < n; i++) rx_handle_close(hs[i]);
        free(a);
        free(e);
        errno = rx_status_to_errno(r);
        return r;
    }
    s.args = a;
    s.env = e;
    rx_handle_t proc = 0;
    int64_t t = rx_spawn_process(path, &proc, hs, info, n, &s);
    free(a);
    free(e);
    if (t < 0) {
        errno = rx_status_to_errno(t);
        return t;
    }
    if (proc_out) *proc_out = proc; else rx_handle_close(proc);
    return RX_OK;
}

int64_t rx_wait_process(rx_handle_t proc) {
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
    rx_object_get_info(proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_handle_close(proc);
    return info.return_code;
}

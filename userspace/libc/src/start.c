// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Startup: read the bootstrap message, set up handles, file descriptors,
// arguments, environment and stdio, then run main().

#include "libc.h"
#include <rustux/processargs.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv, char **envp);

rx_handle_t rx_proc_self, rx_thread_self, rx_job_default, rx_vmar_root;

#define MAX_BOOT_HANDLES 64
static uint32_t boot_info[MAX_BOOT_HANDLES];
static rx_handle_t boot_handles[MAX_BOOT_HANDLES];
static uint32_t boot_count;
// The message itself: argv and environ point into it
static char boot_msg[65536] __attribute__((aligned(8)));

rx_handle_t rx_take_startup_handle(uint32_t info) {
    for (uint32_t i = 0; i < boot_count; i++) {
        if (boot_handles[i] && boot_info[i] == info) {
            rx_handle_t h = boot_handles[i];
            boot_handles[i] = 0;
            return h;
        }
    }
    return 0;
}

// Pointers to `n` NUL-terminated strings starting at `p` (NULL-terminated
// array; `end` bounds the message)
static char **split(char *p, char *end, uint32_t n) {
    char **v = malloc((n + 1) * sizeof *v);
    if (!v) rx_exit(-1);
    for (uint32_t i = 0; i < n; i++) {
        v[i] = p;
        while (p < end && *p) p++;
        if (p >= end) { v[i] = ""; continue; }
        p++;
    }
    v[n] = 0;
    return v;
}

void __libc_start(rx_handle_t bootstrap) {
    uint32_t nb = 0, nh = 0;
    rx_status_t r = rx_channel_read(bootstrap, 0, boot_msg, boot_handles, sizeof boot_msg - 1,
                                    MAX_BOOT_HANDLES, &nb, &nh);
    rx_handle_close(bootstrap);
    rx_proc_args_t *pa = (rx_proc_args_t *)boot_msg;
    if (r != RX_OK || nb < sizeof *pa || pa->protocol != RX_PROCARGS_PROTOCOL ||
        pa->handle_info_off + 4ull * nh > nb || pa->args_off > nb || pa->environ_off > nb) {
        rx_exit(-2); // no usable bootstrap message
    }
    boot_msg[nb] = 0;
    boot_count = nh;
    memcpy(boot_info, boot_msg + pa->handle_info_off, 4 * nh);

    rx_proc_self = rx_take_startup_handle(RX_PA_PROC_SELF);
    rx_thread_self = rx_take_startup_handle(RX_PA_THREAD_SELF);
    rx_job_default = rx_take_startup_handle(RX_PA_JOB_DEFAULT);
    rx_vmar_root = rx_take_startup_handle(RX_PA_VMAR_ROOT);

    // File descriptors: the argument is the fd number and, in bits 12-15,
    // its kind (0 = console, as before step 8)
    for (uint32_t i = 0; i < boot_count; i++) {
        if (boot_handles[i] && (boot_info[i] & 0xFF) == RX_PA_FD) {
            uint32_t arg = boot_info[i] >> 16;
            int fd = (int)(arg & 0xFFF), kind = (int)(arg >> 12);
            if (fd < MAX_FDS && __fds[fd].kind == RX_FD_NONE) {
                __fds[fd].kind = kind ? kind : RX_FD_CONSOLE;
                __fds[fd].handle = boot_handles[i];
                boot_handles[i] = 0;
            }
        }
    }
    rx_ns_set_root(rx_take_startup_handle(RX_PA_NS_DIR));

    __malloc_init();
    char *end = boot_msg + nb;
    char **argv = split(boot_msg + pa->args_off, end, pa->args_num);
    char **envp = split(boot_msg + pa->environ_off, end, pa->environ_num);
    __env_init(envp);
    __fs_init();
    __stdio_init();
    exit(main((int)pa->args_num, argv, environ));
}

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Init Process (PID 1)
//!
//! The first userspace process. It starts the PS/2 keyboard driver and the
//! shell, and is the hub for keyboard input: characters arrive from the
//! driver and from the kernel (USB keyboards) over channels, and init
//! forwards them to the shell over the shell's channel. If the driver or
//! the shell ends, init starts a new one. The shell can ask for a driver
//! restart by sending "restart-kbd".
//!
//! Build: gcc -static -nostdlib -ffreestanding -fno-builtin -fno-stack-protector
//!        -fno-pie -no-pie -O2 -o init.elf init.c

#include "../userspace/c-progs/rx.h"

static rx_handle_t resource;     // root resource, passed to the driver
static rx_handle_t kernel_input; // USB keystrokes from the kernel
static rx_handle_t drv_proc, drv_chan; // keyboard driver
static rx_handle_t sh_proc, sh_chan;   // shell
static int driver_failures;

static void start_driver(void) {
    rx_handle_t ours, theirs, res;
    if (!resource || driver_failures >= 5) return;
    rx_channel_create(0, &ours, &theirs);
    rx_handle_duplicate(resource, RX_RIGHT_SAME_RIGHTS, &res);
    rx_handle_t hs[2] = { res, theirs };
    uint32_t tags[2] = { RX_PA_ROOT_RESOURCE, RX_PA_USER0 };
    if (rx_spawn_process("/bin/ps2kbd", &drv_proc, hs, tags, 2) < 0) {
        rx_puts("init: cannot start /bin/ps2kbd\n");
        rx_handle_close(ours);
        drv_proc = 0;
        driver_failures = 5;
        return;
    }
    drv_chan = ours;
}

static void start_shell(void) {
    rx_handle_t ours, theirs;
    rx_channel_create(0, &ours, &theirs);
    uint32_t tag = RX_PA_USER0;
    if (rx_spawn_process("/bin/shell", &sh_proc, &theirs, &tag, 1) < 0) {
        rx_puts("init: failed to spawn shell!\n");
        rx_exit(1);
    }
    sh_chan = ours;
}

static void forward(rx_handle_t from) {
    char buf[256];
    uint32_t n = 0;
    while (rx_channel_read(from, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {
        if (sh_chan) rx_channel_write(sh_chan, 0, buf, n, 0, 0);
    }
}

static int same(const char *a, const char *b, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) if (a[i] != b[i]) return 0;
    return b[n] == 0;
}

// A request from the shell
static void command(void) {
    char buf[64];
    uint32_t n = 0;
    while (rx_channel_read(sh_chan, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {
        if (same(buf, "restart-kbd", n)) {
            if (drv_proc) {
                rx_puts("init: killing the keyboard driver\n");
                rx_task_kill(drv_proc);
            } else {
                rx_puts("init: no keyboard driver is running\n");
            }
        }
    }
}

static void driver_ended(void) {
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_get_info(drv_proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_handle_close(drv_proc);
    rx_handle_close(drv_chan);
    drv_proc = drv_chan = 0;
    if (info.return_code == 2) {
        rx_puts("init: no PS/2 controller; keyboard driver not needed\n");
        return;
    }
    rx_puts("init: keyboard driver exited (code ");
    rx_putd(info.return_code);
    rx_puts("); starting it again\n");
    if (info.return_code != RX_TASK_RETCODE_SYSCALL_KILL) driver_failures++;
    start_driver();
}

void _start(rx_handle_t bootstrap) {
    rx_startup(bootstrap);
    resource = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    kernel_input = rx_take_startup_handle(RX_PA_KERNEL_INPUT);
    rx_puts("Init process (PID 1) starting...\n");
    start_driver();
    rx_puts("Spawning shell...\n\n");
    start_shell();

    for (;;) {
        rx_wait_item_t items[5];
        int n = 0, i_drv = -1, i_kin = -1, i_sh = -1, i_dproc = -1, i_sproc = -1;
        if (drv_chan) { i_drv = n; items[n++] = (rx_wait_item_t){ drv_chan, RX_CHANNEL_READABLE, 0 }; }
        if (kernel_input) { i_kin = n; items[n++] = (rx_wait_item_t){ kernel_input, RX_CHANNEL_READABLE, 0 }; }
        i_sh = n; items[n++] = (rx_wait_item_t){ sh_chan, RX_CHANNEL_READABLE, 0 };
        if (drv_proc) { i_dproc = n; items[n++] = (rx_wait_item_t){ drv_proc, RX_PROCESS_TERMINATED, 0 }; }
        i_sproc = n; items[n++] = (rx_wait_item_t){ sh_proc, RX_PROCESS_TERMINATED, 0 };

        if (rx_object_wait_many(items, n, RX_TIME_INFINITE) != RX_OK) continue;

        if (i_drv >= 0 && (items[i_drv].pending & RX_CHANNEL_READABLE)) forward(drv_chan);
        if (i_kin >= 0 && (items[i_kin].pending & RX_CHANNEL_READABLE)) forward(kernel_input);
        if (items[i_sh].pending & RX_CHANNEL_READABLE) command();
        if (i_dproc >= 0 && (items[i_dproc].pending & RX_PROCESS_TERMINATED)) driver_ended();
        if (items[i_sproc].pending & RX_PROCESS_TERMINATED) {
            rx_handle_close(sh_proc);
            rx_handle_close(sh_chan);
            rx_puts("\nShell exited; starting a new one\n\n");
            start_shell();
        }
    }
}

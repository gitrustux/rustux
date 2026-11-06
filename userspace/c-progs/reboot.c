// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// reboot: restart the computer. Needs the power resource, which init gives the
// shell and the shell passes on to the programs it starts.

#include <rustux/process.h>
#include <sys/mount.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc > 1) {
        printf("Usage: reboot\nTo restart the computer (needs the power resource the shell passes on).\n");
        return strcmp(argv[1], "--help") ? 2 : 0;
    }
    rx_handle_t power = rx_take_startup_handle(RX_PA_POWER_RESOURCE);
    if (!power) {
        fprintf(stderr, "reboot: not allowed (no power resource)\n");
        return 1;
    }
    // Every mounted filesystem written out and marked clean (step 15)
    printf("reboot: writing out the disks...\n");
    fflush(stdout);
    sync();
    rx_fs_shutdown_all();
    fflush(stdout);
    usleep(200000); // let the console print it
    rx_status_t r = rx_system_powerctl(power, RX_SYSTEM_POWERCTL_REBOOT);
    fprintf(stderr, "reboot: failed (error %lld)\n", (long long)r);
    return 1;
}

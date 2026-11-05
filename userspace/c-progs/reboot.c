// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// reboot: restart the computer. Needs the power resource, which init gives the
// shell and the shell passes on to the programs it starts.

#include <rustux/process.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    rx_handle_t power = rx_take_startup_handle(RX_PA_POWER_RESOURCE);
    if (!power) {
        fprintf(stderr, "reboot: not allowed (no power resource)\n");
        return 1;
    }
    printf("reboot: syncing... (nothing on disk yet)\n");
    fflush(stdout);
    usleep(200000); // let the console print it
    rx_status_t r = rx_system_powerctl(power, RX_SYSTEM_POWERCTL_REBOOT);
    fprintf(stderr, "reboot: failed (error %lld)\n", (long long)r);
    return 1;
}

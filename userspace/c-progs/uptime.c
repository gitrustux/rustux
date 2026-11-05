// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// uptime: the time, how long the system has been running, CPUs, threads
// and how busy the CPUs have been since boot

#include <rustux/process.h>
#include <stdio.h>
#include <time.h>

int main(void) {
    rx_system_stats_t s;
    if (rx_system_get_stats(&s) != RX_OK) return 1;
    time_t now = time(0);
    struct tm tm;
    char clock[16];
    strftime(clock, sizeof clock, "%H:%M:%S", localtime_r(&now, &tm));
    unsigned long long secs = s.uptime / 1000000000ull;
    unsigned long long days = secs / 86400, h = secs / 3600 % 24, m = secs / 60 % 60;
    double busy = s.cpus && s.uptime ? 100.0 * (1.0 - (double)s.idle_time / ((double)s.uptime * (double)s.cpus)) : 0;
    long busy10 = (long)(busy * 10 + 0.5);
    printf(" %s up ", clock);
    if (days) printf("%llu day%s, ", days, days == 1 ? "" : "s");
    printf("%2llu:%02llu, %llu CPU%s, %llu threads, CPUs %ld.%ld%% busy since boot\n", h, m,
           (unsigned long long)s.cpus, s.cpus == 1 ? "" : "s", (unsigned long long)s.threads, busy10 / 10, busy10 % 10);
    return 0;
}

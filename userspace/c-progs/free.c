// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// free [-k|-m|-g|-h]: memory in use and free (KiB by default)

#include <rustux/process.h>
#include <stdio.h>
#include <string.h>

static void show(const char *label, unsigned long long bytes, int unit) {
    if (unit == 'h') {
        const char *s = "BKMGT";
        double v = (double)bytes;
        int i = 0;
        while (v >= 1024 && i < 4) { v /= 1024; i++; }
        long t = (long)(v * 10 + 0.5);
        char buf[32];
        snprintf(buf, sizeof buf, "%ld.%ld%c", t / 10, t % 10, s[i]);
        printf("%s%12s", label, buf);
        return;
    }
    int shift = unit == 'm' ? 20 : unit == 'g' ? 30 : 10;
    printf("%s%12llu", label, bytes >> shift);
}

int main(int argc, char **argv) {
    int unit = 'k';
    if (argc > 1) {
        if (strlen(argv[1]) == 2 && argv[1][0] == '-' && strchr("kmgh", argv[1][1])) unit = argv[1][1];
        else { fprintf(stderr, "usage: free [-k|-m|-g|-h]\n"); return 2; }
    }
    rx_system_stats_t s;
    if (rx_system_get_stats(&s) != RX_OK) return 1;
    printf("     %12s%12s%12s\n", "total", "used", "free");
    show("Mem: ", s.total_bytes, unit);
    show("", s.total_bytes - s.free_bytes, unit);
    show("", s.free_bytes, unit);
    printf("\n");
    return 0;
}

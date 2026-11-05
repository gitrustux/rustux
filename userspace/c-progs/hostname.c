// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// hostname [NAME]: print the host name, or set it (in /etc/hostname, kept
// until the next boot)

#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc > 2) { fprintf(stderr, "usage: hostname [NAME]\n"); return 2; }
    if (argc == 2) {
        FILE *f = fopen("/etc/hostname", "w");
        if (!f) { perror("hostname: /etc/hostname"); return 1; }
        fprintf(f, "%s\n", argv[1]);
        return fclose(f) ? 1 : 0;
    }
    char name[65];
    if (gethostname(name, sizeof name) < 0) return 1;
    puts(name);
    return 0;
}

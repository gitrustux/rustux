// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// cat: copy standard input to standard output until end of input (Ctrl+D
// on the console). Reading files comes with the filesystem in step 8;
// until then only "-" (standard input) is accepted as an argument.

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-")) {
            fprintf(stderr, "cat: %s: files can't be read yet (filesystem comes in step 8)\n", argv[i]);
            return 1;
        }
    }
    char buf[4096];
    for (;;) {
        ssize_t n = read(0, buf, sizeof buf);
        if (n == 0) return 0;
        if (n < 0) {
            fprintf(stderr, "cat: read error: %s\n", strerror(errno));
            return 1;
        }
        if (write(1, buf, (size_t)n) != n) return 1;
    }
}

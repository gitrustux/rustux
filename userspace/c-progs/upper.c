// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// upper: copy standard input to standard output in upper case (a test
// filter for pipes)

#include <ctype.h>
#include <unistd.h>

int main(void) {
    char buf[4096];
    ssize_t n;
    while ((n = read(0, buf, sizeof buf)) > 0) {
        for (ssize_t i = 0; i < n; i++) buf[i] = (char)toupper((unsigned char)buf[i]);
        if (write(1, buf, (size_t)n) != n) return 1;
    }
    return n < 0;
}

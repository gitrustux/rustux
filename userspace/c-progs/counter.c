// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// counter [N]: count from 1 to N (default 10), one number per line

#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    long n = argc > 1 ? strtol(argv[1], 0, 10) : 10;
    for (long i = 1; i <= n; i++) printf("%ld\n", i);
    return 0;
}

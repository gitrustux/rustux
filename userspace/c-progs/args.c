// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// args: print each argument in brackets (to check the shell's quoting)

#include <stdio.h>

int main(int argc, char **argv) {
    printf("%d argument%s:", argc - 1, argc == 2 ? "" : "s");
    for (int i = 1; i < argc; i++) printf(" [%s]", argv[i]);
    printf("\n");
    return 0;
}

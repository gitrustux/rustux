// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// hello: greet, and show the arguments and environment we were given

#include <stdio.h>

int main(int argc, char **argv, char **envp) {
    printf("Hello from userspace!\n");
    for (int i = 0; i < argc; i++) printf("argv[%d] = \"%s\"\n", i, argv[i]);
    int n = 0;
    while (envp[n]) n++;
    printf("%d environment variable%s\n", n, n == 1 ? "" : "s");
    return 0;
}

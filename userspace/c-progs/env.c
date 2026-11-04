// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// env [-i] [NAME=VALUE]... [COMMAND [ARG]...]: print the environment, or
// run COMMAND with a changed environment. -i starts from an empty one.

#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern char **environ;

int main(int argc, char **argv) {
    int i = 1;
    if (i < argc && !strcmp(argv[i], "-i")) {
        clearenv();
        i++;
    }
    for (; i < argc && strchr(argv[i], '=') && argv[i][0] != '='; i++) putenv(argv[i]);
    if (i == argc) {
        for (char **e = environ; *e; e++) puts(*e);
        return 0;
    }
    char path[256];
    snprintf(path, sizeof path, strchr(argv[i], '/') ? "%s" : "/bin/%s", argv[i]);
    rx_handle_t proc;
    if (rx_spawn(path, argv + i, environ, RX_SPAWN_CLONE_STDIO, 0, 0, &proc) != RX_OK) {
        fprintf(stderr, "env: %s: command not found\n", argv[i]);
        return 127;
    }
    return (int)rx_wait_process(proc);
}

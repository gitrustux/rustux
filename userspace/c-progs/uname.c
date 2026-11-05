// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// uname [-a] [-s] [-n] [-r] [-v] [-m] [-o]: system names

#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>

int main(int argc, char **argv) {
    int s = 0, n = 0, r = 0, v = 0, m = 0, o = 0;
    for (int i = 1; i < argc; i++) {
        if (argv[i][0] != '-') { fprintf(stderr, "usage: uname [-asnrvmo]\n"); return 2; }
        for (const char *p = argv[i] + 1; *p; p++) {
            switch (*p) {
            case 'a': s = n = r = v = m = o = 1; break;
            case 's': s = 1; break;
            case 'n': n = 1; break;
            case 'r': r = 1; break;
            case 'v': v = 1; break;
            case 'm': m = 1; break;
            case 'o': o = 1; break;
            default: fprintf(stderr, "uname: unknown option -%c\n", *p); return 2;
            }
        }
    }
    if (!(s | n | r | v | m | o)) s = 1;
    struct utsname u;
    uname(&u);
    const char *parts[6] = { s ? u.sysname : 0, n ? u.nodename : 0, r ? u.release : 0,
                             v ? u.version : 0, m ? u.machine : 0, o ? "Rustica" : 0 };
    int first = 1;
    for (int i = 0; i < 6; i++) {
        if (!parts[i]) continue;
        printf("%s%s", first ? "" : " ", parts[i]);
        first = 0;
    }
    printf("\n");
    return 0;
}

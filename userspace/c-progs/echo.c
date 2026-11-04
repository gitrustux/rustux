// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// echo [-n] [-e] [STRING]...: print the arguments separated by spaces.
// -n: no newline at the end. -e: interpret \n \t \\ \a \b \e, and \c
// (stop output here).

#include <stdio.h>
#include <string.h>

// Returns 1 if \c ended the output
static int put_escaped(const char *s) {
    for (; *s; s++) {
        if (*s != '\\' || !s[1]) { putchar(*s); continue; }
        switch (*++s) {
        case 'n': putchar('\n'); break;
        case 't': putchar('\t'); break;
        case 'a': putchar('\a'); break;
        case 'b': putchar('\b'); break;
        case 'e': putchar(27); break;
        case '\\': putchar('\\'); break;
        case 'c': return 1;
        default: putchar('\\'); putchar(*s); break;
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    int newline = 1, escapes = 0, i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        const char *o = argv[i] + 1;
        if (strspn(o, "neE") != strlen(o)) break; // not an option: print it
        for (; *o; o++) {
            if (*o == 'n') newline = 0;
            else if (*o == 'e') escapes = 1;
            else escapes = 0;
        }
    }
    for (int first = 1; i < argc; i++, first = 0) {
        if (!first) putchar(' ');
        if (escapes ? put_escaped(argv[i]) : (fputs(argv[i], stdout), 0)) return 0;
    }
    if (newline) putchar('\n');
    return 0;
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The console's colour themes: the system's theme files (themefile.h:
// Rustica, Dracula, Forest, Arctic, Sakura), their text, background and 16
// ANSI colours. They are set with OSC escape sequences, which the kernel
// console and the Terminal understand. /etc/console-theme names the one
// login sets at every start; Dracula is the kernel console's own colours.

#ifndef CONSOLETHEME_H
#define CONSOLETHEME_H

#include <dirent.h>
#include <stdio.h>
#include <string.h>
#include "themefile.h"

// The system's theme of that name, or 0 (the result lasts until the next call)
__attribute__((unused)) static const theme_t *console_theme_find(const char *name) {
    static theme_t t;
    return theme_load_system(&t, name) == 0 ? &t : 0;
}

// The sequences that set it, on f (then the screen is cleared, so that it
// is all in the new colours)
__attribute__((unused)) static void console_theme_apply(FILE *f, const theme_t *t) {
    if (!t) return;
    for (int i = 0; i < 16; i++) fprintf(f, "\033]4;%d;#%06X\007", i, t->ansi[i]);
    fprintf(f, "\033]10;#%06X\007\033]11;#%06X\007\033[0m\033[2J\033[H", t->c[TH_TEXT], t->c[TH_BG]);
    fflush(f);
}

// The system's themes' names, sorted, into names (at most max); how many
__attribute__((unused)) static int console_theme_names(char names[][16], int max) {
    int n = 0;
    DIR *d = opendir(THEME_DIR);
    struct dirent *e;
    while (d && n < max && (e = readdir(d))) {
        size_t l = strlen(e->d_name);
        if (l < 7 || strcmp(e->d_name + l - 6, ".theme") || l - 6 > 15) continue;
        memcpy(names[n], e->d_name, l - 6);
        names[n][l - 6] = 0;
        if (theme_name_ok(names[n])) n++;
    }
    if (d) closedir(d);
    for (int i = 1; i < n; i++)
        for (int j = i; j > 0 && strcmp(names[j - 1], names[j]) > 0; j--) {
            char t[16];
            memcpy(t, names[j], 16);
            memcpy(names[j], names[j - 1], 16);
            memcpy(names[j - 1], t, 16);
        }
    return n;
}

// The name in /etc/console-theme into out ("dracula" without one)
__attribute__((unused)) static void console_theme_saved(char *out, size_t n) {
    snprintf(out, n, "dracula");
    FILE *f = fopen("/etc/console-theme", "r");
    if (!f) return;
    char line[64];
    if (fgets(line, sizeof line, f)) {
        line[strcspn(line, " \t\r\n")] = 0;
        if (console_theme_find(line)) snprintf(out, n, "%s", line);
    }
    fclose(f);
}

#endif

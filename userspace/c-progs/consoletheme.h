// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The console's colour themes (2026-11-24): Dracula (the kernel console's
// own colours, the server's default) and Rustica (the desktop's), the
// same as rustica-ui's ANSI colours. They are set with OSC escape
// sequences, which the kernel console and the Terminal understand.
// /etc/console-theme names the one login sets at every start.

#ifndef CONSOLETHEME_H
#define CONSOLETHEME_H

#include <stdio.h>
#include <string.h>

typedef struct {
    const char *name;
    unsigned fg, bg;
    unsigned ansi[16];
} console_theme_t;

static const console_theme_t console_themes[] = {
    { "dracula", 0xF8F8F2, 0x282A36,
      { 0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2,
        0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF } },
    { "rustica", 0xF5F2EF, 0x101012,
      { 0x1C1B20, 0xE5383B, 0x9CCC65, 0xFFB347, 0x6CA6E0, 0xD96BA0, 0x5FC4C4, 0xE8E2DC,
        0x5A5560, 0xFF5A5F, 0xB5E07A, 0xFFD08A, 0x8FC1F0, 0xF08CC0, 0x8AE0E0, 0xFFFFFF } },
};
#define CONSOLE_THEMES (sizeof console_themes / sizeof console_themes[0])

static const console_theme_t *console_theme_find(const char *name) {
    for (size_t i = 0; i < CONSOLE_THEMES; i++)
        if (!strcmp(name, console_themes[i].name)) return &console_themes[i];
    return 0;
}

// The sequences that set it, on f (then the screen is cleared, so that it
// is all in the new colours)
static void console_theme_apply(FILE *f, const console_theme_t *t) {
    for (int i = 0; i < 16; i++) fprintf(f, "\033]4;%d;#%06X\007", i, t->ansi[i]);
    fprintf(f, "\033]10;#%06X\007\033]11;#%06X\007\033[0m\033[2J\033[H", t->fg, t->bg);
    fflush(f);
}

// The name in /etc/console-theme into out ("dracula" without one)
static void console_theme_saved(char *out, size_t n) {
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

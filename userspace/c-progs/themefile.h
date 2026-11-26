// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Theme files (2026-11-25): /usr/share/rustica/themes/NAME.theme and a
// user's own ~/.config/rustica/themes/NAME.theme, key=#RRGGBB lines with
// the semantic tokens below and ansi0-ansi15 (docs/SESSION.md "Themes").
// The compositor, `theme` and login read them here; rustica-ui reads the
// same files (ui.rs).

#ifndef THEMEFILE_H
#define THEMEFILE_H

#include <ctype.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum {
    TH_BG, TH_SURFACE, TH_RAISED, TH_HOVER, TH_BORDER, TH_TEXT, TH_MUTED, TH_PRIMARY, TH_PRIMARY_HOVER,
    TH_ACCENT, TH_SELECTION, TH_SUCCESS, TH_WARNING, TH_DANGER, TH_INFO, TH_TASKBAR, TH_N
};
static const char *const theme_tokens[TH_N] = {
    "bg", "surface", "surface-raised", "surface-hover", "border", "text", "text-muted", "primary", "primary-hover",
    "accent", "selection", "success", "warning", "danger", "info", "taskbar",
};

typedef struct {
    char name[16];  // its id (the file's name)
    char label[33]; // name= in the file
    uint32_t c[TH_N];
    uint32_t ansi[16];
} theme_t;

#define THEME_DIR "/usr/share/rustica/themes"

// Rustica, should its file be missing
static const theme_t theme_builtin = {
    "rustica", "Rustica",
    { 0x101010, 0x202024, 0x2B292B, 0x383438, 0x49302A, 0xFFFFFF, 0x858585, 0xFF6500, 0xFF812B, 0xD71920, 0x57230F,
      0x55C878, 0xFFB347, 0xFF3333, 0x65BDEB, 0x151416 },
    { 0x2B292B, 0xFF3333, 0x55C878, 0xFFB347, 0x65BDEB, 0xD96BA0, 0x5FC4C4, 0xE8E2DC, 0x858585, 0xFF5A5F, 0x8BE0A4,
      0xFFD08A, 0x8FD3F5, 0xF08CC0, 0x8AE0E0, 0xFFFFFF },
};

// A name a theme may have: a-z, 0-9 and -, at most 15
__attribute__((unused)) static int theme_name_ok(const char *n) {
    if (!n[0] || strlen(n) > 15) return 0;
    for (; *n; n++)
        if (!(islower((unsigned char)*n) || isdigit((unsigned char)*n) || *n == '-')) return 0;
    return 1;
}

// A file's text into t (tokens it lacks stay Rustica's)
__attribute__((unused)) static void theme_parse(theme_t *t, const char *name, const char *text) {
    *t = theme_builtin;
    snprintf(t->name, sizeof t->name, "%s", name);
    snprintf(t->label, sizeof t->label, "%s", name);
    const char *p = text;
    while (*p) {
        const char *e = strchr(p, '\n');
        size_t n = e ? (size_t)(e - p) : strlen(p);
        char line[96];
        if (n < sizeof line) {
            memcpy(line, p, n);
            line[n] = 0;
            char *eq = strchr(line, '=');
            if (line[0] != '#' && eq) {
                *eq = 0;
                char *v = eq + 1;
                while (*v == ' ') v++;
                if (!strcmp(line, "name")) {
                    if (*v) snprintf(t->label, sizeof t->label, "%s", v);
                } else {
                    if (*v == '#') v++;
                    char *end;
                    unsigned long c = strtoul(v, &end, 16);
                    if (end - v == 6) {
                        for (int i = 0; i < TH_N; i++)
                            if (!strcmp(line, theme_tokens[i])) t->c[i] = (uint32_t)c;
                        if (!strncmp(line, "ansi", 4) && isdigit((unsigned char)line[4])) {
                            int i = atoi(line + 4);
                            if (i >= 0 && i < 16) t->ansi[i] = (uint32_t)c;
                        }
                    }
                }
            }
        }
        p += n;
        if (*p) p++;
    }
}

// Read a theme file from fd (closed) into t; 0, or -1
__attribute__((unused)) static int theme_read_fd(theme_t *t, const char *name, int fd) {
    if (fd < 0) return -1;
    char buf[4096];
    ssize_t n = 0, r;
    while (n < (ssize_t)sizeof buf - 1 && (r = read(fd, buf + n, sizeof buf - 1 - (size_t)n)) > 0) n += r;
    close(fd);
    if (n <= 0) return -1;
    buf[n] = 0;
    theme_parse(t, name, buf);
    return 0;
}

// The system's theme of that name; -1 if there is none
__attribute__((unused)) static int theme_load_system(theme_t *t, const char *name) {
    if (!theme_name_ok(name)) return -1;
    char path[128];
    snprintf(path, sizeof path, THEME_DIR "/%s.theme", name);
    return theme_read_fd(t, name, open(path, O_RDONLY));
}

// t as a file's text into out; its length
__attribute__((unused)) static int theme_text(const theme_t *t, char *out, size_t size) {
    int k = snprintf(out, size, "name=%s\n", t->label);
    for (int i = 0; i < TH_N && k < (int)size; i++) k += snprintf(out + k, size - (size_t)k, "%s=#%06X\n", theme_tokens[i], t->c[i]);
    for (int i = 0; i < 16 && k < (int)size; i++) k += snprintf(out + k, size - (size_t)k, "ansi%d=#%06X\n", i, t->ansi[i]);
    return k < (int)size ? k : (int)size - 1;
}

#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// theme: the colour theme (2026-11-24; five themes and your own since
// 2026-11-25: Rustica, Dracula, Forest, Arctic, Sakura).
//
//   theme                     what the console and your desktop use
//   theme list                the themes (the system's, then your own)
//   theme NAME                both: the console now (and, as root, at
//                             every start: /etc/console-theme) and your
//                             desktop (~/.config/rustica.conf)
//   theme --console NAME      the console only
//   theme --desktop NAME      the desktop only (as Settings > Appearance;
//                             your own themes too)
//
// The server's console starts in Dracula and the desktop in Rustica.

#include <errno.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

#include "consoletheme.h"

static int desktop_theme(char *out, size_t n) {
    const char *home = getenv("HOME");
    snprintf(out, n, "rustica");
    char p[512], line[256];
    snprintf(p, sizeof p, "%s/.config/rustica.conf", home ? home : "/root");
    const char *files[] = { "/etc/rustica/appearance", p };
    for (int i = 0; i < 2; i++) {
        FILE *f = fopen(files[i], "r");
        if (!f) continue;
        while (fgets(line, sizeof line, f))
            if (!strncmp(line, "theme=", 6)) {
                line[strcspn(line, " \t\r\n")] = 0;
                snprintf(out, n, "%s", line + 6);
            }
        fclose(f);
    }
    return 0;
}

// theme=NAME in ~/.config/rustica.conf (the other lines kept)
static int set_desktop(const char *name) {
    const char *home = getenv("HOME");
    if (!home) home = "/root";
    char dir[512], p[512], tmp[520];
    snprintf(dir, sizeof dir, "%s/.config", home);
    mkdir(dir, 0755);
    snprintf(p, sizeof p, "%s/rustica.conf", dir);
    snprintf(tmp, sizeof tmp, "%s.new", p);
    FILE *in = fopen(p, "r"), *out = fopen(tmp, "w");
    if (!out) {
        fprintf(stderr, "theme: %s: %s\n", tmp, strerror(errno));
        if (in) fclose(in);
        return 1;
    }
    char line[512];
    int done = 0;
    while (in && fgets(line, sizeof line, in)) {
        if (!strncmp(line, "theme=", 6)) {
            if (!done) fprintf(out, "theme=%s\n", name);
            done = 1;
        } else {
            fputs(line, out);
        }
    }
    if (!done) fprintf(out, "theme=%s\n", name);
    if (in) fclose(in);
    if (fclose(out) != 0 || rename(tmp, p) != 0) {
        fprintf(stderr, "theme: %s: %s\n", p, strerror(errno));
        return 1;
    }
    printf("theme: your desktop uses %s\n", name);
    return 0;
}

// Your own themes (~/.config/rustica/themes/NAME.theme): with a name,
// that one if you have it; else the k-th, or 0
static const char *user_theme(const char *name, int k) {
    static char found[16];
    const char *home = getenv("HOME");
    char dir[256];
    snprintf(dir, sizeof dir, "%s/.config/rustica/themes", home ? home : "/root");
    DIR *d = opendir(dir);
    struct dirent *e;
    int i = 0;
    const char *r = 0;
    while (d && !r && (e = readdir(d))) {
        size_t l = strlen(e->d_name);
        if (l < 7 || l - 6 > 15 || strcmp(e->d_name + l - 6, ".theme")) continue;
        memcpy(found, e->d_name, l - 6);
        found[l - 6] = 0;
        if (!theme_name_ok(found)) continue;
        if (name ? !strcmp(name, found) : i++ == k) r = found;
    }
    if (d) closedir(d);
    return r;
}

static int set_console(const char *name) {
    const theme_t *t = console_theme_find(name);
    console_theme_apply(stdout, t);
    if (geteuid() == 0) {
        FILE *f = fopen("/etc/console-theme", "w");
        if (!f || fprintf(f, "%s\n", name) < 0 || fclose(f) != 0) {
            fprintf(stderr, "theme: /etc/console-theme: %s\n", strerror(errno));
            return 1;
        }
        printf("theme: the console uses %s (from now on)\n", name);
    } else {
        printf("theme: the console uses %s until the next start (root keeps it: sudo theme --console %s)\n", name, name);
    }
    return 0;
}

int main(int argc, char **argv) {
    int console = 1, desktop = 1, i = 1;
    if (argc > 1 && !strcmp(argv[1], "--console")) desktop = 0, i++;
    else if (argc > 1 && !strcmp(argv[1], "--desktop")) console = 0, i++;
    if (argc == 2 && (!strcmp(argv[1], "--help") || !strcmp(argv[1], "-h"))) {
        printf("usage: theme [list | [--console | --desktop] NAME]\n");
        return 0;
    }
    if (i >= argc) {
        char c[32], d[32];
        console_theme_saved(c, sizeof c);
        desktop_theme(d, sizeof d);
        printf("console: %s\ndesktop: %s\n", c, d);
        return 0;
    }
    if (!strcmp(argv[i], "list")) {
        char names[64][16];
        int n = console_theme_names(names, 64);
        for (int k = 0; k < n; k++) printf("%s\n", names[k]);
        for (int k = 0; user_theme(0, k); k++) printf("%s (yours)\n", user_theme(0, k));
        return 0;
    }
    if (i + 1 != argc || !(console_theme_find(argv[i]) || (!console && user_theme(argv[i], 0)))) {
        fprintf(stderr, "theme: no theme %s (theme list)\n", argv[i]);
        return 2;
    }
    int r = 0;
    if (console) r |= set_console(argv[i]);
    if (desktop) r |= set_desktop(argv[i]);
    return r;
}

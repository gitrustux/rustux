// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// browser: start the user's default web browser (docs/BROWSER.md).
//
//   browser [URL | FILE]
//   browser --which          the default browser's ID (or .app name)
//
// "Web Browser" in the menu and Files (for .html files) run this, so a
// browser chosen in Settings > Default apps is the one that opens. The
// choice is a line browser=ID in ~/.config/rustica/defaults, else in
// /etc/rustica/defaults (org.rustica.browser, NetSurf), where ID is an
// installed app (/apps/ID: started in its sandbox with appctl; a file is
// given to it with appctl open, a URL with appctl run) or the name of an
// application file in /usr/share/applications (NAME.app: its exec, with
// the argument). If that browser is gone, the system's default is used.

#include <errno.h>
#include <spawn.h>
#include <sys/wait.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define SYSTEM_DEFAULT "org.rustica.browser"

// The value of key= in a file, into out; 1 if found
static int read_key(const char *path, const char *key, char *out, size_t cap) {
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char line[512];
    size_t kl = strlen(key);
    int found = 0;
    while (fgets(line, sizeof line, f)) {
        char *p = line;
        while (*p == ' ' || *p == '\t') p++;
        if (*p == '#' || strncmp(p, key, kl) || p[kl] != '=') continue;
        p += kl + 1;
        while (*p == ' ' || *p == '\t') p++;
        size_t n = strcspn(p, " \t\r\n");
        if (n == 0 || n >= cap) continue;
        memcpy(out, p, n);
        out[n] = 0;
        found = 1;
    }
    fclose(f);
    return found;
}

// A safe name: an app ID or a .app file's name (no path)
static int sane(const char *id) {
    if (!id[0] || id[0] == '.') return 0;
    for (const char *c = id; *c; c++)
        if (!((*c >= 'a' && *c <= 'z') || (*c >= 'A' && *c <= 'Z') || (*c >= '0' && *c <= '9') || *c == '.' || *c == '-' || *c == '_'))
            return 0;
    return 1;
}

static int installed(const char *id) {
    char p[300];
    struct stat st;
    if (!sane(id)) return 0;
    size_t n = strlen(id);
    if (n > 4 && !strcmp(id + n - 4, ".app")) {
        snprintf(p, sizeof p, "/usr/share/applications/%s", id);
        return stat(p, &st) == 0 && strcmp(id, "browser.app") != 0;
    }
    snprintf(p, sizeof p, "/apps/%s/manifest", id);
    return stat(p, &st) == 0;
}

// The default browser: the user's choice, the system's, NetSurf
static void which(char *id, size_t cap) {
    const char *home = getenv("HOME");
    char p[512];
    if (home && *home) {
        snprintf(p, sizeof p, "%s/.config/rustica/defaults", home);
        if (read_key(p, "browser", id, cap) && installed(id)) return;
    }
    if (read_key("/etc/rustica/defaults", "browser", id, cap) && installed(id)) return;
    snprintf(id, cap, "%s", SYSTEM_DEFAULT);
}

extern char **environ;

// Start a program (Rustux has no exec: spawn it and wait); its exit code
static int run(char **av) {
    pid_t pid;
    int r = posix_spawn(&pid, av[0], 0, 0, av, environ);
    if (r != 0) {
        fprintf(stderr, "browser: %s: %s\n", av[0], strerror(r));
        return 1;
    }
    int st = 0;
    if (waitpid(pid, &st, 0) < 0) return 1;
    return WIFEXITED(st) ? WEXITSTATUS(st) : 1;
}

// Is the argument a file here (rather than a URL)?
static int is_file(const char *arg) {
    struct stat st;
    if (strstr(arg, "://")) return 0;
    return stat(arg, &st) == 0 && S_ISREG(st.st_mode);
}

int main(int argc, char **argv) {
    if (argc > 2 || (argc == 2 && (!strcmp(argv[1], "-h") || !strcmp(argv[1], "--help")))) {
        fprintf(stderr, "usage: browser [URL | FILE]   start the default web browser\n"
                        "       browser --which         print the default browser\n");
        return argc == 2 ? 0 : 2;
    }
    char id[256];
    which(id, sizeof id);
    if (argc == 2 && !strcmp(argv[1], "--which")) {
        printf("%s\n", id);
        return 0;
    }
    const char *arg = argc == 2 ? argv[1] : NULL;
    size_t n = strlen(id);
    if (n > 4 && !strcmp(id + n - 4, ".app")) {
        // An application file's exec line, then the argument
        char p[300], exec[512];
        snprintf(p, sizeof p, "/usr/share/applications/%s", id);
        if (!read_key(p, "exec", exec, sizeof exec)) {
            // exec may hold spaces: read the whole line
            fprintf(stderr, "browser: %s has no exec line\n", p);
            return 1;
        }
        FILE *f = fopen(p, "r");
        char line[512];
        while (f && fgets(line, sizeof line, f))
            if (!strncmp(line, "exec=", 5)) {
                line[strcspn(line, "\r\n")] = 0;
                snprintf(exec, sizeof exec, "%s", line + 5);
            }
        if (f) fclose(f);
        char *words[34];
        int k = 0;
        for (char *t = strtok(exec, " \t"); t && k < 32; t = strtok(NULL, " \t")) words[k++] = t;
        if (arg) words[k++] = (char *)arg;
        words[k] = NULL;
        if (k == 0) return 1;
        return run(words);
    }
    // An installed app, in its sandbox: a file is put in its view
    char *av[6];
    int k = 0;
    av[k++] = "/bin/appctl";
    av[k++] = arg && is_file(arg) ? "open" : "run";
    av[k++] = id;
    if (arg) av[k++] = (char *)arg;
    av[k] = NULL;
    return run(av);
}

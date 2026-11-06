// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// umount: detach a mounted filesystem (step 14). Everything it has is
// written to the disk first; the stick can be pulled when umount returns.
//
//   umount [-f] DIR|DEV     DEV as /etc/mtab names it (/dev/sda1 or sda1)
//   umount -a [-f]          every mounted disk filesystem
//
// "busy" means files on it are still open (or a shell is in a directory on
// it); -f unmounts all the same (those files then fail with EIO).

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <unistd.h>

static int fields(char *s, char **f, int max) {
    int n = 0;
    while (n < max) {
        while (*s == ' ' || *s == '\t' || *s == '\n') s++;
        if (!*s) break;
        f[n++] = s;
        while (*s && *s != ' ' && *s != '\t' && *s != '\n') s++;
        if (*s) *s++ = 0;
    }
    return n;
}

struct ent { char src[256], dir[256], type[32]; };

static int read_mtab(struct ent *e, int max) {
    FILE *f = fopen("/etc/mtab", "r");
    if (!f) return 0;
    char line[600], *fl[3];
    int n = 0;
    while (n < max && fgets(line, sizeof line, f)) {
        if (fields(line, fl, 3) < 3) continue;
        snprintf(e[n].src, sizeof e[n].src, "%s", fl[0]);
        snprintf(e[n].dir, sizeof e[n].dir, "%s", fl[1]);
        snprintf(e[n].type, sizeof e[n].type, "%s", fl[2]);
        n++;
    }
    fclose(f);
    return n;
}

static int is_builtin(const char *type) {
    return !strcmp(type, "tmpfs") || !strcmp(type, "bootfs") || !strcmp(type, "devfs");
}

static int one(const char *dir, int flags) {
    if (umount2(dir, flags) < 0) {
        fprintf(stderr, "umount: %s: %s\n", dir,
                errno == EBUSY ? "busy (files are open on it; -f unmounts anyway)" :
                errno == EINVAL ? "not mounted" : strerror(errno));
        return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    int flags = 0, all = 0, c;
    while ((c = getopt(argc, argv, "afh")) != -1) {
        if (c == 'f') flags |= MNT_FORCE;
        else if (c == 'a') all = 1;
        else {
            fprintf(stderr, "usage: umount [-f] DIR|DEV\n       umount -a [-f]\n");
            return 2;
        }
    }
    struct ent tab[40];
    int n = read_mtab(tab, 40), failed = 0;
    if (all) {
        for (int i = n - 1; i >= 0; i--) if (!is_builtin(tab[i].type)) failed |= one(tab[i].dir, flags);
        return failed;
    }
    if (optind != argc - 1) {
        fprintf(stderr, "usage: umount [-f] DIR|DEV\n       umount -a [-f]\n");
        return 2;
    }
    const char *t = argv[optind];
    // A device: the last place it is mounted
    for (int i = n - 1; i >= 0; i--) {
        const char *s = tab[i].src;
        if (!strcmp(s, t) || (!strncmp(s, "/dev/", 5) && !strcmp(s + 5, t))) return one(tab[i].dir, flags);
    }
    return one(t, flags);
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// useradd, userdel, usermod, groupadd, groupdel, chpasswd (step 17):
// users and groups in /etc/passwd, /etc/group and /etc/shadow. One
// program; the name it is run by says which. root's (the files are).
//
//   useradd [-m] [-d HOME] [-s SHELL] [-u UID] [-g GROUP] [-G G1,G2] [-c COMMENT] NAME
//       a new user (uid from 1000; a group of its own unless -g); -m makes
//       its home (/home/NAME, mode 0750, with /etc/skel's files); the
//       password is locked until passwd or chpasswd sets one
//   userdel [-r] NAME           -r: its home too
//   usermod [-a] [-G G1,G2] [-g GROUP] [-s SHELL] [-d HOME] [-c COMMENT] [-L|-U] NAME
//       -G sets the extra groups (-a adds to them); -L locks, -U unlocks
//   groupadd [-g GID] NAME      groupdel NAME
//   chpasswd                    lines "USER:PASSWORD" on stdin

#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "userdb.h"

static const char *prog = "useradd";

static int fail(const char *fmt, const char *arg) {
    fprintf(stderr, "%s: ", prog);
    fprintf(stderr, fmt, arg);
    fputc('\n', stderr);
    return 1;
}

// The first free id at or above `from` in a file's third field
static unsigned free_id(const char *path, unsigned from) {
    char **v = udb_read(path, 0);
    unsigned id = from;
    for (int again = 1; again;) {
        again = 0;
        for (int i = 0; v && v[i]; i++) {
            char f[32];
            if ((unsigned)strtoul(udb_field(v[i], 2, f, sizeof f), 0, 10) == id) { id++; again = 1; }
        }
    }
    udb_free(v);
    return id;
}

static int id_used(const char *path, unsigned id) {
    char **v = udb_read(path, 0);
    int used = 0;
    for (int i = 0; v && v[i] && !used; i++) {
        char f[32];
        used = (unsigned)strtoul(udb_field(v[i], 2, f, sizeof f), 0, 10) == id;
    }
    udb_free(v);
    return used;
}

static int append(const char *path, const char *line, mode_t mode) {
    int n;
    char **v = udb_read(path, &n);
    if (!v) return -ENOMEM;
    char **nv = realloc(v, sizeof *v * (size_t)(n + 2));
    if (!nv) { udb_free(v); return -ENOMEM; }
    nv[n] = strdup(line);
    nv[n + 1] = 0;
    int r = udb_write(path, nv, mode);
    udb_free(nv);
    return r;
}

static int remove_line(const char *path, const char *name, mode_t mode) {
    char **v = udb_read(path, 0);
    int i = udb_find(v, name);
    if (i < 0) { udb_free(v); return 0; }
    free(v[i]);
    for (; v[i]; i++) v[i] = v[i + 1];
    int r = udb_write(path, v, mode);
    udb_free(v);
    return r;
}

// A group's member list (field 3) with `user` added or removed
static int set_member(const char *group, const char *user, int add) {
    char **v = udb_read(UDB_GROUP, 0);
    int i = udb_find(v, group);
    if (i < 0) { udb_free(v); return -ENOENT; }
    char members[1024], out[1024] = "";
    udb_field(v[i], 3, members, sizeof members);
    int present = 0;
    for (char *s = strtok(members, ","); s; s = strtok(0, ",")) {
        if (!strcmp(s, user)) { present = 1; if (!add) continue; }
        if (*out) strcat(out, ",");
        strncat(out, s, sizeof out - strlen(out) - 2);
    }
    if (add && !present) {
        if (*out) strcat(out, ",");
        strncat(out, user, sizeof out - strlen(out) - 1);
    }
    char *nl = udb_set_field(v[i], 3, out);
    free(v[i]);
    v[i] = nl;
    int r = udb_write(UDB_GROUP, v, 0644);
    udb_free(v);
    return r;
}

// Every group's member list without `user`
static void remove_from_all(const char *user) {
    char **v = udb_read(UDB_GROUP, 0);
    for (int i = 0; v && v[i]; i++) {
        char name[64];
        udb_field(v[i], 0, name, sizeof name);
        set_member(name, user, 0);
    }
    udb_free(v);
}

// "G1,G2": each must exist
static int set_groups(const char *user, const char *list, int append_only) {
    char buf[1024];
    snprintf(buf, sizeof buf, "%s", list);
    for (char *s = strtok(buf, ","); s; s = strtok(0, ","))
        if (!getgrnam(s)) return fail("group '%s' does not exist", s);
    if (!append_only) remove_from_all(user);
    snprintf(buf, sizeof buf, "%s", list);
    char *save;
    for (char *s = strtok_r(buf, ",", &save); s; s = strtok_r(0, ",", &save)) set_member(s, user, 1);
    return 0;
}

// Copy /etc/skel's files into a new home
static void copy_skel(const char *home, uid_t uid, gid_t gid) {
    DIR *d = opendir("/etc/skel");
    struct dirent *e;
    while (d && (e = readdir(d))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        char src[512], dst[512], buf[4096];
        snprintf(src, sizeof src, "/etc/skel/%s", e->d_name);
        snprintf(dst, sizeof dst, "%s/%s", home, e->d_name);
        int in = open(src, O_RDONLY), out = in >= 0 ? open(dst, O_WRONLY | O_CREAT | O_EXCL, 0644) : -1;
        ssize_t k;
        while (out >= 0 && (k = read(in, buf, sizeof buf)) > 0) write(out, buf, (size_t)k);
        if (out >= 0) { close(out); chown(dst, uid, gid); }
        if (in >= 0) close(in);
    }
    if (d) closedir(d);
}

static int rm_tree(const char *path) {
    struct stat st;
    if (lstat(path, &st) < 0) return -1;
    if (S_ISDIR(st.st_mode)) {
        DIR *d = opendir(path);
        struct dirent *e;
        while (d && (e = readdir(d))) {
            if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
            char p[1024];
            snprintf(p, sizeof p, "%s/%s", path, e->d_name);
            rm_tree(p);
        }
        if (d) closedir(d);
        return rmdir(path);
    }
    return unlink(path);
}

static int do_useradd(int argc, char **argv) {
    int make_home = 0, c;
    const char *home = 0, *shell = "/bin/shell", *group = 0, *groups = 0, *comment = "";
    long uid = -1;
    while ((c = getopt(argc, argv, "md:s:u:g:G:c:")) != -1) {
        switch (c) {
        case 'm': make_home = 1; break;
        case 'd': home = optarg; break;
        case 's': shell = optarg; break;
        case 'u': uid = strtol(optarg, 0, 10); break;
        case 'g': group = optarg; break;
        case 'G': groups = optarg; break;
        case 'c': comment = optarg; break;
        default: return fail("usage: useradd [-m] [-d HOME] [-s SHELL] [-u UID] [-g GROUP] [-G G1,G2] [-c COMMENT] %s", "NAME");
        }
    }
    if (optind != argc - 1) return fail("usage: useradd [-m] [-d HOME] [-s SHELL] [-u UID] [-g GROUP] [-G G1,G2] [-c COMMENT] %s", "NAME");
    const char *name = argv[optind];
    if (!udb_valid_name(name)) return fail("invalid user name '%s'", name);
    if (getpwnam(name)) return fail("user '%s' already exists", name);
    if (strchr(comment, ':') || strchr(shell, ':') || (home && strchr(home, ':'))) return fail("':' is not allowed in %s", "fields");
    if (uid < 0) uid = (long)free_id(UDB_PASSWD, 1000);
    else if (id_used(UDB_PASSWD, (unsigned)uid)) return fail("that uid is already used (%s)", name);
    gid_t gid;
    if (group) {
        struct group *g = getgrnam(group);
        if (!g) {
            char *end;
            long n = strtol(group, &end, 10);
            if (*end || !getgrgid((gid_t)n)) return fail("group '%s' does not exist", group);
            gid = (gid_t)n;
        } else {
            gid = g->gr_gid;
        }
    } else {
        // A group of its own, with the same number if free
        if (getgrnam(name)) return fail("a group named '%s' exists (use -g)", name);
        gid = id_used(UDB_GROUP, (unsigned)uid) ? free_id(UDB_GROUP, 1000) : (gid_t)uid;
        char line[128];
        snprintf(line, sizeof line, "%s:x:%u:", name, (unsigned)gid);
        if (append(UDB_GROUP, line, 0644) < 0) return fail("cannot write %s", UDB_GROUP);
    }
    char hbuf[300];
    if (!home) {
        snprintf(hbuf, sizeof hbuf, "/home/%s", name);
        home = hbuf;
    }
    char line[1024];
    snprintf(line, sizeof line, "%s:x:%ld:%u:%s:%s:%s", name, uid, (unsigned)gid, comment, home, shell);
    if (append(UDB_PASSWD, line, 0644) < 0) return fail("cannot write %s (are you root?)", UDB_PASSWD);
    if (udb_set_password(name, 0) < 0) return fail("cannot write %s", UDB_SHADOW);
    if (groups && set_groups(name, groups, 1)) return 1;
    if (make_home) {
        if (mkdir(home, 0750) < 0 && errno != EEXIST) {
            fprintf(stderr, "%s: cannot make %s: %s\n", prog, home, strerror(errno));
            return 1;
        }
        chown(home, (uid_t)uid, gid);
        chmod(home, 0750);
        copy_skel(home, (uid_t)uid, gid);
    }
    return 0;
}

static int do_userdel(int argc, char **argv) {
    int rm = 0, c;
    while ((c = getopt(argc, argv, "r")) != -1) {
        if (c == 'r') rm = 1;
        else return fail("usage: userdel [-r] %s", "NAME");
    }
    if (optind != argc - 1) return fail("usage: userdel [-r] %s", "NAME");
    const char *name = argv[optind];
    struct passwd *pw = getpwnam(name);
    if (!pw) return fail("user '%s' does not exist", name);
    if (pw->pw_uid == 0) return fail("'%s' is root: not removed", name);
    char home[300];
    snprintf(home, sizeof home, "%s", pw->pw_dir);
    gid_t gid = pw->pw_gid;
    if (remove_line(UDB_PASSWD, name, 0644) < 0) return fail("cannot write %s (are you root?)", UDB_PASSWD);
    remove_line(UDB_SHADOW, name, 0600);
    remove_from_all(name);
    // Its own group, if nobody else uses it
    struct group *g = getgrgid(gid);
    if (g && !strcmp(g->gr_name, name) && (!g->gr_mem || !g->gr_mem[0])) remove_line(UDB_GROUP, name, 0644);
    if (rm && *home && strcmp(home, "/")) rm_tree(home);
    return 0;
}

static int do_usermod(int argc, char **argv) {
    int add = 0, lock = 0, unlock = 0, c;
    const char *groups = 0, *group = 0, *shell = 0, *home = 0, *comment = 0;
    while ((c = getopt(argc, argv, "aG:g:s:d:c:LU")) != -1) {
        switch (c) {
        case 'a': add = 1; break;
        case 'G': groups = optarg; break;
        case 'g': group = optarg; break;
        case 's': shell = optarg; break;
        case 'd': home = optarg; break;
        case 'c': comment = optarg; break;
        case 'L': lock = 1; break;
        case 'U': unlock = 1; break;
        default: return fail("usage: usermod [-a] [-G G1,G2] [-g GROUP] [-s SHELL] [-d HOME] [-c COMMENT] [-L|-U] %s", "NAME");
        }
    }
    if (optind != argc - 1) return fail("usage: usermod [-a] [-G G1,G2] [-g GROUP] [-s SHELL] [-d HOME] [-c COMMENT] [-L|-U] %s", "NAME");
    const char *name = argv[optind];
    if (!getpwnam(name)) return fail("user '%s' does not exist", name);
    if (groups && set_groups(name, groups, add)) return 1;
    char **v = udb_read(UDB_PASSWD, 0);
    int i = udb_find(v, name);
    struct { int k; const char *val; } ch[4] = { { 3, 0 }, { 4, comment }, { 5, home }, { 6, shell } };
    char gbuf[32];
    if (group) {
        struct group *g = getgrnam(group);
        if (!g) { udb_free(v); return fail("group '%s' does not exist", group); }
        snprintf(gbuf, sizeof gbuf, "%u", (unsigned)g->gr_gid);
        ch[0].val = gbuf;
    }
    int changed = 0;
    for (int k = 0; k < 4 && i >= 0; k++) {
        if (!ch[k].val) continue;
        if (strchr(ch[k].val, ':')) { udb_free(v); return fail("':' is not allowed in %s", "fields"); }
        char *nl = udb_set_field(v[i], ch[k].k, ch[k].val);
        free(v[i]);
        v[i] = nl;
        changed = 1;
    }
    if (changed && udb_write(UDB_PASSWD, v, 0644) < 0) { udb_free(v); return fail("cannot write %s", UDB_PASSWD); }
    udb_free(v);
    if (lock || unlock) {
        char **s = udb_read(UDB_SHADOW, 0);
        int j = udb_find(s, name);
        if (j >= 0) {
            char hash[256];
            udb_field(s[j], 1, hash, sizeof hash);
            char nh[260];
            if (lock && hash[0] != '!') snprintf(nh, sizeof nh, "!%s", hash);
            else if (unlock && hash[0] == '!' && hash[1]) snprintf(nh, sizeof nh, "%s", hash + 1);
            else snprintf(nh, sizeof nh, "%s", hash);
            char *nl = udb_set_field(s[j], 1, nh);
            free(s[j]);
            s[j] = nl;
            if (udb_write(UDB_SHADOW, s, 0600) < 0) { udb_free(s); return fail("cannot write %s", UDB_SHADOW); }
        }
        udb_free(s);
    }
    return 0;
}

static int do_groupadd(int argc, char **argv) {
    long gid = -1;
    int c;
    while ((c = getopt(argc, argv, "g:")) != -1) {
        if (c == 'g') gid = strtol(optarg, 0, 10);
        else return fail("usage: groupadd [-g GID] %s", "NAME");
    }
    if (optind != argc - 1) return fail("usage: groupadd [-g GID] %s", "NAME");
    const char *name = argv[optind];
    if (!udb_valid_name(name)) return fail("invalid group name '%s'", name);
    if (getgrnam(name)) return fail("group '%s' already exists", name);
    if (gid < 0) gid = (long)free_id(UDB_GROUP, 1000);
    else if (id_used(UDB_GROUP, (unsigned)gid)) return fail("that gid is already used (%s)", name);
    char line[128];
    snprintf(line, sizeof line, "%s:x:%ld:", name, gid);
    return append(UDB_GROUP, line, 0644) < 0 ? fail("cannot write %s (are you root?)", UDB_GROUP) : 0;
}

static int do_groupdel(int argc, char **argv) {
    if (argc != 2) return fail("usage: groupdel %s", "NAME");
    struct group *g = getgrnam(argv[1]);
    if (!g) return fail("group '%s' does not exist", argv[1]);
    gid_t gid = g->gr_gid;
    setpwent();
    for (struct passwd *pw; (pw = getpwent());)
        if (pw->pw_gid == gid) { endpwent(); return fail("it is the primary group of %s", pw->pw_name); }
    endpwent();
    return remove_line(UDB_GROUP, argv[1], 0644) < 0 ? fail("cannot write %s (are you root?)", UDB_GROUP) : 0;
}

static int do_chpasswd(void) {
    char line[1024];
    int bad = 0, n = 0;
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\r\n")] = 0;
        char *colon = strchr(line, ':');
        if (!colon || colon == line) { if (*line) bad++; continue; }
        *colon = 0;
        if (!getpwnam(line)) { fprintf(stderr, "chpasswd: no user %s\n", line); bad++; continue; }
        int r = udb_set_password(line, colon + 1);
        memset(colon + 1, 0, strlen(colon + 1));
        if (r < 0) { fprintf(stderr, "chpasswd: cannot write %s (are you root?)\n", UDB_SHADOW); return 1; }
        n++;
    }
    return bad ? 1 : 0;
}

int main(int argc, char **argv) {
    const char *base = strrchr(argv[0], '/') ? strrchr(argv[0], '/') + 1 : argv[0];
    prog = base;
    if (argc == 2 && !strcmp(argv[1], "--help")) {
        printf("useradd, userdel, usermod, groupadd, groupdel, chpasswd: see man %s\n", prog);
        return 0;
    }
    if (!strcmp(base, "userdel")) return do_userdel(argc, argv);
    if (!strcmp(base, "usermod")) return do_usermod(argc, argv);
    if (!strcmp(base, "groupadd")) return do_groupadd(argc, argv);
    if (!strcmp(base, "groupdel")) return do_groupdel(argc, argv);
    if (!strcmp(base, "chpasswd")) return do_chpasswd();
    prog = "useradd";
    return do_useradd(argc, argv);
}

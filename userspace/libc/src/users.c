// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Users and groups from /etc/passwd and /etc/group (the usual
// colon-separated formats), locale stubs, file system statistics,
// semaphores and file times.

#include "libc.h"
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <langinfo.h>
#include <locale.h>
#include <pwd.h>
#include <rustux/fs.h>
#include <semaphore.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/mount.h>
#include <sys/statfs.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
#include <utime.h>

static int fail(int e) {
    errno = e;
    return -1;
}

// ---- /etc/passwd and /etc/group ----

// Split a line at ':' into at most n fields; returns the count
static int fields(char *line, char **f, int n) {
    int k = 0;
    line[strcspn(line, "\r\n")] = 0;
    while (k < n) {
        f[k++] = line;
        char *c = strchr(line, ':');
        if (!c) break;
        *c = 0;
        line = c + 1;
    }
    return k;
}

// Copy strings into the caller's buffer
static char *put(char **p, char *end, const char *s) {
    size_t n = strlen(s) + 1;
    if (*p + n > end) return 0;
    char *r = memcpy(*p, s, n);
    *p += n;
    return r;
}

// Find a user by name (name != 0) or uid
static int find_user(const char *name, uid_t uid, struct passwd *pw, char *buf, size_t len, struct passwd **res) {
    *res = 0;
    FILE *f = fopen("/etc/passwd", "r");
    if (!f) return 0; // no such user
    char line[512];
    while (fgets(line, sizeof line, f)) {
        char *v[7];
        if (fields(line, v, 7) < 7) continue;
        uid_t u = (uid_t)strtoul(v[2], 0, 10);
        if (name ? strcmp(v[0], name) : u != uid) continue;
        char *p = buf, *end = buf + len;
        pw->pw_uid = u;
        pw->pw_gid = (gid_t)strtoul(v[3], 0, 10);
        if (!(pw->pw_name = put(&p, end, v[0])) || !(pw->pw_passwd = put(&p, end, v[1])) ||
            !(pw->pw_gecos = put(&p, end, v[4])) || !(pw->pw_dir = put(&p, end, v[5])) ||
            !(pw->pw_shell = put(&p, end, v[6]))) {
            fclose(f);
            return ERANGE;
        }
        *res = pw;
        break;
    }
    fclose(f);
    return 0;
}

int getpwuid_r(uid_t uid, struct passwd *pw, char *buf, size_t len, struct passwd **res) {
    return find_user(0, uid, pw, buf, len, res);
}
int getpwnam_r(const char *name, struct passwd *pw, char *buf, size_t len, struct passwd **res) {
    return find_user(name, 0, pw, buf, len, res);
}

static struct passwd pw_static;
static char pw_buf[512];
struct passwd *getpwuid(uid_t uid) {
    struct passwd *r;
    int e = getpwuid_r(uid, &pw_static, pw_buf, sizeof pw_buf, &r);
    if (e) errno = e;
    return r;
}
struct passwd *getpwnam(const char *name) {
    struct passwd *r;
    int e = getpwnam_r(name, &pw_static, pw_buf, sizeof pw_buf, &r);
    if (e) errno = e;
    return r;
}

static int pw_index;
void setpwent(void) { pw_index = 0; }
void endpwent(void) { pw_index = 0; }
struct passwd *getpwent(void) {
    FILE *f = fopen("/etc/passwd", "r");
    if (!f) return 0;
    char line[512], name[64] = "";
    int i = 0;
    while (fgets(line, sizeof line, f)) {
        if (!strchr(line, ':')) continue;
        if (i++ == pw_index) {
            snprintf(name, sizeof name, "%.*s", (int)strcspn(line, ":"), line);
            break;
        }
    }
    fclose(f);
    if (!name[0]) return 0;
    pw_index++;
    return getpwnam(name);
}

static int find_group(const char *name, gid_t gid, struct group *gr, char *buf, size_t len, struct group **res) {
    *res = 0;
    FILE *f = fopen("/etc/group", "r");
    if (!f) return 0;
    char line[512];
    while (fgets(line, sizeof line, f)) {
        char *v[4];
        if (fields(line, v, 4) < 3) continue;
        gid_t g = (gid_t)strtoul(v[2], 0, 10);
        if (name ? strcmp(v[0], name) : g != gid) continue;
        // Members: pointers at the start of the buffer (aligned), then the strings
        size_t nmem = 1;
        const char *m = v[3] ? v[3] : "";
        for (const char *c = m; *c; c++) nmem += *c == ',';
        size_t ptrs = (nmem + 1) * sizeof(char *);
        char *p = buf + ((8 - (uintptr_t)buf % 8) % 8), *end = buf + len;
        if (p + ptrs > end) { fclose(f); return ERANGE; }
        gr->gr_mem = (char **)p;
        p += ptrs;
        gr->gr_gid = g;
        if (!(gr->gr_name = put(&p, end, v[0])) || !(gr->gr_passwd = put(&p, end, v[1]))) { fclose(f); return ERANGE; }
        size_t k = 0;
        char mem[512];
        snprintf(mem, sizeof mem, "%s", m);
        char *save;
        for (char *t = strtok_r(mem, ",", &save); t; t = strtok_r(0, ",", &save)) {
            if (!(gr->gr_mem[k++] = put(&p, end, t))) { fclose(f); return ERANGE; }
        }
        gr->gr_mem[k] = 0;
        *res = gr;
        break;
    }
    fclose(f);
    return 0;
}

int getgrgid_r(gid_t gid, struct group *gr, char *buf, size_t len, struct group **res) {
    return find_group(0, gid, gr, buf, len, res);
}
int getgrnam_r(const char *name, struct group *gr, char *buf, size_t len, struct group **res) {
    return find_group(name, 0, gr, buf, len, res);
}

static struct group gr_static;
static char gr_buf[1024];
struct group *getgrgid(gid_t gid) {
    struct group *r;
    int e = getgrgid_r(gid, &gr_static, gr_buf, sizeof gr_buf, &r);
    if (e) errno = e;
    return r;
}
struct group *getgrnam(const char *name) {
    struct group *r;
    int e = getgrnam_r(name, &gr_static, gr_buf, sizeof gr_buf, &r);
    if (e) errno = e;
    return r;
}

static int gr_index;
void setgrent(void) { gr_index = 0; }
void endgrent(void) { gr_index = 0; }
struct group *getgrent(void) {
    FILE *f = fopen("/etc/group", "r");
    if (!f) return 0;
    char line[512], name[64] = "";
    int i = 0;
    while (fgets(line, sizeof line, f)) {
        if (!strchr(line, ':')) continue;
        if (i++ == gr_index) {
            snprintf(name, sizeof name, "%.*s", (int)strcspn(line, ":"), line);
            break;
        }
    }
    fclose(f);
    if (!name[0]) return 0;
    gr_index++;
    return getgrnam(name);
}

// The user's primary group plus every group that lists them
int getgrouplist(const char *user, gid_t group, gid_t *groups, int *ngroups) {
    int n = 0, cap = *ngroups;
    if (n < cap) groups[n] = group;
    n++;
    FILE *f = fopen("/etc/group", "r");
    char line[512];
    while (f && fgets(line, sizeof line, f)) {
        char *v[4];
        if (fields(line, v, 4) < 4) continue;
        gid_t g = (gid_t)strtoul(v[2], 0, 10);
        if (g == group) continue;
        char *save;
        for (char *t = strtok_r(v[3], ",", &save); t; t = strtok_r(0, ",", &save)) {
            if (!strcmp(t, user)) {
                if (n < cap) groups[n] = g;
                n++;
                break;
            }
        }
    }
    if (f) fclose(f);
    *ngroups = n;
    return n > cap ? -1 : n;
}

int getgroups(int n, gid_t list[]) {
    if (n > 0) list[0] = 0;
    return 1;
}

char *getlogin(void) {
    struct passwd *p = getpwuid(getuid());
    return p ? p->pw_name : "root";
}

// ---- locale: "C" with UTF-8 ----

char *setlocale(int category, const char *name) {
    (void)category;
    if (!name || !*name || !strcmp(name, "C") || !strcmp(name, "POSIX") || strstr(name, "UTF-8") || strstr(name, "utf8"))
        return "C.UTF-8";
    return 0;
}

struct lconv *localeconv(void) {
    static struct lconv l = { ".", "", "" };
    return &l;
}

char *nl_langinfo(nl_item item) {
    switch (item) {
    case CODESET: return "UTF-8";
    case RADIXCHAR: return ".";
    case THOUSEP: return "";
    case D_T_FMT: return "%a %b %e %H:%M:%S %Y";
    case D_FMT: return "%m/%d/%y";
    case T_FMT: return "%H:%M:%S";
    default: return "";
    }
}

// ---- file system statistics: memory for tmpfs, the image for bootfs ----

static int fill_statfs(const struct stat *st, const char *path, struct statfs *s) {
    memset(s, 0, sizeof *s);
    rx_system_stats_t sys;
    memset(&sys, 0, sizeof sys);
    rx_system_get_stats(&sys);
    s->f_bsize = s->f_frsize = 4096;
    s->f_namelen = 255;
    s->f_type = TMPFS_MAGIC;
    s->f_blocks = sys.total_bytes / 4096;
    s->f_bfree = s->f_bavail = sys.free_bytes / 4096;
    s->f_files = 1000;
    s->f_ffree = 1000;
    int ro = path && (!strncmp(path, "/boot", 5) || !strncmp(path, "/bin", 4) || !strncmp(path, "/usr", 4));
    (void)st;
    if (ro) {
        s->f_type = RAMFS_MAGIC;
        s->f_bfree = s->f_bavail = 0;
        s->f_flags = 1; // ST_RDONLY
    }
    return 0;
}

// The filesystem's own figures (RX_FS_OP_STATFS, step 14); 0 if it has none
static int ask_statfs(int fd, struct statfs *s) {
    rx_fs_statfs_t r;
    if (rx_statfs_fd(fd, &r) < 0) return 0;
    memset(s, 0, sizeof *s);
    s->f_bsize = s->f_frsize = (unsigned long)r.block_size;
    s->f_blocks = r.blocks;
    s->f_bfree = s->f_bavail = r.blocks_free;
    s->f_files = r.files;
    s->f_ffree = r.files_free;
    s->f_namelen = r.name_max;
    s->f_flags = (r.flags & RX_FS_MOUNT_RDONLY) ? 1 : 0; // ST_RDONLY
    s->f_type = !strcmp(r.type, "vfat") ? MSDOS_SUPER_MAGIC : !strcmp(r.type, "ext2") || !strcmp(r.type, "ext4") ? EXT2_SUPER_MAGIC
              : !strcmp(r.type, "bootfs") ? RAMFS_MAGIC : TMPFS_MAGIC;
    return 1;
}

int fstatfs(int fd, struct statfs *s) {
    if (ask_statfs(fd, s)) return 0;
    struct stat st;
    if (fstat(fd, &st) < 0) return -1;
    struct fd *f = __fd_get(fd);
    return fill_statfs(&st, f ? f->path : 0, s);
}

int statfs(const char *path, struct statfs *s) {
    int fd = open(path, O_RDONLY);
    if (fd >= 0) {
        int ok = ask_statfs(fd, s);
        close(fd);
        if (ok) return 0;
    }
    struct stat st;
    if (stat(path, &st) < 0) return -1;
    char real[PATH_MAX];
    return fill_statfs(&st, realpath(path, real) ? real : path, s);
}

// ---- file times and modes ----

static int setattr_fd(int fd, uint32_t mask, uint32_t mode, int64_t mtime) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE) return 0;
    rx_fs_setattr_t sa = { mask, mode, mtime };
    int r = __fs_call(f->handle, RX_FS_OP_SETATTR, &sa, sizeof sa, 0, 0, 0, 0, 0, 0);
    return r < 0 ? fail(-r) : 0;
}

static int setattr_path(const char *path, uint32_t mask, uint32_t mode, int64_t mtime) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    int r = setattr_fd(fd, mask, mode, mtime);
    int e = errno;
    close(fd);
    errno = e;
    return r;
}

// The modification time to set from a timespec pair (-1: leave it)
static int64_t mtime_of(const struct timespec *t) {
    if (!t || t[1].tv_nsec == UTIME_NOW) return rx_clock_get_utc();
    if (t[1].tv_nsec == UTIME_OMIT) return -1;
    return t[1].tv_sec * 1000000000ll + t[1].tv_nsec;
}

int futimens(int fd, const struct timespec t[2]) {
    int64_t m = mtime_of(t);
    return m < 0 ? 0 : setattr_fd(fd, RX_FS_SETATTR_MTIME, 0, m);
}

int utimensat(int dirfd, const char *path, const struct timespec t[2], int flags) {
    (void)flags;
    int64_t m = mtime_of(t);
    if (!path) return futimens(dirfd, t);
    int fd = openat(dirfd, path, O_RDONLY);
    if (fd < 0) return -1;
    int r = m < 0 ? 0 : setattr_fd(fd, RX_FS_SETATTR_MTIME, 0, m);
    int e = errno;
    close(fd);
    errno = e;
    return r;
}

int utimes(const char *path, const struct timeval t[2]) {
    int64_t m = t ? t[1].tv_sec * 1000000000ll + t[1].tv_usec * 1000ll : rx_clock_get_utc();
    return setattr_path(path, RX_FS_SETATTR_MTIME, 0, m);
}
int lutimes(const char *path, const struct timeval t[2]) { return utimes(path, t); }
int utime(const char *path, const struct utimbuf *t) {
    int64_t m = t ? t->modtime * 1000000000ll : rx_clock_get_utc();
    return setattr_path(path, RX_FS_SETATTR_MTIME, 0, m);
}

int chmod(const char *path, mode_t mode) { return setattr_path(path, RX_FS_SETATTR_MODE, mode & 07777, 0); }
int fchmod(int fd, mode_t mode) { return setattr_fd(fd, RX_FS_SETATTR_MODE, mode & 07777, 0); }
int fchmodat(int dirfd, const char *path, mode_t mode, int flags) {
    (void)flags;
    int fd = openat(dirfd, path, O_RDONLY);
    if (fd < 0) return -1;
    int r = fchmod(fd, mode);
    int e = errno;
    close(fd);
    errno = e;
    return r;
}

// No owners until step 17: everything belongs to root
int chown(const char *path, uid_t u, gid_t g) { (void)u; (void)g; struct stat st; return stat(path, &st); }
int lchown(const char *path, uid_t u, gid_t g) { return chown(path, u, g); }
int fchown(int fd, uid_t u, gid_t g) { (void)u; (void)g; struct stat st; return fstat(fd, &st); }
int fchownat(int dirfd, const char *path, uid_t u, gid_t g, int flags) {
    (void)u; (void)g;
    struct stat st;
    return fstatat(dirfd, path, &st, flags);
}

int mkfifo(const char *path, mode_t mode) { (void)path; (void)mode; return fail(EPERM); }
int mknod(const char *path, mode_t mode, dev_t dev) { (void)path; (void)mode; (void)dev; return fail(EPERM); }
int linkat(int a, const char *old, int b, const char *new, int flags) {
    (void)a; (void)old; (void)b; (void)new; (void)flags;
    return fail(EPERM);
}
int symlinkat(const char *target, int dirfd, const char *link) { (void)target; (void)dirfd; (void)link; return fail(EPERM); }
ssize_t readlinkat(int dirfd, const char *restrict path, char *restrict buf, size_t size) {
    (void)buf; (void)size;
    struct stat st;
    if (fstatat(dirfd, path, &st, 0) < 0) return -1;
    return fail(EINVAL);
}

// ---- semaphores (on the library's futexes) ----

int sem_init(sem_t *s, int pshared, unsigned value) {
    (void)pshared;
    memset(s, 0, sizeof *s);
    s->__val[0] = (int)value;
    return 0;
}
int sem_destroy(sem_t *s) { (void)s; return 0; }

int sem_trywait(sem_t *s) {
    int v = __atomic_load_n(&s->__val[0], __ATOMIC_ACQUIRE);
    while (v > 0) {
        if (__atomic_compare_exchange_n(&s->__val[0], &v, v - 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) return 0;
    }
    return fail(EAGAIN);
}

int sem_timedwait(sem_t *restrict s, const struct timespec *restrict abstime) {
    uint64_t deadline = __deadline_abs(CLOCK_REALTIME, abstime);
    for (;;) {
        if (sem_trywait(s) == 0) return 0;
        __atomic_fetch_add(&s->__val[1], 1, __ATOMIC_ACQ_REL); // waiters
        int r = __futex_wait(&s->__val[0], 0, deadline);
        __atomic_fetch_sub(&s->__val[1], 1, __ATOMIC_ACQ_REL);
        if (r == -ETIMEDOUT) return fail(ETIMEDOUT);
    }
}
int sem_wait(sem_t *s) { return sem_timedwait(s, 0); }

int sem_post(sem_t *s) {
    __atomic_fetch_add(&s->__val[0], 1, __ATOMIC_ACQ_REL);
    if (__atomic_load_n(&s->__val[1], __ATOMIC_ACQUIRE)) __futex_wake(&s->__val[0], 1);
    return 0;
}

int sem_getvalue(sem_t *restrict s, int *restrict value) {
    *value = s->__val[0];
    return 0;
}

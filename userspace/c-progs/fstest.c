// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fstest: the filesystem service, file descriptors and pipes (checklist
// step 8). Works in /tmp/fstest, which it removes at the end.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define DIR_ "/tmp/fstest"
#define PIPE_BYTES (300 * 1024)

static int passed, failed;

static void check(const char *name, int ok) {
    if (ok) passed++;
    else { failed++; printf("  FAIL %s (errno %d: %s)\n", name, errno, strerror(errno)); }
}

static int write_file(const char *path, const char *text) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return -1;
    int r = write(fd, text, strlen(text)) == (ssize_t)strlen(text) ? 0 : -1;
    close(fd);
    return r;
}

// Whole contents of `path` into buf (NUL-terminated); returns the length or -1
static ssize_t read_file(const char *path, char *buf, size_t cap) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    size_t n = 0;
    ssize_t k;
    while (n < cap - 1 && (k = read(fd, buf + n, cap - 1 - n)) > 0) n += (size_t)k;
    close(fd);
    buf[n] = 0;
    return (ssize_t)n;
}

static int fails_with(int r, int e) { return r < 0 && errno == e; }

static void basics(void) {
    char buf[256];
    printf("files:\n");
    check("write a new file", write_file(DIR_ "/a", "hello") == 0);
    check("read it back", read_file(DIR_ "/a", buf, sizeof buf) == 5 && !strcmp(buf, "hello"));
    check("O_EXCL on an existing file -> EEXIST", fails_with(open(DIR_ "/a", O_WRONLY | O_CREAT | O_EXCL, 0644), EEXIST));
    int fd = open(DIR_ "/a", O_WRONLY | O_APPEND);
    check("O_APPEND", fd >= 0 && write(fd, " world", 6) == 6);
    close(fd);
    check("  appended", read_file(DIR_ "/a", buf, sizeof buf) == 11 && !strcmp(buf, "hello world"));
    write_file(DIR_ "/a", "xy");
    check("O_TRUNC", read_file(DIR_ "/a", buf, sizeof buf) == 2 && !strcmp(buf, "xy"));

    fd = open(DIR_ "/s", O_RDWR | O_CREAT, 0644);
    check("lseek SEEK_SET past the end", lseek(fd, 100, SEEK_SET) == 100);
    check("write there", write(fd, "Z", 1) == 1);
    struct stat st;
    check("fstat: size 101, regular file", fstat(fd, &st) == 0 && st.st_size == 101 && S_ISREG(st.st_mode));
    check("lseek SEEK_END", lseek(fd, -1, SEEK_END) == 100);
    check("read the last byte", read(fd, buf, 10) == 1 && buf[0] == 'Z');
    check("read at the end -> 0", read(fd, buf, 10) == 0);
    lseek(fd, 50, SEEK_SET);
    check("the gap reads as zeros", read(fd, buf, 4) == 4 && !buf[0] && !buf[3]);
    check("lseek SEEK_CUR", lseek(fd, -4, SEEK_CUR) == 50);
    check("lseek before 0 -> EINVAL", fails_with((int)lseek(fd, -1, SEEK_SET), EINVAL));
    check("ftruncate", ftruncate(fd, 10) == 0 && fstat(fd, &st) == 0 && st.st_size == 10);

    int d = dup(fd);
    lseek(fd, 3, SEEK_SET);
    check("dup shares the offset", lseek(d, 0, SEEK_CUR) == 3);
    close(d);
    close(fd);

    fd = open(DIR_ "/gone", O_RDWR | O_CREAT, 0644);
    write(fd, "still here", 10);
    check("unlink an open file", unlink(DIR_ "/gone") == 0 && access(DIR_ "/gone", F_OK) < 0);
    lseek(fd, 0, SEEK_SET);
    check("  it stays readable until closed", read(fd, buf, 10) == 10 && !memcmp(buf, "still here", 10));
    close(fd);
}

static void directories(void) {
    char buf[256];
    struct stat st;
    printf("directories:\n");
    check("mkdir", mkdir(DIR_ "/d", 0755) == 0);
    check("mkdir again -> EEXIST", fails_with(mkdir(DIR_ "/d", 0755), EEXIST));
    check("mkdir nested", mkdir(DIR_ "/d/e", 0755) == 0 && write_file(DIR_ "/d/e/f", "deep") == 0);
    check("stat a directory", stat(DIR_ "/d", &st) == 0 && S_ISDIR(st.st_mode));
    check("open missing -> ENOENT", fails_with(open(DIR_ "/nope", O_RDONLY), ENOENT));
    check("path through a file -> ENOTDIR", fails_with(open(DIR_ "/a/x", O_RDONLY), ENOTDIR));
    check("open a directory for writing -> EISDIR", fails_with(open(DIR_ "/d", O_WRONLY), EISDIR));
    check("rmdir non-empty -> ENOTEMPTY", fails_with(rmdir(DIR_ "/d/e"), ENOTEMPTY));
    check("rmdir a file -> ENOTDIR", fails_with(rmdir(DIR_ "/a"), ENOTDIR));
    check("unlink a directory -> EISDIR", fails_with(unlink(DIR_ "/d"), EISDIR));

    DIR *dir = opendir(DIR_);
    int seen_a = 0, seen_d = 0, count = 0;
    struct dirent *e;
    while (dir && (e = readdir(dir))) {
        count++;
        if (!strcmp(e->d_name, "a") && e->d_type == DT_REG) seen_a = 1;
        if (!strcmp(e->d_name, "d") && e->d_type == DT_DIR) seen_d = 1;
    }
    if (dir) closedir(dir);
    check("readdir lists files and directories", seen_a && seen_d && count == 3);

    check("rename a file", rename(DIR_ "/a", DIR_ "/b") == 0 && access(DIR_ "/a", F_OK) < 0 &&
                           read_file(DIR_ "/b", buf, sizeof buf) == 2);
    check("rename into another directory", rename(DIR_ "/b", DIR_ "/d/b") == 0 && access(DIR_ "/d/b", F_OK) == 0);
    write_file(DIR_ "/c", "new");
    check("rename over an existing file", rename(DIR_ "/c", DIR_ "/d/b") == 0 &&
                                          read_file(DIR_ "/d/b", buf, sizeof buf) == 3 && !strcmp(buf, "new"));
    check("rename a directory into itself -> EINVAL", fails_with(rename(DIR_ "/d", DIR_ "/d/e/x"), EINVAL));
    check("rename a directory", rename(DIR_ "/d", DIR_ "/dd") == 0 &&
                                read_file(DIR_ "/dd/e/f", buf, sizeof buf) == 4);

    check("chdir and a relative path", chdir(DIR_ "/dd") == 0 && read_file("e/f", buf, sizeof buf) == 4);
    check("getcwd", getcwd(buf, sizeof buf) && !strcmp(buf, DIR_ "/dd"));
    check("chdir ..", chdir("..") == 0 && getcwd(buf, sizeof buf) && !strcmp(buf, DIR_));
    check("chdir to a file -> ENOTDIR", fails_with(chdir(DIR_ "/s"), ENOTDIR));

    check("/bin is read-only: create -> EROFS", fails_with(open("/bin/new", O_WRONLY | O_CREAT, 0644), EROFS));
    check("/bin is read-only: unlink -> EROFS", fails_with(unlink("/bin/true"), EROFS));
    check("/bin/true is executable", access("/bin/true", X_OK) == 0);
    check("/boot holds the boot image", stat("/boot/bin/shell", &st) == 0 && st.st_size > 1000);
}

static void many_and_large(void) {
    char name[64], buf[64];
    int ok = 1;
    printf("many files, a large file:\n");
    mkdir(DIR_ "/many", 0755);
    for (int i = 0; i < 200 && ok; i++) {
        snprintf(name, sizeof name, DIR_ "/many/f%d", i);
        snprintf(buf, sizeof buf, "file %d", i);
        ok = write_file(name, buf) == 0;
    }
    for (int i = 0; i < 200 && ok; i++) {
        char want[64];
        snprintf(name, sizeof name, DIR_ "/many/f%d", i);
        snprintf(want, sizeof want, "file %d", i);
        ok = read_file(name, buf, sizeof buf) > 0 && !strcmp(buf, want);
    }
    check("200 files written and read back", ok);
    int count = 0;
    DIR *d = opendir(DIR_ "/many");
    while (d && readdir(d)) count++;
    if (d) closedir(d);
    check("  readdir sees all 200", count == 200);
    for (int i = 0; i < 200; i++) {
        snprintf(name, sizeof name, DIR_ "/many/f%d", i);
        unlink(name);
    }
    check("  removed, directory empty", rmdir(DIR_ "/many") == 0);

    enum { SIZE = 1 << 20 };
    char *big = malloc(SIZE), *back = malloc(SIZE);
    for (int i = 0; i < SIZE; i++) big[i] = (char)(i * 7 + i / 4096);
    int fd = open(DIR_ "/big", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check("write 1 MiB", fd >= 0 && write(fd, big, SIZE) == SIZE);
    close(fd);
    fd = open(DIR_ "/big", O_RDONLY);
    size_t got = 0;
    ssize_t k;
    while (got < SIZE && (k = read(fd, back + got, SIZE - got)) > 0) got += (size_t)k;
    close(fd);
    check("  read back identical", got == SIZE && !memcmp(big, back, SIZE));
    unlink(DIR_ "/big");
    free(big);
    free(back);

    FILE *f = fopen(DIR_ "/stdio", "w");
    check("fopen / fprintf", f && fprintf(f, "line %d\nline %d\n", 1, 2) == 14 && fclose(f) == 0);
    f = fopen(DIR_ "/stdio", "r");
    char line[32];
    check("fgets", f && fgets(line, sizeof line, f) && !strcmp(line, "line 1\n"));
    check("ftell", f && ftell(f) == 7);
    check("fseek + fgets", f && fseek(f, 0, SEEK_SET) == 0 && fgets(line, sizeof line, f) && !strcmp(line, "line 1\n"));
    if (f) fclose(f);
}

// Run `argv` with stdout going to `fd`; returns its exit code
static int run_with_stdout(char **argv, int fd) {
    int saved = dup(1);
    dup2(fd, 1);
    rx_handle_t proc;
    rx_status_t r = rx_spawn(argv[0], argv, 0, RX_SPAWN_CLONE_ALL, 0, 0, &proc);
    dup2(saved, 1);
    close(saved);
    return r == RX_OK ? (int)rx_wait_process(proc) : -1;
}

static void two_processes(void) {
    char buf[256];
    printf("files shared with another process:\n");
    int fd = open(DIR_ "/shared", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    write(fd, "parent ", 7);
    char *argv[] = { "/bin/echo", "child", 0 };
    check("a child writes to our open file", run_with_stdout(argv, fd) == 0);
    check("  the offset is shared", lseek(fd, 0, SEEK_CUR) == 13);
    write(fd, "parent again\n", 13);
    close(fd);
    check("  both writes in order", read_file(DIR_ "/shared", buf, sizeof buf) == 26 &&
                                    !strcmp(buf, "parent child\nparent again\n"));
    char *cat[] = { "/bin/cat", DIR_ "/shared", 0 };
    int p[2];
    pipe(p);
    int code = run_with_stdout(cat, p[1]);
    close(p[1]);
    ssize_t n = read(p[0], buf, sizeof buf - 1);
    close(p[0]);
    check("a child reads the file into a pipe", code == 0 && n == 26 && !memcmp(buf, "parent child", 12));
}

// Child side of the slow-reader test: read stdin slowly, check the bytes
static int slow_reader(void) {
    char buf[4096];
    long total = 0;
    int ok = 1;
    ssize_t n;
    while ((n = read(0, buf, sizeof buf)) > 0) {
        for (ssize_t i = 0; i < n; i++) if (buf[i] != (char)((total + i) % 251)) ok = 0;
        total += n;
        usleep(1000);
    }
    return ok && total == PIPE_BYTES ? 0 : 1;
}

static void pipes(void) {
    printf("pipes:\n");
    int p[2];
    char buf[64];
    check("pipe", pipe(p) == 0);
    check("write then read", write(p[1], "abc", 3) == 3 && read(p[0], buf, sizeof buf) == 3 && !memcmp(buf, "abc", 3));
    close(p[1]);
    check("end of input when the writer closes", read(p[0], buf, sizeof buf) == 0);
    close(p[0]);
    pipe(p);
    close(p[0]);
    check("write with no reader -> EPIPE", fails_with((int)write(p[1], "x", 1), EPIPE));
    close(p[1]);

    // A slow reader in another process: the writer must wait for it
    pipe(p);
    int saved = dup(0);
    dup2(p[0], 0);
    close(p[0]);
    char *argv[] = { "/bin/fstest", "--slow-reader", 0 };
    rx_handle_t proc;
    rx_status_t r = rx_spawn(argv[0], argv, 0, RX_SPAWN_CLONE_ALL, 0, 0, &proc);
    dup2(saved, 0);
    close(saved);
    char *data = malloc(PIPE_BYTES);
    for (int i = 0; i < PIPE_BYTES; i++) data[i] = (char)(i % 251);
    uint64_t t0 = rx_clock_get_monotonic();
    ssize_t w = r == RX_OK ? write(p[1], data, PIPE_BYTES) : -1;
    uint64_t ms = (rx_clock_get_monotonic() - t0) / 1000000;
    close(p[1]);
    free(data);
    check("300 KiB into a pipe to a slow reader", w == PIPE_BYTES);
    check("  the writer waited for the reader", ms >= 20);
    check("  the reader got every byte in order", r == RX_OK && rx_wait_process(proc) == 0);
}

static void cleanup(const char *path) {
    struct stat st;
    if (stat(path, &st) < 0) return;
    if (S_ISDIR(st.st_mode)) {
        DIR *d;
        struct dirent *e;
        char child[512];
        while ((d = opendir(path)) && (e = readdir(d))) {
            snprintf(child, sizeof child, "%s/%s", path, e->d_name);
            closedir(d);
            cleanup(child);
        }
        if (d) closedir(d);
        rmdir(path);
    } else {
        unlink(path);
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "--slow-reader")) return slow_reader();
    printf("fstest: files, directories and pipes\n");
    char start[256];
    getcwd(start, sizeof start);
    cleanup(DIR_);
    if (mkdir(DIR_, 0755) < 0) {
        printf("fstest: cannot create " DIR_ ": %s\n", strerror(errno));
        return 1;
    }
    basics();
    directories();
    many_and_large();
    two_processes();
    pipes();
    chdir(start);
    cleanup(DIR_);
    check("clean up", access(DIR_, F_OK) < 0);
    printf("fstest: %d passed, %d failed\n", passed, failed);
    return failed;
}

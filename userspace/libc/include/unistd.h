// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _UNISTD_H
#define _UNISTD_H

#include <sys/types.h>

#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

ssize_t read(int fd, void *buf, size_t count);
ssize_t write(int fd, const void *buf, size_t count);
int close(int fd);
int dup(int fd);
int dup2(int fd, int fd2);
int isatty(int fd);
void _exit(int status) __attribute__((noreturn));
off_t lseek(int fd, off_t offset, int whence);
int pipe(int fds[2]);
int unlink(const char *path);
int rmdir(const char *path);
int chdir(const char *path);
char *getcwd(char *buf, size_t size);
int access(const char *path, int mode);
int ftruncate(int fd, off_t length);
int truncate(const char *path, off_t length);
int fsync(int fd);
int gethostname(char *name, size_t len);
pid_t getpid(void);
pid_t getppid(void);
pid_t gettid(void);
int chown(const char *path, uid_t u, gid_t g);
int lchown(const char *path, uid_t u, gid_t g);
int fchown(int fd, uid_t u, gid_t g);
char *getlogin(void);
int getgroups(int n, gid_t list[]);
uid_t getuid(void);
uid_t geteuid(void);
gid_t getgid(void);
gid_t getegid(void);
int setuid(uid_t u);
int setgid(gid_t g);
pid_t setsid(void);
int setpgid(pid_t p, pid_t g);
pid_t fork(void);
int execve(const char *path, char *const argv[], char *const envp[]);
int execv(const char *path, char *const argv[]);
int execvp(const char *file, char *const argv[]);
int pipe2(int fds[2], int flags);
ssize_t pread(int fd, void *buf, size_t n, off_t off);
ssize_t pwrite(int fd, const void *buf, size_t n, off_t off);
ssize_t readlink(const char *restrict path, char *restrict buf, size_t size);
int fdatasync(int fd);
void sync(void);
long sysconf(int name);
int getpagesize(void);
int chroot(const char *path);
int pause(void);
int sched_yield(void);
int symlink(const char *target, const char *link);
int link(const char *old, const char *new);
int fchdir(int fd);
int faccessat(int dirfd, const char *path, int mode, int flags);
unsigned int alarm(unsigned int seconds);

extern char *optarg;
extern int optind, opterr, optopt;
int getopt(int argc, char *const argv[], const char *optstring);

#define _SC_CLK_TCK 2
#define _SC_OPEN_MAX 4
#define _SC_PAGESIZE 30
#define _SC_PAGE_SIZE 30
#define _SC_GETPW_R_SIZE_MAX 70
#define _SC_NPROCESSORS_CONF 83
#define _SC_NPROCESSORS_ONLN 84
#define _SC_PHYS_PAGES 85

#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

#define F_OK 0
#define X_OK 1
#define W_OK 2
#define R_OK 4
unsigned int sleep(unsigned int seconds);
int usleep(unsigned long usec);

#endif

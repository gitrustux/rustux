// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// More of POSIX, as far as Rustux can give it: memory mappings, poll,
// scatter/gather I/O, fcntl and ioctl, terminal modes, system
// configuration, random bytes, getopt, and calls that Unix programs (and
// Rust's std, which is built for Linux's ABI) expect even where Rustux
// has nothing behind them: signals, user ids, fork/exec, dynamic loading.

#include "libc.h"
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <getopt.h>
#include <poll.h>
#include <rustux/console.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <sys/uio.h>
#include <termios.h>
#include <unistd.h>

static int fail(int e) {
    errno = e;
    return -1;
}

// ---- memory mappings (anonymous only) ----

void *mmap(void *addr, size_t len, int prot, int flags, int fd, off_t off) {
    (void)off;
    if (!(flags & MAP_ANONYMOUS) || fd != -1) {
        errno = ENODEV; // no files mapped yet
        return MAP_FAILED;
    }
    if (!len) {
        errno = EINVAL;
        return MAP_FAILED;
    }
    uint64_t size = (len + 4095) & ~4095ull;
    rx_handle_t vmo;
    rx_status_t r = rx_vmo_create(size, 0, &vmo);
    if (r != RX_OK) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    uint32_t perm = RX_VM_PERM_READ | RX_VM_PERM_WRITE; // mapped writable; mprotect narrows it
    uint64_t offset = 0, out = 0;
    if (flags & MAP_FIXED) {
        if ((uint64_t)addr < RX_ROOT_VMAR_BASE) {
            rx_handle_close(vmo);
            errno = EINVAL;
            return MAP_FAILED;
        }
        rx_vmar_unmap(rx_vmar_root, (uint64_t)addr, size);
        perm |= RX_VM_SPECIFIC;
        offset = (uint64_t)addr - RX_ROOT_VMAR_BASE;
    }
    r = rx_vmar_map(rx_vmar_root, perm, offset, vmo, 0, size, &out);
    rx_handle_close(vmo);
    if (r != RX_OK) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    if (prot != (PROT_READ | PROT_WRITE)) mprotect((void *)out, size, prot);
    return (void *)out;
}

int munmap(void *addr, size_t len) {
    if ((uint64_t)addr & 4095) return fail(EINVAL);
    rx_status_t r = rx_vmar_unmap(rx_vmar_root, (uint64_t)addr, (len + 4095) & ~4095ull);
    return r == RX_OK ? 0 : fail(EINVAL);
}

int mprotect(void *addr, size_t len, int prot) {
    uint32_t p = 0;
    if (prot & PROT_READ) p |= RX_VM_PERM_READ;
    if (prot & PROT_WRITE) p |= RX_VM_PERM_WRITE | RX_VM_PERM_READ;
    if (prot & PROT_EXEC) p |= RX_VM_PERM_EXECUTE | RX_VM_PERM_READ;
    rx_status_t r = rx_vmar_protect(rx_vmar_root, p, (uint64_t)addr & ~4095ull, (len + 4095) & ~4095ull);
    // Guard pages (PROT_NONE) are accepted but not enforced where the
    // kernel can't remove every permission
    if (r != RX_OK && prot == PROT_NONE) return 0;
    return r == RX_OK ? 0 : fail(r == RX_ERR_ACCESS_DENIED ? EACCES : EINVAL);
}

// ---- poll ----

int poll(struct pollfd *fds, nfds_t n, int timeout_ms) {
    rx_wait_item_t items[64];
    int map[64];
    int ready = 0, m = 0;
    if (n > 64) return fail(EINVAL);
    for (nfds_t i = 0; i < n; i++) {
        fds[i].revents = 0;
        if (fds[i].fd < 0) continue;
        struct fd *f = fds[i].fd < MAX_FDS ? &__fds[fds[i].fd] : 0;
        if (!f || f->kind == RX_FD_NONE) {
            fds[i].revents = POLLNVAL;
            ready++;
            continue;
        }
        switch (f->kind) {
        case RX_FD_CONSOLE:
            if ((fds[i].events & POLLIN) && f->rpos < f->rlen) fds[i].revents |= POLLIN;
            if (fds[i].events & POLLOUT) fds[i].revents |= POLLOUT;
            if (!fds[i].revents && (fds[i].events & POLLIN)) {
                items[m].handle = f->handle;
                items[m].waitfor = RX_CHANNEL_READABLE | RX_PEER_CLOSED;
                map[m++] = (int)i;
            }
            break;
        case RX_FD_PIPE:
            items[m].handle = f->handle;
            items[m].waitfor = RX_PEER_CLOSED | ((fds[i].events & POLLIN) ? RX_CHANNEL_READABLE : 0) |
                               ((fds[i].events & POLLOUT) ? RX_CHANNEL_WRITABLE : 0);
            map[m++] = (int)i;
            break;
        default: // files and the log are always ready
            fds[i].revents = fds[i].events & (POLLIN | POLLOUT);
            break;
        }
        if (fds[i].revents) ready++;
    }
    if (m == 0) return ready;
    uint64_t deadline = ready || timeout_ms == 0 ? 0
                        : timeout_ms < 0 ? RX_TIME_INFINITE
                        : rx_clock_get_monotonic() + (uint64_t)timeout_ms * 1000000ull;
    for (int k = 0; k < m; k++) items[k].pending = 0;
    rx_status_t r = rx_object_wait_many(items, (uint64_t)m, deadline);
    // On a timeout the kernel still reports what is pending
    if (r != RX_OK && r != RX_ERR_TIMED_OUT) return fail(EINVAL);
    for (int k = 0; k < m; k++) {
        struct pollfd *p = &fds[map[k]];
        short before = p->revents;
        uint32_t s = items[k].pending;
        if ((s & RX_CHANNEL_READABLE) && (p->events & POLLIN)) p->revents |= POLLIN;
        if ((s & RX_CHANNEL_WRITABLE) && (p->events & POLLOUT)) p->revents |= POLLOUT;
        if (s & RX_PEER_CLOSED) p->revents |= (p->events & POLLOUT) ? POLLERR | POLLHUP : POLLHUP;
        if (!before && p->revents) ready++;
    }
    return ready;
}

// ---- scatter/gather and positioned I/O ----

ssize_t writev(int fd, const struct iovec *iov, int n) {
    ssize_t total = 0;
    for (int i = 0; i < n; i++) {
        if (!iov[i].iov_len) continue;
        ssize_t k = write(fd, iov[i].iov_base, iov[i].iov_len);
        if (k < 0) return total ? total : -1;
        total += k;
        if ((size_t)k < iov[i].iov_len) break;
    }
    return total;
}

ssize_t readv(int fd, const struct iovec *iov, int n) {
    ssize_t total = 0;
    for (int i = 0; i < n; i++) {
        if (!iov[i].iov_len) continue;
        ssize_t k = read(fd, iov[i].iov_base, iov[i].iov_len);
        if (k < 0) return total ? total : -1;
        total += k;
        if ((size_t)k < iov[i].iov_len) break;
    }
    return total;
}

// Not atomic with respect to other users of the same open file
ssize_t pread(int fd, void *buf, size_t n, off_t off) {
    off_t old = lseek(fd, 0, SEEK_CUR);
    if (old < 0 || lseek(fd, off, SEEK_SET) < 0) return -1;
    ssize_t k = read(fd, buf, n);
    int e = errno;
    lseek(fd, old, SEEK_SET);
    errno = e;
    return k;
}

ssize_t pwrite(int fd, const void *buf, size_t n, off_t off) {
    off_t old = lseek(fd, 0, SEEK_CUR);
    if (old < 0 || lseek(fd, off, SEEK_SET) < 0) return -1;
    ssize_t k = write(fd, buf, n);
    int e = errno;
    lseek(fd, old, SEEK_SET);
    errno = e;
    return k;
}

int fdatasync(int fd) { return fsync(fd); }

int pipe2(int fds[2], int flags) {
    if (pipe(fds) < 0) return -1;
    for (int i = 0; i < 2; i++) {
        __fds[fds[i]].cloexec = (flags & O_CLOEXEC) != 0;
        __fds[fds[i]].flags = (i ? O_WRONLY : O_RDONLY) | (flags & O_NONBLOCK);
    }
    return 0;
}

// ---- fcntl and ioctl ----

int fcntl(int fd, int cmd, ...) {
    va_list ap;
    va_start(ap, cmd);
    long arg = va_arg(ap, long);
    va_end(ap);
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    switch (cmd) {
    case F_GETFD: return f->cloexec ? FD_CLOEXEC : 0;
    case F_SETFD: f->cloexec = (arg & FD_CLOEXEC) != 0; return 0;
    case F_GETFL: return f->flags;
    case F_SETFL: f->flags = (f->flags & O_ACCMODE) | ((int)arg & (O_APPEND | O_NONBLOCK)); return 0;
    case F_DUPFD:
    case F_DUPFD_CLOEXEC: {
        int nfd = __fd_alloc((int)arg);
        if (nfd < 0) return -1;
        if (dup2(fd, nfd) < 0) return -1;
        __fds[nfd].cloexec = cmd == F_DUPFD_CLOEXEC;
        return nfd;
    }
    default: return fail(EINVAL);
    }
}

// The console's modes; programs start in cooked mode (the shell sets it
// before starting a program)
static struct termios tty = {
    .c_iflag = ICRNL | IXON | IUTF8,
    .c_oflag = OPOST | ONLCR,
    .c_cflag = CS8 | CREAD | B38400,
    .c_lflag = ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
    .c_cc = { [VINTR] = 3, [VQUIT] = 28, [VERASE] = 127, [VKILL] = 21, [VEOF] = 4, [VMIN] = 1, [VSUSP] = 26 },
    .__c_ispeed = B38400,
    .__c_ospeed = B38400,
};
unsigned __tty_lflag = ISIG | ICANON | ECHO, __tty_vmin = 1, __tty_vtime = 0;

static struct fd *console(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return 0;
    if (f->kind != RX_FD_CONSOLE) {
        errno = ENOTTY;
        return 0;
    }
    return f;
}

int tcgetattr(int fd, struct termios *t) {
    if (!console(fd)) return -1;
    *t = tty;
    return 0;
}

int tcsetattr(int fd, int when, const struct termios *t) {
    (void)when;
    struct fd *f = console(fd);
    if (!f) return -1;
    uint32_t msg[3] = { RX_CONSOLE_OP_SET_MODE, t->c_lflag & (ISIG | ICANON | ECHO), t->c_iflag & ICRNL };
    rx_status_t r = rx_channel_write(f->handle, 0, msg, sizeof msg, 0, 0);
    if (r != RX_OK) return __set_errno_status(r);
    tty = *t;
    __tty_lflag = t->c_lflag;
    __tty_vmin = t->c_cc[VMIN];
    __tty_vtime = t->c_cc[VTIME];
    return 0;
}

void cfmakeraw(struct termios *t) {
    t->c_iflag &= ~(unsigned)(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t->c_oflag &= ~(unsigned)OPOST;
    t->c_lflag &= ~(unsigned)(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    t->c_cflag &= ~(unsigned)(CSIZE | PARENB);
    t->c_cflag |= CS8;
    t->c_cc[VMIN] = 1;
    t->c_cc[VTIME] = 0;
}

speed_t cfgetospeed(const struct termios *t) { return t->__c_ospeed; }
speed_t cfgetispeed(const struct termios *t) { return t->__c_ispeed; }
int cfsetospeed(struct termios *t, speed_t s) { t->__c_ospeed = s; return 0; }
int cfsetispeed(struct termios *t, speed_t s) { t->__c_ispeed = s; return 0; }

int tcflush(int fd, int queue) {
    (void)queue;
    struct fd *f = console(fd);
    if (!f) return -1;
    f->rpos = f->rlen = 0;
    return 0;
}

static int winsize(struct winsize *w) {
    rx_system_stats_t s;
    memset(&s, 0, sizeof s);
    rx_system_get_stats(&s);
    w->ws_col = (unsigned short)(s.console_cols ? s.console_cols : 80);
    w->ws_row = (unsigned short)(s.console_rows ? s.console_rows : 25);
    w->ws_xpixel = w->ws_ypixel = 0;
    return 0;
}

int ioctl(int fd, unsigned long req, ...) {
    va_list ap;
    va_start(ap, req);
    void *arg = va_arg(ap, void *);
    va_end(ap);
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    switch (req) {
    case FIOCLEX: f->cloexec = 1; return 0;
    case FIONCLEX: f->cloexec = 0; return 0;
    case FIONBIO:
        if (*(int *)arg) f->flags |= O_NONBLOCK;
        else f->flags &= ~O_NONBLOCK;
        return 0;
    case TCGETS: return tcgetattr(fd, arg);
    case TCSETS: case TCSETSW: case TCSETSF: return tcsetattr(fd, TCSANOW, arg);
    case TIOCGWINSZ:
        if (!console(fd)) return -1;
        return winsize(arg);
    default:
        return fail(f->kind == RX_FD_CONSOLE ? EINVAL : ENOTTY);
    }
}

// ---- system configuration ----

long sysconf(int name) {
    rx_system_stats_t s;
    switch (name) {
    case _SC_PAGESIZE: return 4096;
    case _SC_CLK_TCK: return 100;
    case _SC_OPEN_MAX: return MAX_FDS;
    case _SC_NPROCESSORS_CONF:
    case _SC_NPROCESSORS_ONLN:
        return rx_system_get_stats(&s) == RX_OK ? (long)s.cpus : 1;
    case _SC_PHYS_PAGES:
        return rx_system_get_stats(&s) == RX_OK ? (long)(s.total_bytes / 4096) : -1;
    case _SC_GETPW_R_SIZE_MAX: return -1;
    default: errno = EINVAL; return -1;
    }
}

int getpagesize(void) { return 4096; }

// No auxiliary vector: the page size, everything else 0
unsigned long getauxval(unsigned long type) {
    if (type == 6) return 4096; // AT_PAGESZ
    errno = ENOENT;
    return 0;
}

// ---- the program's own headers (for unwinders and the like) ----

struct dl_phdr_info {
    uint64_t dlpi_addr;
    const char *dlpi_name;
    const void *dlpi_phdr;
    uint16_t dlpi_phnum;
    unsigned long long dlpi_adds, dlpi_subs;
    size_t dlpi_tls_modid;
    void *dlpi_tls_data;
};
const void *__program_headers(size_t *n);

int dl_iterate_phdr(int (*cb)(struct dl_phdr_info *, size_t, void *), void *data) {
    size_t n;
    const void *ph = __program_headers(&n);
    if (!ph) return 0;
    struct dl_phdr_info info = { 0, "", ph, (uint16_t)n, 1, 0, 0, 0 };
    return cb(&info, sizeof info, data);
}

int _dl_find_object(void *pc, void *result) {
    (void)pc;
    (void)result;
    return -1;
}

// No dynamic loading
void *dlsym(void *restrict handle, const char *restrict name) { (void)handle; (void)name; return 0; }
void *dlopen(const char *path, int flags) { (void)path; (void)flags; return 0; }
int dlclose(void *handle) { (void)handle; return 0; }
char *dlerror(void) { return "dynamic loading is not supported"; }

// ---- random bytes ----

static uint64_t mix(uint64_t x) {
    x += 0x9E3779B97F4A7C15ull;
    x = (x ^ (x >> 30)) * 0xBF58476D1CE4E5B9ull;
    x = (x ^ (x >> 27)) * 0x94D049BB133111EBull;
    return x ^ (x >> 31);
}

static int has_rdrand(void) {
    static int known = -1;
    if (known < 0) {
        unsigned a = 1, b, c, d;
        __asm__ volatile("cpuid" : "+a"(a), "=b"(b), "=c"(c), "=d"(d));
        known = (c >> 30) & 1;
    }
    return known;
}

// RDRAND where the CPU has it, mixed with the time stamp counter and the
// clocks (no kernel entropy source yet)
ssize_t getrandom(void *buf, size_t len, unsigned flags) {
    (void)flags;
    static uint64_t state;
    unsigned char *p = buf;
    uint64_t lo, hi;
    for (size_t i = 0; i < len;) {
        __asm__ volatile("rdtsc" : "=a"(lo), "=d"(hi));
        uint64_t v = mix(state ^ (hi << 32 | lo) ^ rx_clock_get_monotonic() ^ (uint64_t)rx_clock_get_utc());
        if (has_rdrand()) {
            uint64_t r = 0;
            unsigned char ok = 0;
            for (int t = 0; t < 10 && !ok; t++) __asm__ volatile("rdrand %0; setc %1" : "=r"(r), "=qm"(ok));
            v ^= r;
        }
        state = v;
        for (int k = 0; k < 8 && i < len; k++, i++) p[i] = (unsigned char)(v >> (8 * k));
    }
    return (ssize_t)len;
}

int getentropy(void *buf, size_t len) {
    if (len > 256) return fail(EIO);
    getrandom(buf, len, 0);
    return 0;
}

// ---- signals: recorded, never delivered ----

static struct sigaction actions[NSIG];
static stack_t altstack = { 0, SS_DISABLE, 0 };

int sigaction(int sig, const struct sigaction *restrict act, struct sigaction *restrict old) {
    if (sig <= 0 || sig >= NSIG) return fail(EINVAL);
    if (old) *old = actions[sig];
    if (act) {
        if (sig == SIGKILL || sig == SIGSTOP) return fail(EINVAL);
        actions[sig] = *act;
    }
    return 0;
}

sighandler_t signal(int sig, sighandler_t handler) {
    struct sigaction a, old;
    memset(&a, 0, sizeof a);
    a.sa_handler = handler;
    if (sigaction(sig, &a, &old) < 0) return SIG_ERR;
    return old.sa_handler;
}

int sigaltstack(const stack_t *restrict ss, stack_t *restrict old) {
    if (old) *old = altstack;
    if (ss) altstack = *ss;
    return 0;
}

int sigemptyset(sigset_t *s) { memset(s, 0, sizeof *s); return 0; }
int sigfillset(sigset_t *s) { memset(s, 0xFF, sizeof *s); return 0; }
int sigaddset(sigset_t *s, int sig) {
    if (sig <= 0 || sig >= NSIG) return fail(EINVAL);
    s->__bits[(sig - 1) / (8 * sizeof(long))] |= 1ul << ((sig - 1) % (8 * sizeof(long)));
    return 0;
}
int sigdelset(sigset_t *s, int sig) {
    if (sig <= 0 || sig >= NSIG) return fail(EINVAL);
    s->__bits[(sig - 1) / (8 * sizeof(long))] &= ~(1ul << ((sig - 1) % (8 * sizeof(long))));
    return 0;
}
int sigismember(const sigset_t *s, int sig) {
    if (sig <= 0 || sig >= NSIG) return fail(EINVAL);
    return (s->__bits[(sig - 1) / (8 * sizeof(long))] >> ((sig - 1) % (8 * sizeof(long)))) & 1;
}
int sigprocmask(int how, const sigset_t *restrict set, sigset_t *restrict old) {
    (void)how; (void)set;
    if (old) sigemptyset(old);
    return 0;
}
int pthread_sigmask(int how, const sigset_t *restrict set, sigset_t *restrict old) {
    sigprocmask(how, set, old);
    return 0;
}

// A signal sent to ourselves ends the program (as the default action of
// most signals does), unless it is ignored
int raise(int sig) {
    if (sig <= 0 || sig >= NSIG) return fail(EINVAL);
    if (actions[sig].sa_handler == SIG_IGN || sig == SIGCHLD || sig == SIGWINCH || sig == SIGCONT) return 0;
    if (actions[sig].sa_handler != SIG_DFL) {
        actions[sig].sa_handler(sig);
        return 0;
    }
    _exit(128 + sig);
}

int pause(void) {
    rx_handle_t ev;
    if (rx_event_create(0, &ev) == RX_OK) rx_object_wait_one(ev, RX_EVENT_SIGNALED, RX_TIME_INFINITE, 0);
    return fail(EINTR);
}

unsigned int alarm(unsigned int seconds) { (void)seconds; return 0; }

// ---- users and sessions (one user, root, until step 17) ----

uid_t getuid(void) { return 0; }
uid_t geteuid(void) { return 0; }
gid_t getgid(void) { return 0; }
gid_t getegid(void) { return 0; }
int setuid(uid_t u) { return u == 0 ? 0 : fail(EPERM); }
int setgid(gid_t g) { return g == 0 ? 0 : fail(EPERM); }
int setgroups(size_t n, const gid_t *list) { (void)n; (void)list; return 0; }
pid_t setsid(void) { return getpid(); }
int setpgid(pid_t p, pid_t g) { (void)p; (void)g; return 0; }
pid_t getppid(void) { return 0; }
int chroot(const char *path) { (void)path; return fail(EPERM); }

// Programs are started with posix_spawn (or rx_spawn): no fork, no exec
pid_t fork(void) { return fail(ENOSYS); }
int execve(const char *path, char *const argv[], char *const envp[]) { (void)path; (void)argv; (void)envp; return fail(ENOSYS); }
int execv(const char *path, char *const argv[]) { return execve(path, argv, environ); }
int execvp(const char *file, char *const argv[]) { return execve(file, argv, environ); }

// No sockets yet (networking is later)
int socketpair(int domain, int type, int protocol, int fds[2]) {
    (void)domain; (void)type; (void)protocol; (void)fds;
    return fail(EAFNOSUPPORT);
}
ssize_t sendmsg(int fd, const void *msg, int flags) { (void)fd; (void)msg; (void)flags; return fail(ENOTSOCK); }
ssize_t recvmsg(int fd, void *msg, int flags) { (void)fd; (void)msg; (void)flags; return fail(ENOTSOCK); }
ssize_t recv(int fd, void *buf, size_t n, int flags) { (void)fd; (void)buf; (void)n; (void)flags; return fail(ENOTSOCK); }
ssize_t send(int fd, const void *buf, size_t n, int flags) { (void)fd; (void)buf; (void)n; (void)flags; return fail(ENOTSOCK); }

// ---- getopt ----

char *optarg;
int optind = 1, opterr = 1, optopt;
static int optpos = 1;

int getopt(int argc, char *const argv[], const char *opts) {
    if (optind == 0) { optind = 1; optpos = 1; } // reset
    if (optind >= argc || !argv[optind] || argv[optind][0] != '-' || !argv[optind][1]) return -1;
    if (!strcmp(argv[optind], "--")) {
        optind++;
        return -1;
    }
    int quiet = opts[0] == ':';
    char c = argv[optind][optpos];
    const char *o = c == ':' ? 0 : strchr(opts, c);
    optopt = c;
    if (!o) {
        if (opterr && !quiet) fprintf(stderr, "%s: unknown option -%c\n", argv[0], c);
        if (!argv[optind][++optpos]) { optind++; optpos = 1; }
        return '?';
    }
    if (o[1] == ':') {
        if (argv[optind][optpos + 1]) {
            optarg = &argv[optind][optpos + 1];
        } else if (optind + 1 < argc) {
            optarg = argv[++optind];
        } else {
            optind++;
            optpos = 1;
            if (opterr && !quiet) fprintf(stderr, "%s: option -%c needs a value\n", argv[0], c);
            return quiet ? ':' : '?';
        }
        optind++;
        optpos = 1;
        return c;
    }
    if (!argv[optind][++optpos]) { optind++; optpos = 1; }
    return c;
}

// Long options: --name, --name=value, --name value
int getopt_long(int argc, char *const argv[], const char *opts, const struct option *longopts, int *index) {
    if (optind == 0) { optind = 1; optpos = 1; }
    if (optpos == 1 && optind < argc && argv[optind] && !strncmp(argv[optind], "--", 2) && argv[optind][2]) {
        const char *name = argv[optind] + 2, *eq = strchr(name, '=');
        size_t len = eq ? (size_t)(eq - name) : strlen(name);
        for (int i = 0; longopts && longopts[i].name; i++) {
            if (strlen(longopts[i].name) != len || strncmp(longopts[i].name, name, len)) continue;
            optind++;
            optarg = 0;
            if (longopts[i].has_arg == required_argument) {
                if (eq) optarg = (char *)eq + 1;
                else if (optind < argc) optarg = argv[optind++];
                else {
                    if (opterr) fprintf(stderr, "%s: option --%s needs a value\n", argv[0], longopts[i].name);
                    return '?';
                }
            } else if (longopts[i].has_arg == optional_argument && eq) {
                optarg = (char *)eq + 1;
            }
            if (index) *index = i;
            if (longopts[i].flag) {
                *longopts[i].flag = longopts[i].val;
                return 0;
            }
            return longopts[i].val;
        }
        if (opterr) fprintf(stderr, "%s: unknown option --%.*s\n", argv[0], (int)len, name);
        optind++;
        return '?';
    }
    return getopt(argc, argv, opts);
}

// ---- strings ----

int bcmp(const void *a, const void *b, size_t n) { return memcmp(a, b, n); }

void *memrchr(const void *s, int c, size_t n) {
    const unsigned char *p = s;
    while (n--) if (p[n] == (unsigned char)c) return (void *)(p + n);
    return 0;
}

char *stpcpy(char *restrict d, const char *restrict s) {
    while ((*d = *s++)) d++;
    return d;
}

char *strsignal(int sig) {
    static const char *const names[] = {
        [SIGHUP] = "Hangup", [SIGINT] = "Interrupt", [SIGQUIT] = "Quit", [SIGILL] = "Illegal instruction",
        [SIGTRAP] = "Trace trap", [SIGABRT] = "Aborted", [SIGBUS] = "Bus error",
        [SIGFPE] = "Floating point exception", [SIGKILL] = "Killed", [SIGUSR1] = "User signal 1",
        [SIGSEGV] = "Segmentation fault", [SIGUSR2] = "User signal 2", [SIGPIPE] = "Broken pipe",
        [SIGALRM] = "Alarm clock", [SIGTERM] = "Terminated",
    };
    if (sig > 0 && sig < (int)(sizeof names / sizeof *names) && names[sig]) return (char *)names[sig];
    return "Unknown signal";
}

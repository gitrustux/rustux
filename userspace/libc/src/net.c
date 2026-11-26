// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Network sockets (network step D2, docs/NETWORK.md): each socket is a
// connection to the network service, netd (/svc/netsock), speaking
// <rustux/netsock.h>. A connected TCP socket's bytes go through a kernel
// socket pair (the fd's handle), so read, write and poll on it are the
// kernel's, as for a pipe; UDP datagrams are messages on the connection.
// Messages that arrive while the library waits for another (datagrams, new
// connections, errors) are kept in the socket's queue.

#include "libc.h"
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <rustux/netsock.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#define O_NONBLOCK_ 04000

enum { S_OPEN, S_BOUND, S_LISTEN, S_CONNECTING, S_CONNECTED };

struct qmsg {
    struct qmsg *next;
    rx_netsock_msg_t m;
    uint32_t len;       // data bytes
    rx_handle_t h[2];
    uint32_t nh;
    char data[];
};

struct sock {
    volatile int lock;
    int refs;
    int type;            // SOCK_STREAM, SOCK_DGRAM
    int domain;          // AF_INET, AF_INET6
    rx_handle_t ctl;
    int state;
    int err;             // pending error (SO_ERROR), an errno
    int shut_rd, shut_wr, eof;
    rx_netaddr_t local, peer;
    int has_local, has_peer;
    rx_handle_t connecting;   // our end of the data socket while connecting
    struct qmsg *q;
    int reuse, keepalive, nodelay, broadcast;
    uint64_t rcvtimeo, sndtimeo;  // ns; 0: none
    char *peekbuf;       // MSG_PEEK on a stream: bytes read ahead
    uint32_t peeklen;
};

static int fail(int e) {
    errno = e;
    return -1;
}

// ---- byte order and addresses ----

uint16_t htons(uint16_t v) { return (uint16_t)((v << 8) | (v >> 8)); }
uint16_t ntohs(uint16_t v) { return htons(v); }
uint32_t htonl(uint32_t v) { return __builtin_bswap32(v); }
uint32_t ntohl(uint32_t v) { return __builtin_bswap32(v); }

const struct in6_addr in6addr_any = { { { 0 } } };
const struct in6_addr in6addr_loopback = { { { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1 } } };

int inet_aton(const char *s, struct in_addr *a) {
    uint32_t parts[4];
    int n = 0;
    const char *p = s;
    while (n < 4) {
        if (*p < '0' || *p > '9') return 0;
        uint64_t v = 0;
        int base = 10;
        if (p[0] == '0' && (p[1] == 'x' || p[1] == 'X')) { base = 16; p += 2; }
        else if (p[0] == '0' && p[1] >= '0' && p[1] <= '9') base = 8;
        const char *start = p;
        for (;; p++) {
            int d;
            if (*p >= '0' && *p <= '9') d = *p - '0';
            else if (base == 16 && *p >= 'a' && *p <= 'f') d = *p - 'a' + 10;
            else if (base == 16 && *p >= 'A' && *p <= 'F') d = *p - 'A' + 10;
            else break;
            if (d >= base) return 0;
            v = v * (uint64_t)base + (uint64_t)d;
            if (v > 0xffffffffu) return 0;
        }
        if (p == start && base == 16) return 0;
        parts[n++] = (uint32_t)v;
        if (*p != '.') break;
        p++;
    }
    if (*p) return 0;
    uint32_t r;
    switch (n) {
    case 1: r = parts[0]; break;
    case 2: if (parts[0] > 255 || parts[1] > 0xffffff) return 0; r = parts[0] << 24 | parts[1]; break;
    case 3: if (parts[0] > 255 || parts[1] > 255 || parts[2] > 0xffff) return 0; r = parts[0] << 24 | parts[1] << 16 | parts[2]; break;
    case 4:
        for (int i = 0; i < 4; i++) if (parts[i] > 255) return 0;
        r = parts[0] << 24 | parts[1] << 16 | parts[2] << 8 | parts[3];
        break;
    default: return 0;
    }
    if (a) a->s_addr = htonl(r);
    return 1;
}

in_addr_t inet_addr(const char *s) {
    struct in_addr a;
    return inet_aton(s, &a) ? a.s_addr : INADDR_NONE;
}

char *inet_ntoa(struct in_addr a) {
    static char buf[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &a, buf, sizeof buf);
    return buf;
}

static int pton4(const char *s, uint8_t out[4]) {
    // Exactly four decimal parts, as inet_pton wants
    int n = 0;
    while (n < 4) {
        int v = 0, digits = 0;
        while (*s >= '0' && *s <= '9') {
            if (digits && v == 0) return 0; // no leading zeros
            v = v * 10 + (*s++ - '0');
            if (++digits > 3 || v > 255) return 0;
        }
        if (!digits) return 0;
        out[n++] = (uint8_t)v;
        if (n < 4 && *s++ != '.') return 0;
    }
    return *s == 0;
}

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

static int pton6(const char *s, uint8_t out[16]) {
    uint16_t w[8];
    int n = 0, gap = -1;
    if (s[0] == ':') {
        if (s[1] != ':') return 0;
        s++;
    }
    while (*s) {
        if (*s == ':') {
            if (gap >= 0) return 0;
            gap = n;
            s++;
            if (!*s) break;
            continue;
        }
        if (n == 8) return 0;
        // An IPv4 tail
        const char *dot = s;
        while (*dot && *dot != ':' && *dot != '.') dot++;
        if (*dot == '.') {
            uint8_t v4[4];
            if (n > 6 || !pton4(s, v4)) return 0;
            w[n++] = (uint16_t)(v4[0] << 8 | v4[1]);
            w[n++] = (uint16_t)(v4[2] << 8 | v4[3]);
            s += strlen(s);
            break;
        }
        int v = 0, k = 0;
        while (k < 4 && hexval(*s) >= 0) v = v * 16 + hexval(*s++), k++;
        if (!k) return 0;
        w[n++] = (uint16_t)v;
        if (*s == ':') {
            s++;
            if (!*s) return 0;
            if (*s == ':') {
                if (gap >= 0) return 0;
                gap = n;
                s++;
            }
        } else if (*s) {
            return 0;
        }
    }
    if (gap < 0 && n != 8) return 0;
    if (gap >= 0 && n == 8) return 0;
    uint16_t full[8] = { 0 };
    if (gap < 0) {
        memcpy(full, w, sizeof full);
    } else {
        for (int i = 0; i < gap; i++) full[i] = w[i];
        for (int i = gap; i < n; i++) full[8 - (n - i)] = w[i];
    }
    for (int i = 0; i < 8; i++) {
        out[2 * i] = (uint8_t)(full[i] >> 8);
        out[2 * i + 1] = (uint8_t)full[i];
    }
    return 1;
}

int inet_pton(int af, const char *src, void *dst) {
    if (af == AF_INET) return pton4(src, dst);
    if (af == AF_INET6) return pton6(src, dst);
    errno = EAFNOSUPPORT;
    return -1;
}

const char *inet_ntop(int af, const void *src, char *dst, socklen_t size) {
    char buf[INET6_ADDRSTRLEN];
    const uint8_t *a = src;
    if (af == AF_INET) {
        snprintf(buf, sizeof buf, "%u.%u.%u.%u", a[0], a[1], a[2], a[3]);
    } else if (af == AF_INET6) {
        uint16_t w[8];
        for (int i = 0; i < 8; i++) w[i] = (uint16_t)(a[2 * i] << 8 | a[2 * i + 1]);
        // The longest run of zeros (two or more) becomes ::
        int best = -1, bestlen = 0;
        for (int i = 0; i < 8;) {
            if (w[i]) { i++; continue; }
            int j = i;
            while (j < 8 && !w[j]) j++;
            if (j - i > bestlen && j - i > 1) { best = i; bestlen = j - i; }
            i = j;
        }
        char *p = buf;
        if (best == 0 && bestlen == 5 && w[5] == 0xffff) {
            snprintf(buf, sizeof buf, "::ffff:%u.%u.%u.%u", a[12], a[13], a[14], a[15]);
        } else {
            for (int i = 0; i < 8; i++) {
                if (i == best) {
                    *p++ = ':';
                    if (i == 0) *p++ = ':';
                    i += bestlen - 1;
                    continue;
                }
                p += sprintf(p, "%x", w[i]);
                if (i < 7) *p++ = ':';
            }
            *p = 0;
        }
    } else {
        errno = EAFNOSUPPORT;
        return 0;
    }
    if (strlen(buf) + 1 > size) {
        errno = ENOSPC;
        return 0;
    }
    strcpy(dst, buf);
    return dst;
}

// sockaddr <-> netd's addresses
static int to_net(const struct sockaddr *sa, socklen_t len, rx_netaddr_t *out) {
    memset(out, 0, sizeof *out);
    if (!sa || len < sizeof(sa_family_t)) return -EINVAL;
    if (sa->sa_family == AF_INET) {
        if (len < sizeof(struct sockaddr_in)) return -EINVAL;
        const struct sockaddr_in *in = (const void *)sa;
        out->family = AF_INET;
        out->port = ntohs(in->sin_port);
        memcpy(out->addr, &in->sin_addr, 4);
        return 0;
    }
    if (sa->sa_family == AF_INET6) {
        // IPv4-mapped addresses are IPv4 (a dual-stack AF_INET6 socket)
        if (len < sizeof(struct sockaddr_in6)) return -EINVAL;
        const struct sockaddr_in6 *in6 = (const void *)sa;
        static const uint8_t mapped[12] = { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff };
        out->port = ntohs(in6->sin6_port);
        if (!memcmp(in6->sin6_addr.s6_addr, mapped, 12)) {
            out->family = AF_INET;
            memcpy(out->addr, in6->sin6_addr.s6_addr + 12, 4);
            return 0;
        }
        out->family = AF_INET6;
        memcpy(out->addr, in6->sin6_addr.s6_addr, 16);
        return 0;
    }
    if (sa->sa_family == AF_UNSPEC) {
        out->family = AF_INET; // "disconnect" for UDP
        return 1;
    }
    return -EAFNOSUPPORT;
}

// For an AF_INET6 socket: sockaddr_in6, IPv4 addresses as ::ffff:a.b.c.d
static void from_net6(const rx_netaddr_t *a, struct sockaddr *sa, socklen_t *len) {
    struct sockaddr_in6 in6;
    memset(&in6, 0, sizeof in6);
    in6.sin6_family = AF_INET6;
    in6.sin6_port = htons(a->port);
    if (a->family == AF_INET6) {
        memcpy(in6.sin6_addr.s6_addr, a->addr, 16);
    } else if (a->family == AF_INET) {
        in6.sin6_addr.s6_addr[10] = in6.sin6_addr.s6_addr[11] = 0xff;
        memcpy(in6.sin6_addr.s6_addr + 12, a->addr, 4);
    }
    socklen_t n = *len < sizeof in6 ? *len : (socklen_t)sizeof in6;
    memcpy(sa, &in6, n);
    *len = sizeof in6;
}

static void from_net(int domain, const rx_netaddr_t *a, struct sockaddr *sa, socklen_t *len) {
    if (!sa || !len) return;
    if (domain == AF_INET6 || a->family == AF_INET6) {
        from_net6(a, sa, len);
        return;
    }
    struct sockaddr_in in;
    memset(&in, 0, sizeof in);
    in.sin_family = AF_INET;
    in.sin_port = htons(a->port);
    memcpy(&in.sin_addr, a->addr, 4);
    socklen_t n = *len < sizeof in ? *len : (socklen_t)sizeof in;
    memcpy(sa, &in, n);
    *len = sizeof in;
}

// ---- the connection to netd ----

static struct sock *sock_of(int fd, struct fd **pf) {
    struct fd *f = __fd_get(fd);
    if (!f) return 0;
    if (f->kind != RX_FD_SOCKET) {
        errno = ENOTSOCK;
        return 0;
    }
    if (pf) *pf = f;
    return f->sock;
}

static int nonblock(struct fd *f, int flags) {
    return (f->flags & O_NONBLOCK_) || (flags & MSG_DONTWAIT);
}

static uint64_t deadline_after(uint64_t ns) {
    return ns ? rx_clock_get_monotonic() + ns : RX_TIME_INFINITE;
}

// One message from netd into the queue (errors are taken at once): 0, or
// -EAGAIN if none came by the deadline, -ENETDOWN if netd has gone
static int fetch(struct sock *s, uint64_t deadline) {
    for (;;) {
        static __thread char *buf;
        if (!buf && !(buf = malloc(sizeof(rx_netsock_msg_t) + RX_NETSOCK_MAX_DGRAM + 64))) return -ENOMEM;
        uint32_t nb = 0, nh = 0;
        rx_handle_t hs[2] = { 0, 0 };
        __lock(&s->lock);
        rx_status_t r = rx_channel_read(s->ctl, 0, buf, hs, sizeof(rx_netsock_msg_t) + RX_NETSOCK_MAX_DGRAM + 64, 2, &nb, &nh);
        if (r == RX_OK) {
            if (nb < sizeof(rx_netsock_msg_t)) {
                __unlock(&s->lock);
                for (uint32_t k = 0; k < nh; k++) rx_handle_close(hs[k]);
                continue;
            }
            rx_netsock_msg_t m;
            memcpy(&m, buf, sizeof m);
            if (m.op == RX_NETSOCK_ERROR) {
                s->err = -m.status;
                __unlock(&s->lock);
                return 0;
            }
            uint32_t len = nb - (uint32_t)sizeof m;
            struct qmsg *q = malloc(sizeof *q + len);
            if (!q) {
                __unlock(&s->lock);
                for (uint32_t k = 0; k < nh; k++) rx_handle_close(hs[k]);
                return -ENOMEM;
            }
            q->next = 0;
            q->m = m;
            q->len = len;
            q->nh = nh;
            q->h[0] = hs[0];
            q->h[1] = hs[1];
            memcpy(q->data, buf + sizeof m, len);
            struct qmsg **t = &s->q;
            while (*t) t = &(*t)->next;
            *t = q;
            __unlock(&s->lock);
            return 0;
        }
        __unlock(&s->lock);
        if (r == RX_ERR_PEER_CLOSED) return -ENETDOWN;
        if (r != RX_ERR_SHOULD_WAIT) return -EIO;
        if (deadline == 0) return -EAGAIN;
        uint32_t seen = 0;
        rx_status_t w = rx_object_wait_one(s->ctl, RX_CHANNEL_READABLE | RX_PEER_CLOSED, deadline, &seen);
        if (w == RX_ERR_TIMED_OUT) return -EAGAIN;
    }
}

// The first queued message of `op` (removed unless peek), or 0
static struct qmsg *take(struct sock *s, uint32_t op, int peek) {
    __lock(&s->lock);
    for (struct qmsg **p = &s->q; *p; p = &(*p)->next) {
        if ((*p)->m.op == op) {
            struct qmsg *q = *p;
            if (!peek) *p = q->next;
            __unlock(&s->lock);
            return q;
        }
    }
    __unlock(&s->lock);
    return 0;
}

// Wait for a message of `op`: the message, or 0 with errno
static struct qmsg *wait_for(struct sock *s, uint32_t op, uint64_t deadline, int peek) {
    for (;;) {
        struct qmsg *q = take(s, op, peek);
        if (q) return q;
        int r = fetch(s, deadline);
        if (r < 0) {
            errno = -r;
            return 0;
        }
    }
}

// A request and its reply (same op): the reply's status
static int call(struct sock *s, rx_netsock_msg_t *m, rx_handle_t h, rx_netsock_msg_t *reply) {
    rx_status_t r = rx_channel_write(s->ctl, 0, m, sizeof *m, h ? &h : 0, h ? 1 : 0);
    if (r != RX_OK) {
        if (h) rx_handle_close(h);
        return -ENETDOWN;
    }
    struct qmsg *q = wait_for(s, m->op, RX_TIME_INFINITE, 0);
    if (!q) return -errno;
    if (reply) *reply = q->m;
    int st = q->m.status;
    free(q);
    return st;
}

static void opt(struct sock *s, uint32_t which, uint32_t value) {
    rx_netsock_msg_t m;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_SETOPT;
    m.a = which;
    m.b = value;
    rx_channel_write(s->ctl, 0, &m, sizeof m, 0, 0);
}

static struct sock *sock_new(int type, rx_handle_t ctl) {
    struct sock *s = calloc(1, sizeof *s);
    if (!s) return 0;
    s->refs = 1;
    s->type = type;
    s->ctl = ctl;
    return s;
}

static void sock_free(struct sock *s) {
    if (s->ctl) rx_handle_close(s->ctl);
    if (s->connecting) rx_handle_close(s->connecting);
    for (struct qmsg *q = s->q, *n; q; q = n) {
        n = q->next;
        for (uint32_t k = 0; k < q->nh; k++) rx_handle_close(q->h[k]);
        free(q);
    }
    free(s->peekbuf);
    free(s);
}

void __sock_ref(struct fd *f) {
    __lock(&f->sock->lock);
    f->sock->refs++;
    __unlock(&f->sock->lock);
}

void __sock_release(struct fd *f) {
    struct sock *s = f->sock;
    f->sock = 0;
    if (!s) return;
    __lock(&s->lock);
    int last = --s->refs == 0;
    __unlock(&s->lock);
    if (last) sock_free(s);
}

static int new_fd(struct sock *s, rx_handle_t data, int flags) {
    int fd = __fd_alloc(0);
    if (fd < 0) {
        if (data) rx_handle_close(data);
        sock_free(s);
        return -1;
    }
    // Filled in whole: the slot never reads as free meanwhile
    struct fd n = { 0 };
    n.kind = RX_FD_SOCKET;
    n.handle = data;
    n.flags = O_RDWR | ((flags & SOCK_NONBLOCK) ? O_NONBLOCK_ : 0);
    n.cloexec = (flags & SOCK_CLOEXEC) != 0;
    n.sock = s;
    __fds[fd] = n;
    return fd;
}

// ---- the calls ----

int socket(int domain, int type, int protocol) {
    int base = type & 0xf, flags = type & ~0xf;
    if (domain != AF_INET && domain != AF_INET6) return fail(EAFNOSUPPORT);
    if (flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC)) return fail(EINVAL);
    if (base != SOCK_STREAM && base != SOCK_DGRAM) return fail(base == SOCK_RAW ? EPERM : ESOCKTNOSUPPORT);
    if (protocol && !(base == SOCK_STREAM && protocol == IPPROTO_TCP) && !(base == SOCK_DGRAM && protocol == IPPROTO_UDP))
        return fail(EPROTONOSUPPORT);
    rx_handle_t ctl;
    if (rx_service_connect("/svc/netsock", &ctl) < 0) return fail(ENETDOWN);
    struct sock *s = sock_new(base, ctl);
    if (!s) {
        rx_handle_close(ctl);
        return fail(ENOMEM);
    }
    s->domain = domain;
    rx_netsock_msg_t m;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_OPEN;
    m.a = base == SOCK_STREAM ? RX_NETSOCK_STREAM : RX_NETSOCK_DGRAM_TYPE;
    m.b = (uint32_t)domain;
    int r = call(s, &m, 0, 0);
    if (r < 0) {
        sock_free(s);
        return fail(-r);
    }
    return new_fd(s, 0, flags);
}

int socketpair(int domain, int type, int protocol, int sv[2]) {
    (void)protocol;
    if (domain != AF_UNIX) return fail(EOPNOTSUPP);
    if ((type & 0xf) != SOCK_STREAM) return fail(EOPNOTSUPP);
    // A kernel socket pair: two pipe-like fds, each way
    int a = __fd_alloc(0);
    if (a < 0) return -1;
    int b = __fd_alloc(a + 1);
    if (b < 0) {
        __fd_unreserve(a);
        return -1;
    }
    rx_handle_t x, y;
    rx_status_t st = rx_socket_create(0, &x, &y);
    if (st != RX_OK) {
        __fd_unreserve(a);
        __fd_unreserve(b);
        return __set_errno_status(st);
    }
    struct fd n = { 0 };
    n.kind = RX_FD_PIPE;
    n.flags = O_RDWR | ((type & SOCK_NONBLOCK) ? O_NONBLOCK_ : 0);
    n.handle = x;
    __fds[a] = n;
    n.handle = y;
    __fds[b] = n;
    sv[0] = a;
    sv[1] = b;
    return 0;
}

static int do_bind(struct sock *s, const rx_netaddr_t *a) {
    rx_netsock_msg_t m, reply;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_BIND;
    m.addr = *a;
    int r = call(s, &m, 0, &reply);
    if (r < 0) return r;
    s->local = reply.addr;
    s->has_local = 1;
    s->state = S_BOUND;
    return 0;
}

int bind(int fd, const struct sockaddr *addr, socklen_t len) {
    struct sock *s = sock_of(fd, 0);
    if (!s) return -1;
    rx_netaddr_t a;
    int r = to_net(addr, len, &a);
    if (r < 0) return fail(-r);
    if (r > 0) return fail(EAFNOSUPPORT);
    if (s->state != S_OPEN) return fail(EINVAL);
    if (s->reuse) opt(s, RX_NETSOCK_OPT_REUSEADDR, 1);
    r = do_bind(s, &a);
    return r < 0 ? fail(-r) : 0;
}

int listen(int fd, int backlog) {
    struct sock *s = sock_of(fd, 0);
    if (!s) return -1;
    if (s->type != SOCK_STREAM) return fail(EOPNOTSUPP);
    if (s->state != S_OPEN && s->state != S_BOUND && s->state != S_LISTEN) return fail(EINVAL);
    rx_netsock_msg_t m, reply;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_LISTEN;
    m.a = backlog < 1 ? 1 : (uint32_t)backlog;
    if (s->state == S_OPEN && s->reuse) opt(s, RX_NETSOCK_OPT_REUSEADDR, 1);
    int r = call(s, &m, 0, &reply);
    if (r < 0) return fail(-r);
    s->state = S_LISTEN;
    return 0;
}

int accept4(int fd, struct sockaddr *addr, socklen_t *len, int flags) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    if (s->state != S_LISTEN) return fail(EINVAL);
    if (flags & ~(SOCK_NONBLOCK | SOCK_CLOEXEC)) return fail(EINVAL);
    struct qmsg *q = wait_for(s, RX_NETSOCK_ACCEPTED, nonblock(f, 0) ? 0 : deadline_after(s->rcvtimeo), 0);
    if (!q) return -1;
    if (q->nh < 2) {
        for (uint32_t k = 0; k < q->nh; k++) rx_handle_close(q->h[k]);
        free(q);
        return fail(ECONNABORTED);
    }
    struct sock *c = sock_new(SOCK_STREAM, q->h[1]);
    if (!c) {
        rx_handle_close(q->h[0]);
        rx_handle_close(q->h[1]);
        free(q);
        return fail(ENOMEM);
    }
    c->state = S_CONNECTED;
    c->domain = s->domain;
    c->peer = q->m.addr;
    c->local = q->m.addr2;
    c->has_peer = c->has_local = 1;
    c->nodelay = s->nodelay;
    rx_handle_t data = q->h[0];
    rx_netaddr_t peer = q->m.addr;
    free(q);
    int nfd = new_fd(c, data, flags);
    if (nfd >= 0) from_net(s->domain, &peer, addr, len);
    return nfd;
}

int accept(int fd, struct sockaddr *addr, socklen_t *len) {
    return accept4(fd, addr, len, 0);
}

// The answer to a TCP connect: 0, or -1 with errno (EINPROGRESS if it has
// not come yet and deadline is 0)
static int finish_connect(struct fd *f, struct sock *s, uint64_t deadline) {
    struct qmsg *q = wait_for(s, RX_NETSOCK_CONNECT, deadline, 0);
    if (!q) {
        if (errno == EAGAIN) errno = deadline == 0 ? EINPROGRESS : ETIMEDOUT;
        return -1;
    }
    int st = q->m.status;
    if (st == 0) {
        f->handle = s->connecting;
        s->connecting = 0;
        s->state = S_CONNECTED;
        s->local = q->m.addr;
        s->has_local = 1;
        s->peer = q->m.addr2;
        s->has_peer = 1;
        free(q);
        return 0;
    }
    rx_handle_close(s->connecting);
    s->connecting = 0;
    s->state = s->has_local ? S_BOUND : S_OPEN;
    s->has_peer = 0;
    free(q);
    s->err = -st;
    errno = -st;
    return -1;
}

int connect(int fd, const struct sockaddr *addr, socklen_t len) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    rx_netaddr_t a;
    int r = to_net(addr, len, &a);
    if (r < 0) return fail(-r);
    if (s->type == SOCK_DGRAM) {
        rx_netsock_msg_t m, reply;
        memset(&m, 0, sizeof m);
        m.op = RX_NETSOCK_CONNECT;
        m.addr = a;
        r = call(s, &m, 0, &reply);
        if (r < 0) return fail(-r);
        if (a.port == 0 && !a.addr[0] && !a.addr[1] && !a.addr[2] && !a.addr[3]) {
            s->has_peer = 0;
            s->state = S_BOUND;
        } else {
            s->peer = a;
            s->has_peer = 1;
            s->state = S_CONNECTED;
        }
        s->local = reply.addr;
        s->has_local = 1;
        return 0;
    }
    if (r > 0) return fail(EAFNOSUPPORT);
    switch (s->state) {
    case S_CONNECTED: return fail(EISCONN);
    case S_LISTEN: return fail(EINVAL);
    case S_CONNECTING:
        if (nonblock(f, 0)) {
            if (finish_connect(f, s, 0) == 0) return fail(EISCONN);
            if (errno == EINPROGRESS) errno = EALREADY;
            return -1;
        }
        return finish_connect(f, s, deadline_after(s->sndtimeo));
    }
    rx_handle_t mine, theirs;
    rx_status_t st = rx_socket_create(0, &mine, &theirs);
    if (st != RX_OK) return __set_errno_status(st);
    if (s->reuse && s->state == S_OPEN) opt(s, RX_NETSOCK_OPT_REUSEADDR, 1);
    rx_netsock_msg_t m;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_CONNECT;
    m.addr = a;
    if (rx_channel_write(s->ctl, 0, &m, sizeof m, &theirs, 1) != RX_OK) {
        rx_handle_close(mine);
        rx_handle_close(theirs);
        return fail(ENETDOWN);
    }
    s->connecting = mine;
    s->state = S_CONNECTING;
    s->err = 0;
    if (nonblock(f, 0)) return fail(EINPROGRESS);
    return finish_connect(f, s, deadline_after(s->sndtimeo));
}

// A stream's pending error once its data has ended (a reset), or 0
static int stream_error(struct sock *s) {
    while (fetch(s, 0) == 0) {
    }
    int e = s->err;
    s->err = 0;
    return e;
}

static ssize_t stream_recv(struct fd *f, struct sock *s, char *buf, size_t n, int flags) {
    if (s->state == S_CONNECTING) {
        if (finish_connect(f, s, nonblock(f, flags) ? 0 : deadline_after(s->sndtimeo)) < 0)
            return errno == EINPROGRESS ? fail(EAGAIN) : -1;
    }
    if (s->state != S_CONNECTED) return fail(ENOTCONN);
    if (n == 0) return 0;
    if (s->shut_rd) return 0;
    size_t got = 0;
    // Bytes read ahead by a MSG_PEEK first
    if (s->peeklen) {
        size_t k = s->peeklen < n ? s->peeklen : n;
        memcpy(buf, s->peekbuf, k);
        if (!(flags & MSG_PEEK)) {
            memmove(s->peekbuf, s->peekbuf + k, s->peeklen - k);
            s->peeklen -= (uint32_t)k;
        }
        got = k;
        if (got == n || !(flags & MSG_WAITALL)) return (ssize_t)got;
    }
    if (s->eof) return (ssize_t)got;
    uint64_t deadline = deadline_after(s->rcvtimeo);
    for (;;) {
        uint64_t k = 0;
        char *dst = buf + got;
        size_t want = n - got;
        rx_status_t r = rx_socket_read(f->handle, 0, dst, want, &k);
        if (r == RX_OK) {
            if (flags & MSG_PEEK) {
                char *nb = realloc(s->peekbuf, s->peeklen + k);
                if (!nb) return fail(ENOMEM);
                s->peekbuf = nb;
                memcpy(s->peekbuf + s->peeklen, dst, k);
                s->peeklen += (uint32_t)k;
            }
            got += k;
            if (got == n || !(flags & MSG_WAITALL) || (flags & MSG_PEEK)) return (ssize_t)got;
            continue;
        }
        if (r == RX_ERR_PEER_CLOSED) {
            int e = stream_error(s);
            if (e && !got) return fail(e);
            s->eof = 1;
            return (ssize_t)got;
        }
        if (r != RX_ERR_SHOULD_WAIT) return got ? (ssize_t)got : __set_errno_status(r);
        if (got && !(flags & MSG_WAITALL)) return (ssize_t)got;
        if (nonblock(f, flags)) return got ? (ssize_t)got : fail(EAGAIN);
        if (rx_object_wait_one(f->handle, RX_CHANNEL_READABLE | RX_PEER_CLOSED, deadline, 0) == RX_ERR_TIMED_OUT)
            return got ? (ssize_t)got : fail(EAGAIN);
    }
}

static ssize_t stream_send(struct fd *f, struct sock *s, const char *buf, size_t n, int flags) {
    if (s->state == S_CONNECTING) {
        if (finish_connect(f, s, nonblock(f, flags) ? 0 : deadline_after(s->sndtimeo)) < 0)
            return errno == EINPROGRESS ? fail(EAGAIN) : -1;
    }
    if (s->state != S_CONNECTED) return fail(ENOTCONN);
    if (s->shut_wr) return fail(EPIPE);
    uint64_t deadline = deadline_after(s->sndtimeo);
    size_t done = 0;
    while (done < n) {
        uint64_t k = 0;
        rx_status_t r = rx_socket_write(f->handle, 0, buf + done, n - done, &k);
        if (r == RX_OK) {
            done += k;
            continue;
        }
        if (r == RX_ERR_SHOULD_WAIT) {
            if (nonblock(f, flags)) return done ? (ssize_t)done : fail(EAGAIN);
            if (rx_object_wait_one(f->handle, RX_CHANNEL_WRITABLE | RX_PEER_CLOSED, deadline, 0) == RX_ERR_TIMED_OUT)
                return done ? (ssize_t)done : fail(EAGAIN);
            continue;
        }
        if (done) break;
        if (r == RX_ERR_PEER_CLOSED) {
            int e = stream_error(s);
            return fail(e ? e : EPIPE);  // no SIGPIPE: the writer sees the error
        }
        return __set_errno_status(r);
    }
    return (ssize_t)done;
}

static ssize_t dgram_recv(struct fd *f, struct sock *s, char *buf, size_t n, int flags, struct sockaddr *from, socklen_t *fromlen) {
    if (s->shut_rd) return 0;
    struct qmsg *q = wait_for(s, RX_NETSOCK_DGRAM, nonblock(f, flags) ? 0 : deadline_after(s->rcvtimeo), flags & MSG_PEEK);
    if (!q) {
        if (errno == ENETDOWN) return 0;
        return -1;
    }
    size_t k = q->len < n ? q->len : n;
    memcpy(buf, q->data, k);
    size_t full = q->len;
    from_net(s->domain, &q->m.addr, from, fromlen);
    if (!(flags & MSG_PEEK)) free(q);
    return (ssize_t)((flags & MSG_TRUNC) ? full : k);
}

static ssize_t dgram_send(struct sock *s, const char *buf, size_t n, const struct sockaddr *to, socklen_t tolen) {
    rx_netaddr_t dst;
    memset(&dst, 0, sizeof dst);
    if (to) {
        int r = to_net(to, tolen, &dst);
        if (r < 0) return fail(-r);
        if (r > 0) return fail(EAFNOSUPPORT);
    } else if (s->has_peer) {
        dst = s->peer;
    } else {
        return fail(EDESTADDRREQ);
    }
    if (n > RX_NETSOCK_MAX_DGRAM) return fail(EMSGSIZE);
    if (dst.port == 0) return fail(EINVAL);
    if (!s->has_local) {
        rx_netaddr_t any;
        memset(&any, 0, sizeof any);
        any.family = (uint16_t)s->domain;
        int r = do_bind(s, &any);
        if (r < 0) return fail(-r);
    }
    char *msg = malloc(sizeof(rx_netsock_msg_t) + n);
    if (!msg) return fail(ENOMEM);
    rx_netsock_msg_t m;
    memset(&m, 0, sizeof m);
    m.op = RX_NETSOCK_SENDTO;
    m.addr = dst;
    memcpy(msg, &m, sizeof m);
    memcpy(msg + sizeof m, buf, n);
    rx_status_t r = rx_channel_write(s->ctl, 0, msg, (uint32_t)(sizeof m + n), 0, 0);
    free(msg);
    if (r == RX_ERR_SHOULD_WAIT) return fail(ENOBUFS);
    if (r != RX_OK) return fail(ENETDOWN);
    return (ssize_t)n;
}

ssize_t recvfrom(int fd, void *buf, size_t n, int flags, struct sockaddr *from, socklen_t *fromlen) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    if (s->type == SOCK_DGRAM) return dgram_recv(f, s, buf, n, flags, from, fromlen);
    ssize_t r = stream_recv(f, s, buf, n, flags);
    if (r >= 0 && from && fromlen) from_net(s->domain, &s->peer, from, fromlen);
    return r;
}

ssize_t recv(int fd, void *buf, size_t n, int flags) {
    return recvfrom(fd, buf, n, flags, 0, 0);
}

ssize_t sendto(int fd, const void *buf, size_t n, int flags, const struct sockaddr *to, socklen_t tolen) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    if (s->type == SOCK_DGRAM) return dgram_send(s, buf, n, to, tolen);
    if (to && s->state != S_CONNECTED) return fail(EOPNOTSUPP);
    return stream_send(f, s, buf, n, flags);
}

ssize_t send(int fd, const void *buf, size_t n, int flags) {
    return sendto(fd, buf, n, flags, 0, 0);
}

ssize_t sendmsg(int fd, const struct msghdr *msg, int flags) {
    size_t total = 0;
    for (int i = 0; i < msg->msg_iovlen; i++) total += msg->msg_iov[i].iov_len;
    char *b = malloc(total ? total : 1);
    if (!b) return fail(ENOMEM);
    size_t off = 0;
    for (int i = 0; i < msg->msg_iovlen; i++) {
        memcpy(b + off, msg->msg_iov[i].iov_base, msg->msg_iov[i].iov_len);
        off += msg->msg_iov[i].iov_len;
    }
    ssize_t r = sendto(fd, b, total, flags, msg->msg_name, msg->msg_namelen);
    free(b);
    return r;
}

ssize_t recvmsg(int fd, struct msghdr *msg, int flags) {
    size_t total = 0;
    for (int i = 0; i < msg->msg_iovlen; i++) total += msg->msg_iov[i].iov_len;
    char *b = malloc(total ? total : 1);
    if (!b) return fail(ENOMEM);
    socklen_t nl = msg->msg_namelen;
    ssize_t r = recvfrom(fd, b, total, flags, msg->msg_name, msg->msg_name ? &nl : 0);
    if (r >= 0) {
        size_t off = 0, left = (size_t)r < total ? (size_t)r : total;
        for (int i = 0; i < msg->msg_iovlen && left; i++) {
            size_t k = msg->msg_iov[i].iov_len < left ? msg->msg_iov[i].iov_len : left;
            memcpy(msg->msg_iov[i].iov_base, b + off, k);
            off += k;
            left -= k;
        }
        msg->msg_namelen = nl;
        msg->msg_controllen = 0;
        msg->msg_flags = (size_t)r > total ? MSG_TRUNC : 0;
    }
    free(b);
    return r;
}

long __sock_read(struct fd *f, char *buf, size_t n) {
    struct sock *s = f->sock;
    if (s->type == SOCK_DGRAM) return dgram_recv(f, s, buf, n, 0, 0, 0);
    return stream_recv(f, s, buf, n, 0);
}

long __sock_write(struct fd *f, const char *buf, size_t n) {
    struct sock *s = f->sock;
    if (s->type == SOCK_DGRAM) return dgram_send(s, buf, n, 0, 0);
    return stream_send(f, s, buf, n, 0);
}

int shutdown(int fd, int how) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    if (how < SHUT_RD || how > SHUT_RDWR) return fail(EINVAL);
    if (s->type == SOCK_STREAM && s->state != S_CONNECTED) return fail(ENOTCONN);
    if (how != SHUT_WR) s->shut_rd = 1;
    if (how != SHUT_RD && s->type == SOCK_STREAM && !s->shut_wr) {
        uint64_t k = 0;
        rx_socket_write(f->handle, RX_SOCKET_SHUTDOWN_WRITE, 0, 0, &k);
        s->shut_wr = 1;
    }
    return 0;
}

// A connect whose answer has come (a program that polled for it and did
// not ask SO_ERROR, as Rust's connect_timeout): take it now
static void settle(struct fd *f, struct sock *s) {
    if (s->state != S_CONNECTING) return;
    int saved = errno;
    finish_connect(f, s, 0);
    errno = saved;
}

int getsockname(int fd, struct sockaddr *addr, socklen_t *len) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    settle(f, s);
    rx_netaddr_t a;
    memset(&a, 0, sizeof a);
    a.family = (uint16_t)s->domain;
    if (s->has_local) a = s->local;
    from_net(s->domain, &a, addr, len);
    return 0;
}

int getpeername(int fd, struct sockaddr *addr, socklen_t *len) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    settle(f, s);
    if (!s->has_peer || s->state != S_CONNECTED) return fail(ENOTCONN);
    from_net(s->domain, &s->peer, addr, len);
    return 0;
}

static uint64_t tv_ns(const void *val, socklen_t len) {
    if (len < sizeof(struct timeval)) return 0;
    const struct timeval *tv = val;
    return (uint64_t)tv->tv_sec * 1000000000ull + (uint64_t)tv->tv_usec * 1000ull;
}

static int int_val(const void *val, socklen_t len) {
    if (len >= sizeof(int)) return *(const int *)val;
    if (len >= 1) return *(const unsigned char *)val;
    return 0;
}

int setsockopt(int fd, int level, int name, const void *val, socklen_t len) {
    struct sock *s = sock_of(fd, 0);
    if (!s) return -1;
    if (!val && len) return fail(EFAULT);
    int v = int_val(val, len);
    if (level == SOL_SOCKET) {
        switch (name) {
        case SO_REUSEADDR:
            s->reuse = v != 0;
            opt(s, RX_NETSOCK_OPT_REUSEADDR, s->reuse);
            return 0;
        case SO_KEEPALIVE:
            s->keepalive = v != 0;
            opt(s, RX_NETSOCK_OPT_KEEPALIVE, s->keepalive);
            return 0;
        case SO_BROADCAST: s->broadcast = v != 0; return 0;
        case SO_RCVTIMEO: s->rcvtimeo = tv_ns(val, len); return 0;
        case SO_SNDTIMEO: s->sndtimeo = tv_ns(val, len); return 0;
        case SO_RCVBUF: case SO_SNDBUF: case SO_LINGER: case SO_REUSEPORT: case SO_OOBINLINE:
        case SO_RCVLOWAT: case SO_SNDLOWAT: case SO_DONTROUTE: case SO_DEBUG:
            return 0;  // accepted; netd's buffers are fixed
        }
        return fail(ENOPROTOOPT);
    }
    if (level == IPPROTO_TCP) {
        if (s->type != SOCK_STREAM) return fail(ENOPROTOOPT);
        if (name == TCP_NODELAY) {
            s->nodelay = v != 0;
            opt(s, RX_NETSOCK_OPT_NODELAY, s->nodelay);
            return 0;
        }
        if (name == TCP_KEEPIDLE || name == TCP_KEEPINTVL || name == TCP_KEEPCNT || name == TCP_MAXSEG) return 0;
        return fail(ENOPROTOOPT);
    }
    if (level == IPPROTO_IP) {
        if (name == IP_TTL || name == IP_TOS || name == IP_MULTICAST_TTL || name == IP_MULTICAST_LOOP) return 0;
        return fail(ENOPROTOOPT);
    }
    if (level == IPPROTO_IPV6) {
        if (name == IPV6_V6ONLY || name == IPV6_UNICAST_HOPS || name == IPV6_MULTICAST_LOOP) return 0;  // accepted
        return fail(ENOPROTOOPT);
    }
    return fail(ENOPROTOOPT);
}

static int put_int(void *val, socklen_t *len, int v) {
    if (!val || !len) return fail(EFAULT);
    if (*len >= sizeof(int)) {
        *(int *)val = v;
        *len = sizeof(int);
    } else if (*len >= 1) {
        *(unsigned char *)val = (unsigned char)v;
        *len = 1;
    }
    return 0;
}

static int put_tv(void *val, socklen_t *len, uint64_t ns) {
    if (!val || !len || *len < sizeof(struct timeval)) return fail(EINVAL);
    struct timeval *tv = val;
    tv->tv_sec = (time_t)(ns / 1000000000ull);
    tv->tv_usec = (long)(ns % 1000000000ull / 1000);
    *len = sizeof *tv;
    return 0;
}

int getsockopt(int fd, int level, int name, void *val, socklen_t *len) {
    struct fd *f;
    struct sock *s = sock_of(fd, &f);
    if (!s) return -1;
    if (level == SOL_SOCKET) {
        switch (name) {
        case SO_ERROR: {
            if (s->state == S_CONNECTING) {
                int saved = errno;
                if (finish_connect(f, s, 0) < 0 && errno == EINPROGRESS) {
                    errno = saved;
                    return put_int(val, len, 0);
                }
                errno = saved;
            } else {
                while (fetch(s, 0) == 0) {
                }
            }
            int e = s->err;
            s->err = 0;
            return put_int(val, len, e);
        }
        case SO_TYPE: return put_int(val, len, s->type);
        case SO_REUSEADDR: return put_int(val, len, s->reuse);
        case SO_KEEPALIVE: return put_int(val, len, s->keepalive);
        case SO_BROADCAST: return put_int(val, len, s->broadcast);
        case SO_ACCEPTCONN: return put_int(val, len, s->state == S_LISTEN);
        case SO_RCVBUF: case SO_SNDBUF: return put_int(val, len, 65536);
        case SO_RCVTIMEO: return put_tv(val, len, s->rcvtimeo);
        case SO_SNDTIMEO: return put_tv(val, len, s->sndtimeo);
        case SO_LINGER:
            if (!val || !len || *len < sizeof(struct linger)) return fail(EINVAL);
            memset(val, 0, sizeof(struct linger));
            *len = sizeof(struct linger);
            return 0;
        }
        return fail(ENOPROTOOPT);
    }
    if (level == IPPROTO_TCP && name == TCP_NODELAY) return put_int(val, len, s->nodelay);
    if (level == IPPROTO_IP && name == IP_TTL) return put_int(val, len, 64);
    return fail(ENOPROTOOPT);
}

// ---- poll ----

short __sock_poll_prepare(struct fd *f, short events, rx_handle_t *h, uint32_t *signals) {
    struct sock *s = f->sock;
    *h = 0;
    *signals = 0;
    short ready = 0;
    switch (s->state) {
    case S_CONNECTED:
        if (s->type == SOCK_STREAM) {
            if ((events & POLLIN) && (s->peeklen || s->eof || s->shut_rd)) ready |= POLLIN;
            if (ready) return ready;
            *h = f->handle;
            *signals = RX_PEER_CLOSED | ((events & POLLIN) ? RX_CHANNEL_READABLE : 0) | ((events & POLLOUT) ? RX_CHANNEL_WRITABLE : 0);
            return 0;
        }
        __attribute__((fallthrough)); // a connected UDP socket is like an unconnected one
    case S_BOUND:
    case S_OPEN:
        if (s->type == SOCK_DGRAM) {
            if (events & POLLOUT) ready |= POLLOUT;
            if ((events & POLLIN) && take(s, RX_NETSOCK_DGRAM, 1)) ready |= POLLIN;
            if (ready || !(events & POLLIN)) return ready;
            *h = s->ctl;
            *signals = RX_CHANNEL_READABLE | RX_PEER_CLOSED;
            return 0;
        }
        return (short)(POLLOUT | POLLHUP) & (short)(events | POLLHUP);  // as Linux: unconnected stream
    case S_LISTEN:
        if ((events & POLLIN) && take(s, RX_NETSOCK_ACCEPTED, 1)) return POLLIN;
        *h = s->ctl;
        *signals = RX_CHANNEL_READABLE | RX_PEER_CLOSED;
        return 0;
    case S_CONNECTING:
        if (take(s, RX_NETSOCK_CONNECT, 1)) return __sock_poll_result(f, events, RX_CHANNEL_READABLE);
        *h = s->ctl;
        *signals = RX_CHANNEL_READABLE | RX_PEER_CLOSED;
        return 0;
    }
    return 0;
}

short __sock_poll_result(struct fd *f, short events, uint32_t pending) {
    struct sock *s = f->sock;
    short r = 0;
    if (s->state == S_CONNECTED && s->type == SOCK_STREAM) {
        if ((pending & RX_CHANNEL_READABLE) && (events & POLLIN)) r |= POLLIN;
        if ((pending & RX_CHANNEL_WRITABLE) && (events & POLLOUT)) r |= POLLOUT;
        if (pending & RX_PEER_CLOSED) r |= POLLHUP | (events & POLLIN);
        return r;
    }
    if (pending & RX_PEER_CLOSED) return POLLERR | POLLHUP;  // netd has gone
    if (!(pending & RX_CHANNEL_READABLE)) return 0;
    // Something on the control channel: what is it?
    while (fetch(s, 0) == 0) {
    }
    switch (s->state) {
    case S_LISTEN: return take(s, RX_NETSOCK_ACCEPTED, 1) && (events & POLLIN) ? POLLIN : 0;
    case S_CONNECTING:
        if (take(s, RX_NETSOCK_CONNECT, 1)) {
            // Take the answer now: connected (POLLOUT), or failed (POLLERR,
            // the error for SO_ERROR)
            int saved = errno;
            int ok = finish_connect(f, s, 0) == 0;
            errno = saved;
            if (!ok) return POLLOUT | POLLERR | POLLHUP;
            return events & POLLOUT ? POLLOUT : 0;
        }
        return s->err ? POLLERR : 0;
    default:
        if (s->type == SOCK_DGRAM && take(s, RX_NETSOCK_DGRAM, 1)) r |= events & POLLIN;
        if (s->type == SOCK_DGRAM) r |= events & POLLOUT;
        return r;
    }
}

int select(int n, fd_set *rs, fd_set *ws, fd_set *es, struct timeval *tv) {
    struct pollfd p[64];
    int m = 0;
    if (n > FD_SETSIZE) n = FD_SETSIZE;
    for (int fd = 0; fd < n; fd++) {
        short ev = 0;
        if (rs && FD_ISSET(fd, rs)) ev |= POLLIN;
        if (ws && FD_ISSET(fd, ws)) ev |= POLLOUT;
        if (es && FD_ISSET(fd, es)) ev |= POLLPRI;
        if (!ev) continue;
        if (m == 64) return fail(EINVAL);
        p[m].fd = fd;
        p[m].events = ev;
        p[m].revents = 0;
        m++;
    }
    int timeout = tv ? (int)(tv->tv_sec * 1000 + tv->tv_usec / 1000) : -1;
    int r = poll(p, (nfds_t)m, timeout);
    if (r < 0) return -1;
    if (rs) FD_ZERO(rs);
    if (ws) FD_ZERO(ws);
    if (es) FD_ZERO(es);
    int count = 0;
    for (int k = 0; k < m; k++) {
        if (p[k].revents & POLLNVAL) return fail(EBADF);
        if (rs && (p[k].revents & (POLLIN | POLLHUP | POLLERR)) && (p[k].events & POLLIN)) { FD_SET(p[k].fd, rs); count++; }
        if (ws && (p[k].revents & (POLLOUT | POLLERR)) && (p[k].events & POLLOUT)) { FD_SET(p[k].fd, ws); count++; }
        if (es && (p[k].revents & POLLPRI)) { FD_SET(p[k].fd, es); count++; }
    }
    if (tv) tv->tv_sec = tv->tv_usec = 0;
    return count;
}

// ---- names ----

int h_errno;

static const struct { const char *name; int port; int proto; } services[] = {
    { "echo", 7, 0 }, { "discard", 9, 0 }, { "daytime", 13, 0 }, { "ftp-data", 20, 6 }, { "ftp", 21, 6 },
    { "ssh", 22, 6 }, { "telnet", 23, 6 }, { "smtp", 25, 6 }, { "time", 37, 0 }, { "domain", 53, 0 },
    { "tftp", 69, 17 }, { "http", 80, 6 }, { "www", 80, 6 }, { "pop3", 110, 6 }, { "ntp", 123, 17 },
    { "imap", 143, 6 }, { "snmp", 161, 17 }, { "ldap", 389, 6 }, { "https", 443, 6 }, { "submission", 587, 6 },
    { "imaps", 993, 6 }, { "pop3s", 995, 6 }, { "mysql", 3306, 6 }, { "postgresql", 5432, 6 },
    { "http-alt", 8080, 6 },
};

static int service_port(const char *serv, int numeric_only, int *port) {
    if (!serv || !*serv) {
        *port = 0;
        return 0;
    }
    char *end;
    long v = strtol(serv, &end, 10);
    if (!*end) {
        if (v < 0 || v > 65535) return EAI_SERVICE;
        *port = (int)v;
        return 0;
    }
    if (numeric_only) return EAI_NONAME;
    for (size_t k = 0; k < sizeof services / sizeof *services; k++)
        if (!strcmp(services[k].name, serv)) {
            *port = services[k].port;
            return 0;
        }
    return EAI_SERVICE;
}

// /etc/hosts: the first IPv4 address for name (1), or 0
static int hosts_lookup(const char *name, uint8_t out[4], char *canon, size_t clen) {
    FILE *f = fopen("/etc/hosts", "r");
    if (!f) return 0;
    char line[512];
    int found = 0;
    while (!found && fgets(line, sizeof line, f)) {
        char *h = strchr(line, '#');
        if (h) *h = 0;
        char *save, *addr = strtok_r(line, " \t\r\n", &save);
        if (!addr) continue;
        uint8_t a[4];
        if (inet_pton(AF_INET, addr, a) != 1) continue;
        char *first = 0;
        for (char *w = strtok_r(0, " \t\r\n", &save); w; w = strtok_r(0, " \t\r\n", &save)) {
            if (!first) first = w;
            if (!strcasecmp(w, name)) {
                memcpy(out, a, 4);
                if (canon) snprintf(canon, clen, "%s", first);
                found = 1;
                break;
            }
        }
    }
    fclose(f);
    return found;
}

int __dns_lookup4(const char *name, uint8_t (*addrs)[4], int max, char *canon, size_t clen);
int __dns_reverse4(const uint8_t a[4], char *name, size_t len);
int __dns_lookup6(const char *name, uint8_t (*addrs)[16], int max, char *canon, size_t clen);

int getaddrinfo(const char *node, const char *service, const struct addrinfo *hints, struct addrinfo **res) {
    int flags = hints ? hints->ai_flags : 0;
    int family = hints ? hints->ai_family : AF_UNSPEC;
    int socktype = hints ? hints->ai_socktype : 0;
    int protocol = hints ? hints->ai_protocol : 0;
    if (!node && !service) return EAI_NONAME;
    if (family != AF_UNSPEC && family != AF_INET && family != AF_INET6) return EAI_FAMILY;
    if (socktype && socktype != SOCK_STREAM && socktype != SOCK_DGRAM && socktype != SOCK_RAW) return EAI_SOCKTYPE;
    int port;
    int e = service_port(service, (flags & AI_NUMERICSERV) != 0, &port);
    if (e) return e;
    // The addresses: IPv4 first, then IPv6 (AF_UNSPEC), as most of the
    // world's networks are still IPv4
    struct { int fam; uint8_t a[16]; } addrs[32];
    int na = 0;
    int want4 = family != AF_INET6, want6 = family != AF_INET;
    char canon[256] = "";
    uint8_t v4[4], v6[16];
    if (!node) {
        if (want4) {
            uint32_t v = htonl((flags & AI_PASSIVE) ? INADDR_ANY : INADDR_LOOPBACK);
            addrs[na].fam = AF_INET;
            memcpy(addrs[na++].a, &v, 4);
        }
        if (want6) {
            addrs[na].fam = AF_INET6;
            memcpy(addrs[na++].a, (flags & AI_PASSIVE) ? &in6addr_any : &in6addr_loopback, 16);
        }
    } else if (inet_aton(node, (struct in_addr *)v4)) {
        if (!want4) return EAI_NONAME;
        addrs[na].fam = AF_INET;
        memcpy(addrs[na++].a, v4, 4);
        snprintf(canon, sizeof canon, "%s", node);
    } else if (inet_pton(AF_INET6, node, v6) == 1) {
        if (!want6) return EAI_NONAME;
        addrs[na].fam = AF_INET6;
        memcpy(addrs[na++].a, v6, 16);
        snprintf(canon, sizeof canon, "%s", node);
    } else if (flags & AI_NUMERICHOST) {
        return EAI_NONAME;
    } else if (!strcasecmp(node, "localhost") || !strcasecmp(node, "localhost.localdomain") || !strcasecmp(node, "ip6-localhost")) {
        if (want4) {
            addrs[na].fam = AF_INET;
            memcpy(addrs[na++].a, "\x7f\0\0\1", 4);
        }
        if (want6) {
            addrs[na].fam = AF_INET6;
            memcpy(addrs[na++].a, &in6addr_loopback, 16);
        }
        snprintf(canon, sizeof canon, "localhost");
    } else if (want4 && hosts_lookup(node, v4, canon, sizeof canon)) {
        addrs[na].fam = AF_INET;
        memcpy(addrs[na++].a, v4, 4);
    } else {
        // DNS: A and AAAA records from the servers in /etc/resolv.conf
        int err = EAI_NONAME;
        if (want4) {
            uint8_t a4[16][4];
            int n = __dns_lookup4(node, a4, 16, canon, sizeof canon);
            if (n < 0) err = n;
            for (int k = 0; k < n; k++) {
                addrs[na].fam = AF_INET;
                memcpy(addrs[na++].a, a4[k], 4);
            }
        }
        if (want6 && (err != EAI_AGAIN || !want4)) {
            uint8_t a6[16][16];
            int n = __dns_lookup6(node, a6, 16, canon[0] ? 0 : canon, sizeof canon);
            if (n < 0 && !na) err = n;
            for (int k = 0; k < n; k++) {
                addrs[na].fam = AF_INET6;
                memcpy(addrs[na++].a, a6[k], 16);
            }
        }
        if (!na) return err;
    }
    int types[2], nt = 0;
    if (socktype) types[nt++] = socktype;
    else { types[nt++] = SOCK_STREAM; types[nt++] = SOCK_DGRAM; }
    struct addrinfo *head = 0, **tail = &head;
    for (int j = 0; j < na; j++)
    for (int k = 0; k < nt; k++) {
        struct addrinfo *ai = calloc(1, sizeof *ai + sizeof(struct sockaddr_in6));
        if (!ai) {
            freeaddrinfo(head);
            return EAI_MEMORY;
        }
        if (addrs[j].fam == AF_INET) {
            struct sockaddr_in *in = (struct sockaddr_in *)(ai + 1);
            in->sin_family = AF_INET;
            in->sin_port = htons((uint16_t)port);
            memcpy(&in->sin_addr, addrs[j].a, 4);
            ai->ai_addrlen = sizeof *in;
            ai->ai_addr = (struct sockaddr *)in;
        } else {
            struct sockaddr_in6 *in6 = (struct sockaddr_in6 *)(ai + 1);
            in6->sin6_family = AF_INET6;
            in6->sin6_port = htons((uint16_t)port);
            memcpy(in6->sin6_addr.s6_addr, addrs[j].a, 16);
            ai->ai_addrlen = sizeof *in6;
            ai->ai_addr = (struct sockaddr *)in6;
        }
        ai->ai_family = addrs[j].fam;
        ai->ai_socktype = types[k];
        ai->ai_protocol = protocol ? protocol : types[k] == SOCK_STREAM ? IPPROTO_TCP : types[k] == SOCK_DGRAM ? IPPROTO_UDP : 0;
        if ((flags & AI_CANONNAME) && !head) ai->ai_canonname = strdup(canon[0] ? canon : node ? node : "");
        *tail = ai;
        tail = &ai->ai_next;
    }
    *res = head;
    return 0;
}

void freeaddrinfo(struct addrinfo *res) {
    while (res) {
        struct addrinfo *n = res->ai_next;
        free(res->ai_canonname);
        free(res);
        res = n;
    }
}

const char *gai_strerror(int e) {
    switch (e) {
    case 0: return "Success";
    case EAI_BADFLAGS: return "Invalid flags";
    case EAI_NONAME: return "Name does not resolve";
    case EAI_AGAIN: return "Temporary failure in name resolution";
    case EAI_FAIL: return "Non-recoverable failure in name resolution";
    case EAI_FAMILY: return "Unrecognized address family or invalid length";
    case EAI_SOCKTYPE: return "Unrecognized socket type";
    case EAI_SERVICE: return "Unrecognized service";
    case EAI_MEMORY: return "Out of memory";
    case EAI_SYSTEM: return "System error";
    case EAI_OVERFLOW: return "Overflow";
    default: return "Unknown error";
    }
}

int getnameinfo(const struct sockaddr *sa, socklen_t salen, char *host, socklen_t hostlen, char *serv, socklen_t servlen, int flags) {
    if (sa && sa->sa_family == AF_INET6 && salen >= sizeof(struct sockaddr_in6)) {
        const struct sockaddr_in6 *in6 = (const void *)sa;
        if (host && hostlen) {
            char num[INET6_ADDRSTRLEN];
            inet_ntop(AF_INET6, in6->sin6_addr.s6_addr, num, sizeof num);
            const char *name = num;
            if (!(flags & NI_NUMERICHOST) && !memcmp(&in6->sin6_addr, &in6addr_loopback, 16)) name = "localhost";
            else if (!(flags & NI_NUMERICHOST) && (flags & NI_NAMEREQD)) return EAI_NONAME;
            if (strlen(name) >= hostlen) return EAI_OVERFLOW;
            strcpy(host, name);
        }
        if (serv && servlen) {
            char num[8];
            snprintf(num, sizeof num, "%d", ntohs(in6->sin6_port));
            if (strlen(num) >= servlen) return EAI_OVERFLOW;
            strcpy(serv, num);
        }
        return 0;
    }
    if (!sa || salen < sizeof(struct sockaddr_in) || sa->sa_family != AF_INET) return EAI_FAMILY;
    const struct sockaddr_in *in = (const void *)sa;
    if (host && hostlen) {
        char num[INET_ADDRSTRLEN];
        inet_ntop(AF_INET, &in->sin_addr, num, sizeof num);
        const char *name = num;
        char found[NI_MAXHOST];
        if (!(flags & NI_NUMERICHOST)) {
            if (!memcmp(&in->sin_addr, "\x7f\0\0\1", 4)) name = "localhost";
            else if (__dns_reverse4((const uint8_t *)&in->sin_addr, found, sizeof found) == 0) name = found;
            else if (flags & NI_NAMEREQD) return EAI_NONAME;
        }
        if (strlen(name) >= hostlen) return EAI_OVERFLOW;
        strcpy(host, name);
    }
    if (serv && servlen) {
        int port = ntohs(in->sin_port);
        const char *name = 0;
        if (!(flags & NI_NUMERICSERV))
            for (size_t k = 0; k < sizeof services / sizeof *services; k++)
                if (services[k].port == port) { name = services[k].name; break; }
        char num[8];
        if (!name) {
            snprintf(num, sizeof num, "%d", port);
            name = num;
        }
        if (strlen(name) >= servlen) return EAI_OVERFLOW;
        strcpy(serv, name);
    }
    return 0;
}

struct hostent *gethostbyname(const char *name) {
    static struct hostent h;
    static char hname[256];
    static struct in_addr addr;
    static char *list[2], *aliases[1];
    struct addrinfo hints, *res;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_CANONNAME;
    if (getaddrinfo(name, 0, &hints, &res) != 0) {
        h_errno = HOST_NOT_FOUND;
        return 0;
    }
    addr = ((struct sockaddr_in *)res->ai_addr)->sin_addr;
    snprintf(hname, sizeof hname, "%s", res->ai_canonname ? res->ai_canonname : name);
    freeaddrinfo(res);
    list[0] = (char *)&addr;
    list[1] = 0;
    aliases[0] = 0;
    h.h_name = hname;
    h.h_aliases = aliases;
    h.h_addrtype = AF_INET;
    h.h_length = 4;
    h.h_addr_list = list;
    return &h;
}

struct servent *getservbyname(const char *name, const char *proto) {
    static struct servent s;
    static char *aliases[1];
    for (size_t k = 0; k < sizeof services / sizeof *services; k++) {
        if (strcmp(services[k].name, name)) continue;
        if (proto && services[k].proto && ((services[k].proto == 6) != !strcmp(proto, "tcp"))) continue;
        s.s_name = (char *)services[k].name;
        s.s_aliases = aliases;
        s.s_port = htons((uint16_t)services[k].port);
        s.s_proto = (char *)(proto ? proto : services[k].proto == 17 ? "udp" : "tcp");
        return &s;
    }
    return 0;
}

// ---- netd's control protocol (/svc/net), step D6 ----

static rx_handle_t netctl_ch;
static volatile int netctl_lock;

int rx_net_control(const char *req, char *out, size_t cap) {
    if (!out || cap < 2) return fail(EINVAL);
    __lock(&netctl_lock);
    int ret = -1;
    for (int attempt = 0; attempt < 2 && ret < 0; attempt++) {
        if (!netctl_ch && rx_service_connect("/svc/net", &netctl_ch) < 0) {
            netctl_ch = 0;
            break;
        }
        if (rx_channel_write(netctl_ch, 0, req, (uint32_t)strlen(req), 0, 0) != RX_OK) goto again;
        for (;;) {
            uint32_t nb = 0;
            rx_status_t s = rx_channel_read(netctl_ch, 0, out, 0, (uint32_t)(cap - 1), 0, &nb, 0);
            if (s == RX_ERR_SHOULD_WAIT) {
                uint32_t seen = 0;
                rx_object_wait_one(netctl_ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &seen);
                if (!(seen & RX_CHANNEL_READABLE) && (seen & RX_PEER_CLOSED)) goto again;
                continue;
            }
            if (s != RX_OK) goto again;
            out[nb] = 0;
            ret = (int)nb;
            break;
        }
        continue;
    again:
        // netd restarted: connect again once
        rx_handle_close(netctl_ch);
        netctl_ch = 0;
    }
    __unlock(&netctl_lock);
    if (ret < 0) errno = ENETDOWN;
    return ret;
}

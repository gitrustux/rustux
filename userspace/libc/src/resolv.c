// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The DNS resolver (network step D3, docs/NETWORK.md). /etc/resolv.conf
// gives the servers (nameserver, at most 3), the search domains (search
// or domain) and options timeout:N, attempts:N, ndots:N; netd writes it
// from DHCP. A query goes to each server in turn over UDP; an answer with
// the truncated bit set is asked again over TCP.

#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <poll.h>
#include <resolv.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>
#include <rustux/syscalls.h>

// Each thread its own (2026-11-27): libcurl looks names up on threads of
// its own, several at once (a page with many hosts), and res_init()
// rewrites this at every query; shared, one thread's lookup saw another's
// half-read configuration ("Couldn't resolve host name")
static __thread struct {
    struct in_addr ns[3];
    int nns;
    char search[6][256];
    int nsearch;
    int timeout, attempts, ndots;
} conf;

static struct in_addr forced;
static int have_forced;
static __thread char last_server[INET_ADDRSTRLEN];

void rx_res_server(const char *addr) {
    have_forced = addr && inet_aton(addr, &forced);
}

const char *rx_res_last_server(void) {
    return last_server;
}

int res_init(void) {
    memset(&conf, 0, sizeof conf);
    conf.timeout = 2;
    conf.attempts = 2;
    conf.ndots = 1;
    FILE *f = fopen("/etc/resolv.conf", "r");
    if (f) {
        char line[512];
        while (fgets(line, sizeof line, f)) {
            char *save, *w = strtok_r(line, " \t\r\n", &save);
            if (!w || w[0] == '#' || w[0] == ';') continue;
            if (!strcmp(w, "nameserver")) {
                char *a = strtok_r(0, " \t\r\n", &save);
                if (a && conf.nns < 3 && inet_aton(a, &conf.ns[conf.nns])) conf.nns++;
            } else if (!strcmp(w, "search") || !strcmp(w, "domain")) {
                conf.nsearch = 0;
                for (char *d = strtok_r(0, " \t\r\n", &save); d && conf.nsearch < 6; d = strtok_r(0, " \t\r\n", &save))
                    snprintf(conf.search[conf.nsearch++], 256, "%s", d);
            } else if (!strcmp(w, "options")) {
                for (char *o = strtok_r(0, " \t\r\n", &save); o; o = strtok_r(0, " \t\r\n", &save)) {
                    if (!strncmp(o, "timeout:", 8)) conf.timeout = atoi(o + 8);
                    else if (!strncmp(o, "attempts:", 9)) conf.attempts = atoi(o + 9);
                    else if (!strncmp(o, "ndots:", 6)) conf.ndots = atoi(o + 6);
                }
            }
        }
        fclose(f);
    }
    if (conf.timeout < 1) conf.timeout = 1;
    if (conf.timeout > 30) conf.timeout = 30;
    if (conf.attempts < 1) conf.attempts = 1;
    if (conf.attempts > 5) conf.attempts = 5;
    return 0;
}

// The query message for name: its length, or -1
static int make_query(const char *name, int cls, int type, uint16_t id, unsigned char *q, int cap) {
    if (cap < 512) return -1;
    memset(q, 0, 12);
    q[0] = (unsigned char)(id >> 8);
    q[1] = (unsigned char)id;
    q[2] = 0x01; // recursion desired
    q[5] = 1;    // one question
    int p = 12;
    const char *s = name;
    if (!strcmp(s, ".")) s = "";
    while (*s) {
        const char *dot = strchr(s, '.');
        size_t len = dot ? (size_t)(dot - s) : strlen(s);
        if (len == 0 || len > 63 || p + (int)len + 2 > 255 + 12) return -1;
        q[p++] = (unsigned char)len;
        memcpy(q + p, s, len);
        p += (int)len;
        s += len;
        if (*s == '.') s++;
    }
    q[p++] = 0;
    q[p++] = (unsigned char)(type >> 8);
    q[p++] = (unsigned char)type;
    q[p++] = (unsigned char)(cls >> 8);
    q[p++] = (unsigned char)cls;
    return p;
}

static uint16_t random_id(void) {
    static __thread uint64_t x;
    if (!x) x = rx_clock_get_monotonic() ^ (uint64_t)(uintptr_t)&x;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    return (uint16_t)x;
}

static int read_full(int fd, unsigned char *p, int n, int timeout_ms) {
    int got = 0;
    while (got < n) {
        struct pollfd pf = { fd, POLLIN, 0 };
        if (poll(&pf, 1, timeout_ms) <= 0) return -1;
        ssize_t k = read(fd, p + got, (size_t)(n - got));
        if (k <= 0) return -1;
        got += (int)k;
    }
    return got;
}

// Over TCP (a two-byte length before each message)
static int ask_tcp(struct in_addr server, const unsigned char *q, int qlen, unsigned char *ans, int anslen) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    struct sockaddr_in sa = { .sin_family = AF_INET, .sin_port = htons(53), .sin_addr = server };
    struct timeval tv = { conf.timeout, 0 };
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    if (connect(fd, (struct sockaddr *)&sa, sizeof sa) < 0) {
        close(fd);
        return -1;
    }
    unsigned char len2[2] = { (unsigned char)(qlen >> 8), (unsigned char)qlen };
    if (write(fd, len2, 2) != 2 || write(fd, q, (size_t)qlen) != qlen || read_full(fd, len2, 2, conf.timeout * 1000) != 2) {
        close(fd);
        return -1;
    }
    int n = len2[0] << 8 | len2[1];
    unsigned char *buf = malloc((size_t)n);
    if (!buf) {
        close(fd);
        return -1;
    }
    int r = read_full(fd, buf, n, conf.timeout * 1000);
    close(fd);
    if (r != n || n < 12 || buf[0] != q[0] || buf[1] != q[1]) {
        free(buf);
        return -1;
    }
    int k = n < anslen ? n : anslen;
    memcpy(ans, buf, (size_t)k);
    free(buf);
    return k;
}

// One query, to every server in turn: the answer's length, or -1 with
// h_errno
int res_query(const char *name, int cls, int type, unsigned char *ans, int anslen) {
    res_init();
    struct in_addr servers[3];
    int n = 0;
    if (have_forced) servers[n++] = forced;
    else
        for (int i = 0; i < conf.nns; i++) servers[n++] = conf.ns[i];
    if (!n) {
        h_errno = TRY_AGAIN;  // no servers (no DHCP lease yet?)
        return -1;
    }
    unsigned char q[512];
    uint16_t id = random_id();
    int qlen = make_query(name, cls, type, id, q, sizeof q);
    if (qlen < 0) {
        h_errno = NO_RECOVERY;
        return -1;
    }
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) {
        h_errno = TRY_AGAIN;
        return -1;
    }
    int result = -1;
    h_errno = TRY_AGAIN;
    unsigned char buf[4096];
    for (int attempt = 0; attempt < conf.attempts && result < 0; attempt++) {
        for (int i = 0; i < n && result < 0; i++) {
            struct sockaddr_in sa = { .sin_family = AF_INET, .sin_port = htons(53), .sin_addr = servers[i] };
            inet_ntop(AF_INET, &servers[i], last_server, sizeof last_server);
            if (sendto(fd, q, (size_t)qlen, 0, (struct sockaddr *)&sa, sizeof sa) < 0) continue;
            uint64_t deadline = rx_clock_get_monotonic() + (uint64_t)conf.timeout * 1000000000ull;
            for (;;) {
                uint64_t now = rx_clock_get_monotonic();
                if (now >= deadline) break;
                struct pollfd pf = { fd, POLLIN, 0 };
                if (poll(&pf, 1, (int)((deadline - now) / 1000000) + 1) <= 0) break;
                struct sockaddr_in from;
                socklen_t fl = sizeof from;
                ssize_t k = recvfrom(fd, buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
                if (k < 12 || buf[0] != q[0] || buf[1] != q[1] || !(buf[2] & 0x80)) continue; // not our answer
                if (from.sin_addr.s_addr != servers[i].s_addr) continue;
                int rcode = buf[3] & 15;
                if (rcode == 3) {
                    h_errno = HOST_NOT_FOUND;  // NXDOMAIN: no server will say otherwise
                    close(fd);
                    return -1;
                }
                if (rcode != 0) break;  // SERVFAIL, REFUSED: the next server
                if (buf[2] & 0x02) {
                    // Truncated: again over TCP
                    int t = ask_tcp(servers[i], q, qlen, ans, anslen);
                    if (t > 0) result = t;
                    break;
                }
                int c = (int)k < anslen ? (int)k : anslen;
                memcpy(ans, buf, (size_t)c);
                result = c;
                break;
            }
        }
    }
    close(fd);
    if (result > 0) {
        int count = ans[6] << 8 | ans[7];
        if (count == 0) {
            h_errno = NO_DATA;
            return -1;
        }
    }
    return result;
}

int res_search(const char *name, int cls, int type, unsigned char *ans, int anslen) {
    res_init();
    size_t len = strlen(name);
    if (len && name[len - 1] == '.') {
        char abs[NS_MAXDNAME];
        snprintf(abs, sizeof abs, "%.*s", (int)len - 1, name);
        return res_query(abs, cls, type, ans, anslen);
    }
    int dots = 0;
    for (const char *p = name; *p; p++) dots += *p == '.';
    int err = HOST_NOT_FOUND;
    if (dots >= conf.ndots) {
        int r = res_query(name, cls, type, ans, anslen);
        if (r >= 0 || h_errno == TRY_AGAIN) return r;
        err = h_errno;
    }
    for (int i = 0; i < conf.nsearch; i++) {
        char full[NS_MAXDNAME];
        snprintf(full, sizeof full, "%s.%s", name, conf.search[i]);
        int r = res_query(full, cls, type, ans, anslen);
        if (r >= 0) return r;
        if (h_errno == TRY_AGAIN) return -1;
    }
    if (dots < conf.ndots) {
        int r = res_query(name, cls, type, ans, anslen);
        if (r >= 0) return r;
        err = h_errno;
    }
    h_errno = err;
    return -1;
}

int dn_expand(const unsigned char *msg, const unsigned char *eom, const unsigned char *src, char *dst, int dstsiz) {
    const unsigned char *p = src;
    int out = 0, used = -1, jumps = 0;
    if (dstsiz < 1) return -1;
    for (;;) {
        if (p >= eom) return -1;
        unsigned len = *p;
        if (len == 0) {
            if (used < 0) used = (int)(p + 1 - src);
            break;
        }
        if ((len & 0xc0) == 0xc0) {
            if (p + 1 >= eom || ++jumps > 32) return -1;
            if (used < 0) used = (int)(p + 2 - src);
            p = msg + ((len & 0x3f) << 8 | p[1]);
            continue;
        }
        if (len > 63 || p + 1 + len > eom) return -1;
        if (out && out + 1 < dstsiz) dst[out++] = '.';
        if (out + (int)len >= dstsiz) return -1;
        memcpy(dst + out, p + 1, len);
        out += (int)len;
        p += 1 + len;
    }
    if (out == 0 && dstsiz > 1) dst[out++] = '.';
    dst[out] = 0;
    return used;
}

// ---- for getaddrinfo and getnameinfo ----

// The A records for name (following the server's CNAMEs): how many (at
// most max), the canonical name in canon; or an EAI_* error
int __dns_lookup4(const char *name, uint8_t (*addrs)[4], int max, char *canon, size_t clen) {
    unsigned char ans[4096];
    int n = res_search(name, C_IN, T_A, ans, sizeof ans);
    if (n < 0) return h_errno == HOST_NOT_FOUND || h_errno == NO_DATA ? EAI_NONAME : h_errno == TRY_AGAIN ? EAI_AGAIN : EAI_FAIL;
    const unsigned char *eom = ans + n, *p = ans + 12;
    int qd = ans[4] << 8 | ans[5], an = ans[6] << 8 | ans[7];
    char rname[NS_MAXDNAME];
    for (int i = 0; i < qd; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0) return EAI_FAIL;
        p += k + 4;
    }
    int count = 0;
    if (canon) snprintf(canon, clen, "%s", name);
    for (int i = 0; i < an && p < eom; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0 || p + k + 10 > eom) break;
        p += k;
        int type = p[0] << 8 | p[1], rdlen = p[8] << 8 | p[9];
        p += 10;
        if (p + rdlen > eom) break;
        if (type == T_A && rdlen == 4 && count < max) {
            memcpy(addrs[count++], p, 4);
            if (canon) snprintf(canon, clen, "%s", rname);
        }
        p += rdlen;
    }
    return count ? count : EAI_NONAME;
}

// The AAAA records for name: how many, or an EAI_* error
int __dns_lookup6(const char *name, uint8_t (*addrs)[16], int max, char *canon, size_t clen) {
    unsigned char ans[4096];
    int n = res_search(name, C_IN, T_AAAA, ans, sizeof ans);
    if (n < 0) return h_errno == HOST_NOT_FOUND || h_errno == NO_DATA ? EAI_NONAME : h_errno == TRY_AGAIN ? EAI_AGAIN : EAI_FAIL;
    const unsigned char *eom = ans + n, *p = ans + 12;
    int qd = ans[4] << 8 | ans[5], an = ans[6] << 8 | ans[7];
    char rname[NS_MAXDNAME];
    for (int i = 0; i < qd; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0) return EAI_FAIL;
        p += k + 4;
    }
    int count = 0;
    for (int i = 0; i < an && p < eom; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0 || p + k + 10 > eom) break;
        p += k;
        int type = p[0] << 8 | p[1], rdlen = p[8] << 8 | p[9];
        p += 10;
        if (p + rdlen > eom) break;
        if (type == T_AAAA && rdlen == 16 && count < max) {
            memcpy(addrs[count++], p, 16);
            if (canon) snprintf(canon, clen, "%s", rname);
        }
        p += rdlen;
    }
    return count ? count : EAI_NONAME;
}

// The name for an IPv4 address (PTR), or -1
int __dns_reverse4(const uint8_t a[4], char *name, size_t len) {
    char q[64];
    snprintf(q, sizeof q, "%u.%u.%u.%u.in-addr.arpa", a[3], a[2], a[1], a[0]);
    unsigned char ans[4096];
    int n = res_query(q, C_IN, T_PTR, ans, sizeof ans);
    if (n < 0) return -1;
    const unsigned char *eom = ans + n, *p = ans + 12;
    int qd = ans[4] << 8 | ans[5], an = ans[6] << 8 | ans[7];
    char rname[NS_MAXDNAME];
    for (int i = 0; i < qd; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0) return -1;
        p += k + 4;
    }
    for (int i = 0; i < an && p < eom; i++) {
        int k = dn_expand(ans, eom, p, rname, sizeof rname);
        if (k < 0 || p + k + 10 > eom) return -1;
        p += k;
        int type = p[0] << 8 | p[1], rdlen = p[8] << 8 | p[9];
        p += 10;
        if (type == T_PTR) {
            char out[NS_MAXDNAME];
            if (dn_expand(ans, eom, p, out, sizeof out) < 0) return -1;
            snprintf(name, len, "%s", out);
            return 0;
        }
        p += rdlen;
    }
    return -1;
}

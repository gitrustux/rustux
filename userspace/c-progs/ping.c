// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ping: send ICMP echo requests through netd (/svc/net) and print the
// replies, as Linux's ping does (docs/NETWORK.md, man ping). Anyone may
// ping: netd sends the echo requests, as Linux's unprivileged ICMP
// datagram sockets do.
//
//   ping [-c COUNT] [-i SECONDS] [-W SECONDS] [-s SIZE] [-q] HOST
//
// Without -c it pings until stopped (Ctrl+C). The exit status is 0 if a
// reply came, 1 if none did, 2 on an error.

#include "netclient.h"
#include <arpa/inet.h>
#include <netdb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static const char *prog = "ping";

static void usage(void) {
    fprintf(stderr, "usage: ping [-c COUNT] [-i SECONDS] [-W SECONDS] [-s SIZE] [-q] ADDRESS\n");
    exit(2);
}

// Seconds with decimals, in milliseconds ("0.2" -> 200)
static long millis(const char *s) {
    long whole = 0, frac = 0, scale = 1000;
    const char *p = s;
    if (!*p) usage();
    while (*p >= '0' && *p <= '9') whole = whole * 10 + (*p++ - '0');
    if (*p == '.') {
        p++;
        while (*p >= '0' && *p <= '9') {
            if (scale > 1) {
                scale /= 10;
                frac += (*p - '0') * scale;
            }
            p++;
        }
    }
    if (*p) {
        fprintf(stderr, "%s: %s: not a number of seconds\n", prog, s);
        exit(2);
    }
    return whole * 1000 + frac;
}

static unsigned long long isqrt(unsigned long long v) {
    unsigned long long r = 0, b = 1ULL << 62;
    while (b > v) b >>= 2;
    while (b) {
        if (v >= r + b) {
            v -= r + b;
            r = (r >> 1) + b;
        } else {
            r >>= 1;
        }
        b >>= 2;
    }
    return r;
}

static void ms(unsigned long long us, char *out, size_t n) {
    snprintf(out, n, "%llu.%03llu", us / 1000, us % 1000);
}

int main(int argc, char **argv) {
    long count = -1, interval = 1000, wait = 1000, size = 56;
    int quiet = 0, opt;
    while ((opt = getopt(argc, argv, "c:i:W:w:s:qh4")) != -1) {
        switch (opt) {
        case 'c': count = strtol(optarg, 0, 10); if (count <= 0) usage(); break;
        case 'i': interval = millis(optarg); break;
        case 'W': wait = millis(optarg); break;
        case 'w': count = millis(optarg) / 1000; if (count <= 0) count = 1; break;
        case 's': size = strtol(optarg, 0, 10); if (size < 0 || size > 1000) { fprintf(stderr, "%s: the size is 0 to 1000 bytes\n", prog); return 2; } break;
        case 'q': quiet = 1; break;
        case '4': break;
        default: usage();
        }
    }
    if (optind != argc - 1) usage();
    const char *host = argv[optind];
    // A name: through the resolver (/etc/hosts, DNS)
    char addr[INET_ADDRSTRLEN];
    struct addrinfo hints, *res;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_DGRAM;
    int e = getaddrinfo(host, 0, &hints, &res);
    if (e) {
        fprintf(stderr, "%s: %s: %s\n", prog, host, gai_strerror(e));
        return 2;
    }
    inet_ntop(AF_INET, &((struct sockaddr_in *)res->ai_addr)->sin_addr, addr, sizeof addr);
    freeaddrinfo(res);
    if (interval < 200) interval = 200;  // as Linux's limit for users
    if (wait < 1) wait = 1;

    net_connect(prog);
    printf("PING %s (%s) %ld(%ld) bytes of data.\n", host, addr, size, size + 28);
    fflush(stdout);
    long sent = 0, got = 0;
    unsigned long long tmin = ~0ULL, tmax = 0, tsum = 0, tsum2 = 0;
    uint64_t start = rx_clock_get_monotonic();
    char req[128], ans[512];
    for (long seq = 1; count < 0 || seq <= count; seq++) {
        uint64_t t0 = rx_clock_get_monotonic();
        snprintf(req, sizeof req, "ping %s %ld %ld %ld", addr, seq, wait, size);
        if (net_send(req) < 0 || net_recv(ans, sizeof ans) < 0) {
            fprintf(stderr, "%s: the network service did not answer\n", prog);
            return 2;
        }
        sent++;
        char *nl = strchr(ans, '\n');
        if (nl) *nl = 0;
        if (!strncmp(ans, "error: ", 7)) {
            fprintf(stderr, "%s: %s: %s\n", prog, host, ans + 7);
            return 2;
        }
        char from[64];
        unsigned long rseq, bytes;
        unsigned long long us;
        if (!strncmp(ans, "reply ", 6)) {
            // "reply FROM SEQ BYTES MICROSECONDS"
            char *w = ans + 6, *f[4];
            int n = 0;
            while (n < 4 && w) {
                f[n++] = w;
                w = strchr(w, ' ');
                if (w) *w++ = 0;
            }
            if (n == 4) {
                snprintf(from, sizeof from, "%s", f[0]);
                rseq = strtoul(f[1], 0, 10);
                bytes = strtoul(f[2], 0, 10);
                us = strtoull(f[3], 0, 10);
                got++;
                tsum += us;
                tsum2 += us * us;
                if (us < tmin) tmin = us;
                if (us > tmax) tmax = us;
                if (!quiet) {
                    char t[32];
                    ms(us, t, sizeof t);
                    printf("%lu bytes from %s: icmp_seq=%lu time=%s ms\n", bytes + 8, from, rseq, t);
                }
            }
        }
        fflush(stdout);
        if (count >= 0 && seq == count) break;
        uint64_t spent = (rx_clock_get_monotonic() - t0) / 1000000;
        if ((long)spent < interval) usleep((unsigned long)(interval - (long)spent) * 1000);
    }
    unsigned long long total = (rx_clock_get_monotonic() - start) / 1000000;
    printf("\n--- %s ping statistics ---\n", host);
    printf("%ld packets transmitted, %ld received, %ld%% packet loss, time %llums\n", sent, got,
           sent ? (sent - got) * 100 / sent : 0, total);
    if (got) {
        unsigned long long avg = tsum / got;
        unsigned long long var = tsum2 / got - avg * avg;
        char a[32], b[32], c[32], d[32];
        ms(tmin, a, sizeof a);
        ms(avg, b, sizeof b);
        ms(tmax, c, sizeof c);
        ms(isqrt(var), d, sizeof d);
        printf("rtt min/avg/max/mdev = %s/%s/%s/%s ms\n", a, b, c, d);
    }
    return got ? 0 : 1;
}

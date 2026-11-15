// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// nc (netcat): TCP and UDP connections from the command line, in the
// OpenBSD netcat's form (network step D2, docs/NETWORK.md, man nc).
//
//   nc [-uvzN] [-w SECONDS] [-s ADDRESS] [-p PORT] HOST PORT     connect
//   nc -l [-ukv] [-s ADDRESS] [-p] PORT                         listen
//
// Standard input goes to the connection, what comes back to standard
// output. At the end of standard input nc stops sending (a half-close);
// it ends when the other side closes too (with -N, or -w, it ends sooner).

#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

static int udp, verbose, zero, keep, listen_mode, quit_on_eof;
static int family = AF_UNSPEC;  // -4, -6
static socklen_t addr_len(const struct sockaddr_storage *a) {
    return a->ss_family == AF_INET6 ? sizeof(struct sockaddr_in6) : sizeof(struct sockaddr_in);
}
static int timeout_s = -1;
static const char *src_addr, *src_port;

static void usage(void) {
    fprintf(stderr,
            "usage: nc [-uvzN] [-w SECONDS] [-s ADDRESS] [-p PORT] HOST PORT\n"
            "       nc -l [-ukv] [-s ADDRESS] [-p] PORT\n");
    exit(1);
}

static void die(const char *what) {
    fprintf(stderr, "nc: %s: %s\n", what, strerror(errno));
    exit(1);
}

static int resolve(const char *host, const char *port, int passive, struct sockaddr_storage *out) {
    struct addrinfo hints, *res;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = family != AF_UNSPEC ? family : passive && !host ? AF_INET : AF_UNSPEC;
    hints.ai_socktype = udp ? SOCK_DGRAM : SOCK_STREAM;
    hints.ai_flags = passive ? AI_PASSIVE : 0;
    int e = getaddrinfo(host, port, &hints, &res);
    if (e) {
        fprintf(stderr, "nc: %s%s%s: %s\n", host ? host : "", host ? " " : "", port ? port : "", gai_strerror(e));
        return -1;
    }
    memset(out, 0, sizeof *out);
    memcpy(out, res->ai_addr, res->ai_addrlen);
    freeaddrinfo(res);
    return 0;
}

static const char *show(const struct sockaddr_storage *a) {
    static char buf[64];
    char ip[INET6_ADDRSTRLEN];
    unsigned port;
    if (a->ss_family == AF_INET6) {
        const struct sockaddr_in6 *in6 = (const void *)a;
        inet_ntop(AF_INET6, in6->sin6_addr.s6_addr, ip, sizeof ip);
        port = ntohs(in6->sin6_port);
    } else {
        const struct sockaddr_in *in = (const void *)a;
        inet_ntop(AF_INET, &in->sin_addr, ip, sizeof ip);
        port = ntohs(in->sin_port);
    }
    snprintf(buf, sizeof buf, "%s %u", ip, port);
    return buf;
}

static int write_all(int fd, const char *p, size_t n) {
    while (n) {
        ssize_t k = write(fd, p, n);
        if (k < 0) return -1;
        p += k;
        n -= (size_t)k;
    }
    return 0;
}

// Copy both ways until the connection ends
static void transfer(int s) {
    char buf[16384];
    int in_open = 1;
    struct pollfd p[2];
    for (;;) {
        int n = 0;
        p[n].fd = s;
        p[n].events = POLLIN;
        n++;
        if (in_open) {
            p[n].fd = 0;
            p[n].events = POLLIN;
            n++;
        }
        int r = poll(p, (nfds_t)n, timeout_s >= 0 && !in_open ? timeout_s * 1000 : -1);
        if (r < 0) die("poll");
        if (r == 0) return; // -w: quiet for that long after the input ended
        if (p[0].revents & (POLLIN | POLLHUP | POLLERR)) {
            ssize_t k = read(s, buf, sizeof buf);
            if (k < 0) {
                if (errno == EAGAIN) continue;
                if (!udp) fprintf(stderr, "nc: read: %s\n", strerror(errno));
                return;
            }
            if (k == 0) return;
            if (write_all(1, buf, (size_t)k) < 0) return;
        }
        if (in_open && n > 1 && (p[1].revents & (POLLIN | POLLHUP | POLLERR))) {
            ssize_t k = read(0, buf, sizeof buf);
            if (k <= 0) {
                in_open = 0;
                if (!udp) shutdown(s, SHUT_WR);
                if (quit_on_eof || udp) {
                    if (udp && timeout_s < 0) return;
                    if (!udp) return;
                }
                continue;
            }
            if (udp) {
                if (send(s, buf, (size_t)k, 0) < 0) die("send");
            } else if (write_all(s, buf, (size_t)k) < 0) {
                fprintf(stderr, "nc: write: %s\n", strerror(errno));
                return;
            }
        }
    }
}

static int bind_source(int s) {
    if (!src_addr && !src_port) return 0;
    struct sockaddr_storage a;
    if (resolve(src_addr, src_port ? src_port : "0", 1, &a) < 0) return -1;
    if (bind(s, (struct sockaddr *)&a, addr_len(&a)) < 0) die("bind");
    return 0;
}

static int connect_one(const char *host, const char *port) {
    struct sockaddr_storage a;
    if (resolve(host, port, 0, &a) < 0) return 1;
    int s = socket(a.ss_family, udp ? SOCK_DGRAM : SOCK_STREAM, 0);
    if (s < 0) die("socket");
    int fam = family;
    if (family == AF_UNSPEC) family = a.ss_family;  // the source address of the same family
    if (bind_source(s) < 0) return 1;
    family = fam;
    if (timeout_s > 0 && !udp) {
        struct timeval tv = { timeout_s, 0 };
        setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    }
    if (connect(s, (struct sockaddr *)&a, addr_len(&a)) < 0) {
        if (verbose || !zero) fprintf(stderr, "nc: connect to %s %s (%s) failed: %s\n", host, port, udp ? "udp" : "tcp", strerror(errno));
        close(s);
        return 1;
    }
    if (verbose) fprintf(stderr, "Connection to %s %s port [%s/*] succeeded!\n", host, port, udp ? "udp" : "tcp");
    if (!zero) transfer(s);
    close(s);
    return 0;
}

static int listen_on(const char *port) {
    struct sockaddr_storage a;
    if (resolve(src_addr, port, 1, &a) < 0) return 1;
    int s = socket(a.ss_family, udp ? SOCK_DGRAM : SOCK_STREAM, 0);
    if (s < 0) die("socket");
    int on = 1;
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
    if (bind(s, (struct sockaddr *)&a, addr_len(&a)) < 0) die("bind");
    if (verbose) fprintf(stderr, "Listening on %s\n", show(&a));
    if (udp) {
        // The first datagram says who the peer is
        char buf[65536];
        struct sockaddr_storage from;
        socklen_t fl = sizeof from;
        ssize_t k = recvfrom(s, buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
        if (k < 0) die("recvfrom");
        if (verbose) fprintf(stderr, "Connection received on %s\n", show(&from));
        write_all(1, buf, (size_t)k);
        if (connect(s, (struct sockaddr *)&from, addr_len(&from)) < 0) die("connect");
        transfer(s);
        close(s);
        return 0;
    }
    if (listen(s, keep ? 8 : 1) < 0) die("listen");
    do {
        struct sockaddr_storage from;
        socklen_t fl = sizeof from;
        int c = accept(s, (struct sockaddr *)&from, &fl);
        if (c < 0) die("accept");
        if (verbose) fprintf(stderr, "Connection received on %s\n", show(&from));
        transfer(c);
        close(c);
    } while (keep);
    close(s);
    return 0;
}

int main(int argc, char **argv) {
    int opt;
    while ((opt = getopt(argc, argv, "luvzkNw:s:p:h46")) != -1) {
        switch (opt) {
        case 'l': listen_mode = 1; break;
        case 'u': udp = 1; break;
        case 'v': verbose = 1; break;
        case 'z': zero = 1; break;
        case 'k': keep = 1; break;
        case 'N': quit_on_eof = 1; break;
        case 'w': timeout_s = atoi(optarg); break;
        case 's': src_addr = optarg; break;
        case 'p': src_port = optarg; break;
        case '4': family = AF_INET; break;
        case '6': family = AF_INET6; break;
        default: usage();
        }
    }
    int left = argc - optind;
    if (listen_mode) {
        const char *port = src_port;
        if (left == 1) port = argv[optind];
        else if (left == 2) { src_addr = argv[optind]; port = argv[optind + 1]; }
        else if (left != 0) usage();
        if (!port) usage();
        return listen_on(port);
    }
    if (left != 2) usage();
    return connect_one(argv[optind], argv[optind + 1]);
}

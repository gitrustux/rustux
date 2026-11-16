// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// netcfg: the network's configuration (network step D6, docs/NETWORK.md,
// man netcfg): changes /etc/network.conf and has netd apply it at once,
// without a restart. Settings > Network runs it.
//
//   netcfg                                  each interface: method, state,
//                                           addresses, gateway; DNS
//   netcfg IFACE dhcp                       an address from DHCP
//   netcfg IFACE static ADDRESS/PREFIX [GATEWAY]
//   netcfg IFACE off                        left alone
//   netcfg dns [ADDRESS...]                 DNS servers before DHCP's (none: DHCP's only)
//   netcfg search [DOMAIN...]               domains tried for short names
//   netcfg apply [IFACE]                    /etc/network.conf again (after editing it)
//
// Changes are root's. Comments in the file are kept; an interface's
// static6 lines are left alone.

#include "netclient.h"
#include <arpa/inet.h>
#include <ctype.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static const char *prog = "netcfg";
#define CONF "/etc/network.conf"
#define MAXLINES 512

static void usage(void) {
    fprintf(stderr,
            "usage: netcfg\n"
            "       netcfg IFACE dhcp | static ADDRESS/PREFIX [GATEWAY] | off\n"
            "       netcfg dns [ADDRESS...]\n"
            "       netcfg search [DOMAIN...]\n"
            "       netcfg apply [IFACE]\n");
    exit(1);
}

static char *lines[MAXLINES];
static int nlines;

static void load(void) {
    FILE *f = fopen(CONF, "r");
    if (!f) return;
    char buf[512];
    while (nlines < MAXLINES && fgets(buf, sizeof buf, f)) {
        buf[strcspn(buf, "\n")] = 0;
        lines[nlines++] = strdup(buf);
    }
    fclose(f);
}

static void save(void) {
    const char *tmp = CONF ".new";
    FILE *f = fopen(tmp, "w");
    if (!f) {
        fprintf(stderr, "%s: %s: %s\n", prog, tmp, strerror(errno));
        exit(1);
    }
    for (int i = 0; i < nlines; i++)
        if (lines[i]) fprintf(f, "%s\n", lines[i]);
    if (fclose(f) != 0 || rename(tmp, CONF) != 0) {
        fprintf(stderr, "%s: %s: %s\n", prog, CONF, strerror(errno));
        unlink(tmp);
        exit(1);
    }
}

// The line's first two words (not in a comment)
static void words(const char *line, char *w1, char *w2, size_t cap) {
    w1[0] = w2[0] = 0;
    char tmp[512];
    snprintf(tmp, sizeof tmp, "%s", line);
    char *hash = strchr(tmp, '#');
    if (hash) *hash = 0;
    char *save = 0;
    char *a = strtok_r(tmp, " \t", &save);
    char *b = a ? strtok_r(0, " \t", &save) : 0;
    if (a) snprintf(w1, cap, "%s", a);
    if (b) snprintf(w2, cap, "%s", b);
}

// Replace the lines match() selects with the new line (in the
// first one's place, else at the end); new_line 0: only remove them
static void replace(int (*match)(const char *w1, const char *w2, const char *key), const char *key, const char *new_line) {
    int at = -1;
    for (int i = 0; i < nlines; i++) {
        if (!lines[i]) continue;
        char w1[64], w2[64];
        words(lines[i], w1, w2, sizeof w1);
        if (match(w1, w2, key)) {
            if (at < 0) at = i;
            free(lines[i]);
            lines[i] = 0;
        }
    }
    if (!new_line) return;
    if (at >= 0) {
        lines[at] = strdup(new_line);
    } else if (nlines < MAXLINES) {
        lines[nlines++] = strdup(new_line);
    }
}

static int match_iface(const char *w1, const char *w2, const char *iface) {
    return !strcmp(w1, iface) && (!strcmp(w2, "dhcp") || !strcmp(w2, "static") || !strcmp(w2, "off") || !strcmp(w2, "manual"));
}

static int match_key(const char *w1, const char *w2, const char *key) {
    (void)w2;
    return !strcmp(w1, key);
}

static int valid_v4(const char *s) {
    struct in_addr a;
    return inet_pton(AF_INET, s, &a) == 1;
}

static int valid_cidr(const char *s) {
    char a[64];
    const char *slash = strchr(s, '/');
    if (!slash || (size_t)(slash - s) >= sizeof a) return 0;
    snprintf(a, sizeof a, "%.*s", (int)(slash - s), s);
    char *end;
    long p = strtol(slash + 1, &end, 10);
    return valid_v4(a) && *end == 0 && slash[1] && p >= 0 && p <= 32;
}

static int valid_iface(const char *s) {
    if (!isalpha((unsigned char)s[0]) || strlen(s) > 15) return 0;
    for (const char *p = s; *p; p++)
        if (!isalnum((unsigned char)*p)) return 0;
    return strcmp(s, "dns") && strcmp(s, "search") && strcmp(s, "lo");
}

static void need_root(void) {
    if (geteuid() != 0) {
        fprintf(stderr, "%s: changing the network needs root (sudo netcfg ...)\n", prog);
        exit(1);
    }
}

// The method /etc/network.conf gives an interface ("dhcp" when none)
static void method_of(const char *iface, char *out, size_t cap) {
    snprintf(out, cap, "dhcp (no line in " CONF ")");
    for (int i = 0; i < nlines; i++) {
        if (!lines[i]) continue;
        char w1[64], w2[64];
        words(lines[i], w1, w2, sizeof w1);
        if (match_iface(w1, w2, iface)) {
            char tmp[512];
            snprintf(tmp, sizeof tmp, "%s", lines[i]);
            char *hash = strchr(tmp, '#');
            if (hash) *hash = 0;
            char *p = tmp + strspn(tmp, " \t");
            p += strcspn(p, " \t");
            p += strspn(p, " \t");
            size_t n = strlen(p);
            while (n && (p[n - 1] == ' ' || p[n - 1] == '\t')) p[--n] = 0;
            snprintf(out, cap, "%s", p);
            return;
        }
    }
}

// The k-th word (from 0) of a line into out ("" when there are fewer)
static const char *word(const char *line, int k, char *out, size_t cap) {
    out[0] = 0;
    const char *p = line;
    for (int i = 0;; i++) {
        p += strspn(p, " \t");
        if (!*p) return out;
        size_t n = strcspn(p, " \t");
        if (i == k) {
            snprintf(out, cap, "%.*s", (int)n, p);
            return out;
        }
        p += n;
    }
}

static void show(void) {
    load();
    // Copies: net_request's buffer is reused
    char *links = strdup(net_request(prog, "links"));
    char *addrs = strdup(net_request(prog, "addrs"));
    char *routes = strdup(net_request(prog, "routes"));
    char *dns = strdup(net_request(prog, "dns"));
    char *save = 0;
    // NAME DRIVER MAC STATE mtu N rx ...
    for (char *l = strtok_r(links, "\n", &save); l; l = strtok_r(0, "\n", &save)) {
        char name[32], driver[64], mac[32], state[32];
        word(l, 0, name, sizeof name), word(l, 1, driver, sizeof driver), word(l, 2, mac, sizeof mac), word(l, 3, state, sizeof state);
        if (!state[0] || !strcmp(name, "lo")) continue;
        char method[256];
        method_of(name, method, sizeof method);
        for (char *p = driver; *p; p++)
            if (*p == '_') *p = ' ';
        printf("%s: %s, %s (%s, %s)\n", name, method, state, driver, mac);
        char *s2 = 0, *copy = strdup(addrs);
        for (char *a = strtok_r(copy, "\n", &s2); a; a = strtok_r(0, "\n", &s2)) {
            char n[32], addr[80], kind[16], left[32];
            word(a, 0, n, sizeof n), word(a, 1, addr, sizeof addr), word(a, 2, kind, sizeof kind), word(a, 3, left, sizeof left);
            if (!addr[0] || strcmp(n, name)) continue;
            if (!strcmp(kind, "dynamic") && strchr(addr, '.'))
                printf("  address  %s (DHCP, %s s left)\n", addr, left);
            else if (!strcmp(kind, "dynamic"))
                printf("  address  %s (from the router)\n", addr);
            else
                printf("  address  %s\n", addr);
        }
        free(copy);
        copy = strdup(routes);
        for (char *r = strtok_r(copy, "\n", &s2); r; r = strtok_r(0, "\n", &s2)) {
            char w0[16], via[80], dev[32];
            word(r, 0, w0, sizeof w0), word(r, 2, via, sizeof via), word(r, 4, dev, sizeof dev);
            if (!strcmp(w0, "default") && !strcmp(dev, name)) printf("  gateway  %s\n", via);
        }
        free(copy);
    }
    // DNS: network.conf's own, then DHCP's
    printf("DNS:");
    int any = 0;
    for (int i = 0; i < nlines; i++) {
        char w1[64], w2[64];
        if (!lines[i]) continue;
        words(lines[i], w1, w2, sizeof w1);
        if (!strcmp(w1, "dns") || !strcmp(w1, "search")) {
            char tmp[512];
            snprintf(tmp, sizeof tmp, "%s", lines[i]);
            char *hash = strchr(tmp, '#');
            if (hash) *hash = 0;
            char *s3 = 0;
            strtok_r(tmp, " \t", &s3);
            for (char *w = strtok_r(0, " \t", &s3); w; w = strtok_r(0, " \t", &s3)) {
                printf(" %s%s", w, !strcmp(w1, "search") ? " (search)" : "");
                any = 1;
            }
        }
    }
    for (char *d = strtok_r(dns, "\n", &save); d; d = strtok_r(0, "\n", &save)) {
        char n[32], a[80];
        word(d, 0, n, sizeof n), word(d, 1, a, sizeof a);
        if (a[0]) {
            printf(" %s (DHCP on %s)", a, n);
            any = 1;
        }
    }
    printf("%s\n", any ? "" : " none");
}

static void apply(const char *iface) {
    char req[64];
    snprintf(req, sizeof req, iface ? "reconfigure %s" : "reconfigure", iface);
    net_request(prog, req);
}

int main(int argc, char **argv) {
    if (argc == 1 || !strcmp(argv[1], "show")) {
        show();
        return 0;
    }
    const char *a = argv[1];
    if (!strcmp(a, "-h") || !strcmp(a, "--help")) usage();
    if (!strcmp(a, "apply")) {
        need_root();
        apply(argc > 2 ? argv[2] : 0);
        return 0;
    }
    if (!strcmp(a, "dns") || !strcmp(a, "search")) {
        need_root();
        char line[512];
        snprintf(line, sizeof line, "%s", a);
        for (int i = 2; i < argc; i++) {
            if (!strcmp(a, "dns") && !valid_v4(argv[i])) {
                fprintf(stderr, "%s: %s: not an IPv4 address\n", prog, argv[i]);
                return 1;
            }
            size_t n = strlen(line);
            snprintf(line + n, sizeof line - n, " %s", argv[i]);
        }
        load();
        replace(match_key, a, argc > 2 ? line : 0);
        save();
        net_request(prog, "resolv");
        printf("%s: %s\n", prog, argc > 2 ? line : (!strcmp(a, "dns") ? "dns: DHCP's only" : "search: none"));
        return 0;
    }
    if (argc < 3) usage();
    if (!valid_iface(a)) {
        fprintf(stderr, "%s: %s: not an interface name (eth0, eth1, ...)\n", prog, a);
        return 1;
    }
    const char *m = argv[2];
    char line[256];
    if ((!strcmp(m, "dhcp") || !strcmp(m, "off")) && argc == 3) {
        snprintf(line, sizeof line, "%s %s", a, m);
    } else if (!strcmp(m, "static") && (argc == 4 || argc == 5)) {
        if (!valid_cidr(argv[3])) {
            fprintf(stderr, "%s: %s: not an IPv4 address with its prefix (10.0.0.5/24)\n", prog, argv[3]);
            return 1;
        }
        if (argc == 5 && !valid_v4(argv[4])) {
            fprintf(stderr, "%s: %s: not an IPv4 address\n", prog, argv[4]);
            return 1;
        }
        snprintf(line, sizeof line, "%s static %s%s%s", a, argv[3], argc == 5 ? " " : "", argc == 5 ? argv[4] : "");
    } else {
        usage();
    }
    need_root();
    load();
    replace(match_iface, a, line);
    save();
    // The interface may not be there yet (a card plugged in later): the
    // line waits for it
    char *links = net_request(prog, "links");
    int present = 0;
    size_t n = strlen(a);
    for (char *l = links; l && *l; l = strchr(l, '\n') ? strchr(l, '\n') + 1 : 0)
        if (!strncmp(l, a, n) && l[n] == ' ') present = 1;
    if (present) apply(a);
    printf("%s: %s%s\n", prog, line, present ? "" : " (for when the interface appears)");
    return 0;
}

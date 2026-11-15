// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ip: show and change the network interfaces, addresses and routes (netd,
// /svc/net), in iproute2's form (docs/NETWORK.md, man ip).
//
//   ip [-s] [-br] link [show [IF]]
//   ip link set IF up|down
//   ip [-br] addr [show [dev] IF]
//   ip addr add|del ADDRESS/PREFIX dev IF
//   ip route [show]
//   ip route add default via GATEWAY
//   ip route del default
//
// Objects and commands may be shortened as in iproute2 (ip a, ip r, ip l).
// Changes are root's (sudo ip ...); they last until netd restarts:
// /etc/network.conf has the configuration at boot.

#include "netclient.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *prog = "ip";
static int stats, brief;

static int is(const char *word, const char *full) {
    size_t n = strlen(word);
    return n > 0 && n <= strlen(full) && !strncmp(word, full, n);
}

static void usage(void) {
    fprintf(stderr,
            "usage: ip [-s] [-br] link [show [IF]] | link set IF up|down\n"
            "       ip [-br] addr [show [dev] IF] | addr add|del ADDRESS/PREFIX dev IF\n"
            "       ip route [show] | route add default via GATEWAY | route del default\n");
    exit(1);
}

// One interface from "links": name driver mac state mtu MTU rx N tx N RXB TXB
typedef struct {
    char name[32], driver[64], mac[32], state[16];
    unsigned mtu;
    unsigned long long rx, tx, rxb, txb;
} link_t;

static link_t links[32];
static int nlinks;

static void load_links(void) {
    char *text = net_request(prog, "links");
    nlinks = 0;
    for (char *line = strtok(text, "\n"); line && nlinks < 32; line = strtok(0, "\n")) {
        if (!strcmp(line, "ok")) break;
        link_t *l = &links[nlinks];
        char *w[16];
        int n = 0;
        for (char *p = line; *p && n < 16;) {
            while (*p == ' ') p++;
            if (!*p) break;
            w[n++] = p;
            while (*p && *p != ' ') p++;
            if (*p) *p++ = 0;
        }
        if (n < 12) continue;
        snprintf(l->name, sizeof l->name, "%s", w[0]);
        snprintf(l->driver, sizeof l->driver, "%s", w[1]);
        for (char *c = l->driver; *c; c++)
            if (*c == '_') *c = ' ';
        snprintf(l->mac, sizeof l->mac, "%s", w[2]);
        snprintf(l->state, sizeof l->state, "%s", w[3]);
        l->mtu = (unsigned)strtoul(w[5], 0, 10);
        l->rx = strtoull(w[7], 0, 10);
        l->tx = strtoull(w[9], 0, 10);
        l->rxb = strtoull(w[10], 0, 10);
        l->txb = strtoull(w[11], 0, 10);
        nlinks++;
    }
}

static const char *upper_state(const link_t *l) {
    return !strcmp(l->state, "up") ? "UP" : !strcmp(l->state, "down") ? "DOWN" : "DOWN";
}

static void show_link_head(int i, const link_t *l) {
    int lo = !strcmp(l->name, "lo");
    const char *flags = lo ? "LOOPBACK" : "BROADCAST,MULTICAST";
    if (!strcmp(l->state, "up"))
        printf("%d: %s: <%s,UP,LOWER_UP> mtu %u state UP\n", i + 1, l->name, flags, l->mtu);
    else if (!strcmp(l->state, "no-carrier"))
        printf("%d: %s: <NO-CARRIER,%s,UP> mtu %u state DOWN\n", i + 1, l->name, flags, l->mtu);
    else
        printf("%d: %s: <%s> mtu %u state DOWN\n", i + 1, l->name, flags, l->mtu);
    if (lo)
        printf("    link/loopback %s\n", l->mac);
    else
        printf("    link/ether %s brd ff:ff:ff:ff:ff:ff (%s)\n", l->mac, l->driver);
}

static int find_link(const char *name) {
    for (int i = 0; i < nlinks; i++)
        if (!strcmp(links[i].name, name)) return i;
    fprintf(stderr, "%s: %s: no such interface\n", prog, name);
    exit(1);
}

static void link_show(const char *only) {
    load_links();
    int one = only ? find_link(only) : -1;
    for (int i = 0; i < nlinks; i++) {
        if (one >= 0 && i != one) continue;
        link_t *l = &links[i];
        if (brief) {
            printf("%-16s %-14s %s\n", l->name, upper_state(l), l->mac);
            continue;
        }
        show_link_head(i, l);
        if (stats) {
            printf("    RX:  bytes  packets\n    %10llu %8llu\n", l->rxb, l->rx);
            printf("    TX:  bytes  packets\n    %10llu %8llu\n", l->txb, l->tx);
        }
    }
}

static void addr_show(const char *only) {
    load_links();
    int one = only ? find_link(only) : -1;
    char *text = net_request(prog, "addrs");
    // "IF ADDRESS/PREFIX" lines
    char *addrs[64][2];
    int na = 0;
    for (char *line = strtok(text, "\n"); line && na < 64; line = strtok(0, "\n")) {
        if (!strcmp(line, "ok")) break;
        char *sp = strchr(line, ' ');
        if (!sp) continue;
        *sp = 0;
        addrs[na][0] = line;
        addrs[na][1] = sp + 1;
        na++;
    }
    for (int i = 0; i < nlinks; i++) {
        if (one >= 0 && i != one) continue;
        link_t *l = &links[i];
        int lo = !strcmp(l->name, "lo");
        if (brief) {
            printf("%-16s %-14s", l->name, upper_state(l));
            for (int k = 0; k < na; k++)
                if (!strcmp(addrs[k][0], l->name)) printf(" %s", addrs[k][1]);
            printf("\n");
            continue;
        }
        show_link_head(i, l);
        for (int k = 0; k < na; k++) {
            if (strcmp(addrs[k][0], l->name)) continue;
            int v6 = strchr(addrs[k][1], ':') != 0;
            printf("    %s %s scope %s %s\n", v6 ? "inet6" : "inet", addrs[k][1], lo ? "host" : "global", l->name);
        }
    }
}

int main(int argc, char **argv) {
    int i = 1;
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (!strcmp(argv[i], "-s") || !strcmp(argv[i], "-stats")) stats = 1;
        else if (!strcmp(argv[i], "-br") || !strcmp(argv[i], "-brief")) brief = 1;
        else if (!strcmp(argv[i], "-4")) {
        } else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help") || !strcmp(argv[i], "-help")) {
            usage();
        } else {
            fprintf(stderr, "%s: unknown option %s\n", prog, argv[i]);
            usage();
        }
    }
    if (i >= argc) usage();
    const char *obj = argv[i++];
    const char *cmd = i < argc ? argv[i++] : "show";
    char req[256];
    if (is(obj, "link")) {
        if (is(cmd, "show") || is(cmd, "list")) {
            link_show(i < argc ? (strcmp(argv[i], "dev") ? argv[i] : (i + 1 < argc ? argv[i + 1] : 0)) : 0);
        } else if (is(cmd, "set") && argc - i == 2 && (!strcmp(argv[i + 1], "up") || !strcmp(argv[i + 1], "down"))) {
            snprintf(req, sizeof req, "link set %s %s", argv[i], argv[i + 1]);
            net_request(prog, req);
        } else {
            usage();
        }
    } else if (is(obj, "addr") || is(obj, "address")) {
        if (is(cmd, "show") || is(cmd, "list")) {
            const char *dev = 0;
            if (i < argc) dev = strcmp(argv[i], "dev") ? argv[i] : (i + 1 < argc ? argv[i + 1] : 0);
            addr_show(dev);
        } else if ((!strcmp(cmd, "add") || is(cmd, "delete")) && argc - i == 3 && !strcmp(argv[i + 1], "dev")) {
            if (!strchr(argv[i], '/')) {
                fprintf(stderr, "%s: %s: give the prefix length too (ADDRESS/PREFIX, e.g. 10.0.2.15/24)\n", prog, argv[i]);
                return 1;
            }
            snprintf(req, sizeof req, "addr %s %s dev %s", !strcmp(cmd, "add") ? "add" : "del", argv[i], argv[i + 2]);
            net_request(prog, req);
        } else {
            usage();
        }
    } else if (is(obj, "route")) {
        if (is(cmd, "show") || is(cmd, "list")) {
            char *text = net_request(prog, "routes");
            char *ok = strstr(text, "ok\n");
            if (ok) *ok = 0;
            fputs(text, stdout);
        } else if (!strcmp(cmd, "add") && argc - i == 3 && !strcmp(argv[i], "default") && !strcmp(argv[i + 1], "via")) {
            snprintf(req, sizeof req, "route add default via %s", argv[i + 2]);
            net_request(prog, req);
        } else if (is(cmd, "delete") && i < argc && !strcmp(argv[i], "default")) {
            net_request(prog, "route del default");
        } else {
            usage();
        }
    } else {
        fprintf(stderr, "%s: unknown object %s (link, addr, route)\n", prog, obj);
        usage();
    }
    return 0;
}

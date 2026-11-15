// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// host, nslookup: ask DNS (network step D3, docs/NETWORK.md, man host),
// through the C library's resolver and /etc/resolv.conf.
//
//   host [-t TYPE] [-v] NAME|ADDRESS [SERVER]
//   nslookup [-type=TYPE] NAME|ADDRESS [SERVER]
//
// Without -t, host asks for A, AAAA and MX, as BIND's does; an address
// is looked up backwards (PTR).

#include <arpa/inet.h>
#include <netdb.h>
#include <resolv.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

static const struct { const char *name; int type; } types[] = {
    { "A", T_A }, { "NS", T_NS }, { "CNAME", T_CNAME }, { "SOA", T_SOA }, { "PTR", T_PTR },
    { "MX", T_MX }, { "TXT", T_TXT }, { "AAAA", T_AAAA }, { "SRV", T_SRV }, { "ANY", T_ANY },
};

static int nslookup, verbose, shown;

static int type_of(const char *s) {
    for (size_t k = 0; k < sizeof types / sizeof *types; k++)
        if (!strcasecmp(types[k].name, s)) return types[k].type;
    return -1;
}

static const char *type_name(int t) {
    for (size_t k = 0; k < sizeof types / sizeof *types; k++)
        if (types[k].type == t) return types[k].name;
    return "?";
}

// Print the answers of one message
static void show(const unsigned char *ans, int n, const char *asked) {
    const unsigned char *eom = ans + n, *p = ans + 12;
    int qd = ans[4] << 8 | ans[5], an = ans[6] << 8 | ans[7];
    char name[NS_MAXDNAME], data[NS_MAXDNAME];
    for (int i = 0; i < qd; i++) {
        int k = dn_expand(ans, eom, p, name, sizeof name);
        if (k < 0) return;
        p += k + 4;
    }
    for (int i = 0; i < an && p < eom; i++) {
        int k = dn_expand(ans, eom, p, name, sizeof name);
        if (k < 0 || p + k + 10 > eom) return;
        p += k;
        int type = p[0] << 8 | p[1], rdlen = p[8] << 8 | p[9];
        unsigned ttl = (unsigned)p[4] << 24 | (unsigned)p[5] << 16 | (unsigned)p[6] << 8 | p[7];
        p += 10;
        if (p + rdlen > eom) return;
        char ip[INET6_ADDRSTRLEN];
        if (verbose) printf("%s.\t%u\tIN\t%s\t", name, ttl, type_name(type));
        switch (type) {
        case T_A:
            inet_ntop(AF_INET, p, ip, sizeof ip);
            if (verbose) printf("%s\n", ip);
            else if (nslookup) printf("Name:\t%s\nAddress: %s\n", name, ip);
            else printf("%s has address %s\n", name, ip);
            break;
        case T_AAAA:
            inet_ntop(AF_INET6, p, ip, sizeof ip);
            if (verbose) printf("%s\n", ip);
            else if (nslookup) printf("Name:\t%s\nAddress: %s\n", name, ip);
            else printf("%s has IPv6 address %s\n", name, ip);
            break;
        case T_CNAME:
            dn_expand(ans, eom, p, data, sizeof data);
            if (verbose) printf("%s.\n", data);
            else if (nslookup) printf("%s\tcanonical name = %s.\n", name, data);
            else printf("%s is an alias for %s.\n", name, data);
            break;
        case T_PTR:
            dn_expand(ans, eom, p, data, sizeof data);
            if (verbose) printf("%s.\n", data);
            else if (nslookup) printf("%s\tname = %s.\n", name, data);
            else printf("%s domain name pointer %s.\n", name, data);
            break;
        case T_MX:
            dn_expand(ans, eom, p + 2, data, sizeof data);
            if (verbose) printf("%d %s.\n", p[0] << 8 | p[1], data);
            else if (nslookup) printf("%s\tmail exchanger = %d %s.\n", name, p[0] << 8 | p[1], data);
            else printf("%s mail is handled by %d %s.\n", name, p[0] << 8 | p[1], data);
            break;
        case T_NS:
            dn_expand(ans, eom, p, data, sizeof data);
            if (verbose) printf("%s.\n", data);
            else printf("%s name server %s.\n", name, data);
            break;
        case T_TXT: {
            if (!verbose) printf("%s descriptive text ", name);
            for (int o = 0; o < rdlen;) {
                int l = p[o];
                printf("\"%.*s\"%s", l, (const char *)p + o + 1, o + 1 + l < rdlen ? " " : "");
                o += 1 + l;
            }
            printf("\n");
            break;
        }
        default:
            if (verbose) printf("(%d bytes)\n", rdlen);
            else printf("%s has %s record (%d bytes)\n", name, type_name(type), rdlen);
        }
        shown++;
        p += rdlen;
    }
    (void)asked;
}

static int ask(const char *name, int type, int report_missing) {
    unsigned char ans[8192];
    int n = res_search(name, C_IN, type, ans, sizeof ans);
    if (n < 0) {
        if (h_errno == HOST_NOT_FOUND) {
            if (nslookup) printf("** server can't find %s: NXDOMAIN\n", name);
            else printf("Host %s not found: 3(NXDOMAIN)\n", name);
            return 1;
        }
        if (h_errno == TRY_AGAIN) {
            if (nslookup) printf(";; connection timed out; no servers could be reached\n");
            else printf(";; connection timed out; no servers could be reached\n");
            return 1;
        }
        if (h_errno == NO_DATA && report_missing && !nslookup) printf("%s has no %s record\n", name, type_name(type));
        return h_errno == NO_DATA ? 0 : 1;
    }
    show(ans, n, name);
    return 0;
}

int main(int argc, char **argv) {
    const char *p0 = strrchr(argv[0], '/');
    nslookup = !strcmp(p0 ? p0 + 1 : argv[0], "nslookup");
    int type = 0, i = 1;
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (!strcmp(argv[i], "-t") && i + 1 < argc) type = type_of(argv[++i]);
        else if (!strncmp(argv[i], "-type=", 6) || !strncmp(argv[i], "-query=", 7)) type = type_of(strchr(argv[i], '=') + 1);
        else if (!strcmp(argv[i], "-v") || !strcmp(argv[i], "-d")) verbose = 1;
        else if (!strcmp(argv[i], "-a")) { verbose = 1; type = T_ANY; }
        else if (!strcmp(argv[i], "-4")) {}
        else {
            fprintf(stderr, nslookup ? "usage: nslookup [-type=TYPE] NAME [SERVER]\n" : "usage: host [-t TYPE] [-v] NAME [SERVER]\n");
            return 1;
        }
        if (type < 0) {
            fprintf(stderr, "%s: unknown type (A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT, ANY)\n", nslookup ? "nslookup" : "host");
            return 1;
        }
    }
    if (i >= argc || argc - i > 2) {
        fprintf(stderr, nslookup ? "usage: nslookup [-type=TYPE] NAME [SERVER]\n" : "usage: host [-t TYPE] [-v] NAME [SERVER]\n");
        return 1;
    }
    const char *name = argv[i];
    if (argc - i == 2) {
        struct in_addr a;
        if (!inet_aton(argv[i + 1], &a)) {
            fprintf(stderr, "%s: %s: give the server as an address\n", nslookup ? "nslookup" : "host", argv[i + 1]);
            return 1;
        }
        rx_res_server(argv[i + 1]);
        if (!nslookup) printf("Using domain server:\nName: %s\nAddress: %s#53\nAliases: \n\n", argv[i + 1], argv[i + 1]);
    }
    // An address: backwards
    unsigned char a4[4];
    char rev[64];
    if (inet_pton(AF_INET, name, a4) == 1) {
        snprintf(rev, sizeof rev, "%u.%u.%u.%u.in-addr.arpa", a4[3], a4[2], a4[1], a4[0]);
        name = rev;
        if (!type) type = T_PTR;
    }
    if (nslookup) {
        // The server first, as nslookup prints it (asking settles which)
        unsigned char ans[512];
        res_query(".", C_IN, T_NS, ans, sizeof ans);
        const char *srv = rx_res_last_server();
        printf("Server:\t\t%s\nAddress:\t%s#53\n\nNon-authoritative answer:\n", srv, srv);
    }
    int err;
    if (type) {
        err = ask(name, type, 1);
    } else {
        err = ask(name, T_A, 0);
        if (!err) {
            ask(name, T_AAAA, 0);
            if (!nslookup) ask(name, T_MX, 0);
        }
    }
    if (!err && !shown && !nslookup) printf("%s has no %s record\n", name, type ? type_name(type) : "A");
    return err;
}

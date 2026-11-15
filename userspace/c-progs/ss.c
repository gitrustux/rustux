// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ss: the sockets netd has (network step D2, docs/NETWORK.md, man ss), in
// iproute2's form.
//
//   ss [-t] [-u] [-l] [-a] [-n] [-H]
//
// Without -t or -u: both. Without -l or -a: connections only (TCP not
// listening, UDP connected); -l listening (and bound UDP) only; -a all.

#include "netclient.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    int tcp = 0, udp = 0, listening = 0, all = 0, header = 1, opt;
    while ((opt = getopt(argc, argv, "tulanHh4")) != -1) {
        switch (opt) {
        case 't': tcp = 1; break;
        case 'u': udp = 1; break;
        case 'l': listening = 1; break;
        case 'a': all = 1; break;
        case 'n': case '4': break;
        case 'H': header = 0; break;
        default:
            fprintf(stderr, "usage: ss [-t] [-u] [-l] [-a] [-n] [-H]\n");
            return 1;
        }
    }
    if (!tcp && !udp) tcp = udp = 1;
    char *text = net_request("ss", "sockets");
    if (header) printf("%-5s %-11s %6s %6s %-22s %s\n", "Netid", "State", "Recv-Q", "Send-Q", "Local Address:Port", "Peer Address:Port");
    char *lsave = 0;
    for (char *line = strtok_r(text, "\n", &lsave); line; line = strtok_r(0, "\n", &lsave)) {
        if (!strcmp(line, "ok")) break;
        // PROTO STATE RECVQ SENDQ LOCAL PEER
        char *w[6], *save = 0;
        int n = 0;
        for (char *t = strtok_r(line, " ", &save); t && n < 6; t = strtok_r(0, " ", &save)) w[n++] = t;
        if (n != 6) continue;
        const char *proto = w[0], *state = w[1], *local = w[4], *peer = w[5];
        unsigned long rq = strtoul(w[2], 0, 10), sq = strtoul(w[3], 0, 10);
        int is_tcp = !strcmp(proto, "tcp");
        if ((is_tcp && !tcp) || (!is_tcp && !udp)) continue;
        int is_listen = is_tcp ? !strcmp(state, "LISTEN") : !strcmp(peer, "0.0.0.0:*");
        if (!all && listening != is_listen) continue;
        printf("%-5s %-11s %6lu %6lu %-22s %s\n", proto, state, rq, sq, local, peer);
    }
    return 0;
}

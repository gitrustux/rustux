// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The DNS resolver (network step D3, docs/NETWORK.md): queries to the
// servers in /etc/resolv.conf over UDP, and TCP for answers too long for
// it. getaddrinfo and getnameinfo use it.

#ifndef _RESOLV_H
#define _RESOLV_H

#include <arpa/nameser.h>
#include <netinet/in.h>

// The answer to a query (the whole DNS message): its length, or -1 with
// h_errno set (HOST_NOT_FOUND, NO_DATA, TRY_AGAIN, NO_RECOVERY)
int res_query(const char *name, int cls, int type, unsigned char *answer, int anslen);
// The same, trying the search domains for names with fewer dots than ndots
int res_search(const char *name, int cls, int type, unsigned char *answer, int anslen);
// A name in a message at src, expanded into dst: the bytes it took, or -1
int dn_expand(const unsigned char *msg, const unsigned char *eom, const unsigned char *src, char *dst, int dstsiz);
int res_init(void);

// Rustux: ask this server (an IPv4 address) instead of /etc/resolv.conf's
// (0: those again); for host SERVER
void rx_res_server(const char *addr);
// Rustux: the server the last query went to ("10.0.2.3")
const char *rx_res_last_server(void);

#endif

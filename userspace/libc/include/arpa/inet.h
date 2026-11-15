// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _ARPA_INET_H
#define _ARPA_INET_H

#include <netinet/in.h>

int inet_pton(int af, const char *src, void *dst);
const char *inet_ntop(int af, const void *src, char *dst, socklen_t size);
int inet_aton(const char *s, struct in_addr *a);
in_addr_t inet_addr(const char *s);
char *inet_ntoa(struct in_addr a);

#endif

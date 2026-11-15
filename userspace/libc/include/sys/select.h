// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_SELECT_H
#define _SYS_SELECT_H

#include <sys/time.h>

#define FD_SETSIZE 1024
typedef struct { unsigned long fds_bits[FD_SETSIZE / 64]; } fd_set;
#define FD_ZERO(s) do { for (int __i = 0; __i < FD_SETSIZE / 64; __i++) (s)->fds_bits[__i] = 0; } while (0)
#define FD_SET(d, s) ((s)->fds_bits[(d) / 64] |= (1UL << ((d) % 64)))
#define FD_CLR(d, s) ((s)->fds_bits[(d) / 64] &= ~(1UL << ((d) % 64)))
#define FD_ISSET(d, s) (((s)->fds_bits[(d) / 64] & (1UL << ((d) % 64))) != 0)

int select(int n, fd_set *r, fd_set *w, fd_set *e, struct timeval *tv);

#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _POLL_H
#define _POLL_H

#define POLLIN   0x001
#define POLLPRI  0x002
#define POLLOUT  0x004
#define POLLERR  0x008
#define POLLHUP  0x010
#define POLLNVAL 0x020

struct pollfd { int fd; short events, revents; };
typedef unsigned long nfds_t;

int poll(struct pollfd *fds, nfds_t n, int timeout_ms);

#endif

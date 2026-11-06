// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Services (step 12): programs that serve a protocol of their own over a
// channel publish it under /svc; others connect by path. The filesystem
// service hands each connection to the server (RX_FS_OP_SERVE in
// <rustux/fs.h>, docs/DRIVERS.md).
//
//   server:  rx_channel_create(0, &mine, &theirs);
//            rx_service_publish("/svc/name", theirs);
//            then read rx_svc_connect_t messages from `mine`; each carries
//            one handle, a new client connection
//   client:  rx_service_connect("/svc/name", &ch);

#ifndef RUSTUX_SERVICE_H
#define RUSTUX_SERVICE_H

#include <rustux/fs.h>
#include <rustux/syscalls.h>

// 0 or a negative errno (ENOENT: no such service, ECONNREFUSED: its
// server has ended, ENOTSOCK: not a service)
int rx_service_connect(const char *path, rx_handle_t *out);

// Publish (or replace) a service; `server` is the channel end connection
// requests arrive on (consumed). 0 or a negative errno.
int rx_service_publish(const char *path, rx_handle_t server);

#endif

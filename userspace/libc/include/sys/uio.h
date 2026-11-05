// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_UIO_H
#define _SYS_UIO_H

#include <sys/types.h>

struct iovec { void *iov_base; size_t iov_len; };

ssize_t readv(int fd, const struct iovec *iov, int n);
ssize_t writev(int fd, const struct iovec *iov, int n);

#endif

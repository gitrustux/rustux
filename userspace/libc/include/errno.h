// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _ERRNO_H
#define _ERRNO_H

// Per thread
int *__errno_location(void);
#define errno (*__errno_location())

#define EPERM    1
#define ENOENT   2
#define ESRCH    3
#define EINTR    4
#define EIO      5
#define ENXIO    6
#define E2BIG    7
#define ENOEXEC  8
#define EBADF    9
#define ECHILD  10
#define EAGAIN  11
#define EWOULDBLOCK EAGAIN
#define ENOMEM  12
#define EACCES  13
#define EFAULT  14
#define EBUSY   16
#define EEXIST  17
#define EXDEV   18
#define ENODEV  19
#define ENOTDIR 20
#define EISDIR  21
#define EINVAL  22
#define ENFILE  23
#define EMFILE  24
#define ENOTTY  25
#define ETXTBSY 26
#define EFBIG   27
#define ENOSPC  28
#define ESPIPE  29
#define EROFS   30
#define EMLINK  31
#define EPIPE   32
#define EDOM    33
#define ERANGE  34
#define ENAMETOOLONG 36
#define ENOSYS  38
#define ENOTEMPTY 39
#define ENOTSUP 95
#define EOPNOTSUPP ENOTSUP
#define ETIMEDOUT 110
#define ENOTSOCK 88
#define EAFNOSUPPORT 97
#define EADDRINUSE 98
#define ECONNREFUSED 111
#define ENOLCK 37
#define ELOOP 40
#define EOVERFLOW 75
#define EBADMSG 74
#define ENOMEDIUM 123
#define EDEADLK 35
#define ENODATA 61
#define ENOTSUP 95
#define EOPNOTSUPP 95
#define ECANCELED 125

#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "libc.h"
#include <errno.h>
#include <string.h>

int rx_status_to_errno(rx_status_t s) {
    switch (s) {
    case RX_ERR_INVALID_ARGS: return EINVAL;
    case RX_ERR_NO_MEMORY: return ENOMEM;
    case RX_ERR_ACCESS_DENIED: return EACCES;
    case RX_ERR_NOT_FOUND: return ENOENT;
    case RX_ERR_NOT_SUPPORTED: return ENOTSUP;
    case RX_ERR_BAD_HANDLE: return EBADF;
    case RX_ERR_WRONG_TYPE: return EINVAL;
    case RX_ERR_TIMED_OUT: return ETIMEDOUT;
    case RX_ERR_SHOULD_WAIT: return EAGAIN;
    case RX_ERR_PEER_CLOSED: return EPIPE;
    case RX_ERR_BUFFER_TOO_SMALL: return ERANGE;
    case RX_ERR_OUT_OF_RANGE: return E2BIG;
    case RX_ERR_BAD_STATE: return EINVAL;
    case RX_ERR_ALREADY_EXISTS: return EEXIST;
    case RX_ERR_CANCELED: return ECANCELED;
    default: return EIO;
    }
}

int __set_errno_status(rx_status_t s) {
    errno = rx_status_to_errno(s);
    return -1;
}

char *strerror(int e) {
    switch (e) {
    case 0: return "Success";
    case EPERM: return "Operation not permitted";
    case ENOENT: return "No such file or directory";
    case ESRCH: return "No such process";
    case EINTR: return "Interrupted";
    case EIO: return "Input/output error";
    case E2BIG: return "Argument list too long";
    case ENOEXEC: return "Exec format error";
    case EBADF: return "Bad file descriptor";
    case ECHILD: return "No child processes";
    case EAGAIN: return "Resource temporarily unavailable";
    case ENOMEM: return "Out of memory";
    case EACCES: return "Permission denied";
    case EFAULT: return "Bad address";
    case EBUSY: return "Device or resource busy";
    case EEXIST: return "File exists";
    case ENOTDIR: return "Not a directory";
    case EISDIR: return "Is a directory";
    case EINVAL: return "Invalid argument";
    case EMFILE: return "Too many open files";
    case ENOTTY: return "Not a terminal";
    case ENOSPC: return "No space left on device";
    case ESPIPE: return "Illegal seek";
    case EROFS: return "Read-only file system";
    case EPIPE: return "Broken pipe";
    case EDOM: return "Numerical argument out of domain";
    case ERANGE: return "Result out of range";
    case ENAMETOOLONG: return "File name too long";
    case ENOSYS: return "Function not implemented";
    case ENOTEMPTY: return "Directory not empty";
    case ENOTSUP: return "Operation not supported";
    case ETIMEDOUT: return "Timed out";
    case ECANCELED: return "Canceled";
    case EDEADLK: return "Resource deadlock avoided";
    case ENOLCK: return "No locks available";
    case ELOOP: return "Too many levels of symbolic links";
    case EOVERFLOW: return "Value too large";
    case ENOTSOCK: return "Not a socket";
    case EAFNOSUPPORT: return "Address family not supported";
    case EADDRINUSE: return "Address in use";
    case ECONNREFUSED: return "Connection refused";
    case EXDEV: return "Cross-device link";
    case ENODEV: return "No such device";
    case ENXIO: return "No such device or address";
    case EFBIG: return "File too large";
    case EMLINK: return "Too many links";
    case ENFILE: return "Too many open files in system";
    case ETXTBSY: return "Text file busy";
    default: return "Unknown error";
    }
}

int strerror_r(int e, char *buf, size_t len) {
    const char *s = strerror(e);
    size_t n = strlen(s);
    if (!len) return ERANGE;
    if (n >= len) {
        memcpy(buf, s, len - 1);
        buf[len - 1] = 0;
        return ERANGE;
    }
    memcpy(buf, s, n + 1);
    return 0;
}

int __xpg_strerror_r(int e, char *buf, size_t len) { return strerror_r(e, buf, len); }

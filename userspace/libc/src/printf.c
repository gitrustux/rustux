// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// printf formatting: flags "-+ #0", width and precision (also "*"),
// lengths hh h l ll z j t, conversions d i u o x X c s p %. Floating point
// (%f %e %g) comes in step 10 and prints "?" for now.

#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

typedef void (*sink_fn)(void *ctx, const char *s, size_t n);

static void pad(sink_fn out, void *ctx, char c, int n) {
    char buf[32];
    memset(buf, c, sizeof buf);
    while (n > 0) {
        int k = n > 32 ? 32 : n;
        out(ctx, buf, (size_t)k);
        n -= k;
    }
}

int __format(sink_fn out, void *ctx, const char *fmt, va_list ap) {
    int total = 0;
#define EMIT(s, n) do { out(ctx, (s), (n)); total += (int)(n); } while (0)
#define PAD(c, n) do { if ((n) > 0) { pad(out, ctx, (c), (n)); total += (n); } } while (0)
    while (*fmt) {
        if (*fmt != '%') {
            const char *p = fmt;
            while (*p && *p != '%') p++;
            EMIT(fmt, (size_t)(p - fmt));
            fmt = p;
            continue;
        }
        fmt++;
        int left = 0, plus = 0, space = 0, alt = 0, zero = 0;
        for (;; fmt++) {
            if (*fmt == '-') left = 1;
            else if (*fmt == '+') plus = 1;
            else if (*fmt == ' ') space = 1;
            else if (*fmt == '#') alt = 1;
            else if (*fmt == '0') zero = 1;
            else break;
        }
        int width = 0, prec = -1;
        if (*fmt == '*') {
            width = va_arg(ap, int);
            if (width < 0) { left = 1; width = -width; }
            fmt++;
        } else {
            while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        }
        if (*fmt == '.') {
            fmt++;
            prec = 0;
            if (*fmt == '*') {
                prec = va_arg(ap, int);
                if (prec < 0) prec = -1;
                fmt++;
            } else {
                while (*fmt >= '0' && *fmt <= '9') prec = prec * 10 + (*fmt++ - '0');
            }
        }
        int len = 0; // 1 h, 2 hh, 3 l, 4 ll/j/z/t
        for (;; fmt++) {
            if (*fmt == 'h') len = len == 1 ? 2 : 1;
            else if (*fmt == 'l') len = len == 3 ? 4 : 3;
            else if (*fmt == 'z' || *fmt == 'j' || *fmt == 't') len = 4;
            else break;
        }
        char conv = *fmt;
        if (!conv) break;
        fmt++;

        char num[72];
        const char *body = num;
        size_t blen = 0;
        const char *prefix = "";
        int is_num = 0;

        switch (conv) {
        case '%':
            EMIT("%", 1);
            continue;
        case 'c': {
            char c = (char)va_arg(ap, int);
            if (!left) PAD(' ', width - 1);
            EMIT(&c, 1);
            if (left) PAD(' ', width - 1);
            continue;
        }
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            size_t n = prec >= 0 ? strnlen(s, (size_t)prec) : strlen(s);
            if (!left) PAD(' ', width - (int)n);
            EMIT(s, n);
            if (left) PAD(' ', width - (int)n);
            continue;
        }
        case 'd': case 'i': {
            long long v;
            if (len >= 3) v = len == 4 ? va_arg(ap, long long) : va_arg(ap, long);
            else v = va_arg(ap, int);
            if (len == 1) v = (short)v;
            if (len == 2) v = (signed char)v;
            unsigned long long u = v < 0 ? 0 - (unsigned long long)v : (unsigned long long)v;
            char *p = num + sizeof num;
            while (u) { *--p = (char)('0' + u % 10); u /= 10; }
            body = p;
            blen = (size_t)(num + sizeof num - p);
            prefix = v < 0 ? "-" : plus ? "+" : space ? " " : "";
            is_num = 1;
            break;
        }
        case 'u': case 'o': case 'x': case 'X': case 'p': {
            unsigned long long u;
            if (conv == 'p') { u = (uintptr_t)va_arg(ap, void *); alt = 1; }
            else if (len >= 3) u = len == 4 ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned long);
            else u = va_arg(ap, unsigned int);
            if (len == 1 && conv != 'p') u = (unsigned short)u;
            if (len == 2 && conv != 'p') u = (unsigned char)u;
            unsigned base = conv == 'o' ? 8 : conv == 'u' ? 10 : 16;
            const char *digits = conv == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
            int nonzero = u != 0;
            char *p = num + sizeof num;
            while (u) { *--p = digits[u % base]; u /= base; }
            body = p;
            blen = (size_t)(num + sizeof num - p);
            if (alt && base == 16 && (nonzero || conv == 'p')) prefix = conv == 'X' ? "0X" : "0x";
            if (alt && base == 8 && (prec <= (int)blen)) prec = (int)blen + 1;
            is_num = 1;
            break;
        }
        case 'f': case 'F': case 'e': case 'E': case 'g': case 'G': case 'a': case 'A':
            (void)va_arg(ap, double);
            num[0] = '?';
            blen = 1;
            break;
        default:
            // Unknown conversion: print it as written
            EMIT("%", 1);
            EMIT(&conv, 1);
            continue;
        }

        size_t plen = strlen(prefix);
        int zeros = 0;
        if (is_num) {
            if (prec >= 0) {
                if ((int)blen < prec) zeros = prec - (int)blen;
            } else if (zero && !left) {
                int w = width - (int)plen - (int)blen;
                if (w > 0) zeros = w;
            }
            if (blen == 0 && prec != 0 && zeros == 0) zeros = 1; // the value 0
        }
        int n = (int)(plen + blen) + zeros;
        if (!left) PAD(' ', width - n);
        EMIT(prefix, plen);
        PAD('0', zeros);
        EMIT(body, blen);
        if (left) PAD(' ', width - n);
    }
    return total;
#undef EMIT
#undef PAD
}

struct strbuf { char *buf; size_t cap, len; };

static void str_sink(void *ctx, const char *s, size_t n) {
    struct strbuf *b = ctx;
    if (b->len < b->cap) {
        size_t k = b->cap - b->len < n ? b->cap - b->len : n;
        memcpy(b->buf + b->len, s, k);
    }
    b->len += n;
}

int vsnprintf(char *restrict buf, size_t n, const char *restrict fmt, va_list ap) {
    struct strbuf b = { buf, n ? n - 1 : 0, 0 };
    int r = __format(str_sink, &b, fmt, ap);
    if (n) buf[b.len < b.cap ? b.len : b.cap] = 0;
    return r;
}

int vsprintf(char *restrict buf, const char *restrict fmt, va_list ap) {
    return vsnprintf(buf, (size_t)-1 >> 1, fmt, ap);
}

int snprintf(char *restrict buf, size_t n, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(buf, n, fmt, ap);
    va_end(ap);
    return r;
}

int sprintf(char *restrict buf, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsprintf(buf, fmt, ap);
    va_end(ap);
    return r;
}

// dprintf: format into a stack buffer, write in pieces
struct fdbuf { int fd; char buf[512]; size_t len; };

static void fd_flush(struct fdbuf *b) {
    if (b->len) write(b->fd, b->buf, b->len);
    b->len = 0;
}

static void fd_sink(void *ctx, const char *s, size_t n) {
    struct fdbuf *b = ctx;
    while (n) {
        if (b->len == sizeof b->buf) fd_flush(b);
        size_t k = sizeof b->buf - b->len < n ? sizeof b->buf - b->len : n;
        memcpy(b->buf + b->len, s, k);
        b->len += k;
        s += k;
        n -= k;
    }
}

int vdprintf(int fd, const char *restrict fmt, va_list ap) {
    struct fdbuf b;
    b.fd = fd;
    b.len = 0;
    int r = __format(fd_sink, &b, fmt, ap);
    fd_flush(&b);
    return r;
}

int dprintf(int fd, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vdprintf(fd, fmt, ap);
    va_end(ap);
    return r;
}

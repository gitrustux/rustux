// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// printf formatting: flags "-+ #0", width and precision (also "*"),
// lengths hh h l ll z j t L, conversions d i u o x X c s p % f F e E g G
// (a and A print as e). Floating point is converted with long double
// arithmetic: exact to about 18 significant digits, zeros after that.

#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

typedef void (*sink_fn)(void *ctx, const char *s, size_t n);

#define MAX_SIG 18

// Decimal digits of v > 0: `n` significant digits, rounded, in d[] (n may
// be 0: the result is then 0 or a carry to "1"). Returns the decimal
// exponent of the first digit (v ~ d0.d1d2... x 10^exp); *n may grow by
// one on a carry.
static int digits(long double v, int *n, char *d) {
    int e = 0;
    while (v >= 1e32L) { v /= 1e32L; e += 32; }
    while (v >= 10.0L) { v /= 10.0L; e++; }
    while (v < 1e-32L) { v *= 1e32L; e -= 32; }
    while (v < 1.0L) { v *= 10.0L; e--; }
    int want = *n;
    char round = '0';
    for (int i = 0; i <= want; i++) {
        int k = 0;
        if (i < MAX_SIG) {
            k = (int)v;
            if (k > 9) k = 9;
            v = (v - k) * 10.0L;
        }
        if (i < want) d[i] = (char)('0' + k);
        else round = (char)('0' + k);
    }
    if (round >= '5') {
        int i = want - 1;
        while (i >= 0 && d[i] == '9') d[i--] = '0';
        if (i >= 0) {
            d[i]++;
        } else { // all nines (or no digits): 1 followed by zeros, one place up
            memmove(d + 1, d, (size_t)(want > 0 ? want : 0));
            d[0] = '1';
            e++;
            if (want == 0) *n = 1;
        }
    }
    return e;
}

// Format |v| for conversion f, e or g (lower case) into out; returns the length
static int fmt_float(char *out, long double v, char conv, int prec, int alt) {
    char d[600];
    int n = 0, len = 0;
    if (prec < 0) prec = 6;
    if (prec > 300) prec = 300;
    if (v != v) return (int)(strcpy(out, "nan"), 3);
    if (v > 1.7e308L * 1e10L) return (int)(strcpy(out, "inf"), 3);
    int strip = 0;
    if (conv == 'g') {
        int p = prec ? prec : 1;
        int x = 0;
        if (v != 0) {
            n = p;
            x = digits(v, &n, d);
        }
        if (x < p && x >= -4) {
            conv = 'f';
            prec = p - 1 - x;
        } else {
            conv = 'e';
            prec = p - 1;
        }
        strip = !alt;
    }
    if (conv == 'e') {
        int e = 0;
        n = prec + 1;
        if (v == 0) memset(d, '0', (size_t)n);
        else e = digits(v, &n, d);
        out[len++] = d[0];
        if (prec || alt) out[len++] = '.';
        memcpy(out + len, d + 1, (size_t)prec);
        len += prec;
        if (strip) {
            while (out[len - 1] == '0') len--;
            if (out[len - 1] == '.') len--;
        }
        out[len++] = 'e';
        out[len++] = e < 0 ? '-' : '+';
        if (e < 0) e = -e;
        if (e >= 100) out[len++] = (char)('0' + e / 100);
        out[len++] = (char)('0' + e / 10 % 10);
        out[len++] = (char)('0' + e % 10);
        return len;
    }
    // f: digits down to 10^-prec
    int e = -1;
    if (v != 0) {
        int x = 0;
        // first find the exponent, then ask for exactly the digits needed
        long double t = v;
        while (t >= 10.0L) { t /= 10.0L; x++; }
        while (t < 1.0L) { t *= 10.0L; x--; }
        n = x + 1 + prec;
        if (n > 560) n = 560;
        if (n >= 0) e = digits(v, &n, d);
        else n = 0;
    }
    // integer part
    if (e < 0) {
        out[len++] = '0';
    } else {
        for (int i = 0; i <= e; i++) out[len++] = i < n ? d[i] : '0';
    }
    if (prec || alt) out[len++] = '.';
    for (int k = 1; k <= prec; k++) {
        int i = e + k; // index into d
        out[len++] = i >= 0 && i < n ? d[i] : '0';
    }
    if (strip && prec) {
        while (out[len - 1] == '0') len--;
        if (out[len - 1] == '.') len--;
    }
    return len;
}

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
        int len = 0, ldbl = 0; // 1 h, 2 hh, 3 l, 4 ll/j/z/t
        for (;; fmt++) {
            if (*fmt == 'L') ldbl = 1;
            else if (*fmt == 'h') len = len == 1 ? 2 : 1;
            else if (*fmt == 'l') len = len == 3 ? 4 : 3;
            else if (*fmt == 'z' || *fmt == 'j' || *fmt == 't') len = 4;
            else break;
        }
        char conv = *fmt;
        if (!conv) break;
        fmt++;

        char num[640];
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
        case 'f': case 'F': case 'e': case 'E': case 'g': case 'G': case 'a': case 'A': {
            long double v = ldbl ? va_arg(ap, long double) : va_arg(ap, double);
            int neg = v < 0 || (v == 0 && 1.0L / v < 0);
            if (neg) v = -v;
            char c = conv | 0x20;
            if (c == 'a') c = 'e';
            blen = (size_t)fmt_float(num, v, c, prec, alt);
            if (conv >= 'A' && conv <= 'Z')
                for (size_t i = 0; i < blen; i++) if (num[i] >= 'a' && num[i] <= 'z') num[i] -= 32;
            prefix = neg ? "-" : plus ? "+" : space ? " " : "";
            is_num = 1;
            prec = -1; // the precision is used up; '0' pads with zeros
            if (num[0] == 'n' || num[0] == 'i' || num[0] == 'N' || num[0] == 'I') zero = 0;
            break;
        }
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

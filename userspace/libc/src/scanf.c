// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The scanf family (2026-11-26, for the browser's libraries): sscanf,
// vsscanf, fscanf, vfscanf, scanf, vscanf. C99's conversions: d i u o x X
// c s [ ] p n a e f g (and E F G A) and %%, field widths, * (assign
// nothing), the length modifiers hh h l ll j z t L. Numbers are read
// into a buffer as far as their syntax goes and converted by strtoll,
// strtoull and strtod, so the results match those functions.

#include <ctype.h>
#include <limits.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    // a string, or a FILE
    const char *s;
    FILE *f;
    int back;     // one character pushed back (-2: none)
    long count;   // characters read so far (for %n)
} src_t;

static int next(src_t *in) {
    int c;
    if (in->back != -2) {
        c = in->back;
        in->back = -2;
    } else if (in->f) {
        c = fgetc(in->f);
    } else {
        c = *in->s ? (unsigned char)*in->s++ : EOF;
    }
    if (c != EOF) in->count++;
    return c;
}

static void unread(src_t *in, int c) {
    if (c == EOF) return;
    in->back = c;
    in->count--;
}

// At the end: give a pushed-back character back to the stream
static void finish(src_t *in) {
    if (in->f && in->back != -2 && in->back != EOF) ungetc(in->back, in->f);
}

enum { L_NONE, L_HH, L_H, L_L, L_LL, L_J, L_Z, L_T, L_BIGL };

static void store_int(va_list *ap, int len, unsigned long long v) {
    switch (len) {
    case L_HH: *va_arg(*ap, signed char *) = (signed char)v; break;
    case L_H: *va_arg(*ap, short *) = (short)v; break;
    case L_L: *va_arg(*ap, long *) = (long)v; break;
    case L_LL: *va_arg(*ap, long long *) = (long long)v; break;
    case L_J: *va_arg(*ap, intmax_t *) = (intmax_t)v; break;
    case L_Z: *va_arg(*ap, size_t *) = (size_t)v; break;
    case L_T: *va_arg(*ap, ptrdiff_t *) = (ptrdiff_t)v; break;
    default: *va_arg(*ap, int *) = (int)v; break;
    }
}

// The scanset of %[...]: fmt points after the '['; returns past the ']'
static const char *scanset(const char *fmt, unsigned char set[256]) {
    int neg = 0;
    if (*fmt == '^') {
        neg = 1;
        fmt++;
    }
    memset(set, 0, 256);
    if (*fmt == ']') {
        set[']'] = 1;
        fmt++;
    }
    for (; *fmt && *fmt != ']'; fmt++) {
        if (fmt[1] == '-' && fmt[2] && fmt[2] != ']') {
            for (int c = (unsigned char)fmt[0]; c <= (unsigned char)fmt[2]; c++) set[c] = 1;
            fmt += 2;
        } else {
            set[(unsigned char)*fmt] = 1;
        }
    }
    if (neg)
        for (int i = 0; i < 256; i++) set[i] = !set[i];
    return *fmt == ']' ? fmt + 1 : fmt;
}

static int scan(src_t *in, const char *fmt, va_list ap0) {
    va_list ap;
    va_copy(ap, ap0);
    int assigned = 0, any = 0, c = EOF;
    while (*fmt) {
        if (isspace((unsigned char)*fmt)) {
            while (isspace((unsigned char)*fmt)) fmt++;
            while (isspace(c = next(in))) {}
            unread(in, c);
            continue;
        }
        if (*fmt != '%' || fmt[1] == '%') {
            int want = (unsigned char)(*fmt == '%' ? fmt[1] : *fmt);
            fmt += *fmt == '%' ? 2 : 1;
            if (want == '%')
                while (isspace(c = next(in))) {}
            else
                c = next(in);
            if (c != want) {
                unread(in, c);
                if (c == EOF && !any) assigned = EOF;
                break;
            }
            continue;
        }
        fmt++;
        int skip = 0, width = 0, len = L_NONE;
        if (*fmt == '*') {
            skip = 1;
            fmt++;
        }
        while (isdigit((unsigned char)*fmt)) width = width * 10 + (*fmt++ - '0');
        switch (*fmt) {
        case 'h': len = fmt[1] == 'h' ? (fmt++, L_HH) : L_H; fmt++; break;
        case 'l': len = fmt[1] == 'l' ? (fmt++, L_LL) : L_L; fmt++; break;
        case 'j': len = L_J; fmt++; break;
        case 'z': len = L_Z; fmt++; break;
        case 't': len = L_T; fmt++; break;
        case 'L': len = L_BIGL; fmt++; break;
        case 'q': len = L_LL; fmt++; break;
        }
        int conv = (unsigned char)*fmt++;
        if (!conv) break;
        if (conv == 'n') {
            if (!skip) store_int(&ap, len, (unsigned long long)in->count);
            continue;
        }
        // Every conversion but [ c n skips white space first
        if (conv != '[' && conv != 'c') {
            while (isspace(c = next(in))) {}
            unread(in, c);
        }
        if (conv == 'c') {
            if (!width) width = 1;
            char *out = skip ? NULL : va_arg(ap, char *);
            int got = 0;
            while (got < width && (c = next(in)) != EOF) {
                if (out) out[got] = (char)c;
                got++;
            }
            if (got < width) {
                if (!got && !any) assigned = EOF;
                break;
            }
            any = 1;
            if (!skip) assigned++;
            continue;
        }
        if (conv == 's' || conv == '[') {
            unsigned char set[256];
            if (conv == '[') {
                fmt = scanset(fmt, set);
            } else {
                for (int i = 0; i < 256; i++) set[i] = !isspace(i);
                set[0] = 0;
            }
            char *out = skip ? NULL : va_arg(ap, char *);
            int got = 0;
            while ((!width || got < width) && (c = next(in)) != EOF && set[c]) {
                if (out) out[got] = (char)c;
                got++;
            }
            if (!(width && got == width)) unread(in, c);
            if (!got) {
                if (c == EOF && !any) assigned = EOF;
                break;
            }
            if (out) out[got] = 0;
            any = 1;
            if (!skip) assigned++;
            continue;
        }
        // Numbers: their characters into buf, then strto*
        char buf[512];
        int n = 0, lim = width && width < (int)sizeof buf - 1 ? width : (int)sizeof buf - 1;
        int is_float = strchr("aeEfFgGA", conv) != NULL;
        int base = conv == 'd' || conv == 'u' ? 10 : conv == 'o' ? 8 : conv == 'x' || conv == 'X' || conv == 'p' ? 16 : 0;
        c = next(in);
        if ((c == '+' || c == '-') && n < lim) {
            buf[n++] = (char)c;
            c = next(in);
        }
        if (is_float) {
            int digits = 0, hex = 0;
            if (c == '0' && n < lim) {
                buf[n++] = (char)c;
                digits = 1;
                c = next(in);
                if ((c == 'x' || c == 'X') && n < lim) {
                    hex = 1;
                    buf[n++] = (char)c;
                    c = next(in);
                }
            }
            if (!digits && (c == 'i' || c == 'I' || c == 'n' || c == 'N')) {
                // inf, infinity, nan
                const char *word = (c == 'i' || c == 'I') ? "infinity" : "nan";
                int k = 0;
                while (word[k] && n < lim && tolower(c) == word[k]) {
                    buf[n++] = (char)c;
                    k++;
                    c = next(in);
                }
                digits = k >= 3;
            } else {
                while (n < lim && (hex ? isxdigit(c) : isdigit(c))) {
                    buf[n++] = (char)c;
                    digits = 1;
                    c = next(in);
                }
                if (c == '.' && n < lim) {
                    buf[n++] = (char)c;
                    c = next(in);
                    while (n < lim && (hex ? isxdigit(c) : isdigit(c))) {
                        buf[n++] = (char)c;
                        digits = 1;
                        c = next(in);
                    }
                }
                if (digits && n < lim && (hex ? (c == 'p' || c == 'P') : (c == 'e' || c == 'E'))) {
                    buf[n++] = (char)c;
                    c = next(in);
                    if ((c == '+' || c == '-') && n < lim) {
                        buf[n++] = (char)c;
                        c = next(in);
                    }
                    while (n < lim && isdigit(c)) {
                        buf[n++] = (char)c;
                        c = next(in);
                    }
                }
            }
            unread(in, c);
            if (!digits) {
                if (c == EOF && !any) assigned = EOF;
                break;
            }
            buf[n] = 0;
            long double v = strtod(buf, NULL);
            if (!skip) {
                if (len == L_BIGL) *va_arg(ap, long double *) = v;
                else if (len == L_L) *va_arg(ap, double *) = (double)v;
                else *va_arg(ap, float *) = (float)v;
                assigned++;
            }
            any = 1;
            continue;
        }
        // Integers: the base's prefix, then its digits
        int digits = 0;
        if (c == '0' && n < lim) {
            buf[n++] = (char)c;
            digits = 1;
            c = next(in);
            if ((base == 0 || base == 16) && (c == 'x' || c == 'X') && n < lim) {
                buf[n++] = (char)c;
                base = 16;
                digits = 0;
                c = next(in);
            } else if (base == 0) {
                base = 8;
            }
        }
        if (base == 0) base = 10;
        for (; n < lim; c = next(in)) {
            int d = isdigit(c) ? c - '0' : isalpha(c) ? tolower(c) - 'a' + 10 : 99;
            if (d >= base) break;
            buf[n++] = (char)c;
            digits = 1;
        }
        unread(in, c);
        if (!digits) {
            if (c == EOF && !any) assigned = EOF;
            break;
        }
        buf[n] = 0;
        any = 1;
        if (skip) continue;
        if (conv == 'p') {
            *va_arg(ap, void **) = (void *)(uintptr_t)strtoull(buf, NULL, 16);
        } else if (conv == 'd' || conv == 'i') {
            store_int(&ap, len, (unsigned long long)strtoll(buf, NULL, base));
        } else {
            store_int(&ap, len, strtoull(buf, NULL, base));
        }
        assigned++;
    }
    va_end(ap);
    finish(in);
    return assigned;
}

int vsscanf(const char *restrict s, const char *restrict fmt, va_list ap) {
    src_t in = { s, NULL, -2, 0 };
    return scan(&in, fmt, ap);
}

int sscanf(const char *restrict s, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsscanf(s, fmt, ap);
    va_end(ap);
    return r;
}

int vfscanf(FILE *restrict f, const char *restrict fmt, va_list ap) {
    src_t in = { NULL, f, -2, 0 };
    return scan(&in, fmt, ap);
}

int fscanf(FILE *restrict f, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfscanf(f, fmt, ap);
    va_end(ap);
    return r;
}

int vscanf(const char *restrict fmt, va_list ap) { return vfscanf(stdin, fmt, ap); }

int scanf(const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfscanf(stdin, fmt, ap);
    va_end(ap);
    return r;
}

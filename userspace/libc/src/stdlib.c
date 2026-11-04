// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "libc.h"
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define MAX_ATEXIT 32
static void (*atexit_fns[MAX_ATEXIT])(void);
static int atexit_count;

int atexit(void (*fn)(void)) {
    if (atexit_count >= MAX_ATEXIT) return -1;
    atexit_fns[atexit_count++] = fn;
    return 0;
}

void exit(int status) {
    while (atexit_count > 0) atexit_fns[--atexit_count]();
    __stdio_flush_all();
    rx_exit(status);
}

void abort(void) {
    __stdio_flush_all();
    rx_exit(-6); // SIGABRT's number, negated
}

void __assert_fail(const char *expr, const char *file, int line, const char *func) {
    fprintf(stderr, "Assertion failed: %s (%s: %s: %d)\n", expr, file, func, line);
    abort();
}

// Parse an unsigned number; sets *neg for a leading '-'. Returns the value
// saturated to `max` (with *overflow set) when it does not fit.
static unsigned long long parse(const char *s, char **end, int base, int *neg, unsigned long long max, int *overflow) {
    const char *p = s;
    unsigned long long v = 0;
    int any = 0;
    *neg = 0;
    *overflow = 0;
    while (isspace((unsigned char)*p)) p++;
    if (*p == '+' || *p == '-') *neg = *p++ == '-';
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && isxdigit((unsigned char)p[2])) {
        p += 2;
        base = 16;
    } else if (base == 0) {
        base = *p == '0' ? 8 : 10;
    }
    if (base < 2 || base > 36) {
        errno = EINVAL;
        if (end) *end = (char *)s;
        return 0;
    }
    for (;; p++) {
        int d;
        if (isdigit((unsigned char)*p)) d = *p - '0';
        else if (isalpha((unsigned char)*p)) d = tolower((unsigned char)*p) - 'a' + 10;
        else break;
        if (d >= base) break;
        any = 1;
        if (v > (max - (unsigned)d) / (unsigned)base) *overflow = 1;
        else v = v * (unsigned)base + (unsigned)d;
    }
    if (end) *end = (char *)(any ? p : s);
    return *overflow ? max : v;
}

long long strtoll(const char *restrict s, char **restrict end, int base) {
    int neg, of;
    unsigned long long v = parse(s, end, base, &neg, (unsigned long long)LLONG_MAX + 1, &of);
    if (of || (!neg && v > (unsigned long long)LLONG_MAX)) {
        errno = ERANGE;
        return neg ? LLONG_MIN : LLONG_MAX;
    }
    return neg ? (long long)(0 - v) : (long long)v;
}

long strtol(const char *restrict s, char **restrict end, int base) {
    return (long)strtoll(s, end, base);
}

unsigned long long strtoull(const char *restrict s, char **restrict end, int base) {
    int neg, of;
    unsigned long long v = parse(s, end, base, &neg, ULLONG_MAX, &of);
    if (of) {
        errno = ERANGE;
        return ULLONG_MAX;
    }
    return neg ? 0 - v : v;
}

unsigned long strtoul(const char *restrict s, char **restrict end, int base) {
    return (unsigned long)strtoull(s, end, base);
}

int atoi(const char *s) { return (int)strtol(s, 0, 10); }
long atol(const char *s) { return strtol(s, 0, 10); }
long long atoll(const char *s) { return strtoll(s, 0, 10); }
int abs(int v) { return v < 0 ? -v : v; }
long labs(long v) { return v < 0 ? -v : v; }
long long llabs(long long v) { return v < 0 ? -v : v; }

static void swap(char *a, char *b, size_t n) {
    while (n--) { char t = *a; *a++ = *b; *b++ = t; }
}

// Heapsort: O(n log n) worst case, no extra memory
static void sift(char *base, size_t start, size_t end, size_t size, int (*cmp)(const void *, const void *)) {
    size_t root = start;
    while (2 * root + 1 < end) {
        size_t child = 2 * root + 1;
        if (child + 1 < end && cmp(base + child * size, base + (child + 1) * size) < 0) child++;
        if (cmp(base + root * size, base + child * size) >= 0) return;
        swap(base + root * size, base + child * size, size);
        root = child;
    }
}

void qsort(void *b, size_t n, size_t size, int (*cmp)(const void *, const void *)) {
    char *base = b;
    if (n < 2) return;
    for (size_t i = n / 2; i-- > 0;) sift(base, i, n, size, cmp);
    for (size_t end = n - 1; end > 0; end--) {
        swap(base, base + end * size, size);
        sift(base, 0, end, size, cmp);
    }
}

void *bsearch(const void *key, const void *base, size_t n, size_t size, int (*cmp)(const void *, const void *)) {
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        const char *p = (const char *)base + mid * size;
        int c = cmp(key, p);
        if (c == 0) return (void *)p;
        if (c < 0) hi = mid; else lo = mid + 1;
    }
    return 0;
}

static unsigned long long rand_state = 1;
void srand(unsigned int seed) { rand_state = seed; }
int rand(void) {
    rand_state = rand_state * 6364136223846793005ull + 1442695040888963407ull;
    return (int)((rand_state >> 33) & RAND_MAX);
}

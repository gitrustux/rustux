// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Mathematics on the x87 floating-point unit. See <math.h>.

#include <math.h>
#include <stdint.h>
#include <string.h>

double fabs(double x) { return __builtin_fabs(x); }
float fabsf(float x) { return __builtin_fabsf(x); }

double trunc(double x) {
    if (!(fabs(x) < 4503599627370496.0)) return x; // 2^52: already whole (or NaN/inf)
    return (double)(int64_t)x;
}
double floor(double x) {
    double t = trunc(x);
    return t > x ? t - 1.0 : t;
}
double ceil(double x) {
    double t = trunc(x);
    return t < x ? t + 1.0 : t;
}
double round(double x) { return x < 0 ? -floor(-x + 0.5) : floor(x + 0.5); }
float floorf(float x) { return (float)floor(x); }
float ceilf(float x) { return (float)ceil(x); }

double sqrt(double x) {
    __asm__("sqrtsd %1, %0" : "=x"(x) : "x"(x));
    return x;
}
float sqrtf(float x) { return (float)sqrt(x); }

// y * log2(x)
static long double yl2x(long double x, long double y) {
    long double r;
    __asm__("fyl2x" : "=t"(r) : "0"(x), "u"(y) : "st(1)");
    return r;
}

double log2(double x) { return (double)yl2x(x, 1.0L); }
double log(double x) { return (double)yl2x(x, 0.693147180559945309417232121458L); }
double log10(double x) { return (double)yl2x(x, 0.301029995663981195213738894724L); }
float logf(float x) { return (float)log(x); }
float log10f(float x) { return (float)log10(x); }
float log2f(float x) { return (float)log2(x); }

static long double exp2l_(long double x) {
    if (x != x) return x;
    if (x > 16500) return __builtin_infl();
    if (x < -16500) return 0;
    long double i = (long double)(int64_t)x, f = x - i, r;
    if (f > 0.5L) { f -= 1; i += 1; }
    else if (f < -0.5L) { f += 1; i -= 1; }
    __asm__("f2xm1" : "=t"(r) : "0"(f));
    r += 1;
    __asm__("fscale" : "=t"(r) : "0"(r), "u"(i));
    return r;
}

double exp2(double x) { return (double)exp2l_(x); }
double exp(double x) { return (double)exp2l_(x * 1.442695040888963407359924681001892137L); }
float expf(float x) { return (float)exp(x); }

static int is_int(double y) { return trunc(y) == y; }

double pow(double x, double y) {
    if (y == 0) return 1;
    if (x == 1) return 1;
    if (x != x || y != y) return x + y;
    if (x == 0) {
        if (y < 0) return 1.0 / (signbit(x) && is_int(y) && fmod(y, 2) != 0 ? -0.0 : 0.0);
        return signbit(x) && is_int(y) && fmod(y, 2) != 0 ? -0.0 : 0.0;
    }
    if (x < 0) {
        if (!is_int(y)) return __builtin_nan("");
        double r = (double)exp2l_((long double)y * yl2x(-x, 1.0L));
        return fmod(y, 2) != 0 ? -r : r;
    }
    return (double)exp2l_((long double)y * yl2x(x, 1.0L));
}
float powf(float x, float y) { return (float)pow(x, y); }

double fmod(double x, double y) {
    long double a = x, b = y, r;
    uint16_t sw;
    do {
        __asm__("fprem; fnstsw %%ax" : "=t"(r), "=a"(sw) : "0"(a), "u"(b));
        a = r;
    } while (sw & 0x400);
    return (double)r;
}

double sin(double x) { long double r; __asm__("fsin" : "=t"(r) : "0"((long double)x)); return (double)r; }
double cos(double x) { long double r; __asm__("fcos" : "=t"(r) : "0"((long double)x)); return (double)r; }
double tan(double x) { return sin(x) / cos(x); }
double atan2(double y, double x) {
    long double r;
    __asm__("fpatan" : "=t"(r) : "0"((long double)x), "u"((long double)y) : "st(1)");
    return (double)r;
}
double atan(double x) { return atan2(x, 1.0); }

double frexp(double x, int *e) {
    *e = 0;
    if (x == 0 || x != x || isinf(x)) return x;
    uint64_t b;
    memcpy(&b, &x, 8);
    int ex = (int)(b >> 52 & 0x7FF);
    if (ex == 0) { // subnormal
        x *= 18014398509481984.0; // 2^54
        memcpy(&b, &x, 8);
        ex = (int)(b >> 52 & 0x7FF) - 54;
    }
    *e = ex - 1022;
    b = (b & ~(0x7FFull << 52)) | (1022ull << 52);
    memcpy(&x, &b, 8);
    return x;
}

double ldexp(double x, int e) { return (double)((long double)x * exp2l_(e)); }

double modf(double x, double *ip) {
    *ip = trunc(x);
    return x - *ip;
}

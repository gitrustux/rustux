// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Mathematics: on the x87 floating-point unit on x86-64, in plain C
// elsewhere. See <math.h>.

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
#if defined(__x86_64__) && !defined(RX_PORTABLE_MATH)
    __asm__("sqrtsd %1, %0" : "=x"(x) : "x"(x));
    return x;
#else
    return __builtin_sqrt(x);   // one instruction on aarch64 and riscv64 (-fno-math-errno)
#endif
}

#if defined(__x86_64__) && !defined(RX_PORTABLE_MATH)

// The x87 unit's instructions, in 80-bit precision


// y * log2(x)
static long double yl2x(long double x, long double y) {
    long double r;
    __asm__("fyl2x" : "=t"(r) : "0"(x), "u"(y) : "st(1)");
    return r;
}

double log2(double x) { return (double)yl2x(x, 1.0L); }
double log(double x) { return (double)yl2x(x, 0.693147180559945309417232121458L); }
double log10(double x) { return (double)yl2x(x, 0.301029995663981195213738894724L); }

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

double atan2(double y, double x) {
    long double r;
    __asm__("fpatan" : "=t"(r) : "0"((long double)x), "u"((long double)y) : "st(1)");
    return (double)r;
}

double ldexp(double x, int e) { return (double)((long double)x * exp2l_(e)); }

#else

// The same functions in plain C, for every other architecture (and for
// testing on x86: -DRX_PORTABLE_MATH). Accurate to a few units in the last
// place of a double, like the x87 versions; not correctly rounded.

static const double ln2_hi = 6.93147180369123816490e-01; // ln 2, high bits
static const double ln2_lo = 1.90821492927058770002e-10; // and the rest

// x * 2^e, without long double (which is software on aarch64 and riscv64)
double ldexp(double x, int e) {
    if (x == 0 || x != x || isinf(x)) return x;
    while (e > 1000) { x *= 0x1p1000; e -= 1000; if (isinf(x)) return x; }
    while (e < -1000) { x *= 0x1p-1000; e += 1000; if (x == 0) return x; }
    if (e < -1022) { x *= 0x1p-1022; e += 1022; }
    uint64_t b = (uint64_t)(e + 1023) << 52; // 2^e, e in [-1022, 1000]
    double s;
    memcpy(&s, &b, 8);
    return x * s;
}

double log(double x) {
    if (x != x || x == __builtin_inf()) return x;
    if (x == 0) return -__builtin_inf();
    if (x < 0) return __builtin_nan("");
    int e;
    double m = frexp(x, &e); // [0.5, 1)
    if (m < 0.70710678118654752440) { m *= 2; e--; }
    // log m = 2 atanh s, s = (m-1)/(m+1), |s| < 0.172
    double s = (m - 1) / (m + 1), s2 = s * s, sum = 0;
    for (int k = 25; k >= 3; k -= 2) sum = sum * s2 + 1.0 / k;
    double lm = 2 * s + 2 * s * s2 * sum;
    return e * ln2_hi + (lm + e * ln2_lo);
}
double log2(double x) { return log(x) * 1.44269504088896340736; }
double log10(double x) { return log(x) * 0.43429448190325182765; }

// e^r for |r| <= 0.35
static double exp_small(double r) {
    double p = 1;
    for (int k = 20; k >= 1; k--) p = 1 + p * r / k;
    return p;
}
double exp(double x) {
    if (x != x) return x;
    if (x > 709.8) return __builtin_inf();
    if (x < -745.2) return 0;
    double k = round(x * 1.44269504088896340736);
    double r = (x - k * ln2_hi) - k * ln2_lo;
    return ldexp(exp_small(r), (int)k);
}
double exp2(double x) {
    if (x != x) return x;
    if (x > 1024) return __builtin_inf();
    if (x < -1075) return 0;
    double k = round(x);
    return ldexp(exp_small((x - k) * 0.69314718055994530942), (int)k);
}

static int is_int(double y) { return trunc(y) == y; }

double fmod(double x, double y) {
    if (x != x || y != y || isinf(x) || y == 0) return __builtin_nan("");
    if (isinf(y)) return x;
    double ax = fabs(x), ay = fabs(y);
    int ey;
    frexp(ay, &ey);
    while (ax >= ay) {
        int ex;
        frexp(ax, &ex);
        double t = ldexp(ay, ex - ey); // ay * 2^n, the largest not above ax
        if (t > ax) t *= 0.5;
        ax -= t; // exact
    }
    return signbit(x) ? -ax : ax;
}

// log x as hi + lo, about 2^-56 relative, for pow (x finite, > 0)
static double log_hilo(double x, double *lo) {
    int e;
    double m = frexp(x, &e);
    if (m < 0.70710678118654752440) { m *= 2; e--; }
    double f = m - 1, s = f / (2 + f), z = s * s, sum = 0; // f exact
    for (int k = 25; k >= 3; k -= 2) sum = sum * z + 1.0 / k;
    double R = 2 * z * sum;
    double hf = 0.5 * f * f, hf_lo = __builtin_fma(0.5 * f, f, -hf);
    double small = s * (hf + R) - hf_lo;
    // e ln2_hi + f - hf + small, summed without losing the low bits
    double a = e * ln2_hi, h = a + f, l = f - (h - a);
    double h2 = h - hf, l2 = (h - h2) - hf;
    l = l + l2 + small + e * ln2_lo;
    double r = h2 + l;
    *lo = l - (r - h2);
    return r;
}

// e^(h + l), |l| tiny
static double exp_hilo(double h, double l) {
    if (h != h) return h;
    if (h > 709.8) return __builtin_inf();
    if (h < -745.2) return 0;
    double k = round(h * 1.44269504088896340736);
    double r = (h - k * ln2_hi) - k * ln2_lo + l;
    return ldexp(exp_small(r), (int)k);
}

static double pow_pos(double x, double y) {
    double lo, hi = log_hilo(x, &lo);
    double ph = y * hi, pl = __builtin_fma(y, hi, -ph) + y * lo;
    return exp_hilo(ph, pl);
}

double pow(double x, double y) {
    if (y == 0) return 1;
    if (x == 1) return 1;
    if (x != x || y != y) return x + y;
    if (isinf(y)) {
        double ax = fabs(x);
        if (ax == 1) return 1;
        return (ax > 1) == (y > 0) ? __builtin_inf() : 0.0;
    }
    if (x == 0) {
        if (y < 0) return 1.0 / (signbit(x) && is_int(y) && fmod(y, 2) != 0 ? -0.0 : 0.0);
        return signbit(x) && is_int(y) && fmod(y, 2) != 0 ? -0.0 : 0.0;
    }
    if (isinf(x)) {
        int odd = is_int(y) && fmod(y, 2) != 0;
        double r = y > 0 ? __builtin_inf() : 0.0;
        return x < 0 && odd ? -r : r;
    }
    if (x < 0) {
        if (!is_int(y)) return __builtin_nan("");
        double r = pow_pos(-x, y);
        return fmod(y, 2) != 0 ? -r : r;
    }
    return pow_pos(x, y);
}

// x - n*pi/2 for the nearest n (three parts of pi/2, good to |x| ~ 2^30)
static double reduce(double x, int *n) {
    if (fabs(x) > 0x1p30) x = fmod(x, 6.28318530717958647693); // not exact: 2 pi is rounded
    double k = round(x * 0.63661977236758134308);
    *n = (int)((int64_t)k & 3);
    return ((x - k * 1.57079632673412561417e+00) - k * 6.07710050630396597660e-11)
           - k * 2.02226624871116645580e-21;
}
static double sin_small(double r) { // |r| <= pi/4
    double r2 = r * r, p = 0;
    for (int k = 19; k >= 3; k -= 2) p = (1 - p) * r2 / ((double)k * (k - 1));
    return r * (1 - p);
}
static double cos_small(double r) {
    double r2 = r * r, p = 0;
    for (int k = 20; k >= 2; k -= 2) p = (1 - p) * r2 / ((double)k * (k - 1));
    return 1 - p;
}
double sin(double x) {
    if (x != x || isinf(x)) return __builtin_nan("");
    int n;
    double r = reduce(x, &n);
    switch (n) {
    case 0: return sin_small(r);
    case 1: return cos_small(r);
    case 2: return -sin_small(r);
    default: return -cos_small(r);
    }
}
double cos(double x) {
    if (x != x || isinf(x)) return __builtin_nan("");
    int n;
    double r = reduce(x, &n);
    switch (n) {
    case 0: return cos_small(r);
    case 1: return -sin_small(r);
    case 2: return -cos_small(r);
    default: return sin_small(r);
    }
}

// atan x for x >= 0
static double atan_pos(double x) {
    const double pi2 = 1.57079632679489661923, pi6 = 0.52359877559829887308;
    int inv = x > 1;
    if (inv) x = 1 / x;
    int shift = x > 0.26794919243112270647; // tan(pi/12)
    if (shift) x = (x * 1.73205080756887729353 - 1) / (x + 1.73205080756887729353);
    double x2 = x * x, s = 0;
    for (int k = 31; k >= 3; k -= 2) s = 1.0 / k - x2 * s;
    double r = x - x * x2 * s;
    if (shift) r += pi6;
    return inv ? pi2 - r : r;
}
double atan2(double y, double x) {
    const double pi = 3.14159265358979323846, pi2 = 1.57079632679489661923;
    if (x != x || y != y) return x + y;
    if (y == 0) {
        if (signbit(x)) return signbit(y) ? -pi : pi;
        return y; // +-0
    }
    if (x == 0) return signbit(y) ? -pi2 : pi2;
    if (isinf(x) && isinf(y)) {
        double a = signbit(x) ? 3 * pi / 4 : pi / 4;
        return signbit(y) ? -a : a;
    }
    if (isinf(x)) return signbit(x) ? (signbit(y) ? -pi : pi) : (signbit(y) ? -0.0 : 0.0);
    if (isinf(y)) return signbit(y) ? -pi2 : pi2;
    double a = atan_pos(fabs(y / x));
    if (signbit(x)) a = pi - a;
    return signbit(y) ? -a : a;
}

#endif

float sqrtf(float x) { return (float)sqrt(x); }
float logf(float x) { return (float)log(x); }
float log10f(float x) { return (float)log10(x); }
float log2f(float x) { return (float)log2(x); }
float expf(float x) { return (float)exp(x); }
float powf(float x, float y) { return (float)pow(x, y); }
double tan(double x) { return sin(x) / cos(x); }
double atan(double x) { return atan2(x, 1.0); }
// asin and acos from atan2 (tiny-skia's curve maths, step F5)
double asin(double x) { return atan2(x, sqrt((1.0 - x) * (1.0 + x))); }
double acos(double x) { return atan2(sqrt((1.0 - x) * (1.0 + x)), x); }
float sinf(float x) { return (float)sin(x); }
float cosf(float x) { return (float)cos(x); }
float tanf(float x) { return (float)tan(x); }
float atan2f(float y, float x) { return (float)atan2(y, x); }
float atanf(float x) { return (float)atan(x); }
float asinf(float x) { return (float)asin(x); }
float acosf(float x) { return (float)acos(x); }

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


double modf(double x, double *ip) {
    *ip = trunc(x);
    return x - *ip;
}

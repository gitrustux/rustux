// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Mathematics, on the x87 unit's instructions (accurate to about the last
// bit of a double; not the correctly rounded results of a full libm)

#ifndef _MATH_H
#define _MATH_H

#define HUGE_VAL __builtin_huge_val()
#define INFINITY __builtin_inff()
#define NAN __builtin_nanf("")
#define M_PI 3.14159265358979323846
#define M_E 2.7182818284590452354
#define M_LN2 0.69314718055994530942
#define M_LN10 2.30258509299404568402
#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x) __builtin_signbit(x)

double fabs(double x);
double floor(double x);
double ceil(double x);
double trunc(double x);
double round(double x);
double sqrt(double x);
double fmod(double x, double y);
double exp(double x);
double exp2(double x);
double log(double x);
double log2(double x);
double log10(double x);
double pow(double x, double y);
double sin(double x);
double cos(double x);
double tan(double x);
double atan(double x);
double atan2(double y, double x);
double frexp(double x, int *e);
double ldexp(double x, int e);
double modf(double x, double *ip);
float fabsf(float x);
float sqrtf(float x);
float logf(float x);
float log10f(float x);
float log2f(float x);
float expf(float x);
float powf(float x, float y);
float floorf(float x);
float ceilf(float x);

#endif

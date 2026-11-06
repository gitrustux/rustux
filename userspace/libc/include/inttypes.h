// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _INTTYPES_H
#define _INTTYPES_H

#include <stdint.h>

#define PRId8 "d"
#define PRId16 "d"
#define PRId32 "d"
#define PRId64 "ld"
#define PRIi64 "li"
#define PRIu8 "u"
#define PRIu16 "u"
#define PRIu32 "u"
#define PRIu64 "lu"
#define PRIx32 "x"
#define PRIx64 "lx"
#define PRIX64 "lX"
#define PRIo64 "lo"
#define PRIdPTR "ld"
#define PRIuPTR "lu"
#define PRIxPTR "lx"
#define PRIdMAX "ld"
#define PRIuMAX "lu"
#define SCNd64 "ld"
#define SCNu64 "lu"

typedef struct { intmax_t quot, rem; } imaxdiv_t;
intmax_t strtoimax(const char *restrict s, char **restrict end, int base);
uintmax_t strtoumax(const char *restrict s, char **restrict end, int base);

#endif

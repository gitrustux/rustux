// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Character set conversion (POSIX iconv): src/iconv.c, musl's
#ifndef _ICONV_H
#define _ICONV_H

#include <stddef.h>

typedef void *iconv_t;

iconv_t iconv_open(const char *to, const char *from);
size_t iconv(iconv_t cd, char **restrict in, size_t *restrict inb, char **restrict out, size_t *restrict outb);
int iconv_close(iconv_t cd);

#endif

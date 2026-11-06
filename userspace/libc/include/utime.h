// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _UTIME_H
#define _UTIME_H

#include <sys/types.h>

struct utimbuf { time_t actime, modtime; };
int utime(const char *path, const struct utimbuf *t);

#endif

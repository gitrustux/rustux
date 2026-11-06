// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_TIME_H
#define _SYS_TIME_H

#include <sys/types.h>

struct timeval { time_t tv_sec; long tv_usec; };
struct timezone { int tz_minuteswest; int tz_dsttime; };

int gettimeofday(struct timeval *restrict tv, void *restrict tz);
int utimes(const char *path, const struct timeval t[2]);
int lutimes(const char *path, const struct timeval t[2]);

#endif

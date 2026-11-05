// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_RANDOM_H
#define _SYS_RANDOM_H

#include <sys/types.h>

#define GRND_NONBLOCK 1
#define GRND_RANDOM 2

ssize_t getrandom(void *buf, size_t len, unsigned flags);
int getentropy(void *buf, size_t len);

#endif

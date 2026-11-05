// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _GETOPT_H
#define _GETOPT_H

#include <unistd.h>

struct option { const char *name; int has_arg; int *flag; int val; };
#define no_argument 0
#define required_argument 1
#define optional_argument 2

int getopt_long(int argc, char *const argv[], const char *optstring, const struct option *longopts, int *index);

#endif

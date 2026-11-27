// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// basename and dirname, POSIX's (they may change the string given)
#ifndef _LIBGEN_H
#define _LIBGEN_H

char *basename(char *path);
char *dirname(char *path);

#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// No dynamic linking: dlsym answers for a few functions the library has
// (Rust's std looks some up by name), NULL for anything else.

#ifndef _DLFCN_H
#define _DLFCN_H

#define RTLD_DEFAULT ((void *)0)
#define RTLD_NEXT ((void *)-1)
#define RTLD_LAZY 1
#define RTLD_NOW 2

void *dlsym(void *restrict handle, const char *restrict name);
void *dlopen(const char *path, int flags);
int dlclose(void *handle);
char *dlerror(void);

#endif

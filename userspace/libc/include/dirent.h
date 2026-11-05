// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _DIRENT_H
#define _DIRENT_H

#include <sys/types.h>

#define DT_UNKNOWN 0
#define DT_DIR     4
#define DT_REG     8

struct dirent {
    ino_t d_ino;
    unsigned char d_type;
    char d_name[256];
};

typedef struct _DIR DIR;

DIR *opendir(const char *path);
struct dirent *readdir(DIR *d);
void rewinddir(DIR *d);
int closedir(DIR *d);
int dirfd(DIR *d);

#endif

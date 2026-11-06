// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Users from /etc/passwd (one user, root, until step 17)

#ifndef _PWD_H
#define _PWD_H

#include <sys/types.h>

struct passwd {
    char *pw_name, *pw_passwd;
    uid_t pw_uid;
    gid_t pw_gid;
    char *pw_gecos, *pw_dir, *pw_shell;
};

struct passwd *getpwuid(uid_t uid);
struct passwd *getpwnam(const char *name);
int getpwuid_r(uid_t uid, struct passwd *pw, char *buf, size_t len, struct passwd **result);
int getpwnam_r(const char *name, struct passwd *pw, char *buf, size_t len, struct passwd **result);
void setpwent(void);
void endpwent(void);
struct passwd *getpwent(void);

#endif

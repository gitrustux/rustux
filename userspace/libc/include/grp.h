// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Groups from /etc/group

#ifndef _GRP_H
#define _GRP_H

#include <sys/types.h>

struct group {
    char *gr_name, *gr_passwd;
    gid_t gr_gid;
    char **gr_mem;
};

struct group *getgrgid(gid_t gid);
struct group *getgrnam(const char *name);
int getgrgid_r(gid_t gid, struct group *gr, char *buf, size_t len, struct group **result);
int getgrnam_r(const char *name, struct group *gr, char *buf, size_t len, struct group **result);
int getgrouplist(const char *user, gid_t group, gid_t *groups, int *ngroups);
int getgroups(int n, gid_t list[]);
int setgroups(size_t n, const gid_t *list);
void setgrent(void);
void endgrent(void);
struct group *getgrent(void);

#endif

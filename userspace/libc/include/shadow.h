// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// /etc/shadow (step 17): one line per user,
// name:password-hash:last-change:min:max:warn:inactive:expire:flag
// (days since 1970; empty fields read as -1). A hash of "!" or "*" (or
// starting with "!") cannot log in; an empty one needs no password.
#ifndef _SHADOW_H
#define _SHADOW_H
#include <stdio.h>
struct spwd {
    char *sp_namp;
    char *sp_pwdp;
    long sp_lstchg, sp_min, sp_max, sp_warn, sp_inact, sp_expire;
    unsigned long sp_flag;
};
struct spwd *getspnam(const char *name);
struct spwd *getspent(void);
void setspent(void);
void endspent(void);
int putspent(const struct spwd *sp, FILE *f);
#endif

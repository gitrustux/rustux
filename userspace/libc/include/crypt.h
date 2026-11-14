// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Password hashing (step 17): SHA-512 crypt ("$6$")
#ifndef _CRYPT_H
#define _CRYPT_H
struct crypt_data { char output[256]; };
char *crypt(const char *key, const char *setting);
char *crypt_r(const char *key, const char *setting, struct crypt_data *data);
// A setting with a random salt: prefix "$6$" (or 0); count, rnd: unused
// unless rnd holds at least 12 random bytes
char *crypt_gensalt(const char *prefix, unsigned long count, const char *rnd, int nrnd);
#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// One locale: "C" with UTF-8 text

#ifndef _LOCALE_H
#define _LOCALE_H

#define LC_CTYPE 0
#define LC_NUMERIC 1
#define LC_TIME 2
#define LC_COLLATE 3
#define LC_MONETARY 4
#define LC_MESSAGES 5
#define LC_ALL 6

struct lconv { char *decimal_point, *thousands_sep, *grouping; };

char *setlocale(int category, const char *name);
struct lconv *localeconv(void);

#endif

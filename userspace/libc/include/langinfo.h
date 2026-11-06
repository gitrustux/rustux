// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _LANGINFO_H
#define _LANGINFO_H

typedef int nl_item;
#define CODESET 14
#define RADIXCHAR 0x10000
#define THOUSEP 0x10001
#define D_T_FMT 0x20028
#define D_FMT 0x20029
#define T_FMT 0x2002A

char *nl_langinfo(nl_item item);

#endif

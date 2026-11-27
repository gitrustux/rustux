// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Byte order (as glibc's and musl's <endian.h>), from the compiler's own
// macros, so it is right on every architecture (2026-11-26)
#ifndef _ENDIAN_H
#define _ENDIAN_H

#include <stdint.h>

#define __LITTLE_ENDIAN 1234
#define __BIG_ENDIAN 4321
#define __PDP_ENDIAN 3412
#if __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define __BYTE_ORDER __BIG_ENDIAN
#else
#define __BYTE_ORDER __LITTLE_ENDIAN
#endif
#define LITTLE_ENDIAN __LITTLE_ENDIAN
#define BIG_ENDIAN __BIG_ENDIAN
#define PDP_ENDIAN __PDP_ENDIAN
#define BYTE_ORDER __BYTE_ORDER

#if __BYTE_ORDER == __LITTLE_ENDIAN
#define htobe16(x) __builtin_bswap16((uint16_t)(x))
#define htobe32(x) __builtin_bswap32((uint32_t)(x))
#define htobe64(x) __builtin_bswap64((uint64_t)(x))
#define htole16(x) ((uint16_t)(x))
#define htole32(x) ((uint32_t)(x))
#define htole64(x) ((uint64_t)(x))
#else
#define htobe16(x) ((uint16_t)(x))
#define htobe32(x) ((uint32_t)(x))
#define htobe64(x) ((uint64_t)(x))
#define htole16(x) __builtin_bswap16((uint16_t)(x))
#define htole32(x) __builtin_bswap32((uint32_t)(x))
#define htole64(x) __builtin_bswap64((uint64_t)(x))
#endif
#define be16toh(x) htobe16(x)
#define be32toh(x) htobe32(x)
#define be64toh(x) htobe64(x)
#define le16toh(x) htole16(x)
#define le32toh(x) htole32(x)
#define le64toh(x) htole64(x)

#endif

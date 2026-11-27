# Ported C libraries (`userspace/ports/`)

Libraries from elsewhere that Rustica programs link against, built for
Rustux with its C library. Added 2026-11-26 for the web browser (phase W,
`docs/BROWSER.md`).

```
ports/build-ports.sh            # all of them, then the programs in progs/
ports/build-ports.sh NAME...    # those alone (their dependencies built already)
```

It needs what `userspace/build.sh` needs (gcc for x86-64, ar) and that
script run first (`build/libc.a`); NetSurf also needs perl and the build
machine's libpng (for its tools that run during the build). The output is
`userspace/build/sysroot/` (`include/`, `lib/libNAME.a`, `lib/pkgconfig/`)
and, for programs, `userspace/bin/`, which the kernel embeds (NetSurf:
`userspace/apps/bin/`, an app's program).

Each port is a folder: `src/` holds the upstream sources it uses (trimmed:
no tests, documents or other platforms), and `recipe.sh` says where they
came from (repository, tag or commit, licence), what was changed, and how
it is compiled (`compile`, `archive`, `headers`). Sources are unchanged
unless the recipe says otherwise.

| Port | Version | Licence | Notes |
|---|---|---|---|
| zlib | 1.3.1 | zlib | |
| mbedtls | 3.6.2 | Apache 2.0 | `rustux_config.h`: entropy from `getrandom`, no sockets or key files |
| curl | 8.11.1 | curl | `curl_config.h` by hand: HTTP, HTTPS (mbedTLS), DNS on a thread; CA bundle `/etc/ssl/certs/ca-certificates.crt` |
| libpng | 1.6.44 | libpng | the prebuilt `pnglibconf.h` |
| libjpeg-turbo | 3.0.4 | IJG, BSD | no SIMD; its config headers by hand |
| freetype | 2.13.3 | FTL | TrueType, CFF, hinting, the anti-aliasing renderer |
| libwapcaplet, libparserutils, libhubbub, libcss, libdom, libnsutils, libnsbmp, libnsgif, libnspsl, libutf8proc | NetSurf's, November 2026 | MIT | source lists from their own builds; generated files included |
| libnsfb | NetSurf's | MIT | plus `rustica.c`, a surface that is a window of the compositor |
| netsurf | 3.12 (dev) | GPL 2 | its own Makefile with `tools/rx-cc` (a fresh tree each build); our changes in `netsurf/rustica.patch`; the program and its resources go to the app `org.rustica.browser` (`userspace/apps/`) |

Tools:

- `tools/rx-cc`: gcc for Rustux programs, for build systems of their own
  (compiles against the C library and the sysroot, links static Rustux
  executables).
- `tools/make-pc.sh`: pkg-config files for the ports.
- `tools/gperf-lite.py`: what the ports need of GNU gperf (a sorted table
  and binary search instead of a perfect hash), as gperf is not on the
  build machine.

`progs/NAME.c` are programs built against the ports, their libraries on a
`// link:` line: `curltest` (libcurl over mbedTLS, for the network test).

The C library gained, for these ports: `setjmp`/`longjmp` (each
architecture), the `scanf` family, `iconv` (musl's), `basename`/`dirname`,
`scandir`/`alphasort`, `strtof`/`strtold`, `sincos`, `setbuf`,
`fseeko`/`ftello`, `isascii`, `<endian.h>`, every `<inttypes.h>` format,
the `S_I*` mode bits, the `M_*` constants, `EILSEQ`, `_POSIX_VERSION`, and
`fd_set` through `<sys/time.h>`.

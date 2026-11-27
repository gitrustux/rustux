/* libjpeg-turbo 3.0.4's jconfigint.h for Rustux (by hand, from
 * jconfigint.h.in) */
#define BUILD  "20261126"
#define HIDDEN  __attribute__((visibility("hidden")))
#undef inline
#define INLINE  __inline__ __attribute__((always_inline))
#define THREAD_LOCAL  __thread
#define PACKAGE_NAME  "libjpeg-turbo"
#define VERSION  "3.0.4"
#define SIZEOF_SIZE_T  8
#define HAVE_BUILTIN_CTZL
#define FALLTHROUGH  __attribute__((fallthrough));
#ifndef BITS_IN_JSAMPLE
#define BITS_IN_JSAMPLE  8
#endif
#undef C_ARITH_CODING_SUPPORTED
#undef D_ARITH_CODING_SUPPORTED
#undef WITH_SIMD
#if BITS_IN_JSAMPLE == 8
#define C_ARITH_CODING_SUPPORTED 1
#define D_ARITH_CODING_SUPPORTED 1
#endif

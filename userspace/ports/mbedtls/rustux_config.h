/* mbedTLS on Rustux: changes to the default configuration (mbedtls_config.h)
 * for libcurl's TLS (the browser, phase W) */

/* Entropy from the C library's getrandom (no /dev/urandom on Rustux):
 * rustux_entropy.c */
#define MBEDTLS_NO_PLATFORM_ENTROPY
#define MBEDTLS_ENTROPY_HARDWARE_ALT

/* Not used by libcurl, and they assume more of the system */
#undef MBEDTLS_NET_C
#undef MBEDTLS_TIMING_C
#undef MBEDTLS_PSA_ITS_FILE_C
#undef MBEDTLS_PSA_CRYPTO_STORAGE_C

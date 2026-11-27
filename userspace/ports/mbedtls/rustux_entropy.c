/* mbedTLS's hardware entropy hook on Rustux: the C library's getrandom
 * (the CPU's RDRAND mixed with the clocks) */
#include <stddef.h>
#include <sys/random.h>

int mbedtls_hardware_poll(void *data, unsigned char *output, size_t len, size_t *olen) {
    (void)data;
    *olen = getrandom(output, len, 0) == (ssize_t)len ? len : 0;
    return 0;
}

# mbedTLS 3.6.2 (https://github.com/Mbed-TLS/mbedtls, tag v3.6.2; Apache 2.0
# or GPL 2.0+, src/LICENSE): the library unchanged; rustux_config.h changes
# the default configuration (entropy from getrandom, no sockets, timing or
# key storage files), rustux_entropy.c is the entropy hook
F="-Isrc/include -Isrc/library -I. -DMBEDTLS_USER_CONFIG_FILE=\"rustux_config.h\""
compile "$OBJ" "$F" src/library/*.c rustux_entropy.c
archive mbedcrypto "$OBJ"
# One archive with all three (libcurl links -lmbedtls -lmbedx509
# -lmbedcrypto; the same objects serve each name)
cp "$SYSROOT/lib/libmbedcrypto.a" "$SYSROOT/lib/libmbedtls.a"
cp "$SYSROOT/lib/libmbedcrypto.a" "$SYSROOT/lib/libmbedx509.a"
headers . src/include/mbedtls src/include/psa
cp rustux_config.h "$SYSROOT/include/mbedtls/"

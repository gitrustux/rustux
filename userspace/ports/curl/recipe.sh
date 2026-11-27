# libcurl 8.11.1 (https://github.com/curl/curl, tag curl-8_11_1; the curl
# licence, src/COPYING): lib/ and include/ unchanged, without QUIC and SSH (their headers kept, as lib/ includes them, and vquic.c, which says what HTTP/3 cannot do);
# curl_config.h is our configuration (HTTP and HTTPS with mbedTLS)
F="-Isrc/include -Isrc/lib -I. -DHAVE_CONFIG_H -DBUILDING_LIBCURL -DCURL_STATICLIB -include curl_config.h"
compile "$OBJ" "$F" src/lib/*.c src/lib/vtls/*.c src/lib/vauth/*.c src/lib/vquic/vquic.c
archive curl "$OBJ"
headers . src/include/curl

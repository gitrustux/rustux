// curltest: fetch a URL with libcurl (the ported library, over mbedTLS) and
// say what came back: the check that the browser's network library works
// on Rustux (scripts/run-network-test.sh).
//
//   curltest [--cacert FILE] [-k] [-o FILE] URL
//
// Prints "status N, M bytes" and libcurl's versions (or curl's error), and
// the body unless -o.
//
// link: curl mbedcrypto z
#include <curl/curl.h>
#include <stdio.h>
#include <string.h>

static size_t got;

static size_t body(char *p, size_t size, size_t n, void *out) {
    got += size * n;
    if (out) fwrite(p, size, n, out);
    return size * n;
}

int main(int argc, char **argv) {
    const char *url = 0, *ca = 0, *outfile = 0;
    int insecure = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--cacert") && i + 1 < argc) ca = argv[++i];
        else if (!strcmp(argv[i], "-k")) insecure = 1;
        else if (!strcmp(argv[i], "-o") && i + 1 < argc) outfile = argv[++i];
        else url = argv[i];
    }
    if (!url) {
        fprintf(stderr, "usage: curltest [--cacert FILE] [-k] [-o FILE] URL\n");
        return 2;
    }
    curl_global_init(CURL_GLOBAL_DEFAULT);
    CURL *c = curl_easy_init();
    FILE *out = outfile ? fopen(outfile, "wb") : stdout;
    if (!out) {
        perror(outfile);
        return 1;
    }
    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, body);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, out);
    curl_easy_setopt(c, CURLOPT_ACCEPT_ENCODING, "");
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, 20L);
    if (ca) curl_easy_setopt(c, CURLOPT_CAINFO, ca);
    if (insecure) {
        curl_easy_setopt(c, CURLOPT_SSL_VERIFYPEER, 0L);
        curl_easy_setopt(c, CURLOPT_SSL_VERIFYHOST, 0L);
    }
    CURLcode r = curl_easy_perform(c);
    if (outfile) fclose(out);
    if (r != CURLE_OK) {
        printf("\ncurltest: error %d: %s\n", (int)r, curl_easy_strerror(r));
        return 1;
    }
    long status = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
    printf("\ncurltest: status %ld, %zu bytes\n", status, got);
    printf("curltest: %s\n", curl_version());
    curl_easy_cleanup(c);
    return 0;
}

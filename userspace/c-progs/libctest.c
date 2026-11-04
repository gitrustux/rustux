// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// libctest: checks of the C library (formatting, strings, numbers,
// sorting, memory, environment, startup)

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int passed, failed;

static void check(const char *name, int ok) {
    if (ok) passed++;
    else { failed++; printf("  FAIL %s\n", name); }
}

static int fmt_is(const char *want, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
static int fmt_is(const char *want, const char *fmt, ...) {
    char buf[128];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (strcmp(buf, want) || n != (int)strlen(want)) {
        printf("  printf(\"%s\") gave \"%s\" (%d), want \"%s\"\n", fmt, buf, n, want);
        return 0;
    }
    return 1;
}

static char *volatile null_str; // NULL, hidden from the compiler's format checks

static void formatting(void) {
    check("%d", fmt_is("42 -7 0", "%d %d %d", 42, -7, 0));
    check("width/flags", fmt_is("[   42][42   ][00042][+42][ 42]", "[%5d][%-5d][%05d][%+d][% d]", 42, 42, 42, 42, 42));
    check("precision", fmt_is("[00042][  042][]", "[%.5d][%5.3d][%.0d]", 42, 42, 0));
    check("negative zero-pad", fmt_is("-0042", "%05d", -42));
    check("hex/octal", fmt_is("ff FF 0xff 17 017", "%x %X %#x %o %#o", 255, 255, 255, 15, 15));
    check("long long", fmt_is("-9223372036854775808 18446744073709551615", "%lld %llu", LLONG_MIN, ULLONG_MAX));
    check("size_t", fmt_is("123", "%zu", (size_t)123));
    check("hh/h", fmt_is("-1 65535", "%hhd %hu", 255, 65535));
    check("strings", fmt_is("[hi][   hi][hi   ][h][(null)]", "[%s][%5s][%-5s][%.1s][%s]", "hi", "hi", "hi", "hi", null_str));
    check("char/percent", fmt_is("a%b", "%c%%%c", 'a', 'b'));
    check("star width", fmt_is("[  7][7  ]", "[%*d][%*d]", 3, 7, -3, 7));
    check("pointer", fmt_is("0x1000", "%p", (void *)0x1000));
    char small[5];
    int n = snprintf(small, sizeof small, "%s", "truncated");
    check("snprintf truncates", n == 9 && !strcmp(small, "trun"));
}

static int cmp_int(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

static void strings_and_numbers(void) {
    char buf[32];
    check("strlen", strlen("hello") == 5 && strlen("") == 0);
    check("strcmp", strcmp("abc", "abd") < 0 && strcmp("b", "a") > 0 && !strcmp("x", "x"));
    check("strncmp", !strncmp("abcdef", "abcxyz", 3) && strncmp("abc", "abd", 3) < 0);
    check("strcasecmp", !strcasecmp("HeLLo", "hello"));
    check("strchr/strrchr", strchr("a/b/c", '/') - "a/b/c" >= 0 && *strrchr("a/b/c", '/') == '/' && !strchr("abc", 'z'));
    check("strstr", strstr("hello world", "wor") && !strstr("hello", "xyz"));
    check("strspn/strcspn", strspn("aabbc", "ab") == 4 && strcspn("hello world", " ") == 5);
    strcpy(buf, "foo");
    strcat(buf, "bar");
    check("strcpy/strcat", !strcmp(buf, "foobar"));
    char tok[] = "a, b,,c";
    char *save, *t1 = strtok_r(tok, ", ", &save), *t2 = strtok_r(0, ", ", &save), *t3 = strtok_r(0, ", ", &save);
    check("strtok_r", t1 && !strcmp(t1, "a") && t2 && !strcmp(t2, "b") && t3 && !strcmp(t3, "c") && !strtok_r(0, ", ", &save));
    char *d = strdup("copy");
    check("strdup", d && !strcmp(d, "copy"));
    free(d);
    memset(buf, 'x', 10);
    memmove(buf + 2, buf, 5);
    char ov[] = "123456789";
    memmove(ov + 2, ov, 5);
    check("memmove overlap", !strcmp(ov, "121234589"));
    check("memcmp", memcmp("abc", "abd", 3) < 0 && !memcmp("abc", "abc", 3));

    char *end;
    check("strtol", strtol("  -123xyz", &end, 10) == -123 && *end == 'x');
    check("strtol hex", strtol("0x1F", 0, 0) == 31 && strtol("ff", 0, 16) == 255);
    check("strtol octal", strtol("017", 0, 0) == 15);
    errno = 0;
    check("strtol overflow", strtol("99999999999999999999", 0, 10) == LONG_MAX && errno == ERANGE);
    check("strtoul", strtoul("18446744073709551615", 0, 10) == ULONG_MAX);
    check("strtol nothing", strtol("abc", &end, 10) == 0 && !strcmp(end, "abc"));
    check("atoi", atoi("42") == 42 && atoi("-5") == -5);
    check("ctype", isdigit('5') && !isdigit('a') && isspace('\t') && toupper('a') == 'A' && isalpha('Z') && ispunct('!'));

    int v[200];
    for (int i = 0; i < 200; i++) v[i] = (i * 7919) % 211 - 100;
    qsort(v, 200, sizeof *v, cmp_int);
    int sorted = 1;
    for (int i = 1; i < 200; i++) if (v[i - 1] > v[i]) sorted = 0;
    check("qsort", sorted);
    int key = v[123];
    int *hit = bsearch(&key, v, 200, sizeof *v, cmp_int);
    check("bsearch", hit && *hit == key);
}

static void memory(void) {
    // Many sizes, filled with a pattern, checked after all are allocated
    enum { N = 400 };
    static char *p[N];
    static size_t sz[N];
    int ok = 1;
    for (int i = 0; i < N; i++) {
        sz[i] = (size_t)(i * 37 % 5000) + 1 + (i % 50 == 0 ? 100000 : 0);
        p[i] = malloc(sz[i]);
        if (!p[i] || ((unsigned long)p[i] & 15)) { ok = 0; break; }
        memset(p[i], i & 0xFF, sz[i]);
    }
    for (int i = 0; ok && i < N; i++) {
        for (size_t k = 0; k < sz[i]; k++) if ((unsigned char)p[i][k] != (i & 0xFF)) { ok = 0; break; }
    }
    check("malloc 400 blocks, 16-byte aligned, no overlap", ok);
    for (int i = 0; i < N; i += 2) free(p[i]);
    for (int i = 0; i < N; i += 2) p[i] = malloc(sz[i]);
    for (int i = 1; ok && i < N; i += 2) {
        for (size_t k = 0; k < sz[i]; k++) if ((unsigned char)p[i][k] != (i & 0xFF)) { ok = 0; break; }
    }
    check("free and reuse keeps other blocks intact", ok);
    for (int i = 0; i < N; i++) free(p[i]);

    // Churn: memory must be reused, not leaked (a leak would run out)
    ok = 1;
    for (int round = 0; round < 20000 && ok; round++) {
        void *a = malloc(100), *b = malloc(3000), *c = malloc(70000);
        if (!a || !b || !c) ok = 0;
        free(a); free(b); free(c);
    }
    check("20000 rounds of malloc/free", ok);

    char *r = malloc(10);
    strcpy(r, "123456789");
    r = realloc(r, 100000);
    check("realloc keeps contents", r && !strcmp(r, "123456789"));
    free(r);
    int *z = calloc(1000, sizeof *z);
    int zero = z != 0;
    for (int i = 0; zero && i < 1000; i++) if (z[i]) zero = 0;
    check("calloc zeroes", zero);
    free(z);
}

static void environment(int argc, char **argv) {
    check("argv[0] is set", argc >= 1 && argv[0] && argv[0][0]);
    check("argv ends with NULL", argv[argc] == 0);
    check("setenv/getenv", !setenv("RX_TEST", "one", 1) && getenv("RX_TEST") && !strcmp(getenv("RX_TEST"), "one"));
    setenv("RX_TEST", "two", 0);
    check("setenv without overwrite", !strcmp(getenv("RX_TEST"), "one"));
    setenv("RX_TEST", "three", 1);
    check("setenv with overwrite", !strcmp(getenv("RX_TEST"), "three"));
    unsetenv("RX_TEST");
    check("unsetenv", getenv("RX_TEST") == 0);
    check("setenv rejects bad names", setenv("A=B", "x", 1) == -1 && errno == EINVAL);
    check("startup handles", rx_proc_self && rx_vmar_root && rx_job_default && rx_thread_self);
    check("stdio is the console", isatty(0) && isatty(1) && isatty(2));
    check("write to a closed fd fails", write(40, "x", 1) == -1 && errno == EBADF);
}

int main(int argc, char **argv) {
    printf("libctest: the C library\n");
    formatting();
    strings_and_numbers();
    memory();
    environment(argc, argv);
    printf("libctest: %d passed, %d failed\n", passed, failed);
    return failed;
}

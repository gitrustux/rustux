// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Passwords (step 17): crypt() with SHA-512 ("$6$salt$hash", as glibc and
// Linux's /etc/shadow use it, after Ulrich Drepper's "Unix crypt using
// SHA-256 and SHA-512"), the shadow file (<shadow.h>), getpass().

#include "libc.h"
#include <crypt.h>
#include <errno.h>
#include <shadow.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <termios.h>
#include <unistd.h>

// ---- SHA-512 ----

typedef struct {
    uint64_t h[8];
    uint8_t buf[128];
    uint64_t len;   // bytes so far
} sha512_t;

static const uint64_t K[80] = {
    0x428a2f98d728ae22ull, 0x7137449123ef65cdull, 0xb5c0fbcfec4d3b2full, 0xe9b5dba58189dbbcull, 0x3956c25bf348b538ull,
    0x59f111f1b605d019ull, 0x923f82a4af194f9bull, 0xab1c5ed5da6d8118ull, 0xd807aa98a3030242ull, 0x12835b0145706fbeull,
    0x243185be4ee4b28cull, 0x550c7dc3d5ffb4e2ull, 0x72be5d74f27b896full, 0x80deb1fe3b1696b1ull, 0x9bdc06a725c71235ull,
    0xc19bf174cf692694ull, 0xe49b69c19ef14ad2ull, 0xefbe4786384f25e3ull, 0x0fc19dc68b8cd5b5ull, 0x240ca1cc77ac9c65ull,
    0x2de92c6f592b0275ull, 0x4a7484aa6ea6e483ull, 0x5cb0a9dcbd41fbd4ull, 0x76f988da831153b5ull, 0x983e5152ee66dfabull,
    0xa831c66d2db43210ull, 0xb00327c898fb213full, 0xbf597fc7beef0ee4ull, 0xc6e00bf33da88fc2ull, 0xd5a79147930aa725ull,
    0x06ca6351e003826full, 0x142929670a0e6e70ull, 0x27b70a8546d22ffcull, 0x2e1b21385c26c926ull, 0x4d2c6dfc5ac42aedull,
    0x53380d139d95b3dfull, 0x650a73548baf63deull, 0x766a0abb3c77b2a8ull, 0x81c2c92e47edaee6ull, 0x92722c851482353bull,
    0xa2bfe8a14cf10364ull, 0xa81a664bbc423001ull, 0xc24b8b70d0f89791ull, 0xc76c51a30654be30ull, 0xd192e819d6ef5218ull,
    0xd69906245565a910ull, 0xf40e35855771202aull, 0x106aa07032bbd1b8ull, 0x19a4c116b8d2d0c8ull, 0x1e376c085141ab53ull,
    0x2748774cdf8eeb99ull, 0x34b0bcb5e19b48a8ull, 0x391c0cb3c5c95a63ull, 0x4ed8aa4ae3418acbull, 0x5b9cca4f7763e373ull,
    0x682e6ff3d6b2b8a3ull, 0x748f82ee5defb2fcull, 0x78a5636f43172f60ull, 0x84c87814a1f0ab72ull, 0x8cc702081a6439ecull,
    0x90befffa23631e28ull, 0xa4506cebde82bde9ull, 0xbef9a3f7b2c67915ull, 0xc67178f2e372532bull, 0xca273eceea26619cull,
    0xd186b8c721c0c207ull, 0xeada7dd6cde0eb1eull, 0xf57d4f7fee6ed178ull, 0x06f067aa72176fbaull, 0x0a637dc5a2c898a6ull,
    0x113f9804bef90daeull, 0x1b710b35131c471bull, 0x28db77f523047d84ull, 0x32caab7b40c72493ull, 0x3c9ebe0a15c9bebcull,
    0x431d67c49c100d4cull, 0x4cc5d4becb3e42b6ull, 0x597f299cfc657e2aull, 0x5fcb6fab3ad6faecull, 0x6c44198c4a475817ull,
};

static uint64_t ror(uint64_t x, int n) { return (x >> n) | (x << (64 - n)); }

static void sha512_block(sha512_t *s, const uint8_t *p) {
    uint64_t w[80];
    for (int i = 0; i < 16; i++) {
        w[i] = 0;
        for (int k = 0; k < 8; k++) w[i] = w[i] << 8 | p[8 * i + k];
    }
    for (int i = 16; i < 80; i++) {
        uint64_t s0 = ror(w[i - 15], 1) ^ ror(w[i - 15], 8) ^ (w[i - 15] >> 7);
        uint64_t s1 = ror(w[i - 2], 19) ^ ror(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint64_t a = s->h[0], b = s->h[1], c = s->h[2], d = s->h[3], e = s->h[4], f = s->h[5], g = s->h[6], h = s->h[7];
    for (int i = 0; i < 80; i++) {
        uint64_t t1 = h + (ror(e, 14) ^ ror(e, 18) ^ ror(e, 41)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
        uint64_t t2 = (ror(a, 28) ^ ror(a, 34) ^ ror(a, 39)) + ((a & b) ^ (a & c) ^ (b & c));
        h = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
    }
    s->h[0] += a; s->h[1] += b; s->h[2] += c; s->h[3] += d;
    s->h[4] += e; s->h[5] += f; s->h[6] += g; s->h[7] += h;
}

static void sha512_init(sha512_t *s) {
    static const uint64_t iv[8] = { 0x6a09e667f3bcc908ull, 0xbb67ae8584caa73bull, 0x3c6ef372fe94f82bull, 0xa54ff53a5f1d36f1ull,
                                    0x510e527fade682d1ull, 0x9b05688c2b3e6c1full, 0x1f83d9abfb41bd6bull, 0x5be0cd19137e2179ull };
    memcpy(s->h, iv, sizeof iv);
    s->len = 0;
}

static void sha512_add(sha512_t *s, const void *data, size_t n) {
    const uint8_t *p = data;
    while (n) {
        size_t at = s->len % 128, m = 128 - at < n ? 128 - at : n;
        memcpy(s->buf + at, p, m);
        s->len += m;
        p += m;
        n -= m;
        if (s->len % 128 == 0) sha512_block(s, s->buf);
    }
}

static void sha512_end(sha512_t *s, uint8_t out[64]) {
    uint64_t bits = s->len * 8;
    uint8_t pad = 0x80, zero = 0;
    sha512_add(s, &pad, 1);
    while (s->len % 128 != 112) sha512_add(s, &zero, 1);
    uint8_t len[16] = { 0 };
    for (int i = 0; i < 8; i++) len[15 - i] = (uint8_t)(bits >> (8 * i));
    sha512_add(s, len, 16);
    for (int i = 0; i < 8; i++)
        for (int k = 0; k < 8; k++) out[8 * i + k] = (uint8_t)(s->h[i] >> (56 - 8 * k));
}

// ---- crypt: $6$[rounds=N$]salt$hash ----

static const char b64[] = "./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

static char *put64(char *o, uint32_t v, int n) {
    while (n-- > 0) {
        *o++ = b64[v & 0x3f];
        v >>= 6;
    }
    return o;
}

static char *sha512_crypt(const char *key, const char *setting, char *out, size_t cap) {
    const char *salt = setting + 3;
    unsigned long rounds = 5000;
    int custom = 0;
    if (!strncmp(salt, "rounds=", 7)) {
        char *end;
        unsigned long r = strtoul(salt + 7, &end, 10);
        if (*end != '$') return 0;
        if (r < 1000) r = 1000;
        if (r > 999999999) r = 999999999;
        rounds = r;
        custom = 1;
        salt = end + 1;
    }
    size_t sl = strcspn(salt, "$");
    if (sl > 16) sl = 16;
    size_t kl = strlen(key);
    uint8_t a[64], b[64], dp[64], ds[64];
    sha512_t c;
    // B = sha(key salt key)
    sha512_init(&c);
    sha512_add(&c, key, kl);
    sha512_add(&c, salt, sl);
    sha512_add(&c, key, kl);
    sha512_end(&c, b);
    // A = sha(key salt B-for-each-byte-of-key bits...)
    sha512_init(&c);
    sha512_add(&c, key, kl);
    sha512_add(&c, salt, sl);
    size_t i;
    for (i = kl; i > 64; i -= 64) sha512_add(&c, b, 64);
    sha512_add(&c, b, i);
    for (i = kl; i > 0; i >>= 1) {
        if (i & 1) sha512_add(&c, b, 64);
        else sha512_add(&c, key, kl);
    }
    sha512_end(&c, a);
    // P: from the key, S: from the salt
    sha512_init(&c);
    for (i = 0; i < kl; i++) sha512_add(&c, key, kl);
    sha512_end(&c, dp);
    char *p = malloc(kl + 1), *s = malloc(sl + 1);
    if (!p || !s) { free(p); free(s); return 0; }
    for (i = 0; i < kl; i++) p[i] = (char)dp[i % 64];
    sha512_init(&c);
    for (i = 0; i < 16u + a[0]; i++) sha512_add(&c, salt, sl);
    sha512_end(&c, ds);
    for (i = 0; i < sl; i++) s[i] = (char)ds[i % 64];
    for (unsigned long r = 0; r < rounds; r++) {
        sha512_init(&c);
        if (r & 1) sha512_add(&c, p, kl);
        else sha512_add(&c, a, 64);
        if (r % 3) sha512_add(&c, s, sl);
        if (r % 7) sha512_add(&c, p, kl);
        if (r & 1) sha512_add(&c, a, 64);
        else sha512_add(&c, p, kl);
        sha512_end(&c, a);
    }
    memset(p, 0, kl);
    free(p);
    free(s);
    if (cap < 3 + 18 + 17 + 86 + 2) return 0;
    char *o = out;
    o += sprintf(o, "$6$");
    if (custom) o += sprintf(o, "rounds=%lu$", rounds);
    memcpy(o, salt, sl);
    o += sl;
    *o++ = '$';
    static const int order[21][3] = {
        { 0, 21, 42 }, { 22, 43, 1 }, { 44, 2, 23 }, { 3, 24, 45 }, { 25, 46, 4 }, { 47, 5, 26 }, { 6, 27, 48 },
        { 28, 49, 7 }, { 50, 8, 29 }, { 9, 30, 51 }, { 31, 52, 10 }, { 53, 11, 32 }, { 12, 33, 54 }, { 34, 55, 13 },
        { 56, 14, 35 }, { 15, 36, 57 }, { 37, 58, 16 }, { 59, 17, 38 }, { 18, 39, 60 }, { 40, 61, 19 }, { 62, 20, 41 },
    };
    for (int k = 0; k < 21; k++) o = put64(o, (uint32_t)a[order[k][0]] << 16 | (uint32_t)a[order[k][1]] << 8 | a[order[k][2]], 4);
    o = put64(o, a[63], 2);
    *o = 0;
    memset(a, 0, sizeof a);
    return out;
}

char *crypt_r(const char *key, const char *setting, struct crypt_data *d) {
    if (!key || !setting || strncmp(setting, "$6$", 3)) {
        errno = EINVAL;
        return 0;   // only SHA-512 here
    }
    return sha512_crypt(key, setting, d->output, sizeof d->output);
}

char *crypt(const char *key, const char *setting) {
    static struct crypt_data d;
    return crypt_r(key, setting, &d);
}

// A new "$6$" setting with a random salt of 16 characters
char *crypt_gensalt(const char *prefix, unsigned long count, const char *rnd, int nrnd) {
    static char out[64];
    (void)count;
    if (prefix && strcmp(prefix, "$6$")) {
        errno = EINVAL;
        return 0;
    }
    uint8_t r[12];
    if (rnd && nrnd >= 12) memcpy(r, rnd, 12);
    else if (getrandom(r, sizeof r, 0) != sizeof r) return 0;
    char *o = out + sprintf(out, "$6$");
    for (int i = 0; i < 12; i += 3) o = put64(o, (uint32_t)r[i] << 16 | (uint32_t)r[i + 1] << 8 | r[i + 2], 4);
    *o++ = '$';
    *o = 0;
    return out;
}

// ---- /etc/shadow ----

static FILE *spf;
static struct spwd sp_static;
static char sp_line[1024];

static long num(const char *s) { return *s ? strtol(s, 0, 10) : -1; }

// name:password:lastchg:min:max:warn:inactive:expire:flag
static int parse_sp(char *line, struct spwd *sp) {
    char *v[9] = { 0 };
    int n = 0;
    line[strcspn(line, "\n")] = 0;
    for (char *s = line; n < 9;) {
        v[n++] = s;
        char *c = strchr(s, ':');
        if (!c) break;
        *c = 0;
        s = c + 1;
    }
    if (n < 2 || !*v[0]) return -1;
    for (int i = n; i < 9; i++) v[i] = "";
    sp->sp_namp = v[0];
    sp->sp_pwdp = v[1];
    sp->sp_lstchg = num(v[2]);
    sp->sp_min = num(v[3]);
    sp->sp_max = num(v[4]);
    sp->sp_warn = num(v[5]);
    sp->sp_inact = num(v[6]);
    sp->sp_expire = num(v[7]);
    sp->sp_flag = (unsigned long)num(v[8]);
    return 0;
}

void setspent(void) {
    if (spf) rewind(spf);
    else spf = fopen("/etc/shadow", "r");
}

void endspent(void) {
    if (spf) fclose(spf);
    spf = 0;
}

struct spwd *getspent(void) {
    if (!spf) setspent();
    if (!spf) return 0;
    while (fgets(sp_line, sizeof sp_line, spf))
        if (sp_line[0] != '#' && parse_sp(sp_line, &sp_static) == 0) return &sp_static;
    return 0;
}

struct spwd *getspnam(const char *name) {
    FILE *f = fopen("/etc/shadow", "r");
    if (!f) return 0;
    struct spwd *r = 0;
    while (!r && fgets(sp_line, sizeof sp_line, f))
        if (sp_line[0] != '#' && parse_sp(sp_line, &sp_static) == 0 && !strcmp(sp_static.sp_namp, name)) r = &sp_static;
    fclose(f);
    if (!r) errno = ENOENT;
    return r;
}

static void putnum(FILE *f, long v, int last) { if (v >= 0) fprintf(f, "%ld", v); if (!last) fputc(':', f); }

int putspent(const struct spwd *sp, FILE *f) {
    fprintf(f, "%s:%s:", sp->sp_namp, sp->sp_pwdp ? sp->sp_pwdp : "");
    putnum(f, sp->sp_lstchg, 0);
    putnum(f, sp->sp_min, 0);
    putnum(f, sp->sp_max, 0);
    putnum(f, sp->sp_warn, 0);
    putnum(f, sp->sp_inact, 0);
    putnum(f, sp->sp_expire, 0);
    if ((long)sp->sp_flag >= 0 && sp->sp_flag != (unsigned long)-1) fprintf(f, "%lu", sp->sp_flag);
    fputc('\n', f);
    return ferror(f) ? -1 : 0;
}

// ---- getpass: a line read without echo ----

char *getpass(const char *prompt) {
    static char buf[256];
    struct termios old, quiet;
    int fd = 0, tty = tcgetattr(fd, &old) == 0;
    fputs(prompt, stderr);
    fflush(stderr);
    if (tty) {
        quiet = old;
        quiet.c_lflag &= ~(tcflag_t)ECHO;
        tcsetattr(fd, TCSAFLUSH, &quiet);
    }
    size_t n = 0;
    char ch;
    ssize_t r;
    while ((r = read(fd, &ch, 1)) == 1 && ch != '\n' && ch != '\r')
        if (n + 1 < sizeof buf) buf[n++] = ch;
    buf[n] = 0;
    if (tty) tcsetattr(fd, TCSAFLUSH, &old);
    fputs("\n", stderr);
    if (r < 0 || (r == 0 && n == 0)) return 0;
    return buf;
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Buffered streams on file descriptors. stdout is line-buffered on the
// console and fully buffered otherwise; stderr is unbuffered. Reading and
// writing the same stream without fflush in between is not supported.

#include "libc.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

struct _FILE {
    int fd;
    int mode;           // _IOFBF, _IOLBF, _IONBF
    int eof, err;
    int ungot;          // pushed-back character or -1
    char *buf;
    size_t size;
    size_t wlen;        // bytes waiting to be written
    size_t rpos, rlen;  // read buffer window
    char own[BUFSIZ];
};

static FILE files[3];
FILE *stdin = &files[0], *stdout = &files[1], *stderr = &files[2];

void __stdio_init(void) {
    for (int i = 0; i < 3; i++) {
        files[i].fd = i;
        files[i].ungot = -1;
        files[i].buf = files[i].own;
        files[i].size = BUFSIZ;
    }
    stdin->mode = _IOLBF;
    stdout->mode = isatty(1) ? _IOLBF : _IOFBF;
    stderr->mode = _IONBF;
}

int fflush(FILE *f) {
    if (!f) {
        __stdio_flush_all();
        return 0;
    }
    size_t done = 0;
    while (done < f->wlen) {
        ssize_t r = write(f->fd, f->buf + done, f->wlen - done);
        if (r <= 0) {
            f->err = 1;
            f->wlen = 0;
            return EOF;
        }
        done += (size_t)r;
    }
    f->wlen = 0;
    return 0;
}

void __stdio_flush_all(void) {
    fflush(stdout);
    fflush(stderr);
}

int setvbuf(FILE *restrict f, char *restrict buf, int mode, size_t size) {
    fflush(f);
    f->mode = mode;
    if (buf && size) {
        f->buf = buf;
        f->size = size;
    }
    return 0;
}

size_t fwrite(const void *restrict p, size_t size, size_t n, FILE *restrict f) {
    size_t total = size * n;
    if (!total) return 0;
    const char *s = p;
    if (f->mode == _IONBF) {
        if (fflush(f)) return 0;
        size_t done = 0;
        while (done < total) {
            ssize_t r = write(f->fd, s + done, total - done);
            if (r <= 0) { f->err = 1; break; }
            done += (size_t)r;
        }
        return done / size;
    }
    for (size_t i = 0; i < total; i++) {
        f->buf[f->wlen++] = s[i];
        if (f->wlen == f->size || (f->mode == _IOLBF && s[i] == '\n')) {
            if (fflush(f)) return i / size;
        }
    }
    return n;
}

int fputc(int c, FILE *f) {
    unsigned char ch = (unsigned char)c;
    return fwrite(&ch, 1, 1, f) == 1 ? ch : EOF;
}
int putc(int c, FILE *f) { return fputc(c, f); }
int putchar(int c) { return fputc(c, stdout); }

int fputs(const char *restrict s, FILE *restrict f) {
    size_t n = strlen(s);
    return fwrite(s, 1, n, f) == n ? 0 : EOF;
}

int puts(const char *s) {
    if (fputs(s, stdout) == EOF) return EOF;
    return fputc('\n', stdout) == EOF ? EOF : 0;
}

int __format(void (*out)(void *, const char *, size_t), void *ctx, const char *fmt, va_list ap);

static void file_sink(void *ctx, const char *s, size_t n) {
    fwrite(s, 1, n, (FILE *)ctx);
}

int vfprintf(FILE *restrict f, const char *restrict fmt, va_list ap) {
    return __format(file_sink, f, fmt, ap);
}

int vprintf(const char *restrict fmt, va_list ap) {
    return vfprintf(stdout, fmt, ap);
}

int fprintf(FILE *restrict f, const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(f, fmt, ap);
    va_end(ap);
    return r;
}

int printf(const char *restrict fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vfprintf(stdout, fmt, ap);
    va_end(ap);
    return r;
}

// Reading

int fgetc(FILE *f) {
    if (f->ungot >= 0) {
        int c = f->ungot;
        f->ungot = -1;
        return c;
    }
    if (f->rpos >= f->rlen) {
        if (f->eof) return EOF;
        // Reading from the console: flush pending output first (prompts)
        if (f == stdin) fflush(stdout);
        ssize_t r = read(f->fd, f->buf, f->size);
        if (r <= 0) {
            if (r < 0) f->err = 1; else f->eof = 1;
            return EOF;
        }
        f->rpos = 0;
        f->rlen = (size_t)r;
    }
    return (unsigned char)f->buf[f->rpos++];
}
int getc(FILE *f) { return fgetc(f); }
int getchar(void) { return fgetc(stdin); }

int ungetc(int c, FILE *f) {
    if (c == EOF || f->ungot >= 0) return EOF;
    f->ungot = (unsigned char)c;
    f->eof = 0;
    return c;
}

char *fgets(char *restrict s, int n, FILE *restrict f) {
    int i = 0;
    if (n <= 0) return 0;
    while (i < n - 1) {
        int c = fgetc(f);
        if (c == EOF) break;
        s[i++] = (char)c;
        if (c == '\n') break;
    }
    if (i == 0) return 0;
    s[i] = 0;
    return s;
}

size_t fread(void *restrict p, size_t size, size_t n, FILE *restrict f) {
    size_t total = size * n, i = 0;
    char *d = p;
    for (; i < total; i++) {
        int c = fgetc(f);
        if (c == EOF) break;
        d[i] = (char)c;
    }
    return size ? i / size : 0;
}

ssize_t getdelim(char **restrict line, size_t *restrict cap, int delim, FILE *restrict f) {
    size_t len = 0;
    if (!*line || !*cap) {
        *cap = 128;
        if (!(*line = malloc(*cap))) { errno = ENOMEM; return -1; }
    }
    for (;;) {
        int c = fgetc(f);
        if (c == EOF) break;
        if (len + 2 > *cap) {
            size_t nc = *cap * 2;
            char *nl = realloc(*line, nc);
            if (!nl) { errno = ENOMEM; return -1; }
            *line = nl;
            *cap = nc;
        }
        (*line)[len++] = (char)c;
        if (c == delim) break;
    }
    (*line)[len] = 0;
    return len ? (ssize_t)len : -1;
}

ssize_t getline(char **restrict line, size_t *restrict cap, FILE *restrict f) {
    return getdelim(line, cap, '\n', f);
}

int feof(FILE *f) { return f->eof; }
int ferror(FILE *f) { return f->err; }
void clearerr(FILE *f) { f->eof = f->err = 0; }
int fileno(FILE *f) { return f->fd; }

void perror(const char *s) {
    if (s && *s) fprintf(stderr, "%s: %s\n", s, strerror(errno));
    else fprintf(stderr, "%s\n", strerror(errno));
}

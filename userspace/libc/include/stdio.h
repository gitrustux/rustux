// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _STDIO_H
#define _STDIO_H

#include <stdarg.h>
#include <stddef.h>
#include <sys/types.h>

#define EOF (-1)
#define BUFSIZ 4096

#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2

typedef struct _FILE FILE;

extern FILE *stdin, *stdout, *stderr;

int printf(const char *restrict fmt, ...) __attribute__((format(printf, 1, 2)));
int fprintf(FILE *restrict f, const char *restrict fmt, ...) __attribute__((format(printf, 2, 3)));
int dprintf(int fd, const char *restrict fmt, ...) __attribute__((format(printf, 2, 3)));
int sprintf(char *restrict buf, const char *restrict fmt, ...) __attribute__((format(printf, 2, 3)));
int snprintf(char *restrict buf, size_t n, const char *restrict fmt, ...) __attribute__((format(printf, 3, 4)));
int vprintf(const char *restrict fmt, va_list ap);
int vfprintf(FILE *restrict f, const char *restrict fmt, va_list ap);
int vdprintf(int fd, const char *restrict fmt, va_list ap);
int vsprintf(char *restrict buf, const char *restrict fmt, va_list ap);
int vsnprintf(char *restrict buf, size_t n, const char *restrict fmt, va_list ap);

int fputc(int c, FILE *f);
int putc(int c, FILE *f);
int putchar(int c);
int fputs(const char *restrict s, FILE *restrict f);
int puts(const char *s);
size_t fwrite(const void *restrict p, size_t size, size_t n, FILE *restrict f);

int fgetc(FILE *f);
int getc(FILE *f);
int getchar(void);
int ungetc(int c, FILE *f);
char *fgets(char *restrict s, int n, FILE *restrict f);
size_t fread(void *restrict p, size_t size, size_t n, FILE *restrict f);
ssize_t getline(char **restrict line, size_t *restrict cap, FILE *restrict f);
ssize_t getdelim(char **restrict line, size_t *restrict cap, int delim, FILE *restrict f);

int fflush(FILE *f);
int setvbuf(FILE *restrict f, char *restrict buf, int mode, size_t size);
int feof(FILE *f);
int ferror(FILE *f);
void clearerr(FILE *f);
int fileno(FILE *f);
void perror(const char *s);

#endif

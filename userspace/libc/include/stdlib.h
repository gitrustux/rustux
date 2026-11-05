// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
#define RAND_MAX 0x7fffffff

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *p, size_t size);
void free(void *p);
int posix_memalign(void **out, size_t align, size_t size);
void *aligned_alloc(size_t align, size_t size);
void *memalign(size_t align, size_t size);
size_t malloc_usable_size(void *p);
char *realpath(const char *restrict path, char *restrict resolved);
double strtod(const char *restrict s, char **restrict end);
double atof(const char *s);

void exit(int status) __attribute__((noreturn));
void abort(void) __attribute__((noreturn));
int atexit(void (*fn)(void));

char *getenv(const char *name);
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);
int putenv(char *string);
int clearenv(void);

long strtol(const char *restrict s, char **restrict end, int base);
long long strtoll(const char *restrict s, char **restrict end, int base);
unsigned long strtoul(const char *restrict s, char **restrict end, int base);
unsigned long long strtoull(const char *restrict s, char **restrict end, int base);
int atoi(const char *s);
long atol(const char *s);
long long atoll(const char *s);

int abs(int v);
long labs(long v);
long long llabs(long long v);

void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));
void *bsearch(const void *key, const void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));

int rand(void);
void srand(unsigned int seed);

#endif

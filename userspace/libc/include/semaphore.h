// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SEMAPHORE_H
#define _SEMAPHORE_H

#include <time.h>

typedef struct { volatile int __val[4 * sizeof(long) / sizeof(int)]; } sem_t;

int sem_init(sem_t *s, int pshared, unsigned value);
int sem_destroy(sem_t *s);
int sem_wait(sem_t *s);
int sem_trywait(sem_t *s);
int sem_timedwait(sem_t *restrict s, const struct timespec *restrict abstime);
int sem_post(sem_t *s);
int sem_getvalue(sem_t *restrict s, int *restrict value);

#endif

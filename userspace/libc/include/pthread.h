// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// POSIX threads (the parts Rust's std and common C programs use)

#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <sys/types.h>
#include <time.h>

#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1
#define PTHREAD_MUTEX_NORMAL 0
#define PTHREAD_MUTEX_DEFAULT 0
#define PTHREAD_MUTEX_RECURSIVE 1
#define PTHREAD_MUTEX_ERRORCHECK 2
#define PTHREAD_MUTEX_INITIALIZER { { 0 } }
#define PTHREAD_COND_INITIALIZER { { 0 } }
#define PTHREAD_RWLOCK_INITIALIZER { { 0 } }
#define PTHREAD_ONCE_INIT 0
#define PTHREAD_KEYS_MAX 128
#define PTHREAD_STACK_MIN 16384

int pthread_create(pthread_t *restrict t, const pthread_attr_t *restrict attr, void *(*fn)(void *), void *restrict arg);
int pthread_join(pthread_t t, void **result);
int pthread_detach(pthread_t t);
void pthread_exit(void *result) __attribute__((noreturn));
pthread_t pthread_self(void);
int pthread_equal(pthread_t a, pthread_t b);
int pthread_setname_np(pthread_t t, const char *name);
int pthread_getattr_np(pthread_t t, pthread_attr_t *attr);

int pthread_attr_init(pthread_attr_t *a);
int pthread_attr_destroy(pthread_attr_t *a);
int pthread_attr_setstacksize(pthread_attr_t *a, size_t size);
int pthread_attr_getstacksize(const pthread_attr_t *restrict a, size_t *restrict size);
int pthread_attr_getstack(const pthread_attr_t *restrict a, void **restrict addr, size_t *restrict size);
int pthread_attr_setdetachstate(pthread_attr_t *a, int state);
int pthread_attr_getdetachstate(const pthread_attr_t *a, int *state);
int pthread_attr_getguardsize(const pthread_attr_t *restrict a, size_t *restrict size);
int pthread_attr_setguardsize(pthread_attr_t *a, size_t size);

int pthread_key_create(pthread_key_t *key, void (*dtor)(void *));
int pthread_key_delete(pthread_key_t key);
void *pthread_getspecific(pthread_key_t key);
int pthread_setspecific(pthread_key_t key, const void *value);
int pthread_once(pthread_once_t *once, void (*fn)(void));

int pthread_mutex_init(pthread_mutex_t *restrict m, const pthread_mutexattr_t *restrict attr);
int pthread_mutex_destroy(pthread_mutex_t *m);
int pthread_mutex_lock(pthread_mutex_t *m);
int pthread_mutex_trylock(pthread_mutex_t *m);
int pthread_mutex_unlock(pthread_mutex_t *m);
int pthread_mutexattr_init(pthread_mutexattr_t *a);
int pthread_mutexattr_destroy(pthread_mutexattr_t *a);
int pthread_mutexattr_settype(pthread_mutexattr_t *a, int type);

int pthread_cond_init(pthread_cond_t *restrict c, const pthread_condattr_t *restrict attr);
int pthread_cond_destroy(pthread_cond_t *c);
int pthread_cond_wait(pthread_cond_t *restrict c, pthread_mutex_t *restrict m);
int pthread_cond_timedwait(pthread_cond_t *restrict c, pthread_mutex_t *restrict m, const struct timespec *restrict abstime);
int pthread_cond_signal(pthread_cond_t *c);
int pthread_cond_broadcast(pthread_cond_t *c);
int pthread_condattr_init(pthread_condattr_t *a);
int pthread_condattr_destroy(pthread_condattr_t *a);
int pthread_condattr_setclock(pthread_condattr_t *a, clockid_t clock);

#endif

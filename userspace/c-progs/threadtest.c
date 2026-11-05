// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// threadtest: threads in the C library (checklist step 10): thread-local
// storage, create/join/detach, mutexes, condition variables, keys, once,
// errno per thread, sleeping and timed waits

#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int passed, failed;

static void check(const char *name, int ok) {
    if (ok) passed++;
    else { failed++; printf("  FAIL %s\n", name); }
}

static __thread int tls_init = 42;
static __thread long tls_zero[8];

static void *tls_worker(void *arg) {
    long ok = tls_init == 42 && tls_zero[3] == 0;
    tls_init = (int)(long)arg;
    tls_zero[3] = (long)arg;
    usleep(10000);
    ok = ok && tls_init == (int)(long)arg && tls_zero[3] == (long)arg;
    return (void *)ok;
}

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static long counter;

static void *count_worker(void *arg) {
    (void)arg;
    for (int i = 0; i < 20000; i++) {
        pthread_mutex_lock(&m);
        counter++;
        pthread_mutex_unlock(&m);
    }
    return 0;
}

static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int ready, consumed;

static void *consumer(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    while (!ready) pthread_cond_wait(&cv, &m);
    consumed++;
    pthread_mutex_unlock(&m);
    return 0;
}

static void *errno_worker(void *arg) {
    (void)arg;
    errno = 0;
    close(-1); // EBADF in this thread only
    return (void *)(long)(errno == EBADF);
}

static pthread_key_t key;
static int destroyed;
static void key_dtor(void *v) { __atomic_fetch_add(&destroyed, (int)(long)v, __ATOMIC_SEQ_CST); }

static void *key_worker(void *arg) {
    pthread_setspecific(key, arg);
    return (void *)(long)(pthread_getspecific(key) == arg);
}

static int once_runs;
static pthread_once_t once = PTHREAD_ONCE_INIT;
static void once_fn(void) { once_runs++; }
static void *once_worker(void *arg) {
    (void)arg;
    pthread_once(&once, once_fn);
    return 0;
}

static volatile int detached_ran;
static void *detached_worker(void *arg) {
    (void)arg;
    detached_ran = 1;
    return 0;
}

int main(void) {
    printf("threadtest: threads, locks and thread-local storage\n");
    pthread_t t[8];
    void *r;

    check("TLS initial values in main", tls_init == 42 && tls_zero[0] == 0);
    tls_init = 7;
    int ok = 1;
    for (long i = 0; i < 4; i++) ok &= pthread_create(&t[i], 0, tls_worker, (void *)(i + 100)) == 0;
    for (int i = 0; i < 4; i++) { pthread_join(t[i], &r); ok &= r == (void *)1; }
    check("each thread has its own TLS copy", ok);
    check("main's TLS untouched", tls_init == 7 && tls_zero[3] == 0);

    for (int i = 0; i < 4; i++) pthread_create(&t[i], 0, count_worker, 0);
    for (int i = 0; i < 4; i++) pthread_join(t[i], 0);
    check("mutex: 4 x 20000 increments", counter == 80000);

    for (int i = 0; i < 3; i++) pthread_create(&t[i], 0, consumer, 0);
    usleep(20000);
    pthread_mutex_lock(&m);
    ready = 1;
    pthread_cond_broadcast(&cv);
    pthread_mutex_unlock(&m);
    for (int i = 0; i < 3; i++) pthread_join(t[i], 0);
    check("condition variable broadcast wakes all", consumed == 3);

    pthread_mutex_lock(&m);
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    ts.tv_nsec += 50000000;
    if (ts.tv_nsec >= 1000000000) { ts.tv_sec++; ts.tv_nsec -= 1000000000; }
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    int tr = pthread_cond_timedwait(&cv, &m, &ts);
    clock_gettime(CLOCK_MONOTONIC, &b);
    pthread_mutex_unlock(&m);
    long ms = (b.tv_sec - a.tv_sec) * 1000 + (b.tv_nsec - a.tv_nsec) / 1000000;
    check("cond_timedwait times out after 50 ms", tr == ETIMEDOUT && ms >= 45 && ms < 500);

    errno = 0;
    pthread_create(&t[0], 0, errno_worker, 0);
    pthread_join(t[0], &r);
    check("errno is per thread", r == (void *)1 && errno == 0);

    pthread_key_create(&key, key_dtor);
    pthread_setspecific(key, (void *)1000);
    for (long i = 1; i <= 3; i++) pthread_create(&t[i], 0, key_worker, (void *)i);
    ok = 1;
    for (int i = 1; i <= 3; i++) { pthread_join(t[i], &r); ok &= r == (void *)1; }
    check("keys: own value per thread", ok && pthread_getspecific(key) == (void *)1000);
    check("keys: destructors run at thread exit", destroyed == 6);

    for (int i = 0; i < 6; i++) pthread_create(&t[i], 0, once_worker, 0);
    for (int i = 0; i < 6; i++) pthread_join(t[i], 0);
    check("pthread_once runs once", once_runs == 1);

    pthread_mutexattr_t ma;
    pthread_mutexattr_init(&ma);
    pthread_mutexattr_settype(&ma, PTHREAD_MUTEX_RECURSIVE);
    pthread_mutex_t rm;
    pthread_mutex_init(&rm, &ma);
    check("recursive mutex", pthread_mutex_lock(&rm) == 0 && pthread_mutex_lock(&rm) == 0 &&
                             pthread_mutex_unlock(&rm) == 0 && pthread_mutex_unlock(&rm) == 0 &&
                             pthread_mutex_trylock(&rm) == 0 && pthread_mutex_unlock(&rm) == 0);
    check("trylock of a held mutex -> EBUSY", pthread_mutex_lock(&m) == 0 && pthread_mutex_trylock(&m) == EBUSY &&
                                               pthread_mutex_unlock(&m) == 0);

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_attr_setstacksize(&attr, 64 * 1024);
    check("detached thread", pthread_create(&t[0], &attr, detached_worker, 0) == 0);
    for (int i = 0; i < 100 && !detached_ran; i++) usleep(1000);
    check("  ran", detached_ran);
    // Many short threads: memory is given back
    ok = 1;
    for (int i = 0; i < 200 && ok; i++) {
        ok = pthread_create(&t[0], 0, detached_worker, 0) == 0 && pthread_join(t[0], 0) == 0;
    }
    check("200 threads created and joined", ok);

    printf("threadtest: %d passed, %d failed\n", passed, failed);
    return failed;
}

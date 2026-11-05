// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Threads: thread-local storage, POSIX threads, mutexes and condition
// variables, and futexes.
//
// Each thread has one mapping holding its stack, its copy of the
// program's thread-local storage (the PT_TLS template) and its control
// block (struct __pthread); the thread pointer (FS base, set with
// object_set_property) points at the control block. The kernel has no
// futex call: futexes are kept here, as queues of waiting threads per
// address, each thread waiting on an event of its own. Mutexes and
// condition variables are built on them, as is Rust's std, which calls
// syscall(SYS_futex).

#include "libc.h"
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#define DEFAULT_STACK (256u * 1024u)
#define MAIN_STACK_SIZE (256u * 1024u)
#define ROOT_END 0x7ffffffff000ull

// ---- thread-local storage ----

typedef struct {
    uint32_t p_type, p_flags;
    uint64_t p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align;
} phdr_t;

extern const char __ehdr_start[] __attribute__((weak));

static const void *tls_image;
// The TLS block sits tls_offset bytes below the thread pointer: the
// segment's size rounded up to its own alignment, as the linker assumes
// when it computes offsets (variant II). The control block is aligned to
// tcb_align (at least 16).
static size_t tls_filesz, tls_memsz, tls_align = 1, tls_offset, tcb_align = 16;
static int tls_ready;
static int errno_early;
static struct __pthread main_thread_fallback;
static char main_tls_area[16384] __attribute__((aligned(64)));

int *__errno_location(void) {
    return tls_ready ? &__self()->err : &errno_early;
}

const phdr_t *__program_headers(size_t *n) {
    if (!__ehdr_start) { *n = 0; return 0; }
    uint64_t phoff;
    uint16_t phnum;
    memcpy(&phoff, __ehdr_start + 32, 8);
    memcpy(&phnum, __ehdr_start + 56, 2);
    *n = phnum;
    return (const phdr_t *)(__ehdr_start + phoff);
}

static size_t align_up(size_t v, size_t a) { return (v + a - 1) & ~(a - 1); }

// Set up a TLS block and control block at `tcb` (aligned to tcb_align)
static struct __pthread *init_block(char *tcb) {
    char *block = tcb - tls_offset;
    memset(block, 0, tls_offset);
    if (tls_filesz) memcpy(block, tls_image, tls_filesz);
    struct __pthread *t = (struct __pthread *)tcb;
    memset(t, 0, sizeof *t);
    t->self = t;
    return t;
}

static int set_thread_pointer(rx_handle_t thread, struct __pthread *t) {
    uint64_t v = (uint64_t)t;
    return rx_object_set_property(thread, RX_PROP_REGISTER_FS, &v, sizeof v);
}

// Space for TLS and the control block at the top of a mapping
static size_t tls_space(void) { return tls_offset + sizeof(struct __pthread) + tcb_align; }

static void *map_pages(uint64_t size) {
    rx_handle_t vmo;
    uint64_t addr = 0;
    if (!rx_vmar_root || rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    rx_status_t r = rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr);
    rx_handle_close(vmo);
    return r == RX_OK ? (void *)addr : 0;
}

void __init_tls(void) {
    size_t n;
    const phdr_t *ph = __program_headers(&n);
    for (size_t i = 0; i < n; i++) {
        if (ph[i].p_type == 7) { // PT_TLS
            tls_image = (const void *)ph[i].p_vaddr;
            tls_filesz = ph[i].p_filesz;
            tls_memsz = ph[i].p_memsz;
            tls_align = ph[i].p_align ? ph[i].p_align : 1;
        }
    }
    tls_offset = align_up(tls_memsz, tls_align);
    if (tls_align > tcb_align) tcb_align = tls_align;
    char *area = main_tls_area;
    size_t size = sizeof main_tls_area;
    if (tls_space() > size) {
        size = align_up(tls_space(), 4096);
        area = map_pages(size);
    }
    struct __pthread *t;
    if (area) {
        t = init_block((char *)align_up((size_t)area + tls_offset, tcb_align));
    } else {
        t = &main_thread_fallback; // no room for TLS: errno and keys still work
        t->self = t;
    }
    t->handle = rx_thread_self;
    t->stack = (char *)(ROOT_END - MAIN_STACK_SIZE);
    t->stack_size = MAIN_STACK_SIZE;
    strcpy(t->name, "main");
    if (set_thread_pointer(rx_thread_self, t) == RX_OK) {
        tls_ready = 1;
        t->err = errno_early;
    }
}

extern void (*__init_array_start[])(int, char **, char **) __attribute__((weak));
extern void (*__init_array_end[])(int, char **, char **) __attribute__((weak));

void __run_init_array(int argc, char **argv, char **envp) {
    size_t n = (size_t)(__init_array_end - __init_array_start);
    for (size_t i = 0; i < n; i++) __init_array_start[i](argc, argv, envp);
}

// ---- internal locks and futexes ----

void __lock(volatile int *l) {
    for (int spins = 0; __atomic_exchange_n(l, 1, __ATOMIC_ACQUIRE); spins++) {
        if (spins < 100) __builtin_ia32_pause();
        else rx_yield();
    }
}

void __unlock(volatile int *l) { __atomic_store_n(l, 0, __ATOMIC_RELEASE); }

struct waiter {
    volatile int *addr;
    rx_handle_t ev;
    int woken;
    struct waiter *next;
};

#define BUCKETS 64
static volatile int futex_lock;
static struct waiter *buckets[BUCKETS];

static struct waiter **bucket(volatile int *addr) { return &buckets[((uintptr_t)addr >> 2) % BUCKETS]; }

int __futex_wait(volatile int *addr, int val, uint64_t deadline) {
    struct __pthread *me = tls_ready ? __self() : 0;
    rx_handle_t ev = me ? me->wake : 0;
    if (!ev) {
        if (rx_event_create(0, &ev) != RX_OK) {
            rx_yield(); // no event: behave like a spurious wakeup
            return 0;
        }
        if (me) me->wake = ev;
    }
    struct waiter w = { addr, ev, 0, 0 };
    __lock(&futex_lock);
    if (__atomic_load_n(addr, __ATOMIC_SEQ_CST) != val) {
        __unlock(&futex_lock);
        if (!me) rx_handle_close(ev);
        return -EAGAIN;
    }
    rx_object_signal(ev, RX_EVENT_SIGNALED, 0);
    struct waiter **b = bucket(addr);
    w.next = *b;
    *b = &w;
    __unlock(&futex_lock);

    rx_object_wait_one(ev, RX_EVENT_SIGNALED, deadline, 0);

    __lock(&futex_lock);
    int woken = w.woken;
    if (!woken) { // timed out: take ourselves off the queue
        for (struct waiter **p = bucket(addr); *p; p = &(*p)->next) {
            if (*p == &w) { *p = w.next; break; }
        }
    }
    __unlock(&futex_lock);
    if (!me) rx_handle_close(ev);
    return woken ? 0 : -ETIMEDOUT;
}

int __futex_wake(volatile int *addr, int n) {
    int count = 0;
    __lock(&futex_lock);
    struct waiter **p = bucket(addr);
    // The queue is newest first; wake the oldest waiters first
    while (count < n) {
        struct waiter **oldest = 0;
        for (struct waiter **q = p; *q; q = &(*q)->next)
            if ((*q)->addr == addr) oldest = q;
        if (!oldest) break;
        struct waiter *w = *oldest;
        *oldest = w->next;
        w->woken = 1;
        rx_object_signal(w->ev, 0, RX_EVENT_SIGNALED);
        count++;
    }
    __unlock(&futex_lock);
    return count;
}

uint64_t __deadline_abs(int clock, const void *p) {
    if (!p) return RX_TIME_INFINITE;
    const struct timespec *ts = p;
    int64_t want = ts->tv_sec * 1000000000ll + ts->tv_nsec;
    int64_t mono = (int64_t)rx_clock_get_monotonic();
    if (clock == CLOCK_REALTIME) want = mono + (want - rx_clock_get_utc());
    return want < 0 ? 0 : (uint64_t)want;
}

static uint64_t deadline_rel(const struct timespec *ts) {
    if (!ts) return RX_TIME_INFINITE;
    return rx_clock_get_monotonic() + (uint64_t)ts->tv_sec * 1000000000ull + (uint64_t)ts->tv_nsec;
}

// ---- syscall(): the Linux calls Rust's std makes directly ----

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_WAIT_BITSET 9
#define FUTEX_WAKE_BITSET 10
#define FUTEX_PRIVATE 128
#define FUTEX_CLOCK_REALTIME 256

ssize_t getrandom(void *buf, size_t len, unsigned flags);

long syscall(long number, ...) {
    va_list ap;
    va_start(ap, number);
    long a = va_arg(ap, long), b = va_arg(ap, long), c = va_arg(ap, long), d = va_arg(ap, long);
    va_end(ap);
    switch (number) {
    case SYS_futex: {
        volatile int *addr = (volatile int *)a;
        int op = (int)b & ~(FUTEX_PRIVATE | FUTEX_CLOCK_REALTIME);
        int r;
        switch (op) {
        case FUTEX_WAIT:
            r = __futex_wait(addr, (int)c, deadline_rel((const struct timespec *)d));
            break;
        case FUTEX_WAIT_BITSET:
            r = __futex_wait(addr, (int)c, __deadline_abs((b & FUTEX_CLOCK_REALTIME) ? CLOCK_REALTIME : CLOCK_MONOTONIC,
                                                          (const void *)d));
            break;
        case FUTEX_WAKE:
        case FUTEX_WAKE_BITSET:
            return __futex_wake(addr, (int)c);
        default:
            errno = ENOSYS;
            return -1;
        }
        if (r < 0) { errno = -r; return -1; }
        return 0;
    }
    case SYS_getrandom:
        return getrandom((void *)a, (size_t)b, (unsigned)c);
    case SYS_gettid:
        return (long)gettid();
    default:
        errno = ENOSYS;
        return -1;
    }
}

pid_t gettid(void) {
    struct __pthread *t = tls_ready ? __self() : 0;
    if (t && t->tid) return (pid_t)t->tid;
    rx_info_handle_basic_t b;
    uint64_t koid = 0;
    if (rx_object_get_info(t && t->handle ? t->handle : rx_thread_self, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0) == RX_OK)
        koid = b.koid;
    if (t) t->tid = koid;
    return (pid_t)koid;
}

int sched_yield(void) {
    rx_yield();
    return 0;
}

// ---- threads ----

static volatile int list_lock;
static struct __pthread *ended; // detached threads that have ended (their memory is freed later)

// Free the memory of detached threads whose kernel threads have ended
static void reap(void) {
    __lock(&list_lock);
    struct __pthread **p = &ended;
    while (*p) {
        struct __pthread *t = *p;
        if (rx_object_wait_one(t->handle, RX_THREAD_TERMINATED, 0, 0) == RX_OK) {
            *p = t->next;
            rx_handle_close(t->handle);
            if (t->wake) rx_handle_close(t->wake);
            rx_vmar_unmap(rx_vmar_root, (uint64_t)t->map, t->map_size);
        } else {
            p = &t->next;
        }
    }
    __unlock(&list_lock);
}

static void run_destructors(struct __pthread *t);

void pthread_exit(void *result) {
    struct __pthread *t = __self();
    t->result = result;
    run_destructors(t);
    if (!t->map) rx_thread_exit(); // the main thread: the program goes on while others run
    // Detached: queue ourselves to be freed once the kernel thread is gone
    if (__atomic_exchange_n(&t->detach_state, 2, __ATOMIC_ACQ_REL) == 1) {
        __lock(&list_lock);
        t->next = ended;
        ended = t;
        __unlock(&list_lock);
    }
    rx_thread_exit();
    __builtin_unreachable();
}

static void start(struct __pthread *t) __attribute__((noreturn, used, noinline));
static void start(struct __pthread *t) {
    pthread_exit(t->fn(t->arg));
}

// The kernel starts a thread with RSP 16-byte aligned; set up a frame as if
// start() had been called
__asm__(".text\n"
        "__thread_entry:\n"
        "    xor %ebp, %ebp\n"
        "    and $-16, %rsp\n"
        "    call start\n"
        "    ud2\n");
void __thread_entry(void);

int pthread_attr_init(pthread_attr_t *a) {
    memset(a, 0, sizeof *a);
    a->__s[0] = DEFAULT_STACK;
    return 0;
}
int pthread_attr_destroy(pthread_attr_t *a) { (void)a; return 0; }
int pthread_attr_setstacksize(pthread_attr_t *a, size_t size) {
    if (size < PTHREAD_STACK_MIN) return EINVAL;
    a->__s[0] = size;
    return 0;
}
int pthread_attr_getstacksize(const pthread_attr_t *restrict a, size_t *restrict size) {
    *size = a->__s[0] ? a->__s[0] : DEFAULT_STACK;
    return 0;
}
int pthread_attr_getstack(const pthread_attr_t *restrict a, void **restrict addr, size_t *restrict size) {
    *addr = (void *)a->__s[2];
    *size = a->__s[0];
    return 0;
}
int pthread_attr_setdetachstate(pthread_attr_t *a, int state) { a->__s[1] = (unsigned long)state; return 0; }
int pthread_attr_getdetachstate(const pthread_attr_t *a, int *state) { *state = (int)a->__s[1]; return 0; }
int pthread_attr_getguardsize(const pthread_attr_t *restrict a, size_t *restrict size) { (void)a; *size = 0; return 0; }
int pthread_attr_setguardsize(pthread_attr_t *a, size_t size) { (void)a; (void)size; return 0; }

int pthread_create(pthread_t *restrict out, const pthread_attr_t *restrict attr, void *(*fn)(void *), void *restrict arg) {
    reap();
    size_t stack = attr && attr->__s[0] ? attr->__s[0] : DEFAULT_STACK;
    stack = align_up(stack, 4096);
    size_t size = align_up(stack + tls_space(), 4096);
    char *map = map_pages(size);
    if (!map) return EAGAIN;
    struct __pthread *t = init_block((char *)align_up((size_t)map + stack + tls_offset, tcb_align));
    t->map = map;
    t->map_size = size;
    t->stack = map;
    t->stack_size = stack;
    t->fn = fn;
    t->arg = arg;
    t->detach_state = attr && attr->__s[1] == PTHREAD_CREATE_DETACHED;
    strcpy(t->name, "thread");
    rx_handle_t h;
    rx_status_t r = rx_thread_create(rx_proc_self, "thread", 6, 0, &h);
    if (r == RX_OK) {
        t->handle = h;
        r = set_thread_pointer(h, t);
        if (r == RX_OK) r = rx_thread_start(h, (uint64_t)__thread_entry, (uint64_t)(map + stack), (uint64_t)t, 0);
        if (r != RX_OK) rx_handle_close(h);
    }
    if (r != RX_OK) {
        rx_vmar_unmap(rx_vmar_root, (uint64_t)map, size);
        return EAGAIN;
    }
    *out = (pthread_t)t;
    return 0;
}

int pthread_join(pthread_t th, void **result) {
    struct __pthread *t = (struct __pthread *)th;
    if (t == __self()) return EDEADLK;
    if (t->detach_state == 1 || !t->map) return EINVAL;
    rx_object_wait_one(t->handle, RX_THREAD_TERMINATED, RX_TIME_INFINITE, 0);
    if (result) *result = t->result;
    rx_handle_close(t->handle);
    if (t->wake) rx_handle_close(t->wake);
    rx_vmar_unmap(rx_vmar_root, (uint64_t)t->map, t->map_size);
    return 0;
}

int pthread_detach(pthread_t th) {
    struct __pthread *t = (struct __pthread *)th;
    if (!t->map) return 0;
    // Still running: it frees itself when it ends. Already ended: free it now.
    if (__atomic_exchange_n(&t->detach_state, 1, __ATOMIC_ACQ_REL) == 2) return pthread_join(th, 0);
    return 0;
}

pthread_t pthread_self(void) { return (pthread_t)__self(); }
int pthread_equal(pthread_t a, pthread_t b) { return a == b; }

int pthread_setname_np(pthread_t th, const char *name) {
    struct __pthread *t = (struct __pthread *)th;
    if (strlen(name) >= sizeof t->name) return ERANGE;
    strcpy(t->name, name);
    return 0;
}

int pthread_getattr_np(pthread_t th, pthread_attr_t *a) {
    struct __pthread *t = (struct __pthread *)th;
    pthread_attr_init(a);
    a->__s[0] = t->stack_size;
    a->__s[1] = t->detach_state == 1;
    a->__s[2] = (unsigned long)t->stack;
    return 0;
}

// ---- thread-specific data ----

static volatile int keys_lock;
static void (*key_dtor[RX_KEYS_MAX])(void *);
static unsigned char key_used[RX_KEYS_MAX];

int pthread_key_create(pthread_key_t *key, void (*dtor)(void *)) {
    __lock(&keys_lock);
    for (unsigned i = 0; i < RX_KEYS_MAX; i++) {
        if (!key_used[i]) {
            key_used[i] = 1;
            key_dtor[i] = dtor;
            __unlock(&keys_lock);
            *key = i;
            return 0;
        }
    }
    __unlock(&keys_lock);
    return EAGAIN;
}

int pthread_key_delete(pthread_key_t key) {
    if (key >= RX_KEYS_MAX) return EINVAL;
    __lock(&keys_lock);
    key_used[key] = 0;
    key_dtor[key] = 0;
    __unlock(&keys_lock);
    return 0;
}

void *pthread_getspecific(pthread_key_t key) {
    return key < RX_KEYS_MAX ? (void *)__self()->tsd[key] : 0;
}

int pthread_setspecific(pthread_key_t key, const void *value) {
    if (key >= RX_KEYS_MAX) return EINVAL;
    __self()->tsd[key] = value;
    return 0;
}

static void run_destructors(struct __pthread *t) {
    for (int round = 0; round < 4; round++) {
        int any = 0;
        for (unsigned i = 0; i < RX_KEYS_MAX; i++) {
            void (*d)(void *) = key_dtor[i];
            void *v = (void *)t->tsd[i];
            if (d && v) {
                t->tsd[i] = 0;
                d(v);
                any = 1;
            }
        }
        if (!any) break;
    }
}

void __thread_exit_main(void) { if (tls_ready) run_destructors(__self()); }

int pthread_once(pthread_once_t *once, void (*fn)(void)) {
    volatile int *s = (volatile int *)once; // 0 new, 1 running, 2 done
    for (;;) {
        int v = __atomic_load_n(s, __ATOMIC_ACQUIRE);
        if (v == 2) return 0;
        if (v == 0 && __atomic_compare_exchange_n(s, &v, 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
            fn();
            __atomic_store_n(s, 2, __ATOMIC_RELEASE);
            __futex_wake(s, 0x7fffffff);
            return 0;
        }
        if (v == 1) __futex_wait(s, 1, RX_TIME_INFINITE);
    }
}

// ---- mutexes: __i[0] lock word (0 free, 1 locked, 2 locked with
// waiters), __i[1] type, __i[2] owner (low bits of the TCB address),
// __i[3] recursion count ----

int pthread_mutexattr_init(pthread_mutexattr_t *a) { a->__attr = 0; return 0; }
int pthread_mutexattr_destroy(pthread_mutexattr_t *a) { (void)a; return 0; }
int pthread_mutexattr_settype(pthread_mutexattr_t *a, int type) {
    if (type < 0 || type > 2) return EINVAL;
    a->__attr = (unsigned)type;
    return 0;
}

int pthread_mutex_init(pthread_mutex_t *restrict m, const pthread_mutexattr_t *restrict a) {
    memset(m, 0, sizeof *m);
    m->__i[1] = a ? (int)a->__attr : 0;
    return 0;
}
int pthread_mutex_destroy(pthread_mutex_t *m) { (void)m; return 0; }

static int owner_id(void) { return (int)(uintptr_t)__self() | 1; }

int pthread_mutex_trylock(pthread_mutex_t *m) {
    volatile int *w = &m->__i[0];
    if (m->__i[1] && m->__i[2] == owner_id()) {
        if (m->__i[1] == PTHREAD_MUTEX_ERRORCHECK) return EDEADLK;
        m->__i[3]++;
        return 0;
    }
    int z = 0;
    if (!__atomic_compare_exchange_n(w, &z, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return EBUSY;
    if (m->__i[1]) { m->__i[2] = owner_id(); m->__i[3] = 1; }
    return 0;
}

int pthread_mutex_lock(pthread_mutex_t *m) {
    volatile int *w = &m->__i[0];
    if (m->__i[1] && m->__i[2] == owner_id()) {
        if (m->__i[1] == PTHREAD_MUTEX_ERRORCHECK) return EDEADLK;
        m->__i[3]++;
        return 0;
    }
    int z = 0;
    if (!__atomic_compare_exchange_n(w, &z, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        for (int i = 0; i < 100 && __atomic_load_n(w, __ATOMIC_RELAXED); i++) __builtin_ia32_pause();
        while (__atomic_exchange_n(w, 2, __ATOMIC_ACQUIRE) != 0) __futex_wait(w, 2, RX_TIME_INFINITE);
    }
    if (m->__i[1]) { m->__i[2] = owner_id(); m->__i[3] = 1; }
    return 0;
}

int pthread_mutex_unlock(pthread_mutex_t *m) {
    volatile int *w = &m->__i[0];
    if (m->__i[1]) {
        if (m->__i[2] != owner_id()) return EPERM;
        if (--m->__i[3] > 0) return 0;
        m->__i[2] = 0;
    }
    if (__atomic_exchange_n(w, 0, __ATOMIC_RELEASE) == 2) __futex_wake(w, 1);
    return 0;
}

// ---- condition variables: __i[0] sequence, __i[1] clock ----

int pthread_condattr_init(pthread_condattr_t *a) { a->__attr = CLOCK_REALTIME; return 0; }
int pthread_condattr_destroy(pthread_condattr_t *a) { (void)a; return 0; }
int pthread_condattr_setclock(pthread_condattr_t *a, clockid_t clock) { a->__attr = (unsigned)clock; return 0; }

int pthread_cond_init(pthread_cond_t *restrict c, const pthread_condattr_t *restrict a) {
    memset(c, 0, sizeof *c);
    c->__i[1] = a ? (int)a->__attr : CLOCK_REALTIME;
    return 0;
}
int pthread_cond_destroy(pthread_cond_t *c) { (void)c; return 0; }

static int cond_wait(pthread_cond_t *c, pthread_mutex_t *m, uint64_t deadline) {
    volatile int *seq = &c->__i[0];
    int s = __atomic_load_n(seq, __ATOMIC_ACQUIRE);
    // Keep the recursion state of a recursive mutex across the wait
    int count = m->__i[3];
    if (m->__i[1]) m->__i[3] = 1;
    pthread_mutex_unlock(m);
    int r = __futex_wait(seq, s, deadline);
    // Relock, marking the mutex contended (others may have been woken too)
    volatile int *w = &m->__i[0];
    while (__atomic_exchange_n(w, 2, __ATOMIC_ACQUIRE) != 0) __futex_wait(w, 2, RX_TIME_INFINITE);
    if (m->__i[1]) { m->__i[2] = owner_id(); m->__i[3] = count; }
    return r == -ETIMEDOUT ? ETIMEDOUT : 0;
}

int pthread_cond_wait(pthread_cond_t *restrict c, pthread_mutex_t *restrict m) {
    return cond_wait(c, m, RX_TIME_INFINITE);
}

int pthread_cond_timedwait(pthread_cond_t *restrict c, pthread_mutex_t *restrict m, const struct timespec *restrict t) {
    return cond_wait(c, m, __deadline_abs(c->__i[1], t));
}

int pthread_cond_signal(pthread_cond_t *c) {
    __atomic_fetch_add(&c->__i[0], 1, __ATOMIC_RELEASE);
    __futex_wake(&c->__i[0], 1);
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t *c) {
    __atomic_fetch_add(&c->__i[0], 1, __ATOMIC_RELEASE);
    __futex_wake(&c->__i[0], 0x7fffffff);
    return 0;
}

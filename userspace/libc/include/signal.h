// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Signals: Rustux has none. These calls exist so programs written for
// Unix build and run; handlers are recorded but never called.

#ifndef _SIGNAL_H
#define _SIGNAL_H

#include <sys/types.h>

typedef struct { unsigned long __bits[128 / sizeof(long)]; } sigset_t;
typedef void (*sighandler_t)(int);
typedef int sig_atomic_t;

#define SIG_DFL ((sighandler_t)0)
#define SIG_IGN ((sighandler_t)1)
#define SIG_ERR ((sighandler_t)-1)

#define SIGHUP 1
#define SIGINT 2
#define SIGQUIT 3
#define SIGILL 4
#define SIGTRAP 5
#define SIGABRT 6
#define SIGBUS 7
#define SIGFPE 8
#define SIGKILL 9
#define SIGUSR1 10
#define SIGSEGV 11
#define SIGUSR2 12
#define SIGPIPE 13
#define SIGALRM 14
#define SIGTERM 15
#define SIGCHLD 17
#define SIGCONT 18
#define SIGSTOP 19
#define SIGTSTP 20
#define SIGWINCH 28
#define NSIG 65

#define SA_NOCLDSTOP 1
#define SA_SIGINFO 4
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000

#define SIG_BLOCK 0
#define SIG_UNBLOCK 1
#define SIG_SETMASK 2

#define SS_ONSTACK 1
#define SS_DISABLE 2
#define MINSIGSTKSZ 2048
#define SIGSTKSZ 8192

typedef struct { void *ss_sp; int ss_flags; size_t ss_size; } stack_t;

struct sigaction {
    union { sighandler_t sa_handler; void (*sa_sigaction)(int, void *, void *); };
    sigset_t sa_mask;
    int sa_flags;
    void (*sa_restorer)(void);
};

sighandler_t signal(int sig, sighandler_t handler);
int sigaction(int sig, const struct sigaction *restrict act, struct sigaction *restrict old);
int sigaltstack(const stack_t *restrict ss, stack_t *restrict old);
int sigemptyset(sigset_t *s);
int sigfillset(sigset_t *s);
int sigaddset(sigset_t *s, int sig);
int sigdelset(sigset_t *s, int sig);
int sigismember(const sigset_t *s, int sig);
int sigprocmask(int how, const sigset_t *restrict set, sigset_t *restrict old);
int pthread_sigmask(int how, const sigset_t *restrict set, sigset_t *restrict old);
int kill(pid_t pid, int sig);
int raise(int sig);

#endif

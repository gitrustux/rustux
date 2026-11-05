// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_WAIT_H
#define _SYS_WAIT_H

#include <sys/types.h>

#define WNOHANG 1
#define WUNTRACED 2
#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WIFEXITED(s) (!WTERMSIG(s))
#define WIFSIGNALED(s) (((s) & 0xffff) - 1U < 0xffu)
#define WIFSTOPPED(s) 0
#define WCOREDUMP(s) 0

// Children started with posix_spawn. A Rustux exit code becomes an exit
// status (low 8 bits); a killed process (-1024) reads as SIGKILL, a
// crashed one (-1025) as SIGSEGV.
pid_t waitpid(pid_t pid, int *status, int options);
pid_t wait(int *status);

#endif

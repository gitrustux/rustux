// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#include <stddef.h>
#include <stdint.h>

typedef long ssize_t;
typedef long off_t;
typedef int pid_t;
typedef unsigned int uid_t;
typedef unsigned int gid_t;
typedef unsigned int mode_t;
typedef long time_t;
typedef unsigned long ino_t;
typedef unsigned long nlink_t;
typedef long blksize_t;
typedef long blkcnt_t;
typedef unsigned long dev_t;
typedef int clockid_t_;
typedef unsigned long pthread_t;
typedef unsigned int pthread_key_t;
typedef int pthread_once_t;
typedef union { int __i[14]; unsigned long __s[7]; } pthread_attr_t;
typedef union { int __i[10]; void *__p[5]; } pthread_mutex_t;
typedef struct { unsigned __attr; } pthread_mutexattr_t;
typedef union { int __i[12]; void *__p[6]; } pthread_cond_t;
typedef struct { unsigned __attr; } pthread_condattr_t;
typedef union { int __i[14]; void *__p[7]; } pthread_rwlock_t;
typedef struct { unsigned __attr[2]; } pthread_rwlockattr_t;

#endif

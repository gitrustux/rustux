// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// System and authentication services (step 17). Each request is one
// message on the connection a program opens to the service; the reply is
// one message: i32 status (0 or a negative errno), then the op's fields.
// The services decide by the request's sender (rx_channel_read_creds).
//
// /svc/system (init): what only root may do to the whole system.
//   RX_SYSTEM_OP_POWEROFF, RX_SYSTEM_OP_REBOOT: the sessions are ended, the
//     disks written out and marked clean (SHUTDOWN_ALL), then the power goes
//     off or the computer restarts (there is no reply then).
//   RX_SYSTEM_OP_SET_TIME: i64 ns since 1970 (UTC): the clock.
//   RX_SYSTEM_OP_RELOAD_KEYMAP: the console reads /etc/keymap again (anyone).
//
// /svc/auth (authd): running a program as another user (su, sudo).
//   RX_AUTH_OP_RUN: rx_auth_run_t, then NUL-terminated strings: the target
//     user, the working directory, argc arguments, envc environment entries;
//     handles: the caller's stdin, stdout and stderr (with their fd kinds in
//     rx_auth_run_t.kinds). authd asks for a password on those (the target's
//     for su, the caller's own for sudo, unless /etc/sudoers says NOPASSWD
//     or the caller gave it in the last 5 minutes), starts the program as
//     the target user, and replies when it ends: i64 its exit code. If the
//     caller's connection closes first, the program is killed.
//   RX_AUTH_OP_CHECK: rx_auth_run_t with RX_AUTH_SUDO, the target user (no
//     handles): 0 if the caller may use sudo for it at all (sudo -l).
//   RX_AUTH_OP_PASSWD: rx_auth_run_t (argc, envc 0), the user (empty: the
//     caller), the working directory; the caller's stdin, stdout, stderr.
//     authd asks for the current password (not for root), the new one
//     twice, and writes /etc/shadow.

#ifndef RUSTUX_AUTH_H
#define RUSTUX_AUTH_H

#include <stdint.h>

#define RX_SYSTEM_OP_POWEROFF      1u
#define RX_SYSTEM_OP_REBOOT        2u
#define RX_SYSTEM_OP_SET_TIME      3u
#define RX_SYSTEM_OP_RELOAD_KEYMAP 4u

#define RX_AUTH_OP_RUN   1u
#define RX_AUTH_OP_CHECK 2u
#define RX_AUTH_OP_PASSWD 3u

#define RX_AUTH_SUDO   1u   // sudo's rules (else su's)
#define RX_AUTH_LOGIN  2u   // a login shell: environment from the target user (su -, sudo -i)
#define RX_AUTH_STDIN  4u   // sudo -S: the password is the first line of stdin, no prompt
#define RX_AUTH_SHELL  8u   // sudo -s/-i: the target's shell (with the arguments as a command)

typedef struct {
    uint32_t op;
    uint32_t flags;      // RX_AUTH_*
    uint32_t argc, envc;
    uint32_t kinds[3];   // fd kinds of the handles (RX_FD_*), 0 for none
} rx_auth_run_t;

#endif

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Console protocol. A program's stdin, stdout and stderr are usually
// handles to the same channel, whose other end is the console service
// (init). Every message starts with a 32-bit op.
//
//   RX_CONSOLE_OP_WRITE  program -> console: bytes to print
//   RX_CONSOLE_OP_INPUT  console -> program: typed input; in the default
//                        (cooked) mode one message per line, ending in
//                        '\n'. A message with no bytes is end of input
//                        (Ctrl+D on an empty line).
//
// Ctrl+C clears the typed line and raises RX_CONSOLE_SIGNAL_INTERRUPT on
// the program side of the channel; the shell watches for it, ends the
// program in the foreground and clears the signal.

#ifndef RUSTUX_CONSOLE_H
#define RUSTUX_CONSOLE_H

#include <rustux/syscalls.h>

#define RX_CONSOLE_OP_WRITE 1u
#define RX_CONSOLE_OP_INPUT 2u

// Most bytes in one message after the op
#define RX_CONSOLE_MAX_DATA 4096u

#define RX_CONSOLE_SIGNAL_INTERRUPT RX_USER_SIGNAL_0

#endif

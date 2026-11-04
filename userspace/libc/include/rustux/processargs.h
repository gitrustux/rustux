// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The bootstrap message every program receives on the channel handle in
// the first argument of _start (Fuchsia's processargs format):
//
//   header | u32 info word per handle | args (NUL-terminated) | environment
//
// The handles travel with the message, in the same order as the info words.

#ifndef RUSTUX_PROCESSARGS_H
#define RUSTUX_PROCESSARGS_H

#include <stdint.h>

#define RX_PROCARGS_PROTOCOL 0x4150585du
#define RX_PROCARGS_VERSION  0x00010000u

typedef struct {
    uint32_t protocol;
    uint32_t version;
    uint32_t handle_info_off;
    uint32_t args_off;
    uint32_t args_num;
    uint32_t environ_off;
    uint32_t environ_num;
    uint32_t names_off;   // unused (namespace names come in step 8)
    uint32_t names_num;
} rx_proc_args_t;

#endif

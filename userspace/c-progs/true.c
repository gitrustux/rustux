// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// true: exit at once with code 0

#include "rx.h"

void _start(rx_handle_t bootstrap) {
    (void)bootstrap;
    rx_exit(0);
}

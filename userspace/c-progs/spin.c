// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// spin: loop forever without reading input (a test for Ctrl+C)

#include <stdio.h>

int main(void) {
    printf("spin: looping forever; press Ctrl+C to stop me\n");
    for (volatile unsigned long i = 0;; i++) {}
}

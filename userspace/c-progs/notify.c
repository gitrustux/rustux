// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// notify TEXT...: a notification on the desktop (DESKTOP.md step F6): the
// compositor shows it at the top right for 5 seconds.

#include <rustux/window.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: notify TEXT...\n");
        return 2;
    }
    char text[256] = "";
    for (int i = 1; i < argc; i++) {
        if (i > 1) strncat(text, " ", sizeof text - strlen(text) - 1);
        strncat(text, argv[i], sizeof text - strlen(text) - 1);
    }
    int r = rx_window_notify(0, text);
    if (r < 0) {
        fprintf(stderr, "notify: no desktop: %s\n", strerror(-r));
        return 1;
    }
    return 0;
}

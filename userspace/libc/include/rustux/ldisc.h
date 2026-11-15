// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The line discipline (DESKTOP.md step F7): what a terminal does between
// the keys typed and the program reading them, on the console side of a
// console channel (<rustux/console.h>). The text console (init) and every
// pseudo-terminal (posix_openpt, the graphical Terminal) share it.
//
// - Cooked mode (ICANON, the default): the line is edited (echo,
//   Backspace, Ctrl+U, Ctrl+W) and sent when Enter is pressed; Ctrl+D
//   sends what was typed, or end of input on an empty line. Escape
//   sequences (arrows, function keys) are dropped.
// - Raw mode: every key goes out as it is typed.
// - ISIG: Ctrl+C clears the line and raises RX_CONSOLE_SIGNAL_INTERRUPT on
//   the program's end (the shell ends the program in the foreground).
// - ICRNL: a CR typed becomes LF.
// - Echo goes to the echo function, which shows it as the program's
//   output would be shown.

#ifndef RUSTUX_LDISC_H
#define RUSTUX_LDISC_H

#include <stddef.h>
#include <rustux/console.h>

typedef struct rx_ldisc {
    rx_handle_t ch;         // the console's end of the channel
    uint32_t lflag, iflag;  // termios bits: ISIG, ICANON, ECHO; ICRNL
    void (*echo)(struct rx_ldisc *ld, const char *s, size_t n);
    void *ctx;              // for the echo function
    char line[RX_CONSOLE_MAX_DATA - 96];
    size_t line_len;
    char pending[8];        // an escape or UTF-8 sequence cut by the end of a write
    size_t npending;
} rx_ldisc_t;

// Start in cooked mode with echo, ISIG and ICRNL
void rx_ldisc_init(rx_ldisc_t *ld, rx_handle_t ch, void (*echo)(rx_ldisc_t *, const char *, size_t), void *ctx);
// One key's bytes (a character, a control character or a whole escape
// sequence), as the text console's input service sends them
void rx_ldisc_key(rx_ldisc_t *ld, const char *s, size_t n);
// Bytes typed into a pseudo-terminal: split into keys
void rx_ldisc_input(rx_ldisc_t *ld, const char *s, size_t n);
// A message from the program's end: SET_MODE is taken here (0); WRITE
// returns 1 with *data and *len set to the bytes to show; anything else -1
int rx_ldisc_message(rx_ldisc_t *ld, const char *msg, uint32_t n, const char **data, size_t *len);
// Tell the program the terminal's size (columns, rows: TIOCGWINSZ)
int rx_tty_set_size(rx_handle_t ch, unsigned cols, unsigned rows);
// The size of the terminal behind a console channel: 0, or -1 if unknown
int rx_tty_get_size(rx_handle_t ch, unsigned *cols, unsigned *rows);

#endif

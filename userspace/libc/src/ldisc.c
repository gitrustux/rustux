// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The line discipline (<rustux/ldisc.h>), moved out of init in DESKTOP.md
// step F7 so the text console and pseudo-terminals share it.

#include <rustux/ldisc.h>
#include <string.h>

#define LF_ISIG   0000001u
#define LF_ICANON 0000002u
#define LF_ECHO   0000010u
#define IF_ICRNL  0000400u

void rx_ldisc_init(rx_ldisc_t *ld, rx_handle_t ch, void (*echo)(rx_ldisc_t *, const char *, size_t), void *ctx) {
    memset(ld, 0, sizeof *ld);
    ld->ch = ch;
    ld->lflag = LF_ISIG | LF_ICANON | LF_ECHO;
    ld->iflag = IF_ICRNL;
    ld->echo = echo;
    ld->ctx = ctx;
}

static void echo(rx_ldisc_t *ld, const char *s, size_t n) {
    if ((ld->lflag & LF_ECHO) && ld->echo) ld->echo(ld, s, n);
}

static void send_input(rx_ldisc_t *ld, const char *s, size_t n) {
    char msg[4 + RX_CONSOLE_MAX_DATA];
    uint32_t op = RX_CONSOLE_OP_INPUT;
    if (n > RX_CONSOLE_MAX_DATA) n = RX_CONSOLE_MAX_DATA;
    memcpy(msg, &op, 4);
    memcpy(msg + 4, s, n);
    rx_channel_write(ld->ch, 0, msg, (uint32_t)(4 + n), 0, 0); // dropped if nobody reads for a long time
}

// Remove the last character (a whole UTF-8 sequence) from the line
static void rubout(rx_ldisc_t *ld) {
    if (!ld->line_len) return;
    while (ld->line_len > 1 && (ld->line[ld->line_len - 1] & 0xC0) == 0x80) ld->line_len--;
    ld->line_len--;
    echo(ld, "\b \b", 3);
}

static void interrupt(rx_ldisc_t *ld) {
    echo(ld, "^C\n", 3);
    ld->line_len = 0;
    rx_object_signal_peer(ld->ch, 0, RX_CONSOLE_SIGNAL_INTERRUPT);
}

// Cooked mode: edit the line, send it on Enter
static void cooked(rx_ldisc_t *ld, const char *s, size_t n) {
    if (n > 1 && s[0] == 0x1B) return; // arrows, function keys: no line editing here
    unsigned char c = (unsigned char)s[0];
    if (n > 1 || c >= 0x80) { // a UTF-8 character
        if (ld->line_len + n < sizeof ld->line - 1) {
            memcpy(ld->line + ld->line_len, s, n);
            ld->line_len += n;
            echo(ld, s, n);
        }
        return;
    }
    switch (c) {
    case '\r':
    case '\n':
        ld->line[ld->line_len++] = '\n';
        echo(ld, "\n", 1);
        send_input(ld, ld->line, ld->line_len);
        ld->line_len = 0;
        break;
    case 0x08: // Backspace
    case 0x7F:
        rubout(ld);
        break;
    case 0x15: // Ctrl+U: erase the line
        while (ld->line_len) rubout(ld);
        break;
    case 0x17: // Ctrl+W: erase a word
        while (ld->line_len && ld->line[ld->line_len - 1] == ' ') rubout(ld);
        while (ld->line_len && ld->line[ld->line_len - 1] != ' ') rubout(ld);
        break;
    case 0x03: // Ctrl+C
        if (ld->lflag & LF_ISIG) interrupt(ld);
        break;
    case 0x04: // Ctrl+D: end of input on an empty line, else send the line as is
        send_input(ld, ld->line, ld->line_len);
        ld->line_len = 0;
        break;
    default:
        if ((c >= 0x20 || c == '\t') && ld->line_len < sizeof ld->line - 1) {
            ld->line[ld->line_len++] = (char)c;
            echo(ld, (char *)&c, 1);
        }
        break;
    }
}

void rx_ldisc_key(rx_ldisc_t *ld, const char *key, size_t n) {
    char s[16];
    if (!n) return;
    if (n > sizeof s) n = sizeof s;
    memcpy(s, key, n);
    if (n == 1 && s[0] == '\r' && (ld->iflag & IF_ICRNL)) s[0] = '\n';
    if (!(ld->lflag & LF_ICANON)) {
        if ((ld->lflag & LF_ISIG) && n == 1 && s[0] == 0x03) {
            interrupt(ld);
            return;
        }
        echo(ld, s, n);
        send_input(ld, s, n);
        return;
    }
    cooked(ld, s, n);
}

// The length of the key starting at s (at most n bytes), or 0 if it is
// cut off by the end
static size_t key_len(const unsigned char *s, size_t n) {
    if (s[0] == 0x1B) {
        if (n == 1) return 1; // Esc on its own
        if (s[1] == '[') {    // CSI: parameters, then a final byte
            for (size_t i = 2; i < n; i++)
                if (s[i] >= 0x40 && s[i] <= 0x7E) return i + 1;
            return 0;
        }
        if (s[1] == 'O') return n >= 3 ? 3 : 0; // SS3 (F1-F4, keypad)
        return 2;                               // Alt+key
    }
    size_t k = s[0] >= 0xF0 ? 4 : s[0] >= 0xE0 ? 3 : s[0] >= 0xC0 ? 2 : 1;
    return k <= n ? k : 0;
}

void rx_ldisc_input(rx_ldisc_t *ld, const char *in, size_t n) {
    // A sequence left over from the last write comes first
    while (ld->npending && n) {
        ld->pending[ld->npending++] = *in++;
        n--;
        size_t k = key_len((unsigned char *)ld->pending, ld->npending);
        if (k || ld->npending == sizeof ld->pending) {
            if (!k) k = ld->npending;
            rx_ldisc_key(ld, ld->pending, k);
            // Bytes past the key (an Esc followed by more) go round again
            size_t rest = ld->npending - k;
            char tmp[sizeof ld->pending];
            memcpy(tmp, ld->pending + k, rest);
            ld->npending = 0;
            if (rest) rx_ldisc_input(ld, tmp, rest);
        }
    }
    while (n) {
        size_t k = key_len((const unsigned char *)in, n);
        if (!k) { // cut off: wait for the rest
            memcpy(ld->pending, in, n);
            ld->npending = n;
            return;
        }
        rx_ldisc_key(ld, in, k);
        in += k;
        n -= k;
    }
}

int rx_ldisc_message(rx_ldisc_t *ld, const char *msg, uint32_t n, const char **data, size_t *len) {
    uint32_t op = 0;
    if (n >= 4) memcpy(&op, msg, 4);
    if (op == RX_CONSOLE_OP_WRITE) {
        *data = msg + 4;
        *len = n - 4;
        return 1;
    }
    if (op == RX_CONSOLE_OP_SET_MODE && n >= 12) {
        uint32_t was = ld->lflag;
        memcpy(&ld->lflag, msg + 4, 4);
        memcpy(&ld->iflag, msg + 8, 4);
        // Switching to raw mode hands over what was typed so far
        if ((was & LF_ICANON) && !(ld->lflag & LF_ICANON) && ld->line_len) {
            send_input(ld, ld->line, ld->line_len);
            ld->line_len = 0;
        }
        return 0;
    }
    return -1;
}

int rx_tty_set_size(rx_handle_t ch, unsigned cols, unsigned rows) {
    uint16_t v[2] = { (uint16_t)cols, (uint16_t)rows };
    return rx_object_set_property(ch, RX_PROP_TTY_SIZE, v, sizeof v) == RX_OK ? 0 : -1;
}

int rx_tty_get_size(rx_handle_t ch, unsigned *cols, unsigned *rows) {
    uint16_t v[2] = { 0, 0 };
    if (rx_object_get_property(ch, RX_PROP_TTY_SIZE, v, sizeof v) != RX_OK || !v[0] || !v[1]) return -1;
    *cols = v[0];
    *rows = v[1];
    return 0;
}

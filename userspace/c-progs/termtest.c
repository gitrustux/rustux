// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// termtest: a full-screen program (checklist step 10). Switches to the
// alternate screen, draws with escape sequences (box drawing, colours,
// cursor movement) and reads keys in raw mode: the arrow keys move the @,
// every key is shown as the bytes it sent and a name. q leaves (the
// screen comes back as it was).

#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static int rows = 25, cols = 80, x, y;

static void at(int r, int c) { printf("\033[%d;%dH", r, c); }

static void frame(void) {
    printf("\033[0m\033[2J");
    // Border
    at(1, 1);
    printf("\033[36m┌");
    for (int i = 2; i < cols; i++) printf("─");
    printf("┐");
    for (int r = 2; r < rows; r++) {
        at(r, 1);
        printf("│");
        at(r, cols);
        printf("│");
    }
    at(rows, 1);
    printf("└");
    for (int i = 2; i < cols; i++) printf("─");
    printf("┘\033[0m");
    at(1, 3);
    printf("\033[1;33m termtest \033[0m");
    at(2, 3);
    printf("screen %d x %d   arrows move the @, q leaves", cols, rows);
    // Colours: 16 and a 256-colour ramp
    at(4, 3);
    for (int i = 0; i < 16; i++) printf("\033[%d;%dm %2d \033[0m", i < 8 ? 30 : 90, i < 8 ? 40 + i : 100 + i - 8, i);
    at(5, 3);
    for (int i = 0; i < 24 && 3 + i * 3 < cols - 3; i++) printf("\033[48;5;%dm   ", 232 + i);
    printf("\033[0m");
    at(6, 3);
    printf("\033[1mbold\033[0m \033[4munderline\033[0m \033[7mreverse\033[0m \033[38;2;255;128;0mtrue colour\033[0m "
           "UTF-8: äöü ß é ✓ • → ░▒▓█");
    fflush(stdout);
}

static void draw_at(int show) {
    at(y, x);
    printf(show ? "\033[1;32m@\033[0m" : " ");
}

static const char *name_of(const unsigned char *b, int n) {
    static char buf[32];
    if (n == 1) {
        if (b[0] == 0x1B) return "Esc";
        if (b[0] == '\r') return "Enter";
        if (b[0] == '\t') return "Tab";
        if (b[0] == 0x7F) return "Backspace";
        if (b[0] < 0x20) { snprintf(buf, sizeof buf, "Ctrl+%c", b[0] + '@'); return buf; }
        snprintf(buf, sizeof buf, "'%c'", b[0]);
        return buf;
    }
    if (n >= 3 && b[0] == 0x1B && (b[1] == '[' || b[1] == 'O')) {
        static const struct { const char *seq, *name; } k[] = {
            { "[A", "Up" }, { "[B", "Down" }, { "[C", "Right" }, { "[D", "Left" }, { "[H", "Home" }, { "[F", "End" },
            { "[2~", "Insert" }, { "[3~", "Delete" }, { "[5~", "PgUp" }, { "[6~", "PgDn" }, { "[Z", "Shift+Tab" },
            { "OP", "F1" }, { "OQ", "F2" }, { "OR", "F3" }, { "OS", "F4" }, { "[15~", "F5" }, { "[17~", "F6" },
            { "[18~", "F7" }, { "[19~", "F8" }, { "[20~", "F9" }, { "[21~", "F10" }, { "[23~", "F11" }, { "[24~", "F12" },
            { "[1;5A", "Ctrl+Up" }, { "[1;5B", "Ctrl+Down" }, { "[1;5C", "Ctrl+Right" }, { "[1;5D", "Ctrl+Left" },
            { "[1;2A", "Shift+Up" }, { "[1;2B", "Shift+Down" }, { "[1;3A", "Alt+Up" },
        };
        for (size_t i = 0; i < sizeof k / sizeof *k; i++)
            if (strlen(k[i].seq) == (size_t)n - 1 && !memcmp(k[i].seq, b + 1, (size_t)n - 1)) return k[i].name;
        return "(sequence)";
    }
    if (n == 2 && b[0] == 0x1B) { snprintf(buf, sizeof buf, "Alt+%c", b[1]); return buf; }
    return "(text)";
}

int main(void) {
    struct termios old, raw;
    if (tcgetattr(0, &old) < 0) {
        fprintf(stderr, "termtest: not a terminal\n");
        return 1;
    }
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_col) {
        cols = ws.ws_col;
        rows = ws.ws_row;
    }
    raw = old;
    cfmakeraw(&raw);
    tcsetattr(0, TCSANOW, &raw);
    printf("\033[?1049h\033[?25l"); // alternate screen, no cursor
    frame();
    x = cols / 2;
    y = rows / 2;
    draw_at(1);
    int keys = 0;
    for (;;) {
        fflush(stdout);
        unsigned char b[32];
        ssize_t n = read(0, b, sizeof b);
        if (n <= 0) break;
        if (n == 1 && b[0] == 'q') break;
        keys++;
        draw_at(0);
        if (n == 3 && b[0] == 0x1B && b[1] == '[') {
            if (b[2] == 'A' && y > 9) y--;
            if (b[2] == 'B' && y < rows - 1) y++;
            if (b[2] == 'C' && x < cols - 1) x++;
            if (b[2] == 'D' && x > 2) x--;
        }
        draw_at(1);
        at(8, 3);
        printf("\033[Kkey %d:", keys);
        for (ssize_t i = 0; i < n; i++) printf(" %02x", b[i]);
        printf("  %s", name_of(b, (int)n));
        at(8, cols);
        printf("\033[36m│\033[0m");
    }
    printf("\033[0m\033[?25h\033[?1049l"); // back to the normal screen
    fflush(stdout);
    tcsetattr(0, TCSANOW, &old);
    printf("termtest: %d keys\n", keys);
    return 0;
}

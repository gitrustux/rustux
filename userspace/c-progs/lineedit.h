// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Line editing for the shell (checklist step 10). The console is put in
// raw mode while a line is typed; keys arrive as bytes and escape
// sequences (xterm style) and the line is redrawn with escape sequences.
//
//   Left/Right, Ctrl+B/Ctrl+F    move a character
//   Ctrl+Left/Right, Alt+B/F     move a word
//   Home/End, Ctrl+A/Ctrl+E      start / end of the line
//   Backspace, Delete, Ctrl+D    delete before / at the cursor (Ctrl+D on
//                                an empty line: end of input)
//   Ctrl+W, Alt+Backspace        delete the word before the cursor
//   Ctrl+U / Ctrl+K              delete to the start / to the end
//   Up/Down, Ctrl+P/Ctrl+N       history (the last 200 lines)
//   Tab                          complete a command (first word) or a path;
//                                a second Tab lists the choices
//   Ctrl+L                       clear the screen
//   Ctrl+C                       abandon the line
//
// UTF-8 is kept whole (the cursor moves by characters); every character
// is taken to be one column wide.

#ifndef LINEEDIT_H
#define LINEEDIT_H

#include <dirent.h>
#include <rustux/console.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

#define LE_HISTORY 200

struct le {
    char *buf;
    size_t cap, len, cur;        // bytes
    const char *prompt;
    size_t prompt_cols;          // visible width of the prompt
    size_t cols;                 // screen width
    size_t shown;                // cursor position (characters from the line start) on screen
    char *history[LE_HISTORY];
    int nhist, hpos;
    char *draft;                 // the line being typed while browsing history
    int tabs;                    // consecutive Tab presses
    rx_handle_t console;
    unsigned char in[64];
    int in_len, in_pos;
};

static struct termios le_cooked;
static int le_have_cooked;

// Raw mode for editing (Ctrl+C still raises the interrupt); 0 if the
// console has no modes (not a console)
static int le_raw(void) {
    if (!le_have_cooked) {
        if (tcgetattr(0, &le_cooked) < 0) return 0;
        le_have_cooked = 1;
    }
    struct termios t = le_cooked;
    t.c_lflag &= ~(unsigned)(ICANON | ECHO);
    t.c_iflag &= ~(unsigned)ICRNL;
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
    return tcsetattr(0, TCSANOW, &t) == 0;
}

// Back to the default (cooked) mode, for the programs the shell starts
static void le_cooked_mode(void) {
    if (le_have_cooked) {
        le_cooked.c_lflag |= ICANON | ECHO | ISIG;
        le_cooked.c_iflag |= ICRNL;
        tcsetattr(0, TCSANOW, &le_cooked);
    }
}

static void le_out(const char *s, size_t n) {
    while (n) {
        ssize_t k = write(1, s, n);
        if (k <= 0) return;
        s += k;
        n -= (size_t)k;
    }
}
static void le_puts(const char *s) { le_out(s, strlen(s)); }

// Characters in s[0..n) (UTF-8 continuation bytes don't count)
static size_t le_chars(const char *s, size_t n) {
    size_t c = 0;
    for (size_t i = 0; i < n; i++) c += ((unsigned char)s[i] & 0xC0) != 0x80;
    return c;
}

// Visible width of a prompt with escape sequences in it
static size_t le_width(const char *s) {
    size_t w = 0;
    while (*s) {
        if (*s == '\033') {
            s++;
            if (*s == '[') {
                s++;
                while (*s && !(*s >= 0x40 && *s <= 0x7E)) s++;
            }
            if (*s) s++;
            continue;
        }
        w += ((unsigned char)*s & 0xC0) != 0x80;
        s++;
    }
    return w;
}

// Move the cursor between two positions (characters from the start of the
// prompt)
static void le_move(struct le *e, size_t from, size_t to) {
    char seq[32];
    if (from == to) return;
    long dr = (long)(to / e->cols) - (long)(from / e->cols);
    if (dr < 0) le_out(seq, (size_t)snprintf(seq, sizeof seq, "\033[%ldA", -dr));
    if (dr > 0) le_out(seq, (size_t)snprintf(seq, sizeof seq, "\033[%ldB", dr));
    le_out("\r", 1);
    if (to % e->cols) le_out(seq, (size_t)snprintf(seq, sizeof seq, "\033[%zuC", to % e->cols));
}

static void le_refresh(struct le *e) {
    size_t start = e->prompt_cols;
    le_move(e, start + e->shown, start);
    le_out(e->buf, e->len);
    le_puts("\033[J");
    size_t end = start + le_chars(e->buf, e->len);
    if (end % e->cols == 0) le_out("\n", 1); // leave the pending wrap at the right margin
    size_t at = start + le_chars(e->buf, e->cur);
    le_move(e, end, at);
    e->shown = at - start;
}

static void le_insert(struct le *e, const char *s, size_t n) {
    if (e->len + n + 1 > e->cap) return;
    memmove(e->buf + e->cur + n, e->buf + e->cur, e->len - e->cur);
    memcpy(e->buf + e->cur, s, n);
    e->len += n;
    e->cur += n;
    e->buf[e->len] = 0;
}

static void le_delete(struct le *e, size_t from, size_t to) {
    memmove(e->buf + from, e->buf + to, e->len - to);
    e->len -= to - from;
    if (e->cur > to) e->cur -= to - from;
    else if (e->cur > from) e->cur = from;
    e->buf[e->len] = 0;
}

static size_t le_prev(struct le *e, size_t i) {
    if (i == 0) return 0;
    do i--; while (i > 0 && ((unsigned char)e->buf[i] & 0xC0) == 0x80);
    return i;
}
static size_t le_next(struct le *e, size_t i) {
    if (i >= e->len) return e->len;
    do i++; while (i < e->len && ((unsigned char)e->buf[i] & 0xC0) == 0x80);
    return i;
}
static int le_wordch(char c) { return c != ' ' && c != '/' && c != '\t'; }
static size_t le_word_left(struct le *e, size_t i) {
    while (i > 0 && !le_wordch(e->buf[i - 1])) i--;
    while (i > 0 && le_wordch(e->buf[i - 1])) i--;
    return i;
}
static size_t le_word_right(struct le *e, size_t i) {
    while (i < e->len && !le_wordch(e->buf[i])) i++;
    while (i < e->len && le_wordch(e->buf[i])) i++;
    return i;
}

static void le_set(struct le *e, const char *s) {
    size_t n = strlen(s);
    if (n >= e->cap) n = e->cap - 1;
    memcpy(e->buf, s, n);
    e->buf[n] = 0;
    e->len = e->cur = n;
}

static void le_add_history(struct le *e, const char *line) {
    size_t n = strlen(line);
    while (n && (line[n - 1] == '\n' || line[n - 1] == ' ')) n--;
    if (!n) return;
    if (e->nhist && strlen(e->history[e->nhist - 1]) == n && !strncmp(e->history[e->nhist - 1], line, n)) return;
    if (e->nhist == LE_HISTORY) {
        free(e->history[0]);
        memmove(e->history, e->history + 1, (LE_HISTORY - 1) * sizeof *e->history);
        e->nhist--;
    }
    char *h = malloc(n + 1);
    if (!h) return;
    memcpy(h, line, n);
    h[n] = 0;
    e->history[e->nhist++] = h;
}

// ---- Tab completion ----

#define LE_MAX_MATCHES 512

struct le_matches {
    char *name[LE_MAX_MATCHES];
    unsigned char dir[LE_MAX_MATCHES];
    int n;
};

static void le_match_add(struct le_matches *m, const char *name, int dir) {
    for (int i = 0; i < m->n; i++)
        if (!strcmp(m->name[i], name)) return;
    if (m->n < LE_MAX_MATCHES && (m->name[m->n] = strdup(name))) m->dir[m->n++] = (unsigned char)dir;
}

static void le_match_dir(struct le_matches *m, const char *dir, const char *prefix, int programs) {
    DIR *d = opendir(*dir ? dir : ".");
    if (!d) return;
    size_t plen = strlen(prefix);
    struct dirent *de;
    while ((de = readdir(d))) {
        if (strncmp(de->d_name, prefix, plen)) continue;
        if (de->d_name[0] == '.' && prefix[0] != '.') continue;
        int isdir = de->d_type == DT_DIR;
        if (programs && isdir) continue;
        le_match_add(m, de->d_name, isdir);
    }
    closedir(d);
}

static int le_cmp(const void *a, const void *b) { return strcmp(*(char *const *)a, *(char *const *)b); }

static void le_list(struct le *e, struct le_matches *m) {
    // In columns, sorted
    char *sorted[LE_MAX_MATCHES];
    size_t w = 0;
    for (int i = 0; i < m->n; i++) {
        sorted[i] = m->name[i];
        if (strlen(sorted[i]) + 2 > w) w = strlen(sorted[i]) + 2;
    }
    qsort(sorted, (size_t)m->n, sizeof *sorted, le_cmp);
    size_t per = e->cols / w ? e->cols / w : 1;
    size_t rows = ((size_t)m->n + per - 1) / per;
    le_move(e, e->prompt_cols + e->shown, e->prompt_cols + le_chars(e->buf, e->len));
    le_puts("\n");
    for (size_t r = 0; r < rows; r++) {
        for (size_t c = 0; c < per; c++) {
            size_t i = c * rows + r;
            if (i >= (size_t)m->n) break;
            printf("%-*s", (int)w, sorted[i]);
        }
        printf("\n");
    }
    fflush(stdout);
    le_puts(e->prompt);
    e->shown = 0;
    le_refresh(e);
}

// The shell's built-in commands (completed like programs)
static const char *const *le_builtins(void);

static void le_complete(struct le *e) {
    // The word before the cursor
    size_t ws = e->cur;
    while (ws > 0 && e->buf[ws - 1] != ' ' && e->buf[ws - 1] != '|' && e->buf[ws - 1] != ';' &&
           e->buf[ws - 1] != '&' && e->buf[ws - 1] != '<' && e->buf[ws - 1] != '>') ws--;
    size_t before = ws;
    while (before > 0 && e->buf[before - 1] == ' ') before--;
    int first = before == 0 || strchr("|;&", e->buf[before - 1]);
    char word[512];
    size_t wl = e->cur - ws;
    if (wl >= sizeof word) return;
    memcpy(word, e->buf + ws, wl);
    word[wl] = 0;

    struct le_matches m;
    m.n = 0;
    const char *base = word;
    char dir[512] = "";
    char *slash = strrchr(word, '/');
    if (first && !slash) {
        for (const char *const *b = le_builtins(); *b; b++)
            if (!strncmp(*b, word, wl)) le_match_add(&m, *b, 0);
        const char *path = getenv("PATH");
        char p[512];
        snprintf(p, sizeof p, "%s", path ? path : "/bin");
        char *save;
        for (char *d = strtok_r(p, ":", &save); d; d = strtok_r(0, ":", &save)) le_match_dir(&m, d, word, 1);
    } else {
        if (slash) {
            size_t dl = (size_t)(slash - word) + 1;
            memcpy(dir, word, dl);
            dir[dl] = 0;
            base = slash + 1;
        }
        // "~/" means $HOME
        char real[600];
        if (dir[0] == '~' && dir[1] == '/' && getenv("HOME")) snprintf(real, sizeof real, "%s%s", getenv("HOME"), dir + 1);
        else snprintf(real, sizeof real, "%s", dir);
        le_match_dir(&m, real, base, 0);
    }
    if (m.n == 0) {
        le_puts("\a");
    } else {
        // Longest common prefix of the matches
        size_t common = strlen(m.name[0]);
        for (int i = 1; i < m.n; i++) {
            size_t k = 0;
            while (k < common && m.name[i][k] == m.name[0][k]) k++;
            common = k;
        }
        size_t have = strlen(base);
        if (common > have) {
            le_insert(e, m.name[0] + have, common - have);
            e->tabs = -1; // progress: the next Tab doesn't list yet
        } else if (m.n == 1) {
            // Complete: a directory gets '/', anything else a space
            le_insert(e, m.dir[0] ? "/" : " ", 1);
        } else if (e->tabs >= 1) {
            le_list(e, &m);
        } else {
            le_puts("\a");
        }
        if (m.n == 1 && common > have) le_insert(e, m.dir[0] ? "/" : " ", 1);
        le_refresh(e);
    }
    for (int i = 0; i < m.n; i++) free(m.name[i]);
}

// ---- reading keys ----

// The next byte typed; -1 on Ctrl+C (the console's interrupt signal), -2
// at end of input
static int le_getc(struct le *e) {
    while (e->in_pos >= e->in_len) {
        uint32_t observed = 0;
        rx_object_wait_one(e->console, RX_CHANNEL_READABLE | RX_PEER_CLOSED | RX_CONSOLE_SIGNAL_INTERRUPT,
                           RX_TIME_INFINITE, &observed);
        if (observed & RX_CONSOLE_SIGNAL_INTERRUPT) {
            rx_object_signal(e->console, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
            return -1;
        }
        ssize_t n = read(0, e->in, sizeof e->in);
        if (n <= 0) return -2;
        e->in_len = (int)n;
        e->in_pos = 0;
    }
    return e->in[e->in_pos++];
}

enum { K_UP = 0x100, K_DOWN, K_LEFT, K_RIGHT, K_HOME, K_END, K_DEL, K_WLEFT, K_WRIGHT, K_ALT_BS, K_NONE };

// One key: a byte, or K_* for an escape sequence
static int le_key(struct le *e) {
    int c = le_getc(e);
    if (c != 0x1B) return c;
    int c2 = le_getc(e);
    if (c2 < 0) return c2;
    if (c2 == 'b' || c2 == 'B') return K_WLEFT;
    if (c2 == 'f' || c2 == 'F') return K_WRIGHT;
    if (c2 == 0x7F || c2 == 0x08) return K_ALT_BS;
    if (c2 != '[' && c2 != 'O') return K_NONE;
    int p[4] = { 0, 0, 0, 0 }, np = 0, f;
    for (;;) {
        f = le_getc(e);
        if (f < 0) return f;
        if (f >= '0' && f <= '9') { if (np == 0) np = 1; p[np - 1] = p[np - 1] * 10 + (f - '0'); }
        else if (f == ';') { if (np == 0) np = 1; if (np < 4) np++; }
        else break;
    }
    int ctrl = np >= 2 && ((p[1] - 1) & 4);
    switch (f) {
    case 'A': return K_UP;
    case 'B': return K_DOWN;
    case 'C': return ctrl ? K_WRIGHT : K_RIGHT;
    case 'D': return ctrl ? K_WLEFT : K_LEFT;
    case 'H': return K_HOME;
    case 'F': return K_END;
    case '~':
        switch (p[0]) {
        case 1: case 7: return K_HOME;
        case 4: case 8: return K_END;
        case 3: return K_DEL;
        }
    }
    return K_NONE;
}

// Read a line with editing. Returns its length (the line ends in '\n'),
// 0 at end of input, -1 on Ctrl+C.
static int le_read(struct le *e, const char *prompt, char *buf, size_t cap) {
    struct winsize ws;
    e->cols = ioctl(0, TIOCGWINSZ, &ws) == 0 && ws.ws_col ? ws.ws_col : 80;
    e->buf = buf;
    e->cap = cap - 1; // room for the '\n'
    e->len = e->cur = e->shown = 0;
    buf[0] = 0;
    e->prompt = prompt;
    e->prompt_cols = le_width(prompt);
    e->hpos = e->nhist;
    e->tabs = 0;
    free(e->draft);
    e->draft = 0;
    le_puts(prompt);
    for (;;) {
        int k = le_key(e);
        int was_tab = k == '\t';
        switch (k) {
        case -1: // Ctrl+C
            le_move(e, e->prompt_cols + e->shown, e->prompt_cols + le_chars(buf, e->len));
            le_puts("^C\n");
            return -1;
        case -2:
            return 0;
        case '\r': case '\n':
            le_move(e, e->prompt_cols + e->shown, e->prompt_cols + le_chars(buf, e->len));
            le_puts("\n");
            le_add_history(e, buf);
            buf[e->len++] = '\n';
            buf[e->len] = 0;
            return (int)e->len;
        case 4: // Ctrl+D
            if (e->len == 0) {
                le_puts("\n");
                return 0;
            }
            /* fall through */
        case K_DEL:
            if (e->cur < e->len) le_delete(e, e->cur, le_next(e, e->cur));
            break;
        case 0x7F: case 0x08:
            if (e->cur > 0) le_delete(e, le_prev(e, e->cur), e->cur);
            break;
        case 0x17: case K_ALT_BS: // Ctrl+W
            le_delete(e, le_word_left(e, e->cur), e->cur);
            break;
        case 0x15: le_delete(e, 0, e->cur); break;      // Ctrl+U
        case 0x0B: le_delete(e, e->cur, e->len); break; // Ctrl+K
        case 0x01: case K_HOME: e->cur = 0; break;
        case 0x05: case K_END: e->cur = e->len; break;
        case 0x02: case K_LEFT: e->cur = le_prev(e, e->cur); break;
        case 0x06: case K_RIGHT: e->cur = le_next(e, e->cur); break;
        case K_WLEFT: e->cur = le_word_left(e, e->cur); break;
        case K_WRIGHT: e->cur = le_word_right(e, e->cur); break;
        case 0x0C: // Ctrl+L
            le_puts("\033[2J\033[H");
            le_puts(prompt);
            e->shown = 0;
            break;
        case 0x10: case K_UP: // Ctrl+P
            if (e->hpos > 0) {
                if (e->hpos == e->nhist) {
                    free(e->draft);
                    e->draft = strdup(buf);
                }
                le_set(e, e->history[--e->hpos]);
            }
            break;
        case 0x0E: case K_DOWN: // Ctrl+N
            if (e->hpos < e->nhist) {
                e->hpos++;
                le_set(e, e->hpos == e->nhist ? (e->draft ? e->draft : "") : e->history[e->hpos]);
            }
            break;
        case '\t':
            le_complete(e);
            e->tabs++;
            continue;
        default:
            if (k >= 0x20 && k < 0x100 && k != 0x7F) {
                char ch = (char)k;
                le_insert(e, &ch, 1);
                // The rest of a UTF-8 character comes in the same message
                if (k >= 0xC0 && e->in_pos < e->in_len) {
                    int more = k >= 0xF0 ? 3 : k >= 0xE0 ? 2 : 1;
                    while (more-- && e->in_pos < e->in_len) {
                        ch = (char)e->in[e->in_pos++];
                        le_insert(e, &ch, 1);
                    }
                }
                // Typing at the end of the line: just echo it
                if (e->cur == e->len && (e->prompt_cols + le_chars(buf, e->len)) % e->cols != 0) {
                    size_t c = le_prev(e, e->cur);
                    le_out(buf + c, e->cur - c);
                    e->shown = le_chars(buf, e->len);
                    continue;
                }
            }
            break;
        }
        if (!was_tab) e->tabs = 0;
        le_refresh(e);
    }
}

#endif

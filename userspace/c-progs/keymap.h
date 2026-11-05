// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Keyboard layouts: key events (HID usages from the PS/2 driver and the
// kernel's USB driver) to the bytes a terminal sends. Used by init.
//
// The US layout is built in. Others are text files in
// /usr/share/keymaps/NAME, chosen in /etc/keymap; each line changes one
// key: "USAGE NORMAL SHIFT [ALTGR]" with USAGE in hex (HID page 7, e.g. 1c
// for the key labelled Y on a US keyboard) and each character as itself
// (UTF-8), U+XXXX, or - for nothing. '#' starts a comment.

#ifndef KEYMAP_H
#define KEYMAP_H

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// HID modifier bits
#define KM_LCTRL  0x01
#define KM_LSHIFT 0x02
#define KM_LALT   0x04
#define KM_RCTRL  0x10
#define KM_RSHIFT 0x20
#define KM_RALT   0x40
#define KM_CTRL  (KM_LCTRL | KM_RCTRL)
#define KM_SHIFT (KM_LSHIFT | KM_RSHIFT)

// Code points for usages 0x04-0x64: normal, shift, AltGr (0 = none)
static uint32_t km_map[0x65][3];
static int km_caps, km_numlock = 1;
static int km_has_altgr;
static char km_name[32] = "us";

static void km_us(void) {
    static const char normal[] = "abcdefghijklmnopqrstuvwxyz1234567890";
    static const char shifted[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZ!@#$%^&*()";
    memset(km_map, 0, sizeof km_map);
    for (int i = 0; i < 36; i++) {
        km_map[0x04 + i][0] = (uint8_t)normal[i];
        km_map[0x04 + i][1] = (uint8_t)shifted[i];
    }
    static const struct { uint8_t u; char n, s; } punct[] = {
        { 0x2C, ' ', ' ' }, { 0x2D, '-', '_' }, { 0x2E, '=', '+' }, { 0x2F, '[', '{' }, { 0x30, ']', '}' },
        { 0x31, '\\', '|' }, { 0x32, '#', '~' }, { 0x33, ';', ':' }, { 0x34, '\'', '"' }, { 0x35, '`', '~' },
        { 0x36, ',', '<' }, { 0x37, '.', '>' }, { 0x38, '/', '?' }, { 0x64, '\\', '|' },
    };
    for (size_t i = 0; i < sizeof punct / sizeof *punct; i++) {
        km_map[punct[i].u][0] = (uint8_t)punct[i].n;
        km_map[punct[i].u][1] = (uint8_t)punct[i].s;
    }
    km_has_altgr = 0;
}

// One character field of a layout line
static uint32_t km_char(const char *t) {
    if (!strcmp(t, "-")) return 0;
    if ((t[0] == 'U' || t[0] == 'u') && t[1] == '+') return (uint32_t)strtoul(t + 2, 0, 16);
    const unsigned char *s = (const unsigned char *)t;
    if (s[0] < 0x80) return s[0];
    if ((s[0] & 0xE0) == 0xC0) return (uint32_t)(s[0] & 0x1F) << 6 | (s[1] & 0x3F);
    if ((s[0] & 0xF0) == 0xE0) return (uint32_t)(s[0] & 0x0F) << 12 | (uint32_t)(s[1] & 0x3F) << 6 | (s[2] & 0x3F);
    return (uint32_t)(s[0] & 0x07) << 18 | (uint32_t)(s[1] & 0x3F) << 12 | (uint32_t)(s[2] & 0x3F) << 6 | (s[3] & 0x3F);
}

// Load the layout named in /etc/keymap (US if none or not found).
// Returns the name in use.
static const char *km_load(void) {
    km_us();
    strcpy(km_name, "us");
    char name[32] = "";
    int fd = open("/etc/keymap", O_RDONLY);
    if (fd >= 0) {
        ssize_t n = read(fd, name, sizeof name - 1);
        close(fd);
        name[n > 0 ? n : 0] = 0;
        name[strcspn(name, " \t\r\n")] = 0;
    }
    if (!name[0] || !strcmp(name, "us") || strchr(name, '/')) return km_name;
    char path[96];
    snprintf(path, sizeof path, "/usr/share/keymaps/%s", name);
    FILE *f = fopen(path, "r");
    if (!f) return km_name;
    char line[160];
    while (fgets(line, sizeof line, f)) {
        if (line[strspn(line, " \t")] == '#') continue; // comment ('#' elsewhere is a character)
        char *save, *tok[4];
        int n = 0;
        for (char *t = strtok_r(line, " \t\r\n", &save); t && n < 4; t = strtok_r(0, " \t\r\n", &save)) tok[n++] = t;
        if (n < 3) continue;
        unsigned long u = strtoul(tok[0], 0, 16);
        if (u < 0x04 || u > 0x64) continue;
        km_map[u][0] = km_char(tok[1]);
        km_map[u][1] = km_char(tok[2]);
        km_map[u][2] = n > 3 ? km_char(tok[3]) : 0;
        if (km_map[u][2]) km_has_altgr = 1;
    }
    fclose(f);
    snprintf(km_name, sizeof km_name, "%s", name);
    return km_name;
}

static int km_put_utf8(uint32_t cp, char *out) {
    if (cp < 0x80) { out[0] = (char)cp; return 1; }
    if (cp < 0x800) { out[0] = (char)(0xC0 | cp >> 6); out[1] = (char)(0x80 | (cp & 0x3F)); return 2; }
    if (cp < 0x10000) {
        out[0] = (char)(0xE0 | cp >> 12); out[1] = (char)(0x80 | (cp >> 6 & 0x3F)); out[2] = (char)(0x80 | (cp & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | cp >> 18); out[1] = (char)(0x80 | (cp >> 12 & 0x3F));
    out[2] = (char)(0x80 | (cp >> 6 & 0x3F)); out[3] = (char)(0x80 | (cp & 0x3F));
    return 4;
}

// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
static int km_modparam(uint8_t mods) {
    return 1 + ((mods & KM_SHIFT) ? 1 : 0) + ((mods & (KM_LALT | KM_RALT)) ? 2 : 0) + ((mods & KM_CTRL) ? 4 : 0);
}

// "ESC [ final" or "ESC [ 1 ; m final"
static int km_csi(char *out, char final, uint8_t mods) {
    int m = km_modparam(mods);
    return m > 1 ? sprintf(out, "\033[1;%d%c", m, final) : sprintf(out, "\033[%c", final);
}

// "ESC [ n ~" or "ESC [ n ; m ~"
static int km_tilde(char *out, int n, uint8_t mods) {
    int m = km_modparam(mods);
    return m > 1 ? sprintf(out, "\033[%d;%d~", n, m) : sprintf(out, "\033[%d~", n);
}

// The bytes a key press sends (0 if none). `cr` chooses what Enter sends.
static int km_translate(uint8_t usage, uint8_t mods, int cr, char *out) {
    int shift = (mods & KM_SHIFT) != 0, ctrl = (mods & KM_CTRL) != 0;
    int altgr = (mods & KM_RALT) && km_has_altgr;
    int alt = (mods & KM_LALT) || ((mods & KM_RALT) && !km_has_altgr);
    switch (usage) {
    case 0x39: km_caps = !km_caps; return 0;
    case 0x53: km_numlock = !km_numlock; return 0;
    case 0x28: case 0x58: out[0] = cr ? '\r' : '\n'; return 1;
    case 0x29: out[0] = 0x1B; return 1;
    case 0x2A: out[0] = ctrl ? 0x08 : 0x7F; return 1;
    case 0x2B:
        if (shift) return sprintf(out, "\033[Z");
        out[0] = '\t';
        return 1;
    case 0x3A: case 0x3B: case 0x3C: case 0x3D: { // F1-F4
        int m = km_modparam(mods);
        char f = (char)('P' + usage - 0x3A);
        return m > 1 ? sprintf(out, "\033[1;%d%c", m, f) : sprintf(out, "\033O%c", f);
    }
    case 0x3E: return km_tilde(out, 15, mods);
    case 0x3F: return km_tilde(out, 17, mods);
    case 0x40: return km_tilde(out, 18, mods);
    case 0x41: return km_tilde(out, 19, mods);
    case 0x42: return km_tilde(out, 20, mods);
    case 0x43: return km_tilde(out, 21, mods);
    case 0x44: return km_tilde(out, 23, mods);
    case 0x45: return km_tilde(out, 24, mods);
    case 0x49: return km_tilde(out, 2, mods);
    case 0x4A: return km_csi(out, 'H', mods);
    case 0x4B: return km_tilde(out, 5, mods);
    case 0x4C: return km_tilde(out, 3, mods);
    case 0x4D: return km_csi(out, 'F', mods);
    case 0x4E: return km_tilde(out, 6, mods);
    case 0x4F: return km_csi(out, 'C', mods);
    case 0x50: return km_csi(out, 'D', mods);
    case 0x51: return km_csi(out, 'B', mods);
    case 0x52: return km_csi(out, 'A', mods);
    }
    // Keypad
    if (usage >= 0x54 && usage <= 0x63) {
        static const char kp[] = "/*-+\n1234567890.";
        char c = kp[usage - 0x54];
        if (usage >= 0x59 && !km_numlock) {
            // Navigation without Num Lock
            static const uint8_t nav[] = { 0x4D, 0x51, 0x4E, 0x50, 0, 0x4F, 0x4A, 0x52, 0x4B, 0x49, 0x4C };
            uint8_t u = nav[usage - 0x59];
            return u ? km_translate(u, mods, cr, out) : 0;
        }
        if (c == '\n') c = cr ? '\r' : '\n';
        out[0] = c;
        return 1;
    }
    if (usage > 0x64 || !km_map[usage][0]) return 0;
    uint32_t cp = km_map[usage][altgr ? 2 : shift ? 1 : 0];
    if (altgr && !cp) cp = km_map[usage][shift ? 1 : 0];
    if (km_caps && !altgr) {
        if (cp >= 'a' && cp <= 'z') cp -= 32;
        else if (cp >= 'A' && cp <= 'Z') cp += 32;
    }
    if (!cp) return 0;
    int n = 0;
    if (alt) out[n++] = 0x1B; // Alt+key: ESC prefix
    if (ctrl) {
        uint32_t base = km_map[usage][0];
        if (base >= 'a' && base <= 'z') { out[n++] = (char)(base - 'a' + 1); return n; }
        switch (base) {
        case '[': out[n++] = 0x1B; return n;
        case '\\': out[n++] = 0x1C; return n;
        case ']': out[n++] = 0x1D; return n;
        case '6': out[n++] = 0x1E; return n;
        case '-': case '/': out[n++] = 0x1F; return n;
        case '2': case ' ': out[n++] = 0; return n;
        }
    }
    return n + km_put_utf8(cp, out + n);
}

#endif

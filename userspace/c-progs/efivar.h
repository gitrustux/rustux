// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// efivar.h: UEFI variables from userspace (step 19), for efibootmgr,
// bootctl and the installer: reading and writing them (rx_efi_variable),
// UTF-16 strings, load options (BootNNNN) and device paths.

#ifndef EFIVAR_H
#define EFIVAR_H

#include <errno.h>
#include <rustux/gpt.h>
#include <rustux/syscalls.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define EFI_GLOBAL_GUID "8be4df61-93ca-11d2-aa0d-00e098032b8c"
#define EFI_LOADER_GUID "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f" // systemd's boot loader interface

#define EV_NV 1u
#define EV_BS 2u
#define EV_RT 4u
#define EV_DEFAULT (EV_NV | EV_BS | EV_RT)

#define LOAD_OPTION_ACTIVE 1u

static __attribute__((unused)) int ev_errno(rx_status_t s) {
    switch (s) {
    case RX_ERR_NOT_FOUND: return -ENOENT;
    case RX_ERR_NOT_SUPPORTED: return -ENOSYS;
    case RX_ERR_ACCESS_DENIED: return -EPERM;
    case RX_ERR_BUFFER_TOO_SMALL: return -ENOSPC;
    case RX_ERR_INVALID_ARGS: return -EINVAL;
    case RX_ERR_NO_MEMORY: return -ENOMEM;
    default: return -EIO;
    }
}

static __attribute__((unused)) const char *ev_strerror(int e) {
    if (e == -ENOSYS) return "no UEFI runtime services (not booted through UEFI, or noefi on the kernel command line)";
    if (e == -EPERM) return "Operation not permitted (only root may change UEFI variables)";
    return strerror(-e);
}

// UTF-8 -> UTF-16LE with a NUL; returns the u16 count with the NUL
static __attribute__((unused)) size_t ev_utf16(const char *s, uint16_t *out, size_t cap) {
    size_t n = 0;
    while (*s && n + 2 < cap) {
        unsigned c = (unsigned char)*s++;
        if (c >= 0xC0 && (*s & 0xC0) == 0x80) {
            if (c < 0xE0) c = (c & 0x1F) << 6 | (*s++ & 0x3F);
            else if (c < 0xF0 && (s[1] & 0xC0) == 0x80) { c = (c & 0x0F) << 12 | (s[0] & 0x3F) << 6 | (s[1] & 0x3F); s += 2; }
        }
        out[n++] = (uint16_t)c;
    }
    out[n++] = 0;
    return n;
}

// UTF-16LE (up to NUL or `n` units) -> UTF-8
static __attribute__((unused)) void ev_utf8(const uint16_t *in, size_t n, char *out, size_t cap) {
    size_t k = 0;
    for (size_t i = 0; i < n && in[i] && k + 4 < cap; i++) {
        unsigned c = in[i];
        if (c < 0x80) out[k++] = (char)c;
        else if (c < 0x800) { out[k++] = (char)(0xC0 | c >> 6); out[k++] = (char)(0x80 | (c & 0x3F)); }
        else { out[k++] = (char)(0xE0 | c >> 12); out[k++] = (char)(0x80 | (c >> 6 & 0x3F)); out[k++] = (char)(0x80 | (c & 0x3F)); }
    }
    out[k] = 0;
}

// Size, or a negative errno (-ENOENT: no such variable)
static __attribute__((unused)) int ev_get(const char *guid, const char *name, void *buf, size_t cap, uint32_t *attrs) {
    uint16_t n16[256];
    rx_efi_var_t v;
    memset(&v, 0, sizeof v);
    if (rx_guid_parse(guid, v.guid) < 0) return -EINVAL;
    v.name_len = (uint32_t)ev_utf16(name, n16, 256);
    v.name = (uint64_t)(uintptr_t)n16;
    v.data = (uint64_t)(uintptr_t)buf;
    v.data_len = cap;
    rx_status_t s = rx_efi_variable(RX_EFI_VAR_GET, &v);
    if (s != RX_OK) return ev_errno(s);
    if (attrs) *attrs = v.attributes;
    return (int)v.data_len;
}

static __attribute__((unused)) int ev_set(const char *guid, const char *name, uint32_t attrs, const void *data, size_t len) {
    uint16_t n16[256];
    rx_efi_var_t v;
    memset(&v, 0, sizeof v);
    if (rx_guid_parse(guid, v.guid) < 0) return -EINVAL;
    v.attributes = attrs;
    v.name_len = (uint32_t)ev_utf16(name, n16, 256);
    v.name = (uint64_t)(uintptr_t)n16;
    v.data = (uint64_t)(uintptr_t)data;
    v.data_len = len;
    rx_status_t s = rx_efi_variable(RX_EFI_VAR_SET, &v);
    return s == RX_OK ? 0 : ev_errno(s);
}

static __attribute__((unused)) int ev_delete(const char *guid, const char *name) {
    int r = ev_set(guid, name, 0, 0, 0);
    return r == -ENOENT ? 0 : r;
}

// The variable after (name, guid); start with name "" and a zero guid.
// 0, or -ENOENT at the end.
static __attribute__((unused)) int ev_next(char *name, size_t cap, uint8_t guid[16]) {
    uint16_t n16[512];
    ev_utf16(name, n16, 512);
    rx_efi_var_t v;
    memset(&v, 0, sizeof v);
    memcpy(v.guid, guid, 16);
    v.name_len = 512;
    v.name = (uint64_t)(uintptr_t)n16;
    rx_status_t s = rx_efi_variable(RX_EFI_VAR_NEXT, &v);
    if (s != RX_OK) return ev_errno(s);
    memcpy(guid, v.guid, 16);
    ev_utf8(n16, 512, name, cap);
    return 0;
}

// A UTF-16 string variable as UTF-8 ("" if missing): 0 or -errno
static __attribute__((unused)) int ev_get_str(const char *guid, const char *name, char *out, size_t cap) {
    uint16_t buf[1024];
    out[0] = 0;
    int n = ev_get(guid, name, buf, sizeof buf - 2, 0);
    if (n < 0) return n;
    buf[n / 2] = 0;
    ev_utf8(buf, (size_t)n / 2, out, cap);
    return 0;
}

static __attribute__((unused)) int ev_set_str(const char *guid, const char *name, uint32_t attrs, const char *s) {
    uint16_t buf[512];
    size_t n = ev_utf16(s, buf, 512);
    return ev_set(guid, name, attrs, buf, n * 2);
}

// ---- device paths ----

static __attribute__((unused)) uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
static __attribute__((unused)) uint32_t rd32le(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static __attribute__((unused)) uint64_t rd64le(const uint8_t *p) { return rd32le(p) | (uint64_t)rd32le(p + 4) << 32; }

// As efibootmgr shows them: HD(1,GPT,guid,0x800,0x100000)/File(\EFI\...)
static __attribute__((unused)) void dp_text(const uint8_t *p, size_t len, char *out, size_t cap) {
    size_t k = 0;
    out[0] = 0;
#define DP_ADD(...) do { if (k < cap) { int w_ = snprintf(out + k, cap - k, __VA_ARGS__); if (w_ > 0) k += (size_t)w_; } } while (0)
    for (size_t off = 0; off + 4 <= len;) {
        uint8_t t = p[off], st = p[off + 1];
        uint16_t l = rd16(p + off + 2);
        if (l < 4 || off + l > len) break;
        const uint8_t *d = p + off + 4;
        size_t dl = l - 4u;
        if (t == 0x7F) break;
        if (k) DP_ADD("/");
        if (t == 4 && st == 1 && dl >= 38) {
            char g[37] = "";
            if (d[37] == 2) rx_guid_format(d + 20, g);
            else snprintf(g, sizeof g, "%08x", rd32le(d + 20));
            DP_ADD("HD(%u,%s,%s,0x%llx,0x%llx)", rd32le(d), d[36] == 2 ? "GPT" : "MBR", g, (unsigned long long)rd64le(d + 4),
                   (unsigned long long)rd64le(d + 12));
        } else if (t == 4 && st == 4) {
            char f[512];
            uint16_t w[256];
            size_t n = dl / 2 < 255 ? dl / 2 : 255;
            for (size_t i = 0; i < n; i++) w[i] = rd16(d + 2 * i);
            w[n] = 0;
            ev_utf8(w, n, f, sizeof f);
            DP_ADD("File(%s)", f);
        } else if (t == 2 && st == 1 && dl >= 8) {
            DP_ADD("PciRoot(0x%x)", rd32le(d + 4));
        } else if (t == 1 && st == 1 && dl >= 2) {
            DP_ADD("Pci(0x%x,0x%x)", d[1], d[0]);
        } else if (t == 3 && st == 18 && dl >= 6) {
            DP_ADD("Sata(0x%x,0x%x,0x%x)", rd16(d), rd16(d + 2), rd16(d + 4));
        } else if (t == 3 && st == 11) {
            DP_ADD("MAC(");
            for (size_t i = 0; i < 6 && i < dl; i++) DP_ADD("%02x", d[i]);
            DP_ADD(")");
        } else if (t == 3 && st == 12) {
            DP_ADD("IPv4(...)");
        } else if (t == 3 && st == 13) {
            DP_ADD("IPv6(...)");
        } else if (t == 3 && st == 24) {
            DP_ADD("Uri(...)");
        } else if (t == 3 && st == 5) {
            DP_ADD("USB(0x%x,0x%x)", dl > 0 ? d[0] : 0, dl > 1 ? d[1] : 0);
        } else if (t == 3 && st == 23 && dl >= 4) {
            DP_ADD("NVMe(0x%x,...)", rd32le(d));
        } else if (t == 4 && st == 6) {
            DP_ADD("FvFile(...)");
        } else if (t == 4 && st == 7) {
            DP_ADD("FvVol(...)");
        } else {
            DP_ADD("Path(%u,%u)", t, st);
        }
        off += l;
    }
#undef DP_ADD
}

// HD(part)/File(path)/End: the short form efibootmgr writes too (the
// firmware finds the disk by the partition's GUID). The length.
static __attribute__((unused)) size_t dp_build_hd_file(uint8_t *out, size_t cap, uint32_t partnum, uint64_t start, uint64_t size,
                                                       const uint8_t guid[16], const char *path) {
    uint16_t w[256];
    char p2[256];
    snprintf(p2, sizeof p2, "%s", path);
    for (char *c = p2; *c; c++) if (*c == '/') *c = '\\';
    size_t n = ev_utf16(p2, w, 256);
    size_t need = 42 + 4 + n * 2 + 4;
    if (need > cap) return 0;
    uint8_t *q = out;
    q[0] = 4; q[1] = 1; q[2] = 42; q[3] = 0;
    memcpy(q + 4, &partnum, 4);
    memcpy(q + 8, &start, 8);
    memcpy(q + 16, &size, 8);
    memcpy(q + 24, guid, 16);
    q[40] = 2; // GPT
    q[41] = 2; // GUID signature
    q += 42;
    uint16_t fl = (uint16_t)(4 + n * 2);
    q[0] = 4; q[1] = 4; q[2] = (uint8_t)fl; q[3] = (uint8_t)(fl >> 8);
    for (size_t i = 0; i < n; i++) { q[4 + 2 * i] = (uint8_t)w[i]; q[5 + 2 * i] = (uint8_t)(w[i] >> 8); }
    q += fl;
    q[0] = 0x7F; q[1] = 0xFF; q[2] = 4; q[3] = 0;
    return need;
}

// ---- load options (BootNNNN) ----

typedef struct {
    uint32_t attrs;
    char desc[256];
    const uint8_t *dp;
    size_t dp_len;
    const uint8_t *opt;
    size_t opt_len;
} load_option_t;

static __attribute__((unused)) int lo_parse(const uint8_t *b, size_t n, load_option_t *lo) {
    if (n < 8) return -1;
    lo->attrs = rd32le(b);
    uint16_t fpl = rd16(b + 4);
    size_t i = 6;
    uint16_t w[256];
    size_t k = 0;
    while (i + 1 < n && rd16(b + i)) {
        if (k < 255) w[k++] = rd16(b + i);
        i += 2;
    }
    w[k] = 0;
    i += 2;
    ev_utf8(w, k, lo->desc, sizeof lo->desc);
    if (i + fpl > n) return -1;
    lo->dp = b + i;
    lo->dp_len = fpl;
    lo->opt = b + i + fpl;
    lo->opt_len = n - i - fpl;
    return 0;
}

static __attribute__((unused)) size_t lo_build(uint8_t *out, size_t cap, uint32_t attrs, const char *desc, const uint8_t *dp, size_t dp_len) {
    uint16_t w[256];
    size_t n = ev_utf16(desc, w, 256);
    size_t need = 6 + n * 2 + dp_len;
    if (need > cap) return 0;
    memcpy(out, &attrs, 4);
    out[4] = (uint8_t)dp_len;
    out[5] = (uint8_t)(dp_len >> 8);
    for (size_t i = 0; i < n; i++) { out[6 + 2 * i] = (uint8_t)w[i]; out[7 + 2 * i] = (uint8_t)(w[i] >> 8); }
    memcpy(out + 6 + n * 2, dp, dp_len);
    return need;
}

// BootOrder as numbers; the count or -errno
static __attribute__((unused)) int ev_boot_order(uint16_t *order, int max) {
    uint8_t b[512];
    int n = ev_get(EFI_GLOBAL_GUID, "BootOrder", b, sizeof b, 0);
    if (n == -ENOENT) return 0;
    if (n < 0) return n;
    int k = 0;
    for (int i = 0; i + 1 < n && k < max; i += 2) order[k++] = rd16(b + i);
    return k;
}

static __attribute__((unused)) int ev_set_boot_order(const uint16_t *order, int n) {
    if (!n) return ev_delete(EFI_GLOBAL_GUID, "BootOrder");
    return ev_set(EFI_GLOBAL_GUID, "BootOrder", EV_DEFAULT, order, (size_t)n * 2);
}

#endif

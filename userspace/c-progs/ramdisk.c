// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ramdisk: a block device in memory, served with the block protocol
// (<rustux/block.h>) and published to devmgr (it shows in lsblk as
// ram0, ram1, ...). Runs until killed; the device goes away with it.
//
//   ramdisk [-r] [-b BLOCKSIZE] [-q] SIZE      SIZE: bytes, or with K, M or G
//
// The first block device for testing filesystems and tools before the
// disk drivers of step 13.

#include <errno.h>
#include <rustux/block.h>
#include <rustux/device.h>
#include <rustux/fs.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_CONNS 32
#define MAX_VMOS  16

static rx_block_info_t info;
static uint8_t *disk;

struct conn {
    rx_handle_t ch;
    rx_handle_t vmos[MAX_VMOS]; // vmoid = index + 1
};

static struct conn conns[MAX_CONNS];
static int nconns;
static char req[sizeof(rx_block_req_t) + RX_BLOCK_MAX_IO], rep[sizeof(rx_block_reply_t) + RX_BLOCK_MAX_IO + 256];

static uint64_t parse_size(const char *s) {
    char *end;
    uint64_t v = strtoull(s, &end, 10);
    switch (*end) {
    case 'k': case 'K': v <<= 10; end++; break;
    case 'm': case 'M': v <<= 20; end++; break;
    case 'g': case 'G': v <<= 30; end++; break;
    }
    if (*end == 'i' || *end == 'B') end++;
    if (*end == 'B') end++;
    return *end ? 0 : v;
}

static void send_reply(rx_handle_t ch, uint32_t txid, int status, const void *data, size_t len) {
    rx_block_reply_t r = { txid, status };
    memcpy(rep, &r, sizeof r);
    if (status < 0) len = 0;
    if (len) memcpy(rep + sizeof r, data, len);
    rx_channel_write(ch, 0, rep, (uint32_t)(sizeof r + len), 0, 0);
}

// Byte range of a request, or a negative errno
static int64_t range(const rx_block_req_t *q, uint64_t *off) {
    if (q->count == 0) return -EINVAL;
    if (q->block >= info.block_count || q->count > info.block_count - q->block) return -ERANGE;
    *off = q->block * info.block_size;
    return (int64_t)q->count * info.block_size;
}

static int serve(struct conn *c) {
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_status_t s = rx_channel_read(c->ch, 0, req, &h, sizeof req, 1, &nb, &nh);
    if (s == RX_ERR_SHOULD_WAIT) return 0;
    if (s != RX_OK) return -1;
    if (!nh) h = 0;
    if (nb < sizeof(rx_block_req_t)) {
        if (h) rx_handle_close(h);
        return 0;
    }
    rx_block_req_t q;
    memcpy(&q, req, sizeof q);
    const char *data = req + sizeof q;
    size_t dlen = nb - sizeof q;
    uint64_t off = 0;
    int64_t len;
    int ro = info.flags & RX_BLOCK_FLAG_READONLY;
    switch (q.op) {
    case RX_BLOCK_OP_GET_INFO:
        send_reply(c->ch, q.txid, 0, &info, sizeof info);
        break;
    case RX_BLOCK_OP_READ:
        len = range(&q, &off);
        if (len >= 0 && len > RX_BLOCK_MAX_IO) len = -EINVAL;
        send_reply(c->ch, q.txid, len < 0 ? (int)len : 0, disk + off, len < 0 ? 0 : (size_t)len);
        break;
    case RX_BLOCK_OP_WRITE:
        len = range(&q, &off);
        if (len >= 0 && (len > RX_BLOCK_MAX_IO || (size_t)len != dlen)) len = -EINVAL;
        if (len >= 0 && ro) len = -EROFS;
        if (len > 0) memcpy(disk + off, data, (size_t)len);
        send_reply(c->ch, q.txid, len < 0 ? (int)len : 0, 0, 0);
        break;
    case RX_BLOCK_OP_FLUSH:
        send_reply(c->ch, q.txid, 0, 0, 0); // memory: always written
        break;
    case RX_BLOCK_OP_ATTACH_VMO: {
        int slot = -1;
        for (int k = 0; k < MAX_VMOS && slot < 0; k++) if (!c->vmos[k]) slot = k;
        if (!h || slot < 0) {
            if (h) rx_handle_close(h);
            send_reply(c->ch, q.txid, h ? -ENOSPC : -EINVAL, 0, 0);
            break;
        }
        c->vmos[slot] = h;
        h = 0;
        uint32_t id = (uint32_t)slot + 1;
        send_reply(c->ch, q.txid, 0, &id, 4);
        break;
    }
    case RX_BLOCK_OP_READ_VMO:
    case RX_BLOCK_OP_WRITE_VMO: {
        len = range(&q, &off);
        rx_handle_t v = q.vmoid >= 1 && q.vmoid <= MAX_VMOS ? c->vmos[q.vmoid - 1] : 0;
        if (len >= 0 && !v) len = -EINVAL;
        if (len >= 0 && q.op == RX_BLOCK_OP_WRITE_VMO && ro) len = -EROFS;
        if (len >= 0) {
            rx_status_t st = q.op == RX_BLOCK_OP_READ_VMO ? rx_vmo_write(v, disk + off, q.vmo_offset, (uint64_t)len)
                                                          : rx_vmo_read(v, disk + off, q.vmo_offset, (uint64_t)len);
            if (st != RX_OK) len = st == RX_ERR_OUT_OF_RANGE ? -ERANGE : -EIO;
        }
        send_reply(c->ch, q.txid, len < 0 ? (int)len : 0, 0, 0);
        break;
    }
    case RX_BLOCK_OP_DETACH_VMO:
        if (q.vmoid >= 1 && q.vmoid <= MAX_VMOS && c->vmos[q.vmoid - 1]) {
            rx_handle_close(c->vmos[q.vmoid - 1]);
            c->vmos[q.vmoid - 1] = 0;
            send_reply(c->ch, q.txid, 0, 0, 0);
        } else {
            send_reply(c->ch, q.txid, -EINVAL, 0, 0);
        }
        break;
    default:
        send_reply(c->ch, q.txid, -EINVAL, 0, 0);
    }
    if (h) rx_handle_close(h);
    return 0;
}

static void drop(int k) {
    rx_handle_close(conns[k].ch);
    for (int v = 0; v < MAX_VMOS; v++) if (conns[k].vmos[v]) rx_handle_close(conns[k].vmos[v]);
    conns[k] = conns[--nconns];
}

static int usage(FILE *f, int code) {
    fprintf(f, "usage: ramdisk [-r] [-b BLOCKSIZE] [-q] SIZE\n"
               "Serve a block device in memory (ram0, ram1, ...) until killed; run it with &.\n"
               "  SIZE          bytes, or with K, M or G (16M)\n"
               "  -b BLOCKSIZE  bytes per block, a power of two from 512 (default 512)\n"
               "  -r            read-only\n"
               "  -q            print nothing\n");
    return code;
}

int main(int argc, char **argv) {
    uint32_t bs = 512;
    int ro = 0, quiet = 0;
    const char *size_arg = 0;
    for (int a = 1; a < argc; a++) {
        if (!strcmp(argv[a], "-r")) ro = 1;
        else if (!strcmp(argv[a], "-q")) quiet = 1;
        else if (!strcmp(argv[a], "-b") && a + 1 < argc) bs = (uint32_t)strtoul(argv[++a], 0, 10);
        else if (!strcmp(argv[a], "--help") || !strcmp(argv[a], "-h")) return usage(stdout, 0);
        else if (argv[a][0] != '-' && !size_arg) size_arg = argv[a];
        else return usage(stderr, 2);
    }
    uint64_t size = size_arg ? parse_size(size_arg) : 0;
    if (!size || bs < 512 || (bs & (bs - 1)) || size % bs || size > (1ull << 30)) {
        fprintf(stderr, "ramdisk: SIZE must be a multiple of the block size, at most 1G\n");
        return usage(stderr, 2);
    }
    rx_handle_t vmo;
    uint64_t addr;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK) {
        fprintf(stderr, "ramdisk: not enough memory for %s\n", size_arg);
        return 1;
    }
    disk = (uint8_t *)addr;
    strcpy(info.name, "ram");
    info.block_count = size / bs;
    info.block_size = bs;
    info.flags = ro ? RX_BLOCK_FLAG_READONLY : 0;
    strcpy(info.driver, "ramdisk");
    strcpy(info.model, "RAM disk");

    rx_handle_t dm, listen, theirs;
    int r = rx_devmgr_connect(&dm);
    if (r < 0) {
        fprintf(stderr, "ramdisk: cannot reach the device manager (%s)\n", strerror(-r));
        return 1;
    }
    rx_channel_create(0, &listen, &theirs);
    rx_block_info_t got;
    r = rx_dev_call(dm, RX_DM_OP_PUBLISH_BLOCK, &info, sizeof info, theirs, &got, sizeof got, 0, 0);
    rx_handle_close(dm);
    if (r < 0) {
        fprintf(stderr, "ramdisk: cannot publish the device (%s)\n", strerror(-r));
        return 1;
    }
    info = got;
    if (!quiet) printf("%s: %llu blocks of %u bytes%s\n", info.name, (unsigned long long)info.block_count, bs, ro ? ", read-only" : "");

    for (;;) {
        rx_wait_item_t items[1 + MAX_CONNS];
        items[0] = (rx_wait_item_t){ listen, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        for (int k = 0; k < nconns; k++) items[1 + k] = (rx_wait_item_t){ conns[k].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        if (rx_object_wait_many(items, 1 + (uint64_t)nconns, RX_TIME_INFINITE) != RX_OK) continue;
        for (int k = nconns - 1; k >= 0; k--) {
            if (items[1 + k].pending && serve(&conns[k]) < 0) drop(k);
        }
        if (items[0].pending & RX_CHANNEL_READABLE) {
            rx_svc_connect_t m;
            rx_handle_t h;
            uint32_t nb, nh;
            while (rx_channel_read(listen, 0, &m, &h, sizeof m, 1, &nb, &nh) == RX_OK) {
                if (!nh) continue;
                if (nconns < MAX_CONNS) {
                    memset(&conns[nconns], 0, sizeof conns[nconns]);
                    conns[nconns++].ch = h;
                } else {
                    rx_handle_close(h);
                }
            }
        } else if (items[0].pending & RX_PEER_CLOSED) {
            return 0; // devmgr is gone
        }
    }
}

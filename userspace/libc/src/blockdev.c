// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Serving a disk and its partitions (<rustux/blockdev.h>).

#include <errno.h>
#include <rustux/blockdev.h>
#include <rustux/device.h>
#include <rustux/gpt.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_DEVS  (1 + RX_GPT_MAX_PARTS)
#define MAX_CONNS 32
#define MAX_VMOS  16
#define BUF_SIZE  (128u * 1024u)

struct dev {
    rx_block_info_t info;
    rx_handle_t listen;     // devmgr sends new connections here
};

struct conn {
    rx_handle_t ch;
    int dev;                // index into devs
    rx_handle_t vmos[MAX_VMOS];  // vmoid = index + 1
};

static const rx_blockdev_ops_t *ops;
static void *octx;
static unsigned bflags;
static uint32_t chunk;      // blocks per call to ops
static struct dev devs[MAX_DEVS];
static int ndevs;
static struct conn conns[MAX_CONNS];
static int nconns;
static unsigned generation;  // bumped when the partitions are replaced
static uint8_t *buf;
static char req[sizeof(rx_block_req_t) + RX_BLOCK_MAX_IO], rep[sizeof(rx_block_reply_t) + sizeof(rx_block_info_t) + RX_BLOCK_MAX_IO];

// ---- talking to the disk (whole-disk block numbers) ----

static int disk_read(void *c, uint64_t lba, uint32_t count, void *out) {
    (void)c;
    uint32_t bs = devs[0].info.block_size;
    for (uint32_t done = 0; done < count; ) {
        uint32_t n = count - done < chunk ? count - done : chunk;
        int r = ops->read(octx, lba + done, n, (uint8_t *)out + (size_t)done * bs);
        if (r < 0) return r;
        done += n;
    }
    return 0;
}

static int disk_write(void *c, uint64_t lba, uint32_t count, const void *in) {
    (void)c;
    if (!ops->write || (devs[0].info.flags & RX_BLOCK_FLAG_READONLY)) return -EROFS;
    uint32_t bs = devs[0].info.block_size;
    for (uint32_t done = 0; done < count; ) {
        uint32_t n = count - done < chunk ? count - done : chunk;
        int r = ops->write(octx, lba + done, n, (const uint8_t *)in + (size_t)done * bs);
        if (r < 0) return r;
        done += n;
    }
    return 0;
}

// ---- publishing ----

static int publish(struct dev *d) {
    rx_handle_t dm, theirs;
    int r = rx_devmgr_connect(&dm);
    if (r < 0) return r;
    if (rx_channel_create(0, &d->listen, &theirs) != RX_OK) {
        rx_handle_close(dm);
        return -ENOMEM;
    }
    rx_block_info_t got;
    r = rx_dev_call(dm, RX_DM_OP_PUBLISH_BLOCK, &d->info, sizeof d->info, theirs, &got, sizeof got, 0, 0);
    rx_handle_close(dm);
    if (r < 0) {
        rx_handle_close(d->listen);
        d->listen = 0;
        return r;
    }
    d->info = got;
    return 0;
}

static void add_partition(int number, uint64_t first, uint64_t count, unsigned table) {
    if (ndevs >= MAX_DEVS || !count) return;
    struct dev *d = &devs[ndevs];
    memset(d, 0, sizeof *d);
    const rx_block_info_t *disk = &devs[0].info;
    d->info.block_count = count;
    d->info.block_size = disk->block_size;
    d->info.flags = (disk->flags & (RX_BLOCK_FLAG_READONLY | RX_BLOCK_FLAG_REMOVABLE)) | RX_BLOCK_FLAG_PARTITION | table;
    memcpy(d->info.driver, disk->driver, sizeof d->info.driver);
    memcpy(d->info.model, disk->model, sizeof d->info.model);
    d->info.start = first;
    d->info.number = (uint32_t)number;
    ndevs++;
}

// Read the partition table and publish a device per partition
static void scan(void) {
    rx_block_info_t *disk = &devs[0].info;
    disk->flags &= ~(RX_BLOCK_FLAG_GPT | RX_BLOCK_FLAG_MBR);
    memset(disk->guid, 0, sizeof disk->guid);
    if (bflags & RX_BLOCKDEV_NO_PARTITIONS) return;
    static rx_gpt_t gpt;
    int r = rx_gpt_read(&gpt, disk->block_size, disk->block_count, disk_read, 0);
    if (r == 0) {
        disk->flags |= RX_BLOCK_FLAG_GPT;
        memcpy(disk->guid, gpt.disk_guid, 16);
        for (uint32_t i = 0; i < gpt.entries; i++) {
            rx_gpt_part_t *p = &gpt.parts[i];
            if (rx_guid_is_zero(p->type)) continue;
            int before = ndevs;
            add_partition((int)i + 1, p->first, p->last - p->first + 1, RX_BLOCK_FLAG_GPT);
            if (ndevs == before) continue;
            rx_block_info_t *q = &devs[ndevs - 1].info;
            memcpy(q->label, p->name, sizeof q->label);
            memcpy(q->type_guid, p->type, 16);
            memcpy(q->guid, p->guid, 16);
        }
        if (gpt.from_backup) fprintf(stderr, "%s: the primary GPT is damaged; using the backup copy\n", disk->name);
    } else if (r == -EBADMSG) {
        fprintf(stderr, "%s: the GPT is damaged (both copies); no partitions\n", disk->name);
    } else {
        rx_mbr_part_t mp[4];
        if (rx_mbr_read(mp, disk->block_size, disk_read, 0) > 0) {
            disk->flags |= RX_BLOCK_FLAG_MBR;
            for (int i = 0; i < 4; i++) {
                if (!mp[i].type || (uint64_t)mp[i].first + mp[i].count > disk->block_count) continue;
                int before = ndevs;
                add_partition(i + 1, mp[i].first, mp[i].count, RX_BLOCK_FLAG_MBR);
                if (ndevs > before) devs[ndevs - 1].info.type_guid[0] = mp[i].type;
            }
        }
    }
}

static void unpublish_partitions(void) {
    for (int k = 1; k < ndevs; k++) if (devs[k].listen) rx_handle_close(devs[k].listen);
    ndevs = 1;
}

// Partitions are named after the disk: vda -> vda1, nvme0n1 -> nvme0n1p1.
// devmgr keeps the disk's info as it was published; after a rescan the
// disk's own GET_INFO has the new table type and GUID.
static void publish_partitions(void) {
    const char *dn = devs[0].info.name;
    size_t len = strlen(dn);
    int digit = len && dn[len - 1] >= '0' && dn[len - 1] <= '9';
    for (int k = 1; k < ndevs; k++) {
        rx_block_info_t *p = &devs[k].info;
        snprintf(p->name, sizeof p->name, "%.24s%s%u", dn, digit ? "p" : "", p->number);
        memcpy(p->parent, dn, sizeof p->parent);
        if (publish(&devs[k]) < 0) {
            fprintf(stderr, "%s: cannot publish %s\n", devs[0].info.name, devs[k].info.name);
            devs[k].listen = 0;
        }
    }
}

int rx_blockdev_publish(const rx_blockdev_ops_t *o, void *ctx, rx_block_info_t *info, uint32_t max_blocks, unsigned flags) {
    if (!o || !o->read || !info || info->block_size < 512 || (info->block_size & (info->block_size - 1)) || !info->block_count) return -EINVAL;
    ops = o;
    octx = ctx;
    bflags = flags;
    if (!buf && !(buf = malloc(BUF_SIZE))) return -ENOMEM;
    chunk = BUF_SIZE / info->block_size;
    if (max_blocks && max_blocks < chunk) chunk = max_blocks;
    if (!chunk) return -EINVAL;
    if (!o->write) info->flags |= RX_BLOCK_FLAG_READONLY;
    memset(devs, 0, sizeof devs);
    devs[0].info = *info;
    ndevs = 1;
    // The table first, so devmgr gets the disk with its table type; then
    // the disk (which gets its name), then the partitions named after it
    scan();
    int r = publish(&devs[0]);
    if (r < 0) {
        ndevs = 1;
        return r;
    }
    *info = devs[0].info;
    publish_partitions();
    return 0;
}

// ---- requests ----

static void send_reply(rx_handle_t ch, uint32_t txid, int status, const void *data, size_t len) {
    rx_block_reply_t r = { txid, status };
    memcpy(rep, &r, sizeof r);
    if (status < 0) len = 0;
    if (len) memcpy(rep + sizeof r, data, len);
    rx_channel_write(ch, 0, rep, (uint32_t)(sizeof r + len), 0, 0);
}

// Checks a request's range on device `d`; the first block on the disk
// in *lba, the byte count returned (or a negative errno)
static int64_t range(const struct dev *d, const rx_block_req_t *q, uint64_t *lba) {
    if (q->count == 0) return -EINVAL;
    if (q->block >= d->info.block_count || q->count > d->info.block_count - q->block) return -ERANGE;
    *lba = d->info.start + q->block;
    return (int64_t)q->count * d->info.block_size;
}

static int rescan(struct conn *c) {
    if (c->dev != 0) return -EINVAL;
    if (bflags & RX_BLOCKDEV_NO_PARTITIONS) return -ENOTSUP;
    for (int k = 0; k < nconns; k++) if (conns[k].dev > 0) return -EBUSY;
    unpublish_partitions();
    scan();
    publish_partitions();
    generation++;
    return 0;
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
    struct dev *d = &devs[c->dev];
    uint32_t bs = d->info.block_size;
    const char *data = req + sizeof q;
    size_t dlen = nb - sizeof q;
    uint64_t lba = 0;
    int64_t len;
    int ro = (d->info.flags & RX_BLOCK_FLAG_READONLY) || !ops->write;
    switch (q.op) {
    case RX_BLOCK_OP_GET_INFO:
        send_reply(c->ch, q.txid, 0, &d->info, sizeof d->info);
        break;
    case RX_BLOCK_OP_READ:
        len = range(d, &q, &lba);
        if (len > RX_BLOCK_MAX_IO) len = -EINVAL;
        if (len > 0) {
            int r = disk_read(0, lba, q.count, buf);
            if (r < 0) len = r;
        }
        send_reply(c->ch, q.txid, len < 0 ? (int)len : 0, buf, len < 0 ? 0 : (size_t)len);
        break;
    case RX_BLOCK_OP_WRITE:
        len = range(d, &q, &lba);
        if (len >= 0 && (len > RX_BLOCK_MAX_IO || (size_t)len != dlen)) len = -EINVAL;
        if (len >= 0 && ro) len = -EROFS;
        if (len > 0) {
            int r = disk_write(0, lba, q.count, data);
            if (r < 0) len = r;
        }
        send_reply(c->ch, q.txid, len < 0 ? (int)len : 0, 0, 0);
        break;
    case RX_BLOCK_OP_FLUSH: {
        int r = ops->flush && !ro ? ops->flush(octx) : 0;
        send_reply(c->ch, q.txid, r, 0, 0);
        break;
    }
    case RX_BLOCK_OP_ATTACH_VMO: {
        int slot = -1;
        for (int k = 0; k < MAX_VMOS && slot < 0; k++) if (!c->vmos[k]) slot = k;
        if (!h || slot < 0) {
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
        len = range(d, &q, &lba);
        rx_handle_t v = q.vmoid >= 1 && q.vmoid <= MAX_VMOS ? c->vmos[q.vmoid - 1] : 0;
        if (len >= 0 && !v) len = -EINVAL;
        if (len >= 0 && q.op == RX_BLOCK_OP_WRITE_VMO && ro) len = -EROFS;
        for (uint32_t done = 0; len >= 0 && done < q.count; ) {
            uint32_t n = q.count - done < chunk ? q.count - done : chunk;
            uint64_t off = q.vmo_offset + (uint64_t)done * bs;
            int r;
            if (q.op == RX_BLOCK_OP_READ_VMO) {
                r = disk_read(0, lba + done, n, buf);
                if (r >= 0 && rx_vmo_write(v, buf, off, (uint64_t)n * bs) != RX_OK) r = -ERANGE;
            } else {
                r = rx_vmo_read(v, buf, off, (uint64_t)n * bs) == RX_OK ? disk_write(0, lba + done, n, buf) : -ERANGE;
            }
            if (r < 0) len = r;
            done += n;
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
    case RX_BLOCK_OP_RESCAN:
        send_reply(c->ch, q.txid, rescan(c), 0, 0);
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

int rx_blockdev_run(void) {
    for (;;) {
        rx_wait_item_t items[MAX_DEVS + MAX_CONNS];
        int which[MAX_DEVS];    // device of each listen item
        int n = 0;
        for (int k = 0; k < ndevs; k++) {
            if (!devs[k].listen) continue;
            which[n] = k;
            items[n++] = (rx_wait_item_t){ devs[k].listen, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        }
        int first_conn = n;
        for (int k = 0; k < nconns; k++) items[n++] = (rx_wait_item_t){ conns[k].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        if (rx_object_wait_many(items, (uint64_t)n, RX_TIME_INFINITE) != RX_OK) continue;
        unsigned gen = generation;
        for (int k = nconns - 1; k >= 0; k--) {
            if (items[first_conn + k].pending && serve(&conns[k]) < 0) drop(k);
        }
        // A rescan replaced the partitions: their wait items are stale
        if (gen != generation) continue;
        for (int i = 0; i < first_conn; i++) {
            int d = which[i];
            if (items[i].pending & RX_CHANNEL_READABLE) {
                rx_svc_connect_t m;
                rx_handle_t h;
                uint32_t nb, nh;
                while (rx_channel_read(devs[d].listen, 0, &m, &h, sizeof m, 1, &nb, &nh) == RX_OK) {
                    if (!nh) continue;
                    if (nconns < MAX_CONNS) {
                        memset(&conns[nconns], 0, sizeof conns[nconns]);
                        conns[nconns].ch = h;
                        conns[nconns++].dev = d;
                    } else {
                        rx_handle_close(h);
                    }
                }
            } else if (items[i].pending & RX_PEER_CLOSED) {
                if (d == 0) return 0; // devmgr is gone
                rx_handle_close(devs[d].listen);
                devs[d].listen = 0;
            }
        }
    }
}

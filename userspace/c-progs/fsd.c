// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fsd: the filesystem service. Serves one directory tree over channels
// (protocol in <rustux/fs.h>, docs/FS.md):
//
//   /          tmpfs (in memory, writable)
//   /tmp       tmpfs
//   /root      tmpfs (root's home)
//   /etc       tmpfs, filled from the boot image's etc/ at start (so the
//              settings can be changed until the next boot)
//   /boot      bootfs: the boot image, read-only
//   /bin       bootfs's bin directory (mounted again)
//   /usr       bootfs's usr directory (mounted again)
//   /dev       devices: null (reads nothing, takes every write) and zero
//              (reads zeros)
//
// init starts fsd with the boot image VMO (PA_VMO_BOOTFS) and the server
// end of the first root connection (PA_USER0). Every open file or
// directory is a connection; connections opened from another one (OPEN) or
// cloned (CLONE) are added to the set fsd waits on. Single-threaded: one
// request at a time.

#include <errno.h>
#include <fcntl.h>
#include <rustux/fs.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define MAX_CONNS 1000

struct node {
    uint32_t type;          // RX_FS_TYPE_FILE, _DIR or _CHR
    int dev;                // devices: DEV_NULL or DEV_ZERO
    uint32_t mode;
    int read_only;          // bootfs
    uint32_t fsid;          // which filesystem: 1 tmpfs, 2 bootfs, 3 devices (st_dev)
    char *name;
    struct node *parent;
    struct node *first, *next; // children (directories), siblings
    struct node *mount;     // a directory mounted here (followed on lookup)
    uint64_t ino;
    int64_t mtime;
    int linked;             // still in a directory
    int open;               // open file descriptions referring to it
    // File contents: tmpfs files own `data`; bootfs files point into the image
    char *data;
    const char *rodata;
    uint64_t size, cap;
};

// An open file description: shared by connections cloned from each other
struct ofd {
    struct node *node;
    uint64_t offset;
    int flags;              // O_* from open
    int refs;
    uint32_t dir_pos;       // READDIR position (entries returned so far)
};

struct conn {
    rx_handle_t ch;
    struct ofd *ofd;
};

static struct conn conns[MAX_CONNS];
static int nconns;
static uint64_t next_ino = 1;
// The boot image: its VMO and where we mapped it (GET_VMO)
static rx_handle_t image_vmo;
static const unsigned char *image_base;

static struct node *root;

static int64_t now(void) { return rx_clock_get_utc(); } // ns since 1970

static struct node *new_node(uint32_t type, const char *name, struct node *parent, uint32_t mode) {
    struct node *n = calloc(1, sizeof *n);
    if (!n) return 0;
    n->type = type;
    n->mode = mode;
    n->name = strdup(name);
    n->ino = next_ino++;
    n->mtime = now();
    n->fsid = 1;
    if (parent) {
        n->parent = parent;
        n->read_only = parent->read_only;
        n->fsid = parent->fsid;
        // Keep entries in creation order: append
        struct node **p = &parent->first;
        while (*p) p = &(*p)->next;
        *p = n;
        n->linked = 1;
        parent->mtime = n->mtime;
    }
    return n;
}

static void detach(struct node *n) {
    struct node **p = &n->parent->first;
    while (*p && *p != n) p = &(*p)->next;
    if (*p) *p = n->next;
    n->next = 0;
    n->parent->mtime = now();
    n->linked = 0;
}

static void maybe_free(struct node *n) {
    if (n->linked || n->open) return;
    free(n->data);
    free(n->name);
    free(n);
}

static struct node *follow(struct node *n) {
    while (n && n->mount) n = n->mount;
    return n;
}

static struct node *child(struct node *dir, const char *name, size_t len) {
    for (struct node *c = dir->first; c; c = c->next) {
        if (strlen(c->name) == len && !memcmp(c->name, name, len)) return follow(c);
    }
    return 0;
}

// Walk `path` (len bytes, '/'-separated, relative) from `dir`. If `parent`
// is not NULL, stop before the last component: *parent gets its directory
// and *last/*last_len the name. Returns 0 or a negative errno.
static int walk(struct node *dir, const char *path, size_t len, struct node **out,
                struct node **parent, const char **last, size_t *last_len) {
    struct node *n = follow(dir);
    size_t i = 0;
    while (i < len && path[i] == '/') i++;
    if (i == len) {
        if (parent) return -EEXIST; // no last component: it is the directory itself
        *out = n;
        return 0;
    }
    for (;;) {
        size_t j = i;
        while (j < len && path[j] != '/') j++;
        size_t k = j;
        while (k < len && path[k] == '/') k++;
        int final = k == len;
        if (j - i > 255) return -ENAMETOOLONG;
        if ((j - i == 1 && path[i] == '.') || (j - i == 2 && path[i] == '.' && path[i + 1] == '.')) return -EINVAL;
        if (n->type != RX_FS_TYPE_DIR) return -ENOTDIR;
        if (final && parent) {
            *parent = n;
            *last = path + i;
            *last_len = j - i;
            return 0;
        }
        struct node *c = child(n, path + i, j - i);
        if (!c) return -ENOENT;
        if (final) {
            *out = c;
            return 0;
        }
        n = c;
        i = k;
    }
}

static void attr_of(struct node *n, rx_fs_attr_t *a) {
    memset(a, 0, sizeof *a);
    a->type = n->type;
    a->mode = n->mode;
    a->size = n->size;
    a->ino = n->ino;
    a->nlink = n->type == RX_FS_TYPE_DIR ? 2 : 1;
    a->mtime_ns = n->mtime;
    a->read_only = (uint32_t)n->read_only;
    a->fsid = n->fsid;
}

// ---- connections ----

static int add_conn(rx_handle_t ch, struct ofd *o) {
    if (nconns == MAX_CONNS) {
        rx_handle_close(ch);
        return -EMFILE;
    }
    conns[nconns].ch = ch;
    conns[nconns++].ofd = o;
    o->refs++;
    return 0;
}

static struct ofd *new_ofd(struct node *n, int flags) {
    struct ofd *o = calloc(1, sizeof *o);
    if (!o) return 0;
    o->node = n;
    o->flags = flags;
    n->open++;
    return o;
}

static void drop_conn(int i) {
    struct ofd *o = conns[i].ofd;
    rx_handle_close(conns[i].ch);
    conns[i] = conns[--nconns];
    if (--o->refs == 0) {
        o->node->open--;
        maybe_free(o->node);
        free(o);
    }
}

// ---- requests ----

static char req[sizeof(rx_fs_hdr_t) + RX_FS_MAX_IO + 8192];
static char rep[sizeof(rx_fs_reply_t) + RX_FS_MAX_IO + 64];

static int set_size(struct node *n, uint64_t size) {
    if (size > n->cap) {
        uint64_t cap = n->cap ? n->cap : 256;
        while (cap < size) cap *= 2;
        char *d = realloc(n->data, cap);
        if (!d) return -ENOSPC;
        n->data = d;
        n->cap = cap;
    }
    if (size > n->size) memset(n->data + n->size, 0, size - n->size);
    n->size = size;
    n->mtime = now();
    return 0;
}

static int do_open(struct ofd *dir, const char *path, size_t plen, uint32_t flags, uint32_t mode,
                   rx_handle_t server, uint32_t *type) {
    struct node *n = 0, *parent;
    const char *last;
    size_t last_len;
    int r = walk(dir->node, path, plen, &n, 0, 0, 0);
    if (r == -ENOENT && (flags & O_CREAT)) {
        r = walk(dir->node, path, plen, 0, &parent, &last, &last_len);
        if (r < 0) return r;
        if (parent->read_only) return -EROFS;
        char name[256];
        memcpy(name, last, last_len);
        name[last_len] = 0;
        n = new_node(RX_FS_TYPE_FILE, name, parent, mode & 07777);
        if (!n) return -ENOSPC;
    } else if (r < 0) {
        return r;
    } else if ((flags & O_CREAT) && (flags & O_EXCL)) {
        return -EEXIST;
    }
    int writing = (flags & O_ACCMODE) != O_RDONLY;
    if (n->type == RX_FS_TYPE_DIR && writing) return -EISDIR;
    if ((flags & O_DIRECTORY) && n->type != RX_FS_TYPE_DIR) return -ENOTDIR;
    if ((writing || (flags & O_TRUNC)) && n->read_only) return -EROFS;
    if ((flags & O_TRUNC) && writing && n->type == RX_FS_TYPE_FILE) set_size(n, 0);
    struct ofd *o = new_ofd(n, (int)flags);
    if (!o) return -ENOMEM;
    r = add_conn(server, o);
    if (r < 0) {
        n->open--;
        free(o);
        return r;
    }
    *type = n->type;
    return 0;
}

#define DEV_NULL 1
#define DEV_ZERO 2

static int do_read(struct ofd *o, uint32_t count, char *out, size_t *len) {
    struct node *n = o->node;
    if (n->type == RX_FS_TYPE_DIR) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_WRONLY) return -EBADF;
    if (count > RX_FS_MAX_IO) count = RX_FS_MAX_IO;
    if (n->type == RX_FS_TYPE_CHR) {
        *len = n->dev == DEV_ZERO ? count : 0;
        memset(out, 0, *len);
        return 0;
    }
    uint64_t k = o->offset >= n->size ? 0 : n->size - o->offset;
    if (k > count) k = count;
    memcpy(out, (n->rodata ? n->rodata : n->data) + o->offset, k);
    o->offset += k;
    *len = k;
    return 0;
}

static int do_write(struct ofd *o, const char *data, size_t k, uint64_t *wrote) {
    struct node *n = o->node;
    if (n->type == RX_FS_TYPE_DIR) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_RDONLY) return -EBADF;
    if (n->type == RX_FS_TYPE_CHR) { // both devices take everything
        *wrote = k;
        return 0;
    }
    if (n->read_only) return -EROFS;
    if (o->flags & O_APPEND) o->offset = n->size;
    if (o->offset + k > n->size) {
        int r = set_size(n, o->offset + k);
        if (r < 0) return r;
    }
    memcpy(n->data + o->offset, data, k);
    o->offset += k;
    n->mtime = now();
    *wrote = k;
    return 0;
}

static int do_readdir(struct ofd *o, uint32_t max, char *out, size_t *len) {
    struct node *d = o->node;
    if (d->type != RX_FS_TYPE_DIR) return -ENOTDIR;
    if (max > RX_FS_MAX_IO) max = RX_FS_MAX_IO;
    struct node *c = d->first;
    for (uint32_t i = 0; c && i < o->dir_pos; i++) c = c->next;
    size_t used = 0;
    for (; c; c = c->next) {
        struct node *t = follow(c);
        size_t nl = strlen(c->name);
        if (used + 10 + nl > max) break;
        memcpy(out + used, &t->ino, 8);
        out[used + 8] = (char)t->type;
        out[used + 9] = (char)nl;
        memcpy(out + used + 10, c->name, nl);
        used += 10 + nl;
        o->dir_pos++;
    }
    *len = used;
    return 0;
}

static int is_ancestor(struct node *a, struct node *b) {
    for (; b; b = b->parent) if (b == a) return 1;
    return 0;
}

static int do_rename(struct ofd *dir, const char *a, size_t alen, const char *b, size_t blen) {
    struct node *src, *src_dir, *dst_dir, *existing = 0;
    const char *sn, *dn;
    size_t snl, dnl;
    int r = walk(dir->node, a, alen, 0, &src_dir, &sn, &snl);
    if (r < 0) return r;
    src = child(src_dir, sn, snl);
    if (!src) return -ENOENT;
    r = walk(dir->node, b, blen, 0, &dst_dir, &dn, &dnl);
    if (r < 0) return r;
    if (src_dir->read_only || dst_dir->read_only) return -EROFS;
    // The entry itself, not what is mounted on it
    for (struct node *c = src_dir->first; c; c = c->next) {
        if (strlen(c->name) == snl && !memcmp(c->name, sn, snl)) { src = c; break; }
    }
    if (src->mount) return -EBUSY;
    if (src->type == RX_FS_TYPE_DIR && is_ancestor(src, dst_dir)) return -EINVAL;
    for (struct node *c = dst_dir->first; c; c = c->next) {
        if (strlen(c->name) == dnl && !memcmp(c->name, dn, dnl)) { existing = c; break; }
    }
    if (existing == src) return 0;
    if (existing) {
        if (existing->mount) return -EBUSY;
        if (existing->type == RX_FS_TYPE_DIR && src->type != RX_FS_TYPE_DIR) return -EISDIR;
        if (existing->type != RX_FS_TYPE_DIR && src->type == RX_FS_TYPE_DIR) return -ENOTDIR;
        if (existing->type == RX_FS_TYPE_DIR && existing->first) return -ENOTEMPTY;
        detach(existing);
        maybe_free(existing);
    }
    char *name = malloc(dnl + 1);
    if (!name) return -ENOMEM;
    memcpy(name, dn, dnl);
    name[dnl] = 0;
    detach(src);
    free(src->name);
    src->name = name;
    src->parent = dst_dir;
    struct node **p = &dst_dir->first;
    while (*p) p = &(*p)->next;
    *p = src;
    src->linked = 1;
    dst_dir->mtime = now();
    return 0;
}

static int do_unlink(struct ofd *dir, const char *path, size_t plen, uint32_t flags) {
    struct node *parent, *n = 0;
    const char *last;
    size_t last_len;
    int r = walk(dir->node, path, plen, 0, &parent, &last, &last_len);
    if (r < 0) return r;
    for (struct node *c = parent->first; c; c = c->next) {
        if (strlen(c->name) == last_len && !memcmp(c->name, last, last_len)) { n = c; break; }
    }
    if (!n) return -ENOENT;
    if (parent->read_only) return -EROFS;
    if (n->mount) return -EBUSY;
    if (flags & RX_FS_UNLINK_DIR) {
        if (n->type != RX_FS_TYPE_DIR) return -ENOTDIR;
        if (n->first) return -ENOTEMPTY;
    } else if (n->type == RX_FS_TYPE_DIR) {
        return -EISDIR;
    }
    detach(n);
    maybe_free(n);
    return 0;
}

// Handle one request on connection `i`; returns 0, or -1 if the connection
// was dropped
static int serve(int i) {
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_status_t s = rx_channel_read(conns[i].ch, 0, req, &h, sizeof req, 1, &nb, &nh);
    if (s == RX_ERR_SHOULD_WAIT) return 0;
    if (s != RX_OK) {
        drop_conn(i);
        return -1;
    }
    if (nb < sizeof(rx_fs_hdr_t)) {
        if (nh) rx_handle_close(h);
        return 0;
    }
    rx_fs_hdr_t hdr;
    memcpy(&hdr, req, sizeof hdr);
    const char *body = req + sizeof hdr;
    size_t blen = nb - sizeof hdr;
    struct ofd *o = conns[i].ofd;
    char *out = rep + sizeof(rx_fs_reply_t);
    size_t olen = 0;
    int st = 0;
    uint32_t u32;
    rx_handle_t reply_handle = 0; // sent with the reply (GET_VMO)

    switch (hdr.op) {
    case RX_FS_OP_OPEN: {
        if (blen < 8 || !nh) { st = -EINVAL; break; }
        uint32_t f[2];
        memcpy(f, body, 8);
        if (o->node->type != RX_FS_TYPE_DIR) { st = -ENOTDIR; break; }
        st = do_open(o, body + 8, blen - 8, f[0], f[1], h, &u32);
        if (st == 0) {
            h = 0; // the new connection owns it
            memcpy(out, &u32, 4);
            olen = 4;
        }
        o = conns[i].ofd; // conns[] may have moved? no: add_conn appends
        break;
    }
    case RX_FS_OP_CLONE:
        if (!nh) { st = -EINVAL; break; }
        st = add_conn(h, o);
        h = 0;
        break;
    case RX_FS_OP_STAT:
        attr_of(o->node, (rx_fs_attr_t *)out);
        olen = sizeof(rx_fs_attr_t);
        break;
    case RX_FS_OP_STAT_AT: {
        struct node *n;
        st = walk(o->node, body, blen, &n, 0, 0, 0);
        if (st == 0) {
            attr_of(n, (rx_fs_attr_t *)out);
            olen = sizeof(rx_fs_attr_t);
        }
        break;
    }
    case RX_FS_OP_READ:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_read(o, u32, out, &olen);
        break;
    case RX_FS_OP_WRITE: {
        uint64_t wrote = 0;
        st = do_write(o, body, blen > RX_FS_MAX_IO ? RX_FS_MAX_IO : blen, &wrote);
        memcpy(out, &wrote, 8);
        olen = 8;
        break;
    }
    case RX_FS_OP_SEEK: {
        if (blen < 12) { st = -EINVAL; break; }
        int64_t off;
        memcpy(&off, body, 8);
        memcpy(&u32, body + 8, 4);
        int64_t base = u32 == SEEK_SET ? 0 : u32 == SEEK_CUR ? (int64_t)o->offset
                     : u32 == SEEK_END ? (int64_t)o->node->size : -1;
        if (base < 0 || base + off < 0) { st = -EINVAL; break; }
        o->offset = (uint64_t)(base + off);
        memcpy(out, &o->offset, 8);
        olen = 8;
        break;
    }
    case RX_FS_OP_TRUNCATE: {
        uint64_t len;
        if (blen < 8) { st = -EINVAL; break; }
        memcpy(&len, body, 8);
        if (o->node->type == RX_FS_TYPE_DIR) st = -EISDIR;
        else if (o->node->read_only) st = -EROFS;
        else if ((o->flags & O_ACCMODE) == O_RDONLY) st = -EBADF;
        else if (o->node->type == RX_FS_TYPE_CHR) st = 0;
        else st = set_size(o->node, len);
        break;
    }
    case RX_FS_OP_GET_VMO: {
        rx_handle_t dup;
        if (o->node->type != RX_FS_TYPE_FILE || !o->node->rodata || !image_vmo) { st = -ENOTSUP; break; }
        if (rx_handle_duplicate(image_vmo, RX_RIGHT_SAME_RIGHTS, &dup) != RX_OK) { st = -ENOMEM; break; }
        uint64_t v[2] = { (uint64_t)((const unsigned char *)o->node->rodata - image_base), o->node->size };
        memcpy(out, v, sizeof v);
        olen = sizeof v;
        reply_handle = dup;
        st = 0;
        break;
    }
    case RX_FS_OP_SETATTR: {
        rx_fs_setattr_t sa;
        if (blen < sizeof sa) { st = -EINVAL; break; }
        memcpy(&sa, body, sizeof sa);
        if (o->node->read_only) { st = -EROFS; break; }
        if (sa.mask & RX_FS_SETATTR_MODE) o->node->mode = sa.mode & 07777;
        if (sa.mask & RX_FS_SETATTR_MTIME) o->node->mtime = sa.mtime_ns;
        st = 0;
        break;
    }
    case RX_FS_OP_READDIR:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_readdir(o, u32, out, &olen);
        break;
    case RX_FS_OP_REWIND:
        o->dir_pos = 0;
        break;
    case RX_FS_OP_MKDIR: {
        struct node *parent;
        const char *last;
        size_t last_len;
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = walk(o->node, body + 4, blen - 4, 0, &parent, &last, &last_len);
        if (st < 0) break;
        if (child(parent, last, last_len)) { st = -EEXIST; break; }
        if (parent->read_only) { st = -EROFS; break; }
        char name[256];
        memcpy(name, last, last_len);
        name[last_len] = 0;
        if (!new_node(RX_FS_TYPE_DIR, name, parent, u32 & 07777)) st = -ENOSPC;
        break;
    }
    case RX_FS_OP_UNLINK:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_unlink(o, body + 4, blen - 4, u32);
        break;
    case RX_FS_OP_RENAME:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        if (u32 > blen - 4) { st = -EINVAL; break; }
        st = do_rename(o, body + 4, u32, body + 4 + u32, blen - 4 - u32);
        break;
    case RX_FS_OP_SYNC:
        break; // all in memory
    default:
        st = -ENOSYS;
        break;
    }
    if (h) rx_handle_close(h);
    rx_fs_reply_t r = { hdr.txid, st };
    memcpy(rep, &r, sizeof r);
    if (rx_channel_write(conns[i].ch, 0, rep, (uint32_t)(sizeof r + (st < 0 ? 0 : olen)),
                         reply_handle ? &reply_handle : 0, reply_handle ? 1 : 0) != RX_OK && reply_handle)
        rx_handle_close(reply_handle);
    return 0;
}

// ---- boot image ----

static uint32_t rd32(const unsigned char *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

static struct node *bootfs_load(rx_handle_t vmo) {
    uint64_t size = 0, addr = 0;
    if (rx_vmo_get_size(vmo, &size) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ, 0, vmo, 0, (size + 4095) & ~4095ull, &addr) != RX_OK) return 0;
    const unsigned char *img = (const unsigned char *)addr;
    image_vmo = vmo;
    image_base = img;
    struct node *top = new_node(RX_FS_TYPE_DIR, "boot", 0, 0555);
    top->read_only = 1;
    top->fsid = 2;
    if (size < 12 || rd32(img) != 0x52555458) return top;
    uint32_t count = rd32(img + 4), table = rd32(img + 8);
    for (uint32_t i = 0; i < count && table + 16ull * (i + 1) <= size; i++) {
        const unsigned char *e = img + table + 16 * i;
        uint32_t noff = rd32(e), doff = rd32(e + 4), len = rd32(e + 8);
        if (noff >= size || (uint64_t)doff + len > size) continue;
        const char *name = (const char *)img + noff;
        // Make the directories along the path, then the file
        struct node *d = top;
        const char *p = name;
        for (const char *slash; (slash = strchr(p, '/')); p = slash + 1) {
            struct node *c = child(d, p, (size_t)(slash - p));
            if (!c) {
                char part[256];
                size_t k = (size_t)(slash - p) < 255 ? (size_t)(slash - p) : 255;
                memcpy(part, p, k);
                part[k] = 0;
                c = new_node(RX_FS_TYPE_DIR, part, d, 0555);
            }
            d = c;
        }
        // Programs in bin/ are executable
        struct node *f = new_node(RX_FS_TYPE_FILE, p, d, !strncmp(name, "bin/", 4) ? 0555 : 0444);
        f->rodata = (const char *)img + doff;
        f->size = len;
    }
    return top;
}

int main(void) {
    rx_handle_t bootfs = rx_take_startup_handle(RX_PA_VMO_BOOTFS);
    rx_handle_t first = rx_take_startup_handle(RX_PA_USER0);
    if (!first) {
        fprintf(stderr, "fsd: no connection to serve\n");
        return 1;
    }
    root = new_node(RX_FS_TYPE_DIR, "", 0, 0755);
    new_node(RX_FS_TYPE_DIR, "tmp", root, 01777);
    new_node(RX_FS_TYPE_DIR, "root", root, 0700);
    struct node *boot_top = bootfs ? bootfs_load(bootfs) : 0;
    if (boot_top) {
        struct node *m = new_node(RX_FS_TYPE_DIR, "boot", root, 0555);
        m->mount = boot_top;
        boot_top->parent = root;
        struct node *bin = child(boot_top, "bin", 3);
        if (bin) {
            struct node *b = new_node(RX_FS_TYPE_DIR, "bin", root, 0555);
            b->mount = bin;
        }
        struct node *usr = child(boot_top, "usr", 3);
        if (usr) {
            struct node *u = new_node(RX_FS_TYPE_DIR, "usr", root, 0555);
            u->mount = usr;
        }
        // /etc: a writable copy of the boot image's etc/
        struct node *etc = new_node(RX_FS_TYPE_DIR, "etc", root, 0755);
        struct node *src = child(boot_top, "etc", 3);
        for (struct node *f = src ? src->first : 0; f; f = f->next) {
            if (f->type != RX_FS_TYPE_FILE) continue;
            struct node *c = new_node(RX_FS_TYPE_FILE, f->name, etc, 0644);
            if (c && set_size(c, f->size) == 0) memcpy(c->data, f->rodata, f->size);
        }
    }
    struct node *dev = new_node(RX_FS_TYPE_DIR, "dev", root, 0755);
    dev->read_only = 1; // no new files here
    dev->fsid = 3;
    struct node *null = new_node(RX_FS_TYPE_CHR, "null", dev, 0666);
    struct node *zero = new_node(RX_FS_TYPE_CHR, "zero", dev, 0666);
    // Writable themselves (the directory stays read-only)
    if (null) { null->dev = DEV_NULL; null->read_only = 0; }
    if (zero) { zero->dev = DEV_ZERO; zero->read_only = 0; }
    add_conn(first, new_ofd(root, O_RDONLY | O_DIRECTORY));

    static rx_wait_item_t items[MAX_CONNS];
    for (;;) {
        int n = nconns;
        for (int i = 0; i < n; i++) {
            items[i].handle = conns[i].ch;
            items[i].waitfor = RX_CHANNEL_READABLE | RX_PEER_CLOSED;
            items[i].pending = 0;
        }
        if (n == 0) return 0; // nobody left to serve
        if (rx_object_wait_many(items, (uint64_t)n, RX_TIME_INFINITE) != RX_OK) continue;
        // Serve from the end so drop_conn (which moves the last entry into
        // the freed slot) doesn't disturb entries not yet visited
        for (int i = n - 1; i >= 0; i--) {
            if (items[i].pending & RX_CHANNEL_READABLE) {
                if (serve(i) < 0) continue;
            } else if (items[i].pending & RX_PEER_CLOSED) {
                drop_conn(i);
            }
        }
    }
}

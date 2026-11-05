// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Memory allocator. Small blocks (up to 4 KiB with their header) come in
// power-of-two size classes carved from 256 KiB arenas; freed blocks go on
// a list per class. Larger blocks get a VMO of their own, mapped through
// the root VMAR and unmapped on free. One lock guards the lists and the
// current arena. Blocks are 16-byte aligned; posix_memalign and friends
// give larger alignments by over-allocating.

#include "libc.h"
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define HDR 16u
#define MIN_SHIFT 5u            // 32-byte blocks
#define MAX_SHIFT 12u           // 4096-byte blocks
#define NCLASS (MAX_SHIFT - MIN_SHIFT + 1)
#define ARENA (256u * 1024u)
#define LARGE 0xFFu
#define ALIGNED 0xFEu           // header in front of an aligned block: size = distance back
#define MAGIC 0x52584D41u       // "RXMA"

struct hdr {
    uint32_t magic;
    uint32_t cls;               // size class index, or LARGE
    uint64_t size;              // block size (small) or mapping size (large)
};

struct free_block { struct free_block *next; };

static struct free_block *free_list[NCLASS];
static char *arena_next, *arena_end;
static volatile int lock;

void __malloc_init(void) {}

// Map `size` bytes (a multiple of 4096) of fresh zeroed memory
static void *map_pages(uint64_t size) {
    rx_handle_t vmo;
    uint64_t addr = 0;
    if (!rx_vmar_root || rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    rx_status_t r = rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr);
    rx_handle_close(vmo);
    return r == RX_OK ? (void *)addr : 0;
}

void *malloc(size_t n) {
    if (n > (1ull << 40)) {
        errno = ENOMEM;
        return 0;
    }
    size_t need = n + HDR;
    if (need > (1u << MAX_SHIFT)) {
        uint64_t size = (need + 4095) & ~4095ull;
        struct hdr *h = map_pages(size);
        if (!h) { errno = ENOMEM; return 0; }
        h->magic = MAGIC;
        h->cls = LARGE;
        h->size = size;
        return (char *)h + HDR;
    }
    uint32_t cls = 0;
    while ((1u << (MIN_SHIFT + cls)) < need) cls++;
    uint32_t bsize = 1u << (MIN_SHIFT + cls);
    struct hdr *h;
    __lock(&lock);
    if (free_list[cls]) {
        h = (struct hdr *)free_list[cls];
        free_list[cls] = free_list[cls]->next;
    } else {
        if (arena_end - arena_next < (long)bsize) {
            char *a = map_pages(ARENA);
            if (!a) { __unlock(&lock); errno = ENOMEM; return 0; }
            // The rest of the old arena is lost; at most 4 KiB per arena
            arena_next = a;
            arena_end = a + ARENA;
        }
        h = (struct hdr *)arena_next;
        arena_next += bsize;
    }
    __unlock(&lock);
    h->magic = MAGIC;
    h->cls = cls;
    h->size = bsize;
    return (char *)h + HDR;
}

static struct hdr *header(void *p) {
    struct hdr *h = (struct hdr *)((char *)p - HDR);
    if (h->magic != MAGIC) {
        static const char msg[] = "libc: free/realloc of a bad pointer\n";
        write(2, msg, sizeof msg - 1);
        abort();
    }
    return h;
}

void free(void *p) {
    if (!p) return;
    struct hdr *h = header(p);
    if (h->cls == ALIGNED) {
        h->magic = 0;
        free((char *)p - h->size);
        return;
    }
    h->magic = 0;
    if (h->cls == LARGE) {
        rx_vmar_unmap(rx_vmar_root, (uint64_t)h, h->size);
        return;
    }
    struct free_block *b = (struct free_block *)h;
    __lock(&lock);
    b->next = free_list[h->cls];
    free_list[h->cls] = b;
    __unlock(&lock);
}

// Bytes usable from p on
static size_t usable(void *p, struct hdr *h) {
    if (h->cls == ALIGNED) {
        char *orig = (char *)p - h->size;
        return usable(orig, header(orig)) - h->size;
    }
    return h->size - HDR;
}

size_t malloc_usable_size(void *p) { return p ? usable(p, header(p)) : 0; }

void *memalign(size_t align, size_t n) {
    if (align <= HDR) return malloc(n);
    if (align & (align - 1)) { errno = EINVAL; return 0; }
    if (n > (1ull << 40)) { errno = ENOMEM; return 0; }
    char *p = malloc(n + align + HDR);
    if (!p) return 0;
    char *q = (char *)(((uintptr_t)p + HDR + align - 1) & ~(uintptr_t)(align - 1));
    struct hdr *h = (struct hdr *)(q - HDR);
    h->magic = MAGIC;
    h->cls = ALIGNED;
    h->size = (uint64_t)(q - p);
    return q;
}

int posix_memalign(void **out, size_t align, size_t n) {
    if (align < sizeof(void *) || (align & (align - 1))) return EINVAL;
    void *p = memalign(align, n);
    if (!p) return ENOMEM;
    *out = p;
    return 0;
}

void *aligned_alloc(size_t align, size_t n) { return memalign(align, n); }

void *calloc(size_t n, size_t size) {
    if (size && n > ((size_t)-1) / size) {
        errno = ENOMEM;
        return 0;
    }
    void *p = malloc(n * size);
    if (p) memset(p, 0, n * size);
    return p;
}

void *realloc(void *p, size_t n) {
    if (!p) return malloc(n);
    if (!n) { free(p); return 0; }
    struct hdr *h = header(p);
    size_t have = usable(p, h);
    if (n <= have) return p;
    void *q = malloc(n);
    if (!q) return 0;
    memcpy(q, p, have);
    free(p);
    return q;
}

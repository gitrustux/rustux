// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// virtio-gpu: the display service on QEMU's, KVM's and Proxmox's virtual
// GPU (DESKTOP.md step F8, docs/DISPLAY.md). 2D only: the back buffer is
// a host resource backed by guest memory; PRESENT transfers the damaged
// rectangles to the host and flushes them; the pointer is the host's
// cursor; when the host's window (or a VNC or SPICE client) changes size,
// the screen does too; several heads (scanouts) are one wide screen, side
// by side. The protocol server is shared with display (displaysvc.h).
//
// devmgr starts it for 1af4:1050 (/etc/drivers, "display": instead of
// /bin/display), with the device channel (RX_PA_USER0) and, when UEFI
// left a framebuffer (virtio-vga: the VGA part of the same device), the
// framebuffer VMO (RX_PA_USER1), the claim (RX_PA_USER2) and its geometry:
//
//   virtio-gpu BDF [WIDTH HEIGHT STRIDE RED GREEN BLUE OFFSET]
//
// The text console is the kernel's, on that framebuffer: Ctrl+Alt+F1
// turns the virtio scanout off, which shows the VGA framebuffer again.
// Without one (virtio-gpu-pci) the console cannot be shown.
// Spec: "Virtual I/O Device (VIRTIO) Version 1.2", sections 4.1 and 5.7.

#include <rustux/driver.h>
#include <time.h>

#include "displaysvc.h"

// ---- virtio PCI (as virtio-blk) ----

#define CAP_COMMON 1
#define CAP_NOTIFY 2
#define CAP_DEVICE 4

#define C_DFSELECT  0x00
#define C_DF        0x04
#define C_GFSELECT  0x08
#define C_GF        0x0C
#define C_MSIX_CFG  0x10
#define C_STATUS    0x14
#define C_QSELECT   0x16
#define C_QSIZE     0x18
#define C_QMSIX     0x1A
#define C_QENABLE   0x1C
#define C_QNOTIFY   0x1E
#define C_QDESC     0x20
#define C_QDRIVER   0x28
#define C_QDEVICE   0x30

#define S_ACK       1
#define S_DRIVER    2
#define S_DRIVER_OK 4
#define S_FEAT_OK   8
#define S_FAILED    128
#define F_VERSION_1 (1ull << 32)
#define NO_VECTOR   0xFFFF

#define D_NEXT  1
#define D_WRITE 2

struct desc { uint64_t addr; uint32_t len; uint16_t flags, next; };

struct vq {
    volatile struct desc *desc;
    volatile uint16_t *avail;   // flags, idx, ring[size]
    volatile uint16_t *used;    // flags, idx, then {u32 id, u32 len}[size]
    uint16_t size, last_used;
    volatile uint8_t *notify;
    rx_dma_buf_t mem;
};

static const char *where = "?";
static rx_handle_t dev, bti;
static volatile uint8_t *common, *notify_base, *devcfg;
static uint32_t notify_mult;
static struct vq ctrlq, cursorq;
static rx_dma_buf_t cmdbuf;       // requests at 0, the reply at REPLY_AT
#define CMD_SIZE (64u * 1024u)
#define REPLY_AT (60u * 1024u)

static uint8_t r8(volatile uint8_t *b, uint32_t o) { return b[o]; }
static uint16_t r16(volatile uint8_t *b, uint32_t o) { return *(volatile uint16_t *)(b + o); }
static uint32_t r32(volatile uint8_t *b, uint32_t o) { return *(volatile uint32_t *)(b + o); }
static void w8(volatile uint8_t *b, uint32_t o, uint8_t v) { b[o] = v; }
static void w16(volatile uint8_t *b, uint32_t o, uint16_t v) { *(volatile uint16_t *)(b + o) = v; }
static void w32(volatile uint8_t *b, uint32_t o, uint32_t v) { *(volatile uint32_t *)(b + o) = v; }
static void w64(volatile uint8_t *b, uint32_t o, uint64_t v) { w32(b, o, (uint32_t)v); w32(b, o + 4, (uint32_t)(v >> 32)); }

static void sleep_us(long us) {
    struct timespec ts = { us / 1000000, (us % 1000000) * 1000 };
    nanosleep(&ts, 0);
}

static int fail(const char *what) {
    fprintf(stderr, "virtio-gpu %s: %s\n", where, what);
    if (common) w8(common, C_STATUS, r8(common, C_STATUS) | S_FAILED);
    return 1;
}

static volatile uint8_t *cap_target(uint8_t cap) {
    static volatile uint8_t *bars[6];
    uint8_t bar = (uint8_t)rx_drv_cfg_read(dev, cap + 4, 1);
    uint32_t off = rx_drv_cfg_read(dev, cap + 8, 4);
    if (bar > 5) return 0;
    if (!bars[bar]) bars[bar] = rx_drv_map_bar(dev, bar, 0);
    return bars[bar] ? bars[bar] + off : 0;
}

static int vq_setup(uint16_t index, struct vq *q) {
    w16(common, C_QSELECT, index);
    uint16_t max = r16(common, C_QSIZE);
    if (!max) return -1;
    q->size = max < 8 ? max : 8;
    w16(common, C_QSIZE, q->size);
    if (rx_dma_alloc(bti, 2 * 4096, 0, &q->mem) < 0) return -1;
    q->desc = (volatile struct desc *)q->mem.va;
    q->avail = (volatile uint16_t *)(q->mem.va + 1024);
    q->used = (volatile uint16_t *)(q->mem.va + 4096);
    w64(common, C_QDESC, q->mem.pa);
    w64(common, C_QDRIVER, q->mem.pa + 1024);
    w64(common, C_QDEVICE, q->mem.pa + 4096);
    w16(common, C_QMSIX, NO_VECTOR);
    q->notify = notify_base + (uint32_t)r16(common, C_QNOTIFY) * notify_mult;
    w16(common, C_QENABLE, 1);
    return 0;
}

// A request of len bytes (in cmdbuf) and, if rlen, room for the reply;
// waits until the device has used it (polling: commands are quick)
static int submit(struct vq *q, uint32_t len, uint32_t rlen) {
    q->desc[0] = (struct desc){ cmdbuf.pa, len, (uint16_t)(rlen ? D_NEXT : 0), 1 };
    if (rlen) q->desc[1] = (struct desc){ cmdbuf.pa + REPLY_AT, rlen, D_WRITE, 0 };
    uint16_t idx = q->avail[1];
    q->avail[2 + idx % q->size] = 0;
    rx_dma_barrier();
    q->avail[1] = (uint16_t)(idx + 1);
    rx_dma_barrier();
    w16(q->notify, 0, (uint16_t)(q == &cursorq));
    for (long spin = 0; q->used[1] == q->last_used; spin++) {
        if (spin > 200) sleep_us(spin < 1200 ? 20 : 1000);
        if (spin > 6000) return -ETIMEDOUT;
    }
    rx_dma_barrier();
    q->last_used++;
    return 0;
}

// ---- virtio-gpu commands (5.7.6) ----

#define CMD_GET_DISPLAY_INFO     0x0100
#define CMD_RESOURCE_CREATE_2D   0x0101
#define CMD_RESOURCE_UNREF       0x0102
#define CMD_SET_SCANOUT          0x0103
#define CMD_RESOURCE_FLUSH       0x0104
#define CMD_TRANSFER_TO_HOST_2D  0x0105
#define CMD_RESOURCE_ATTACH_BACKING 0x0106
#define CMD_RESOURCE_DETACH_BACKING 0x0107
#define CMD_UPDATE_CURSOR        0x0300
#define CMD_MOVE_CURSOR          0x0301
#define RESP_OK_NODATA           0x1100
#define RESP_OK_DISPLAY_INFO     0x1101

#define FMT_B8G8R8A8 1  // bytes B, G, R, A: the u32 0xAARRGGBB
#define FMT_B8G8R8X8 2  // 0x00RRGGBB, as our back buffers

#define EVENT_DISPLAY 1
#define MAX_SCANOUTS 16

struct hdr { uint32_t type, flags; uint64_t fence; uint32_t ctx, ring_pad; };
struct rect { uint32_t x, y, w, h; };

static uint8_t *req(void) { return (uint8_t *)cmdbuf.va; }

static void put_hdr(uint32_t type) {
    struct hdr h = { type, 0, 0, 0, 0 };
    memcpy(req(), &h, sizeof h);
}

static uint32_t reply_type(void) {
    uint32_t t;
    memcpy(&t, (const void *)(cmdbuf.va + REPLY_AT), 4);
    return t;
}

// A control command of len bytes: 0 if the device answered OK_NODATA
static int ctrl(uint32_t len) {
    if (submit(&ctrlq, len, sizeof(struct hdr)) < 0) return -ETIMEDOUT;
    return reply_type() == RESP_OK_NODATA ? 0 : -EIO;
}

struct mode { struct rect r; uint32_t enabled, flags; };

static int display_info(struct mode *modes) {
    put_hdr(CMD_GET_DISPLAY_INFO);
    uint32_t rlen = sizeof(struct hdr) + MAX_SCANOUTS * sizeof(struct mode);
    if (submit(&ctrlq, sizeof(struct hdr), rlen) < 0 || reply_type() != RESP_OK_DISPLAY_INFO) return -EIO;
    memcpy(modes, (const void *)(cmdbuf.va + REPLY_AT + sizeof(struct hdr)), MAX_SCANOUTS * sizeof(struct mode));
    return 0;
}

static int resource_create(uint32_t id, uint32_t format, uint32_t w, uint32_t h) {
    put_hdr(CMD_RESOURCE_CREATE_2D);
    uint32_t b[4] = { id, format, w, h };
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    return ctrl(sizeof(struct hdr) + sizeof b);
}

static void resource_unref(uint32_t id) {
    put_hdr(CMD_RESOURCE_DETACH_BACKING);
    uint32_t b[2] = { id, 0 };
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    ctrl(sizeof(struct hdr) + sizeof b);
    put_hdr(CMD_RESOURCE_UNREF);
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    ctrl(sizeof(struct hdr) + sizeof b);
}

// The pages at addrs[0..n) (one per 4 KiB), merged into runs
static int attach_backing(uint32_t id, const uint64_t *addrs, size_t n) {
    put_hdr(CMD_RESOURCE_ATTACH_BACKING);
    uint8_t *p = req() + sizeof(struct hdr) + 8;
    uint32_t entries = 0;
    size_t max = (REPLY_AT - sizeof(struct hdr) - 8) / 16;
    for (size_t i = 0; i < n;) {
        size_t j = i + 1;
        while (j < n && addrs[j] == addrs[j - 1] + 4096) j++;
        if (entries == max) return -E2BIG;
        uint64_t addr = addrs[i];
        uint32_t len = (uint32_t)((j - i) * 4096), pad = 0;
        memcpy(p, &addr, 8);
        memcpy(p + 8, &len, 4);
        memcpy(p + 12, &pad, 4);
        p += 16;
        entries++;
        i = j;
    }
    uint32_t b[2] = { id, entries };
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    return ctrl(sizeof(struct hdr) + 8 + entries * 16);
}

static int set_scanout(uint32_t scanout, uint32_t id, struct rect r) {
    put_hdr(CMD_SET_SCANOUT);
    uint32_t b[6] = { r.x, r.y, r.w, r.h, scanout, id };
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    return ctrl(sizeof(struct hdr) + sizeof b);
}

static int transfer(uint32_t id, struct rect r, uint64_t offset) {
    put_hdr(CMD_TRANSFER_TO_HOST_2D);
    uint8_t *p = req() + sizeof(struct hdr);
    memcpy(p, &r, sizeof r);
    memcpy(p + 16, &offset, 8);
    uint32_t b[2] = { id, 0 };
    memcpy(p + 24, b, sizeof b);
    return ctrl(sizeof(struct hdr) + 32);
}

static int flush(uint32_t id, struct rect r) {
    put_hdr(CMD_RESOURCE_FLUSH);
    uint8_t *p = req() + sizeof(struct hdr);
    memcpy(p, &r, sizeof r);
    uint32_t b[2] = { id, 0 };
    memcpy(p + 16, b, sizeof b);
    return ctrl(sizeof(struct hdr) + 24);
}

// ---- the screen ----

static struct mode modes[MAX_SCANOUTS];
static struct { uint32_t scanout; struct rect r; } heads[MAX_SCANOUTS]; // r: where it is in the resource
static int nheads;
static uint32_t res_id = 10;         // the back buffer's resource (a new id at each size)
static rx_handle_t back_pmt;
static uint64_t back_size;

// The fallback when the host tells nothing (no head enabled)
static uint32_t fb_width, fb_height;

// Lay the enabled heads side by side: the screen's size
static void layout(uint32_t *w, uint32_t *h) {
    nheads = 0;
    uint32_t x = 0, maxh = 0;
    for (uint32_t i = 0; i < MAX_SCANOUTS; i++) {
        if (!modes[i].enabled || !modes[i].r.w || !modes[i].r.h) continue;
        heads[nheads].scanout = i;
        heads[nheads].r = (struct rect){ x, 0, modes[i].r.w, modes[i].r.h };
        x += modes[i].r.w;
        if (modes[i].r.h > maxh) maxh = modes[i].r.h;
        nheads++;
    }
    if (!nheads) {
        heads[0].scanout = 0;
        heads[0].r = (struct rect){ 0, 0, fb_width ? fb_width : 1024, fb_height ? fb_height : 768 };
        nheads = 1;
        x = heads[0].r.w;
        maxh = heads[0].r.h;
    }
    *w = x;
    *h = maxh;
    dsp_primary_w = heads[0].r.w;
    dsp_primary_h = heads[0].r.h;
}

// A back buffer and its resource for w x h: 0 or a negative errno. The
// old ones stay until the new are set up
static int make_back(uint32_t w, uint32_t h) {
    uint64_t size = ((uint64_t)w * h * 4 + 4095) & ~4095ull;
    size_t pages = (size_t)(size / 4096);
    rx_handle_t vmo, pmt;
    uint64_t addr = 0;
    uint64_t *addrs = malloc(pages * sizeof *addrs);
    if (!addrs) return -ENOMEM;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) {
        free(addrs);
        return -ENOMEM;
    }
    if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE | RX_VM_MAP_RANGE, 0, vmo, 0, size, &addr) != RX_OK ||
        rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE, vmo, 0, size, addrs, &pmt) != RX_OK) {
        free(addrs);
        rx_handle_close(vmo);
        return -ENOMEM;
    }
    uint32_t id = res_id + 1;
    int r = resource_create(id, FMT_B8G8R8X8, w, h);
    if (r == 0) r = attach_backing(id, addrs, pages);
    free(addrs);
    if (r < 0) {
        rx_pmt_unpin(pmt);
        rx_vmar_unmap(rx_vmar_root, addr, size);
        rx_handle_close(vmo);
        return r;
    }
    // Out with the old
    if (dsp_back_vmo) {
        resource_unref(res_id);
        rx_pmt_unpin(back_pmt);
        rx_vmar_unmap(rx_vmar_root, (uint64_t)dsp_back, back_size);
        rx_handle_close(dsp_back_vmo);
    }
    res_id = id;
    dsp_back_vmo = vmo;
    dsp_back = (uint32_t *)addr;
    back_pmt = pmt;
    back_size = size;
    dsp_width = w;
    dsp_height = h;
    return 0;
}

static void scanouts_on(int on) {
    for (int i = 0; i < nheads; i++)
        set_scanout(heads[i].scanout, on ? res_id : 0, on ? heads[i].r : (struct rect){ 0, 0, 0, 0 });
}

// The text console: the kernel draws it into memory of ours (it cannot
// be the UEFI framebuffer: the device shows only its resources), a
// resource we copy to the host when the kernel says it drew
#define CONSOLE_ID 3
static rx_handle_t claim;
static volatile uint8_t *fb;      // the console's pixels (ours, after the redirect)
static uint32_t fb_stride, fb_format;
static uint64_t fb_phys;
static int console_ok;

static int setup_console(uint32_t red) {
    rx_handle_t vmo;
    uint64_t size = ((uint64_t)fb_stride * fb_height + 4095) & ~4095ull, addr = 0;
    if (rx_vmo_create_contiguous(bti, size, 0, &vmo) != RX_OK) return -1;
    rx_handle_t pmt;
    if (rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE | RX_BTI_CONTIGUOUS, vmo, 0, size, &fb_phys, &pmt) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK)
        return -1;
    // GOP's layouts: 0x00RRGGBB (blue byte first) or red byte first
    fb_format = red == 0xFF0000 ? FMT_B8G8R8X8 : 134 /* R8G8B8X8 */;
    if (resource_create(CONSOLE_ID, fb_format, fb_stride / 4, fb_height) < 0) return -1;
    uint64_t *pages = malloc((size_t)(size / 4096) * 8);
    if (!pages) return -1;
    for (uint64_t i = 0; i < size / 4096; i++) pages[i] = fb_phys + i * 4096;
    int r = attach_backing(CONSOLE_ID, pages, (size_t)(size / 4096));
    free(pages);
    if (r < 0) return -1;
    fb = (volatile uint8_t *)addr;
    if (rx_display_console_vmo(claim, RX_DISPLAY_REDIRECT, vmo) != RX_OK) return -1;
    return 0;
}

static void console_copy(void) {
    struct rect r = { 0, 0, fb_stride / 4, fb_height };
    transfer(CONSOLE_ID, r, 0);
    flush(CONSOLE_ID, (struct rect){ 0, 0, fb_width, fb_height });
}

static void vg_present(const rx_display_rect_t *rs, int n) {
    rx_display_rect_t all = { 0, 0, dsp_width, dsp_height };
    if (!n) {
        rs = &all;
        n = 1;
    }
    // Many small rectangles: one transfer of what holds them all
    rx_display_rect_t box;
    if (n > 8) {
        uint32_t x0 = dsp_width, y0 = dsp_height, x1 = 0, y1 = 0;
        for (int i = 0; i < n; i++) {
            if (rs[i].x < x0) x0 = rs[i].x;
            if (rs[i].y < y0) y0 = rs[i].y;
            if (rs[i].x + rs[i].w > x1) x1 = rs[i].x + rs[i].w;
            if (rs[i].y + rs[i].h > y1) y1 = rs[i].y + rs[i].h;
        }
        box = (rx_display_rect_t){ x0, y0, x1 - x0, y1 - y0 };
        rs = &box;
        n = 1;
    }
    for (int i = 0; i < n; i++) {
        struct rect r = { rs[i].x, rs[i].y, rs[i].w, rs[i].h };
        transfer(res_id, r, ((uint64_t)r.y * dsp_width + r.x) * 4);
        flush(res_id, r);
    }
}

static void console_on(void) {
    // The console on the first head, the others dark
    for (int i = 1; i < nheads; i++) set_scanout(heads[i].scanout, 0, (struct rect){ 0, 0, 0, 0 });
    set_scanout(heads[0].scanout, CONSOLE_ID, (struct rect){ 0, 0, fb_width, fb_height });
    console_copy();
}

static void vg_show(int graphics) {
    if (graphics) {
        if (claim) rx_display_console(claim, RX_DISPLAY_GRAPHICS);
        scanouts_on(1);
    } else {
        if (claim) rx_display_console(claim, RX_DISPLAY_TEXT);
        if (console_ok) console_on();
        else scanouts_on(0);
    }
}

static rx_handle_t vg_snapshot(uint32_t *ow, uint32_t *oh) {
    rx_handle_t vmo;
    if (dsp_graphics || !fb) {
        uint64_t size = (uint64_t)dsp_width * dsp_height * 4;
        if (rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
        if (dsp_graphics) rx_vmo_write(vmo, dsp_back, 0, size);
        *ow = dsp_width;
        *oh = dsp_height;
        return vmo;
    }
    // The text console, from the framebuffer (32-bit BGRX in QEMU)
    uint64_t size = (uint64_t)fb_width * fb_height * 4;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    uint32_t *row = malloc((size_t)fb_width * 4);
    if (!row) {
        rx_handle_close(vmo);
        return 0;
    }
    for (uint32_t r = 0; r < fb_height; r++) {
        volatile const uint32_t *src = (volatile const uint32_t *)(fb + (uint64_t)r * fb_stride);
        for (uint32_t i = 0; i < fb_width; i++) row[i] = src[i] & 0xFFFFFF;
        rx_vmo_write(vmo, row, (uint64_t)r * fb_width * 4, (uint64_t)fb_width * 4);
    }
    free(row);
    *ow = fb_width;
    *oh = fb_height;
    return vmo;
}

// ---- the cursor ----

#define CURSOR_ID 2
static rx_dma_buf_t cursor_mem;
static int cursor_ok, cursor_shown;
static uint32_t cursor_hx, cursor_hy;

static int cursor_cmd(uint32_t type, uint32_t scanout, int x, int y, uint32_t id) {
    put_hdr(type);
    uint32_t b[8] = { scanout, (uint32_t)x, (uint32_t)y, 0, id, cursor_hx, cursor_hy, 0 };
    memcpy(req() + sizeof(struct hdr), b, sizeof b);
    return submit(&cursorq, sizeof(struct hdr) + sizeof b, 0);
}

static int vg_cursor_set(const uint32_t *argb, uint32_t hx, uint32_t hy) {
    if (!cursor_ok) return -ENOTSUP;
    memcpy((void *)cursor_mem.va, argb, 64 * 64 * 4);
    cursor_hx = hx < 64 ? hx : 63;
    cursor_hy = hy < 64 ? hy : 63;
    if (transfer(CURSOR_ID, (struct rect){ 0, 0, 64, 64 }, 0) < 0) return -EIO;
    cursor_shown = 0; // the next move shows it with the new image
    return 0;
}

static void vg_cursor_move(int x, int y, int visible) {
    if (!cursor_ok) return;
    // The head the pointer is on, and its place there
    int h = 0;
    for (int i = 0; i < nheads; i++)
        if ((uint32_t)x >= heads[i].r.x && (uint32_t)x < heads[i].r.x + heads[i].r.w) h = i;
    int lx = x - (int)heads[h].r.x, ly = y - (int)heads[h].r.y;
    static int last_head = -1;
    if (!visible) {
        if (cursor_shown) cursor_cmd(CMD_UPDATE_CURSOR, heads[last_head < 0 ? 0 : last_head].scanout, 0, 0, 0);
        cursor_shown = 0;
        return;
    }
    if (!cursor_shown || h != last_head) {
        // Off the old head, on the new one with the image
        if (cursor_shown && last_head >= 0 && h != last_head) cursor_cmd(CMD_UPDATE_CURSOR, heads[last_head].scanout, 0, 0, 0);
        cursor_cmd(CMD_UPDATE_CURSOR, heads[h].scanout, lx, ly, CURSOR_ID);
        cursor_shown = 1;
        last_head = h;
        return;
    }
    cursor_cmd(CMD_MOVE_CURSOR, heads[h].scanout, lx, ly, CURSOR_ID);
}

// ---- size changes: the host asks through the configuration ----

static rx_handle_t tick;

static int vg_wait_items(rx_wait_item_t *items) {
    items[0] = (rx_wait_item_t){ tick, RX_TIMER_SIGNALED, 0 };
    return 1;
}

static void vg_handle(const rx_wait_item_t *items, int n) {
    (void)n;
    if (!(items[0].pending & RX_TIMER_SIGNALED)) return;
    // The console is copied 30 times a second when it changed
    int text = !dsp_graphics && console_ok;
    rx_timer_set(tick, rx_clock_get_monotonic() + (text ? 33000000ull : 250000000ull), 0);
    if (text && rx_display_console(claim, RX_DISPLAY_DRAWN) == 1) console_copy();
    uint32_t ev = r32(devcfg, 0);
    if (!(ev & EVENT_DISPLAY)) return;
    w32(devcfg, 4, EVENT_DISPLAY);
    if (display_info(modes) < 0) return;
    uint32_t w, h;
    layout(&w, &h);
    if (w == dsp_width && h == dsp_height) {
        if (dsp_graphics) scanouts_on(1); // heads may have moved
        else if (console_ok) console_on();
        return;
    }
    if (make_back(w, h) < 0) {
        fprintf(stderr, "virtio-gpu %s: no memory for %ux%u\n", where, w, h);
        return;
    }
    printf("virtio-gpu: screen %ux%u (%d head%s)\n", w, h, nheads, nheads == 1 ? "" : "s");
    if (dsp_graphics) {
        scanouts_on(1);
        vg_present(0, 0);
    } else if (console_ok) {
        console_on();
    }
    dsp_resized();
}

int main(int argc, char **argv) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "virtio-gpu: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    // The UEFI framebuffer, if any (the console's)
    // (the framebuffer itself is not needed: the console is redrawn into
    // our memory)
    rx_handle_t fb_vmo = rx_take_startup_handle(RX_PA_USER1);
    claim = rx_take_startup_handle(RX_PA_USER2);
    uint32_t fb_red = 0xFF0000;
    if (fb_vmo) rx_handle_close(fb_vmo);
    if (claim && argc >= 9) {
        fb_width = (uint32_t)strtoul(argv[2], 0, 10);
        fb_height = (uint32_t)strtoul(argv[3], 0, 10);
        fb_stride = (uint32_t)strtoul(argv[4], 0, 10);
        fb_red = (uint32_t)strtoul(argv[5], 0, 16);
    }

    uint8_t cap = 0;
    while ((cap = rx_drv_find_cap(dev, 0x09, cap))) {
        uint8_t type = (uint8_t)rx_drv_cfg_read(dev, cap + 3, 1);
        if (type == CAP_COMMON && !common) common = cap_target(cap);
        else if (type == CAP_NOTIFY && !notify_base) {
            notify_base = cap_target(cap);
            notify_mult = rx_drv_cfg_read(dev, cap + 16, 4);
        } else if (type == CAP_DEVICE && !devcfg) devcfg = cap_target(cap);
    }
    if (!common || !notify_base || !devcfg) return fail("no virtio 1.0 configuration structures");

    w8(common, C_STATUS, 0);
    for (int k = 0; k < 1000 && r8(common, C_STATUS); k++) sleep_us(100);
    w8(common, C_STATUS, S_ACK);
    w8(common, C_STATUS, S_ACK | S_DRIVER);
    w32(common, C_DFSELECT, 1);
    uint64_t f = (uint64_t)r32(common, C_DF) << 32;
    if (!(f & F_VERSION_1)) return fail("the device does not offer VIRTIO_F_VERSION_1");
    w32(common, C_GFSELECT, 0);
    w32(common, C_GF, 0);         // no 3D (VIRGL), no EDID
    w32(common, C_GFSELECT, 1);
    w32(common, C_GF, (uint32_t)(F_VERSION_1 >> 32));
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK);
    if (!(r8(common, C_STATUS) & S_FEAT_OK)) return fail("the device refused our features");

    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    if (rx_dma_alloc(bti, CMD_SIZE, 0, &cmdbuf) < 0) return fail("no DMA memory");
    rx_drv_bus_master(dev, 1);
    w16(common, C_MSIX_CFG, NO_VECTOR);
    if (vq_setup(0, &ctrlq) < 0 || vq_setup(1, &cursorq) < 0) return fail("cannot set up the queues");
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK | S_DRIVER_OK);

    if (display_info(modes) < 0) return fail("no display information");
    w32(devcfg, 4, r32(devcfg, 0)); // events seen
    uint32_t w, h;
    layout(&w, &h);
    if (make_back(w, h) < 0) return fail("cannot make the screen's resource");
    printf("virtio-gpu: screen %ux%u (%d head%s)\n", w, h, nheads, nheads == 1 ? "" : "s");

    // The cursor's resource
    if (rx_dma_alloc(bti, 64 * 64 * 4, 0, &cursor_mem) == 0 && resource_create(CURSOR_ID, FMT_B8G8R8A8, 64, 64) == 0) {
        uint64_t pa[4] = { cursor_mem.pa, cursor_mem.pa + 4096, cursor_mem.pa + 8192, cursor_mem.pa + 12288 };
        cursor_ok = attach_backing(CURSOR_ID, pa, 4) == 0;
    }
    if (rx_timer_create(0, 0, &tick) != RX_OK) return fail("no timer");
    rx_timer_set(tick, rx_clock_get_monotonic() + 250000000ull, 0);

    static dsp_backend_t gpu = { vg_present, vg_show, vg_snapshot, vg_cursor_set, vg_cursor_move, vg_wait_items, vg_handle,
                                 RX_DISPLAY_CAP_RESIZE };
    if (cursor_ok) gpu.caps |= RX_DISPLAY_CAP_CURSOR;
    // The console, until an owner draws
    if (fb_width && claim) {
        console_ok = setup_console(fb_red) == 0;
        if (!console_ok) fprintf(stderr, "virtio-gpu %s: no text console\n", where);
    }
    if (console_ok) console_on();
    else scanouts_on(0);
    return dsp_run("virtio-gpu", &gpu);
}

// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// objtest: handles, events, event pairs, signals, waiting, timers.
// Prints one line per check and a summary; run it from the shell.

#include "rx.h"

static int passed, failed;

static void check(const char *name, int ok, int64_t got) {
    rx_puts(ok ? "  ok   " : "  FAIL ");
    rx_puts(name);
    if (!ok) {
        rx_puts(" (got ");
        rx_putd(got);
        rx_puts(")");
    }
    rx_puts("\n");
    if (ok) passed++; else failed++;
}

#define MS 1000000ull

static int same(const char *a, const char *b, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) if (a[i] != b[i]) return 0;
    return 1;
}

static void channel_tests(void) {
    rx_handle_t c0 = 0, c1 = 0, ev = 0, weak = 0, got[4] = {0}, p0 = 0, p1 = 0;
    uint32_t observed = 0, nb = 0, nh = 0;
    char buf[64];
    int64_t r;
    rx_puts("channels:\n");

    r = rx_channel_create(0, &c0, &c1);
    check("channel_create", r == RX_OK && c0 && c1 && rx_object_get_type(c0) == RX_OBJ_TYPE_CHANNEL, r);
    rx_object_wait_one(c0, 0xFFFFFFFF, 0, &observed);
    check("new endpoint is WRITABLE, not READABLE", (observed & RX_CHANNEL_WRITABLE) && !(observed & RX_CHANNEL_READABLE), observed);
    r = rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("read empty channel -> SHOULD_WAIT", r == RX_ERR_SHOULD_WAIT, r);

    r = rx_channel_write(c0, 0, "hello", 5, 0, 0);
    check("write 5 bytes", r == RX_OK, r);
    r = rx_object_wait_one(c1, RX_CHANNEL_READABLE, 0, &observed);
    check("other end becomes READABLE", r == RX_OK, r);
    r = rx_channel_read(c0, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("writer's own end has nothing", r == RX_ERR_SHOULD_WAIT, r);
    r = rx_channel_read(c1, 0, buf, got, 2, 4, &nb, &nh);
    check("small buffer -> BUFFER_TOO_SMALL with sizes", r == RX_ERR_BUFFER_TOO_SMALL && nb == 5 && nh == 0, r);
    r = rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("message was kept; read it", r == RX_OK && nb == 5 && nh == 0 && same(buf, "hello", 5), r);
    rx_object_wait_one(c1, RX_CHANNEL_READABLE, 0, &observed);
    check("READABLE cleared when empty", !(observed & RX_CHANNEL_READABLE), observed);

    rx_channel_write(c0, 0, "one", 3, 0, 0);
    rx_channel_write(c0, 0, "two", 3, 0, 0);
    rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, 0);
    r = same(buf, "one", 3);
    rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, 0);
    check("messages arrive in order", r && same(buf, "two", 3), 0);

    // Moving a handle
    rx_event_create(0, &ev);
    r = rx_channel_write(c0, 0, "ev", 2, &ev, 1);
    check("write a message carrying an event handle", r == RX_OK, r);
    r = rx_object_get_type(ev);
    check("sender's handle is gone", r == RX_ERR_BAD_HANDLE, r);
    r = rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("receiver gets 1 handle", r == RX_OK && nh == 1 && got[0] != 0, r);
    check("received handle is the event", rx_object_get_type(got[0]) == RX_OBJ_TYPE_EVENT, rx_object_get_type(got[0]));
    rx_object_signal(got[0], 0, RX_EVENT_SIGNALED);
    r = rx_object_wait_one(got[0], RX_EVENT_SIGNALED, 0, &observed);
    check("received handle keeps its rights", r == RX_OK, r);

    r = rx_handle_duplicate(got[0], RX_RIGHT_WAIT, &weak);
    r = rx_channel_write(c0, 0, "x", 1, &weak, 1);
    check("handle without TRANSFER -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    check("  ... and it is consumed anyway (as in Zircon)", rx_object_get_type(weak) == RX_ERR_BAD_HANDLE, 0);
    r = rx_channel_read(c1, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("a failed write queues nothing", r == RX_ERR_SHOULD_WAIT, r);
    r = rx_channel_write(c0, 0, "x", 1, &c0, 1);
    check("writing a channel into itself -> NOT_SUPPORTED", r == RX_ERR_NOT_SUPPORTED, r);
    rx_handle_t bogus = 0x12345;
    r = rx_channel_write(c1, 0, "x", 1, &bogus, 1);
    check("bad handle in the list -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);

    // c0 was consumed by the failed write above; make a new pair
    rx_channel_create(0, &c0, &c1);
    r = rx_object_signal_peer(c0, 0, RX_USER_SIGNAL_0);
    r = rx_object_wait_one(c1, RX_USER_SIGNAL_0, 0, &observed);
    check("signal_peer on a channel", r == RX_OK, r);

    // Closing
    rx_eventpair_create(0, &p0, &p1);
    rx_channel_write(c0, 0, "keep", 4, &p1, 1);
    rx_channel_write(c1, 0, "last", 4, 0, 0);
    rx_handle_close(c1);
    rx_object_wait_one(c0, 0xFFFFFFFF, 0, &observed);
    check("close one end: PEER_CLOSED, not WRITABLE", (observed & RX_PEER_CLOSED) && !(observed & RX_CHANNEL_WRITABLE), observed);
    r = rx_object_wait_one(p0, RX_PEER_CLOSED, 0, &observed);
    check("handles in unread messages are closed", r == RX_OK, r);
    r = rx_channel_write(c0, 0, "x", 1, 0, 0);
    check("write after close -> PEER_CLOSED", r == RX_ERR_PEER_CLOSED, r);
    r = rx_channel_read(c0, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("queued message still readable after close", r == RX_OK && same(buf, "last", 4), r);
    r = rx_channel_read(c0, 0, buf, got, sizeof buf, 4, &nb, &nh);
    check("then read -> PEER_CLOSED", r == RX_ERR_PEER_CLOSED, r);
    rx_handle_close(c0);
    rx_handle_close(p0);
    rx_handle_close(got[0]);
}

static void memory_tests(rx_handle_t root) {
    rx_handle_t vmo = 0, ro = 0, child = 0, tmp = 0;
    uint64_t size = 0, addr = 0, addr2 = 0;
    char buf[16];
    int64_t r;
    rx_puts("memory:\n");

    check("root VMAR handle passed at start", root != 0 && rx_object_get_type(root) == RX_OBJ_TYPE_VMAR, root);
    r = rx_vmo_create(10000, 0, &vmo);
    check("vmo_create", r == RX_OK && rx_object_get_type(vmo) == RX_OBJ_TYPE_VMO, r);
    r = rx_vmo_get_size(vmo, &size);
    check("size rounded up to pages (12288)", r == RX_OK && size == 12288, (int64_t)size);
    r = rx_vmo_write(vmo, "abc", 4096 + 10, 3);
    check("vmo_write", r == RX_OK, r);
    buf[0] = 'x';
    r = rx_vmo_read(vmo, buf, 4096 + 10, 3);
    check("vmo_read gets it back", r == RX_OK && same(buf, "abc", 3), r);
    buf[0] = 'x';
    rx_vmo_read(vmo, buf, 8192, 4);
    check("untouched memory reads as zero", buf[0] == 0 && buf[3] == 0, buf[0]);
    r = rx_vmo_read(vmo, buf, 12280, 16);
    check("read past the end -> OUT_OF_RANGE", r == RX_ERR_OUT_OF_RANGE, r);
    r = rx_vmo_create(1ull << 31, 0, &tmp);
    check("2 GiB VMO -> OUT_OF_RANGE", r == RX_ERR_OUT_OF_RANGE, r);

    r = rx_vmar_map(root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 12288, &addr);
    check("vmar_map read/write (placed at or above 4 GiB)", r == RX_OK && addr >= RX_VM_ALLOC_HINT, r);
    volatile char *p = (volatile char *)addr;
    check("mapping shows the VMO's contents", p[4096 + 10] == 'a' && p[4096 + 12] == 'c', p[4096 + 10]);
    p[100] = 'Z';
    rx_vmo_read(vmo, buf, 100, 1);
    check("stores through the mapping reach the VMO", buf[0] == 'Z', buf[0]);
    rx_handle_close(vmo);
    p[200] = 'Q';
    check("mapping outlives the closed VMO handle", p[200] == 'Q' && p[100] == 'Z', p[200]);

    r = rx_vmar_protect(root, RX_VM_PERM_READ, addr, 12288);
    check("vmar_protect to read-only", r == RX_OK, r);
    r = rx_event_create(0, (rx_handle_t *)(addr + 64));
    check("  kernel refuses to write there", r == RX_ERR_INVALID_ARGS, r);
    check("  still readable", p[100] == 'Z', p[100]);
    r = rx_vmar_protect(root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, addr, 12288);
    r = rx_event_create(0, &tmp);
    r = rx_event_create(0, (rx_handle_t *)(addr + 64));
    check("protect back to read/write", r == RX_OK, r);

    r = rx_vmar_unmap(root, addr + 4096, 4096);
    check("unmap the middle page", r == RX_OK, r);
    r = rx_event_create(0, (rx_handle_t *)(addr + 4096 + 64));
    check("  middle page is gone", r == RX_ERR_INVALID_ARGS, r);
    r = rx_event_create(0, (rx_handle_t *)(addr + 8192 + 64));
    check("  pages on both sides remain", r == RX_OK && p[100] == 'Z', r);
    r = rx_vmar_unmap(root, addr + 4096, 4096);
    check("unmap it again -> NOT_FOUND", r == RX_ERR_NOT_FOUND, r);
    r = rx_vmar_unmap(root, 0x1000, 4096);
    check("unmap below the VMAR (null page) -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);
    rx_vmar_unmap(root, addr, 12288);

    rx_vmo_create(8192, 0, &vmo);
    r = rx_vmar_map(root, RX_VM_PERM_READ | RX_VM_SPECIFIC, 0x10000000, vmo, 0, 8192, &addr2);
    check("map at a specific address", r == RX_OK && addr2 == RX_ROOT_VMAR_BASE + 0x10000000, r);
    r = rx_vmar_map(root, RX_VM_PERM_READ | RX_VM_SPECIFIC, 0x10001000, vmo, 0, 4096, &addr);
    check("overlapping specific map -> ALREADY_EXISTS", r == RX_ERR_ALREADY_EXISTS, r);
    r = rx_vmar_map(root, RX_VM_PERM_WRITE, 0, vmo, 0, 4096, &addr);
    check("write without read -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);
    r = rx_vmar_map(root, RX_VM_PERM_READ | RX_VM_PERM_EXECUTE, 0, vmo, 0, 4096, &addr);
    check("execute without EXECUTE right -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_vmar_map(root, RX_VM_PERM_READ, 0, vmo, 4096, 8192, &addr);
    check("range past the VMO's end -> OUT_OF_RANGE", r == RX_ERR_OUT_OF_RANGE, r);

    rx_handle_duplicate(vmo, RX_RIGHT_READ | RX_RIGHT_MAP, &ro);
    r = rx_vmar_map(root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, ro, 0, 4096, &addr);
    check("read-only VMO handle: map writable -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_vmar_map(root, RX_VM_PERM_READ, 0, ro, 0, 4096, &addr);
    check("  map read-only works", r == RX_OK, r);
    r = rx_vmar_protect(root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, addr, 4096);
    check("  and cannot be made writable later", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_vmo_write(ro, "x", 0, 1);
    check("  vmo_write through it -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);

    rx_vmo_write(vmo, "orig", 0, 4);
    r = rx_vmo_create_child(vmo, 1, 0, 4096, &child);
    check("vmo_create_child (snapshot)", r == RX_OK, r);
    rx_vmo_write(vmo, "new!", 0, 4);
    rx_vmo_read(child, buf, 0, 4);
    check("  child keeps the old contents", same(buf, "orig", 4), buf[0]);
    rx_vmo_write(child, "kid", 0, 3);
    rx_vmo_read(vmo, buf, 0, 4);
    check("  parent does not see the child's writes", same(buf, "new!", 4), buf[0]);

    rx_handle_close(vmo);
    rx_handle_close(ro);
    rx_handle_close(child);
}

void _start(rx_handle_t bootstrap) {
    rx_startup(bootstrap);
    rx_handle_t root_vmar = rx_vmar_root;
    rx_handle_t ev = 0, dup = 0, tm = 0, p0 = 0, p1 = 0;
    uint32_t observed = 0;
    int64_t r;
    rx_puts("objtest: handles, events, signals, waiting, timers\n");

    uint64_t t0 = rx_clock_get_monotonic();
    uint64_t t1 = rx_clock_get_monotonic();
    check("clock is monotonic", t1 >= t0 && t0 > 0, (int64_t)(t1 - t0));

    // Events
    r = rx_event_create(0, &ev);
    check("event_create", r == RX_OK && ev != 0, r);
    r = rx_object_get_type(ev);
    check("event has type EVENT", r == RX_OBJ_TYPE_EVENT, r);
    r = rx_object_wait_one(ev, RX_EVENT_SIGNALED, 0, &observed);
    check("poll unsignaled event -> TIMED_OUT", r == RX_ERR_TIMED_OUT, r);
    r = rx_object_signal(ev, 0, RX_EVENT_SIGNALED);
    check("object_signal SIGNALED", r == RX_OK, r);
    r = rx_object_wait_one(ev, RX_EVENT_SIGNALED, RX_TIME_INFINITE, &observed);
    check("wait on signaled event returns at once", r == RX_OK && (observed & RX_EVENT_SIGNALED), r);
    r = rx_object_signal(ev, RX_EVENT_SIGNALED, 0);
    check("object_signal clear", r == RX_OK, r);
    r = rx_object_signal(ev, 0, RX_PEER_CLOSED);
    check("object_signal of a system bit -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);

    // Blocking with a deadline
    t0 = rx_clock_get_monotonic();
    r = rx_object_wait_one(ev, RX_EVENT_SIGNALED, t0 + 100 * MS, &observed);
    t1 = rx_clock_get_monotonic();
    check("wait with 100 ms deadline -> TIMED_OUT", r == RX_ERR_TIMED_OUT, r);
    check("  ... after at least 100 ms", t1 - t0 >= 100 * MS, (int64_t)((t1 - t0) / MS));
    check("  ... and not much later (< 200 ms)", t1 - t0 < 200 * MS, (int64_t)((t1 - t0) / MS));

    // Rights
    r = rx_handle_duplicate(ev, RX_RIGHT_WAIT, &dup);
    check("duplicate with WAIT only", r == RX_OK && dup != 0 && dup != ev, r);
    r = rx_object_signal(dup, 0, RX_EVENT_SIGNALED);
    check("signal through WAIT-only handle -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_handle_duplicate(dup, RX_RIGHT_WAIT, &p0);
    check("duplicate without DUPLICATE right -> ACCESS_DENIED", r == RX_ERR_ACCESS_DENIED, r);
    r = rx_handle_duplicate(ev, RX_RIGHT_WAIT | (1u << 20), &p0);
    check("duplicate asking for more rights -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);
    rx_object_signal(ev, 0, RX_USER_SIGNAL_0);
    r = rx_object_wait_one(dup, RX_USER_SIGNAL_0, 0, &observed);
    check("duplicate sees the same object's signals", r == RX_OK, r);

    // Timers
    r = rx_timer_create(0, 0, &tm);
    check("timer_create", r == RX_OK && rx_object_get_type(tm) == RX_OBJ_TYPE_TIMER, r);
    t0 = rx_clock_get_monotonic();
    rx_timer_set(tm, t0 + 150 * MS, 0);
    r = rx_object_wait_one(tm, RX_TIMER_SIGNALED, RX_TIME_INFINITE, &observed);
    t1 = rx_clock_get_monotonic();
    check("timer fires (wait forever)", r == RX_OK && (observed & RX_TIMER_SIGNALED), r);
    check("  ... after at least 150 ms", t1 - t0 >= 150 * MS, (int64_t)((t1 - t0) / MS));
    rx_timer_set(tm, rx_clock_get_monotonic() + 50 * MS, 0);
    rx_timer_cancel(tm);
    r = rx_object_wait_one(tm, RX_TIMER_SIGNALED, rx_clock_get_monotonic() + 100 * MS, 0);
    check("cancelled timer does not fire", r == RX_ERR_TIMED_OUT, r);

    // wait_many: the timer wins over an unsignaled event
    rx_object_signal(ev, RX_EVENT_SIGNALED | RX_USER_SIGNAL_0, 0);
    rx_timer_set(tm, rx_clock_get_monotonic() + 80 * MS, 0);
    rx_wait_item_t items[2] = {
        { ev, RX_EVENT_SIGNALED, 0 },
        { tm, RX_TIMER_SIGNALED, 0 },
    };
    r = rx_object_wait_many(items, 2, RX_TIME_INFINITE);
    check("wait_many returns when the timer fires", r == RX_OK && (items[1].pending & RX_TIMER_SIGNALED) && !(items[0].pending & RX_EVENT_SIGNALED), r);

    // Event pairs
    r = rx_eventpair_create(0, &p0, &p1);
    check("eventpair_create", r == RX_OK && p0 && p1 && p0 != p1, r);
    r = rx_object_signal_peer(p0, 0, RX_USER_SIGNAL_0);
    check("signal_peer", r == RX_OK, r);
    r = rx_object_wait_one(p1, RX_USER_SIGNAL_0, 0, &observed);
    check("peer sees the signal", r == RX_OK, r);
    r = rx_object_wait_one(p0, RX_USER_SIGNAL_0, 0, &observed);
    check("sender does not", r == RX_ERR_TIMED_OUT, r);
    r = rx_handle_close(p0);
    check("close one end", r == RX_OK, r);
    r = rx_object_wait_one(p1, RX_PEER_CLOSED, 0, &observed);
    check("other end sees PEER_CLOSED", r == RX_OK, r);
    r = rx_object_signal_peer(p1, 0, RX_USER_SIGNAL_0);
    check("signal_peer after close -> PEER_CLOSED", r == RX_ERR_PEER_CLOSED, r);

    // Handle lifetime and bad arguments
    r = rx_handle_close(dup);
    check("close duplicate", r == RX_OK, r);
    r = rx_handle_close(dup);
    check("close it again -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_object_signal(dup, 0, RX_EVENT_SIGNALED);
    check("use closed handle -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_object_get_type(ev);
    check("original still valid", r == RX_OBJ_TYPE_EVENT, r);
    r = rx_timer_set(ev, 0, 0);
    check("timer_set on an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_event_create(0, (rx_handle_t *)0x10);
    check("unmapped out pointer -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);
    r = rx_event_create(0, (rx_handle_t *)0xffff800000001000ull);
    check("kernel out pointer -> INVALID_ARGS", r == RX_ERR_INVALID_ARGS, r);

    channel_tests();
    memory_tests(root_vmar);

    rx_puts("objtest: ");
    rx_putd(passed);
    rx_puts(" passed, ");
    rx_putd(failed);
    rx_puts(failed ? " FAILED\n" : " failed\n");
    rx_exit(failed);
}

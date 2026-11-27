// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The sound service's clients (<rustux/sound.h>, docs/SOUND.md)

#include <errno.h>
#include <rustux/service.h>
#include <rustux/sound.h>
#include <string.h>

#include "libc.h"

static rx_handle_t ctl_ch;
static volatile int ctl_lock;

static void lock(void) {
    __lock(&ctl_lock);
}

static void unlock(void) {
    __unlock(&ctl_lock);
}

// One message from ch into buf (waiting until deadline): its size, or -1
// (peer gone, or no answer in time)
static int read_msg_until(rx_handle_t ch, void *buf, uint32_t cap, uint64_t deadline) {
    for (;;) {
        uint32_t nb = 0, nh = 0;
        rx_handle_t h;
        rx_status_t s = rx_channel_read(ch, 0, buf, &h, cap, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t seen = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, deadline, &seen);
            if (!(seen & RX_CHANNEL_READABLE) && (seen & RX_PEER_CLOSED)) return -1;
            if (!(seen & RX_CHANNEL_READABLE) && deadline != RX_TIME_INFINITE && rx_clock_get_monotonic() >= deadline) return -1;
            continue;
        }
        if (s != RX_OK) return -1;
        if (nh) rx_handle_close(h);
        return (int)nb;
    }
}

static int read_msg(rx_handle_t ch, void *buf, uint32_t cap) { return read_msg_until(ch, buf, cap, RX_TIME_INFINITE); }

int rx_sound_control(const char *req, char *out, size_t cap) {
    if (!out || cap < 2) {
        errno = EINVAL;
        return -1;
    }
    lock();
    int ret = -1;
    for (int attempt = 0; attempt < 2 && ret < 0; attempt++) {
        if (!ctl_ch && rx_service_connect("/svc/sound", &ctl_ch) < 0) {
            ctl_ch = 0;
            break;
        }
        if (rx_channel_write(ctl_ch, 0, req, (uint32_t)strlen(req), 0, 0) == RX_OK) {
            // At most 2 seconds: the compositor asks too (the volume keys)
            int n = read_msg_until(ctl_ch, out, (uint32_t)(cap - 1), rx_clock_get_monotonic() + 2000000000ull);
            if (n >= 0) {
                out[n] = 0;
                ret = n;
                break;
            }
        }
        // soundd restarted: connect again once
        rx_handle_close(ctl_ch);
        ctl_ch = 0;
    }
    unlock();
    if (ret < 0) errno = ENOENT;
    return ret;
}

int rx_sound_open(rx_sound_t *s, uint32_t rate, uint32_t channels) {
    memset(s, 0, sizeof *s);
    if (rate < 8000 || rate > 192000 || channels < 1 || channels > 2) return -EINVAL;
    if (rx_service_connect("/svc/sound", &s->ch) < 0) return -ENOENT;
    rx_sound_msg_t m = { RX_SOUND_OPEN, rate, channels, 0 };
    if (rx_channel_write(s->ch, 0, &m, sizeof m, 0, 0) != RX_OK) {
        rx_handle_close(s->ch);
        return -EIO;
    }
    for (;;) {
        rx_sound_msg_t ev;
        int n = read_msg(s->ch, &ev, sizeof ev);
        if (n < 0) {
            rx_handle_close(s->ch);
            s->ch = 0;
            return -EIO;
        }
        if (n < (int)sizeof ev) continue;
        if (ev.op == RX_SOUND_EV_OPENED) {
            if ((int32_t)ev.a < 0) {
                rx_handle_close(s->ch);
                s->ch = 0;
                return (int32_t)ev.a;
            }
            s->rate = rate;
            s->channels = channels;
            s->credit = ev.b;
            return 0;
        }
    }
}

// Credit and other events: wait for one if `block`
static int events(rx_sound_t *s, int block) {
    for (;;) {
        rx_sound_msg_t ev;
        uint32_t nb = 0, nh = 0;
        rx_handle_t h;
        rx_status_t st = rx_channel_read(s->ch, 0, &ev, &h, sizeof ev, 1, &nb, &nh);
        if (st == RX_ERR_SHOULD_WAIT) {
            if (!block) return 0;
            uint32_t seen = 0;
            rx_object_wait_one(s->ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &seen);
            if (!(seen & RX_CHANNEL_READABLE) && (seen & RX_PEER_CLOSED)) return -EIO;
            continue;
        }
        if (st != RX_OK) return -EIO;
        if (nh) rx_handle_close(h);
        if (nb < sizeof ev) continue;
        if (ev.op == RX_SOUND_EV_CREDIT) {
            s->credit += ev.a;
            return 1;
        }
        if (ev.op == RX_SOUND_EV_DRAINED) return 2;
    }
}

ssize_t rx_sound_write(rx_sound_t *s, const int16_t *samples, size_t frames) {
    if (!s->ch) return -EBADF;
    size_t fb = 2u * s->channels;
    char buf[sizeof(rx_sound_msg_t) + RX_SOUND_MAX_DATA];
    size_t done = 0;
    while (done < frames) {
        int r = events(s, s->credit < fb);
        if (r < 0) return done ? (ssize_t)done : r;
        size_t n = frames - done;
        size_t room = s->credit / fb;
        if (n > room) n = room;
        if (n > RX_SOUND_MAX_DATA / fb) n = RX_SOUND_MAX_DATA / fb;
        if (n == 0) continue;
        rx_sound_msg_t m = { RX_SOUND_DATA, 0, 0, 0 };
        memcpy(buf, &m, sizeof m);
        memcpy(buf + sizeof m, (const char *)samples + done * fb, n * fb);
        if (rx_channel_write(s->ch, 0, buf, (uint32_t)(sizeof m + n * fb), 0, 0) != RX_OK) return done ? (ssize_t)done : -EIO;
        s->credit -= n * fb;
        done += n;
    }
    return (ssize_t)done;
}

int rx_sound_drain(rx_sound_t *s) {
    if (!s->ch) return -EBADF;
    rx_sound_msg_t m = { RX_SOUND_DRAIN, 0, 0, 0 };
    if (rx_channel_write(s->ch, 0, &m, sizeof m, 0, 0) != RX_OK) return -EIO;
    for (;;) {
        int r = events(s, 1);
        if (r < 0) return r;
        if (r == 2) return 0;
    }
}

void rx_sound_close(rx_sound_t *s) {
    if (s->ch) rx_handle_close(s->ch);
    s->ch = 0;
}

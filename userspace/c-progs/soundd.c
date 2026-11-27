// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// soundd: the sound service (phase S, docs/SOUND.md, <rustux/sound.h>).
//
// Sound drivers (hda) connect to /svc/sounddev and lend it their ring
// buffer; programs connect to /svc/sound and send it streams of samples,
// or text requests (the volume, muting, what there is). soundd mixes every
// stream, converted to 48000 Hz stereo (linear interpolation), at the
// volume, into the ring of the device in use, a few periods ahead of what
// the device has played; it starts the device when there is something to
// play and stops it two seconds after the last sound. The volume and
// muting are kept in /var/lib/sound/state.
//
// The device in use: the first with analog outputs (speakers, headphones,
// line out), in the order they came.

#include <errno.h>
#include <fcntl.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/sound.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAXDEV   4
#define MAXCLI   48
#define LEAD     6u          // periods written ahead of the device
#define IDLE_NS  2000000000ull

typedef struct {
    rx_handle_t ch;
    int used, analog, running;
    char name[96];
    char outputs[256];
    volatile uint8_t *ring;
    uint32_t ring_bytes, period;
    uint64_t written, played;   // bytes since START
    uint32_t last_pos;
    uint64_t last_sound;        // when real samples were last mixed in
} device_t;

typedef struct {
    rx_handle_t ch;
    int used, stream;
    uid_t uid;
    uint32_t rate, chans;
    uint8_t *q;                 // its samples not yet played
    size_t qcap, qhead, qlen;   // bytes
    uint64_t phase, step;       // 32.32 input frames
    size_t since_credit;
    int drain;                  // DRAIN asked: answer once played
    uint64_t drain_at;          // the device position that plays its last sample
} client_t;

static device_t devs[MAXDEV];
static client_t clis[MAXCLI];
static int volume = 70, muted;
static int verbose;

static void say(const char *fmt, const char *a) {
    printf("soundd: ");
    printf(fmt, a);
    printf("\n");
    fflush(stdout);
}

// ---- the state on disk ----

static void load_state(void) {
    FILE *f = fopen("/var/lib/sound/state", "r");
    if (!f) return;
    char line[64];
    while (fgets(line, sizeof line, f)) {
        if (!strncmp(line, "volume=", 7)) volume = atoi(line + 7);
        else if (!strncmp(line, "muted=", 6)) muted = atoi(line + 6) != 0;
    }
    fclose(f);
    if (volume < 0) volume = 0;
    if (volume > 100) volume = 100;
}

static void save_state(void) {
    mkdir("/var/lib", 0755);
    mkdir("/var/lib/sound", 0755);
    FILE *f = fopen("/var/lib/sound/state.new", "w");
    if (!f) return;
    fprintf(f, "volume=%d\nmuted=%d\n", volume, muted);
    fclose(f);
    rename("/var/lib/sound/state.new", "/var/lib/sound/state");
}

// ---- devices ----

static device_t *active(void) {
    for (int i = 0; i < MAXDEV; i++)
        if (devs[i].used && devs[i].analog) return &devs[i];
    for (int i = 0; i < MAXDEV; i++)
        if (devs[i].used) return &devs[i];
    return 0;
}

static void dev_send(device_t *d, uint32_t op, uint32_t a) {
    rx_sound_msg_t m = { op, a, 0, 0 };
    rx_channel_write(d->ch, 0, &m, sizeof m, 0, 0);
}

static void send_ev(client_t *c, uint32_t op, uint32_t a, uint32_t b) {
    rx_sound_msg_t m = { op, a, b, 0 };
    rx_channel_write(c->ch, 0, &m, sizeof m, 0, 0);
}

// The gain for the volume (a square law: half way is a quarter, about
// -12 dB), 1.0 = 65536
static int32_t gain(void) {
    if (muted) return 0;
    return (int32_t)((int64_t)volume * volume * 65536 / 10000);
}

static int any_sound(void) {
    for (int i = 0; i < MAXCLI; i++)
        if (clis[i].used && clis[i].stream && clis[i].qlen) return 1;
    return 0;
}

// One client's next `n` output frames added into mix; 1 if it had any
static int mix_client(client_t *c, int32_t *mix, uint32_t n) {
    size_t fb = 2u * c->chans;
    size_t avail = c->qlen / fb;
    if (!avail) return 0;
    uint64_t ph = c->phase;
    uint32_t i;
    for (i = 0; i < n; i++) {
        size_t idx = (size_t)(ph >> 32);
        if (idx >= avail) break;
        uint32_t frac = (uint32_t)ph;
        size_t nidx = idx + 1 < avail ? idx + 1 : idx;
        for (uint32_t ch = 0; ch < 2; ch++) {
            uint32_t sc = c->chans == 2 ? ch : 0;
            size_t o0 = (c->qhead + idx * fb + sc * 2) % c->qcap, o1 = (c->qhead + nidx * fb + sc * 2) % c->qcap;
            int32_t s0 = (int16_t)(c->q[o0] | c->q[(o0 + 1) % c->qcap] << 8);
            int32_t s1 = (int16_t)(c->q[o1] | c->q[(o1 + 1) % c->qcap] << 8);
            int32_t v = s0 + (int32_t)(((int64_t)(s1 - s0) * frac) >> 32);
            mix[i * 2 + ch] += v;
        }
        ph += c->step;
    }
    size_t used = (size_t)(ph >> 32);
    if (used > avail) used = avail;
    c->phase = ph - ((uint64_t)used << 32);
    c->qhead = (c->qhead + used * fb) % c->qcap;
    c->qlen -= used * fb;
    c->since_credit += used * fb;
    if (c->since_credit >= c->qcap / 4) {
        send_ev(c, RX_SOUND_EV_CREDIT, (uint32_t)c->since_credit, 0);
        c->since_credit = 0;
    }
    return 1;
}

// Write the ring from d->written up to `target` bytes since START
static void fill(device_t *d, uint64_t target) {
    static int32_t mix[2 * 4096];
    static int16_t out[2 * 4096];
    int32_t g = gain();
    while (d->written < target) {
        uint32_t off = (uint32_t)(d->written % d->ring_bytes);
        uint64_t n = target - d->written;
        if (n > d->ring_bytes - off) n = d->ring_bytes - off;
        if (n > sizeof out) n = sizeof out;
        uint32_t frames = (uint32_t)(n / RX_SOUND_FRAME);
        if (!frames) break;
        memset(mix, 0, frames * 2 * sizeof mix[0]);
        int sound = 0;
        for (int i = 0; i < MAXCLI; i++)
            if (clis[i].used && clis[i].stream && mix_client(&clis[i], mix, frames)) {
                sound = 1;
                if (!clis[i].qlen && clis[i].drain && !clis[i].drain_at) clis[i].drain_at = d->written + frames * RX_SOUND_FRAME;
            }
        for (uint32_t k = 0; k < frames * 2; k++) {
            int64_t v = ((int64_t)mix[k] * g) >> 16;
            out[k] = (int16_t)(v > 32767 ? 32767 : v < -32768 ? -32768 : v);
        }
        memcpy((uint8_t *)d->ring + off, out, frames * RX_SOUND_FRAME);
        d->written += frames * RX_SOUND_FRAME;
        if (sound) d->last_sound = rx_clock_get_monotonic();
    }
    // The period after it silent, so that if we are late the device
    // plays silence rather than what it played a ring ago
    uint32_t off = (uint32_t)(target % d->ring_bytes);
    uint32_t z = d->period;
    if (z > d->ring_bytes - off) z = d->ring_bytes - off;
    memset((uint8_t *)d->ring + off, 0, z);
}

static void dev_start(device_t *d) {
    memset((void *)d->ring, 0, d->ring_bytes);
    d->written = 0;
    d->played = 0;
    d->last_pos = 0;
    d->last_sound = rx_clock_get_monotonic();
    fill(d, (uint64_t)LEAD * d->period);
    dev_send(d, RX_SOUNDDEV_START, 0);
    d->running = 1;
    if (verbose) say("started %s", d->name);
}

static void dev_stop(device_t *d) {
    dev_send(d, RX_SOUNDDEV_STOP, 0);
    d->running = 0;
    if (verbose) say("stopped %s", d->name);
}

static void position(device_t *d, uint32_t pos) {
    // 32 bits of bytes played since START: extend
    uint32_t delta = pos - d->last_pos;
    d->last_pos = pos;
    d->played += delta;
    if (d->written < d->played) d->written = d->played; // late: skip what was missed
    fill(d, d->played + (uint64_t)LEAD * d->period);
    for (int i = 0; i < MAXCLI; i++) {
        client_t *c = &clis[i];
        if (c->used && c->drain && c->drain_at && d->played >= c->drain_at) {
            c->drain = 0;
            c->drain_at = 0;
            send_ev(c, RX_SOUND_EV_DRAINED, 0, 0);
        }
    }
}

static void remove_device(device_t *d) {
    say("%s is gone", d->name);
    rx_handle_close(d->ch);
    if (d->ring) rx_vmar_unmap(rx_vmar_root, (uint64_t)(uintptr_t)d->ring, d->ring_bytes);
    memset(d, 0, sizeof *d);
}

static void device_message(device_t *d) {
    char buf[sizeof(rx_sound_msg_t) + 512];
    for (;;) {
        uint32_t nb = 0, nh = 0;
        rx_handle_t h = 0;
        rx_status_t s = rx_channel_read(d->ch, 0, buf, &h, sizeof buf - 1, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) return;
        if (s != RX_OK) {
            remove_device(d);
            return;
        }
        rx_sound_msg_t m;
        if (nb < sizeof m) {
            if (nh) rx_handle_close(h);
            continue;
        }
        memcpy(&m, buf, sizeof m);
        buf[nb] = 0;
        if (m.op == RX_SOUNDDEV_HELLO && nh && !d->ring) {
            uint64_t addr = 0;
            if (m.a == 0 || m.b == 0 || m.a % m.b || m.b % RX_SOUND_FRAME ||
                rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, h, 0, m.a, &addr) != RX_OK) {
                say("a device's ring cannot be used (%s)", buf + sizeof m);
                rx_handle_close(h);
                continue;
            }
            rx_handle_close(h);
            d->ring = (volatile uint8_t *)(uintptr_t)addr;
            d->ring_bytes = m.a;
            d->period = m.b;
            d->analog = (m.c & RX_SOUNDDEV_ANALOG) != 0;
            snprintf(d->name, sizeof d->name, "%s", buf + sizeof m);
            say("device %s", d->name);
            if (d == active()) dev_send(d, RX_SOUNDDEV_MUTE, (uint32_t)muted);
        } else {
            if (nh) rx_handle_close(h);
            if (m.op == RX_SOUNDDEV_OUTPUTS) {
                snprintf(d->outputs, sizeof d->outputs, "%s", buf + sizeof m);
                for (char *p = d->outputs; *p; p++)
                    if (*p == '\n' && p[1]) *p = ',';
                d->outputs[strcspn(d->outputs, "\n")] = 0;
            } else if (m.op == RX_SOUNDDEV_POSITION && d->running && d->ring) {
                position(d, m.a);
            }
        }
    }
}

// ---- clients ----

static void close_client(client_t *c) {
    rx_handle_close(c->ch);
    free(c->q);
    memset(c, 0, sizeof *c);
}

static void set_volume(int v) {
    if (v < 0) v = 0;
    if (v > 100) v = 100;
    volume = v;
    save_state();
}

static void set_muted(int m) {
    muted = m;
    device_t *d = active();
    if (d && d->ring) dev_send(d, RX_SOUNDDEV_MUTE, (uint32_t)m);
    save_state();
}

static void control(client_t *c, char *req) {
    char out[2048];
    size_t n = 0;
    req[strcspn(req, "\r\n")] = 0;
    char *arg = strchr(req, ' ');
    if (arg) *arg++ = 0;
    int ok = 1;
    if (!strcmp(req, "volume") && arg) {
        int v = atoi(arg + (arg[0] == '+' || arg[0] == '-'));
        if (arg[0] == '+') v = volume + v;
        else if (arg[0] == '-') v = volume - v;
        set_volume(v);
        printf("soundd: volume %d\n", volume);
    } else if (!strcmp(req, "mute")) {
        set_muted(1);
    } else if (!strcmp(req, "unmute")) {
        set_muted(0);
    } else if (!strcmp(req, "toggle")) {
        set_muted(!muted);
    } else if (strcmp(req, "status") != 0) {
        n += (size_t)snprintf(out + n, sizeof out - n, "error: %s? (status, volume N|+N|-N, mute, unmute, toggle)\n", req);
        ok = 0;
    }
    if (ok) {
        if (!strcmp(req, "mute") || !strcmp(req, "unmute") || !strcmp(req, "toggle")) printf("soundd: %s\n", muted ? "muted" : "unmuted");
        device_t *a = active();
        int streams = 0;
        for (int i = 0; i < MAXCLI; i++)
            if (clis[i].used && clis[i].stream) streams++;
        n += (size_t)snprintf(out + n, sizeof out - n, "volume %d\nmuted %s\n", volume, muted ? "yes" : "no");
        for (int i = 0; i < MAXDEV; i++) {
            device_t *d = &devs[i];
            if (!d->used || !d->ring) continue;
            n += (size_t)snprintf(out + n, sizeof out - n, "device %s%s%s\noutputs %s\n", d->name, d == a ? " (in use)" : "",
                                  d->running ? " playing" : "", d->outputs[0] ? d->outputs : "-");
        }
        if (!a || !a->ring) n += (size_t)snprintf(out + n, sizeof out - n, "device none\n");
        n += (size_t)snprintf(out + n, sizeof out - n, "streams %d\nok\n", streams);
    }
    rx_channel_write(c->ch, 0, out, (uint32_t)n, 0, 0);
    fflush(stdout);
}

static void client_message(client_t *c) {
    static char buf[sizeof(rx_sound_msg_t) + RX_SOUND_MAX_DATA + 64];
    for (int count = 0; count < 64; count++) {
        uint32_t nb = 0, nh = 0;
        rx_handle_t h = 0;
        rx_status_t s = rx_channel_read(c->ch, 0, buf, &h, sizeof buf - 1, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) return;
        if (s != RX_OK) {
            close_client(c);
            return;
        }
        if (nh) rx_handle_close(h);
        rx_sound_msg_t m;
        memset(&m, 0, sizeof m);
        if (nb >= sizeof m) memcpy(&m, buf, sizeof m);
        if (!c->stream) {
            if (nb >= sizeof m && m.op == RX_SOUND_OPEN) {
                if (m.a < 8000 || m.a > 192000 || m.b < 1 || m.b > 2) {
                    send_ev(c, RX_SOUND_EV_OPENED, (uint32_t)-EINVAL, 0);
                    continue;
                }
                c->rate = m.a;
                c->chans = m.b;
                c->qcap = (size_t)m.a * m.b; // half a second of 16-bit samples
                c->qcap -= c->qcap % (2u * m.b);
                c->q = malloc(c->qcap);
                if (!c->q) {
                    send_ev(c, RX_SOUND_EV_OPENED, (uint32_t)-ENOMEM, 0);
                    continue;
                }
                c->step = ((uint64_t)m.a << 32) / RX_SOUND_RATE;
                c->stream = 1;
                send_ev(c, RX_SOUND_EV_OPENED, 0, (uint32_t)c->qcap);
                continue;
            }
            buf[nb] = 0;
            control(c, buf);
            continue;
        }
        if (m.op == RX_SOUND_DATA) {
            size_t len = nb - sizeof m;
            size_t fb = 2u * c->chans;
            len -= len % fb;
            if (len > c->qcap - c->qlen) len = c->qcap - c->qlen; // beyond its credit: dropped
            for (size_t k = 0; k < len; k++) c->q[(c->qhead + c->qlen + k) % c->qcap] = (uint8_t)buf[sizeof m + k];
            c->qlen += len;
            device_t *d = active();
            if (!d || !d->ring) {
                // Nowhere to play it: taken at once
                c->since_credit += c->qlen;
                c->qhead = (c->qhead + c->qlen) % c->qcap;
                c->qlen = 0;
                send_ev(c, RX_SOUND_EV_CREDIT, (uint32_t)c->since_credit, 0);
                c->since_credit = 0;
            } else if (!d->running) {
                dev_start(d);
            }
        } else if (m.op == RX_SOUND_DRAIN) {
            device_t *d = active();
            if (!c->qlen && (!d || !d->running || d->played >= d->written)) {
                send_ev(c, RX_SOUND_EV_DRAINED, 0, 0);
            } else {
                c->drain = 1;
                c->drain_at = c->qlen ? 0 : (d ? d->written : 0);
            }
        }
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "-v")) verbose = 1;
    rx_malloc_commit(1);
    load_state();
    rx_handle_t lsound, ldev, theirs;
    if (rx_channel_create(0, &lsound, &theirs) != RX_OK || rx_service_publish("/svc/sound", theirs) < 0) {
        fprintf(stderr, "soundd: cannot publish /svc/sound\n");
        return 1;
    }
    if (rx_channel_create(0, &ldev, &theirs) != RX_OK || rx_service_publish("/svc/sounddev", theirs) < 0) {
        fprintf(stderr, "soundd: cannot publish /svc/sounddev\n");
        return 1;
    }
    printf("soundd: ready (volume %d%s)\n", volume, muted ? ", muted" : "");
    fflush(stdout);
    for (;;) {
        rx_wait_item_t items[2 + MAXDEV + MAXCLI];
        int n = 0, di[MAXDEV], ci[MAXCLI];
        items[n++] = (rx_wait_item_t){ lsound, RX_CHANNEL_READABLE, 0 };
        items[n++] = (rx_wait_item_t){ ldev, RX_CHANNEL_READABLE, 0 };
        for (int i = 0; i < MAXDEV; i++) {
            di[i] = -1;
            if (devs[i].used) {
                di[i] = n;
                items[n++] = (rx_wait_item_t){ devs[i].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
            }
        }
        for (int i = 0; i < MAXCLI; i++) {
            ci[i] = -1;
            if (clis[i].used) {
                ci[i] = n;
                items[n++] = (rx_wait_item_t){ clis[i].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
            }
        }
        rx_object_wait_many(items, (uint64_t)n, rx_clock_get_monotonic() + 500000000ull);
        // New connections
        for (int w = 0; w < 2; w++) {
            rx_handle_t l = w ? ldev : lsound;
            for (;;) {
                char b[16];
                rx_handle_t h;
                uint32_t nb = 0, nh = 0;
                if (rx_channel_read(l, 0, b, &h, sizeof b, 1, &nb, &nh) != RX_OK) break;
                if (!nh) continue;
                int placed = 0;
                if (w) {
                    for (int i = 0; i < MAXDEV && !placed; i++)
                        if (!devs[i].used) {
                            memset(&devs[i], 0, sizeof devs[i]);
                            devs[i].used = 1;
                            devs[i].ch = h;
                            placed = 1;
                        }
                } else {
                    for (int i = 0; i < MAXCLI && !placed; i++)
                        if (!clis[i].used) {
                            memset(&clis[i], 0, sizeof clis[i]);
                            clis[i].used = 1;
                            clis[i].ch = h;
                            placed = 1;
                        }
                }
                if (!placed) rx_handle_close(h);
            }
        }
        for (int i = 0; i < MAXDEV; i++)
            if (di[i] >= 0 && devs[i].used && items[di[i]].pending) device_message(&devs[i]);
        for (int i = 0; i < MAXCLI; i++)
            if (ci[i] >= 0 && clis[i].used && items[ci[i]].pending) client_message(&clis[i]);
        // Quiet for a while: the device stops
        uint64_t now = rx_clock_get_monotonic();
        for (int i = 0; i < MAXDEV; i++) {
            device_t *d = &devs[i];
            int draining = 0;
            for (int k = 0; k < MAXCLI; k++)
                if (clis[k].used && clis[k].drain) draining = 1;
            if (d->used && d->running && !any_sound() && !draining && now - d->last_sound > IDLE_NS) dev_stop(d);
        }
    }
}

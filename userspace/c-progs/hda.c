// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// hda: Intel High Definition Audio (phase S, docs/SOUND.md), the sound of
// nearly every PC since 2004: the laptop's Realtek ALC298 on Intel's
// controller, the desktop PC's Realtek ALC897 on AMD's, QEMU's intel-hda.
// devmgr starts it for every HD Audio controller (PCI class 04:03, and
// Intel's audio DSPs in their HD Audio mode, /etc/drivers).
//
//   hda BDF
//
// The controller: reset, the command ring (CORB) and the answer ring
// (RIRB) in DMA memory (the immediate command registers if the rings do
// not answer), one output stream. The codecs on its link are asked for
// their widgets: the analog output pins (speakers, headphones, line out;
// not HDMI or S/PDIF, which need more than this) are each given a path to
// a converter (a DAC), through the mixers and selectors between, chosen,
// unmuted and set to 0 dB; every DAC listens to the same stream, so all
// outputs play the same sound. Headphones plugged in turn the speakers off
// (or, with no speakers, the line outs), as Linux does by default; the
// jacks are looked at twice a second.
//
// The stream: 48000 Hz, 16-bit, stereo, from a ring of 16 periods of 10 ms
// that soundd fills (/svc/sounddev, <rustux/sound.h>); the driver tells it
// each period played (the interrupt at each period's end, or a look every
// 5 ms without one). Volume is soundd's; the driver mutes on request.

#include <errno.h>
#include <pthread.h>
#include <rustux/driver.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/sound.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

// Controller registers
#define GCAP      0x00
#define GCTL      0x08
#define STATESTS  0x0E
#define INTCTL    0x20
#define INTSTS    0x24
#define CORBLBASE 0x40
#define CORBUBASE 0x44
#define CORBWP    0x48
#define CORBRP    0x4A
#define CORBCTL   0x4C
#define CORBSIZE  0x4E
#define RIRBLBASE 0x50
#define RIRBUBASE 0x54
#define RIRBWP    0x58
#define RINTCNT   0x5A
#define RIRBCTL   0x5C
#define RIRBSTS   0x5D
#define RIRBSIZE  0x5E
#define ICOI      0x60
#define ICII      0x64
#define ICIS      0x68
// A stream descriptor's registers (from 0x80 + 0x20 * its index)
#define SD_CTL    0x00   // 3 bytes: SRST 1, RUN 2, IOCE 4; the stream's tag in bits 20-23
#define SD_STS    0x03
#define SD_LPIB   0x04
#define SD_CBL    0x08
#define SD_LVI    0x0C
#define SD_FMT    0x12
#define SD_BDPL   0x18
#define SD_BDPU   0x1C

// Verbs
#define V_GET_PARAM     0xF00
#define V_GET_CONN      0xF02
#define V_SET_SELECT    0x701
#define V_SET_POWER     0x705
#define V_SET_STREAM    0x706
#define V_SET_PIN       0x707
#define V_PIN_SENSE     0xF09
#define V_EXEC_SENSE    0x709
#define V_SET_EAPD      0x70C
#define V_GET_DEFCFG    0xF1C
#define V4_SET_FORMAT   0x2
#define V4_SET_AMP      0x3
// Parameters
#define P_VENDOR    0x00
#define P_NODES     0x04
#define P_FGTYPE    0x05
#define P_WCAPS     0x09
#define P_PINCAPS   0x0C
#define P_INAMP     0x0D
#define P_CONNLEN   0x0E
#define P_OUTAMP    0x12

enum { W_DAC = 0, W_ADC = 1, W_MIXER = 2, W_SELECTOR = 3, W_PIN = 4 };
enum { K_LINE = 0, K_SPEAKER = 1, K_HEADPHONES = 2 };
static const char *kind_name[] = { "line", "speaker", "headphones" };

#define PERIOD   1920u                 // 10 ms at 48000 Hz, 4 bytes a frame
#define NPERIODS 16u
#define RING     (PERIOD * NPERIODS)
#define FORMAT   0x0011u               // 48 kHz, 16 bits, 2 channels
#define TAG      1u

typedef struct {
    uint8_t type;
    uint32_t caps, pincaps, defcfg, outamp, inamp;
    uint8_t nconn;
    uint8_t conn[32];
} widget_t;

typedef struct {
    int kind;
    int nodes[8];   // from the pin to the DAC
    int sel[8];     // the connection each node takes to the next
    int len;
    int sense;      // it can tell whether a plug is in
    int plugged;    // -1: cannot tell
    int on;
} out_t;

static const char *where = "?";
static rx_handle_t dev, irq, bti;
static volatile uint8_t *regs;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_dma_buf_t corb, rirb, bdl, ring;
static unsigned corb_n, rirb_n, rirb_rp;
static int immediate, polling;
static uint16_t gcap;
static int sd_index;            // the output stream's descriptor
static volatile uint8_t *sd;
static int cad = -1;            // the codec in use
static uint32_t codec_id;
static widget_t wd[256];
static int afg, wstart, wcount;
static out_t outs[8];
static int nouts;
static int running, muted;
static rx_handle_t svc;         // soundd

static uint8_t rd8(uint32_t r) { return regs[r]; }
static uint16_t rd16(uint32_t r) { return *(volatile uint16_t *)(regs + r); }
static uint32_t rd32(uint32_t r) { return *(volatile uint32_t *)(regs + r); }
static void wr8(uint32_t r, uint8_t v) { regs[r] = v; }
static void wr16(uint32_t r, uint16_t v) { *(volatile uint16_t *)(regs + r) = v; }
static void wr32(uint32_t r, uint32_t v) { *(volatile uint32_t *)(regs + r) = v; }

static void sleep_us(uint64_t us) {
    struct timespec t = { (time_t)(us / 1000000), (long)(us % 1000000) * 1000 };
    nanosleep(&t, 0);
}

static uint64_t now_us(void) { return rx_clock_get_monotonic() / 1000; }

static int fail(const char *what) {
    fprintf(stderr, "hda %s: %s\n", where, what);
    return 1;
}

// Wait until (reg & mask) == want (an 8-, 16- or 32-bit register): 0, or -1
// after `ms`
static int wait_bits(uint32_t reg, int width, uint32_t mask, uint32_t want, uint32_t ms) {
    uint64_t end = now_us() + ms * 1000ull;
    for (;;) {
        uint32_t v = width == 8 ? rd8(reg) : width == 16 ? rd16(reg) : rd32(reg);
        if ((v & mask) == want) return 0;
        if (now_us() > end) return -1;
        sleep_us(20);
    }
}

// ---- commands ----

static int send_immediate(uint32_t verb, uint32_t *resp) {
    if (wait_bits(ICIS, 16, 1, 0, 10) < 0) return -1;
    wr32(ICOI, verb);
    wr16(ICIS, 3); // busy, and the old answer's valid bit cleared
    uint64_t end = now_us() + 50000;
    while (now_us() < end) {
        uint16_t st = rd16(ICIS);
        if ((st & 3) == 2) {
            *resp = rd32(ICII);
            return 0;
        }
        sleep_us(10);
    }
    return -1;
}

static int send_ring(uint32_t verb, uint32_t *resp) {
    unsigned wp = (rd16(CORBWP) & 0xFF) + 1;
    if (wp >= corb_n) wp = 0;
    ((volatile uint32_t *)corb.va)[wp] = verb;
    rx_dma_barrier();
    wr16(CORBWP, (uint16_t)wp);
    uint64_t end = now_us() + 50000;
    while (now_us() < end) {
        unsigned w = rd16(RIRBWP) & 0xFF;
        while (rirb_rp != w) {
            rirb_rp = (rirb_rp + 1) % rirb_n;
            volatile uint32_t *e = (volatile uint32_t *)(rirb.va + rirb_rp * 8);
            uint32_t r = e[0], ex = e[1];
            if (ex & 0x10) continue; // unsolicited: the jacks are polled instead
            *resp = r;
            wr8(RIRBSTS, 0x05);
            return 0;
        }
        sleep_us(10);
    }
    return -1;
}

// A verb to the codec in use: its answer, or 0xFFFFFFFF if none came
static uint32_t verb_raw(uint32_t v) {
    uint32_t r = 0xFFFFFFFFu;
    if (!immediate) {
        if (send_ring(v, &r) == 0) return r;
        if (send_ring(v, &r) == 0) return r;
        printf("hda %s: the command ring does not answer (CORB rp %04x wp %04x ctl %02x, RIRB wp %04x ctl %02x sts %02x, read %u); using the immediate command registers\n",
               where, rd16(CORBRP), rd16(CORBWP), rd8(CORBCTL), rd16(RIRBWP), rd8(RIRBCTL), rd8(RIRBSTS), rirb_rp);
        immediate = 1;
    }
    if (send_immediate(v, &r) < 0) return 0xFFFFFFFFu;
    return r;
}

static uint32_t verb_at(int c, int nid, uint32_t v12, uint32_t payload) {
    return verb_raw((uint32_t)c << 28 | (uint32_t)nid << 20 | v12 << 8 | (payload & 0xFF));
}

static uint32_t verb(int nid, uint32_t v12, uint32_t payload) { return verb_at(cad, nid, v12, payload); }

static uint32_t verb4(int nid, uint32_t v4, uint32_t payload) {
    return verb_raw((uint32_t)cad << 28 | (uint32_t)nid << 20 | v4 << 16 | (payload & 0xFFFF));
}

static uint32_t param_at(int c, int nid, uint32_t p) { return verb_at(c, nid, V_GET_PARAM, p); }

// ---- the controller ----

static int reset_controller(void) {
    wr16(STATESTS, 0x7FFF);
    wr32(GCTL, rd32(GCTL) & ~1u);
    if (wait_bits(GCTL, 32, 1, 0, 100) < 0) return -1;
    sleep_us(1000);
    wr32(GCTL, rd32(GCTL) | 1u);
    if (wait_bits(GCTL, 32, 1, 1, 100) < 0) return -1;
    // The codecs announce themselves within 521 us of the reset; some
    // take longer
    sleep_us(20000);
    return 0;
}

static unsigned ring_size(uint32_t reg, unsigned *n) {
    uint8_t cap = rd8(reg) >> 4;
    if (cap & 4) { *n = 256; return 2; }
    if (cap & 2) { *n = 16; return 1; }
    *n = 2;
    return 0;
}

static int setup_rings(int low) {
    if (rx_dma_alloc(bti, 4096, low, &corb) < 0 || rx_dma_alloc(bti, 4096, low, &rirb) < 0) return -1;
    wr8(CORBCTL, 0);
    wait_bits(CORBCTL, 8, 2, 0, 10);
    wr32(CORBLBASE, (uint32_t)corb.pa);
    wr32(CORBUBASE, (uint32_t)(corb.pa >> 32));
    wr8(CORBSIZE, (uint8_t)((rd8(CORBSIZE) & 0xFC) | ring_size(CORBSIZE, &corb_n)));
    wr16(CORBWP, 0);
    wr16(CORBRP, 0x8000);
    wait_bits(CORBRP, 16, 0x8000, 0x8000, 10); // not every controller shows it
    wr16(CORBRP, 0);
    wait_bits(CORBRP, 16, 0x8000, 0, 10);
    wr8(CORBCTL, 2);
    wr8(RIRBCTL, 0);
    wait_bits(RIRBCTL, 8, 2, 0, 10);
    wr32(RIRBLBASE, (uint32_t)rirb.pa);
    wr32(RIRBUBASE, (uint32_t)(rirb.pa >> 32));
    wr8(RIRBSIZE, (uint8_t)((rd8(RIRBSIZE) & 0xFC) | ring_size(RIRBSIZE, &rirb_n)));
    wr16(RIRBWP, 0x8000);
    wr16(RINTCNT, 1);
    rirb_rp = 0;
    // DMA on, and the response status (as Linux): a controller may hold
    // the command ring after RINTCNT answers until that status is cleared
    // (QEMU's does); no interrupt is asked for (INTCTL's CIE stays off)
    wr8(RIRBSTS, 0x05);
    wr8(RIRBCTL, 3);
    return 0;
}

// ---- the codec ----

static void read_conns(int nid) {
    widget_t *w = &wd[nid];
    w->nconn = 0;
    if (!(w->caps & (1u << 8))) return;
    uint32_t cl = param_at(cad, nid, P_CONNLEN);
    if (cl == 0xFFFFFFFFu) return;
    int lng = (cl >> 7) & 1, n = cl & 0x7F;
    int per = lng ? 2 : 4, bits = lng ? 16 : 8;
    int prev = -1;
    for (int i = 0; i < n && w->nconn < 32; i += per) {
        uint32_t r = verb(nid, V_GET_CONN, (uint32_t)i);
        for (int k = 0; k < per && i + k < n && w->nconn < 32; k++) {
            uint32_t e = (r >> (k * bits)) & ((1u << bits) - 1);
            int range = (e >> (bits - 1)) & 1;
            int id = (int)(e & ((1u << (bits - 1)) - 1));
            if (range && prev >= 0) {
                for (int j = prev + 1; j <= id && w->nconn < 32; j++) w->conn[w->nconn++] = (uint8_t)j;
            } else {
                w->conn[w->nconn++] = (uint8_t)id;
            }
            prev = id;
        }
    }
}

// From node nid down to a DAC: the path into o from position depth
static int find_path(out_t *o, int nid, int depth) {
    if (nid < wstart || nid >= wstart + wcount || depth >= 8) return 0;
    widget_t *w = &wd[nid];
    o->nodes[depth] = nid;
    if (w->type == W_DAC) {
        o->len = depth + 1;
        return 1;
    }
    if (depth > 0 && w->type != W_MIXER && w->type != W_SELECTOR) return 0;
    for (int i = 0; i < w->nconn; i++) {
        o->sel[depth] = i;
        if (find_path(o, w->conn[i], depth + 1)) return 1;
    }
    return 0;
}

static uint32_t amp_caps(int nid, int out) {
    widget_t *w = &wd[nid];
    uint32_t own = out ? w->outamp : w->inamp;
    if ((w->caps & (1u << 3)) && own) return own;
    return param_at(cad, afg, out ? P_OUTAMP : P_INAMP);
}

// An amplifier at 0 dB (its offset), or muted
static void set_amp(int nid, int out, int index, int mute) {
    uint32_t caps = amp_caps(nid, out);
    uint32_t gain = caps & 0x7F;
    if (gain > ((caps >> 8) & 0x7F)) gain = (caps >> 8) & 0x7F;
    uint32_t v = (out ? 0x8000u : 0x4000u) | 0x3000u | ((uint32_t)index << 8) | (mute ? 0x80u : 0) | gain;
    verb4(nid, V4_SET_AMP, v);
}

static int parse_codec(int c) {
    cad = c;
    codec_id = param_at(c, 0, P_VENDOR);
    uint32_t nodes = param_at(c, 0, P_NODES);
    if (codec_id == 0xFFFFFFFFu || nodes == 0xFFFFFFFFu) return -1;
    afg = -1;
    int fs = (nodes >> 16) & 0xFF, fn = nodes & 0xFF;
    for (int f = fs; f < fs + fn; f++)
        if ((param_at(c, f, P_FGTYPE) & 0xFF) == 1) {
            afg = f;
            break;
        }
    if (afg < 0) return -1;
    verb(afg, V_SET_POWER, 0);
    sleep_us(10000);
    uint32_t wn = param_at(c, afg, P_NODES);
    wstart = (wn >> 16) & 0xFF;
    wcount = wn & 0xFF;
    if (wstart + wcount > 256) wcount = 256 - wstart;
    memset(wd, 0, sizeof wd);
    for (int n = wstart; n < wstart + wcount; n++) {
        widget_t *w = &wd[n];
        w->caps = param_at(c, n, P_WCAPS);
        w->type = (uint8_t)((w->caps >> 20) & 0xF);
        if (w->caps & (1u << 2)) w->outamp = param_at(c, n, P_OUTAMP);
        if (w->caps & (1u << 1)) w->inamp = param_at(c, n, P_INAMP);
        if (w->type == W_PIN) {
            w->pincaps = param_at(c, n, P_PINCAPS);
            w->defcfg = verb(n, V_GET_DEFCFG, 0);
        }
        read_conns(n);
    }
    // The analog outputs, in the order of their association and sequence
    nouts = 0;
    for (int pass = 0; pass < 3 && nouts < 8; pass++)
        for (int n = wstart; n < wstart + wcount && nouts < 8; n++) {
            widget_t *w = &wd[n];
            if (w->type != W_PIN || !(w->pincaps & (1u << 4))) continue;
            uint32_t conn = w->defcfg >> 30, devk = (w->defcfg >> 20) & 0xF;
            if (conn == 1) continue; // nothing attached
            int kind = devk == 0 ? K_LINE : devk == 1 ? K_SPEAKER : devk == 2 ? K_HEADPHONES : -1;
            // speakers and line outs first, then headphones
            if (kind < 0 || (pass == 0 && kind != K_SPEAKER) || (pass == 1 && kind != K_LINE) || (pass == 2 && kind != K_HEADPHONES)) continue;
            out_t o;
            memset(&o, 0, sizeof o);
            o.kind = kind;
            if (!find_path(&o, n, 0)) {
                printf("hda %s: pin %02x (%s): no way to a converter\n", where, n, kind_name[kind]);
                continue;
            }
            o.sense = (w->pincaps & (1u << 2)) && conn != 2; // a jack that tells
            o.plugged = -1;
            o.on = 1;
            outs[nouts++] = o;
        }
    return nouts > 0 ? 0 : -1;
}

static void pin_ctl(out_t *o, int on) {
    uint32_t v = on ? 0x40u : 0;
    if (on && o->kind == K_HEADPHONES && (wd[o->nodes[0]].pincaps & (1u << 3))) v |= 0x80;
    verb(o->nodes[0], V_SET_PIN, v);
    o->on = on;
}

// Each output's path: chosen, powered, unmuted; its DAC on our stream
static void setup_outputs(void) {
    for (int i = 0; i < nouts; i++) {
        out_t *o = &outs[i];
        for (int k = 0; k < o->len; k++) {
            int nid = o->nodes[k];
            widget_t *w = &wd[nid];
            if (w->caps & (1u << 10)) verb(nid, V_SET_POWER, 0);
            if (k + 1 < o->len) {
                if (w->type == W_MIXER) {
                    // the mixer's input from the path, the others muted
                    if (w->caps & (1u << 1))
                        for (int j = 0; j < w->nconn; j++) set_amp(nid, 0, j, j != o->sel[k]);
                } else if (w->nconn > 1) {
                    verb(nid, V_SET_SELECT, (uint32_t)o->sel[k]);
                }
            }
            if (w->caps & (1u << 2)) set_amp(nid, 1, 0, 0);
            if (w->type == W_DAC) {
                verb4(nid, V4_SET_FORMAT, FORMAT);
                verb(nid, V_SET_STREAM, TAG << 4);
            }
        }
        widget_t *pin = &wd[o->nodes[0]];
        if (pin->pincaps & (1u << 16)) verb(o->nodes[0], V_SET_EAPD, 0x02);
        pin_ctl(o, 1);
    }
}

// Headphones in: the speakers off (or, without speakers, the line outs);
// out: back on. Returns 1 if anything changed.
static int check_jacks(void) {
    int changed = 0, hp = 0, speakers = 0;
    for (int i = 0; i < nouts; i++) {
        out_t *o = &outs[i];
        if (o->kind == K_SPEAKER) speakers = 1;
        if (!o->sense) continue;
        int nid = o->nodes[0];
        if (wd[nid].pincaps & (1u << 1)) verb(nid, V_EXEC_SENSE, 0);
        uint32_t r = verb(nid, V_PIN_SENSE, 0);
        int p = r == 0xFFFFFFFFu ? -1 : (int)(r >> 31);
        if (p != o->plugged) {
            o->plugged = p;
            changed = 1;
        }
        if (o->kind == K_HEADPHONES && p == 1) hp = 1;
    }
    for (int i = 0; i < nouts; i++) {
        out_t *o = &outs[i];
        int want = 1;
        if (hp && (o->kind == K_SPEAKER || (!speakers && o->kind == K_LINE))) want = 0;
        if (want != o->on) {
            pin_ctl(o, want);
            changed = 1;
        }
    }
    return changed;
}

static void send_outputs(void) {
    if (!svc) return;
    char buf[sizeof(rx_sound_msg_t) + 512];
    rx_sound_msg_t m = { RX_SOUNDDEV_OUTPUTS, 0, 0, 0 };
    size_t n = sizeof m;
    for (int i = 0; i < nouts && n + 40 < sizeof buf; i++) {
        out_t *o = &outs[i];
        const char *st = o->plugged == 0 ? "unplugged" : o->on ? "on" : "off";
        n += (size_t)snprintf(buf + n, sizeof buf - n, "%s %s\n", kind_name[o->kind], st);
    }
    memcpy(buf, &m, sizeof m);
    rx_channel_write(svc, 0, buf, (uint32_t)n, 0, 0);
}

static void mute_outputs(int m) {
    for (int i = 0; i < nouts; i++) {
        out_t *o = &outs[i];
        int dac = o->nodes[o->len - 1];
        if (wd[dac].caps & (1u << 2)) set_amp(dac, 1, 0, m);
        int pin = o->nodes[0];
        if (wd[pin].caps & (1u << 2)) set_amp(pin, 1, 0, m);
    }
    muted = m;
}

// ---- the stream ----

static void stream_reset(void) {
    sd[SD_CTL] = (uint8_t)(sd[SD_CTL] & ~2u);
    for (int i = 0; i < 1000 && (sd[SD_CTL] & 2); i++) sleep_us(10);
    sd[SD_CTL] = (uint8_t)(sd[SD_CTL] | 1u);
    for (int i = 0; i < 1000 && !(sd[SD_CTL] & 1); i++) sleep_us(10);
    sleep_us(10);
    sd[SD_CTL] = (uint8_t)(sd[SD_CTL] & ~1u);
    for (int i = 0; i < 1000 && (sd[SD_CTL] & 1); i++) sleep_us(10);
    sd[SD_STS] = 0x1C;
}

static void stream_program(void) {
    stream_reset();
    volatile uint32_t *b = (volatile uint32_t *)bdl.va;
    for (unsigned i = 0; i < NPERIODS; i++) {
        uint64_t a = ring.pa + (uint64_t)i * PERIOD;
        b[i * 4 + 0] = (uint32_t)a;
        b[i * 4 + 1] = (uint32_t)(a >> 32);
        b[i * 4 + 2] = PERIOD;
        b[i * 4 + 3] = 1; // an interrupt at its end
    }
    rx_dma_barrier();
    *(volatile uint32_t *)(sd + SD_CBL) = RING;
    *(volatile uint16_t *)(sd + SD_LVI) = (uint16_t)(NPERIODS - 1);
    *(volatile uint16_t *)(sd + SD_FMT) = (uint16_t)FORMAT;
    *(volatile uint32_t *)(sd + SD_BDPL) = (uint32_t)bdl.pa;
    *(volatile uint32_t *)(sd + SD_BDPU) = (uint32_t)(bdl.pa >> 32);
    sd[SD_CTL + 2] = (uint8_t)((sd[SD_CTL + 2] & 0x0F) | (TAG << 4));
    // The converters were told the format while the stream was off
    for (int i = 0; i < nouts; i++) {
        int dac = outs[i].nodes[outs[i].len - 1];
        verb4(dac, V4_SET_FORMAT, FORMAT);
        verb(dac, V_SET_STREAM, TAG << 4);
    }
}

static uint32_t played_wraps, played_last;
static uint64_t played_reported;

static void stream_start(void) {
    stream_program();
    played_wraps = 0;
    played_last = 0;
    played_reported = 0;
    wr32(INTCTL, rd32(INTCTL) | 0x80000000u | (1u << sd_index));
    sd[SD_CTL] = (uint8_t)(sd[SD_CTL] | 0x04 | 0x02); // interrupt on completion, run
    running = 1;
}

static void stream_stop(void) {
    sd[SD_CTL] = (uint8_t)(sd[SD_CTL] & ~0x06u);
    for (int i = 0; i < 1000 && (sd[SD_CTL] & 2); i++) sleep_us(10);
    wr32(INTCTL, rd32(INTCTL) & ~(1u << sd_index));
    sd[SD_STS] = 0x1C;
    running = 0;
}

static void *irq_thread(void *arg) {
    (void)arg;
    uint64_t next_jack = 0;
    for (;;) {
        if (polling || !running) sleep_us(running ? 5000 : 100000);
        else rx_drv_wait_irq(irq, 5);
        pthread_mutex_lock(&lock);
        uint32_t is = rd32(INTSTS);
        if (is & (1u << sd_index)) sd[SD_STS] = 0x1C;
        if (running) {
            uint32_t lpib = *(volatile uint32_t *)(sd + SD_LPIB) % RING;
            if (lpib < played_last) played_wraps++;
            played_last = lpib;
            uint64_t played = (uint64_t)played_wraps * RING + lpib;
            if (played / PERIOD > played_reported / PERIOD && svc) {
                played_reported = played;
                rx_sound_msg_t m = { RX_SOUNDDEV_POSITION, (uint32_t)played, 0, 0 };
                rx_channel_write(svc, 0, &m, sizeof m, 0, 0);
            }
        }
        uint64_t t = now_us();
        if (t >= next_jack) {
            next_jack = t + 500000;
            if (check_jacks()) {
                for (int i = 0; i < nouts; i++)
                    printf("hda %s: %s %s\n", where, kind_name[outs[i].kind], outs[i].plugged == 0 ? "unplugged" : outs[i].on ? "on" : "off");
                send_outputs();
            }
        }
        pthread_mutex_unlock(&lock);
    }
    return 0;
}

static const char *vendor_name(uint32_t id) {
    switch (id >> 16) {
    case 0x10EC: return "Realtek";
    case 0x8086: return "Intel";
    case 0x1002: return "AMD";
    case 0x1022: return "AMD";
    case 0x10DE: return "NVIDIA";
    case 0x14F1: return "Conexant";
    case 0x111D: case 0x8384: return "IDT";
    case 0x11D4: return "Analog Devices";
    case 0x1106: return "VIA";
    case 0x13F6: return "C-Media";
    case 0x1AF4: return "QEMU";
    default: return "codec";
    }
}

static void codec_name(char *buf, size_t cap) {
    uint32_t dev16 = codec_id & 0xFFFF;
    if ((codec_id >> 16) == 0x10EC) snprintf(buf, cap, "Realtek ALC%x", dev16);
    else snprintf(buf, cap, "%s %04x:%04x", vendor_name(codec_id), codec_id >> 16, dev16);
}

int main(int argc, char **argv) {
    rx_malloc_commit(1);
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "hda: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    uint32_t id = rx_drv_cfg_read(dev, 0, 4);
    uint16_t vendor = (uint16_t)id;
    // PCI power state D0
    uint8_t pm = rx_drv_find_cap(dev, 0x01, 0);
    if (pm) {
        uint16_t pmcsr = (uint16_t)rx_drv_cfg_read(dev, (uint16_t)(pm + 4), 2);
        if (pmcsr & 3) {
            rx_drv_cfg_write(dev, (uint16_t)(pm + 4), 2, pmcsr & ~3u);
            sleep_us(10000);
        }
    }
    // As Linux: traffic class 0 on Intel's; snooping on AMD's (and ATI's)
    if (vendor == 0x8086) rx_drv_cfg_write(dev, 0x44, 1, rx_drv_cfg_read(dev, 0x44, 1) & 0xF8);
    if (vendor == 0x1002 || vendor == 0x1022) rx_drv_cfg_write(dev, 0x42, 1, (rx_drv_cfg_read(dev, 0x42, 1) & 0xF8) | 0x02);
    regs = rx_drv_map_bar(dev, 0, 0);
    if (!regs) return fail("cannot map the registers (BAR 0)");
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    rx_drv_bus_master(dev, 1);
    if (rd32(GCAP) == 0xFFFFFFFFu) return fail("the registers read as all ones (the controller is not answering)");
    gcap = rd16(GCAP);
    int low = !(gcap & 1);
    if (reset_controller() < 0) return fail("the controller did not come out of reset");
    uint16_t codecs = rd16(STATESTS);
    wr16(STATESTS, codecs);
    if (setup_rings(low) < 0) return fail("cannot allocate the command rings");
    int iss = (gcap >> 8) & 0xF, oss = (gcap >> 12) & 0xF;
    printf("hda %s: %04x:%04x, %d output and %d input streams, codecs %04x\n", where, vendor, id >> 16, oss, iss, codecs);
    if (oss == 0) return fail("no output stream");
    if (!codecs) return fail("no codec answered");
    // The first codec with analog outputs
    int found = 0;
    for (int c = 0; c < 15 && !found; c++) {
        if (!(codecs & (1u << c))) continue;
        uint32_t vid = param_at(c, 0, P_VENDOR);
        if (parse_codec(c) == 0) found = 1;
        else printf("hda %s: codec %d (%s %08x): no analog outputs\n", where, c, vendor_name(vid), vid);
    }
    if (!found) {
        printf("hda %s: nothing to play on here (HDMI and DisplayPort sound are not supported yet)\n", where);
        // Stay, so devmgr does not start it again and again
        for (;;) sleep_us(3600000000ull);
    }
    char name[64];
    codec_name(name, sizeof name);
    for (int i = 0; i < nouts; i++) {
        out_t *o = &outs[i];
        char path[64] = "";
        size_t pl = 0;
        for (int k = 0; k < o->len; k++) pl += (size_t)snprintf(path + pl, sizeof path - pl, "%s%02x", k ? " > " : "", o->nodes[k]);
        printf("hda %s: %s %s: %s%s\n", where, name, kind_name[o->kind], path, o->sense ? " (jack sense)" : "");
    }
    setup_outputs();
    check_jacks();
    sd_index = iss;
    sd = regs + 0x80 + 0x20 * sd_index;
    if (rx_dma_alloc(bti, 4096, low, &bdl) < 0 || rx_dma_alloc(bti, RING, low, &ring) < 0) return fail("cannot allocate the stream's memory");
    uint32_t used = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSI, 0, &irq, &used) < 0) polling = 1;
    printf("hda %s: %s ready, 48000 Hz 16-bit stereo%s\n", where, name, polling ? " (polling)" : "");

    pthread_t t;
    if (pthread_create(&t, 0, irq_thread, 0) != 0) return fail("no thread");

    // soundd: connect (and again if it goes away)
    char hello[sizeof(rx_sound_msg_t) + 128];
    for (;;) {
        rx_handle_t ch;
        if (rx_service_connect("/svc/sounddev", &ch) < 0) {
            sleep_us(500000);
            continue;
        }
        rx_handle_t vmo;
        if (rx_handle_duplicate(ring.vmo, RX_RIGHT_SAME_RIGHTS, &vmo) != RX_OK) {
            rx_handle_close(ch);
            return fail("cannot share the ring");
        }
        rx_sound_msg_t m = { RX_SOUNDDEV_HELLO, RING, PERIOD, RX_SOUNDDEV_ANALOG };
        memcpy(hello, &m, sizeof m);
        int n = snprintf(hello + sizeof m, sizeof hello - sizeof m, "%s (%s)", name, where);
        rx_channel_write(ch, 0, hello, (uint32_t)(sizeof m + (size_t)n), &vmo, 1);
        pthread_mutex_lock(&lock);
        svc = ch;
        send_outputs();
        pthread_mutex_unlock(&lock);
        for (;;) {
            rx_sound_msg_t r;
            uint32_t nb = 0, nh = 0;
            rx_handle_t h;
            rx_status_t s = rx_channel_read(ch, 0, &r, &h, sizeof r, 1, &nb, &nh);
            if (s == RX_ERR_SHOULD_WAIT) {
                rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
                continue;
            }
            if (s != RX_OK) break;
            if (nh) rx_handle_close(h);
            if (nb < sizeof r) continue;
            pthread_mutex_lock(&lock);
            if (r.op == RX_SOUNDDEV_START) stream_start();
            else if (r.op == RX_SOUNDDEV_STOP) stream_stop();
            else if (r.op == RX_SOUNDDEV_MUTE) mute_outputs(r.a != 0);
            pthread_mutex_unlock(&lock);
        }
        pthread_mutex_lock(&lock);
        if (running) stream_stop();
        svc = 0;
        pthread_mutex_unlock(&lock);
        rx_handle_close(ch);
        printf("hda %s: soundd is gone; waiting for it\n", where);
    }
}

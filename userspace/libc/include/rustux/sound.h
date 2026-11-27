// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Sound (phase S, docs/SOUND.md): the sound service soundd and its two
// sides.
//
// Drivers (hda) connect to /svc/sounddev, as network drivers connect to
// netd: HELLO with the device's ring buffer (a VMO the device plays from
// in a loop: 48000 Hz, 16-bit, two channels, interleaved), its period and
// its name; OUTPUTS whenever the outputs change (a headphone plugged in);
// POSITION each time the device has played another period. soundd mixes
// ahead of the position into the ring, and starts and stops the device.
//
// Programs connect to /svc/sound. A text request ("status", "volume 50",
// "volume +5", "volume -5", "mute", "unmute", "toggle") gets a text answer
// ending "ok" (or "error: WHY"): rx_sound_control. Or, as the first
// message, OPEN makes the connection a stream: 16-bit samples at its rate
// (8000-192000 Hz) and channels (1 or 2), sent as DATA messages as far as
// the credit soundd gives (CREDIT events); DRAIN is answered when all of
// it has played. rx_sound_open / write / drain / close do all that.

#ifndef RUSTUX_SOUND_H
#define RUSTUX_SOUND_H

#include <rustux/syscalls.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#define RX_SOUND_RATE      48000u  // the devices' format: 48 kHz, s16, stereo
#define RX_SOUND_FRAME     4u      // bytes a frame

typedef struct {
    uint32_t op;
    uint32_t a, b, c;
} rx_sound_msg_t;

// Drivers -> soundd (/svc/sounddev)
#define RX_SOUNDDEV_HELLO     1u  // a ring bytes, b period bytes, c RX_SOUNDDEV_ANALOG...; + name; the ring VMO attached
#define RX_SOUNDDEV_OUTPUTS   2u  // + lines "KIND STATE" (speaker, headphones, line; on, off, unplugged)
#define RX_SOUNDDEV_POSITION  3u  // a: bytes played since START (it wraps the ring)
// soundd -> drivers
#define RX_SOUNDDEV_START     10u // play the ring from its start
#define RX_SOUNDDEV_STOP      11u
#define RX_SOUNDDEV_MUTE      12u // a 1: mute the outputs (the codec's amplifiers), 0: not

#define RX_SOUNDDEV_ANALOG    1u  // speakers, headphones or line out (not only HDMI)

// Programs -> soundd (/svc/sound): a stream
#define RX_SOUND_OPEN         1u  // a rate, b channels; answered with RX_SOUND_EV_OPENED (a 0 or -errno, b the first credit)
#define RX_SOUND_DATA         2u  // + samples (whole frames)
#define RX_SOUND_DRAIN        3u  // answered with RX_SOUND_EV_DRAINED once everything sent has played
// soundd -> programs
#define RX_SOUND_EV_OPENED    100u
#define RX_SOUND_EV_CREDIT    101u // a: bytes more that may be sent
#define RX_SOUND_EV_DRAINED   102u

#define RX_SOUND_MAX_DATA     32768u // bytes of samples a DATA message

// A text request (see above) and its answer in out: its length or -1
// (errno: ENOENT when soundd is not running)
int rx_sound_control(const char *req, char *out, size_t cap);

typedef struct {
    rx_handle_t ch;
    uint32_t rate, channels;
    size_t credit;
} rx_sound_t;

// A stream of 16-bit samples at rate (Hz) and channels (1, 2): 0 or a
// negative errno
int rx_sound_open(rx_sound_t *s, uint32_t rate, uint32_t channels);
// frames (each `channels` samples), waiting for room as needed: the
// frames sent or a negative errno
ssize_t rx_sound_write(rx_sound_t *s, const int16_t *samples, size_t frames);
// Wait until everything written has played
int rx_sound_drain(rx_sound_t *s);
void rx_sound_close(rx_sound_t *s);

#endif

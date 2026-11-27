# Sound (phase S)

Rustica plays sound through **Intel High Definition Audio**, the sound of
nearly every PC since 2004: the laptop's Realtek ALC298 on Intel's
controller, the desktop PC's Realtek ALC897 on AMD's, QEMU's `intel-hda`.
Added 2026-11-26 (step S1, with the volume controls of S3 and the apps'
`audio` permission of S4).

```
soundctl                     the volume, muting, the devices and their outputs
soundctl volume 50           (0-100), or +5, -5
soundctl mute | unmute | toggle
soundctl test                a short chord: left, right, both
play FILE.wav                16-bit PCM, 1 or 2 channels, any rate
```

On the desktop: the panel's speaker (the volume; the wheel over it
changes it, a click opens Settings > Sound), **Settings > Sound** (a
slider, Mute, the outputs, a test sound), and the **volume keys** (Mute,
Volume Up, Volume Down: a notification says the new volume).

## How it fits together

```
programs ──/svc/sound──▶ soundd ──/svc/sounddev──▶ hda (a driver per controller)
 (soundctl, apps)        mixes, volume              the codec, the stream
```

**The driver, `hda`** (`userspace/c-progs/hda.c`), started by devmgr for
every HD Audio controller (PCI class 04:03, and Intel's audio DSPs of 2018
on in their HD Audio mode; `/etc/drivers`):

- the controller: reset; the command ring (CORB) and the answer ring
  (RIRB) in DMA memory, the immediate command registers if the rings do
  not answer; snooping on AMD's controllers and traffic class 0 on
  Intel's, as Linux does;
- the codecs on its link: the first with analog outputs is used. Its
  widgets (converters, mixers, selectors, pins) are read; each output
  pin (speakers, headphones, line out; its default configuration says
  which) gets a path to a DAC through the mixers and selectors between,
  which is chosen, powered, unmuted and set to 0 dB; EAPD (the external
  amplifier) turned on where a pin has it. Every DAC listens to the same
  stream, so all outputs play the same sound;
- headphones: the jacks that can tell are looked at twice a second;
  headphones plugged in turn the speakers off (or, without speakers, the
  line outs: a desktop's rear jack), as Linux does by default;
- the stream: 48000 Hz, 16-bit, stereo, from a ring of 16 periods of
  10 ms in DMA memory, which it lends to soundd; an interrupt (MSI) at
  each period's end, or a look every 5 ms without one; it tells soundd the
  position each period. `dmesg | grep hda` shows what it found: the
  controller, the codec, each output's path, "ready".

Not yet: HDMI and DisplayPort sound (their codecs need the monitor's
ELD), microphones and line in, codec-specific fixes (Linux has many for
laptops whose speakers need a GPIO or a vendor register), USB audio.

**The sound service, `soundd`** (`userspace/c-progs/soundd.c`, the unit
`sound` in `/usr/lib/svc`): publishes `/svc/sound` (programs) and
`/svc/sounddev` (drivers). It mixes every stream, converted to 48000 Hz
stereo (linear interpolation; mono to both channels), at the volume, into
the ring of the device in use, six periods (60 ms) ahead of what the
device has played, and the period after that silent (so a late soundd
gives silence, not old sound); it starts the device when there is
something to play and stops it two seconds after the last sound. The
device in use is the first with analog outputs. The volume (0-100, a
square law: 50 is a quarter, about -12 dB) and muting are kept in
`/var/lib/sound/state`; muting also mutes the codec's amplifiers.

**The protocol** (`<rustux/sound.h>`): a text request on `/svc/sound`
(`status`, `volume N|+N|-N`, `mute`, `unmute`, `toggle`) is answered in
text ending `ok` (`rx_sound_control`; rustica-ui's `sound` module for the
panel and Settings); or `OPEN` with a rate (8000-192000 Hz) and channels
(1 or 2) makes the connection a stream of 16-bit samples, sent as far as
soundd's credit allows (half a second), `DRAIN` answered once it has all
played (`rx_sound_open`, `rx_sound_write`, `rx_sound_drain`,
`rx_sound_close` in the C library).

**Apps** (FoxDen) with the `audio` permission get `/svc/sound` in their
sandbox (`FOXDEN.md`).

## The volume keys

The compositor takes HID's Mute (0x7F), Volume Up (0x80) and Volume Down
(0x81) from any keyboard; the PS/2 keyboard driver gives them for the
scancodes E0 20, E0 30 and E0 2E (many laptops' Fn keys, multimedia
keyboards). USB keyboards whose media keys are on a second interface (the
HID consumer page) are not read yet: the kernel's USB keyboards are boot
keyboards.

## Test

`scripts/run-sound-test.sh` (`TESTS.md`): QEMU's `intel-hda` with an
output codec whose sound QEMU writes to a WAV file; the script listens to
it: a 1000 Hz tone at volume 100 and at 50 (a quarter as loud), nothing
while muted, a 440 Hz tone made at 44100 Hz in one channel comes out at
440 Hz in both, `play` of a 22050 Hz WAV file, the volume kept across a
restart of soundd, the codec answering through the command ring.

## Next

- S1 on the hardware: the laptop's ALC298 and the desktop PC's ALC897
  (`dmesg | grep hda`, `soundctl test`, headphones in and out).
- Microphones (recording through soundd), HDMI/DisplayPort sound, USB
  audio, the consumer-page media keys, per-app volume, system sounds.

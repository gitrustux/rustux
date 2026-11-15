# The input service (DESKTOP.md step F3)

Keyboards, mice and tablets all go through one service, `inputd`, which
applies the keyboard layout and decides who gets each event.

## Devices

| Device | Driver | Records |
|--------|--------|---------|
| USB keyboards (boot protocol) | kernel xHCI (step 13) | keys |
| USB mice, tablets, touchpads that report as mice | kernel xHCI: the interface's HID report descriptor is read for buttons, X, Y and the wheel (relative or absolute); a boot mouse whose descriptor says nothing useful is read in the boot protocol | pointer |
| A device with a keyboard and a mouse interface (wireless receivers) | both interfaces, configured together | keys and pointer |
| PS/2 keyboard | `/bin/ps2kbd` (step 4) | keys |
| PS/2 mouse (QEMU's default; touchpads in PS/2 mode) | `/bin/ps2kbd`, on the auxiliary port and IRQ 12 (now bindable); the wheel if it answers as an IntelliMouse | pointer |

Both sources send the same records over a channel: a key is 4 bytes
(usage, modifiers, flags, 0); a pointer event 8 (buttons, wheel, flags with
bit 0 absolute, 0x81, then X and Y as little-endian 16-bit numbers:
relative motion with Y down, or 0..32767 across the screen).

## inputd

init starts `/bin/inputd` (and starts it again if it ends) with a copy of
the kernel's input channel, a channel for the console and the root
resource; inputd starts `ps2kbd` (and restarts it; `ps2kbd` ends when
inputd does, freeing the IRQs). inputd:

- applies the layout (`keymap.h`, `/etc/keymap`, `/usr/share/keymaps`;
  moved here from init), so the console and graphics share it. It starts
  after the root filesystem is mounted, so an installed system's layout
  is used from boot on (init read `/etc/keymap` before mounting, from the
  boot image: a layout set with `localectl` applied only until a reboot);
  `localectl set-keymap` and `loadkeys` reload it;
- scales relative mouse movement by the pointer speed (`/etc/input.conf`,
  `pointer_speed=PERCENT`, 25-400; `inputctl set-pointer-speed`, Settings >
  Mouse; step F7), read at start and with the layout; absolute devices
  (tablets) are not scaled;
- repeats held USB keys (500 ms, then 30 a second; a PS/2 keyboard repeats
  by itself);
- handles Ctrl+Alt+F1 (text console) and Ctrl+Alt+F7 (graphics) itself;
- gives each event to **the one consumer with the focus**:
  1. a program that grabbed the input (`GRAB`, root: `evtest`), until it
     lets go or ends;
  2. else, while graphics are on the screen, the program registered for
     them (`GRAPHICS`, root: the compositor, step F4, `WINDOWS.md`), or
     the display service's owner (`KEY` and `POINTER` to `/svc/display`,
     `docs/DISPLAY.md`);
  3. else the console (init): the bytes of each key press, Enter as CR
     (init makes it a newline when the terminal's ICRNL is on), and the
     usage and modifiers (Shift+PgUp/PgDn scroll the console).

No other program sees input: nothing reads the devices but the kernel and
`ps2kbd`, and only inputd reads their channels.

## Protocol (`<rustux/input.h>`, `/svc/input`)

Requests are `rx_input_req_t { u32 op; u32 arg; }`, each answered with an
`int32_t` (0 or a negative errno): `GRAB` (root; `EBUSY` if another
program has the focus), `UNGRAB` (closing the channel does it too),
`RELOAD_KEYMAP` (anyone), `RESTART_KBD` (root; the shell's `restart-kbd`),
`GRAPHICS` (root: events while graphics are shown, until the channel
closes; `EBUSY` if another program has it).
A grabbing client receives `rx_input_event_t`: type (key or pointer), HID
usage or buttons, modifiers, flags (pressed, repeat; absolute), x, y,
wheel, the layout's text for a key press (UTF-8 or an escape sequence) and
a monotonic timestamp.

## Commands

- `evtest [-t SECONDS]`: grabs the input and prints every event until Esc.
- `fbtest -n 0`: a cursor that follows the mouse or tablet; clicks and the
  wheel are printed.

## Tests

`scripts/run-input-test.sh` (`docs/TESTS.md`): PS/2 keyboard and mouse,
then USB keyboard, mouse and tablet, in QEMU.

## Not yet

- Touchpad gestures, multi-touch, scrolling with two fingers: a touchpad
  is only a mouse if it reports as one (PS/2 or a HID mouse).
- Pointer acceleration and settings; left-handed buttons.
- Several keyboards with different layouts; compose and dead keys.

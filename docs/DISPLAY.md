# The display service (DESKTOP.md step F2)

The first piece of the graphical desktop: a userspace service that owns the
screen and lets one program at a time draw on it, with the kernel's text
console still there underneath. Everything is software rendering on the
framebuffer UEFI GOP set up at boot (the resolution the firmware chose).

## Pieces

```
 kernel                        devmgr                 display              owner (fbtest, later the compositor)
 ------                        ------                 -------              -----
 GOP framebuffer  -- claim -->  display_claim  -- VMO, claim, geometry --> /svc/display <-- ACQUIRE / PRESENT / RELEASE
 text console     <-- display_console (show / hide) ------------------------  |
                                                                               v
 inputd (keys, pointer) ------- SWITCH (Ctrl+Alt+F1/F7), KEY, POINTER ------> service --> EV_KEY, EV_POINTER, EV_VISIBLE --> owner
```

- **Kernel.** At boot the kernel records the framebuffer GOP describes
  (address, size, width, height, stride, pixel layout; GOP's BGR and RGB
  formats and bit masks that are one of those two byte orders). It loads a
  PAT with write-combining in entry 1 on every CPU, so a physical VMO can be
  mapped write-combining (`vmo_set_cache_policy`). `display_claim` (0x9B,
  root resource) returns the geometry, a write-combining physical VMO over
  the framebuffer and a *claim* object; `display_console` (0x9C) switches
  between the text console and the claim holder's pixels. While graphics
  are shown the console draws nothing but keeps its text (and the kernel
  log gets everything); when shown again it draws all of it. When the
  claim's last handle closes, the console comes back; a kernel panic shows
  it whatever the claim says. `docs/SYSCALL.md` has the details.
- **devmgr** claims the framebuffer at start and starts `/bin/display` with
  the VMO, the claim and the geometry (`display W H STRIDE RED GREEN BLUE
  OFFSET`), and starts it again if it ends (giving up after five ends
  within 5 s each, as for drivers). Without a framebuffer it does nothing.
- **display** (`c-progs/display.c`) maps the framebuffer, keeps a back
  buffer in RAM (committed at once: it is a system service) and publishes
  `/svc/display`.
- **inputd**, the input service (step F3, `docs/INPUT.md`), turns
  Ctrl+Alt+F1 and Ctrl+Alt+F7 into SWITCH requests, and while graphics
  are shown (and no program grabbed the input) sends every key and pointer
  event to the service (KEY, POINTER), which passes them to the owner. The
  compositor (step F4) will take them from inputd directly.

## Protocol (`<rustux/display.h>`)

Messages are `rx_display_msg_t { u32 op; i32 status; u32 a, b; }`,
PRESENT's followed by its rectangles. Replies carry the op with
`RX_DISPLAY_REPLY` set and `status` 0 or a negative errno.

| Op | Who | What |
|----|-----|------|
| `INFO` | anyone | width and height |
| `ACQUIRE` | root | become the owner: the back buffer VMO (width x height pixels `0x00RRGGBB`, `width * 4` bytes a row), cleared to black, and the graphics on the screen. `EBUSY` if someone else owns it |
| `PRESENT` | the owner | copy rectangles (none: all) of the back buffer to the screen, converting to its pixel layout; the reply comes when they are there (`a` = 1 if shown) |
| `RELEASE` | the owner | give the screen back to the console (closing the channel does the same) |
| `SNAPSHOT` | root | what is on the screen now, read from the framebuffer: a VMO of `0x00RRGGBB` pixels |
| `SWITCH` | root (inputd) | `a` = text or graphics; no reply |
| `KEY` | root (inputd) | a key event for the owner; no reply |
| `POINTER` | root (inputd) | a pointer event for the owner: `a` = buttons, wheel << 8, flags << 16; `b` = x, y << 16; no reply |

Events to the owner: `EV_KEY` (`a` = usage, modifiers << 8, flags << 16, as
the input records: usage, modifiers << 8, flags << 16), `EV_POINTER` (as
`POINTER`) and `EV_VISIBLE` (`a` = 1 or 0).

An owner that waits for PRESENT's reply before drawing again never shows a
half-drawn frame of its own; there is no vertical sync on a GOP framebuffer,
so the copy itself can still cross a refresh of the screen.

The client side is in libc (`src/display.c`): `rx_display_open`,
`rx_display_present`, `rx_display_poll`, `rx_display_close` and
`rx_display_snapshot`.

## Commands

- `fbtest [-n FRAMES] [--pattern S] [--check]`: animation with frame
  times, a still test pattern, or a check that a snapshot is what was drawn.
- `screenshot FILE.png`: the screen as a PNG, with the CRC-32 of its pixels.

## Tests

`scripts/run-display-test.sh` (`docs/TESTS.md`): the snapshot check, the
screenshot against QEMU's `screendump` and against the test pattern
computed by the script, Ctrl+Alt+F1/F7, ending the owner with a key, 200
frames of animation, and killing the service (the console comes back and
devmgr starts it again).

QEMU (1280x800, TCG on two host cores): about 380 frames a second of
fbtest's animation, 1.4 ms to draw and 1.2 ms to present a frame.

## Not yet

- Pixel layouts other than 32-bit BGRX and RGBX (16-bit, 10-bit colour):
  no framebuffer is offered. The RGBX path has only run on paper so far:
  QEMU's GOP is BGRX.
- Changing the resolution (it is the firmware's; rboot could set a GOP mode
  before starting the kernel).
- Only root may own the screen or take snapshots; the compositor (F4) will
  be the only owner and give each window its own buffer.
- While the display has the screen another CPU's kernel panic message only
  shows if the display service stops drawing (it is killed with everything
  else when the panicking CPU halts only itself).
- The kernel's direct map of the framebuffer stays write-back (the
  firmware's MTRRs decide its real memory type on hardware); the console
  and the display never draw at the same time.

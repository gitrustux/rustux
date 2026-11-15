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
`rx_display_snapshot`; since step F8 also `rx_display_cursor_set`,
`rx_display_cursor_move` and `rx_display_primary`.

Step F8 added, for virtio-gpu:

| Op / event | What |
|----|-----|
| `CAPS` | reply `a` = `RX_DISPLAY_CAP_CURSOR` (a hardware cursor), `RX_DISPLAY_CAP_RESIZE` (the size can change); `rx_display_open` asks and keeps it in `caps` |
| `CURSOR_SET` | (the owner) the pointer's image: a VMO of 64 x 64 `0xAARRGGBB` pixels attached, `a` = hot spot x \| y << 16; `ENOTSUP` without a hardware cursor |
| `CURSOR_MOVE` | (the owner) `a` = x \| y << 16, `b` = shown; no reply |
| `PRIMARY` | the first head's width and height (several heads make one screen, side by side) |
| `EV_RESIZE` | the screen's new width and height, with a new back buffer attached; the client library maps it in place of the old one before handing the event on; the owner draws all of it again |

The compositor uses the hardware cursor when there is one (then moving
the pointer redraws nothing), keeps the panel, maximised and full-screen
windows, the lock screen and notifications on the first head, maps a
tablet onto it, and follows a resize (`WINDOWS.md`).

## virtio-gpu (step F8)

QEMU, KVM and Proxmox give a guest a virtual GPU: virtio-gpu, as
`virtio-vga` (Proxmox's "VirtIO-GPU"; a VGA part UEFI draws on) or
`virtio-gpu-pci` (no VGA: UEFI leaves no framebuffer). `/etc/drivers`
names its driver with the word `display`: devmgr then hands it the UEFI
framebuffer and its claim, if there is one, instead of starting
`/bin/display` (which takes over if the driver keeps failing: the GOP
framebuffer still works without virtio-gpu).

`virtio-gpu` (`c-progs/virtio-gpu.c`) serves the same `/svc/display` with
the same server code as `display` (`c-progs/displaysvc.h`, a backend
each):

- **2D**: the back buffer is guest memory (a VMO, pinned) backing a host
  resource (`RESOURCE_CREATE_2D`, `ATTACH_BACKING`, B8G8R8X8, that is
  `0x00RRGGBB`); PRESENT is `TRANSFER_TO_HOST_2D` and `RESOURCE_FLUSH` of
  each rectangle (more than eight: of what holds them all). Commands go
  through the control queue and are waited for by polling (they are
  quick); no interrupts.
- **The cursor** is the host's: a 64 x 64 resource (B8G8R8A8),
  `UPDATE_CURSOR` and `MOVE_CURSOR` on the cursor queue.
- **Size**: the device's configuration is looked at four times a second;
  when the host says the display changed (a QEMU window, a VNC or SPICE
  client, the Proxmox console resized), the driver asks for the heads'
  sizes, makes a new back buffer and resource, and the owner gets
  `EV_RESIZE`.
- **Heads**: the enabled heads (scanouts) are laid side by side as one
  screen, each scanout showing its part of the one resource; the first is
  the primary (`PRIMARY`).
- **The text console**: the device shows only its resources, not the VGA
  framebuffer UEFI drew on, so the kernel draws its console into memory
  of the driver's instead (`display_console` `REDIRECT`, `SYSCALL.md`),
  which backs a third resource; while the console is shown the driver
  copies it to the host when the kernel says it drew (`DRAWN`, looked at
  30 times a second). With `virtio-gpu-pci` there is no console to
  redirect: Ctrl+Alt+F1 shows black (the console is still there for the
  log and the serial line).

Not done: 3D (virgl, Venus) until something needs it; interrupts instead
of polling; EDID; a kernel panic is not seen while the console is
redirected (it draws into the driver's memory, which nobody copies any
more); when devmgr gives up on the driver and starts `/bin/display`, a
`virtio-vga` device still shows its last resource rather than the VGA
framebuffer (nothing resets it).

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
- Changing the resolution on the GOP framebuffer (it is the firmware's;
  rboot could set a GOP mode before starting the kernel). virtio-gpu
  changes it with the host (step F8).
- Only root may own the screen or take snapshots; the compositor (F4) will
  be the only owner and give each window its own buffer.
- While the display has the screen another CPU's kernel panic message only
  shows if the display service stops drawing (it is killed with everything
  else when the panicking CPU halts only itself).
- The kernel's direct map of the framebuffer stays write-back (the
  firmware's MTRRs decide its real memory type on hardware); the console
  and the display never draw at the same time.

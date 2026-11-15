# The compositor and windows (DESKTOP.md step F4)

Several programs share the screen through windows. The compositor is a
userspace program: it takes the screen from the display service
(`DISPLAY.md`) and the input from the input service (`INPUT.md`), and
clients talk to it over `/svc/window`.

```
 wintest, rustica-window, ...        compositor                        display, inputd
 ----------------------------        ----------                        ---------------
 buffer (VMO) --CREATE/COMMIT-->  copy of each window ---PRESENT--->  framebuffer
             <--CONFIGURE, KEY,   title bars, cursor  <--key, pointer-- inputd (GRAPHICS)
                POINTER, FOCUS,
                FRAME, CLOSE
```

## Starting it

`compositor [-v]` as root (from the console: `compositor &`), then window
programs (`wintest NAME &`, `rustica-window &`). Ctrl+Alt+F1 goes back to
the console (the compositor keeps running, unseen), Ctrl+Alt+F7 to the
windows. The desktop session (step F6) will start it at login.

## Protocol (`<rustux/window.h>`)

One channel per window. The client creates a VMO of w x h pixels
(`0x00RRGGBB`, `w * 4` bytes a row), maps it, and sends `CREATE` with the
title, the size, an optional position and the VMO (read-only for the
compositor). It draws, then `COMMIT`s the damaged rectangles: the
compositor copies them into its own copy of the window at once, so the
client may draw the next frame straight away, and sends `FRAME` when they
are on the screen. Drawing, committing and waiting for `FRAME` paces
animation to what the screen takes.

| Compositor to client | |
|---|---|
| `CONFIGURE w h` | the size the window should be (resize, maximise, full screen): the client sends `BUFFER` with a new VMO of that size, draws, commits; the window keeps its old content until then |
| `KEY` | HID usage, modifiers, pressed/released/repeat, the layout's text: focused window only |
| `POINTER` | x, y in the content, buttons, wheel; motion, button or wheel; `LEAVE` when the pointer leaves the content |
| `FOCUS` | in or out |
| `STATE` | maximised, full screen, minimised |
| `CLOSE_REQUEST` | the close button or Alt+F4 |
| `FRAME` | the last commit is on the screen |
| `WINDOW_INFO` | (to the panel) x -1 starts a new list, then one per window: its id, title (15 characters), focused, minimised; sent on every change |

| Client to compositor | |
|---|---|
| `CREATE` | title, size, position or -1,-1, the buffer: answered with the window's place |
| `COMMIT` | up to 64 damaged rectangles (none: all) |
| `BUFFER` | a new buffer after `CONFIGURE` |
| `SET_TITLE`, `SET_FULLSCREEN` | |
| `CLIPBOARD_SET`, `CLIPBOARD_GET` | text, up to 4000 bytes; `GET` is answered with the text |
| `SET_ROLE` | `PANEL` (step F6): no frame, docked at the bottom with the screen's width, always on top, never focused (it gets the pointer while over it); maximised windows stop above it |
| `ACTIVATE` | (the panel) a window's id: focus and raise it, bring it back if minimised, minimise it if it has the focus |
| `NOTIFY` | text: a notification, top right, 5 s; also as a connection's first message, without a window (`notify`) |
| `LOCK` | lock the screen (`SESSION.md`) |

Clients: `rx_window_*` in libc (`src/window.c`) for C, the
`rustica-window` crate (`userspace/rust/rustica-window`, a safe wrapper of
it) for Rust.

## Window management

- Click: focus and raise (the click also reaches the window).
- Title bar: drag to move (it cannot go above the top of the screen);
  double-click to maximise or restore. Its buttons, right to left: close
  (asks the client), maximise or restore, minimise.
- The right and bottom border and the corner (with 5 pixels outside them):
  drag to resize. The client is asked for one size at a time, the next
  when it has answered.
- Alt+Tab: the bottom-most other window comes to the top with the focus
  (a minimised one comes back); repeated, it goes through all of them.
- Alt+F4: ask the focused window to close. F11: full screen and back.
- Keys go to the focused window only. Pointer events go to it while the
  pointer is over its content; while a button is held, to the window it
  went down in.

## The desktop session (step F6)

`desktop --run` starts the compositor with a control channel on which it
names the session's user (`user NAME`, or `user` for none). The compositor
draws a wallpaper (a dithered gradient) instead of a plain colour, shows
notifications, and has a lock screen: `LOCK`, Super+L or Ctrl+Alt+L hide
every window and take the keys until the session's user's password
(checked against `/etc/shadow`) is typed. `SESSION.md` has the rest.

## Robustness

- A client that ends or is killed: its channel closes, its window goes.
- A client that ignores a close request loses its window after 5 seconds
  (its channel is closed; it learns it from the next call).
- A client that stops answering otherwise: it only keeps its last frame
  (the compositor never waits for a client).
- The compositor that ends or is killed: the display service shows the
  console again, the input service sends keys to it, and clients see their
  channel close. Starting `compositor` again gives an empty desktop.

## Drawing

Software only, into the display's back buffer: for each damaged rectangle
the wallpaper, the windows bottom to top (border, title bar with the
8x16 console font, buttons, content) and the cursor; then one `PRESENT`
of the damaged rectangles. Only what changed is drawn and copied.

## Tests

`scripts/run-window-test.sh` (`docs/TESTS.md`): with a USB tablet for exact
pointer positions, two `wintest` windows overlap (screendump pixels), click
to focus, keys to the focused window only, move, resize, maximise and
restore, minimise and Alt+Tab, F11, the clipboard, a killed client, a
client ignoring a close request, an animating client paced by frames,
Alt+F4, and the compositor killed (the console comes back).

## Not yet

- Window decorations drawn with the toolkit's fonts (step F5); resizing
  from the left and top edges; snapping; per-window input methods.
- Drag and drop; clipboard formats other than text.
- Clients are any user's; the session (F6) will limit them to the logged
  in user's processes.
- Double buffering is the client's choice (it may draw while the
  compositor copies only during `COMMIT`); there is no vertical sync.

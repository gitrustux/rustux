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
| `WINDOW_INFO` | (to the panel) x -1 starts a new list, then one per window: its id, title (15 characters), focused, minimised; since 2026-11-27 the event's `info` holds its app (`SET_APP`), a 0 byte and its whole title; sent on every change |
| `APPEARANCE` | (110) the theme or the wallpaper changed, and a new window's first event; text: the theme's name; `n` bytes follow, its colours as a theme file's text (`rx_window_drop_text`). `SESSION.md` "Themes" |
| `DESKTOP` | (111, to the panel) the desktop's menu was used: flags 1 Change wallpaper, 2 Appearance settings |

| Client to compositor | |
|---|---|
| `CREATE` | title, size, position or -1,-1 (the compositor chooses), or `RX_WINDOW_AT_END` (-2) for x or y: against the work area's right or bottom edge (menus by the panel); the buffer: answered with the window's place |
| `COMMIT` | up to 64 damaged rectangles (none: all) |
| `BUFFER` | a new buffer after `CONFIGURE` |
| `SET_TITLE`, `SET_FULLSCREEN` | |
| `SET_APP` | (2026-11-27) + text: the app the window belongs to, an app's ID or a `.app` file's name (letters, digits, `.`, `-`, `_`; 63 at most), for the panel's icons (`rx_window_set_app`, `Window::set_app`). The C library sends it as the window is made: `APP_ID` (which appctl gives every app), else the program's name |
| `SET_CURSOR` | (2026-11-26) flags: the pointer's shape over the window's content, `RX_WINDOW_CURSOR_ARROW`, `HAND` (a link), `TEXT`, `WAIT`, `MOVE`, `RESIZE_H`, `RESIZE_V`, `RESIZE_NWSE`, `RESIZE_NESW` (`rx_window_set_cursor`, `Window::set_cursor`). The compositor draws the resize arrows on windows' edges and while resizing itself |
| `CLIPBOARD_SET`, `CLIPBOARD_GET` | text, up to 4000 bytes; `GET` is answered with the text |
| `SET_ROLE` | `PANEL` (step F6): no frame, docked at an edge, always on top, never focused (it gets the pointer while over it); maximised and tiled windows stay out of it. `DESKTOP` (step F12): no frame, the work area, always below every other window, never in the panel's list; its buffer's top byte is premultiplied alpha, so it is drawn over the wallpaper (the desktop's icons) |
| `DRAG_START` | (step F12) text: while the left button is held in this window, drag these file paths (a line each): a label follows the pointer, and on release the window under it gets `EV_DROP` (x, y, flags 1 if it is the same window, the text after the event) |
| `SET_PANEL` | (the panel, step F10) flags: its edge (`RX_WINDOW_EDGE_BOTTOM`, `TOP`, `LEFT`, `RIGHT`), x: its thickness (24-400), y: 1 auto-hide (it takes no room and shows while the pointer is at that edge or over it). Maximised and tiled windows, the desktop and notifications move to the new work area |
| `ACTIVATE` | (the panel) a window's id: focus and raise it, bring it back if minimised, minimise it if it has the focus |
| `NOTIFY` | text: a notification, top right, 5 s; also as a connection's first message, without a window (`notify`) |
| `LOCK` | lock the screen (`SESSION.md`) |

Clients: `rx_window_*` in libc (`src/window.c`) for C, the
`rustica-window` crate (`userspace/rust/rustica-window`, a safe wrapper of
it) for Rust. `rx_window_wake(ch)` (step F7; `Window::waker()` in Rust)
may be called from any thread: the window's next `rx_window_next_event`
returns `EV_WAKE` (a user signal on the client's own end of the channel,
nothing goes to the compositor), so a program waiting for its window can
be woken when, for example, a terminal's program prints.

## Window management

- Click: focus and raise (the click also reaches the window).
- Title bar: drag to move (it cannot go above the top of the screen);
  double-click to maximise or restore. Its buttons, right to left: close
  (asks the client), maximise or restore, minimise.
- The right and bottom border and the corner (with 5 pixels outside them):
  drag to resize. The client is asked for one size at a time, the next
  when it has answered.
- **Tiling** (step F13): a window dragged with the pointer to the left or
  right edge of the work area fills that half; to the edge's upper or
  lower quarter, that quarter; to the top edge, the whole work area
  (maximised). A tinted outline shows where while dragging. Dragged away,
  it gets its own size back. Super+Left and Super+Right tile the focused
  window (from the other half, they first give it its size back),
  Super+Up maximises, Super+Down restores a tiled or maximised window and
  minimises any other. A maximised window can be dragged out too.
- Alt+Tab: the bottom-most other window comes to the top with the focus
  (a minimised one comes back); repeated, it goes through all of them.
- Alt+F4: ask the focused window to close. F11: full screen and back.
- **Screensaver** (step F15): after `blank=` idle minutes (in
  `~/.config/rustica.conf`, else `/etc/rustica/appearance`; 15 by
  default, 0 never) the screen goes black (`screensaver=blank`) or shows
  the wallpaper dimmed (`dim`); any key or movement brings it back (and
  does nothing else); `lock_on_blank=yes` locks the screen then. Only in a
  session, only while the desktop is shown.
- Keys go to the focused window only. Pointer events go to it while the
  pointer is over its content; while a button is held, to the window it
  went down in. The wheel scrolls the window under the pointer, focused
  or not (step F13).

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
of the damaged rectangles. The cursor is the shape for where the pointer
is (the window's own over its content, the resize arrows on an edge),
drawn with its hot spot at the pointer, or given to the display as its
hardware cursor when it has one (virtio-gpu); the shapes are bitmaps in
`compositor.c`, made from masks by an outline (black outline, white
fill; the text cursor the other way round). Only what changed is drawn and copied. Titles
are UTF-8: the font has ASCII, dashes, a times sign and quotes are drawn
as their ASCII look-alikes, anything else as '?' (step F7).

## Tests

`scripts/run-window-test.sh` (`docs/TESTS.md`): with a USB tablet for exact
pointer positions, two `wintest` windows overlap (screendump pixels), click
to focus, keys to the focused window only, move, resize, maximise and
restore, minimise and Alt+Tab, F11, the clipboard, a killed client, a
client ignoring a close request, an animating client paced by frames,
Alt+F4, and the compositor killed (the console comes back).

## The display's size and heads (step F8)

On virtio-gpu (`DISPLAY.md`) the pointer is the display's hardware
cursor (the arrow sent once, then only its position: moving the pointer
redraws nothing); when the display changes size the compositor makes the
wallpaper again, docks the panel at the new bottom, gives maximised and
full-screen windows the new size, keeps other windows' title bars on the
screen, and draws everything. With several heads the screen is one, side
by side; the panel, maximised and full-screen windows, the lock screen
and notifications stay on the first head, and a tablet's absolute
position covers the first head (as QEMU's window).

## Not yet

- Window decorations drawn with the toolkit's fonts (step F5); resizing
  from the left and top edges; snapping; per-window input methods.
- Drag and drop; clipboard formats other than text.
- Clients are any user's; the session (F6) will limit them to the logged
  in user's processes.
- Double buffering is the client's choice (it may draw while the
  compositor copies only during `COMMIT`); there is no vertical sync.

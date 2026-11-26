# The toolkit (DESKTOP.md step F5)

Rustica's graphical programs are written with **rustica-ui**, a small
toolkit of our own in Rust (`userspace/rust/rustica-ui`), drawing with
tiny-skia and fontdue into a window of the compositor (`WINDOWS.md`).
This page records why, what it has, and what it does not have yet.

## The decision

DESKTOP.md listed three candidates, to be built as the same demo and
compared: (a) Slint with its software renderer, (b) iced with its
tiny-skia renderer, (c) our own toolkit on tiny-skia and a font library.
On 2026-11-15 we chose to **build (c) first** and assess (a) and (b) on
paper, building them only if ours falls short:

| | Slint | iced | rustica-ui (ours) |
|---|---|---|---|
| Licence | GPLv3, or Slint's royalty-free licence with conditions, or commercial | MIT | ours (MIT); dependencies BSD-3, MIT, Apache-2.0, Zlib; Inter under the OFL |
| Porting work | a platform backend (window, events, software renderer's target) | a windowing backend (winit-like) and its async runtime | none: written for the compositor |
| Footprint | a markup compiler at build time, a runtime | large (wgpu or tiny-skia, async executor, many crates) | `uidemo` is 1.1 MB static, 30 crates |
| Look | polished, themable | good, themable | five themes and the user's own, as we draw it |
| Keyboard navigation | yes | partial | yes, from the start |
| Maintenance | upstream, fast moving | upstream, breaking releases | ours to maintain |

Reasons: no licence question to settle before the desktop exists; no
porting layer over a window system that is itself new and still moving
(F4); the immediate-mode design is small enough to know completely, and
every widget the first applications need is in it. The cost is
maintenance and polish that a mature toolkit would bring; if the first
real applications (F7) need more than rustica-ui can sensibly grow,
iced is the fallback to build next (MIT, tiny-skia renderer).

## What it has

- **Drawing** (`gfx.rs`): a canvas over a tiny-skia pixmap: anti-aliased
  rounded rectangles, paths, strokes, clip rectangles; pixel-aligned fills
  done directly; images.
- **Text**: fontdue rasterises TrueType/OpenType glyphs with
  anti-aliasing (cached per character and size), with kerning; Latin,
  Greek and Cyrillic (no complex shaping: Arabic, Indic scripts need a
  shaper such as rustybuzz later). The font is **Inter** (Regular and
  SemiBold, SIL Open Font License, `/usr/share/fonts/inter`, with
  `OFL.txt`). UTF-8 everywhere.
- **Images**: PNG (tiny-skia's decoder) and JPEG (zune-jpeg).
- **Widgets** (`ui.rs`, immediate mode: the program describes the window
  every frame): labels and headings, buttons (and a primary button),
  check boxes, one-line text fields and a multi-line text editor field
  (cursor, selection with Shift or the mouse, Ctrl+A, Ctrl+C, Ctrl+X,
  Ctrl+V through the compositor's clipboard), lists (only the visible
  rows drawn: 1000 items cost nothing), trees, scroll areas, tabs, a menu
  bar with drop-down menus, modal dialogs; rows and columns for layout.
- **Keyboard navigation**: Tab and Shift+Tab move the focus (a ring shows
  it when it came from the keyboard), Enter and Space press buttons,
  arrows, Home, End, Page Up and Down in lists, trees and tabs; F10 opens
  the menus; Esc closes menus and dialogs.
- **Theme** (2026-11-25): the semantic tokens of a theme file
  (`SESSION.md` "Themes": Rustica, the default, Dracula, Forest, Arctic,
  Sakura, and the user's own): `Theme` (`get`/`set` by token, `parse`,
  `to_text`), `theme_named()` (the user's file, the system's, built in),
  `list_themes()`, `current_theme()` (`RUSTICA_THEME` with
  `RUSTICA_THEME_COLORS`, else `theme=` in `~/.config/rustica.conf`, else
  `/etc/rustica/appearance`); a running window follows a change from the
  colours the compositor sends (`theme_from_event`). Widgets use the
  tokens only (hover: `surface_hover`, `accent_hover`); `card()` is a
  clickable area to draw in (the theme previews); `Icon::Fox` the fox. **Scaling**: integer, 1x and 2x (`RUSTICA_SCALE`,
  or `uidemo --scale 2`; else `scale=` in `~/.config/rustica.conf`, which
  Settings writes: step F7).
- **Password fields** (`password_field`): dots, nothing copied out (step
  F6).
- **Control** (step F6): the frame function can ask for a frame after a
  while without input (`wake_after`: a clock), send requests to the
  window system (`requests`: the panel role, activating a window, lock,
  notify, full screen), and read the events that are not input (`events`:
  the window list for the panel). `Ui::padding` sets the margin.
- **Step F7** added, for the applications (`APPS.md`): a **table**
  (`ui.table`: column headers, widths and alignment, a small icon per row
  (folder, file, picture, program, drive, trash), only the visible rows
  drawn; arrows, Page Up/Down, Home, End; Enter or a double-click opens a
  row; the right button selects one; the keys it does not use are handed
  back), multi-line dialog messages, `Request::Title` (the window's title,
  sent when it changes), `Control::waker` (another thread wakes the frame
  function), the right mouse button in `Input`, `focus_if_none`,
  `has_focus`, `remaining_height`. `Key::ctrl()` and `shift()` are public.
  `RUSTICA_WINDOW_AT=X,Y` places a window that has no position of its own
  (scripts and tests).
- **Steps F10-F13** added (2026-11-25): `scroll_page` (a scroll area
  filling the rest of the window: a page that scrolls when the window is
  too small; `remaining_height` inside a scroll area counts its visible
  part), the wheel taken by the innermost list, text area or scroll area
  under the pointer, `button_ex` (a button of a given width, its label
  shortened, plain, primary or marked as current; it reports the right
  button too), `elide`, `row_h`, the icons `Home` and `TrashFull`,
  `Ui::transparent` (frames start transparent and the window gets the
  alpha: the desktop role), `Request::SetPanel`.
- **Running** (`app.rs`): `rustica_ui::run(title, w, h, position, frame)`
  opens the window, turns the compositor's events into input, calls the
  frame function, copies the canvas into the window's buffer and commits.
  A frame is drawn after every mouse button change and key, so none is
  lost when they come quickly.

The C library gained `asin`, `acos` and the `float` forms of the
trigonometric functions for tiny-skia.

## The demo

`uidemo [--scale N] [X,Y]`: a menu bar (File, Edit, View), tabs with a
sign-up form (fields, check box, Submit and Reset, a dialog), a list of
1000 items next to a tree, the text editor field with Greek and Cyrillic,
and a PNG and a JPEG in a scroll area. It prints what is done
(`uidemo: ...`), which `scripts/run-ui-test.sh` checks (`docs/TESTS.md`).

## Writing a program

```rust
fn main() {
    let mut name = String::new();
    rustica_ui::run("Hello", 360, 160, None, |ui, ctl| {
        ui.label("Your name");
        let (_, enter) = ui.text_field("name", &mut name, 240.0);
        if ui.button("Greet") || enter {
            println!("Hello, {name}");
        }
        if ui.button("Quit") {
            ctl.quit = true;
        }
    })
    .unwrap();
}
```

A package in `userspace/rust/` with `rustica-ui = { path =
"../rustica-ui" }`; `build-rust.sh` builds it (`rustux-bins` lists the
programs of a package whose name is not its program's).

## Not yet

- Complex text shaping and right-to-left text; font fallback; emoji.
- Fractional scaling; high-contrast and light themes.
- Accessibility (a screen reader interface).
- Drawing only what changed (every frame is drawn whole, then committed
  whole: fine at these sizes, wasteful for large windows).
- Combo boxes, sliders, progress bars, date pickers, rich text.

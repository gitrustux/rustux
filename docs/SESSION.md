# The desktop session (DESKTOP.md step F6)

Booting to a graphical login, a user's desktop with a panel, launching
programs, locking the screen and logging out; the text console stays on
Ctrl+Alt+F1 throughout.

## Starting it

- **At boot**: "Rustica (desktop)" in the boot menu (an installed system's
  `rustica-desktop.conf`, `INSTALL.md`) or "Try Rustica (desktop)" on the
  live stick (`BOOT.md`) put `desktop` on the kernel command line: init
  boots the **graphical** target (`SERVICES.md`), everything multi-user has
  plus `desktop.svc`.
- **From the console**: `desktop` (root) asks the service manager to start
  the `desktop` unit; `desktop --stop` stops it. Ctrl+Alt+F1 goes back to
  the console, Ctrl+Alt+F7 to the desktop.

## The pieces

```
 init (service manager) -- desktop.svc --> desktop --run (root)
                                             |-- compositor (root; control channel: "user NAME")
                                             |-- greeter (root; stdin/stdout: "login\tNAME\tPASSWORD" -> "ok"/"fail")
                                             `-- job of the session, as the user:
                                                   panel --> launcher (the menu) --> applications
                                                     `-- desk (the desktop's icons)
```

- **desktop --run** (`c-progs/desktop.c`): starts the compositor, then over
  and over: the greeter; when it reports a name and password, desktop
  checks them against `/etc/shadow` (as `login` does: a locked account is
  refused) and answers; then the session: a job of its own, the panel
  started in it as the user (`rx_spawn_as`: the user's uid, groups, HOME,
  USER, PATH, in the home directory). When the panel ends (Log out), the
  job is killed with everything in it, and the greeter comes back. The
  first session starts without the login if `/etc/autologin` names a user
  (the live image: root). If the compositor ends, so does desktop --run,
  and the service manager starts it again.
- **greeter** (Rust, rustica-ui): full screen, the host name, Name and
  Password (dots), Log in; "Wrong name or password" on failure.
- **panel** (Rust, rustica-ui), the session's first program and its life
  (step F10): docked at an edge (the compositor's panel role: no frame,
  always on top, never focused; maximised windows stay out of it) and
  made of applets in three zones, start, centre and end, as Cinnamon's:
  the **fox** button (the menu, since 2026-11-25 a fox in place of the
  word "Apps", which FoxDen holds: a second click, or the Super key pressed alone,
  which the compositor passes on as `RX_WINDOW_EV_DESKTOP` with
  `RX_WINDOW_DESKTOP_LAUNCHER`, opens it or closes it), the launchers
  (pinned apps: FoxDen, Files, Terminal), the window list (click: focus,
  or minimise when it has the focus; a minimised one in brackets; the
  buttons share the room left, their titles shortened, and past 90 pixels
  each a "+N" button stands for the rest, so the end of the panel never
  leaves the screen), Updates (when there are some), the keyboard layout
  (`/etc/keymap`), volume (a placeholder), the network, the clock, Lock,
  Log out, and the power button at the far end (`powermenu`: Lock screen,
  Log out, Restart, Shut down, Power settings; Suspend and Hibernate wait
  for ACPI). Its settings are `~/.config/rustica/panel.conf` (else
  `/etc/rustica/panel.conf`): `position=bottom|top|left|right`, `size=`
  (a bottom or top panel's height, 36), `width=` (a side panel's, 180),
  `autohide=yes|no`, `start=`, `centre=`, `end=` (applet names: `menu
  launchers windows updates keyboard volume network clock lock logout
  power`),
  `launchers=` (`.app` names or FoxDen app IDs). Settings > Panel writes
  them, and a right click on the panel opens it; the panel reads them
  again within a second and asks the compositor to dock it anew
  (`SET_PANEL`). It starts the commands in `/etc/autostart` and the
  desktop's icons (`desk`, again if they fail).
- **launcher**, the menu (Rust, step F11): a search field, focused at
  once (apps by name, description and keywords; Settings pages; Lock,
  Log out, Restart, Shut down; files in the home folder by name; Up, Down,
  Enter, Esc), or, without a search, the categories (from `categories=`
  in `.app` files and `Categories:` in apps' manifests, freedesktop.org
  names) and their apps; a side bar of favourites
  (`~/.config/rustica/favorites`) and places (Home, Documents, Downloads,
  Pictures, Trash); for the app chosen Open, Favourite, Pin to panel, To
  desktop. Apps come from `/usr/share/applications/*.app` (`name=`,
  `exec=`, `comment=`, `categories=`, `keywords=`; `types=` lists the
  file name extensions an application opens, for Files: step F7,
  `APPS.md`) and the installed apps (FoxDen). Log out and Lock end the
  menu with status 10 and 11, which the panel carries out.
- **desk**, the desktop's icons (Rust, step F12): the compositor's
  desktop role. Home, Trash (full or empty), the drives under `/media`,
  then `~/Desktop`'s folders, files and app shortcuts (`.app` files: the
  menu's To desktop; `~/Desktop` is made when the session starts); in
  columns, or where one was dragged
  (`~/.config/rustica/desktop-icons`). A double click or Enter opens one;
  dropped on the trash, or with Delete, it goes to the trash. Files,
  folders and shortcuts can be dragged into Files (and Files' rows onto
  the desktop, a folder icon or the trash), through the compositor's drag
  and drop; Home, Trash and drives stay. A right
  click: Open, Move to the trash, Empty the trash; on the desktop New
  folder, Open in Terminal, Change background, Desktop settings. Settings
  > Desktop (`~/.config/rustica/desktop.conf`) hides them or some of them.
- **compositor** (`WINDOWS.md`): the wallpaper, notifications (top
  right of the work area, 5 seconds, three at most: `notify TEXT`, or
  `RX_WINDOW_NOTIFY` from any client), the lock screen, tiling, the
  desktop's own menu when `desk` is not running.

## Appearance (2026-11-24; themes 2026-11-25)

### Themes

Five themes come with the system, each a file of **semantic tokens** in
`/usr/share/rustica/themes/NAME.theme`; users make their own in
Settings > Appearance, kept in `~/.config/rustica/themes/NAME.theme`:

| Theme | Background | Surface | Primary | Accent | Panel |
|---|---|---|---|---|---|
| **Rustica** (the desktop's default) | `#101010` | `#202024` | `#FF6500` | `#D71920` | `#151416` |
| **Dracula** (the server console's default) | `#282A36` | `#343746` | `#BD93F9` | `#FF79C6` | `#21232E` |
| **Forest** | `#101B17` | `#192720` | `#4F9D69` | `#C5A66A` | `#0C1511` |
| **Arctic** | `#101923` | `#192A3A` | `#69C9E8` | `#A9DFF2` | `#0B141D` |
| **Sakura** | `#1D1520` | `#30202F` | `#D77BA6` | `#F0AFCB` | `#171019` |

A theme file is `key=#RRGGBB` lines (`#` comments) with `name=` (what
people see) and these tokens: `bg`, `surface`, `surface-raised`,
`surface-hover`, `border`, `text`, `text-muted`, `primary`,
`primary-hover`, `accent`, `selection`, `success`, `warning`, `danger`,
`info`, `taskbar` (the panel), and `ansi0`-`ansi15` (the terminals' 16
colours, normal then bright). A token a file lacks is Rustica's. Its name
(the file's) is a-z, 0-9 and `-`, at most 15. What is worked out rather
than given: the text on primary buttons (white if its contrast with the
primary colour is 2.9:1 or more, else the background) and the keyboard
focus ring (`primary-hover`).

- **One place:** every program draws from the tokens. rustica-ui (`Theme`
  in `ui.rs`: `background`, `surface`, `surface_hi`, `surface_hover`,
  `border`, `text`, `dim`, `accent` = primary, `accent_hover`,
  `secondary` = accent, `selection`, `good`, `warning`, `danger`,
  `info`, `taskbar`, `ansi`) reads the files; so does the compositor
  (`themefile.h`: title bars from `surface`, the unfocused ones half way
  to `bg`, the focused border `primary`, the close button `danger`,
  menus `surface` with `surface-hover`, the lock screen and
  notifications `surface`), and the console (`theme --console`: `text`,
  `bg` and the 16 ANSI colours). Rustica (and Dracula in rustica-ui) is
  built in too, should a file be missing.
- **Where it is chosen:** `theme=` in `~/.config/rustica.conf`, else in
  `/etc/rustica/appearance`; a user's own theme file of a name comes
  before the system's. Settings > Appearance > Themes shows a card per
  theme (a small picture of its background, panel, a window, text, a
  selection, a primary button and its accent) and switches at a click;
  `theme --desktop NAME` does the same at the console; it is kept across
  restarts (the configuration file).
- **How it spreads, at once:** the compositor looks at the session user's
  file every second; on a change it reads the theme file, redraws, and
  sends every window `RX_WINDOW_EV_APPEARANCE` with the theme's name and
  its colours (the file's text after the event), so every window changes
  without a restart, and a new window gets the same as its first event.
  Sandboxed apps cannot read the user's files: appctl starts them with
  `RUSTICA_THEME` and `RUSTICA_THEME_COLORS` (the text, `;` for newlines),
  and the compositor's events do the rest. The compositor reads the
  user's files as root, so it takes only plain files that user owns.
- **A theme of one's own:** Settings > Appearance > Make your own from
  this one... copies the chosen theme into an editor: a name, each token
  by hex code beside a swatch, a preview that follows, warnings where text
  would be hard to read (under 4.5:1 on the background or surfaces, 3:1 on
  primary buttons); Save and use it writes
  `~/.config/rustica/themes/ID.theme` (the ID from the name) and switches
  to it. The user's own themes can be changed or deleted there; the
  terminal colours are kept from the theme it was made from. A system
  theme's name cannot be taken.
- **The fox:** `/usr/share/rustica/fox.svg` (and `fox-outline.svg`), drawn
  by the panel's menu button (rustica-ui `Icon::Fox`, `FOX_OUTLINE`,
  `FOX_EYES`) and in the theme previews, is the brand's orange `#FF6500`
  in every theme.

### Wallpapers

Separate from the theme (any wallpaper with any theme): the Rustica fox
(the default; a portrait cut on screens taller than wide), Dracula
(liquid violet), Forest (a forest at night), Arctic (aurora over ice),
Sakura (cherry blossom), all QOI in `/usr/share/backgrounds/` (made with
`scripts/make-wallpaper.py`); a gradient in the theme's colours; or a
picture of the user's.

- Where it is set: `wallpaper=` (`rustica`, `dracula`, `forest`,
  `arctic`, `sakura`, `gradient`, or a QOI file of the user's) in
  `~/.config/rustica.conf`, else in `/etc/rustica/appearance`. Settings >
  Appearance writes it: a button per wallpaper, or a picture (PNG, JPEG or
  QOI) from any folder, by its path, the file chooser, or the list of
  `~/Pictures` and the home folder, which becomes
  `~/.config/rustica/wallpaper-N.qoi`. Files > Set as wallpaper runs
  `settings --wallpaper FILE`, which does the same without a window.
- A right click on the wallpaper opens the desktop's menu (Change
  background... opens Settings > Appearance).
- The console: `theme --console NAME` (OSC 4, 10 and 11 sequences, which
  the kernel console understands); as root it is kept in
  `/etc/console-theme`, which login sets at every start. `theme list`
  lists the system's themes and the user's own.

## Logging out of the live system

On the live image `/etc/autologin` names root, whose session starts
without the login. After Log out, the greeter (`greeter --auto root`)
offers **Continue as root**, which needs no password (the desktop accepts
it only for the user `/etc/autologin` names), beside the usual name and
password. An installed system has no `/etc/autologin`, so its login always
asks.

## Locking

Lock in the panel, Super+L or Ctrl+Alt+L: the compositor hides every window
behind the lock screen and takes all keys; the session's user's password
and Enter unlock it ("Wrong password" otherwise). A user without a
password (the live image's root) cannot lock: a notification says so.
Ctrl+Alt+F1 still reaches the console (whose own login protects it).

## Who may do what

The compositor, desktop and greeter run as root: they own the screen and
the input and read `/etc/shadow`. Everything of the session runs as the
user. Window clients may be any user's (the compositor does not check yet).

## Not yet

- Several sessions, switching users, logging in as root graphically from
  a system with a root password (it works; nothing stops it).
- Volume and network in the panel (phases D and later), notifications
  with actions, a notification history.
- The lock screen with the toolkit's fonts; idle locking and screen
  blanking.
- Session services under the service manager per user: the session is a
  job, not a set of units.

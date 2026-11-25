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
                                                   panel --> launcher --> applications
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
- **panel** (Rust, rustica-ui), the session's first program and its life:
  docked at the bottom (the compositor's panel role: no frame, always on
  top, never focused; maximised windows stop above it). FoxDen (the app
  store, `FOXDEN.md` step FD5) beside Applications. Applications (the
  launcher: a second click, or the Super key pressed alone, which the
  compositor passes on as `RX_WINDOW_EV_DESKTOP` with
  `RX_WINDOW_DESKTOP_LAUNCHER`, opens it or closes it), a button per window (click: focus it, or minimise it when it
  has the focus; a minimised one in brackets), the keyboard layout
  (`/etc/keymap`), volume and network placeholders, the clock (redrawn
  every 5 seconds), Lock, Log out. It starts the commands in
  `/etc/autostart`.
- **launcher** (Rust): the applications in `/usr/share/applications/*.app`
  (`name=`, `exec=`, `comment=` lines; `types=` lists the file name
  extensions an application opens, for Files: step F7, `APPS.md`), in a
  list; Enter or Open starts one, Esc or Close ends it. Step F7 added
  Terminal, Files, Settings, the Text Editor and the Image Viewer (the
  last two apps since FoxDen step FD6b, listed as installed apps).
- **compositor** (`WINDOWS.md`): the wallpaper, notifications (top
  right, 5 seconds, three at most: `notify TEXT`, or `RX_WINDOW_NOTIFY`
  from any client), the lock screen, the desktop's menu.

## Appearance (2026-11-24)

Two themes: **Rustica**, the desktop's default (fiery orange `#FF5A1F`,
fox red `#C8101E`, near-black `#101012`, smoky charcoal `#1C1B20`, warm
white `#F5F2EF`, amber `#FFB347`; Inter throughout, the title bars too),
and **Dracula**, the server console's default and the desktop's earlier
look. The wallpaper is the Rustica fox (`/usr/share/backgrounds/`, QOI;
the portrait one on screens taller than wide), a gradient in the theme's
colours, or a picture of the user's.

- Where it is set: `theme=` and `wallpaper=` (`rustica`, `gradient`, or a
  QOI file of the user's) in `~/.config/rustica.conf`, else in
  `/etc/rustica/appearance`. Settings > Appearance writes them: a picture
  (PNG, JPEG or QOI) from any folder, by its path, the file chooser, or
  the list of `~/Pictures` and the home folder, becomes
  `~/.config/rustica/wallpaper-N.qoi`. Files > Set as wallpaper runs
  `settings --wallpaper FILE`, which does the same without a window. `theme
  --desktop NAME` sets the theme.
- How it spreads: the compositor looks at the session user's file every
  second; on a change it makes the wallpaper again, redraws, and sends
  every window `RX_WINDOW_EV_APPEARANCE` with the theme's name, so
  rustica-ui windows (and the Terminal) change at once. Sandboxed apps
  start with `RUSTICA_THEME` from appctl, since they cannot read the
  user's file. The compositor reads the user's files as root, so it takes
  only plain files that user owns.
- A right click on the wallpaper opens a menu (Change wallpaper...,
  Appearance settings...); the compositor sends the choice to the panel
  (`RX_WINDOW_EV_DESKTOP`), which runs as the user and opens Settings >
  Appearance.
- The console: `theme --console NAME` (OSC 4, 10 and 11 sequences, which
  the kernel console understands); as root it is kept in
  `/etc/console-theme`, which login sets at every start.

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

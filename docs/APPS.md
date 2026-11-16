# The applications (DESKTOP.md step F7)

Terminal, Files and Settings, and the first two of the applications "next
in line", the Text Editor and the Image Viewer. They are one Rust package,
`userspace/rust/rustica-apps` (programs `terminal`, `files`, `settings`,
`editor`, `viewer`), built on the toolkit (`GUI.md`) and started from the
launcher (`/usr/share/applications/*.app`) or a shell.

## Pseudo-terminals

A terminal in a window needs what the text console has: a line
discipline between the keys typed and the program reading them, and a
size the program can ask for.

- **The line discipline** moved out of init into the C library
  (`<rustux/ldisc.h>`, `libc/src/ldisc.c`): cooked mode (echo, Backspace,
  Ctrl+U, Ctrl+W, the line sent on Enter, Ctrl+D), raw mode, ISIG (Ctrl+C
  clears the line and raises `RX_CONSOLE_SIGNAL_INTERRUPT` on the
  program's end, as before) and ICRNL. init runs it for the text console
  (one key at a time from the input service); a pseudo-terminal runs the
  same code on a stream of bytes, split into keys (characters, UTF-8
  sequences, escape sequences; one cut by the end of a write waits for
  its rest).
- **posix_openpt** (`libc/src/pty.c`) makes a console channel
  (`<rustux/console.h>`) and gives the calling process its console end as
  the master: `write(master)` types (through the line discipline, in this
  process), `read(master)` returns what the programs print plus the echo.
  The other end is the **slave**, the same kind of handle a program on the
  text console has, so the shell, `top` and everything else run on it
  unchanged. `open(ptsname(master))` gives the slave once as a file
  descriptor, to pass to a program (`posix_spawn` with `dup2`). When every
  copy of the slave is closed (the program ended), reading the master
  fails with `EIO`. `grantpt` and `unlockpt` do nothing; `poll` works on
  the master.
- **The size**: a channel has a terminal size now, the kernel property
  `RX_PROP_TTY_SIZE` (5), the same for both ends (set with WRITE, read
  with READ; `SYSCALL.md`). `ioctl(master, TIOCSWINSZ)` sets it, a
  program's `ioctl(TIOCGWINSZ)` reads it; init sets the text console's
  from the screen, so the old fallback (the kernel console's size) is
  only for channels without one.

Unlike Linux there is no `/dev/pts` directory, no controlling terminal and
no SIGWINCH: a slave has a name only in the process that opened the
master, Ctrl+C reaches the shell as before, and programs read the size
again when they redraw (`top` does each second).

`ptytest` (in `runtests terminal`) runs a program of its own and the
shell on pseudo-terminals and checks the size, echo, Backspace, cooked
and raw mode, Ctrl+C and the end.

## Terminal

`terminal [-e PROG ARGS]`: the shell (`TERM=xterm-256color`) on a
pseudo-terminal in a window. The emulation (`rustica-apps/src/vt.rs`) has
the text console's sequences (`DISPLAY.md`): cursor movement, erasing,
inserting and deleting lines and characters, scrolling regions, the
alternate screen, 16, 256 and true colours (kept as they are, not
approximated), bold, dim, underline and reverse; cursor position and
device attribute reports; OSC 0/2 titles. A scrollback of 5000 lines.

The text is JetBrains Mono (OFL, `/usr/share/fonts/jetbrains-mono`); box
drawing and block characters are drawn as lines and rectangles so they
join between cells. The window is 80 x 24 cells at first and any size
after a resize (the programs are told).

Tabs (Ctrl+Shift+T, the + button; Ctrl+Shift+W or the tab's x closes,
Ctrl+PgUp/PgDn switch); a tab closes when its program ends, the window
when the last one does. Selection with the mouse, Ctrl+Shift+C copy,
Ctrl+Shift+V or the middle button paste (lines end in CR, as Enter);
Ctrl+= / Ctrl+- / Ctrl+0 font size; Shift+PgUp/PgDn and the wheel scroll
back. Every other key goes to the program as the input service's layout
made it (the same bytes as the console's).

The window is drawn by the terminal itself (rustica-window and the
toolkit's canvas, not immediate-mode widgets); a thread per tab reads the
master and wakes the window's event loop (`rx_window_wake`, `WINDOWS.md`).

## Files

`files [FOLDER]`: a menu bar (File, Edit, View, Go), Back, Up, New folder
and the path (type one and Enter), the side bar and the folder as a table
(name with an icon, size, modified; folders first).

- **Opening**: Enter or a double-click opens a folder, or a file with the
  application whose `types=` lists its extension (`.app` files): text with
  the Text Editor, PNG and JPEG with the Image Viewer. File > Open in
  Terminal starts a terminal in the folder.
- **Changing**: Ctrl+C / Ctrl+X / Ctrl+V (a folder is copied with all it
  holds; a name that exists gets " (copy)"; a move to another disk is a
  copy and a delete), F2 renames, Ctrl+Shift+N makes a folder, Delete
  moves to the trash, Shift+Delete deletes for good (after asking).
- **The trash** follows freedesktop.org's: `~/.local/share/Trash/files`
  and `info/NAME.trashinfo` (`Path=`, `DeletionDate=`). In the trash:
  Restore puts a file back where it came from, Empty trash deletes all.
  Something on another disk (a USB stick) cannot be moved there: Files
  offers to delete it instead.
- **Properties** (Alt+Enter, File > Properties): kind (and the application
  that opens it), size (of everything in a folder), modified, accessed and
  changed times, owner and group, permissions.
- **Disks**: the side bar has Home, Computer (/), Trash, and every USB
  stick or removable disk with a filesystem (`lsblk -J`, every 3 seconds).
  Clicking one that is not mounted mounts it at `/media/LABEL` (`mount`,
  step 14) and shows it; Eject under the side bar unmounts the one shown
  (`umount`: written out, safe to pull) and says so. The system's own
  disks are left out.

Mounting needs root for now (the mount tool starts the filesystem's
server): in a user's session Files shows mount's error.

## Settings

`settings [SECTION]`, a list of sections on the left:

| Section | What | Where it is kept |
|---|---|---|
| Display | the scale of windows: 100% or 200% (`RUSTICA_SCALE` still wins) | `~/.config/rustica.conf` (`scale=`), the user's |
| Keyboard | the layout, for the console too | `/etc/keymap` (`localectl set-keymap`) |
| Mouse | the pointer speed, 25-400% of the mouse's own movement (relative mice; a tablet is exact) | `/etc/input.conf` (`inputctl set-pointer-speed`) |
| Network | the cards (state, address, gateway, traffic, the DHCP lease); each one Automatic (DHCP), a fixed address and gateway, or off; DNS servers of your own (network step D6) | `/etc/network.conf` (`netcfg`, applied at once) |
| Date & Time | the clock (local time) and the time zone (a list to filter) | the RTC (`timedatectl set-time`), `/etc/timezone` |
| Users | the users (root and uid 1000 and up), add one (name, password, administrator), remove one with their home, make one an administrator or not | `/etc/passwd`, `group`, `shadow` (`useradd -m`, `chpasswd`, `usermod -G`, `userdel -r`) |
| Disks | disks and partitions: size, filesystem, where mounted (read only) | `lsblk` |
| About | Rustica's version and codename (`/etc/os-release`: Rustica 0.1, RedFox), the kernel, the computer's name, processors, memory, graphics, how long it has run | |

The user's own settings are written directly. The system's are root's: in
root's session Settings runs the commands itself; in another user's it
asks for **their** password once and runs them with `sudo -S` (so the
user must be allowed by `/etc/sudoers`: an administrator, group `sudo`);
a refused password or a failing command shows its message.

The input service reads `/etc/input.conf` at start and whenever the
layout is reloaded (`INPUT.md`); the panel shows the layout as it changes.

## Text Editor and Image Viewer

- `editor [FILE]`: the toolkit's text editor field over the whole window,
  a status line (lines, characters), File > Save (Ctrl+S; a new file asks
  for a name) and Quit (Ctrl+Q; with changes it asks to save). The title
  marks unsaved changes.
- `viewer FILE`: the picture made smaller (never larger) to fit the
  window, centred; Left and Right step through the pictures in its folder.

## Tests

- `runtests terminal` (QEMU, every build): pseudo-terminals (`ptytest`).
- `scripts/run-apps-test.sh` (QEMU with a USB tablet and a USB stick):
  Terminal (a command, a second tab, top, tabs closing), Files (copy to
  the stick mounted from the side bar, paste, rename, trash, new folder,
  properties, a file opened and saved in the Text Editor, eject; the copy
  is checked on the stick image afterwards), Settings as root (mouse
  speed, time zone, scale, a new administrator, the network: a fixed
  address and a DNS server, then DHCP again, with the panel's Net
  following) and as that user (her password through sudo), and the
  keyboard layout.

## Not yet

- A pseudo-terminal per tab is in the terminal's process: a terminal that
  crashes ends its tabs' shells (they get end of input).
- Drag and drop, several selected files, thumbnails, searching; mounting
  as a user (a polkit-like rule); trash on USB sticks.
- Settings: the screen resolution (step F8), key repeat, sound, per-user
  keyboard layouts; in Network: IPv6 settings, the host name, the
  firewall (`fwctl`), Wi-Fi.
- The System Monitor and the Calculator (the other applications "next in
  line").
- Seen in QEMU only: while Files reads a USB stick (every 3 seconds, for
  the side bar), a very short click of the USB tablet can be missed (the
  test holds the button half a second). To look at with the USB HID
  driver.

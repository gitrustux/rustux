# Users, permissions and system settings (step 17)

Rustica is a multi-user system: every process acts as a user, files have
owners and permission bits that the filesystem servers enforce, people
sign in at a login prompt, and `sudo` and `su` give root's rights to those
allowed to have them.

## Who a process is

The kernel keeps each process's **credentials**: real, effective and
saved user and group ids, up to 16 supplementary groups, and the umask
(`rx_creds_t`, docs/SYSCALL.md `PROCESS_GET_CREDS`). A new process gets a
copy of its creator's, so a user's shell and everything it starts act as
that user. Only root may change to another user, and the change is one
way: a non-root process may only switch among its own ids.

Programs that start something as another user (`login`, and `authd` for
`su` and `sudo`) are root; they set the credentials of the new process
before it starts (`rx_spawn_as()`), so they never lower their own.
There are no setuid programs: a set-user-id bit on a file is kept but
means nothing.

Every message on a channel carries a copy of its writer's credentials,
and servers read them with the message (`rx_channel_read_creds()`). That
is how a server knows who asks:

| Server | Decides by the sender |
|--------|-----------------------|
| fsd, ext2fs, fatfs | every path and file request (below) |
| devmgr | opening a disk or partition: root or group `disk` (gid 6); publishing one: root |
| init `/svc/system` | power-off, reboot, setting the clock: root |
| authd `/svc/auth` | su, sudo, passwd (below) |
| the kernel | `task_kill` of another user's process: root only; handles to them from a job: look and wait only |

fsd forwards path requests to mounted filesystems on its own channel, so
it adds the original sender (`RX_FS_OP_AS`); a server takes that only
from a root sender.

## Files

The usual Unix rules, in every filesystem server (`c-progs/perm.h`):

- Read, write and execute (search, for directories) bits for the owner,
  the group and others; root may do anything but run a file without any
  x bit. Each directory on a path needs search permission.
- Making, removing or renaming a name needs write and search permission
  on the directory; in a sticky directory (`/tmp`, mode 1777) only the
  file's owner, the directory's owner or root may remove or rename it.
  Moving a directory to another parent needs write permission on it
  (its `..` changes).
- `chmod`: the owner or root; `chown`: root (the owner may change the
  group to one of its own); setting times: the owner or root (anyone who
  may write, to "now"). A new owner clears the setuid and setgid bits.
- New files belong to the creator's effective user and group, or in a
  setgid directory to its group (new directories there are setgid too).
  The umask applies (the C library takes it out of the mode).
- Mounting, unmounting and the final shutdown request are root's.

On tmpfs (fsd's own) and ext2/3/4 the owners and modes are kept (on disk
for ext). FAT has no owners: everything on it is root's (directories
0755, files 0644), so others may read and only root may change it.

`/etc/shadow` is 0600 and `/etc/sudoers` 0440 (root only); `/root` is
0700, `/tmp` 1777, a user's home 0750.

## Accounts

The files are Linux's:

- `/etc/passwd`: `name:x:uid:gid:comment:home:shell`
- `/etc/group`: `name:x:gid:member,member`
- `/etc/shadow`: `name:hash:lastchange:min:max:warn:inactive:expire:`,
  the hash SHA-512 crypt (`$6$...`, as glibc makes it; `!` locks the
  account, an empty field needs no password)

The image has `root` (locked: no password works; the live system logs it
in automatically), `nobody`, and groups `root`, `disk` (6), `sudo` (27),
`users` (100), `nogroup`. `useradd` gives new users uids from 1000, a
group of their own and a locked password until `passwd` or `chpasswd`
sets one.

## Signing in

init runs `login` on the console (a one-screen system has one), in a job
of its own, and again when it ends. `login` asks for a user name and
password (three tries, two seconds after a wrong one), then starts the
user's shell from `/etc/passwd` in the home directory with `HOME`,
`USER`, `LOGNAME`, `SHELL` and `PATH` set, after `/etc/motd`.

Autologin (no password): `/etc/autologin` names the user (the live image:
root), or the kernel command line says `autologin=USER`; `login` on the
command line asks anyway. A disk root filesystem gets the image's `/etc`
files once, when it is fresh (no `/etc/passwd` yet); from then on its
`/etc` is its own, so removing `/etc/autologin` there makes it ask.

The shell's prompt is `rustux:/dir > ` for root and
`name@rustux:/dir $ ` for others.

## su, sudo, passwd

They are clients of **authd**, a root service init starts
(`/svc/auth`, `<rustux/auth.h>`). The client sends what to run, its
working directory, environment and terminal (stdin, stdout, stderr);
authd knows who asks from the kernel, asks for a password on that
terminal, and runs the program as the target user in a job of its own.
The client waits; if it is ended (Ctrl+C), authd ends the program.

- `su [-] [USER] [-c CMD]`: the target's password (none when root asks),
  then its shell. `-` gives a login environment.
- `sudo [-u USER] [-i|-s] [-S] CMD...`: allowed by `/etc/sudoers`; the
  caller's own password (remembered for 5 minutes), unless the rule says
  `NOPASSWD:`. The environment is reset: `PATH=/bin`, the target's
  `HOME`, `USER`, `LOGNAME`, `SHELL`, the caller's `TERM`, `SUDO_USER`,
  `SUDO_UID`, `SUDO_GID`. `-S` reads the password from stdin; `-l` says
  whether one may use sudo at all.
- `passwd [USER]`: one's own password (after the current one), or root
  any user's.

`/etc/sudoers` lines: `WHO HOST=(RUNAS) [NOPASSWD:] COMMANDS`, WHO a user
or `%group`, HOST not checked, RUNAS `ALL` or user names (default root),
COMMANDS `ALL` or full paths. The image allows root and group `sudo`.
Each use (and refusal) goes to `/var/log/auth.log`.

## System settings and shutdown

- `hostname NAME`: `/etc/hostname` (root).
- `timedatectl`: the time, the zone; `set-timezone ZONE` (`/etc/timezone`,
  from `/usr/share/zoneinfo/zones`), `set-time` (through `/svc/system`),
  `set-local-rtc 0|1` (`/etc/adjtime`: the hardware clock keeps UTC or,
  as Windows does, local time).
- `localectl`: the keyboard layout; `set-keymap NAME` writes
  `/etc/keymap` and the console uses it at once.
- `poweroff`, `reboot` (root, or `sudo poweroff`): init ends the session
  and authd (with what they started), every filesystem writes everything
  out and is marked clean (`SHUTDOWN_ALL`), the root filesystem last, then
  the power goes off or the computer restarts.

## Not done (yet)

- Services run as root; the service manager (step 18) will run them as
  other users.
- No access control lists, no capabilities split from root, no PAM; the
  shadow file's ageing fields are kept but not enforced.
- The sudo timestamp is per user, not per terminal; there is no
  `sudo -k`.
- FAT mounts cannot be given to a user (`uid=` options).

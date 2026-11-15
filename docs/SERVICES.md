# Services and logging (step 18)

`init` is Rustica's service manager. After the early boot (fsd, the device
manager, the keyboard driver, the root filesystem), everything runs as a
**service**: the system log, authd, the mounts of `/etc/fstab`, the login
on the console. Each one is described by a unit file, started in dependency
order, run as its user, restarted when its unit says, and its output goes
to the system log. `svc` (or `systemctl`) controls them and `logs` (or
`journalctl`) shows the log.

The code: `userspace/c-progs/svcmgr.h` (included by `init.c`), `svc.c`,
`logd.c`, `logs.c`, `logger.c`, `logfile.h`; the protocols are in
`<rustux/auth.h>` (`RX_SYSTEM_OP_SVC`) and `<rustux/log.h>`; libc has
`syslog()`.

## Units

A unit is `/etc/svc/NAME.svc` (Linux keeps the port list in `/etc/services`,
so the directory is `/etc/svc`): `key=value` lines, `#` comments,
`[Section]` lines ignored, so a simple systemd unit needs few changes.

| Key | Meaning (default) |
|-----|-------------------|
| `description` | what `svc list` shows |
| `exec` | the program and its arguments; a name without `/` is in `/bin`; `"..."`, `'...'` and `\` quote as in the shell |
| `user` | the user it runs as (root) |
| `type` | `simple`: runs until stopped; `oneshot`: done when it exits with 0 (simple) |
| `restart` | `no`, `on-failure` (a code other than 0, killed, crashed), `always` (no) |
| `restart_delay` | seconds before a restart, decimals allowed (1) |
| `start_limit` | more starts than this within 30 s fail it; 0 never (5) |
| `needs` | units started first; it fails if one of them fails; they are started even if not enabled |
| `after` | units it waits for if they are starting anyway (ordering only) |
| `ready` | a service path (`/svc/log`): the unit counts as up once it can be connected to (10 s at most); without it, as soon as the program runs |
| `target` | `multi-user`, `rescue`, `install`, `graphical`, or several (multi-user) |
| `env` | `NAME=VALUE`, repeatable; besides PATH, HOME, USER, LOGNAME, SHELL, PWD, SERVICE |
| `dir` | the working directory (`/`) |
| `log` | `console`: stdout and stderr to the screen instead of the log |
| `console` | `yes`: the unit holds the console (fds 0-2 are the console, with the shell's control channel) |
| `system_job` | `yes`: it gets the system job (to look at and end other processes: authd) |
| `keep_processes` | `yes`: what it started stays when its program ends (`mounts`: the filesystem servers, which end when the filesystems are shut down); not ended by `svc stop` or at power-off either (step 19) |

systemd's spellings `ExecStart`, `Requires`, `WantedBy`, `RestartSec`,
`Environment`, `WorkingDirectory` are accepted too. Unknown keys are logged
and ignored.

A unit is **enabled** when `/etc/svc/enabled/NAME` exists (`svc enable`
makes it). At boot the enabled units whose targets include the boot's
target start.

The image's units:

| Unit | What | Targets |
|------|------|---------|
| `logd` | the system log, `ready=/svc/log`, output on the screen | multi-user, rescue |
| `authd` | su, sudo, passwd; `system_job` | multi-user |
| `mounts` | `mount -a` (oneshot, `keep_processes`) | multi-user, rescue |
| `console` | `login` (with `-f USER` for autologin) | multi-user |
| `rescue-shell` | a root shell, no login | rescue |
| `installer` | `rustica-install` on the console (step 20) | install |
| `netd` | the network service, `ready=/svc/net` (`NETWORK.md`) | multi-user |
| `emergency` | built in, never in a file: a root shell when nothing else can hold the console | none |

Without `/etc/svc` init uses the same units built in (`svc list` says
`builtin`). A disk root made before step 18 gets `/etc/svc` from the boot
image at its first boot: fsd copies a directory of the image's `/etc` that
the disk has none of (files of `/etc` itself only on a fresh disk, as in
step 17).

## How they run

- **Order.** A waiting unit starts when every `needs` unit is up (running
  and ready, or a oneshot that has exited with 0) and no `after` unit is
  still on its way up. A `needs` unit that does not exist or fails fails
  it too.
- **Users.** init starts the program with the user's credentials
  (`rx_spawn_as`, as `login` does): its uid, gid and groups from
  `/etc/passwd` and `/etc/group`, and their environment.
- **Jobs.** Each unit runs in a job of its own; when its program ends,
  init ends the job, so nothing it started is left over (unless
  `keep_processes`). Stopping a unit ends the job.
- **Output.** Its stdout and stderr are one end of a socket; logd reads the
  other end (`RX_LOG_OP_STREAM`, which only root may send) and records
  each line under the unit's name, with its pid and uid. A line starting
  `<N>` (0-7) has that priority, as sd-daemon's do.
- **Restarts.** When the program ends, `restart` decides; the restart
  waits `restart_delay`. A unit started `start_limit` times within 30 s is
  failed instead (`svc reset-failed` clears it; `svc start` of a failed
  unit does too). init logs every start, exit and restart.
- **The console.** One unit holds it. If none can (the console unit is
  disabled, failed or missing), init starts `emergency`, a root shell, and
  says so on the screen.
- **States** (`svc list`): `inactive`, `waiting` (for what it comes
  after), `starting` (waiting for `ready`), `running`, `exited` (a
  oneshot that finished), `failed`, `stopping`, `restarting`.

## Targets

`multi-user` is the default: every enabled multi-user unit, ending in the
login. `rescue`, `single` or `S` (or `target=rescue`) on the kernel command
line boots the rescue target instead: logd, the mounts and a root shell on
the console with no login, for repairs (a forgotten password: `svc start
authd`, then `passwd USER`). `svc isolate multi-user` starts the rest from there, `svc isolate
rescue` goes back. `installer` (or `target=install`) boots the install
target (step 20): logd, the mounts and the installer on the console;
`logd.svc` and `mounts.svc` name it among their targets. `desktop` (or
`target=graphical`) boots the graphical target (desktop step F6,
`SESSION.md`): everything multi-user has, and the units that name
`graphical`, which is `desktop.svc` (the compositor, the graphical login
and the session; the console login stays on Ctrl+Alt+F1). `svc isolate
graphical` switches to it, `svc start desktop` (or the `desktop` command)
starts the desktop alone.

## Shutdown

`poweroff` and `reboot` (or `svc poweroff`) ask init: it stops every unit
(their jobs are ended; up to 2 s to go), then logd last so that it records
the rest, then every filesystem writes everything out and is marked clean
(step 17), then the power. The next boot's `logs -b -1` ends with "the
services have stopped; writing out the disks".

## The system log

**logd** serves `/svc/log` and writes `/var/log/system.log`: one message a
line, tab-separated fields `TIME BOOT PRIORITY TAG PID UID MESSAGE` (TIME
in UTC with milliseconds, BOOT the boot's number from
`/var/log/boot.count`, PRIORITY syslog's 0-7). The file is root's, group
`adm` (4), mode 0640. At 1 MiB it is rotated: `system.log.1` (newest) to
`.4`, so the log takes at most 5 MiB.

Messages come from

- **services**: their output (above);
- **programs**: `syslog()` (libc) and `logger` send `RX_LOG_OP_WRITE`
  messages; logd records the sender's pid and uid as the kernel gives them,
  and the tag they chose (only root may use `kernel` and `init`);
- **init**: starts, exits, restarts, failures, shutdown (tag `init`;
  messages from before logd was up are kept and sent then);
- **the kernel**: its log (what `dmesg` shows), read every half second;
  the boot's messages are in the first read, with logd's start time.

authd also logs su, sudo and passwd there (as well as `/var/log/auth.log`).

**logs** reads the files, oldest first: `-u UNIT` (or `-t`), `-k` (the
kernel, this boot), `-b [N]` (this boot, `-1` the one before), `-p PRIO`,
`-g TEXT`, `--since`/`--until` (dates, `today`, `-10min`), `-n N`, `-r`,
`-f` (follow), `-o short|short-iso|json|cat`, `--list-boots`,
`--disk-usage`. Root and group adm may read the log; others are told so.

On the live image the log is on tmpfs, so it lasts one boot; on a disk
root it lasts across boots.

## The protocol

`svc` sends `rx_svc_req_t {op = RX_SYSTEM_OP_SVC, cmd, name}` to
`/svc/system`. The reply is an i32 status and text: for `RX_SVC_LIST` a
`target` line and one line per unit (`RX_SVC_LIST_FIELDS` tab-separated
fields: name, state, enabled, pid, since, starts, last exit, targets, user,
type, restart, exec, description, reason), for `RX_SVC_STATUS` the unit's
line, for the others a message. LIST and STATUS are anyone's; START, STOP,
RESTART, ENABLE, DISABLE, RELOAD, RESET_FAILED and ISOLATE are root's
(`-EPERM`).

## Not done (yet)

- No scheduled jobs (`cron`, timers); no socket activation; no readiness
  notification other than `ready=` (a service path).
- Units have no resource limits, no sandboxing beyond their user; a unit's
  user is not checked for being a system account.
- logd keeps no index: `logs` reads every file (at most 5 MiB); the
  kernel's messages have logd's time, not the kernel's.
- If logd restarts, the services started before keep writing to their old
  sockets (lost) until they restart.
- `svc isolate` from the rescue shell ends that shell before `svc` can
  print.
- A filesystem mounted by hand from a session runs its server in the
  session's job: logging out ends it (the mounts of `/etc/fstab` are
  safe: `keep_processes`).

# FoxDen: apps (the plan: FOXDEN.md in the Rustux project)

Apps are kept apart from the system: self-contained `.rpg` packages of
their own kind, each in a folder of its own, from a repository of their
own (step FD3), run in a sandbox (step FD2). The same `rpg` installs
them. This is the technical side, step by step.

## App packages (step FD1)

An app is a `.rpg` package (`PACKAGES.md`) with `Kind: app` in its control
file. Its name is the app's ID, reverse DNS: lower-case parts of letters,
digits and `-`, two or more, joined by dots (`org.rustica.calculator`).

Everything it has is in **`/apps/ID/`**, and nothing anywhere else. Its
description is **`/apps/ID/manifest`**, in the control file's format:

```
Id: org.rustica.example          the package's name
Name: Example                    what Applications shows
Summary: an example app          one line
Exec: bin/example ARG...         its program, relative to /apps/ID
Icon: icon.png                   optional, relative to /apps/ID
Categories: Utility Graphics     optional
Permissions: network files:documents   optional; step FD2 applies them
Platform: 1                      the platform level it is made for
```

The permissions an app may ask for are FOXDEN.md's: `network`,
`files:documents`, `files:pictures`, `files:music`, `files:videos`,
`files:downloads`, `files:home`, `removable`, `audio`, `camera`,
`location`, `system`. The **platform level** is the set of interfaces an
app may rely on (the C library's, the toolkit's, the services); this
system has level 1, and an app made for a higher one is refused ("a newer
Rustica runs it").

**Building one:** `rpg build DIR` with `DIR/RPG/control` saying `Kind:
app`: the rest of DIR is the app's folder (the manifest at its top), so
the author never writes `/apps/ID`; rpg puts it there and checks the
result as an install would.

**What rpg refuses** in an app (at build and install):

- a file outside `/apps/ID` (and a system package with anything under
  `/apps`: that folder is the apps');
- scripts (`postinst`, `prerm`, `postrm`): they would run as root;
- set-user or set-group bits; a link with an absolute target, or one that
  leaves the app's folder;
- `Depends`, `Conflicts`, `Provides`, `Replaces` and the like: an app is
  self-contained, two apps never conflict;
- a manifest that is missing, names another ID, has an `Exec` that is not
  an executable file in the folder (or an `Icon` that is not a file
  there), an unknown permission, or a platform level this system lacks;
- an app from a system repository: apps come from app sources, with the
  apps key (step FD3); until then, from package files only.

**Installing and removing:** `rpg install ./org.rustica.example_1.0_x86_64.rpg`
(root, for now: step FD4's installer service lets users install
sandboxed apps), `rpg list --apps` (ID, version, name and summary from the
manifest; `--json`), `rpg remove ID`. The users' data for an app is in
`~/.local/share/apps/ID`, `~/.config/apps/ID` and `~/.cache/apps/ID`;
`rpg remove` leaves it, `rpg remove --purge` removes it from every home
in `/etc/passwd`.

**Applications** (the launcher, `SESSION.md`) lists installed apps from
their manifests beside the system's `/usr/share/applications/*.app`, and
starts it with `appctl run ID`, in its sandbox (step FD2, below).

`cargo test` in `userspace/rust/rpg` checks each refusal (`app.rs`);
`scripts/run-apppkg-test.sh` (`TESTS.md`) installs an app built on the
host, starts it from Applications, and removes it without a trace.

## The sandbox (step FD2)

An app runs as the user who starts it, but sees and does much less than
the user's own programs. `appctl run ID` (`man appctl`; what Applications
uses) starts the manifest's `Exec` with two limits:

**A root of its own.** Its namespace root is a view (`FS.md` "Views"): an
`fsd` root that sees only these paths, anything else is not there.

| What | Paths |
|------|-------|
| read-only | `/apps/ID`, `/usr`, `/etc/passwd`, `group`, `timezone`, `localtime`, `os-release`, `hostname` |
| read-write | `~/.local/share/apps/ID`, `~/.config/apps/ID`, `~/.cache/apps/ID` (made if new; it starts in the first, `TMPDIR` is in the last) |
| open only | `/dev/null`, `/dev/zero`, `/svc/window` (its windows), `/svc/log` |
| `network` | `/svc/netsock` (sockets; not `/svc/net`, the network's settings), `/etc/resolv.conf`, `/etc/hosts` |
| `files:documents` ... | `~/Documents` read-write (`files:pictures` `~/Pictures`, `files:music`, `files:videos`, `files:downloads` likewise) |
| `files:home` | the whole home read-write |
| `removable` | `/media` |

So an app finds no `/bin` (no shell to start), no other app, nothing of
the home but its own folders, no service but the window, log and (with
`network`) socket services: not `/svc/auth`, `/svc/packages`,
`/svc/display` or `/svc/devmgr`. Links are followed inside the view, so a
link it makes in its data folder leads nowhere else. `audio`, `camera` and
`location` give nothing yet (no services for them). `system` is never
given until apps from the system's own sources can be told apart (step
FD3).

**A job with the sandbox policy** (`SYSCALL.md` `JOB_SET_POLICY`): the
calls that reach the system without a handle (changing its own user, the
kernel console, the clock, physical memory, drivers, power, firmware
variables) are refused, for it and anything it starts. The audit of the
other calls: each acts on a handle the app holds, or on itself.

**The user decides** in `~/.config/apps/permissions`: a line `ID:
PERMISSION...` (or `ID: -`) narrows what an app gets; it never gets more
than its manifest asks for. `appctl permissions ID` shows both, `appctl
view ID` the paths. (Asking when an app first wants a permission is step
FD5's, with the FoxDen app.)

What the sandbox does not do yet: the user's own file permissions still
apply inside it (an app acts as its user), so it is a narrowing, not a
separate account; windows are not marked as an app's; and the window and
log services serve apps as they serve everyone.

`scripts/run-sandbox-test.sh` (`TESTS.md`) runs two apps that try to get
out: everything they should not reach is refused, what their permissions
give works.

## Not yet

The apps repository and its key (FD3), installing as a user through
`/svc/packages` (FD4), the FoxDen app (FD5), app updates and the file
chooser (FD6), outside developers (FD7).

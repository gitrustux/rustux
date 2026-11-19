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
- an app from a system repository: apps come from app sources, with an
  apps key (step FD3), or from package files.

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

## The apps repository (step FD3)

Apps come from **app sources**, lines of `/etc/rpg/sources.list` that
start with `apps`; the image has one:

```
apps https://rustux.com/download/apps  platform 1
```

An app source is laid out by **platform level**, not release:
`platform/1/Release` (signed), `platform/1/x86_64/Apps.gz` and
`Apps.json.gz` (every app, with its name, summary, categories,
permissions and icon from its manifest), `pool/L/ID/`, `media/ID/`
(`RELEASES.md` "The apps repository"). `rpg update` reads it as it reads
a system repository, with two rules of its own:

- **its own keys.** An app source's `Release` must be signed by a key in
  `/etc/rpg/keys/apps/` (`rpg key add --apps FILE`; the image has
  `rustux-apps-2026`), and a system source's by one in `/etc/rpg/keys/`:
  a key is trusted for its kind only, so the apps key can never sign a
  system package and the archive key never an app;
- **apps only.** An index that lists a system package refuses the whole
  source; a package downloaded from an app source must be an app, one
  from a system source must not be.

Then apps work as packages do, by their ID: `rpg search calculator`
(names and summaries; `(app)`), `rpg show org.rustica.calculator`,
`rpg install org.rustica.calculator` (it shows what the app asks for
before installing: "asks for: network, files:documents", or "nothing"),
`rpg upgrade` (apps with the system), `rpg remove`. Installing is still
root's (step FD4 lets users install sandboxed apps).

**The first apps**, in `userspace/apps/` (control file, manifest, icon)
with their programs in `userspace/rust/foxden-apps`:

| App | What it is | Permissions |
|-----|-----------|-------------|
| `org.rustica.calculator` | Calculator: + - × ÷, brackets, the usual precedence, from buttons or the keyboard | none |
| `org.rustica.sysmon` | System Monitor: the processors and memory over the last minute, uptime, threads, the kernel's version | none (no process list: that needs the system's job, the `system` permission's) |

The Text Editor and the Image Viewer stay in the system for now: in a
sandbox they could open only the folders a permission gives, so they move
out with the file chooser (step FD6).

**Publishing:** `scripts/build-apps.sh` makes the packages;
`scripts/publish-apps.sh --key APPSKEY DIR` indexes (`rpg index --apps`),
signs, checks and publishes them, keeping each app's two newest versions
(`RELEASES.md`). The handoffs run it after the system's publish.

`cargo test` in `userspace/rust/rpg` checks the rules (`repo.rs`: an
apps repository read, refused when signed with the archive key, changed
after signing, listing a system package, or for another platform);
`scripts/run-appsrepo-test.sh` (`TESTS.md`) installs, runs, upgrades and
removes the apps from a test apps source in QEMU, and
`scripts/run-publish-test.sh` publishes them.

## Not yet

Installing as a user through `/svc/packages`, and the `system`
permission for the system's own apps with the administrator's say (FD4),
the FoxDen app (FD5), app updates in the background, the file chooser and
moving the Text Editor and Image Viewer out (FD6), outside developers
(FD7).

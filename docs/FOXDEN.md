# FoxDen: apps (the plan: FOXDEN.md in the Rustux project)

Apps are kept apart from the system: self-contained `.rpg` packages of
their own kind, each in a folder of its own, from a repository of their
own (step FD3) and run in a sandbox (step FD2). The same `rpg` installs
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
starts `/apps/ID/EXEC`. Until step FD2 an app runs as any program the
user starts, without a sandbox.

`cargo test` in `userspace/rust/rpg` checks each refusal (`app.rs`);
`scripts/run-apppkg-test.sh` (`TESTS.md`) installs an app built on the
host, starts it from Applications, and removes it without a trace.

## Not yet

The sandbox (FD2: a root of its own, a job policy, the permissions), the
apps repository and its key (FD3), installing as a user through
`/svc/packages` (FD4), the FoxDen app (FD5), app updates and the file
chooser (FD6), outside developers (FD7).

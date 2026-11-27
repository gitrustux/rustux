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
Screenshots: shots/1.png ...     optional, relative to /apps/ID; FoxDen shows them
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
(an administrator: a package file is not from a signed source; step
FD4), `rpg list --apps` (ID, version, name and summary from the
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
| `network` | `/svc/netsock` (sockets; not `/svc/net`, the network's settings), `/etc/resolv.conf`, `/etc/hosts`, `/etc/ssl/certs` (the certificates the system trusts, for HTTPS) |
| `files:documents` ... | `~/Documents` read-write (`files:pictures` `~/Pictures`, `files:music`, `files:videos`, `files:downloads` likewise; made if the home has none yet) |
| `audio` | `/svc/sound` (playing sound, `SOUND.md`) |
| `files:home` | the whole home read-write |
| `removable` | `/media` |

So an app finds no `/bin` (no shell to start), no other app, nothing of
the home but its own folders, no service but the window, log and (with
`network`) socket services: not `/svc/auth`, `/svc/packages`,
`/svc/display` or `/svc/devmgr`. Links are followed inside the view, so a
link it makes in its data folder leads nowhere else. `camera` and
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
`rpg upgrade` (apps with the system), `rpg remove`. Any user may run
them (step FD4, below).

**The first apps**, in `userspace/apps/` (control file, manifest, icon)
with their programs in `userspace/rust/foxden-apps`:

| App | What it is | Permissions |
|-----|-----------|-------------|
| `org.rustica.calculator` | Calculator: + - × ÷, brackets, the usual precedence, from buttons or the keyboard | none |
| `org.rustica.sysmon` | System Monitor: the processors and memory over the last minute, uptime, threads, the kernel's version | none (no process list: that needs the system's job, the `system` permission's) |
| `org.rustica.texteditor` | Text Editor (step FD6b): one text file; Open and Save as through the file chooser | none |
| `org.rustica.imageviewer` | Image Viewer (step FD6b): a PNG or JPEG picture made smaller to fit | none |

The Text Editor and the Image Viewer were system programs until step
FD6b ("Opening files" below); they come with the system.

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

## Installing as a user (step FD4)

An app cannot leave its sandbox, so installing one is not a change to
the system: any user may install, upgrade and remove the apps of the app
sources, without sudo. rpg run by someone other than root sends `install`,
`remove`, `update` and `upgrade` to the installer service, `packagesd`
(`/svc/packages`, root; `PACKAGES.md` "Updates on the desktop"), which
runs rpg as root and sends its output back. Before it changes anything,
it asks rpg what would change (`rpg plan install|upgrade|remove ...`, a
line a package: `step OP NAME FROM TO KIND ORIGIN PERMISSIONS`) and
decides, under the same lock as the change itself:

| What would change | Who |
|-------------------|-----|
| apps from an app source, asking for neither `files:home` nor `system`: install, upgrade, remove | anyone |
| a system package (installed, upgraded or removed) | an administrator |
| an app asking for `files:home` (the whole home) or `system` (no sandbox) | an administrator |
| a package file (`rpg install ./FILE.rpg`: not from a signed source) | an administrator |
| `rpg remove --purge` (every user's data for the app) | an administrator |

"An administrator" is root, or someone in group `sudo` with their own
password; anyone else may name an administrator, who types theirs (as
polkit asks):

```
carol$ rpg install hello
hello is a system package: an administrator's password is needed
An administrator's name: erin
[rpg] password for erin:
```

Three wrong passwords end it; each is logged, as is who asked for what
and who allowed it (`logs -t packagesd`). `rpg update` as a user is the
service's check, its output shown. Package names are checked (letters,
digits, `+ - .`) and package files must be absolute paths ending in
`.rpg`, read only once an administrator said yes.

The `system` permission is still never given to an app: an app that asks
for it is installed only with an administrator's say, but runs sandboxed
(what it would get, the system's job and no view, comes with a reason to
give it; the System Monitor's process list waits for it).

`scripts/run-installsvc-test.sh` (`TESTS.md`): carol, not an
administrator, installs, upgrades and removes an app without a password
and is refused a system package, a `files:home` app, a package file and
`--purge`, until erin, an administrator, gives her password.

## The FoxDen app (step FD5)

**FoxDen** (`foxden`, Rust and rustica-ui; in Applications and on the
panel, beside Applications) is the store's window. `foxden ID` opens an
app's page, `foxden --updates` the Updates page.

- **Home**: every app the app sources have, as cards (icon, name,
  summary; "Installed" or "Update"). **Categories**: the apps by their
  manifests' categories. **Search**: by name, ID, summary, description
  and category.
- **An app's page**: its icon, Install (or Open, Update, Remove; Remove
  asks first: the folder goes, the user's data stays), its screenshots,
  its description, what it may do in plain words ("Use the network and
  the internet", "Read and change your Documents folder"; "Nothing beyond
  its own folder and a window" when it asks for nothing), and its version,
  developer, licence, download and installed sizes, categories, ID and
  source.
- **Installed**: each installed app with Open, Details, and a checkbox
  per permission it asks for: turning one off writes
  `~/.config/apps/permissions`, which `appctl` reads when the app next
  starts (step FD2).
- **Updates**: apps with a newer version, and the system's updates from
  the installer service's last check; "Check for updates" (`rpg update`)
  and "Install all updates" (`rpg upgrade`).

FoxDen reads what it shows from `rpg list --apps --available` (new: a
stanza per app, the source's newest with `Source:` its address, and
`Installed:` and `Upgradable:` when they apply) and does what it does with
rpg: `rpg install -y ID`, `rpg remove ID`, `rpg upgrade -y`, `rpg update`,
its output shown as it runs. For a user other than root rpg goes through
the installer service (step FD4); when that needs an administrator,
FoxDen asks in a sheet of its own (the user's password if they are in
group sudo, else an administrator's name and password) and hands it to
rpg. Icons and screenshots come from the source's `media/ID/` (`rpg index
--apps` copies the manifest's `Icon` and `Screenshots` there) and are kept
in `~/.cache/foxden/ID/`, fetched once (curl, or a copy for `file://`).
FoxDen is a system program, not an app: it is not sandboxed, but it does
nothing a user could not do with rpg.

`scripts/run-foxden-test.sh` (`TESTS.md`): FoxDen lists a test source's
apps, installs the Calculator from its page (it appears in Applications
and starts sandboxed), removes it; carol gets a `files:home` app with
erin's name and password in FoxDen's sheet, and turns a permission off on
Installed.

## Updates and the file chooser (step FD6)

**App updates in the background.** After its daily check (and `updates
check`), the installer service upgrades each app whose new version asks
for no more than the installed one (`rpg plan upgrade` against the
installed manifest's `Permissions`): `rpg install -y ID`, no question,
as an app cannot leave its sandbox. A new version that asks for more
**waits for the user**: the status file has `appwaits ID FROM TO
PERMISSIONS`, FoxDen's Updates page shows it ("Waiting for you: these new
versions ask for more", in plain words) with **Allow and update**, and the
user's yes is that install. The apps updated are kept in the status
(`appupdated SECONDS ID FROM TO`, the newest 20; FoxDen's "Updated in the
background"), and the panel notifies both ("Calculator was updated to
1.1.0"; "System Monitor 1.1.0 asks for more (network): FoxDen > Updates").
`/etc/rpg/updates.conf`: `apps auto` (the default) or `apps ask` (no app
upgraded without the user; they are ordinary updates then). The system's
own updates never install by themselves (`PACKAGES.md`).

**Settings > Apps**: each installed app with its version, what it takes
up (its folder, the user's data for it), a checkbox per permission it asks
for (the same file as FoxDen's Installed, `appctl` and `rpg permissions
ID [grant|revoke PERMISSION...]`), Open, In FoxDen,
and Remove (no administrator: removing an app takes nothing from the
user). Settings > Updates and FoxDen's Updates show the same list, the
installer service's last check.

**The file chooser.** An app without a `files:` permission still opens
and saves the files the user picks (`<rustux/portal.h>`):

```c
int fd = rx_portal_open("Open a note", "*.txt *.md", name, sizeof name);
int fd = rx_portal_save("Save the picture", "picture.png", name, sizeof name);
```

`appctl`, which started the app and waits for it as the user outside the
sandbox, is its chooser: the app gets a channel at start (`RX_PA_PORTAL`),
and a request on it makes appctl show **`chooser`** (a window of the
session's, as the user: "Portal Test wants to open a file"; the places,
the folder, a name, Open or Save, Cancel), open the file the user picks
(read only, or written from empty) and hand the app that file's
connection alone (`rx_fd_clone_handle`, `rx_fd_from_handle` in the C
library). The app gets a descriptor, not a path: no folder beside it, no
other file, nothing it may keep but the descriptor; Cancel gives
`-ECANCELED`. Each app has its own channel, so the chooser always names
the app that asks. (The plan named a session service, `/svc/portal`; a
user's session may not publish services, and appctl already knows the app
and is the user, so the chooser lives there.)

`scripts/run-appupdates-test.sh` (`TESTS.md`): a Calculator update
installs by itself, a System Monitor update asking for `network` waits
until the user says yes, the panel notifies both; an app without `files:`
permissions opens the file the user picks in the chooser and nothing else,
saves a copy through it, and gets nothing when the user cancels.

## Opening files; the apps that come with the system (step FD6b)

**Opening a file with an app.** An app's manifest may say which files it
opens, by their name's extension:

```
Types: txt md conf
```

Files opens such a file (Enter, a double-click) with the app as it opens
others with an application's `types=` (`APPS.md`): **`appctl open ID
FILE...`**, which starts the app as `appctl run` does, with each file in
its view at its own path, read-write if the user may write it, else read
only, and the paths as its arguments. The app opens the path it is given
as it would anywhere; its folder lists that file alone, nothing beside
it. A folder, a missing file, a file the user may not read and a path with
`.` or `..` are refused; `appctl view ID FILE...` shows the view it would
get.

**The Text Editor and the Image Viewer are apps**: `org.rustica.texteditor`
and `org.rustica.imageviewer` (`userspace/apps/`, programs in
`userspace/rust/foxden-apps`), with no permissions. So is **the web
browser** (2026-11-26): NetSurf, `org.rustica.browser` (its program from
`userspace/ports/netsurf`, its resources in the app), with `network` and
`files:downloads` (`BROWSER.md`); "Web Browser" in the menu and Files'
web pages go through `/bin/browser` to the default browser (Settings >
Default apps). The editor saves a
file opened from Files in place; a new file's Save, Save as and Open
(Ctrl+O) ask with the file chooser and write through the descriptor it
gives (rewritten from the start, cut to the new length, on each save). Run
outside a sandbox (no chooser to ask: `-ENOSYS`), Save as asks for a path
instead. The viewer shows the picture it was given; Left and Right step
through the pictures it can see in its folder (opened from Files: only
that one).

**The apps that come with the system.** `userspace/apps/preinstalled`
lists them; the build puts each one's folder, with its program, in the
boot image as `/usr/share/rpg/apps/ID/`. The installer service, when it
starts, makes a package of each one not yet installed (`rpg build` of a
copy, its program made executable) and installs it (`rpg install -y`),
then notes the ID in `/var/lib/rpg/preinstalled`: once only, so an app an
administrator removed stays removed (and an app installed another way is
noted and left). From then on it is an app like any other: it updates
from the app sources (the apps repository publishes both), Settings >
Apps and FoxDen remove it. While it installs them, another rpg waits for
its lock (a minute at most) rather than failing.

`scripts/run-openwith-test.sh` (`TESTS.md`): both installed at start and
noted; `appctl view` with files; the editor given `~/Documents/a.txt`
saves it in place; the viewer given one of two pictures sees one; a new
text saved through the chooser and saved again shorter, Open through the
chooser; an app removed is not installed again. `run-apps-test.sh` opens
a text file from Files in the editor, now an app.

## Outside developers (step FD7a)

Apps from outside the project come through the **foxden-apps** repository
on GitHub (`https://github.com/gitrustux/foxden-apps`, its README the
developers' guide), Flathub's way: a pull request with the app's source;
checks and a build on GitHub; a maintainer's review and merge; a release
built from `main`; then rustux.com checks it again and signs it into the
apps repository with the store's key, which never comes near GitHub.

**Developers.** `developers/ID/` in foxden-apps: `developer` (`Id`,
`Name`, `GitHub`) and `key.pub`, the developer's own Ed25519 key (made
with openssl; the private half never leaves them). The ID comes from the
GitHub login, `io.github.LOGIN`; their apps' IDs are under it. Only that
GitHub account changes that folder and those apps (the checks see the
pull request's author); `MAINTAINERS` anything.

**An app's folder**, `apps/ID/`: `manifest`, `RPG/control`, pictures,
`src/` (Rust) and `SIGNATURE`. The manifest's new fields (rpg, `app.rs`):

```
Developer: io.github.alice          the app's ID must be under it
Developer-Name: Alice Example
Source: https://github.com/gitrustux/foxden-apps/tree/main/apps/ID
```

`rpg build` and `rpg install` refuse an app outside its `Developer`'s
namespace; `rpg index --apps` copies the three into the index
(`App-Developer`, `App-Developer-Name`, `App-Source`), which FoxDen's app
page shows (Developer, Source code). **An installed app's update must come
from the same developer**: rpg refuses one whose `Developer` differs ("an
app does not pass to another developer"), so an app's ID cannot be taken
over by a new key or a mistake in the store.

**The signature.** The developer signs the folder: `rpg dev digest DIR`
(`rpg-source 1`, then `file PATH SHA256` for each file but `SIGNATURE`,
sorted; `ci/digest.py` in foxden-apps makes the same without rpg), signed
with `openssl pkeyutl -sign -rawin` into `SIGNATURE` (`sign.sh`); `rpg dev
verify DIR KEY.pub` checks it. It shows that the developer meant exactly
this source, and travels with the app's folder to the server.

**Built from source.** `scripts/build-outside-app.sh APPDIR OUTDIR`: the
crate is made by the script (`Cargo.toml` with `rustica-ui` as the only
dependency, the toolkit's dependencies at the system apps' versions), the
developer's `src/` in it, built for Rustux as the system's programs are;
the program goes to `bin/app` and `rpg build` makes the package; `rpg dev
check-package FILE DIR` then checks that the package is the folder built:
the same control fields, the folder's files (but `src/`, `RPG/`,
`SIGNATURE`) byte for byte, and only the program added. The source may not
read files or the environment while it is built (`include!` and the like,
`env!`, `#[path]`), nor use unstable features or `extern crate`; with no
build scripts and no other crates nothing of the developer's runs while it
builds.

**foxden-apps' checks** (`ci/check.py`, on every pull request, with a
read-only token and no secrets): the folder's layout and sizes, the
manifest and control file (ID under the developer, `Exec: bin/app`,
`Source`, platform 1, known permissions but `system`, an SPDX licence from
a list), the source rules, the signature by the developer's registered
key, who may change what, versions going up, the developer unchanged;
then the changed apps are built (`ci/build.sh`, with rustux at the commit
`ci/rustux-commit` names) and the packages kept with the run for the
reviewer.

**Releases.** On `main`, `publish.yml` builds each app whose version has
no release yet and releases it as `ID-VERSION` with a build provenance
attestation (GitHub's `attest-build-provenance`); the job that builds (and
so runs the apps' compilation) has a read-only token, the job that
releases runs nothing of the apps'.

**Publishing.** `scripts/fetch-outside-apps.sh FOXDEN_APPS OUTDIR` on the
server, from a checkout of foxden-apps' `main`: for each app its current
version's release, checked: the developer registered and the ID under
theirs and not the project's (`org.rustica`, ...), the folder signed by
their key, the package the folder built, and, when `gh` is signed in, the
provenance (`gh attestation verify`). An app without a release yet is
skipped. `scripts/publish-apps.sh --add DIR` then publishes them with the
project's apps (never one in the project's namespaces). The handoff does
both.

What the server trusts: the build of reviewed source on GitHub's runners
(the attestation says which workflow and commit). A wrong binary would
still be sandboxed like any app.

`scripts/run-outside-test.sh` (`TESTS.md`): a developer's folder signed,
built from source and checked (a changed folder, a package with an extra
file and an app outside its developer's namespace refused), fetched and
published with the project's apps (the project's namespace refused); in
QEMU it installs, shows its developer and source, runs sandboxed, and a
newer version of the same ID from another developer is refused.

## Apps from outside FoxDen (step FD7c)

Apps can still come from elsewhere, Android's way: a package file a
developer hands out (`rpg build`, then `rpg install ./FILE.rpg` or FoxDen),
or an app source of their own (signed with their own key, which an
administrator adds). They run in the same sandbox; what changes is that
everyone can see they are not from FoxDen, and that they cannot pass for
an app that is.

**Which sources are FoxDen.** An app source whose index is signed by a
store key: `/etc/rpg/store-keys` (key IDs, one a line; the image's lists
`rustux-apps-2026`; without the file, that key). rpg works it out from the
Release and signature `rpg update` kept, so nothing is trusted from the
index itself.

**Where an app came from**, kept in rpg's database as each app's `Origin`:
`foxden`, `system` (the apps that come with the system: the installer
service installs them with `rpg install --preinstalled`, root only),
`file`, or `source URL`. `rpg list --apps --available` shows `From` (where
the version it offers is from) and `Installed-From`; `rpg plan`'s ORIGIN
is `foxden` for FoxDen, `apps` for another app source.

**FoxDen first.** An app FoxDen has comes from FoxDen only: another
source's app of the same ID, even a newer version, is not offered; and an
app installed from FoxDen (or the system's) takes its updates from FoxDen
only. An app installed from FoxDen is not replaced by a package file or
another source ("installed from FoxDen; a package file does not replace
it": remove it first). So an outside package cannot take over an
installed app by claiming its ID and developer.

**The warning.** Installing an app not from FoxDen prints "ID VERSION is
not from FoxDen (a package file | the app source URL): nobody has checked
it. It runs in its sandbox with ...". FoxDen marks such an app "Not from
FoxDen" on its card and page, shows its developer as "claims to be ...,
not verified", and asks before installing it ("Install anyway"). `foxden
FILE.rpg` shows a package file's app (`rpg list --apps --file FILE`, which
also takes its icon) with Install; Files opens `.rpg` files with FoxDen.
Settings > Apps says "Not from FoxDen" under such an app.

**Who may install package files.** An administrator, by default.
`/etc/rpg/install.conf`: `outside anyone` lets any user install an app
from a package file that asks for neither `files:home` nor `system` (those,
and system packages, always need an administrator). An administrator sets
it with `rpg config outside anyone|admin` or Settings > Apps ("Allow
everyone to install apps from outside FoxDen"); the installer service's
`config` request needs an administrator's password. The installer service
copies a user's package file into `/var/cache/rpg/incoming` first (only a
plain file that is theirs or readable by everyone; opened, then checked)
and plans and installs the copy, so it cannot change in between.

**Developer mode.** `appctl run --dir DIR [ARG...]` runs the app in a
folder of the user's (its manifest and its program: `rpg build`'s DIR, or
`build-outside-app.sh`'s folder) without installing it: the same sandbox
as when installed, for that user alone, with DIR read only at its own path
(fsd views list real paths) and the data folders of the manifest's ID. No
password: nothing is installed. `appctl view|open|permissions --dir DIR`
too. It says "developer mode: ID from DIR (not installed, not checked by
anyone ...)", and notes when an installed app has that ID (they share the
user's data folders for it).

`scripts/run-sideload-test.sh` (`TESTS.md`): From and Installed-From;
FoxDen's app not shadowed by a third party's newer one; a package file
with the warning; a package file not replacing the FoxDen app; a third
party's app with the warning; carol refused a package file, then allowed
with `outside anyone`, still refused one asking for `files:home`, unable
to change the setting; `rpg list --apps --file`; developer mode's view and
run; FoxDen showing a package file's app.

## Not yet

The `system` permission's sandbox-free run; FD7b (a service on rustux.com:
ratings and reviews, the web storefront, logins); taking an outside app
down from the index (today: a maintainer removes it from foxden-apps and
the pool by hand); developer IDs other than `io.github.LOGIN` (a domain
the developer proves); reproducible builds, so the server could rebuild
and compare.

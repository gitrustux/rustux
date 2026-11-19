# Packages

Rustica's packages and their one tool, `rpg` (`PACKAGES.md` in the
planning documents, phase E; `man rpg`). Step E1: the `.rpg` format,
`rpg build`, installing and removing package files, the package
database, and recovery from a power cut. Step E2: signed repositories,
`rpg update`, `rpg upgrade`, `rpg install NAME` with dependencies, and
`rpg index` for the publishing side. Step E3: the system as packages,
image-based: the kernel file with its read-only boot image is one
package per version, beside an edition package, and `rpg boot update`
writes rboot's entries. FoxDen's apps (`FOXDEN.md`) come after.

`rpg` is Rust (`userspace/rust/rpg`): flate2 for gzip, sha2, its own tar
reader and writer, ed25519-dalek for signatures, ureq on rustls (ring)
with Mozilla's root certificates for HTTPS, as `curl`, and a few C
library calls (`sys.rs`). It builds for Rustux and for Linux alike:
packages are built and indexed on the publishing side too.

## The `.rpg` file

A gzipped tar (POSIX ustar, GNU long names; the host's `tar tzf` reads
it), named `NAME_VERSION_ARCH.rpg`:

| Entry | |
|---|---|
| `control` | the package's fields (below) |
| `files` | a line per file, folder and symbolic link, tab-separated: type (`f`, `d`, `l`), mode, owner, group, the file's SHA-256 (a link's target, `-` for a folder), `conf` or `-`, the path |
| `scripts/postinst`, `scripts/prerm` | optional shell scripts |
| `data/...` | the files, rooted at `/` |

Reading a package checks it whole before anything happens: every file
in `data/` is in `files` with the same kind and SHA-256, and nothing is
in one but not the other; paths are absolute and clean (no `..`).

The control file, as Debian's (a field's value goes on in lines that
start with a space; ` .` is an empty line):

```
Package: hello                lower-case letters, digits, + - . (2 or more)
Version: 1.0-1                [EPOCH:]UPSTREAM[-REVISION], compared as Debian's
Architecture: all             x86_64, aarch64 or all
Kind: system                  (the default) or app: an app, FOXDEN.md (FoxDen step FD1)
Depends: rustica-base (>= 0.1), foo | bar
Conflicts: ...                Provides: ...      (Provides: names only)
Maintainer: ...               Homepage: ...      Section: ...
Installed-Size: 12            KiB, written by rpg build
Description: the summary line
 More text.
```

**Versions** compare as `dpkg --compare-versions`: the epoch as a
number, then the upstream version and the revision piece by piece,
digits as numbers and the rest as text, `~` before everything:
`1.0~rc1 < 1.0 < 1.0+b1 < 1.0.1`, `1.9 < 1.10`, `1:0.1 > 2.0`.

**`rpg build DIR`**: `DIR/RPG/control` (and `postinst`, `prerm`), every
other file under `DIR` the data (`DIR/usr/bin/x` is `/usr/bin/x`), owned
by root, with the files' own modes; files under `/etc` are configuration
files. The tar's times are 0, so the same tree gives the same package.

## Where packages can go

On Rustux `/bin`, `/usr` and `/boot` are the boot image (`FS.md`): they
come with the kernel and are read-only, on an installed system too. So a
package installs into what the root filesystem holds: `/etc`, `/var`,
`/opt`, `/home`, or a new folder at the top, and onto the EFI partition,
mounted at `/boot/efi` (the system's own packages, below). A package
with another file in the boot image is refused before anything changes
("...read-only: packages go in /opt, /etc and /var; nothing was
changed"). The system's programs change by changing the whole image:
step E3 decided this (image-based, as against splitting `/bin` and
`/usr` into packages on the disk).

## The database

`/var/lib/rpg/` (under `--root DIR` when given): `status` (a stanza per
package, its control fields and `Status: installed`, or `config-files`
for one removed with its configuration files kept), `info/NAME.files`
and `info/NAME.postinst`/`prerm`, and while something is changing
`journal`, `pending/` and `lock`. Files are rewritten under a new name
and renamed into place. Anyone may read them; changes are root's (with
`--root`, the folder's owner's).

## Installing

`rpg install ./FILE.rpg...`: the packages are read and checked; the
architecture must be this computer's or `all`; `Depends` must be met by
what is installed or in the same command (with `Provides`); `Conflicts`
either way refuse; a file belonging to another package refuses the
whole install ("nothing was changed"); so does any error while the new
files are written (the folders made for them go too); a file not belonging to any
package is replaced (the system is not packaged yet, until E3). Several
packages install one after the other, dependencies first.

Then, for each one:

1. **The plan** goes to `pending/` (the new control and files list, the
   scripts, and `actions`: a line per path: `rename`, `new`, `skip`,
   `delete`, `keep` or `rmdir`) and `journal` says `phase begin`.
2. **Staging**: the folders are made; each new file is written beside
   its place as `PATH.rpg-tmp`, with its mode and owner, and synced.
3. `journal` says `phase staged`.
4. **Each `.rpg-tmp` is renamed over its file**: one rename, so a
   program sees the old file or the new one, never half of one. Old files
   the new version does not have are deleted, emptied folders removed.
5. **The database**: `status` and `info/`; `journal` says `phase
   configure`; **postinst** runs (`install`, or `upgrade OLD-VERSION`;
   `RPG_ROOT` and `RPG_PACKAGE` set).
6. `journal` and `pending/` go.

**Configuration files** (everything in `/etc`) are the administrator's:
for each one, the file here is compared with the version the old
package installed. Missing or unchanged: replaced; already the new one:
left; **changed here: kept, and the package's new one written beside it
as `PATH.rpg-new`** (rpg says so). An old configuration file the new
version dropped is kept if it was changed.

**Interrupted** (a power cut, a crash, a kill): `rpg fix` (and every
`rpg install` or `rpg remove`, before its own work) reads `journal`.
At `begin`, the new files were not all written: the `.rpg-tmp` files
go and the old version stays as it was. At `staged`, everything new
is on the disk: the renames whose `.rpg-tmp` is still there are done,
then the rest of the steps. At `configure`, postinst runs again (so
scripts must be safe to run twice). Queries (`rpg list`, `show`,
`verify`...) say an operation was interrupted.

The **lock** holds the process ID and the uptime: a lock whose process
is gone, or from before a restart, is stale and taken over.

## Removing

`rpg remove NAME...`: refused while an installed package needs it
(unless it goes too, in the same command); prerm runs (`remove`); its
files and emptied folders go, except configuration files: they stay,
with the folders that hold them, and the package becomes
`config-files` until `rpg remove --purge NAME` (which also deletes the
`.rpg-new` files). The removal is journaled from `staged` on.

## Queries

- `rpg list [--installed] [--json]`: name, version, architecture, summary
- `rpg show NAME` (the status stanza), `rpg show ./FILE.rpg` and
  `rpg info ./FILE.rpg` (control, scripts, the files with their modes)
- `rpg files NAME`, `rpg owner PATH...` (relative paths from the current
  folder; folders can belong to several packages)
- `rpg verify [NAME...]`: each file's SHA-256 (or link target) against
  the database: `missing` or `changed` lines (configuration files
  marked), else `NAME: N files as installed`; exit status 1 if anything
  but a configuration file differs

## Repositories (step E2)

A repository is static files, Debian's layout (any web server, mirror or
disk serves it):

```
dists/SUITE/Release                   Origin, Suite, Date, Architectures, Sections,
                                      and SHA256: " HASH SIZE SECTION/ARCH/Packages.gz" lines
dists/SUITE/Release.sig               the 64-byte Ed25519 signature of Release
dists/SUITE/SECTION/ARCH/Packages.gz  a stanza per package: its control fields,
                                      Filename, Size, SHA256 (Architecture: all ones in each)
pool/SECTION/L/NAME/NAME_VERSION_ARCH.rpg   (L: the first letter; lib + one more for lib...)
```

Rustica's own is `https://rustux.com/download/rustica`, suite `redfox`,
section `main` (published from step E5), signed with the archive key
`rustux-archive-2026` (key ID `65527118d2c1e504`), whose public key the
image has in `/etc/rpg/keys/rustux-archive-2026.pub`.

**Sources**: `/etc/rpg/sources.list` and `/etc/rpg/sources.list.d/*.list`,
a line per repository, `URL SUITE SECTION...` (https://, http:// or
file://; `apps URL platform LEVEL` lines are app sources: FoxDen's, step
FD3, `FOXDEN.md` "The apps repository"). `rpg source
list`, `rpg source add URL SUITE SECTION...`, `rpg source remove N`.

**Keys**: `/etc/rpg/keys/*.pub` for system sources, `/etc/rpg/keys/apps/`
for app sources (a key is trusted only for its kind): a line
`rpg-ed25519 HEX NAME`. `rpg key add [--apps] FILE [NAME]` also takes OpenSSL's
PEM public key; `rpg key list` shows each key's ID (the first 8 bytes of
the SHA-256 of the public key); `rpg key remove ID|NAME`.

**Trust: one signature covers everything.** `rpg update` fetches each
source's `Release` and `Release.sig` and refuses the source unless a
trusted key signed exactly those bytes and `Suite:` is the one asked
for; then each section's `Packages.gz`, refused unless its size and
SHA-256 are the ones `Release` gives, and each of its entries is checked
(a clean `Filename`, a `Size`, a `SHA256`, the control fields). Only
when the whole source is checked do its lists go into
`/var/lib/rpg/lists/` (`PREFIX_Release`, `PREFIX_Release.sig`,
`PREFIX_SECTION_ARCH_Packages`, the prefix made from the address and
suite); a refused source keeps the lists it had ("its lists from before
are kept"), and `rpg update` exits 1. Lists of sources no longer
configured are removed. A downloaded package is refused unless its size
and SHA-256 are the index's ("refused, nothing was installed"), and the
package inside must be the name and version the index says. HTTPS (with
Mozilla's roots, and `/etc/rpg/ca-certificates.pem` for a private
repository's CA) keeps downloads private; the signature makes even a
mirror or a broken-into server unable to change a package.

**Choosing versions**: a package's candidate is the highest version any
source has (the first source's, when two have the same).

**`rpg install NAME...`** (with `./FILE.rpg` too, in one command): the
named packages' candidates; then for each dependency not met by what is
installed or going in, the first alternative a source can satisfy, at
the highest version that fits (by name, or by `Provides` for a
relation without a version); then the whole result is checked: every
package's `Depends` met, no `Conflicts`. The plan is shown ("To
install", with what each dependency is for; "To upgrade"; the download
and installed sizes) and asked about (`Continue? [Y/n]`; `-y` does not
ask; no answer, as from a script, is no). Downloads go to
`/var/cache/rpg/archives/` (a copy there whose hash matches is used
again), then everything installs as in E1, dependencies first. A
package installed already at its candidate is left (if it was installed
for another, it is now marked as the administrator's).

**`Auto: yes`** in the status marks a package installed for another;
`rpg autoremove` removes those nothing installed by the administrator
needs any more (following `Depends` from them). **`Hold: yes`**
(`rpg hold NAME`, `rpg unhold`) keeps a package at its version.

**`rpg upgrade`**: every installed package whose candidate is newer, with
new dependencies as for install. A held package is kept back, and so is
an upgrade that cannot be made to fit (each one tried alone, then the
ones that fit together): "Kept back: NAME: the reason"; the rest go
ahead.

**Queries**: `rpg search WORD...` (names and descriptions; `--names-only`;
marks what is installed), `rpg show NAME` (the installed entry, else the
candidate with its source), `rpg policy NAME` (installed, candidate,
held, every version and where from; `rpg policy` alone lists the
sources), `rpg list --upgradable`, `--held`, `rpg depends NAME`, `rpg
rdepends NAME`, `rpg download NAME` (into the current folder, checked),
`rpg clean` (empties the cache).

**Publishing** (`rpg index REPO [--suite redfox] [--section main] [--arch
x86_64] [--origin Rustux]`): every `.rpg` under `REPO/pool` read and
checked whole (twice the same name, version and architecture refused),
`Packages.gz` for each architecture and `Release`; then `Release` is
signed apart, with the private key and nothing but OpenSSL 3:

```
openssl pkeyutl -sign -inkey rustux-archive-2026.key -rawin \
    -in REPO/dists/redfox/Release -out REPO/dists/redfox/Release.sig
```

`rpg pool-path FILE.rpg [SECTION]` prints where in the pool a package
file belongs. Step E5 puts this in the handoffs: `scripts/publish-repo.sh`
builds, indexes, signs and checks the repository rustux.com serves
(`RELEASES.md`).

Not yet: a `Valid-Until` in `Release` (a mirror could serve an old,
correctly signed release, holding back updates), delta downloads,
signatures on the packages themselves.

## The system's packages (step E3)

The system is image-based: the kernel file `rustux.efi` carries the boot
image (every program, `/bin` and `/usr`), so a version of the system is
one file, and an upgrade puts a new one beside the old.
`scripts/make-system-packages.sh OUTDIR [VERSION]` makes them from a
built kernel and rboot (VERSION defaults to 0.1.N, N the commits so far):

| Package | Holds |
| --- | --- |
| `rustica-system-VERSION` | `/boot/efi/EFI/Rustica/rustux-VERSION.efi` (Section: images) |
| `rustica-system` | nothing: depends on `rustica-system-VERSION (= VERSION)` and `rboot (>= VERSION)` |
| `rboot` | `/boot/efi/EFI/Rustica/rboot.efi`, the boot manager |
| `rustica-server` | `/var/lib/rustica/edition`: `server`, the text login by default |
| `rustica-desktop` | the same file: `desktop`, the graphical login by default |

The two editions conflict: one is installed, and installing the other
switches (`rpg install rustica-desktop` replaces `rustica-server`: each
says `Replaces:` the other beside `Conflicts:`, as Debian's do, and rpg
removes the replaced package first; step E4 made this work). Each
image's name holds its version, so `rpg upgrade` installs the new image
package beside the old one, as Debian does kernels: `rustica-system`'s
new version depends on it. Image packages are never autoremoved (the
previous one is the way back); removing one is `rpg remove
rustica-system-0.1.1`. Step E4 keeps two and falls back to the previous
image when a new one fails to start (below).

The image and edition packages' scripts (`postinst`, and `postrm`, new in
E3: run after removing) run `rpg boot update`, which writes rboot's
entries in `/boot/efi/loader/entries` from what is on the EFI partition:

- `rustica.conf`: the newest image, "Rustica Server"; with the desktop
  edition "Rustica (text console)";
- `rustica-desktop.conf`: the desktop edition's, "Rustica", the newest
  image with `desktop` on its command line;
- `rustica-VERSION.conf`: each older image, "Rustica VERSION (previous)".

The command line comes from `/etc/rustica/boot.conf` (`options
root=UUID=... quiet`, which the installer writes), the edition from
`/var/lib/rustica/edition`; `loader.conf`'s `default` follows the edition
and its other lines stay. Entries rpg wrote start with `# Written by rpg
boot update`, and it removes its own that are gone; rboot adds each
entry's recovery entry itself (`BOOT.md`). `rpg boot list` shows the
images, the edition and the running command line. An installation from
before E3 (an unversioned `rustux.efi`, its own `rustica.conf`) keeps
working: `rpg boot update` takes the options from its entry and shows
its image as "(before packages)".

The live medium carries these packages in `\rustica\packages`
(`build-live-image.sh`, which needs cargo for the host's rpg;
`PACKAGES=0` leaves them out, `PKG_VERSION` sets the version). The
installer (`INSTALL.md`) asks for the edition and installs them with
`rpg --root` (with `RPG_ESP`, where it mounted the new system's EFI
partition: rpg and the packages' scripts then find `/boot/efi` there), so
an installed system's database knows its image from the start; a medium without them gets the kernel copied, as before.

## Boot counting and two images (step E4)

A new image is tried before it is trusted, as systemd-boot's automatic
boot assessment does it:

1. `rpg boot update` writes a new image's entries with a counter in the
   file name, `rustica+3.conf`: three tries (only when there is an older
   image to fall back to; the first image installed has none). Every
   entry's command line gets `panic=10`, so a kernel that panics restarts
   after 10 seconds instead of waiting on its message.
2. rboot lowers the counter before it starts the entry (`rustica+2-1.conf`,
   `rboot: rustica: try 1 of 3`) and names the file in the
   `LoaderBootCountPath` variable. It also adds `BOOT_IMAGE=PATH` to a
   Rustica entry's command line, so the system knows which image runs.
3. When a multi-user or graphical boot has started everything, init's
   built-in `boot-complete` unit runs `bootctl bless`: the entry becomes
   `rustica.conf`, the image is good. (Built into init, which comes with
   the image, it reaches installations whose `/etc/svc` is older.)
4. An entry at `+0` failed: rboot puts every entry of that image at the
   end of its menu, "(failed to start)", and starts the first good one
   by default instead, the previous image (`rboot: rustica failed to
   start: rustica-0.1.2 instead`). A one-shot choice (`bootctl
   set-oneshot`) or the menu can still start it.

Each image's state (good, on trial, failed) is in its entries' file
names, and `rpg boot update` carries it over when it writes the entries
again (another edition, an image that becomes the previous one). `rpg
boot status` says which image runs and how each one did:

```
Running: 0.1.2 (/EFI/Rustica/rustux-0.1.2.efi)
newest    0.1.3                    FAILED to start (3 tries): not started by default
previous  0.1.2                    good: it started
```

**Two images are kept:** after an install or upgrade, image packages
beyond the newest and the running one are removed ("rpg: two system
images are kept ...: removing rustica-system-0.1.1"); the two newest when
it is not known which runs (`--root`, or a boot without `BOOT_IMAGE=`).
The running image is never removed.

Not done: a kernel that hangs instead of panicking is not restarted (that
needs a watchdog: the firmware's stops at ExitBootServices); a boot that
reaches its target but is broken otherwise counts as good.

## Updates on the desktop (step E6)

**The installer service**, `packagesd` (`/svc/packages`, root; its unit
comes with the image in `/usr/lib/svc`, `SERVICES.md`), runs rpg for the
desktop and for administrators without sudo. Requests are one text
message each:

| Request | Who | What |
|---------|-----|------|
| `status` | anyone | the last check |
| `check` | anyone | `rpg update`, then `rpg upgrade --plan` |
| `update` | anyone | the same as `check`, `rpg update`'s lines streamed back |
| `upgrade`, `install NAME\|/FILE.rpg...`, `remove [--purge] NAME...` | by what they change (FoxDen step FD4, `FOXDEN.md`): apps from app sources anyone; anything else an administrator | `rpg upgrade -y`, `rpg install -y`, `rpg remove`, their lines streamed back ("out ..."), then "done CODE" |

It checks by itself once a day (`/etc/rpg/updates.conf`: `check daily` or
`never`; `delay SECONDS` before the first check, 120) and writes what it
found to `/var/lib/rpg/updates`, which anyone may read: `checked`,
`error`, and `rpg upgrade --plan`'s lines (`step NAME FROM TO SIZE WHY`,
`kept NAME REASON`). `rpg upgrade --plan` is new: what an upgrade would do,
a line a package, changing nothing (no root needed).

- **`updates`** (`man updates`): `updates` shows the last check, `updates
  check` checks now, `updates install` installs (the password asked, or
  `-S`: from stdin).
- **The panel** reads the status file every 30 seconds: "Updates (N)"
  when there are some, which opens Settings > Updates, and a notification
  when N grows.
- **Settings > Updates**: the last check and its packages (installed and
  new versions, sizes), Check now, Install updates (in the background, its
  output shown; another user than root gives their password in the bar,
  as for the other system settings), and after a new system image
  "Restart now" (it starts at the next restart; the previous one stays in
  the menu). The sources are listed.

Step FD4 (`FOXDEN.md`, "Installing as a user") extended the service to
installing and removing, and rpg itself asks it when not run as root;
FoxDen's Updates page (step FD5) will use it too.

## Tests

`cargo test` in `userspace/rust/rpg` (versions, control files, the tar,
keys and signatures), and `scripts/run-packages-test.sh` (`TESTS.md`): a package built on the
host with the host's rpg, installed in QEMU on an ext4 root; packages
built in Rustux; an upgrade with a changed configuration file;
conflicts, dependencies, removal, `--purge`, a user refused; then two
real power cuts (QEMU killed) in a 300-file upgrade, one during the
renames (`rpg fix` finishes it) and one while staging (`rpg fix` undoes
it). `RPG_TEST_PAUSE=stage:N` or `rename:N` makes rpg stop after that
file, synced, for the test to cut the power there. A fourth boot (step
E2) uses repositories the host built with its rpg (`rpg index`), signed
with a test key (OpenSSL) and served over HTTPS behind a test CA, one
address whose contents the guest switches: a dependency installed with
its package, a newer release (hold, kept back, upgrade), autoremove, and
the refusals: an index changed after signing, a Release signed by another
key (the old lists kept), a package changed in the pool, an unknown CA,
a user updating.

The installer test (`scripts/run-install-test.sh`, `TESTS.md`) covers
step E3: the server edition installed as packages 0.1.1, `rpg list`,
`verify` and `boot list` on the installed system, an upgrade to image
0.1.2 from a signed repository on a second disk (0.1.2 starts next, 0.1.1
in rboot's menu as the previous one), and the desktop edition installed
over it (rboot starts the graphical login). Step E4 in the same test:
image 0.1.2 counted then blessed; an upgrade to 0.1.3 removes 0.1.1 (two
images kept); 0.1.3, made to fail with the test word `crash=boot` on its
entry (the kernel panics just before init), panics three times and
rboot falls back to 0.1.2, as `rpg boot status` then says.

Step E6: `scripts/run-updates-test.sh` (`TESTS.md`): the live desktop with
a test repository on a second disk; the service's own check finds an
update and a new dependency, the panel shows them and notifies, a user who
is not an administrator and an administrator with a wrong password are
refused, Settings > Updates installs them.

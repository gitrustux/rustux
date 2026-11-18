# Packages

Rustica's packages and their one tool, `rpg` (`PACKAGES.md` in the
planning documents, phase E; `man rpg`). Step E1, here: the `.rpg`
format, `rpg build`, installing and removing package files, the
package database, and recovery from a power cut. Repositories, signed
indexes and `rpg update`/`rpg upgrade` come with step E2; the system
itself split into packages with E3; FoxDen's apps (`FOXDEN.md`) after
E2.

`rpg` is Rust (`userspace/rust/rpg`): flate2 for gzip, sha2, its own tar
reader and writer, a few C library calls (`sys.rs`). It builds for
Rustux and for Linux alike: packages are built, and from E2 indexed, on
the publishing side too.

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
Kind: system                  (the default) or app: FoxDen's, refused until then
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

## Where packages can go (until step E3)

On Rustux `/bin`, `/usr` and `/boot` are the boot image (`FS.md`): they
come with the kernel and are read-only, on an installed system too. So a
package installs into what the root filesystem holds: `/etc`, `/var`,
`/opt`, `/home`, or a new folder at the top. A package with a file in
the boot image is refused before anything changes ("...read-only, until
step E3: packages go in /opt, /etc and /var; nothing was changed").
Step E3, the system as packages, changes this: an installed system's
`/bin` and `/usr` come from its disk, and the boot image only serves the
live medium and recovery.

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

## Tests

`cargo test` in `userspace/rust/rpg` (versions, control files, the tar),
and `scripts/run-packages-test.sh` (`TESTS.md`): a package built on the
host with the host's rpg, installed in QEMU on an ext4 root; packages
built in Rustux; an upgrade with a changed configuration file;
conflicts, dependencies, removal, `--purge`, a user refused; then two
real power cuts (QEMU killed) in a 300-file upgrade, one during the
renames (`rpg fix` finishes it) and one while staging (`rpg fix` undoes
it). `RPG_TEST_PAUSE=stage:N` or `rename:N` makes rpg stop after that
file, synced, for the test to cut the power there.

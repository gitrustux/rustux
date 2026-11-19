# Releases and the repository (package step E5)

How Rustica's packages get from the build to the computers that run it.
The package format, rpg and the repository layout are in `PACKAGES.md`;
this is the publishing side.

## Names and versions

- **Rustica** is the system, on the **Rustux** kernel. A release has a
  codename: the first is **redfox**, which is also the repository's suite
  (`dists/redfox`).
- **A version per build:** `0.1.N`, N the number of commits in the rustux
  repository (`git rev-list --count HEAD`). Every system package of one
  build has it: `rustica-system-0.1.N`, `rustica-system`, `rboot`,
  `rustica-server`, `rustica-desktop` (`scripts/make-system-packages.sh`).
  The live image built from the same tree carries the same version in
  `\rustica\packages`, so an installation from it and the repository agree.
- **A published version never changes.** Publishing a version that is
  there already does nothing; a package file of that name with other
  contents is refused. Clients cache packages by name and checksum.
- `0.1` becomes `0.2` (and so on) when a release is made, by hand, in
  `make-system-packages.sh`'s default; the codename changes with the
  release (below).

## The repository

`https://rustux.com/download/rustica`, which the image's
`/etc/rpg/sources.list` names, is on the server at
`/var/www/rustux.com/html/download/rustica`: static files, served by the
web server as they are (nothing runs there):

```
download/rustica/
  dists/redfox/Release                  Origin, Suite, Date, the index's size and SHA-256
  dists/redfox/Release.sig              its Ed25519 signature (64 bytes)
  dists/redfox/main/x86_64/Packages.gz  every package: name, version, depends, size, SHA-256, path
  pool/main/r/rustica-system-0.1.N/rustica-system-0.1.N_0.1.N_x86_64.rpg
  pool/main/r/rboot/rboot_0.1.N_x86_64.rpg ...
```

The **newest three versions** are in the index and the pool (`--keep`): a
client that read the index a moment before a publish still finds its
packages, and rpg upgrade always has the newest.

## Publishing

Each handoff's `apply.sh` publishes after building and pushing (from step
E5 on); by hand:

```
cd /var/www/rustux.com/prod
scripts/publish-repo.sh --key /root/keys/rustux-archive-2026.key /var/www/rustux.com/html/download/rustica
```

`scripts/publish-repo.sh` (needs OpenSSL 3, cargo, the kernel and rboot
built):

1. checks the key: its public half must be the archive key the image
   trusts (`files/etc/rpg/keys/rustux-archive-2026.pub`), and it must not
   be inside the web root or `/var/www`;
2. makes this version's packages and puts them into `pool/` (new files
   only);
3. makes the index of the newest three versions in a staging folder
   (`.staging`, hard links to the pool), signs `Release` with the key
   (`openssl pkeyutl -sign -rawin`), and checks it as a client would:
   `rpg update` from that folder with only the public key; it must read
   every package;
4. only then puts the new `dists/` in place of the old one (two renames:
   a client in that instant fails once and succeeds on its next update),
   and removes the pool files of older versions.

Nothing is published if any step fails; the previous index stays. Then
`apply.sh` reads `Release` back over HTTPS and runs `rpg update` against
`https://rustux.com/download/rustica` with the image's public key, as a
Rustica would.

## The archive key

`rustux-archive-2026` (Ed25519; key ID `65527118d2c1e504`). The public key
is in the image (`/etc/rpg/keys/rustux-archive-2026.pub`). The private key
is the owner's: on the server outside the web root (by default
`/root/keys/rustux-archive-2026.key`, mode 600), never in git, never in
`/var/www`. Losing it means a new key and a new image for everyone;
keeping an offline copy is worth it.

Replacing it (later): the next key goes out in a `rustica-keyring` package
signed with the current one, well before the current one is retired.

## The apps repository (FoxDen step FD3)

`https://rustux.com/download/apps` (on the server
`/var/www/rustux.com/html/download/apps`), the source line
`apps https://rustux.com/download/apps platform 1` in the image: static
files too, laid out by **platform level** instead of release (an app
keeps working across releases with the same level, so nothing of it moves
to archive.rustux.com):

```
download/apps/
  platform/1/Release, Release.sig       signed with the apps key
  platform/1/x86_64/Apps.gz             every app: its control fields, Filename, Size,
                                        SHA256, App-Name, App-Summary, Categories,
                                        Permissions, Platform, Icon
  platform/1/x86_64/Apps.json.gz        the same as JSON (web pages, later)
  pool/o/org.rustica.calculator/org.rustica.calculator_1.0.0_x86_64.rpg
  media/org.rustica.calculator/icon.png
```

The apps are the folders of `userspace/apps/` (control file, manifest,
icon; programs from `userspace/rust/foxden-apps`, built into
`userspace/apps/bin/`): `scripts/build-apps.sh OUTDIR` makes their
packages. Each handoff's `apply.sh` publishes them after the system's;
by hand:

```
cd /var/www/rustux.com/prod
scripts/publish-apps.sh --key /root/keys/rustux-apps-2026.key /var/www/rustux.com/html/download/apps
```

It works as `publish-repo.sh` does (the key checked, a staging folder, the
index signed and read back by `rpg update` with only the public key, then
put in place), with two differences: an app is published when its
`Version` changes (a published version never changes: the same version
with other contents is refused), and each app's newest **two** versions
are kept.

**The apps key**, `rustux-apps-2026` (Ed25519; key ID
`2f06e52fed979364`), signs the apps repository only: the image trusts it
in `/etc/rpg/keys/apps/`, where rpg looks for app sources' keys and
nowhere else, so it can never sign a system package, and the archive key
can never sign an app. Its private key is kept as the archive key's is:
`/root/keys/rustux-apps-2026.key` (mode 600) by default, never in git or
`/var/www`.

## Retiring a release: archive.rustux.com

When a release is followed by the next (redfox, then another codename),
the old one moves, as Ubuntu's old releases move to old-releases.ubuntu.com:

1. its `dists/redfox` and its pool are copied to
   `https://archive.rustux.com/download/rustica` (the same layout; the
   same signatures stay valid, since the files do not change);
2. a last update of the old release points its sources there (a
   `rustica-release` package owning `/etc/rpg/sources.list.d/`: not made
   yet);
3. `rustux.com/download/rustica` then serves only the new release's suite.

## Mirrors

Later. The layout is plain files and the signature covers everything, so a
mirror needs nothing but a copy (rsync); clients trust the key, not the
server.

## Not done (yet)

- `Valid-Until` in `Release`: a correctly signed old index served again
  would hold updates back (rpg does not check a date yet).
- `by-hash` index files (Debian's way to make the switch to a new index
  atomic for clients in the middle of an update).
- The `rustica-release` and `rustica-keyring` packages.
- Reproducible builds: the same commit built twice gives other bytes, so
  the repository's packages and the live image's are the same version but
  not the same files.

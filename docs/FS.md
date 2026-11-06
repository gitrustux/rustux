# Rustux Filesystem: Service, Protocol and Boot Image

Files live in a userspace service, `fsd`, as in Fuchsia: the kernel keeps
no filesystem code (checklist step 8). Programs reach it through the C
library (`open`, `read`, `opendir`, ...; `docs/USERSPACE.md`).

## The tree

| Path | Filesystem | Notes |
|------|------------|-------|
| `/` | tmpfs | in memory, writable; lost at reboot. Or a disk: the root filesystem (step 15, below) |
| `/tmp` | tmpfs | |
| `/root` | tmpfs | root's home (`$HOME`); the shell starts here |
| `/etc` | tmpfs | settings (`hostname`, `timezone`, `adjtime`, `keymap`, `passwd`, `group`, `fstab`, `drivers`), copied from `/boot/etc` at start; `mtab` is a link to `/run/mtab` |
| `/boot` | bootfs | the boot image, read-only |
| `/bin` | bootfs | `/boot/bin` mounted again: the programs |
| `/usr` | bootfs | `/boot/usr` mounted again (`/usr/share/zoneinfo/zones`, `/usr/share/keymaps/`, `/usr/share/man/`, `/usr/share/tests/`) |
| `/dev` | devices | `null` (reads nothing, takes every write) and `zero` (reads zeros); no new files here |
| `/svc` | tmpfs | services: `devmgr`, `edu`, ... (step 12; see below) |
| `/mnt` | tmpfs | an empty directory to mount disks on (step 14) |
| `/run` | tmpfs | runtime state: `mtab` (what is mounted), `cmdline` (the kernel command line) |
| `/boot/efi` | FAT | the EFI system partition, mounted at boot from `/etc/fstab` when there is one (step 14) |

Writes, creates, renames and removals under `/boot`, `/bin` and `/usr` fail with
`EROFS`. Filesystems on disks are mounted into this tree (see Mounts):
FAT12/16/32 since step 14, ext2 since step 15, ext4 in step 16.

### The root filesystem on a disk (step 15)

With `root=` on the kernel command line, `init` has `mount --root` mount
that filesystem on `/` before the shell starts (waiting up to 30 s for the
disk). It then holds everything at the top except `fsd`'s own
directories, which stay as in the live system: `/boot`, `/bin` and `/usr`
(the boot image: the programs always come with the kernel), `/dev`,
`/svc`, `/tmp`, `/mnt` and `/run` (in memory, empty at each boot).
`/etc`, `/root`, `/home`, `/var` and anything else made at the top are on
the disk and survive reboots; names the disk has for fsd's directories
(a `/bin` of its own) are hidden. When it is mounted, `fsd` creates
`/etc`, `/root`, `/home` and `/var` on it if missing, copies the files of
`/boot/etc` that `/etc` lacks (changed settings are kept), and makes
`/etc/mtab` a link to `/run/mtab`. `poweroff` and `reboot` write every
filesystem out and mark it clean (`RX_FS_OP_SHUTDOWN_ALL`), the root last.

The kernel command line is the UEFI load options (a boot manager's entry;
`rboot` in step 19), or else the first line of `\EFI\BOOT\cmdline.txt`
next to the kernel on the EFI partition (a USB stick, QEMU). Its words
are `init`'s arguments and are in `/run/cmdline`:

| Word | Meaning |
|------|---------|
| `root=SPEC` | the root filesystem: `UUID=...`, `LABEL=...`, `PARTLABEL=...` or a device name (`vda2`); `root=live` or none: from memory, as before |
| `rootfstype=TYPE` | its type (default: what `blkid` finds) |
| `ro`, `rw` | mount it read-only (default `rw`) |

If the root filesystem cannot be mounted, `init` says so and the system
runs from memory.

### Symbolic links (step 15)

tmpfs and ext2 have symbolic links; FAT and bootfs have none. Whoever
serves a path follows the links on it: `fsd` for its own directories, the
filesystem's server for its part. A server that meets a link it has to
follow replies `RX_FS_REDIRECT` (status 1) with the path to use instead
(below); `fsd` normalises it and starts the request again from the root,
so a link can point anywhere in the tree, across filesystems. After 40
links: `ELOOP`. Links are followed everywhere in a path and at its end
for `OPEN` (unless `O_NOFOLLOW`) and `STAT_AT`; not at the end for
`LSTAT_AT`, `READLINK`, `UNLINK`, `RENAME`, `SYMLINK`, `LINK`, `MKDIR`. The
C library still resolves `..` by the text of the path, before any link
is looked at (as a shell's `cd ..` does), and `realpath` follows links.
Hard links exist on ext2 (`LINK`); tmpfs and FAT refuse them (`EPERM`).

Permissions are recorded (`ls -l` shows them) but not enforced until users
exist (step 17).

## Start-up

1. The kernel finds `bin/init` in the boot image built into it and starts
   it, passing the whole image as a read-only VMO (`PA_VMO_BOOTFS`).
2. `init` maps the image, finds `bin/fsd` and starts it with the library's
   loader from memory (`rx_spawn_image`), giving it the image VMO and the
   server end of a channel (`PA_USER0`). The client end becomes init's
   namespace root.
3. From then on programs load from `/bin` through `fsd`. Each program gets
   its own connection to the root (`PA_NS_DIR`), cloned from its parent's
   (`RX_SPAWN_CLONE_NS`).

If `fsd` ends, `init` says so; nothing restarts it (its files would be gone
anyway).

## Connections

Every open file or directory is a channel to `fsd` (a **connection**). A
request is one message; the reply comes back on the same channel. The
client waits for the reply before sending another request (each process
has its own connections, so replies don't mix).

Several connections can share one **open file description** (offset,
flags), like Unix `dup()` across processes: `CLONE` makes a new connection
to the same one. The C library uses it to hand a file fd to a child (the
shell's `> file`), so parent and child write at the same offset.

A file removed while open stays readable and writable through its open
connections until the last one closes.

## Protocol (`<rustux/fs.h>`)

Request: `{ u32 txid; u32 op; }` then the op's fields. Reply:
`{ u32 txid; i32 status; }` then the op's fields; `status` is 0 or a
negative errno. Paths are relative to the connection's directory,
`/`-separated, with no `.` or `..` (the C library resolves those against
the current directory; `fsd` rejects them with `EINVAL`).

| Op | On | Request | Reply |
|----|----|---------|-------|
| `OPEN` 1 | directory | `u32 flags` (`O_*`), `u32 mode`, path; one handle: the server end of the new connection | `u32 type` (1 file, 2 directory) |
| `CLONE` 2 | any | one handle: the server end of a new connection to the same open file | — |
| `STAT` 3 | any | — | `rx_fs_attr_t` |
| `READ` 4 | file | `u32 count` (≤ 32 KiB) | the bytes; advances the offset |
| `WRITE` 5 | file | the bytes (≤ 32 KiB) | `u64 written`; `O_APPEND` writes at the end |
| `SEEK` 6 | file | `i64 offset`, `u32 whence` | `u64 offset` |
| `TRUNCATE` 7 | file | `u64 length` | — |
| `READDIR` 8 | directory | `u32 max bytes` | entries `{ u64 ino; u8 type; u8 name_len; name }`; none at the end |
| `REWIND` 9 | directory | — | — (READDIR starts over) |
| `MKDIR` 10 | directory | `u32 mode`, path | — |
| `UNLINK` 11 | directory | `u32 flags` (1 = remove a directory), path | — |
| `RENAME` 12 | directory | `u32 old_len`, old path, new path | — |
| `SYNC` 13 | any | — | — |
| `STAT_AT` 14 | directory | path | `rx_fs_attr_t` |
| `SETATTR` 15 | any | `rx_fs_setattr_t { u32 mask; u32 mode; i64 mtime_ns; }`; mask 1 sets the permission bits, 2 the modification time | — (`EROFS` on bootfs) |
| `SERVE` 17 | directory | path; one handle: a channel to the server | — (makes or replaces a service node; `EEXIST` over something else) |
| `GET_VMO` 16 | file | — | `u64 offset; u64 size;` and one handle: the boot image VMO, read-only. Boot-image files only (`ENOTSUP` otherwise); the loader maps programs straight from it |
| `MOUNT` 18 | directory | `rx_fs_mount_t { u32 flags; u16 source_len; u16 type_len; }`, source, type, path; one handle: the root directory connection of the filesystem's server | — (`EBUSY` on `/` or a mount point, `ENOTDIR`) |
| `UNMOUNT` 19 | directory | `u32 flags` (1 = force), path of the mount point | — (`EINVAL` if nothing is mounted there, `EBUSY` if files are open) |
| `SHUTDOWN` 20 | root of a mounted filesystem | `u32 flags` (1 = force) | — : the server writes everything out and ends; `EBUSY` while other connections are open, unless forced. Sent by `fsd` on `UNMOUNT` |
| `STATFS` 21 | any | — | `rx_fs_statfs_t { u64 block_size, blocks, blocks_free, files, files_free; u32 name_max; u32 flags; char type[16]; }` (`statfs`, `df`) |
| `SYMLINK` 22 | directory | `u32 target_len`, target, path | — (`EEXIST`, `EPERM` on FAT) |
| `READLINK` 23 | directory | path | the target (`EINVAL` if not a link) |
| `LINK` 24 | directory | `u32 old_len`, old path, new path | — (a hard link: `EXDEV` across filesystems, `EPERM` for directories, on tmpfs and FAT) |
| `LSTAT_AT` 25 | directory | path | `rx_fs_attr_t` of the path itself (a link is not followed) |
| `SHUTDOWN_ALL` 26 | `fsd`'s root | — | — : every mounted filesystem is written out and marked clean, the root filesystem last (`poweroff`, `reboot`) |

A path request may be answered with status 1, `RX_FS_REDIRECT`: `u8
which` (0 the request's path, 1 the second path of `RENAME`/`LINK`), then
the path to use instead, from the namespace root if it starts with `/`,
else from the replying server's root. A redirected `OPEN` sends the
connection's server end back with the reply. Only `fsd` sees these; it
retries (see Symbolic links).

Types: 1 file, 2 directory, 3 device (`/dev/null`, `/dev/zero`; step 10),
4 service (step 12; `stat` shows `S_IFSOCK`), 5 symbolic link (step 15).

`rx_fs_attr_t` is `{ u32 type; u32 mode; u64 size; u64 ino; u64 nlink;
i64 mtime_ns; u32 read_only; u32 fsid; u32 uid, gid; i64 atime_ns,
ctime_ns; u64 blocks; }` (the last five since step 15; a filesystem that
fills `ctime_ns` also counts `blocks`, in 512-byte units; else the C
library uses the modification time and the size). `rx_fs_setattr_t` grew
the same way: `{ u32 mask; u32 mode; i64 mtime_ns; u32 uid, gid; i64
atime_ns; }`, mask 4 the owner, 8 the group, 16 the access time. Times
are UTC (nanoseconds since 1970). `fsid` tells the filesystems apart (1 tmpfs, 2 bootfs,
3 devices); `stat` reports it as `st_dev`, which `df`, `find -xdev` and
`cp` use.

Errors follow POSIX: `ENOENT`, `EEXIST`, `ENOTDIR`, `EISDIR`, `ENOTEMPTY`,
`EROFS`, `EBUSY` (removing or renaming a mount point), `EINVAL` (moving a
directory into itself), `EBADF` (reading a write-only connection),
`ENOSYS` (unknown op), `ENOTSUP` (`GET_VMO` on a file not in the boot image).

## Services

A program that serves a protocol of its own publishes it with `SERVE`
(`rx_service_publish("/svc/NAME", channel)` in the C library,
`<rustux/service.h>`). An `OPEN` of the service sends the opener's new
connection to the server as an `rx_svc_connect_t { u32 op; u32 flags; }`
message carrying the handle, and replies type 4; the opener then talks
the server's protocol (`rx_service_connect`). If the server has ended,
`OPEN` fails with `ECONNREFUSED`; publishing again replaces the old
channel (a restarted driver does this). `open()` refuses services with
`ENXIO`, since they do not speak the file protocol. `devmgr` publishes
`/svc/devmgr`, the `edu` driver `/svc/edu` (`docs/DRIVERS.md`).

## Mounts

A filesystem on a disk is served by a program of its own (`/bin/fatfs`
for FAT), speaking the same protocol. `mount` starts the server with the
server end of a channel (`PA_USER0`) and the block device's name, checks
that it answers a `STAT`, and hands the client end to `fsd` with `MOUNT`
(`rx_mount()` in `<sys/mount.h>`). From then on:

- **Path requests** (`OPEN`, `STAT_AT`, `MKDIR`, `UNLINK`, `RENAME`) whose
  path runs through the mount point are forwarded by `fsd` to the server's
  root connection with the rest of the path, and the reply is passed back.
  `fsd` waits for the reply (at most 30 s, then `ETIMEDOUT`), so a server
  must never use the filesystem itself while serving: `fatfs` reads the
  time zone before it starts.
- **Opened files** talk to the server directly: `OPEN`'s connection handle
  goes to the server, so reads and writes do not pass through `fsd`.
- `RENAME` across filesystems fails with `EXDEV` (`mv` then copies);
  hard links, services and `GET_VMO` are not on disk filesystems.
- `SYNC` on `fsd` is forwarded to every mounted filesystem (`sync`).
- `/etc/mtab` gets a line per mount: `source dir type rw|ro 0 0`.

`umount` sends `UNMOUNT`; `fsd` sends `SHUTDOWN` to the server, which
refuses while files are open on it (`EBUSY`) unless forced, else writes
out everything and ends. A server that has crashed can always be
unmounted.

At boot `init` runs `mount -a`, which mounts what `/etc/fstab` lists,
waiting up to 10 seconds for disks to appear. The default `/etc/fstab`
mounts the EFI system partition at `/boot/efi`: the one on a removable
disk (a USB stick Rustux started from) if one appears within those 10
seconds; else one on a fixed
disk (Windows' own) is mounted read-only, unless its options say `rw!`.

### FAT (`fatfs`)

FAT12, FAT16 and FAT32, read and write, as Windows writes them:

- Long file names (VFAT, UTF-16 on disk, UTF-8 to programs), with 8.3
  short names made as Windows does (`LONGFI~1.TXT`); names that fit 8.3
  get no long entries, lower case kept in the NT case bits. Lookups ignore
  case (ASCII, Latin-1, Latin Extended-A, Greek, Cyrillic), so
  `cat README.TXT` finds `readme.txt`; renaming only the case is a no-op,
  as `mv` sees one file.
- Names may not contain `\ / : * ? " < > |` or control characters
  (`EINVAL`).
- Times are kept in local time (FAT has no time zone; Windows does the
  same), at 2-second resolution. The permission bits map to the
  read-only attribute (`chmod a-w`); there are no owners or links.
- Files are at most 4 GiB - 1 (`EFBIG`); there are no holes: growing a
  file writes zeros.
- Directory and FAT changes reach the disk before the request is
  answered; file data within half a second, or when the file is closed.
  The FSInfo free count is kept up to date. Clusters of a file removed
  while open are freed when it is closed.
- `statfs` gives cluster size and counts; `df` shows them.

`mkfs.fat` makes FAT filesystems as Windows' format does, `fsck.fat`
checks them (docs/COMMANDS.md).

### ext2 (`ext2fs`)

ext2 as Linux's `mke2fs -t ext2` makes it, read and write: revision 0
and 1, blocks of 1, 2 or 4 KiB, 128- or 256-byte inodes (nanosecond times
in the larger ones), sparse superblocks, large files, file types in
directories, direct, indirect, double and triple indirect blocks.

- Files: holes (sparse files read zeros and take no blocks), truncation
  frees blocks and zeroes the rest of the last one, at most 4 TiB with
  4 KiB blocks. A full disk fails the write with `ENOSPC` and leaves no
  blocks past the file's end.
- Directories: entries are found by a scan; indexed ones (`dir_index`,
  as Linux makes large directories) are read the same way and lose the
  index when changed, as with Linux's own ext2 driver; e2fsck accepts
  both.
- Symbolic links (short ones in the inode), hard links (up to 65000),
  owners and groups (32-bit), all permission bits including set-user-ID,
  access times as Linux's `relatime`.
- Allocation: directories go to the group with the most free inodes,
  files near their directory, blocks after the previous block of the
  file; the bitmaps, the group counts and the superblock are kept exact.
- Metadata reaches the disk before each reply, file data within half a
  second or at close. While mounted for writing the superblock says "not
  clean"; unmounting (or `poweroff`) marks it clean, so a crash makes
  `e2fsck` check it, and mounting a filesystem that was not clean prints
  a warning.
- Filesystems with ext3/ext4 features this server does not keep (a
  journal, checksums, ...) are served read-only; those it cannot read
  (extents, 64-bit, inline data, ...) are refused until step 16.
- Not kept up to date: the backup superblocks and descriptors (Linux does
  not either, except when resizing); extended attributes are left as they
  are.

`mkfs.ext2` and `fsck.ext2` make and check ext2 filesystems
(docs/COMMANDS.md).

## Limits

- `fsd` serves up to 1000 connections (open files and directories of all
  programs together) and handles one request at a time.
- tmpfs files grow in memory without a quota.

## Boot image (bootfs) layout

Built by `build.rs` from every file under `files/` (same path: `files/etc/hostname`
is `etc/hostname`) and every file in `userspace/bin/` (as `bin/<name>`). Little-endian:

```
offset 0   u32 magic 0x52555458 ("RUTX")
       4   u32 count
       8   u32 table offset (12)
      12   count × { u32 name offset; u32 data offset; u32 size; u32 0 }
           then for each file: its NUL-terminated name, then its data
```

Names are paths without a leading `/` (`bin/shell`, `test.txt`); `fsd`
creates the directories along them.

The data of `bin/` files starts on a 4096-byte boundary of the image, so
the loader can map a program's read-only segments from the image VMO
without copying them.

The image is padded to whole pages and placed on a page boundary inside
the kernel, and the image VMO is made of those same pages (a "wired"
VMO, never freed): the image sits in RAM once, not twice. With the
commands that saves about 18 MB, which matters on small machines
(128 MB under QEMU leaves about 58 MB for the kernel to manage).

`userspace/links` lists extra names, one `NAME PROGRAM` per line
(`ls coreutils`): each becomes a `bin/NAME` entry whose data offset and
size are those of `bin/PROGRAM`, so the multi-call binaries are stored
once. Lines naming a missing program or an existing file are skipped.
`build.rs` prints `Boot image: N files, M links`.

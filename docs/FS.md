# Rustux Filesystem: Service, Protocol and Boot Image

Files live in a userspace service, `fsd`, as in Fuchsia: the kernel keeps
no filesystem code (checklist step 8). Programs reach it through the C
library (`open`, `read`, `opendir`, ...; `docs/USERSPACE.md`).

## The tree

| Path | Filesystem | Notes |
|------|------------|-------|
| `/` | tmpfs | in memory, writable; lost at reboot |
| `/tmp` | tmpfs | |
| `/root` | tmpfs | root's home (`$HOME`); the shell starts here |
| `/etc` | tmpfs | settings (`hostname`, `timezone`, `adjtime`, `keymap`, `passwd`, `group`, `mtab`), copied from `/boot/etc` at start |
| `/boot` | bootfs | the boot image, read-only |
| `/bin` | bootfs | `/boot/bin` mounted again: the programs |
| `/usr` | bootfs | `/boot/usr` mounted again (`/usr/share/zoneinfo/zones`, `/usr/share/keymaps/`, `/usr/share/man/`, `/usr/share/tests/`) |
| `/dev` | devices | `null` (reads nothing, takes every write) and `zero` (reads zeros); no new files here |
| `/svc` | tmpfs | services: `devmgr`, `edu`, ... (step 12; see below) |
| `/mnt` | tmpfs | an empty directory to mount disks on (step 14) |
| `/boot/efi` | FAT | the EFI system partition, mounted at boot from `/etc/fstab` when there is one (step 14) |

Writes, creates, renames and removals under `/boot`, `/bin` and `/usr` fail with
`EROFS`. Filesystems on disks are mounted into this tree (see Mounts):
FAT12/16/32 since step 14, ext2/ext4 in steps 15-16.

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

Types: 1 file, 2 directory, 3 device (`/dev/null`, `/dev/zero`; step 10),
4 service (step 12; `stat` shows `S_IFSOCK`).

`rx_fs_attr_t` is `{ u32 type; u32 mode; u64 size; u64 ino; u64 nlink;
i64 mtime_ns; u32 read_only; u32 fsid; }`. Times are UTC (nanoseconds
since 1970). `fsid` tells the filesystems apart (1 tmpfs, 2 bootfs,
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

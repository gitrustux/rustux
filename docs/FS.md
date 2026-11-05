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
| `/etc` | tmpfs | settings (`hostname`, `timezone`, `adjtime`, `keymap`), copied from `/boot/etc` at start |
| `/boot` | bootfs | the boot image, read-only |
| `/bin` | bootfs | `/boot/bin` mounted again: the programs |
| `/usr` | bootfs | `/boot/usr` mounted again (`/usr/share/zoneinfo/zones`, `/usr/share/keymaps/`) |
| `/dev` | devices | `null` (reads nothing, takes every write) and `zero` (reads zeros); no new files here |

Writes, creates, renames and removals under `/boot`, `/bin` and `/usr` fail with
`EROFS`. Files on disk come in phase B (FAT32 in step 14, ext2/ext4 in
steps 15-16), mounted into this tree.

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

Types: 1 file, 2 directory, 3 device (`/dev/null`, `/dev/zero`; step 10).

`rx_fs_attr_t` is `{ u32 type; u32 mode; u64 size; u64 ino; u64 nlink;
i64 mtime_ns; u32 read_only; u32 pad; }`. Times are from the monotonic
clock until step 9 brings wall-clock time.

Errors follow POSIX: `ENOENT`, `EEXIST`, `ENOTDIR`, `EISDIR`, `ENOTEMPTY`,
`EROFS`, `EBUSY` (removing or renaming a mount point), `EINVAL` (moving a
directory into itself), `EBADF` (reading a write-only connection),
`ENOSYS` (unknown op).

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

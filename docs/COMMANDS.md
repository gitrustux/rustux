# Rustux Commands

The programs in `/bin` (checklist step 11), where they come from and how
they differ from their Linux counterparts. Every command answers
`--help`; `man NAME` shows a manual page where there is one
(`/usr/share/man`, from `files/usr/share/man/`) and the `--help` text
otherwise. `help` at the shell gives a short list.

## Where they come from

| Program | Source | Commands |
|---------|--------|----------|
| `coreutils` | [uutils coreutils](https://github.com/uutils/coreutils) 0.12 (MIT), built for Rustux: `userspace/rust/coreutils` | `basename cat chmod cksum comm cp cut date dd df dirname du echo env expand expr false fmt fold head id join link ln ls md5sum mkdir mktemp mv nl nproc numfmt od paste printenv printf pwd readlink realpath rm rmdir seq sha256sum shuf sleep sort split stat sync tac tail tee test [ touch tr true truncate tsort uname unexpand uniq unlink wc whoami yes base64` |
| `tools` | Rustux, `userspace/rust/tools` (Rust), with uutils findutils 0.11 and diffutils 0.6 and the kibi editor | `grep egrep fgrep less more man edit nano tar gzip gunzip zcat find xargs diff cmp` |
| C programs | Rustux, `userspace/c-progs/` | `shell` (also `sh`), `ps kill top free uptime dmesg hostname reboot poweroff`, `lspci lsblk ramdisk` (step 12), `gpt blkid fdisk blk` (step 13), `mount umount mkfs.fat fsck.fat` and the FAT server `fatfs` (step 14), `mkfs.ext2 fsck.ext2` and the ext2 server `ext2fs` (step 15), `mkfs.ext4 tune2fs` (`fsck.ext2` also as `fsck.ext3 fsck.ext4`; `ext2fs` serves ext3 and ext4, step 16), tests |
| scripts | `userspace/bin/` | `runtests` |

One program serves many names: the boot image stores each name in
`userspace/links` as another entry pointing at the same bytes (no extra
space), and the program looks at the name it was started by. `coreutils
--list` and `tools --list` show what each one holds.

The C stand-ins of steps 7-9 (`ls`, `cat`, `echo`, `mkdir`, `rm`, `mv`,
`env`, `true`, `false`, `date`, `uname`) were replaced by uutils.
`rustica/tools/cli` had Linux versions of the system commands (reading
`/proc`); on Rustux they are C programs on the kernel's calls (step 9),
with `top` new in step 11.

## Output

The commands print what their GNU counterparts print, so scripts and
habits carry over. Machine-readable output, which `apps/CHECKLIST.md`
asks for, is an option where it has been added: `ps --json`,
`free --json`, `uptime --json`, `lspci --json`, `lsblk --json`, `blkid -o export`, `grep --json` (one object per matching
line), `tar -t --json` (one object per entry). The uutils commands have
none yet (their GNU options like `stat -c` and `ls` columns cover some
uses).

## Differences from Linux

| Command | Difference |
|---------|------------|
| `ln` | symbolic links (`-s`) on tmpfs and ext2/3/4; hard links on ext2/3/4 only (tmpfs and FAT: "Permission denied", across filesystems: "Cross-device link") |
| `chmod`, `touch` | change the recorded mode and time; permissions are not enforced until users exist (step 17); `chown` is not provided |
| `df` | lists the file systems in `/etc/mtab` (`tmpfs` or the root disk for `/`, the boot image, `/dev`, mounted disks); tmpfs sizes are memory |
| `id`, `whoami` | always root (`/etc/passwd`, `/etc/group`) |
| `uname -o` | `Rustux` |
| `date` | time zones from `$TZ` or `/etc/timezone` as the C library reads them; `date -s` fails (setting the clock needs the root resource) |
| `tail -f` | polls once a second (no inotify; patched to fall back quietly) |
| `find` | `-user`, `-group`: everything is root's |
| `xargs` | looks programs up in `$PATH` itself (patched, `rust/vendor/README.md`); `-P` works |
| `kill` | ends the process: there are no signals, so `-9`, `-TERM` etc. all mean the same |
| `ps`, `top` | PID is the process's koid; JOB is the job it runs in |
| `edit` / `nano` | kibi, not GNU nano: Ctrl+S save, Ctrl+Q quit, Ctrl+F find (`man edit`) |
| `less` | keys come from the console even when the text comes through a pipe |
| `grep` | basic (-G), extended (-E) and fixed (-F) patterns on the `regex` crate; no back-references (`\1`) |
| `tar` | creates and reads ustar/GNU archives, with `-z` for gzip; no `-j`/`-J` (bzip2, xz) |
| `man` | plain text pages, no sections; a command without a page shows its `--help` |
| `lspci` | the device list comes from devmgr (`DRIVERS.md`); names from a small built-in table (no `pci.ids`), so most devices show as `Vendor Device dddd`; `-n`, `-nn`, `-v`, `-k`, `-D`, `-s`, `-d`, `--json` (no `-t`, `-x`) |
| `lsblk` | block devices published to devmgr: disks with their partitions below them; `-b`, `-n`, `-l`, `-d`, `-f`, `-o` (`lsblk --help` lists the columns), `-J`; no `MAJ:MIN` (no device numbers); MOUNTPOINTS from `/etc/mtab` |
| `blkid` | names are devmgr's (`vda1`), not `/dev` paths; tags `TYPE LABEL UUID PARTLABEL PARTUUID PTTYPE PTUUID`; filesystems recognised by signature: vfat, exfat, ntfs, BitLocker, ext2/3/4, swap, iso9660 (no label for NTFS and exFAT); `-o full|value|export`, `-s TAG`; `-L LABEL`, `-U UUID` print the device holding that filesystem (as `findfs`) |
| `fdisk` | listing only (`fdisk -l`); tables are changed with `gpt` |
| `gpt` | Rustux's, like FreeBSD's: `show create add remove label type destroy types`; new partitions on 1 MiB boundaries; asks the driver to re-read the table, so partitions appear at once |
| `blk` | Rustux's: reads and writes block devices (`blk read vda1 -n 64`, `blk write vda5 < file`), since they are not files in `/dev` yet |
| `ramdisk` | Rustux's: serves a block device in memory until killed (`ramdisk 16M &`; `-n NAME`, `-b BLOCKSIZE`, `-r`); partition tables on it work as on a disk |
| `mount` | `mount` lists `/etc/mtab`; `mount [-t vfat] [-o ro] DEV DIR`, `mount DIR` (from `/etc/fstab`), `mount -a [-w SECONDS]`. DEV is a block device name (`sda1`, `/dev/sda1`), `LABEL=`, `UUID=`, `PARTLABEL=` or `EFI` (the EFI system partition, removable disks first). Types vfat (FAT12/16/32), ext2, ext3 and ext4 (features the server does not keep up to date: read-only); options `ro rw rw! noauto nofail defaults` (`FS.md`, "Mounts"). `mount --root SPEC` is init's, at boot (`root=` on the kernel command line) |
| `umount` | `umount [-f] DIR|DEV`, `umount -a`; busy while files are open on it (`-f` anyway). Everything is on the disk when it returns |
| `mkfs.fat` (`mkfs.vfat`) | `-F 12|16|32` (default by size: FAT12 < 8 MiB, FAT16 < 512 MiB, else FAT32), `-n LABEL`, `-i SERIAL` (hex), `-s SECTORS` per cluster, `-I` for a whole disk with a partition table; refuses mounted devices. Cluster sizes as Windows' format |
| `fsck.fat` (`fsck.vfat`) | checks only (boot sector, FAT copies, chains, sizes, `.`/`..`, long names, lost clusters, FSInfo); `-a` corrects the FSInfo free count, nothing else; `-v` lists the files. Exit 0 clean, 1 corrected, 4 errors |
| `df` | all mounted filesystems from `/etc/mtab`; FAT ones show their cluster counts |
| `mkfs.ext2` (`mke2fs`) | as Linux's `mke2fs -t ext2` without the resize inode: 4 KiB blocks (1 KiB below 4 MiB), 256-byte inodes, one per 16 KiB (4 KiB below 512 MiB), 5% reserved; `-b BLOCKSIZE`, `-i BYTES-PER-INODE`, `-L LABEL`, `-U UUID`, `-m PERCENT`, `-I` (whole disk with a partition table); refuses mounted devices |
| `fsck.ext2` (`fsck.ext3`, `fsck.ext4`) | replays the journal (`-a`, `-y`, `-p`; with `-n` only reports that it needs it), then checks: the five passes of `e2fsck` (inodes and blocks, direct and indirect or extents; directories; connectivity; link counts, the orphan list; bitmaps, as Linux makes them for "uninit" groups, and counts) and the checksums (superblock, descriptors, bitmaps, inodes, extent and directory blocks); `-a` corrects the free counts, nothing else; `-v` lists directories. Exit 0 clean, 1 corrected (or replayed), 4 errors (repair with `e2fsck -f` on Linux) |
| `mkfs.ext4` | as Linux's `mkfs.ext4` without the resize inode: features `has_journal ext_attr dir_index filetype extent 64bit flex_bg sparse_super large_file huge_file dir_nlink extra_isize metadata_csum`, 4 KiB blocks (1 KiB below 4 MiB), 256-byte inodes, one per 16 KiB (4 KiB below 512 MiB), flexible groups of 16, a journal of 4 to 512 MiB by size (`-J size=MiB`, `-O ^has_journal` for none); `-b -i -L -U -m -I` as `mkfs.ext2` |
| `tune2fs` | `-l DEV`: the superblock as Linux's `tune2fs -l` (features, counts, times, journal, checksum); `-L LABEL DEV`: a new label (not while mounted) |
| `shell` | no `fork`: see `USERSPACE.md`, "The shell", for how pipelines with built-ins run |

## Tests

`runtests` runs the command test suite (`files/usr/share/tests/`, see
`TESTS.md`); `edutest`, `blktest` and `drvtest` test the drivers
(`DRIVERS.md`); `systest`, `fstest`, `libctest`, `threadtest`, `hello-std`
and the kernel tests are listed in `USERSPACE.md` and `SYSCALL.md`.

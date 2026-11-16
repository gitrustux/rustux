# Rustux Commands

The programs in `/bin` (checklist step 11), where they come from and how
they differ from their Linux counterparts. Every command answers
`--help`; `man NAME` shows a manual page where there is one
(`/usr/share/man`, from `files/usr/share/man/`) and the `--help` text
otherwise. `help` at the shell gives a short list.

## Where they come from

| Program | Source | Commands |
|---------|--------|----------|
| `coreutils` | [uutils coreutils](https://github.com/uutils/coreutils) 0.12 (MIT), built for Rustux: `userspace/rust/coreutils` | `basename cat chgrp chmod chown cksum comm cp cut date dd df dirname du echo env expand expr false fmt fold groups head id join link ln ls md5sum mkdir mktemp mv nl nproc numfmt od paste printenv printf pwd readlink realpath rm rmdir seq sha256sum shuf sleep sort split stat sync tac tail tee test [ touch tr true truncate tsort uname unexpand uniq unlink wc whoami yes base64` |
| `tools` | Rustux, `userspace/rust/tools` (Rust), with uutils findutils 0.11 and diffutils 0.6 and the kibi editor | `grep egrep fgrep less more man edit nano tar gzip gunzip zcat find xargs diff cmp` |
| C programs | Rustux, `userspace/c-progs/` | `shell` (also `sh`), `ps kill top free uptime dmesg hostname reboot poweroff`, `lspci lsblk ramdisk` (step 12), `gpt blkid fdisk blk` (step 13), `mount umount mkfs.fat fsck.fat` and the FAT server `fatfs` (step 14), `mkfs.ext2 fsck.ext2` and the ext2 server `ext2fs` (step 15), `mkfs.ext4 tune2fs` (`fsck.ext2` also as `fsck.ext3 fsck.ext4`; `ext2fs` serves ext3 and ext4, step 16), `login su sudo passwd useradd userdel usermod groupadd groupdel chpasswd timedatectl localectl` and the services `authd` (and init's `/svc/system`) (step 17), `svc logs logger` and the services `logd` and init's service manager (step 18), `ip ping` and the driver `virtio-net` (network step D1; the service `netd` is Rust, `userspace/rust/netd`), `nc ss` (step D2; `nettest` is Rust), `host nslookup` (step D3; `curl` is Rust), the driver `e1000e` (step D4), `fwctl` and `sshd-session` (step D5; `sshd` is Rust), `netcfg` (step D6), tests |
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
| `chmod`, `chown`, `chgrp`, `touch` | permissions are enforced by the filesystem servers (step 17, `USERS.md`): `chmod` by the owner or root, `chown` by root (the owner may change the group to one of its own); FAT files are root's and cannot be changed |
| `df` | lists the file systems in `/etc/mtab` (`tmpfs` or the root disk for `/`, the boot image, `/dev`, mounted disks); tmpfs sizes are memory |
| `id`, `whoami`, `groups` | the process's credentials from the kernel, names from `/etc/passwd` and `/etc/group` |
| `uname -o` | `Rustux` |
| `date` | time zones from `$TZ` or `/etc/timezone` as the C library reads them; `date -s` fails (the clock is set by root with `timedatectl set-time`, through init) |
| `tail -f` | polls once a second (no inotify; patched to fall back quietly) |
| `find` | `-user`, `-group` work (owners are kept on tmpfs and ext2/3/4; FAT files are root's) |
| `xargs` | looks programs up in `$PATH` itself (patched, `rust/vendor/README.md`); `-P` works |
| `kill` | ends the process: there are no signals, so `-9`, `-TERM` etc. all mean the same; another user's process only as root ("Operation not permitted") |
| `ps`, `top` | PID is the process's koid; USER the user it acts as; JOB is the job it runs in |
| `edit` / `nano` | kibi, not GNU nano: Ctrl+S save, Ctrl+Q quit, Ctrl+F find (`man edit`) |
| `less` | keys come from the console even when the text comes through a pipe |
| `grep` | basic (-G), extended (-E) and fixed (-F) patterns on the `regex` crate; no back-references (`\1`) |
| `tar` | creates and reads ustar/GNU archives, with `-z` for gzip; no `-j`/`-J` (bzip2, xz) |
| `man` | plain text pages, no sections; a command without a page shows its `--help` |
| `lspci` | the device list comes from devmgr (`DRIVERS.md`); names from a small built-in table (no `pci.ids`), so most devices show as `Vendor Device dddd`; `-n`, `-nn`, `-v`, `-k`, `-D`, `-s`, `-d`, `--json` (no `-t`, `-x`) |
| `lsblk` | block devices published to devmgr: disks with their partitions below them; `-b`, `-n`, `-l`, `-d`, `-f`, `-o` (`lsblk --help` lists the columns), `-J`; no `MAJ:MIN` (no device numbers); MOUNTPOINTS from `/etc/mtab` |
| `blkid` | names are devmgr's (`vda1`), not `/dev` paths; tags `TYPE LABEL UUID PARTLABEL PARTUUID PTTYPE PTUUID`; filesystems recognised by signature: vfat, exfat, ntfs, BitLocker, ext2/3/4, swap, iso9660 (no label for NTFS and exFAT); `-o full|value|export`, `-s TAG`; `-L LABEL`, `-U UUID` print the device holding that filesystem (as `findfs`) |
| `ip` | `link`, `addr`, `route` (show; `add`/`del` of addresses and the default route, `link set up/down`, as root), `-s`, `-br`; from netd (`NETWORK.md`); no `neigh`, `rule`, `-j`, IPv6 yet |
| `ping` | IPv4 addresses and `localhost` (no names until DNS, step D3); `-c -i -W -w -s -q`; no TTL in its lines (netd does not pass it on) |
| `nc` | OpenBSD netcat's options `-l -k -u -v -z -N -w -s -p`; no `-e`/`-c` (programs on the connection), no proxies, no Unix-domain sockets, IPv4 only; a half-close at the end of the input |
| `host`, `nslookup` | `-t TYPE`, `-v`, a server; nslookup without its interactive mode; IPv4 servers |
| `curl` | HTTP/1.1 and HTTPS (TLS 1.2 and 1.3); no HTTP/2, FTP or other protocols, no cookies (`-b`/`-c`), no progress meter, `-w` with a few variables |
| `netcfg` | Rustux's own (`/etc/network.conf`, applied at once): `IFACE dhcp`, `static` or `off`, `dns`, `search`, `apply` |
| `fwctl` | Rustux's own (no iptables or nftables syntax): `allow`, `deny`, `append`, `delete`, `default`, `enable`, `disable`, `reload`, `save` |
| `ss` | `-t -u -l -a -n -H`; no `-p` (processes), `-e`, filters; Recv-Q/Send-Q are smoltcp's buffers |
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
| `login` | init runs it on the console (step 17): user name and password (three tries), then the user's shell; `login -f USER` (root only) without a password (autologin: `/etc/autologin`, `autologin=USER` on the kernel command line) |
| `su` | `su [-] [-l] [USER] [-c CMD]`: through authd (`USERS.md`); the target's password, none when root asks |
| `sudo` | `sudo [-u USER] [-i | -s] [-S] CMD...`, `sudo -l`: through authd; `/etc/sudoers` (users, `%group`, RUNAS, `NOPASSWD:`, full command paths); the caller's password, remembered 5 minutes; no `-k`, `-e`, `-b`; logged in `/var/log/auth.log` |
| `passwd` | `passwd [USER]`: through authd (one's own after the current password; root any); SHA-512 in `/etc/shadow`; no `-l -u -d -e` (see `usermod -L/-U`) |
| `useradd` | `-m -d -s -u -g -G -c`; uids from 1000, a group of its own, the password locked until set; `-m` copies `/etc/skel` |
| `userdel`, `usermod` | `userdel [-r]`; `usermod [-a] -G GROUPS`, `-g -s -d -c`, `-L` / `-U` (lock, unlock) |
| `groupadd`, `groupdel` | `groupadd [-g GID]`; a group that is someone's primary group is not removed |
| `chpasswd` | lines `USER:PASSWORD` on stdin (root) |
| `timedatectl` | `status`, `list-timezones`, `set-timezone ZONE`, `set-time "YYYY-MM-DD HH:MM:SS"`, `set-local-rtc 0|1`; no NTP |
| `localectl` | `status`, `list-keymaps`, `set-keymap NAME` (the input service changes layout at once, for the console and graphics alike); the locale is always `C.UTF-8` |
| `poweroff`, `reboot` | root (or `sudo poweroff`): init stops the services (logd last), writes out and marks clean every filesystem, then the power |
| `svc` (`systemctl`) | the service manager (step 18, `SERVICES.md`): `list` (`list-units`, `list-unit-files`), `status [UNIT]` (with the unit's last log lines), `start stop restart`, `enable disable [--now]`, `reload` (`daemon-reload`), `reset-failed`, `isolate rescue|multi-user`, `is-active is-enabled is-failed`, `cat`, `show`, `get-default`, `poweroff`, `reboot`; `--json`; units are `/etc/svc/NAME.svc`; no `mask`, timers, sockets |
| `logs` (`journalctl`) | the system log (`/var/log/system.log`): `-u -t -k -b [N] -p -g -S/--since -U/--until -n -r -f -o short|short-iso|json|cat --list-boots --disk-usage`; root and group adm; no `--vacuum`, `--rotate`, `-D` |
| `efibootmgr` | the firmware's boot entries (step 19, `BOOT.md`): list (`-v`), `-o` order, `-n`/`-N` BootNext, `-b N -B` delete, `-a`/`-A`, `-t`/`-T` timeout, `-c [-d DISK -p PART] [-l LOADER] [-L LABEL]` (default: rboot on the ESP this boot came from); changes: root; no `-r` (driver entries), `-e`, `-@` |
| `bootctl` | the boot manager rboot through systemd's loader variables (step 19): `status`, `list`, `set-default`, `set-oneshot`, `set-timeout`, `set-timeout-oneshot`, `reboot-to-firmware`, `is-installed`, `--esp-path=`; no `install`, `update`, `remove` yet (step 20) |
| `rustica-install` | the installer (step 20, `INSTALL.md`): asks (keyboard, host, time zone, user, root, disk, erase/alongside/replace/manual), shows every change, writes after `install`; `--disk NAME|auto --mode ... --user ... --yes --dry-run` for scripts; log `/var/log/install.log` |
| `fbtest` | draws through the display service (step F2, `DISPLAY.md`): `-n FRAMES` animation with frame times (0: until Esc or q), a cursor that follows the mouse with its clicks and wheel printed (step F3), `--pattern S` a still test pattern, `--check` snapshot equals what was drawn; Esc or q ends it; root |
| `evtest` | prints input events (step F3, `INPUT.md`): takes the focus from the input service and prints keys (usage, modifiers, the layout's text) and pointer motion, buttons and wheel until Esc (or `-t SECONDS`); root |
| `compositor` | windows on the screen (step F4, `WINDOWS.md`): click to focus, drag to move and resize, title bar buttons, Alt+Tab, Alt+F4, F11; `-v` prints what happens; root |
| `wintest` | a test window: `NAME [-c RRGGBB] [-s WxH] [-p X,Y] [-a] [-n] [-k] [-t TEXT]`; prints the events it gets |
| `rustica-window` | a window from Rust (the `rustica-window` crate's demo) |
| `desktop` | start the desktop (root; the `desktop` unit: compositor, graphical login, session); `--stop`; `--run` is the unit's program (step F6, `SESSION.md`) |
| `notify` | `notify TEXT...`: a notification on the desktop (top right, 5 s) |
| `greeter`, `panel`, `launcher` | the graphical login, the panel and the application launcher (started by `desktop` and the panel) |
| `terminal` | the Terminal (step F7, `APPS.md`): the shell (or `-e PROG ARGS`) on a pseudo-terminal in a window; tabs, copy and paste, font size, scrollback |
| `files` | the file manager (step F7): `files [FOLDER]`; copy, cut, paste, rename, trash, new folders, properties, open with an application; USB sticks mounted under `/media` and ejected |
| `settings` | Settings (step F7): `settings [display|keyboard|mouse|time|users|disks|about]`; system changes as root, or with the user's password through `sudo -S` |
| `editor`, `viewer` | the Text Editor (`editor [FILE]`, Ctrl+S, Ctrl+Q) and the Image Viewer (`viewer FILE`, Left/Right) |
| `inputctl` | the input service's settings in `/etc/input.conf`: `status`, `set-pointer-speed PERCENT` (25-400; root) |
| `uidemo` | the toolkit's demo (step F5, `GUI.md`): menus, tabs, a form, a long list, a tree, a text editor field, images, dialogs; `--scale 2` |
| `screenshot` | `screenshot FILE.png`: the screen (text console or graphics) as a PNG, with the CRC-32 of its pixels; root |
| `logger` | `-t TAG -p [FACILITY.]PRIO -s -f FILE [MESSAGE]`; to logd (`/svc/log`); no `--journald`, `-u` |
| `dmesg` | the kernel's log since boot (the last 64 KiB); `logs -k` has it with times |
| `hostname` | `hostname NAME` writes `/etc/hostname` (root) |
| `shell` | no `fork`: see `USERSPACE.md`, "The shell", for how pipelines with built-ins run |

## Tests

`runtests` runs the command test suite (`files/usr/share/tests/`, see
`TESTS.md`); `edutest`, `blktest` and `drvtest` test the drivers
(`DRIVERS.md`); `ptytest` tests pseudo-terminals (`APPS.md`); `systest`,
`fstest`, `libctest`, `threadtest`, `hello-std` and the kernel tests are
listed in `USERSPACE.md` and `SYSCALL.md`.

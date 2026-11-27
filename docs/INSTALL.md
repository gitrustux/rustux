# Installing Rustica Server (step 20)

The live image is the install image. Its boot menu (rboot, BOOT.md) has

- **Try Rustica Server**: the live system, from memory: the disks are not
  touched unless you mount them yourself (the kernel reads
  `\EFI\BOOT\cmdline.txt` if there is one, as before);
- **Install Rustica**: the kernel command line says `installer`,
  so init boots the **install target**: the log, the mounts and the
  installer on the console (`installer.svc`), nothing else;
- the recovery entry for the first, and "Firmware setup".

`rustica-install` (as root) also runs from a live session. When the
installer started by init ends without restarting, the live system takes
the console (`svc isolate multi-user`).

## What it asks

Each question is a screen of its own in the boot menu's style
(2026-11-26): the red frame with the step's name, the question, then the
choices numbered. A number picks one at once, or the arrows and Enter;
choices that cannot be taken here are shown dimmed with the reason (no
GPT, too little free space, no earlier Rustica, a disk too small). The
time zones (a long list) are found by typing part of a name (Berlin) or
their number. Only names and passwords are typed, and the two safety
checks: `install` to go on after the summary, and the disk's name before
an internal NVMe disk is written. Without a terminal (input from a pipe)
the lists are printed numbered and a number is read.

1. The edition (package step E3, `PACKAGES.md`): server, the text login,
   or desktop, the graphical login (the default when the live system
   runs the desktop; `--edition`). Then the language (English, for now)
   and keyboard layout (English (US), German, English (UK); used at
   once, so the passwords are typed in it).
2. Host name, time zone (a list to search).
3. The first user: full name, user name, password; it is an
   administrator (group `sudo`). Root: locked (the default: use `sudo`),
   or a password.
4. The disk: every disk with its size, model, partition table,
   partitions (type, filesystem, label) and the systems on them (the
   loaders on each EFI partition: Windows Boot Manager, Linux, Rustica;
   NTFS data; an earlier Rustica root). The installation medium is listed
   but not offered.
5. How:
   - **erase**: a new GPT with an EFI partition (512 MiB, FAT32) and the
     root (the rest, ext4);
   - **alongside**: in the disk's largest free space (at least 512 MiB,
     plus 512 MiB if there is no EFI partition to reuse). The installer
     does not shrink partitions: shrink Windows' in Windows first;
     an EFI partition it reuses must have room for Rustica's kernel (it
     carries the whole system: about 70 MiB in November 2026; Windows'
     own EFI partition is often only 100 MB, about 30 MB of it used).
     When it has not, alongside, replace and that partition in manual
     are not offered (the menu says why), and with `--mode` the
     installer stops before it writes anything;
   - **replace** an earlier Rustica (its EFI partition is reused, its root
     partition, labelled "Rustica root", formatted);
   - **manual**: the EFI partition (an existing one, or new in the free
     space) and the root (a partition to format, or new).
6. A summary of every change: what is created, what is formatted, what
   is kept untouched (each other partition is named). Nothing has been
   written before this point. Typing `install` writes; anything else
   stops.
7. For the computer's internal NVMe disk, once more: its name, to allow
   writing to it (below).

## What it does, in this order

1. NVMe only: unlocks the disk for writing (`RX_BLOCK_OP_ALLOW_WRITE`).
2. The partition table: a new one (erase) or the new partitions added to
   the existing one (the others keep their numbers and contents); the
   disk re-reads it.
3. `mkfs.fat -F 32 -n ESP` for a new EFI partition, `mkfs.ext4 -L
   rustica-root -U <new UUID>` for the root.
4. The root: the top directories; `/etc` copied from the boot image (as
   fsd seeds a fresh disk), without `/etc/autologin` (the installed system
   asks who logs in); `hostname`, `timezone`, `keymap`, `fstab` (the EFI
   partition rboot started from at `/boot/efi`), `motd`; the user in
   `passwd`, `group` (its own, and `sudo`), `shadow` (SHA-512 crypt); root
   locked (`!`) or its hash; `/home/USER` (0750); with Windows on the disk
   `/etc/adjtime` says LOCAL (Windows keeps the hardware clock in local
   time).
5. The EFI partition, mounted at `/run/install/esp` (beside the new
   root: fsd mounts nothing inside another filesystem), and
   `/etc/rustica/boot.conf` (`options root=UUID=... quiet`, and `nvme=rw`
   on NVMe). When the medium has the system's packages (`\rustica\packages`,
   package step E3): an earlier Rustica's `\EFI\Rustica\rustux*` and
   `rustica*` entries removed, then `rpg --root` installs `rboot`,
   `rustica-system-VERSION`, `rustica-system` and the edition
   (`RPG_ESP=/run/install/esp`: the new system's `/boot/efi`), whose
   scripts write the entries (`rpg boot update`: "Rustica Server", or
   "Rustica" and "Rustica (text console)" for the desktop edition), and
   the installed system's package database knows its image. A medium
   without them gets the earlier way: `\EFI\Rustica\rustux.efi` (the kernel the live system
   started from: `\EFI\Rustica\rustux.efi` on the medium, or its
   `\EFI\BOOT\BOOTX64.EFI` when that is the kernel) and `rboot.efi` (from
   the medium, or `/usr/lib/rboot/rboot.efi` in the boot image), written
   under other names and then renamed; `\EFI\BOOT\BOOTX64.EFI` (rboot) only
   if there is none (Windows' fallback is never replaced); `\loader\
   loader.conf` (timeout 3, or 5 with Windows) and `\loader\entries\
   rustica.conf` (`root=UUID=... quiet`, and `nvme=rw` on NVMe), and
   `rustica-desktop.conf`, the same with `desktop`: "Rustica (desktop)",
   the graphical login (desktop step F6, `SESSION.md`). rboot finds
   Windows and Linux itself at each boot.
6. Everything written out; the log (`/var/log/install.log`) copied to the
   new system; both unmounted.
7. Last: a firmware boot entry "Rustica" for `\EFI\Rustica\rboot.efi`
   (an existing one for the same loader on the same partition is reused)
   first in `BootOrder`.

A power cut before step 7 leaves the firmware starting what it started
before (Windows, an earlier Rustica): their partitions and files are not
touched, and the firmware's order has not changed. After a cut, "replace"
or "manual" finishes the job (the new root partition is there).

`--dry-run` stops after the summary and writes nothing. The log of every
step and every program run: `/var/log/install.log`.

## The internal NVMe disk

The NVMe driver serves its disk **write-locked** (RX_BLOCKDEV_WRITE_LOCKED,
DRIVERS.md): every write is refused, as in steps 13-19, unless

- the kernel command line has `nvme=rw`, which the installer puts in the
  entry of a Rustica installed on the NVMe disk, or
- root sends `RX_BLOCK_OP_ALLOW_WRITE` (with the words "allow writes"),
  which only the installer does, after the summary was accepted and the
  disk's name typed. It lasts until the driver ends (the next boot).

So a live session never writes to the laptop's disk by accident, and the
installed system can.

## Options (scripts, tests)

```
rustica-install --disk NAME|auto --mode erase|alongside|replace|manual
                [--esp PART|new] [--root PART|new]
                --user NAME (--password PASS | --password-file FILE) [--fullname TEXT]
                [--lock-root | --root-password PASS]
                [--hostname NAME] [--timezone ZONE] [--keymap NAME]
                [--yes] [--dry-run] [--reboot | --no-reboot]
```

`--disk auto`: the only disk besides the medium. `--yes`: no questions
(the summary is still shown and logged; NVMe disks are unlocked without
the extra question). Passwords on the command line are visible to `ps`:
use them in tests only.

## Tests

`scripts/run-install-test.sh` (TESTS.md): QEMU with a live medium made as
`build-live-image.sh` makes it, and

- an empty 2 GiB **NVMe** disk: the menu's Install entry and the installer
  answered on the console; the disk alone boots through rboot to a login
  prompt, the user logs in (groups `sudo`, the time zone), writes a file,
  `sudo poweroff`; the next boot has the file; `e2fsck -f` is clean;
- the same disk, **replaced** from a live session, twice: one firmware
  entry (the second installation reuses the first's), the new user logs
  in, the old one is gone;
- a disk with **"Windows"** (an EFI partition with a stand-in loader as
  `bootmgfw.efi` and as the fallback, a data partition) and free space: a
  dry run writes nothing; installed **alongside**; rboot's menu starts
  "Windows"; Rustica starts by default with LOCAL in `/etc/adjtime`;
  Windows' data partition and boot files are byte for byte the same;
- the same disk with the **power cut** during the installation: the
  computer still starts "Windows".
- a disk whose EFI partition (40 MiB) is too small for the kernel:
  alongside is refused and nothing is written.

## Not done (yet)

- No swap (Rustux has none); no encryption; no LVM or RAID.
- Partitions are not shrunk or moved: free space must exist.
- MBR disks: only "erase" (which makes a GPT).
- Only English; the keyboard layouts there are (`us`, `de`, `uk`).
- `bootctl install`/`update` (a new kernel onto the EFI partition of an
  installed system): reinstalling with "replace" does it, keeping nothing
  of the old root.

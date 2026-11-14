# Booting: rboot, the firmware's entries (step 19)

**rboot** is Rustica's boot manager: a UEFI application on an EFI system
partition (ESP) that shows a menu of what can be started (Rustica, its
recovery and live variants, Windows, Linux, the firmware setup), counts
down to the default, and starts the chosen one with its command line. It
is modelled on systemd-boot: the same entry files, the same `loader.conf`
keys, the same EFI variables, so `bootctl` works with it. The running
system changes the firmware's boot entries with `efibootmgr`.

Code: `rboot/` (its own crate: `cd rboot && cargo build --release --target
x86_64-unknown-uefi`; `build.sh` builds it), the kernel's side in
`src/efi.rs` (runtime services) and `src/main.rs` (`quiet`, `bootfs=`),
`userspace/c-progs/efibootmgr.c`, `bootctl.c`, `efivar.h`.

## The ESP

```
\EFI\BOOT\BOOTX64.EFI          rboot (what the firmware starts from a stick)
\EFI\Rustica\rboot.efi         rboot (what a firmware entry names)
\EFI\Rustica\rustux.efi        the kernel (with its boot image built in)
\loader\loader.conf            the menu's settings
\loader\entries\*.conf         one file per entry
```

`build-live-image.sh` lays the stick out this way (`RBOOT=0`: the kernel as
`BOOTX64.EFI`, as before step 19), with two entries (step 20): "Try Rustica
Server" with no options (the kernel then still reads
`\EFI\BOOT\cmdline.txt` when there is one) and "Install Rustica Server"
(`installer`: INSTALL.md). rboot adds no recovery entry for an entry whose
options say `installer`. An installed system's ESP is laid out the same
way by `rustica-install`.

### loader.conf

| Key | Meaning |
|-----|---------|
| `default ID` | the entry to start (`*` at the end matches any rest: `rustica*`) |
| `timeout N` | seconds to wait (default 5); `0` starts at once unless a key is held; `menu-force` waits forever |
| `editor no` | no `e` (editing the command line) |
| `auto-entries no` | no Windows or Linux entries found on the disks |
| `auto-firmware no` | no "Firmware setup" entry |

### Entries

`\loader\entries\ID.conf`; ID is the file's name without `.conf`:

```
title   Rustica
efi     /EFI/Rustica/rustux.efi
options root=UUID=... quiet
bootfs  /EFI/Rustica/bootfs.img   (optional: a boot image from a file)
sort-key 1                         (optional: the order; then the id)
```

`linux` is taken for `efi`. For each entry whose program is `rustux.efi`
rboot adds **ID-recovery** ("(recovery)": the options without `quiet` and
`autologin`, plus `rescue`: the rescue target, a root shell) and, for the
first one with a `root=` other than `live`, **ID-live** ("(live, in
memory)": `root=live`, the disks untouched). Without any Rustica entry, an
`\EFI\Rustica\rustux.efi` on rboot's partition is offered as "Rustica".

Found on every FAT partition the firmware sees: `\EFI\Microsoft\Boot\
bootmgfw.efi` ("Windows Boot Manager", id `auto-windows`) and, in each
other `\EFI\DIR`, `shimx64.efi`, `grubx64.efi` or `systemd-bootx64.efi`
("Linux (Dir)", id `auto-dir`). "Firmware setup" (`auto-reboot-to-
firmware-setup`) when the firmware supports it: it sets the
OsIndications bit and restarts.

## The menu

Up/Down, Home/End choose; Enter starts; `e` edits the command line of
this start only (Enter starts, Esc cancels); `d` makes the selection the
default (`LoaderEntryDefault`). Any key stops the countdown. If a program
returns or cannot be loaded, rboot says so and shows the menu again.

The default is, in order: `LoaderEntryOneShot` (once, then removed),
`LoaderEntryDefault` (`bootctl set-default`, or `d`), `default` in
loader.conf, the first entry. The timeout: `LoaderConfigTimeoutOneShot`,
`LoaderConfigTimeout` (`bootctl set-timeout`), loader.conf, 5 s.

rboot writes what it shows to the debug port (QEMU's debugcon) too:
`rboot: N entries: ...`, `rboot: booting ID in N s`, `rboot: booting ID
(PATH OPTIONS)`.

## Variables

rboot speaks systemd's boot loader interface (vendor GUID
`4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`):

| Variable | Set by | Meaning |
|----------|--------|---------|
| `LoaderInfo` | rboot | "rboot 0.1" |
| `LoaderEntries` | rboot | the menu's ids |
| `LoaderEntrySelected` | rboot | the id it started |
| `LoaderDevicePartUUID` | rboot | its partition's GUID (`mount` mounts that ESP as `EFI`) |
| `LoaderEntryDefault` | bootctl, `d` | the default (kept) |
| `LoaderEntryOneShot` | bootctl | the next boot's (removed when used) |
| `LoaderConfigTimeout`, `...OneShot` | bootctl | the timeout |

## The kernel's side

- The **command line** is the load options rboot gives (else
  `\EFI\BOOT\cmdline.txt`, as before); the kernel logs it (`[BOOT] command
  line: ...`). New words: `quiet` (the kernel's boot lines stay off the
  screen; `dmesg` has them), `noefi` (no runtime services), `bootfs=PATH`
  (the boot image from that file on the kernel's partition, loaded before
  ExitBootServices into memory the kernel then leaves alone). FS.md lists
  them all.
- **Runtime services**: at boot the kernel keeps the UEFI memory map's
  runtime regions and calls SetVirtualAddressMap with their kernel
  addresses (code and data in the direct map, MMIO uncached), so the
  firmware's variable services work afterwards. The syscall
  `efi_variable` (SYSCALL.md) gives userspace GetVariable,
  GetNextVariableName (anyone) and SetVariable (root).

## Commands

- `efibootmgr` (as Linux's): lists `BootCurrent`, `BootNext`, `Timeout`,
  `BootOrder` and each `BootNNNN` with its device path; `-o` order, `-n`/
  `-N` BootNext, `-b N -B` delete, `-a`/`-A` active, `-t`/`-T` timeout,
  `-c [-d DISK -p PART] [-l LOADER] [-L LABEL]` a new entry first in the
  order (default: rboot on the partition this boot came from). Changes:
  root.
- `bootctl`: `status`, `list` (the menu's entries, the default and the
  current one marked), `set-default ID`, `set-oneshot ID`, `set-timeout
  N`, `set-timeout-oneshot N`, `reboot-to-firmware [yes|no]`,
  `is-installed`; `--esp-path=` (default `/boot/efi`). Installing rboot
  is `rustica-install`'s job (INSTALL.md).

## Tests

`scripts/run-rboot-test.sh` (TESTS.md): QEMU with a variable store of its
own and two GPT disks (`scripts/gpt-wrap.py`): rboot's ESP and one with
stand-ins for Windows and Debian (`rboot/src/bin/fakeos.rs`, which reports
how it was started and powers off). Six boots: the menu to Windows, to
Debian, an edited command line (`noefi`), the default with `quiet` where
`efibootmgr -c` and `bootctl set-default`/`set-timeout` are used, the
firmware starting that new entry and rboot the recovery entry after 1 s,
and an entry with `bootfs=`.

## Not done (yet)

- Secure Boot: rboot and the kernel are not signed (turn Secure Boot off,
  or enrol them yourself); no shim.
- No graphical menu, no mouse; the text console's 80x25 mode.
- No boot counting (automatic fallback after failed boots), no
  `LoaderTimeInitUSec`/`LoaderTimeExecUSec` timing variables.
- Linux kernels are not started directly (no EFI-stub `linux` with
  `initrd`): their own loaders (shim, GRUB) are.
- `bootctl install`, `update`, `remove`: `rustica-install` installs rboot
  (INSTALL.md); there is no separate update yet.

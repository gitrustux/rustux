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
| C programs | Rustux, `userspace/c-progs/` | `shell` (also `sh`), `ps kill top free uptime dmesg hostname reboot poweroff`, `lspci lsblk ramdisk` (step 12), tests |
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
`free --json`, `uptime --json`, `lspci --json`, `lsblk --json`, `grep --json` (one object per matching
line), `tar -t --json` (one object per entry). The uutils commands have
none yet (their GNU options like `stat -c` and `ls` columns cover some
uses).

## Differences from Linux

| Command | Difference |
|---------|------------|
| `ln` | fails (Permission denied): no hard or symbolic links yet |
| `chmod`, `touch` | change the recorded mode and time; permissions are not enforced until users exist (step 17); `chown` is not provided |
| `df` | lists the file systems in `/etc/mtab` (`tmpfs` for `/`, the boot image, `/dev`); sizes are memory |
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
| `lsblk` | block devices published to devmgr (RAM disks for now; disks in step 13); no partitions, `MAJ:MIN` or mount points yet; `-b`, `-n`, `-J` |
| `ramdisk` | Rustux's: serves a block device in memory until killed (`ramdisk 16M &`) |
| `shell` | no `fork`: see `USERSPACE.md`, "The shell", for how pipelines with built-ins run |

## Tests

`runtests` runs the command test suite (`files/usr/share/tests/`, see
`TESTS.md`); `edutest`, `blktest` and `drvtest` test the drivers
(`DRIVERS.md`); `systest`, `fstest`, `libctest`, `threadtest`, `hello-std`
and the kernel tests are listed in `USERSPACE.md` and `SYSCALL.md`.

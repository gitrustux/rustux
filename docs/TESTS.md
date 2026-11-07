# Testing the Rustux Kernel

How to build the kernel, boot it in QEMU, and debug it. This replaces the older `TEST_INTERRUPTS.md` and the January version of this file.

## Prerequisites

```bash
rustup toolchain install nightly          # the kernel uses #![feature(abi_x86_interrupt)]
rustup target add x86_64-unknown-uefi
sudo apt install qemu-system-x86 ovmf dosfstools mtools
```

Without `rustup` (e.g. a sandbox), a distro Rust plus its matching source package also works, building `core`/`alloc` from source:

```bash
sudo apt install rustc-1.91 cargo-1.91 rust-1.91-src lld
RUSTC_BOOTSTRAP=1 cargo-1.91 build --release --bin rustux --features uefi_kernel \
  --target x86_64-unknown-uefi \
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem \
  --config 'target.x86_64-unknown-uefi.linker="lld-link"'
```

## Build

```bash
./build.sh                                   # kernel + rustux.img
# or the kernel only:
cargo build --release --bin rustux --features uefi_kernel --target x86_64-unknown-uefi
```

The output is `target/x86_64-unknown-uefi/release/rustux.efi`. **Check its size:** a working kernel is about 1.8 MB. A file of a few KB means the compiler proved the entry path always panics and removed the kernel (this was the cause of the old "EFI OK" freeze).

`build.rs` builds a boot image (bootfs, `docs/FS.md`) with every program in `userspace/bin/` as `bin/<name>`; the kernel starts `bin/init` from it, and the filesystem service serves it as `/boot` and `/bin` (`init`, `shell`, the commands and the tests). The binaries are committed, so building the kernel needs no C compiler.

Rebuilding the user programs and the C library after changing C code (needs `gcc` and `ar`; details in `docs/USERSPACE.md`):

```bash
userspace/build.sh
```


## Command tests (step 11)

`build.sh` ends by running the command test suite in QEMU
(`scripts/run-command-tests.sh`, which needs `qemu-system-x86_64`, OVMF,
`socat` and `python3`; `SKIP_TESTS=1 ./build.sh` skips it). It boots the
kernel, types `runtests -v` at the shell and fails the build unless every
test passes. The tests are shell scripts in `files/usr/share/tests/`
(served as `/usr/share/tests/`), each with the output it must print in a
`.out` file next to it; `runtests` at the Rustux shell runs them by hand.

To add a test: write `NAME.sh`, run it with `bash --posix` on Linux to make
`NAME.out` (the commands behave like GNU's), check that output by eye, and
run `runtests NAME` on Rustux.

## Device tests (step 12)

The command tests boot QEMU with `-device edu,dma_mask=0xffffffff`, QEMU's
educational PCI device, so `devices.sh` also runs `edutest`: the `edu`
driver (started by `devmgr`) maps the device's registers, takes an MSI
interrupt and does DMA in both directions (10 checks). `blktest` (16
checks, block protocol against RAM disks) and `drvtest` (driver calls
refused without the root resource) need no device. Without the `edu`
device, `devices.sh` skips `edutest`, so the laptop runs it too.

Step 13 adds a disk: the command tests make one with
`scripts/make-test-disk.py` and attach it three times, as virtio-blk, as
NVMe and as a USB stick on a `qemu-xhci` controller (`snapshot=on`, so
the file stays as it was), and `disks.sh` checks all three drivers.

By hand: `-device qemu-xhci,id=x -drive if=none,id=u,file=stick.img,format=raw -device usb-storage,bus=x.0,drive=u`;
`device_del` / `device_add` in the QEMU monitor unplug and plug it. `partitions.sh` uses RAM disks and runs anywhere. The
script also checks a disk image after Rustux wrote to it
(`make-test-disk.py check disk.img`), with its own GPT code.
`docs/DRIVERS.md` explains the device mask and what each test covers.

## FAT tests (step 14)

`fat.sh` (in the command tests, on RAM disks, so it runs anywhere) makes
FAT32, FAT16 and FAT12 filesystems with `mkfs.fat` (also on 4 KiB
sectors), mounts them and checks file operations through `fsd`'s
forwarding: long and non-ASCII names, case-insensitive lookups, a large
copy, a directory of 120 files, append, truncate, renames and moves,
removal, times, the read-only bit, refused names, `df`, `umount` while a
file is open (busy), a read-only mount, and `fsck.fat` on each result.
`disks.sh` also checks that the test disk's EFI partition on the USB
stick was mounted at `/boot/efi` at boot.

`scripts/run-fat-test.sh` checks Rustux's FAT against the host's tools:
it boots QEMU with a FAT32 disk made by the host's `mkfs.fat` and an
empty one, has Rustux run `/usr/share/fat/host-check.sh` (about 60 MiB
of large files and the main programs copied, removals and renames,
`mkfs.fat` on the empty disk, then filling it up), then runs the host's `fsck.fat -n` on both
and reads every file back with mtools. It needs dosfstools and mtools.

On a real machine: mount the stick's EFI partition (it is at `/boot/efi`
after boot), write files, `umount`, then open them in Windows, or run
`fsck.fat -n` on Linux.

## ext2 tests (step 15)

`ext2.sh` (in the command tests, on RAM disks) checks symbolic links on
tmpfs (relative, absolute, dangling, loops, `realpath`), then on a
filesystem made by `mkfs.ext2`: files and a large copy, a directory of
400 entries growing past a block, symbolic links within, out of and
pointing at directories (fast and slow ones), hard links, sparse files
and truncation, times and set-user-ID bits, renames across directories
and into itself (refused), `umount` while a file is open, `fsck.ext2`, a
read-only mount; with 1 KiB blocks, double indirect blocks and a full
disk.

`scripts/run-ext2-test.sh` boots QEMU three times: with `root=UUID=...`
in `\EFI\BOOT\cmdline.txt`, so the root filesystem is a host-made ext2
disk, `boot1.sh` copies about 30 MiB and the main programs onto another
host-made ext2 disk (`mkfs.ext2 -d`, with links and a sparse file),
removes, renames and links, runs `mkfs.ext2` on a third disk and fills
it, and writes files under `/root` and `/home`; the second boot finds
them; the third, with no command line and no disks, runs from memory.
Then the host's `e2fsck -f -n` checks the three disks (all must be clean
and marked clean) and `debugfs` reads the files back. It needs e2fsprogs.

## ext4 tests (step 16)

`ext4.sh` (in the command tests, on RAM disks): `mkfs.ext4` and `tune2fs
-l`, a large copy, two files appended to in turns (many extents) and
truncated, a directory of 300 names, a slow symbolic link, a hard link,
a file removed while open (its space comes back at close), `fsck.ext4`,
a read-only mount; then 1 KiB blocks without a journal, filled up.

`scripts/run-ext4-test.sh` boots QEMU with `root=UUID=...`: the root
filesystem is ext4 made by the host's `mkfs.ext4` with its defaults.
`/usr/share/ext4/boot1.sh` copies about 30 MiB and the main programs onto
a second host-made ext4 disk (`-O orphan_file,metadata_csum_seed`, with
links, a sparse file and a directory `e2fsck -D` has indexed: names are
added to and removed from it), removes, renames and links, runs
`mkfs.ext4` on a third disk, fills it, `fsck.ext4` and `tune2fs -l`; the
second boot finds what the first wrote on the root. The host's `e2fsck
-f -n` checks the disks (clean, marked clean, no recovery needed),
`debugfs` reads the files back, and the third disk must have Linux's
default features.

Then the power-loss test, `POWERLOSS_RUNS` times (6): `load.sh` writes
hard on an ext4 disk and on the root (copies, appends, small files,
renames, removals, truncation, links) until QEMU is killed, 3 to 20
seconds in. Copies of both disks go to the host's `e2fsck -fy`, which
must replay the journal Rustux wrote and find nothing else to fix; then
Rustux boots again, replays it itself at mount, unmounts, and the host's
`e2fsck -f -n` must find both disks clean.

The server's own code was also tested on the host (not in the tree): the
same ext2fs.c driven by a random workload (files, appends, overwrites,
truncation, removal while open, renames, links, symbolic links,
directories, full disks) against images from Linux's `mkfs.ext2`,
`mkfs.ext3` and `mkfs.ext4` (defaults, 1 KiB blocks, without a journal,
`uninit_bg` without 64-bit, `orphan_file`), checked by e2fsck and against
a model of the files; and cut off at a random write, with each unflushed
write kept or lost at random, then replayed (by ext2fs and by e2fsck)
and checked.

## Run in QEMU

```bash
./test-qemu.sh          # builds rustux.img, boots it, checks the log for [TICK]
```

USB keyboards: add a controller and devices, e.g. a keyboard behind a hub, and turn off PS/2 so typing can only come through USB:

```bash
qemu-system-x86_64 -machine q35,i8042=off ... \
  -device qemu-xhci,id=x -device usb-hub,bus=x.0,port=1 \
  -device usb-tablet,bus=x.0,port=1.1 -device usb-kbd,bus=x.0,port=1.2
```

The monitor commands `device_del` / `device_add` on the hub simulate a KVM switching away and back. The boot log lists each device (`[USB] Port 5: ...`, `keyboard 0627:0001 ready`).

Try different memory sizes too: the PMM is built from the firmware's memory map, so `-m 128M`, `-m 512M` and `-m 6G` (memory above 4 GiB) exercise different layouts.

Manually:

```bash
qemu-system-x86_64 -machine q35 -m 512M -smp 1 \
  -bios /usr/share/ovmf/OVMF.fd \
  -drive file=rustux.img,format=raw \
  -debugcon file:/tmp/rustux-qemu-debug.log \
  -no-reboot
```

To build an image by hand:

```bash
dd if=/dev/zero of=rustux.img bs=1M count=64
mkfs.fat -F 32 rustux.img
mkdir -p esp/EFI/BOOT && cp target/x86_64-unknown-uefi/release/rustux.efi esp/EFI/BOOT/BOOTX64.EFI
mcopy -i rustux.img -s esp/EFI ::
```

### Expected debug output

The kernel writes progress to the QEMU debug port (0xE9). A good boot looks like this (heap and PMM lines omitted):

```
EBS OK
[PMM] UEFI memory map: 9 free regions -> 8 arenas
[PMM]   0x100000 - 0x800000  7 MiB
...
[PMM] Usable: 454 MiB, page records: 2748 KiB at 0x1b8c6000
[VM] Kernel page tables active, physical memory mapped at 0xffff800000000000
[VM] Kernel image relocated to 0xffffffff80000000
[STACK] Switching to kernel stack: vaddr=0xffff800000442000 size=0x40000
[INIT] Heap: 16 MB at physical 0x1780000
[INIT] Heap initialized successfully (16MB)
[DMA] Self-test passed
[1/5] Setting up GDT...           ✓ GDT configured
[2/5] Setting up IDT...           ✓ IDT configured
[3/5] Installing timer handler... ✓ Timer handler at vector 32
[3.5/5] Installing keyboard handler...
[3.6/5] Installing syscall handler...
[4/5] Initializing APIC...        ✓ APIC initialized
[4.5/5] Configuring keyboard IRQ... IRQ1 → Vector 33
PS/2: test=55 cfg=77->65 port=00 kbd=FA
[5/5] Configuring timer...
[DISPLAY] Text console initialized
[INIT] init started
[TICK]
```

The screen turns red at UEFI entry and green after ExitBootServices, then the console clears to the Dracula background and the shell prompt (`rustux >`) appears. Try `help`, `ls /bin`, `echo hi > /tmp/a`, `cat /tmp/a | upper`, `fstest`.

A CPU exception in a user program makes the shell print `[crashed (CPU exception): -1025]`; the kernel prints a `[FAULT] ...` line with RIP, error code and fault address on the debug port; the shell keeps running.

## Debugging techniques

**Watch CPU exceptions and interrupts.** Add `-d int,cpu_reset -D /tmp/qemu.log`. Each line shows the vector (`v=0e` page fault, `v=0d` general protection, `v=06` invalid opcode, `v=20` timer, `v=21` keyboard), the privilege level (`cpl=3` is userspace), `IP` and `CR2`.

**Use the QEMU monitor.** Add `-monitor unix:/tmp/mon.sock,server,nowait`, then send commands with `socat`:

```bash
m() { echo "$1" | socat - UNIX-CONNECT:/tmp/mon.sock; }
m stop                       # pause the CPU
m "info registers"           # RIP, RSP, CR3 ...
m "gva2gpa 0x401000"         # where does a virtual address map in the current CR3?
m "xp /16xb 0x2001000"       # dump physical memory
m "sendkey h"; m "sendkey ret"   # type into the guest
m "screendump /tmp/screen.ppm"   # capture the framebuffer
```

**Debug with GDB.** Add `-s -S` to QEMU and attach with `gdb` → `target remote :1234`.

**Inspect the binary.** `objdump -d target/x86_64-unknown-uefi/release/rustux.efi | head` shows whether the kernel was optimised away (a tiny `.text` ending in a `hlt; jmp` loop is the panic handler).

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| `rustux.efi` is a few KB | Something on the entry path always panics (e.g. `mem::zeroed()` of a reference, `uefi::system::with_stdout` after ExitBootServices); the optimizer removed the rest |
| QEMU exits right after a step (with `-no-reboot`) | Triple fault; run with `-d int,cpu_reset` and look at the last exception |
| `bin/init missing from the boot image` (kernel panic) | `userspace/bin/` is empty: run `userspace/build.sh` |
| `[FAULT] ... in KERNEL mode` | Kernel bug; the line shows RIP and (for page faults) the address |
| A program crashes at once | Built with the `syscall` instruction instead of `int 0x80`, or not with `userspace/build.sh` |
| Triple fault on the first interrupt from user mode | TSS/IDT/GDT descriptor wrong for an upper-half address (the TSS descriptor once dropped bits 63:48 of its base) |
| No `[TICK]` | LAPIC timer not configured, or interrupts not enabled |
| `[PMM] OUT OF MEMORY` | The line shows what was asked for (pages, flags) and how many pages are still free |
| `PS/2: test=55 ... kbd=FA` | 8042 self-test passed (`55`), keyboard port OK (`00`), keyboard acknowledged scanning (`FA`); `--` means no reply |
| `[DMA] SELF-TEST FAILED` | Contiguous or below-4 GiB allocation broken in the PMM |

## Kernel test mode

`--features kernel_test` builds `src/test_entry.rs`, a minimal interrupt test (GDT, IDT, LAPIC timer) that prints `[TICK n]` every 10 ticks on the debug port and reports `TEST PASSED` once it has seen at least 10 ticks. The handlers it uses are in `src/arch/amd64/test.rs`.

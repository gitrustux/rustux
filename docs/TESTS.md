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

`build.rs` embeds a ramdisk with `bin/init`, `bin/shell`, `bin/hello` and `bin/counter`. It uses freshly built `target/*.elf` files when present, otherwise the prebuilt binaries in `test-userspace/` and `userspace/c-progs/`.

Rebuilding the user programs (no C library; `-ffreestanding -fno-builtin` stops GCC from inserting `strlen`/`memcpy` calls):

```bash
F="-static -nostdlib -ffreestanding -fno-builtin -fno-stack-protector -fno-pie -no-pie -O2"
gcc $F -o test-userspace/init.elf test-userspace/init.c
gcc $F -o test-userspace/shell/shell.elf test-userspace/shell/shell.c
gcc $F -T userspace/c-progs/linker.ld -o userspace/c-progs/hello.elf userspace/c-progs/hello.c
gcc $F -T userspace/c-progs/linker.ld -o userspace/c-progs/counter.elf userspace/c-progs/counter.c
```

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
[INIT] Found init.elf in ramdisk
[INIT] Entry point: 0x401000
[SPAWN] Loading process from ramdisk: /bin/shell
[SPAWN] Created process PID=2 entry=0x401080
[TICK]
```

The screen turns red at UEFI entry and green after ExitBootServices, then the console clears to the Dracula background and the shell prompt (`rustux >`) appears. Try `help`, `hello`, `counter`, `echo hi`, `ps`.

A CPU exception in a user program prints `[<name>: process killed]` on screen and a `[FAULT] ...` line with RIP, error code and fault address on the debug port; the shell keeps running.

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
| `init.elf not found in ramdisk` | Ramdisk built without ELFs, or offsets wrong in `build.rs` |
| `[FAULT] ... in KERNEL mode` | Kernel bug; the line shows RIP and (for page faults) the address |
| Fault in user mode right after `[SPAWN]` | Program linked over the kernel image, or built with the `syscall` instruction instead of `int 0x80` |
| No `[TICK]` | LAPIC timer not configured, or interrupts not enabled |
| `[PMM] OUT OF MEMORY` | The line shows what was asked for (pages, flags) and how many pages are still free |
| `PS/2: test=55 ... kbd=FA` | 8042 self-test passed (`55`), keyboard port OK (`00`), keyboard acknowledged scanning (`FA`); `--` means no reply |
| `[DMA] SELF-TEST FAILED` | Contiguous or below-4 GiB allocation broken in the PMM |

## Kernel test mode

`--features kernel_test` builds `src/test_entry.rs`, a minimal interrupt test (GDT, IDT, LAPIC timer) that prints `[TICK n]` every 10 ticks on the debug port and reports `TEST PASSED` once it has seen at least 10 ticks. The handlers it uses are in `src/arch/amd64/test.rs`.

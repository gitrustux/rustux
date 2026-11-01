# Project History and Decisions

A short record of how Rustux got to its current shape and why. It merges the old `docs/decisions/` notes, the original porting brief, early planning chats and the January 2025 `PLAN.md`.

## Origins: seL4, then Zircon

The project started as **"Rustic"**, a kernel for an OS called **Rustica**. The first idea was to translate **seL4** into Rust. That was dropped: seL4's value is its formal verification (Isabelle proofs with Haskell models), and a Rust translation would lose the proofs while keeping the hard parts.

The direction became a **Zircon-style capability microkernel** (the Fuchsia kernel): handles with rights, jobs/processes/threads, VMOs, channels and ports. The kernel was renamed **Rustux**; Rustica stays the name of the OS built on it.

## Original porting brief

The first plan was a file-by-file port of Zircon to Rust. The conventions from that brief still apply to the codebase:

- Rename **Zircon/Fuchsia → Rustux** and the **`zx` prefix → `rx`** (e.g. `rx_handle_t`, `RxStatus`).
- License: **MIT**. Copyright lines read **"The Rustux Authors"**.
- Translate everything to Rust except assembly (`.S`) and build files, which are reviewed and updated.
- Use Rust doc comments, `Result`-based error handling, explicit memory barriers, `usize` for indices, Rust naming conventions, and full implementations rather than stubs.

The large architecture checklist that grew out of this plan is now [`ROADMAP.md`](ROADMAP.md).

## 2025-01: Wrong directory structure

The first refactor created `rustux/kernel/src/kernel/...` with bootloaders and apps alongside. The kernel was moved to the repository root (`src/arch`, `src/drivers`, `src/interrupt`, `src/sched`, `src/mm`, `src/process`, `lib.rs`). The interrupt-controller abstraction from that refactor (the `InterruptController` trait and `X86_64InterruptController`, using the IOAPIC for routing and sending EOI to the local APIC) survived and is in `src/traits.rs` and `src/arch/amd64/`.

## 2025-01: "No output from QEMU + OVMF"

A January 2025 investigation concluded that QEMU 7.2 + OVMF produced no console output for EFI applications at all, which blocked testing of PMM debug counters, the kernel stack reservation (0x200000–0x240000) and userspace page tables. With current QEMU and OVMF the debug port (0xE9) works; see [`TESTS.md`](TESTS.md).

## 2026-01: Monorepo split

The project was split into separate repositories:

| Repository | Contents |
|------------|----------|
| `gitrustux/rustux` | This kernel |
| `gitrustux/loader` | Legacy UEFI kernel (`kernel-efi`), `uefi-loader`, USB/xHCI work |
| `gitrustux/rustica` | The OS distribution; includes this kernel as a submodule at `repo/kernel` |
| `gitrustux/apps` | Userspace applications |

The old monorepo is preserved as tag `archive/monorepo` in this repository.

## 2026-05 to 2026-10: Keyboard and boot problems

Keyboard input on real hardware failed for months. The causes turned out to be a PS/2-only driver on USB-only machines, several xHCI driver bugs, and a kernel build that never ran past "EFI OK". The full story is in [`usb-keyboard.md`](usb-keyboard.md).

## 2026-10-31: Consolidation

- `gitrustux/rustux` (the `prod/` folder on the server) became the only kernel source; the copy in `loader/rustux` was merged and removed.
- The "EFI OK" stub build was fixed; the kernel now boots in QEMU through every subsystem and enters ring 3.
- Remaining blocker at that point: user programs ran inside the firmware identity mapping copied into every address space (resolved in the next entry).

## 2026-10-31: Kernel memory management and first working userspace

Checklist step 1. The kernel now builds its own page tables at boot: all physical memory is mapped at `0xffff_8000_0000_0000` (PML4 entry 256, 1 GiB pages where the CPU supports them, 2 MiB otherwise), and the kernel stack, heap, page tables, APIC and framebuffer are reached through that range. Each process gets a private lower half that contains only its own pages plus the kernel image mapped supervisor-only (the old code copied the firmware's lower half into every process and added the User bit to kernel entries). Processes switch by swapping kernel stacks (`src/process/run.rs`) on yield, exit and user-mode timer ticks, with 128 KiB kernel stacks (16 KiB overflowed in `spawn`). CPU exceptions are handled: user faults kill the process, kernel faults halt with a report.

Along the way: the syscall ABI moved to the documented registers (`rdi, rsi, rdx, ...`); the console got a full 8×16 font, ANSI colours and the Dracula palette; Enter/Backspace no longer fire twice (they also triggered on key release); `getpid` returns the real PID. In QEMU the kernel boots to the shell and runs `hello`, `counter`, `echo` and `ps`.

## 2026-11-01: Physical memory from the UEFI memory map, DMA allocator

Checklist step 1, continued. The PMM no longer uses fixed zones (kernel 2–16 MB, user 32–128 MB) and a heap at a fixed address outside them. `main.rs` records every free region of the UEFI memory map after ExitBootServices; the PMM sorts and merges them, splits them at 4 GiB, and makes one arena per region. The page records (24 bytes per 4 KiB page) are carved from RAM and reached through the direct map, instead of a 2 MB static buffer that could describe at most ~340 MB. Allocation starts from a per-arena hint instead of scanning from the first page, prefers memory above 4 GiB, and supports aligned contiguous runs. The kernel heap and boot stack are allocated from the PMM (the boot stack used to be 64 single pages assumed to be contiguous).

`src/mm/dma.rs` adds `dma_alloc` / `dma_alloc_aligned` / `dma_free`: zeroed, physically contiguous buffers with both addresses, optionally below 4 GiB and aligned to a boundary, for the USB port. A self-test runs at boot. Tested in QEMU with 128 MB, 512 MB and 6 GB of RAM.

## 2026-11-01: PS/2 driver ready for real hardware

Lessons from getting `kernel-efi` to take input on the test laptop (MSI GS75; see the loader README's hardware notes). The firmware there leaves the 8042's keyboard clock disabled, so the driver now sets the controller up itself: ports off, flush, self-test, configuration byte with IRQ1 and translation on and the mouse port off, keyboard port test, keyboard port on, scanning on, with one diagnostic line (`PS/2: test=55 cfg=75->65 port=00 kbd=FA`). The IRQ handler drains every pending byte, the input buffer shared with it uses volatile accesses, and `read_char` polls the controller when IRQ1 is not delivered.

## 2026-11-01: USB keyboards

`src/drivers/usb/` ports the `kernel-efi` USB driver that was brought up on the test laptop (Intel `8086:06ED`, keyboards behind a KVM's VIA VL817 hub): PCI scan for the chipset xHCI controller, BIOS handoff, reset, rings and event ring in DMA memory, commands with real timeouts, abort and retry, control transfers, USB 2.0 hubs (nested, transaction translator for full/low-speed devices behind a high-speed hub), up to four HID boot keyboards, and root-port hot-plug so a KVM can switch away and back. Differences from `kernel-efi`: every structure comes from the DMA allocator (physical address for the controller, direct-map address for the CPU), the registers are mapped uncached in a new MMIO window (`kernel_map::map_mmio`, PML4 entry 257, shared by all address spaces), and 64-byte contexts are supported. The driver is polled from the keyboard's `read_char`, so keys and hot-plug are serviced while a program waits on `read`. The TSC is now calibrated against the PIT at boot (it was a fixed 2 GHz guess). `init` and the shell no longer clear the screen, so the boot log (USB devices found) stays visible.


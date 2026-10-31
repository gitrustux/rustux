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
- Known blocker: user programs are linked at `0x400000`, inside the firmware identity mapping that every address space copies into its lower half. Next step: move the kernel out of the lower half.

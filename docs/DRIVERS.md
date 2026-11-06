# Drivers in Userspace: devmgr, PCI and Block Devices

Step 12 moves device drivers out of the kernel, as in Fuchsia. The kernel
keeps only what must be privileged: mapping device memory, routing
interrupts, and handing out DMA addresses. A **device manager** (`devmgr`)
finds the PCI devices and starts one driver process per device; a driver
gets its own device's resources and nothing else.

The kernel still drives two things itself: the screen (UEFI frame buffer)
and the USB keyboard (xHCI). Moving xHCI out comes with USB storage in
step 13; until then devmgr leaves display and USB controllers alone.

## Pieces

| Piece | Where | What |
|-------|-------|------|
| Kernel calls | `docs/SYSCALL.md` ("Drivers") | physical and contiguous VMOs, cache policy, BTIs and pinning, MSI interrupt objects, I/O port resources, ACPI tables |
| `devmgr` | `userspace/c-progs/devmgr.c` | PCI bus driver and device manager; serves `/svc/devmgr` |
| `/etc/drivers` | `files/etc/drivers` | which program drives which device |
| Device channel | `<rustux/device.h>` | a driver's only way to its device |
| Block protocol | `<rustux/block.h>` | what disk drivers serve and filesystems use |
| Services | `<rustux/service.h>`, `docs/FS.md` | `/svc/NAME`: a program's own protocol reached by path |
| `edu` | `userspace/c-progs/edu.c` | test driver for QEMU's `edu` device |
| `ramdisk` | `userspace/c-progs/ramdisk.c` | a block device in memory |
| `lspci`, `lsblk` | `userspace/c-progs/` | listings (`docs/COMMANDS.md`) |
| Tests | `drvtest`, `edutest`, `blktest`, `/usr/share/tests/devices.sh` | `docs/TESTS.md` |

## Start-up

1. `init` starts `fsd`, then `devmgr` with a copy of the root resource
   (nobody else gets it; the shell gets only the power resource).
2. `devmgr` reads the ACPI MCFG table (`acpi_table_read`) and maps the PCI
   configuration space (ECAM) as an uncached physical VMO, one 1 MiB bus
   window at a time. Without MCFG it uses ports 0xCF8/0xCFC
   (`ioports_request`).
3. It walks bus 0 and every bus behind a PCI-to-PCI bridge, and records
   each function: IDs, class, subsystem, BARs, capabilities (MSI, MSI-X,
   PCI Express), legacy interrupt pin and line.
4. BARs are sized (written with all ones, decoding off meanwhile) except on
   devices the kernel uses (display, USB) and bridges; lspci shows those
   without a size.
5. It checks that the kernel refuses a device-memory VMO over RAM (with
   the address of a buffer it just pinned); a warning is printed if not.
6. It publishes `/svc/devmgr` and starts the drivers named in
   `/etc/drivers`. It prints nothing unless something goes wrong.

`init` restarts `devmgr` if it ends (three times at most).

## /etc/drivers

```
# VENDOR:DEVICE         PROGRAM
1234:11e8   /bin/edu
# class=CC[:SS[:PI]]    PROGRAM
class=01:08:02  /bin/nvme      (step 13)
```

The first matching line for a device wins. Devices the kernel uses are
never matched. The driver is started with the device's address as its
argument (`edu 00:03.0`), copies of devmgr's standard output and error,
the namespace, and one handle, `PA_USER0`: the device channel.

## The device channel

Requests and replies are `{ u32 txid; u32 op; }` / `{ u32 txid; i32
status; }` followed by fields (`<rustux/device.h>`); `rx_dev_call()` in
the C library does one round trip.

| Op | Request | Reply |
|----|---------|-------|
| `GET_INFO` 1 | — | `rx_pci_info_t` |
| `CONFIG_READ` 2 | `u16 offset, u16 width` | `u32 value` |
| `CONFIG_WRITE` 3 | `u16 offset, u16 width, u32 value` | — (EPERM for BARs, the ROM, MSI and MSI-X: devmgr's) |
| `GET_BAR` 4 | `u32 index` | `rx_pci_bar_t` + a handle: memory BAR: a physical VMO (uncached), I/O BAR: an IOPORT resource for `ioports_request`; decoding is turned on |
| `GET_INTERRUPT` 5 | `u32 mode, u32 index` | `u32 mode used` + an interrupt object |
| `GET_BTI` 6 | — | a BTI for DMA |
| `BUS_MASTER` 7 | `u32 enable` | — |

Interrupts: `RX_DEV_IRQ_MSI` (index 0), `RX_DEV_IRQ_MSIX` (index below
the table size) or `RX_DEV_IRQ_ANY` (MSI-X if the device has it). devmgr
takes a free MSI vector from the kernel (`interrupt_create` with
`RX_INTERRUPT_MSI`), writes its address and data into the device's MSI
capability or MSI-X table entry, enables it, turns on bus mastering and
turns off INTx. Legacy INTx is not offered yet: which IOAPIC input a PCI
pin is wired to is in the ACPI namespace (`_PRT`), which needs an AML
interpreter; the devices that matter (NVMe, xHCI, virtio, network) all
have MSI or MSI-X.

When a driver ends, devmgr turns off the device's MSI, MSI-X and bus
mastering, closes the channel and starts the driver again. After five
endings within 5 seconds of starting, in a row, it gives up and says so.
`lspci -k` shows the driver's process and whether it was restarted.

### What a driver does

```c
rx_handle_t dev = rx_take_startup_handle(RX_PA_USER0), vmo, irq, bti;
rx_pci_bar_t bar;
uint32_t idx = 0, mode[2] = { RX_DEV_IRQ_ANY, 0 }, used;
rx_dev_call(dev, RX_DEV_OP_GET_BAR, &idx, 4, 0, &bar, sizeof bar, 0, &vmo);
rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, len, &regs);
rx_dev_call(dev, RX_DEV_OP_GET_INTERRUPT, mode, 8, 0, &used, 4, 0, &irq);
rx_dev_call(dev, RX_DEV_OP_GET_BTI, 0, 0, 0, 0, 0, 0, &bti);
// DMA buffer: contiguous memory, its device address from bti_pin
rx_vmo_create_contiguous(bti, 8192, 0, &buf);
rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE | RX_BTI_CONTIGUOUS, buf, 0, 8192, &dev_addr, &pmt);
for (;;) { rx_interrupt_wait(irq, 0); /* talk to the device */ rx_interrupt_ack(irq); }
```

`edu.c` is a complete example.

## DMA

A **BTI** (bus transaction initiator) stands for one device's view of
memory. With it a driver allocates contiguous buffers
(`vmo_create_contiguous`, optionally below 4 GiB) and pins any VMO
(`bti_pin`), which returns the address of each page as the device must
use it and keeps the pages in place until `pmt_unpin` (or until the PMT
handle is closed, for example when the driver dies). Without an IOMMU
these are physical addresses; with one (later) the same calls would map
the pages in the device's translation tables, so drivers need no change.

## /svc/devmgr

| Op | Request | Reply |
|----|---------|-------|
| `LIST_PCI` 1 | — | `u32 count`, `rx_pci_info_t[count]` |
| `LIST_BLOCK` 2 | — | `u32 count`, `rx_block_info_t[count]` |
| `CONNECT_BLOCK` 3 | name; a handle: the server end of a new connection | — (ENOENT: no such device) |
| `PUBLISH_BLOCK` 4 | `rx_block_info_t` (name: a prefix); a handle: a channel for connection requests | `rx_block_info_t` with the name given (`ram0`) |

A published block device is removed when its server's channel closes
(the server ended). Connections are passed to the server as
`rx_svc_connect_t` messages carrying the handle, as `fsd` does for
services.

## Block devices

The block protocol (`<rustux/block.h>`): requests are `rx_block_req_t {
txid, op, block, count, vmoid, vmo_offset }`, replies `{ txid, status }`.

| Op | What |
|----|------|
| `GET_INFO` 1 | `rx_block_info_t`: name, block count, block size (power of two, at least 512), flags (read-only, removable), driver, model |
| `READ` 2 / `WRITE` 3 | `count` blocks from `block`, data in the message (at most 32 KiB) |
| `FLUSH` 4 | everything written is on the medium |
| `ATTACH_VMO` 5 | a VMO handle; reply: `u32 vmoid` |
| `READ_VMO` 6 / `WRITE_VMO` 7 | `count` blocks between the device and the attached VMO at `vmo_offset` (any size) |
| `DETACH_VMO` 8 | forget a vmoid |

Errors: `ERANGE` past the end, `EROFS` writing a read-only device,
`EINVAL` for bad fields. `rx_block_open(name)` and `rx_block_call()` in the
C library are the client side. `ramdisk SIZE &` serves one from memory;
the disk drivers of step 13 serve the same protocol.

## Testing

```sh
qemu-system-x86_64 ... -device edu,dma_mask=0xffffffff
```

The `edu` device's DMA engine accepts only addresses under its DMA mask,
28 bits by default; QEMU stops with an error if a buffer lies above it, so
the tests raise the mask to 4 GiB (buffers come from below 4 GiB).

| Program | Checks |
|---------|--------|
| `edutest` | asks the `edu` driver to run its checks: no root resource, identification and liveness registers through the BAR VMO, the factorial unit, an MSI raised by the device, DMA both ways with a contiguous buffer and with pinned pages, the BTI's pinned count (10 checks) |
| `blktest` | block protocol against RAM disks it starts: info, read/write, range and size errors, flush, VMO transfers, read-only devices, removal (16 checks) |
| `drvtest` | every driver call refused without the root resource (13 checks) |
| `devices.sh` | in `runtests`: lspci, lspci --json, blktest, ramdisk + lsblk, edutest when an edu device is present |

## Not yet

- MSI-X is programmed (table entry, enable, unmask) but no test device
  uses it yet; the NVMe and virtio drivers of step 13 will
- INTx through the IOAPIC (needs `_PRT` from the ACPI namespace)
- An IOMMU (VT-d / AMD-Vi): DMA addresses are physical
- Write-combining mappings (needs the PAT)
- Hot-plug, power management, PCI segments other than 0
- The kernel's xHCI driver as a userspace driver (with USB storage, step 13)

# Drivers in Userspace: devmgr, PCI and Block Devices

Step 12 moves device drivers out of the kernel, as in Fuchsia. The kernel
keeps only what must be privileged: mapping device memory, routing
interrupts, and handing out DMA addresses. A **device manager** (`devmgr`)
finds the PCI devices and starts one driver process per device; a driver
gets its own device's resources and nothing else.

The kernel still drives two things itself: the screen (UEFI frame buffer)
and the USB controller (xHCI): keyboards, and since step 13 the transport
of USB mass storage, whose SCSI driver is in userspace (`usb-storage`,
below). devmgr leaves display and USB controllers alone.

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
| Driver helpers | `<rustux/driver.h>` | config space, BARs, interrupts, DMA buffers in plain calls (step 13) |
| Disk serving | `<rustux/blockdev.h>` | the block protocol, partitions and rescans for every disk driver (step 13) |
| Partition tables | `<rustux/gpt.h>`, `<rustux/fsprobe.h>` | GPT read/write, MBR read, filesystem signatures (step 13) |
| `virtio-blk`, `nvme`, `usb-storage` | `userspace/c-progs/` | disk drivers (step 13) |
| `lspci`, `lsblk`, `blkid`, `fdisk`, `gpt`, `blk` | `userspace/c-progs/` | listings and tools (`docs/COMMANDS.md`) |
| Tests | `drvtest`, `edutest`, `blktest`, `/usr/share/tests/devices.sh`, `partitions.sh`, `disks.sh` | `docs/TESTS.md` |

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
1af4:1001   /bin/virtio-blk
1af4:1042   /bin/virtio-blk
# class=CC[:SS[:PI]]    PROGRAM
class=01:08:02  /bin/nvme
# usb=CLASS[:SUBCLASS[:PROTOCOL]]  PROGRAM   (USB devices the kernel set up)
usb=08:06:50    /bin/usb-storage
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
| `PUBLISH_BLOCK` 4 | `rx_block_info_t`; a handle: a channel for connection requests | `rx_block_info_t` with the name given: `#` in the name becomes the first free number (`ram#` → `ram0`), `@` the first free letter (`vd@` → `vda`); a name without either is used as it is (`vda1`), `EEXIST` if taken |

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
| `RESCAN` 9 | read the partition table again and replace the partition devices (whole disks; `EBUSY` while a partition is open) |

`rx_block_info_t` also carries partition data (step 13): flags
`PARTITION`, `GPT`, `MBR`; `label`, `type_guid`, `guid` (a disk's: the
GPT disk GUID), `parent`, `start` (first block on the parent), `number`.

Errors: `ERANGE` past the end, `EROFS` writing a read-only device,
`EINVAL` for bad fields. `rx_block_open(name)` and `rx_block_call()` in the
C library are the client side. `ramdisk SIZE &` serves one from memory;
the disk drivers of step 13 serve the same protocol.

## Disk drivers (step 13)

A disk driver supplies three functions over whole blocks, `read`,
`write` (NULL: read-only) and `flush`, and `<rustux/blockdev.h>` does the
rest: it reads the partition table, publishes the disk and one device
per partition, serves the block protocol to every client (requests on a
partition are moved by its start and checked against its end), splits
large transfers to the driver's limit, and re-reads the table on
`RESCAN`. One request at a time; the driver copies between the caller's
buffer and its DMA buffer.

```c
rx_block_info_t info = { .name = "vd@", .block_count = n, .block_size = 512 };
rx_blockdev_ops_t ops = { rd, wr, fl };
rx_blockdev_publish(&ops, 0, &info, max_blocks, 0);   // info.name is now "vda"
return rx_blockdev_run();
```

`<rustux/driver.h>` wraps the device channel (`rx_drv_cfg_read`,
`rx_drv_find_cap`, `rx_drv_map_bar`, `rx_drv_interrupt`, `rx_drv_bti`),
waits for an interrupt with a time limit (`rx_drv_wait_irq`, built on
the new deadline of `interrupt_wait`) and allocates pinned, contiguous
DMA buffers (`rx_dma_alloc`).

| Driver | Devices | Notes |
|--------|---------|-------|
| `virtio-blk` | `1af4:1001`, `1af4:1042`: QEMU, KVM, cloud servers | virtio 1.0 (the capabilities' configuration structures), one split virtqueue, MSI-X vector 0 (polling if the device has none or no interrupt comes within 10 s); `vda`, `vdb`, ...; read-only if the device says so; flush when offered |
| `usb-storage` | USB class 8, bulk-only (`usb=08:06:50`) | see "USB storage"; `sda`, `sdb`, ...; removable flag from INQUIRY, read-only when the write-protect switch is on |
| `nvme` | class `01:08:02` | **read-only until the installer (step 20)**: no write function, no write command in the driver at all; one admin and one I/O queue pair on MSI-X (MSI, or polling), PRP lists for transfers over 8 KiB, the controller's transfer limit (MDTS), 512-byte and 4 KiB blocks, the first active namespace: `nvme0n1` |

### USB storage

The USB controller stays with the kernel, which already enumerates every
device for the keyboards (hubs and KVM switches included). For a
mass-storage interface with the bulk-only protocol it sets the
configuration and both bulk endpoints up and keeps the device in a table;
`SYSCALL.md` ("USB mass storage") has the five calls. devmgr asks for the
list every second (`usb_list`, with the root resource), so sticks plugged
in later are found; for a new device it opens a handle (`usb_open`) and
starts the driver named by the `usb=` line in `/etc/drivers` with it.
When the device disappears devmgr stops its driver, which removes its
block devices; a stick plugged in again is a new device.

`usb-storage` speaks Bulk-Only Transport (a 31-byte command block, the
data, a 13-byte status block) and SCSI: INQUIRY, TEST UNIT READY, READ
CAPACITY (10 and 16), READ and WRITE (10, or 16 beyond 2 TiB blocks),
MODE SENSE (write protection), SYNCHRONIZE CACHE, REQUEST SENSE. A stall
is cleared and the status read; a bad status block, a phase error or a
time-out leads to a reset recovery (Bulk-Only Reset and both endpoints
cleared) and the command is tried again, up to three times. LUN 0 only.

Time-outs (step 15): 10 s for the command block, 30 s for the data and
the status (as Linux's SCSI disks), since a stick may stall while it
erases flash. A command that times out twice (with a reset recovery
between) is split: from then on commands move at most 16 KiB, after
another such failure 4 KiB (some sticks cope with short writes only);
if even that fails, the driver has the kernel reset the stick's port
(`usb_reset`) and ends; the stick is enumerated again and served by a new
driver (its block devices come back, mounted filesystems on it get
I/O errors).

Events the controller posts while the kernel waits for one transfer
(keyboard reports, port changes) are kept for the keyboard loop, so
typing during disk transfers is not lost.

### Partitions

GPT (UEFI specification, chapter 5) is read from the primary header, or
from the backup at the end of the disk if the primary is damaged (both
header and entry CRCs are checked); entries outside the disk are
ignored. Without a GPT, the four primary MBR entries are used (extended
partitions are listed, not followed). Partitions are named after the
disk: `vda1`, `nvme0n1p2` (with `p` after a digit), numbered by their
GPT entry. `gpt` writes the backup copy first, then the primary, then the
protective MBR, so a crash leaves one good copy.

Filesystems are recognised by signature (`<rustux/fsprobe.h>`: vfat,
exfat, ntfs, BitLocker, ext2/3/4, swap, iso9660) for `blkid` and
`lsblk -f`; mounting them starts in step 14.

Block devices are not files in `/dev` yet: `blk read` and `blk write`
reach them from the shell.

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
| `drvtest` | every driver call refused without the root resource, the USB calls included (17 checks) |
| `devices.sh` | in `runtests`: lspci, lspci --json, blktest, ramdisk + lsblk, edutest when an edu device is present |
| `partitions.sh` | in `runtests`: GPT create, add, label, remove, destroy on RAM disks of 512-byte and 4 KiB blocks; writes through a partition read back through the disk; lsblk, blkid, fdisk -l (any machine) |
| `disks.sh` | in `runtests`: with the test disk attached as virtio-blk, NVMe and a USB stick (the build's command tests do this), its partitions, the NTFS signature, a new partition written and read back (virtio, USB), NVMe writes refused; prints nothing without those disks |

`scripts/make-test-disk.py make DISK.img` builds the test disk: 64 MiB,
GPT, shaped like a Windows laptop's (EFI System on FAT, Microsoft
reserved, Microsoft basic data with an NTFS boot sector) plus an ext4
Linux partition and free space. `check DISK.img` verifies a GPT with its
own code (both copies, CRCs) after Rustux wrote to it. Attach disks with
`snapshot=on` when the image must stay as it is: OVMF writes an `NvVars`
file to the EFI partition of any writable disk at every boot.

## Not yet

- The xHCI driver in userspace (it stays in the kernel; storage uses its
  bulk transfers); USB Attached SCSI (UAS), several LUNs, USB 3 hubs
  (a USB 3 stick behind a USB 3 hub is not found yet; on a root port or
  a USB 2 hub it is)
- AHCI (SATA)
- NVMe writes (step 20), several I/O queues, more than one namespace
- Several requests in flight per disk
- INTx through the IOAPIC (needs `_PRT` from the ACPI namespace)
- An IOMMU (VT-d / AMD-Vi): DMA addresses are physical
- Write-combining mappings (needs the PAT)
- Hot-plug, power management, PCI segments other than 0
- The kernel's xHCI driver as a userspace driver (with USB storage, step 13)

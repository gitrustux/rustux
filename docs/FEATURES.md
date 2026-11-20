# What Rustux and Rustica do differently

The strengths of the Rustux kernel and the Rustica operating system's
design, compared with Windows and Linux. They are properties of how the
system is built, so they hold as it grows; how far each one is done is in
the documents it points to.

## Security from capabilities, not from deny lists

A Rustux process reaches only what it holds a handle to: a file, a
channel, a service, a piece of memory, a device's registers. There is no
global name space a program can wander through (docs/OBJECTS.md,
docs/SYSCALL.md). Fuchsia and seL4 work this way.

- **An app's sandbox is what it was given.** A FoxDen app starts with a
  filesystem view of its own folders, a window and what its permissions
  add. Anything else it cannot name, so it cannot reach it, and the
  kernel enforces this (docs/FOXDEN.md "The sandbox").
- **Job policies** shut off the calls that reach the system without a
  handle (credentials, clocks, devices, power), for an app and everything
  it starts, and they can never be removed.
- Linux sandboxes (Flatpak, containers) and Windows' AppContainer start
  from a process that can see everything and add rules to block it; a
  forgotten rule is a hole. Here a forgotten grant is only a missing
  feature.

## A microkernel: failures stay where they happen

Drivers and filesystems are ordinary programs, not kernel code: the
keyboard, PCI, virtio and NVMe disks, USB storage, network cards, ext4
and FAT (docs/DRIVERS.md, docs/FS.md).

- A driver that crashes is restarted by `devmgr`; the rest of the system
  carries on.
- A driver gets only its own device's registers, interrupt and DMA
  memory, so a bug in it cannot reach the rest of memory or other
  devices.
- On Windows and Linux most drivers run inside the kernel, where one
  faulty driver can crash or compromise the whole machine.

## Written in Rust, with the rules checked by the build

- The kernel and most of the system are Rust. Memory-safety bugs (use
  after free, buffer overflows, data races) are the largest class of
  security bugs in Windows and Linux (Microsoft and Google both put it at
  about 70%); Rust rules most of them out.
- The kernel build refuses `static mut` (shared state goes through locks,
  atomics or per-CPU data: docs/CONCURRENCY.md) and new assembly outside
  `src/arch` (docs/PORTING.md).

## Updates that cannot leave a half-updated system

The system is one image: the kernel with its read-only `/bin` and `/usr`,
installed as a package beside the previous one (docs/PACKAGES.md).

- An update replaces the whole image. Nothing is changed in place, so a
  power cut during an update leaves the old system as it was.
- The previous image stays bootable, and **boot counting** falls back to
  it by itself if a new one fails to start.
- Repositories are signed (Ed25519), and a published version never
  changes.
- Windows updates and Linux package upgrades change files of the running
  system one by one, so a failure halfway can leave a broken mix. This is
  closer to ChromeOS or Fedora Silverblue, and built in from the start.

## Apps kept apart from the system

- Each app lives in its own folder, `/apps/ID`, with its data in the
  user's own folders. Two apps can never conflict, and removing one leaves
  nothing behind (docs/FOXDEN.md).
- Installing a sandboxed app needs no administrator: it cannot change the
  system. System changes always do.
- The apps repository has a key of its own that can never sign a system
  package, so the busier apps pipeline cannot touch the kernel.
- An app's updates must come from the same developer, and a new version
  asking for more permissions waits for the user.
- Outside developers submit **source**, which is checked, reviewed and
  built in public, released with build provenance, and checked again
  before rustux.com signs it into FoxDen.
- Apps from outside FoxDen still install, with a warning, and can never
  replace an app installed from FoxDen.
- That is the good part of the Play Store and Flathub together, without
  the habit of running any installer as administrator.

## Small enough to understand

- One project, in readable size, covers everything from the boot manager
  (`rboot`) to the desktop and the app store. Windows is closed and
  enormous; Linux has over 30 million lines.
- It is documented in this folder, step by step, with a test for each
  part (docs/TESTS.md).

## Compatible where it matters

- **Disks:** ext4 and FAT32, readable and checkable with Linux's tools,
  and Linux can read Rustica's disks.
- **Dual boot:** the installer goes alongside Windows without touching
  it, and `rboot` starts Windows and Linux too (docs/INSTALL.md,
  docs/BOOT.md).
- **Programs:** Rust programs build with the standard library against
  Rustica's C library (docs/RUST.md). The familiar commands come from
  uutils (docs/COMMANDS.md), and network, SSH and HTTPS work as usual.
- **A live system:** the image runs entirely from memory without touching
  the disks, and is also the installer and a rescue system.

## Ready for more than one architecture

- One source tree, one system repository and one app store for every
  architecture (docs/PORTING.md).
- The CPU-specific code is gathered in one place, and a check keeps it
  there.
- aarch64 and riscv64 are the next targets, and the mobile edition builds
  on the aarch64 port.

## Where this design fits best

- Servers and appliances, where isolation and updates that cannot break
  matter most.
- Kiosks, embedded and mobile devices, where the hardware is known.
- Teaching and research, as a modern operating system one can read end
  to end.

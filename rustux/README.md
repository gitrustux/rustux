# Rustux Kernel

**Refactored UEFI x86_64 Kernel with Cross-Architecture Support**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Overview

Rustux is a refactored UEFI x86_64 kernel written in Rust, featuring:
- Cross-architecture interrupt controller support
- ACPI-based hardware discovery
- Zircon-inspired capability-based security model
- Multi-architecture support (x86_64, ARM64, RISC-V)

## Quick Start

```bash
# Build the kernel
cargo build

# Run QEMU test
./test-qemu.sh

# Create bootable image
./build.sh
```

## Documentation

See [docs/README.md](docs/README.md) for complete documentation including:
- Architecture overview
- Build instructions
- Testing procedures
- Migration progress

## Repository

- **Main Repo:** https://github.com/gitrustux/rustux
- **Working Directory:** `/var/www/rustux.com/prod/rustux/`

## License

MIT License - see [LICENSE](LICENSE) for details.

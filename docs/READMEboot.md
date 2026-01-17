# Rustica Apps

Official application suite for the Rustux operating system.

## Overview

This repository contains all user-space applications for Rustux, organized into:
- **CLI utilities**: Essential command-line tools
- **GUI applications**: Aurora desktop environment components
- **Shared libraries**: Common utilities for app development

## Directory Structure

See [STRUCTURE.md](STRUCTURE.md) for the complete directory layout.

## Building

```bash
# Build all apps
./scripts/build-all.sh

# Build specific app
cargo build -p redit

# Build with release optimization
cargo build --release -p redit
```

## Repository Structure

- `cli/` - Command-line utilities
- `gui/` - Desktop applications (Aurora)
- `libs/` - Shared Rust libraries
- `examples/` - Example applications
- `tests/` - Integration tests
- `scripts/` - Build and deployment scripts

## License

MIT - See LICENSE file for details.

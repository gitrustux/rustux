# Crates changed for Rustux

Copies of crates.io crates with small changes, used by path (or through
`[patch.crates-io]`) in the packages that need them.

| Crate | Version | Change |
|-------|---------|--------|
| `platform-info` | 2.2.1 (MIT) | The operating system name (`uname -o`) is `Rustux` instead of `Linux` |
| `kibi` | 0.3.3 (MIT or Apache-2.0) | Only the library (the `edit` command in `rust/tools` runs it); minimum Rust version lowered to 1.91 (the toolchain used) |
| `findutils` | 0.11.0 (MIT) | `xargs` looks programs up in `$PATH` itself (Rust's std can only start a program with a changed environment through `posix_spawn` when its path is known; Rustux has no `fork`); tests, benchmarks and binaries removed |
| `diffutils` | 0.6.0 (MIT or Apache-2.0) | `diff`'s entry point exported from the library (`rust/tools` provides the `diff` and `cmp` commands); the tests and the binary removed |
| `uu_tail` | 0.12.0 (MIT) | `tail -f` polls (once a second) when inotify can't be used, instead of failing; tests and the binary removed |

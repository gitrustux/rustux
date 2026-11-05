# Rust Programs on Rustux

How programs written in Rust, with the standard library, run on Rustux
(checklist step 10), the options that were weighed, and the limits.

## The choice

| Option | What it takes | Verdict |
|--------|---------------|---------|
| (a) Rust's `x86_64-unknown-fuchsia` target, with Fuchsia-compatible `libc`, `libzircon` and `fdio` over Rustux's calls (the checklist's first idea) | Fuchsia's std links `libzircon.so` and `libfdio.so` dynamically and calls Zircon through its vDSO; Rustux's calls differ in numbers and details and there is no dynamic linking. Every `zx_*` and `fdio_*` function std uses would need a faithful stand-in | Not workable now |
| (b) A Rustux target (`x86_64-unknown-rustux`) with std's Unix layer ported to it, as Redox has | A custom target JSON, a nightly toolchain, and a fork of std (or upstream work) that must follow every Rust release | The long-term answer once the system calls settle; too much to carry now |
| (c) Build for `x86_64-unknown-linux-musl` and link against the Rustux C library, which provides the part of musl's ABI that std calls | No changes to Rust or std; the C library grows (it needed most of this anyway) | **Chosen** |
| (d) `no_std` programs with a Rustux runtime crate | Simple, but no `std`: no threads, `Command` or files through std, and most crates won't build | Not enough for uutils (step 11) |

Option (c) works because Rust's std on Linux is a thin layer over libc:
it calls `open`, `read`, `pthread_create`, `posix_spawn` and so on, plus
`syscall(SYS_futex)` for its locks. Programs are built for musl but never
see musl: the linker wrapper puts the Rustux C library and start file in
musl's place. If std starts to need a function the library lacks, linking
fails and names it, so nothing breaks silently.

## Building

```sh
userspace/build.sh          # the C library (once)
userspace/build-rust.sh     # every package in userspace/rust/ -> userspace/bin/
```

`build-rust.sh` needs a Rust toolchain with the `rust-src` component, gcc
and ld. It rebuilds std from source for the musl target
(`-Zbuild-std=std,panic_abort`, allowed on a stable toolchain through
`RUSTC_BOOTSTRAP=1`), so the musl target itself need not be installed. It
passes:

| Flag | Why |
|------|-----|
| `-C panic=abort` | No unwinding tables are loaded; a panic prints its message and ends the program (exit code -6, as `abort()` does) |
| `-C relocation-model=static`, `-C target-feature=+crt-static` | Programs are static and not position-independent, loaded at 1 MiB |
| `-C linker=userspace/rust/rxld.sh`, `-C linker-flavor=ld`, `-C link-self-contained=no` | The wrapper drops musl's `crt1.o`/`crti.o`/`crtbegin.o`/... and `-lc`, then links with `rustux.ld`, the library's `crt0.o`, `libc.a` and gcc's `libgcc` |

A new program is a new Cargo package under `userspace/rust/` (its binary
named like the package); the built binary is committed in `userspace/bin/`
and goes into the boot image like the C programs. Rebuild the kernel
afterwards.

`userspace/rust/hello-std` exercises std: printing, arguments and the
environment, files and directories, metadata, `/dev/null`, threads with a
mutex, a channel and thread-locals, sleeping and the clocks, child
processes (`Command::output`, `status`, piped stdin/stdout, `current_dir`,
`env_clear`, a program found on `$PATH`) and a `HashMap` (random keys). It
prints `hello-std: 28 passed, 0 failed` on QEMU.

## What the C library does for std

| std uses | The library |
|----------|-------------|
| `#[thread_local]` statics (local-exec TLS through `%fs`) | Copies the `PT_TLS` template below each thread's control block and points the FS base there (`object_set_property(REGISTER_FS)`); the linker script loads the ELF headers so the library can find `PT_TLS` |
| `syscall(SYS_futex, ...)` for `Mutex`, `Condvar`, `Once`, thread parking | Futexes emulated in the library: per-address queues of waiting threads, each with its own event (`FUTEX_WAIT`, `WAIT_BITSET` with an absolute deadline on either clock, `WAKE`) |
| `pthread_create`, `join`, `detach`, keys, `pthread_getattr_np`, `setname_np` | `thread.c` |
| `struct stat`, `struct dirent`, `struct termios`, `sigset_t`, `posix_spawn_file_actions_t`, `O_*` and `errno` values | musl's x86-64 layouts and numbers |
| `posix_spawn(p)` with file actions (`adddup2`, `addclose`, `addchdir_np`), `waitpid`, `kill`, `pipe2`, `poll` | `pspawn.c` and `posix.c` on top of `rx_spawn` and sockets |
| `getrandom` (through `syscall`) for `HashMap` keys | RDRAND where the CPU has it, mixed with the clocks |
| `mmap`/`mprotect`/`sigaltstack`/`sigaction` for the stack-overflow handler | Succeed; no guard pages, no signals delivered |
| `dl_iterate_phdr`, `_dl_find_object` (unwinder) | The program's own headers; not found |
| `.init_array` constructors | Run before `main` with `argc, argv, envp` |

## What does not work yet

- **Networking** (`std::net`): there are no sockets; calls fail.
- **Signals**: nothing is delivered. `Child::kill` works (it ends the
  process); a stack overflow is a page fault that ends the program
  (`[crashed (CPU exception)]`), not std's "stack overflow" message.
- **`fork`/`exec`**: `Command` must be able to use `posix_spawn`, so
  `uid`/`gid`/`pre_exec`/`process_group` (Unix `CommandExt`) fail.
- **Symbolic links and permissions**: `read_link` fails with `EINVAL`,
  `set_permissions` changes nothing (no users until step 17).
- **`File` locking, memory-mapped files, `fs::copy` fast paths** (it falls
  back to read/write).
- **Unwinding**: `panic=abort` only; `catch_unwind` cannot catch.
- Rust's std is built against musl's ABI as Rust's `libc` crate describes
  it; if a future Rust release changes which functions std calls, the
  library has to follow (the link step reports what is missing).

# Rustux Userspace: C Library, Console and Programs

How programs start, print, read input, use files, run threads and start
other programs (checklist steps 7, 8 and 10). System calls are in
`SYSCALL.md`, the filesystem service and its protocol in `FS.md`, Rust
programs in `RUST.md`.

## Layout

| Path | Contents |
|------|----------|
| `userspace/libc/include/` | Headers: `stdio.h`, `stdlib.h`, `string.h`, `ctype.h`, `errno.h`, `unistd.h`, `fcntl.h`, `dirent.h`, `limits.h`, `assert.h`, `time.h`, `pthread.h`, `sched.h`, `signal.h`, `spawn.h`, `termios.h`, `poll.h`, `getopt.h`, `dlfcn.h`, `sys/` (`types`, `stat`, `time`, `utsname`, `wait`, `mman`, `ioctl`, `uio`, `random`, `syscall`), and `rustux/` (system calls, processargs, console and file protocols, spawning) |
| `userspace/libc/src/` | The library: startup, file descriptors, files and directories (`fs.c`), the ELF loader (`spawn.c`), threads and TLS (`thread.c`), `posix_spawn`/`waitpid` (`pspawn.c`), the rest of POSIX (`posix.c`), stdio, `printf`, `malloc`, environment, strings, numbers, time |
| `userspace/libc/rustux.ld` | Linker script for programs (static, loaded at 1 MiB with their ELF headers) |
| `userspace/c-progs/` | One program per `.c` file: `init`, `shell`, commands, tests; `keymap.h` (layouts, for `init`) and `lineedit.h` (the shell's line editor) |
| `userspace/rust/` | Rust programs, one Cargo package each (`RUST.md`) |
| `userspace/bin/` | The built programs (committed); `build.rs` puts each in the boot image, served as `/bin/<name>` |
| `userspace/build.sh` | Builds the library and every C program into `bin/` |
| `userspace/build-rust.sh` | Builds the Rust programs into `bin/` |

Build after changing C code: `userspace/build.sh` (needs any x86-64 `gcc`
and `ar`), then rebuild the kernel. A new program is just a new `.c` file
in `c-progs/` with a `main`.

## Program start

The kernel starts a program at `_start` (`libc/src/crt0.c`) with the
bootstrap channel in RDI. The library reads the one message on it
(processargs format, see `SYSCALL.md`), then:

- keeps the standard handles in `rx_proc_self`, `rx_thread_self`,
  `rx_job_default` and `rx_vmar_root` (`<rustux/process.h>`);
- sets up the main thread's thread-local storage (below);
- puts `PA_FD` handles behind their file descriptors and `PA_NS_DIR` as
  the namespace root; takes the current directory from `$PWD`;
- keeps every other handle for `rx_take_startup_handle(info)`;
- builds `argv` and `environ`, sets up stdio, runs the constructors
  (`.init_array`, with `argc, argv, envp`), calls `main(argc, argv, envp)`
  and passes its return value to `exit()`.

## File descriptors

Each fd is a handle plus a kind (`libc/src/fd.c`):

| Kind | Handle | `read` | `write` |
|------|--------|--------|---------|
| console | channel to the console service | one typed line at a time (raw mode: the keys as typed); 0 at end of input | `WRITE` messages of up to 4 KiB |
| log | debug log (`init` only) | not supported | `debuglog_write` |
| file | connection to `fsd` (file or directory) | `READ` requests | `WRITE` requests |
| pipe | socket | waits for bytes; 0 when the writer is gone | waits for room; `EPIPE` when the reader is gone |

`read`, `write`, `close`, `lseek`, `dup`, `dup2`, `pipe`, `pipe2`,
`fstat`, `isatty`, `fcntl` (`F_GETFD`/`SETFD`/`GETFL`/`SETFL`/`DUPFD`),
`ioctl` (terminal modes and size, `FIOCLEX`), `poll` (consoles and pipes
wait; files are always ready), `readv`/`writev`, `pread`/`pwrite` and
`fsync` work on fds. Path calls: `open`, `creat`, `stat`, `access`,
`mkdir`, `unlink`, `rmdir`, `rename`, `remove`, `truncate`, `chdir`,
`fchdir`, `getcwd`, `realpath`, the `*at` calls (`openat`, `fstatat`,
`unlinkat`, `mkdirat`, `renameat`; a file fd remembers its path),
`opendir`/`fdopendir`/`readdir`/`closedir`; stdio has `fopen`, `fdopen`,
`fclose`, `fseek`, `ftell`. `struct stat` and `struct dirent` have musl's
layout (Rust's std reads them). The current directory is a path kept by the
library (and in `$PWD`, so children start there); relative paths, `.` and
`..` are resolved against it before a request goes to `fsd`.

There are no signals: a program writing to a pipe whose reader has ended
gets `EPIPE` from `write`.

## Console

`init` is the console service. Programs' fds 0-2 are copies of one channel
handle; `init` holds the other end.

- **Output:** each `write` sends `[u32 op = 1][bytes]`; `init` prints it on
  the screen.
- **Input, cooked mode** (the default): `init` edits the typed line:
  printable characters are echoed, Backspace removes one character, Ctrl+W
  a word, Ctrl+U the whole line. Enter sends `[u32 op = 2][line + '\n']`
  to whoever reads the console. Ctrl+D sends the line without a newline,
  or an empty message (end of input) on an empty line.
- **Input, raw mode:** each key's bytes as it is typed: characters in
  UTF-8, xterm escape sequences for arrows and function keys
  (`SYSCALL.md`, "Console and keyboard input"). A program switches with
  `tcsetattr()` (`<termios.h>`: `ICANON`, `ECHO`, `ISIG`, `ICRNL`; `VMIN`
  0 with `VTIME` makes `read` wait at most VTIME tenths of a second) or
  `cfmakeraw()`; `tcsetattr` sends `[u32 op = 3][u32 lflag][u32 iflag]`.
  The mode belongs to the console, not the program: the shell sets cooked
  mode before it starts one. `ioctl(fd, TIOCGWINSZ)` gives the screen
  size.
- **Keyboard layouts:** `/etc/keymap` names the layout (`us` built in;
  `de` and `uk` in `/usr/share/keymaps`, a text file per layout, format in
  `c-progs/keymap.h`). The shell's `loadkeys NAME` changes it.
- **Screen:** a VT100/xterm terminal (colours, cursor movement, the
  alternate screen, UTF-8 with box drawing and accented letters);
  Shift+PgUp/PgDn scroll back 500 lines.
- **Ctrl+C:** `init` drops the typed line and raises
  `RX_CONSOLE_SIGNAL_INTERRUPT` (`USER_SIGNAL_0`) on the programs' end. The
  shell, waiting for its program, kills it, clears the signal and throws
  away unread input. Programs don't see the signal themselves (no Unix
  signals).


## Starting programs

```c
#include <rustux/process.h>

char *argv[] = { "echo", "hello", 0 };
rx_handle_t proc;
if (rx_spawn("/bin/echo", argv, NULL /* our environment */, RX_SPAWN_CLONE_ALL,
             NULL, 0, &proc) == RX_OK) {
    int64_t code = rx_wait_process(proc);
}
```

`RX_SPAWN_CLONE_STDIO` gives the child fds 0-2 (a file fd becomes a new
connection to the same open file, sharing its offset);
`RX_SPAWN_CLONE_NS` gives it a connection to our namespace root;
`RX_SPAWN_CLONE_ALL` is both. Extra handles are passed as
`rx_spawn_handle_t { info, handle }` (moved into the child).
`rx_spawn_etc` starts the child in a given job (the shell puts each
pipeline in its own job, so Ctrl+C can end all of it), `rx_spawn_image`
from an ELF image in memory (init starts `fsd` this way).

POSIX programs use `posix_spawn`/`posix_spawnp` (`<spawn.h>`; file
actions `adddup2`, `addclose`, `addopen` and `addchdir_np` shape the
child's fds 0-2 and its directory; other fds are not passed on, as if all
were close-on-exec), `waitpid`/`wait` (`WNOHANG`; a killed child reads as
`SIGKILL`, a crashed one as `SIGSEGV`) and `kill` (any signal but 0 ends
the process). A pid is the process's koid. There is no `fork` or `exec`
(they fail with `ENOSYS`).

## Threads

`<pthread.h>`: `pthread_create`/`join`/`detach`/`exit`/`self`, attributes
(stack size, detached), keys with destructors, `pthread_once`, mutexes
(normal, recursive, error-checking), condition variables (with
`timedwait` on either clock), `sched_yield`. Each thread gets one mapping
for its stack (256 KiB unless set), its copy of the program's
thread-local storage (`__thread`, Rust's `thread_local!`) and its control
block; the FS base points at the block (`object_set_property`,
`SYSCALL.md`). `errno` is per thread.

The kernel has no futex call: futexes are emulated in the library (a
queue of waiting threads per address, each waiting on an event of its
own), and `syscall(SYS_futex, ...)` answers the `FUTEX_WAIT`, `WAIT_BITSET`
and `WAKE` calls Rust's std makes (`syscall` also does `SYS_getrandom` and
`SYS_gettid`; anything else is `ENOSYS`). `malloc` and the filesystem calls
are thread-safe; stdio's `FILE`s are not locked yet.

## Other POSIX calls

`mmap` (anonymous memory only), `munmap`, `mprotect`; `sysconf`,
`getauxval` (page size), `getrandom`/`getentropy` (RDRAND mixed with the
clocks), `getopt`/`getopt_long`, `strtod`, `posix_memalign`, `strerror_r`.
Signals are recorded but never delivered (`sigaction`, `signal`, signal
sets and masks succeed; `raise` ends the program unless the signal is
ignored or caught). User ids are all 0 (root) until step 17.
`dl_iterate_phdr` reports the program's own headers; `dlopen`/`dlsym`
find nothing (no dynamic linking).

The loader (`libc/src/spawn.c`) reads the file, checks it is a static
x86-64 ELF, copies its segments into a VMO, maps each page with its
segment's permissions, maps a 256 KiB stack at the top of the address
space and starts the first thread with the processargs message.

## The shell

`userspace/c-progs/shell.c`. Line editing (`lineedit.h`): arrows,
Home/End, Ctrl+A/E/B/F, Ctrl+Left/Right and Alt+B/F by word, Backspace,
Delete, Ctrl+D, Ctrl+W, Ctrl+U, Ctrl+K, Ctrl+L, history with Up/Down
(Ctrl+P/N; the last 200 lines, not kept across reboots), Tab completion of
commands (first word: built-ins and `$PATH`) and paths (a second Tab lists
the choices). The console is in raw mode while a line is typed and in
cooked mode while programs run. Quoting (`'...'`, `"..."`, `\`), variables
(`$NAME`, `${NAME}`, `$?`), `NAME=value` (shell variable), `export`,
`unset`, `set`, `NAME=value command` (for one command), pipelines
(`a | b | c`), redirection (`<`, `>`, `>>`, `2>`, `2>>`, `2>&1`), lists
(`;`, `&&`, `||`; each part expanded just before it runs), `cd` (`cd`,
`cd -`), `pwd`, `$PATH` lookup. The prompt shows the current directory.
Exit codes below 0 (killed -1024, crashed -1025) are printed; Ctrl+C ends
every program of the pipeline in the foreground and sets `$?` to 130.
Built-ins can be redirected (`pwd > file`) but not used inside a pipeline
yet.

## Commands and tests in `/bin`

Small C stand-ins until uutils (step 11):

| Program | What it does |
|---------|--------------|
| `ls [-l] [-a] [PATH]...` | list directories |
| `cat [FILE]...` | copy files (or standard input) to standard output |
| `mkdir [-p] DIR...`, `rm [-r] [-f] PATH...`, `mv SRC... DEST` | make, remove, move |
| `echo [-n] [-e] ...` | print the arguments |
| `upper` | standard input to upper case (a pipe test filter) |
| `env [-i] [NAME=VALUE]... [COMMAND]` | print the environment or run a command with a changed one |
| `true`, `false` | exit 0 / 1 |
| `hello`, `counter [N]`, `args ...` | small examples (`args` shows how the shell split a line) |
| `ps`, `kill PID...` | list processes (PID, job, threads, memory, CPU time, name); end one |
| `loadkeys [LAYOUT]` (built in) | keyboard layout: `us`, `de`, `uk` |
| `uptime`, `free [-h]`, `date [-u] [+FORMAT]`, `uname [-a]`, `hostname [NAME]` | system information |
| `dmesg` | the kernel log |
| `reboot`, `poweroff` | restart or turn off (need the power resource the shell passes on) |
| `systest` | 39 checks of process and system information, clocks and time zones |
| `threadtest` | 15 checks of threads, TLS, mutexes, condition variables, keys |
| `termtest` | a full-screen program: draws with escape sequences, shows each key's bytes in raw mode (q leaves) |
| `hello-std` | Rust with std: 28 checks of files, threads, child processes, time (`RUST.md`) |
| `fstest` | 64 checks of files, directories, two processes on one file, pipes (with a slow reader) |
| `libctest` | 50 checks of the C library |
| `spin` | loops forever (stop it with Ctrl+C) |
| `objtest`, `ipctest`, `spawntest`, `drvtest`, `smptest` | kernel tests (`SYSCALL.md`) |
| `init`, `fsd`, `shell`, `ps2kbd` | the system itself |

## Time

`time`, `clock_gettime` (`CLOCK_REALTIME`, `CLOCK_MONOTONIC`),
`gettimeofday`, `nanosleep`, `gmtime`/`localtime` (and `_r`), `mktime`,
`timegm`, `strftime`, `ctime`/`asctime`, `clock` (CPU time).

The wall clock comes from the hardware real-time clock at boot. The time
zone is `$TZ`, else `/etc/timezone`: either a POSIX rule
(`CST6CDT,M3.2.0,M11.1.0`) or a zone name looked up in
`/usr/share/zoneinfo/zones` (about 70 common zones with today's rules; no
history of past changes). Both files come from `files/` in the kernel
repository (served under `/boot` and copied to `/etc` at start, so they can
be changed until the next boot).

**A computer that also runs Windows** keeps local time in its hardware
clock. Put `LOCAL` on the third line of `files/etc/adjtime` and the zone
in `files/etc/timezone` (for example `America/Chicago`), rebuild the
image, and `init` corrects the clock at start.

Other system calls in the library: `uname` (kernel version, host name from
`/etc/hostname`), `gethostname`, `getpid` (the process koid).

## Limits for now

- `printf` formats floating point with long double arithmetic: exact to
  about 18 significant digits. `strtod` likewise.
- stdio `FILE`s are not locked: two threads writing to one `FILE` can mix
  their output.
- Every character on the screen is one column wide (no double-width
  characters); the line editor assumes the same.
- Up to 64 file descriptors; arguments and environment together up to
  32 KiB; programs up to 64 MiB.
- No symbolic links, no file locking, no `fork`/`exec` (programs are
  started with `rx_spawn` or `posix_spawn`), no Unix signals, no sockets,
  no file mappings (`mmap` of anonymous memory only), no guard pages below
  thread stacks.
- `stdout` to a file or pipe is fully buffered (flushed at exit or when the
  buffer fills), as on Unix.

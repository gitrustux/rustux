# Rustux Userspace: C Library, Console and Programs

How programs start, print, read input, use files and start other programs
(checklist steps 7 and 8). System calls are in `SYSCALL.md`, the
filesystem service and its protocol in `FS.md`.

## Layout

| Path | Contents |
|------|----------|
| `userspace/libc/include/` | Headers: `stdio.h`, `stdlib.h`, `string.h`, `ctype.h`, `errno.h`, `unistd.h`, `fcntl.h`, `dirent.h`, `limits.h`, `assert.h`, `sys/types.h`, `sys/stat.h`, and `rustux/` (system calls, processargs, console and file protocols, spawning) |
| `userspace/libc/src/` | The library: startup, file descriptors, files and directories (`fs.c`), the ELF loader (`spawn.c`), stdio, `printf`, `malloc`, environment, strings, numbers |
| `userspace/libc/rustux.ld` | Linker script for programs (static, loaded at 1 MiB) |
| `userspace/c-progs/` | One program per `.c` file: `init`, `shell`, commands, tests |
| `userspace/bin/` | The built programs (committed); `build.rs` puts each in the boot image, served as `/bin/<name>` |
| `userspace/build.sh` | Builds the library and every program into `bin/` |

Build after changing C code: `userspace/build.sh` (needs any x86-64 `gcc`
and `ar`), then rebuild the kernel. A new program is just a new `.c` file
in `c-progs/` with a `main`.

## Program start

The kernel starts a program at `_start` (`libc/src/crt0.c`) with the
bootstrap channel in RDI. The library reads the one message on it
(processargs format, see `SYSCALL.md`), then:

- keeps the standard handles in `rx_proc_self`, `rx_thread_self`,
  `rx_job_default` and `rx_vmar_root` (`<rustux/process.h>`);
- puts `PA_FD` handles behind their file descriptors and `PA_NS_DIR` as
  the namespace root; takes the current directory from `$PWD`;
- keeps every other handle for `rx_take_startup_handle(info)`;
- builds `argv` and `environ`, sets up stdio, calls
  `main(argc, argv, envp)` and passes its return value to `exit()`.

## File descriptors

Each fd is a handle plus a kind (`libc/src/fd.c`):

| Kind | Handle | `read` | `write` |
|------|--------|--------|---------|
| console | channel to the console service | one typed line at a time; 0 at end of input | `WRITE` messages of up to 4 KiB |
| log | debug log (`init` only) | not supported | `debuglog_write` |
| file | connection to `fsd` (file or directory) | `READ` requests | `WRITE` requests |
| pipe | socket | waits for bytes; 0 when the writer is gone | waits for room; `EPIPE` when the reader is gone |

`read`, `write`, `close`, `lseek`, `dup`, `dup2`, `pipe`, `fstat` and
`isatty` work on fds. Path calls: `open`, `creat`, `stat`, `access`,
`mkdir`, `unlink`, `rmdir`, `rename`, `remove`, `truncate`, `chdir`,
`getcwd`, `opendir`/`readdir`/`closedir`; stdio has `fopen`, `fdopen`,
`fclose`, `fseek`, `ftell`. The current directory is a path kept by the
library (and in `$PWD`, so children start there); relative paths, `.` and
`..` are resolved against it before a request goes to `fsd`.

There are no signals: a program writing to a pipe whose reader has ended
gets `EPIPE` from `write`.

## Console

`init` is the console service. Programs' fds 0-2 are copies of one channel
handle; `init` holds the other end.

- **Output:** each `write` sends `[u32 op = 1][bytes]`; `init` prints it on
  the screen.
- **Input:** `init` edits the typed line: printable characters are echoed,
  Backspace removes one character, Ctrl+U the whole line. Enter sends
  `[u32 op = 2][line + '\n']` to whoever reads the console. Ctrl+D sends
  the line without a newline, or an empty message (end of input) on an
  empty line.
- **Ctrl+C:** `init` drops the typed line and raises
  `RX_CONSOLE_SIGNAL_INTERRUPT` (`USER_SIGNAL_0`) on the programs' end. The
  shell, waiting for its program, kills it, clears the signal and throws
  away unread input. Programs don't see the signal themselves (no Unix
  signals).

Raw mode (keys one at a time, for editors and line editing in the shell)
and terminal escape sequences come in step 10.

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

The loader (`libc/src/spawn.c`) reads the file, checks it is a static
x86-64 ELF, copies its segments into a VMO, maps each page with its
segment's permissions, maps a 256 KiB stack at the top of the address
space and starts the first thread with the processargs message.

## The shell

`userspace/c-progs/shell.c`. Quoting (`'...'`, `"..."`, `\`), variables
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
| `fstest` | 64 checks of files, directories, two processes on one file, pipes (with a slow reader) |
| `libctest` | 50 checks of the C library |
| `spin` | loops forever (stop it with Ctrl+C) |
| `objtest`, `ipctest`, `spawntest`, `drvtest`, `smptest` | kernel tests (`SYSCALL.md`) |
| `init`, `fsd`, `shell`, `ps2kbd` | the system itself |

## Limits for now

- `printf` has no floating point (`%f` prints `?`; step 10).
- `malloc` is not thread-safe; nothing else in the library is either
  (threads come in step 10).
- Up to 64 file descriptors; arguments and environment together up to
  32 KiB; programs up to 64 MiB.
- No symbolic links, no file locking, no `fork`/`exec` (programs are
  started with `rx_spawn`), no Unix signals.
- `stdout` to a file or pipe is fully buffered (flushed at exit or when the
  buffer fills), as on Unix.

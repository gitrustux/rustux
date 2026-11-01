// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux Shell - Userspace Shell Program
//!
//! Build: x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector shell.c -o shell.elf

// Syscall numbers (matching rustux kernel)
#include "../../userspace/c-progs/rx.h"

#define SYS_WRITE        0x60  // write(fd, buf, len)
#define SYS_READ         0x61  // read(fd, buf, len)
#define SYS_OPEN         0x62  // open(path, flags)
#define SYS_CLOSE        0x63  // close(fd)
#define SYS_LSEEK        0x64  // lseek(fd, offset, whence)
#define SYS_GETPID       0x70  // getpid()
#define SYS_GETPPID      0x71  // getppid()
#define SYS_YIELD        0x72  // yield()
#define SYS_EXIT         0x06  // exit(code)
#define SYS_SPAWN        0x03  // spawn(path)
#define SYS_SPAWN_PROCESS 0x0B // spawn_process(path, *process_handle)
#define SYS_WAIT_ONE     0x26  // object_wait_one(handle, signals, deadline, *observed)
#define SYS_HANDLE_CLOSE 0x07  // handle_close(handle)
#define SYS_GET_INFO     0x0A  // object_get_info(handle, topic, *buf, size, *actual, *avail)

#define PROCESS_TERMINATED (1u << 3)
#define INFO_PROCESS       3

// File descriptor numbers
#define STDIN   0
#define STDOUT  1
#define STDERR  2

// Buffer sizes
#define INPUT_BUFFER_SIZE  512
#define MAX_ARGS           16

// ANSI Color Codes
#define ANSI_RESET         "\033[0m"
#define ANSI_RED           "\033[31m"
#define ANSI_GREEN         "\033[32m"
#define ANSI_YELLOW        "\033[33m"
#define ANSI_BLUE          "\033[34m"
#define ANSI_MAGENTA       "\033[35m"
#define ANSI_CYAN          "\033[36m"
#define ANSI_WHITE         "\033[37m"

// =============================================================
// SYSCALL INTERFACE
// =============================================================

static inline long syscall1(long number, long arg1) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (arg1)
        : "memory"
    );
    return ret;
}

static inline long syscall3(long number, long arg1, long arg2, long arg3) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (arg1), "S" (arg2), "d" (arg3)
        : "memory"
    );
    return ret;
}

// Syscall wrappers
static inline long sys_write(int fd, const char *buf, long len) {
    return syscall3(SYS_WRITE, fd, (long)buf, len);
}

static inline long sys_read(int fd, char *buf, long len) {
    return syscall3(SYS_READ, fd, (long)buf, len);
}

static inline long syscall6(long number, long a1, long a2, long a3, long a4, long a5, long a6) {
    long ret;
    register long r10 __asm__("r10") = a4;
    register long r8 __asm__("r8") = a5;
    register long r9 __asm__("r9") = a6;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (a1), "S" (a2), "d" (a3), "r" (r10), "r" (r8), "r" (r9)
        : "memory"
    );
    return ret;
}

// Start /bin/<name>; *proc receives a process handle to wait on
static inline long sys_spawn_process(const char *path, unsigned int *proc) {
    return syscall6(SYS_SPAWN_PROCESS, (long)path, (long)proc, 0, 0, 0, 0);
}

// Block until the process has exited; returns its exit code
static long wait_process(unsigned int proc) {
    struct { long return_code; long start_time; unsigned int flags, pad; } info = {0, 0, 0, 0};
    syscall6(SYS_WAIT_ONE, proc, PROCESS_TERMINATED, -1L, 0, 0, 0);
    syscall6(SYS_GET_INFO, proc, INFO_PROCESS, (long)&info, sizeof info, 0, 0);
    syscall1(SYS_HANDLE_CLOSE, proc);
    return info.return_code;
}

static inline void sys_exit(int code) {
    syscall1(SYS_EXIT, code);
    __builtin_unreachable();
}

// =============================================================
// UTILITY FUNCTIONS
// =============================================================

static inline void print(const char *str) {
    long len = 0;
    const char *p = str;
    while (*p) {
        len++;
        p++;
    }
    sys_write(STDOUT, str, len);
}

static inline void print_color(const char *color, const char *str) {
    print(color);
    print(str);
    print(ANSI_RESET);
}

static inline long strlen(const char *str) {
    long len = 0;
    while (str[len]) len++;
    return len;
}

static inline int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return *(unsigned char*)a - *(unsigned char*)b;
}

static inline int strncmp(const char *a, const char *b, long n) {
    while (n > 0 && *a && *a == *b) {
        a++;
        b++;
        n--;
    }
    if (n == 0) return 0;
    return *(unsigned char*)a - *(unsigned char*)b;
}

// =============================================================
// BUILT-IN COMMANDS
// =============================================================

static void cmd_help(void) {
    print("\n");
    print_color(ANSI_CYAN, "Available Commands:\n\n");
    print("  Built-in Commands:\n");
    print("    help     - Show this help message\n");
    print("    clear    - Clear the screen\n");
    print("    echo     - Print arguments\n");
    print("    ps       - List running processes\n");
    print("    restart-kbd - Kill the keyboard driver (init starts a new one)\n");
    print("    exit     - Exit the shell\n\n");
    print("  External Programs:\n");
    print("    hello    - Hello world program\n");
    print("    counter  - Counter program\n");
    print("    objtest, ipctest, spawntest, drvtest - kernel tests\n\n");
}

static void cmd_clear(void) {
    print("\033[2J");  // Clear screen
    print("\033[H");   // Move cursor to home
}

static void cmd_echo(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        if (i > 1) print(" ");
        print(argv[i]);
    }
    print("\n");
}

static void cmd_ps(void) {
    print("\n");
    print_color(ANSI_CYAN, "Running Processes:\n\n");
    print("  PID  PPID  NAME\n");
    print("  ---  ----  ----\n");
    print("    1     0  init\n");
    print("    2     1  shell\n\n");
}

static void cmd_exit(int argc, char **argv) {
    int exit_code = 0;
    if (argc > 1) {
        // Simple exit code parsing (0-9 only for simplicity)
        if (argv[1][0] >= '0' && argv[1][0] <= '9') {
            exit_code = argv[1][0] - '0';
        }
    }
    print_color(ANSI_GREEN, "Exiting shell.\n");
    sys_exit(exit_code);
}

// =============================================================
// COMMAND PARSING
// =============================================================

static int parse_command(char *line, int *argc, char **argv) {
    *argc = 0;
    char *p = line;

    // Skip leading whitespace
    while (*p == ' ' || *p == '\t') p++;

    while (*p && *p != '\n') {
        if (*argc >= MAX_ARGS) break;

        // Save argument start
        argv[(*argc)++] = p;

        // Find end of argument
        while (*p && *p != ' ' && *p != '\t' && *p != '\n') p++;

        // Null-terminate and move to next
        if (*p) {
            *p = '\0';
            p++;
        }

        // Skip whitespace before next argument
        while (*p == ' ' || *p == '\t') p++;
    }

    return *argc > 0;
}

// =============================================================
// EXTERNAL COMMAND EXECUTION
// =============================================================

static int spawn_external(const char *name) {
    char path[128];
    char *p = path;
    const char *cmd_name = name;  // `name` is advanced while building the path

    // Build path: /bin/<name>
    const char prefix[] = "/bin/";
    const char *pp = prefix;
    while (*pp) *p++ = *pp++;

    while (*name && p < path + sizeof(path) - 1) {
        *p++ = *name++;
    }
    *p = '\0';

    // Run the program and wait for it to finish
    unsigned int proc = 0;
    long tid = sys_spawn_process(path, &proc);

    if (tid < 0) {
        print_color(ANSI_RED, "error: ");
        print("command not found: ");
        print(cmd_name);
        print("\n");
        return -1;
    }

    long code = wait_process(proc);
    if (code != 0) {
        // Print the exit code (may be negative: killed = -1024, crashed = -1025)
        char buf[24];
        int i = 23;
        int neg = code < 0;
        unsigned long u = neg ? (unsigned long)(-code) : (unsigned long)code;
        buf[i] = '\0';
        do { buf[--i] = '0' + (u % 10); u /= 10; } while (u);
        if (neg) buf[--i] = '-';
        print_color(ANSI_RED, "[exit code ");
        print(&buf[i]);
        print("]\n");
    }
    return 0;
}

// =============================================================
// SHELL MAIN LOOP
// =============================================================

static void show_welcome(void) {
    print("\n");
    print_color(ANSI_MAGENTA, "╔════════════════════════════════════════════════════════════════╗\n");
    print("║                                                                ║\n");
    print("║                    Welcome to Rustux OS                        ║\n");
    print("║                    Dracula Theme Shell                        ║\n");
    print("║                                                                ║\n");
    print("║  Type 'help' for available commands                           ║\n");
    print("║                                                                ║\n");
    print("╚════════════════════════════════════════════════════════════════╝\n");
    print("\n");
}

static void print_prompt(void) {
    print_color(ANSI_MAGENTA, "rustux");
    print(" ");
    print_color(ANSI_CYAN, ">");
    print(" ");
}

// =============================================================
// ENTRY POINT
// =============================================================

// Keyboard input arrives from init over a channel (init forwards the PS/2
// driver's and the kernel's USB keystrokes)
static rx_handle_t input;
static char pending[256];
static uint32_t pending_len, pending_pos;

// Next typed character (blocks); 0 if the channel is gone
static char read_key(void) {
    while (pending_pos >= pending_len) {
        pending_pos = pending_len = 0;
        uint32_t observed = 0;
        rx_object_wait_one(input, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &observed);
        if (rx_channel_read(input, 0, pending, 0, sizeof pending, 0, &pending_len, 0) != RX_OK) {
            pending_len = 0;
            if (observed & RX_PEER_CLOSED) return 0;
        }
    }
    return pending[pending_pos++];
}

void _start(rx_handle_t bootstrap) {
    static char input_buffer[INPUT_BUFFER_SIZE];
    static char *argv[MAX_ARGS];
    int argc;

    rx_startup(bootstrap);
    input = rx_take_startup_handle(RX_PA_USER0);

    // Show welcome below the boot log (the kernel clears the screen when
    // its console starts; the USB/PS/2 messages stay visible)
    show_welcome();

    // Main shell loop
    while (1) {
        print_prompt();

        // Read input line
        long count = 0;
        char *buf = input_buffer;

        while (1) {
            *buf = read_key();
            if (*buf == 0) break;

            if (*buf == '\n') {
                print("\n");
                break;
            } else if (*buf == 0x08) {  // Backspace
                if (count > 0) {
                    count--;
                    buf--;
                    // Erase character on screen
                    print("\010 \010");  // backspace, space, backspace
                }
            } else if (*buf >= 0x20 && *buf <= 0x7E) {  // Printable ASCII
                sys_write(STDOUT, buf, 1);  // echo
                count++;
                buf++;
            }

            if (count >= INPUT_BUFFER_SIZE - 1) break;
        }

        *buf = '\0';

        // Parse command
        if (!parse_command(input_buffer, &argc, argv)) {
            continue;  // Empty line
        }

        // Execute command
        if (argc == 0) continue;

        char *cmd = argv[0];

        // Check built-in commands
        if (strcmp(cmd, "help") == 0) {
            cmd_help();
        } else if (strcmp(cmd, "clear") == 0) {
            cmd_clear();
        } else if (strcmp(cmd, "echo") == 0) {
            cmd_echo(argc, argv);
        } else if (strcmp(cmd, "ps") == 0) {
            cmd_ps();
        } else if (strcmp(cmd, "restart-kbd") == 0) {
            rx_channel_write(input, 0, "restart-kbd", 11, 0, 0);
        } else if (strcmp(cmd, "exit") == 0) {
            cmd_exit(argc, argv);
        } else {
            // Try to spawn external program
            spawn_external(cmd);
        }
    }

    // Should never reach here
    sys_exit(0);
}

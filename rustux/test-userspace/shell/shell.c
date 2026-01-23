// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Rustux Shell - Userspace Shell Program
//!
//! Build: x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector shell.c -o shell.elf

// Syscall numbers (matching rustux kernel)
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
        : "a" (number), "b" (arg1)
        : "memory"
    );
    return ret;
}

static inline long syscall3(long number, long arg1, long arg2, long arg3) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "b" (arg1), "c" (arg2), "d" (arg3)
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

static inline long sys_spawn(const char *path) {
    return syscall1(SYS_SPAWN, (long)path);
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
    print("    exit     - Exit the shell\n\n");
    print("  External Programs:\n");
    print("    hello    - Hello world program\n");
    print("    counter  - Counter program\n\n");
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

    // Build path: /bin/<name>
    const char prefix[] = "/bin/";
    const char *pp = prefix;
    while (*pp) *p++ = *pp++;

    while (*name && p < path + sizeof(path) - 1) {
        *p++ = *name++;
    }
    *p = '\0';

    // Try to spawn the program
    long pid = sys_spawn(path);

    if (pid < 0) {
        print_color(ANSI_RED, "error: ");
        print("command not found: ");
        print(name);
        print("\n");
        return -1;
    }

    // Success
    print_color(ANSI_GREEN, "✓ ");
    print("started process with PID ");

    // Simple decimal conversion for PID
    char pid_buf[16];
    char *ppid = pid_buf;
    long temp = pid;
    int digits = 0;
    if (temp == 0) {
        *ppid++ = '0';
        digits++;
    } else {
        while (temp > 0) {
            ppid[digits++] = '0' + (temp % 10);
            temp /= 10;
        }
    }
    // Reverse the digits
    for (int i = 0; i < digits / 2; i++) {
        char tmp = ppid[i];
        ppid[i] = ppid[digits - 1 - i];
        ppid[digits - 1 - i] = tmp;
    }
    ppid[digits] = '\0';
    print(pid_buf);
    print("\n");

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

void _start(void) {
    static char input_buffer[INPUT_BUFFER_SIZE];
    static char *argv[MAX_ARGS];
    int argc;

    // Clear screen and show welcome
    cmd_clear();
    show_welcome();

    // Main shell loop
    while (1) {
        print_prompt();

        // Read input line
        long count = 0;
        char *buf = input_buffer;

        while (1) {
            long ret = sys_read(STDIN, buf, 1);
            if (ret <= 0) break;

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

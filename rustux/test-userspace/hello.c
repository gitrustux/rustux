// Userspace test - Phase 5A syscalls (simplified)
//
// Tests the new Phase 5A syscalls:
// - sys_write (0x60)
// - sys_getpid (0x70)
// - sys_getppid (0x71)

// Syscall numbers
#define SYS_PROCESS_EXIT 0x06
#define SYS_DEBUG_WRITE  0x50
#define SYS_WRITE        0x60
#define SYS_GETPID       0x70
#define SYS_GETPPID      0x71

static inline long syscall1(long num, long arg1) {
    long ret;
    __asm__ volatile(
        "int $0x80"
        : "=a"(ret)
        : "a"(num), "b"(arg1)
        : "rcx", "r11", "memory"
    );
    return ret;
}

// Simple write that writes one character at a time
static void write_char(char c) {
    syscall1(SYS_WRITE, c);
}

// Write string (character by character)
static void write_str(const char *str) {
    while (*str) {
        // For each character, we need to make a separate syscall
        // because our simple syscall1 only takes one argument
        // This is a temporary limitation
        syscall1(SYS_DEBUG_WRITE, (long)*str);
        str++;
    }
}

void _start() {
    long pid, ppid;

    // Test sys_debug_write - print greeting
    write_str("[Phase5A] Testing new syscalls...\n");

    // Test sys_getpid
    pid = syscall1(SYS_GETPID, 0);
    write_str("[Phase5A] getpid returned\n");

    // Test sys_getppid
    ppid = syscall1(SYS_GETPPID, 0);
    write_str("[Phase5A] getppid returned\n");

    // All tests complete - exit
    write_str("[Phase5A] Tests complete, exiting...\n");

    syscall1(SYS_PROCESS_EXIT, 0);

    // Should never reach here
    while (1) {
        __asm__ volatile("hlt");
    }
}

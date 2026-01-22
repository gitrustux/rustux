// Userspace test program using syscalls
// Build: x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector hello-syscall.c -o hello-syscall.elf

// Syscall numbers
#define SYS_DEBUG_WRITE 0x50
#define SYS_PROCESS_EXIT 0x06

// Inline syscall function
static inline long syscall(long number, long arg1, long arg2) {
    long ret;
    __asm__ volatile (
        "syscall"
        : "=a" (ret)
        : "a" (number), "D" (arg1), "S" (arg2)
        : "rcx", "r11", "memory"
    );
    return ret;
}

// Debug write syscall
static inline long sys_debug_write(const char *str, long len) {
    return syscall(SYS_DEBUG_WRITE, (long)str, len);
}

// Process exit syscall
static inline void sys_exit(long code) {
    syscall(SYS_PROCESS_EXIT, code, 0);
    // Should never return
    __builtin_unreachable();
}

void _start(void) {
    const char *msg = "Hello from userspace using syscalls!\n";
    long len = 0;
    const char *p = msg;
    
    // Calculate string length
    while (*p) {
        len++;
        p++;
    }
    
    // Write to debug console
    sys_debug_write(msg, len);
    
    // Exit
    sys_exit(0);
    
    // Should never reach here
    __builtin_unreachable();
}

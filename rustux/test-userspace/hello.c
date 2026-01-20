// Simple test program for userspace execution
// Writes "Hello from userspace!" to debug console (port 0xE9)
// Then spins forever

static inline void debug_write(char c) {
    __asm__ volatile("outb %0, $0xE9" : : "a"(c));
}

static inline void debug_write_str(const char *s) {
    while (*s) {
        debug_write(*s++);
    }
}

void _start() {
    debug_write_str("Hello from userspace!\n");

    // Spin forever
    while (1) {
        __asm__ volatile("hlt");
    }
}

// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Init Process (PID 1)
//!
//! The first userspace process. It starts the filesystem service (fsd)
//! from the boot image, then the PS/2 keyboard driver and the shell,
//! restarts the driver or the shell if either ends, and is the console
//! service:
//!
//! - Key events (HID usages) arrive from the PS/2 driver and from the
//!   kernel (USB keyboards) over channels. Init turns them into characters
//!   and escape sequences with the keyboard layout (keymap.h, /etc/keymap)
//!   and repeats held USB keys.
//! - In cooked mode (the default) it edits the typed line (echo,
//!   Backspace, Ctrl+U, Ctrl+W) and sends it to whoever reads the console
//!   when Enter is pressed; Ctrl+D sends end of input. In raw mode (set
//!   with tcsetattr) every key goes out as it is pressed.
//! - Ctrl+C (with ISIG) clears the line and raises the interrupt signal,
//!   which the shell uses to end the program in the foreground.
//! - Shift+PgUp / Shift+PgDn scroll the screen back and forth.
//! - Programs print by writing to the console channel; init prints what
//!   they write on the screen through the kernel's debug log.
//!
//! The protocol is in <rustux/console.h>. The shell can ask for a driver
//! restart ("restart-kbd") or a new layout ("reload-keymap") on its
//! control channel.

#include <rustux/console.h>
#include <rustux/process.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "keymap.h"

static rx_handle_t resource;     // root resource, passed to the driver
static rx_handle_t kernel_input; // USB keystrokes from the kernel
static rx_handle_t con_server;   // console: our end
static rx_handle_t con_client;   // console: the programs' end (copied to each)
static rx_handle_t drv_proc, drv_chan; // keyboard driver
static rx_handle_t sh_proc, sh_ctl;    // shell
static rx_handle_t fsd_proc;           // filesystem service
static rx_handle_t dm_proc;            // device manager
static int driver_failures;

// ---- console ----

// Terminal mode, set by programs with tcsetattr (termios bits)
#define LF_ISIG   0000001u
#define LF_ICANON 0000002u
#define LF_ECHO   0000010u
#define IF_ICRNL  0000400u
static uint32_t lflag = LF_ISIG | LF_ICANON | LF_ECHO, iflag = IF_ICRNL;

static char line[RX_CONSOLE_MAX_DATA - 96];
static size_t line_len;
static int scrolled; // the screen shows the scrollback

static void echo(const char *s, size_t n) {
    write(1, s, n);
}

static void send_input(const char *s, size_t n) {
    char msg[4 + RX_CONSOLE_MAX_DATA];
    uint32_t op = RX_CONSOLE_OP_INPUT;
    memcpy(msg, &op, 4);
    memcpy(msg + 4, s, n);
    rx_channel_write(con_server, 0, msg, (uint32_t)(4 + n), 0, 0); // dropped if nobody reads for a long time
}

// Remove the last character (a whole UTF-8 sequence) from the line
static void rubout(void) {
    if (!line_len) return;
    while (line_len > 1 && (line[line_len - 1] & 0xC0) == 0x80) line_len--;
    line_len--;
    if (lflag & LF_ECHO) echo("\b \b", 3);
}

static void interrupt(void) {
    if (lflag & LF_ECHO) echo("^C\n", 3);
    line_len = 0;
    rx_object_signal_peer(con_server, 0, RX_CONSOLE_SIGNAL_INTERRUPT);
}

// Cooked mode: edit the line, send it on Enter
static void cooked(const char *s, int n) {
    if (n > 1 && s[0] == 0x1B) return; // arrows, function keys: no line editing here
    unsigned char c = (unsigned char)s[0];
    if (n > 1 || c >= 0x80) { // a UTF-8 character
        if (line_len + (size_t)n < sizeof line - 1) {
            memcpy(line + line_len, s, (size_t)n);
            line_len += (size_t)n;
            if (lflag & LF_ECHO) echo(s, (size_t)n);
        }
        return;
    }
    switch (c) {
    case '\r':
    case '\n':
        line[line_len++] = '\n';
        if (lflag & LF_ECHO) echo("\n", 1);
        send_input(line, line_len);
        line_len = 0;
        break;
    case 0x08: // Backspace
    case 0x7F:
        rubout();
        break;
    case 0x15: // Ctrl+U: erase the line
        while (line_len) rubout();
        break;
    case 0x17: // Ctrl+W: erase a word
        while (line_len && line[line_len - 1] == ' ') rubout();
        while (line_len && line[line_len - 1] != ' ') rubout();
        break;
    case 0x03: // Ctrl+C
        if (lflag & LF_ISIG) interrupt();
        break;
    case 0x04: // Ctrl+D: end of input on an empty line, else send the line as is
        send_input(line, line_len);
        line_len = 0;
        break;
    default:
        if ((c >= 0x20 || c == '\t') && line_len < sizeof line - 1) {
            line[line_len++] = (char)c;
            if (lflag & LF_ECHO) echo((char *)&c, 1);
        }
        break;
    }
}

// The bytes of one key press, in the current mode
static void key_bytes(char *s, int n) {
    if (!(lflag & LF_ICANON)) {
        if ((lflag & LF_ISIG) && n == 1 && s[0] == 0x03) {
            interrupt();
            return;
        }
        if (n == 1 && s[0] == '\r' && (iflag & IF_ICRNL)) s[0] = '\n';
        if (lflag & LF_ECHO) echo(s, (size_t)n);
        send_input(s, (size_t)n);
        return;
    }
    cooked(s, n);
}

// Held keys repeat: the PS/2 keyboard does it itself; for USB keyboards
// init does, 500 ms after the press, then 30 times a second
static rx_handle_t repeat_timer;
static uint8_t repeat_usage, repeat_mods;
static int repeating;

static void key_event(uint8_t usage, uint8_t mods, uint8_t flags) {
    int pressed = flags & 1;
    if (usage >= 0xE0) return; // modifiers: their state comes with every event
    if (!pressed) {
        if (usage == repeat_usage) repeating = 0;
        return;
    }
    repeating = 0;
    // Shift+PgUp / Shift+PgDn: the console's scrollback, not for programs
    if ((mods & KM_SHIFT) && (usage == 0x4B || usage == 0x4E)) {
        echo(usage == 0x4B ? "\033[1^" : "\033[2^", 4);
        scrolled = 1;
        return;
    }
    if (scrolled) {
        echo("\033[0^", 4);
        scrolled = 0;
    }
    char out[16];
    int n = km_translate(usage, mods, !(iflag & IF_ICRNL), out);
    if (n > 0) key_bytes(out, n);
    if (!(flags & 2) && n > 0) {
        repeating = 1;
        repeat_usage = usage;
        repeat_mods = mods;
        rx_timer_set(repeat_timer, rx_clock_get_monotonic() + 500000000ull, 0);
    }
}

static void repeat_fired(void) {
    char out[16];
    int n = km_translate(repeat_usage, repeat_mods, !(iflag & IF_ICRNL), out);
    if (n > 0) key_bytes(out, n);
    rx_timer_set(repeat_timer, rx_clock_get_monotonic() + 33000000ull, 0);
}

// Key events from the driver or the kernel: 4 bytes each
static void keys_from(rx_handle_t from) {
    unsigned char buf[256];
    uint32_t n = 0;
    while (rx_channel_read(from, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {
        for (uint32_t i = 0; i + 4 <= n; i += 4) key_event(buf[i], buf[i + 1], buf[i + 2]);
    }
}

// Output and mode changes from programs
static void console_output(void) {
    static char msg[4 + RX_CONSOLE_MAX_DATA];
    uint32_t n = 0;
    while (rx_channel_read(con_server, 0, msg, 0, sizeof msg, 0, &n, 0) == RX_OK) {
        uint32_t op = 0;
        if (n >= 4) memcpy(&op, msg, 4);
        if (op == RX_CONSOLE_OP_WRITE && n > 4) {
            write(1, msg + 4, n - 4);
            scrolled = 0;
        } else if (op == RX_CONSOLE_OP_SET_MODE && n >= 12) {
            uint32_t was = lflag;
            memcpy(&lflag, msg + 4, 4);
            memcpy(&iflag, msg + 8, 4);
            // Switching to raw mode hands over what was typed so far
            if ((was & LF_ICANON) && !(lflag & LF_ICANON) && line_len) {
                send_input(line, line_len);
                line_len = 0;
            }
        }
    }
}

// A copy of the programs' end of the console
static rx_handle_t console_copy(void) {
    rx_handle_t h = 0;
    rx_handle_duplicate(con_client, RX_RIGHT_SAME_RIGHTS, &h);
    return h;
}

// ---- children ----

static uint32_t rd32(const unsigned char *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

// Start fsd from the boot image and make it our namespace root. Everything
// after this loads programs from /bin through it.
static void start_fsd(rx_handle_t bootfs) {
    uint64_t size = 0, addr = 0;
    if (rx_vmo_get_size(bootfs, &size) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ, 0, bootfs, 0, (size + 4095) & ~4095ull, &addr) != RX_OK) {
        printf("init: cannot map the boot image\n");
        return;
    }
    const unsigned char *img = (const unsigned char *)addr;
    const void *elf = 0;
    uint32_t len = 0, count = rd32(img + 4), table = rd32(img + 8);
    for (uint32_t i = 0; i < count; i++) {
        const unsigned char *e = img + table + 16 * i;
        if (!strcmp((const char *)img + rd32(e), "bin/fsd")) {
            elf = img + rd32(e + 4);
            len = rd32(e + 8);
        }
    }
    rx_handle_t client, server;
    rx_channel_create(0, &client, &server);
    rx_spawn_handle_t hs[] = {
        { RX_PA_VMO_BOOTFS, bootfs },
        { RX_PA_USER0, server },
        { RX_PA_HND(RX_PA_FD, 1), console_copy() },
        { RX_PA_HND(RX_PA_FD, 2), console_copy() },
    };
    char *argv[] = { "fsd", 0 };
    if (!elf || rx_spawn_image(rx_job_default, "fsd", elf, len, argv, 0, 0, hs, 4, &fsd_proc) != RX_OK) {
        printf("init: cannot start the filesystem service\n");
        rx_handle_close(client);
        return;
    }
    rx_ns_set_root(client);
}

// The device manager (PCI devices and their drivers) gets the root
// resource; it publishes /svc/devmgr
static void start_devmgr(void) {
    rx_handle_t res;
    if (!resource || rx_handle_duplicate(resource, RX_RIGHT_SAME_RIGHTS, &res) != RX_OK) return;
    rx_spawn_handle_t hs[] = {
        { RX_PA_ROOT_RESOURCE, res },
        { RX_PA_HND(RX_PA_FD, 1), console_copy() },
        { RX_PA_HND(RX_PA_FD, 2), console_copy() },
    };
    char *argv[] = { "devmgr", 0 };
    if (rx_spawn("/bin/devmgr", argv, 0, RX_SPAWN_CLONE_NS, hs, 3, &dm_proc) != RX_OK) {
        printf("init: cannot start /bin/devmgr\n");
        dm_proc = 0;
    }
}

static void start_driver(void) {
    rx_handle_t ours, theirs, res;
    if (!resource || driver_failures >= 5) return;
    rx_channel_create(0, &ours, &theirs);
    rx_handle_duplicate(resource, RX_RIGHT_SAME_RIGHTS, &res);
    rx_spawn_handle_t hs[] = {
        { RX_PA_ROOT_RESOURCE, res },
        { RX_PA_USER0, theirs },
        { RX_PA_HND(RX_PA_FD, 1), console_copy() },
        { RX_PA_HND(RX_PA_FD, 2), console_copy() },
    };
    char *argv[] = { "ps2kbd", 0 };
    if (rx_spawn("/bin/ps2kbd", argv, 0, 0, hs, 4, &drv_proc) != RX_OK) {
        printf("init: cannot start /bin/ps2kbd\n");
        rx_handle_close(ours);
        drv_proc = 0;
        driver_failures = 5;
        return;
    }
    drv_chan = ours;
}

static void start_shell(void) {
    rx_handle_t ours, theirs;
    rx_channel_create(0, &ours, &theirs);
    rx_handle_t power = 0;
    if (resource) rx_resource_create(resource, RX_RSRC_KIND_POWER, &power);
    rx_spawn_handle_t hs[] = {
        { RX_PA_HND(RX_PA_FD, 0), console_copy() },
        { RX_PA_HND(RX_PA_FD, 1), console_copy() },
        { RX_PA_HND(RX_PA_FD, 2), console_copy() },
        { RX_PA_USER0, theirs },
        { RX_PA_POWER_RESOURCE, power },
    };
    char *argv[] = { "shell", 0 };
    char *envp[] = { "PATH=/bin", "HOME=/root", "PWD=/root", "SHELL=/bin/shell", "USER=root", 0 };
    // A fresh shell starts with no leftover interrupt
    rx_object_signal(con_client, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
    if (rx_spawn("/bin/shell", argv, envp, RX_SPAWN_CLONE_NS, hs, power ? 5 : 4, &sh_proc) != RX_OK) {
        printf("init: failed to spawn shell!\n");
        rx_exit(1);
    }
    sh_ctl = ours;
}

// The real-time clock holds local time when /etc/adjtime says LOCAL (as
// Windows keeps it): the kernel read it as UTC, so correct the clock by
// the offset of the zone in /etc/timezone
static void adjust_clock(void) {
    char buf[128] = "";
    int fd = open("/etc/adjtime", O_RDONLY);
    if (fd < 0) return;
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) return;
    buf[n] = 0;
    if (!strstr(buf, "LOCAL")) return;
    time_t wall = (time_t)(rx_clock_get_utc() / 1000000000); // actually local time
    struct tm tm;
    gmtime_r(&wall, &tm);
    tm.tm_isdst = -1;
    time_t utc = mktime(&tm);
    rx_clock_set_utc(resource, (int64_t)utc * 1000000000 + rx_clock_get_utc() % 1000000000);
    printf("init: the hardware clock keeps local time (/etc/adjtime); clock set to %s", ctime(&utc));
}

// A request from the shell
static void command(void) {
    char buf[64];
    uint32_t n = 0;
    while (rx_channel_read(sh_ctl, 0, buf, 0, sizeof buf - 1, 0, &n, 0) == RX_OK) {
        buf[n] = 0;
        if (!strcmp(buf, "reload-keymap")) {
            km_load(); // quietly: the shell has its prompt up already
        } else if (!strcmp(buf, "restart-kbd")) {
            if (drv_proc) {
                printf("init: killing the keyboard driver\n");
                rx_task_kill(drv_proc);
            } else {
                printf("init: no keyboard driver is running\n");
            }
        }
    }
}

static void driver_ended(void) {
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_get_info(drv_proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_handle_close(drv_proc);
    rx_handle_close(drv_chan);
    drv_proc = drv_chan = 0;
    if (info.return_code == 2) {
        printf("init: no PS/2 controller; keyboard driver not needed\n");
        return;
    }
    printf("init: keyboard driver exited (code %lld); starting it again\n", (long long)info.return_code);
    if (info.return_code != RX_TASK_RETCODE_SYSCALL_KILL) driver_failures++;
    start_driver();
}

int main(void) {
    rx_handle_t log;
    resource = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    kernel_input = rx_take_startup_handle(RX_PA_KERNEL_INPUT);
    if (!resource || rx_debuglog_create(resource, 0, &log) != RX_OK) rx_exit(1);
    rx_handle_t log2;
    rx_handle_duplicate(log, RX_RIGHT_SAME_RIGHTS, &log2);
    rx_fd_bind(1, RX_FD_LOG, log);
    rx_fd_bind(2, RX_FD_LOG, log2);
    setvbuf(stdout, 0, _IONBF, 0);
    rx_channel_create(0, &con_server, &con_client);

    printf("Init process (PID 1) starting...\n");
    rx_handle_t bootfs = rx_take_startup_handle(RX_PA_VMO_BOOTFS);
    if (bootfs) start_fsd(bootfs);
    adjust_clock();
    rx_timer_create(0, 0, &repeat_timer);
    km_load();
    start_devmgr();
    start_driver();
    printf("Spawning shell...\n\n");
    start_shell();

    for (;;) {
        rx_wait_item_t items[10];
        int n = 0, i_drv = -1, i_kin = -1, i_dproc = -1, i_fsd = -1;
        int i_rep = -1, i_dm = -1;
        if (repeating) { i_rep = n; items[n++] = (rx_wait_item_t){ repeat_timer, RX_TIMER_SIGNALED, 0 }; }
        int i_out = n; items[n++] = (rx_wait_item_t){ con_server, RX_CHANNEL_READABLE, 0 };
        if (drv_chan) { i_drv = n; items[n++] = (rx_wait_item_t){ drv_chan, RX_CHANNEL_READABLE, 0 }; }
        if (kernel_input) { i_kin = n; items[n++] = (rx_wait_item_t){ kernel_input, RX_CHANNEL_READABLE, 0 }; }
        int i_sh = n; items[n++] = (rx_wait_item_t){ sh_ctl, RX_CHANNEL_READABLE, 0 };
        if (drv_proc) { i_dproc = n; items[n++] = (rx_wait_item_t){ drv_proc, RX_PROCESS_TERMINATED, 0 }; }
        int i_sproc = n; items[n++] = (rx_wait_item_t){ sh_proc, RX_PROCESS_TERMINATED, 0 };
        if (fsd_proc) { i_fsd = n; items[n++] = (rx_wait_item_t){ fsd_proc, RX_PROCESS_TERMINATED, 0 }; }
        if (dm_proc) { i_dm = n; items[n++] = (rx_wait_item_t){ dm_proc, RX_PROCESS_TERMINATED, 0 }; }

        if (rx_object_wait_many(items, n, RX_TIME_INFINITE) != RX_OK) continue;

        if (i_rep >= 0 && repeating && (items[i_rep].pending & RX_TIMER_SIGNALED)) repeat_fired();
        // Output first, so a prompt appears before the echo of what is typed
        if (items[i_out].pending & RX_CHANNEL_READABLE) console_output();
        if (i_drv >= 0 && (items[i_drv].pending & RX_CHANNEL_READABLE)) keys_from(drv_chan);
        if (i_kin >= 0 && (items[i_kin].pending & RX_CHANNEL_READABLE)) keys_from(kernel_input);
        if (items[i_sh].pending & RX_CHANNEL_READABLE) command();
        if (i_dproc >= 0 && (items[i_dproc].pending & RX_PROCESS_TERMINATED)) driver_ended();
        if (i_fsd >= 0 && (items[i_fsd].pending & RX_PROCESS_TERMINATED)) {
            printf("init: the filesystem service ended (code %lld); files are gone until a reboot\n",
                   (long long)rx_wait_process(fsd_proc));
            fsd_proc = 0;
        }
        if (i_dm >= 0 && (items[i_dm].pending & RX_PROCESS_TERMINATED)) {
            static int dm_restarts;
            long long code = (long long)rx_wait_process(dm_proc);
            dm_proc = 0;
            if (++dm_restarts <= 3) {
                printf("init: the device manager ended (code %lld); starting it again\n", code);
                start_devmgr();
            } else {
                printf("init: the device manager ended (code %lld); not starting it again\n", code);
            }
        }
        if (items[i_sproc].pending & RX_PROCESS_TERMINATED) {
            console_output(); // its last words
            rx_handle_close(sh_proc);
            rx_handle_close(sh_ctl);
            printf("\nShell exited; starting a new one\n\n");
            start_shell();
        }
    }
}

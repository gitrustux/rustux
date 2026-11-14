// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// logd: the system log (step 18). It serves /svc/log (<rustux/log.h>) and
// writes every message to /var/log/system.log, one line each, rotated at
// 1 MiB (system.log.1 ... .4). Messages come from
//
//   - programs: syslog(), logger (RX_LOG_OP_WRITE; the pid and uid are the
//     sender's, from the kernel)
//   - services: init gives each service's stdout and stderr to logd as a
//     socket (RX_LOG_OP_STREAM); each line is a message. A line may start
//     with a priority as <N> (0-7), as systemd's services do
//   - the kernel: its log (what dmesg shows), read every half second
//
// The file is root's, group adm (4), mode 0640: `logs` reads it. Each boot
// gets a number (/var/log/boot.count), recorded with every message.

#include <errno.h>
#include <fcntl.h>
#include <rustux/log.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define MAX_CLIENTS 64
#define MAX_STREAMS 48
#define MAX_SIZE (1024 * 1024)
#define ADM_GID 4

typedef struct {
    rx_handle_t sock;
    char tag[RX_LOG_TAG_MAX];
    uint64_t pid;
    uint32_t uid, prio;
    char buf[RX_LOG_TEXT_MAX];
    size_t len;
} stream_t;

static rx_handle_t listen_ch, klog;
static rx_handle_t clients[MAX_CLIENTS];
static int nclients;
static stream_t streams[MAX_STREAMS];
static int nstreams;
static char kbuf[RX_LOG_TEXT_MAX];
static size_t klen;
static int log_fd = -1;
static long log_size;
static unsigned boot;

static void open_log(void) {
    if (log_fd >= 0) return;
    mkdir("/var", 0755);
    mkdir("/var/log", 0755);
    struct stat st;
    int fresh = stat(RX_LOG_FILE, &st) < 0;
    log_fd = open(RX_LOG_FILE, O_WRONLY | O_CREAT | O_APPEND, 0640);
    if (log_fd < 0) return;
    if (fresh) {
        chown(RX_LOG_FILE, 0, ADM_GID);
        chmod(RX_LOG_FILE, 0640);
    }
    log_size = fstat(log_fd, &st) == 0 ? (long)st.st_size : 0;
}

static void rotate(void) {
    char a[64], b[64];
    close(log_fd);
    log_fd = -1;
    snprintf(a, sizeof a, "%s.%d", RX_LOG_FILE, RX_LOG_KEEP);
    unlink(a);
    for (int k = RX_LOG_KEEP - 1; k >= 1; k--) {
        snprintf(a, sizeof a, "%s.%d", RX_LOG_FILE, k);
        snprintf(b, sizeof b, "%s.%d", RX_LOG_FILE, k + 1);
        rename(a, b);
    }
    snprintf(b, sizeof b, "%s.1", RX_LOG_FILE);
    rename(RX_LOG_FILE, b);
    open_log();
}

// One message: TIME BOOT PRIORITY TAG PID UID MESSAGE
static void record(unsigned prio, const char *tag, uint64_t pid, uint32_t uid, const char *text, size_t n) {
    char line[RX_LOG_TEXT_MAX + 160];
    int64_t now = rx_clock_get_utc();
    time_t t = (time_t)(now / 1000000000);
    struct tm tm;
    gmtime_r(&t, &tm);
    // A priority at the start, <N>, as systemd's services write it
    if (n >= 3 && text[0] == '<' && text[1] >= '0' && text[1] <= '7' && text[2] == '>') {
        prio = (unsigned)(text[1] - '0');
        text += 3;
        n -= 3;
    }
    int k = snprintf(line, sizeof line, "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ\t%u\t%u\t", tm.tm_year + 1900, tm.tm_mon + 1,
                     tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, (int)(now / 1000000 % 1000), boot, prio > 7 ? 7 : prio);
    // The tag: no tabs or spaces
    for (const char *p = tag; *p && k < 100; p++) line[k++] = (*p == '\t' || *p == ' ' || (unsigned char)*p < 0x20) ? '_' : *p;
    if (!tag[0]) k += snprintf(line + k, sizeof line - (size_t)k, "unknown");
    k += snprintf(line + k, sizeof line - (size_t)k, "\t%llu\t%u\t", (unsigned long long)pid, uid);
    // The text: escape sequences (colours) dropped, control characters as spaces
    for (size_t i = 0; i < n && (size_t)k < sizeof line - 2; i++) {
        unsigned char c = (unsigned char)text[i];
        if (c == 0x1b) {
            if (i + 1 < n && text[i + 1] == '[') {
                i += 2;
                while (i < n && !((unsigned char)text[i] >= 0x40 && (unsigned char)text[i] <= 0x7e)) i++;
            }
            continue;
        }
        if (c == '\r') continue;
        line[k++] = (c < 0x20 || c == 0x7f) ? ' ' : (char)c;
    }
    while (k > 0 && line[k - 1] == ' ') k--;
    line[k++] = '\n';
    open_log();
    if (log_fd < 0) return;
    if (log_size + k > MAX_SIZE) rotate();
    if (log_fd < 0) return;
    if (write(log_fd, line, (size_t)k) == k) log_size += k;
}

static void start_boot(void) {
    char buf[32] = "";
    int fd = open(RX_LOG_BOOT, O_RDONLY);
    if (fd >= 0) {
        ssize_t n = read(fd, buf, sizeof buf - 1);
        buf[n > 0 ? n : 0] = 0;
        close(fd);
    }
    boot = (unsigned)strtoul(buf, 0, 10) + 1;
    mkdir("/var", 0755);
    mkdir("/var/log", 0755);
    fd = open(RX_LOG_BOOT ".new", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) {
        int n = snprintf(buf, sizeof buf, "%u\n", boot);
        write(fd, buf, (size_t)n);
        close(fd);
        rename(RX_LOG_BOOT ".new", RX_LOG_BOOT);
    }
}

// Lines of the kernel log
static unsigned kernel_prio(const char *s, size_t n) {
    char low[160];
    size_t m = n < sizeof low - 1 ? n : sizeof low - 1;
    for (size_t i = 0; i < m; i++) low[i] = (char)(s[i] >= 'A' && s[i] <= 'Z' ? s[i] + 32 : s[i]);
    low[m] = 0;
    if (strstr(low, "panic")) return 0;
    if (strstr(low, "error") || strstr(low, "fail")) return 3;
    if (strstr(low, "warn")) return 4;
    return 6;
}

static void read_kernel(void) {
    if (!klog) return;
    for (;;) {
        char buf[4096];
        rx_status_t n = rx_debuglog_read(klog, 0, buf, sizeof buf);
        if (n <= 0) return;
        for (rx_status_t i = 0; i < n; i++) {
            if (buf[i] == '\n' || klen == sizeof kbuf - 1) {
                if (klen) record(kernel_prio(kbuf, klen), "kernel", 0, 0, kbuf, klen);
                klen = 0;
                if (buf[i] == '\n') continue;
            }
            kbuf[klen++] = buf[i];
        }
    }
}

static void close_stream(int k) {
    stream_t *s = &streams[k];
    if (s->len) record(s->prio, s->tag, s->pid, s->uid, s->buf, s->len);
    rx_handle_close(s->sock);
    streams[k] = streams[--nstreams];
}

// Returns 0 when the stream has ended
static int read_stream(stream_t *s) {
    for (;;) {
        char buf[1024];
        uint64_t got = 0;
        rx_status_t r = rx_socket_read(s->sock, 0, buf, sizeof buf, &got);
        if (r == RX_ERR_SHOULD_WAIT) return 1;
        if (r != RX_OK) return 0;
        for (uint64_t i = 0; i < got; i++) {
            if (buf[i] == '\n' || s->len == sizeof s->buf - 1) {
                if (s->len) record(s->prio, s->tag, s->pid, s->uid, s->buf, s->len);
                s->len = 0;
                if (buf[i] == '\n') continue;
            }
            s->buf[s->len++] = buf[i];
        }
    }
}

static void client_message(int k) {
    struct {
        rx_log_msg_t h;
        char text[RX_LOG_TEXT_MAX];
    } m;
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_sender_t who;
    for (;;) {
        rx_status_t r = rx_channel_read_creds(clients[k], &m, &h, sizeof m, 1, &nb, &nh, &who);
        if (r == RX_ERR_SHOULD_WAIT) return;
        if (r != RX_OK) {
            rx_handle_close(clients[k]);
            clients[k] = clients[--nclients];
            return;
        }
        if (nb < sizeof m.h) {
            if (nh) rx_handle_close(h);
            continue;
        }
        m.h.tag[RX_LOG_TAG_MAX - 1] = 0;
        if (m.h.op == RX_LOG_OP_STREAM && nh && who.euid == 0 && nstreams < MAX_STREAMS) {
            stream_t *s = &streams[nstreams++];
            memset(s, 0, sizeof *s);
            s->sock = h;
            snprintf(s->tag, sizeof s->tag, "%s", m.h.tag);
            s->pid = m.h.pid;
            s->uid = m.h.uid;
            s->prio = m.h.priority > 7 ? 6 : m.h.priority;
            continue;
        }
        if (nh) rx_handle_close(h);
        if (m.h.op != RX_LOG_OP_WRITE) continue;
        char tag[RX_LOG_TAG_MAX + 8];
        // Only root speaks for the kernel and init
        if (who.euid != 0 && (!strcmp(m.h.tag, "kernel") || !strcmp(m.h.tag, "init")))
            snprintf(tag, sizeof tag, "user-%s", m.h.tag);
        else
            snprintf(tag, sizeof tag, "%s", m.h.tag);
        record(m.h.priority > 7 ? 7 : m.h.priority, tag, who.pid, who.euid, m.text, nb - sizeof m.h);
    }
}

static void accept_clients(void) {
    rx_svc_connect_t m;
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    while (rx_channel_read(listen_ch, 0, &m, &h, sizeof m, 1, &nb, &nh) == RX_OK) {
        if (!nh) continue;
        if (nclients == MAX_CLIENTS) rx_handle_close(h);
        else clients[nclients++] = h;
    }
}

int main(void) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    rx_handle_t theirs;
    start_boot();
    if (rx_channel_create(0, &listen_ch, &theirs) != RX_OK || rx_service_publish("/svc/log", theirs) < 0) {
        fprintf(stderr, "logd: cannot publish /svc/log\n");
        return 1;
    }
    if (rx_debuglog_create(0, RX_LOG_FLAG_READABLE, &klog) != RX_OK) klog = 0;
    char hello[80];
    int n = snprintf(hello, sizeof hello, "boot %u: logging to %s", boot, RX_LOG_FILE);
    record(6, "logd", (uint64_t)getpid(), 0, hello, (size_t)n);
    read_kernel();
    for (;;) {
        rx_wait_item_t items[1 + MAX_CLIENTS + MAX_STREAMS];
        int ni = 0;
        items[ni++] = (rx_wait_item_t){ listen_ch, RX_CHANNEL_READABLE, 0 };
        for (int k = 0; k < nclients; k++) items[ni++] = (rx_wait_item_t){ clients[k], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        for (int k = 0; k < nstreams; k++) items[ni++] = (rx_wait_item_t){ streams[k].sock, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        rx_object_wait_many(items, (uint64_t)ni, rx_clock_get_monotonic() + 500000000ull);
        // Streams and clients from the back: closing one moves the last into its place
        for (int k = nstreams - 1; k >= 0; k--)
            if (items[1 + nclients + k].pending && !read_stream(&streams[k])) close_stream(k);
        for (int k = nclients - 1; k >= 0; k--)
            if (items[1 + k].pending) client_message(k);
        if (items[0].pending & RX_CHANNEL_READABLE) accept_clients();
        read_kernel();
    }
}

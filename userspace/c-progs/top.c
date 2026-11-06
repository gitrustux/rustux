// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// top: processes by CPU use, refreshed on the screen (checklist step 11)
//
//   top [-d SECONDS] [-n COUNT] [-b]
//
// -d: seconds between updates (default 2), -n: stop after COUNT updates,
// -b: plain text, one report after another (no screen control, no keys).
// Keys: q leaves, space updates now, m/c/p sort by memory/CPU/pid.

#include "tasks.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define MAX_PROCS 512

struct proc {
    uint64_t pid, job, threads, mem, cpu;
    double pct;
    char name[RX_MAX_NAME_LEN];
};

static struct proc procs[MAX_PROCS], prev[MAX_PROCS];
static int nprocs, nprev;
static char sort_key = 'c';

static void collect(rx_handle_t p, uint64_t koid, uint64_t job, void *ctx) {
    (void)ctx;
    if (nprocs == MAX_PROCS) return;
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_get_info(p, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    if (info.flags & RX_INFO_PROCESS_FLAG_EXITED) return;
    struct proc *x = &procs[nprocs++];
    memset(x, 0, sizeof *x);
    x->pid = koid;
    x->job = job;
    strcpy(x->name, "?");
    rx_object_get_property(p, RX_PROP_NAME, x->name, sizeof x->name);
    rx_info_task_stats_t st = { 0, 0, 0, 0 };
    rx_info_task_runtime_t rt = { 0, 0, 0, 0 };
    uint64_t th[64], n = 0, avail = 0;
    rx_object_get_info(p, RX_INFO_TASK_STATS, &st, sizeof st, 0, 0);
    rx_object_get_info(p, RX_INFO_TASK_RUNTIME, &rt, sizeof rt, 0, 0);
    rx_object_get_info(p, RX_INFO_PROCESS_THREADS, th, sizeof th, &n, &avail);
    x->threads = avail;
    x->mem = st.mem_private_bytes;
    x->cpu = rt.cpu_time;
}

static int cmp(const void *a, const void *b) {
    const struct proc *x = a, *y = b;
    if (sort_key == 'm') return x->mem < y->mem ? 1 : x->mem > y->mem ? -1 : 0;
    if (sort_key == 'p') return x->pid < y->pid ? -1 : 1;
    if (x->pct != y->pct) return x->pct < y->pct ? 1 : -1;
    return x->cpu < y->cpu ? 1 : x->cpu > y->cpu ? -1 : 0;
}

static void sample(uint64_t interval_ns) {
    memcpy(prev, procs, sizeof procs[0] * (size_t)nprocs);
    nprev = nprocs;
    nprocs = 0;
    rx_walk_job(rx_system_job(), rx_koid_of(rx_system_job()), collect, 0);
    for (int i = 0; i < nprocs; i++) {
        uint64_t before = 0;
        for (int k = 0; k < nprev; k++)
            if (prev[k].pid == procs[i].pid) before = prev[k].cpu;
        procs[i].pct = interval_ns ? 100.0 * (double)(procs[i].cpu - before) / (double)interval_ns : 0;
    }
    qsort(procs, (size_t)nprocs, sizeof *procs, cmp);
}

static const char *EL = "\033[K"; // erase to the end of the line (screen only)

static void report(int screen, int rows, uint64_t idle_delta, uint64_t interval_ns) {
    if (!screen) EL = "";
    rx_system_stats_t s;
    memset(&s, 0, sizeof s);
    rx_system_get_stats(&s);
    time_t now = time(0);
    struct tm tm;
    char clock[16];
    strftime(clock, sizeof clock, "%H:%M:%S", localtime_r(&now, &tm));
    unsigned long long up = s.uptime / 1000000000ull;
    double busy = interval_ns && s.cpus ? 100.0 * (1.0 - (double)idle_delta / ((double)interval_ns * (double)s.cpus)) : 0;
    if (busy < 0) busy = 0;
    if (screen) printf("\033[H");
    printf("top - %s up %llu:%02llu:%02llu, %llu CPU%s, %d processes, %llu threads%s\n", clock, up / 3600, up / 60 % 60,
           up % 60, (unsigned long long)s.cpus, s.cpus == 1 ? "" : "s", nprocs, (unsigned long long)s.threads, EL);
    printf("CPU: %5.1f%% busy   Memory: %llu MiB total, %llu MiB used, %llu MiB free%s\n", busy,
           (unsigned long long)(s.total_bytes >> 20), (unsigned long long)((s.total_bytes - s.free_bytes) >> 20),
           (unsigned long long)(s.free_bytes >> 20), EL);
    printf("%s\n", EL);
    if (screen) printf("\033[7m");
    printf("%7s %7s %4s %10s %6s %11s  %s", "PID", "JOB", "THR", "MEM(KiB)", "CPU%", "TIME", screen ? "NAME                " : "NAME");
    if (screen) printf("\033[K\033[0m");
    printf("\n");
    int shown = 0;
    for (int i = 0; i < nprocs; i++) {
        if (screen && shown >= rows - 5) break;
        long long ms = (long long)(procs[i].cpu / 1000000);
        printf("%7llu %7llu %4llu %10llu %6.1f %4lld:%02lld.%03lld  %-20s%s\n", (unsigned long long)procs[i].pid,
               (unsigned long long)procs[i].job, (unsigned long long)procs[i].threads,
               (unsigned long long)(procs[i].mem / 1024), procs[i].pct, ms / 60000, ms / 1000 % 60, ms % 1000,
               procs[i].name, EL);
        shown++;
    }
    if (screen) printf("\033[J");
    fflush(stdout);
}

int main(int argc, char **argv) {
    double delay = 2;
    long count = -1;
    int batch = !isatty(1);
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-d") && i + 1 < argc) delay = atof(argv[++i]);
        else if (!strncmp(argv[i], "-d", 2) && argv[i][2]) delay = atof(argv[i] + 2);
        else if (!strcmp(argv[i], "-n") && i + 1 < argc) count = atol(argv[++i]);
        else if (!strncmp(argv[i], "-n", 2) && argv[i][2]) count = atol(argv[i] + 2);
        else if (!strcmp(argv[i], "-b")) batch = 1;
        else {
            printf("Usage: top [-d SECONDS] [-n COUNT] [-b]\n"
                   "Processes by CPU use, updated every 2 seconds (-d). -n: stop after COUNT updates.\n"
                   "-b: plain text reports. Keys: q quit, space update, m/c/p sort by memory/CPU/pid.\n");
            return strcmp(argv[i], "--help") ? 2 : 0;
        }
    }
    if (delay < 0.1) delay = 0.1;
    struct termios old, raw;
    int screen = !batch && tcgetattr(0, &old) == 0;
    int rows = 25;
    if (screen) {
        struct winsize ws;
        if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_row) rows = ws.ws_row;
        raw = old;
        cfmakeraw(&raw);
        raw.c_lflag |= ISIG;
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 1;
        tcsetattr(0, TCSANOW, &raw);
        printf("\033[?1049h\033[?25l\033[2J");
    }
    rx_system_stats_t s;
    rx_system_get_stats(&s);
    uint64_t last = s.uptime, last_idle = s.idle_time;
    // A first, short sample so the first report has CPU figures
    sample(0);
    usleep(250000);
    for (long n = 0; count < 0 || n < count; n++) {
        rx_system_get_stats(&s);
        uint64_t interval = s.uptime - last, idle = s.idle_time - last_idle;
        last = s.uptime;
        last_idle = s.idle_time;
        sample(interval);
        report(screen, rows, idle, interval);
        if (batch) printf("\n");
        if (count >= 0 && n + 1 >= count) break;
        // Wait for the next update, or a key
        uint64_t until = rx_clock_get_monotonic() + (uint64_t)(delay * 1e9);
        int quit = 0, now = 0;
        while (!quit && !now && rx_clock_get_monotonic() < until) {
            if (!screen) {
                usleep(100000);
                continue;
            }
            char c;
            if (read(0, &c, 1) == 1) {
                if (c == 'q' || c == 'Q' || c == 3) quit = 1;
                else if (c == ' ') now = 1;
                else if (c == 'm' || c == 'c' || c == 'p' || c == 'M' || c == 'P') {
                    sort_key = (char)(c | 0x20);
                    now = 1;
                }
            }
        }
        if (quit) break;
    }
    if (screen) {
        printf("\033[0m\033[?25h\033[?1049l");
        fflush(stdout);
        tcsetattr(0, TCSANOW, &old);
    }
    return 0;
}

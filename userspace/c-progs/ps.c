// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ps: list processes (everything in the system job the shell passes on,
// and its child jobs)
//
//   PID    process id (koid)       JOB   the job it is in
//   THR    running threads         MEM   memory it uses (KiB)
//   TIME   CPU time used           NAME  program name

#include "tasks.h"
#include <stdio.h>
#include <string.h>

static int count, json;

static void show(rx_handle_t p, uint64_t koid, uint64_t job, void *ctx) {
    (void)ctx;
    char name[RX_MAX_NAME_LEN] = "?";
    rx_info_task_stats_t st = { 0, 0, 0, 0 };
    rx_info_task_runtime_t rt = { 0, 0, 0, 0 };
    rx_info_process_t info = { 0, 0, 0, 0 };
    uint64_t threads[64], nt = 0, avail = 0;
    rx_object_get_property(p, RX_PROP_NAME, name, sizeof name);
    rx_object_get_info(p, RX_INFO_TASK_STATS, &st, sizeof st, 0, 0);
    rx_object_get_info(p, RX_INFO_TASK_RUNTIME, &rt, sizeof rt, 0, 0);
    rx_object_get_info(p, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_object_get_info(p, RX_INFO_PROCESS_THREADS, threads, sizeof threads, &nt, &avail);
    if (info.flags & RX_INFO_PROCESS_FLAG_EXITED) return;
    long long ms = rt.cpu_time / 1000000;
    if (json) {
        printf("{\"pid\":%llu,\"job\":%llu,\"threads\":%llu,\"memory\":%llu,\"cpu_ns\":%llu,\"name\":\"%s\"}\n",
               (unsigned long long)koid, (unsigned long long)job, (unsigned long long)avail,
               (unsigned long long)st.mem_private_bytes, (unsigned long long)rt.cpu_time, name);
        count++;
        return;
    }
    printf("%6llu %6llu %3llu %8llu %4lld:%02lld.%03lld %s\n", (unsigned long long)koid, (unsigned long long)job,
           (unsigned long long)avail, (unsigned long long)(st.mem_private_bytes / 1024),
           ms / 60000, ms / 1000 % 60, ms % 1000, name);
    count++;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "--json")) {
        json = 1;
    } else if (argc > 1) {
        printf("Usage: ps [--json]\nList processes: PID, JOB (the job it runs in), THR (threads), MEM (KiB),\n"
               "TIME (CPU time), NAME. --json: one object per process.\n");
        return strcmp(argv[1], "--help") ? 2 : 0;
    }
    if (!json) printf("%6s %6s %3s %8s %12s %s\n", "PID", "JOB", "THR", "MEM(KiB)", "TIME", "NAME");
    rx_walk_job(rx_system_job(), rx_koid_of(rx_system_job()), show, 0);
    return count ? 0 : 1;
}

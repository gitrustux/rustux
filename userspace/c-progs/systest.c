// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// systest: process and system information (checklist step 9): object
// info topics, finding and killing processes, CPU time, memory, clocks,
// time zones, the kernel log and version

#include "tasks.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

static int passed, failed;

static void check(const char *name, int ok) {
    if (ok) passed++;
    else { failed++; printf("  FAIL %s\n", name); }
}

static int local_is(const char *tz, time_t t, const char *want) {
    char buf[64];
    struct tm tm;
    setenv("TZ", tz, 1);
    strftime(buf, sizeof buf, "%Y-%m-%d %H:%M:%S %Z %z", localtime_r(&t, &tm));
    if (strcmp(buf, want)) {
        printf("  %s at %lld: got \"%s\", want \"%s\"\n", tz, (long long)t, buf, want);
        return 0;
    }
    return 1;
}

static void tasks(void) {
    printf("processes:\n");
    rx_info_handle_basic_t b;
    check("HANDLE_BASIC of ourselves", rx_object_get_info(rx_proc_self, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0) == RX_OK &&
                                         b.type == RX_OBJ_TYPE_PROCESS && b.koid != 0);
    check("  related koid is our job", b.related_koid == rx_koid_of(rx_job_default));
    check("getpid() is our koid", (uint64_t)getpid() == b.koid);
    char name[RX_MAX_NAME_LEN];
    check("name property", rx_object_get_property(rx_proc_self, RX_PROP_NAME, name, sizeof name) == RX_OK &&
                           !strcmp(name, "systest"));
    uint64_t koids[16], n = 0, avail = 0;
    check("PROCESS_THREADS: one thread", rx_object_get_info(rx_proc_self, RX_INFO_PROCESS_THREADS, koids, sizeof koids, &n, &avail) == RX_OK &&
                                          n == 1 && avail == 1);
    int found = 0;
    if (rx_object_get_info(rx_job_default, RX_INFO_JOB_PROCESSES, koids, sizeof koids, &n, &avail) == RX_OK)
        for (uint64_t i = 0; i < n; i++) found |= koids[i] == b.koid;
    check("JOB_PROCESSES of our job lists us", found);
    rx_handle_t me;
    check("object_get_child finds us by koid", rx_object_get_child(rx_job_default, b.koid, RX_RIGHT_SAME_RIGHTS, &me) == RX_OK &&
                                               rx_koid_of(me) == b.koid);
    rx_handle_close(me);
    check("object_get_child of a missing koid -> NOT_FOUND",
          rx_object_get_child(rx_job_default, 1, RX_RIGHT_SAME_RIGHTS, &me) == RX_ERR_NOT_FOUND);

    rx_info_task_stats_t st;
    check("TASK_STATS: memory mapped and committed", rx_object_get_info(rx_proc_self, RX_INFO_TASK_STATS, &st, sizeof st, 0, 0) == RX_OK &&
                                                     st.mem_mapped_bytes > 100000 && st.mem_private_bytes > 0 &&
                                                     st.mem_private_bytes <= st.mem_mapped_bytes);
    rx_info_task_runtime_t r0, r1;
    rx_object_get_info(rx_proc_self, RX_INFO_TASK_RUNTIME, &r0, sizeof r0, 0, 0);
    uint64_t t0 = rx_clock_get_monotonic();
    while (rx_clock_get_monotonic() - t0 < 60000000ull) {}
    rx_object_get_info(rx_proc_self, RX_INFO_TASK_RUNTIME, &r1, sizeof r1, 0, 0);
    int64_t used = (r1.cpu_time - r0.cpu_time) / 1000000;
    check("TASK_RUNTIME: 60 ms of work counted", used >= 40 && used <= 80);
    if (used < 40 || used > 80) printf("    (counted %lld ms)\n", (long long)used);

    // Kill a process found through the job tree
    char *argv[] = { "/bin/spin", 0 };
    rx_handle_t proc;
    int ok = rx_spawn("/bin/spin", argv, 0, RX_SPAWN_CLONE_NS, 0, 0, &proc) == RX_OK;
    uint64_t victim = ok ? rx_koid_of(proc) : 0;
    usleep(20000);
    rx_handle_t found_h;
    ok = ok && rx_object_get_child(rx_job_default, victim, RX_RIGHT_SAME_RIGHTS, &found_h) == RX_OK;
    ok = ok && rx_task_kill(found_h) == RX_OK;
    if (ok) rx_handle_close(found_h);
    check("find a running program by koid and kill it", ok && rx_wait_process(proc) == RX_TASK_RETCODE_SYSCALL_KILL);
}

static void system_info(void) {
    printf("system:\n");
    rx_system_stats_t s;
    check("system_get_stats", rx_system_get_stats(&s) == RX_OK);
    check("  memory: total > free > 0", s.total_bytes > s.free_bytes && s.free_bytes > 0);
    check("  CPUs and threads", s.cpus >= 1 && s.cpus <= 16 && s.threads >= 4);
    check("  uptime", s.uptime > 1000000000ull && s.idle_time <= s.uptime * s.cpus);
    char v[128];
    check("version string", rx_system_get_version(v, sizeof v) > 0 && !strncmp(v, "Rustux ", 7) && strstr(v, "x86_64"));
    check("version: too small a buffer -> BUFFER_TOO_SMALL", rx_system_get_version(v, 4) == RX_ERR_BUFFER_TOO_SMALL);
    struct utsname u;
    check("uname", uname(&u) == 0 && !strcmp(u.sysname, "Rustux") && u.release[0] && !strcmp(u.machine, "x86_64"));

    rx_handle_t log;
    char buf[4096];
    rx_status_t n = rx_debuglog_create(0, RX_LOG_FLAG_READABLE, &log) == RX_OK ? rx_debuglog_read(log, 0, buf, sizeof buf - 1) : -1;
    if (n > 0) buf[n] = 0;
    check("kernel log readable without a resource", n > 0);
    rx_handle_t w;
    check("writing the log needs the root resource", rx_debuglog_create(0, 0, &w) != RX_OK);
    check("power control needs the power resource", rx_system_powerctl(log, RX_SYSTEM_POWERCTL_REBOOT) == RX_ERR_WRONG_TYPE);
    check("setting the clock needs the root resource", rx_clock_set_utc(0, 0) != RX_OK);
}

static void clocks(void) {
    printf("clocks and time zones:\n");
    time_t now = time(0);
    check("wall clock is after 2024 (real-time clock read)", now > 1704067200);
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    usleep(10000);
    clock_gettime(CLOCK_MONOTONIC, &b);
    check("monotonic clock advances", (b.tv_sec - a.tv_sec) * 1000000000L + (b.tv_nsec - a.tv_nsec) >= 9000000);

    struct tm tm;
    time_t zero = 0;
    gmtime_r(&zero, &tm);
    check("gmtime(0) is Thursday 1970-01-01", tm.tm_year == 70 && tm.tm_mon == 0 && tm.tm_mday == 1 && tm.tm_wday == 4);
    struct tm x = { 0, 0, 21, 4, 10, 126, 0, 0, 0, 0, 0 };
    check("timegm", timegm(&x) == 1793826000 && x.tm_wday == 3 && x.tm_yday == 307);
    char buf[64];
    strftime(buf, sizeof buf, "%a %b %e %H:%M:%S %Y|%F %T|%j|%I %p", &x);
    check("strftime", !strcmp(buf, "Wed Nov  4 21:00:00 2026|2026-11-04 21:00:00|308|09 PM"));

    check("TZ=UTC", local_is("UTC", 1793826000, "2026-11-04 21:00:00 UTC +0000"));
    check("America/Chicago in summer (CDT)", local_is("America/Chicago", 1782907200, "2026-07-01 07:00:00 CDT -0500"));
    check("America/Chicago in winter (CST)", local_is("America/Chicago", 1796126400, "2026-12-01 06:00:00 CST -0600"));
    check("  last second of CST in March", local_is("America/Chicago", 1772956799, "2026-03-08 01:59:59 CST -0600"));
    check("  first second of CDT", local_is("America/Chicago", 1772956800, "2026-03-08 03:00:00 CDT -0500"));
    check("  last second of CDT in November", local_is("America/Chicago", 1793516399, "2026-11-01 01:59:59 CDT -0500"));
    check("  back to CST", local_is("America/Chicago", 1793516400, "2026-11-01 01:00:00 CST -0600"));
    check("a POSIX rule string", local_is("EST5EDT,M3.2.0,M11.1.0", 1782907200, "2026-07-01 08:00:00 EDT -0400"));
    check("Europe/London in summer (BST)", local_is("Europe/London", 1784073600, "2026-07-15 01:00:00 BST +0100"));
    check("Australia/Sydney in January (southern summer)", local_is("Australia/Sydney", 1768435200, "2026-01-15 11:00:00 AEDT +1100"));
    check("Asia/Kolkata (half hour)", local_is("Asia/Kolkata", 1768435200, "2026-01-15 05:30:00 IST +0530"));
    setenv("TZ", "America/Chicago", 1);
    struct tm l = { 0, 0, 7, 1, 6, 126, 0, 0, -1, 0, 0 };
    check("mktime (local time to UTC)", mktime(&l) == 1782907200 && l.tm_isdst == 1);
    unsetenv("TZ");
}

int main(void) {
    printf("systest: process and system information\n");
    tasks();
    system_info();
    clocks();
    printf("systest: %d passed, %d failed\n", passed, failed);
    return failed;
}

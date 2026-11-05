// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Walking the job tree (for ps and kill): every process in a job and its
// child jobs.

#ifndef TASKS_H
#define TASKS_H

#include <rustux/process.h>

typedef void (*rx_visit_fn)(rx_handle_t proc, uint64_t koid, uint64_t job_koid, void *ctx);

static inline void rx_walk_job(rx_handle_t job, uint64_t job_koid, rx_visit_fn visit, void *ctx) {
    uint64_t koids[256];
    uint64_t n = 0, avail = 0;
    if (rx_object_get_info(job, RX_INFO_JOB_PROCESSES, koids, sizeof koids, &n, &avail) == RX_OK) {
        for (uint64_t i = 0; i < n; i++) {
            rx_handle_t p;
            if (rx_object_get_child(job, koids[i], RX_RIGHT_SAME_RIGHTS, &p) == RX_OK) {
                visit(p, koids[i], job_koid, ctx);
                rx_handle_close(p);
            }
        }
    }
    if (rx_object_get_info(job, RX_INFO_JOB_CHILDREN, koids, sizeof koids, &n, &avail) == RX_OK) {
        for (uint64_t i = 0; i < n; i++) {
            rx_handle_t j;
            if (rx_object_get_child(job, koids[i], RX_RIGHT_SAME_RIGHTS, &j) == RX_OK) {
                rx_walk_job(j, koids[i], visit, ctx);
                rx_handle_close(j);
            }
        }
    }
}

// The job to look at: the system job the shell passes on, else our own
static inline rx_handle_t rx_system_job(void) {
    static rx_handle_t job;
    if (!job) job = rx_take_startup_handle(RX_PA_JOB_SYSTEM);
    return job ? job : rx_job_default;
}

static inline uint64_t rx_koid_of(rx_handle_t h) {
    rx_info_handle_basic_t b = { 0, 0, 0, 0 };
    rx_object_get_info(h, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0);
    return b.koid;
}

#endif

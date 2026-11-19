// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The file chooser's client (<rustux/portal.h>): a request on the channel
// appctl hands the app (RX_PA_PORTAL): "open\nTITLE\nPATTERNS" or
// "save\nTITLE\nNAME"; the answer "ok NAME" with the file's connection,
// "cancel", or "error ERRNO MESSAGE".

#include "libc.h"
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <rustux/portal.h>
#include <rustux/process.h>
#include <rustux/syscalls.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t portal;
static int taken;

static int ask(const char *verb, const char *title, const char *extra, int flags, char *name, size_t cap) {
    pthread_mutex_lock(&lock);
    if (!taken) {
        portal = rx_take_startup_handle(RX_PA_HND(RX_PA_PORTAL, 0));
        taken = 1;
    }
    if (!portal) {
        pthread_mutex_unlock(&lock);
        return -ENOSYS;
    }
    char req[1024];
    int n = snprintf(req, sizeof req, "%s\n%s\n%s", verb, title ? title : "", extra ? extra : "");
    if (n < 0 || (size_t)n >= sizeof req) {
        pthread_mutex_unlock(&lock);
        return -ENAMETOOLONG;
    }
    for (char *c = req; *c; c++)
        if (*c == '\r') *c = ' ';
    if (rx_channel_write(portal, 0, req, (uint32_t)n, 0, 0) != RX_OK) {
        pthread_mutex_unlock(&lock);
        return -EPIPE;
    }
    char rep[1024];
    rx_handle_t h = 0;
    uint32_t got = 0, nh = 0;
    for (;;) {
        rx_status_t s = rx_channel_read(portal, 0, rep, &h, sizeof rep - 1, 1, &got, &nh);
        if (s == RX_OK) break;
        if (s != RX_ERR_SHOULD_WAIT) {
            pthread_mutex_unlock(&lock);
            return -EPIPE;
        }
        uint32_t seen = 0;
        rx_object_wait_one(portal, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &seen);
        if (!(seen & RX_CHANNEL_READABLE)) {
            pthread_mutex_unlock(&lock);
            return -EPIPE;
        }
    }
    pthread_mutex_unlock(&lock);
    rep[got] = 0;
    if (!strncmp(rep, "ok ", 3) && nh == 1) {
        if (name && cap) snprintf(name, cap, "%s", rep + 3);
        int fd = rx_fd_from_handle(h, flags);
        if (fd < 0) rx_handle_close(h);
        return fd;
    }
    if (nh) rx_handle_close(h);
    if (!strcmp(rep, "cancel")) return -ECANCELED;
    if (!strncmp(rep, "error ", 6)) {
        int e = atoi(rep + 6);
        return e > 0 ? -e : -EIO;
    }
    return -EIO;
}

int rx_portal_open(const char *title, const char *patterns, char *name, size_t cap) {
    return ask("open", title, patterns, O_RDONLY, name, cap);
}

int rx_portal_save(const char *title, const char *suggested, char *name, size_t cap) {
    return ask("save", title, suggested, O_WRONLY, name, cap);
}

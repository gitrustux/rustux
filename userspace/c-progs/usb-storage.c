// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// usb-storage: USB sticks and disks (step 13). USB Mass Storage Class,
// Bulk-Only Transport (BOT), with the SCSI commands of SBC. The kernel's
// xHCI driver found the device and set up its two bulk endpoints; devmgr
// starts this driver with a USB handle (startup handle PA_USER0) through
// which it moves bytes (rx_usb_bulk, rx_usb_control, rx_usb_clear_halt).
// The disk is served through <rustux/blockdev.h> as sda, sdb, ...
//
// LUN 0 only (card readers with several slots show their first one).
// When the device is unplugged every call fails with PEER_CLOSED and the
// driver exits, which removes its block devices.

#include <errno.h>
#include <rustux/blockdev.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define CBW_SIG 0x43425355u  // "USBC"
#define CSW_SIG 0x53425355u  // "USBS"
#define MAX_XFER 65536u      // one bulk call
#define TIMEOUT_MS 10000     // the command block
#define DATA_TIMEOUT_MS 30000 // its data and status (as Linux's SCSI disk time-out)

// The most bytes one SCSI command moves: halved after a time-out (some
// sticks stall on long writes), down to 4 KiB
static uint32_t cmd_max = MAX_XFER;

static rx_handle_t usb;
static uint8_t iface;
static const char *where = "?";
static uint32_t tag = 1, block_size;
static uint64_t blocks;
static uint8_t *iobuf;

// Sense data of the last failed command
static uint8_t sense_key, asc, ascq;

static void put_be32(uint8_t *p, uint32_t v) { p[0] = (uint8_t)(v >> 24); p[1] = (uint8_t)(v >> 16); p[2] = (uint8_t)(v >> 8); p[3] = (uint8_t)v; }
static uint32_t be32(const uint8_t *p) { return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3]; }
static void put_le32(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); p[2] = (uint8_t)(v >> 16); p[3] = (uint8_t)(v >> 24); }
static uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }

static void sleep_ms(long ms) {
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000 };
    nanosleep(&ts, 0);
}

// The device is gone: leave, which removes our block devices
static void gone(int64_t r) {
    if (r == RX_ERR_PEER_CLOSED) exit(0);
}

// BOT reset recovery (USB MSC BOT 5.3.4): Bulk-Only Reset, then clear
// both endpoints
static void reset_recovery(void) {
    uint8_t setup[8] = { 0x21, 0xFF, 0, 0, iface, 0, 0, 0 };
    gone(rx_usb_control(usb, setup, 0, 0));
    gone(rx_usb_clear_halt(usb, RX_USB_IN));
    gone(rx_usb_clear_halt(usb, 0));
}

// One SCSI command through BOT. data: len bytes in or out. Returns 0, or
// -EIO with the sense data read (CHECK CONDITION), or another -errno.
static int command(const uint8_t *cdb, int cdblen, void *data, uint32_t len, int in, int want_sense);

static void request_sense(void) {
    uint8_t cdb[6] = { 0x03, 0, 0, 0, 18, 0 }, s[18] = { 0 };
    sense_key = asc = ascq = 0;
    if (command(cdb, 6, s, 18, 1, 0) == 0) {
        sense_key = s[2] & 0x0F;
        asc = s[12];
        ascq = s[13];
    }
}

static int command(const uint8_t *cdb, int cdblen, void *data, uint32_t len, int in, int want_sense) {
    uint8_t cbw[31] = { 0 }, csw[13];
    uint32_t t = tag++;
    put_le32(cbw, CBW_SIG);
    put_le32(cbw + 4, t);
    put_le32(cbw + 8, len);
    cbw[12] = in ? 0x80 : 0;
    cbw[13] = 0;              // LUN 0
    cbw[14] = (uint8_t)cdblen;
    memcpy(cbw + 15, cdb, (size_t)cdblen);
    int64_t r = rx_usb_bulk(usb, 0, cbw, 31, TIMEOUT_MS);
    gone(r);
    if (r != 31) {
        reset_recovery();
        return r == RX_ERR_TIMED_OUT ? -ETIMEDOUT : -EIO;
    }
    // Data stage, in pieces of at most one bulk call
    for (uint32_t done = 0; done < len; ) {
        uint32_t n = len - done < MAX_XFER ? len - done : MAX_XFER;
        r = rx_usb_bulk(usb, in ? RX_USB_IN : 0, (uint8_t *)data + done, n, DATA_TIMEOUT_MS);
        gone(r);
        if (r == RX_ERR_IO_REFUSED) {   // stalled: clear it, then the status
            gone(rx_usb_clear_halt(usb, in ? RX_USB_IN : 0));
            break;
        }
        if (r < 0) {
            reset_recovery();
            return r == RX_ERR_TIMED_OUT ? -ETIMEDOUT : -EIO;
        }
        done += (uint32_t)r;
        if ((uint32_t)r < n) break;     // short: the device had less
    }
    // Status: one retry after clearing a stall
    for (int attempt = 0; ; attempt++) {
        r = rx_usb_bulk(usb, RX_USB_IN, csw, 13, DATA_TIMEOUT_MS);
        gone(r);
        if (r == RX_ERR_IO_REFUSED && attempt == 0) {
            gone(rx_usb_clear_halt(usb, RX_USB_IN));
            continue;
        }
        break;
    }
    if (r != 13 || le32(csw) != CSW_SIG || le32(csw + 4) != t) {
        reset_recovery();
        return r == RX_ERR_TIMED_OUT ? -ETIMEDOUT : -EIO;
    }
    if (csw[12] == 0) return 0;
    if (csw[12] == 2) {         // phase error
        reset_recovery();
        return -EIO;
    }
    if (want_sense) request_sense();
    return -EIO;
}

static int is_medium_error(void) { return sense_key == 2 && asc == 0x3A; }

static int rw1(int write, uint64_t lba, uint32_t count, void *buf);

// A read or write, in commands of at most cmd_max bytes. A command that
// times out is tried again after a Bulk-Only Reset; one that times out
// again makes the commands smaller; when even that fails the stick's port
// is reset (it comes back as a new device, with a new driver).
static int timeouts;   // commands in a row that timed out

static int rw(int write, uint64_t lba, uint32_t count, void *buf) {
    uint32_t per = cmd_max / block_size ? cmd_max / block_size : 1;
    for (uint32_t done = 0; done < count;) {
        uint32_t n = count - done < per ? count - done : per;
        int r = rw1(write, lba + done, n, (uint8_t *)buf + (size_t)done * block_size);
        if (r == -ETIMEDOUT) {
            // Smaller commands (16 KiB, then 4 KiB), then a port reset
            if (++timeouts <= 2) {
                uint32_t want = timeouts == 1 ? 16384 : 4096;
                if (want < block_size) want = block_size;
                if (want < cmd_max) {
                    cmd_max = want;
                    fprintf(stderr, "usb-storage %s: commands of at most %u KiB from now on\n", where, cmd_max / 1024);
                }
                per = cmd_max / block_size ? cmd_max / block_size : 1;
                continue;   // the same blocks again
            }
            fprintf(stderr, "usb-storage %s: the device does not answer: resetting its port\n", where);
            rx_usb_reset(usb);
            exit(0);        // a new driver takes the device that comes back
        }
        if (r < 0) return r;
        timeouts = 0;
        done += n;
    }
    return 0;
}

static int rw1(int write, uint64_t lba, uint32_t count, void *buf) {
    uint8_t cdb[16] = { 0 };
    int len;
    if (lba + count <= 0xFFFFFFFFull && count <= 0xFFFF) {
        cdb[0] = write ? 0x2A : 0x28;   // READ(10) / WRITE(10)
        put_be32(cdb + 2, (uint32_t)lba);
        cdb[7] = (uint8_t)(count >> 8);
        cdb[8] = (uint8_t)count;
        len = 10;
    } else {
        cdb[0] = write ? 0x8A : 0x88;   // READ(16) / WRITE(16)
        put_be32(cdb + 2, (uint32_t)(lba >> 32));
        put_be32(cdb + 6, (uint32_t)lba);
        put_be32(cdb + 10, count);
        len = 16;
    }
    int r = 0;
    for (int attempt = 0; attempt < 3; attempt++) {
        sense_key = asc = ascq = 0;
        r = command(cdb, len, buf, count * block_size, !write, 1);
        if (r == 0) return 0;
        if (is_medium_error()) return -ENOMEDIUM;
        if (sense_key == 7) return -EROFS;      // DATA PROTECT
        // Retry UNIT ATTENTION, and transport failures (a bad status
        // block), after which command() did a reset recovery; a time-out
        // once (rw makes the commands smaller after that)
        if (sense_key != 6 && sense_key != 0) break;
        if (r == -ETIMEDOUT && attempt >= 1) return r;
    }
    fprintf(stderr, "usb-storage %s: %s of %u blocks at %llu failed (sense %x/%02x/%02x)\n", where, write ? "write" : "read",
            count, (unsigned long long)lba, sense_key, asc, ascq);
    return r;
}

static int rd(void *ctx, uint64_t block, uint32_t count, void *buf) {
    (void)ctx;
    int r = rw(0, block, count, iobuf);
    if (r == 0) memcpy(buf, iobuf, (size_t)count * block_size);
    return r;
}

static int wr(void *ctx, uint64_t block, uint32_t count, const void *buf) {
    (void)ctx;
    memcpy(iobuf, buf, (size_t)count * block_size);
    return rw(1, block, count, iobuf);
}

static int fl(void *ctx) {
    (void)ctx;
    uint8_t cdb[10] = { 0x35 };     // SYNCHRONIZE CACHE(10)
    int r = command(cdb, 10, 0, 0, 0, 1);
    // Many sticks do not know it (ILLEGAL REQUEST): nothing is cached then
    return r == 0 || sense_key == 5 ? 0 : r;
}

static void trim(char *s) {
    size_t n = strlen(s);
    while (n && s[n - 1] == ' ') s[--n] = 0;
}

int main(int argc, char **argv) {
    if (argc > 1) where = argv[1];
    usb = rx_take_startup_handle(RX_PA_USER0);
    if (!usb) {
        fprintf(stderr, "usb-storage: started without a USB device (devmgr starts this driver)\n");
        return 2;
    }
    if (argc > 2) iface = (uint8_t)atoi(argv[2]);
    iobuf = malloc(MAX_XFER * 2);
    if (!iobuf) return 1;

    // INQUIRY: what it is, removable or not
    uint8_t inq[36] = { 0 }, cdb[16] = { 0 };
    cdb[0] = 0x12;
    cdb[4] = 36;
    if (command(cdb, 6, inq, 36, 1, 1) < 0) {
        // One reset and another try: some devices need it after configuration
        reset_recovery();
        if (command(cdb, 6, inq, 36, 1, 1) < 0) {
            fprintf(stderr, "usb-storage %s: INQUIRY failed\n", where);
            return 1;
        }
    }
    int type = inq[0] & 0x1F, removable = (inq[1] & 0x80) != 0;
    if (type != 0 && type != 0x0E) {   // direct access (or simplified) only
        fprintf(stderr, "usb-storage %s: SCSI device type %#x is not a disk; ignored\n", where, type);
        for (;;) sleep_ms(3600 * 1000); // stay, so devmgr does not restart us
    }
    char vendor[9], product[17];
    memcpy(vendor, inq + 8, 8);
    vendor[8] = 0;
    memcpy(product, inq + 16, 16);
    product[16] = 0;
    trim(vendor);
    trim(product);

    // TEST UNIT READY until the medium is there (a few seconds at most)
    int ready = 0;
    for (int k = 0; k < 20 && !ready; k++) {
        memset(cdb, 0, sizeof cdb);
        if (command(cdb, 6, 0, 0, 0, 1) == 0) ready = 1;
        else if (is_medium_error()) break;
        else sleep_ms(250);
    }
    if (!ready) {
        fprintf(stderr, "usb-storage %s: %s %s: no medium\n", where, vendor, product);
        for (;;) sleep_ms(3600 * 1000);   // card readers: wait (no hot-swap of cards yet)
    }

    // READ CAPACITY(10), or (16) for 2 TiB and more
    uint8_t cap[32] = { 0 };
    memset(cdb, 0, sizeof cdb);
    cdb[0] = 0x25;
    if (command(cdb, 10, cap, 8, 1, 1) < 0) {
        fprintf(stderr, "usb-storage %s: READ CAPACITY failed\n", where);
        return 1;
    }
    blocks = (uint64_t)be32(cap) + 1;
    block_size = be32(cap + 4);
    if (be32(cap) == 0xFFFFFFFFu) {
        memset(cdb, 0, sizeof cdb);
        cdb[0] = 0x9E;
        cdb[1] = 0x10;
        cdb[13] = 32;
        if (command(cdb, 16, cap, 32, 1, 1) < 0) {
            fprintf(stderr, "usb-storage %s: READ CAPACITY(16) failed\n", where);
            return 1;
        }
        blocks = ((uint64_t)be32(cap) << 32 | be32(cap + 4)) + 1;
        block_size = be32(cap + 8);
    }
    if (block_size < 512 || block_size > 4096 || (block_size & (block_size - 1))) {
        fprintf(stderr, "usb-storage %s: unsupported block size %u\n", where, block_size);
        return 1;
    }

    // MODE SENSE(6): the write-protect switch (not every device answers)
    uint8_t ms[192] = { 0 };
    memset(cdb, 0, sizeof cdb);
    cdb[0] = 0x1A;
    cdb[2] = 0x3F;
    cdb[4] = 192;
    int protect = command(cdb, 6, ms, 192, 1, 1) == 0 && (ms[2] & 0x80);

    rx_block_info_t info = { 0 };
    strcpy(info.name, "sd@");
    info.block_count = blocks;
    info.block_size = block_size;
    info.flags = (removable ? RX_BLOCK_FLAG_REMOVABLE : 0) | (protect ? RX_BLOCK_FLAG_READONLY : 0);
    strcpy(info.driver, "usb-storage");
    snprintf(info.model, sizeof info.model, "%s%s%s", vendor, vendor[0] && product[0] ? " " : "", product);
    rx_blockdev_ops_t ops = { rd, protect ? 0 : wr, fl };
    int r = rx_blockdev_publish(&ops, 0, &info, MAX_XFER / block_size, 0);
    if (r < 0) {
        fprintf(stderr, "usb-storage %s: cannot publish the disk: %s\n", where, strerror(-r));
        return 1;
    }
    return rx_blockdev_run();
}

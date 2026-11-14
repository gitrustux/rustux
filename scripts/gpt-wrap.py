#!/usr/bin/env python3
# Copyright 2026 The Rustux Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT
"""Put filesystem images into a GPT disk image (for the boot tests):

    scripts/gpt-wrap.py [--disk D] OUT TYPE:IMAGE [TYPE:IMAGE ...]

TYPE is esp, linux or msdata. Each partition starts on a 1 MiB boundary;
partition k gets the GUID 22222222-0000-4000-8000-0000000000kk (k = 1, 2,
...; with --disk D: 22222222-0000-4000-8000-DDDDDD0000kk), so tests know it."""
import struct, sys, uuid, zlib

BS = 512
TYPES = {"esp": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "linux": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
         "msdata": "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"}


def header(lba, alt, first, last, disk_guid, entries_lba, entries_crc):
    h = struct.pack("<8sIIIIQQQQ16sQIII", b"EFI PART", 0x10000, 92, 0, 0, lba, alt, first, last,
                    disk_guid, entries_lba, 128, 128, entries_crc)
    crc = zlib.crc32(h) & 0xFFFFFFFF
    return h[:16] + struct.pack("<I", crc) + h[20:] + bytes(BS - len(h))


def main():
    args = sys.argv[1:]
    disk_no = 0
    if args[0] == "--disk":
        disk_no, args = int(args[1]), args[2:]
    out, specs = args[0], args[1:]
    parts, lba = [], 2048
    for k, spec in enumerate(specs):
        t, path = spec.split(":", 1)
        data = open(path, "rb").read()
        n = (len(data) + BS - 1) // BS
        parts.append((TYPES[t], lba, lba + n - 1, data, k + 1))
        lba = (lba + n + 2047) // 2048 * 2048
    blocks = lba + 2048
    disk = bytearray(blocks * BS)
    entries = bytearray(128 * 128)
    for i, (t, first, last, data, k) in enumerate(parts):
        guid = uuid.UUID("22222222-0000-4000-8000-%06d%06d" % (disk_no, k)).bytes_le
        entries[i * 128:(i + 1) * 128] = (uuid.UUID(t).bytes_le + guid + struct.pack("<QQQ", first, last, 0)
                                          + ("part%d" % k).encode("utf-16-le").ljust(72, b"\0"))
        disk[first * BS:first * BS + len(data)] = data
    ecrc = zlib.crc32(entries) & 0xFFFFFFFF
    dguid = uuid.UUID("5A7C0FFE-0000-4321-ABCD-%012d" % disk_no).bytes_le  # each disk its own
    disk[BS:2 * BS] = header(1, blocks - 1, 34, blocks - 34, dguid, 2, ecrc)
    disk[2 * BS:2 * BS + len(entries)] = entries
    disk[(blocks - 33) * BS:(blocks - 1) * BS] = entries
    disk[(blocks - 1) * BS:] = header(blocks - 1, 1, 34, blocks - 34, dguid, blocks - 33, ecrc)
    mbr = bytearray(BS)
    mbr[446:462] = bytes([0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF]) + struct.pack("<II", 1, min(blocks - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xAA"
    disk[0:BS] = mbr
    open(out, "wb").write(disk)


main()

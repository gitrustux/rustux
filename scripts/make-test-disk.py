#!/usr/bin/env python3
# Copyright 2026 The Rustux Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT

"""Test disks for the disk drivers (step 13, docs/TESTS.md).

    make-test-disk.py make DISK.img        a 64 MiB GPT disk shaped like a
                                           Windows laptop's, plus a Linux
                                           partition, free space at the end
    make-test-disk.py mbr DISK.img         a 16 MiB disk with an MBR table
    make-test-disk.py check DISK.img       verify a GPT (both copies, CRCs)
                                           and list it, e.g. after Rustux
                                           wrote to it

An implementation of its own (not Rustux's C code), so each side checks
the other. Needs python3; mkfs.vfat and mkfs.ext4 if present (else those
partitions get only a recognisable signature).
"""

import os
import shutil
import struct
import subprocess
import sys
import tempfile
import uuid
import zlib

BS = 512
EFI = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
MSRES = "E3C9E316-0B5C-4DB8-817D-F92DF00215AE"
MSDATA = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"
LINUX = "0FC63DAF-8483-4772-8E79-3D69D8477DE4"
NAMES = {EFI: "EFI System", MSRES: "Microsoft reserved", MSDATA: "Microsoft basic data", LINUX: "Linux filesystem"}


def guid_bytes(s):
    return uuid.UUID(s).bytes_le


def guid_str(b):
    return str(uuid.UUID(bytes_le=bytes(b))).upper()


def header(lba, alt, first, last, disk_guid, entries_lba, entries_crc):
    h = struct.pack("<8sIIIIQQQQ16sQIII", b"EFI PART", 0x10000, 92, 0, 0, lba, alt, first, last,
                    disk_guid, entries_lba, 128, 128, entries_crc)
    crc = zlib.crc32(h) & 0xFFFFFFFF
    h = h[:16] + struct.pack("<I", crc) + h[20:]
    return h + bytes(BS - len(h))


def fs_image(kind, blocks, label):
    """A filesystem made by the host's tools, or just its signature"""
    data = bytearray(blocks * BS)
    tool = {"vfat": "mkfs.vfat", "ext4": "mkfs.ext4"}.get(kind)
    if tool and shutil.which(tool):
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.truncate(blocks * BS)
            name = f.name
        if kind == "vfat":
            cmd = [tool, "-n", label, "-i", "1234ABCD", name]
        else:
            cmd = [tool, "-q", "-F", "-L", label, "-U", "6f0e9a3c-1f2b-4c5d-8e9f-0a1b2c3d4e5f", name]
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        with open(name, "rb") as f:
            data = bytearray(f.read())
        os.unlink(name)
    elif kind == "ntfs":
        # A boot sector with NTFS's signature and a serial number
        data[0:3] = b"\xEB\x52\x90"
        data[3:11] = b"NTFS    "
        data[0x48:0x50] = struct.pack("<Q", 0x0123456789ABCDEF)
        data[510:512] = b"\x55\xAA"
    return data


def make(path):
    size = 64 << 20
    blocks = size // BS
    disk = bytearray(size)
    first_usable, last_usable = 34, blocks - 34
    parts = [  # type, first, last, name, filesystem, fs label
        (EFI, 2048, 2048 + 16 * 2048 - 1, "EFI system partition", "vfat", "SYSTEM"),
        (MSRES, 34816, 34816 + 2048 - 1, "Microsoft reserved partition", None, None),
        (MSDATA, 36864, 36864 + 8 * 2048 - 1, "Basic data partition", "ntfs", None),
        (LINUX, 53248, 53248 + 16 * 2048 - 1, "rustica", "ext4", "rustica-root"),
    ]   # free: blocks 86016 to the end (about 22 MiB)
    entries = bytearray(128 * 128)
    for i, (t, first, last, name, fs, label) in enumerate(parts):
        guid = uuid.UUID(int=(0x11111111111141118111111111111100 + i + 1)).bytes_le
        e = guid_bytes(t) + guid + struct.pack("<QQQ", first, last, 0) + name.encode("utf-16-le").ljust(72, b"\0")
        entries[i * 128:(i + 1) * 128] = e
        if fs:
            img = fs_image(fs, last - first + 1, label)
            disk[first * BS:first * BS + len(img)] = img
    ecrc = zlib.crc32(entries) & 0xFFFFFFFF
    dguid = uuid.UUID("5A7C0FFE-1234-4321-ABCD-0123456789AB").bytes_le
    disk[BS:2 * BS] = header(1, blocks - 1, first_usable, last_usable, dguid, 2, ecrc)
    disk[2 * BS:2 * BS + len(entries)] = entries
    disk[(blocks - 33) * BS:(blocks - 1) * BS] = entries
    disk[(blocks - 1) * BS:] = header(blocks - 1, 1, first_usable, last_usable, dguid, blocks - 33, ecrc)
    mbr = bytearray(BS)
    mbr[446:462] = bytes([0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF]) + struct.pack("<II", 1, blocks - 1)
    mbr[510:512] = b"\x55\xAA"
    disk[0:BS] = mbr
    with open(path, "wb") as f:
        f.write(disk)
    print(f"{path}: 64 MiB GPT test disk; free space from block 86016")


def make_mbr(path):
    size = 16 << 20
    disk = bytearray(size)
    mbr = bytearray(BS)
    for i, (typ, first, count) in enumerate([(0x0C, 2048, 8192), (0x83, 10240, 22528)]):
        mbr[446 + 16 * i:446 + 16 * (i + 1)] = bytes([0x80 if i == 0 else 0, 0, 0, 0, typ, 0, 0, 0]) + struct.pack("<II", first, count)
    mbr[440:444] = struct.pack("<I", 0xDEADBEEF)
    mbr[510:512] = b"\x55\xAA"
    disk[0:BS] = mbr
    img = fs_image("vfat", 8192, "STICK")
    disk[2048 * BS:2048 * BS + len(img)] = img
    with open(path, "wb") as f:
        f.write(disk)
    print(f"{path}: 16 MiB MBR test disk (FAT 4 MiB, Linux 11 MiB)")


def read_copy(d, lba, blocks):
    h = d[lba * BS:(lba + 1) * BS]
    if h[:8] != b"EFI PART":
        return None, "no signature"
    hsize, hcrc = struct.unpack_from("<II", h, 12)
    raw = bytearray(h[:hsize])
    raw[16:20] = b"\0\0\0\0"
    if zlib.crc32(raw) & 0xFFFFFFFF != hcrc:
        return None, "header CRC"
    my, alt, first, last = struct.unpack_from("<QQQQ", h, 24)
    dguid = h[56:72]
    elba, count, esize, ecrc = struct.unpack_from("<QIII", h, 72)
    if my != lba:
        return None, "wrong own LBA"
    e = d[elba * BS:elba * BS + count * esize]
    if zlib.crc32(e) & 0xFFFFFFFF != ecrc:
        return None, "entries CRC"
    parts = []
    for i in range(count):
        x = e[i * esize:(i + 1) * esize]
        if x[:16] == bytes(16):
            continue
        f, l = struct.unpack_from("<QQ", x, 32)
        name = x[56:128].decode("utf-16-le").rstrip("\0")
        parts.append((i + 1, guid_str(x[:16]), guid_str(x[16:32]), f, l, name))
    return (guid_str(dguid), alt, first, last, parts), None


def check(path):
    with open(path, "rb") as f:
        d = f.read()
    blocks = len(d) // BS
    if d[510:512] != b"\x55\xAA" or d[446 + 4] != 0xEE:
        print("protective MBR: missing")
        return 1
    p, err1 = read_copy(d, 1, blocks)
    b, err2 = read_copy(d, blocks - 1, blocks)
    print(f"primary: {'ok' if p else err1}; backup: {'ok' if b else err2}")
    if not p or not b:
        return 1
    if p[0] != b[0] or p[4] != b[4]:
        print("primary and backup differ")
        return 1
    print(f"disk GUID {p[0]}, usable {p[2]}-{p[3]}")
    for n, t, g, f, l, name in p[4]:
        print(f"{n:3} {f:8} {l:8} {NAMES.get(t, t):22} {name}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("make", "mbr", "check"):
        print(__doc__)
        sys.exit(2)
    sys.exit({"make": make, "mbr": make_mbr, "check": check}[sys.argv[1]](sys.argv[2]) or 0)

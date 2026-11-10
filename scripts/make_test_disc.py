#!/usr/bin/env python3
"""Create a tiny ISO-9660 homebrew disc using only Python's standard library.

Usage: python3 scripts/make_test_disc.py demo.exe demo.iso
The input must be a self-contained PS-X EXE. No Sony/game data is included.
The resulting data-only ISO is for emulator tests, not for burning a retail disc.
"""
import argparse
from pathlib import Path
import struct

SECTOR = 2048


def dual16(value):
    return struct.pack("<H", value) + struct.pack(">H", value)


def dual32(value):
    return struct.pack("<I", value) + struct.pack(">I", value)


def record(name, lba, length, directory=False):
    size = 33 + len(name) + (len(name) % 2 == 0)
    data = bytearray(size)
    data[0] = size
    data[2:10] = dual32(lba)
    data[10:18] = dual32(length)
    data[18:25] = bytes([126, 1, 1, 0, 0, 0, 0])
    data[25] = 2 if directory else 0
    data[28:32] = dual16(1)
    data[32] = len(name)
    data[33:33 + len(name)] = name
    return data


def build(exe):
    if len(exe) < 2048 or exe[:8] != b"PS-X EXE":
        raise ValueError("Input is not a PS-X EXE")
    # Root directory 20, SYSTEM.CNF 21, TEST.EXE 22 onwards.
    sectors = max(300, 22 + (len(exe) + SECTOR - 1) // SECTOR)
    image = bytearray(sectors * SECTOR)
    pvd = bytearray(SECTOR)
    pvd[:7] = b"\x01CD001\x01"
    pvd[8:40] = b"PLAYSTATION".ljust(32)
    pvd[40:72] = b"PS1_GPUI_HOMEBREW".ljust(32)
    pvd[80:88] = dual32(sectors)
    pvd[120:124] = dual16(1)
    pvd[124:128] = dual16(1)
    pvd[128:132] = dual16(SECTOR)
    pvd[132:140] = dual32(10)
    pvd[140:144] = struct.pack("<I", 18)
    pvd[148:152] = struct.pack(">I", 19)
    pvd[156:190] = record(b"\0", 20, SECTOR, True)
    pvd[881] = 1
    image[16 * SECTOR:17 * SECTOR] = pvd
    image[17 * SECTOR:17 * SECTOR + 7] = b"\xffCD001\x01"
    image[18 * SECTOR:18 * SECTOR + 10] = b"\x01\x00" + struct.pack("<IH", 20, 1) + b"\x00\x00"
    image[19 * SECTOR:19 * SECTOR + 10] = b"\x01\x00" + struct.pack(">IH", 20, 1) + b"\x00\x00"
    config = b"BOOT = cdrom:\\TEST.EXE;1\r\nTCB = 4\r\nEVENT = 16\r\nSTACK = 801FFF00\r\n"
    root = (record(b"\0", 20, SECTOR, True) + record(b"\1", 20, SECTOR, True)
            + record(b"SYSTEM.CNF;1", 21, len(config)) + record(b"TEST.EXE;1", 22, len(exe)))
    image[20 * SECTOR:20 * SECTOR + len(root)] = root
    image[21 * SECTOR:21 * SECTOR + len(config)] = config
    image[22 * SECTOR:22 * SECTOR + len(exe)] = exe
    return image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("exe", type=Path)
    parser.add_argument("iso", type=Path)
    parser.add_argument("--static-demo", action="store_true", help="Make the exported built-in demo draw once and idle")
    args = parser.parse_args()
    exe = bytearray(args.exe.read_bytes())
    if args.static_demo:
        # Recognize the original demo's completion-counter store, then replace its loop.
        words = struct.unpack("<" + "I" * ((len(exe) - 2048) // 4), exe[2048:])
        index = words.index(0xAC100100) + 1
        pc = struct.unpack_from("<I", exe, 0x10)[0] + index * 4
        struct.pack_into("<I", exe, 2048 + index * 4, 0x08000000 | ((pc >> 2) & 0x03FFFFFF))
    image = build(exe)
    with args.iso.open("xb") as output:
        output.write(image)
    print(f"Created {args.iso}: {len(image) // SECTOR} sectors, original homebrew only")


if __name__ == "__main__":
    main()

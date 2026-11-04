#!/usr/bin/env python3
"""Writes the two test icons shell32_test.rc embeds (deterministic; run
by build.sh): one.ico (16x16 and 32x32, 32 bits per pixel, the documented
ICO layout: ICONDIR, ICONDIRENTRYs, then BITMAPINFOHEADER + XOR pixels +
AND mask per image) and two.ico (32x32 only)."""
import struct, sys, os

def image(size, colour):
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = struct.pack("<I", colour) * (size * size)
    mask_row = ((size + 31) // 32) * 4
    mask = b"\0" * (mask_row * size)
    return header + pixels + mask

def ico(path, images):
    count = len(images)
    out = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    blobs = []
    for size, colour in images:
        data = image(size, colour)
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        blobs.append(data)
    with open(path, "wb") as f:
        f.write(out + b"".join(blobs))

here = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
ico(os.path.join(here, "one.ico"), [(16, 0xff2060c0), (32, 0xffc06020)])
ico(os.path.join(here, "two.ico"), [(32, 0xff20c060)])

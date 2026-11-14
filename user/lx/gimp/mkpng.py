import struct, zlib, sys
w, h = 64, 48
rows = []
for y in range(h):
    row = bytearray([0])
    for x in range(w):
        r = (x * 4) & 255
        g = (y * 5) & 255
        b = 255 if (x // 8 + y // 8) % 2 else 32
        if 20 <= x < 44 and 14 <= y < 34:
            r, g, b = 250, 200, 40
        row += bytes([r, g, b])
    rows.append(bytes(row))
raw = b"".join(rows)
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)

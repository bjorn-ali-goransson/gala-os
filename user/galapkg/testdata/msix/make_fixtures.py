#!/usr/bin/env python3
"""Makes the MSIX/AppX test packages of galapkg's MSIX reader
(src/msix.rs, src/blockmap.rs; ARCHITECTURE "Installing software").

Run: python3 -I user/galapkg/testdata/msix/make_fixtures.py
Then: sh user/galapkg/testdata/msix/check_fixtures.sh (unzip, zipinfo,
7z and sha256sum cross-check the result independently).

makeappx is Windows-only, so the packages are written here from
Microsoft's documentation alone: the AppxBlockMap schema (64 KiB blocks
of the uncompressed data, each hashed with the BlockMap's HashMethod and
written in base64; a Size per block only for deflated files, the number
of compressed bytes of that block; File Size = the uncompressed size,
LfhSize = the size of the file's zip local header), the package
manifest and bundle manifest schema references, and OPC's
[Content_Types].xml (ECMA-376 Part 2). The zip container is written by
hand the ZIP64 way with data descriptors (PKWARE APPNOTE), each 64 KiB
block of a deflated file compressed as its own byte-aligned run (a full
flush after each block) so that a block's Size means something.
zipfile.msix instead uses Python's own zipfile module (classic zip, all
stored), an independent writer.

AppxSignature.p7x here is a placeholder (not a PKCS#7 signature): the
reader only notes its presence.

Python 3 standard library only; output is deterministic for a given
zlib version (other zlib versions may compress differently: the
fixtures are checked in, so tests do not depend on regeneration).
"""

import base64
import hashlib
import os
import struct
import sys
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "src")
BLOCK = 65536
BLOCKMAP_NS = "http://schemas.microsoft.com/appx/2010/blockmap"
SHA256_URI = "http://www.w3.org/2001/04/xmlenc#sha256"
DOS_TIME, DOS_DATE = 0, ((2026 - 1980) << 9) | (11 << 5) | 3  # 2026-11-03

FOOTPRINT_UNLISTED = {"AppxBlockMap.xml", "[Content_Types].xml", "AppxSignature.p7x"}


def read_src(name):
    with open(os.path.join(SRC, name), "rb") as f:
        return f.read()


def part_name(name):
    """A file name (backslashes) as an OPC part name's zip item name
    (ECMA-376 Part 2: '/' separators, characters outside the URI set
    percent-encoded as UTF-8)."""
    out = []
    for ch in name.replace("\\", "/"):
        if ch.isascii() and (ch.isalnum() or ch in "-._~/!$&'()*+,;=:@[]"):
            out.append(ch)
        else:
            out.extend("%%%02X" % b for b in ch.encode("utf-8"))
    return "".join(out)


def blocks(data):
    return [data[i:i + BLOCK] for i in range(0, len(data), BLOCK)]


def deflate_blocks(data):
    """Each 64 KiB block deflated as its own byte-aligned run (raw
    deflate, a full flush after each block, the stream finished after
    the last): returns the runs."""
    c = zlib.compressobj(6, zlib.DEFLATED, -15, 9)
    runs = []
    bl = blocks(data)
    for i, b in enumerate(bl):
        last = i == len(bl) - 1
        runs.append(c.compress(b) + c.flush(zlib.Z_FINISH if last else zlib.Z_FULL_FLUSH))
    return runs


class Entry:
    def __init__(self, name, data, deflate):
        self.name = name  # as in the package (backslashes)
        self.zname = part_name(name)
        self.data = data
        self.deflate = deflate and len(data) > 0
        self.runs = deflate_blocks(data) if self.deflate else None
        self.stored = b"".join(self.runs) if self.deflate else data
        self.lfh_size = 30 + len(self.zname.encode()) + 20


def zip64_archive(entries):
    """A ZIP64 archive with data descriptors: local headers carry no
    sizes (zip64 extra field of zeros), the central directory carries
    every size and offset in its zip64 extra field."""
    out = bytearray()
    central = bytearray()
    for e in entries:
        offset = len(out)
        crc = zlib.crc32(e.data)
        method = 8 if e.deflate else 0
        zname = e.zname.encode()
        out += struct.pack("<IHHHHHIIIHH", 0x04034B50, 45, 0x0008, method, DOS_TIME, DOS_DATE,
                           0, 0xFFFFFFFF, 0xFFFFFFFF, len(zname), 20)
        out += zname + struct.pack("<HHQQ", 1, 16, 0, 0)
        assert len(out) - offset == e.lfh_size
        e.data_offset = len(out)
        out += e.stored
        out += struct.pack("<IIQQ", 0x08074B50, crc, len(e.stored), len(e.data))
        central += struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 45, 45, 0x0008, method, DOS_TIME,
                               DOS_DATE, crc, 0xFFFFFFFF, 0xFFFFFFFF, len(zname), 28, 0, 0, 0, 0,
                               0xFFFFFFFF)
        central += zname + struct.pack("<HHQQQ", 1, 24, len(e.data), len(e.stored), offset)
    cd_offset = len(out)
    out += central
    z64 = len(out)
    n = len(entries)
    out += struct.pack("<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0, n, n, len(central), cd_offset)
    out += struct.pack("<IIQI", 0x07064B50, 0, z64, 1)
    out += struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0)
    return bytes(out)


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace('"', "&quot;")


def block_map(entries, tweak=None):
    """AppxBlockMap.xml for the entries (footprint files excluded).
    `tweak(entry, attrs, block_list)` may falsify it."""
    lines = ['<?xml version="1.0" encoding="UTF-8"?>',
             f'<BlockMap xmlns="{BLOCKMAP_NS}" HashMethod="{SHA256_URI}">']
    for e in entries:
        if e.name in FOOTPRINT_UNLISTED:
            continue
        attrs = {"Name": e.name, "Size": str(len(e.data)), "LfhSize": str(e.lfh_size)}
        bl = []
        for i, b in enumerate(blocks(e.data)):
            h = {"Hash": base64.b64encode(hashlib.sha256(b).digest()).decode()}
            if e.deflate:
                h["Size"] = str(len(e.runs[i]))
            bl.append(h)
        if tweak:
            tweak(e, attrs, bl)
        head = " ".join(f'{k}="{esc(v)}"' for k, v in attrs.items())
        if not bl:
            lines.append(f"<File {head}/>")
            continue
        lines.append(f"<File {head}>")
        for h in bl:
            lines.append("<Block " + " ".join(f'{k}="{v}"' for k, v in h.items()) + "/>")
        lines.append("</File>")
    lines.append("</BlockMap>")
    return ("\r\n".join(lines) + "\r\n").encode()


def content_types(names, overrides):
    exts = sorted({n.rsplit(".", 1)[1].lower() for n in names
                   if "." in n and n not in overrides and n != "[Content_Types].xml"})
    types = {"exe": "application/x-msdownload", "png": "image/png", "txt": "text/plain",
             "bin": "application/octet-stream", "msix": "application/vnd.ms-appx",
             "dll": "application/x-msdownload"}
    lines = ['<?xml version="1.0" encoding="UTF-8"?>',
             '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">']
    for x in exts:
        lines.append(f'<Default Extension="{x}" ContentType="{types.get(x, "application/octet-stream")}"/>')
    for n, t in overrides.items():
        if n in names:
            lines.append(f'<Override PartName="/{part_name(n)}" ContentType="{t}"/>')
    lines.append("</Types>")
    return ("\r\n".join(lines) + "\r\n").encode()


OVERRIDES = {
    "AppxManifest.xml": "application/vnd.ms-appx.manifest+xml",
    "AppxBlockMap.xml": "application/vnd.ms-appx.blockmap+xml",
    "AppxSignature.p7x": "application/vnd.ms-appx.signature",
    "AppxMetadata\\AppxBundleManifest.xml": "application/vnd.ms-appx.bundlemanifest+xml",
}

SIGNATURE = b"PKCX" + b"gala placeholder: not a signature\r\n"


def payload(big, blob):
    """The payload files: (name, data, deflate)."""
    exe = b"MZ" + bytes(58) + struct.pack("<I", 64) + b"PE\0\0" + bytes(range(256)) * 6
    png = b"\x89PNG\r\n\x1a\n" + bytes(200)
    files = [("app.exe", exe, True), ("Assets\\My Logo.png", png, False)]
    if big:
        # Text that deflates well, 3 blocks and a bit.
        lines = [f"line {i % 64:02d}: the quick brown fox jumps over the lazy dog\n"
                 for i in range(3000)]
        text = "".join(lines).encode()[: 3 * BLOCK + 1234]
        files.append(("data\\big.txt", text, True))
        files.append(("data\\empty.txt", b"", False))
    if blob:
        # Stored and incompressible-ish: two blocks.
        x, out = 12345, bytearray()
        while len(out) < BLOCK + 300:
            x = (x * 1103515245 + 12345) & 0x7FFFFFFF
            out.append((x >> 16) & 0xFF)
        files.append(("data\\blob.bin", bytes(out), False))
    return files


def package(arch="x64", big=True, blob=False, tweak=None, mutate=None, drop=(), add=(),
            listed_only=(), signed=True):
    """An .msix: payload, manifest, block map, content types, signature.
    `mutate(name, data)` changes a payload file after the block map was
    computed (the zip stays valid); `drop` removes entries after the
    block map is made; `add` adds entries the block map does not list;
    `listed_only` are files the block map lists that are not in the zip."""
    manifest = read_src("AppxManifest.xml").replace(b"{ARCH}", arch.encode())
    files = payload(big, blob) + [("AppxManifest.xml", manifest, True)]
    entries = [Entry(n, d, z) for n, d, z in files]
    bm = block_map(entries + [Entry(n, d, False) for n, d in listed_only], tweak)
    if mutate:
        # The block map keeps the original hashes but the changed data's
        # block Sizes, so only a hash disagrees.
        orig = {e.name: [base64.b64encode(hashlib.sha256(b).digest()).decode()
                         for b in blocks(e.data)] for e in entries}
        entries = [Entry(e.name, mutate(e.name, e.data), e.deflate) for e in entries]

        def keep_hashes(e, attrs, bl):
            for i, h in enumerate(bl):
                h["Hash"] = orig[e.name][i]
        bm = block_map(entries, keep_hashes)
    entries = [e for e in entries if e.name not in drop]
    entries += [Entry(n, d, False) for n, d in add]
    tail = [("AppxBlockMap.xml", bm, True)]
    if signed:
        tail.append(("AppxSignature.p7x", SIGNATURE, False))
    names = [e.name for e in entries] + [n for n, _, _ in tail] + ["[Content_Types].xml"]
    tail.append(("[Content_Types].xml", content_types(names, OVERRIDES), True))
    entries += [Entry(n, d, z) for n, d, z in tail]
    return zip64_archive(entries)


def bundle(packages, bad_offset=False):
    """An .msixbundle of (file name, arch, package bytes), stored."""
    entries = [Entry(n, d, False) for n, _, d in packages]
    # Lay the packages out first: their offsets go into the manifest.
    zip64_archive(entries)
    rows = []
    for (n, arch, d), e in zip(packages, entries):
        off = e.data_offset + (1 if bad_offset and arch == "x86" else 0)
        rows.append(f'    <Package Type="application" Version="1.2.3.0" Architecture="{arch}" '
                    f'FileName="{n}" Offset="{off}" Size="{len(d)}">\n'
                    f'      <Resources><Resource Language="en-us" /></Resources>\n'
                    f'    </Package>')
    manifest = read_src("AppxBundleManifest.xml").replace(b"{PACKAGES}", "\n".join(rows).encode())
    entries.append(Entry("AppxMetadata\\AppxBundleManifest.xml", manifest, True))
    tail = [("AppxBlockMap.xml", block_map(entries), True), ("AppxSignature.p7x", SIGNATURE, False)]
    names = [e.name for e in entries] + [n for n, _, _ in tail] + ["[Content_Types].xml"]
    tail.append(("[Content_Types].xml", content_types(names, OVERRIDES), True))
    entries += [Entry(n, d, z) for n, d, z in tail]
    out = zip64_archive(entries)
    for (n, _, d), e in zip(packages, entries):
        assert out[e.data_offset:e.data_offset + len(d)] == d
    return out


def zipfile_package():
    """A package written by Python's zipfile (classic zip, everything
    stored, so no block has a Size)."""
    import io
    manifest = read_src("AppxManifest.xml").replace(b"{ARCH}", b"neutral")
    files = payload(False, False) + [("AppxManifest.xml", manifest, False)]
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        entries = []
        for n, d, _ in files:
            info = zipfile.ZipInfo(part_name(n), date_time=(2026, 11, 3, 0, 0, 0))
            z.writestr(info, d)
            e = Entry(n, d, False)
            e.lfh_size = 30 + len(part_name(n).encode())  # zipfile writes no extra field
            entries.append(e)
        bm = block_map(entries)
        z.writestr(zipfile.ZipInfo("AppxBlockMap.xml", date_time=(2026, 11, 3, 0, 0, 0)), bm)
        names = [n for n, _, _ in files] + ["AppxBlockMap.xml", "[Content_Types].xml"]
        z.writestr(zipfile.ZipInfo("[Content_Types].xml", date_time=(2026, 11, 3, 0, 0, 0)),
                   content_types(names, OVERRIDES))
    return buf.getvalue()


def main():
    out = {}
    out["app.msix"] = package(blob=True)
    out["zipfile.msix"] = zipfile_package()
    out["unsigned.msix"] = package(big=False, signed=False)

    def flip_block1(name, data):
        if name != "data\\big.txt":
            return data
        b = bytearray(data)
        b[BLOCK + 100] ^= 0x20  # a letter's case, in block 1
        return bytes(b)

    out["bad_hash.msix"] = package(mutate=flip_block1)
    out["missing_file.msix"] = package(drop=("app.exe",))
    out["unlisted_file.msix"] = package(add=(("extra.dll", b"MZ not in the block map"),))

    def wrong_size(e, attrs, bl):
        if e.name == "app.exe":
            attrs["Size"] = str(int(attrs["Size"]) + 1)

    def wrong_lfh(e, attrs, bl):
        if e.name == "Assets\\My Logo.png":
            attrs["LfhSize"] = str(int(attrs["LfhSize"]) - 20)

    def wrong_block_size(e, attrs, bl):
        if e.name == "data\\big.txt":
            bl[2]["Size"] = str(int(bl[2]["Size"]) + 3)

    out["wrong_size.msix"] = package(tweak=wrong_size)
    out["wrong_lfh.msix"] = package(tweak=wrong_lfh)
    out["wrong_block_size.msix"] = package(tweak=wrong_block_size)

    # A block map listing a file the package does not have.
    out["extra_listed.msix"] = package(big=False, listed_only=(("Assets\\ghost.png", b"boo"),))

    x64 = package(arch="x64", big=False)
    x86 = package(arch="x86", big=False)
    pk = [("Gala.TestApp_1.2.3.0_x64.msix", "x64", x64), ("Gala.TestApp_1.2.3.0_x86.msix", "x86", x86)]
    out["app.msixbundle"] = bundle(pk)
    out["bad_offset.msixbundle"] = bundle(pk, bad_offset=True)

    for name, data in out.items():
        with open(os.path.join(HERE, name), "wb") as f:
            f.write(data)
        print(f"{name}: {len(data)} bytes, SHA-256 {hashlib.sha256(data).hexdigest()}")
    big = max(len(d) for d in out.values())
    if big > 100_000:
        sys.exit(f"a fixture is {big} bytes: keep them well under 100 KB")


if __name__ == "__main__":
    main()

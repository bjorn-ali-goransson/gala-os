"""Writes the Setup program an Inno Setup installer carries (what its
setup loader unpacks to %TEMP%\\is-*.tmp\\<name>.tmp and runs), so its
imports can be surveyed before booting (tools/import_survey.py). Never run
the result: it is only read.

The format, from Inno Setup's own source (github.com/jrsoftware/issrc,
read as documentation: Shared.Struct.pas TSetupLdrOffsetTable,
Compression.Base.pas TCompressedBlockReader and TransformCallInstructions,
Compression.LZMA1SmallDecompressor.pas): the loader's offset table
("rDlPtS" + 6 bytes, version, total size, OffsetEXE, UncompressedSizeEXE,
CRCEXE, ...); at OffsetEXE a compressed block (CRC-32 of the header, the
stored size as an Int64, a compressed flag) of CRC-checked 4 KiB chunks
holding LZMA1 data (5 property bytes, then the stream); the result's
relative CALL/JMP targets were made absolute before compression and are
turned back here, and the CRC-32 checked.

usage: python3 -I tools/inno_setup_exe.py <installer.exe> <out.tmp>
(docs/WORKFLOW.md "Installing software")."""
import lzma, struct, sys, zlib

data = open(sys.argv[1], "rb").read()
at = data.find(b"rDlPtS")
if at < 0:
    sys.exit("no setup loader offset table (not an Inno Setup installer?)")
_version, _total, offset_exe, size_exe, crc_exe = struct.unpack_from("<IqqIi", data, at + 12)
p = offset_exe
header_crc = struct.unpack_from("<I", data, p)[0]
stored, _compressed = struct.unpack_from("<q?", data, p + 4)
if zlib.crc32(data[p + 4:p + 13]) != header_crc:
    sys.exit("block header CRC mismatch")
p += 13
end = p + stored
packed = bytearray()
while p < end:
    crc = struct.unpack_from("<I", data, p)[0]
    p += 4
    n = min(4096, end - p)
    chunk = data[p:p + n]
    p += n
    if zlib.crc32(chunk) != crc:
        sys.exit(f"chunk CRC mismatch at {p - n:#x}")
    packed += chunk
props, dict_size = packed[0], int.from_bytes(packed[1:5], "little")
lc, rest = props % 9, props // 9
lzma1 = {"id": lzma.FILTER_LZMA1, "lc": lc, "lp": rest % 5, "pb": rest // 5, "dict_size": dict_size}
out = bytearray(lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=[lzma1]).decompress(
    bytes(packed[5:]), max_length=size_exe))
# TransformCallInstructions, decoding.
i, last = 0, len(out) - 4
while i < last:
    if out[i] in (0xE8, 0xE9):
        i += 1
        if out[i + 3] in (0x00, 0xFF):
            rel = (out[i] | out[i + 1] << 8 | out[i + 2] << 16) - ((i + 4) & 0xFFFFFF)
            rel &= 0xFFFFFFFF
            if rel & 0x800000:
                out[i + 3] ^= 0xFF
            out[i:i + 3] = (rel & 0xFFFFFF).to_bytes(3, "little")
        i += 4
    else:
        i += 1
if zlib.crc32(out) != crc_exe & 0xFFFFFFFF:
    sys.exit("the unpacked Setup's CRC-32 does not match the table's")
open(sys.argv[2], "wb").write(out)
print(f"{sys.argv[2]}: {len(out)} bytes", file=sys.stderr)

"""Writes user/kernel32/src/nls/tables.rs: the Unicode normalization data
kernel32's FoldStringW uses (ARCHITECTURE "Locales and text"), for the
Basic Multilingual Plane, from the Unicode Character Database as Python's
unicodedata module carries it (the version is written into the file):

- DECOMP: every BMP character whose canonical (NFD) or compatibility
  (NFKD) decomposition is not itself, with both, fully decomposed, as
  UTF-16 (Hangul syllables are left out: their decomposition is
  arithmetic, Unicode's chapter 3.12);
- COMPOSE: the primary composites (canonical pairs that NFC composes
  again: no singletons, no composition exclusions), BMP only, sorted by
  (first, second);
- CCC: the non-zero canonical combining classes of BMP characters.

usage: python3 -I tools/nls_tables.py > user/kernel32/src/nls/tables.rs
"""
import unicodedata as ud

HANGUL = range(0xAC00, 0xD7A4)
SURROGATES = range(0xD800, 0xE000)


def utf16(s):
    out = []
    for ch in s:
        c = ord(ch)
        if c >= 0x10000:
            c -= 0x10000
            out += [0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF)]
        else:
            out.append(c)
    return out


data = []
decomp = []
compose = []
ccc = []
for c in range(0x10000):
    if c in HANGUL or c in SURROGATES:
        continue
    ch = chr(c)
    nfd, nfkd = ud.normalize("NFD", ch), ud.normalize("NFKD", ch)
    if nfd != ch or nfkd != ch:
        d, k = utf16(nfd), utf16(nfkd)
        assert len(d) < 256 and len(k) < 256
        entry = [c, len(data), len(d)]
        data += d
        entry += [len(data), len(k)]
        data += k
        decomp.append(entry)
    cc = ud.combining(ch)
    if cc:
        ccc.append((c, cc))
    m = ud.decomposition(ch)
    if m and not m.startswith("<"):
        parts = [int(x, 16) for x in m.split()]
        if len(parts) == 2 and all(p < 0x10000 for p in parts):
            if ud.normalize("NFC", chr(parts[0]) + chr(parts[1])) == ch:
                compose.append((parts[0], parts[1], c))
compose.sort()
assert len(data) < 0x10000

print(f"//! Unicode {ud.unidata_version} normalization data for the BMP, written by")
print("//! tools/nls_tables.py from Python's unicodedata (Unicode's Character")
print("//! Database; THIRD_PARTY.md). Do not edit.")
print()
print("/// (character, canonical start, length, compatibility start, length) into DATA.")
print("pub static DECOMP: [(u16, u16, u8, u16, u8); %d] = [" % len(decomp))
for c, ds, dl, ks, kl in decomp:
    print(f"    (0x{c:04x}, {ds}, {dl}, {ks}, {kl}),")
print("];")
print()
print("/// The decompositions' UTF-16 units.")
print("pub static DATA: [u16; %d] = [" % len(data))
for i in range(0, len(data), 12):
    print("    " + " ".join(f"0x{u:04x}," for u in data[i : i + 12]))
print("];")
print()
print("/// (first, second, composite), sorted.")
print("pub static COMPOSE: [(u16, u16, u16); %d] = [" % len(compose))
for a, b, c in compose:
    print(f"    (0x{a:04x}, 0x{b:04x}, 0x{c:04x}),")
print("];")
print()
print("/// (character, canonical combining class), sorted.")
print("pub static CCC: [(u16, u8); %d] = [" % len(ccc))
for c, cc in ccc:
    print(f"    (0x{c:04x}, {cc}),")
print("];")

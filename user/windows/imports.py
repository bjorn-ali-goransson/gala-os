#!/usr/bin/env python3
"""Which imports of a test program this machine's DLLs do not export.

A program that exits STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) names no
function itself: the loader failed before main. This reads the program's
import directory and each named DLL's export directory from the PE
headers (file format only, no disassembly: docs/CLEAN_ROOM.md) and prints
the imports nothing on this machine exports.

    python imports.py user/bin/gala.exe [...]

An api-ms-win-* or ext-ms-* name is an API set: the real export lives in
another DLL and the apiset schema redirects it, so a miss there is
reported as "apiset" and is not conclusive.
"""

import os
import struct
import sys

SYS32 = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")


class PE:
    def __init__(self, path):
        with open(path, "rb") as f:
            self.d = f.read()
        self.path = path
        if self.d[:2] != b"MZ":
            raise ValueError(f"{path}: not a PE file")
        self.pe = struct.unpack_from("<I", self.d, 0x3C)[0]
        if self.d[self.pe:self.pe + 4] != b"PE\0\0":
            raise ValueError(f"{path}: no PE signature")
        self.nsec, = struct.unpack_from("<H", self.d, self.pe + 6)
        opt = self.pe + 24
        self.magic, = struct.unpack_from("<H", self.d, opt)
        self.plus = self.magic == 0x20B           # PE32+
        ndir_off = opt + (108 if self.plus else 92)
        self.ndir, = struct.unpack_from("<I", self.d, ndir_off)
        self.dirs = opt + (112 if self.plus else 96)
        sec = self.pe + 24 + struct.unpack_from("<H", self.d, self.pe + 20)[0]
        self.sections = []
        for i in range(self.nsec):
            o = sec + i * 40
            # VirtualSize, VirtualAddress, SizeOfRawData, PointerToRawData
            vsz, va, rsz, raw = struct.unpack_from("<IIII", self.d, o + 8)
            self.sections.append((va, max(vsz, rsz), raw))

    def dir(self, i):
        if i >= self.ndir:
            return (0, 0)
        return struct.unpack_from("<II", self.d, self.dirs + i * 8)

    def off(self, va):
        """File offset of a virtual address, or None."""
        for s_va, size, raw in self.sections:
            if s_va <= va < s_va + size:
                return raw + (va - s_va)
        return None

    def cstr(self, va):
        o = self.off(va)
        if o is None:
            return None
        end = self.d.index(b"\0", o)
        return self.d[o:end].decode("ascii", "replace")

    def imports(self):
        """[(dll, [name or '#ordinal', ...]), ...]"""
        va, _ = self.dir(1)
        if not va:
            return []
        out, o = [], self.off(va)
        while True:
            lookup, _, _, name_va, first = struct.unpack_from("<IIIII", self.d, o)
            if not name_va:
                break
            dll = self.cstr(name_va) or "?"
            names, t = [], self.off(lookup or first)
            step = 8 if self.plus else 4
            fmt = "<Q" if self.plus else "<I"
            flag = 1 << (63 if self.plus else 31)
            while True:
                entry, = struct.unpack_from(fmt, self.d, t)
                if not entry:
                    break
                if entry & flag:
                    names.append(f"#{entry & 0xFFFF}")
                else:
                    # Hint/Name table entry: a 2-byte hint, then the name.
                    names.append(self.cstr((entry & 0x7FFFFFFF) + 2) or "?")
                t += step
            out.append((dll, names))
            o += 20
        return out

    def exports(self):
        va, _ = self.dir(0)
        if not va:
            return set()
        o = self.off(va)
        if o is None:
            return set()
        nnames, = struct.unpack_from("<I", self.d, o + 24)
        names_va, = struct.unpack_from("<I", self.d, o + 32)
        t = self.off(names_va)
        if t is None:
            return set()
        out = set()
        for i in range(nnames):
            rva, = struct.unpack_from("<I", self.d, t + i * 4)
            n = self.cstr(rva)
            if n:
                out.add(n)
        return out


_cache = {}


def exports_of(dll):
    key = dll.lower()
    if key not in _cache:
        p = os.path.join(SYS32, dll)
        try:
            _cache[key] = PE(p).exports()
        except (OSError, ValueError):
            _cache[key] = None
    return _cache[key]


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    for path in argv:
        print(f"== {os.path.basename(path)}")
        try:
            pe = PE(path)
        except (OSError, ValueError) as e:
            print(f"   {e}")
            continue
        missing = 0
        for dll, names in pe.imports():
            apiset = dll.lower().startswith(("api-ms-", "ext-ms-"))
            exp = exports_of(dll)
            if exp is None:
                if apiset:
                    # No such file: an api set is a name the loader resolves
                    # through the apiset schema, so this says nothing.
                    print(f"   apiset  {dll} ({len(names)} imports, not checked)")
                else:
                    print(f"   MISSING {dll}: no such DLL ({len(names)} imports)")
                    missing += len(names)
                continue
            gone = [n for n in names if not n.startswith("#") and n not in exp]
            if gone:
                kind = "apiset" if dll.lower().startswith(("api-ms-", "ext-ms-")) else "MISSING"
                for n in gone:
                    print(f"   {kind}  {dll}!{n}")
                missing += len(gone)
        if not missing:
            print("   every named import is exported on this machine")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

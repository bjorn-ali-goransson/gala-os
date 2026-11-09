#!/usr/bin/env python3
"""Makes `selfsigned.cab` for wintrust_test and cab_probe: a cabinet
(Microsoft's CAB format: one folder, one stored file `hello.txt`) with an
Authenticode signature of a code signing certificate whose issuer, a
test root, is not in the signature (as Wine's SelfSignedFile32/64 images
are signed), so WinVerifyTrust answers CERT_E_CHAINING, and S_OK with
WTD_HASH_ONLY_FLAG.

The signature is laid out as Microsoft's signed cabinets are (the CDN's
v64.cab, ofl.cab, releasehistory.cab, read with `openssl asn1parse`):
- the header's reserve area (flag RESERVE_PRESENT, cbCFHeader 20) holds
  00 00 10 00, the signature's offset and its size, and 8 zero bytes
  (Wine's crypt.c: CAB_SIGNINFO); the signature follows the cabinet at
  cbCabinet, its DER padded with zeros to a multiple of 8;
- SpcIndirectDataContent's data is an SpcLink (1.3.6.1.4.1.311.2.1.28)
  holding a moniker (class A6B586D5B4A12466AE05A217DA8E60D6) whose
  serialized data is one attribute, 1.3.6.1.4.1.311.2.5.1, the SHA-256
  of the 4 bytes of cbCabinet;
- the digest (SHA-256) covers the cabinet up to the signature but for
  the header's reserved1 field (bytes 4..8), cbCFHeader (36..38) and
  the reserve area (40..60): what every signed CDN cabinet's digest
  matches (ARCHITECTURE "wintrust: Authenticode").
The signed attributes are those of Wine's images: contentType,
SpcSpOpusInfo (empty), SpcStatementType (individual), messageDigest.

Needs openssl. Keys are made anew on each run, so the checked-in file is
the one of record; rerun only to change the test data:
    python3 -I user/wintrust_test/data/make_cab.py user/wintrust_test/data/selfsigned.cab
"""

import hashlib
import os
import struct
import subprocess
import sys
import tempfile


def der(tag, body):
    n = len(body)
    if n < 0x80:
        size = bytes([n])
    else:
        b = n.to_bytes((n.bit_length() + 7) // 8, "big")
        size = bytes([0x80 | len(b)]) + b
    return bytes([tag]) + size + body


def seq(*parts):
    return der(0x30, b"".join(parts))


def set_of(*parts):
    return der(0x31, b"".join(sorted(parts)))


def oid(text):
    nums = [int(x) for x in text.split(".")]
    body = bytes([40 * nums[0] + nums[1]])
    for n in nums[2:]:
        chunk = [n & 0x7F]
        n >>= 7
        while n:
            chunk.append(0x80 | (n & 0x7F))
            n >>= 7
        body += bytes(reversed(chunk))
    return der(0x06, body)


def octets(b):
    return der(0x04, b)


NULL = b"\x05\x00"
SHA256 = seq(oid("2.16.840.1.101.3.4.2.1"), NULL)


def cabinet(reserve_size):
    """The cabinet without its signature, with the signature's place
    (offset, size) to be filled in: (bytes, offset of the reserve area)."""
    name = b"hello.txt\x00"
    data = b"hello from a signed cabinet\r\n"
    header_len = 36 + 4 + 20
    folders_at = header_len
    files_at = folders_at + 8
    data_at = files_at + 16 + len(name)
    end = data_at + 8 + len(data)
    head = struct.pack(
        "<4sIIIIIBBHHHHH",
        b"MSCF", 0, end, 0, files_at, 0, 3, 1, 1, 1, 0x0004, 0x1234, 0,
    )
    head += struct.pack("<HBB", 20, 0, 0)
    head += struct.pack("<HHII", 0, 0x10, end, reserve_size) + bytes(8)
    folder = struct.pack("<IHH", data_at, 1, 0)
    # 2026-01-01 12:00:00, FILE_ATTRIBUTE_ARCHIVE.
    entry = struct.pack("<IIHHHH", len(data), 0, 0, (46 << 9) | (1 << 5) | 1, 12 << 11, 0x20) + name
    block = struct.pack("<IHH", 0, len(data), len(data)) + data
    cab = head + folder + entry + block
    assert len(cab) == end
    return cab


def digest(cab, sig_at):
    h = hashlib.sha256()
    h.update(cab[0:4])
    h.update(cab[8:36])
    h.update(cab[38:40])
    reserve = struct.unpack_from("<H", cab, 36)[0]
    h.update(cab[40 + reserve:sig_at])
    return h.digest()


def run(*args, **kw):
    subprocess.run(args, check=True, capture_output=True, **kw)


def main(out):
    with tempfile.TemporaryDirectory() as tmp:
        p = lambda n: os.path.join(tmp, n)
        with open(p("ext.cnf"), "w") as f:
            f.write(
                "[root]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n"
                "[leaf]\nbasicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\n"
                "extendedKeyUsage=codeSigning\n"
            )
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", p("root.key"),
            "-out", p("root.pem"), "-subj", "/CN=gala test cabinet root", "-days", "9000",
            "-sha256", "-extensions", "root", "-config", p("ext.cnf"))
        run("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", p("leaf.key"),
            "-out", p("leaf.csr"), "-subj", "/CN=gala test cabinet signer")
        run("openssl", "x509", "-req", "-in", p("leaf.csr"), "-CA", p("root.pem"), "-CAkey",
            p("root.key"), "-set_serial", "0x1a2b3c4d", "-days", "9000", "-sha256",
            "-extfile", p("ext.cnf"), "-extensions", "leaf", "-out", p("leaf.pem"))
        run("openssl", "x509", "-in", p("leaf.pem"), "-outform", "DER", "-out", p("leaf.der"))
        leaf = open(p("leaf.der"), "rb").read()

        # The certificate's issuer and serial, for the SignerInfo: the
        # TBSCertificate's fields after its version and serial.
        def children(b):
            out, i = [], 0
            while i < len(b):
                tag, n = b[i], b[i + 1]
                hl = 2
                if n & 0x80:
                    k = n & 0x7F
                    n = int.from_bytes(b[i + 2:i + 2 + k], "big")
                    hl += k
                out.append(b[i:i + hl + n])
                i += hl + n
            return out

        def body(t):
            n = t[1]
            return t[2 + (n & 0x7F if n & 0x80 else 0):]

        tbs = children(body(children(body(leaf))[0]))
        serial, issuer = tbs[1], tbs[3]

        # Two passes: the signature's size goes into the cabinet's header,
        # which the digest leaves out, so the first pass only measures.
        size = 0
        for _ in range(3):
            cab = cabinet(size)
            sig_at = len(cab)
            moniker_data = set_of(seq(oid("1.3.6.1.4.1.311.2.5.1"),
                                      set_of(octets(hashlib.sha256(cab[8:12]).digest()))))
            link = der(0xA1, octets(bytes.fromhex("A6B586D5B4A12466AE05A217DA8E60D6"))
                       + octets(moniker_data))
            indirect = seq(seq(oid("1.3.6.1.4.1.311.2.1.28"), link),
                           seq(SHA256, octets(digest(cab, sig_at))))
            indirect_body = indirect[2 + (indirect[1] & 0x7F if indirect[1] & 0x80 else 0):]
            attrs = [
                seq(oid("1.2.840.113549.1.9.3"), set_of(oid("1.3.6.1.4.1.311.2.1.4"))),
                seq(oid("1.3.6.1.4.1.311.2.1.12"), set_of(seq())),
                seq(oid("1.3.6.1.4.1.311.2.1.11"), set_of(seq(oid("1.3.6.1.4.1.311.2.1.21")))),
                seq(oid("1.2.840.113549.1.9.4"), set_of(octets(hashlib.sha256(indirect_body).digest()))),
            ]
            signed = set_of(*attrs)
            open(p("attrs.der"), "wb").write(signed)
            run("openssl", "dgst", "-sha256", "-sign", p("leaf.key"), "-out", p("attrs.sig"),
                p("attrs.der"))
            encrypted = open(p("attrs.sig"), "rb").read()
            signer = seq(
                der(0x02, b"\x01"),
                seq(issuer, serial),
                SHA256,
                b"\xa0" + signed[1:],
                seq(oid("1.2.840.113549.1.1.1"), NULL),
                octets(encrypted),
            )
            signed_data = seq(
                der(0x02, b"\x01"),
                set_of(SHA256),
                seq(oid("1.3.6.1.4.1.311.2.1.4"), der(0xA0, indirect)),
                der(0xA0, leaf),
                set_of(signer),
            )
            pkcs7 = seq(oid("1.2.840.113549.1.7.2"), der(0xA0, signed_data))
            pkcs7 += bytes(-len(pkcs7) % 8)
            if len(pkcs7) == size:
                break
            size = len(pkcs7)
        open(out, "wb").write(cab + pkcs7)


if __name__ == "__main__":
    main(sys.argv[1])

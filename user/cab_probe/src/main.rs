//! A probe, not a test: what Windows' WinVerifyTrust (Authenticode,
//! WINTRUST_ACTION_GENERIC_VERIFY_V2) answers for signed cabinets and
//! for copies of them with one thing changed, and which candidate digest
//! is the one their signatures carry. Its output is the expected values
//! of wintrust_test's cabinet cases and of gala's cabinet digest
//! (ARCHITECTURE "wintrust: Authenticode"); it is on no required list.
//!
//! For each cabinet (the arguments' paths: run-all.ps1 downloads
//! Microsoft's v64.cab and releasehistory.cab from its CDN; then the
//! embedded `../wintrust_test/data/selfsigned.cab`, signed by a test
//! certificate whose root is not in the signature) it prints:
//! - the header (Microsoft's CAB format): cbCabinet, flags, cbCFHeader
//!   and the reserve area's signature offset and size (Wine's crypt.c);
//! - CryptSIPRetrieveSubjectGUID's subject;
//! - the digest the signature's SpcIndirectDataContent carries
//!   (CryptMsg* of the PKCS #7 at that offset), and for each candidate
//!   digest (byte ranges of the file, below) whether it matches;
//! - WinVerifyTrust's answer, full (WTD_REVOKE_NONE) and with
//!   WTD_HASH_ONLY_FLAG, for the file as it is and for copies with one
//!   byte of the header changed (each byte through the reserve area), a
//!   byte of the folder entry, of the data, of the signature and of its
//!   padding, 4 bytes appended, the last byte cut, the RESERVE_PRESENT
//!   flag cleared;
//! - and once, WinVerifyTrust of a zeroed WINTRUST_DATA asking only
//!   WTD_STATEACTION_CLOSE, as ODT's setup.exe calls it.
//!
//! Prints `cab_probe: ok` at the end and exits 0; a cabinet that cannot
//! be read is reported and skipped. It runs on gala too (scenario
//! `cab_probe`, the embedded cabinet only), so the two outputs compare
//! line by line.

#![no_std]
#![no_main]

use core::ptr::null;

use ntrt as _;

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> i32;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut u8;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[link(name = "wintrust", kind = "raw-dylib")]
unsafe extern "system" {
    fn WinVerifyTrust(window: usize, action: *const Guid, data: *mut WintrustData) -> i32;
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CryptMsgOpenToDecode(
        encoding: u32,
        flags: u32,
        kind: u32,
        prov: usize,
        recipient: usize,
        stream: usize,
    ) -> usize;
    fn CryptMsgUpdate(msg: usize, data: *const u8, len: u32, last: i32) -> i32;
    fn CryptMsgGetParam(msg: usize, param: u32, index: u32, out: *mut u8, len: *mut u32) -> i32;
    fn CryptMsgClose(msg: usize) -> i32;
}

#[link(name = "bcrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn BCryptHash(
        alg: usize,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        out: *mut u8,
        out_len: u32,
    ) -> u32;
    fn BCryptCreateHash(
        alg: usize,
        hash: *mut usize,
        object: *mut u8,
        object_len: u32,
        secret: *const u8,
        secret_len: u32,
        flags: u32,
    ) -> u32;
    fn BCryptHashData(hash: usize, input: *const u8, len: u32, flags: u32) -> u32;
    fn BCryptFinishHash(hash: usize, out: *mut u8, len: u32, flags: u32) -> u32;
    fn BCryptDestroyHash(hash: usize) -> u32;
}

#[repr(C)]
#[derive(Default)]
struct Guid(u32, u16, u16, [u8; 8]);

/// WINTRUST_ACTION_GENERIC_VERIFY_V2.
const GENERIC_VERIFY_V2: Guid =
    Guid(0x00aa_c56b, 0xcd44, 0x11d0, [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee]);

/// WINTRUST_FILE_INFO.
#[repr(C)]
struct FileInfo {
    size: u32,
    path: *const u16,
    file: Handle,
    known_subject: *const Guid,
}

/// WINTRUST_DATA (with Windows 8's pSignatureSettings).
#[repr(C)]
struct WintrustData {
    size: u32,
    policy_callback: usize,
    sip_client: usize,
    ui_choice: u32,
    revocation: u32,
    union_choice: u32,
    file: *const FileInfo,
    state_action: u32,
    state: usize,
    url_reference: *const u16,
    prov_flags: u32,
    ui_context: u32,
    signature_settings: usize,
}

const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const WTD_HASH_ONLY_FLAG: u32 = 0x200;
/// bcrypt's algorithm pseudo-handles.
const BCRYPT_SHA1_ALG_HANDLE: usize = 0x31;
const BCRYPT_SHA256_ALG_HANDLE: usize = 0x41;
const BCRYPT_SHA384_ALG_HANDLE: usize = 0x51;

struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        let l = Self { buf: [0; 240], n: 0 };
        l.s(b"cab_probe: ")
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn w(mut self, s: &[u16]) -> Self {
        for &c in s.iter().take_while(|&&c| c != 0) {
            self = self.s(&[if c < 0x80 { c as u8 } else { b'?' }]);
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64, digits: usize) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..digits {
            d[2 + i] = b"0123456789abcdef"[((v >> (4 * (digits - 1 - i))) & 0xf) as usize];
        }
        self.s(&d[..2 + digits])
    }

    fn bytes(mut self, b: &[u8]) -> Self {
        for &x in b {
            self = self.s(&[
                b"0123456789abcdef"[usize::from(x >> 4)],
                b"0123456789abcdef"[usize::from(x & 15)],
            ]);
        }
        self
    }

    fn print(self) {
        let line = self.s(b"\r\n");
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                line.buf.as_ptr(),
                line.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn u16_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 2).map_or(0, |x| u64::from(u16::from_le_bytes([x[0], x[1]])))
}

fn u32_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 4).map_or(0, |x| u64::from(u32::from_le_bytes([x[0], x[1], x[2], x[3]])))
}

/// A path in the temporary directory: `<temp>\gala_cab_probe.cab`.
fn temp_path() -> [u16; 300] {
    let mut out = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetTempPathW(260, out.as_mut_ptr()) } as usize;
    for (at, &b) in (n.min(260)..).zip(b"gala_cab_probe.cab") {
        out[at] = u16::from(b);
    }
    out
}

/// Writes the parts in order to the file `path` (made anew).
fn write_file(path: &[u16], parts: &[&[u8]]) -> bool {
    // SAFETY: a NUL-terminated path; our handle and buffers.
    unsafe {
        let h = CreateFileW(path.as_ptr(), GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
        if h == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut ok = true;
        for part in parts {
            let mut written = 0u32;
            ok &= WriteFile(h, part.as_ptr(), part.len() as u32, &mut written, 0) != 0
                && written as usize == part.len();
        }
        CloseHandle(h);
        ok
    }
}

/// The whole file `path` in a process-heap block (never freed).
fn read_file(path: &[u16]) -> Option<&'static [u8]> {
    // SAFETY: a NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(path.as_ptr(), GENERIC_READ, 1, 0, OPEN_EXISTING, 0, 0);
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut size = 0i64;
        let mut out = None;
        if GetFileSizeEx(h, &mut size) != 0 && (1..=64 << 20).contains(&size) {
            let p = HeapAlloc(GetProcessHeap(), 0, size as usize);
            let mut got = 0u32;
            if !p.is_null() && ReadFile(h, p, size as u32, &mut got, 0) != 0 && got as i64 == size {
                out = Some(core::slice::from_raw_parts(p, size as usize));
            }
        }
        CloseHandle(h);
        out
    }
}

/// WinVerifyTrust of `path` (WTD_STATEACTION_IGNORE, no revocation, no
/// UI): the status.
fn verify(path: &[u16], prov_flags: u32) -> u32 {
    let file = FileInfo {
        size: core::mem::size_of::<FileInfo>() as u32,
        path: path.as_ptr(),
        file: 0,
        known_subject: null(),
    };
    let mut data = WintrustData {
        size: core::mem::size_of::<WintrustData>() as u32,
        policy_callback: 0,
        sip_client: 0,
        ui_choice: 2,  // WTD_UI_NONE
        revocation: 0, // WTD_REVOKE_NONE
        union_choice: 1,
        file: &file,
        state_action: 0,
        state: 0,
        url_reference: null(),
        prov_flags,
        ui_context: 0,
        signature_settings: 0,
    };
    // SAFETY: our structures.
    unsafe { WinVerifyTrust(0, &GENERIC_VERIFY_V2, &mut data) as u32 }
}

/// Writes `parts` to the temporary file and prints both answers.
fn case(label: &[u8], what: &[u8], at: u64, parts: &[&[u8]]) {
    let path = temp_path();
    if !write_file(&path, parts) {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        Line::new().s(label).s(b": ").s(what).s(b": cannot write, error ").dec(e.into()).print();
        return;
    }
    let full = verify(&path, 0);
    let hash_only = verify(&path, WTD_HASH_ONLY_FLAG);
    Line::new()
        .s(label)
        .s(b": ")
        .s(what)
        .s(b" @")
        .hex(at, 4)
        .s(b": full ")
        .hex(full.into(), 8)
        .s(b" hash-only ")
        .hex(hash_only.into(), 8)
        .print();
}

/// The DER element at the start of `b`: (header length, body length).
fn der(b: &[u8]) -> Option<(usize, usize)> {
    let first = *b.get(1)?;
    if first < 0x80 {
        return Some((2, usize::from(first)));
    }
    let n = usize::from(first & 0x7f);
    if n == 0 || n > 4 {
        return None;
    }
    let mut v = 0usize;
    for i in 0..n {
        v = (v << 8) | usize::from(*b.get(2 + i)?);
    }
    Some((2 + n, v))
}

/// The children of the DER element at the start of `b`.
fn children(b: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = der(b).and_then(|(h, n)| b.get(h..h + n)).unwrap_or(&[]);
    let mut rest = body;
    core::iter::from_fn(move || {
        let (h, n) = der(rest)?;
        let (item, tail) = rest.split_at_checked(h + n)?;
        rest = tail;
        Some(item)
    })
}

/// The digest of the signature at `sig` (SpcIndirectDataContent's
/// DigestInfo: the algorithm's OID body and the digest), or None.
fn signed_digest(sig: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    let (h, n) = der(sig)?;
    let der_sig = sig.get(..h + n)?;
    // SAFETY: crypt32's documented calls; our buffers (the content is
    // copied into a heap block that is never freed).
    unsafe {
        let msg = CryptMsgOpenToDecode(0x1_0001, 0, 0, 0, 0, 0);
        if msg == 0 {
            return None;
        }
        let mut out = None;
        let mut len = 0u32;
        if CryptMsgUpdate(msg, der_sig.as_ptr(), der_sig.len() as u32, 1) != 0
            && CryptMsgGetParam(msg, 2, 0, core::ptr::null_mut(), &mut len) != 0
        {
            let p = HeapAlloc(GetProcessHeap(), 0, len as usize);
            if !p.is_null() && CryptMsgGetParam(msg, 2, 0, p, &mut len) != 0 {
                out = Some(core::slice::from_raw_parts(p, len as usize));
            }
        }
        CryptMsgClose(msg);
        let content: &'static [u8] = out?;
        // SpcIndirectDataContent ::= SEQUENCE { data, messageDigest
        // DigestInfo ::= SEQUENCE { AlgorithmIdentifier, OCTET STRING } }
        let info = children(content).nth(1)?;
        let mut parts = children(info);
        let alg = parts.next()?;
        let digest = parts.next()?;
        let oid = children(alg).next()?;
        let (oh, on) = der(oid)?;
        let (dh, dn) = der(digest)?;
        Some((oid.get(oh..oh + on)?, digest.get(dh..dh + dn)?))
    }
}

/// A candidate digest: its name and the file's byte ranges it hashes.
type Candidate<'a> = (&'a [u8], &'a [(u64, u64)]);

/// The hash of the byte ranges of `file` with bcrypt's pseudo-handle.
fn hash(file: &[u8], ranges: &[(u64, u64)], alg: usize, out: &mut [u8]) -> bool {
    let mut h = 0usize;
    // SAFETY: bcrypt's documented calls; our buffers.
    unsafe {
        if BCryptCreateHash(alg, &mut h, core::ptr::null_mut(), 0, null(), 0, 0) != 0 {
            return false;
        }
        let mut ok = true;
        for &(start, end) in ranges {
            match file.get(start as usize..end as usize) {
                Some(part) => ok &= BCryptHashData(h, part.as_ptr(), part.len() as u32, 0) == 0,
                None => ok = false,
            }
        }
        ok &= BCryptFinishHash(h, out.as_mut_ptr(), out.len() as u32, 0) == 0;
        BCryptDestroyHash(h);
        ok
    }
}

fn probe(label: &[u8], file: &[u8]) {
    if file.len() < 60 || &file[..4] != b"MSCF" {
        Line::new().s(label).s(b": not a cabinet (").dec(file.len() as u64).s(b" bytes)").print();
        return;
    }
    let cb_cabinet = u32_at(file, 8);
    let flags = u16_at(file, 30);
    let reserve = if flags & 4 != 0 { u16_at(file, 36) } else { 0 };
    let (sig, sig_size) = (u32_at(file, 44), u32_at(file, 48));
    Line::new()
        .s(label)
        .s(b": ")
        .dec(file.len() as u64)
        .s(b" bytes, cbCabinet ")
        .dec(cb_cabinet)
        .s(b", version ")
        .dec(file[25].into())
        .s(b".")
        .dec(file[24].into())
        .s(b", folders ")
        .dec(u16_at(file, 26))
        .s(b", files ")
        .dec(u16_at(file, 28))
        .s(b", flags ")
        .hex(flags, 4)
        .s(b", cbCFHeader ")
        .dec(reserve)
        .s(b", cbCFFolder ")
        .dec(file[38].into())
        .s(b", cbCFData ")
        .dec(file[39].into())
        .s(b", reserve ")
        .bytes(&file[40..52])
        .s(b", signature at ")
        .dec(sig)
        .s(b" size ")
        .dec(sig_size)
        .print();

    let path = temp_path();
    if write_file(&path, &[file]) {
        let mut subject = Guid::default();
        // Looked up, not imported: gala's wintrust has no SIP functions,
        // and the probe runs there too.
        let name: [u16; 9] = [119, 105, 110, 116, 114, 117, 115, 116, 0]; // "wintrust"
        // SAFETY: plain calls; a NUL-terminated name.
        let f = unsafe {
            GetProcAddress(
                GetModuleHandleW(name.as_ptr()),
                c"CryptSIPRetrieveSubjectGUID".as_ptr().cast(),
            )
        };
        type RetrieveGuid = unsafe extern "system" fn(*const u16, Handle, *mut Guid) -> i32;
        if f == 0 {
            Line::new().s(label).s(b": no CryptSIPRetrieveSubjectGUID").print();
        } else {
            // SAFETY: the function's documented signature; a
            // NUL-terminated path; our GUID.
            let ok = unsafe {
                core::mem::transmute::<usize, RetrieveGuid>(f)(path.as_ptr(), 0, &mut subject)
            };
            // SAFETY: plain call.
            let e = if ok == 0 { unsafe { GetLastError() } } else { 0 };
            Line::new()
                .s(label)
                .s(b": CryptSIPRetrieveSubjectGUID ")
                .dec(ok as u64)
                .s(b" error ")
                .hex(e.into(), 8)
                .s(b" subject ")
                .hex(subject.0.into(), 8)
                .s(b"-")
                .hex(subject.1.into(), 4)
                .s(b"-")
                .hex(subject.2.into(), 4)
                .s(b"-")
                .bytes(&subject.3)
                .print();
        }
    }

    let r = 40 + reserve;
    let end = sig.min(file.len() as u64);
    if let Some((oid, digest)) = file.get(sig as usize..).and_then(signed_digest) {
        Line::new()
            .s(label)
            .s(b": signed digest ")
            .bytes(digest)
            .s(b" (algorithm OID ")
            .bytes(oid)
            .s(b")")
            .print();
        let alg = match digest.len() {
            20 => BCRYPT_SHA1_ALG_HANDLE,
            32 => BCRYPT_SHA256_ALG_HANDLE,
            _ => BCRYPT_SHA384_ALG_HANDLE,
        };
        let candidates: [Candidate; 7] = [
            (
                b"A: 0..4, 8..36, 38..40, reserve's end..signature (gala's)",
                &[(0, 4), (8, 36), (38, 40), (r, end)],
            ),
            (b"B: 0..signature", &[(0, end)]),
            (b"C: 0..cbCabinet", &[(0, cb_cabinet)]),
            (b"D: A with reserved1", &[(0, 36), (38, 40), (r, end)]),
            (b"E: A with cbCFHeader", &[(0, 4), (8, 40), (r, end)]),
            (b"F: A with the reserve area", &[(0, 4), (8, 36), (38, end)]),
            (b"G: A to the file's end", &[(0, 4), (8, 36), (38, 40), (r, file.len() as u64)]),
        ];
        for (name, ranges) in candidates {
            let mut out = [0u8; 48];
            let ok = hash(file, ranges, alg, &mut out[..digest.len()]);
            let matches = ok && &out[..digest.len()] == digest;
            Line::new()
                .s(label)
                .s(b": candidate ")
                .s(name)
                .s(if matches {
                    b": matches"
                } else if ok {
                    b": differs"
                } else {
                    b": not computed"
                })
                .print();
        }
        let mut cb = [0u8; 32];
        // SAFETY: our buffers.
        let ok = unsafe {
            BCryptHash(
                BCRYPT_SHA256_ALG_HANDLE,
                null(),
                0,
                file[8..12].as_ptr(),
                4,
                cb.as_mut_ptr(),
                32,
            )
        } == 0;
        if ok {
            Line::new()
                .s(label)
                .s(b": SHA-256 of cbCabinet's 4 bytes (the moniker's 1.3.6.1.4.1.311.2.5.1?) ")
                .bytes(&cb)
                .print();
        }
    } else {
        Line::new()
            .s(label)
            .s(b": no SpcIndirectDataContent digest found at the signature's offset")
            .print();
    }

    case(label, b"as it is", 0, &[file]);
    let mut byte = [0u8; 1];
    let names: [&[u8]; 20] = [
        b"signature 'MSCF'",
        b"signature 'MSCF'",
        b"signature 'MSCF'",
        b"signature 'MSCF'",
        b"reserved1",
        b"cbCabinet",
        b"reserved2",
        b"coffFiles",
        b"reserved3",
        b"versionMinor",
        b"versionMajor",
        b"cFolders",
        b"cFiles",
        b"flags",
        b"setID",
        b"iCabinet",
        b"cbCFHeader",
        b"cbCFFolder",
        b"cbCFData",
        b"reserve area",
    ];
    let field = |at: usize| -> &[u8] {
        let index = match at {
            0..4 => at,
            4..8 => 4,
            8..12 => 5,
            12..16 => 6,
            16..20 => 7,
            20..24 => 8,
            24 => 9,
            25 => 10,
            26..28 => 11,
            28..30 => 12,
            30..32 => 13,
            32..34 => 14,
            34..36 => 15,
            36..38 => 16,
            38 => 17,
            39 => 18,
            _ => 19,
        };
        names[index]
    };
    for at in 0..r as usize {
        byte[0] = file[at] ^ 0x5a;
        case(label, field(at), at as u64, &[&file[..at], &byte, &file[at + 1..]]);
    }
    let mut one = |what: &[u8], at: usize| {
        if at < file.len() {
            byte[0] = file[at] ^ 0x5a;
            case(label, what, at as u64, &[&file[..at], &byte, &file[at + 1..]]);
        }
    };
    one(b"the first folder entry", r as usize);
    one(b"the last byte before the signature", end as usize - 1);
    one(b"the signature's first byte", end as usize);
    one(b"the signature's last byte (padding?)", (sig + sig_size) as usize - 1);
    let flag = [file[30] & !4];
    case(label, b"RESERVE_PRESENT cleared", 30, &[&file[..30], &flag, &file[31..]]);
    case(label, b"4 bytes appended", file.len() as u64, &[file, &[0x11, 0x22, 0x33, 0x44]]);
    case(label, b"the last byte cut", file.len() as u64 - 1, &[&file[..file.len() - 1]]);
    case(label, b"only 'MSCF'", 0, &[&file[..4]]);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.as_ptr()) };
}

/// What ODT's setup.exe calls after verifying its cabinet, three times:
/// WinVerifyTrust with a WINTRUST_DATA that is all zero (cbStruct 0, no
/// file) but dwStateAction, WTD_STATEACTION_CLOSE. Wine, and gala, take
/// a cbStruct that does not reach dwStateAction as WTD_STATEACTION_IGNORE
/// (a verification without a subject); what does Windows answer?
fn zeroed_close() {
    let mut data = WintrustData {
        size: 0,
        policy_callback: 0,
        sip_client: 0,
        ui_choice: 0,
        revocation: 0,
        union_choice: 0,
        file: null(),
        state_action: 2, // WTD_STATEACTION_CLOSE
        state: 0,
        url_reference: null(),
        prov_flags: 0,
        ui_context: 0,
        signature_settings: 0,
    };
    // SAFETY: our structure; plain calls.
    let (r, e) = unsafe {
        SetLastError(0xdead_beef);
        let r = WinVerifyTrust(0, &GENERIC_VERIFY_V2, &mut data) as u32;
        (r, GetLastError())
    };
    Line::new()
        .s(b"zeroed WINTRUST_DATA with WTD_STATEACTION_CLOSE: ")
        .hex(r.into(), 8)
        .s(b", last error ")
        .hex(e.into(), 8)
        .print();
}

/// The command line's arguments after the program's name, each a
/// NUL-terminated path (quotes removed), into `out`.
fn arguments(out: &mut [[u16; 300]; 4]) -> usize {
    // SAFETY: the process's NUL-terminated command line.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let (mut count, mut i, mut first) = (0, 0, true);
    while i < line.len() {
        while i < line.len() && line[i] == u16::from(b' ') {
            i += 1;
        }
        if i >= line.len() {
            break;
        }
        let quoted = line[i] == u16::from(b'"');
        if quoted {
            i += 1;
        }
        let start = i;
        while i < line.len() && line[i] != u16::from(if quoted { b'"' } else { b' ' }) {
            i += 1;
        }
        let word = &line[start..i];
        i += 1;
        if first {
            first = false;
            continue;
        }
        if count < out.len() && word.len() < 299 {
            out[count] = [0; 300];
            out[count][..word.len()].copy_from_slice(word);
            count += 1;
        }
    }
    count
}

fn run() -> Result<(), u32> {
    let mut paths = [[0u16; 300]; 4];
    let n = arguments(&mut paths);
    for path in &paths[..n] {
        let mut label = [0u8; 80];
        let name = path.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
        let mut len = 0;
        for &c in path[name..].iter().take_while(|&&c| c != 0).take(80) {
            label[len] = if c < 0x80 { c as u8 } else { b'?' };
            len += 1;
        }
        match read_file(path) {
            Some(file) => probe(&label[..len], file),
            None => {
                // SAFETY: plain call.
                let e = unsafe { GetLastError() };
                Line::new().w(path).s(b": cannot read, error ").dec(e.into()).print();
            }
        }
    }
    probe(b"selfsigned.cab", include_bytes!("../../wintrust_test/data/selfsigned.cab"));
    zeroed_close();
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(b"ok").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

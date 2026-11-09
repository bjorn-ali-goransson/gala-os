//! wintrust.dll's WinVerifyTrust with WINTRUST_ACTION_GENERIC_VERIFY_V2
//! (Authenticode) and the WTHelper functions over its provider data,
//! which the Office Deployment Tool's setup.exe imports (ROADMAP OF).
//!
//! Expected values, and where they come from:
//! - Wine's conformance tests (dlls/wintrust/tests/softpub.c, run against
//!   Windows), test_wintrust: an empty file is TRUST_E_SUBJECT_FORM_UNKNOWN
//!   and an unsigned image TRUST_E_NOSIGNATURE, each also the last error;
//!   WTD_STATEACTION_CLOSE is S_OK and leaves the last error; the
//!   provider data (WTHelperProvDataFromStateData) has the window handle
//!   passed as hWndParent.
//! - The same file's test_wintrust_digest, with its two images signed by
//!   a certificate of a root that is not in the signature
//!   (`data/selfsigned32.exe`, `data/selfsigned64.exe`: Wine's
//!   SelfSignedFile32/64): as they are, CERT_E_CHAINING, and S_OK with
//!   WTD_HASH_ONLY_FLAG; 4 bytes appended, TRUST_E_NOSIGNATURE (both);
//!   the first 4 bytes replaced, TRUST_E_SUBJECT_FORM_UNKNOWN (hash only:
//!   TRUST_E_NOSIGNATURE); 4 bytes at offset 19 replaced, TRUST_E_BAD_DIGEST
//!   (hash only: TRUST_E_NOSIGNATURE); cut after the DOS header,
//!   TRUST_E_SUBJECT_FORM_UNKNOWN (hash only: TRUST_E_NOSIGNATURE); cut
//!   after the DOS header and twice the NT headers' size,
//!   TRUST_E_NOSIGNATURE (both).
//! - Microsoft's documentation of WinVerifyTrust: an action GUID no
//!   provider serves is TRUST_E_PROVIDER_UNKNOWN; of the WTHelper
//!   functions: an index past the signers or the certificates is NULL.
//! - The Authenticode specification ("Windows Authenticode Portable
//!   Executable Signature Format"): the image hash leaves out the
//!   checksum, the certificate table's directory entry and the table, so
//!   a changed CheckSum still verifies; a changed byte of a section does
//!   not (TRUST_E_BAD_DIGEST, as Wine's offset-19 case).
//! - Real signed images, when present (not committed: the files at their
//!   download locations, which the boot scenario ships to C:\gala): the
//!   Office Deployment Tool's setup.exe (Microsoft Corporation, by
//!   Microsoft Windows Code Signing PCA 2024, from Microsoft Root
//!   Certificate Authority 2010; an RFC 3161 timestamp) and CPython
//!   3.13.1's python.exe (Python Software Foundation, a certificate valid
//!   3-6 December 2024, from Microsoft Identity Verification Root
//!   Certificate Authority 2020; trusted today through its RFC 3161
//!   timestamp of 2024-12-03 20:13:09.973 UTC). Both verify (S_OK) on Windows,
//!   whose root store holds those roots; with WTD_REVOKE_WHOLECHAIN too
//!   (Windows fetches the CRLs; gala has no fetching and its Authenticode
//!   policy accepts an offline revocation check as Windows' default
//!   Software Publishing state, 0x23c00, WTPF_OFFLINEOK_*, does).
//!   setup.exe's chain is 3 certificates, python.exe's 4, the last one a
//!   trusted, self-signed root.
//!
//! Not confirmed on Windows yet (CI's run of this program confirms them):
//! a changed byte in the signer's encrypted digest is taken as
//! NTE_BAD_SIGNATURE, CryptMsgControl's answer (Wine reports
//! TRUST_E_CERT_SIGNATURE); an RFC 3161 timestamp is one counter signer
//! (csCounterSigners 1) and sets the signer's sftVerifyAsOf to its time.
//!
//! With the argument `require` (gala's scenario) the real images must be
//! there; without it (Windows' run from user/bin) their checks are
//! skipped when C:\gala\setup.exe or C:\gala\python.exe is missing, and
//! say so.
//!
//! The same source builds `wow64_wintrust_test`, the 32-bit (x86)
//! program. Prints `wintrust_test: ok` (`wow64_wintrust_test: ok`), or
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ptr::null;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "wintrust_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_wintrust_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
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
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut u8;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "wintrust", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "wintrust", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WinVerifyTrust(window: usize, action: *const Guid, data: *mut WintrustData) -> i32;
    fn WTHelperProvDataFromStateData(state: usize) -> *const ProvData;
    fn WTHelperGetProvSignerFromChain(
        data: *const ProvData,
        signer: u32,
        counter: i32,
        counter_index: u32,
    ) -> *const Signer;
    fn WTHelperGetProvCertFromChain(signer: *const Signer, index: u32) -> *const ProvCert;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "crypt32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "crypt32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CertGetNameStringW(
        cert: usize,
        kind: u32,
        flags: u32,
        para: *const u8,
        name: *mut u16,
        len: u32,
    ) -> u32;
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// WINTRUST_ACTION_GENERIC_VERIFY_V2.
const GENERIC_VERIFY_V2: Guid =
    Guid(0x00aa_c56b, 0xcd44, 0x11d0, [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee]);
/// A GUID no trust provider serves (made up).
const NO_PROVIDER: Guid =
    Guid(0x5c1f_0a77, 0x31d2, 0x4e8b, [0x9a, 0x51, 0x6e, 0x0d, 0x24, 0xb8, 0x7c, 0x13]);

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

/// CRYPT_PROVIDER_DATA through pasSigners.
#[repr(C)]
struct ProvData {
    size: u32,
    wintrust_data: usize,
    opened_file: i32,
    window: usize,
    action: usize,
    prov: usize,
    error: u32,
    reg_security: u32,
    reg_policy: u32,
    functions: usize,
    step_count: u32,
    step_errors: *const u32,
    store_count: u32,
    stores: usize,
    encoding: u32,
    msg: usize,
    signer_count: u32,
    signers: *const Signer,
}

/// CRYPT_PROVIDER_SGNR.
#[repr(C)]
struct Signer {
    size: u32,
    verify_as_of: [u32; 2],
    cert_count: u32,
    certs: *const ProvCert,
    signer_type: u32,
    info: *const SignerInfo,
    error: u32,
    counter_count: u32,
    counters: *const Signer,
    chain: usize,
}

/// CRYPT_PROVIDER_CERT.
#[repr(C)]
struct ProvCert {
    size: u32,
    cert: usize,
    commercial: i32,
    trusted_root: i32,
    self_signed: i32,
    test_cert: i32,
    revoked_reason: u32,
    confidence: u32,
    error: u32,
    trust_list: usize,
    trust_list_signer: i32,
    ctl: usize,
    ctl_error: u32,
    cyclic: i32,
    element: usize,
}

#[repr(C)]
struct Blob {
    size: u32,
    data: *const u8,
}

#[repr(C)]
struct Algorithm {
    oid: *const u8,
    parameters: Blob,
}

/// CMSG_SIGNER_INFO through EncryptedHash.
#[repr(C)]
struct SignerInfo {
    version: u32,
    issuer: Blob,
    serial: Blob,
    hash_algorithm: Algorithm,
    hash_encryption: Algorithm,
    encrypted_hash: Blob,
}

const S_OK: u32 = 0;
const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800b_0001;
const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800b_0003;
const TRUST_E_NOSIGNATURE: u32 = 0x800b_0100;
const CERT_E_CHAINING: u32 = 0x800b_010a;
const TRUST_E_BAD_DIGEST: u32 = 0x8009_6010;
const NTE_BAD_SIGNATURE: u32 = 0x8009_0006;

const WTD_UI_NONE: u32 = 2;
const WTD_REVOKE_NONE: u32 = 0;
const WTD_REVOKE_WHOLECHAIN: u32 = 1;
const WTD_CHOICE_FILE: u32 = 1;
const WTD_STATEACTION_VERIFY: u32 = 1;
const WTD_STATEACTION_CLOSE: u32 = 2;
const WTD_HASH_ONLY_FLAG: u32 = 0x200;

const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const CERT_NAME_SIMPLE_DISPLAY_TYPE: u32 = 4;

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
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

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(text: &[u8]) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, &b) in out.iter_mut().zip(text) {
        *slot = u16::from(b);
    }
    out
}

/// A path in the temporary directory: `<temp>\gala_wt_<name>`.
fn temp_path(name: &[u8]) -> [u16; 300] {
    let mut out = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetTempPathW(260, out.as_mut_ptr()) } as usize;
    let mut at = n.min(260);
    for &b in b"gala_wt_".iter().chain(name) {
        if at < out.len() - 1 {
            out[at] = u16::from(b);
            at += 1;
        }
    }
    out[at] = 0;
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

/// The whole file `path`, in a process-heap block (never freed: the
/// program ends soon), or None if it cannot be read.
fn read_file(path: &[u16]) -> Option<&'static mut [u8]> {
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
                out = Some(core::slice::from_raw_parts_mut(p, size as usize));
            }
        }
        CloseHandle(h);
        out
    }
}

fn new_data(file: *const FileInfo, revocation: u32, prov_flags: u32) -> WintrustData {
    WintrustData {
        size: core::mem::size_of::<WintrustData>() as u32,
        policy_callback: 0,
        sip_client: 0,
        ui_choice: WTD_UI_NONE,
        revocation,
        union_choice: WTD_CHOICE_FILE,
        file,
        state_action: WTD_STATEACTION_VERIFY,
        state: 0,
        url_reference: null(),
        prov_flags,
        ui_context: 0,
        signature_settings: 0,
    }
}

fn new_file(path: &[u16]) -> FileInfo {
    FileInfo {
        size: core::mem::size_of::<FileInfo>() as u32,
        path: path.as_ptr(),
        file: 0,
        known_subject: null(),
    }
}

/// Closes the state of a WTD_STATEACTION_VERIFY call: S_OK, the last
/// error left as it was (Wine's test_wintrust).
fn close(c: &mut Checks, window: usize, data: &mut WintrustData) -> Result<(), u32> {
    data.state_action = WTD_STATEACTION_CLOSE;
    // SAFETY: plain calls; our structures.
    unsafe {
        SetLastError(0xdead_beef);
        let r = WinVerifyTrust(window, &GENERIC_VERIFY_V2, data) as u32;
        c.eq(r, S_OK)?;
        c.eq(GetLastError(), 0xdead_beef)?;
    }
    Ok(())
}

/// WinVerifyTrust of the file `path` (WTD_STATEACTION_VERIFY, then
/// CLOSE), as Wine's call_winverify: the status.
fn verify(c: &mut Checks, path: &[u16], hash_only: bool) -> Result<u32, u32> {
    let file = new_file(path);
    let flags = if hash_only { WTD_HASH_ONLY_FLAG } else { 0 };
    let mut data = new_data(&file, WTD_REVOKE_NONE, flags);
    // SAFETY: our structures.
    let r = unsafe { WinVerifyTrust(0, &GENERIC_VERIFY_V2, &mut data) } as u32;
    close(c, 0, &mut data)?;
    Ok(r)
}

/// Wine's test_wintrust: an empty file, then an unsigned image (this
/// program), with the provider data's window handle.
fn basics(c: &mut Checks) -> Result<(), u32> {
    let empty = temp_path(b"empty.exe");
    c.ok(write_file(&empty, &[]), 0)?;
    let file = new_file(&empty);
    let mut data = new_data(&file, WTD_REVOKE_WHOLECHAIN, 0);
    // SAFETY: plain calls; our structures.
    unsafe {
        SetLastError(0xdead_beef);
        let r = WinVerifyTrust(INVALID_HANDLE_VALUE, &GENERIC_VERIFY_V2, &mut data) as u32;
        c.eq(r, TRUST_E_SUBJECT_FORM_UNKNOWN)?;
        c.eq(GetLastError(), r)?;
    }
    close(c, INVALID_HANDLE_VALUE, &mut data)?;
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(empty.as_ptr()) };

    let mut me = [0u16; 300];
    // SAFETY: our buffer.
    c.ok(unsafe { GetModuleFileNameW(0, me.as_mut_ptr(), 299) } != 0, 0)?;
    for window in [INVALID_HANDLE_VALUE, 0] {
        let file = new_file(&me);
        let mut data = new_data(&file, WTD_REVOKE_WHOLECHAIN, 0);
        // SAFETY: plain calls; our structures; the provider data the
        // verify call keeps until its close.
        unsafe {
            SetLastError(0xdead_beef);
            let r = WinVerifyTrust(window, &GENERIC_VERIFY_V2, &mut data) as u32;
            c.eq(r, TRUST_E_NOSIGNATURE)?;
            c.eq(GetLastError(), r)?;
            let prov = WTHelperProvDataFromStateData(data.state);
            c.ok(!prov.is_null(), 0)?;
            c.ok((*prov).window == window, (*prov).window as u64)?;
            c.eq((*prov).signer_count, 0)?;
            c.ok(WTHelperGetProvSignerFromChain(prov, 0, 0, 0).is_null(), 0)?;
        }
        close(c, window, &mut data)?;
    }

    // An action no provider serves.
    let file = new_file(&me);
    let mut data = new_data(&file, WTD_REVOKE_NONE, 0);
    data.state_action = 0; // WTD_STATEACTION_IGNORE
    // SAFETY: our structures.
    let r = unsafe { WinVerifyTrust(0, &NO_PROVIDER, &mut data) } as u32;
    c.eq(r, TRUST_E_PROVIDER_UNKNOWN)?;
    // WTHelperGetProvCertFromChain of no signer.
    // SAFETY: a NULL signer is answered NULL.
    c.ok(unsafe { WTHelperGetProvCertFromChain(null(), 0) }.is_null(), 0)?;
    Ok(())
}

/// Wine's test_wintrust_digest over one of its two self-signed images.
fn digest_cases(c: &mut Checks, image: &[u8], nt_headers: usize) -> Result<(), u32> {
    const DUMMY: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
    const DOS_HEADER: usize = 64;
    let path = temp_path(b"digest.exe");
    let cases: [(&[&[u8]], u32, u32); 6] = [
        (&[image], CERT_E_CHAINING, S_OK),
        (&[image, &DUMMY], TRUST_E_NOSIGNATURE, TRUST_E_NOSIGNATURE),
        (&[&DUMMY, &image[4..]], TRUST_E_SUBJECT_FORM_UNKNOWN, TRUST_E_NOSIGNATURE),
        (&[&image[..19], &DUMMY, &image[23..]], TRUST_E_BAD_DIGEST, TRUST_E_NOSIGNATURE),
        (&[&image[..DOS_HEADER]], TRUST_E_SUBJECT_FORM_UNKNOWN, TRUST_E_NOSIGNATURE),
        (&[&image[..DOS_HEADER + 2 * nt_headers]], TRUST_E_NOSIGNATURE, TRUST_E_NOSIGNATURE),
    ];
    for (parts, full, hash_only) in cases {
        c.ok(write_file(&path, parts), 0)?;
        let r = verify(c, &path, false)?;
        c.eq(r, full)?;
        let r = verify(c, &path, true)?;
        c.eq(r, hash_only)?;
    }
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.as_ptr()) };
    Ok(())
}

/// Whether the command line asks for the real images (`require`).
fn required() -> bool {
    // SAFETY: the process's NUL-terminated command line.
    unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let line = core::slice::from_raw_parts(p, n);
        let want = wide::<7>(b"require");
        line.windows(7).any(|w| w == &want[..7])
    }
}

fn little(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

/// The PE's (CheckSum offset, certificate table offset, the first
/// section's raw data offset).
fn pe_offsets(image: &[u8]) -> (usize, usize, usize) {
    let nt = little(image, 0x3c);
    let optional = nt + 24;
    let pe32 = image[optional] == 0x0b && image[optional + 1] == 0x01;
    let directories = optional + if pe32 { 96 } else { 112 };
    let table = little(image, directories + 4 * 8);
    let optional_size = usize::from(u16::from_le_bytes([image[nt + 20], image[nt + 21]]));
    let section = optional + optional_size;
    (optional + 64, table, little(image, section + 20))
}

/// A simple display name of a provider certificate as ASCII.
fn cert_name(cert: usize, out: &mut [u8; 80]) -> usize {
    let mut buf = [0u16; 80];
    // SAFETY: a certificate context of the provider data; our buffer.
    let n = unsafe {
        CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), buf.as_mut_ptr(), 80)
    } as usize;
    let n = n.saturating_sub(1).min(80);
    for (o, &w) in out.iter_mut().zip(&buf[..n]) {
        *o = w as u8;
    }
    n
}

/// A real image: verifies, its chain is `chain` certificates long with
/// the end certificate's and the root's names, its signer hashes with
/// `hash_oid`; with `as_of`, the signer's sftVerifyAsOf (its timestamp's
/// time). Returns the signer's encrypted hash (a copy of 16 bytes from
/// its middle) for the corruption cases.
#[allow(clippy::too_many_arguments)]
fn real_image(
    c: &mut Checks,
    path: &[u16],
    chain: u32,
    end_name: &[u8],
    root_name: &[u8],
    hash_oid: &[u8],
    as_of: Option<u64>,
    revocation: u32,
) -> Result<[u8; 16], u32> {
    let file = new_file(path);
    let mut data = new_data(&file, revocation, 0);
    let mut sample = [0u8; 16];
    // SAFETY: plain calls; our structures; the provider data (and what it
    // points to) stays valid until the close below.
    unsafe {
        let r = WinVerifyTrust(0, &GENERIC_VERIFY_V2, &mut data) as u32;
        c.eq(r, S_OK)?;
        let prov = WTHelperProvDataFromStateData(data.state);
        c.ok(!prov.is_null(), 0)?;
        c.eq((*prov).signer_count, 1)?;
        let signer = WTHelperGetProvSignerFromChain(prov, 0, 0, 0);
        c.ok(signer == (*prov).signers, signer as u64)?;
        c.ok(WTHelperGetProvSignerFromChain(prov, 1, 0, 0).is_null(), 0)?;
        let s = &*signer;
        c.eq(s.cert_count, chain)?;
        c.ok(s.chain != 0, 0)?;
        c.ok(!s.info.is_null(), 0)?;
        let info = &*s.info;
        let mut oid = [0u8; 32];
        let mut n = 0;
        while n < 31 && *info.hash_algorithm.oid.add(n) != 0 {
            oid[n] = *info.hash_algorithm.oid.add(n);
            n += 1;
        }
        c.ok(&oid[..n] == hash_oid, n as u64)?;
        let hash = core::slice::from_raw_parts(
            info.encrypted_hash.data,
            info.encrypted_hash.size as usize,
        );
        c.ok(hash.len() >= 64, hash.len() as u64)?;
        let mid = hash.len() / 2;
        sample.copy_from_slice(&hash[mid..mid + 16]);
        let end = WTHelperGetProvCertFromChain(signer, 0);
        c.ok(!end.is_null(), 0)?;
        let mut name = [0u8; 80];
        let k = cert_name((*end).cert, &mut name);
        c.ok(&name[..k] == end_name, k as u64)?;
        let root = WTHelperGetProvCertFromChain(signer, chain - 1);
        c.ok(!root.is_null(), 0)?;
        let k = cert_name((*root).cert, &mut name);
        c.ok(&name[..k] == root_name, k as u64)?;
        c.eq((*root).trusted_root as u32, 1)?;
        c.eq((*root).self_signed as u32, 1)?;
        c.eq((*end).self_signed as u32, 0)?;
        c.ok(WTHelperGetProvCertFromChain(signer, chain).is_null(), 0)?;
        // An RFC 3161 timestamp: one counter signer (not confirmed).
        c.eq(s.counter_count, 1)?;
        let counter = WTHelperGetProvSignerFromChain(prov, 0, 1, 0);
        c.ok(counter == s.counters, counter as u64)?;
        c.ok(WTHelperGetProvSignerFromChain(prov, 0, 1, 1).is_null(), 0)?;
        if let Some(t) = as_of {
            let got = u64::from(s.verify_as_of[0]) | u64::from(s.verify_as_of[1]) << 32;
            c.ok(got == t, got)?;
        }
    }
    close(c, 0, &mut data)?;
    Ok(sample)
}

/// FILETIME of a UTC date and time.
fn filetime(y: u64, mo: u64, d: u64, h: u64, mi: u64, s: u64) -> u64 {
    let leap = |y: u64| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    const CUM: [u64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let before = y - 1601;
    let mut days = before * 365 + before / 4 - before / 100 + before / 400 + CUM[(mo - 1) as usize];
    if mo > 2 && leap(y) {
        days += 1;
    }
    days += d - 1;
    (((days * 24 + h) * 60 + mi) * 60 + s) * 10_000_000
}

/// ODT's setup.exe and its corrupted copies.
fn setup_exe(c: &mut Checks, path: &[u16]) -> Result<(), u32> {
    const SHA256: &[u8] = b"2.16.840.1.101.3.4.2.1";
    let sample = real_image(
        c,
        path,
        3,
        b"Microsoft Corporation",
        b"Microsoft Root Certificate Authority 2010",
        SHA256,
        None,
        WTD_REVOKE_NONE,
    )?;
    real_image(
        c,
        path,
        3,
        b"Microsoft Corporation",
        b"Microsoft Root Certificate Authority 2010",
        SHA256,
        None,
        WTD_REVOKE_WHOLECHAIN,
    )?;
    // The caller's handle instead of the path.
    let mut file = new_file(path);
    // SAFETY: a NUL-terminated path; our handle and structures.
    unsafe {
        file.file = CreateFileW(path.as_ptr(), GENERIC_READ, 1, 0, OPEN_EXISTING, 0, 0);
        c.ok(file.file != INVALID_HANDLE_VALUE, 0)?;
        let mut data = new_data(&file, WTD_REVOKE_NONE, 0);
        c.eq(WinVerifyTrust(0, &GENERIC_VERIFY_V2, &mut data) as u32, S_OK)?;
        close(c, 0, &mut data)?;
        CloseHandle(file.file);
    }

    let Some(image) = read_file(path) else { return c.ok(false, 0) };
    let (checksum, table, code) = pe_offsets(image);
    c.ok(table > code && table < image.len(), table as u64)?;
    let copy = temp_path(b"setup.exe");
    // A changed CheckSum: outside the image hash.
    image[checksum] ^= 0x5a;
    c.ok(write_file(&copy, &[image]), 0)?;
    let r = verify(c, &copy, false)?;
    c.eq(r, S_OK)?;
    image[checksum] ^= 0x5a;
    // A changed byte of the first section.
    image[code + 0x10] ^= 0x01;
    c.ok(write_file(&copy, &[image]), 0)?;
    let r = verify(c, &copy, false)?;
    c.eq(r, TRUST_E_BAD_DIGEST)?;
    let r = verify(c, &copy, true)?;
    c.eq(r, TRUST_E_NOSIGNATURE)?;
    image[code + 0x10] ^= 0x01;
    // A changed byte of the signer's encrypted digest (not confirmed on
    // Windows: CryptMsgControl's NTE_BAD_SIGNATURE).
    let at = image[table..].windows(16).position(|w| w == sample).map(|p| table + p);
    c.ok(at.is_some(), 0)?;
    let at = at.unwrap_or(0) + 8;
    image[at] ^= 0x01;
    c.ok(write_file(&copy, &[image]), 0)?;
    let r = verify(c, &copy, false)?;
    c.eq(r, NTE_BAD_SIGNATURE)?;
    image[at] ^= 0x01;
    // Data after the certificate table.
    c.ok(write_file(&copy, &[image, &[0x11, 0x22, 0x33, 0x44]]), 0)?;
    let r = verify(c, &copy, false)?;
    c.eq(r, TRUST_E_NOSIGNATURE)?;
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(copy.as_ptr()) };
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    basics(&mut c)?;
    // IMAGE_NT_HEADERS32 is 248 bytes, IMAGE_NT_HEADERS64 264.
    digest_cases(&mut c, include_bytes!("../data/selfsigned32.exe"), 248)?;
    digest_cases(&mut c, include_bytes!("../data/selfsigned64.exe"), 264)?;

    let require = required();
    let setup = wide::<32>(b"C:\\gala\\setup.exe");
    let python = wide::<32>(b"C:\\gala\\python.exe");
    let present = |p: &[u16]| {
        // SAFETY: a NUL-terminated path; our handle.
        unsafe {
            let h = CreateFileW(p.as_ptr(), GENERIC_READ, 1, 0, OPEN_EXISTING, 0, 0);
            h != INVALID_HANDLE_VALUE && CloseHandle(h) != 0
        }
    };
    if present(&setup) {
        setup_exe(&mut c, &setup)?;
    } else {
        c.ok(!require, 0)?;
        Line::new()
            .s(NAME.as_bytes())
            .s(b": no C:\\gala\\setup.exe, its checks skipped\r\n")
            .print();
    }
    if present(&python) {
        real_image(
            &mut c,
            &python,
            4,
            b"Python Software Foundation",
            b"Microsoft Identity Verification Root Certificate Authority 2020",
            b"2.16.840.1.101.3.4.2.1",
            Some(filetime(2024, 12, 3, 20, 13, 9) + 9_730_000),
            WTD_REVOKE_NONE,
        )?;
    } else {
        c.ok(!require, 0)?;
        Line::new()
            .s(NAME.as_bytes())
            .s(b": no C:\\gala\\python.exe, its checks skipped\r\n")
            .print();
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
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

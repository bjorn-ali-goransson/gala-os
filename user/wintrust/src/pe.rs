//! The PE image as Authenticode sees it ("Windows Authenticode Portable
//! Executable Signature Format"): which kind of subject a file is, where
//! its certificate table is, the WIN_CERTIFICATE's PKCS #7 SignedData,
//! and the image hash, which covers the file up to the certificate table
//! but for the optional header's CheckSum and the certificate table's
//! data directory entry.
//!
//! The file is read through the caller's (or wintrust's) handle at
//! offsets, in 64 KiB pieces, and hashed through bcrypt's algorithm
//! pseudo-handles. Only one layout is hashed: the bytes in file order
//! (headers, sections, data after them) up to the table, which is what
//! the specification's section-by-section order gives for an image whose
//! sections follow its headers in order; page hashes
//! (SpcPeImageData's 1.3.6.1.4.1.311.2.3.1/2) are not checked, as
//! WinVerifyTrust does not (code integrity checks them at load).

use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn SetFilePointerEx(h: usize, distance: i64, new: *mut i64, method: u32) -> i32;
    fn GetLastError() -> u32;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "bcrypt", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "bcrypt", kind = "raw-dylib"))]
unsafe extern "system" {
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

pub(crate) const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800b_0003;
pub(crate) const TRUST_E_NOSIGNATURE: u32 = 0x800b_0100;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// The largest certificate table read (Windows' signatures are a few
/// tens of KiB; a catalog-sized one is far below this).
const MAX_TABLE: u64 = 16 << 20;

/// Where a PE image's Authenticode pieces are.
pub(crate) struct Image {
    /// The CheckSum field's offset.
    pub checksum: u64,
    /// The certificate table's data directory entry's offset.
    pub directory: u64,
    /// The certificate table's offset and size.
    pub table: u64,
    pub table_size: u64,
}

/// Reads `buf.len()` bytes at `offset`; false if the file is shorter.
pub(crate) fn read_at(h: usize, offset: u64, buf: &mut [u8]) -> bool {
    let mut got = 0u32;
    // SAFETY: the file handle; our buffer.
    unsafe {
        SetFilePointerEx(h, offset as i64, core::ptr::null_mut(), 0) != 0
            && ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, core::ptr::null_mut()) != 0
            && got as usize == buf.len()
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// What the file of `size` bytes is: a PE image with a certificate table,
/// or the error WinVerifyTrust gives (as Windows does in Wine's
/// test_wintrust_digest): TRUST_E_SUBJECT_FORM_UNKNOWN for a file that is
/// no PE image (empty, no "MZ", no "PE\0\0" header inside it; a cabinet
/// or a catalog is recorded, gala having no SIP for them),
/// TRUST_E_NOSIGNATURE for an image whose certificate table is missing,
/// does not end the file, or is not where its directory says.
pub(crate) fn examine(h: usize, size: u64) -> Result<Image, u32> {
    let mut head = [0u8; 1024];
    let n = (size as usize).min(head.len());
    if n == 0 || !read_at(h, 0, &mut head[..n]) {
        return Err(TRUST_E_SUBJECT_FORM_UNKNOWN);
    }
    let head = &head[..n];
    if head.starts_with(b"MSCF") {
        crate::unsupported(b"wintrust.dll!WinVerifyTrust(subject: cabinet)", 0);
        return Err(TRUST_E_SUBJECT_FORM_UNKNOWN);
    }
    if !head.starts_with(b"MZ") || head.len() < 64 {
        if head.first() == Some(&0x30) {
            crate::unsupported(b"wintrust.dll!WinVerifyTrust(subject: catalog or PKCS #7)", 0);
        }
        return Err(TRUST_E_SUBJECT_FORM_UNKNOWN);
    }
    let nt = u64::from(u32_at(head, 0x3c).unwrap_or(0));
    // The NT headers: the signature, the file header (20 bytes) and the
    // optional header through its data directories.
    let mut hdr = [0u8; 24 + 240];
    let avail = size.saturating_sub(nt).min(hdr.len() as u64) as usize;
    if avail < 4 || !read_at(h, nt, &mut hdr[..avail]) || &hdr[..4] != b"PE\0\0" {
        return Err(TRUST_E_SUBJECT_FORM_UNKNOWN);
    }
    let hdr = &hdr[..avail];
    let optional = 24usize;
    let directories = match u16_at(hdr, optional) {
        Some(0x10b) => optional + 96,
        Some(0x20b) => optional + 112,
        _ => return Err(TRUST_E_NOSIGNATURE),
    };
    let entry = directories + 4 * 8;
    let (Some(table), Some(table_size)) = (u32_at(hdr, entry), u32_at(hdr, entry + 4)) else {
        return Err(TRUST_E_NOSIGNATURE);
    };
    let (table, table_size) = (u64::from(table), u64::from(table_size));
    let directory = nt + entry as u64;
    if table_size < 8
        || table < directory + 8
        || table > size
        || table + table_size != size
        || table_size > MAX_TABLE
    {
        return Err(TRUST_E_NOSIGNATURE);
    }
    Ok(Image { checksum: nt + optional as u64 + 64, directory, table, table_size })
}

/// A heap block holding `len` bytes, freed on drop.
pub(crate) struct Heap {
    pub p: *mut u8,
    pub len: usize,
}

impl Heap {
    pub fn new(len: usize) -> Option<Heap> {
        // SAFETY: plain allocation.
        let p = unsafe { HeapAlloc(GetProcessHeap(), 0, len.max(1)) };
        (!p.is_null()).then_some(Heap { p, len })
    }

    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: our block of `len` bytes.
        unsafe { core::slice::from_raw_parts_mut(self.p, self.len) }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // SAFETY: our block.
        unsafe { HeapFree(GetProcessHeap(), 0, self.p) };
    }
}

/// The PKCS #7 SignedData of the image's first WIN_CERTIFICATE
/// (wRevision WIN_CERT_REVISION_2_0 or _1_0, wCertificateType
/// WIN_CERT_TYPE_PKCS_SIGNED_DATA), in a heap block with its offset and
/// length; TRUST_E_NOSIGNATURE for any other.
pub(crate) fn signature(h: usize, image: &Image) -> Result<(Heap, usize, usize), u32> {
    let mut table = Heap::new(image.table_size as usize).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    if !read_at(h, image.table, table.bytes()) {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let t = table.bytes();
    let length = u32_at(t, 0).unwrap_or(0) as usize;
    let revision = u16_at(t, 4).unwrap_or(0);
    let kind = u16_at(t, 6).unwrap_or(0);
    if length < 8 || length > t.len() || !matches!(revision, 0x100 | 0x200) || kind != 2 {
        return Err(TRUST_E_NOSIGNATURE);
    }
    if (length + 7) & !7 < t.len() {
        crate::unsupported(b"wintrust.dll!WinVerifyTrust(more than one WIN_CERTIFICATE)", 0);
    }
    Ok((table, 8, length - 8))
}

/// The digest algorithm named by OID text `oid`: bcrypt's pseudo-handle
/// and the digest's length.
pub(crate) fn digest_algorithm(oid: &[u8]) -> Option<(usize, usize)> {
    match oid {
        b"1.3.14.3.2.26" => Some((0x31, 20)),
        b"2.16.840.1.101.3.4.2.1" => Some((0x41, 32)),
        b"2.16.840.1.101.3.4.2.2" => Some((0x51, 48)),
        b"2.16.840.1.101.3.4.2.3" => Some((0x61, 64)),
        b"1.2.840.113549.2.5" => Some((0x21, 16)),
        _ => None,
    }
}

/// The image hash with algorithm `alg` (a bcrypt pseudo-handle) into
/// `out`: the file up to the certificate table without the CheckSum
/// and the table's directory entry. The error is the step's (a read's
/// last error, or bcrypt's NTSTATUS).
pub(crate) fn image_hash(h: usize, image: &Image, alg: usize, out: &mut [u8]) -> Result<(), u32> {
    let mut hash = 0usize;
    // SAFETY: bcrypt's documented call; a pseudo-handle, no object buffer.
    let status = unsafe {
        BCryptCreateHash(alg, &mut hash, core::ptr::null_mut(), 0, core::ptr::null(), 0, 0)
    };
    if status != 0 {
        return Err(status);
    }
    let r = (|| {
        let mut buf = Heap::new(64 << 10).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
        let ranges = [
            (0, image.checksum),
            (image.checksum + 4, image.directory),
            (image.directory + 8, image.table),
        ];
        for (start, end) in ranges {
            let mut at = start;
            while at < end {
                let n = (end - at).min(buf.len as u64) as usize;
                let piece = &mut buf.bytes()[..n];
                if !read_at(h, at, piece) {
                    // SAFETY: plain call.
                    return Err(unsafe { GetLastError() });
                }
                // SAFETY: our hash and buffer.
                let status = unsafe { BCryptHashData(hash, piece.as_ptr(), n as u32, 0) };
                if status != 0 {
                    return Err(status);
                }
                at += n as u64;
            }
        }
        // SAFETY: our hash and the caller's buffer.
        let status = unsafe { BCryptFinishHash(hash, out.as_mut_ptr(), out.len() as u32, 0) };
        if status != 0 { Err(status) } else { Ok(()) }
    })();
    // SAFETY: our hash.
    unsafe { BCryptDestroyHash(hash) };
    r
}

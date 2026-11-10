//! The cabinet as Authenticode sees it: where a signed cabinet keeps its
//! PKCS #7 SignedData, and the digest its SpcIndirectDataContent carries.
//!
//! - **The signature's place** (Wine's crypt.c, CAB_SIGNINFO; Microsoft's
//!   CAB format for the header): the header (36 bytes) has the flag
//!   RESERVE_PRESENT (0x0004), then cbCFHeader (u16), cbCFFolder and
//!   cbCFData (u8 each) and cbCFHeader bytes of reserve, whose first 12
//!   are a word 00 00 10 00, the signature's offset and its size (u32
//!   each). What Windows answers for a cabinet with a byte of each changed
//!   (`cab_probe`, measured on 26200, issue #59) gives the rules: without
//!   the flag, with less than 12 bytes of reserve, another first word, a
//!   cbCFData that is not 0, a signature that does not start at
//!   cbCabinet, or one outside the file, the cabinet is not signed
//!   (TRUST_E_NOSIGNATURE). The version is not checked (a changed one is
//!   TRUST_E_BAD_DIGEST, as any byte of the digest).
//!   The signature is its DER, then zeros to its size (padding that is
//!   not zero: TRUST_E_NOSIGNATURE; a size short of the DER: the decode's
//!   CRYPT_E_BAD_MSG).
//! - **The digest** (no published source documents it; established by
//!   matching the SpcIndirectDataContent digest of four signed cabinets
//!   of Microsoft's CDN, v64.cab, v32.cab, ofl.cab, releasehistory.cab,
//!   against hashes of every subset of the header's fields: exactly one
//!   matches, the same for all four): the file from 0 up to the
//!   signature, leaving out the header's reserved1 field (bytes 4..8),
//!   cbCFHeader (36..38) and the reserve area (40..40+cbCFHeader), which
//!   holds the signature's offset and size; then whatever follows the
//!   signature. cbCabinet, the folders, files and data, and any
//!   per-folder and per-data reserve, are in it. Microsoft's cabinets end
//!   with their signature; 4 bytes appended are TRUST_E_BAD_DIGEST on
//!   Windows (`cab_probe`), so the bytes after it are hashed.
//!   `cab_probe` measures what Windows answers for altered copies.
//! - The SpcIndirectDataContent's data is an SpcLink
//!   (1.3.6.1.4.1.311.2.1.28; SPC_CAB_DATA_OBJID 1.3.6.1.4.1.311.2.1.25 is
//!   taken too): a moniker whose one attribute,
//!   1.3.6.1.4.1.311.2.5.1, is the SHA-256 of cbCabinet's 4 bytes. It is
//!   not checked: cbCabinet is in the digest already.

use crate::pe::{Heap, read_at, u16_at, u32_at};

const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// The largest signature read (Microsoft's are about 10 KiB).
const MAX_SIGNATURE: u64 = 16 << 20;

/// Where a signed cabinet's Authenticode pieces are.
pub(crate) struct Cabinet {
    /// The file's size.
    pub size: u64,
    /// The header's reserve area's size (cbCFHeader).
    pub reserve: u64,
    /// The signature's offset and size.
    pub signature: u64,
    pub signature_size: u64,
}

/// Whether the file starts as a cabinet does ("MSCF": the cabinet
/// subject, as CryptSIPRetrieveSubjectGUID answers for those 4 bytes in
/// Wine's sip.c tests).
pub(crate) fn is_cabinet(h: usize, size: u64) -> bool {
    let mut magic = [0u8; 4];
    size >= 4 && read_at(h, 0, &mut magic) && &magic == b"MSCF"
}

/// Where the signature of the cabinet of `size` bytes is, or
/// TRUST_E_NOSIGNATURE (module comment).
pub(crate) fn examine(h: usize, size: u64) -> Result<Cabinet, u32> {
    let mut head = [0u8; 52];
    if size < head.len() as u64 || !read_at(h, 0, &mut head) {
        return Err(crate::pe::TRUST_E_NOSIGNATURE);
    }
    let cb_cabinet = u64::from(u32_at(&head, 8).unwrap_or(0));
    let flags = u16_at(&head, 30).unwrap_or(0);
    let reserve = u64::from(u16_at(&head, 36).unwrap_or(0));
    let cb_cf_data = head[39];
    let first = u32_at(&head, 40).unwrap_or(0);
    let signature = u64::from(u32_at(&head, 44).unwrap_or(0));
    let signature_size = u64::from(u32_at(&head, 48).unwrap_or(0));
    if flags & 0x0004 == 0 || reserve < 12 || cb_cf_data != 0 || first != 0x0010_0000 {
        return Err(crate::pe::TRUST_E_NOSIGNATURE);
    }
    if signature != cb_cabinet
        || signature < 40 + reserve
        || signature_size == 0
        || signature_size > MAX_SIGNATURE
        || signature + signature_size > size
    {
        return Err(crate::pe::TRUST_E_NOSIGNATURE);
    }
    Ok(Cabinet { size, reserve, signature, signature_size })
}

/// The cabinet's PKCS #7 SignedData, in a heap block with its offset and
/// length (the block may end in padding, which decoding ignores).
pub(crate) fn signature(h: usize, cab: &Cabinet) -> Result<(Heap, usize, usize), u32> {
    let mut block = Heap::new(cab.signature_size as usize).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    if !read_at(h, cab.signature, block.bytes()) {
        return Err(crate::pe::TRUST_E_NOSIGNATURE);
    }
    // The DER's own length: Microsoft pads the signature to 8 bytes, with
    // zeros. Padding that is not zero loses the signature (the
    // signature's last byte changed is TRUST_E_NOSIGNATURE on Windows,
    // `cab_probe`, issue #59).
    let len = der_length(block.bytes()).unwrap_or(cab.signature_size as usize);
    if block.bytes()[len..].iter().any(|&b| b != 0) {
        return Err(crate::pe::TRUST_E_NOSIGNATURE);
    }
    Ok((block, 0, len))
}

/// The length of the DER element at the start of `b`, if it fits.
fn der_length(b: &[u8]) -> Option<usize> {
    let first = *b.get(1)?;
    let (header, body) = if first < 0x80 {
        (2, usize::from(first))
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let mut v = 0usize;
        for i in 0..n {
            v = (v << 8) | usize::from(*b.get(2 + i)?);
        }
        (2 + n, v)
    };
    (header + body <= b.len()).then_some(header + body)
}

/// The cabinet's Authenticode digest with algorithm `alg` (a bcrypt
/// pseudo-handle) into `out` (module comment).
pub(crate) fn hash(h: usize, cab: &Cabinet, alg: usize, out: &mut [u8]) -> Result<(), u32> {
    crate::pe::hash_ranges(
        h,
        &[
            (0, 4),
            (8, 36),
            (38, 40),
            (40 + cab.reserve, cab.signature),
            (cab.signature + cab.signature_size, cab.size),
        ],
        alg,
        out,
    )
}

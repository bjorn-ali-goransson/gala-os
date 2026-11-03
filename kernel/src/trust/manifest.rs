//! The signed manifest (R8, ARCHITECTURE "Chain of trust", steps 4-5): which
//! files make up a release, with their sizes and SHA-256, signed with an
//! Ed25519 release key. Pure and allocation-free: the kernel verifies it at
//! boot; xtask (which includes this file) writes and signs it; host tests
//! below.
//!
//! Format version 1, little-endian:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | magic `GALAMANI` |
//! | 8 | 4 | format version (1) |
//! | 12 | 4 | header length (56; a newer header may be longer, readers skip the rest) |
//! | 16 | 8 | release number (strictly increasing between releases) |
//! | 24 | 8 | rollback floor: the oldest release that may run after this one (at most the release) |
//! | 32 | 8 | key id: the first 8 bytes of SHA-256 of the signing key's public half, as a u64 |
//! | 40 | 4 | entry count |
//! | 44 | 4 | the entries' length in bytes |
//! | 48 | 8 | flags (0; a reader refuses bits it does not know) |
//! | 56 | | entries: size (u64), SHA-256 (32), path length (u16), path (UTF-8), zeros to a multiple of 8 |
//! | end - 64 | 64 | Ed25519 signature over every byte before it |

use super::sha256;

pub const MAGIC: [u8; 8] = *b"GALAMANI";
/// The format this code writes, and the newest it reads.
pub const VERSION: u32 = 1;
/// The oldest format it reads.
pub const MIN_VERSION: u32 = 1;
pub const HEADER_LEN: usize = 56;
pub const SIGNATURE_LEN: usize = 64;
/// An entry's fixed part (size, hash, path length).
const ENTRY_FIXED: usize = 8 + 32 + 2;

/// Why a manifest is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// Shorter than its header and signature.
    Short,
    Magic,
    /// A format version this code does not read.
    Version(u32),
    /// Lengths or entries that do not add up.
    Layout,
    /// Flags this code does not know.
    Flags(u64),
    /// Signed by another key (its id, and the id this kernel trusts).
    Key {
        found: u64,
        trusted: u64,
    },
    /// The signature does not verify.
    Signature,
    /// A rollback floor above the release itself.
    FloorAboveRelease {
        release: u64,
        floor: u64,
    },
    /// The release is below the floor stored on this machine.
    BelowFloor {
        release: u64,
        floor: u64,
    },
}

impl core::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ManifestError::Short => f.write_str("too short"),
            ManifestError::Magic => f.write_str("not a manifest (magic)"),
            ManifestError::Version(v) => {
                write!(f, "format version {v} (this kernel reads {MIN_VERSION} to {VERSION})")
            }
            ManifestError::Layout => f.write_str("damaged (lengths or entries)"),
            ManifestError::Flags(x) => write!(f, "unknown flags {x:#x}"),
            ManifestError::Key { found, trusted } => {
                write!(f, "signed by key {found:016x}, this kernel trusts {trusted:016x}")
            }
            ManifestError::Signature => f.write_str("the signature does not verify"),
            ManifestError::FloorAboveRelease { release, floor } => {
                write!(f, "rollback floor {floor} above its release {release}")
            }
            ManifestError::BelowFloor { release, floor } => {
                write!(f, "release {release} is below the rollback floor {floor}")
            }
        }
    }
}

/// A file a manifest lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub path: &'a str,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// A verified manifest (its signature checked against the trusted key).
#[derive(Debug, Clone, Copy)]
pub struct Manifest<'a> {
    pub version: u32,
    pub release: u64,
    pub floor: u64,
    pub key_id: u64,
    count: u32,
    entries: &'a [u8],
}

/// The id of a public key (what a manifest names its signer by).
pub fn key_id(public: &[u8; 32]) -> u64 {
    let h = sha256::digest(public);
    u64::from_le_bytes([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut x = [0u8; 4];
    x.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(x)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut x = [0u8; 8];
    x.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(x)
}

/// Reads the entry at the start of `b`: the entry and its length.
fn entry_at(b: &[u8]) -> Option<(Entry<'_>, usize)> {
    if b.len() < ENTRY_FIXED {
        return None;
    }
    let size = u64_at(b, 0);
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&b[8..40]);
    let len = usize::from(u16::from_le_bytes([b[40], b[41]]));
    let path = b.get(ENTRY_FIXED..ENTRY_FIXED + len)?;
    let path = core::str::from_utf8(path).ok()?;
    let total = (ENTRY_FIXED + len).next_multiple_of(8);
    let pad = b.get(ENTRY_FIXED + len..total)?;
    if pad.iter().any(|&x| x != 0) {
        return None;
    }
    Some((Entry { path, size, sha256: sha }, total))
}

/// Checks `bytes` as a manifest signed by `key` (the Ed25519 public key
/// this kernel trusts).
pub fn verify<'a>(bytes: &'a [u8], key: &[u8; 32]) -> Result<Manifest<'a>, ManifestError> {
    if bytes.len() < HEADER_LEN + SIGNATURE_LEN {
        return Err(ManifestError::Short);
    }
    if bytes[0..8] != MAGIC {
        return Err(ManifestError::Magic);
    }
    let version = u32_at(bytes, 8);
    if !(MIN_VERSION..=VERSION).contains(&version) {
        return Err(ManifestError::Version(version));
    }
    let header_len = u32_at(bytes, 12) as usize;
    let count = u32_at(bytes, 40);
    let entries_len = u32_at(bytes, 44) as usize;
    let body = header_len.checked_add(entries_len).ok_or(ManifestError::Layout)?;
    if header_len < HEADER_LEN || body.checked_add(SIGNATURE_LEN) != Some(bytes.len()) {
        return Err(ManifestError::Layout);
    }
    // The signer first: a manifest of another key is reported as such.
    let found = u64_at(bytes, 32);
    let trusted = key_id(key);
    if found != trusted {
        return Err(ManifestError::Key { found, trusted });
    }
    let mut signature = [0u8; SIGNATURE_LEN];
    signature.copy_from_slice(&bytes[body..]);
    ed25519_compact::PublicKey::new(*key)
        .verify(&bytes[..body], &ed25519_compact::Signature::new(signature))
        .map_err(|_| ManifestError::Signature)?;
    // Signed: the rest is what the release key holder wrote, checked all
    // the same (a signing tool's bug must not become a kernel fault).
    let flags = u64_at(bytes, 48);
    if flags != 0 {
        return Err(ManifestError::Flags(flags));
    }
    let release = u64_at(bytes, 16);
    let floor = u64_at(bytes, 24);
    if floor > release {
        return Err(ManifestError::FloorAboveRelease { release, floor });
    }
    let entries = &bytes[header_len..body];
    let mut at = 0;
    for _ in 0..count {
        let (_, n) = entry_at(&entries[at..]).ok_or(ManifestError::Layout)?;
        at += n;
    }
    if at != entries.len() {
        return Err(ManifestError::Layout);
    }
    Ok(Manifest { version, release, floor, key_id: found, count, entries })
}

impl<'a> Manifest<'a> {
    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn entries(&self) -> Entries<'a> {
        Entries { rest: self.entries }
    }

    /// The entry for a file of `size` bytes hashing to `sha256`.
    pub fn find(&self, size: u64, sha256: &[u8; 32]) -> Option<Entry<'a>> {
        self.entries().find(|e| e.size == size && e.sha256 == *sha256)
    }

    /// Whether this release may run on a machine whose stored rollback
    /// floor is `stored`.
    pub fn allowed_above(&self, stored: u64) -> Result<(), ManifestError> {
        if self.release < stored {
            return Err(ManifestError::BelowFloor { release: self.release, floor: stored });
        }
        Ok(())
    }
}

pub struct Entries<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Entries<'a> {
    type Item = Entry<'a>;
    fn next(&mut self) -> Option<Entry<'a>> {
        let (e, n) = entry_at(self.rest)?;
        self.rest = &self.rest[n..];
        Some(e)
    }
}

/// What the manifest's header says (for `encode`).
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub release: u64,
    pub floor: u64,
    pub key_id: u64,
}

/// The bytes `encode` writes for `entries`.
pub fn encoded_len(entries: &[Entry<'_>]) -> usize {
    HEADER_LEN
        + entries.iter().map(|e| (ENTRY_FIXED + e.path.len()).next_multiple_of(8)).sum::<usize>()
        + SIGNATURE_LEN
}

/// Writes a manifest into `out` (exactly `encoded_len` bytes), signed by
/// `sign` (Ed25519 over the bytes it is given). None if a path is longer
/// than 65535 bytes or the lengths do not fit.
pub fn encode(
    out: &mut [u8],
    header: &Header,
    entries: &[Entry<'_>],
    sign: impl FnOnce(&[u8]) -> [u8; SIGNATURE_LEN],
) -> Option<()> {
    if out.len() != encoded_len(entries) {
        return None;
    }
    out.fill(0);
    out[0..8].copy_from_slice(&MAGIC);
    out[8..12].copy_from_slice(&VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    out[16..24].copy_from_slice(&header.release.to_le_bytes());
    out[24..32].copy_from_slice(&header.floor.to_le_bytes());
    out[32..40].copy_from_slice(&header.key_id.to_le_bytes());
    out[40..44].copy_from_slice(&u32::try_from(entries.len()).ok()?.to_le_bytes());
    let entries_len = out.len() - HEADER_LEN - SIGNATURE_LEN;
    out[44..48].copy_from_slice(&u32::try_from(entries_len).ok()?.to_le_bytes());
    let mut at = HEADER_LEN;
    for e in entries {
        let len = u16::try_from(e.path.len()).ok()?;
        out[at..at + 8].copy_from_slice(&e.size.to_le_bytes());
        out[at + 8..at + 40].copy_from_slice(&e.sha256);
        out[at + 40..at + 42].copy_from_slice(&len.to_le_bytes());
        out[at + ENTRY_FIXED..at + ENTRY_FIXED + e.path.len()].copy_from_slice(e.path.as_bytes());
        at += (ENTRY_FIXED + e.path.len()).next_multiple_of(8);
    }
    let signature = sign(&out[..at]);
    out[at..].copy_from_slice(&signature);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_compact::{KeyPair, Seed};
    use std::vec::Vec;

    fn keys(seed: u8) -> KeyPair {
        KeyPair::from_seed(Seed::new([seed; 32]))
    }

    fn manifest(pair: &KeyPair, release: u64, floor: u64) -> Vec<u8> {
        let a = sha256::digest(b"kernel");
        let b = sha256::digest(b"hello.exe");
        let entries = [
            Entry { path: "/boot/kernel", size: 6, sha256: a },
            Entry { path: "/boot/hello.exe", size: 9, sha256: b },
            Entry { path: "", size: 0, sha256: sha256::digest(b"") },
        ];
        let header = Header { release, floor, key_id: key_id(&pair.pk) };
        let mut out = std::vec![0u8; encoded_len(&entries)];
        encode(&mut out, &header, &entries, |m| *pair.sk.sign(m, None)).unwrap();
        out
    }

    #[test]
    fn round_trip() {
        let pair = keys(7);
        let bytes = manifest(&pair, 3, 2);
        let m = verify(&bytes, &pair.pk).unwrap();
        assert_eq!((m.version, m.release, m.floor, m.len()), (1, 3, 2, 3));
        let paths: Vec<&str> = m.entries().map(|e| e.path).collect();
        assert_eq!(paths, ["/boot/kernel", "/boot/hello.exe", ""]);
        assert_eq!(m.find(9, &sha256::digest(b"hello.exe")).unwrap().path, "/boot/hello.exe");
        // The hash with another size, or a file not listed: not found.
        assert!(m.find(8, &sha256::digest(b"hello.exe")).is_none());
        assert!(m.find(5, &sha256::digest(b"other")).is_none());
        assert_eq!(bytes.len() % 8, 0);
    }

    #[test]
    fn damage_is_refused() {
        let pair = keys(7);
        let good = manifest(&pair, 3, 2);
        // Any byte changed: the magic, the version, a length, the key id,
        // an entry, the signature.
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] ^= 0x20;
            assert!(verify(&bad, &pair.pk).is_err(), "byte {at} changed was accepted");
        }
        assert_eq!(verify(&good[..good.len() - 1], &pair.pk).unwrap_err(), ManifestError::Layout);
        assert_eq!(verify(&good[..40], &pair.pk).unwrap_err(), ManifestError::Short);
        let mut v = good.clone();
        v[8] = 2;
        assert_eq!(verify(&v, &pair.pk).unwrap_err(), ManifestError::Version(2));
        let mut s = good.clone();
        let last = s.len() - 1;
        s[last] ^= 1;
        assert_eq!(verify(&s, &pair.pk).unwrap_err(), ManifestError::Signature);
        // An entry changed (a file's hash): the signature no longer holds.
        let mut e = good.clone();
        e[HEADER_LEN + 8] ^= 1;
        assert_eq!(verify(&e, &pair.pk).unwrap_err(), ManifestError::Signature);
    }

    #[test]
    fn wrong_key_is_refused() {
        let release_key = keys(1);
        let test_key = keys(2);
        let bytes = manifest(&test_key, 3, 2);
        assert_eq!(
            verify(&bytes, &release_key.pk).unwrap_err(),
            ManifestError::Key { found: key_id(&test_key.pk), trusted: key_id(&release_key.pk) }
        );
        // The right id with a signature made by another key.
        let mut forged = manifest(&release_key, 3, 2);
        let body = forged.len() - SIGNATURE_LEN;
        let sig = test_key.sk.sign(&forged[..body], None);
        forged[body..].copy_from_slice(&*sig);
        assert_eq!(verify(&forged, &release_key.pk).unwrap_err(), ManifestError::Signature);
    }

    #[test]
    fn rollback_floor() {
        let pair = keys(9);
        let m2 = manifest(&pair, 2, 2);
        let m1 = manifest(&pair, 1, 1);
        let two = verify(&m2, &pair.pk).unwrap();
        let one = verify(&m1, &pair.pk).unwrap();
        assert_eq!(two.allowed_above(0), Ok(()));
        assert_eq!(two.allowed_above(2), Ok(()));
        // Release 2 raised the floor to 2: release 1 is refused.
        assert_eq!(one.allowed_above(2), Err(ManifestError::BelowFloor { release: 1, floor: 2 }));
        // A floor above the release is a broken manifest.
        let bad = manifest(&pair, 1, 2);
        assert_eq!(
            verify(&bad, &pair.pk).unwrap_err(),
            ManifestError::FloorAboveRelease { release: 1, floor: 2 }
        );
    }

    #[test]
    fn unknown_flags_and_longer_headers() {
        let pair = keys(4);
        let good = manifest(&pair, 5, 5);
        let resign = |mut b: Vec<u8>| {
            let body = b.len() - SIGNATURE_LEN;
            let sig = pair.sk.sign(&b[..body], None);
            b[body..].copy_from_slice(&*sig);
            b
        };
        let mut f = good.clone();
        f[48] = 1;
        assert_eq!(verify(&resign(f), &pair.pk).unwrap_err(), ManifestError::Flags(1));
        // A header 8 bytes longer (a future field) is skipped.
        let mut long = good[..HEADER_LEN].to_vec();
        long[12] = (HEADER_LEN + 8) as u8;
        long.extend_from_slice(&[0xee; 8]);
        long.extend_from_slice(&good[HEADER_LEN..]);
        let m = resign(long);
        assert_eq!(verify(&m, &pair.pk).unwrap().len(), 3);
    }
}

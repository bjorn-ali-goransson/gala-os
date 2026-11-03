//! The chain of trust after boot (R8, ARCHITECTURE "Chain of trust"):
//! Limine has checked the kernel and its boot modules against the hashes
//! in its enrolled config (steps 2-3); from here the kernel checks what it
//! loads later against a signed manifest (steps 4-5) and keeps a rollback
//! floor on the disk (step 6).
//!
//! The public key the kernel trusts is built in: `GALA_TRUST_KEY` (64 hex
//! digits, the Ed25519 public key) when the kernel is compiled; xtask sets
//! it to its test key (`target/trust/`). A kernel built without it is an
//! *unsigned build*: it checks nothing and never raises the floor.

pub mod manifest;
pub mod sha256;

use crate::boot::BootInfo;
use crate::sync::IrqMutex;
use manifest::{Manifest, ManifestError};

/// The boot module holding the manifest (xtask ships `/boot/gala.manifest`
/// with module string `manifest`).
pub const MODULE_STRING: &str = "manifest";

/// The release key this kernel trusts, if it was built with one.
pub const KEY: Option<[u8; 32]> = match option_env!("GALA_TRUST_KEY") {
    Some(hex) => Some(parse_key(hex.as_bytes())),
    None => None,
};

/// 64 hex digits as 32 bytes (at compile time: a bad key fails the build).
const fn parse_key(hex: &[u8]) -> [u8; 32] {
    const fn digit(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("GALA_TRUST_KEY: not a hex digit"),
        }
    }
    assert!(hex.len() == 64, "GALA_TRUST_KEY: 64 hex digits expected");
    let mut key = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        key[i] = (digit(hex[2 * i]) << 4) | digit(hex[2 * i + 1]);
        i += 1;
    }
    key
}

/// What the boot found.
#[derive(Clone, Copy)]
enum State {
    /// `boot` has not run (gala-um, or early).
    Unknown,
    /// No key built in: nothing is checked.
    Unsigned,
    /// No manifest module.
    Missing,
    Rejected(ManifestError),
    Verified(Manifest<'static>),
}

static STATE: IrqMutex<State> = IrqMutex::new(State::Unknown);

/// Why an image is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untrusted {
    /// The boot had no manifest.
    NoManifest,
    /// The boot's manifest was refused.
    Rejected(ManifestError),
    /// Not listed: the image's size and the start of its SHA-256.
    NotListed { release: u64, size: u64, sha256: [u8; 8] },
}

impl core::fmt::Display for Untrusted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Untrusted::NoManifest => f.write_str("this kernel booted without a signed manifest"),
            Untrusted::Rejected(e) => write!(f, "this boot's manifest was refused ({e})"),
            Untrusted::NotListed { release, size, sha256 } => {
                write!(f, "it is not in the signed manifest of release {release} (SHA-256 ")?;
                for b in sha256 {
                    write!(f, "{b:02x}")?;
                }
                write!(f, "..., {size} bytes)")
            }
        }
    }
}

/// Finds and verifies the boot's manifest (once per kernel, a kernel
/// handed over to included: the modules are still there).
pub fn boot(info: &BootInfo) {
    let state = match KEY {
        None => {
            info!("manifest: unsigned build (no release key built in): nothing is checked");
            State::Unsigned
        }
        Some(key) => match info.modules.iter().find(|m| m.cmdline == MODULE_STRING) {
            None => {
                warn!("manifest: none in the boot modules; no kernel will be handed over to");
                State::Missing
            }
            Some(module) => match manifest::verify(module.data, &key) {
                Ok(m) => {
                    info!(
                        "manifest: release {} verified (rollback floor {}, {} files, key {:016x})",
                        m.release,
                        m.floor,
                        m.len(),
                        m.key_id
                    );
                    State::Verified(m)
                }
                Err(e) => {
                    error!(
                        "manifest: {} refused: {e}; no kernel will be handed over to",
                        module.path
                    );
                    State::Rejected(e)
                }
            },
        },
    };
    *STATE.lock() = state;
}

/// R8 step 4 for a kernel handover: `file` must be listed, size and
/// SHA-256, in the verified manifest (an unsigned build takes anything).
pub fn trusted(file: &[u8]) -> Result<(), Untrusted> {
    let state = *STATE.lock();
    let m = match state {
        State::Unknown | State::Unsigned => return Ok(()),
        State::Missing => return Err(Untrusted::NoManifest),
        State::Rejected(e) => return Err(Untrusted::Rejected(e)),
        State::Verified(m) => m,
    };
    let hash = digest_of(file);
    match m.find(file.len() as u64, &hash) {
        Some(e) => {
            info!("manifest: the next kernel is {} of release {}", e.path, m.release);
            Ok(())
        }
        None => {
            let mut start = [0u8; 8];
            start.copy_from_slice(&hash[..8]);
            Err(Untrusted::NotListed { release: m.release, size: file.len() as u64, sha256: start })
        }
    }
}

/// The last image hashed: its address, length and SHA-256 (boot modules
/// and the kernel file do not move or change).
static HASHED: IrqMutex<Option<(usize, usize, [u8; 32])>> = IrqMutex::new(None);

fn digest_of(file: &[u8]) -> [u8; 32] {
    let key = (file.as_ptr() as usize, file.len());
    if let Some((at, len, hash)) = *HASHED.lock()
        && (at, len) == key
    {
        return hash;
    }
    let hash = sha256::digest(file);
    *HASHED.lock() = Some((key.0, key.1, hash));
    hash
}

/// Hashes the image a handover will check, ahead of time (at boot, with
/// `handover=`): under emulation the SHA-256 of a kernel takes a few
/// hundred milliseconds, which would otherwise pass between a program's
/// request and the handover.
pub fn prepare(file: &[u8]) {
    if matches!(*STATE.lock(), State::Verified(_)) {
        digest_of(file);
    }
}

/// The verified release (None for an unsigned build or a refused manifest).
fn verified() -> Option<Manifest<'static>> {
    match *STATE.lock() {
        State::Verified(m) => Some(m),
        _ => None,
    }
}

/// R8 step 6, after the volume is mounted: the release must not be below
/// the floor stored on the disk. Err with the reason: the kernel must not
/// go on (an older release put back).
pub fn check_rollback(stored: Option<u64>) -> Result<(), ManifestError> {
    let (Some(m), Some(stored)) = (verified(), stored) else { return Ok(()) };
    m.allowed_above(stored)?;
    info!("rollback: release {} runs on a disk whose floor is {stored}", m.release);
    Ok(())
}

/// What the disk's rollback floor should be raised to after a boot that
/// got this far with a verified manifest (the release works): the
/// manifest's floor, and its release as the highest seen.
pub fn floor_for_disk() -> Option<crate::fs::volume::Floor> {
    verified().map(|m| crate::fs::volume::Floor { floor: m.floor, highest: m.release })
}

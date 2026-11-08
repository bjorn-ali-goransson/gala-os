//! The user's floating-point and SIMD state (stub): V0-V31 (the 512-byte
//! "legacy" part, as x86-64's FXSAVE image is), FPCR and FPSR; SVE not
//! yet. Component bit 0 is the legacy part.

use super::unported;

/// The legacy part's component bits.
pub const LEGACY: u64 = 1;
/// Everything this architecture's state format has.
pub const SUPPORTED: u64 = LEGACY;
/// V0-V31.
pub const LEGACY_SIZE: usize = 512;
/// The legacy part and FPCR/FPSR.
pub const MAX_SIZE: usize = LEGACY_SIZE + 16;

pub fn enabled() -> u64 {
    LEGACY
}

/// The state's size with the components `mask`.
pub fn size_of(_mask: u64) -> usize {
    MAX_SIZE
}

/// The state's size with the enabled components.
pub fn size() -> usize {
    MAX_SIZE
}

#[derive(Clone)]
pub struct XState([u8; MAX_SIZE]);

impl core::fmt::Debug for XState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("XState").finish_non_exhaustive()
    }
}

impl PartialEq for XState {
    fn eq(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}
impl Eq for XState {}

impl XState {
    /// The reset state (all zero: FPCR's defaults).
    pub const fn initial() -> Self {
        XState([0; MAX_SIZE])
    }

    pub fn from_legacy(legacy: &[u8; LEGACY_SIZE]) -> Self {
        let mut s = Self::initial();
        s.0[..LEGACY_SIZE].copy_from_slice(legacy);
        s
    }

    pub fn bytes(&self) -> &[u8; MAX_SIZE] {
        &self.0
    }

    pub fn bytes_mut(&mut self) -> &mut [u8; MAX_SIZE] {
        &mut self.0
    }

    pub fn legacy(&self) -> &[u8; LEGACY_SIZE] {
        self.0[..LEGACY_SIZE].try_into().unwrap_or(&[0; LEGACY_SIZE])
    }

    /// The components holding values (always the legacy part here).
    pub fn bv(&self) -> u64 {
        LEGACY
    }

    pub fn set_bv(&mut self, _bv: u64) {}
}

pub fn save() -> XState {
    unported("xstate::save")
}

pub fn restore(_state: &XState) {
    unported("xstate::restore")
}

/// The floating-point control register (the interface's name is
/// x86-64's MXCSR).
pub fn set_mxcsr(_value: u32) {
    unported("xstate::set_mxcsr")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoMemory;

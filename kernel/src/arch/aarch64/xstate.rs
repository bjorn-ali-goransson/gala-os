//! The user's floating-point and SIMD state: V0-V31 (the 512-byte
//! "legacy" part, as x86-64's FXSAVE image is), FPCR and FPSR; SVE not
//! yet. Component bit 0 is the legacy part. The kernel is soft-float and
//! never uses these registers, so during a system call they hold the
//! calling thread's values (the context switch saves and restores them):
//! `save` and `restore` read and write them directly.

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

/// The save area's layout (the context switch's): V0-V31 (16 bytes
/// each), then FPCR and FPSR (8 bytes each); 16-byte aligned for the
/// pairs of Q registers.
#[derive(Clone)]
#[repr(C, align(16))]
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

unsafe extern "C" {
    fn gala_aarch64_fp_save(area: *mut u8);
    fn gala_aarch64_fp_restore(area: *const u8);
}

/// The running thread's FP/SIMD state.
pub fn save() -> XState {
    let mut state = XState::initial();
    // SAFETY: writes MAX_SIZE bytes (V0-V31, FPCR, FPSR) into the state,
    // which is 16-byte aligned as the Q stores need.
    unsafe { gala_aarch64_fp_save(state.0.as_mut_ptr()) };
    state
}

/// Loads `state` as the running thread's FP/SIMD state (what it resumes
/// with in user mode).
pub fn restore(state: &XState) {
    // SAFETY: reads MAX_SIZE bytes of the 16-byte aligned state; only the
    // FP/SIMD registers, which the kernel does not use, change.
    unsafe { gala_aarch64_fp_restore(state.0.as_ptr()) };
}

// The layout is the context switch's (`context.rs`): V0-V31, then FPCR
// and FPSR as 8-byte words.
core::arch::global_asm!(
    r#"
    .arch_extension fp
    .arch_extension simd
    .global gala_aarch64_fp_save
gala_aarch64_fp_save:
    stp q0, q1, [x0, #0]
    stp q2, q3, [x0, #32]
    stp q4, q5, [x0, #64]
    stp q6, q7, [x0, #96]
    stp q8, q9, [x0, #128]
    stp q10, q11, [x0, #160]
    stp q12, q13, [x0, #192]
    stp q14, q15, [x0, #224]
    stp q16, q17, [x0, #256]
    stp q18, q19, [x0, #288]
    stp q20, q21, [x0, #320]
    stp q22, q23, [x0, #352]
    stp q24, q25, [x0, #384]
    stp q26, q27, [x0, #416]
    stp q28, q29, [x0, #448]
    stp q30, q31, [x0, #480]
    mrs x9, fpcr
    mrs x10, fpsr
    add x0, x0, #512
    stp x9, x10, [x0]
    ret
    .global gala_aarch64_fp_restore
gala_aarch64_fp_restore:
    ldp q0, q1, [x0, #0]
    ldp q2, q3, [x0, #32]
    ldp q4, q5, [x0, #64]
    ldp q6, q7, [x0, #96]
    ldp q8, q9, [x0, #128]
    ldp q10, q11, [x0, #160]
    ldp q12, q13, [x0, #192]
    ldp q14, q15, [x0, #224]
    ldp q16, q17, [x0, #256]
    ldp q18, q19, [x0, #288]
    ldp q20, q21, [x0, #320]
    ldp q22, q23, [x0, #352]
    ldp q24, q25, [x0, #384]
    ldp q26, q27, [x0, #416]
    ldp q28, q29, [x0, #448]
    ldp q30, q31, [x0, #480]
    add x0, x0, #512
    ldp x9, x10, [x0]
    msr fpcr, x9
    msr fpsr, x10
    ret
"#
);

/// The floating-point control register (the interface's name is
/// x86-64's MXCSR).
pub fn set_mxcsr(_value: u32) {
    unported("xstate::set_mxcsr")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoMemory;

//! Extended processor state (XSAVE; ARCHITECTURE "x87/SSE/AVX state").
//!
//! Which state components gala enables (XCR0: x87, SSE, AVX and, where
//! the CPU has them, the three AVX-512 components, as Windows 11 does),
//! their layout (the standard, non-compacted XSAVE format: the offsets
//! are architectural and checked against CPUID leaf 0xD), the user state
//! as a value (`XState`) and per thread (`XArea`, what a context switch
//! saves into), and saving and loading the current CPU's state.
//!
//! The kernel itself is soft-float: it never uses x87/SSE/AVX registers,
//! so in a trap or system call they still hold the user's state.

use core::alloc::Layout;
use core::arch::asm;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::cpu;

/// State components (XCR0 bits; XSTATE_* feature numbers in winnt.h).
pub const X87: u64 = 1 << 0;
pub const SSE: u64 = 1 << 1;
pub const AVX: u64 = 1 << 2;
pub const OPMASK: u64 = 1 << 5;
pub const ZMM_HI256: u64 = 1 << 6;
pub const HI16_ZMM: u64 = 1 << 7;
pub const LEGACY: u64 = X87 | SSE;
pub const AVX512: u64 = OPMASK | ZMM_HI256 | HI16_ZMM;
/// What gala enables when the CPU has it: Windows 11's user-mode set
/// (MPX, PKRU, AMX and the supervisor components stay off).
pub const SUPPORTED: u64 = LEGACY | AVX | AVX512;

/// The legacy (FXSAVE) region and the XSAVE header.
pub const LEGACY_SIZE: usize = 512;
pub const HEADER_SIZE: usize = 64;
/// Offset and size of each extended component in the standard format
/// (component number = index; 0 and 1 live in the legacy region).
pub const COMPONENTS: [(usize, usize); 8] = [
    (0, 160),     // x87: FCW..ST7 (FltSave's first 160 bytes)
    (160, 256),   // SSE: XMM0-15 (FltSave + 160)
    (576, 256),   // AVX: the upper halves of YMM0-15
    (960, 64),    // MPX BNDREGS (never enabled)
    (1024, 64),   // MPX BNDCSR (never enabled)
    (1088, 64),   // AVX-512 opmask k0-k7
    (1152, 512),  // AVX-512 ZMM_Hi256: upper halves of ZMM0-15
    (1664, 1024), // AVX-512 Hi16_ZMM: ZMM16-31
];
/// The largest area gala uses: every SUPPORTED component.
pub const MAX_SIZE: usize = 2688;

const MXCSR: usize = 24;
const MXCSR_MASK: usize = 28;
const XSTATE_BV: usize = LEGACY_SIZE;
const XCOMP_BV: usize = LEGACY_SIZE + 8;
const DEFAULT_MXCSR: u32 = 0x1f80;
const DEFAULT_FCW: u16 = 0x037f;

/// How a context switch saves state (read by `gala_context_switch`).
pub mod mode {
    /// No XSAVE: FXSAVE/FXRSTOR (x87 and SSE only).
    pub const FXSAVE: u8 = 0;
    pub const XSAVE: u8 = 1;
    /// XSAVEOPT (skips components in their initial state or unmodified
    /// since the XRSTOR from the same area).
    pub const XSAVEOPT: u8 = 2;
}

/// The save mode (`mode::*`), set once by the boot CPU.
#[unsafe(no_mangle)]
pub static GALA_XSAVE_MODE: AtomicU8 = AtomicU8::new(mode::FXSAVE);
/// The enabled components (XCR0); LEGACY without XSAVE. 0: not chosen yet.
static ENABLED: AtomicU64 = AtomicU64::new(0);
/// The MXCSR bits the CPU supports (FXSAVE's MXCSR_MASK).
static MXCSR_BITS: AtomicU32 = AtomicU32::new(0xffbf);

/// The components enabled (XCR0; x87 and SSE where there is no XSAVE).
pub fn enabled() -> u64 {
    match ENABLED.load(Ordering::Relaxed) {
        0 => LEGACY,
        m => m,
    }
}

/// Whether XSAVE is in use (CR4.OSXSAVE set).
pub fn uses_xsave() -> bool {
    GALA_XSAVE_MODE.load(Ordering::Relaxed) != mode::FXSAVE
}

/// The standard-format size of the components in `mask` (the legacy
/// region and the header at least).
pub fn size_of(mask: u64) -> usize {
    let mut size = LEGACY_SIZE + HEADER_SIZE;
    for (i, &(offset, len)) in COMPONENTS.iter().enumerate().skip(2) {
        if mask & (1 << i) != 0 {
            size = size.max(offset + len);
        }
    }
    size
}

/// The size of the enabled state (what CPUID.0xD.0:EBX reports for XCR0).
pub fn size() -> usize {
    size_of(enabled())
}

/// The components gala enables on this CPU: SUPPORTED ∩ what CPUID offers,
/// AVX-512 only whole and with AVX, each at its architectural offset.
fn choose() -> (u64, u8) {
    if cpu::cpuid(1, 0).ecx & (1 << 26) == 0 {
        return (LEGACY, mode::FXSAVE);
    }
    let leaf = cpu::cpuid(0xd, 0);
    if leaf.eax & 3 != 3 {
        return (LEGACY, mode::FXSAVE);
    }
    let mask = usable(u64::from(leaf.eax) | u64::from(leaf.edx) << 32);
    let optimized = cpu::cpuid(0xd, 1).eax & 1 != 0;
    (mask, if optimized { mode::XSAVEOPT } else { mode::XSAVE })
}

/// Of the components in `offered` (what CPUID offers, or a host's XCR0),
/// those gala can use: SUPPORTED ones at their architectural offsets
/// (CPUID leaf 0xD), AVX-512 only whole and with AVX. x87 and SSE always.
pub fn usable(offered: u64) -> u64 {
    let mut mask = (offered & SUPPORTED) | LEGACY;
    for (i, &(offset, len)) in COMPONENTS.iter().enumerate().skip(2) {
        if mask & (1 << i) == 0 {
            continue;
        }
        let sub = cpu::cpuid(0xd, i as u32);
        if sub.ebx as usize != offset || sub.eax as usize != len {
            mask &= !(1 << i);
        }
    }
    if mask & AVX == 0 || mask & AVX512 != AVX512 {
        mask &= !AVX512;
    }
    mask
}

/// Enables the extended state on this CPU (every CPU, in its own setup
/// path, after a kernel handover too): CR4.OSXSAVE and XCR0. The first
/// call (the boot CPU) chooses the components.
pub fn enable_here() {
    if ENABLED.load(Ordering::Acquire) == 0 {
        MXCSR_BITS.store(mxcsr_mask(&save_legacy_only()), Ordering::Relaxed);
        let (mask, how) = choose();
        GALA_XSAVE_MODE.store(how, Ordering::Relaxed);
        ENABLED.store(mask, Ordering::Release);
    }
    if !uses_xsave() {
        return;
    }
    let mask = enabled();
    // SAFETY: CPUID reports XSAVE, so CR4.OSXSAVE exists; XCR0 gets
    // components CPUID offers, x87 and SSE included, AVX-512 only whole
    // and with AVX (the architectural rules for XSETBV).
    unsafe {
        let cr4 = cpu::read_cr4() | (1 << 18);
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        asm!("xsetbv", in("ecx") 0, in("eax") mask as u32, in("edx") (mask >> 32) as u32,
             options(nomem, nostack, preserves_flags));
    }
    // Start from the initial state, and from an XRSTOR of a buffer that
    // is no thread's area (XSAVEOPT's "unmodified since the XRSTOR from
    // this address" must not match an area from before, a handover's).
    restore(&XState::initial());
}

/// x87/SSE/AVX(-512) register state in the standard XSAVE layout: the
/// legacy region (CONTEXT.FltSave's layout), the header (XSTATE_BV: the
/// components not in their initial state), then the extended components.
/// Only the first `size()` bytes are used.
#[repr(C, align(64))]
#[derive(Clone)]
pub struct XState([u8; MAX_SIZE]);

impl core::fmt::Debug for XState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("XState").field("bv", &self.bv()).field("mxcsr", &self.mxcsr()).finish()
    }
}

impl PartialEq for XState {
    fn eq(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}
impl Eq for XState {}

impl XState {
    /// The power-on state: FCW 0x037f, MXCSR 0x1f80 (all exceptions
    /// masked), everything else zero; x87 and SSE marked present so that
    /// XRSTOR loads these values.
    pub const fn initial() -> Self {
        let mut a = [0; MAX_SIZE];
        a[0] = DEFAULT_FCW as u8;
        a[1] = (DEFAULT_FCW >> 8) as u8;
        a[MXCSR] = DEFAULT_MXCSR as u8;
        a[MXCSR + 1] = (DEFAULT_MXCSR >> 8) as u8;
        a[XSTATE_BV] = LEGACY as u8;
        XState(a)
    }

    /// The state with `legacy` as its x87/SSE part (an FXSAVE image).
    pub fn from_legacy(legacy: &[u8; LEGACY_SIZE]) -> Self {
        let mut s = Self::initial();
        s.set_legacy(legacy);
        s
    }

    /// The state as the processor saves it (the standard XSAVE image; the
    /// first `size()` bytes are in use): what the Linux signal frame's
    /// FPU state and gala-um's host context hold.
    pub fn bytes(&self) -> &[u8; MAX_SIZE] {
        &self.0
    }

    pub fn bytes_mut(&mut self) -> &mut [u8; MAX_SIZE] {
        &mut self.0
    }

    /// The FXSAVE image (CONTEXT.FltSave).
    pub fn legacy(&self) -> &[u8; LEGACY_SIZE] {
        self.0[..LEGACY_SIZE].try_into().unwrap_or(&[0; LEGACY_SIZE])
    }

    pub fn set_legacy(&mut self, legacy: &[u8; LEGACY_SIZE]) {
        // The XSAVE header is not part of the legacy region; bytes 464..512
        // are software-available and kept.
        self.0[..LEGACY_SIZE].copy_from_slice(legacy);
        self.set_bv(self.bv() | LEGACY);
    }

    /// XSTATE_BV: which components hold values (the others are in their
    /// initial state, whatever their bytes say).
    pub fn bv(&self) -> u64 {
        u64::from_le_bytes(self.0[XSTATE_BV..XSTATE_BV + 8].try_into().unwrap_or([0; 8]))
    }

    pub fn set_bv(&mut self, bv: u64) {
        self.0[XSTATE_BV..XSTATE_BV + 8].copy_from_slice(&bv.to_le_bytes());
    }

    pub fn mxcsr(&self) -> u32 {
        u32::from_le_bytes(self.0[MXCSR..MXCSR + 4].try_into().unwrap_or([0; 4]))
    }

    pub fn set_mxcsr(&mut self, value: u32) {
        self.0[MXCSR..MXCSR + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Component `i` (2 and up: the extended ones) as bytes.
    pub fn component(&self, i: usize) -> &[u8] {
        let (offset, len) = COMPONENTS[i];
        &self.0[offset..offset + len]
    }

    /// Component `i`'s bytes to change; marks it present.
    pub fn component_mut(&mut self, i: usize) -> &mut [u8] {
        self.set_bv(self.bv() | 1 << i);
        let (offset, len) = COMPONENTS[i];
        &mut self.0[offset..offset + len]
    }

    /// Writes the initial values of the components (of `mask`) that
    /// XSTATE_BV says are in their initial state, so the bytes can be read
    /// as data (XSAVEOPT does not write such components). XSTATE_BV stays.
    pub fn normalize(&mut self, mask: u64) {
        let bv = self.bv();
        if mask & X87 != 0 && bv & X87 == 0 {
            // FCW 0x037f, everything else of the x87 part (FSW, FTW, FOP,
            // FIP, FDP, ST0-7) zero; MXCSR (bytes 24..32) is SSE's.
            let mxcsr: [u8; 8] = self.0[MXCSR..MXCSR + 8].try_into().unwrap_or([0; 8]);
            self.0[..160].fill(0);
            self.0[..2].copy_from_slice(&DEFAULT_FCW.to_le_bytes());
            self.0[MXCSR..MXCSR + 8].copy_from_slice(&mxcsr);
        }
        if mask & SSE != 0 && bv & SSE == 0 {
            self.0[160..416].fill(0);
        }
        for (i, &(offset, len)) in COMPONENTS.iter().enumerate().skip(2) {
            if mask & (1 << i) != 0 && bv & (1 << i) == 0 {
                self.0[offset..offset + len].fill(0);
            }
        }
    }

    /// Makes the state safe to load with XRSTOR/FXRSTOR, however it was
    /// filled in (from a program's CONTEXT, a handover record): only the
    /// enabled components, the standard format, reserved header bytes
    /// zero, MXCSR with only the bits `mxcsr_mask` allows.
    pub fn sanitize(&mut self, enabled: u64, mxcsr_mask: u32) {
        self.set_bv(self.bv() & enabled);
        self.0[XCOMP_BV..LEGACY_SIZE + HEADER_SIZE].fill(0);
        self.set_mxcsr(self.mxcsr() & mxcsr_mask);
    }
}

/// MXCSR bits the CPU supports (FXSAVE's MXCSR_MASK; 0 means the default).
fn mxcsr_mask(state: &XState) -> u32 {
    match u32::from_le_bytes(state.0[MXCSR_MASK..MXCSR_MASK + 4].try_into().unwrap_or([0; 4])) {
        0 => 0xffbf,
        m => m,
    }
}

/// The current CPU's user state (the kernel never uses these registers,
/// so in a trap or system call they hold the user's), with the bytes of
/// components in their initial state filled in (`normalize`).
pub fn save() -> XState {
    let mut state = XState::initial();
    let p = state.0.as_mut_ptr();
    if uses_xsave() {
        state.set_bv(0);
        // SAFETY: the buffer is 64-byte aligned and MAX_SIZE >= the size
        // of every enabled component (`choose`); EDX:EAX = all ones asks
        // for every enabled component.
        unsafe { asm!("xsave64 [{}]", in(reg) p, in("eax") -1, in("edx") -1, options(nostack)) };
    } else {
        // SAFETY: FXSAVE writes 512 bytes to a 16-byte aligned buffer.
        unsafe { asm!("fxsave64 [{}]", in(reg) p, options(nostack)) };
    }
    state.normalize(enabled());
    state
}

/// Loads `state` into this CPU's registers (as user code may have filled
/// it in: `sanitize`d first, since XRSTOR and FXRSTOR fault on reserved
/// bits).
pub fn restore(state: &XState) {
    let mut s = state.clone();
    // x87 and SSE always loaded from the bytes (MXCSR with them).
    s.normalize(LEGACY);
    s.set_bv(s.bv() | LEGACY);
    s.sanitize(enabled(), MXCSR_BITS.load(Ordering::Relaxed));
    let p = s.0.as_ptr();
    if uses_xsave() {
        // SAFETY: a 64-byte aligned standard-format area whose header has
        // only enabled components and zero reserved bytes, and whose MXCSR
        // has only supported bits.
        unsafe {
            asm!("xrstor64 [{}]", in(reg) p, in("eax") -1, in("edx") -1, options(nostack, readonly))
        };
    } else {
        // SAFETY: FXRSTOR reads 512 bytes from a 16-byte aligned buffer
        // whose MXCSR has only supported bits.
        unsafe { asm!("fxrstor64 [{}]", in(reg) p, options(nostack, readonly)) };
    }
}

/// FXSAVE alone (for MXCSR_MASK).
fn save_legacy_only() -> XState {
    let mut state = XState::initial();
    // SAFETY: FXSAVE writes 512 bytes to a 64-byte aligned buffer.
    unsafe { asm!("fxsave64 [{}]", in(reg) state.0.as_mut_ptr(), options(nostack)) };
    state
}

/// Sets MXCSR of the current state (reserved bits cleared).
pub fn set_mxcsr(value: u32) {
    let mut state = save();
    state.set_mxcsr(value);
    restore(&state);
}

/// A thread's save area for context switches: `size()` bytes, 64-byte
/// aligned, on the heap (allocated fallibly). `gala_context_switch`
/// saves into it and loads from it; `load` reads it as an `XState`.
#[repr(transparent)]
pub struct XArea(NonNull<u8>);

// SAFETY: the area is plain bytes owned by its thread's context; the
// scheduler serializes access (only the switch writes it, and `load` is
// only called for a thread on no CPU).
unsafe impl Send for XArea {}
// SAFETY: as above.
unsafe impl Sync for XArea {}

/// Allocation failed: no memory for a save area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoMemory;

impl XArea {
    fn layout() -> Layout {
        // MAX_SIZE when the components are not chosen yet (before the boot
        // CPU's setup; nothing allocates areas that early).
        Layout::from_size_align(size().max(LEGACY_SIZE + HEADER_SIZE), 64)
            .unwrap_or(Layout::new::<XState>())
    }

    /// An area holding the initial state.
    pub fn new() -> Result<Self, NoMemory> {
        let layout = Self::layout();
        // SAFETY: the layout has a non-zero size.
        let p = NonNull::new(unsafe { alloc::alloc::alloc(layout) }).ok_or(NoMemory)?;
        let initial = XState::initial();
        // SAFETY: `p` is a fresh allocation of `layout.size()` bytes,
        // which is at most MAX_SIZE.
        unsafe { p.as_ptr().copy_from_nonoverlapping(initial.0.as_ptr(), layout.size()) };
        Ok(XArea(p))
    }

    /// The address `gala_context_switch` saves to and loads from.
    pub fn as_ptr(&self) -> *mut u8 {
        self.0.as_ptr()
    }

    /// The state saved in the area, readable as data (`normalize`d).
    ///
    /// # Safety
    /// Nothing may write the area meanwhile: its thread is on no CPU.
    pub unsafe fn load(&self) -> XState {
        let mut state = XState::initial();
        let len = Self::layout().size();
        // SAFETY: the area has `len` bytes; the caller excludes writers.
        unsafe { self.0.as_ptr().copy_to_nonoverlapping(state.0.as_mut_ptr(), len) };
        if !uses_xsave() {
            state.set_bv(LEGACY);
        }
        state.normalize(enabled());
        state
    }
}

impl Drop for XArea {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout (the size does not
        // change once chosen, and areas are only made after that).
        unsafe { alloc::alloc::dealloc(self.0.as_ptr(), Self::layout()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_layout() {
        assert_eq!(size_of(LEGACY), 576);
        assert_eq!(size_of(LEGACY | AVX), 832);
        assert_eq!(size_of(SUPPORTED), MAX_SIZE);
        let s = XState::initial();
        assert_eq!(s.mxcsr(), 0x1f80);
        assert_eq!(s.bv(), LEGACY);
    }

    #[test]
    fn normalize_fills_initial_components() {
        let mut s = XState([0xee; MAX_SIZE]);
        s.set_bv(0);
        s.set_mxcsr(0x1f80);
        s.normalize(LEGACY | AVX);
        assert_eq!(&s.0[..2], &[0x7f, 0x03]);
        assert!(s.0[2..24].iter().all(|&b| b == 0));
        assert_eq!(s.mxcsr(), 0x1f80);
        assert!(s.0[160..416].iter().all(|&b| b == 0));
        assert!(s.component(2).iter().all(|&b| b == 0));
        // Not enabled: left alone.
        assert!(s.component(5).iter().all(|&b| b == 0xee));
        assert_eq!(s.bv(), 0);
    }

    #[test]
    fn sanitize_and_component_mut() {
        let mut s = XState::initial();
        s.0[XCOMP_BV] = 1;
        s.0[LEGACY_SIZE + 20] = 5;
        s.set_bv(0xff);
        s.set_mxcsr(0xffff_ffff);
        s.sanitize(LEGACY | AVX, 0xffbf);
        assert_eq!(s.bv(), 7);
        assert!(s.0[XCOMP_BV..LEGACY_SIZE + HEADER_SIZE].iter().all(|&b| b == 0));
        assert_eq!(s.mxcsr(), 0xffbf);
        let mut s = XState::initial();
        s.component_mut(2)[0] = 9;
        assert_eq!(s.bv(), 7);
        assert_eq!(s.0[576], 9);
    }
}

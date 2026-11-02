//! The x64 CONTEXT record (public layout, 0x4d0 bytes) and its extension
//! for the extended state (CONTEXT_EX and the XSTATE area, as winnt.h and
//! the WDK's wdm.h lay them out): what NtCreateThread and NtContinue
//! read, what exception and APC dispatch put on the user stack, and what
//! NtGetContextThread/NtSetContextThread exchange.
//!
//! CONTEXT_EX (three CONTEXT_CHUNKs: All, Legacy, XState, each a signed
//! offset from the CONTEXT_EX and a length) follows the CONTEXT. The
//! XState chunk is an XSAVE header (Mask: the features present, then the
//! compaction mask, zero here: gala uses the standard format) followed by
//! the extended components at their standard offsets less 512 (x87 and
//! SSE live in CONTEXT.FltSave).

use super::platform::{UserRegs, XState};
use crate::arch::xstate::{self, LEGACY, LEGACY_SIZE};

pub const SIZE: usize = 0x4d0;
/// sizeof(CONTEXT_EX).
pub const EX_SIZE: usize = 0x18;
/// The XSTATE header (XSAVE_AREA_HEADER).
const HEADER: usize = 64;
const CONTEXT_FLAGS: usize = 0x30;
const MX_CSR: usize = 0x34;
const SEG_CS: usize = 0x38;
const SEG_DS: usize = 0x3a;
const SEG_SS: usize = 0x42;
const EFLAGS: usize = 0x44;
const DR0: usize = 0x48;
/// Rax..R15 (16 registers, in `UserRegs::gpr` order).
const GPR: usize = 0x78;
const RSP_INDEX: usize = 4;
const RIP: usize = 0xf8;
/// FltSave (XMM_SAVE_AREA32): ControlWord and MxCsr.
const FLT_CONTROL_WORD: usize = 0x100;
const FLT_MX_CSR: usize = 0x118;
/// Bytes that hold the integer and control state.
pub const INTEGER_PART: usize = RIP + 8;

pub const CONTEXT_AMD64: u32 = 0x10_0000;
pub const CONTEXT_CONTROL: u32 = 0x10_0001;
pub const CONTEXT_INTEGER: u32 = 0x10_0002;
pub const CONTEXT_SEGMENTS: u32 = 0x10_0004;
pub const CONTEXT_FLOATING_POINT: u32 = 0x10_0008;
pub const CONTEXT_DEBUG_REGISTERS: u32 = 0x10_0010;
pub const CONTEXT_XSTATE: u32 = 0x10_0040;
/// CONTEXT_AMD64 | CONTROL | INTEGER | FLOATING_POINT.
pub const CONTEXT_FULL: u32 = 0x10_000b;
/// FltSave: the FXSAVE image (512 bytes); Xmm0 is at FltSave + 0xa0.
const FLT_SAVE: usize = 0x100;
pub(super) const USER_CS: u16 = 0x33;
pub(super) const USER_DS: u16 = 0x2b;
const DEFAULT_MXCSR: u32 = 0x1f80;
const DEFAULT_FPU_CONTROL: u16 = 0x27f;
const RFLAGS_IF: u32 = 0x200;
/// The most XSTATE area gala reads or writes (header and every extended
/// component it can enable).
pub const MAX_XSTATE: usize = xstate::MAX_SIZE - LEGACY_SIZE;

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    b[off..off + v.len()].copy_from_slice(v);
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap_or([0; 4]))
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// A CONTEXT's ContextFlags.
pub fn flags(ctx: &[u8]) -> u32 {
    u32_at(ctx, CONTEXT_FLAGS)
}

pub fn set_flags(ctx: &mut [u8], flags: u32) {
    put(ctx, CONTEXT_FLAGS, &flags.to_le_bytes());
}

/// Whether `flags` asks for all of `part` (CONTEXT_AMD64 | the part's bit).
pub fn has(flags: u32, part: u32) -> bool {
    flags & part == part
}

/// The extended components programs see (beyond x87 and SSE).
pub fn extended_features() -> u64 {
    super::platform::xstate_enabled() & !LEGACY
}

/// The length of the XSTATE area for the enabled features (header and
/// components, standard format); 0 when there are none beyond x87/SSE.
pub fn xstate_len() -> usize {
    let ext = extended_features();
    if ext == 0 { 0 } else { xstate::size_of(ext | LEGACY) - LEGACY_SIZE }
}

/// Writes the parts of a CONTEXT that `flags` names, from user registers
/// `regs` and x87/SSE state `fx` (default state if `None`); other bytes
/// are left as they are, ContextFlags too.
pub fn fill(b: &mut [u8], flags: u32, regs: &UserRegs, fx: Option<&XState>) {
    if has(flags, CONTEXT_CONTROL) {
        put(b, SEG_CS, &USER_CS.to_le_bytes());
        put(b, SEG_SS, &USER_DS.to_le_bytes());
        put(b, EFLAGS, &((regs.rflags as u32) | RFLAGS_IF).to_le_bytes());
        put(b, GPR + 8 * RSP_INDEX, &regs.gpr[RSP_INDEX].to_le_bytes());
        put(b, RIP, &regs.rip.to_le_bytes());
    }
    if has(flags, CONTEXT_INTEGER) {
        for (i, r) in regs.gpr.iter().enumerate().filter(|(i, _)| *i != RSP_INDEX) {
            put(b, GPR + 8 * i, &r.to_le_bytes());
        }
    }
    if has(flags, CONTEXT_SEGMENTS) {
        for seg in (SEG_DS..SEG_SS).step_by(2) {
            put(b, seg, &USER_DS.to_le_bytes());
        }
    }
    if has(flags, CONTEXT_FLOATING_POINT) {
        match fx {
            Some(fx) => put(b, FLT_SAVE, fx.legacy()),
            None => {
                b[FLT_SAVE..FLT_SAVE + LEGACY_SIZE].fill(0);
                put(b, FLT_CONTROL_WORD, &DEFAULT_FPU_CONTROL.to_le_bytes());
                put(b, FLT_MX_CSR, &DEFAULT_MXCSR.to_le_bytes());
            }
        }
        // MxCsr duplicates FltSave.MxCsr.
        let mxcsr = u32_at(b, FLT_MX_CSR);
        put(b, MX_CSR, &mxcsr.to_le_bytes());
    }
    if has(flags, CONTEXT_DEBUG_REGISTERS) {
        b[DR0..GPR].fill(0);
    }
}

/// A full CONTEXT (CONTEXT_FULL | CONTEXT_SEGMENTS) for user registers
/// `regs` and x87/SSE state `fx` (default state if `None`).
pub fn to_bytes(regs: &UserRegs, fx: Option<&XState>) -> [u8; SIZE] {
    let mut b = [0u8; SIZE];
    let flags = CONTEXT_FULL | CONTEXT_SEGMENTS;
    set_flags(&mut b, flags);
    fill(&mut b, flags, regs, fx);
    b
}

/// MxCsr of a full CONTEXT (`SIZE` bytes), if it has floating-point state.
pub fn mxcsr(ctx: &[u8]) -> Option<u32> {
    has(flags(ctx), CONTEXT_FLOATING_POINT).then(|| u32_at(ctx, MX_CSR))
}

/// The x87/SSE state of a full CONTEXT (`SIZE` bytes), if it has one:
/// FltSave, with MXCSR taken from MxCsr.
pub fn float_state(ctx: &[u8]) -> Option<[u8; LEGACY_SIZE]> {
    let mxcsr = mxcsr(ctx)?;
    let mut fx = [0u8; LEGACY_SIZE];
    fx.copy_from_slice(&ctx[FLT_SAVE..FLT_SAVE + LEGACY_SIZE]);
    fx[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    Some(fx)
}

/// The integer and control registers of a CONTEXT (at least
/// `INTEGER_PART` bytes).
pub fn from_bytes(ctx: &[u8]) -> UserRegs {
    let mut regs = UserRegs::default();
    apply(&mut regs, ctx, CONTEXT_CONTROL | CONTEXT_INTEGER);
    regs
}

/// Overwrites the registers of `regs` that `flags` names (CONTROL: rsp,
/// rip, rflags; INTEGER: the others) with the CONTEXT's.
pub fn apply(regs: &mut UserRegs, ctx: &[u8], flags: u32) {
    let word = |off: usize| u64_at(ctx, off);
    if has(flags, CONTEXT_INTEGER) {
        for (i, r) in regs.gpr.iter_mut().enumerate().filter(|(i, _)| *i != RSP_INDEX) {
            *r = word(GPR + 8 * i);
        }
    }
    if has(flags, CONTEXT_CONTROL) {
        regs.gpr[RSP_INDEX] = word(GPR + 8 * RSP_INDEX);
        regs.rip = word(RIP);
        regs.rflags = u64::from(u32_at(ctx, EFLAGS));
    }
}

/// A CONTEXT_EX for a CONTEXT followed (at `xstate_at` bytes from the
/// CONTEXT's start) by an XSTATE area of `xstate_len` bytes.
pub fn context_ex(xstate_at: usize, xstate_len: usize) -> [u8; EX_SIZE] {
    let mut ex = [0u8; EX_SIZE];
    let back = (-(SIZE as i32)).to_le_bytes();
    // All: from the CONTEXT to the end of the XSTATE area.
    put(&mut ex, 0, &back);
    put(&mut ex, 4, &((xstate_at + xstate_len) as u32).to_le_bytes());
    // Legacy: the CONTEXT.
    put(&mut ex, 8, &back);
    put(&mut ex, 12, &(SIZE as u32).to_le_bytes());
    // XState: relative to the CONTEXT_EX.
    put(&mut ex, 16, &((xstate_at - SIZE) as i32).to_le_bytes());
    put(&mut ex, 20, &(xstate_len as u32).to_le_bytes());
    ex
}

/// Where a CONTEXT_EX (read from just after a CONTEXT) puts the XSTATE
/// area: (offset from the CONTEXT's start, length). None if it does not
/// describe a CONTEXT followed by an XSTATE area with at least a header.
pub fn xstate_area(ex: &[u8; EX_SIZE]) -> Option<(u64, usize)> {
    let legacy_offset = u32_at(ex, 8) as i32;
    let legacy_len = u32_at(ex, 12);
    let offset = u32_at(ex, 16) as i32;
    let len = u32_at(ex, 20) as usize;
    if legacy_offset != -(SIZE as i32) || legacy_len != SIZE as u32 {
        return None;
    }
    // After the CONTEXT_EX, within reach, room for the header.
    if offset < EX_SIZE as i32 || offset > 0x1_0000 || !(HEADER..=0x1_0000).contains(&len) {
        return None;
    }
    Some((SIZE as u64 + offset as u64, len))
}

/// The XSTATE area for `state`: the header's Mask is the extended
/// features of `wanted` that hold values (the others are in their initial
/// state, as GetXStateFeaturesMask then reports), and each wanted
/// feature's bytes. `len` bytes are written (`xstate_len()` at most).
pub fn xstate_bytes(state: &XState, wanted: u64, len: usize) -> [u8; MAX_XSTATE] {
    let mut area = [0u8; MAX_XSTATE];
    let ext = wanted & extended_features();
    put(&mut area, 0, &(state.bv() & ext).to_le_bytes());
    for (i, &(offset, size)) in xstate::COMPONENTS.iter().enumerate().skip(2) {
        let at = offset - LEGACY_SIZE;
        if ext & (1 << i) != 0 && at + size <= len.min(MAX_XSTATE) {
            area[at..at + size].copy_from_slice(state.component(i));
        }
    }
    area
}

/// Applies an XSTATE area (`area`: as much of it as the CONTEXT_EX
/// describes) to `state`: the features in the header's Mask take the
/// area's bytes; the other enabled extended features return to their
/// initial state (as XRSTOR does). An area in the compacted format, or
/// one too short for a feature it names, leaves that feature alone.
pub fn apply_xstate(state: &mut XState, area: &[u8]) {
    if area.len() < HEADER {
        return;
    }
    let mask = u64_at(area, 0) & extended_features();
    if u64_at(area, 8) & (1 << 63) != 0 {
        return; // compacted: gala reports CompactionEnabled = 0
    }
    for (i, &(offset, size)) in xstate::COMPONENTS.iter().enumerate().skip(2) {
        let bit = 1 << i;
        if extended_features() & bit == 0 {
            continue;
        }
        let at = offset - LEGACY_SIZE;
        if mask & bit == 0 {
            state.set_bv(state.bv() & !bit);
        } else if at + size <= area.len() {
            state.component_mut(i).copy_from_slice(&area[at..at + size]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_layout() {
        let mut regs = UserRegs::default();
        for (i, r) in regs.gpr.iter_mut().enumerate() {
            *r = 0x1000 + i as u64;
        }
        regs.rip = 0x1_4000_1000;
        regs.rflags = 0x202;
        let b = to_bytes(&regs, None);
        let back = from_bytes(&b);
        assert_eq!(back.gpr, regs.gpr);
        assert_eq!(back.rip, regs.rip);
        assert_eq!(back.rflags, 0x202);
        // Rcx at 0x80, Rsp at 0x98 (public layout).
        assert_eq!(u64::from_le_bytes(b[0x80..0x88].try_into().unwrap()), 0x1001);
        assert_eq!(u64::from_le_bytes(b[0x98..0xa0].try_into().unwrap()), 0x1004);
        assert_eq!(u16::from_le_bytes([b[0x38], b[0x39]]), 0x33);
        assert_eq!(mxcsr(&b), Some(0x1f80));
        assert_eq!(flags(&b), 0x10_000f);
    }

    #[test]
    fn parts_follow_the_flags() {
        let mut regs = UserRegs::default();
        regs.gpr = [7; 16];
        regs.rip = 0x1234;
        let mut b = [0xeeu8; SIZE];
        fill(&mut b, CONTEXT_CONTROL, &regs, None);
        assert_eq!(u64_at(&b, RIP), 0x1234);
        assert_eq!(u64_at(&b, GPR + 8 * RSP_INDEX), 7);
        assert_eq!(b[GPR], 0xee, "Rax is CONTEXT_INTEGER's");
        assert_eq!(b[FLT_SAVE], 0xee, "FltSave is CONTEXT_FLOATING_POINT's");
        let mut back = UserRegs::default();
        back.gpr[0] = 3;
        apply(&mut back, &b, CONTEXT_CONTROL);
        assert_eq!((back.gpr[0], back.gpr[RSP_INDEX], back.rip), (3, 7, 0x1234));
    }

    #[test]
    fn float_state_round_trip() {
        let mut fx = XState::initial();
        let mut legacy = *fx.legacy();
        legacy[24..28].copy_from_slice(&0x7f80u32.to_le_bytes()); // MXCSR
        legacy[0xa0] = 0xab; // xmm0, lowest byte
        fx.set_legacy(&legacy);
        let mut b = to_bytes(&UserRegs::default(), Some(&fx));
        assert_eq!(b[0x1a0], 0xab, "Xmm0 at 0x1a0");
        assert_eq!(u32::from_le_bytes(b[0x34..0x38].try_into().unwrap()), 0x7f80);
        assert_eq!(u32::from_le_bytes(b[0x118..0x11c].try_into().unwrap()), 0x7f80);
        // MxCsr (0x34) wins over FltSave.MxCsr when resuming.
        b[0x34..0x38].copy_from_slice(&0x3f80u32.to_le_bytes());
        let back = float_state(&b).unwrap();
        assert_eq!(back[0xa0], 0xab);
        assert_eq!(u32::from_le_bytes(back[24..28].try_into().unwrap()), 0x3f80);
        // Without CONTEXT_FLOATING_POINT there is no state to load.
        b[0x30..0x34].copy_from_slice(&0x10_0003u32.to_le_bytes());
        assert!(float_state(&b).is_none());
    }

    #[test]
    fn context_ex_layout() {
        // As in the exception frame: XSTATE at 0x5c0, AVX only (320 bytes).
        let ex = context_ex(0x5c0, 320);
        assert_eq!(u32_at(&ex, 0) as i32, -0x4d0);
        assert_eq!(u32_at(&ex, 4), 0x5c0 + 320);
        assert_eq!((u32_at(&ex, 8) as i32, u32_at(&ex, 12)), (-0x4d0, 0x4d0));
        assert_eq!((u32_at(&ex, 16), u32_at(&ex, 20)), (0xf0, 320));
        assert_eq!(xstate_area(&ex), Some((0x5c0, 320)));
        let mut bad = ex;
        bad[12] = 0;
        assert_eq!(xstate_area(&bad), None);
    }
}

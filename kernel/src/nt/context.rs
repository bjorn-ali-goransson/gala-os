//! The x64 CONTEXT record (public layout, 0x4d0 bytes): what NtCreateThread
//! and NtContinue read and what the kernel hands LdrInitializeThunk.

use super::platform::FxState;
use super::platform::UserRegs;

pub const SIZE: usize = 0x4d0;
const CONTEXT_FLAGS: usize = 0x30;
const MX_CSR: usize = 0x34;
const SEG_CS: usize = 0x38;
const SEG_DS: usize = 0x3a;
const SEG_SS: usize = 0x42;
const EFLAGS: usize = 0x44;
/// Rax..R15 (16 registers, in `UserRegs::gpr` order).
const GPR: usize = 0x78;
const RIP: usize = 0xf8;
/// FltSave (XMM_SAVE_AREA32): ControlWord and MxCsr.
const FLT_CONTROL_WORD: usize = 0x100;
const FLT_MX_CSR: usize = 0x118;
/// Bytes that hold the integer and control state.
pub const INTEGER_PART: usize = RIP + 8;

/// CONTEXT_AMD64 | CONTROL | INTEGER | FLOATING_POINT.
const CONTEXT_FULL: u32 = 0x10_000b;
const CONTEXT_FLOATING_POINT: u32 = 0x10_0008;
/// FltSave: the FXSAVE image (512 bytes); Xmm0 is at FltSave + 0xa0.
const FLT_SAVE: usize = 0x100;
const USER_CS: u16 = 0x33;
const USER_DS: u16 = 0x2b;
const DEFAULT_MXCSR: u32 = 0x1f80;
const DEFAULT_FPU_CONTROL: u16 = 0x27f;
const RFLAGS_IF: u32 = 0x200;

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    b[off..off + v.len()].copy_from_slice(v);
}

/// A full CONTEXT for user registers `regs` and x87/SSE state `fx`
/// (default state if `None`).
pub fn to_bytes(regs: &UserRegs, fx: Option<&FxState>) -> [u8; SIZE] {
    let mut b = [0u8; SIZE];
    put(&mut b, CONTEXT_FLAGS, &CONTEXT_FULL.to_le_bytes());
    put(&mut b, SEG_CS, &USER_CS.to_le_bytes());
    for seg in (SEG_DS..=SEG_SS).step_by(2) {
        put(&mut b, seg, &USER_DS.to_le_bytes());
    }
    put(&mut b, EFLAGS, &((regs.rflags as u32) | RFLAGS_IF).to_le_bytes());
    for (i, r) in regs.gpr.iter().enumerate() {
        put(&mut b, GPR + 8 * i, &r.to_le_bytes());
    }
    put(&mut b, RIP, &regs.rip.to_le_bytes());
    match fx {
        Some(fx) => put(&mut b, FLT_SAVE, &fx.0),
        None => {
            put(&mut b, FLT_CONTROL_WORD, &DEFAULT_FPU_CONTROL.to_le_bytes());
            put(&mut b, FLT_MX_CSR, &DEFAULT_MXCSR.to_le_bytes());
        }
    }
    // MxCsr duplicates FltSave.MxCsr.
    let mxcsr = [b[FLT_MX_CSR], b[FLT_MX_CSR + 1], b[FLT_MX_CSR + 2], b[FLT_MX_CSR + 3]];
    put(&mut b, MX_CSR, &mxcsr);
    b
}

/// MxCsr of a full CONTEXT (`SIZE` bytes), if it has floating-point state.
pub fn mxcsr(ctx: &[u8]) -> Option<u32> {
    let flags = u32::from_le_bytes(ctx[CONTEXT_FLAGS..CONTEXT_FLAGS + 4].try_into().ok()?);
    (flags & CONTEXT_FLOATING_POINT == CONTEXT_FLOATING_POINT)
        .then(|| u32::from_le_bytes(ctx[MX_CSR..MX_CSR + 4].try_into().unwrap_or([0; 4])))
}

/// The x87/SSE state of a full CONTEXT (`SIZE` bytes), if it has one:
/// FltSave, with MXCSR taken from MxCsr.
pub fn float_state(ctx: &[u8]) -> Option<FxState> {
    let mxcsr = mxcsr(ctx)?;
    let mut fx = FxState([0; 512]);
    fx.0.copy_from_slice(&ctx[FLT_SAVE..FLT_SAVE + 512]);
    fx.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    Some(fx)
}

/// The integer and control registers of a CONTEXT (at least
/// `INTEGER_PART` bytes).
// TODO: honour ContextFlags and restore the XMM/x87 state.
pub fn from_bytes(ctx: &[u8]) -> UserRegs {
    let word = |off: usize| u64::from_le_bytes(ctx[off..off + 8].try_into().unwrap_or([0; 8]));
    let mut regs = UserRegs::default();
    for (i, r) in regs.gpr.iter_mut().enumerate() {
        *r = word(GPR + 8 * i);
    }
    regs.rip = word(RIP);
    regs.rflags =
        u64::from(u32::from_le_bytes(ctx[EFLAGS..EFLAGS + 4].try_into().unwrap_or([0; 4])));
    regs
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
    }

    #[test]
    fn float_state_round_trip() {
        let mut fx = FxState([0; 512]);
        fx.0[24..28].copy_from_slice(&0x7f80u32.to_le_bytes()); // MXCSR
        fx.0[0xa0] = 0xab; // xmm0, lowest byte
        let mut b = to_bytes(&UserRegs::default(), Some(&fx));
        assert_eq!(b[0x1a0], 0xab, "Xmm0 at 0x1a0");
        assert_eq!(u32::from_le_bytes(b[0x34..0x38].try_into().unwrap()), 0x7f80);
        assert_eq!(u32::from_le_bytes(b[0x118..0x11c].try_into().unwrap()), 0x7f80);
        // MxCsr (0x34) wins over FltSave.MxCsr when resuming.
        b[0x34..0x38].copy_from_slice(&0x3f80u32.to_le_bytes());
        let back = float_state(&b).unwrap();
        assert_eq!(back.0[0xa0], 0xab);
        assert_eq!(u32::from_le_bytes(back.0[24..28].try_into().unwrap()), 0x3f80);
        // Without CONTEXT_FLOATING_POINT there is no state to load.
        b[0x30..0x34].copy_from_slice(&0x10_0003u32.to_le_bytes());
        assert!(float_state(&b).is_none());
    }
}

//! The x64 CONTEXT record (public layout, 0x4d0 bytes): what NtCreateThread
//! and NtContinue read and what the kernel hands LdrInitializeThunk.

use crate::arch::syscall::UserRegs;

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
const USER_CS: u16 = 0x33;
const USER_DS: u16 = 0x2b;
const DEFAULT_MXCSR: u32 = 0x1f80;
const DEFAULT_FPU_CONTROL: u16 = 0x27f;
const RFLAGS_IF: u32 = 0x200;

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    b[off..off + v.len()].copy_from_slice(v);
}

/// A full CONTEXT for user registers `regs` (default FPU/SSE state).
pub fn to_bytes(regs: &UserRegs) -> [u8; SIZE] {
    let mut b = [0u8; SIZE];
    put(&mut b, CONTEXT_FLAGS, &CONTEXT_FULL.to_le_bytes());
    put(&mut b, MX_CSR, &DEFAULT_MXCSR.to_le_bytes());
    put(&mut b, SEG_CS, &USER_CS.to_le_bytes());
    for seg in (SEG_DS..=SEG_SS).step_by(2) {
        put(&mut b, seg, &USER_DS.to_le_bytes());
    }
    put(&mut b, EFLAGS, &((regs.rflags as u32) | RFLAGS_IF).to_le_bytes());
    for (i, r) in regs.gpr.iter().enumerate() {
        put(&mut b, GPR + 8 * i, &r.to_le_bytes());
    }
    put(&mut b, RIP, &regs.rip.to_le_bytes());
    put(&mut b, FLT_CONTROL_WORD, &DEFAULT_FPU_CONTROL.to_le_bytes());
    put(&mut b, FLT_MX_CSR, &DEFAULT_MXCSR.to_le_bytes());
    b
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
        let b = to_bytes(&regs);
        let back = from_bytes(&b);
        assert_eq!(back.gpr, regs.gpr);
        assert_eq!(back.rip, regs.rip);
        assert_eq!(back.rflags, 0x202);
        // Rcx at 0x80, Rsp at 0x98 (public layout).
        assert_eq!(u64::from_le_bytes(b[0x80..0x88].try_into().unwrap()), 0x1001);
        assert_eq!(u64::from_le_bytes(b[0x98..0xa0].try_into().unwrap()), 0x1004);
        assert_eq!(u16::from_le_bytes([b[0x38], b[0x39]]), 0x33);
    }
}

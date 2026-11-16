//! ARM64's CONTEXT record (ARM64_NT_CONTEXT, winnt.h: 0x390 bytes): what
//! LdrInitializeThunk is given, what NtContinue and NtCreateThread read.
//!
//! ContextFlags (+0), Cpsr (+4), X0-X28 (+8), Fp (+0xf0), Lr (+0xf8),
//! Sp (+0x100), Pc (+0x108), V0-V31 (+0x110, 16 bytes each), Fpcr
//! (+0x310), Fpsr (+0x314), then the debug registers (+0x318). The parts:
//! CONTEXT_CONTROL is Fp, Lr, Sp, Pc and Cpsr; CONTEXT_INTEGER X0-X28;
//! CONTEXT_FLOATING_POINT V0-V31, Fpcr and Fpsr.
//!
//! Plain data (no architecture types), so the host's tests build it on any
//! machine (`abi/mod.rs`); `arm64/mod.rs` converts to the kernel's
//! registers and FP/SIMD state.

/// sizeof(ARM64_NT_CONTEXT).
pub const SIZE: usize = 0x390;

pub const CONTEXT_ARM64: u32 = 0x40_0000;
pub const CONTEXT_CONTROL: u32 = 0x40_0001;
pub const CONTEXT_INTEGER: u32 = 0x40_0002;
pub const CONTEXT_FLOATING_POINT: u32 = 0x40_0004;
pub const CONTEXT_FULL: u32 = 0x40_0007;

const FLAGS: usize = 0;
const CPSR: usize = 4;
const X0: usize = 8;
const FP: usize = 0xf0;
const LR: usize = 0xf8;
const SP: usize = 0x100;
const PC: usize = 0x108;
const V0: usize = 0x110;
const FPCR: usize = 0x310;
const FPSR: usize = 0x314;

/// V0-V31.
pub const VECTOR_BYTES: usize = 512;
/// The kernel's FP/SIMD save area (`xstate`): V0-V31, then FPCR and FPSR
/// as 8-byte words.
pub const FP_STATE_BYTES: usize = VECTOR_BYTES + 16;

/// X0-X28: the integer part's registers (Fp and Lr are the control part's).
const INTEGER_REGS: usize = 29;

/// The registers a CONTEXT holds besides the floating-point part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    /// X0-X30 (X29 the frame pointer, X30 the link register).
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub cpsr: u32,
}

fn u32_at(ctx: &[u8], off: usize) -> u32 {
    ctx.get(off..off + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(ctx: &[u8], off: usize) -> u64 {
    ctx.get(off..off + 8).and_then(|b| b.try_into().ok()).map_or(0, u64::from_le_bytes)
}

fn put(ctx: &mut [u8], off: usize, bytes: &[u8]) {
    if let Some(dst) = ctx.get_mut(off..off + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

pub fn flags(ctx: &[u8]) -> u32 {
    u32_at(ctx, FLAGS)
}

/// Whether `flags` asks for `part` (with CONTEXT_ARM64).
pub fn has(flags: u32, part: u32) -> bool {
    flags & part == part
}

/// `regs` as a CONTEXT: the control and integer parts, and the
/// floating-point part from the save area `fp` if given (CONTEXT_FULL
/// then).
pub fn to_bytes(regs: &Regs, fp: Option<&[u8; FP_STATE_BYTES]>) -> [u8; SIZE] {
    let mut b = [0u8; SIZE];
    let flags = if fp.is_some() { CONTEXT_FULL } else { CONTEXT_CONTROL | CONTEXT_INTEGER };
    put(&mut b, FLAGS, &flags.to_le_bytes());
    put(&mut b, CPSR, &regs.cpsr.to_le_bytes());
    for (i, x) in regs.x.iter().take(INTEGER_REGS).enumerate() {
        put(&mut b, X0 + 8 * i, &x.to_le_bytes());
    }
    put(&mut b, FP, &regs.x[29].to_le_bytes());
    put(&mut b, LR, &regs.x[30].to_le_bytes());
    put(&mut b, SP, &regs.sp.to_le_bytes());
    put(&mut b, PC, &regs.pc.to_le_bytes());
    if let Some(state) = fp {
        put(&mut b, V0, &state[..VECTOR_BYTES]);
        // The save area's FPCR and FPSR are 8-byte words; CONTEXT's are 4.
        put(&mut b, FPCR, &state[VECTOR_BYTES..VECTOR_BYTES + 4]);
        put(&mut b, FPSR, &state[VECTOR_BYTES + 8..VECTOR_BYTES + 12]);
    }
    b
}

/// Writes into `ctx` the parts `flags` names (NtGetContextThread): the
/// control part from `regs`, the integer part, the floating-point part
/// from the save area `fp`; the rest of `ctx` (its flags among them) as
/// it was.
pub fn fill(ctx: &mut [u8], flags: u32, regs: &Regs, fp: &[u8; FP_STATE_BYTES]) {
    let full = to_bytes(regs, Some(fp));
    let mut copy = |from: usize, to: usize| {
        if let (Some(dst), Some(src)) = (ctx.get_mut(from..to), full.get(from..to)) {
            dst.copy_from_slice(src);
        }
    };
    if has(flags, CONTEXT_CONTROL) {
        copy(CPSR, CPSR + 4);
        copy(FP, PC + 8);
    }
    if has(flags, CONTEXT_INTEGER) {
        copy(X0, FP);
    }
    if has(flags, CONTEXT_FLOATING_POINT) {
        copy(V0, FPSR + 4);
    }
}

/// The parts of `ctx` its `flags` name, applied to `regs`: the control
/// part (Fp, Lr, Sp, Pc, Cpsr) and the integer part (X0-X28). The
/// floating-point part is `float_state`'s.
pub fn apply(regs: &mut Regs, ctx: &[u8], flags: u32) {
    if has(flags, CONTEXT_CONTROL) {
        regs.x[29] = u64_at(ctx, FP);
        regs.x[30] = u64_at(ctx, LR);
        regs.sp = u64_at(ctx, SP);
        regs.pc = u64_at(ctx, PC);
        regs.cpsr = u32_at(ctx, CPSR);
    }
    if has(flags, CONTEXT_INTEGER) {
        for (i, x) in regs.x.iter_mut().take(INTEGER_REGS).enumerate() {
            *x = u64_at(ctx, X0 + 8 * i);
        }
    }
}

/// The control and integer parts of `ctx`, whatever its flags say (a new
/// thread's CONTEXT has every register).
pub fn from_bytes(ctx: &[u8]) -> Regs {
    let mut regs = Regs::default();
    apply(&mut regs, ctx, CONTEXT_CONTROL | CONTEXT_INTEGER);
    regs
}

/// The floating-point part in the save area's layout, if `ctx`'s flags
/// have it.
pub fn float_state(ctx: &[u8]) -> Option<[u8; FP_STATE_BYTES]> {
    if !has(flags(ctx), CONTEXT_FLOATING_POINT) {
        return None;
    }
    let mut state = [0u8; FP_STATE_BYTES];
    state[..VECTOR_BYTES].copy_from_slice(ctx.get(V0..V0 + VECTOR_BYTES)?);
    state[VECTOR_BYTES..VECTOR_BYTES + 4].copy_from_slice(ctx.get(FPCR..FPCR + 4)?);
    state[VECTOR_BYTES + 8..VECTOR_BYTES + 12].copy_from_slice(ctx.get(FPSR..FPSR + 4)?);
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Regs {
        let mut regs =
            Regs { sp: 0x7000_0000, pc: 0x1_4000_1000, cpsr: 0x6000_0000, ..Regs::default() };
        for (i, x) in regs.x.iter_mut().enumerate() {
            *x = 0x1000 + i as u64;
        }
        regs
    }

    #[test]
    fn layout_offsets() {
        let b = to_bytes(&sample(), None);
        assert_eq!(flags(&b), CONTEXT_CONTROL | CONTEXT_INTEGER);
        // Every part's flag carries CONTEXT_ARM64 (NtContinue refuses a
        // record without it).
        assert!(has(flags(&b), CONTEXT_ARM64));
        assert!(!has(0x10_0003, CONTEXT_ARM64)); // x64's CONTEXT_CONTROL | INTEGER
        assert_eq!(u32_at(&b, 4), 0x6000_0000); // Cpsr
        assert_eq!(u64_at(&b, 8), 0x1000); // X0
        assert_eq!(u64_at(&b, 8 + 8 * 28), 0x1000 + 28); // X28
        assert_eq!(u64_at(&b, 0xf0), 0x1000 + 29); // Fp
        assert_eq!(u64_at(&b, 0xf8), 0x1000 + 30); // Lr
        assert_eq!(u64_at(&b, 0x100), 0x7000_0000); // Sp
        assert_eq!(u64_at(&b, 0x108), 0x1_4000_1000); // Pc
    }

    #[test]
    fn round_trip_and_parts() {
        let regs = sample();
        let b = to_bytes(&regs, None);
        assert_eq!(from_bytes(&b), regs);
        // Only the control part: the integer registers stay as they were.
        let mut other = Regs::default();
        apply(&mut other, &b, CONTEXT_CONTROL);
        assert_eq!(other.x[0], 0);
        assert_eq!(
            (other.pc, other.sp, other.x[29], other.x[30]),
            (regs.pc, regs.sp, regs.x[29], regs.x[30])
        );
        // Only the integer part: X0-X28, not Fp, Lr or the control registers.
        let mut other = Regs::default();
        apply(&mut other, &b, CONTEXT_INTEGER);
        assert_eq!((other.x[28], other.x[29], other.pc), (regs.x[28], 0, 0));
    }

    #[test]
    fn fill_writes_the_named_parts_only() {
        let regs = sample();
        let mut fp = [0u8; FP_STATE_BYTES];
        fp[0] = 0xab;
        // The integer part only: X0-X28 written, the control part and V0
        // untouched (0xee).
        let mut b = [0xeeu8; SIZE];
        fill(&mut b, CONTEXT_INTEGER, &regs, &fp);
        assert_eq!(u64_at(&b, 8), 0x1000);
        assert_eq!(u64_at(&b, 8 + 8 * 28), 0x1000 + 28);
        assert_eq!(u64_at(&b, 0x108), 0xeeee_eeee_eeee_eeee);
        assert_eq!(b[0x110], 0xee);
        assert_eq!(u32_at(&b, 0), 0xeeee_eeee); // the flags are the caller's
        // The control part: Fp, Lr, Sp, Pc and Cpsr.
        fill(&mut b, CONTEXT_CONTROL, &regs, &fp);
        assert_eq!((u64_at(&b, 0x100), u64_at(&b, 0x108)), (regs.sp, regs.pc));
        assert_eq!((u64_at(&b, 0xf0), u64_at(&b, 0xf8)), (regs.x[29], regs.x[30]));
        assert_eq!(u32_at(&b, 4), regs.cpsr);
        // The floating-point part.
        fill(&mut b, CONTEXT_FLOATING_POINT, &regs, &fp);
        assert_eq!(b[0x110], 0xab);
    }

    #[test]
    fn float_part() {
        let mut state = [0u8; FP_STATE_BYTES];
        state[0] = 0xab; // V0's low byte
        state[VECTOR_BYTES] = 0x12; // FPCR
        state[VECTOR_BYTES + 8] = 0x34; // FPSR
        let b = to_bytes(&Regs::default(), Some(&state));
        assert_eq!(flags(&b), CONTEXT_FULL);
        assert_eq!((b[0x110], b[0x310], b[0x314]), (0xab, 0x12, 0x34));
        assert_eq!(float_state(&b), Some(state));
        assert_eq!(float_state(&to_bytes(&Regs::default(), None)), None);
    }
}

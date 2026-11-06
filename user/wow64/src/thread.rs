//! 32-bit threads and their contexts: NtCreateThread from 32-bit code
//! (the x86 CONTEXT and INITIAL_TEB as the 64-bit ones, with SegCs 0x23:
//! the kernel then makes a WOW64 thread), NtGetContextThread and
//! NtSetContextThread with the x86 CONTEXT (from the thread's CPU area
//! while it is in wow64.dll's code, from its registers when the kernel
//! stopped it in 32-bit code), NtContinue.

use crate::generic::{self, K, Slot};
use crate::mem::{self, Res, set32, u32_at, u64_at};
use crate::{
    Args, CPU_CONTEXT, NtStatus, STATUS_NOT_SUPPORTED, STATUS_SUCCESS, TEB64_CPU_SLOT, is_error,
    syscall, unsupported, unsupported_service,
};

pub const NT_CREATE_THREAD: u32 = 0x4e;
pub const NT_CONTINUE: u32 = 0x43;
pub const NT_QUEUE_APC_THREAD: u32 = 0x45;
pub const NT_GET_CONTEXT_THREAD: u32 = 0xf2;
pub const NT_SET_CONTEXT_THREAD: u32 = 0x18d;
const NT_QUERY_INFORMATION_THREAD: u32 = 0x25;

/// The services this module converts, with their argument counts.
pub static SERVICES: &[(u32, usize)] = &[
    (NT_CREATE_THREAD, 8),
    (NT_CONTINUE, 2),
    (NT_QUEUE_APC_THREAD, 5),
    (NT_GET_CONTEXT_THREAD, 2),
    (NT_SET_CONTEXT_THREAD, 2),
];

/// What a service returns: its status, or (NtContinue, NtSetContextThread
/// of the caller) the eax the caller's new context has.
pub fn call(number: u32, a: &Args, cpu: u64) -> NtStatus {
    let result = match number {
        NT_CREATE_THREAD => create(a),
        NT_GET_CONTEXT_THREAD => get_context(a, cpu),
        NT_SET_CONTEXT_THREAD => set_context(a.h(0), a.p(1), cpu),
        NT_CONTINUE => continue_with(a.p(0), a.u(1) as u8 != 0, cpu),
        NT_QUEUE_APC_THREAD => crate::dispatch::queue_apc(a),
        _ => {
            unsupported_service(number, b"thread");
            Ok(STATUS_NOT_SUPPORTED)
        }
    };
    result.unwrap_or_else(|e| e)
}

/// x86 CONTEXT (winnt.h).
pub(crate) mod x86 {
    pub const SIZE: usize = 0x2cc;
    pub const FLAGS: usize = 0x00;
    pub const FLOAT_SAVE: usize = 0x1c;
    pub const SEG_GS: usize = 0x8c;
    pub const SEG_FS: usize = 0x90;
    pub const SEG_ES: usize = 0x94;
    pub const SEG_DS: usize = 0x98;
    pub const EDI: usize = 0x9c;
    pub const ESI: usize = 0xa0;
    pub const EBX: usize = 0xa4;
    pub const EDX: usize = 0xa8;
    pub const ECX: usize = 0xac;
    pub const EAX: usize = 0xb0;
    pub const EBP: usize = 0xb4;
    pub const EIP: usize = 0xb8;
    pub const SEG_CS: usize = 0xbc;
    pub const EFLAGS: usize = 0xc0;
    pub const ESP: usize = 0xc4;
    pub const SEG_SS: usize = 0xc8;
    pub const EXTENDED: usize = 0xcc;
    pub const CONTROL: u32 = 0x1_0001;
    pub const INTEGER: u32 = 0x1_0002;
    pub const SEGMENTS: u32 = 0x1_0004;
    pub const FLOATING_POINT: u32 = 0x1_0008;
    pub const EXTENDED_REGISTERS: u32 = 0x1_0020;
    /// The integer registers in x64 CONTEXT order (Rax, Rcx, Rdx, Rbx,
    /// Rsp, Rbp, Rsi, Rdi).
    pub const GPR: [usize; 8] = [EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI];
}

/// x64 CONTEXT.
mod x64 {
    pub const SIZE: usize = 0x4d0;
    pub const FLAGS: usize = 0x30;
    pub const MX_CSR: usize = 0x34;
    pub const SEG_CS: usize = 0x38;
    pub const EFLAGS: usize = 0x44;
    pub const GPR: usize = 0x78;
    pub const RIP: usize = 0xf8;
    pub const FLT_SAVE: usize = 0x100;
    /// CONTEXT_AMD64 | CONTROL | INTEGER | SEGMENTS | FLOATING_POINT.
    pub const ALL: u32 = 0x10_000f;
    pub const CONTROL_INTEGER: u32 = 0x10_0003;
    pub const FLOATING_POINT: u32 = 0x10_0008;
}

pub(crate) const CS32: u32 = 0x23;
const DS32: u32 = 0x2b;
const FS32: u32 = 0x53;
const CURRENT_THREAD: u64 = u64::MAX - 1;

fn has(flags: u32, part: u32) -> bool {
    flags & part == part
}

/// NtCreateThread(*ThreadHandle, DesiredAccess, ObjectAttributes, Process,
/// *ClientId, *ThreadContext (x86), *InitialTeb (x86), CreateSuspended).
fn create(a: &Args) -> Res<NtStatus> {
    let kinds = [K::Oh, K::U, K::Oa, K::H, K::Ocid, K::U, K::U, K::U];
    let mut ctx32 = [0u8; x86::SIZE];
    mem::read(a.p(5), &mut ctx32)?;
    let mut teb32 = [0u8; 20];
    mem::read(a.p(6), &mut teb32)?;
    let mut ctx = [0u8; x64::SIZE];
    set32(&mut ctx, x64::FLAGS, x64::CONTROL_INTEGER);
    set32(&mut ctx, x64::MX_CSR, 0x1f80);
    ctx[x64::SEG_CS..x64::SEG_CS + 2].copy_from_slice(&(CS32 as u16).to_le_bytes());
    set32(&mut ctx, x64::EFLAGS, u32_at(&ctx32, x86::EFLAGS));
    for (i, &at) in x86::GPR.iter().enumerate() {
        let v = u64::from(u32_at(&ctx32, at));
        ctx[x64::GPR + 8 * i..x64::GPR + 8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    let eip = u64::from(u32_at(&ctx32, x86::EIP));
    ctx[x64::RIP..x64::RIP + 8].copy_from_slice(&eip.to_le_bytes());
    // INITIAL_TEB: OldStackBase, OldStackLimit, StackBase, StackLimit,
    // StackAllocationBase.
    let mut teb = [0u64; 5];
    for (i, v) in teb.iter_mut().enumerate() {
        *v = u32_at(&teb32, 4 * i).into();
    }
    let mut slots: [Slot; 16] = Default::default();
    let mut out = [0u64; 8];
    generic::convert_in(&kinds, &a.v[..8], &mut out, &mut slots)?;
    out[5] = ctx.as_ptr() as u64;
    out[6] = teb.as_ptr() as u64;
    let status = syscall(NT_CREATE_THREAD, &out);
    generic::convert_out(&kinds, &a.v[..8], &slots, status)?;
    Ok(status)
}

/// Whether `thread` is the calling thread (the pseudo-handle, or a handle
/// to it: its ClientId's thread).
fn is_current(thread: u64) -> Res<bool> {
    if thread == CURRENT_THREAD {
        return Ok(true);
    }
    let mut b = [0u64; 6];
    let args = [thread, 0, b.as_mut_ptr() as u64, 48, 0];
    let status = syscall(NT_QUERY_INFORMATION_THREAD, &args);
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    let teb: u64;
    // SAFETY: GS is the running thread's 64-bit TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags));
    }
    // SAFETY: the TEB is mapped while its thread runs; ClientId at 0x40.
    let me = unsafe { core::ptr::read((teb + 0x48) as *const u64) };
    Ok(b[3] == me)
}

/// The CPU area of another thread of this process (its 64-bit TEB's
/// TlsSlots[1]).
fn area_of(thread: u64) -> Res<u64> {
    let mut b = [0u64; 6];
    let args = [thread, 0, b.as_mut_ptr() as u64, 48, 0];
    let status = syscall(NT_QUERY_INFORMATION_THREAD, &args);
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    let mut slot = [0u8; 8];
    mem::read(b[1] + TEB64_CPU_SLOT, &mut slot)?;
    Ok(u64::from_le_bytes(slot))
}

/// Windows' selectors in an x86 CONTEXT (CS 0x23, SS/DS/ES/GS 0x2b, FS
/// 0x53: the TEB32).
pub(crate) fn set_segments(c: &mut [u8; x86::SIZE]) {
    for (at, v) in [
        (x86::SEG_CS, CS32),
        (x86::SEG_SS, DS32),
        (x86::SEG_GS, DS32),
        (x86::SEG_FS, FS32),
        (x86::SEG_ES, DS32),
        (x86::SEG_DS, DS32),
    ] {
        set32(c, at, v);
    }
}

/// The x87 state in FLOATING_SAVE_AREA form (FSAVE's, 112 bytes) from an
/// FXSAVE image.
pub(crate) fn float_save(fx: &[u8]) -> [u8; 112] {
    let mut f = [0u8; 112];
    // ControlWord, StatusWord (as 32-bit words), TagWord (full: valid or
    // empty from the abridged bits).
    f[0..2].copy_from_slice(&fx[0..2]);
    f[4..6].copy_from_slice(&fx[2..4]);
    let mut tag = 0u32;
    for i in 0..8 {
        if fx[4] & (1 << i) == 0 {
            tag |= 3 << (2 * i);
        }
    }
    set32(&mut f, 8, tag | 0xffff_0000);
    // ErrorOffset, ErrorSelector (FCS, the opcode above), DataOffset,
    // DataSelector.
    f[12..16].copy_from_slice(&fx[8..12]);
    let fop = u32::from(u16::from_le_bytes([fx[6], fx[7]])) & 0x7ff;
    set32(&mut f, 16, u32::from(u16::from_le_bytes([fx[12], fx[13]])) | (fop << 16));
    f[20..24].copy_from_slice(&fx[16..20]);
    f[24..26].copy_from_slice(&fx[20..22]);
    // The eight registers, 10 bytes each.
    for i in 0..8 {
        f[28 + 10 * i..38 + 10 * i].copy_from_slice(&fx[32 + 16 * i..42 + 16 * i]);
    }
    f
}

/// This thread's x87/SSE state (FXSAVE).
fn fxsave() -> [u8; 512] {
    #[repr(align(16))]
    struct Area([u8; 512]);
    let mut area = Area([0; 512]);
    // SAFETY: writes 512 bytes to a 16-byte aligned buffer.
    unsafe { core::arch::asm!("fxsave [{}]", in(reg) area.0.as_mut_ptr(), options(nostack)) };
    area.0
}

/// Loads x87/SSE state from an FXSAVE image (MXCSR's reserved bits
/// cleared: setting them faults).
pub(crate) fn fxrstor(image: &[u8]) {
    #[repr(align(16))]
    struct Area([u8; 512]);
    let mut area = Area([0; 512]);
    area.0.copy_from_slice(&image[..512]);
    let mxcsr = u32_at(&area.0, 24) & 0xffbf;
    set32(&mut area.0, 24, mxcsr);
    // SAFETY: reads 512 bytes from a 16-byte aligned buffer holding a
    // valid image (reserved MXCSR bits clear).
    unsafe { core::arch::asm!("fxrstor [{}]", in(reg) area.0.as_ptr(), options(nostack)) };
}

/// Fills the parts of `out` (an x86 CONTEXT) its ContextFlags name from
/// `ctx`, an x86 context with the registers, and `fx`, the x87/SSE state.
fn fill(out: &mut [u8; x86::SIZE], ctx: &[u8], fx: &[u8]) {
    let flags = u32_at(out, x86::FLAGS);
    let copy =
        |out: &mut [u8; x86::SIZE], at: usize| out[at..at + 4].copy_from_slice(&ctx[at..at + 4]);
    if has(flags, x86::CONTROL) {
        for at in [x86::EBP, x86::EIP, x86::EFLAGS, x86::ESP] {
            copy(out, at);
        }
        set32(out, x86::SEG_CS, CS32);
        set32(out, x86::SEG_SS, DS32);
    }
    if has(flags, x86::INTEGER) {
        for at in [x86::EDI, x86::ESI, x86::EBX, x86::EDX, x86::ECX, x86::EAX] {
            copy(out, at);
        }
    }
    if has(flags, x86::SEGMENTS) {
        for (at, v) in
            [(x86::SEG_GS, DS32), (x86::SEG_FS, FS32), (x86::SEG_ES, DS32), (x86::SEG_DS, DS32)]
        {
            set32(out, at, v);
        }
    }
    if has(flags, x86::FLOATING_POINT) {
        out[x86::FLOAT_SAVE..x86::FLOAT_SAVE + 112].copy_from_slice(&float_save(fx));
    }
    if has(flags, x86::EXTENDED_REGISTERS) {
        out[x86::EXTENDED..x86::EXTENDED + 512].copy_from_slice(&fx[..512]);
    }
}

/// The x86 registers of a 64-bit CONTEXT whose thread was stopped in
/// 32-bit code (their low halves), as an x86 context.
fn from_x64(ctx: &[u8]) -> [u8; x86::SIZE] {
    let mut c = [0u8; x86::SIZE];
    for (i, &at) in x86::GPR.iter().enumerate() {
        set32(&mut c, at, u64_at(ctx, x64::GPR + 8 * i) as u32);
    }
    set32(&mut c, x86::EIP, u64_at(ctx, x64::RIP) as u32);
    set32(&mut c, x86::EFLAGS, u32_at(ctx, x64::EFLAGS));
    c
}

/// NtGetContextThread(Thread, *Context (x86)).
fn get_context(a: &Args, cpu: u64) -> Res<NtStatus> {
    let (thread, at) = (a.h(0), a.p(1));
    let mut out = [0u8; x86::SIZE];
    mem::read(at, &mut out)?;
    let mut regs = [0u8; x86::SIZE];
    let fx;
    if is_current(thread)? {
        // The caller's registers at its system call, kept by the transition.
        mem::read(cpu + CPU_CONTEXT, &mut regs)?;
        fx = fxsave();
    } else {
        let mut ctx = [0u8; x64::SIZE];
        set32(&mut ctx, x64::FLAGS, x64::ALL);
        let status = syscall(NT_GET_CONTEXT_THREAD, &[thread, ctx.as_mut_ptr() as u64]);
        if status != STATUS_SUCCESS {
            return Ok(status);
        }
        let mut f = [0u8; 512];
        f.copy_from_slice(&ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512]);
        fx = f;
        if u32::from(u16::from_le_bytes([ctx[x64::SEG_CS], ctx[x64::SEG_CS + 1]])) == CS32 {
            // Stopped in 32-bit code.
            regs = from_x64(&ctx);
        } else {
            // In wow64.dll's code (a system call): its caller's registers.
            mem::read(area_of(thread)? + CPU_CONTEXT, &mut regs)?;
        }
    }
    fill(&mut out, &regs, &fx);
    mem::write(at, &out)?;
    Ok(STATUS_SUCCESS)
}

/// Applies the parts of x86 context `new` its ContextFlags name to the x86
/// context `regs` (segments stay Windows' selectors).
fn apply(regs: &mut [u8], new: &[u8; x86::SIZE]) {
    let flags = u32_at(new, x86::FLAGS);
    let mut copy = |at: usize| regs[at..at + 4].copy_from_slice(&new[at..at + 4]);
    if has(flags, x86::CONTROL) {
        for at in [x86::EBP, x86::EIP, x86::EFLAGS, x86::ESP] {
            copy(at);
        }
    }
    if has(flags, x86::INTEGER) {
        for at in [x86::EDI, x86::ESI, x86::EBX, x86::EDX, x86::ECX, x86::EAX] {
            copy(at);
        }
    }
    set32(regs, x86::SEG_CS, CS32);
    set32(regs, x86::SEG_SS, DS32);
}

/// The caller's own registers become `new`'s (the CPU area, which the
/// transition's way back loads): the eax it resumes with, as this call's
/// result.
fn set_own(new: &[u8; x86::SIZE], cpu: u64) -> Res<NtStatus> {
    let area = cpu + CPU_CONTEXT;
    let mut regs = [0u8; x86::SIZE];
    mem::read(area, &mut regs)?;
    apply(&mut regs, new);
    if has(u32_at(new, x86::FLAGS), x86::EXTENDED_REGISTERS) {
        fxrstor(&new[x86::EXTENDED..x86::EXTENDED + 512]);
    }
    mem::write(area, &regs)?;
    Ok(if has(u32_at(new, x86::FLAGS), x86::INTEGER) {
        u32_at(&regs, x86::EAX)
    } else {
        STATUS_SUCCESS
    })
}

/// NtSetContextThread(Thread, *Context (x86)).
fn set_context(thread: u64, at: u64, cpu: u64) -> Res<NtStatus> {
    let mut new = [0u8; x86::SIZE];
    mem::read(at, &mut new)?;
    let flags = u32_at(&new, x86::FLAGS);
    if has(flags, x86::FLOATING_POINT) && !has(flags, x86::EXTENDED_REGISTERS) {
        unsupported(b"wow64.dll!NtSetContextThread(CONTEXT_FLOATING_POINT)", flags.into());
    }
    if is_current(thread)? {
        return set_own(&new, cpu);
    }
    let mut ctx = [0u8; x64::SIZE];
    set32(&mut ctx, x64::FLAGS, x64::ALL);
    let status = syscall(NT_GET_CONTEXT_THREAD, &[thread, ctx.as_mut_ptr() as u64]);
    if status != STATUS_SUCCESS {
        return Ok(status);
    }
    if u32::from(u16::from_le_bytes([ctx[x64::SEG_CS], ctx[x64::SEG_CS + 1]])) == CS32 {
        // Stopped in 32-bit code: its registers, through the kernel.
        let mut regs = from_x64(&ctx);
        apply(&mut regs, &new);
        for (i, &at) in x86::GPR.iter().enumerate() {
            let v = u64::from(u32_at(&regs, at));
            ctx[x64::GPR + 8 * i..x64::GPR + 8 * i + 8].copy_from_slice(&v.to_le_bytes());
        }
        let eip = u64::from(u32_at(&regs, x86::EIP));
        ctx[x64::RIP..x64::RIP + 8].copy_from_slice(&eip.to_le_bytes());
        set32(&mut ctx, x64::EFLAGS, u32_at(&regs, x86::EFLAGS));
        let mut flags64 = x64::CONTROL_INTEGER;
        if has(flags, x86::EXTENDED_REGISTERS) {
            ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512]
                .copy_from_slice(&new[x86::EXTENDED..x86::EXTENDED + 512]);
            flags64 |= x64::FLOATING_POINT;
        }
        set32(&mut ctx, x64::FLAGS, flags64);
        return Ok(syscall(NT_SET_CONTEXT_THREAD, &[thread, ctx.as_ptr() as u64]));
    }
    // In wow64.dll's code: the registers it goes back to 32-bit code with.
    let area = area_of(thread)? + CPU_CONTEXT;
    let mut regs = [0u8; x86::SIZE];
    mem::read(area, &mut regs)?;
    apply(&mut regs, &new);
    mem::write(area, &regs)?;
    if has(flags, x86::EXTENDED_REGISTERS) {
        set32(&mut ctx, x64::FLAGS, x64::FLOATING_POINT);
        ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512]
            .copy_from_slice(&new[x86::EXTENDED..x86::EXTENDED + 512]);
        let status = syscall(NT_SET_CONTEXT_THREAD, &[thread, ctx.as_ptr() as u64]);
        if is_error(status) {
            return Ok(status);
        }
    }
    Ok(STATUS_SUCCESS)
}

/// NtContinue(*Context (x86), TestAlert): the caller goes on with the
/// context; with TestAlert, the next queued user APC runs first (an
/// alertable zero wait: the kernel delivers it to `dispatch::apc`, which
/// resumes this context after it).
fn continue_with(at: u64, test_alert: bool, cpu: u64) -> Res<NtStatus> {
    let mut new = [0u8; x86::SIZE];
    mem::read(at, &mut new)?;
    let eax = set_own(&new, cpu)?;
    if test_alert {
        // SAFETY: a system call with arguments on this stack.
        unsafe { gala_wow64_test_alert() };
    }
    Ok(eax)
}

unsafe extern "C" {
    /// NtDelayExecution(TRUE, 0): delivers the next user APC, if any.
    fn gala_wow64_test_alert();
}

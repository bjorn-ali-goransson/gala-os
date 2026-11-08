//! Exceptions and APCs of 32-bit code (ARCHITECTURE "Plan: WOW64"): the
//! kernel delivers them in 64-bit code here, and this hands them to the
//! 32-bit ntdll's dispatchers on the 32-bit stack (their addresses are in
//! the thread's CPU area; without a 32-bit ntdll there are none).
//!
//! - An exception in 32-bit code: the kernel's x64 CONTEXT (SegCs 0x23,
//!   the 32-bit registers zero-extended) and EXCEPTION_RECORD on the 64-bit
//!   stack become the x86 ones on the 32-bit stack, below the faulting
//!   esp, with [EXCEPTION_RECORD*, CONTEXT*] at the new esp, where the
//!   32-bit KiUserExceptionDispatcher starts.
//! - A user APC: a 32-bit routine is queued as wow64.dll's marker routine
//!   with the 32-bit routine and its context packed in NormalContext
//!   (`queue_apc`). When the kernel runs it (an alertable wait in
//!   wow64.dll's code, or NtContinue's TestAlert) the x86 CONTEXT of the
//!   32-bit caller (the CPU area: the system call's caller, with eax the
//!   call's status, STATUS_USER_APC) goes on the 32-bit stack with
//!   [routine, NormalContext, SystemArgument1, SystemArgument2, CONTEXT*]
//!   below it, where the 32-bit KiUserApcDispatcher starts.
//! - GalaUnhandledException from the 32-bit ntdll: its x86 record and
//!   context, as the x64 ones (SegCs 0x23: the crash report shows 32-bit
//!   registers).
//!
//! The 64-bit frames below are left behind: every transition from 32-bit
//! code starts again at the top of the 64-bit stack.

use crate::mem::{self, Res, set32, u32_at, u64_at};
use crate::thread::{self, x86};
use crate::{Args, CPU_CONTEXT, NtStatus, TEB64_CPU_SLOT, syscall, unsupported};

/// The CPU area's dispatchers (the kernel's `layout::wow64::cpu`).
const CPU_DISPATCHER32: u64 = 0x2d8;
const CPU_APC_DISPATCHER32: u64 = 0x2dc;

const GALA_UNHANDLED_EXCEPTION: u32 = 0x7f5c;
const NT_TERMINATE_PROCESS: u32 = 0x2c;
const STATUS_USER_APC: u32 = 0xc0;

/// x64 CONTEXT offsets.
mod x64 {
    pub const SIZE: usize = 0x4d0;
    pub const P1_HOME: usize = 0x00;
    pub const FLAGS: usize = 0x30;
    pub const SEG_CS: usize = 0x38;
    pub const EFLAGS: usize = 0x44;
    pub const GPR: usize = 0x78;
    pub const RIP: usize = 0xf8;
    pub const FLT_SAVE: usize = 0x100;
    /// CONTEXT_AMD64 | CONTROL | INTEGER.
    pub const CONTROL_INTEGER: u32 = 0x10_0003;
}

/// EXCEPTION_RECORD: x64 (code, flags, record, address at 0x10, count at
/// 0x18, parameters at 0x20, 8 bytes each) and x86 (0x50 bytes: address
/// at 0x0c, count at 0x10, parameters at 0x14, 4 bytes each).
const RECORD64_SIZE: usize = 0x98;
const RECORD32_SIZE: usize = 0x50;

/// The x86 context flags of an exception's CONTEXT: CONTROL, INTEGER,
/// SEGMENTS, FLOATING_POINT, EXTENDED_REGISTERS.
const EXCEPTION_FLAGS32: u32 = 0x1_002f;

/// The services this module converts, with their argument counts.
pub static SERVICES: &[(u32, usize)] = &[(GALA_UNHANDLED_EXCEPTION, 2)];

/// This thread's CPU area (TEB64.TlsSlots[1]).
fn cpu_area() -> u64 {
    let teb: u64;
    // SAFETY: GS is the running thread's 64-bit TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags));
    }
    // SAFETY: the TEB is this thread's; the kernel set the slot.
    unsafe { core::ptr::read((teb + TEB64_CPU_SLOT) as *const u64) }
}

/// The area's u32 at `off` (it is this thread's: written by the kernel).
fn area_u32(area: u64, off: u64) -> u32 {
    // SAFETY: inside this thread's CPU area.
    unsafe { core::ptr::read_unaligned((area + off) as *const u32) }
}

unsafe extern "C" {
    /// Enters 32-bit code with the context in the CPU area (`rcx`).
    fn gala_wow64_enter32(area: u64) -> !;
    /// The return address of `gala_wow64_test_alert`'s system call.
    fn gala_wow64_test_alert_return();
}

/// The x86 CONTEXT of a 64-bit one whose thread was in 32-bit code: the
/// registers' low halves, Windows' selectors, the x87/SSE state.
fn context32(ctx: &[u8]) -> [u8; x86::SIZE] {
    let mut c = [0u8; x86::SIZE];
    set32(&mut c, x86::FLAGS, EXCEPTION_FLAGS32);
    for (i, &at) in x86::GPR.iter().enumerate() {
        set32(&mut c, at, u64_at(ctx, x64::GPR + 8 * i) as u32);
    }
    set32(&mut c, x86::EIP, u64_at(ctx, x64::RIP) as u32);
    set32(&mut c, x86::EFLAGS, u32_at(ctx, x64::EFLAGS));
    thread::set_segments(&mut c);
    let fx = &ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512];
    c[x86::FLOAT_SAVE..x86::FLOAT_SAVE + 112].copy_from_slice(&thread::float_save(fx));
    c[x86::EXTENDED..x86::EXTENDED + 512].copy_from_slice(fx);
    c
}

/// The x86 EXCEPTION_RECORD of an x64 one (parameters truncated).
fn record32(r: &[u8]) -> [u8; RECORD32_SIZE] {
    let mut out = [0u8; RECORD32_SIZE];
    out[0..8].copy_from_slice(&r[0..8]); // code, flags
    set32(&mut out, 0x0c, u64_at(r, 0x10) as u32);
    let count = u32_at(r, 0x18).min(15);
    set32(&mut out, 0x10, count);
    for i in 0..count as usize {
        set32(&mut out, 0x14 + 4 * i, u64_at(r, 0x20 + 8 * i) as u32);
    }
    out
}

/// Starts 32-bit code at `eip` with esp `esp` and the other registers of
/// x86 context `ctx` (through the CPU area); the x87/SSE state from
/// `fx` if given.
fn enter(area: u64, ctx: &[u8; x86::SIZE], eip: u32, esp: u32, fx: Option<&[u8]>) -> ! {
    let mut regs = *ctx;
    set32(&mut regs, x86::EIP, eip);
    set32(&mut regs, x86::ESP, esp);
    // SAFETY: this thread's CPU area, its context's place.
    unsafe {
        core::ptr::copy_nonoverlapping(regs.as_ptr(), (area + CPU_CONTEXT) as *mut u8, x86::SIZE);
    }
    if let Some(fx) = fx {
        thread::fxrstor(fx);
    }
    // SAFETY: the area holds a complete 32-bit context.
    unsafe { gala_wow64_enter32(area) }
}

/// An exception in 32-bit code (`context`: the kernel's x64 CONTEXT with
/// SegCs 0x23, `record` its EXCEPTION_RECORD, both on this stack): to the
/// 32-bit ntdll's KiUserExceptionDispatcher. If it cannot be (no 32-bit
/// ntdll, a 32-bit stack that cannot take the frame) the process ends
/// with the crash report of the 32-bit registers.
pub fn exception32(context: u64, record: u64) -> ! {
    // SAFETY: the kernel put both on this stack for this call.
    let (ctx, rec) = unsafe {
        (
            core::slice::from_raw_parts(context as *const u8, x64::SIZE),
            core::slice::from_raw_parts(record as *const u8, RECORD64_SIZE),
        )
    };
    let area = cpu_area();
    let dispatcher = area_u32(area, CPU_DISPATCHER32);
    let c = context32(ctx);
    let r = record32(rec);
    let esp = u32_at(&c, x86::ESP);
    let ctx_at = (esp.wrapping_sub(x86::SIZE as u32)) & !0xf;
    let rec_at = ctx_at.wrapping_sub(RECORD32_SIZE as u32);
    let new_esp = rec_at.wrapping_sub(8);
    let mut args = [0u8; 8];
    set32(&mut args, 0, rec_at);
    set32(&mut args, 4, ctx_at);
    let pushed = dispatcher != 0
        && new_esp < esp
        && mem::write(ctx_at.into(), &c).is_ok()
        && mem::write(rec_at.into(), &r).is_ok()
        && mem::write(new_esp.into(), &args).is_ok();
    if !pushed {
        if dispatcher != 0 {
            unsupported(b"wow64.dll!exception (32-bit stack full)", esp.into());
        }
        syscall(GALA_UNHANDLED_EXCEPTION, &[record, context]);
        syscall(NT_TERMINATE_PROCESS, &[u64::MAX, u32_at(rec, 0).into()]);
        loop {
            core::hint::spin_loop();
        }
    }
    enter(area, &c, dispatcher, new_esp, Some(&ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512]))
}

/// What a 32-bit APC's routine is queued as: this byte's address (never
/// run: `apc` recognizes it).
static APC32_MARKER: u8 = 0;

/// The routine a 32-bit APC routine `routine` with context `context`
/// (NormalContext, an I/O or timer APC's context) is queued as: (the
/// marker, the two packed in the 64-bit context).
pub fn apc32(routine: u32, context: u32) -> (u64, u64) {
    (&raw const APC32_MARKER as u64, (u64::from(routine) << 32) | u64::from(context))
}

/// NtQueueApcThread(Thread, ApcRoutine, ApcArgument1..3) from 32-bit code:
/// the marker routine, with the 32-bit routine and its first argument in
/// NormalContext.
pub fn queue_apc(a: &Args) -> Res<NtStatus> {
    let (marker, packed) = apc32(a.u(1), a.u(2));
    Ok(syscall(thread::NT_QUEUE_APC_THREAD, &[a.h(0), marker, packed, a.p(3), a.p(4)]))
}

/// A user APC (`context`: the kernel's x64 CONTEXT on this stack, its
/// P1Home..P4Home NormalContext, SystemArgument1, SystemArgument2,
/// NormalRoutine): a 32-bit routine to the 32-bit ntdll's
/// KiUserApcDispatcher.
pub fn apc(context: u64) -> ! {
    // SAFETY: the kernel put it on this stack for this call.
    let ctx = unsafe { core::slice::from_raw_parts(context as *const u8, x64::SIZE) };
    let home = |i: usize| u64_at(ctx, x64::P1_HOME + 8 * i);
    let area = cpu_area();
    let dispatcher = area_u32(area, CPU_APC_DISPATCHER32);
    let resume = |why: &[u8]| -> ! {
        unsupported(why, home(3));
        syscall(crate::thread::NT_CONTINUE, &[context, 1]);
        loop {
            core::hint::spin_loop();
        }
    };
    if home(3) != &raw const APC32_MARKER as u64 {
        resume(b"wow64.dll!APC (64-bit routine)");
    }
    if dispatcher == 0 {
        resume(b"wow64.dll!APC (no 32-bit ntdll)");
    }
    // The 32-bit caller to resume: the CPU area's (the system call's
    // caller), which returns the call's status, STATUS_USER_APC; after
    // NtContinue's TestAlert (`continue_with`) the context being resumed,
    // as it is.
    let mut c = [0u8; x86::SIZE];
    // SAFETY: this thread's CPU area.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (area + CPU_CONTEXT) as *const u8,
            c.as_mut_ptr(),
            x86::SIZE,
        );
    }
    if u64_at(ctx, x64::RIP) != gala_wow64_test_alert_return as *const () as u64 {
        set32(&mut c, x86::EAX, STATUS_USER_APC);
    }
    set32(&mut c, x86::FLAGS, EXCEPTION_FLAGS32);
    thread::set_segments(&mut c);
    let fx = &ctx[x64::FLT_SAVE..x64::FLT_SAVE + 512];
    c[x86::FLOAT_SAVE..x86::FLOAT_SAVE + 112].copy_from_slice(&thread::float_save(fx));
    c[x86::EXTENDED..x86::EXTENDED + 512].copy_from_slice(fx);
    let esp = u32_at(&c, x86::ESP);
    let ctx_at = (esp.wrapping_sub(x86::SIZE as u32)) & !0xf;
    let new_esp = ctx_at.wrapping_sub(20);
    let mut frame = [0u8; 20];
    let packed = home(0);
    set32(&mut frame, 0, (packed >> 32) as u32);
    set32(&mut frame, 4, packed as u32);
    set32(&mut frame, 8, home(1) as u32);
    set32(&mut frame, 12, home(2) as u32);
    set32(&mut frame, 16, ctx_at);
    let pushed = new_esp < esp
        && mem::write(ctx_at.into(), &c).is_ok()
        && mem::write(new_esp.into(), &frame).is_ok();
    if !pushed {
        resume(b"wow64.dll!APC (32-bit stack full)");
    }
    enter(area, &c, dispatcher, new_esp, Some(fx))
}

/// GalaUnhandledException(*ExceptionRecord, *Context) from the 32-bit
/// ntdll: the x86 record and context as the x64 ones (SegCs 0x23).
pub fn call(number: u32, a: &Args) -> NtStatus {
    debug_assert_eq!(number, GALA_UNHANDLED_EXCEPTION);
    unhandled(a).unwrap_or_else(|e| e)
}

fn unhandled(a: &Args) -> Res<NtStatus> {
    if a.p(0) == 0 {
        return Ok(syscall(GALA_UNHANDLED_EXCEPTION, &[0, 0]));
    }
    let mut r = [0u8; RECORD32_SIZE];
    mem::read(a.p(0), &mut r)?;
    let mut c = [0u8; x86::SIZE];
    mem::read(a.p(1), &mut c)?;
    let mut rec = [0u8; RECORD64_SIZE];
    rec[0..8].copy_from_slice(&r[0..8]);
    rec[0x10..0x18].copy_from_slice(&u64::from(u32_at(&r, 0x0c)).to_le_bytes());
    let count = u32_at(&r, 0x10).min(15);
    rec[0x18..0x1c].copy_from_slice(&count.to_le_bytes());
    for i in 0..count as usize {
        let v = u64::from(u32_at(&r, 0x14 + 4 * i));
        rec[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    let mut ctx = [0u8; x64::SIZE];
    set32(&mut ctx, x64::FLAGS, x64::CONTROL_INTEGER);
    ctx[x64::SEG_CS..x64::SEG_CS + 2].copy_from_slice(&(thread::CS32 as u16).to_le_bytes());
    set32(&mut ctx, x64::EFLAGS, u32_at(&c, x86::EFLAGS));
    for (i, &at) in x86::GPR.iter().enumerate() {
        let v = u64::from(u32_at(&c, at));
        ctx[x64::GPR + 8 * i..x64::GPR + 8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    let eip = u64::from(u32_at(&c, x86::EIP));
    ctx[x64::RIP..x64::RIP + 8].copy_from_slice(&eip.to_le_bytes());
    Ok(syscall(GALA_UNHANDLED_EXCEPTION, &[rec.as_ptr() as u64, ctx.as_ptr() as u64]))
}

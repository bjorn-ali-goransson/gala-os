//! Extended register state, with results expected to match real Windows:
//! threads keep their vector registers and floating-point control across
//! preemption; an exception's CONTEXT carries the faulting thread's vector
//! registers and control word (to a vectored handler registered through
//! ntdll), and changes the handler makes are restored by NtContinue;
//! NtCreateThread ignores the CONTEXT's floating-point part: the new
//! thread starts with the default control word and v0 0;
//! NtSetContextThread's floating-point part on a thread created suspended
//! is what it starts with. x64 (`x64.rs`): xmm0-xmm15 and MXCSR (its
//! default 0x1f80; MxCsr and FltSave.MxCsr). ARM64 (`arm64.rs`): d0-d31
//! and FPCR (its default 0; one Fpcr). Prints `fpu_test: ok` or exits
//! with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use ntrt::*;

#[cfg(target_arch = "aarch64")]
mod arm64;
#[cfg(target_arch = "aarch64")]
use arm64 as arch;
#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "x86_64")]
use x64 as arch;

use arch::{
    CONTEXT_FLOATING_POINT, ExceptionPointers, NEW_THREAD_CSR, NEW_THREAD_V0, REGS, SEEN, V0_SET,
    V0_VALUE, V1_NEW, V1_VALUE, csr, pattern,
};

const THREADS: usize = 3;
pub const SPINS: u64 = 3_000_000;

/// Thread `t`'s registers survived: 1 = ok, else 2.
static RESULTS: [AtomicU32; THREADS + 1] = [const { AtomicU32::new(0) }; THREADS + 1];

fn hold_and_check(t: usize) -> bool {
    let mut values = [0u64; REGS];
    for (r, v) in values.iter_mut().enumerate() {
        *v = pattern(t as u64, r as u64);
    }
    let mut out = [0u64; REGS];
    let got = arch::hold_registers(&values, csr(t as u32 % 4), &mut out);
    out == values && got == csr(t as u32 % 4)
}

extern "C" fn holder(t: usize) -> ! {
    let ok = hold_and_check(t);
    RESULTS[t].store(if ok { 1 } else { 2 }, Ordering::SeqCst);
    exit_thread(0)
}

type SetContext = extern "system" fn(Handle, *const Context) -> NtStatus;
type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("fpu_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    // (1) Registers survive preemption, for every thread at once.
    let mut handles = [Handle(0); THREADS];
    for (t, h) in handles.iter_mut().enumerate() {
        let (s, handle, _) = create_thread(holder, t, false);
        c.eq(s.into(), ok)?;
        *h = handle;
    }
    let main_ok = hold_and_check(THREADS);
    for h in handles {
        c.eq(wait(h, None).into(), ok)?;
        close(h);
    }
    for (t, r) in RESULTS.iter().take(THREADS).enumerate() {
        c.eq(r.load(Ordering::SeqCst).into(), 1).map_err(|e| e + 100 * t as u32)?;
    }
    c.ok(main_ok, 0)?;

    // (2) CONTEXT carries the vector registers and the control word;
    // NtContinue restores changes.
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let Some(add) = find_export(ntdll, b"RtlAddVectoredExceptionHandler") else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signature of the export.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    c.ok(!add(1, arch::handler).is_null(), 0)?;
    let (x0, x1, got) = arch::fault_with_state();
    c.eq(SEEN[0].load(Ordering::SeqCst), csr(3).into())?;
    c.eq(SEEN[1].load(Ordering::SeqCst), csr(3).into())?;
    c.eq(SEEN[2].load(Ordering::SeqCst), V0_VALUE)?;
    c.eq(SEEN[3].load(Ordering::SeqCst), V1_VALUE)?;
    c.eq(x0, V0_VALUE)?;
    c.eq(x1, V1_NEW)?;
    c.eq(got.into(), csr(1).into())?;

    // (3) NtCreateThread ignores the CONTEXT's floating-point state: with
    // the control word and v0 set and CONTEXT_FLOATING_POINT in the flags,
    // the thread starts with the default control word and v0 0 (x64:
    // MXCSR 0x1f80, Windows 11).
    let mut ctx = Context::for_thread(arch::report_csr, 0, 0);
    arch::set_context_fp(&mut ctx, csr(2), 0xabcd_1234);
    let (s, t, _) = create_thread_with_context(&mut ctx);
    c.eq(s.into(), ok)?;
    c.eq(wait(t, None).into(), ok)?;
    close(t);
    c.eq(NEW_THREAD_CSR.load(Ordering::SeqCst).into(), csr(0).into())?;
    c.eq(NEW_THREAD_V0.load(Ordering::SeqCst), 0)?;

    // (4) NtSetContextThread with CONTEXT_FLOATING_POINT on a thread
    // created suspended, before it first runs: it starts with that control
    // word and v0 (applied on resume, as for any suspended thread).
    let Some(set) = find_export(ntdll, b"NtSetContextThread") else { return c.ok(false, 2) };
    // SAFETY: the documented signature of the export.
    let set = unsafe { core::mem::transmute::<usize, SetContext>(set) };
    let (s, t, _) = create_thread(arch::report_csr, 0, true);
    c.eq(s.into(), ok)?;
    let mut ctx = Context::for_thread(arch::report_csr, 0, 0);
    ctx.context_flags = CONTEXT_FLOATING_POINT;
    arch::set_context_fp(&mut ctx, csr(2), V0_SET);
    c.eq(set(t, &ctx).into(), ok)?;
    c.eq(resume_thread(t).1.into(), 1)?;
    c.eq(wait(t, None).into(), ok)?;
    close(t);
    c.eq(NEW_THREAD_CSR.load(Ordering::SeqCst).into(), csr(2).into())?;
    c.eq(NEW_THREAD_V0.load(Ordering::SeqCst), V0_SET)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("fpu_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}

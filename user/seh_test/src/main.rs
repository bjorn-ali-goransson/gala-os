//! Frame-based exception handling (x64 SEH), as documented in "x64
//! exception handling" (Microsoft Learn): unwind information in .pdata and
//! .xdata, language-specific handlers and the ntdll functions that use
//! them. The functions under test are written in assembly with explicit
//! unwind directives, so that their prologues and frames are known:
//!
//! - RtlPcToFileHeader and RtlLookupFunctionEntry find this image and the
//!   function's RUNTIME_FUNCTION.
//! - RtlVirtualUnwind unwinds a frame from its body (restores the pushed
//!   registers, returns the handler, its data and the establisher frame),
//!   from inside its prologue (only what ran is undone, no handler) and
//!   from its epilogue (emulated, no handler).
//! - RtlCaptureContext records the caller's registers, Rip = its return
//!   address and Rsp = the caller's rsp after the return.
//! - RtlRaiseException dispatches to frame handlers, innermost first, with
//!   ExceptionAddress = its return address; a handler returning
//!   ExceptionContinueExecution resumes there.
//! - A handler returning ExceptionContinueSearch passes the exception on;
//!   an outer handler calls RtlUnwindEx to its own frame: the inner
//!   handler runs again with EXCEPTION_UNWINDING, the outer one with
//!   EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND, and execution resumes
//!   at the target with rax = the return value and the target frame's
//!   nonvolatile registers.
//! - The same for a hardware exception (an access violation).
//!
//! Prints `seh_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_TARGET_UNWIND: u32 = 0x20;
const EXCEPTION_CONTINUE: u32 = 0;
const EXCEPTION_CONTINUE_SEARCH: u32 = 1;
const UNW_FLAG_EHANDLER: u32 = 1;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
/// Raised and continued by the inner handler.
const CODE_CONTINUE: u32 = 0xe000_0001;
/// Raised and unwound by the outer handler.
const CODE_UNWIND: u32 = 0xe000_0002;
/// rbx in seh_outer's body, and in seh_inner's.
const OUTER_RBX: u64 = 0x1111_0000_0000_1111;
const INNER_RBX: u64 = 0x2222_0000_0000_2222;

// EXCEPTION_RECORD, CONTEXT and DISPATCHER_CONTEXT (public x64 layouts).
const ER_CODE: usize = 0x00;
const ER_FLAGS: usize = 0x04;
const ER_ADDRESS: usize = 0x10;
const ER_NUMBER_PARAMETERS: usize = 0x18;
const ER_INFORMATION: usize = 0x20;
const RECORD_SIZE: usize = 0x98;
const CTX_FLAGS: usize = 0x30;
const CTX_SEG_CS: usize = 0x38;
const CTX_RBX: usize = 0x90;
const CTX_RSP: usize = 0x98;
const CTX_RSI: usize = 0xa8;
const CTX_R12: usize = 0xd8;
const CTX_RIP: usize = 0xf8;
const CONTEXT_SIZE: usize = 0x4d0;
const CONTEXT_FULL: u64 = 0x10_000b;
const DC_CONTROL_PC: usize = 0x00;
const DC_IMAGE_BASE: usize = 0x08;
const DC_CONTEXT_RECORD: usize = 0x28;
const DC_LANGUAGE_HANDLER: usize = 0x30;
const DC_HISTORY_TABLE: usize = 0x40;

// seh_outer(f, arg): pushes rbx and rsi, allocates 0x28 bytes, sets rbx,
// records its rsp (its establisher frame) and calls seh_inner(f, arg);
// seh_outer_resume is the return point and the unwind target. Its handler
// is outer_handler. seh_inner(f, arg): pushes rbx, allocates 0x20, sets
// rbx and calls f(arg); its handler is inner_handler. seh_raise(record)
// calls RtlRaiseException; seh_raise_ret is the return address.
// seh_capture(context) calls RtlCaptureContext with known rbx and r12.
global_asm!(
    ".text",
    ".globl seh_outer",
    ".globl seh_outer_resume",
    ".globl seh_outer_epilogue",
    ".def seh_outer; .scl 2; .type 32; .endef",
    "seh_outer:",
    ".seh_proc seh_outer",
    "push rbx",
    ".seh_pushreg rbx",
    "push rsi",
    ".seh_pushreg rsi",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    ".seh_handler {outer_handler}, @except, @unwind",
    "mov [rip + {outer_rsp}], rsp",
    "mov rbx, {outer_rbx}",
    "mov rsi, rbx",
    "call seh_inner",
    "seh_outer_resume:",
    "mov [rip + {resume_rbx}], rbx",
    "mov [rip + {resume_rsi}], rsi",
    "seh_outer_epilogue:",
    "add rsp, 0x28",
    "pop rsi",
    "pop rbx",
    "ret",
    ".seh_endproc",
    "",
    ".def seh_inner; .scl 3; .type 32; .endef",
    "seh_inner:",
    ".seh_proc seh_inner",
    "push rbx",
    ".seh_pushreg rbx",
    "sub rsp, 0x20",
    ".seh_stackalloc 0x20",
    ".seh_endprologue",
    ".seh_handler {inner_handler}, @except, @unwind",
    "mov rbx, {inner_rbx}",
    "mov rax, rcx",
    "mov rcx, rdx",
    "call rax",
    "nop",
    "add rsp, 0x20",
    "pop rbx",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_raise",
    ".globl seh_raise_ret",
    ".def seh_raise; .scl 2; .type 32; .endef",
    "seh_raise:",
    ".seh_proc seh_raise",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    "call qword ptr [rip + {raise}]",
    "seh_raise_ret:",
    "mov eax, 5",
    "add rsp, 0x28",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_capture",
    ".globl seh_capture_ret",
    ".def seh_capture; .scl 2; .type 32; .endef",
    "seh_capture:",
    ".seh_proc seh_capture",
    "push rbx",
    ".seh_pushreg rbx",
    "push r12",
    ".seh_pushreg r12",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    "mov rbx, {outer_rbx}",
    "mov r12, {inner_rbx}",
    "mov [rip + {capture_rsp}], rsp",
    "call qword ptr [rip + {capture}]",
    "seh_capture_ret:",
    "nop",
    "add rsp, 0x28",
    "pop r12",
    "pop rbx",
    "ret",
    ".seh_endproc",
    outer_handler = sym outer_handler,
    inner_handler = sym inner_handler,
    outer_rsp = sym OUTER_RSP,
    resume_rbx = sym RESUME_RBX,
    resume_rsi = sym RESUME_RSI,
    capture_rsp = sym CAPTURE_RSP,
    raise = sym RTL_RAISE,
    capture = sym RTL_CAPTURE,
    outer_rbx = const OUTER_RBX,
    inner_rbx = const INNER_RBX,
);

unsafe extern "C" {
    fn seh_outer(f: extern "system" fn(u64) -> u64, arg: u64) -> u64;
    fn seh_outer_resume();
    fn seh_outer_epilogue();
    fn seh_raise(record: *mut u8) -> u64;
    fn seh_raise_ret();
    fn seh_capture(context: *mut u8);
    fn seh_capture_ret();
}

static OUTER_RSP: AtomicU64 = AtomicU64::new(0);
static RESUME_RBX: AtomicU64 = AtomicU64::new(0);
static RESUME_RSI: AtomicU64 = AtomicU64::new(0);
static CAPTURE_RSP: AtomicU64 = AtomicU64::new(0);
static RTL_RAISE: AtomicUsize = AtomicUsize::new(0);
static RTL_CAPTURE: AtomicUsize = AtomicUsize::new(0);
static RTL_UNWIND_EX: AtomicUsize = AtomicUsize::new(0);

/// What the handlers saw. Inner: calls, flags of each call (up to 2),
/// code, ExceptionAddress, parameter count, info[0], info[1],
/// Context.Rip. Outer: calls, flags of each call, code, establisher
/// frame, ControlPc, ImageBase, LanguageHandler.
static INNER: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static OUTER: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// The value the outer handler unwinds with.
static UNWIND_VALUE: AtomicU64 = AtomicU64::new(0);

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the structures the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

fn rd32(p: *const u8, off: usize) -> u32 {
    rd(p, off) as u32
}

fn wr(p: *mut u8, off: usize, v: u64) {
    // SAFETY: offsets inside buffers of the test's own.
    unsafe { (p.add(off) as *mut u64).write_unaligned(v) }
}

fn reset() {
    for a in INNER.iter().chain(OUTER.iter()) {
        a.store(0, Ordering::SeqCst);
    }
}

extern "system" fn inner_handler(
    record: *mut u8,
    _frame: u64,
    context: *mut u8,
    _dispatcher: *mut u8,
) -> u32 {
    let calls = INNER[0].fetch_add(1, Ordering::SeqCst);
    let flags = rd32(record, ER_FLAGS);
    if calls < 2 {
        INNER[1 + calls as usize].store(u64::from(flags), Ordering::SeqCst);
    }
    if flags & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let values = [
        u64::from(rd32(record, ER_CODE)),
        rd(record, ER_ADDRESS),
        u64::from(rd32(record, ER_NUMBER_PARAMETERS)),
        rd(record, ER_INFORMATION),
        rd(record, ER_INFORMATION + 8),
        rd(context, CTX_RIP),
    ];
    for (slot, v) in INNER[3..].iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    if rd32(record, ER_CODE) == CODE_CONTINUE {
        EXCEPTION_CONTINUE
    } else {
        EXCEPTION_CONTINUE_SEARCH
    }
}

type UnwindEx = extern "system" fn(u64, usize, *mut u8, u64, *mut u8, *mut u8);

extern "system" fn outer_handler(
    record: *mut u8,
    frame: u64,
    _context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    let calls = OUTER[0].fetch_add(1, Ordering::SeqCst);
    let flags = rd32(record, ER_FLAGS);
    if calls < 2 {
        OUTER[1 + calls as usize].store(u64::from(flags), Ordering::SeqCst);
    }
    if flags & EXCEPTION_UNWINDING != 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let values = [
        u64::from(rd32(record, ER_CODE)),
        frame,
        rd(dispatcher, DC_CONTROL_PC),
        rd(dispatcher, DC_IMAGE_BASE),
        rd(dispatcher, DC_LANGUAGE_HANDLER),
    ];
    for (slot, v) in OUTER[3..].iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    // SAFETY: the documented signature of ntdll's RtlUnwindEx.
    let unwind: UnwindEx = unsafe { core::mem::transmute(RTL_UNWIND_EX.load(Ordering::SeqCst)) };
    unwind(
        frame,
        seh_outer_resume as *const () as usize,
        record,
        UNWIND_VALUE.load(Ordering::SeqCst),
        rd(dispatcher, DC_CONTEXT_RECORD) as *mut u8,
        rd(dispatcher, DC_HISTORY_TABLE) as *mut u8,
    );
    EXCEPTION_CONTINUE_SEARCH // not reached
}

/// Raises `code` with parameters 7 and 9 through seh_raise.
extern "system" fn raise_it(code: u64) -> u64 {
    let mut record = [0u8; RECORD_SIZE];
    let r = record.as_mut_ptr();
    wr(r, ER_CODE, code);
    wr(r, ER_NUMBER_PARAMETERS, 2);
    wr(r, ER_INFORMATION, 7);
    wr(r, ER_INFORMATION + 8, 9);
    // SAFETY: seh_raise passes the record to RtlRaiseException.
    unsafe { seh_raise(r) }
}

/// Reads address 8: an access violation.
extern "system" fn fault_it(_: u64) -> u64 {
    // SAFETY: the load faults; the outer handler unwinds past this frame.
    unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<u64>(8)) }
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("seh_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32((got >> 32) as u32));
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn inner(i: usize) -> u64 {
    INNER[i].load(Ordering::SeqCst)
}

fn outer(i: usize) -> u64 {
    OUTER[i].load(Ordering::SeqCst)
}

type PcToFileHeader = extern "system" fn(usize, *mut usize) -> usize;
type LookupFunctionEntry = extern "system" fn(u64, *mut u64, *mut u8) -> *const [u32; 3];
type VirtualUnwind =
    extern "system" fn(u32, u64, u64, *const [u32; 3], *mut u8, *mut u64, *mut u64, *mut u8) -> u64;

/// A CONTEXT, 16-byte aligned.
#[repr(C, align(16))]
struct Context([u8; CONTEXT_SIZE]);

/// Unwinds seh_outer at `pc` with rsp = `stack`, rbx = rsi = 0; returns
/// (handler, handler data, establisher frame) and leaves the result in
/// `ctx`.
fn virtual_unwind(
    ntdll: &Fns,
    pc: u64,
    stack: *mut u64,
    ctx: &mut Context,
) -> Result<(u64, u64, u64), u32> {
    let mut base = 0u64;
    let entry = (ntdll.lookup)(pc, &mut base, core::ptr::null_mut());
    if entry.is_null() {
        return Err(0);
    }
    ctx.0 = [0; CONTEXT_SIZE];
    let c = ctx.0.as_mut_ptr();
    wr(c, CTX_FLAGS, CONTEXT_FULL);
    wr(c, CTX_RIP, pc);
    wr(c, CTX_RSP, stack as u64);
    let (mut data, mut frame) = (0u64, 0u64);
    let handler = (ntdll.unwind)(
        UNW_FLAG_EHANDLER,
        base,
        pc,
        entry,
        c,
        &mut data,
        &mut frame,
        core::ptr::null_mut(),
    );
    Ok((handler, data, frame))
}

struct Fns {
    lookup: LookupFunctionEntry,
    unwind: VirtualUnwind,
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let names: [&[u8]; 6] = [
        b"RtlPcToFileHeader",
        b"RtlLookupFunctionEntry",
        b"RtlVirtualUnwind",
        b"RtlCaptureContext",
        b"RtlRaiseException",
        b"RtlUnwindEx",
    ];
    let mut f = [0usize; 6];
    for (slot, name) in f.iter_mut().zip(names) {
        *slot = find_export(ntdll, name).unwrap_or(0);
        c.ok(*slot != 0, 0)?;
    }
    RTL_CAPTURE.store(f[3], Ordering::SeqCst);
    RTL_RAISE.store(f[4], Ordering::SeqCst);
    RTL_UNWIND_EX.store(f[5], Ordering::SeqCst);
    // SAFETY: the documented signatures of these ntdll exports.
    let (pc_to_header, fns) = unsafe {
        (
            core::mem::transmute::<usize, PcToFileHeader>(f[0]),
            Fns {
                lookup: core::mem::transmute::<usize, LookupFunctionEntry>(f[1]),
                unwind: core::mem::transmute::<usize, VirtualUnwind>(f[2]),
            },
        )
    };
    let image = image_base();
    let outer_fn = seh_outer as *const () as usize;

    // RtlPcToFileHeader: this image for its code, null outside images.
    let mut base = 0usize;
    c.eq(pc_to_header(outer_fn + 3, &mut base) as u64, image as u64)?;
    c.eq(base as u64, image as u64)?;
    c.eq(pc_to_header(16, &mut base) as u64, 0)?;
    c.eq(base as u64, 0)?;

    // RtlLookupFunctionEntry: seh_outer's entry, starting at its address.
    let mut base64 = 0u64;
    let entry = (fns.lookup)(outer_fn as u64 + 3, &mut base64, core::ptr::null_mut());
    c.ok(!entry.is_null(), 0)?;
    c.eq(base64, image as u64)?;
    // SAFETY: a RUNTIME_FUNCTION in this image's .pdata.
    let begin = unsafe { (*entry)[0] };
    c.eq(u64::from(begin), (outer_fn - image) as u64)?;

    // RtlVirtualUnwind from seh_outer's body: a fake stack with the saved
    // rsi at +0x28, the saved rbx at +0x30 and a return address at +0x38.
    let mut stack = [0u64; 16];
    let sp = stack.as_mut_ptr();
    let put = |i: usize, v: u64| {
        // SAFETY: inside `stack`.
        unsafe { sp.add(i).write_volatile(v) }
    };
    put(5, 0x5151); // rsi
    put(6, 0xb0b0); // rbx
    put(7, 0x1234_5678); // return address
    let mut ctx = Context([0; CONTEXT_SIZE]);
    let (handler, data, frame) =
        virtual_unwind(&fns, seh_outer_resume as *const () as u64, sp, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, outer_handler as *const () as u64)?;
    c.ok(data != 0, data)?;
    c.eq(frame, sp as u64)?;
    c.eq(rd(cp, CTX_RIP), 0x1234_5678)?;
    c.eq(rd(cp, CTX_RSP), sp as u64 + 0x40)?;
    c.eq(rd(cp, CTX_RBX), 0xb0b0)?;
    c.eq(rd(cp, CTX_RSI), 0x5151)?;

    // From inside the prologue, after `push rbx` (1 byte): only that push
    // is undone, and no handler is reported.
    put(0, 0xb1b1); // rbx
    put(1, 0x4321); // return address
    let (handler, _, _) = virtual_unwind(&fns, outer_fn as u64 + 1, sp, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_RIP), 0x4321)?;
    c.eq(rd(cp, CTX_RSP), sp as u64 + 0x10)?;
    c.eq(rd(cp, CTX_RBX), 0xb1b1)?;

    // From the epilogue (at `add rsp, 0x28`): emulated, no handler.
    let (handler, _, _) =
        virtual_unwind(&fns, seh_outer_epilogue as *const () as u64, sp, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_RIP), 0x1234_5678)?;
    c.eq(rd(cp, CTX_RSP), sp as u64 + 0x40)?;
    c.eq(rd(cp, CTX_RBX), 0xb0b0)?;
    c.eq(rd(cp, CTX_RSI), 0x5151)?;

    // RtlCaptureContext: the caller's registers, its return address and
    // its rsp after the return.
    let mut ctx = Context([0; CONTEXT_SIZE]);
    // SAFETY: seh_capture fills the CONTEXT.
    unsafe { seh_capture(ctx.0.as_mut_ptr()) };
    let cp = ctx.0.as_ptr();
    c.eq(rd(cp, CTX_RIP), seh_capture_ret as *const () as u64)?;
    c.eq(rd(cp, CTX_RSP), CAPTURE_RSP.load(Ordering::SeqCst))?;
    c.eq(rd(cp, CTX_RBX), OUTER_RBX)?;
    c.eq(rd(cp, CTX_R12), INNER_RBX)?;
    c.eq(rd(cp, CTX_FLAGS) & CONTEXT_FULL, CONTEXT_FULL)?;
    c.eq(rd(cp, CTX_SEG_CS) & 0xffff, 0x33)?;

    // RtlRaiseException, continued by the inner handler: seh_raise returns
    // 5 through raise_it, seh_inner and seh_outer.
    reset();
    // SAFETY: seh_outer calls raise_it(CODE_CONTINUE) under its handlers.
    let r = unsafe { seh_outer(raise_it, CODE_CONTINUE.into()) };
    c.eq(r, 5)?;
    c.eq(inner(0), 1)?;
    c.eq(inner(1), 0)?; // flags
    c.eq(inner(3), CODE_CONTINUE.into())?;
    c.eq(inner(4), seh_raise_ret as *const () as u64)?; // ExceptionAddress
    c.eq(inner(5), 2)?;
    c.eq(inner(6), 7)?;
    c.eq(inner(7), 9)?;
    c.eq(inner(8), seh_raise_ret as *const () as u64)?; // Context.Rip
    c.eq(outer(0), 0)?;

    // Passed on by the inner handler; the outer one unwinds to
    // seh_outer_resume with 42.
    reset();
    UNWIND_VALUE.store(42, Ordering::SeqCst);
    RESUME_RBX.store(0, Ordering::SeqCst);
    // SAFETY: as above; the outer handler unwinds to seh_outer_resume.
    let r = unsafe { seh_outer(raise_it, CODE_UNWIND.into()) };
    c.eq(r, 42)?;
    c.eq(inner(0), 2)?;
    c.eq(inner(1), 0)?;
    c.eq(inner(2), EXCEPTION_UNWINDING.into())?;
    c.eq(inner(3), CODE_UNWIND.into())?;
    c.eq(outer(0), 2)?;
    c.eq(outer(1), 0)?;
    c.eq(outer(2), (EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND).into())?;
    c.eq(outer(3), CODE_UNWIND.into())?;
    c.eq(outer(4), OUTER_RSP.load(Ordering::SeqCst))?; // establisher frame
    c.eq(outer(5), seh_outer_resume as *const () as u64)?; // ControlPc: the return address
    c.eq(outer(6), image as u64)?;
    c.eq(outer(7), outer_handler as *const () as u64)?; // LanguageHandler
    c.eq(RESUME_RBX.load(Ordering::SeqCst), OUTER_RBX)?;
    c.eq(RESUME_RSI.load(Ordering::SeqCst), OUTER_RBX)?;

    // A hardware exception: the access violation in fault_it, passed on by
    // the inner handler and unwound by the outer one with 43.
    reset();
    UNWIND_VALUE.store(43, Ordering::SeqCst);
    RESUME_RBX.store(0, Ordering::SeqCst);
    // SAFETY: as above.
    let r = unsafe { seh_outer(fault_it, 0) };
    c.eq(r, 43)?;
    c.eq(inner(0), 2)?;
    c.eq(inner(3), STATUS_ACCESS_VIOLATION.into())?;
    c.eq(inner(5), 2)?;
    c.eq(inner(6), 0)?; // read
    c.eq(inner(7), 8)?; // address
    c.eq(outer(0), 2)?;
    c.eq(outer(2), (EXCEPTION_UNWINDING | EXCEPTION_TARGET_UNWIND).into())?;
    c.eq(outer(3), STATUS_ACCESS_VIOLATION.into())?;
    c.eq(RESUME_RBX.load(Ordering::SeqCst), OUTER_RBX)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("seh_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

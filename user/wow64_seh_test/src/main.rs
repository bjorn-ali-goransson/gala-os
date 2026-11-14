//! Exceptions and APCs in 32-bit (x86) code under WOW64 (ROADMAP WOW stage
//! 3), with the results of 64-bit Windows' WOW64: the documented x86 SEH
//! model (EXCEPTION_REGISTRATION_RECORDs linked at fs:[0], handlers called
//! as `handler(record, frame, context, dispatcher_context)`), the 32-bit
//! ntdll's RtlUnwind, RtlRaiseException and vectored handlers, and user
//! APCs with 32-bit routines.
//!
//! - An access violation (a write to address 0) caught by a registered
//!   frame: STATUS_ACCESS_VIOLATION, ExceptionAddress the faulting
//!   instruction, parameters (1 = write, address 0), the x86 CONTEXT (CS
//!   0x23, Eip the faulting instruction, ebx as the code set it, esp on the
//!   stack); the handler unwinds to its frame (RtlUnwind, with ebx, esi,
//!   edi and ebp saved around it: Windows' does not preserve them) and
//!   resumes there. A read of address 0x10: parameters (0, 0x10).
//! - While a handler runs, fs:[0] is not the program's: the dispatcher
//!   has linked a registration of its own (below the faulting context's
//!   stack pointer, its Next the program's newest registration), as x86
//!   Windows calls every handler (Pietrek's "A Crash Course on the Depths
//!   of Win32 Structured Exception Handling": the nested-exception frame).
//!   Microsoft's C++ runtime relies on it: its catch saves that head,
//!   unwinds and links it back above the catching frame.
//! - A frame whose handler returns ExceptionContinueSearch under one that
//!   handles: the inner handler is called to dispatch, then once with
//!   EXCEPTION_UNWINDING by the outer handler's RtlUnwind; fs:[0] is back
//!   to what it was.
//! - int3: STATUS_BREAKPOINT at the int3 (ExceptionAddress and Eip), and
//!   ExceptionContinueExecution with Eip past it resumes there.
//! - Division by zero: STATUS_INTEGER_DIVIDE_BY_ZERO.
//! - RtlRaiseException: a vectored handler sees the code, flags 0 and the
//!   parameters first and continues execution (the frames' handlers are not
//!   called); after removing it, a frame's handler gets the same exception.
//! - APCs: two queued to this thread with NtQueueApcThread (32-bit
//!   routines, stdcall with three arguments) run in order, with their
//!   arguments, during an alertable NtDelayExecution, which returns
//!   STATUS_USER_APC; a non-alertable wait runs none. A timer's APC
//!   routine (NtSetTimer) runs with its context during an alertable wait.
//!
//! Prints `wow64_seh_test: ok` and exits with 0, or exits with the number
//! of the failing check (`(line 0x...) wow64_seh_test: check N failed`
//! first). With the argument `crash` it writes to address 0 with no
//! handler: the exception goes through the 32-bit ntdll's dispatcher,
//! finds none and ends the process (scenario wow64_seh_crash).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use ntrt::*;

#[link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn RtlUnwind(frame: usize, ip: usize, record: *mut u8, value: usize);
    fn RtlRaiseException(record: *mut u8);
    fn RtlAddVectoredExceptionHandler(first: u32, handler: Vectored) -> usize;
    fn RtlRemoveVectoredExceptionHandler(cookie: usize) -> u32;
    fn NtQueueApcThread(thread: isize, routine: Normal, a1: usize, a2: usize, a3: usize) -> u32;
    fn NtDelayExecution(alertable: u8, interval: *const i64) -> u32;
    fn NtCreateTimer(handle: *mut usize, access: u32, attributes: usize, kind: u32) -> u32;
    fn NtSetTimer(
        h: usize,
        due: *const i64,
        routine: Normal,
        context: usize,
        resume: u8,
        period: i32,
        previous: *mut u8,
    ) -> u32;
    fn NtClose(h: usize) -> u32;
}

/// RtlUnwind(frame, 0, record, 0) with ebx, esi, edi and ebp saved
/// around it: the 32-bit RtlUnwind on Windows does not preserve them (a
/// handler that kept `frame` in esi across the call read it back as 0), so
/// callers save them themselves, as the C runtime's unwind helpers do.
///
/// # Safety
/// RtlUnwind's: `frame` a registration on this thread's fs:[0] list,
/// `record` the exception being dispatched.
unsafe fn unwind(frame: usize, record: usize) {
    // SAFETY: the registers RtlUnwind may change are pushed and popped
    // around it; it pops its four stdcall arguments.
    unsafe {
        core::arch::asm!(
            "push ebx",
            "push esi",
            "push edi",
            "push ebp",
            "push 0",
            "push ecx",
            "push 0",
            "push eax",
            "call edx",
            "pop ebp",
            "pop edi",
            "pop esi",
            "pop ebx",
            in("eax") frame,
            in("ecx") record,
            in("edx") RtlUnwind as unsafe extern "system" fn(usize, usize, *mut u8, usize) as usize,
            clobber_abi("system"),
        )
    };
}

type Vectored = extern "system" fn(*mut [usize; 2]) -> i32;
type Normal = extern "system" fn(usize, usize, usize);

const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const STATUS_BREAKPOINT: u32 = 0x8000_0003;
const STATUS_INTEGER_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
const STATUS_USER_APC: u32 = 0xc0;
const RAISED: u32 = 0xe000_1234;
const EXCEPTION_UNWINDING: u32 = 2;

// x86 CONTEXT offsets.
const CTX_FLAGS: usize = 0x00;
const CTX_EBX: usize = 0xa4;
const CTX_EAX: usize = 0xb0;
const CTX_EBP: usize = 0xb4;
const CTX_EIP: usize = 0xb8;
const CTX_CS: usize = 0xbc;
const CTX_ESP: usize = 0xc4;

// `gala_seh_try(f, arg)` / `gala_seh_try_search(f, arg)` (cdecl): call
// `f(arg)` (cdecl) under a registration record whose handler is
// `catch_handler` / `search_handler`. The record is extended: [Next,
// Handler, the frame's ebp, the landing]; `catch_handler` resumes at the
// landing with esp at the record and eax the exception code.
// `gala_write_null`: ebx = 0x12345678, then a write of 1 to address 0 (at
// +8); `gala_read_low`: a read of address 0x10 (at +0); `gala_int3`: int3
// (at +0), then returns 7; `gala_divide(n)`: 100 / n.
macro_rules! seh_try {
    ($name:literal, $landing:literal, $handler:ident) => {
        core::arch::global_asm!(
            concat!(".globl ", $name),
            concat!($name, ":"),
            "push ebp",
            "mov ebp, esp",
            "push ebx",
            "push esi",
            "push edi",
            concat!("lea eax, [", $landing, "]"),
            "push eax",
            "push ebp",
            "lea eax, [{handler}]",
            "push eax",
            "push dword ptr fs:[0]",
            "mov fs:[0], esp",
            "push dword ptr [ebp + 12]",
            "call dword ptr [ebp + 8]",
            "add esp, 4",
            concat!($landing, ":"),
            "pop ecx",
            "mov fs:[0], ecx",
            "add esp, 12",
            "pop edi",
            "pop esi",
            "pop ebx",
            "pop ebp",
            "ret",
            handler = sym $handler,
        );
    };
}
seh_try!("_gala_seh_try", "_gala_seh_landing", catch_handler);
seh_try!("_gala_seh_try_search", "_gala_seh_search_landing", search_handler);

core::arch::global_asm!(
    ".globl _gala_write_null",
    "_gala_write_null:",
    "push ebx",
    "mov ebx, 0x12345678",
    "xor eax, eax",
    "mov dword ptr [eax], 1",
    "pop ebx",
    "ret",
    ".globl _gala_read_low",
    "_gala_read_low:",
    "mov eax, dword ptr [0x10]",
    "ret",
    ".globl _gala_int3",
    "_gala_int3:",
    "int3",
    "mov eax, 7",
    "ret",
    ".globl _gala_divide",
    "_gala_divide:",
    "mov eax, 100",
    "xor edx, edx",
    "div dword ptr [esp + 4]",
    "ret",
);

unsafe extern "C" {
    fn gala_seh_try(f: unsafe extern "C" fn(u32) -> u32, arg: u32) -> u32;
    fn gala_seh_try_search(f: unsafe extern "C" fn(u32) -> u32, arg: u32) -> u32;
    fn gala_write_null(arg: u32) -> u32;
    fn gala_read_low(arg: u32) -> u32;
    fn gala_int3(arg: u32) -> u32;
    fn gala_divide(n: u32) -> u32;
}

/// What the last dispatch call of `catch_handler` saw: code, flags,
/// address, parameter count, parameters 0 and 1, the context's flags,
/// CS, Eip, Ebx, Esp.
static SEEN: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];
/// `catch_handler`'s mode: 0 catch (unwind and resume at the landing), 1
/// continue past an int3.
static MODE: AtomicU32 = AtomicU32::new(0);
/// Calls of `search_handler`: to dispatch, with EXCEPTION_UNWINDING.
static SEARCHED: AtomicU32 = AtomicU32::new(0);
static UNWOUND: AtomicU32 = AtomicU32::new(0);
/// Calls of `catch_handler` (dispatch only).
static CAUGHT: AtomicU32 = AtomicU32::new(0);
/// What `catch_handler` last saw of fs:[0]: the head, its Next, and the
/// establisher frame it was called for.
static HEAD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

fn rd(addr: usize) -> u32 {
    // SAFETY: the callers read the record and context the dispatcher
    // passed, and registration records on this stack.
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

fn wr(addr: usize, v: u32) {
    // SAFETY: the context the dispatcher passed (resumed after).
    unsafe { core::ptr::write_unaligned(addr as *mut u32, v) }
}

unsafe extern "C" fn catch_handler(record: usize, frame: usize, context: usize, _dc: usize) -> u32 {
    if rd(record + 4) & EXCEPTION_UNWINDING != 0 {
        return 1; // ExceptionContinueSearch
    }
    CAUGHT.fetch_add(1, Ordering::SeqCst);
    let head = teb_field(0);
    for (s, v) in HEAD.iter().zip([head as u32, rd(head), frame as u32]) {
        s.store(v, Ordering::SeqCst);
    }
    let fields = [
        rd(record),
        rd(record + 4),
        rd(record + 0x0c),
        rd(record + 0x10),
        rd(record + 0x14),
        rd(record + 0x18),
        rd(context + CTX_FLAGS),
        rd(context + CTX_CS),
        rd(context + CTX_EIP),
        rd(context + CTX_EBX),
        rd(context + CTX_ESP),
    ];
    for (s, v) in SEEN.iter().zip(fields) {
        s.store(v, Ordering::SeqCst);
    }
    if MODE.load(Ordering::SeqCst) == 1 {
        wr(context + CTX_EIP, rd(context + CTX_EIP) + 1);
        return 0; // ExceptionContinueExecution
    }
    // The frames above this one are unwound, then the frame's function
    // goes on at its landing (esp: the record; ebp: the frame's).
    // SAFETY: our own registration; the record the dispatcher passed.
    unsafe { unwind(frame, record) };
    wr(context + CTX_ESP, frame as u32);
    wr(context + CTX_EBP, rd(frame + 8));
    wr(context + CTX_EIP, rd(frame + 12));
    wr(context + CTX_EAX, rd(record));
    0
}

unsafe extern "C" fn search_handler(record: usize, _frame: usize, _ctx: usize, _dc: usize) -> u32 {
    if rd(record + 4) & EXCEPTION_UNWINDING != 0 {
        UNWOUND.fetch_add(1, Ordering::SeqCst);
    } else {
        SEARCHED.fetch_add(1, Ordering::SeqCst);
    }
    1 // ExceptionContinueSearch
}

fn seen(i: usize) -> u32 {
    SEEN[i].load(Ordering::SeqCst)
}

extern "C" fn nested_inner(_: u32) -> u32 {
    // SAFETY: the access violation is caught by the outer frame.
    unsafe { gala_seh_try_search(gala_write_null, 0) }
}

extern "C" fn raise_it(_: u32) -> u32 {
    raise(0x55);
    9
}

/// RtlRaiseException(RAISED) with parameters (0x11, `p`).
fn raise(p: u32) {
    let mut record = [0u32; 20];
    record[0] = RAISED;
    record[4] = 2; // NumberParameters
    record[5] = 0x11;
    record[6] = p;
    // SAFETY: a valid EXCEPTION_RECORD.
    unsafe { RtlRaiseException(record.as_mut_ptr().cast()) };
}

static VECTORED: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];

extern "system" fn vectored(pointers: *mut [usize; 2]) -> i32 {
    // SAFETY: EXCEPTION_POINTERS the dispatcher passed.
    let record = unsafe { (*pointers)[0] };
    if rd(record) != RAISED {
        return 0; // EXCEPTION_CONTINUE_SEARCH
    }
    let fields =
        [rd(record), rd(record + 4), rd(record + 0x10), rd(record + 0x14), rd(record + 0x18)];
    for (s, v) in VECTORED.iter().zip(fields) {
        s.store(v, Ordering::SeqCst);
    }
    -1 // EXCEPTION_CONTINUE_EXECUTION
}

static APCS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static TIMER_CONTEXT: AtomicU32 = AtomicU32::new(0);

extern "system" fn timer_apc(context: usize, _low: usize, _high: usize) {
    TIMER_CONTEXT.store(context as u32, Ordering::SeqCst);
}
static APC_COUNT: AtomicU32 = AtomicU32::new(0);

extern "system" fn apc(a: usize, b: usize, c: usize) {
    let n = APC_COUNT.fetch_add(1, Ordering::SeqCst) as usize;
    if n < 2 {
        APCS[4 * n].store(a as u32, Ordering::SeqCst);
        APCS[4 * n + 1].store(b as u32, Ordering::SeqCst);
        APCS[4 * n + 2].store(c as u32, Ordering::SeqCst);
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        if let Ok(text) = core::str::from_utf8(&at) {
            print(text);
        }
        let mut line = *b"wow64_seh_test: check 00000000 failed, got 00000000\n";
        line[22..30].copy_from_slice(&hex32(self.0));
        line[43..51].copy_from_slice(&hex32(got));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let list = teb_field(0);
    let (stack_base, stack_limit) = (teb_field(4) as u32, teb_field(8) as u32);

    // An access violation caught by a frame.
    MODE.store(0, Ordering::SeqCst);
    // SAFETY: the fault is caught by the registered frame.
    let got = unsafe { gala_seh_try(gala_write_null, 0) };
    c.eq(got, STATUS_ACCESS_VIOLATION)?;
    let fault = gala_write_null as *const () as u32 + 8;
    c.eq(seen(0), STATUS_ACCESS_VIOLATION)?;
    c.eq(seen(1), 0)?; // ExceptionFlags
    c.eq(seen(2), fault)?; // ExceptionAddress
    c.eq(seen(3), 2)?; // NumberParameters
    c.eq(seen(4), 1)?; // a write
    c.eq(seen(5), 0)?; // at address 0
    c.eq(seen(6) & 0x1_0003, 0x1_0003)?; // CONTEXT_CONTROL | CONTEXT_INTEGER
    c.eq(seen(7), 0x23)?;
    c.eq(seen(8), fault)?;
    c.eq(seen(9), 0x1234_5678)?;
    c.ok(seen(10) > stack_limit && seen(10) < stack_base, seen(10))?;
    c.eq(teb_field(0) as u32, list as u32)?;
    // The handler ran under the dispatcher's own registration, linked
    // above the program's frame (deeper on the stack than the fault).
    let [head, next, frame] = HEAD.each_ref().map(|h| h.load(Ordering::SeqCst));
    c.ok(head != frame, head)?;
    c.eq(next, frame)?;
    c.ok(head < seen(10) && head > stack_limit, head)?;
    // A read.
    // SAFETY: as above.
    c.eq(unsafe { gala_seh_try(gala_read_low, 0) }, STATUS_ACCESS_VIOLATION)?;
    c.eq(seen(2), gala_read_low as *const () as u32)?;
    c.eq(seen(4), 0)?;
    c.eq(seen(5), 0x10)?;

    // Nested frames: the inner one searches on, the outer one catches and
    // unwinds the inner one.
    // SAFETY: as above.
    c.eq(unsafe { gala_seh_try(nested_inner, 0) }, STATUS_ACCESS_VIOLATION)?;
    c.eq(SEARCHED.load(Ordering::SeqCst), 1)?;
    c.eq(UNWOUND.load(Ordering::SeqCst), 1)?;
    c.eq(teb_field(0) as u32, list as u32)?;

    // int3, continued past.
    MODE.store(1, Ordering::SeqCst);
    // SAFETY: the breakpoint is handled and continued.
    c.eq(unsafe { gala_seh_try(gala_int3, 0) }, 7)?;
    c.eq(seen(0), STATUS_BREAKPOINT)?;
    c.eq(seen(2), gala_int3 as *const () as u32)?;
    c.eq(seen(8), gala_int3 as *const () as u32)?;
    MODE.store(0, Ordering::SeqCst);

    // Division by zero.
    // SAFETY: as above.
    c.eq(unsafe { gala_seh_try(gala_divide, 0) }, STATUS_INTEGER_DIVIDE_BY_ZERO)?;
    // SAFETY: no exception.
    c.eq(unsafe { gala_seh_try(gala_divide, 5) }, 20)?;

    // RtlRaiseException: a vectored handler first, which continues.
    let caught = CAUGHT.load(Ordering::SeqCst);
    // SAFETY: a handler of the right type.
    let cookie = unsafe { RtlAddVectoredExceptionHandler(1, vectored) };
    c.ok(cookie != 0, 0)?;
    raise(0x22);
    c.eq(VECTORED[0].load(Ordering::SeqCst), RAISED)?;
    c.eq(VECTORED[1].load(Ordering::SeqCst), 0)?;
    c.eq(VECTORED[2].load(Ordering::SeqCst), 2)?;
    c.eq(VECTORED[3].load(Ordering::SeqCst), 0x11)?;
    c.eq(VECTORED[4].load(Ordering::SeqCst), 0x22)?;
    // SAFETY: as above (under a frame: the vectored handler still comes
    // first).
    c.eq(unsafe { gala_seh_try(raise_it, 0) }, 9)?;
    c.eq(CAUGHT.load(Ordering::SeqCst), caught)?;
    c.eq(VECTORED[4].load(Ordering::SeqCst), 0x55)?;
    // SAFETY: the cookie from above.
    c.eq(unsafe { RtlRemoveVectoredExceptionHandler(cookie) }, 1)?;
    // Without it: the frame's handler.
    // SAFETY: as above.
    c.eq(unsafe { gala_seh_try(raise_it, 0) }, RAISED)?;
    c.eq(seen(3), 2)?;
    c.eq(seen(4), 0x11)?;
    c.eq(seen(5), 0x55)?;
    c.eq(teb_field(0) as u32, list as u32)?;

    // APCs.
    let interval = -10_000i64; // 1 ms
    // SAFETY: valid arguments; the routines are ours.
    unsafe {
        c.eq(NtQueueApcThread(-2, apc, 1, 2, 3), 0)?;
        c.eq(NtQueueApcThread(-2, apc, 4, 5, 6), 0)?;
        c.eq(NtDelayExecution(0, &interval), 0)?;
        c.eq(APC_COUNT.load(Ordering::SeqCst), 0)?;
        c.eq(NtDelayExecution(1, &interval), STATUS_USER_APC)?;
    }
    c.eq(APC_COUNT.load(Ordering::SeqCst), 2)?;
    for (i, want) in [1, 2, 3, 0, 4, 5, 6].iter().enumerate() {
        if i != 3 {
            c.eq(APCS[i].load(Ordering::SeqCst), *want)?;
        }
    }
    // A timer's APC routine.
    let mut timer = 0usize;
    let due = -10_000i64; // 1 ms
    let wait = -500_000i64; // 50 ms
    // SAFETY: valid arguments; the routine is ours.
    unsafe {
        c.eq(NtCreateTimer(&mut timer, 0x1f_0003, 0, 0), 0)?; // TIMER_ALL_ACCESS
        c.eq(NtSetTimer(timer, &due, timer_apc, 0x77, 0, 0, core::ptr::null_mut()), 0)?;
        c.eq(NtDelayExecution(1, &wait), STATUS_USER_APC)?;
        c.eq(NtClose(timer), 0)?;
    }
    c.eq(TIMER_CONTEXT.load(Ordering::SeqCst), 0x77)?;
    Ok(())
}

fn main() -> u32 {
    let pp = process_parameters() as usize;
    let (len, buf) = (
        usize::from(read_u16(pp + params::COMMAND_LINE)),
        rd(pp + params::COMMAND_LINE + 4) as usize,
    );
    if ends_with(len, buf, " crash") {
        print("wow64_seh_test: writing to address 0\n");
        // SAFETY: deliberately faults with no handler; the process ends.
        unsafe { gala_write_null(0) };
    }
    match run() {
        Ok(()) => {
            print("wow64_seh_test: ok\n");
            0
        }
        Err(n) => n,
    }
}

fn read_u16(addr: usize) -> u16 {
    // SAFETY: the process parameters, mapped for the process's life.
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

/// Whether the UTF-16 string ends with `tail` (ASCII, case ignored).
fn ends_with(len: usize, buffer: usize, tail: &str) -> bool {
    let chars = len / 2;
    if buffer == 0 || chars < tail.len() {
        return false;
    }
    let start = buffer + 2 * (chars - tail.len());
    tail.bytes().enumerate().all(|(i, b)| {
        let c = read_u16(start + 2 * i);
        c < 0x80 && (c as u8).eq_ignore_ascii_case(&b)
    })
}

entry!(main);

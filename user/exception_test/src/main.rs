//! User-mode exception dispatch: a vectored exception handler registered
//! through ntdll's RtlAddVectoredExceptionHandler sees breakpoints, access
//! violations (read and write), integer division by zero and illegal
//! instructions with the record and CONTEXT real Windows provides, and
//! continues execution by returning EXCEPTION_CONTINUE_EXECUTION. Prints
//! `exception_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const STATUS_BREAKPOINT: u32 = 0x8000_0003;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const STATUS_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
const STATUS_INTEGER_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
/// Stored in r12 before each fault; the CONTEXT must carry it.
const MARK: u64 = 0x5eed_0000_c0de_0001;

// EXCEPTION_RECORD and CONTEXT (public x64 layouts).
const ER_CODE: usize = 0x00;
const ER_ADDRESS: usize = 0x10;
const ER_NUMBER_PARAMETERS: usize = 0x18;
const ER_INFORMATION: usize = 0x20;
const CTX_RSP: usize = 0x98;
const CTX_R12: usize = 0xd8;
const CTX_R13: usize = 0xe0;
const CTX_RIP: usize = 0xf8;

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// What the handler saw: code, address, parameter count, info[0], info[1],
/// Context.Rip, Context.Rsp, Context.R12, calls.
static SEEN: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the record/context the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    let (record, context) = unsafe { ((*pointers).record, (*pointers).context) };
    let code = rd(record, ER_CODE) as u32;
    let values = [
        u64::from(code),
        rd(record, ER_ADDRESS),
        rd(record, ER_NUMBER_PARAMETERS) & 0xffff_ffff,
        rd(record, ER_INFORMATION),
        rd(record, ER_INFORMATION + 8),
        rd(context, CTX_RIP),
        rd(context, CTX_RSP),
        rd(context, CTX_R12),
    ];
    for (slot, v) in SEEN.iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    SEEN[8].fetch_add(1, Ordering::SeqCst);
    // Continue: past the int3, or at the resume label left in r13.
    let resume =
        if code == STATUS_BREAKPOINT { rd(context, CTX_RIP) + 1 } else { rd(context, CTX_R13) };
    // SAFETY: writing Rip of the context that will be resumed.
    unsafe { (context.add(CTX_RIP) as *mut u64).write_unaligned(resume) };
    EXCEPTION_CONTINUE_EXECUTION
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
        print("exception_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn seen(i: usize) -> u64 {
    SEEN[i].load(Ordering::SeqCst)
}

/// Checks code, address, Rip/Rsp/R12 and the call count of the last fault.
fn check_common(c: &mut Checks, code: u32, at: u64, sp: u64, calls: u64) -> Result<(), u32> {
    c.eq(seen(0), code.into())?;
    c.eq(seen(1), at)?; // ExceptionAddress
    c.eq(seen(5), at)?; // Context.Rip
    c.eq(seen(6), sp)?; // Context.Rsp
    c.eq(seen(7), MARK)?; // preserved registers
    c.eq(seen(8), calls)
}

type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;
type RemoveHandler = extern "system" fn(*mut c_void) -> u32;

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let add = find_export(ntdll, b"RtlAddVectoredExceptionHandler");
    let remove = find_export(ntdll, b"RtlRemoveVectoredExceptionHandler");
    c.ok(add.is_some() && remove.is_some(), 0)?;
    // SAFETY: the documented signatures of these ntdll exports.
    let (add, remove): (AddHandler, RemoveHandler) = unsafe {
        (
            core::mem::transmute::<usize, AddHandler>(add.unwrap()),
            core::mem::transmute::<usize, RemoveHandler>(remove.unwrap()),
        )
    };
    let cookie = add(1, handler);
    c.ok(!cookie.is_null(), 0)?;

    // int3: STATUS_BREAKPOINT at the int3 itself; the handler skips it.
    let (at, sp): (u64, u64);
    // SAFETY: the handler resumes after the int3; r12 is restored by us.
    unsafe {
        asm!(
            "lea {at}, [rip + 2f]",
            "mov {sp}, rsp",
            "mov r12, {mark}",
            "2:",
            "int3",
            at = out(reg) at,
            sp = out(reg) sp,
            mark = in(reg) MARK,
            out("r12") _,
        )
    };
    check_common(&mut c, STATUS_BREAKPOINT, at, sp, 1)?;

    // Read of address 8: access violation, information = (0 = read, 8).
    let (at, sp): (u64, u64);
    // SAFETY: the faulting load never completes; the handler resumes at 3:.
    unsafe {
        asm!(
            "lea {at}, [rip + 2f]",
            "lea r13, [rip + 3f]",
            "mov {sp}, rsp",
            "mov r12, {mark}",
            "2:",
            "mov rax, qword ptr [8]",
            "3:",
            at = out(reg) at,
            sp = out(reg) sp,
            mark = in(reg) MARK,
            out("r12") _,
            out("r13") _,
            out("rax") _,
        )
    };
    check_common(&mut c, STATUS_ACCESS_VIOLATION, at, sp, 2)?;
    c.eq(seen(2), 2)?;
    c.eq(seen(3), 0)?;
    c.eq(seen(4), 8)?;

    // Write to a read-only page: information = (1 = write, address).
    let (s, page, _) = allocate(0, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(protect(page, 4096, PAGE_READONLY).0.into(), STATUS_SUCCESS.into())?;
    let target = (page + 0x10) as u64;
    let (at, sp): (u64, u64);
    // SAFETY: the faulting store never completes; the handler resumes at 3:.
    unsafe {
        asm!(
            "lea {at}, [rip + 2f]",
            "lea r13, [rip + 3f]",
            "mov {sp}, rsp",
            "mov r12, {mark}",
            "2:",
            "mov qword ptr [{p}], 1",
            "3:",
            p = in(reg) target,
            at = out(reg) at,
            sp = out(reg) sp,
            mark = in(reg) MARK,
            out("r12") _,
            out("r13") _,
        )
    };
    check_common(&mut c, STATUS_ACCESS_VIOLATION, at, sp, 3)?;
    c.eq(seen(3), 1)?;
    c.eq(seen(4), target)?;

    // Integer division by zero.
    let (at, sp): (u64, u64);
    // SAFETY: the faulting div never completes; the handler resumes at 3:.
    unsafe {
        asm!(
            "lea {at}, [rip + 2f]",
            "lea r13, [rip + 3f]",
            "mov {sp}, rsp",
            "mov r12, {mark}",
            "xor edx, edx",
            "xor ecx, ecx",
            "mov eax, 1",
            "2:",
            "div ecx",
            "3:",
            at = out(reg) at,
            sp = out(reg) sp,
            mark = in(reg) MARK,
            out("r12") _,
            out("r13") _,
            out("rax") _,
            out("rcx") _,
            out("rdx") _,
        )
    };
    check_common(&mut c, STATUS_INTEGER_DIVIDE_BY_ZERO, at, sp, 4)?;

    // ud2: illegal instruction.
    let (at, sp): (u64, u64);
    // SAFETY: ud2 never completes; the handler resumes at 3:.
    unsafe {
        asm!(
            "lea {at}, [rip + 2f]",
            "lea r13, [rip + 3f]",
            "mov {sp}, rsp",
            "mov r12, {mark}",
            "2:",
            "ud2",
            "3:",
            at = out(reg) at,
            sp = out(reg) sp,
            mark = in(reg) MARK,
            out("r12") _,
            out("r13") _,
        )
    };
    check_common(&mut c, STATUS_ILLEGAL_INSTRUCTION, at, sp, 5)?;

    c.ok(remove(cookie) != 0, 0)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("exception_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

//! User-mode exception dispatch: a vectored exception handler registered
//! through ntdll's RtlAddVectoredExceptionHandler sees breakpoints, access
//! violations (read and write), integer division by zero and illegal
//! instructions with the record and CONTEXT real Windows provides, and
//! continues execution by returning EXCEPTION_CONTINUE_EXECUTION. Prints
//! `exception_test: ok` or exits with the number of the failing check.
//!
//! Built for x64 and ARM64: each architecture's faulting instructions and
//! CONTEXT offsets are its `arch` module's. On ARM64 a breakpoint is `brk
//! #0xf000`, reported at the `brk` itself (the handler steps over its four
//! bytes); integer division does not trap, so a division by zero is the
//! compiler's check ending in `brk #0xf004`; `udf` is an illegal
//! instruction (Windows ARM64's mapping of the `brk` immediates, as Wine's
//! arm64 signal handling implements it).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const STATUS_BREAKPOINT: u32 = 0x8000_0003;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const STATUS_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
const STATUS_INTEGER_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
/// Stored in a nonvolatile register (r12, x20) before each fault; the
/// CONTEXT must carry it.
const MARK: u64 = 0x5eed_0000_c0de_0001;

// EXCEPTION_RECORD (the 64-bit layout, both architectures').
const ER_CODE: usize = 0x00;
const ER_ADDRESS: usize = 0x10;
const ER_NUMBER_PARAMETERS: usize = 0x18;
const ER_INFORMATION: usize = 0x20;

/// x64: the faulting sequences (each returns the faulting instruction's
/// address and the stack pointer at it; r13 holds where the handler
/// resumes) and the CONTEXT's offsets.
#[cfg(target_arch = "x86_64")]
mod arch {
    use super::MARK;
    use core::arch::asm;

    pub const CTX_SP: usize = 0x98;
    /// The register holding MARK, and the one holding the resume address.
    pub const CTX_MARK: usize = 0xd8; // r12
    pub const CTX_RESUME: usize = 0xe0; // r13
    pub const CTX_PC: usize = 0xf8;
    /// What the handler adds to a breakpoint's address to step over it.
    pub const BREAKPOINT_LENGTH: u64 = 1;

    pub fn breakpoint() -> (u64, u64) {
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
        (at, sp)
    }

    /// A read of address 8.
    pub fn read_8() -> (u64, u64) {
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
        (at, sp)
    }

    /// A write to `target`.
    pub fn write(target: u64) -> (u64, u64) {
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
        (at, sp)
    }

    pub fn divide_by_zero() -> (u64, u64) {
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
        (at, sp)
    }

    /// ud2.
    pub fn illegal_instruction() -> (u64, u64) {
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
        (at, sp)
    }
}

/// ARM64: as x64's, with x20 holding MARK and x21 the resume address (x18
/// is the TEB's and LLVM keeps x19).
#[cfg(target_arch = "aarch64")]
mod arch {
    use super::MARK;
    use core::arch::asm;

    pub const CTX_SP: usize = 0x100;
    pub const CTX_MARK: usize = 0x08 + 8 * 20; // X20
    pub const CTX_RESUME: usize = 0x08 + 8 * 21; // X21
    pub const CTX_PC: usize = 0x108;
    pub const BREAKPOINT_LENGTH: u64 = 4;

    pub fn breakpoint() -> (u64, u64) {
        let (at, sp): (u64, u64);
        // SAFETY: the handler resumes after the brk; x20 is ours.
        unsafe {
            asm!(
                "adr {at}, 2f",
                "mov {sp}, sp",
                "mov x20, {mark}",
                "2:",
                "brk #0xf000",
                at = out(reg) at,
                sp = out(reg) sp,
                mark = in(reg) MARK,
                out("x20") _,
            )
        };
        (at, sp)
    }

    pub fn read_8() -> (u64, u64) {
        let (at, sp): (u64, u64);
        // SAFETY: the faulting load never completes; the handler resumes at 3:.
        unsafe {
            asm!(
                "adr {at}, 2f",
                "adr x21, 3f",
                "mov {sp}, sp",
                "mov x20, {mark}",
                "mov x9, #8",
                "2:",
                "ldr x9, [x9]",
                "3:",
                at = out(reg) at,
                sp = out(reg) sp,
                mark = in(reg) MARK,
                out("x20") _,
                out("x21") _,
                out("x9") _,
            )
        };
        (at, sp)
    }

    pub fn write(target: u64) -> (u64, u64) {
        let (at, sp): (u64, u64);
        // SAFETY: the faulting store never completes; the handler resumes at 3:.
        unsafe {
            asm!(
                "adr {at}, 2f",
                "adr x21, 3f",
                "mov {sp}, sp",
                "mov x20, {mark}",
                "mov x9, #1",
                "2:",
                "str x9, [{p}]",
                "3:",
                p = in(reg) target,
                at = out(reg) at,
                sp = out(reg) sp,
                mark = in(reg) MARK,
                out("x20") _,
                out("x21") _,
                out("x9") _,
            )
        };
        (at, sp)
    }

    /// The compiler's division check: a zero divisor reaches `brk #0xf004`.
    pub fn divide_by_zero() -> (u64, u64) {
        let (at, sp): (u64, u64);
        // SAFETY: the brk never completes; the handler resumes at 3:.
        unsafe {
            asm!(
                "adr {at}, 2f",
                "adr x21, 3f",
                "mov {sp}, sp",
                "mov x20, {mark}",
                "mov x9, #1",
                "mov x10, #0",
                "cbnz x10, 4f",
                "2:",
                "brk #0xf004",
                "4:",
                "udiv x9, x9, x10",
                "3:",
                at = out(reg) at,
                sp = out(reg) sp,
                mark = in(reg) MARK,
                out("x20") _,
                out("x21") _,
                out("x9") _,
                out("x10") _,
            )
        };
        (at, sp)
    }

    /// udf: permanently undefined.
    pub fn illegal_instruction() -> (u64, u64) {
        let (at, sp): (u64, u64);
        // SAFETY: udf never completes; the handler resumes at 3:.
        unsafe {
            asm!(
                "adr {at}, 2f",
                "adr x21, 3f",
                "mov {sp}, sp",
                "mov x20, {mark}",
                "2:",
                "udf #0",
                "3:",
                at = out(reg) at,
                sp = out(reg) sp,
                mark = in(reg) MARK,
                out("x20") _,
                out("x21") _,
            )
        };
        (at, sp)
    }
}

use arch::*;

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// What the handler saw: code, address, parameter count, info[0], info[1],
/// the CONTEXT's pc, sp and MARK register, calls.
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
        rd(context, CTX_PC),
        rd(context, CTX_SP),
        rd(context, CTX_MARK),
    ];
    for (slot, v) in SEEN.iter().zip(values) {
        slot.store(v, Ordering::SeqCst);
    }
    SEEN[8].fetch_add(1, Ordering::SeqCst);
    // Continue: past the breakpoint, or at the resume label left in a
    // register.
    let resume = if code == STATUS_BREAKPOINT {
        rd(context, CTX_PC) + BREAKPOINT_LENGTH
    } else {
        rd(context, CTX_RESUME)
    };
    // SAFETY: writing the pc of the context that will be resumed.
    unsafe { (context.add(CTX_PC) as *mut u64).write_unaligned(resume) };
    EXCEPTION_CONTINUE_EXECUTION
}

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
        print("exception_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn seen(i: usize) -> u64 {
    SEEN[i].load(Ordering::SeqCst)
}

/// Checks code, address, the CONTEXT's pc, sp and MARK register and the
/// call count of the last fault.
fn check_common(c: &mut Checks, code: u32, at: u64, sp: u64, calls: u64) -> Result<(), u32> {
    c.eq(seen(0), code.into())?;
    c.eq(seen(1), at)?; // ExceptionAddress
    c.eq(seen(5), at)?; // the CONTEXT's pc
    c.eq(seen(6), sp)?; // the CONTEXT's sp
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

    // A breakpoint: STATUS_BREAKPOINT at the instruction itself; the
    // handler steps over it.
    let (at, sp) = breakpoint();
    check_common(&mut c, STATUS_BREAKPOINT, at, sp, 1)?;

    // Read of address 8: access violation, information = (0 = read, 8).
    let (at, sp) = read_8();
    check_common(&mut c, STATUS_ACCESS_VIOLATION, at, sp, 2)?;
    c.eq(seen(2), 2)?;
    c.eq(seen(3), 0)?;
    c.eq(seen(4), 8)?;

    // Write to a read-only page: information = (1 = write, address).
    let (s, page, _) = allocate(0, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(protect(page, 4096, PAGE_READONLY).0.into(), STATUS_SUCCESS.into())?;
    let target = (page + 0x10) as u64;
    let (at, sp) = write(target);
    check_common(&mut c, STATUS_ACCESS_VIOLATION, at, sp, 3)?;
    c.eq(seen(3), 1)?;
    c.eq(seen(4), target)?;

    // Integer division by zero.
    let (at, sp) = divide_by_zero();
    check_common(&mut c, STATUS_INTEGER_DIVIDE_BY_ZERO, at, sp, 4)?;

    // An illegal instruction.
    let (at, sp) = illegal_instruction();
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

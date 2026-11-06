//! The x86 parts of msvcrt (SysWOW64's, ARCHITECTURE "Plan: WOW64"): the
//! C-variadic entry points, setjmp/longjmp with the x86 jump buffer, and
//! the floating-point control (the x87 control word and MXCSR).
//!
//! Sources: Microsoft Learn's x86 calling conventions (cdecl: arguments
//! on the stack, the caller pops them), its `_controlfp`/`_control87`
//! page, the `_JUMP_BUFFER` of mingw-w64's setjmp.h, and
//! Wine's msvcrt (its `_setjmp3`/`longjmp`) as documentation.

use crate::scan::vsscanf;
use crate::stdio::{_vsnprintf, vfprintf, vfwprintf, vprintf, vsprintf};
use crate::wide::{_execv, _execvp, _gala_vfscanf, _gala_vswprintf};
use core::arch::{asm, naked_asm};

#[link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn RtlUnwind(target_frame: usize, target_ip: usize, record: usize, value: usize);
}

// ---- Variadic entry points ------------------------------------------------

/// `name(fixed..., ...)` calling `target(fixed..., va_list)`: with cdecl
/// the variadic arguments follow the `fixed` ones on the stack, so the
/// `va_list` is their address (`va.rs`); the fixed arguments are pushed
/// again below it (each `push` moves the next one to the same offset).
macro_rules! variadic {
    ($($(#[$doc:meta])* $name:ident($fixed:literal) => $target:path;)*) => {$(
        $(#[$doc])*
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> i32 {
            naked_asm!(
                "lea eax, [esp + {list}]",
                "push eax",
                ".rept {fixed}",
                "push dword ptr [esp + {list}]",
                ".endr",
                "call {target}",
                "add esp, {list}",
                "ret",
                list = const 4 + 4 * $fixed,
                fixed = const $fixed,
                target = sym $target,
            )
        }
    )*};
}

variadic! {
    /// printf(fmt, ...).
    printf(1) => vprintf;
    /// fprintf(file, fmt, ...).
    fprintf(2) => vfprintf;
    /// sprintf(buf, fmt, ...).
    sprintf(2) => vsprintf;
    /// _snprintf(buf, count, fmt, ...).
    _snprintf(3) => _vsnprintf;
    /// fwprintf(file, fmt, ...).
    fwprintf(2) => vfwprintf;
    /// swprintf(buf, fmt, ...) (msvcrt's, without a count).
    swprintf(2) => _gala_vswprintf;
    /// sscanf(input, fmt, ...).
    sscanf(2) => vsscanf;
    /// fscanf(file, fmt, ...).
    fscanf(2) => _gala_vfscanf;
    /// _execl(path, arg0, ..., NULL): the arguments after the path are the
    /// NULL-terminated argv.
    _execl(1) => _execv;
    /// _execlp(file, arg0, ..., NULL).
    _execlp(1) => _execvp;
}

// ---- setjmp/longjmp -----------------------------------------------------------

/// The x86 `_JUMP_BUFFER` (setjmp.h; jmp_buf is 16 ints): the callee-saved
/// registers, the stack pointer and return address of the setjmp call,
/// the SEH registration (fs:[0]) and, from `_setjmp3`, its try level, the
/// "VC20" cookie and an unwind function with its data.
#[repr(C)]
pub struct JumpBuffer {
    ebp: u32,
    ebx: u32,
    edi: u32,
    esi: u32,
    esp: u32,
    eip: u32,
    registration: u32,
    try_level: u32,
    cookie: u32,
    unwind_func: u32,
    unwind_data: [u32; 6],
}

const _: () = assert!(size_of::<JumpBuffer>() == 64);

/// `_setjmp3`'s cookie ("VC20"): the buffer holds its try level and unwind
/// function.
const JMP_MAGIC: u32 = 0x5643_3230;
/// No `__try` level.
const TRYLEVEL_NONE: u32 = u32::MAX;

/// The rest of `_setjmp3(buf, count, unwind_func, try_level, data...)`
/// after the registers: up to `count` values (the unwind function, the try
/// level, then up to six words of data for it).
extern "C" fn setjmp3_rest(buf: &mut JumpBuffer, args: *const u32) {
    buf.cookie = JMP_MAGIC;
    buf.unwind_func = 0;
    buf.try_level = TRYLEVEL_NONE;
    buf.unwind_data = [0; 6];
    // SAFETY: the caller's argument count and the values after it.
    unsafe {
        let count = *args as usize;
        if count >= 1 {
            buf.unwind_func = *args.add(1);
        }
        if count >= 2 {
            buf.try_level = *args.add(2);
        }
        for i in 0..count.saturating_sub(2).min(6) {
            buf.unwind_data[i] = *args.add(3 + i);
        }
    }
}

/// `_setjmp(buf)`: the registers only (no cookie, no try level).
extern "C" fn setjmp_rest(buf: &mut JumpBuffer) {
    buf.cookie = 0;
    buf.unwind_func = 0;
    buf.try_level = TRYLEVEL_NONE;
    buf.unwind_data = [0; 6];
}

/// _setjmp3(buf, count, ...): what mingw's and MSVC's x86 `setjmp`
/// becomes. Saves ebp, ebx, edi, esi, the stack pointer after the return
/// and the return address, and the SEH registration; returns 0.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _setjmp3() -> i32 {
    naked_asm!(
        "mov edx, [esp + 4]",
        "mov [edx], ebp",
        "mov [edx + 4], ebx",
        "mov [edx + 8], edi",
        "mov [edx + 12], esi",
        "lea ecx, [esp + 4]",
        "mov [edx + 16], ecx",
        "mov ecx, [esp]",
        "mov [edx + 20], ecx",
        "mov ecx, fs:[0]",
        "mov [edx + 24], ecx",
        "lea ecx, [esp + 8]",
        "push ecx",
        "push edx",
        "call {rest}",
        "add esp, 8",
        "xor eax, eax",
        "ret",
        rest = sym setjmp3_rest,
    )
}

/// _setjmp(buf): `_setjmp3` without the extra values.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _setjmp() -> i32 {
    naked_asm!(
        "mov edx, [esp + 4]",
        "mov [edx], ebp",
        "mov [edx + 4], ebx",
        "mov [edx + 8], edi",
        "mov [edx + 12], esi",
        "lea ecx, [esp + 4]",
        "mov [edx + 16], ecx",
        "mov ecx, [esp]",
        "mov [edx + 20], ecx",
        "mov ecx, fs:[0]",
        "mov [edx + 24], ecx",
        "push edx",
        "call {rest}",
        "add esp, 4",
        "xor eax, eax",
        "ret",
        rest = sym setjmp_rest,
    )
}

/// The SEH registration list's head (TEB32's ExceptionList).
fn exception_list() -> u32 {
    let head: u32;
    // SAFETY: reads fs:[0], the running thread's TEB32.
    unsafe { asm!("mov {}, fs:[0]", out(reg) head, options(nostack, readonly, preserves_flags)) };
    head
}

/// Before longjmp moves the stack: the SEH frames registered since setjmp
/// are unwound (their handlers called with EXCEPTION_UNWINDING, fs:[0]
/// back to the buffer's registration, as RtlUnwind does), then a
/// `_setjmp3` buffer's unwind function runs (stdcall, the buffer).
extern "C" fn longjmp_unwind(buf: &JumpBuffer) {
    let target = buf.registration;
    if target != 0 && target != exception_list() {
        // SAFETY: the target frame is in this thread's chain (registered
        // before the setjmp, which is still live; 0xffffffff, the end of
        // the chain, unwinds every frame); RtlUnwind returns here.
        unsafe { RtlUnwind(target as usize, 0, 0, 0) };
    }
    if buf.cookie == JMP_MAGIC && buf.unwind_func != 0 {
        // SAFETY: the function the compiler stored for this buffer.
        let f: extern "system" fn(&JumpBuffer) =
            unsafe { core::mem::transmute(buf.unwind_func as usize) };
        f(buf);
    } else if buf.cookie == JMP_MAGIC && buf.try_level != TRYLEVEL_NONE {
        // A __try level without an unwind function: msvcrt would unwind
        // the scope table of the setjmp's own _except_handler3 frame
        // (_local_unwind2), which gala's msvcrt does not have yet.
        crate::missing(b"msvcrt.dll!longjmp(try level)", u64::from(buf.try_level));
    }
}

/// longjmp(buf, value): back to the setjmp that filled `buf`, which then
/// returns `value` (1 for 0).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn longjmp() -> ! {
    naked_asm!(
        "mov ebx, [esp + 4]",
        "mov esi, [esp + 8]",
        "push ebx",
        "call {unwind}",
        "add esp, 4",
        "mov eax, esi",
        "test eax, eax",
        "jnz 2f",
        "inc eax",
        "2:",
        "mov ebp, [ebx]",
        "mov edi, [ebx + 8]",
        "mov esi, [ebx + 12]",
        "mov esp, [ebx + 16]",
        "mov ecx, [ebx + 20]",
        "mov ebx, [ebx + 4]",
        "jmp ecx",
        unwind = sym longjmp_unwind,
    )
}

// ---- The floating-point control ----------------------------------------------------

/// msvcrt's abstract control word (float.h).
const EM_INVALID: u32 = 0x10;
const EM_DENORMAL: u32 = 0x8_0000;
const EM_ZERODIVIDE: u32 = 0x8;
const EM_OVERFLOW: u32 = 0x4;
const EM_UNDERFLOW: u32 = 0x2;
const EM_INEXACT: u32 = 0x1;
const MCW_RC: u32 = 0x300;
const MCW_PC: u32 = 0x3_0000;
const PC_24: u32 = 0x2_0000;
const PC_53: u32 = 0x1_0000;
const IC_AFFINE: u32 = 0x4_0000;
const DN_FLUSH: u32 = 0x0100_0000;
const MCW_DN: u32 = 0x0300_0000;
/// The x87 and SSE control words disagree (`_control87`'s answer).
const EM_AMBIGUOUS: u32 = 0x8000_0000;

/// (abstract mask bit, x87 control word bit, MXCSR bit) of the exceptions.
const EXCEPTIONS: [(u32, u32, u32); 6] = [
    (EM_INVALID, 0x1, 0x80),
    (EM_DENORMAL, 0x2, 0x100),
    (EM_ZERODIVIDE, 0x4, 0x200),
    (EM_OVERFLOW, 0x8, 0x400),
    (EM_UNDERFLOW, 0x10, 0x800),
    (EM_INEXACT, 0x20, 0x1000),
];

/// The x87 control word in the abstract form: the masks, the rounding
/// (RC, bits 10-11, in the same order as _RC_*), the precision (PC, bits
/// 8-9: 0 is 24 bits, 2 is 53, 3 is 64) and the infinity control.
fn from_x87(cw: u32) -> u32 {
    let mut word = EXCEPTIONS.iter().filter(|e| cw & e.1 != 0).fold(0, |w, e| w | e.0);
    word |= ((cw >> 10) & 3) << 8;
    word |= match (cw >> 8) & 3 {
        0 => PC_24,
        2 => PC_53,
        _ => 0, // _PC_64
    };
    if cw & 0x1000 != 0 {
        word |= IC_AFFINE;
    }
    word
}

fn to_x87(word: u32, cw: u32) -> u32 {
    let mut out = cw & !(0x3f | 0x300 | 0xc00 | 0x1000);
    for (a, x, _) in EXCEPTIONS {
        if word & a != 0 {
            out |= x;
        }
    }
    out |= ((word & MCW_RC) >> 8) << 10;
    out |= match word & MCW_PC {
        PC_24 => 0,
        PC_53 => 0x200,
        _ => 0x300,
    };
    if word & IC_AFFINE != 0 {
        out |= 0x1000;
    }
    out
}

/// MXCSR in the abstract form: the masks, the rounding (bits 13-14) and
/// flush-to-zero (_DN_FLUSH).
fn from_sse(mx: u32) -> u32 {
    let mut word = EXCEPTIONS.iter().filter(|e| mx & e.2 != 0).fold(0, |w, e| w | e.0);
    word |= ((mx >> 13) & 3) << 8;
    if mx & 0x8000 != 0 {
        word |= DN_FLUSH;
    }
    word
}

fn to_sse(word: u32, mx: u32) -> u32 {
    let mut out = mx & !(0x1f80 | 0x6000 | 0x8000);
    for (a, _, m) in EXCEPTIONS {
        if word & a != 0 {
            out |= m;
        }
    }
    out |= ((word & MCW_RC) >> 8) << 13;
    if word & MCW_DN == DN_FLUSH {
        out |= 0x8000;
    }
    out
}

fn read_control() -> (u32, u32) {
    let (mut cw, mut mx) = (0u16, 0u32);
    // SAFETY: stores the two control words into locals.
    unsafe {
        asm!("fnstcw [{}]", in(reg) &raw mut cw, options(nostack));
        asm!("stmxcsr [{}]", in(reg) &raw mut mx, options(nostack));
    }
    (u32::from(cw), mx)
}

fn write_control(cw: u32, mx: u32) {
    let cw = cw as u16;
    // SAFETY: loads control words whose status bits are the old ones.
    unsafe {
        asm!("fldcw [{}]", in(reg) &raw const cw, options(nostack));
        asm!("ldmxcsr [{}]", in(reg) &raw const mx, options(nostack));
    }
}

/// _control87(new, mask): both control words (x87 and SSE) get `new`'s
/// `mask` bits, as on x86 Windows; the new word, the x87 one's form with
/// SSE's denormal control, and _EM_AMBIGUOUS when the two disagree on the
/// masks or the rounding.
#[unsafe(no_mangle)]
pub extern "C" fn _control87(new: u32, mask: u32) -> u32 {
    let (cw, mx) = read_control();
    let x87 = (from_x87(cw) & !mask) | (new & mask);
    let sse = (from_sse(mx) & !mask) | (new & mask);
    let (cw, mx) = (to_x87(x87, cw), to_sse(sse, mx));
    write_control(cw, mx);
    let (x87, sse) = (from_x87(cw), from_sse(mx));
    let common = 0x8_001f | MCW_RC;
    let mut word = x87 | (sse & MCW_DN);
    if x87 & common != sse & common {
        word |= EM_AMBIGUOUS;
    }
    word
}

/// _controlfp(new, mask): `_control87` that leaves the denormal exception
/// mask alone (as documented).
#[unsafe(no_mangle)]
pub extern "C" fn _controlfp(new: u32, mask: u32) -> u32 {
    _control87(new, mask & !EM_DENORMAL)
}

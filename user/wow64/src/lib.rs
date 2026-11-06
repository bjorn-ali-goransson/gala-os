//! gala's wow64.dll: the 64-bit side of a 32-bit (WOW64) process
//! (ARCHITECTURE "Plan: WOW64").
//!
//! The kernel maps it into every 32-bit process, below the process's
//! 32-bit limit, and starts each of its threads at `GalaWow64Start` with
//! the thread's CPU area (TEB64.TlsSlots[1], WOW64_TLS_CPURESERVED: the
//! WOW64_CPURESERVED header, the thread's 32-bit context, its 64-bit stack
//! pointer). It enters 32-bit code (CS 0x23) with that context.
//!
//! A 32-bit system call (`mov eax, N; call fs:[0xc0]`, the x64 service
//! number) comes to `GalaWow64Transition`, 32-bit code that switches to
//! 64-bit mode (CS 0x33); the 64-bit part saves the caller's registers in
//! the CPU area, moves to the thread's 64-bit stack and calls
//! `gala_wow64_service`, which converts the 32-bit arguments and
//! structures, makes the 64-bit system call and converts the results
//! back. Then the thread re-enters 32-bit code after the caller's `call`,
//! with the status in eax. A service without a thunk yet fails with
//! STATUS_NOT_IMPLEMENTED and is recorded in the ledger
//! (`wow64.dll!service 0x...`).
//!
//! Conversions: `generic` (services converted argument by argument, from
//! a table of argument kinds), `info` (information classes and arrays),
//! `thread` (32-bit threads and their contexts). Every access to the
//! caller's memory goes through `mem`, fault-safe: an invalid 32-bit
//! pointer fails the call with STATUS_ACCESS_VIOLATION (the kernel hands
//! faults in wow64.dll's 64-bit code to `GalaWow64Exception`).
//!
//! No imports: system calls are made directly (`gala_wow64_syscall`).

#![no_std]

use core::arch::global_asm;

mod generic;
mod info;
mod mem;
mod thread;

// The CPU area (the kernel's `layout::wow64::cpu` and `context32`).
const TEB64_CPU_SLOT: u64 = 0x1480 + 8;
const CPU_CONTEXT: u64 = 0x04;
const CPU_STACK64: u64 = 0x2d0;
const CTX_EDI: u64 = CPU_CONTEXT + 0x9c;
const CTX_ESI: u64 = CPU_CONTEXT + 0xa0;
const CTX_EBX: u64 = CPU_CONTEXT + 0xa4;
const CTX_EDX: u64 = CPU_CONTEXT + 0xa8;
const CTX_ECX: u64 = CPU_CONTEXT + 0xac;
const CTX_EAX: u64 = CPU_CONTEXT + 0xb0;
const CTX_EBP: u64 = CPU_CONTEXT + 0xb4;
const CTX_EIP: u64 = CPU_CONTEXT + 0xb8;
const CTX_CS: u64 = CPU_CONTEXT + 0xbc;
const CTX_EFLAGS: u64 = CPU_CONTEXT + 0xc0;
const CTX_ESP: u64 = CPU_CONTEXT + 0xc4;
const CTX_SS: u64 = CPU_CONTEXT + 0xc8;
/// The flags 32-bit code may set (CF, PF, AF, ZF, SF, DF, OF), and IF.
const USER_FLAGS: u64 = 0xcd5;

global_asm!(
    // GalaWow64Start: rcx = the CPU area, rsp = the 64-bit stack (as if
    // called). The 64-bit stack's top is where every transition from
    // 32-bit code starts.
    ".globl GalaWow64Start",
    "GalaWow64Start:",
    "and rsp, -16",
    "mov [rcx + {stack64}], rsp",
    "jmp gala_wow64_enter32",
    // Enters 32-bit code with the context in the CPU area at rcx. DS, ES
    // and FS (the TEB32) are the kernel's to set (sched::set_teb32); GS
    // stays: its base is the 64-bit TEB, which the transition reads.
    "gala_wow64_enter32:",
    "mov eax, [rcx + {ss}]",
    "push rax",
    "mov eax, [rcx + {esp}]",
    "push rax",
    "mov eax, [rcx + {eflags}]",
    "and eax, {flags}",
    "or eax, 0x202",
    "push rax",
    "mov eax, [rcx + {cs}]",
    "push rax",
    "mov eax, [rcx + {eip}]",
    "push rax",
    "mov edi, [rcx + {edi}]",
    "mov esi, [rcx + {esi}]",
    "mov ebx, [rcx + {ebx}]",
    "mov ebp, [rcx + {ebp}]",
    "mov edx, [rcx + {edx}]",
    "mov eax, [rcx + {eax}]",
    "mov ecx, [rcx + {ecx}]",
    "iretq",
    // GalaWow64Transition: 32-bit code. eax = the service number; [esp] =
    // the return into the stub, [esp + 4] the stub's caller's, the
    // arguments above. A far return to CS 0x33 at label 2, position
    // independent: label 1's address (from a call) plus the 6 bytes of
    // `pop ecx; add ecx, 6; push ecx; retf` (59 83 c1 06 51 cb). ecx is
    // the stub's to clobber, as on Windows.
    ".globl GalaWow64Transition",
    "GalaWow64Transition:",
    ".code32",
    "push 0x33",
    "call 1f",
    "1:",
    "pop ecx",
    "add ecx, 6",
    "push ecx",
    "retf",
    ".code64",
    "2:",
    // The upper halves of the registers are undefined after compatibility
    // mode: the 32-bit values, zero-extended.
    "mov esp, esp",
    "mov eax, eax",
    "mov r11, gs:[0x30]",
    "mov r11, [r11 + {slot}]",
    "mov [r11 + {edi}], edi",
    "mov [r11 + {esi}], esi",
    "mov [r11 + {ebx}], ebx",
    "mov [r11 + {ebp}], ebp",
    "mov [r11 + {eax}], eax",
    "mov [r11 + {ecx}], ecx",
    "mov [r11 + {edx}], edx",
    "mov r10d, [rsp]",
    "mov [r11 + {eip}], r10d",
    "lea r10d, [rsp + 4]",
    "mov [r11 + {esp}], r10d",
    "pushfq",
    "pop r10",
    "mov [r11 + {eflags}], r10d",
    "lea edx, [rsp + 8]",
    "mov ecx, eax",
    "mov r8, r11",
    "mov rsp, [r11 + {stack64}]",
    "sub rsp, 0x20",
    "call gala_wow64_service",
    "mov r11, gs:[0x30]",
    "mov rcx, [r11 + {slot}]",
    "mov [rcx + {eax}], eax",
    "jmp gala_wow64_enter32",
    // gala_wow64_syscall(number: rcx, args: rdx = 16 u64s) -> rax: the
    // 64-bit system call (the x64 convention: r10, rdx, r8, r9, then the
    // stack above a return address and the shadow area).
    ".globl gala_wow64_syscall",
    "gala_wow64_syscall:",
    "push rbp",
    "mov rbp, rsp",
    "push rsi",
    "push rdi",
    "sub rsp, 0xb0",
    "and rsp, -16",
    "mov rax, rcx",
    "lea rsi, [rdx + 0x20]",
    "lea rdi, [rsp + 0x28]",
    "mov ecx, 12",
    "rep movsq",
    "mov r10, [rdx]",
    "mov r8, [rdx + 0x10]",
    "mov r9, [rdx + 0x18]",
    "mov rdx, [rdx + 0x08]",
    "syscall",
    "lea rsp, [rbp - 0x10]",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "ret",
    // gala_wow64_copy(dst: rcx, src: rdx, len: r8) -> rax: 0, or 1 if a
    // byte faulted. `gala_wow64_copy_access` is its one instruction that
    // touches a caller's memory: GalaWow64Exception resumes a fault there
    // at `gala_wow64_copy_fault` (rsp as at the fault: the two pushes).
    ".globl gala_wow64_copy",
    "gala_wow64_copy:",
    "push rsi",
    "push rdi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    ".globl gala_wow64_copy_access",
    "gala_wow64_copy_access:",
    "rep movsb",
    "xor eax, eax",
    "pop rdi",
    "pop rsi",
    "ret",
    ".globl gala_wow64_copy_fault",
    "gala_wow64_copy_fault:",
    "cld",
    "mov eax, 1",
    "pop rdi",
    "pop rsi",
    "ret",
    // GalaWow64Exception: the kernel's exception dispatch for wow64.dll's
    // 64-bit code, as for ntdll's KiUserExceptionDispatcher: rsp -> the
    // CONTEXT, the EXCEPTION_RECORD at +0x4f0.
    ".globl GalaWow64Exception",
    "GalaWow64Exception:",
    "mov rcx, rsp",
    "lea rdx, [rsp + 0x4f0]",
    "and rsp, -16",
    "sub rsp, 0x20",
    "call gala_wow64_exception",
    "ud2",
    slot = const TEB64_CPU_SLOT,
    stack64 = const CPU_STACK64,
    edi = const CTX_EDI,
    esi = const CTX_ESI,
    ebx = const CTX_EBX,
    edx = const CTX_EDX,
    ecx = const CTX_ECX,
    eax = const CTX_EAX,
    ebp = const CTX_EBP,
    eip = const CTX_EIP,
    cs = const CTX_CS,
    eflags = const CTX_EFLAGS,
    esp = const CTX_ESP,
    ss = const CTX_SS,
    flags = const USER_FLAGS,
);

unsafe extern "C" {
    fn gala_wow64_syscall(number: u64, args: *const [u64; 16]) -> u64;
    fn gala_wow64_copy_access();
    fn gala_wow64_copy_fault();
}

type NtStatus = u32;
const STATUS_SUCCESS: NtStatus = 0;
const STATUS_BUFFER_OVERFLOW: NtStatus = 0x8000_0005;
const STATUS_NOT_IMPLEMENTED: NtStatus = 0xc000_0002;
const STATUS_INVALID_INFO_CLASS: NtStatus = 0xc000_0003;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
const STATUS_ACCESS_VIOLATION: NtStatus = 0xc000_0005;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xc000_0023;
const STATUS_NOT_SUPPORTED: NtStatus = 0xc000_00bb;

const NT_TERMINATE_PROCESS: u32 = 0x2c;
const NT_CONTINUE: u32 = 0x43;
const GALA_RECORD_MISSING: u32 = 0x7f00;
const GALA_UNHANDLED_EXCEPTION: u32 = 0x7f5c;

/// Whether `status` is an error (NT_ERROR: severity 3).
fn is_error(status: NtStatus) -> bool {
    status >= 0xc000_0000
}

/// The 64-bit system call `number` with `args`.
fn syscall(number: u32, args: &[u64]) -> NtStatus {
    let mut all = [0u64; 16];
    all[..args.len()].copy_from_slice(args);
    // SAFETY: the kernel validates every argument; the array is 16 long,
    // as the assembly reads.
    unsafe { gala_wow64_syscall(u64::from(number), &all) as NtStatus }
}

/// Records a gap in the missing-features ledger (GalaRecordMissing, what
/// 2: unsupported).
fn unsupported(name: &[u8], detail: u64) {
    syscall(GALA_RECORD_MISSING, &[name.as_ptr() as u64, name.len() as u64, 2, detail]);
}

/// Records service `number`'s missing case `what` (`wow64.dll!service
/// 0x0045 (32-bit APC)`).
fn unsupported_service(number: u32, what: &[u8]) {
    let mut name = [0u8; 64];
    let prefix = b"wow64.dll!service 0x0000 (";
    name[..prefix.len()].copy_from_slice(prefix);
    hex4(&mut name[20..24], number);
    let what = &what[..what.len().min(64 - prefix.len() - 1)];
    name[prefix.len()..prefix.len() + what.len()].copy_from_slice(what);
    let len = prefix.len() + what.len();
    name[len] = b')';
    unsupported(&name[..len + 1], number.into());
}

/// A 32-bit caller's arguments (copied from its stack, fault-safe).
pub struct Args {
    pub v: [u32; 16],
}

impl Args {
    fn read(at: u32, count: usize) -> mem::Res<Args> {
        let mut raw = [0u8; 64];
        mem::read(u64::from(at), &mut raw[..4 * count])?;
        let mut v = [0u32; 16];
        for (i, w) in v.iter_mut().enumerate() {
            *w = mem::u32_at(&raw, 4 * i);
        }
        Ok(Args { v })
    }
    fn u(&self, i: usize) -> u32 {
        self.v[i]
    }
    /// A pointer, zero-extended.
    fn p(&self, i: usize) -> u64 {
        u64::from(self.v[i])
    }
    /// A handle, sign-extended (pseudo-handles are negative).
    fn h(&self, i: usize) -> u64 {
        self.v[i] as i32 as i64 as u64
    }
}

/// The number of arguments of the services converted here (None: not
/// converted).
fn argument_count(number: u32) -> Option<usize> {
    if let Some(kinds) = generic::kinds(number) {
        return Some(kinds.len());
    }
    info::SERVICES.iter().chain(thread::SERVICES).find(|(n, _)| *n == number).map(|&(_, c)| c)
}

/// The 32-bit system call `number` with its arguments at `args`, from the
/// thread whose CPU area is `cpu`; the value for eax (the status, or
/// after NtContinue the context's eax).
#[unsafe(no_mangle)]
extern "C" fn gala_wow64_service(number: u32, args: u32, cpu: u64) -> u32 {
    let Some(count) = argument_count(number) else {
        let mut name = *b"wow64.dll!service 0x0000";
        hex4(&mut name[20..], number);
        unsupported(&name, number.into());
        return STATUS_NOT_IMPLEMENTED;
    };
    let a = match Args::read(args, count) {
        Ok(a) => a,
        Err(e) => return e,
    };
    if let Some(kinds) = generic::kinds(number) {
        return generic::call(number, kinds, &a.v[..count]);
    }
    if info::SERVICES.iter().any(|(n, _)| *n == number) {
        return info::call(number, &a);
    }
    thread::call(number, &a, cpu)
}

fn hex4(out: &mut [u8], value: u32) {
    for (i, b) in out.iter_mut().enumerate().take(4) {
        *b = b"0123456789abcdef"[((value >> (12 - 4 * i)) & 0xf) as usize];
    }
}

/// An exception in wow64.dll's own 64-bit code (`context`: the x64
/// CONTEXT, `record`: the EXCEPTION_RECORD). A fault on a caller's memory
/// in `gala_wow64_copy` resumes at its failure exit; anything else is a
/// bug here: reported (GalaUnhandledException: the crash report shows
/// wow64.dll's registers) and the process ends.
#[unsafe(no_mangle)]
extern "C" fn gala_wow64_exception(context: u64, record: u64) -> ! {
    const RIP: u64 = 0xf8;
    // SAFETY: the kernel put both on this stack for this call.
    let (rip, code) = unsafe {
        (core::ptr::read((context + RIP) as *const u64), core::ptr::read(record as *const u32))
    };
    if rip == gala_wow64_copy_access as *const () as u64 {
        // SAFETY: as above; the CONTEXT is this thread's to change.
        unsafe {
            core::ptr::write((context + RIP) as *mut u64, gala_wow64_copy_fault as *const () as u64)
        };
        syscall(NT_CONTINUE, &[context, 0]);
    }
    syscall(GALA_UNHANDLED_EXCEPTION, &[record, context]);
    syscall(NT_TERMINATE_PROCESS, &[u64::MAX, code.into()]);
    loop {
        core::hint::spin_loop();
    }
}

// The memory intrinsics the compiler calls (wow64.dll imports nothing),
// with `rep` string instructions so they cannot become calls to
// themselves.
global_asm!(
    ".globl memcpy",
    "memcpy:",
    "mov rax, rcx",
    "push rdi",
    "push rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "rep movsb",
    "pop rsi",
    "pop rdi",
    "ret",
    ".globl memmove",
    "memmove:",
    "mov rax, rcx",
    "push rdi",
    "push rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "cmp rdi, rsi",
    "jbe 2f",
    // Overlapping with the destination above: backwards.
    "lea rsi, [rsi + rcx - 1]",
    "lea rdi, [rdi + rcx - 1]",
    "std",
    "rep movsb",
    "cld",
    "jmp 3f",
    "2:",
    "rep movsb",
    "3:",
    "pop rsi",
    "pop rdi",
    "ret",
    ".globl memset",
    "memset:",
    "mov r9, rcx",
    "push rdi",
    "mov rdi, rcx",
    "mov eax, edx",
    "mov rcx, r8",
    "rep stosb",
    "pop rdi",
    "mov rax, r9",
    "ret",
    // __chkstk (rax = the frame's size): touches each page of a frame
    // larger than one, top down, so the 64-bit stack grows through its
    // guard page; changes no register but the flags.
    ".globl __chkstk",
    "__chkstk:",
    "push rcx",
    "push rax",
    "lea rcx, [rsp + 24]",
    "cmp rax, 0x1000",
    "jb 2f",
    "1:",
    "sub rcx, 0x1000",
    "test [rcx], rcx",
    "sub rax, 0x1000",
    "cmp rax, 0x1000",
    "ja 1b",
    "2:",
    "sub rcx, rax",
    "test [rcx], rcx",
    "pop rax",
    "pop rcx",
    "ret",
);

/// MSVC marker symbol that object files using floating point reference.
#[unsafe(no_mangle)]
#[used]
pub static _fltused: i32 = 0;

/// Unwind personality referenced by the prebuilt `core` library's unwind
/// tables. With `panic = abort` nothing unwinds, so it is never called.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    syscall(NT_TERMINATE_PROCESS, &[u64::MAX, 0xc000_0409]);
    loop {
        core::hint::spin_loop();
    }
}

/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        // SAFETY: within the caller-guaranteed ranges.
        let (x, y) =
            unsafe { (core::ptr::read_volatile(a.add(i)), core::ptr::read_volatile(b.add(i))) };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

/// Same as `memcmp` (LLVM may call it for equality-only comparisons).
///
/// # Safety
/// Both ranges must be readable for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: forwarded.
    unsafe { memcmp(a, b, n) }
}

/// The DLL's entry point (never called: the kernel maps the DLL).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // NtTerminateProcess(NtCurrentProcess(), STATUS_STACK_BUFFER_OVERRUN).
    syscall(NT_TERMINATE_PROCESS, &[u64::MAX, 0xc000_0409]);
    loop {
        core::hint::spin_loop();
    }
}

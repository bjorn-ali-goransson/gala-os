//! The x64 parts of kernel32 (System32's): the TEB through GS and the
//! offsets of the x64 TEB, PEB, process parameters, CONTEXT and PE32+
//! optional header (the public x64 layouts: winnt.h, winternl.h). The x86
//! build (SysWOW64's kernel32, ARCHITECTURE "Plan: WOW64") has the same
//! names in `x86.rs`.

/// TEB offsets (x64).
pub const TEB_STACK_BASE: usize = 0x08;
pub const TEB_SELF: usize = 0x30;
pub const TEB_CLIENT_PROCESS: usize = 0x40;
pub const TEB_CLIENT_THREAD: usize = 0x48;
pub const TEB_PEB: usize = 0x60;
pub const TEB_LAST_ERROR: usize = 0x68;
pub const TEB_DEALLOCATION_STACK: usize = 0x1478;
pub const TEB_TLS_SLOTS: usize = 0x1480;
pub const TEB_HARD_ERROR_MODE: usize = 0x16b0;
pub const TEB_GUARANTEED_STACK_BYTES: usize = 0x1748;

/// PEB offsets (x64).
pub const PEB_BEING_DEBUGGED: usize = 0x02;
pub const PEB_IMAGE_BASE: usize = 0x10;
pub const PEB_PROCESS_PARAMETERS: usize = 0x20;
pub const PEB_PROCESS_HEAP: usize = 0x30;
pub const PEB_SESSION_ID: usize = 0x2c0;

/// RTL_USER_PROCESS_PARAMETERS offsets (x64): the standard handles
/// (input, output, error: pointer-sized, in that order), CurrentDirectory
/// (DosPath, a UNICODE_STRING, then Handle), ImagePathName, CommandLine
/// and RuntimeData (UNICODE_STRINGs: Length, MaximumLength, Buffer),
/// Environment, EnvironmentSize.
pub const PARAMS_STANDARD_INPUT: usize = 0x20;
pub const PARAMS_CURRENT_DIRECTORY: usize = 0x38;
pub const PARAMS_CURRENT_DIRECTORY_HANDLE: usize = 0x48;
pub const PARAMS_IMAGE_PATH: usize = 0x60;
pub const PARAMS_COMMAND_LINE: usize = 0x70;
pub const PARAMS_ENVIRONMENT: usize = 0x80;
pub const PARAMS_RUNTIME_DATA: usize = 0xe0;
pub const PARAMS_ENVIRONMENT_SIZE: usize = 0x3f0;

/// The optional header's SizeOfStackReserve and SizeOfStackCommit (PE32+:
/// 8 bytes each), from the optional header's start.
pub const OPTIONAL_STACK_RESERVE: usize = 72;
pub const OPTIONAL_STACK_COMMIT: usize = 80;

/// CONTEXT (x64): its size (16-byte aligned), where ContextFlags is, the
/// architecture flag, the flag of the FXSAVE area (FltSave) and of the
/// XSTATE, where FltSave is, and the byte ranges each part flag covers.
pub const CONTEXT_SIZE: usize = 0x4d0;
pub const CONTEXT_FLAGS_AT: usize = 0x30;
pub const CONTEXT_ARCH: u32 = 0x10_0000;
pub const CONTEXT_LEGACY_STATE: u32 = 0x10_0008; // CONTEXT_FLOATING_POINT
pub const CONTEXT_XSTATE: u32 = 0x10_0040;
pub const CONTEXT_FXSAVE: usize = 0x100;
pub const CONTEXT_PARTS: &[(u32, &[(usize, usize)])] = &[
    // CONTEXT_CONTROL: SegCs, SegSs, EFlags; Rsp; Rip.
    (0x1, &[(0x38, 0x3a), (0x42, 0x48), (0x98, 0xa0), (0xf8, 0x100)]),
    // CONTEXT_INTEGER: Rax..Rbx, Rbp..R15 (not Rsp).
    (0x2, &[(0x78, 0x98), (0xa0, 0xf8)]),
    // CONTEXT_SEGMENTS: SegDs, SegEs, SegFs, SegGs.
    (0x4, &[(0x3a, 0x42)]),
    // CONTEXT_FLOATING_POINT: MxCsr, FltSave.
    (0x8, &[(0x34, 0x38), (0x100, 0x300)]),
    // CONTEXT_DEBUG_REGISTERS: Dr0-Dr3, Dr6, Dr7.
    (0x10, &[(0x48, 0x78)]),
];

/// PROCESSOR_ARCHITECTURE_AMD64 and PROCESSOR_AMD_X8664: what
/// GetSystemInfo reports.
pub const PROCESSOR_ARCHITECTURE: u16 = 9;
pub const PROCESSOR_TYPE: u32 = 8664;

/// The native system directory: System32 (a 32-bit build names it
/// Sysnative).
pub const NATIVE_SYSTEM_DIRECTORY: &[u8] = b"\\system32";

/// Reads the pointer-sized field of the current TEB at `offset`.
pub fn teb_read(offset: usize) -> usize {
    let v: usize;
    // SAFETY: gs:[offset] is inside the current thread's TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    v
}

/// Writes the pointer-sized field of the current TEB at `offset`.
pub fn teb_write(offset: usize, value: usize) {
    // SAFETY: gs:[offset] is inside the current thread's TEB, a field the
    // caller names.
    unsafe { core::arch::asm!("mov gs:[{}], {}", in(reg) offset, in(reg) value, options(nostack)) };
}

/// Writes the 32-bit field of the current TEB at `offset`.
pub fn teb_write_u32(offset: usize, value: u32) {
    // SAFETY: as teb_write.
    unsafe {
        core::arch::asm!("mov gs:[{}], {:e}", in(reg) offset, in(reg) value, options(nostack))
    };
}

/// The time stamp counter.
pub fn rdtsc() -> u64 {
    // SAFETY: rdtsc only reads the time stamp counter.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The time stamp counter and IA32_TSC_AUX (rdtscp).
pub fn rdtscp(aux: &mut u32) -> u64 {
    // SAFETY: rdtscp reads the time stamp counter and TSC_AUX.
    unsafe { core::arch::x86_64::__rdtscp(aux) }
}

pub use core::arch::x86_64::{__cpuid as cpuid, __cpuid_count as cpuid_count};

/// CONTEXT (x64 layout, 0x4d0 bytes, 16-byte aligned).
#[repr(C, align(16))]
pub struct Context {
    home: [u64; 6],
    context_flags: u32,
    mx_csr: u32,
    /// SegCs, SegDs, SegEs, SegFs, SegGs, SegSs.
    segs: [u16; 6],
    eflags: u32,
    dr: [u64; 6],
    /// Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8..R15.
    gpr: [u64; 16],
    rip: u64,
    rest: [u8; 0x4d0 - 0x100],
}

/// The CONTEXT a new thread starts with: at `entry(args[0], args[1])`
/// with its stack ending at `top`, as if called.
pub fn thread_context(entry: usize, args: [usize; 2], top: usize) -> Context {
    // SAFETY: an all-zero CONTEXT is a valid bit pattern.
    let mut context: Context = unsafe { core::mem::zeroed() };
    context.context_flags = 0x10_000b; // CONTEXT_FULL
    context.mx_csr = 0x1f80;
    context.segs = [0x33, 0x2b, 0x2b, 0x53, 0x2b, 0x2b];
    context.eflags = 0x200;
    context.rip = entry as u64;
    // The start routine also in rax, where NtCreateThread takes the
    // thread's Win32 start address (ThreadQuerySetWin32StartAddress).
    context.gpr[0] = args[0] as u64; // rax
    context.gpr[1] = args[0] as u64; // rcx
    context.gpr[2] = args[1] as u64; // rdx
    context.gpr[4] = (top - 0x28) as u64; // rsp, as if called
    context
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER (x64, 16 bytes): the first entry in the high quadword (bits
// 4..63, bit 0 = HeaderType 1), depth (16 bits) and a sequence number (48
// bits) in the low one; both change together with cmpxchg16b, so a pop
// cannot be fooled by an entry popped and pushed again meanwhile.

#[repr(C, align(16))]
pub struct SListHeader {
    low: u64,
    high: u64,
}

#[repr(C)]
pub struct SListEntry {
    pub next: *mut SListEntry,
}

/// A header's value: (depth and sequence, first entry and type).
pub type SListValue = (u64, u64);

/// An empty header's value.
pub const SLIST_EMPTY: SListValue = (0, 1);

/// Compares the header with `old` and, if equal, stores `new`; returns
/// the value seen.
pub fn slist_cas(head: *mut SListHeader, old: SListValue, new: SListValue) -> SListValue {
    let (mut low, mut high) = old;
    // SAFETY: a 16-byte aligned SLIST_HEADER; cmpxchg16b compares rdx:rax
    // with it and stores rcx:rbx if equal, else loads it into rdx:rax.
    // rbx is reserved by LLVM, so it is swapped in and out around the
    // instruction.
    unsafe {
        core::arch::asm!(
            "xchg rbx, {new_low}",
            "lock cmpxchg16b [{head}]",
            "mov rbx, {new_low}",
            head = in(reg) head,
            new_low = inout(reg) new.0 => _,
            in("rcx") new.1,
            inout("rax") low,
            inout("rdx") high,
        );
    }
    (low, high)
}

/// The header's value, read atomically (a compare-exchange that fails,
/// or rewrites the same value).
pub fn slist_read(head: *mut SListHeader) -> SListValue {
    slist_cas(head, (0, 0), (0, 0))
}

pub fn slist_first(v: SListValue) -> *mut SListEntry {
    (v.1 & !0xf) as *mut SListEntry
}

pub fn slist_depth(v: SListValue) -> u16 {
    v.0 as u16
}

/// The value with `first` on top, `depth` entries and the next sequence
/// number.
pub fn slist_value(old: SListValue, first: *mut SListEntry, depth: u16) -> SListValue {
    let sequence = (old.0 >> 16).wrapping_add(1) & 0xffff_ffff_ffff;
    (u64::from(depth) | sequence << 16, first as u64 | 1)
}

pub fn slist_init(head: *mut SListHeader) {
    // SAFETY: the caller's aligned header.
    unsafe { head.write(SListHeader { low: SLIST_EMPTY.0, high: SLIST_EMPTY.1 }) };
}

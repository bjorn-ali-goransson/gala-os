//! The x86 parts of kernel32, built as the 32-bit kernel32 in
//! `C:\Windows\SysWOW64` (ARCHITECTURE "Plan: WOW64"): the TEB32 through
//! FS and the offsets of the x86 TEB, PEB, process parameters, CONTEXT
//! and PE32 optional header (the public x86 layouts: winnt.h,
//! winternl.h). The names are `x64.rs`'s.

// The memory routines, MSVC's 64-bit division helpers and the unwind
// personality the compiler calls, and strlen (x64: ntdll's): ntrt's x86
// runtime (nothing else of it).
pub use ntrt::strlen;

/// TEB offsets (x86).
pub const TEB_STACK_BASE: usize = 0x04;
pub const TEB_SELF: usize = 0x18;
pub const TEB_CLIENT_PROCESS: usize = 0x20;
pub const TEB_CLIENT_THREAD: usize = 0x24;
pub const TEB_PEB: usize = 0x30;
pub const TEB_LAST_ERROR: usize = 0x34;
pub const TEB_DEALLOCATION_STACK: usize = 0xe0c;
pub const TEB_TLS_SLOTS: usize = 0xe10;
pub const TEB_HARD_ERROR_MODE: usize = 0xf28;
pub const TEB_GUARANTEED_STACK_BYTES: usize = 0xf78;

/// PEB offsets (x86).
pub const PEB_BEING_DEBUGGED: usize = 0x02;
pub const PEB_IMAGE_BASE: usize = 0x08;
pub const PEB_PROCESS_PARAMETERS: usize = 0x10;
pub const PEB_PROCESS_HEAP: usize = 0x18;
pub const PEB_SESSION_ID: usize = 0x1d4;

/// RTL_USER_PROCESS_PARAMETERS offsets (x86): as `x64.rs`'s, with 4-byte
/// pointers and handles (UNICODE_STRING is 8 bytes,
/// RTL_DRIVE_LETTER_CURDIR 16).
pub const PARAMS_STANDARD_INPUT: usize = 0x18;
pub const PARAMS_CURRENT_DIRECTORY: usize = 0x24;
pub const PARAMS_CURRENT_DIRECTORY_HANDLE: usize = 0x2c;
pub const PARAMS_IMAGE_PATH: usize = 0x38;
pub const PARAMS_COMMAND_LINE: usize = 0x40;
pub const PARAMS_ENVIRONMENT: usize = 0x48;
pub const PARAMS_RUNTIME_DATA: usize = 0x88;
pub const PARAMS_ENVIRONMENT_SIZE: usize = 0x290;

/// The optional header's SizeOfStackReserve and SizeOfStackCommit (PE32:
/// 4 bytes each, BaseOfData before ImageBase), from its start.
pub const OPTIONAL_STACK_RESERVE: usize = 72;
pub const OPTIONAL_STACK_COMMIT: usize = 76;

/// CONTEXT (x86, 0x2cc bytes): ContextFlags first; CONTEXT_i386; the
/// FXSAVE area is ExtendedRegisters (CONTEXT_EXTENDED_REGISTERS);
/// CONTEXT_XSTATE; the byte ranges of each part flag.
pub const CONTEXT_SIZE: usize = 0x2cc;
pub const CONTEXT_FLAGS_AT: usize = 0;
pub const CONTEXT_ARCH: u32 = 0x1_0000;
pub const CONTEXT_LEGACY_STATE: u32 = 0x1_0020; // CONTEXT_EXTENDED_REGISTERS
pub const CONTEXT_XSTATE: u32 = 0x1_0040;
pub const CONTEXT_FXSAVE: usize = 0xcc;
pub const CONTEXT_PARTS: &[(u32, &[(usize, usize)])] = &[
    // CONTEXT_CONTROL: Ebp, Eip, SegCs, EFlags, Esp, SegSs.
    (0x1, &[(0xb4, 0xcc)]),
    // CONTEXT_INTEGER: Edi, Esi, Ebx, Edx, Ecx, Eax.
    (0x2, &[(0x9c, 0xb4)]),
    // CONTEXT_SEGMENTS: SegGs, SegFs, SegEs, SegDs.
    (0x4, &[(0x8c, 0x9c)]),
    // CONTEXT_FLOATING_POINT: FloatSave.
    (0x8, &[(0x1c, 0x8c)]),
    // CONTEXT_DEBUG_REGISTERS: Dr0-Dr3, Dr6, Dr7.
    (0x10, &[(0x04, 0x1c)]),
    // CONTEXT_EXTENDED_REGISTERS: the FXSAVE area.
    (0x20, &[(0xcc, 0x2cc)]),
];

/// PROCESSOR_ARCHITECTURE_INTEL and PROCESSOR_INTEL_PENTIUM: what
/// GetSystemInfo tells a 32-bit program under WOW64 (GetNativeSystemInfo
/// gives the native AMD64 answers).
pub const PROCESSOR_ARCHITECTURE: u16 = 0;
pub const PROCESSOR_TYPE: u32 = 586;

/// The system directory a 32-bit program is given: SysWOW64, as
/// documented for GetSystemDirectory under WOW64.
pub const SYSTEM_DIRECTORY: &[u8] = b"\\SysWOW64";

/// Reads the pointer-sized field of the current TEB32 at `offset`.
pub fn teb_read(offset: usize) -> usize {
    let v: usize;
    // SAFETY: fs:[offset] is inside the current thread's TEB32.
    unsafe {
        core::arch::asm!("mov {}, fs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    v
}

/// Writes the pointer-sized field of the current TEB32 at `offset`.
pub fn teb_write(offset: usize, value: usize) {
    // SAFETY: fs:[offset] is inside the current thread's TEB32, a field
    // the caller names.
    unsafe { core::arch::asm!("mov fs:[{}], {}", in(reg) offset, in(reg) value, options(nostack)) };
}

/// Writes the 32-bit field of the current TEB32 at `offset`.
pub fn teb_write_u32(offset: usize, value: u32) {
    teb_write(offset, value as usize);
}

/// The time stamp counter.
pub fn rdtsc() -> u64 {
    // SAFETY: rdtsc only reads the time stamp counter.
    unsafe { core::arch::x86::_rdtsc() }
}

/// The time stamp counter and IA32_TSC_AUX (rdtscp).
pub fn rdtscp(aux: &mut u32) -> u64 {
    // SAFETY: rdtscp reads the time stamp counter and TSC_AUX.
    unsafe { core::arch::x86::__rdtscp(aux) }
}

pub use core::arch::x86::{__cpuid as cpuid, __cpuid_count as cpuid_count};

/// CONTEXT (x86 layout, 0x2cc bytes).
#[repr(C)]
pub struct Context {
    context_flags: u32,
    dr: [u32; 6],
    float_save: [u8; 112],
    /// SegGs, SegFs, SegEs, SegDs.
    segs: [u32; 4],
    /// Edi, Esi, Ebx, Edx, Ecx, Eax.
    gpr: [u32; 6],
    ebp: u32,
    eip: u32,
    seg_cs: u32,
    eflags: u32,
    esp: u32,
    seg_ss: u32,
    extended_registers: [u8; 512],
}

/// The CONTEXT a new thread starts with: at `entry(args[0], args[1])`
/// (stdcall) with its stack ending at `top`: the arguments and a null
/// return address on it, written here (the stack's top is committed).
pub fn thread_context(entry: usize, args: [usize; 2], top: usize) -> Context {
    let esp = top - 16;
    // SAFETY: the new thread's committed stack, not yet in use.
    unsafe { (esp as *mut [usize; 3]).write([0, args[0], args[1]]) };
    // SAFETY: an all-zero CONTEXT is a valid bit pattern.
    let mut context: Context = unsafe { core::mem::zeroed() };
    context.context_flags = 0x1_0007; // CONTEXT_FULL (i386)
    context.segs = [0x2b, 0x53, 0x2b, 0x2b];
    context.seg_cs = 0x23;
    context.seg_ss = 0x2b;
    context.eflags = 0x200;
    context.eip = entry as u32;
    // The start routine in eax, where NtCreateThread takes the thread's
    // Win32 start address (ThreadQuerySetWin32StartAddress).
    context.gpr[5] = args[0] as u32; // eax
    context.esp = esp as u32;
    context
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER (x86, 8 bytes): the first entry in the low doubleword, the
// depth (16 bits) and a sequence number (16 bits) in the high one; all
// change together with cmpxchg8b (a 64-bit compare-exchange), so a pop
// cannot be fooled by an entry popped and pushed again meanwhile.

#[repr(C, align(8))]
pub struct SListHeader {
    value: u64,
}

#[repr(C)]
pub struct SListEntry {
    pub next: *mut SListEntry,
}

/// A header's value.
pub type SListValue = u64;

/// An empty header's value.
pub const SLIST_EMPTY: SListValue = 0;

/// Compares the header with `old` and, if equal, stores `new`; returns
/// the value seen.
pub fn slist_cas(head: *mut SListHeader, old: SListValue, new: SListValue) -> SListValue {
    // SAFETY: the caller's 8-byte aligned SLIST_HEADER, only changed
    // atomically.
    let header = unsafe { core::sync::atomic::AtomicU64::from_ptr(head.cast()) };
    use core::sync::atomic::Ordering;
    match header.compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(seen) | Err(seen) => seen,
    }
}

/// The header's value, read atomically (a compare-exchange that fails,
/// or rewrites the same value).
pub fn slist_read(head: *mut SListHeader) -> SListValue {
    slist_cas(head, 0, 0)
}

pub fn slist_first(v: SListValue) -> *mut SListEntry {
    v as u32 as *mut SListEntry
}

pub fn slist_depth(v: SListValue) -> u16 {
    (v >> 32) as u16
}

/// The value with `first` on top, `depth` entries and the next sequence
/// number.
pub fn slist_value(old: SListValue, first: *mut SListEntry, depth: u16) -> SListValue {
    let sequence = ((old >> 48) as u16).wrapping_add(1);
    u64::from(first as u32) | u64::from(depth) << 32 | u64::from(sequence) << 48
}

pub fn slist_init(head: *mut SListHeader) {
    // SAFETY: the caller's aligned header.
    unsafe { head.write(SListHeader { value: SLIST_EMPTY }) };
}

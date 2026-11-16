//! The ARM64 parts of kernel32 (ROADMAP AR); code outside the
//! per-architecture modules that assumes x86 fails the ARM64 check (CI's
//! Clippy step, `--target aarch64-pc-windows-msvc`). The names are
//! `x64.rs`'s.
//!
//! What the public documents say plainly is filled in: the TEB through
//! x18 (Microsoft's "Overview of ARM64 ABI conventions": in user mode x18
//! points to the TEB), the 64-bit layouts (`win64.rs`), ARM64's CONTEXT
//! (winnt.h), PROCESSOR_ARCHITECTURE_ARM64, and what is the processor's
//! own (the virtual counter, an exclusive pair for SLIST_HEADER), the
//! processor number from the kernel (NtGetCurrentProcessorNumber).

pub use super::win64::*;

/// CONTEXT (ARM64, 0x390 bytes, winnt.h): ContextFlags first;
/// CONTEXT_ARM64; CONTEXT_ARM64_XSTATE; no FXSAVE area and no XState
/// feature in the CONTEXT itself (its floating-point part is
/// CONTEXT_FLOATING_POINT's V0-V31, Fpcr, Fpsr); the byte ranges of each
/// part flag.
pub const CONTEXT_SIZE: usize = 0x390;
pub const CONTEXT_FLAGS_AT: usize = 0;
pub const CONTEXT_ARCH: u32 = 0x40_0000;
pub const CONTEXT_LEGACY_STATE: u32 = 0x40_0004; // CONTEXT_FLOATING_POINT
pub const CONTEXT_XSTATE: u32 = 0x40_0020;
pub const CONTEXT_FXSAVE: Option<usize> = None;
pub const CONTEXT_LEGACY_FEATURES: u64 = 0;
pub const CONTEXT_PARTS: &[(u32, &[(usize, usize)])] = &[
    // CONTEXT_CONTROL: Cpsr; Fp, Lr, Sp, Pc.
    (0x1, &[(0x4, 0x8), (0xf0, 0x110)]),
    // CONTEXT_INTEGER: X0-X28.
    (0x2, &[(0x8, 0xf0)]),
    // CONTEXT_FLOATING_POINT: V0-V31, Fpcr, Fpsr.
    (0x4, &[(0x110, 0x318)]),
    // CONTEXT_DEBUG_REGISTERS: Bcr, Bvr, Wcr, Wvr.
    (0x8, &[(0x318, 0x390)]),
];

/// PROCESSOR_ARCHITECTURE_ARM64: what GetSystemInfo reports. The
/// processor type GetSystemInfo gives on ARM64 is to be recorded on ARM64
/// Windows with the port; 0 until then.
pub const PROCESSOR_ARCHITECTURE: u16 = 12;
pub const PROCESSOR_TYPE: u32 = 0;
/// What GetNativeSystemInfo reports: the same.
pub const NATIVE_PROCESSOR_ARCHITECTURE: u16 = PROCESSOR_ARCHITECTURE;
pub const NATIVE_PROCESSOR_TYPE: u32 = PROCESSOR_TYPE;

/// The native system directory: System32.
pub const NATIVE_SYSTEM_DIRECTORY: &[u8] = b"\\system32";

/// Reads the pointer-sized field of the current TEB at `offset`.
pub fn teb_read(offset: usize) -> usize {
    let v: usize;
    // SAFETY: x18 holds the TEB; [x18 + offset] is a field of it.
    unsafe {
        core::arch::asm!("ldr {}, [x18, {}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

/// Writes the pointer-sized field of the current TEB at `offset`.
pub fn teb_write(offset: usize, value: usize) {
    // SAFETY: as teb_read, a field the caller names.
    unsafe {
        core::arch::asm!("str {}, [x18, {}]", in(reg) value, in(reg) offset, options(nostack, preserves_flags))
    };
}

/// Writes the 32-bit field of the current TEB at `offset`.
pub fn teb_write_u32(offset: usize, value: u32) {
    // SAFETY: as teb_write.
    unsafe {
        core::arch::asm!("str {:w}, [x18, {}]", in(reg) value, in(reg) offset, options(nostack, preserves_flags))
    };
}

/// A free-running counter: the virtual counter (CNTVCT_EL0).
pub fn cycles() -> u64 {
    let v: u64;
    // SAFETY: reads the virtual counter; no memory, no flags.
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

/// Makes code written to [base, base + size) visible to instruction
/// fetch on every processor (Arm's "Concurrent modification and execution
/// of instructions", the cache maintenance sequence): each data cache line
/// cleaned to the point of unification, then each instruction cache line
/// invalidated, with the barriers between; the line sizes from CTR_EL0
/// (DminLine, IminLine), whose IDC and DIC bits say a step is not needed.
pub fn sync_code(base: usize, size: usize) {
    let ctr: u64;
    // SAFETY: reads CTR_EL0 (the kernel lets user mode: SCTLR_EL1.UCT).
    unsafe {
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags))
    };
    let dline = 4usize << ((ctr >> 16) & 0xf);
    let iline = 4usize << (ctr & 0xf);
    let end = base + size;
    if ctr & (1 << 28) == 0 {
        let mut at = base & !(dline - 1);
        while at < end {
            // SAFETY: cleans a line of this process's code (readable).
            unsafe {
                core::arch::asm!("dc cvau, {}", in(reg) at, options(nostack, preserves_flags))
            };
            at += dline;
        }
    }
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    if ctr & (1 << 29) == 0 {
        let mut at = base & !(iline - 1);
        while at < end {
            // SAFETY: invalidates a line of this process's code.
            unsafe {
                core::arch::asm!("ic ivau, {}", in(reg) at, options(nostack, preserves_flags))
            };
            at += iline;
        }
    }
    // SAFETY: barriers: the invalidation completes, this processor
    // refetches.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtGetCurrentProcessorNumber() -> u32;
}

/// The processor running this thread, as the kernel tells it
/// (NtGetCurrentProcessorNumber: ARM64 has no user-readable equivalent of
/// x64's TSC_AUX).
pub fn current_processor() -> u32 {
    // SAFETY: no arguments; returns the number.
    unsafe { NtGetCurrentProcessorNumber() }
}

/// CONTEXT (ARM64 layout, winnt.h's ARM64_NT_CONTEXT: 0x390 bytes,
/// 16-byte aligned).
#[repr(C, align(16))]
pub struct Context {
    context_flags: u32,
    cpsr: u32,
    /// X0..X28, Fp (x29), Lr (x30).
    x: [u64; 31],
    sp: u64,
    pc: u64,
    v: [u128; 32],
    fpcr: u32,
    fpsr: u32,
    bcr: [u32; 8],
    bvr: [u64; 8],
    wcr: [u32; 2],
    wvr: [u64; 2],
}

const _: () = assert!(size_of::<Context>() == 0x390);

/// CONTEXT_ARM64 | CONTEXT_CONTROL | CONTEXT_INTEGER |
/// CONTEXT_FLOATING_POINT.
const CONTEXT_FULL: u32 = 0x40_0007;

/// The CONTEXT a new thread starts with: at `entry(args[0], args[1])`
/// (x0, x1) with its stack ending at `top` (16-byte aligned), no return
/// address (lr 0). NtCreateThread takes the Win32 start address
/// (ThreadQuerySetWin32StartAddress) from x0, the start routine here, as
/// x64's from rax.
pub fn thread_context(entry: usize, args: [usize; 2], top: usize) -> Context {
    // SAFETY: an all-zero CONTEXT is a valid bit pattern.
    let mut context: Context = unsafe { core::mem::zeroed() };
    context.context_flags = CONTEXT_FULL;
    context.pc = entry as u64;
    context.x[0] = args[0] as u64;
    context.x[1] = args[1] as u64;
    context.sp = (top & !0xf) as u64;
    context
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER (16 bytes): x64's encoding (depth and sequence in the low
// quadword, the first entry and HeaderType 1 in the high one), changed
// together by an exclusive pair (ldaxp/stlxp). Whether ARM64 Windows
// encodes its header bits the same is to be checked with the port.

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
    let (low, high): (u64, u64);
    // SAFETY: a 16-byte aligned SLIST_HEADER. The pair is loaded
    // exclusively; if it equals `old`, `new` is stored, else the value
    // seen is stored back (so the read is single-copy atomic as well);
    // either store failing (another writer) retries.
    unsafe {
        core::arch::asm!(
            "2:",
            "ldaxp {low}, {high}, [{head}]",
            "cmp {low}, {old_low}",
            "ccmp {high}, {old_high}, #0, eq",
            "b.ne 3f",
            "stlxp {status:w}, {new_low}, {new_high}, [{head}]",
            "cbnz {status:w}, 2b",
            "b 4f",
            "3:",
            "stlxp {status:w}, {low}, {high}, [{head}]",
            "cbnz {status:w}, 2b",
            "4:",
            head = in(reg) head,
            old_low = in(reg) old.0,
            old_high = in(reg) old.1,
            new_low = in(reg) new.0,
            new_high = in(reg) new.1,
            low = out(reg) low,
            high = out(reg) high,
            status = out(reg) _,
            options(nostack),
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

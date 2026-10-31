//! CONTEXT records in caller buffers (InitializeContext, CopyContext) and
//! the XState functions, as documented for x64. gala saves FXSAVE state
//! only (no XSAVE: CR4.OSXSAVE is clear, so CPUID shows no OS support for
//! AVX), so the enabled features are x87 and SSE and no context has an
//! extended (CONTEXT_XSTATE) area.

use super::{Bool, FALSE, TRUE, fail_with, unsupported};

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NOT_SUPPORTED: u32 = 50;

/// sizeof(CONTEXT) on x64; it must be 16-byte aligned.
const CONTEXT_SIZE: usize = 0x4d0;
const CONTEXT_AMD64: u32 = 0x10_0000;
const CONTEXT_XSTATE: u32 = 0x40;

/// XSTATE_MASK_LEGACY_FLOATING_POINT | XSTATE_MASK_LEGACY_SSE.
#[unsafe(no_mangle)]
pub extern "system" fn GetEnabledXStateFeatures() -> u64 {
    3
}

/// InitializeContext(Buffer, ContextFlags, *Context, *ContextLength): an
/// aligned CONTEXT inside the buffer with its flags set; with too small a
/// buffer, the size needed.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeContext(
    buf: *mut u8,
    flags: u32,
    context: *mut *mut u8,
    len: *mut u32,
) -> Bool {
    if len.is_null() || flags & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if flags & CONTEXT_XSTATE != 0 {
        unsupported(b"kernel32.dll!InitializeContext(CONTEXT_XSTATE)", flags.into());
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    let need = CONTEXT_SIZE + 15;
    // SAFETY: the caller's length, buffer and output.
    unsafe {
        if buf.is_null() || (*len as usize) < need {
            *len = need as u32;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        let ctx = ((buf as usize + 15) & !15) as *mut u8;
        core::ptr::write_bytes(ctx, 0, CONTEXT_SIZE);
        *(ctx.add(0x30) as *mut u32) = flags;
        *context = ctx;
    }
    TRUE
}

/// The byte ranges of a CONTEXT each flag covers (x64 CONTEXT layout).
const PARTS: &[(u32, &[(usize, usize)])] = &[
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

/// CopyContext(Destination, ContextFlags, Source): the parts `flags`
/// names that both contexts have.
#[unsafe(no_mangle)]
pub extern "system" fn CopyContext(dst: *mut u8, flags: u32, src: *const u8) -> Bool {
    if dst.is_null() || src.is_null() || flags & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: two CONTEXT records of the caller's.
    unsafe {
        let dst_flags = *(dst.add(0x30) as *const u32);
        let src_flags = *(src.add(0x30) as *const u32);
        let parts = flags & dst_flags & src_flags & 0x1f;
        for &(bit, ranges) in PARTS {
            if parts & bit != 0 {
                for &(a, b) in ranges {
                    core::ptr::copy_nonoverlapping(src.add(a), dst.add(a), b - a);
                }
            }
        }
    }
    TRUE
}

/// LocateXStateFeature: the legacy features live in FltSave (x87: its
/// first 160 bytes, SSE: the XMM registers at 160); there are no others.
#[unsafe(no_mangle)]
pub extern "system" fn LocateXStateFeature(
    context: *mut u8,
    feature: u32,
    len: *mut u32,
) -> *mut u8 {
    let (offset, size) = match feature {
        0 => (0x100, 160),
        1 => (0x100 + 160, 256),
        _ => return core::ptr::null_mut(),
    };
    if !len.is_null() {
        // SAFETY: the caller's output.
        unsafe { *len = size };
    }
    context.wrapping_add(offset)
}

/// Set/GetXStateFeaturesMask: only for contexts with an XSTATE area,
/// which gala never makes.
#[unsafe(no_mangle)]
pub extern "system" fn SetXStateFeaturesMask(_context: *mut u8, _mask: u64) -> Bool {
    fail_with(ERROR_INVALID_PARAMETER, FALSE)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetXStateFeaturesMask(_context: *mut u8, mask: *mut u64) -> Bool {
    if !mask.is_null() {
        // SAFETY: the caller's output.
        unsafe { *mask = 0 };
    }
    fail_with(ERROR_INVALID_PARAMETER, FALSE)
}

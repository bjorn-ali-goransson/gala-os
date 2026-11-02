//! CONTEXT records in caller buffers (InitializeContext, InitializeContext2,
//! CopyContext) and the XState functions, as documented for x64. The
//! features come from KUSER_SHARED_DATA's XState configuration (the
//! kernel enables x87, SSE, AVX and, where the CPU has it, AVX-512). A
//! context with CONTEXT_XSTATE is the CONTEXT, its CONTEXT_EX right after
//! it (chunks All, Legacy, XState: offsets from the CONTEXT_EX) and an
//! XSTATE area (an XSAVE header, whose Mask names the features present,
//! then the extended features at their standard offsets less 512: x87
//! and SSE are in CONTEXT.FltSave). The standard (non-compacted) format
//! only: the configuration says CompactionEnabled = 0.

use super::{Bool, FALSE, TRUE, fail_with, unsupported};

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NOT_SUPPORTED: u32 = 50;

/// sizeof(CONTEXT) on x64; it must be 16-byte aligned.
const CONTEXT_SIZE: usize = 0x4d0;
/// sizeof(CONTEXT_EX).
const CONTEXT_EX_SIZE: usize = 0x18;
const CONTEXT_AMD64: u32 = 0x10_0000;
const CONTEXT_FLOATING_POINT: u32 = 0x10_0008;
const CONTEXT_XSTATE: u32 = 0x10_0040;
const LEGACY_FEATURES: u64 = 3;
/// The XSAVE header at the start of the XSTATE area.
const XSAVE_HEADER: usize = 64;
const LEGACY_SIZE: u32 = 512;

/// KUSER_SHARED_DATA's XSTATE_CONFIGURATION (at 0x7ffe0000 + 0x3d8):
/// EnabledFeatures at 0, Size at 0x10, Features[64] (offset, size) at 0x18.
const XSTATE_CONFIGURATION: usize = 0x7ffe_03d8;

fn config_u64(offset: usize) -> u64 {
    // SAFETY: KUSER_SHARED_DATA is mapped read-only in every process.
    unsafe { ((XSTATE_CONFIGURATION + offset) as *const u64).read_volatile() }
}

fn config_u32(offset: usize) -> u32 {
    // SAFETY: as above.
    unsafe { ((XSTATE_CONFIGURATION + offset) as *const u32).read_volatile() }
}

/// The extended features (beyond x87 and SSE) the system enables.
fn extended() -> u64 {
    GetEnabledXStateFeatures() & !LEGACY_FEATURES
}

/// Feature `id`'s (offset, size) in the standard XSAVE layout.
fn feature(id: u32) -> (u32, u32) {
    let at = 0x18 + 8 * id as usize;
    (config_u32(at), config_u32(at + 4))
}

/// The XSTATE area's length: the header and every enabled extended
/// feature (the configuration's Size less the legacy region).
fn xstate_len() -> usize {
    config_u32(0x10).saturating_sub(LEGACY_SIZE) as usize
}

fn flags_of(ctx: *const u8) -> u32 {
    // SAFETY: the caller's CONTEXT; ContextFlags at 0x30.
    unsafe { (ctx.add(0x30) as *const u32).read_unaligned() }
}

fn has(flags: u32, part: u32) -> bool {
    flags & part == part
}

/// The XSTATE area of a context with CONTEXT_XSTATE, from its CONTEXT_EX:
/// (address, length). None if the CONTEXT_EX does not describe one.
fn xstate_area(ctx: *mut u8) -> Option<(*mut u8, usize)> {
    if !has(flags_of(ctx), CONTEXT_XSTATE) {
        return None;
    }
    let ex = ctx.wrapping_add(CONTEXT_SIZE);
    // SAFETY: a context with CONTEXT_XSTATE has its CONTEXT_EX after it.
    let (legacy, offset, len) = unsafe {
        (
            (ex.add(8) as *const i32).read_unaligned(),
            (ex.add(16) as *const i32).read_unaligned(),
            (ex.add(20) as *const u32).read_unaligned() as usize,
        )
    };
    if legacy != -(CONTEXT_SIZE as i32) || offset < CONTEXT_EX_SIZE as i32 || len < XSAVE_HEADER {
        return None;
    }
    Some((ex.wrapping_add(offset as usize), len))
}

fn header_mask(area: *const u8) -> u64 {
    // SAFETY: the XSAVE header's Mask, at the area's start.
    unsafe { (area as *const u64).read_unaligned() }
}

fn set_header_mask(area: *mut u8, mask: u64) {
    // SAFETY: as above.
    unsafe { (area as *mut u64).write_unaligned(mask) }
}

/// The enabled features (KUSER_SHARED_DATA's XState.EnabledFeatures).
#[unsafe(no_mangle)]
pub extern "system" fn GetEnabledXStateFeatures() -> u64 {
    config_u64(0) | LEGACY_FEATURES
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
    InitializeContext2(buf, flags, context, len, 0)
}

/// InitializeContext2(Buffer, ContextFlags, *Context, *ContextLength,
/// XStateCompactionMask): as InitializeContext; with CONTEXT_XSTATE the
/// CONTEXT_EX and an XSTATE area for every enabled feature follow the
/// CONTEXT (64-byte aligned, Mask 0). The compaction mask only matters
/// where the system uses the compacted format, which gala does not.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeContext2(
    buf: *mut u8,
    flags: u32,
    context: *mut *mut u8,
    len: *mut u32,
    _compaction_mask: u64,
) -> Bool {
    if len.is_null() || flags & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let xstate = has(flags, CONTEXT_XSTATE);
    if xstate && extended() == 0 {
        unsupported(b"kernel32.dll!InitializeContext(CONTEXT_XSTATE)", flags.into());
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    let xlen = if xstate { xstate_len() } else { 0 };
    let need = if xstate {
        // CONTEXT (16-aligned), CONTEXT_EX, the area 64-aligned.
        CONTEXT_SIZE + 15 + CONTEXT_EX_SIZE + 63 + xlen
    } else {
        CONTEXT_SIZE + 15
    };
    // SAFETY: the caller's length, buffer and output.
    unsafe {
        if buf.is_null() || (*len as usize) < need {
            *len = need as u32;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        let ctx = ((buf as usize + 15) & !15) as *mut u8;
        core::ptr::write_bytes(ctx, 0, CONTEXT_SIZE);
        *(ctx.add(0x30) as *mut u32) = flags;
        if xstate {
            let ex = ctx.add(CONTEXT_SIZE);
            let area = ((ex as usize + CONTEXT_EX_SIZE + 63) & !63) as *mut u8;
            let area_at = area as usize - ctx as usize;
            let chunks: [i32; 6] = [
                -(CONTEXT_SIZE as i32),
                (area_at + xlen) as i32,
                -(CONTEXT_SIZE as i32),
                CONTEXT_SIZE as i32,
                (area_at - CONTEXT_SIZE) as i32,
                xlen as i32,
            ];
            for (i, v) in chunks.iter().enumerate() {
                (ex.add(4 * i) as *mut i32).write_unaligned(*v);
            }
            core::ptr::write_bytes(area, 0, xlen);
        }
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
/// names that both contexts have; with CONTEXT_XSTATE, the extended
/// features present in the source (its Mask), as far as the destination's
/// area holds them.
#[unsafe(no_mangle)]
pub extern "system" fn CopyContext(dst: *mut u8, flags: u32, src: *const u8) -> Bool {
    if dst.is_null() || src.is_null() || flags & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let dst_flags = flags_of(dst);
    let src_flags = flags_of(src);
    let parts = flags & dst_flags & src_flags & 0x1f;
    // SAFETY: two CONTEXT records of the caller's.
    unsafe {
        for &(bit, ranges) in PARTS {
            if parts & bit != 0 {
                for &(a, b) in ranges {
                    core::ptr::copy_nonoverlapping(src.add(a), dst.add(a), b - a);
                }
            }
        }
    }
    if has(flags, CONTEXT_XSTATE)
        && has(dst_flags, CONTEXT_XSTATE)
        && has(src_flags, CONTEXT_XSTATE)
    {
        let (Some((to, to_len)), Some((from, from_len))) =
            (xstate_area(dst), xstate_area(src as *mut u8))
        else {
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        };
        let mask = header_mask(from) & extended();
        let mut copied = 0;
        for id in 2..64u32 {
            if mask & (1 << id) == 0 {
                continue;
            }
            let (offset, size) = feature(id);
            let at = (offset - LEGACY_SIZE) as usize;
            let end = at + size as usize;
            if end <= to_len && end <= from_len {
                // SAFETY: inside both areas, as their CONTEXT_EX say.
                unsafe { core::ptr::copy_nonoverlapping(from.add(at), to.add(at), size as usize) };
                copied |= 1 << id;
            }
        }
        set_header_mask(to, copied);
    }
    TRUE
}

/// LocateXStateFeature: x87 and SSE live in FltSave (x87: its first 160
/// bytes, SSE: the XMM registers at 160); the others in the XSTATE area
/// of a context with CONTEXT_XSTATE, if enabled and inside the area.
#[unsafe(no_mangle)]
pub extern "system" fn LocateXStateFeature(
    context: *mut u8,
    feature_id: u32,
    len: *mut u32,
) -> *mut u8 {
    if context.is_null() {
        return core::ptr::null_mut();
    }
    let (at, size) = match feature_id {
        0 => (context.wrapping_add(0x100), 160),
        1 => (context.wrapping_add(0x100 + 160), 256),
        2..64 => {
            if extended() & (1 << feature_id) == 0 {
                return core::ptr::null_mut();
            }
            let Some((area, area_len)) = xstate_area(context) else {
                return core::ptr::null_mut();
            };
            let (offset, size) = feature(feature_id);
            let at = (offset - LEGACY_SIZE) as usize;
            if at + size as usize > area_len {
                return core::ptr::null_mut();
            }
            (area.wrapping_add(at), size)
        }
        _ => return core::ptr::null_mut(),
    };
    if !len.is_null() {
        // SAFETY: the caller's output.
        unsafe { *len = size };
    }
    at
}

/// SetXStateFeaturesMask: which features the context holds (and, for
/// SetThreadContext, sets: the others return to their initial state).
/// x87/SSE are CONTEXT_FLOATING_POINT; the others need CONTEXT_XSTATE.
#[unsafe(no_mangle)]
pub extern "system" fn SetXStateFeaturesMask(context: *mut u8, mask: u64) -> Bool {
    if context.is_null() || flags_of(context) & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if mask & LEGACY_FEATURES != 0 {
        // SAFETY: the caller's CONTEXT.
        unsafe {
            (context.add(0x30) as *mut u32)
                .write_unaligned(flags_of(context) | CONTEXT_FLOATING_POINT)
        };
    }
    let Some((area, _)) = xstate_area(context) else {
        return if mask & !LEGACY_FEATURES == 0 {
            TRUE
        } else {
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        };
    };
    set_header_mask(area, mask & extended());
    TRUE
}

/// GetXStateFeaturesMask: the features the context holds (x87/SSE with
/// CONTEXT_FLOATING_POINT; the others from the XSTATE area's Mask).
#[unsafe(no_mangle)]
pub extern "system" fn GetXStateFeaturesMask(context: *mut u8, mask: *mut u64) -> Bool {
    if context.is_null() || mask.is_null() || flags_of(context) & CONTEXT_AMD64 == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let flags = flags_of(context);
    let mut m = if has(flags, CONTEXT_FLOATING_POINT) { LEGACY_FEATURES } else { 0 };
    if let Some((area, _)) = xstate_area(context) {
        m |= header_mask(area) & extended();
    }
    // SAFETY: the caller's output.
    unsafe { *mask = m };
    TRUE
}

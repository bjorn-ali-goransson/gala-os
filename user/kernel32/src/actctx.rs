//! Activation contexts (CreateActCtxW and friends), as documented: a
//! context comes from a manifest (an RT_MANIFEST resource of a module or
//! PE file, or a .manifest file) and is activated on a per-thread stack.
//! gala reads nothing from the manifest yet (no side-by-side DLL
//! redirection, no COM class registration from manifests); the contexts
//! exist so that programs that create and activate them (.NET's apphost,
//! comctl32 users) work.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use super::{Bool, FALSE, Handle, TRUE, fail_with};

const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
const ERROR_INVALID_PARAMETER: u32 = 87;

const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x8;
const ACTCTX_FLAG_HMODULE_VALID: u32 = 0x80;

/// ACTCTXW.
#[repr(C)]
pub struct ActCtx {
    size: u32,
    flags: u32,
    source: *const u16,
    architecture: u16,
    language: u16,
    assembly_directory: *const u16,
    resource_name: *const u16,
    application_name: *const u16,
    module: Handle,
}

/// A context: its reference count (the handle is its address).
struct Context {
    refs: AtomicU32,
}

fn new_context() -> Handle {
    let p =
        super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, core::mem::size_of::<Context>())
            .cast::<Context>();
    if p.is_null() {
        return fail_with(8, INVALID_HANDLE_VALUE); // ERROR_NOT_ENOUGH_MEMORY
    }
    // SAFETY: fresh memory of the context's size.
    unsafe { p.write(Context { refs: AtomicU32::new(1) }) };
    p.cast()
}

/// CreateActCtxW: the manifest must exist - resource `resource_name`
/// (default 1) of type RT_MANIFEST in the module (or the PE file
/// `source`), or the manifest file `source` itself.
#[unsafe(no_mangle)]
pub extern "system" fn CreateActCtxW(ctx: *const ActCtx) -> Handle {
    if ctx.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    // SAFETY: the caller's ACTCTXW (cbSize checked first).
    let c = unsafe { &*ctx };
    // At least cbSize, dwFlags and lpSource.
    if (c.size as usize) < core::mem::offset_of!(ActCtx, architecture) {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let name = if c.flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0 {
        c.resource_name
    } else {
        core::ptr::without_provenance(1)
    };
    let manifest_type = core::ptr::without_provenance(24); // RT_MANIFEST
    let found = if c.flags & ACTCTX_FLAG_HMODULE_VALID != 0 {
        !super::resource::FindResourceExW(c.module, manifest_type, name, 0).is_null()
    } else if c.source.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    } else {
        // A PE file's resource, or a manifest file.
        match super::mapping::map_image_file(c.source) {
            Ok(base) => {
                let h = (base | 2) as Handle;
                let found = !super::resource::FindResourceExW(h, manifest_type, name, 0).is_null();
                super::mapping::unmap_image_file(base);
                found
            }
            Err(e) => {
                if super::GetFileAttributesW(c.source) == u32::MAX {
                    return fail_with(e, INVALID_HANDLE_VALUE);
                }
                true // a .manifest file
            }
        }
    };
    if !found {
        return INVALID_HANDLE_VALUE; // FindResourceExW set the error
    }
    new_context()
}

#[unsafe(no_mangle)]
pub extern "system" fn AddRefActCtx(ctx: Handle) {
    if !ctx.is_null() && ctx != INVALID_HANDLE_VALUE {
        // SAFETY: a context of ours.
        unsafe { (*ctx.cast::<Context>()).refs.fetch_add(1, Ordering::Relaxed) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseActCtx(ctx: Handle) {
    if ctx.is_null() || ctx == INVALID_HANDLE_VALUE {
        return;
    }
    // SAFETY: a context of ours, freed with its last reference.
    unsafe {
        if (*ctx.cast::<Context>()).refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            super::heap::HeapFree(super::heap::GetProcessHeap(), 0, ctx);
        }
    }
}

/// The activation stacks: per thread (by id), up to 16 contexts with the
/// cookie of each activation.
const THREADS: usize = 64;
const DEPTH: usize = 16;

/// One thread's stack: its id, depth and (context, cookie) entries.
type Stack = (u32, usize, [(usize, usize); DEPTH]);

struct Stacks {
    lock: AtomicBool,
    stacks: core::cell::UnsafeCell<[Stack; THREADS]>,
}

// SAFETY: `stacks` is only touched with `lock` held.
unsafe impl Sync for Stacks {}

static STACKS: Stacks = Stacks {
    lock: AtomicBool::new(false),
    stacks: core::cell::UnsafeCell::new([(0, 0, [(0, 0); DEPTH]); THREADS]),
};
static NEXT_COOKIE: AtomicUsize = AtomicUsize::new(1);

/// `f` on this thread's stack: (depth, entries).
fn with_stack<R>(f: impl FnOnce(&mut usize, &mut [(usize, usize); DEPTH]) -> R) -> Option<R> {
    let me = super::GetCurrentThreadId();
    while STACKS
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let stacks = unsafe { &mut *STACKS.stacks.get() };
    let slot = stacks
        .iter()
        .position(|s| s.0 == me)
        .or_else(|| stacks.iter().position(|s| s.0 == 0 || s.1 == 0));
    let r = slot.map(|i| {
        let s = &mut stacks[i];
        s.0 = me;
        f(&mut s.1, &mut s.2)
    });
    STACKS.lock.store(false, Ordering::Release);
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn ActivateActCtx(ctx: Handle, cookie: *mut usize) -> Bool {
    if cookie.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let value = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
    let pushed = with_stack(|depth, entries| {
        if *depth == DEPTH {
            return false;
        }
        entries[*depth] = (ctx as usize, value);
        *depth += 1;
        true
    });
    if pushed != Some(true) {
        super::unsupported(b"kernel32.dll!ActivateActCtx(stack full)", DEPTH as u64);
        return fail_with(8, FALSE);
    }
    AddRefActCtx(ctx);
    // SAFETY: the caller's output.
    unsafe { *cookie = value };
    TRUE
}

/// DeactivateActCtx: pops the activation `cookie` (and any above it).
#[unsafe(no_mangle)]
pub extern "system" fn DeactivateActCtx(_flags: u32, cookie: usize) -> Bool {
    let mut released = [0usize; DEPTH];
    let popped = with_stack(|depth, entries| {
        let Some(at) = entries[..*depth].iter().position(|e| e.1 == cookie) else { return 0 };
        let n = *depth - at;
        for (i, e) in entries[at..*depth].iter().enumerate() {
            released[i] = e.0;
        }
        *depth = at;
        n
    })
    .unwrap_or(0);
    if popped == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    for &ctx in &released[..popped] {
        ReleaseActCtx(ctx as Handle);
    }
    TRUE
}

/// GetCurrentActCtx: the innermost active context, referenced (null if
/// none is active).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentActCtx(out: *mut Handle) -> Bool {
    if out.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let ctx = with_stack(|depth, entries| if *depth == 0 { 0 } else { entries[*depth - 1].0 })
        .unwrap_or(0);
    AddRefActCtx(ctx as Handle);
    // SAFETY: the caller's output.
    unsafe { *out = ctx as *mut c_void };
    TRUE
}

const QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX: u32 = 0x4;
const QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE: u32 = 0x8;
const QUERY_ACTCTX_FLAG_ACTCTX_IS_ADDRESS: u32 = 0x10;
const QUERY_ACTCTX_FLAG_NO_ADDREF: u32 = 0x8000_0000;

/// ACTIVATION_CONTEXT_BASIC_INFORMATION.
#[repr(C)]
struct BasicInformation {
    actctx: Handle,
    flags: u32,
}

/// QueryActCtxW(flags, actctx, subinstance, class, buffer, size, &written):
/// ActivationContextBasicInformation (1) of the context given, or with
/// QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX of the active one (`actctx` must
/// then be NULL): the handle (referenced unless
/// QUERY_ACTCTX_FLAG_NO_ADDREF) and flags 0. The other classes describe a
/// manifest's contents, which gala's contexts do not keep: recorded,
/// ERROR_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn QueryActCtxW(
    flags: u32,
    actctx: Handle,
    _subinstance: *mut c_void,
    class: u32,
    buffer: *mut c_void,
    size: usize,
    written: *mut usize,
) -> Bool {
    if !written.is_null() {
        // SAFETY: the caller's SIZE_T.
        unsafe { *written = 0 };
    }
    let known = QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX
        | QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE
        | QUERY_ACTCTX_FLAG_ACTCTX_IS_ADDRESS
        | QUERY_ACTCTX_FLAG_NO_ADDREF;
    if flags & !known != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let ctx = if flags & QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX != 0 {
        if !actctx.is_null() {
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
        with_stack(|depth, entries| if *depth == 0 { 0 } else { entries[*depth - 1].0 })
            .unwrap_or(0)
    } else if flags & (QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE | QUERY_ACTCTX_FLAG_ACTCTX_IS_ADDRESS)
        != 0
    {
        // A module's own manifest context: gala keeps none per module.
        super::unsupported(b"kernel32.dll!QueryActCtxW(module context)", flags.into());
        0
    } else {
        actctx as usize
    };
    match class {
        1 => {
            let need = core::mem::size_of::<BasicInformation>();
            if !written.is_null() {
                // SAFETY: the caller's SIZE_T.
                unsafe { *written = need };
            }
            if buffer.is_null() || size < need {
                return fail_with(122, FALSE); // ERROR_INSUFFICIENT_BUFFER
            }
            if flags & QUERY_ACTCTX_FLAG_NO_ADDREF == 0 {
                AddRefActCtx(ctx as Handle);
            }
            // SAFETY: the caller's buffer of `size` >= need bytes.
            unsafe {
                buffer
                    .cast::<BasicInformation>()
                    .write_unaligned(BasicInformation { actctx: ctx as Handle, flags: 0 })
            };
            TRUE
        }
        _ => {
            super::unsupported(b"kernel32.dll!QueryActCtxW(class)", class.into());
            fail_with(ERROR_INVALID_PARAMETER, FALSE)
        }
    }
}

/// ACTCTX_SECTION_KEYED_DATA, up to ulAssemblyRosterIndex (the smallest
/// cbSize accepted is the offset after it).
#[repr(C)]
struct KeyedData {
    size: u32,
    version: u32,
    data: usize,
    length: u32,
    section_global: usize,
    section_global_length: u32,
    section_base: usize,
    section_total_length: u32,
    actctx: Handle,
    assembly_roster_index: u32,
}

const ERROR_SXS_SECTION_NOT_FOUND: u32 = 14004;
const ERROR_SXS_KEY_NOT_FOUND: u32 = 14007;

/// FindActCtxSectionStringW(flags, guid, section, name, &data): looks
/// `name` up in a section (DLL redirection, window classes, ...) of the
/// active contexts. gala's contexts keep no manifest sections, so a valid
/// request finds nothing: ERROR_SXS_KEY_NOT_FOUND for the documented
/// sections (1-13), ERROR_SXS_SECTION_NOT_FOUND for others; recorded when
/// a context is active (its manifest might have had the key).
#[unsafe(no_mangle)]
pub extern "system" fn FindActCtxSectionStringW(
    flags: u32,
    _guid: *const c_void,
    section: u32,
    name: *const u16,
    data: *mut c_void,
) -> Bool {
    let min = core::mem::offset_of!(KeyedData, assembly_roster_index) + 4;
    // SAFETY: the caller's structure's first field.
    if name.is_null() || data.is_null() || (unsafe { *data.cast::<u32>() } as usize) < min {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if flags & !1 != 0 {
        // Only FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX is defined.
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let active = with_stack(|depth, _| *depth).unwrap_or(0);
    if active > 0 {
        super::unsupported(
            b"kernel32.dll!FindActCtxSectionStringW(manifest sections)",
            section.into(),
        );
    }
    if (1..=13).contains(&section) {
        fail_with(ERROR_SXS_KEY_NOT_FOUND, FALSE)
    } else {
        fail_with(ERROR_SXS_SECTION_NOT_FOUND, FALSE)
    }
}

/// FindActCtxSectionStringA: the W function with the name widened.
#[unsafe(no_mangle)]
pub extern "system" fn FindActCtxSectionStringA(
    flags: u32,
    guid: *const c_void,
    section: u32,
    name: *const u8,
    data: *mut c_void,
) -> Bool {
    let mut buf = [0u16; 260];
    match super::sync::wide_name(name, &mut buf) {
        Ok(name) if !name.is_null() => FindActCtxSectionStringW(flags, guid, section, name, data),
        _ => fail_with(ERROR_INVALID_PARAMETER, FALSE),
    }
}

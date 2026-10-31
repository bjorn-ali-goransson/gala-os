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
    if (c.size as usize) < 16 {
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

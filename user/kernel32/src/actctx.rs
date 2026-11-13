//! Activation contexts (CreateActCtxW and friends), as documented: a
//! context comes from a manifest (an RT_MANIFEST resource of a module or
//! PE file, or a .manifest file) and is activated on a per-thread stack.
//!
//! A context is its manifest read (`manifest.rs`: the assemblies, its
//! dependencies found beside the application or the manifest, their
//! files, window classes, COM classes and ProgIDs, the run level and the
//! compatibility list) and the sections built from it once
//! (`sections.rs`: DLL redirection, window classes, COM servers, ProgIDs
//! in Windows' layouts). FindActCtxSectionString/Guid look a key up in
//! the active context, then in the process's default context: the
//! executable's own manifest (RT_MANIFEST 1, else `<exe>.manifest`),
//! read on first use. QueryActCtxW answers every documented class from
//! the model. gala's loader does not redirect DLL loads or window
//! classes through a context: programs that ask see what Windows would
//! tell them.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use super::buf::Buf;
use super::{Bool, FALSE, Handle, TRUE, fail_with};

mod manifest;
mod sections;

use manifest::Model;
use sections::Sections;

const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_SXS_CANT_GEN_ACTCTX: u32 = 14001;
const ERROR_SXS_SECTION_NOT_FOUND: u32 = 14000;
const ERROR_SXS_KEY_NOT_FOUND: u32 = 14007;

const ACTCTX_FLAG_PROCESSOR_ARCHITECTURE_VALID: u32 = 0x1;
const ACTCTX_FLAG_LANGID_VALID: u32 = 0x2;
const ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID: u32 = 0x4;
const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x8;
const ACTCTX_FLAG_SET_PROCESS_DEFAULT: u32 = 0x10;
const ACTCTX_FLAG_APPLICATION_NAME_VALID: u32 = 0x20;
const ACTCTX_FLAG_HMODULE_VALID: u32 = 0x80;

/// ACTIVATION_CONTEXT_PATH_TYPE_NONE / _WIN32_FILE.
const PATH_NONE: u32 = 1;
const PATH_WIN32_FILE: u32 = 2;

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

/// ACTCTXA.
#[repr(C)]
pub struct ActCtxA {
    size: u32,
    flags: u32,
    source: *const u8,
    architecture: u16,
    language: u16,
    assembly_directory: *const u8,
    resource_name: *const u8,
    application_name: *const u8,
    module: Handle,
}

/// "ACTX": tells a context of ours from anything else passed as one.
const MAGIC: u32 = 0x5854_4341;

/// A context (the handle is its address).
struct Context {
    magic: u32,
    refs: AtomicU32,
    model: Model,
    sections: Sections,
}

/// The context behind handle `h`, if it is one of ours.
fn context<'a>(h: usize) -> Option<&'a Context> {
    if h == 0 || h == usize::MAX || !h.is_multiple_of(core::mem::align_of::<Context>()) {
        return None;
    }
    // SAFETY: a handle the caller got from CreateActCtx (checked by its
    // magic before anything else is read).
    let c = unsafe { &*(h as *const Context) };
    (c.magic == MAGIC).then_some(c)
}

/// Records a manifest gala refused (the ledger's "other": what, where).
fn note_refused(reason: &[u8], at: usize) {
    let mut name = [0u8; 64];
    let head = b"kernel32.dll!CreateActCtxW: ";
    let n = head.len().min(64);
    name[..n].copy_from_slice(&head[..n]);
    let m = reason.len().min(64 - n);
    name[n..n + m].copy_from_slice(&reason[..m]);
    super::note(&name[..n + m], at as u64);
}

/// Records a dependency found nowhere (its name, as far as it fits).
fn note_missing_dependency(assembly: &[u16]) {
    let mut name = [0u8; 64];
    let head = b"kernel32.dll!CreateActCtxW: no assembly ";
    name[..head.len()].copy_from_slice(head);
    let mut n = head.len();
    for &c in assembly.iter().take(64 - n) {
        name[n] = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
        n += 1;
    }
    super::note(&name[..n], 0);
}

/// Reads the whole file `path` (NUL-terminated) into `out`.
fn read_file(path: *const u16, out: &mut Buf<u8>) -> bool {
    const GENERIC_READ: u32 = 0x8000_0000;
    // Share read and delete, OPEN_EXISTING.
    let h = super::CreateFileW(
        path,
        GENERIC_READ,
        0x5,
        core::ptr::null_mut(),
        3,
        0,
        core::ptr::null_mut(),
    );
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut chunk = [0u8; 4096];
    let ok = loop {
        let mut n = 0u32;
        if super::ReadFile(h, chunk.as_mut_ptr(), chunk.len() as u32, &mut n, core::ptr::null_mut())
            == FALSE
        {
            break false;
        }
        if n == 0 {
            break true;
        }
        if !out.push_all(&chunk[..n as usize]) {
            super::SetLastError(ERROR_NOT_ENOUGH_MEMORY);
            break false;
        }
    };
    super::CloseHandle(h);
    ok
}

/// The bytes of RT_MANIFEST resource `name` of the image at `base`.
fn module_manifest(
    base: usize,
    name: super::resource::Key,
    lang: u16,
    out: &mut Buf<u8>,
) -> Result<(), u32> {
    use super::resource::{Key, find_resource, resource_bytes};
    let entry = find_resource(base, Key::Id(24), name, lang)?;
    if !out.push_all(resource_bytes(base, entry)) {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    Ok(())
}

/// The manifest resource `id` of the PE file `path` (a private
/// assembly's DLL).
fn read_pe_manifest(path: *const u16, id: u32, out: &mut Buf<u8>) -> bool {
    let Ok(base) = super::mapping::map_image_file(path) else { return false };
    let found = module_manifest(base, super::resource::Key::Id(id), 0, out).is_ok();
    super::mapping::unmap_image_file(base);
    found
}

/// The directory part of `path`, with its final backslash.
fn directory_of(path: &[u16]) -> &[u16] {
    let end = path
        .iter()
        .rposition(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
        .map_or(0, |i| i + 1);
    &path[..end]
}

/// A full path of `name` (NUL-terminated) relative to the current
/// directory, NUL-terminated.
fn full_path(name: *const u16, out: &mut Buf<u16>) -> bool {
    let mut buf = [0u16; 1024];
    let n = super::GetFullPathNameW(name, buf.len() as u32, buf.as_mut_ptr(), core::ptr::null_mut())
        as usize;
    n != 0 && n < buf.len() && out.push_all(&buf[..n]) && out.push(0)
}

fn wide_slice<'a>(s: *const u16) -> &'a [u16] {
    super::wide_slice(s)
}

/// Where a manifest comes from.
enum Source {
    /// Resource `name` (language `lang`) of the loaded module or the
    /// mapped PE at `base`, whose file is `path`.
    Resource {
        base: usize,
        name: super::resource::Key,
        lang: u16,
        mapped: bool,
    },
    File,
}

/// Makes a context. The error is the last error to set.
fn create(c: &ActCtx) -> Result<*mut Context, u32> {
    let through = |field: usize| (c.size as usize) >= field;
    use core::mem::{offset_of, size_of};
    let ptr = size_of::<usize>();
    let flags = c.flags;
    if flags & !0xff != 0
        || !through(offset_of!(ActCtx, source) + ptr)
        || (flags & ACTCTX_FLAG_PROCESSOR_ARCHITECTURE_VALID != 0
            && !through(offset_of!(ActCtx, architecture) + 2))
        || (flags & ACTCTX_FLAG_LANGID_VALID != 0 && !through(offset_of!(ActCtx, language) + 2))
        || (flags & ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID != 0
            && !through(offset_of!(ActCtx, assembly_directory) + ptr))
        || (flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0
            && !through(offset_of!(ActCtx, resource_name) + ptr))
        || (flags & ACTCTX_FLAG_APPLICATION_NAME_VALID != 0
            && !through(offset_of!(ActCtx, application_name) + ptr))
        || (flags & ACTCTX_FLAG_HMODULE_VALID != 0 && !through(offset_of!(ActCtx, module) + ptr))
    {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0 && c.resource_name.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if flags & ACTCTX_FLAG_SET_PROCESS_DEFAULT != 0 {
        super::unsupported(
            b"kernel32.dll!CreateActCtxW(ACTCTX_FLAG_SET_PROCESS_DEFAULT)",
            flags.into(),
        );
    }
    let lang = if flags & ACTCTX_FLAG_LANGID_VALID != 0 { c.language } else { 0 };
    let module =
        if flags & ACTCTX_FLAG_HMODULE_VALID != 0 { c.module } else { core::ptr::null_mut() };
    let resource = if flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0 {
        super::resource::Key::from_wide(c.resource_name)
    } else {
        // CREATEPROCESS_MANIFEST_RESOURCE_ID.
        super::resource::Key::Id(1)
    };
    let mut model = Model::new();
    // The application directory: lpApplicationName, else the module's
    // (the executable's) directory.
    if flags & ACTCTX_FLAG_APPLICATION_NAME_VALID != 0 && !c.application_name.is_null() {
        model.appdir = model.keep(wide_slice(c.application_name)).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    } else {
        let (path, n) = super::process::module_path(module)?;
        model.appdir = model.keep(directory_of(&path[..n])).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    }
    // The source's full path: lpSource, inside lpAssemblyDirectory when
    // relative and that is given.
    let mut path = Buf::<u16>::new();
    if !c.source.is_null() {
        let src = wide_slice(c.source);
        let relative = !(src.len() >= 2 && src[1] == u16::from(b':'))
            && !src.first().is_some_and(|&ch| ch == u16::from(b'\\') || ch == u16::from(b'/'));
        let mut joined = Buf::<u16>::new();
        let name = if flags & ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID != 0
            && relative
            && !c.assembly_directory.is_null()
        {
            let ok = joined.push_all(wide_slice(c.assembly_directory))
                && joined.push(u16::from(b'\\'))
                && joined.push_all(src)
                && joined.push(0);
            if !ok {
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            joined.as_slice().as_ptr()
        } else {
            c.source
        };
        if !full_path(name, &mut path) {
            return Err(super::GetLastError());
        }
    }
    let source = if flags & ACTCTX_FLAG_HMODULE_VALID != 0
        && (flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0 || c.source.is_null())
    {
        let base = if module.is_null() {
            super::mapping::image_base() as usize
        } else {
            module as usize & !3
        };
        path = Buf::new();
        let (p, n) = super::process::module_path(module)?;
        if !(path.push_all(&p[..n]) && path.push(0)) {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        Source::Resource { base, name: resource, lang, mapped: false }
    } else if flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0 {
        if c.source.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let base = super::mapping::map_image_file(path.as_slice().as_ptr())?;
        Source::Resource { base, name: resource, lang, mapped: true }
    } else if c.source.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    } else {
        Source::File
    };
    let mut bytes = Buf::<u8>::new();
    match source {
        Source::Resource { base, name, lang, mapped } => {
            let found = module_manifest(base, name, lang, &mut bytes);
            if mapped {
                super::mapping::unmap_image_file(base);
            }
            if let Err(e) = found {
                // The manifest file beside it: "<file>[.<id>].manifest".
                let mut alt = Buf::<u16>::new();
                let mut ok = alt.push_all(&path.as_slice()[..path.len().saturating_sub(1)]);
                if let super::resource::Key::Id(id) = name
                    && id != 1
                {
                    ok = ok && alt.push(u16::from(b'.'));
                    let mut digits = [0u16; 10];
                    let mut k = 0;
                    let mut v = id;
                    loop {
                        digits[k] = u16::from(b'0') + (v % 10) as u16;
                        k += 1;
                        v /= 10;
                        if v == 0 {
                            break;
                        }
                    }
                    for d in digits[..k].iter().rev() {
                        ok = ok && alt.push(*d);
                    }
                }
                ok = ok && b".manifest".iter().all(|&b| alt.push(u16::from(b))) && alt.push(0);
                if !ok {
                    return Err(ERROR_NOT_ENOUGH_MEMORY);
                }
                if !read_file(alt.as_slice().as_ptr(), &mut bytes) {
                    return Err(e);
                }
                path = alt;
            }
        }
        Source::File => {
            if !read_file(path.as_slice().as_ptr(), &mut bytes) {
                return Err(super::GetLastError());
            }
        }
    }
    let full = &path.as_slice()[..path.len().saturating_sub(1)];
    let mut text = Buf::<u16>::new();
    if !manifest::decode(bytes.as_slice(), &mut text) {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    let built = model
        .parse(text.as_slice(), full, &[], None)
        .and_then(|()| model.resolve(directory_of(full)));
    if let Err(refused) = built {
        note_refused(refused.reason, refused.at);
        return Err(if refused.reason == b"no memory" {
            ERROR_NOT_ENOUGH_MEMORY
        } else {
            ERROR_SXS_CANT_GEN_ACTCTX
        });
    }
    if model.skipped != 0 {
        // Content read but without sections yet (typelibs, proxy/stubs,
        // CLR classes, WinRT classes, loadFrom): its lookups find nothing.
        super::unsupported(
            b"kernel32.dll!CreateActCtxW(manifest sections not built)",
            model.skipped.into(),
        );
    }
    static SEED: AtomicU32 = AtomicU32::new(0x9e37_79b9);
    let seed = SEED.fetch_add(0x6d2b_79f5, Ordering::Relaxed) ^ super::GetTickCount();
    let sections = sections::build(&model, seed).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    let p =
        super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, core::mem::size_of::<Context>())
            .cast::<Context>();
    if p.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    // SAFETY: fresh memory of the context's size and alignment (the
    // process heap's blocks are 16-byte aligned).
    unsafe { p.write(Context { magic: MAGIC, refs: AtomicU32::new(1), model, sections }) };
    Ok(p)
}

/// CreateActCtxW: the manifest must exist - resource `lpResourceName`
/// (default 1) of type RT_MANIFEST of hModule or of the PE file
/// lpSource, or the manifest file lpSource - and parse, with every
/// dependency found (ERROR_SXS_CANT_GEN_ACTCTX otherwise).
#[unsafe(no_mangle)]
pub extern "system" fn CreateActCtxW(ctx: *const ActCtx) -> Handle {
    if ctx.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    // SAFETY: the caller's ACTCTXW (cbSize checked before each field).
    match create(unsafe { &*ctx }) {
        Ok(p) => p.cast(),
        Err(e) => fail_with(e, INVALID_HANDLE_VALUE),
    }
}

/// CreateActCtxA: the W function with the strings widened.
#[unsafe(no_mangle)]
pub extern "system" fn CreateActCtxA(ctx: *const ActCtxA) -> Handle {
    if ctx.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    // SAFETY: the caller's ACTCTXA; only the fields its size covers are
    // read.
    let a = unsafe { &*ctx };
    let covers = |field: usize| (a.size as usize) >= field + core::mem::size_of::<usize>();
    use core::mem::offset_of;
    let mut bufs = [[0u16; 260]; 4];
    let [b0, b1, b2, b3] = &mut bufs;
    let widen = |p: *const u8, buf: &mut [u16; 260], valid: bool| -> Result<*const u16, ()> {
        if !valid || p.is_null() {
            return Ok(core::ptr::null());
        }
        super::sync::wide_name(p, buf).map_err(|_| ())
    };
    let flags = a.flags;
    let source = widen(a.source, b0, covers(offset_of!(ActCtxA, source)));
    let dir = widen(
        a.assembly_directory,
        b1,
        flags & ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID != 0
            && covers(offset_of!(ActCtxA, assembly_directory)),
    );
    let resource = if flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0
        && covers(offset_of!(ActCtxA, resource_name))
        && (a.resource_name as usize) >> 16 == 0
    {
        Ok(a.resource_name.cast::<u16>())
    } else {
        widen(
            a.resource_name,
            b2,
            flags & ACTCTX_FLAG_RESOURCE_NAME_VALID != 0
                && covers(offset_of!(ActCtxA, resource_name)),
        )
    };
    let app = widen(
        a.application_name,
        b3,
        flags & ACTCTX_FLAG_APPLICATION_NAME_VALID != 0
            && covers(offset_of!(ActCtxA, application_name)),
    );
    let (Ok(source), Ok(dir), Ok(resource), Ok(app)) = (source, dir, resource, app) else {
        return fail_with(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    };
    let w = ActCtx {
        size: a.size,
        flags,
        source,
        architecture: if covers(offset_of!(ActCtxA, architecture)) { a.architecture } else { 0 },
        language: if covers(offset_of!(ActCtxA, language)) { a.language } else { 0 },
        assembly_directory: dir,
        resource_name: resource,
        application_name: app,
        module: if covers(offset_of!(ActCtxA, module)) { a.module } else { core::ptr::null_mut() },
    };
    CreateActCtxW(&w)
}

#[unsafe(no_mangle)]
pub extern "system" fn AddRefActCtx(ctx: Handle) {
    if let Some(c) = context(ctx as usize) {
        c.refs.fetch_add(1, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseActCtx(ctx: Handle) {
    let Some(c) = context(ctx as usize) else { return };
    if c.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        let p = ctx.cast::<Context>();
        // SAFETY: the last reference: nothing else uses the context; its
        // model and sections free their buffers, then the block goes.
        unsafe {
            (*p).magic = 0;
            core::ptr::drop_in_place(p);
        }
        super::heap::HeapFree(super::heap::GetProcessHeap(), 0, ctx);
    }
}

/// The process's default context: the executable's manifest, made on
/// first use (0: it has none, or it was refused: recorded). Never freed.
fn process_default() -> usize {
    const NONE: usize = 1;
    static DEFAULT: AtomicUsize = AtomicUsize::new(0);
    let d = DEFAULT.load(Ordering::Acquire);
    if d != 0 {
        return if d == NONE { 0 } else { d };
    }
    let exe = super::mapping::image_base();
    let made = if super::resource::find_resource(
        exe as usize,
        super::resource::Key::Id(24),
        super::resource::Key::Id(1),
        0,
    )
    .is_ok()
    {
        let c = ActCtx {
            size: core::mem::size_of::<ActCtx>() as u32,
            flags: ACTCTX_FLAG_RESOURCE_NAME_VALID | ACTCTX_FLAG_HMODULE_VALID,
            source: core::ptr::null(),
            architecture: 0,
            language: 0,
            assembly_directory: core::ptr::null(),
            resource_name: core::ptr::without_provenance(1),
            application_name: core::ptr::null(),
            module: exe,
        };
        Some(create(&c))
    } else {
        // "<exe>.manifest" beside it.
        let mut path = Buf::<u16>::new();
        let ok = super::process::module_path(core::ptr::null_mut()).is_ok_and(|(p, n)| {
            path.push_all(&p[..n])
                && b".manifest".iter().all(|&b| path.push(u16::from(b)))
                && path.push(0)
        });
        let exists = ok && super::GetFileAttributesW(path.as_slice().as_ptr()) != u32::MAX;
        exists.then(|| {
            let c = ActCtx {
                size: core::mem::size_of::<ActCtx>() as u32,
                flags: 0,
                source: path.as_slice().as_ptr(),
                architecture: 0,
                language: 0,
                assembly_directory: core::ptr::null(),
                resource_name: core::ptr::null(),
                application_name: core::ptr::null(),
                module: core::ptr::null_mut(),
            };
            create(&c)
        })
    };
    let value = match made {
        Some(Ok(p)) => p as usize,
        Some(Err(e)) => {
            // Windows would not have started the process.
            super::note(b"kernel32.dll!process default activation context refused", e.into());
            NONE
        }
        None => NONE,
    };
    match DEFAULT.compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            if value == NONE {
                0
            } else {
                value
            }
        }
        Err(other) => {
            if value != NONE {
                ReleaseActCtx(value as Handle);
            }
            if other == NONE { 0 } else { other }
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

/// The innermost active context of this thread (0: none).
fn active() -> usize {
    with_stack(|depth, entries| if *depth == 0 { 0 } else { entries[*depth - 1].0 }).unwrap_or(0)
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
/// none is active: the process default is not on the stack).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentActCtx(out: *mut Handle) -> Bool {
    if out.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let ctx = active();
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

/// ACTIVATION_CONTEXT_DETAILED_INFORMATION.
#[repr(C)]
struct DetailedInformation {
    flags: u32,
    format_version: u32,
    assembly_count: u32,
    root_manifest_type: u32,
    root_manifest_chars: u32,
    root_config_type: u32,
    root_config_chars: u32,
    appdir_type: u32,
    appdir_chars: u32,
    root_manifest: usize,
    root_config: usize,
    appdir: usize,
}

/// ACTIVATION_CONTEXT_ASSEMBLY_DETAILED_INFORMATION.
#[repr(C)]
struct AssemblyInformation {
    flags: u32,
    identity_bytes: u32,
    manifest_type: u32,
    manifest_bytes: u32,
    manifest_time: u64,
    policy_type: u32,
    policy_bytes: u32,
    policy_time: u64,
    satellite_roster_index: u32,
    manifest_version_major: u32,
    manifest_version_minor: u32,
    policy_version_major: u32,
    policy_version_minor: u32,
    directory_bytes: u32,
    identity: usize,
    manifest: usize,
    policy: usize,
    directory: usize,
    file_count: u32,
}

/// ASSEMBLY_FILE_DETAILED_INFORMATION.
#[repr(C)]
struct FileInformation {
    flags: u32,
    name_bytes: u32,
    path_bytes: u32,
    name: usize,
    path: usize,
}

/// ACTIVATION_CONTEXT_RUN_LEVEL_INFORMATION.
#[repr(C)]
struct RunLevelInformation {
    flags: u32,
    run_level: u32,
    ui_access: u32,
}

/// COMPATIBILITY_CONTEXT_ELEMENT (Windows 10 1903's, with
/// MaxVersionTested), after ACTIVATION_CONTEXT_COMPATIBILITY_INFORMATION's
/// ElementCount (8 bytes with its padding).
#[repr(C)]
struct CompatElement {
    id: [u8; 16],
    kind: u32,
    max_version: u64,
}

/// Writes `head` and then `strings` (each NUL-terminated, in order) to
/// the caller's buffer; `head`'s pointer fields are set by `fix` from the
/// strings' addresses (None for an empty one).
fn write_with_strings<T>(
    buffer: *mut c_void,
    head: T,
    strings: &[&[u16]],
    fix: impl FnOnce(&mut T, &[usize]),
) {
    let mut head = head;
    let mut at = buffer as usize + core::mem::size_of::<T>();
    let mut addrs = [0usize; 4];
    for (i, s) in strings.iter().enumerate() {
        if s.is_empty() && i > 0 {
            continue;
        }
        addrs[i] = at;
        // SAFETY: the caller's buffer holds the head and every string
        // (its size was checked against the total).
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), at as *mut u16, s.len());
            *((at + 2 * s.len()) as *mut u16) = 0;
        }
        at += 2 * (s.len() + 1);
    }
    fix(&mut head, &addrs[..strings.len()]);
    // SAFETY: as above.
    unsafe { buffer.cast::<T>().write_unaligned(head) };
}

/// Bytes `strings` take with their NULs (an empty one after the first
/// takes none).
fn strings_size(strings: &[&[u16]]) -> usize {
    strings
        .iter()
        .enumerate()
        .filter(|&(i, s)| i == 0 || !s.is_empty())
        .map(|(_, s)| 2 * (s.len() + 1))
        .sum()
}

/// The context QueryActCtxW's arguments name (0: the empty default).
fn query_target(flags: u32, actctx: Handle, class: u32) -> Result<usize, u32> {
    if flags & QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX != 0 {
        if !actctx.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let a = active();
        return Ok(if a == 0 && class != 1 { process_default() } else { a });
    }
    if flags & (QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE | QUERY_ACTCTX_FLAG_ACTCTX_IS_ADDRESS) != 0 {
        if actctx.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let mut base: Handle = core::ptr::null_mut();
        // FROM_ADDRESS | UNCHANGED_REFCOUNT.
        if super::text::GetModuleHandleExW(4 | 2, actctx.cast(), &mut base) == FALSE
            || (flags & QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE != 0 && base != actctx)
        {
            return Err(ERROR_MOD_NOT_FOUND);
        }
        let ctx = module_context(base as usize);
        return Ok(if ctx == 0 && class != 1 { process_default() } else { ctx });
    }
    if actctx.is_null() {
        return Ok(if class == 1 { 0 } else { process_default() });
    }
    Ok(actctx as usize)
}

/// A loaded module's own context: a DLL's manifest resource 2
/// (ISOLATIONAWARE_MANIFEST_RESOURCE_ID), made on first use; 0 for the
/// executable (the process default) and a DLL without one.
fn module_context(base: usize) -> usize {
    const SLOTS: usize = 64;
    static MODULES: [(AtomicUsize, AtomicUsize); SLOTS] =
        [const { (AtomicUsize::new(0), AtomicUsize::new(0)) }; SLOTS];
    if base == super::mapping::image_base() as usize {
        return 0;
    }
    if let Some((_, c)) = MODULES.iter().find(|(m, _)| m.load(Ordering::Acquire) == base) {
        return c.load(Ordering::Acquire);
    }
    let key = super::resource::Key::Id(2);
    let made = if super::resource::find_resource(base, super::resource::Key::Id(24), key, 0).is_ok()
    {
        let c = ActCtx {
            size: core::mem::size_of::<ActCtx>() as u32,
            flags: ACTCTX_FLAG_RESOURCE_NAME_VALID | ACTCTX_FLAG_HMODULE_VALID,
            source: core::ptr::null(),
            architecture: 0,
            language: 0,
            assembly_directory: core::ptr::null(),
            resource_name: core::ptr::without_provenance(2),
            application_name: core::ptr::null(),
            module: base as Handle,
        };
        match create(&c) {
            Ok(p) => p as usize,
            Err(e) => {
                super::note(b"kernel32.dll!a DLL's activation context refused", e.into());
                0
            }
        }
    } else {
        0
    };
    // Kept for the module's life (a slot per module; when they run out
    // the context is made again on each query and released with the
    // caller's reference).
    for (m, c) in &MODULES {
        if m.compare_exchange(0, base, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            c.store(made, Ordering::Release);
            return made;
        }
        if m.load(Ordering::Acquire) == base {
            if made != 0 {
                ReleaseActCtx(made as Handle);
            }
            return c.load(Ordering::Acquire);
        }
    }
    super::unsupported(b"kernel32.dll!QueryActCtxW(module contexts table full)", SLOTS as u64);
    made
}

/// QueryActCtxW(flags, actctx, subinstance, class, buffer, size,
/// &written): the documented ACTIVATION_CONTEXT_INFO_CLASS classes of the
/// context given (NULL: the process default), the active one
/// (QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX; `actctx` must then be NULL) or a
/// module's (QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE/_IS_ADDRESS).
/// ERROR_INSUFFICIENT_BUFFER with the size needed in `written` when the
/// buffer is too small.
#[unsafe(no_mangle)]
pub extern "system" fn QueryActCtxW(
    flags: u32,
    actctx: Handle,
    subinstance: *mut c_void,
    class: u32,
    buffer: *mut c_void,
    size: usize,
    written: *mut usize,
) -> Bool {
    let set_written = |n: usize| {
        if !written.is_null() {
            // SAFETY: the caller's SIZE_T.
            unsafe { *written = n };
        }
    };
    set_written(0);
    let known = QUERY_ACTCTX_FLAG_USE_ACTIVE_ACTCTX
        | QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE
        | QUERY_ACTCTX_FLAG_ACTCTX_IS_ADDRESS
        | QUERY_ACTCTX_FLAG_NO_ADDREF;
    if flags & !known != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let ctx = match query_target(flags, actctx, class) {
        Ok(c) => c,
        Err(e) => return fail_with(e, FALSE),
    };
    // Room for `need` bytes (else ERROR_INSUFFICIENT_BUFFER, `need` in
    // `written`).
    let room = |need: usize| {
        set_written(need);
        !buffer.is_null() && size >= need
    };
    if class == 1 {
        let need = core::mem::size_of::<BasicInformation>();
        if !room(need) {
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
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
        return TRUE;
    }
    if !(2..=6).contains(&class) {
        super::unsupported(b"kernel32.dll!QueryActCtxW(class)", class.into());
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let c = if ctx == 0 {
        None
    } else {
        match context(ctx) {
            Some(c) => Some(c),
            None => return fail_with(ERROR_INVALID_PARAMETER, FALSE),
        }
    };
    let m = c.map(|c| &c.model);
    match class {
        2 => {
            // ActivationContextDetailedInformation: the root manifest's
            // path and the application directory.
            let (manifest, appdir, count) = match m {
                Some(m) => (
                    m.str(m.assemblies.as_slice()[0].manifest),
                    m.str(m.appdir),
                    m.assemblies.len() as u32,
                ),
                None => (&[][..], &[][..], 0),
            };
            let strings: [&[u16]; 2] = [manifest, appdir];
            let need = core::mem::size_of::<DetailedInformation>()
                + strings
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| 2 * (s.len() + 1))
                    .sum::<usize>();
            if !room(need) {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
            }
            let known = m.is_some();
            let head = DetailedInformation {
                flags: 0,
                format_version: u32::from(known),
                assembly_count: count,
                root_manifest_type: if !known {
                    0
                } else if manifest.is_empty() {
                    PATH_NONE
                } else {
                    PATH_WIN32_FILE
                },
                root_manifest_chars: manifest.len() as u32,
                root_config_type: if known { PATH_NONE } else { 0 },
                root_config_chars: 0,
                appdir_type: if known { PATH_WIN32_FILE } else { 0 },
                appdir_chars: appdir.len() as u32,
                root_manifest: 0,
                root_config: 0,
                appdir: 0,
            };
            let mut at = buffer as usize + core::mem::size_of::<DetailedInformation>();
            let mut addrs = [0usize; 2];
            for (i, s) in strings.iter().enumerate() {
                if s.is_empty() {
                    continue;
                }
                addrs[i] = at;
                // SAFETY: inside the caller's buffer (`need` bytes).
                unsafe {
                    core::ptr::copy_nonoverlapping(s.as_ptr(), at as *mut u16, s.len());
                    *((at + 2 * s.len()) as *mut u16) = 0;
                }
                at += 2 * (s.len() + 1);
            }
            let head = DetailedInformation { root_manifest: addrs[0], appdir: addrs[1], ..head };
            // SAFETY: as above.
            unsafe { buffer.cast::<DetailedInformation>().write_unaligned(head) };
            TRUE
        }
        3 => {
            // AssemblyDetailedInformationInActivationContext: assembly
            // *subinstance (1-based).
            let Some(m) = m else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
            if subinstance.is_null() {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            // SAFETY: the caller's DWORD index.
            let index = unsafe { *subinstance.cast::<u32>() } as usize;
            if index == 0 || index > m.assemblies.len() {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            let a = &m.assemblies.as_slice()[index - 1];
            let mut identity = Buf::<u16>::new();
            if !m.encoded_identity(&a.id, &mut identity) {
                return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
            }
            let strings: [&[u16]; 3] = [identity.as_slice(), m.str(a.manifest), m.str(a.directory)];
            let need = core::mem::size_of::<AssemblyInformation>() + strings_size(&strings);
            if !room(need) {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
            }
            let head = AssemblyInformation {
                flags: if index == 1 { 1 } else { 0x10 },
                identity_bytes: 2 * identity.len() as u32,
                manifest_type: if a.manifest.is_empty() { PATH_NONE } else { PATH_WIN32_FILE },
                manifest_bytes: 2 * a.manifest.len,
                manifest_time: 0,
                policy_type: PATH_NONE,
                policy_bytes: 0,
                policy_time: 0,
                satellite_roster_index: 0,
                manifest_version_major: 1,
                manifest_version_minor: 0,
                policy_version_major: 0,
                policy_version_minor: 0,
                directory_bytes: 2 * a.directory.len,
                identity: 0,
                manifest: 0,
                policy: 0,
                directory: 0,
                file_count: a.files,
            };
            write_with_strings(buffer, head, &strings, |h, at| {
                h.identity = at[0];
                h.manifest = at[1];
                h.directory = at[2];
            });
            TRUE
        }
        4 => {
            // FileInformationInAssemblyOfAssemblyInActivationContext:
            // ACTIVATION_CONTEXT_QUERY_INDEX { assembly (0-based), file }.
            let Some(m) = m else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
            if subinstance.is_null() {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            // SAFETY: the caller's two DWORDs.
            let (ai, fi) =
                unsafe { (*subinstance.cast::<u32>(), *subinstance.cast::<u32>().add(1)) };
            let Some(a) = m.assemblies.as_slice().get(ai as usize) else {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            };
            if fi >= a.files {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            let name = m.str(m.files.as_slice()[(a.first_file + fi) as usize].name);
            let need = core::mem::size_of::<FileInformation>() + 2 * (name.len() + 1);
            if !room(need) {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
            }
            // On success Windows reports 0 bytes written.
            set_written(0);
            let head = FileInformation {
                // ACTIVATION_CONTEXT_SECTION_DLL_REDIRECTION.
                flags: 2,
                name_bytes: 2 * name.len() as u32,
                path_bytes: 0,
                name: 0,
                path: 0,
            };
            write_with_strings(buffer, head, &[name], |h, at| h.name = at[0]);
            TRUE
        }
        5 => {
            // RunlevelInformationInActivationContext: the root's
            // requestedExecutionLevel (0: unspecified).
            let need = core::mem::size_of::<RunLevelInformation>();
            if !room(need) {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
            }
            let (run_level, ui_access) = m.map_or((0, 0), |m| {
                (m.assemblies.as_slice()[0].run_level, m.assemblies.as_slice()[0].ui_access)
            });
            // SAFETY: the caller's buffer of `need` bytes.
            unsafe {
                buffer.cast::<RunLevelInformation>().write_unaligned(RunLevelInformation {
                    flags: 0,
                    run_level,
                    ui_access,
                })
            };
            TRUE
        }
        _ => {
            // CompatibilityInformationInActivationContext: the root's
            // supportedOS and maxversiontested entries, in order.
            let compat = m.map_or(&[][..], |m| m.compat.as_slice());
            let need = 8 + compat.len() * core::mem::size_of::<CompatElement>();
            if !room(need) {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
            }
            // SAFETY: the caller's buffer of `need` bytes.
            unsafe {
                buffer.cast::<u32>().write_unaligned(compat.len() as u32);
                buffer.cast::<u32>().add(1).write_unaligned(0);
                let elements = buffer.cast::<u8>().add(8).cast::<CompatElement>();
                for (i, e) in compat.iter().enumerate() {
                    elements.add(i).write_unaligned(CompatElement {
                        id: e.id,
                        kind: e.kind,
                        max_version: e.max_version,
                    });
                }
            }
            TRUE
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

const FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX: u32 = 1;

/// Fills the caller's keyed data with a found key: its data at `data`
/// (`length` bytes) inside section `base`, the section's global data.
fn fill(
    out: *mut c_void,
    ctx: usize,
    flags: u32,
    section: &[u8],
    found: (usize, usize, u32),
    length: usize,
    global: (usize, usize),
) {
    let base = section.as_ptr() as usize;
    let actctx = if flags & FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX != 0 {
        AddRefActCtx(ctx as Handle);
        ctx as Handle
    } else {
        core::ptr::null_mut()
    };
    let d = out.cast::<KeyedData>();
    // SAFETY: the caller's structure, at least up to the roster index
    // (cbSize was checked); its cbSize is left as it is.
    unsafe {
        (*d).version = 1;
        (*d).data = base + found.0;
        (*d).length = length as u32;
        (*d).section_global = if global.1 == 0 { 0 } else { base + global.0 };
        (*d).section_global_length = global.1 as u32;
        (*d).section_base = base;
        (*d).section_total_length = section.len() as u32;
        (*d).actctx = actctx;
        (*d).assembly_roster_index = found.2;
    }
}

fn rd32(s: &[u8], at: usize) -> usize {
    u32::from_le_bytes([s[at], s[at + 1], s[at + 2], s[at + 3]]) as usize
}

/// Checks a keyed-data argument: present and with a cbSize covering
/// ulAssemblyRosterIndex.
fn keyed_data_ok(data: *mut c_void) -> bool {
    let min = core::mem::offset_of!(KeyedData, assembly_roster_index) + 4;
    // SAFETY: the caller's structure's first field.
    !data.is_null() && (unsafe { *data.cast::<u32>() } as usize) >= min
}

/// The contexts a lookup searches: the active one, then the process
/// default.
fn search_order() -> [usize; 2] {
    let a = active();
    let d = process_default();
    [a, if d == a { 0 } else { d }]
}

/// Records a lookup in a section gala does not build when the context's
/// manifest had content for it.
fn record_skipped(ctx: &Context, section: u32) {
    use manifest::{SKIPPED_CLR, SKIPPED_PROXY_STUB, SKIPPED_TYPELIB, SKIPPED_WINRT};
    let relevant = match section {
        5 => SKIPPED_PROXY_STUB,
        6 => SKIPPED_TYPELIB,
        9 => SKIPPED_CLR,
        12 => SKIPPED_WINRT,
        _ => 0,
    };
    if ctx.model.skipped & relevant != 0 {
        super::unsupported(b"kernel32.dll!FindActCtxSection(section not built)", section.into());
    }
}

/// FindActCtxSectionStringW(flags, guid, section, name, &data): looks
/// `name` up in the active context, then the process default: DLL
/// redirection (2), window classes (3) and ProgIDs (7); the other
/// documented sections (1-13) find nothing (ERROR_SXS_KEY_NOT_FOUND),
/// others are ERROR_SXS_SECTION_NOT_FOUND.
#[unsafe(no_mangle)]
pub extern "system" fn FindActCtxSectionStringW(
    flags: u32,
    _guid: *const c_void,
    section: u32,
    name: *const u16,
    data: *mut c_void,
) -> Bool {
    if name.is_null() || !keyed_data_ok(data) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if flags & !FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if !(1..=13).contains(&section) {
        return fail_with(ERROR_SXS_SECTION_NOT_FOUND, FALSE);
    }
    let key = wide_slice(name);
    for h in search_order() {
        let Some(c) = context(h) else { continue };
        let s = match section {
            2 => c.sections.dll.as_slice(),
            3 => c.sections.classes.as_slice(),
            7 => c.sections.progid.as_slice(),
            _ => {
                record_skipped(c, section);
                continue;
            }
        };
        let Some(found) = sections::find_string(s, key) else { continue };
        let (length, global) = match section {
            // The record: size, flags, path data.
            2 => (rd32(s, found.0), (0, 0)),
            // The record and its two names with their NULs.
            3 => (found.1, (0, 0)),
            _ => (rd32(s, found.0), (rd32(s, 36), rd32(s, 40))),
        };
        fill(data, h, flags, s, found, length, global);
        return TRUE;
    }
    fail_with(ERROR_SXS_KEY_NOT_FOUND, FALSE)
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

/// FindActCtxSectionGuid(flags, extension, section, guid, &data): COM
/// servers (4) by CLSID (or the alias a ProgID's data points to); the
/// other GUID sections (interfaces 5, type libraries 6, CLR surrogates 9)
/// find nothing.
#[unsafe(no_mangle)]
pub extern "system" fn FindActCtxSectionGuid(
    flags: u32,
    _extension: *const c_void,
    section: u32,
    guid: *const [u8; 16],
    data: *mut c_void,
) -> Bool {
    if guid.is_null()
        || !keyed_data_ok(data)
        || flags & !FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX != 0
    {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if !matches!(section, 4 | 5 | 6 | 9) {
        return fail_with(ERROR_SXS_SECTION_NOT_FOUND, FALSE);
    }
    // SAFETY: the caller's GUID.
    let key = unsafe { *guid };
    for h in search_order() {
        let Some(c) = context(h) else { continue };
        if section != 4 {
            record_skipped(c, section);
            continue;
        }
        let s = c.sections.com.as_slice();
        let Some(found) = sections::find_guid(s, &key) else { continue };
        // The record, its CLR data and its ProgID with the NUL.
        let progid = rd32(s, found.0 + 84);
        let length =
            sections::COM_DATA + rd32(s, found.0 + 92) + if progid != 0 { progid + 2 } else { 0 };
        fill(data, h, flags, s, found, length, (rd32(s, 32), rd32(s, 36)));
        return TRUE;
    }
    fail_with(ERROR_SXS_KEY_NOT_FOUND, FALSE)
}

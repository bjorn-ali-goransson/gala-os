//! A small symbol handler (dbghelp.h), as documented: SymInitialize(W),
//! SymCleanup, SymSetOptions/SymGetOptions, SymLoadModuleExW,
//! SymUnloadModule64 and SymFromAddr. Symbols are the modules' exports
//! (read from the image mapped at the module's base in this process, or
//! else from its file): gala reads no PDBs yet, so a load is recorded once
//! as "exports only" and SymFromAddr finds the nearest export at or below
//! the address. See user/dbghelp_test.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
    fn GetCurrentProcess() -> usize;
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, size: usize) -> usize;
}

#[repr(C)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

fn unsupported(what: &[u8], detail: u64) {
    crate::image::unsupported(what, detail);
}

fn fail<T>(e: u32, v: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(e) };
    v
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// A loaded module: its base and size (0 base: a free slot).
#[derive(Clone, Copy)]
struct Module {
    base: u64,
    size: u64,
}

const MODULES: usize = 64;

static LOCK: AtomicBool = AtomicBool::new(false);
static mut TABLE: [Module; MODULES] = [Module { base: 0, size: 0 }; MODULES];
static OPTIONS: AtomicU32 = AtomicU32::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static RECORDED: AtomicBool = AtomicBool::new(false);

fn with<T>(f: impl FnOnce(&mut [Module; MODULES]) -> T) -> T {
    while LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: the table is only touched under LOCK.
    let table = &raw mut TABLE;
    let r = f(unsafe { &mut *table });
    LOCK.store(false, Ordering::Release);
    r
}

/// Whether `process` is this process (the only one gala reads symbols
/// of: other processes' memory would need ReadProcessMemory).
pub(crate) fn own(process: usize) -> bool {
    // SAFETY: plain call.
    process == unsafe { GetCurrentProcess() } || process == usize::MAX || process == 0
}

/// SymInitializeW: starts the handler for this process (the search path
/// and `invade` are not used: modules are loaded explicitly).
#[unsafe(no_mangle)]
pub extern "system" fn SymInitializeW(process: usize, _path: *const u16, invade: i32) -> i32 {
    if !own(process) {
        unsupported(b"dbghelp.dll!SymInitialize(another process)", 0);
    }
    if invade != 0 {
        unsupported(b"dbghelp.dll!SymInitialize(invade)", 0);
    }
    INITIALIZED.store(true, Ordering::Release);
    1
}

/// SymInitialize: as SymInitializeW.
#[unsafe(no_mangle)]
pub extern "system" fn SymInitialize(process: usize, _path: *const u8, invade: i32) -> i32 {
    SymInitializeW(process, core::ptr::null(), invade)
}

/// SymCleanup: forgets every module.
#[unsafe(no_mangle)]
pub extern "system" fn SymCleanup(_process: usize) -> i32 {
    with(|t| t.iter_mut().for_each(|m| *m = Module { base: 0, size: 0 }));
    INITIALIZED.store(false, Ordering::Release);
    1
}

/// SymSetOptions: kept (and returned); they change nothing here.
#[unsafe(no_mangle)]
pub extern "system" fn SymSetOptions(options: u32) -> u32 {
    OPTIONS.store(options, Ordering::Relaxed);
    options
}

#[unsafe(no_mangle)]
pub extern "system" fn SymGetOptions() -> u32 {
    OPTIONS.load(Ordering::Relaxed)
}

/// Whether `[at, at + len)` is committed, readable memory.
fn readable(at: u64, len: u64) -> bool {
    let mut p = at;
    while p < at + len {
        // SAFETY: zeroed plain data, filled by VirtualQuery.
        let mut info: MemoryInfo = unsafe { core::mem::zeroed() };
        // SAFETY: a query of our own address space.
        let n = unsafe {
            VirtualQuery(p as *const c_void, &mut info, core::mem::size_of::<MemoryInfo>())
        };
        const MEM_COMMIT: u32 = 0x1000;
        const NOACCESS: u32 = 0x01;
        const GUARD: u32 = 0x100;
        if n == 0
            || info.state != MEM_COMMIT
            || info.protect & (NOACCESS | GUARD) != 0
            || info.protect == 0
        {
            return false;
        }
        p = (info.base + info.region_size) as u64;
    }
    true
}

fn u16_at(base: u64, off: u64) -> u16 {
    // SAFETY: inside a mapped image checked readable by the callers.
    unsafe { ((base + off) as *const u16).read_unaligned() }
}

fn u32_at(base: u64, off: u64) -> u32 {
    // SAFETY: as above.
    unsafe { ((base + off) as *const u32).read_unaligned() }
}

/// The SizeOfImage of the image mapped at `base`, if one is.
fn mapped_size(base: u64) -> Option<u64> {
    if base == 0 || !readable(base, 0x40) || u16_at(base, 0) != 0x5a4d {
        return None;
    }
    let nt = u64::from(u32_at(base, 0x3c));
    if nt > 0x1000 || !readable(base + nt, 0x100) || u32_at(base, nt) != 0x4550 {
        return None;
    }
    Some(u64::from(u32_at(base, nt + 24 + 56)))
}

/// SymLoadModuleExW: the module at `base` (of `size`, or its image's
/// SizeOfImage) added; its base, or 0 with ERROR_SUCCESS when it is
/// already loaded. gala reads its exports from the mapped image (an image
/// not mapped in this process is not read yet: recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SymLoadModuleExW(
    process: usize,
    _file: usize,
    _image: *const u16,
    _module: *const u16,
    base: u64,
    size: u32,
    _data: *const c_void,
    flags: u32,
) -> u64 {
    if !own(process) {
        unsupported(b"dbghelp.dll!SymLoadModuleExW(another process)", 0);
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    if !INITIALIZED.load(Ordering::Acquire) {
        unsupported(b"dbghelp.dll!SymLoadModuleExW(before SymInitialize)", 0);
    }
    if base == 0 {
        unsupported(b"dbghelp.dll!SymLoadModuleExW(no base: from the file)", 0);
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let size = match (size, mapped_size(base)) {
        (0, Some(s)) => s,
        (0, None) => {
            unsupported(b"dbghelp.dll!SymLoadModuleExW(image not mapped)", flags.into());
            return fail(ERROR_MOD_NOT_FOUND, 0);
        }
        (s, _) => u64::from(s),
    };
    if !RECORDED.swap(true, Ordering::Relaxed) {
        unsupported(b"dbghelp.dll!SymLoadModuleExW(exports only, no PDB)", 0);
    }
    let r = with(|t| {
        if t.iter().any(|m| m.base == base) {
            return Ok(0);
        }
        let slot = t.iter_mut().find(|m| m.base == 0).ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
        *slot = Module { base, size };
        Ok(base)
    });
    match r {
        Ok(0) => fail(ERROR_SUCCESS, 0),
        Ok(b) => b,
        Err(e) => fail(e, 0),
    }
}

/// SymUnloadModule64: the module at `base` forgotten; FALSE with
/// ERROR_MOD_NOT_FOUND if it was not loaded.
#[unsafe(no_mangle)]
pub extern "system" fn SymUnloadModule64(_process: usize, base: u64) -> i32 {
    let found = with(|t| match t.iter_mut().find(|m| m.base == base && base != 0) {
        Some(m) => {
            *m = Module { base: 0, size: 0 };
            true
        }
        None => false,
    });
    if found { 1 } else { fail(ERROR_MOD_NOT_FOUND, 0) }
}

/// SYMBOL_INFO's fixed part (the name follows at offset 84).
#[repr(C)]
pub struct SymbolInfo {
    size_of_struct: u32,
    type_index: u32,
    reserved: [u64; 2],
    index: u32,
    size: u32,
    mod_base: u64,
    flags: u32,
    value: u64,
    address: u64,
    register: u32,
    scope: u32,
    tag: u32,
    name_len: u32,
    max_name_len: u32,
    name: [u8; 1],
}

const SYMFLAG_EXPORT: u32 = 0x200;
/// SymTagPublicSymbol.
const SYM_TAG_PUBLIC: u32 = 10;

/// The export of the image at `base` nearest at or below `rva`: its RVA
/// and name (a pointer into the image, NUL-terminated).
fn nearest_export(base: u64, size: u64, rva: u32) -> Option<(u32, u64)> {
    let nt = u64::from(u32_at(base, 0x3c));
    let opt = nt + 24;
    let dirs = match u16_at(base, opt) {
        0x10b => opt + 96,
        0x20b => opt + 112,
        _ => return None,
    };
    let (edir, esize) = (u64::from(u32_at(base, dirs)), u64::from(u32_at(base, dirs + 4)));
    if edir == 0 || edir + esize > size || !readable(base + edir, 40) {
        return None;
    }
    let names = u64::from(u32_at(base, edir + 24));
    let functions = u64::from(u32_at(base, edir + 28));
    let name_table = u64::from(u32_at(base, edir + 32));
    let ordinals = u64::from(u32_at(base, edir + 36));
    let mut best: Option<(u32, u64)> = None;
    for i in 0..names.min(65536) {
        let ordinal = u64::from(u16_at(base, ordinals + i * 2));
        let f = u32_at(base, functions + ordinal * 4);
        // Forwarders point into the export directory: not code.
        if u64::from(f) >= edir && u64::from(f) < edir + esize {
            continue;
        }
        if f <= rva && best.is_none_or(|(b, _)| f > b) {
            best = Some((f, base + u64::from(u32_at(base, name_table + i * 4))));
        }
    }
    best
}

/// SymFromAddr: the symbol at or below `address` in a loaded module
/// (an export: SYMFLAG_EXPORT), its name (truncated to MaxNameLen) and
/// the displacement; FALSE with ERROR_MOD_NOT_FOUND outside the loaded
/// modules, ERROR_INVALID_ADDRESS with no symbol below the address.
#[unsafe(no_mangle)]
pub extern "system" fn SymFromAddr(
    _process: usize,
    address: u64,
    displacement: *mut u64,
    info: *mut SymbolInfo,
) -> i32 {
    // SizeOfStruct must be sizeof(SYMBOL_INFO), 88: else
    // ERROR_INVALID_PARAMETER with the structure untouched (Windows 11
    // 25H2).
    // SAFETY: the caller's SYMBOL_INFO.
    if info.is_null() || unsafe { (*info).size_of_struct } != 88 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let module = with(|t| {
        t.iter().copied().find(|m| m.base != 0 && address >= m.base && address < m.base + m.size)
    });
    let Some(m) = module else { return fail(ERROR_MOD_NOT_FOUND, 0) };
    if mapped_size(m.base).is_none() {
        return fail(ERROR_MOD_NOT_FOUND, 0);
    }
    let Some((rva, name)) = nearest_export(m.base, m.size, (address - m.base) as u32) else {
        return fail(ERROR_INVALID_ADDRESS, 0);
    };
    // SAFETY: a NUL-terminated export name in the mapped image.
    let name = unsafe {
        let mut n = 0;
        while n < 4096 && *((name + n as u64) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(name as *const u8, n)
    };
    // SAFETY: the caller's SYMBOL_INFO with MaxNameLen bytes for the name.
    unsafe {
        let s = &mut *info;
        let room = s.max_name_len as usize;
        s.type_index = 0;
        s.index = 0;
        s.size = 0;
        // An export-table symbol (gala reads no PDB) has no ModBase
        // (Windows 11 25H2: 0, Flags SYMFLAG_EXPORT; Windows' Size, 112
        // for SymFromAddr, is not known from the exports: 0 here).
        s.mod_base = 0;
        s.flags = SYMFLAG_EXPORT;
        s.value = 0;
        s.address = m.base + u64::from(rva);
        s.register = 0;
        s.scope = 0;
        s.tag = SYM_TAG_PUBLIC;
        s.name_len = name.len() as u32;
        if room > 0 {
            let n = name.len().min(room - 1);
            let dst = s.name.as_mut_ptr();
            core::ptr::copy_nonoverlapping(name.as_ptr(), dst, n);
            dst.add(n).write(0);
        }
        if !displacement.is_null() {
            *displacement = address - s.address;
        }
    }
    1
}

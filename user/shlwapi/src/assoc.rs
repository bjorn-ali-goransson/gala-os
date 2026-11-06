//! AssocQueryStringW (file associations from the registry's classes) and
//! SHCreateStreamOnFileEx (an IStream on a file), as documented; see
//! user/shlwapi_test/url.c. advapi32 is found at run time, as for
//! SHDeleteKeyW, so loading shlwapi does not load it.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::paths_more::chars;
use crate::urls::{Buf, give};
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(
        h: usize,
        buf: *const u8,
        n: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn SetFilePointerEx(h: usize, distance: i64, new: *mut i64, method: u32) -> i32;
    fn SetEndOfFile(h: usize) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn GetFileTime(h: usize, created: *mut u64, accessed: *mut u64, written: *mut u64) -> i32;
    fn FlushFileBuffers(h: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000eu32 as i32;
const STG_E_ACCESSDENIED: i32 = 0x8003_0005u32 as i32;
const STG_E_INVALIDFUNCTION: i32 = 0x8003_0001u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_NO_ASSOCIATION).
const NO_ASSOCIATION: i32 = 0x8007_0483u32 as i32;

fn hresult_from_win32(e: u32) -> i32 {
    if e == 0 { E_INVALIDARG } else { (0x8007_0000 | (e & 0xffff)) as i32 }
}

fn unsupported(what: &[u8], detail: u64) {
    crate::strings::unsupported(what, detail);
}

fn wide<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, &c) in w.iter_mut().zip(s) {
        *d = c.into();
    }
    w
}

// ---- Associations ----

type RegOpen = extern "system" fn(usize, *const u16, u32, u32, *mut usize) -> i32;
type RegQuery = extern "system" fn(usize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
type RegClose = extern "system" fn(usize) -> i32;

struct Registry {
    open: RegOpen,
    query: RegQuery,
    close: RegClose,
}

fn registry() -> Option<Registry> {
    let dll = wide(b"advapi32.dll\0");
    // SAFETY: NUL-terminated names; advapi32's documented signatures.
    unsafe {
        let m = LoadLibraryW(dll.as_ptr());
        if m == 0 {
            return None;
        }
        let f = |n: &core::ffi::CStr| {
            let p = GetProcAddress(m, n.as_ptr().cast());
            (p != 0).then_some(p)
        };
        Some(Registry {
            open: core::mem::transmute::<usize, RegOpen>(f(c"RegOpenKeyExW")?),
            query: core::mem::transmute::<usize, RegQuery>(f(c"RegQueryValueExW")?),
            close: core::mem::transmute::<usize, RegClose>(f(c"RegCloseKey")?),
        })
    }
}

const HKEY_CLASSES_ROOT: usize = 0x8000_0000;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const KEY_READ: u32 = 0x20019;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;

/// The string value `name` (None: the default value) of the class key
/// `path` (under the user's classes first, as HKCR merges them, then
/// HKEY_CLASSES_ROOT) into `out`.
fn class_value(r: &Registry, path: &[u16], name: Option<&[u16]>, out: &mut Buf) -> bool {
    let mut full = Buf::new();
    full.extend(&wide(b"Software\\Classes\\")[..17]);
    full.extend(path);
    full.push(0);
    let mut plain = Buf::new();
    plain.extend(path);
    plain.push(0);
    let mut value_name = Buf::new();
    if let Some(n) = name {
        value_name.extend(n);
    }
    value_name.push(0);
    for (root, sub) in [(HKEY_CURRENT_USER, full.text()), (HKEY_CLASSES_ROOT, plain.text())] {
        let mut key = 0;
        if (r.open)(root, sub.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            continue;
        }
        let mut data = [0u16; 1024];
        let (mut kind, mut size) = (0u32, 2048u32);
        let got = (r.query)(
            key,
            value_name.text().as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut size,
        );
        (r.close)(key);
        if got == 0 && (kind == REG_SZ || kind == REG_EXPAND_SZ) {
            let n = (size as usize / 2).min(1024);
            let n = data[..n].iter().position(|&c| c == 0).unwrap_or(n);
            out.extend(&data[..n]);
            return true;
        }
    }
    false
}

const ASSOCSTR_COMMAND: u32 = 1;
const ASSOCSTR_EXECUTABLE: u32 = 2;
const ASSOCSTR_FRIENDLYDOCNAME: u32 = 3;
const ASSOCSTR_CONTENTTYPE: u32 = 14;
const ASSOCSTR_DEFAULTICON: u32 = 15;

/// AssocQueryStringW: what the registry's classes say about an
/// extension (`.txt`) or ProgID: ASSOCSTR_COMMAND (the verb's command,
/// `pszExtra` or the shell key's default verb or "open"),
/// ASSOCSTR_EXECUTABLE (that command's program), ASSOCSTR_FRIENDLYDOCNAME
/// (the ProgID's name), ASSOCSTR_CONTENTTYPE, ASSOCSTR_DEFAULTICON.
/// S_FALSE and the size with no buffer, E_POINTER and the size when it is
/// short, HRESULT_FROM_WIN32(ERROR_NO_ASSOCIATION) when nothing is
/// registered. Other kinds are recorded.
#[unsafe(no_mangle)]
pub extern "system" fn AssocQueryStringW(
    _flags: u32,
    what: u32,
    assoc: *const u16,
    extra: *const u16,
    out: *mut u16,
    count: *mut u32,
) -> i32 {
    if assoc.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let Some(r) = registry() else {
        unsupported(b"shlwapi.dll!AssocQueryStringW(no advapi32)", 0);
        return NO_ASSOCIATION;
    };
    let a = chars(assoc);
    let mut progid = Buf::new();
    if a.first() == Some(&u16::from(b'.')) {
        if what == ASSOCSTR_CONTENTTYPE {
            let mut v = Buf::new();
            let name = wide(b"Content Type");
            return if class_value(&r, a, Some(&name), &mut v) {
                hand(&v, out, count)
            } else {
                NO_ASSOCIATION
            };
        }
        if !class_value(&r, a, None, &mut progid) || progid.text().is_empty() {
            return NO_ASSOCIATION;
        }
    } else {
        progid.extend(a);
    }
    let mut v = Buf::new();
    let found = match what {
        ASSOCSTR_FRIENDLYDOCNAME => class_value(&r, progid.text(), None, &mut v),
        ASSOCSTR_DEFAULTICON => {
            let mut key = Buf::new();
            key.extend(progid.text());
            key.extend(&wide(b"\\DefaultIcon"));
            class_value(&r, key.text(), None, &mut v)
        }
        ASSOCSTR_COMMAND | ASSOCSTR_EXECUTABLE => {
            let mut shell = Buf::new();
            shell.extend(progid.text());
            shell.extend(&wide(b"\\shell"));
            let mut verb = Buf::new();
            let e = chars(extra);
            if !e.is_empty() {
                verb.extend(e);
            } else if !class_value(&r, shell.text(), None, &mut verb) || verb.text().is_empty() {
                verb = Buf::new();
                verb.extend(&wide(b"open"));
            }
            let mut key = Buf::new();
            key.extend(shell.text());
            key.push(u16::from(b'\\'));
            key.extend(verb.text());
            key.extend(&wide(b"\\command"));
            let mut command = Buf::new();
            let ok = class_value(&r, key.text(), None, &mut command);
            if ok && what == ASSOCSTR_EXECUTABLE {
                let c = command.text();
                let program = if c.first() == Some(&u16::from(b'"')) {
                    let end = c[1..]
                        .iter()
                        .position(|&x| x == u16::from(b'"'))
                        .map_or(c.len(), |i| i + 1);
                    &c[1..end]
                } else {
                    let end = c.iter().position(|&x| x == 0x20).unwrap_or(c.len());
                    &c[..end]
                };
                v.extend(program);
            } else {
                v.extend(command.text());
            }
            ok
        }
        _ => {
            unsupported(b"shlwapi.dll!AssocQueryStringW(ASSOCSTR)", what.into());
            false
        }
    };
    if found { hand(&v, out, count) } else { NO_ASSOCIATION }
}

/// AssocQueryString's output: the count includes the NUL.
fn hand(v: &Buf, out: *mut u16, count: *mut u32) -> i32 {
    let needed = v.text().len() as u32 + 1;
    if out.is_null() {
        // SAFETY: the caller's count.
        unsafe { *count = needed };
        return S_FALSE;
    }
    let hr = give(v, out, count, S_OK);
    if hr == E_POINTER {
        return E_POINTER;
    }
    if hr == S_OK {
        // SAFETY: as above.
        unsafe { *count = needed };
    }
    hr
}

// ---- A stream on a file ----

/// The stream object: its vtable, references, the file and its mode.
#[repr(C)]
struct FileStream {
    vtbl: *const StreamVtbl,
    refs: AtomicU32,
    file: usize,
    writable: bool,
    mode: u32,
}

#[repr(C)]
struct StreamVtbl {
    query: extern "system" fn(*mut FileStream, *const [u8; 16], *mut *mut c_void) -> i32,
    add_ref: extern "system" fn(*mut FileStream) -> u32,
    release: extern "system" fn(*mut FileStream) -> u32,
    read: extern "system" fn(*mut FileStream, *mut u8, u32, *mut u32) -> i32,
    write: extern "system" fn(*mut FileStream, *const u8, u32, *mut u32) -> i32,
    seek: extern "system" fn(*mut FileStream, i64, u32, *mut u64) -> i32,
    set_size: extern "system" fn(*mut FileStream, u64) -> i32,
    copy_to: extern "system" fn(*mut FileStream, *mut c_void, u64, *mut u64, *mut u64) -> i32,
    commit: extern "system" fn(*mut FileStream, u32) -> i32,
    revert: extern "system" fn(*mut FileStream) -> i32,
    lock_region: extern "system" fn(*mut FileStream, u64, u64, u32) -> i32,
    unlock_region: extern "system" fn(*mut FileStream, u64, u64, u32) -> i32,
    stat: extern "system" fn(*mut FileStream, *mut StatStg, u32) -> i32,
    clone: extern "system" fn(*mut FileStream, *mut *mut c_void) -> i32,
}

/// STATSTG.
#[repr(C)]
struct StatStg {
    name: *mut u16,
    kind: u32,
    size: u64,
    modified: u64,
    created: u64,
    accessed: u64,
    mode: u32,
    locks: u32,
    clsid: [u8; 16],
    state: u32,
    reserved: u32,
}

static VTBL: StreamVtbl = StreamVtbl {
    query,
    add_ref,
    release,
    read,
    write,
    seek,
    set_size,
    copy_to,
    commit,
    revert,
    lock_region,
    unlock_region,
    stat,
    clone,
};

const IID_IUNKNOWN: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const IID_ISTREAM: [u8; 16] = [0xc, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
/// {0c733a30-2a1c-11ce-ade5-00aa0044773d}
const IID_ISEQUENTIALSTREAM: [u8; 16] = [
    0x30, 0x3a, 0x73, 0x0c, 0x1c, 0x2a, 0xce, 0x11, 0xad, 0xe5, 0x00, 0xaa, 0x00, 0x44, 0x77, 0x3d,
];

extern "system" fn query(
    this: *mut FileStream,
    iid: *const [u8; 16],
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_ISTREAM || iid == IID_ISEQUENTIALSTREAM {
        add_ref(this);
        // SAFETY: the caller's output pointer.
        unsafe { *out = this.cast() };
        return S_OK;
    }
    // SAFETY: as above.
    unsafe { *out = core::ptr::null_mut() };
    E_NOINTERFACE
}

fn me<'a>(this: *mut FileStream) -> &'a FileStream {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &*this }
}

extern "system" fn add_ref(this: *mut FileStream) -> u32 {
    me(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn release(this: *mut FileStream) -> u32 {
    let left = me(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the last reference: our handle and our block.
        unsafe {
            CloseHandle(me(this).file);
            HeapFree(GetProcessHeap(), 0, this.cast());
        }
    }
    left
}

extern "system" fn read(this: *mut FileStream, buf: *mut u8, n: u32, got: *mut u32) -> i32 {
    let mut done = 0u32;
    // SAFETY: the caller's buffer of `n` bytes; our handle.
    if unsafe { ReadFile(me(this).file, buf, n, &mut done, core::ptr::null_mut()) } == 0 {
        // SAFETY: plain call.
        return hresult_from_win32(unsafe { GetLastError() });
    }
    if !got.is_null() {
        // SAFETY: the caller's count.
        unsafe { *got = done };
    }
    S_OK
}

extern "system" fn write(this: *mut FileStream, buf: *const u8, n: u32, put: *mut u32) -> i32 {
    if !me(this).writable {
        return STG_E_ACCESSDENIED;
    }
    let mut done = 0u32;
    // SAFETY: the caller's `n` bytes; our handle.
    if unsafe { WriteFile(me(this).file, buf, n, &mut done, core::ptr::null_mut()) } == 0 {
        // SAFETY: plain call.
        return hresult_from_win32(unsafe { GetLastError() });
    }
    if !put.is_null() {
        // SAFETY: the caller's count.
        unsafe { *put = done };
    }
    S_OK
}

extern "system" fn seek(this: *mut FileStream, distance: i64, origin: u32, new: *mut u64) -> i32 {
    if origin > 2 {
        return STG_E_INVALIDFUNCTION;
    }
    let mut at = 0i64;
    // SAFETY: our handle (STREAM_SEEK_* are FILE_BEGIN/CURRENT/END).
    if unsafe { SetFilePointerEx(me(this).file, distance, &mut at, origin) } == 0 {
        return STG_E_INVALIDFUNCTION;
    }
    if !new.is_null() {
        // SAFETY: the caller's position.
        unsafe { *new = at as u64 };
    }
    S_OK
}

extern "system" fn set_size(this: *mut FileStream, size: u64) -> i32 {
    if !me(this).writable {
        return STG_E_ACCESSDENIED;
    }
    let f = me(this).file;
    let mut at = 0i64;
    // SAFETY: our handle; the position is put back.
    unsafe {
        if SetFilePointerEx(f, 0, &mut at, 1) == 0
            || SetFilePointerEx(f, size as i64, core::ptr::null_mut(), 0) == 0
            || SetEndOfFile(f) == 0
        {
            return hresult_from_win32(GetLastError());
        }
        SetFilePointerEx(f, at, core::ptr::null_mut(), 0);
    }
    S_OK
}

extern "system" fn copy_to(
    _: *mut FileStream,
    _: *mut c_void,
    _: u64,
    _: *mut u64,
    _: *mut u64,
) -> i32 {
    unsupported(b"shlwapi.dll!SHCreateStreamOnFileEx(IStream::CopyTo)", 0);
    E_NOTIMPL
}

extern "system" fn commit(this: *mut FileStream, _flags: u32) -> i32 {
    // SAFETY: our handle.
    unsafe { FlushFileBuffers(me(this).file) };
    S_OK
}

extern "system" fn revert(_: *mut FileStream) -> i32 {
    // A file stream is not transacted: nothing to revert.
    S_OK
}

extern "system" fn lock_region(_: *mut FileStream, _: u64, _: u64, _: u32) -> i32 {
    unsupported(b"shlwapi.dll!SHCreateStreamOnFileEx(IStream::LockRegion)", 0);
    STG_E_INVALIDFUNCTION
}

extern "system" fn unlock_region(_: *mut FileStream, _: u64, _: u64, _: u32) -> i32 {
    STG_E_INVALIDFUNCTION
}

extern "system" fn stat(this: *mut FileStream, out: *mut StatStg, flags: u32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    let f = me(this).file;
    let (mut size, mut c, mut a, mut w) = (0i64, 0u64, 0u64, 0u64);
    // SAFETY: our handle and locals.
    unsafe {
        GetFileSizeEx(f, &mut size);
        GetFileTime(f, &mut c, &mut a, &mut w);
    }
    const STATFLAG_NONAME: u32 = 1;
    if flags & STATFLAG_NONAME == 0 {
        // The name would come from the task allocator (ole32).
        unsupported(b"shlwapi.dll!SHCreateStreamOnFileEx(IStream::Stat name)", 0);
    }
    let st = StatStg {
        name: core::ptr::null_mut(),
        kind: 2, // STGTY_STREAM
        size: size as u64,
        modified: w,
        created: c,
        accessed: a,
        mode: me(this).mode,
        locks: 0,
        clsid: [0; 16],
        state: 0,
        reserved: 0,
    };
    // SAFETY: the caller's STATSTG.
    unsafe { out.write(st) };
    S_OK
}

extern "system" fn clone(_: *mut FileStream, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *out = core::ptr::null_mut() };
    }
    unsupported(b"shlwapi.dll!SHCreateStreamOnFileEx(IStream::Clone)", 0);
    E_NOTIMPL
}

const STGM_WRITE: u32 = 1;
const STGM_READWRITE: u32 = 2;
const STGM_SHARE_EXCLUSIVE: u32 = 0x10;
const STGM_SHARE_DENY_WRITE: u32 = 0x20;
const STGM_SHARE_DENY_READ: u32 = 0x30;
const STGM_CREATE: u32 = 0x1000;
const STGM_TRANSACTED: u32 = 0x0001_0000;

/// SHCreateStreamOnFileEx: an IStream on the file `path` (grfMode's
/// access and sharing; STGM_CREATE replaces the file, `create` without
/// it makes a new one, else it must exist). The file's error as an
/// HRESULT when it cannot be opened. Transacted mode, CopyTo, Clone and
/// region locks are not supported (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateStreamOnFileEx(
    path: *const u16,
    mode: u32,
    attributes: u32,
    create: i32,
    _template: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output pointer.
    unsafe { *out = core::ptr::null_mut() };
    if path.is_null() {
        return E_INVALIDARG;
    }
    if mode & STGM_TRANSACTED != 0 {
        unsupported(b"shlwapi.dll!SHCreateStreamOnFileEx(STGM_TRANSACTED)", mode.into());
    }
    let (access, writable) = match mode & 3 {
        STGM_WRITE => (0x4000_0000u32, true),
        STGM_READWRITE => (0xc000_0000u32, true),
        _ => (0x8000_0000u32, false),
    };
    let share = match mode & 0x70 {
        STGM_SHARE_EXCLUSIVE => 0,
        STGM_SHARE_DENY_WRITE => 1,
        STGM_SHARE_DENY_READ => 2,
        _ => 7,
    };
    let disposition = if mode & STGM_CREATE != 0 {
        2 // CREATE_ALWAYS
    } else if create != 0 {
        1 // CREATE_NEW
    } else {
        3 // OPEN_EXISTING
    };
    let attributes = if attributes == 0 { 0x80 } else { attributes };
    // SAFETY: the caller's NUL-terminated path.
    let h =
        unsafe { CreateFileW(path, access, share, core::ptr::null(), disposition, attributes, 0) };
    if h == usize::MAX {
        // SAFETY: plain call.
        return hresult_from_win32(unsafe { GetLastError() });
    }
    // SAFETY: plain call; the block is ours.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<FileStream>()) }
        as *mut FileStream;
    if p.is_null() {
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        return E_OUTOFMEMORY;
    }
    // SAFETY: a new block of the object's size.
    unsafe {
        p.write(FileStream { vtbl: &VTBL, refs: AtomicU32::new(1), file: h, writable, mode });
        *out = p.cast();
    }
    S_OK
}

const STGM_CONVERT: u32 = 0x0002_0000;
const STGM_DELETEONRELEASE: u32 = 0x0400_0000;

/// SHCreateStreamOnFileW: SHCreateStreamOnFileEx with no attributes and
/// no `create` (STGM_CREATE replaces the file; without it the file must
/// exist). STGM_CONVERT, STGM_DELETEONRELEASE and STGM_TRANSACTED are
/// E_INVALIDARG (Wine's istream.c tests, checked on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateStreamOnFileW(
    path: *const u16,
    mode: u32,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    if mode & (STGM_CONVERT | STGM_DELETEONRELEASE | STGM_TRANSACTED) != 0 {
        // SAFETY: the caller's output pointer.
        unsafe { *out = core::ptr::null_mut() };
        return E_INVALIDARG;
    }
    SHCreateStreamOnFileEx(path, mode, 0, 0, core::ptr::null_mut(), out)
}

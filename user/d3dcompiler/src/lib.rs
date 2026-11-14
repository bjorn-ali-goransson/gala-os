//! gala's d3dcompiler_47.dll (the HLSL compiler's DLL; ROADMAP GPU G2):
//! D3DCreateBlob works (an ID3DBlob of the size asked, zeroed); there is
//! no HLSL compiler yet, so D3DCompile, D3DCompile2 and
//! D3DCompileFromFile fail as documented for a compile that cannot be
//! done: an error HRESULT (E_NOTIMPL), no code blob, an error-message
//! blob saying why, the call in the ledger with its target profile
//! (`d3dcompiler_47.dll!D3DCompile(ps_4_0)`). The decision (ROADMAP G2):
//! compiled shaders need a Direct3D device, which gala does not give yet
//! (`d3d11.dll` fails with DXGI_ERROR_UNSUPPORTED), so a compiler
//! comes with DXVK; until then a program that loads this DLL starts, and
//! one that compiles learns that it cannot (Office's Click-to-Run UI
//! imports D3DCompile).
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

include!("../../gpu_common.rs");

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

const S_OK: i32 = 0;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000eu32 as i32;

type Guid = [u8; 16];
const IID_IUNKNOWN: Guid = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
/// IID_ID3DBlob (= IID_ID3D10Blob): {8BA5FB08-5195-40E2-AC58-0D989C3A0102}.
const IID_ID3DBLOB: Guid = [
    0x08, 0xfb, 0xa5, 0x8b, 0x95, 0x51, 0xe2, 0x40, 0xac, 0x58, 0x0d, 0x98, 0x9c, 0x3a, 0x01, 0x02,
];

/// An ID3DBlob: its vtable, a count, the size, the bytes after it.
#[repr(C)]
struct Blob {
    vtable: *const *const (),
    refs: AtomicU32,
    size: usize,
}

struct VTable([*const (); 5]);
// SAFETY: function addresses only.
unsafe impl Sync for VTable {}

static VTABLE: VTable = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    pointer as *const (),
    size as *const (),
]);

fn new_blob(size: usize) -> *mut Blob {
    let Some(total) = size.checked_add(size_of::<Blob>()) else { return core::ptr::null_mut() };
    // SAFETY: a plain call; HEAP_ZERO_MEMORY.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0x8, total) }.cast::<Blob>();
    if !p.is_null() {
        // SAFETY: a new block that holds a Blob.
        unsafe { p.write(Blob { vtable: VTABLE.0.as_ptr(), refs: AtomicU32::new(1), size }) };
    }
    p
}

extern "system" fn query(p: *mut Blob, iid: *const Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() || iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let id = unsafe { *iid };
    if id == IID_IUNKNOWN || id == IID_ID3DBLOB {
        add_ref(p);
        // SAFETY: the caller's output pointer.
        unsafe { *out = p.cast() };
        return S_OK;
    }
    clear(out);
    E_NOINTERFACE
}

extern "system" fn add_ref(p: *mut Blob) -> u32 {
    // SAFETY: a blob of ours.
    unsafe { (*p).refs.fetch_add(1, Ordering::AcqRel) + 1 }
}

extern "system" fn release(p: *mut Blob) -> u32 {
    // SAFETY: a blob of ours, freed with its last reference.
    unsafe {
        let left = (*p).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            HeapFree(GetProcessHeap(), 0, p.cast());
        }
        left
    }
}

extern "system" fn pointer(p: *mut Blob) -> *mut u8 {
    // SAFETY: the bytes follow the header.
    unsafe { p.cast::<u8>().add(size_of::<Blob>()) }
}

extern "system" fn size(p: *mut Blob) -> usize {
    // SAFETY: a blob of ours.
    unsafe { (*p).size }
}

/// D3DCreateBlob(Size, ppBlob): a zeroed blob of `size` bytes.
#[unsafe(no_mangle)]
pub extern "system" fn D3DCreateBlob(size: usize, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let b = new_blob(size);
    // SAFETY: the caller's output pointer.
    unsafe { *out = b.cast() };
    if b.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// The failure of every compile: no code, the reason as the error
/// messages (NUL-terminated text, as the compiler's), the ledger told the
/// target profile.
fn cannot_compile(target: *const u8, code: *mut *mut c_void, errors: *mut *mut c_void) -> i32 {
    clear(code);
    let mut name = [0u8; 64];
    let mut n = 0;
    for &b in b"d3dcompiler_47.dll!D3DCompile(" {
        name[n] = b;
        n += 1;
    }
    if !target.is_null() {
        for i in 0..16 {
            // SAFETY: the caller's NUL-terminated profile name.
            let c = unsafe { *target.add(i) };
            if c == 0 {
                break;
            }
            name[n] = c;
            n += 1;
        }
    }
    name[n] = b')';
    unsupported(&name[..n + 1]);
    if !errors.is_null() {
        const TEXT: &[u8] = b"gala: no HLSL compiler yet (d3dcompiler_47.dll, ROADMAP GPU G2)\0";
        let b = new_blob(TEXT.len());
        if !b.is_null() {
            // SAFETY: the blob holds TEXT.len() bytes.
            unsafe { memcpy(pointer(b), TEXT.as_ptr(), TEXT.len()) };
        }
        // SAFETY: the caller's output pointer.
        unsafe { *errors = b.cast() };
    }
    E_NOTIMPL
}

/// D3DCompile(pSrcData, SrcDataSize, pSourceName, pDefines, pInclude,
/// pEntrypoint, pTarget, Flags1, Flags2, ppCode, ppErrorMsgs).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn D3DCompile(
    _src: *const c_void,
    _len: usize,
    _source_name: *const u8,
    _defines: *const c_void,
    _include: *mut c_void,
    _entry: *const u8,
    target: *const u8,
    _flags1: u32,
    _flags2: u32,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> i32 {
    cannot_compile(target, code, errors)
}

/// D3DCompile2: D3DCompile with secondary data.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn D3DCompile2(
    _src: *const c_void,
    _len: usize,
    _source_name: *const u8,
    _defines: *const c_void,
    _include: *mut c_void,
    _entry: *const u8,
    target: *const u8,
    _flags1: u32,
    _flags2: u32,
    _secondary_flags: u32,
    _secondary: *const c_void,
    _secondary_len: usize,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> i32 {
    cannot_compile(target, code, errors)
}

/// D3DCompileFromFile(pFileName, pDefines, pInclude, pEntrypoint,
/// pTarget, Flags1, Flags2, ppCode, ppErrorMsgs).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn D3DCompileFromFile(
    _file: *const u16,
    _defines: *const c_void,
    _include: *mut c_void,
    _entry: *const u8,
    target: *const u8,
    _flags1: u32,
    _flags2: u32,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> i32 {
    cannot_compile(target, code, errors)
}

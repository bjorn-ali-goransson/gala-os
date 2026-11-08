//! IsOS, QISearch and SHCreateMemStream (shlwapi.h), which Chromium
//! imports by ordinal.

const OS_NT: u32 = 1;
const OS_WIN2000ORGREATER: u32 = 7;
const OS_XPORGREATER: u32 = 18;

/// IsOS(dwOS): whether the system is of the kind asked. gala is a
/// Windows NT client (10/11 compatible): OS_NT, OS_WIN2000ORGREATER and
/// OS_XPORGREATER hold; the 9x, server, domain-member, WOW64 and other
/// kinds do not.
#[unsafe(no_mangle)]
pub extern "system" fn IsOS(os: u32) -> i32 {
    i32::from(matches!(os, OS_NT | OS_WIN2000ORGREATER | OS_XPORGREATER))
}

const E_POINTER: i32 = 0x8000_4003_u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
/// IID_IUnknown {00000000-0000-0000-C000-000000000046}.
const IID_IUNKNOWN: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];

/// QISearch(that, pqit, riid, ppv): QueryInterface from a table of
/// QITAB { const IID* piid; DWORD dwOffset } ending with a null piid: the
/// interface at `that + dwOffset` of the matching entry (the first entry's
/// for IID_IUnknown), AddRef'd; E_NOINTERFACE otherwise.
#[repr(C)]
pub struct QiTab {
    iid: *const [u8; 16],
    offset: u32,
}

#[unsafe(no_mangle)]
pub extern "system" fn QISearch(
    that: *mut u8,
    table: *const QiTab,
    riid: *const [u8; 16],
    ppv: *mut *mut u8,
) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID, table and object.
    unsafe {
        *ppv = core::ptr::null_mut();
        let want = *riid;
        let mut entry = table;
        let mut first = true;
        loop {
            let piid = core::ptr::addr_of!((*entry).iid).read_unaligned();
            if piid.is_null() {
                return E_NOINTERFACE;
            }
            if *piid == want || (first && want == IID_IUNKNOWN) {
                let offset = core::ptr::addr_of!((*entry).offset).read_unaligned() as usize;
                let object = that.add(offset);
                // IUnknown::AddRef is the second vtable slot.
                let vtable = object.cast::<*const usize>().read();
                let add_ref: extern "system" fn(*mut u8) -> u32 =
                    core::mem::transmute(*vtable.add(1));
                add_ref(object);
                *ppv = object;
                return 0;
            }
            first = false;
            entry = entry.add(1);
        }
    }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

/// ole32's CreateStreamOnHGlobal, loaded when first needed (a static
/// import would make every program that loads shlwapi load ole32).
fn create_stream(out: *mut *mut u8) -> i32 {
    const OLE32: [u16; 10] = [111, 108, 101, 51, 50, 46, 100, 108, 108, 0]; // "ole32.dll"
    // SAFETY: NUL-terminated names; ole32's CreateStreamOnHGlobal(HGLOBAL,
    // BOOL, IStream**).
    unsafe {
        let module = LoadLibraryW(OLE32.as_ptr());
        let f = if module == 0 {
            0
        } else {
            GetProcAddress(module, c"CreateStreamOnHGlobal".as_ptr().cast())
        };
        if f == 0 {
            return E_NOINTERFACE;
        }
        let f: extern "system" fn(usize, i32, *mut *mut u8) -> i32 = core::mem::transmute(f);
        f(0, 1, out)
    }
}

/// SHCreateMemStream(data, size): a memory IStream holding a copy of the
/// `size` bytes at `data` (empty for NULL), positioned at its start; NULL
/// on failure (ole32's memory stream).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateMemStream(data: *const u8, size: u32) -> *mut u8 {
    let mut stream = core::ptr::null_mut();
    if create_stream(&mut stream) < 0 || stream.is_null() {
        return core::ptr::null_mut();
    }
    // IStream: Release is slot 2, Write 4, Seek 5.
    // SAFETY: a live IStream; the caller's `size` bytes.
    unsafe {
        let vtable = stream.cast::<*const usize>().read();
        let release: extern "system" fn(*mut u8) -> u32 = core::mem::transmute(*vtable.add(2));
        let write: extern "system" fn(*mut u8, *const u8, u32, *mut u32) -> i32 =
            core::mem::transmute(*vtable.add(4));
        let seek: extern "system" fn(*mut u8, i64, u32, *mut u64) -> i32 =
            core::mem::transmute(*vtable.add(5));
        if !data.is_null() && size > 0 {
            let mut written = 0;
            if write(stream, data, size, &mut written) < 0 || written != size {
                release(stream);
                return core::ptr::null_mut();
            }
        }
        if seek(stream, 0, 0, core::ptr::null_mut()) < 0 {
            release(stream);
            return core::ptr::null_mut();
        }
    }
    stream
}

// PathFindExtensionW and PathRemoveExtensionW live in paths_more.rs.

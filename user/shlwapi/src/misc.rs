//! IsOS and QISearch (shlwapi.h), which Chromium imports by ordinal.

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
#[unsafe(no_mangle)]
pub extern "system" fn QISearch(
    that: *mut u8,
    table: *const u8,
    riid: *const [u8; 16],
    ppv: *mut *mut u8,
) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID, table (16-byte entries) and object.
    unsafe {
        *ppv = core::ptr::null_mut();
        let want = *riid;
        let mut entry = table;
        let mut first = true;
        loop {
            let piid = entry.cast::<*const [u8; 16]>().read_unaligned();
            if piid.is_null() {
                return E_NOINTERFACE;
            }
            if *piid == want || (first && want == IID_IUNKNOWN) {
                let offset = entry.add(8).cast::<u32>().read_unaligned() as usize;
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
            entry = entry.add(16);
        }
    }
}

// PathFindExtensionW and PathRemoveExtensionW live in paths_more.rs.

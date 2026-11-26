//! COM basics the .NET runtime imports (ole32 and oleaut32 by ordinal),
//! with the results real Windows gives (documented contracts) on a
//! machine with no COM classes or type libraries registered for these
//! GUIDs:
//!
//! - ole32: CoInitializeEx (S_OK, then S_FALSE; RPC_E_CHANGED_MODE for the
//!   other apartment kind), CoUninitialize, CoInitializeSecurity (once:
//!   then RPC_E_TOO_LATE), initialize spies (called
//!   around initialization with the apartment reference counts),
//!   CoGetMalloc (the task allocator), GUID strings (CLSIDFromString,
//!   IIDFromString, StringFromCLSID/IID), CoGetContextToken (CO_E_NOTINITIALIZED outside an apartment),
//!   CoGetObjectContext and CoGetApartmentType (MTA; on a thread that
//!   never initialized COM while the MTA exists: MTA, qualifier
//!   APTTYPEQUALIFIER_IMPLICIT_MTA; CO_E_NOTINITIALIZED with no MTA),
//!   the thread's OLE data (TEB.ReservedForOle) carrying the apartment
//!   flags where programs read them (0x140 MTA, 0x80 STA, cleared when
//!   the thread leaves),
//!   CoTaskMem*,
//!   CoCreateGuid (random version-4 GUIDs), CoGetClassObject
//!   (REGDB_E_CLASSNOTREG), CLSIDFromProgID (CO_E_CLASSSTRING),
//!   ProgIDFromCLSID (REGDB_E_CLASSNOTREG, the output null),
//!   CLSID_GlobalOptions (the class COM serves out of itself with no
//!   registry key: in-process contexts only, an IClassFactory whose
//!   instances are IGlobalOptions over per-process state),
//!   ProgIDFromCLSID (REGDB_E_CLASSNOTREG, the output null),
//!   CreateStreamOnHGlobal (an IStream: write, seek, read, stat),
//!   CoCreateFreeThreadedMarshaler (IMarshal), CoWaitForMultipleHandles
//!   (RPC_S_CALLPENDING on timeout).
//! - oleaut32: BSTRs (length prefix, byte strings), VARIANTs (VariantClear
//!   frees, VariantChangeType(Ex) between numbers and strings),
//!   SAFEARRAYs (vectors, bounds, elements, the stored VARTYPE,
//!   descriptors), Get/SetErrorInfo, VarCyFromDec, type libraries that are
//!   not registered (TYPE_E_LIBNOTREGISTERED), the number conversions
//!   Office's wwlib imports (VarR4FromR8 and VarR8FromR4 with their
//!   rounding and overflow, VarBstrFromI4, VarBstrFromR4 and
//!   VarBstrFromR8 with Automation's fixed-or-scientific rule) and the
//!   process's running objects (RegisterActiveObject,
//!   RevokeActiveObject, GetActiveObject), each documented where it is
//!   checked.
//!
//! Prints `com_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type HResult = i32;
type Guid = [u8; 16];

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
}

/// What an uninitialized thread's CoGetApartmentType gave: the HRESULT,
/// the type and the qualifier.
static mut IMPLICIT: [i32; 3] = [0; 3];

extern "system" fn implicit_thread(_: *mut c_void) -> u32 {
    let (mut kind, mut qualifier) = (-1, -1);
    // SAFETY: plain calls; the static is read after the thread ends.
    unsafe {
        let hr = CoGetApartmentType(&mut kind, &mut qualifier);
        IMPLICIT = [hr, kind, qualifier];
    }
    0
}

/// The apartment flags in the thread's OLE data (TEB.ReservedForOle, x64
/// 0x1758: two pointers, the apartment id, then the flags, as Chromium's
/// base::win::GetComApartmentTypeForThread reads them): 0x80 STA, 0x140
/// MTA; 0 with no OLE data.
fn ole_flags() -> u32 {
    let teb = teb_field(0x30);
    // SAFETY: TEB+0x30 is the TEB's self pointer; the OLE data, when
    // there is some, is ole32's block of at least 24 bytes.
    unsafe {
        let ole = *((teb + 0x1758) as *const usize);
        if ole == 0 { 0 } else { *((ole + 20) as *const u32) }
    }
}

/// An STA thread's flags: [after CoInitializeEx(STA), after
/// CoUninitialize].
static mut STA_FLAGS: [u32; 2] = [0; 2];

extern "system" fn sta_thread(_: *mut c_void) -> u32 {
    // SAFETY: plain calls; the static is read after the thread ends.
    unsafe {
        CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        let inside = ole_flags();
        CoUninitialize();
        STA_FLAGS = [inside, ole_flags()];
    }
    0
}

/// CoGetApartmentType on a new thread that never initializes COM.
fn on_uninitialized_thread() -> [i32; 3] {
    // SAFETY: plain calls; the thread has ended when the static is read.
    unsafe {
        let t = CreateThread(
            core::ptr::null_mut(),
            0,
            implicit_thread,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        if t.is_null() {
            return [-1; 3];
        }
        WaitForSingleObject(t, 0xffff_ffff);
        CloseHandle(t);
        IMPLICIT
    }
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoUninitialize();
    fn CoInitializeSecurity(
        descriptor: *mut c_void,
        auth_count: i32,
        auth_services: *mut c_void,
        reserved1: *mut c_void,
        authn_level: u32,
        imp_level: u32,
        auth_list: *mut c_void,
        capabilities: u32,
        reserved3: *mut c_void,
    ) -> HResult;
    fn CoRegisterInitializeSpy(spy: *mut c_void, cookie: *mut u64) -> HResult;
    fn CoRevokeInitializeSpy(cookie: u64) -> HResult;
    fn CoGetContextToken(token: *mut usize) -> HResult;
    fn CoGetObjectContext(iid: *const Guid, out: *mut *mut c_void) -> HResult;
    fn CoGetApartmentType(kind: *mut i32, qualifier: *mut i32) -> HResult;
    fn CoTaskMemAlloc(size: usize) -> *mut u8;
    fn CoTaskMemRealloc(p: *mut u8, size: usize) -> *mut u8;
    fn CoTaskMemFree(p: *mut u8);
    fn CoGetMalloc(context: u32, malloc: *mut *mut c_void) -> HResult;
    fn CoCreateGuid(guid: *mut Guid) -> HResult;
    fn CLSIDFromString(text: *const u16, clsid: *mut Guid) -> HResult;
    fn IIDFromString(text: *const u16, iid: *mut Guid) -> HResult;
    fn StringFromCLSID(clsid: *const Guid, text: *mut *mut u16) -> HResult;
    fn StringFromIID(iid: *const Guid, text: *mut *mut u16) -> HResult;
    fn CoGetClassObject(
        clsid: *const Guid,
        context: u32,
        info: *mut c_void,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
    fn CLSIDFromProgID(progid: *const u16, clsid: *mut Guid) -> HResult;
    fn ProgIDFromCLSID(clsid: *const Guid, progid: *mut *mut u16) -> HResult;
    fn CreateStreamOnHGlobal(global: Handle, delete: Bool, out: *mut *mut c_void) -> HResult;
    fn CoCreateFreeThreadedMarshaler(outer: *mut c_void, out: *mut *mut c_void) -> HResult;
    fn CoWaitForMultipleHandles(
        flags: u32,
        ms: u32,
        count: u32,
        handles: *const Handle,
        index: *mut u32,
    ) -> HResult;
}

#[link(name = "oleaut32", kind = "raw-dylib")]
unsafe extern "system" {
    #[link_ordinal(2)]
    fn SysAllocString(s: *const u16) -> *mut u16;
    #[link_ordinal(4)]
    fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
    #[link_ordinal(6)]
    fn SysFreeString(s: *mut u16);
    #[link_ordinal(7)]
    fn SysStringLen(s: *const u16) -> u32;
    #[link_ordinal(149)]
    fn SysStringByteLen(s: *const u16) -> u32;
    #[link_ordinal(150)]
    fn SysAllocStringByteLen(s: *const u8, len: u32) -> *mut u16;
    #[link_ordinal(8)]
    fn VariantInit(v: *mut Variant);
    #[link_ordinal(9)]
    fn VariantClear(v: *mut Variant) -> HResult;
    #[link_ordinal(12)]
    fn VariantChangeType(dst: *mut Variant, src: *const Variant, flags: u16, vt: u16) -> HResult;
    #[link_ordinal(147)]
    fn VariantChangeTypeEx(
        dst: *mut Variant,
        src: *const Variant,
        lcid: u32,
        flags: u16,
        vt: u16,
    ) -> HResult;
    #[link_ordinal(411)]
    fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> *mut SafeArray;
    #[link_ordinal(16)]
    fn SafeArrayDestroy(sa: *mut SafeArray) -> HResult;
    #[link_ordinal(17)]
    fn SafeArrayGetDim(sa: *const SafeArray) -> u32;
    #[link_ordinal(18)]
    fn SafeArrayGetElemsize(sa: *const SafeArray) -> u32;
    #[link_ordinal(20)]
    fn SafeArrayGetLBound(sa: *const SafeArray, dim: u32, bound: *mut i32) -> HResult;
    #[link_ordinal(19)]
    fn SafeArrayGetUBound(sa: *const SafeArray, dim: u32, bound: *mut i32) -> HResult;
    #[link_ordinal(26)]
    fn SafeArrayPutElement(sa: *mut SafeArray, index: *const i32, value: *const c_void) -> HResult;
    #[link_ordinal(77)]
    fn SafeArrayGetVartype(sa: *const SafeArray, vt: *mut u16) -> HResult;
    #[link_ordinal(41)]
    fn SafeArrayAllocDescriptorEx(vt: u16, dims: u32, out: *mut *mut SafeArray) -> HResult;
    #[link_ordinal(37)]
    fn SafeArrayAllocData(sa: *mut SafeArray) -> HResult;
    #[link_ordinal(200)]
    fn GetErrorInfo(reserved: u32, out: *mut *mut c_void) -> HResult;
    #[link_ordinal(201)]
    fn SetErrorInfo(reserved: u32, info: *mut c_void) -> HResult;
    #[link_ordinal(228)]
    fn VarCyFromDec(dec: *const Decimal, out: *mut i64) -> HResult;
    #[link_ordinal(71)]
    fn VarR4FromR8(v: f64, out: *mut f32) -> HResult;
    #[link_ordinal(81)]
    fn VarR8FromR4(v: f32, out: *mut f64) -> HResult;
    #[link_ordinal(110)]
    fn VarBstrFromI4(v: i32, lcid: u32, flags: u32, out: *mut *mut u16) -> HResult;
    #[link_ordinal(111)]
    fn VarBstrFromR4(v: f32, lcid: u32, flags: u32, out: *mut *mut u16) -> HResult;
    #[link_ordinal(112)]
    fn VarBstrFromR8(v: f64, lcid: u32, flags: u32, out: *mut *mut u16) -> HResult;
    #[link_ordinal(33)]
    fn RegisterActiveObject(
        unk: *mut c_void,
        clsid: *const Guid,
        flags: u32,
        cookie: *mut u32,
    ) -> HResult;
    #[link_ordinal(34)]
    fn RevokeActiveObject(cookie: u32, reserved: *mut c_void) -> HResult;
    #[link_ordinal(35)]
    fn GetActiveObject(clsid: *const Guid, reserved: *mut c_void, out: *mut *mut c_void)
    -> HResult;
    #[link_ordinal(162)]
    fn LoadRegTypeLib(
        guid: *const Guid,
        major: u16,
        minor: u16,
        lcid: u32,
        out: *mut *mut c_void,
    ) -> HResult;
    #[link_ordinal(164)]
    fn QueryPathOfRegTypeLib(
        guid: *const Guid,
        major: u16,
        minor: u16,
        lcid: u32,
        path: *mut *mut u16,
    ) -> HResult;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// VARIANT: vt, three reserved words, then the value (16 bytes).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: [u64; 2],
}

/// SAFEARRAY with one bound.
#[repr(C)]
struct SafeArray {
    dims: u16,
    features: u16,
    element_size: u32,
    locks: u32,
    data: *mut c_void,
    count: u32,
    lower: i32,
}

/// DECIMAL.
#[repr(C)]
struct Decimal {
    reserved: u16,
    scale: u8,
    sign: u8,
    hi: u32,
    lo: u64,
}

const S_OK: u64 = 0;
const S_FALSE: u64 = 1;
const E_INVALIDARG: u64 = 0x8007_0057;
const DISP_E_OVERFLOW: u64 = 0x8002_000a;
const CO_E_NOTINITIALIZED: u64 = 0x8004_01f0;
const MK_E_UNAVAILABLE: u64 = 0x8004_01e3;
const MK_S_MONIKERALREADYREGISTERED: u64 = 0x0004_01e7;
/// en-US, the one locale gala formats numbers in.
const LCID_EN_US: u32 = 0x409;
const COINIT_MULTITHREADED: u32 = 0;
const COINIT_APARTMENTTHREADED: u32 = 2;
const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_R8: u16 = 5;
const VT_BSTR: u16 = 8;

/// An HRESULT as the checks compare it.
fn hr(v: HResult) -> u64 {
    u64::from(v as u32)
}

/// {00000000-0000-0000-C000-000000000046} and friends: IIDs whose last
/// eight bytes are C0 00 00 00 00 00 00 46.
const fn ole_iid(data1: u32) -> Guid {
    let d = data1.to_le_bytes();
    [d[0], d[1], d[2], d[3], 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]
}

const IID_IUNKNOWN: Guid = ole_iid(0);
const IID_IMARSHAL: Guid = ole_iid(3);
const IID_IINITIALIZESPY: Guid = ole_iid(0x34);
const IID_ICOMTHREADINGINFO: Guid = ole_iid(0x1ce);
/// A GUID nothing registers.
const NOBODY: Guid = *b"gala-no-such-cls";
/// Two CLSIDs of this test's own, for the running objects.
const CLSID_RUNNING: Guid = *b"gala-running-obj";
const CLSID_OTHER: Guid = *b"gala-other-objct";

/// CLSID_GlobalOptions: a class COM serves out of itself, with no
/// registry key of any kind. `cguid.h` declares `CLSID_GlobalOptions`
/// and `uuid.lib` carries the value; `objidlbase.h` declares
/// `IGlobalOptions` as `MIDL_INTERFACE("0000015B-0000-0000-C000-...")`.
const CLSID_GLOBALOPTIONS: Guid = ole_iid(0x34b);
const IID_IGLOBALOPTIONS: Guid = ole_iid(0x15b);
const IID_ICLASSFACTORY: Guid = ole_iid(1);

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex64(v: u64) -> [u8; 16] {
    let mut s = [0u8; 16];
    for (i, c) in s.iter_mut().enumerate() {
        let d = ((v >> (60 - 4 * i)) & 0xf) as u8;
        *c = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    s
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"com_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex64(got));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        if got != want {
            out(b"com_test: want ");
            out(&hex64(want));
            out(b"
");
        }
        self.ok(got == want, got)
    }

    /// The UTF-16 text at `p` is `want` (not necessarily terminated).
    #[track_caller]
    fn text(&mut self, p: *const u16, want: &str) -> Result<(), u32> {
        // SAFETY: at least `want.len()` characters at `p`.
        let same = unsafe { want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) };
        // SAFETY: as above.
        self.ok(same, unsafe { *p }.into())
    }
}

fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

/// The COM object's vtable entry `i` called as `fn(this, args...)`.
macro_rules! call {
    ($obj:expr, $i:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj as *mut *const usize;
        let f: extern "system" fn(*mut *const usize $(, $t)*) -> $r =
            core::mem::transmute(*(*obj).add($i));
        f(obj $(, $a)*)
    }};
}

fn release(obj: *mut c_void) -> u32 {
    // SAFETY: a COM object; Release is vtable entry 2.
    unsafe { call!(obj, 2, fn() -> u32) }
}

fn query(obj: *mut c_void, iid: &Guid, out: *mut *mut c_void) -> HResult {
    // SAFETY: a COM object; QueryInterface is vtable entry 0.
    unsafe { call!(obj, 0, fn(*const Guid, *mut *mut c_void) -> HResult, iid, out) }
}

// ---- An IInitializeSpy that records what it is told ----

#[repr(C)]
struct Spy {
    vtable: *const usize,
    refs: u32,
    pre_init: u32,
    post_init: u32,
    pre_uninit: u32,
    post_uninit: u32,
    last_refs: u32,
}

extern "system" fn spy_query(this: *mut Spy, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    // SAFETY: COM's contract: valid IID and output pointers.
    unsafe {
        if *iid == IID_IUNKNOWN || *iid == IID_IINITIALIZESPY {
            (*this).refs += 1;
            *out = this.cast();
            return 0;
        }
        *out = core::ptr::null_mut();
    }
    0x8000_4002u32 as i32 // E_NOINTERFACE
}

extern "system" fn spy_add_ref(this: *mut Spy) -> u32 {
    // SAFETY: our object.
    unsafe {
        (*this).refs += 1;
        (*this).refs
    }
}

extern "system" fn spy_release(this: *mut Spy) -> u32 {
    // SAFETY: our object (static; never freed).
    unsafe {
        (*this).refs -= 1;
        (*this).refs
    }
}

extern "system" fn spy_pre_init(this: *mut Spy, _flags: u32, refs: u32) -> HResult {
    // SAFETY: our object.
    unsafe {
        (*this).pre_init += 1;
        (*this).last_refs = refs;
    }
    0
}

extern "system" fn spy_post_init(
    this: *mut Spy,
    result: HResult,
    _flags: u32,
    refs: u32,
) -> HResult {
    // SAFETY: our object.
    unsafe {
        (*this).post_init += 1;
        (*this).last_refs = refs;
    }
    result
}

extern "system" fn spy_pre_uninit(this: *mut Spy, refs: u32) -> HResult {
    // SAFETY: our object.
    unsafe {
        (*this).pre_uninit += 1;
        (*this).last_refs = refs;
    }
    0
}

extern "system" fn spy_post_uninit(this: *mut Spy, refs: u32) -> HResult {
    // SAFETY: our object.
    unsafe {
        (*this).post_uninit += 1;
        (*this).last_refs = refs;
    }
    0
}

struct VTable([*const (); 7]);
// SAFETY: function addresses only.
unsafe impl Sync for VTable {}

static SPY_VTABLE: VTable = VTable([
    spy_query as *const (),
    spy_add_ref as *const (),
    spy_release as *const (),
    spy_pre_init as *const (),
    spy_post_init as *const (),
    spy_pre_uninit as *const (),
    spy_post_uninit as *const (),
]);

fn apartments(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: ole32 calls with buffers and objects of ours.
    unsafe {
        let mut token = 7usize;
        c.eq(hr(CoGetContextToken(&mut token)), 0x8004_01f0)?; // CO_E_NOTINITIALIZED
        let mut spy = Spy {
            vtable: SPY_VTABLE.0.as_ptr().cast(),
            refs: 1,
            pre_init: 0,
            post_init: 0,
            pre_uninit: 0,
            post_uninit: 0,
            last_refs: 99,
        };
        let mut cookie = 0u64;
        let me: *mut Spy = &mut spy;
        c.eq(hr(CoRegisterInitializeSpy(me.cast(), &mut cookie)), S_OK)?;
        c.eq(hr(CoInitializeEx(core::ptr::null_mut(), COINIT_MULTITHREADED)), S_OK)?;
        c.ok(
            (*me).pre_init == 1 && (*me).post_init == 1 && (*me).last_refs == 1,
            (*me).last_refs.into(),
        )?;
        c.eq(hr(CoInitializeEx(core::ptr::null_mut(), COINIT_MULTITHREADED)), S_FALSE)?;
        c.eq((*me).last_refs.into(), 2)?;
        let changed = CoInitializeEx(core::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        c.eq(hr(changed), 0x8001_0106)?; // RPC_E_CHANGED_MODE
        // Process-wide security, once (as SQL Server sets it): default
        // authentication, RPC_C_AUTHN_LEVEL_CONNECT (2),
        // RPC_C_IMP_LEVEL_IDENTIFY (2), EOAC_NONE; a second call is
        // RPC_E_TOO_LATE.
        let null = core::ptr::null_mut();
        let first = CoInitializeSecurity(null, -1, null, null, 2, 2, null, 0, null);
        c.eq(hr(first), S_OK)?;
        let again = CoInitializeSecurity(null, -1, null, null, 2, 2, null, 0, null);
        c.eq(hr(again), 0x8001_0119)?; // RPC_E_TOO_LATE

        c.eq(hr(CoGetContextToken(&mut token)), S_OK)?;
        c.ok(token != 0 && token != 7, 0)?;
        let (mut kind, mut qualifier) = (-1, -1);
        c.eq(hr(CoGetApartmentType(&mut kind, &mut qualifier)), S_OK)?;
        c.eq(kind as u64, 1)?; // APTTYPE_MTA
        // While the MTA exists, a thread that never initialized COM is in
        // it implicitly: APTTYPE_MTA, APTTYPEQUALIFIER_IMPLICIT_MTA (1).
        let [implicit_hr, implicit_kind, implicit_qualifier] = on_uninitialized_thread();
        c.eq(hr(implicit_hr), S_OK)?;
        c.eq(implicit_kind as u64, 1)?;
        c.eq(implicit_qualifier as u64, 1)?;
        // The thread's OLE data says MTA where programs read it (Chromium:
        // OleTlsData's apartment_flags), and an STA thread's says STA
        // until it leaves.
        c.eq(u64::from(ole_flags() & 0x1c0), 0x140)?;
        let t = CreateThread(
            core::ptr::null_mut(),
            0,
            sta_thread,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        c.ok(!t.is_null(), 0)?;
        WaitForSingleObject(t, 0xffff_ffff);
        CloseHandle(t);
        let [sta_inside, sta_after] = STA_FLAGS;
        c.eq(u64::from(sta_inside & 0x1c0), 0x80)?;
        c.eq(u64::from(sta_after & 0x1c0), 0)?;
        let mut context = core::ptr::null_mut();
        c.eq(hr(CoGetObjectContext(&IID_IUNKNOWN, &mut context)), S_OK)?;
        let mut info = core::ptr::null_mut();
        c.eq(hr(query(context, &IID_ICOMTHREADINGINFO, &mut info)), S_OK)?;
        let mut apt = -1i32;
        // IComThreadingInfo::GetCurrentApartmentType (entry 3).
        c.eq(hr(call!(info, 3, fn(*mut i32) -> HResult, &mut apt)), S_OK)?;
        c.eq(apt as u64, 1)?;
        release(info);
        release(context);

        CoUninitialize();
        c.ok((*me).pre_uninit == 1 && (*me).last_refs == 1, (*me).last_refs.into())?;
        CoUninitialize();
        c.ok((*me).post_uninit == 2 && (*me).last_refs == 0, (*me).last_refs.into())?;
        c.eq(hr(CoGetContextToken(&mut token)), 0x8004_01f0)?;
        // The MTA's last thread has left: no implicit MTA any more.
        c.eq(hr(on_uninitialized_thread()[0]), 0x8004_01f0)?; // CO_E_NOTINITIALIZED
        c.eq(u64::from(ole_flags() & 0x1c0), 0)?;
        c.eq(hr(CoRevokeInitializeSpy(cookie)), S_OK)?;
        c.eq(hr(CoRevokeInitializeSpy(cookie)), 0x8007_0057)?; // E_INVALIDARG
        // Revoked: no longer told.
        CoInitializeEx(core::ptr::null_mut(), COINIT_MULTITHREADED);
        c.eq((*me).pre_init.into(), 3)?;
    }
    Ok(())
}

fn memory_guids_classes(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: ole32 calls with buffers of ours (inside the MTA).
    unsafe {
        let p = CoTaskMemAlloc(100);
        c.ok(!p.is_null(), 0)?;
        *p.add(99) = 0x5a;
        let p = CoTaskMemRealloc(p, 4000);
        c.ok(!p.is_null() && *p.add(99) == 0x5a, 0)?;
        CoTaskMemFree(p);
        CoTaskMemFree(core::ptr::null_mut());

        let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
        c.eq(hr(CoCreateGuid(&mut a)), S_OK)?;
        c.eq(hr(CoCreateGuid(&mut b)), S_OK)?;
        c.ok(a != b, 0)?;
        c.eq((a[7] >> 4).into(), 4)?; // version 4 (Data3's top nibble)
        c.eq((a[8] & 0xc0).into(), 0x80)?; // RFC 4122 variant

        let mut obj = 7 as *mut c_void;
        let r = CoGetClassObject(&NOBODY, 1, core::ptr::null_mut(), &IID_IUNKNOWN, &mut obj);
        c.eq(hr(r), 0x8004_0154)?; // REGDB_E_CLASSNOTREG
        c.ok(obj.is_null(), obj as u64)?;
        let progid = w("Gala.NoSuchProgID");
        let mut clsid = [7u8; 16];
        c.eq(hr(CLSIDFromProgID(progid.as_ptr(), &mut clsid)), 0x8004_01f3)?; // CO_E_CLASSSTRING
        let mut text = 7 as *mut u16;
        c.eq(hr(ProgIDFromCLSID(&NOBODY, &mut text)), 0x8004_0154)?; // REGDB_E_CLASSNOTREG
        c.ok(text.is_null(), text as u64)?;

        // An IStream on memory.
        let mut stream = core::ptr::null_mut();
        c.eq(hr(CreateStreamOnHGlobal(core::ptr::null_mut(), 1, &mut stream)), S_OK)?;
        let mut n = 0u32;
        let r =
            call!(stream, 4, fn(*const u8, u32, *mut u32) -> HResult, b"hello".as_ptr(), 5, &mut n);
        c.ok(r == 0 && n == 5, n.into())?;
        let mut pos = 9u64;
        c.eq(hr(call!(stream, 5, fn(i64, u32, *mut u64) -> HResult, 1, 0, &mut pos)), S_OK)?;
        c.eq(pos, 1)?;
        let mut buf = [0u8; 16];
        let r =
            call!(stream, 3, fn(*mut u8, u32, *mut u32) -> HResult, buf.as_mut_ptr(), 16, &mut n);
        c.ok(r == 0 && n == 4 && &buf[..4] == b"ello", n.into())?;
        let mut stat = [0u64; 10];
        // STATFLAG_NONAME (1): no name to free.
        c.eq(hr(call!(stream, 12, fn(*mut u64, u32) -> HResult, stat.as_mut_ptr(), 1)), S_OK)?;
        c.ok(stat[2] == 5 && stat[0] == 0, stat[2])?; // cbSize; pwcsName
        c.eq(release(stream).into(), 0)?;

        let mut ftm = core::ptr::null_mut();
        c.eq(hr(CoCreateFreeThreadedMarshaler(core::ptr::null_mut(), &mut ftm)), S_OK)?;
        let mut marshal = core::ptr::null_mut();
        c.eq(hr(query(ftm, &IID_IMARSHAL, &mut marshal)), S_OK)?;
        release(marshal);
        c.eq(release(ftm).into(), 0)?;

        let e = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let mut index = 7u32;
        c.eq(hr(CoWaitForMultipleHandles(0, 0, 1, &e, &mut index)), 0x8001_0115)?; // RPC_S_CALLPENDING
        let set = CreateEventW(core::ptr::null_mut(), 1, 1, core::ptr::null());
        let both = [e, set];
        c.eq(hr(CoWaitForMultipleHandles(0, 0, 2, both.as_ptr(), &mut index)), S_OK)?;
        c.eq(index.into(), 1)?;
        CloseHandle(e);
        CloseHandle(set);
    }
    Ok(())
}

/// Entry 3 of IGlobalOptions: Set(property, value).
fn global_set(obj: *mut c_void, property: u32, value: usize) -> HResult {
    // SAFETY: an IGlobalOptions of ours.
    unsafe { call!(obj, 3, fn(u32, usize) -> HResult, property, value) }
}

/// Entry 4 of IGlobalOptions: Query(property, out).
fn global_query(obj: *mut c_void, property: u32, value: *mut usize) -> HResult {
    // SAFETY: an IGlobalOptions of ours, and the caller's output.
    unsafe { call!(obj, 4, fn(u32, *mut usize) -> HResult, property, value) }
}

/// CLSID_GlobalOptions, the class Word's OLE startup creates before it
/// opens a document: COM answers for it out of itself, with no registry
/// key, so a class store that only knows the registry can never find it.
///
/// What Windows does: in-process contexts answer and an out-of-process
/// one is REGDB_E_CLASSNOTREG; the class object is an IClassFactory and
/// nothing else; every CreateInstance is a different object, over state
/// that belongs to the process (a fresh object sees what an earlier one
/// set); the properties are 1..=8, a number outside that range and a
/// value outside a property's range are both E_INVALIDARG, COMGLB_APPID
/// is not readable at all, and a Query with nowhere to put the answer is
/// E_INVALIDARG. Property 7 (RESERVED2) reads back a value that differs
/// from machine to machine, so nothing here asks for it.
fn global_options(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: ole32 calls with buffers of ours (inside the MTA).
    unsafe {
        // No registry identity at all, so no ProgID.
        let mut text = 7 as *mut u16;
        c.eq(hr(ProgIDFromCLSID(&CLSID_GLOBALOPTIONS, &mut text)), 0x8004_0154)?;
        c.ok(text.is_null(), text as u64)?;

        // INPROC_SERVER, INPROC_HANDLER and CLSCTX_ALL all answer.
        for context in [1u32, 2, 23] {
            let mut obj = 7 as *mut c_void;
            let r = CoGetClassObject(
                &CLSID_GLOBALOPTIONS,
                context,
                core::ptr::null_mut(),
                &IID_ICLASSFACTORY,
                &mut obj,
            );
            c.eq(hr(r), S_OK)?;
            c.ok(!obj.is_null(), context.into())?;
            release(obj);
        }
        // LOCAL_SERVER on its own does not: there is no server.
        let mut obj = 7 as *mut c_void;
        let r = CoGetClassObject(
            &CLSID_GLOBALOPTIONS,
            4,
            core::ptr::null_mut(),
            &IID_ICLASSFACTORY,
            &mut obj,
        );
        c.eq(hr(r), 0x8004_0154)?; // REGDB_E_CLASSNOTREG
        c.ok(obj.is_null(), obj as u64)?;

        // IUnknown gets the same class object.
        let mut unk = 7 as *mut c_void;
        let r = CoGetClassObject(
            &CLSID_GLOBALOPTIONS,
            1,
            core::ptr::null_mut(),
            &IID_IUNKNOWN,
            &mut unk,
        );
        c.eq(hr(r), S_OK)?;
        c.ok(!unk.is_null(), unk as u64)?;
        release(unk);

        let mut cf = 7 as *mut c_void;
        let r = CoGetClassObject(
            &CLSID_GLOBALOPTIONS,
            1,
            core::ptr::null_mut(),
            &IID_ICLASSFACTORY,
            &mut cf,
        );
        c.eq(hr(r), S_OK)?;

        // The class object is a factory and an IUnknown, and nothing else.
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(cf, &IID_IUNKNOWN, &mut other)), S_OK)?;
        release(other);
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(cf, &IID_ICLASSFACTORY, &mut other)), S_OK)?;
        release(other);
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(cf, &IID_IMARSHAL, &mut other)), 0x8000_4002)?; // E_NOINTERFACE
        c.ok(other.is_null(), other as u64)?;

        // IClassFactory::CreateInstance (entry 3), and the call Word makes.
        let mut g = 7 as *mut c_void;
        let r = call!(
            cf,
            3,
            fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
            core::ptr::null_mut(),
            &IID_IGLOBALOPTIONS,
            &mut g
        );
        c.eq(hr(r), S_OK)?;
        c.ok(!g.is_null(), g as u64)?;

        let mut g2 = 7 as *mut c_void;
        let r = CoCreateInstance(
            &CLSID_GLOBALOPTIONS,
            core::ptr::null_mut(),
            1,
            &IID_IGLOBALOPTIONS,
            &mut g2,
        );
        c.eq(hr(r), S_OK)?;
        // Not a singleton: every create is its own object.
        c.ok(!g2.is_null() && g2 != g, g2 as u64)?;

        // The object is an IGlobalOptions and an IUnknown; it does not
        // marshal.
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(g, &IID_IUNKNOWN, &mut other)), S_OK)?;
        release(other);
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(g, &IID_IGLOBALOPTIONS, &mut other)), S_OK)?;
        release(other);
        let mut other = 7 as *mut c_void;
        c.eq(hr(query(g, &IID_IMARSHAL, &mut other)), 0x8000_4002)?; // E_NOINTERFACE
        c.ok(other.is_null(), other as u64)?;

        // The defaults, read before anything is set. The SDK documents 0
        // for all of them; this build answers PRIVATE_POOL for the thread
        // pool and HYBRID for the unmarshaling policy.
        for (property, want) in [(1u32, 0u64), (3, 1), (4, 0), (5, 2), (6, 0), (8, 0)] {
            let mut v = 7usize;
            c.eq(hr(global_query(g, property, &mut v)), S_OK)?;
            c.eq(v as u64, want)?;
        }
        let mut v = 7usize;
        c.eq(hr(global_query(g, 2, &mut v)), 0x8000_4005)?; // E_FAIL: COMGLB_APPID
        for property in [0u32, 9, 10] {
            let mut v = 7usize;
            c.eq(hr(global_query(g, property, &mut v)), E_INVALIDARG)?;
        }
        c.eq(hr(global_query(g, 1, core::ptr::null_mut())), E_INVALIDARG)?;

        // What Office sets. The value is checked as well as the property.
        c.eq(hr(global_set(g, 1, 2)), S_OK)?; // DONOT_HANDLE_ANY
        c.eq(hr(global_set(g, 3, 1)), S_OK)?; // PRIVATE_POOL
        c.eq(hr(global_set(g, 5, 1)), S_OK)?; // STRONG
        c.eq(hr(global_set(g, 0, 0)), E_INVALIDARG)?;
        c.eq(hr(global_set(g, 9, 0)), E_INVALIDARG)?;
        c.eq(hr(global_set(g, 1, 99)), E_INVALIDARG)?;

        // The three Sets took, and the refused one left the property alone.
        for (property, want) in [(1u32, 2u64), (3, 1), (5, 1)] {
            let mut v = 7usize;
            c.eq(hr(global_query(g, property, &mut v)), S_OK)?;
            c.eq(v as u64, want)?;
        }

        // The state is the process's, not the object's.
        let mut g3 = 7 as *mut c_void;
        let r = CoCreateInstance(
            &CLSID_GLOBALOPTIONS,
            core::ptr::null_mut(),
            1,
            &IID_IGLOBALOPTIONS,
            &mut g3,
        );
        c.eq(hr(r), S_OK)?;
        for (property, want) in [(1u32, 2u64), (3, 1), (5, 1)] {
            let mut v = 7usize;
            c.eq(hr(global_query(g3, property, &mut v)), S_OK)?;
            c.eq(v as u64, want)?;
        }

        release(g3);
        release(g2);
        release(g);
        release(cf);
    }
    Ok(())
}

fn automation(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: oleaut32 calls with buffers of ours.
    unsafe {
        // BSTRs: a byte length before the text, NUL after it.
        let gala = w("gala");
        let s = SysAllocString(gala.as_ptr());
        c.eq(SysStringLen(s).into(), 4)?;
        c.eq(SysStringByteLen(s).into(), 8)?;
        c.eq((*(s.cast::<u32>().sub(1))).into(), 8)?;
        c.ok(*s.add(4) == 0, 0)?;
        c.text(s, "gala")?;
        SysFreeString(s);
        let s = SysAllocStringLen(core::ptr::null(), 3);
        c.ok(SysStringLen(s) == 3 && *s.add(3) == 0, 0)?;
        SysFreeString(s);
        let s = SysAllocStringByteLen(b"abc".as_ptr(), 3);
        c.ok(SysStringByteLen(s) == 3 && SysStringLen(s) == 1, SysStringByteLen(s).into())?;
        SysFreeString(s);
        c.eq(SysStringLen(core::ptr::null()).into(), 0)?;
        SysFreeString(core::ptr::null_mut());

        // VARIANTs.
        let mut v = Variant { vt: 99, reserved: [0; 3], value: [0; 2] };
        VariantInit(&mut v);
        c.eq(v.vt.into(), VT_EMPTY.into())?;
        v.vt = VT_I4;
        v.value[0] = 42;
        let mut d = Variant { vt: 0, reserved: [0; 3], value: [0; 2] };
        VariantInit(&mut d);
        c.eq(hr(VariantChangeType(&mut d, &v, 0, VT_BSTR)), S_OK)?;
        c.eq(d.vt.into(), VT_BSTR.into())?;
        let text = d.value[0] as *const u16;
        c.ok(SysStringLen(text) == 2, SysStringLen(text).into())?;
        c.text(text, "42")?;
        // Back from the string, through the Ex form with a locale.
        let mut n = Variant { vt: 0, reserved: [0; 3], value: [0; 2] };
        c.eq(hr(VariantChangeTypeEx(&mut n, &d, 0x409, 0, VT_R8)), S_OK)?;
        c.ok(n.vt == VT_R8 && f64::from_bits(n.value[0]) == 42.0, n.value[0])?;
        c.eq(hr(VariantClear(&mut d)), S_OK)?;
        c.eq(d.vt.into(), VT_EMPTY.into())?;
        let bad = w("nope");
        d.vt = VT_BSTR;
        d.value[0] = SysAllocString(bad.as_ptr()) as u64;
        c.eq(hr(VariantChangeType(&mut n, &d, 0, VT_I4)), 0x8002_0005)?; // DISP_E_TYPEMISMATCH
        VariantClear(&mut d);

        // SAFEARRAYs.
        let sa = SafeArrayCreateVector(VT_I4, 0, 5);
        c.ok(!sa.is_null(), 0)?;
        c.eq(SafeArrayGetDim(sa).into(), 1)?;
        c.eq(SafeArrayGetElemsize(sa).into(), 4)?;
        let (mut lower, mut upper) = (-1, -1);
        c.eq(hr(SafeArrayGetLBound(sa, 1, &mut lower)), S_OK)?;
        c.eq(hr(SafeArrayGetUBound(sa, 1, &mut upper)), S_OK)?;
        c.ok(lower == 0 && upper == 4, upper as u64)?;
        c.eq(hr(SafeArrayGetLBound(sa, 2, &mut lower)), 0x8002_000b)?; // DISP_E_BADINDEX
        let (index, value) = (2i32, 77i32);
        c.eq(hr(SafeArrayPutElement(sa, &index, (&value as *const i32).cast())), S_OK)?;
        c.eq((*((*sa).data as *const i32).add(2)) as u64, 77)?;
        let bad_index = 5i32;
        c.eq(hr(SafeArrayPutElement(sa, &bad_index, (&value as *const i32).cast())), 0x8002_000b)?;
        let mut vt = 0u16;
        c.eq(hr(SafeArrayGetVartype(sa, &mut vt)), S_OK)?;
        c.eq(vt.into(), VT_I4.into())?;
        c.eq(hr(SafeArrayDestroy(sa)), S_OK)?;
        let mut sa = core::ptr::null_mut();
        c.eq(hr(SafeArrayAllocDescriptorEx(VT_I4, 1, &mut sa)), S_OK)?;
        // FADF_HAVEVARTYPE and the VARTYPE before the descriptor, but no
        // element size (Windows 11 25H2: cbElements 0, still 0 after
        // SafeArrayAllocData, whose pvData is not NULL).
        c.eq((*sa).features.into(), 0x80)?; // FADF_HAVEVARTYPE
        c.eq((sa.cast::<u8>().sub(4) as *const u32).read().into(), VT_I4.into())?;
        c.eq((*sa).element_size.into(), 0)?;
        (*sa).count = 3;
        (*sa).lower = 1;
        c.eq(hr(SafeArrayAllocData(sa)), S_OK)?;
        c.ok(!(*sa).data.is_null() && (*sa).element_size == 0, (*sa).element_size.into())?;
        c.eq(hr(SafeArrayDestroy(sa)), S_OK)?;

        // Error info: none set.
        let mut info = 7 as *mut c_void;
        c.eq(hr(GetErrorInfo(0, &mut info)), S_FALSE)?;
        c.ok(info.is_null(), 0)?;
        c.eq(hr(SetErrorInfo(0, core::ptr::null_mut())), S_OK)?;

        // 12.345 as a currency (four decimal places, scaled by 10000).
        let dec = Decimal { reserved: 0, scale: 3, sign: 0, hi: 0, lo: 12345 };
        let mut cy = 0i64;
        c.eq(hr(VarCyFromDec(&dec, &mut cy)), S_OK)?;
        c.eq(cy as u64, 123_450)?;
        let neg = Decimal { reserved: 0, scale: 0, sign: 0x80, hi: 0, lo: 2 };
        c.eq(hr(VarCyFromDec(&neg, &mut cy)), S_OK)?;
        c.eq(cy as u64, -20_000i64 as u64)?;

        // No type libraries registered.
        let mut lib = core::ptr::null_mut();
        c.eq(hr(LoadRegTypeLib(&NOBODY, 1, 0, 0, &mut lib)), 0x8002_801d)?; // TYPE_E_LIBNOTREGISTERED
        let mut path = core::ptr::null_mut();
        c.eq(hr(QueryPathOfRegTypeLib(&NOBODY, 1, 0, 0, &mut path)), 0x8002_801d)?;
    }
    Ok(())
}

/// CoGetMalloc(MEMCTX_TASK): the task allocator (IMalloc: QueryInterface,
/// AddRef, Release, Alloc, Realloc, Free, GetSize, DidAlloc,
/// HeapMinimize), sharing its memory with CoTaskMem*; another context is
/// E_INVALIDARG.
fn task_allocator(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: IMalloc's vtable entries as documented.
    unsafe {
        let mut malloc = core::ptr::null_mut();
        c.eq(hr(CoGetMalloc(1, &mut malloc)), S_OK)?;
        c.ok(!malloc.is_null(), 0)?;
        let p = call!(malloc, 3, fn(usize) -> *mut u8, 32);
        c.ok(!p.is_null(), 0)?;
        *p = 7;
        c.ok(call!(malloc, 6, fn(*mut u8) -> usize, p) >= 32, 0)?; // GetSize
        c.eq(call!(malloc, 7, fn(*mut u8) -> i32, p) as u64, 1)?; // DidAlloc
        let q = call!(malloc, 4, fn(*mut u8, usize) -> *mut u8, p, 64); // Realloc
        c.ok(!q.is_null() && *q == 7, 0)?;
        CoTaskMemFree(q); // the same memory as CoTaskMem*
        let r = CoTaskMemAlloc(8);
        call!(malloc, 5, fn(*mut u8) -> (), r); // Free
        release(malloc);
        c.eq(hr(CoGetMalloc(2, &mut malloc)), 0x8007_0057)?; // E_INVALIDARG
    }
    Ok(())
}

/// GUID strings: "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}" (StringFromCLSID
/// writes upper case into CoTaskMem memory; parsing takes either case);
/// CLSIDFromString of a malformed string is CO_E_CLASSSTRING,
/// IIDFromString's E_INVALIDARG; NULL is the null GUID.
fn guid_strings(c: &mut Checks) -> Result<(), u32> {
    // IID_IUnknown {00000000-0000-0000-C000-000000000046}.
    let unknown: Guid = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
    let text = "{00000000-0000-0000-C000-000000000046}";
    let mut wide = [0u16; 40];
    for (d, b) in wide.iter_mut().zip(text.bytes()) {
        *d = b.into();
    }
    // SAFETY: ole32 calls with buffers of ours.
    unsafe {
        let mut g: Guid = [0xff; 16];
        c.eq(hr(CLSIDFromString(wide.as_ptr(), &mut g)), S_OK)?;
        c.ok(g == unknown, g[8].into())?;
        let mut lower = wide;
        lower[20] = u16::from(b'c');
        c.eq(hr(IIDFromString(lower.as_ptr(), &mut g)), S_OK)?;
        c.ok(g == unknown, 0)?;
        let mut s = core::ptr::null_mut();
        c.eq(hr(StringFromCLSID(&unknown, &mut s)), S_OK)?;
        c.ok(
            text.bytes().enumerate().all(|(i, b)| *s.add(i) == u16::from(b)) && *s.add(38) == 0,
            0,
        )?;
        CoTaskMemFree(s.cast());
        c.eq(hr(StringFromIID(&unknown, &mut s)), S_OK)?;
        CoTaskMemFree(s.cast());
        let bad = [0x7bu16, 0x31, 0x7d, 0]; // "{1}"
        c.eq(hr(CLSIDFromString(bad.as_ptr(), &mut g)), 0x8004_01f3)?; // CO_E_CLASSSTRING
        c.eq(hr(IIDFromString(bad.as_ptr(), &mut g)), 0x8007_0057)?; // E_INVALIDARG
        c.eq(hr(CLSIDFromString(core::ptr::null(), &mut g)), S_OK)?;
        c.ok(g == [0; 16], 0)?;
    }
    Ok(())
}

// ---- oleaut32's number conversions and active objects ----

/// A BSTR is exactly `want` (its length as well as its characters).
fn bstr_is(c: &mut Checks, s: *const u16, want: &str) -> Result<(), u32> {
    // SAFETY: a BSTR, or NULL.
    c.eq(unsafe { SysStringLen(s) }.into(), want.len() as u64)?;
    c.text(s, want)
}

/// The conversions Office's wwlib imports, with the results Windows 11
/// gives:
///
/// - VarR8FromR4: the exact widening, always S_OK, infinities and NaNs
///   with it (0x7fc00000 becomes 0x7ff8000000000000), -0 still -0.
/// - VarR4FromR8: rounded to nearest f32, ties to even (16777219
///   becomes 0x4b800002); S_OK whenever the rounded result is finite,
///   DISP_E_OVERFLOW otherwise, which includes an infinite or NaN
///   source, where the output is left alone. Underflow is 0 (or -0) with
///   S_OK and subnormals come through (1e-44 becomes 0x00000007).
/// - VarBstrFromI4: plain decimal, no grouping, the locale ignored.
/// - VarBstrFromR4 and VarBstrFromR8: the number with up to 7 (R4) or 15
///   (R8) significant digits, fixed notation when the integer digits and
///   the decimals it needs both fit in that many digits and scientific
///   ("1.234567E+07", the exponent signed and at least two digits)
///   otherwise; "0" and "-0" for the zeroes, "inf", "-inf" and "nan".
fn conversions(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: oleaut32 calls with outputs of ours.
    unsafe {
        let mut r8 = 0.0f64;
        for (src, want) in [
            (0x3f00_0000u32, 0x3fe0_0000_0000_0000u64),
            (0x8000_0000, 0x8000_0000_0000_0000),
            (0x3eaa_aaab, 0x3fd5_5555_6000_0000),
            (0x7f7f_ffff, 0x47ef_ffff_e000_0000),
            (0x0080_0000, 0x3810_0000_0000_0000),
            (0x0000_0007, 0x36cc_0000_0000_0000),
            (0x7f80_0000, 0x7ff0_0000_0000_0000),
            (0xff80_0000, 0xfff0_0000_0000_0000),
            (0x7fc0_0000, 0x7ff8_0000_0000_0000),
        ] {
            c.eq(hr(VarR8FromR4(f32::from_bits(src), &mut r8)), S_OK)?;
            c.eq(r8.to_bits(), want)?;
        }

        let mut r4 = 0.0f32;
        for (src, want) in [
            (0x3fe0_0000_0000_0000u64, 0x3f00_0000u32),
            (0x8000_0000_0000_0000, 0x8000_0000),
            (0x3fd5_5555_5555_5555, 0x3eaa_aaab),
            // Ties to even: 16777217 down, 16777219 up.
            (0x4170_0000_1000_0000, 0x4b80_0000),
            (0x4170_0000_3000_0000, 0x4b80_0002),
            // Just inside f32::MAX once rounded.
            (0x47ef_ffff_e000_0001, 0x7f7f_ffff),
            // Underflow, and a subnormal result.
            (0x3662_44ce_242c_5561, 0x0000_0000),
            (0xb662_44ce_242c_5561, 0x8000_0000),
            (0x36cc_8b82_1885_4567, 0x0000_0007),
        ] {
            c.eq(hr(VarR4FromR8(f64::from_bits(src), &mut r4)), S_OK)?;
            c.eq(r4.to_bits().into(), want.into())?;
        }
        // Too large, infinite or NaN: DISP_E_OVERFLOW, the output kept.
        for src in [
            0x47ef_ffff_f514_a7bcu64,
            0x4807_8287_f49c_4a1d,
            0xc807_8287_f49c_4a1d,
            0x7fef_ffff_ffff_ffff,
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
            0x7ff8_0000_0000_0000,
        ] {
            r4 = 42.0;
            c.eq(hr(VarR4FromR8(f64::from_bits(src), &mut r4)), DISP_E_OVERFLOW)?;
            c.eq(r4.to_bits().into(), 42.0f32.to_bits().into())?;
        }

        let mut s = core::ptr::null_mut();
        for (v, want) in [
            (0i32, "0"),
            (1, "1"),
            (-1, "-1"),
            (1234567, "1234567"),
            (-1234567, "-1234567"),
            (i32::MAX, "2147483647"),
            (i32::MIN, "-2147483648"),
        ] {
            c.eq(hr(VarBstrFromI4(v, LCID_EN_US, 0, &mut s)), S_OK)?;
            bstr_is(c, s, want)?;
            SysFreeString(s);
        }
        // The locale and the flags change nothing for an integer.
        c.eq(hr(VarBstrFromI4(1234567, 0, 0x8000_0000, &mut s)), S_OK)?;
        bstr_is(c, s, "1234567")?;
        SysFreeString(s);

        for (src, want) in [
            (0x0000_0000u32, "0"),
            (0x8000_0000, "-0"),
            (0x3fc0_0000, "1.5"),
            (0xbfc0_0000, "-1.5"),
            (0x3eaa_aaab, "0.3333333"),
            (0x7f7f_ffff, "3.402823E+38"),
            (0x0000_0007, "9.809089E-45"),
            (0x7f80_0000, "inf"),
            (0xff80_0000, "-inf"),
            (0x7fc0_0000, "nan"),
            (0x322b_cc77, "1E-08"),
            (0x3749_539c, "0.000012"),
            (0x4996_b438, "1234567"),
            (0x4b3c_6146, "1.234567E+07"),
            // Ties round away from zero, not to even.
            (0x4974_2408, "1000001"),
            (0x4974_2418, "1000002"),
            (0x4b3c_614b, "1.234568E+07"),
            (0x4b3c_6155, "1.234569E+07"),
            (0x60ad_78ec, "1E+20"),
            (0x3c4a_457e, "1.234567E-02"),
        ] {
            c.eq(hr(VarBstrFromR4(f32::from_bits(src), LCID_EN_US, 0, &mut s)), S_OK)?;
            bstr_is(c, s, want)?;
            SysFreeString(s);
        }

        for (src, want) in [
            (0x0000_0000_0000_0000u64, "0"),
            (0x8000_0000_0000_0000, "-0"),
            (0x7ff0_0000_0000_0000, "inf"),
            (0xfff0_0000_0000_0000, "-inf"),
            (0x7ff8_0000_0000_0000, "nan"),
            (0x3ff8_0000_0000_0000, "1.5"),
            // The rule's edges: 15 decimals fit, 16 do not; 15 integer
            // digits fit, 16 do not.
            (0x3cd2_03af_9ee7_5616, "0.000000000000001"),
            (0x3c9c_d2b2_97d8_89bc, "1E-16"),
            (0x430c_6bf5_2634_0000, "1E+15"),
            (0x430c_6bf5_2633_fff8, "999999999999999"),
            (0x3fd5_5555_5555_5555, "0.333333333333333"),
            // Ties round away from zero here too.
            (0x42d6_bcc4_1e90_0020, "100000000000001"),
            (0x42d6_bcc4_1e90_0060, "100000000000002"),
            (0x4311_8b54_f22a_ead4, "1.23456789012345E+15"),
            (0x4311_8b54_f22a_eafc, "1.23456789012346E+15"),
            // Three exponent digits, and a fraction that stays fixed.
            (0x7e37_e43c_8800_759c, "1E+300"),
            (0x01a5_6e1f_c2f8_f359, "1E-300"),
            (0x3eef_7510_4d55_1d69, "0.000015"),
        ] {
            c.eq(hr(VarBstrFromR8(f64::from_bits(src), LCID_EN_US, 0, &mut s)), S_OK)?;
            bstr_is(c, s, want)?;
            SysFreeString(s);
        }
    }
    Ok(())
}

/// A test object: a strict IUnknown (only IID_IUnknown succeeds, as the
/// marshaller needs) that counts its references.
#[repr(C)]
struct Active {
    vtable: *const usize,
    refs: u32,
}

extern "system" fn active_query(
    this: *mut Active,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: our own object and the caller's IID and output.
    unsafe {
        if *iid == IID_IUNKNOWN {
            (*this).refs += 1;
            *out = this.cast();
            return 0;
        }
        *out = core::ptr::null_mut();
    }
    0x8000_4002u32 as i32 // E_NOINTERFACE
}

extern "system" fn active_add_ref(this: *mut Active) -> u32 {
    // SAFETY: our own object.
    unsafe {
        (*this).refs += 1;
        (*this).refs
    }
}

extern "system" fn active_release(this: *mut Active) -> u32 {
    // SAFETY: our own object.
    unsafe {
        (*this).refs -= 1;
        (*this).refs
    }
}

static ACTIVE_VTABLE: VTable = VTable([
    active_query as *const (),
    active_add_ref as *const (),
    active_release as *const (),
    core::ptr::null(),
    core::ptr::null(),
    core::ptr::null(),
    core::ptr::null(),
]);

/// RegisterActiveObject, RevokeActiveObject and GetActiveObject before
/// any apartment exists: CO_E_NOTINITIALIZED from all three (measured on
/// Windows 11).
fn active_objects_before_com(c: &mut Checks) -> Result<(), u32> {
    let mut object = Active { vtable: ACTIVE_VTABLE.0.as_ptr().cast(), refs: 1 };
    let me: *mut Active = &mut object;
    let mut cookie = 0u32;
    let mut got = core::ptr::null_mut();
    // SAFETY: oleaut32 calls with an object and outputs of ours.
    unsafe {
        c.eq(
            hr(RegisterActiveObject(me.cast(), &CLSID_RUNNING, 0, &mut cookie)),
            CO_E_NOTINITIALIZED,
        )?;
        c.eq(
            hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)),
            CO_E_NOTINITIALIZED,
        )?;
        c.eq(hr(RevokeActiveObject(1, core::ptr::null_mut())), CO_E_NOTINITIALIZED)?;
    }
    c.eq(object.refs.into(), 1)
}

/// The running objects of a process, as Windows 11 keeps them:
///
/// - A null object, a null cookie output or a null CLSID is
///   E_INVALIDARG, as is a null CLSID or output for GetActiveObject and
///   a cookie of 0 or one never given for RevokeActiveObject (so
///   revoking the same cookie twice fails the second time).
/// - The first registration of a CLSID is S_OK with a non-zero cookie;
///   another of the same CLSID is MK_S_MONIKERALREADYREGISTERED, still
///   with a cookie of its own.
/// - Registering holds one reference until revoked, weak as well as
///   strong, and GetActiveObject AddRefs what it gives.
/// - GetActiveObject gives the first registration still in force, so
///   after revoking that one the next answers; with none,
///   MK_E_UNAVAILABLE.
fn active_objects(c: &mut Checks) -> Result<(), u32> {
    let mut first = Active { vtable: ACTIVE_VTABLE.0.as_ptr().cast(), refs: 1 };
    let mut second = Active { vtable: ACTIVE_VTABLE.0.as_ptr().cast(), refs: 1 };
    let (a, b): (*mut Active, *mut Active) = (&mut first, &mut second);
    let mut cookie = 0u32;
    let mut got = core::ptr::null_mut();
    // SAFETY: oleaut32 calls with objects and outputs of ours.
    unsafe {
        // The arguments.
        c.eq(
            hr(RegisterActiveObject(core::ptr::null_mut(), &CLSID_RUNNING, 0, &mut cookie)),
            E_INVALIDARG,
        )?;
        c.eq(
            hr(RegisterActiveObject(a.cast(), &CLSID_RUNNING, 0, core::ptr::null_mut())),
            E_INVALIDARG,
        )?;
        c.eq(hr(RegisterActiveObject(a.cast(), core::ptr::null(), 0, &mut cookie)), E_INVALIDARG)?;
        c.eq(
            hr(GetActiveObject(core::ptr::null(), core::ptr::null_mut(), &mut got)),
            E_INVALIDARG,
        )?;
        c.eq(
            hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), core::ptr::null_mut())),
            E_INVALIDARG,
        )?;
        c.eq(hr(RevokeActiveObject(0, core::ptr::null_mut())), E_INVALIDARG)?;
        c.eq(hr(RevokeActiveObject(0x1234_5678, core::ptr::null_mut())), E_INVALIDARG)?;
        c.eq(first.refs.into(), 1)?;

        // Nothing registered yet.
        c.eq(
            hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)),
            MK_E_UNAVAILABLE,
        )?;
        c.ok(got.is_null(), 0)?;

        // Two objects under one CLSID: the first one answers.
        let mut one = 0u32;
        c.eq(hr(RegisterActiveObject(a.cast(), &CLSID_RUNNING, 0, &mut one)), S_OK)?;
        c.ok(one != 0, one.into())?;
        c.eq(first.refs.into(), 2)?;
        c.eq(hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)), S_OK)?;
        c.ok(got == a.cast(), got as u64)?;
        c.eq(first.refs.into(), 3)?;
        c.eq(u64::from(release(got)), 2)?;
        let mut two = 0u32;
        c.eq(
            hr(RegisterActiveObject(b.cast(), &CLSID_RUNNING, 0, &mut two)),
            MK_S_MONIKERALREADYREGISTERED,
        )?;
        c.ok(two != 0 && two != one, two.into())?;
        c.eq(second.refs.into(), 2)?;
        c.eq(hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)), S_OK)?;
        c.ok(got == a.cast(), got as u64)?;
        c.eq(u64::from(release(got)), 2)?;

        // Revoking the first leaves the second to answer.
        c.eq(hr(RevokeActiveObject(one, core::ptr::null_mut())), S_OK)?;
        c.eq(first.refs.into(), 1)?;
        c.eq(hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)), S_OK)?;
        c.ok(got == b.cast(), got as u64)?;
        c.eq(u64::from(release(got)), 2)?;
        c.eq(hr(RevokeActiveObject(one, core::ptr::null_mut())), E_INVALIDARG)?;
        c.eq(hr(RevokeActiveObject(two, core::ptr::null_mut())), S_OK)?;
        c.eq(second.refs.into(), 1)?;
        c.eq(
            hr(GetActiveObject(&CLSID_RUNNING, core::ptr::null_mut(), &mut got)),
            MK_E_UNAVAILABLE,
        )?;

        // A weak registration holds a reference too, and answers.
        let mut weak = 0u32;
        c.eq(hr(RegisterActiveObject(b.cast(), &CLSID_OTHER, 1, &mut weak)), S_OK)?;
        c.eq(second.refs.into(), 2)?;
        c.eq(hr(GetActiveObject(&CLSID_OTHER, core::ptr::null_mut(), &mut got)), S_OK)?;
        c.ok(got == b.cast(), got as u64)?;
        c.eq(u64::from(release(got)), 2)?;
        c.eq(hr(RevokeActiveObject(weak, core::ptr::null_mut())), S_OK)?;
        c.eq(second.refs.into(), 1)?;
        // Another CLSID is its own; this one is free again.
        c.eq(hr(GetActiveObject(&CLSID_OTHER, core::ptr::null_mut(), &mut got)), MK_E_UNAVAILABLE)?;
    }
    c.eq(first.refs.into(), 1)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // Before anything initializes COM, so the active objects see a
    // process without an apartment.
    active_objects_before_com(&mut c)?;
    guid_strings(&mut c)?;
    task_allocator(&mut c)?;
    apartments(&mut c)?;
    memory_guids_classes(&mut c)?;
    global_options(&mut c)?;
    automation(&mut c)?;
    conversions(&mut c)?;
    // After apartments(), which leaves this thread in the MTA.
    active_objects(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"com_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}

/// A pointer-sized field of the current thread's TEB (x64: `gs:[offset]`;
/// ARM64: x18 is the TEB).
fn teb_field(offset: usize) -> usize {
    let v: usize;
    // SAFETY: the TEB is mapped for the thread's life; `offset` is one of
    // its pointer-sized fields.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    // SAFETY: as above (x18 holds the TEB on ARM64 Windows).
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("ldr {}, [x18, {}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    v
}

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
//!   CoGetObjectContext and CoGetApartmentType (MTA), CoTaskMem*,
//!   CoCreateGuid (random version-4 GUIDs), CoGetClassObject
//!   (REGDB_E_CLASSNOTREG), CLSIDFromProgID (CO_E_CLASSSTRING),
//!   CreateStreamOnHGlobal (an IStream: write, seek, read, stat),
//!   CoCreateFreeThreadedMarshaler (IMarshal), CoWaitForMultipleHandles
//!   (RPC_S_CALLPENDING on timeout).
//! - oleaut32: BSTRs (length prefix, byte strings), VARIANTs (VariantClear
//!   frees, VariantChangeType(Ex) between numbers and strings),
//!   SAFEARRAYs (vectors, bounds, elements, the stored VARTYPE,
//!   descriptors), Get/SetErrorInfo, VarCyFromDec, type libraries that are
//!   not registered (TYPE_E_LIBNOTREGISTERED).
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
    fn CLSIDFromProgID(progid: *const u16, clsid: *mut Guid) -> HResult;
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
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"com_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// The UTF-16 text at `p` is `want` (not necessarily terminated).
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
        (*sa).count = 3;
        (*sa).lower = 1;
        c.eq(hr(SafeArrayAllocData(sa)), S_OK)?;
        c.ok(!(*sa).data.is_null() && (*sa).element_size == 4, 0)?;
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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    guid_strings(&mut c)?;
    task_allocator(&mut c)?;
    apartments(&mut c)?;
    memory_guids_classes(&mut c)?;
    automation(&mut c)
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

//! combase.dll: Windows Runtime strings, initialization, activation and
//! error origination, imported from combase.dll as C++/WinRT programs
//! find them (the Office Deployment Tool's setup.exe, ROADMAP OF). The
//! expected values are Windows' as Wine's conformance tests observe them
//! (dlls/combase/tests/string.c and roapi.c, run against Windows), the
//! documentation of each function, and real Windows on CI's runner (this
//! program's run there):
//! - HSTRINGs: copies and references (a reference's header is the
//!   caller's HSTRING_HEADER, flags 1, pointing at the caller's text; a
//!   copy's header is followed by its reference count, 1, and its text);
//!   the header's layout (flags, length, two words, the text pointer:
//!   24 bytes on x64, 20 on x86); empty strings are NULL; E_INVALIDARG
//!   and E_POINTER for missing arguments; a reference whose text is not
//!   NUL-terminated at its length fails; embedded NULs counted; duplicates
//!   of a copy are the same string, of a reference a new one; string
//!   buffers (preallocate, promote, a buffer whose terminator was
//!   overwritten refused); substrings with E_BOUNDS; concatenation;
//!   ordinal comparison; trimming;
//! - RoGetActivationFactory before any apartment: CO_E_NOTINITIALIZED;
//!   RoInitialize: S_OK, then S_FALSE, RPC_E_CHANGED_MODE for the other
//!   kind; an unknown class: REGDB_E_CLASSNOTREG and a NULL output from
//!   RoGetActivationFactory and RoActivateInstance;
//! - CoIncrementMTAUsage: a thread outside any apartment is then an
//!   implicit MTA member (APTTYPE_MTA, APTTYPEQUALIFIER_IMPLICIT_MTA) and
//!   activation works there;
//! - Windows.Management.Deployment.PackageManager (present on every
//!   Windows; gala's answers as a machine with no packages):
//!   RoGetActivationFactory and RoActivateInstance succeed, the instance
//!   is an IPackageManager named by GetRuntimeClassName, and
//!   FindPackagesForUser("", the Store Office's family name) is an empty
//!   collection (no Store Office on CI's runner);
//! - errors: RoGetErrorReportingFlags E_POINTER without an output and
//!   RO_ERROR_REPORTING_USESETERRORINFO at start; RoOriginateErrorW FALSE
//!   for S_OK, TRUE for a failure, whose restricted error object
//!   GetRestrictedErrorInfo hands over with the HRESULT and the message
//!   (GetErrorDetails), leaving none behind;
//! - ole32's call cancellation (`cancellation`; Microsoft's
//!   documentation of CoEnableCallCancellation, CoDisableCallCancellation
//!   and CoCancelCall).
//!
//! The same source builds `wow64_combase_test`, the 32-bit (x86) program.
//! Prints `combase_test: ok` (`wow64_combase_test: ok`), or `<name>:
//! check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "combase_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_combase_test";

type Handle = usize;
type HResult = i32;
type HString = *mut c_void;
type Guid = [u8; 16];

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "oleaut32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "oleaut32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SysFreeString(s: *mut u16);
    fn SysStringLen(s: *const u16) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ole32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ole32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CoEnableCallCancellation(reserved: *mut c_void) -> HResult;
    fn CoDisableCallCancellation(reserved: *mut c_void) -> HResult;
    fn CoCancelCall(thread: u32, timeout: u32) -> HResult;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "combase", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "combase", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WindowsCreateString(text: *const u16, len: u32, out: *mut HString) -> HResult;
    fn WindowsCreateStringReference(
        text: *const u16,
        len: u32,
        header: *mut Header,
        out: *mut HString,
    ) -> HResult;
    fn WindowsDeleteString(s: HString) -> HResult;
    fn WindowsDuplicateString(s: HString, out: *mut HString) -> HResult;
    fn WindowsGetStringLen(s: HString) -> u32;
    fn WindowsGetStringRawBuffer(s: HString, len: *mut u32) -> *const u16;
    fn WindowsIsStringEmpty(s: HString) -> i32;
    fn WindowsStringHasEmbeddedNull(s: HString, out: *mut i32) -> HResult;
    fn WindowsCompareStringOrdinal(a: HString, b: HString, out: *mut i32) -> HResult;
    fn WindowsSubstring(s: HString, start: u32, out: *mut HString) -> HResult;
    fn WindowsSubstringWithSpecifiedLength(
        s: HString,
        start: u32,
        len: u32,
        out: *mut HString,
    ) -> HResult;
    fn WindowsConcatString(a: HString, b: HString, out: *mut HString) -> HResult;
    fn WindowsTrimStringStart(s: HString, set: HString, out: *mut HString) -> HResult;
    fn WindowsTrimStringEnd(s: HString, set: HString, out: *mut HString) -> HResult;
    fn WindowsPreallocateStringBuffer(len: u32, text: *mut *mut u16, buf: *mut HString) -> HResult;
    fn WindowsPromoteStringBuffer(buf: HString, out: *mut HString) -> HResult;
    fn WindowsDeleteStringBuffer(buf: HString) -> HResult;
    fn RoInitialize(kind: u32) -> HResult;
    fn RoUninitialize();
    fn RoGetActivationFactory(class: HString, iid: *const Guid, out: *mut *mut c_void) -> HResult;
    fn RoActivateInstance(class: HString, out: *mut *mut c_void) -> HResult;
    fn CoIncrementMTAUsage(cookie: *mut usize) -> HResult;
    fn CoDecrementMTAUsage(cookie: usize) -> HResult;
    fn CoGetApartmentType(kind: *mut i32, qualifier: *mut i32) -> HResult;
    fn RoGetErrorReportingFlags(flags: *mut u32) -> HResult;
    fn RoOriginateErrorW(hr: HResult, max: u32, message: *const u16) -> i32;
    fn GetRestrictedErrorInfo(out: *mut *mut c_void) -> HResult;
}

/// HSTRING_HEADER (24 bytes on x64, 20 on x86).
#[cfg(target_arch = "x86_64")]
type Header = [usize; 3];
#[cfg(target_arch = "x86")]
type Header = [usize; 5];

/// The layout Windows gives an HSTRING's header (Wine's string tests).
#[repr(C)]
struct HStringHeader {
    flags: u32,
    len: u32,
    _unused: [u32; 2],
    text: *const u16,
}

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const E_BOUNDS: HResult = 0x8000_000bu32 as i32;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0u32 as i32;
const RPC_E_CHANGED_MODE: HResult = 0x8001_0106u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const RO_INIT_SINGLETHREADED: u32 = 0;
const RO_INIT_MULTITHREADED: u32 = 1;
const RO_ERROR_REPORTING_USESETERRORINFO: u32 = 4;
const APTTYPE_MTA: i32 = 1;
const APTTYPEQUALIFIER_IMPLICIT_MTA: i32 = 1;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_IACTIVATIONFACTORY: Guid = guid(0x35, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// {9A7D4B65-5E8F-4FC7-A2E5-7F6925CB8B53}
const IID_IPACKAGEMANAGER: Guid =
    guid(0x9a7d4b65, 0x5e8f, 0x4fc7, [0xa2, 0xe5, 0x7f, 0x69, 0x25, 0xcb, 0x8b, 0x53]);
/// {82BA7092-4C88-427D-A7BC-16DD93FEB67E}
const IID_IRESTRICTEDERRORINFO: Guid =
    guid(0x82ba7092, 0x4c88, 0x427d, [0xa7, 0xbc, 0x16, 0xdd, 0x93, 0xfe, 0xb6, 0x7e]);

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    /// An HRESULT that should be `want`.
    #[track_caller]
    fn hr(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

/// `text` as UTF-16 (ASCII), NUL-terminated, in N units.
const fn wide<const N: usize>(text: &[u8]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < text.len() {
        out[i] = text[i] as u16;
        i += 1;
    }
    out
}

/// "abcdef" and its NUL (Wine's input_string).
static ABCDEF: [u16; 8] = wide(b"abcdef");
static ABC: [u16; 4] = wide(b"abc");
static DEF: [u16; 4] = wide(b"def");
/// "a\0c\0ef" and its NUL (Wine's input_embed_null).
static EMBED_NULL: [u16; 7] = [b'a' as u16, 0, b'c' as u16, 0, b'e' as u16, b'f' as u16, 0];

/// Wine's check_string: emptiness, embedded NULs, length, the raw buffer
/// (never NULL, NUL-terminated) and its content.
#[track_caller]
fn check_string(c: &mut Checks, s: HString, want: &[u16], has_null: bool) -> Result<(), u32> {
    let mut null = -1;
    let mut len = 99;
    // SAFETY: a string the test made (or NULL); outputs on the stack.
    unsafe {
        c.ok((WindowsIsStringEmpty(s) != 0) == want.is_empty(), 0)?;
        c.hr(WindowsStringHasEmbeddedNull(s, &mut null), S_OK)?;
        c.ok((null != 0) == has_null, null as u64)?;
        c.ok(WindowsGetStringLen(s) as usize == want.len(), u64::from(WindowsGetStringLen(s)))?;
        let p = WindowsGetStringRawBuffer(s, &mut len);
        c.ok(!p.is_null() && len as usize == want.len(), u64::from(len))?;
        let p = WindowsGetStringRawBuffer(s, null_mut());
        c.ok(!p.is_null() && *p.add(want.len()) == 0, 0)?;
        c.ok(core::slice::from_raw_parts(p, want.len()) == want, 0)?;
    }
    Ok(())
}

fn create_delete(c: &mut Checks) -> Result<(), u32> {
    let mut s: HString = null_mut();
    let mut header: Header = [0; _];
    // SAFETY: the test's strings and outputs.
    unsafe {
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..6], false)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, null_mut()), E_INVALIDARG)?;
        c.hr(WindowsCreateString(null(), 6, &mut s), E_POINTER)?;
        c.hr(WindowsDeleteString(null_mut()), S_OK)?;

        c.hr(WindowsCreateStringReference(ABCDEF.as_ptr(), 6, &mut header, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..6], false)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        // Not NUL-terminated at the length given (Windows: 0x80000017,
        // Wine notes; the documentation says E_INVALIDARG), even at 0.
        let hr = WindowsCreateStringReference(ABCDEF.as_ptr(), 5, &mut header, &mut s);
        c.ok(hr < 0, hr as u32 as u64)?;
        let hr = WindowsCreateStringReference(ABCDEF.as_ptr(), 0, &mut header, &mut s);
        c.ok(hr < 0, hr as u32 as u64)?;
        c.hr(WindowsCreateStringReference(ABCDEF.as_ptr(), 6, null_mut(), &mut s), E_INVALIDARG)?;
        c.hr(
            WindowsCreateStringReference(ABCDEF.as_ptr(), 6, &mut header, null_mut()),
            E_INVALIDARG,
        )?;
        c.hr(WindowsCreateStringReference(null(), 6, &mut header, &mut s), E_POINTER)?;

        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 3, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..3], false)?;
        c.hr(WindowsDeleteString(s), S_OK)?;

        // Empty strings are NULL.
        let empty = [0u16; 1];
        for (text, reference) in
            [(empty.as_ptr(), false), (ABCDEF.as_ptr(), false), (null(), false)]
                .into_iter()
                .chain([(empty.as_ptr(), true), (null(), true)])
        {
            s = 1 as HString;
            let hr = if reference {
                WindowsCreateStringReference(text, 0, &mut header, &mut s)
            } else {
                WindowsCreateString(text, 0, &mut s)
            };
            c.hr(hr, S_OK)?;
            c.ok(s.is_null(), s as u64)?;
            c.hr(WindowsDeleteString(s), S_OK)?;
        }
    }
    Ok(())
}

fn duplicate_and_access(c: &mut Checks) -> Result<(), u32> {
    let (mut s, mut s2): (HString, HString) = (null_mut(), null_mut());
    let mut header: Header = [0; _];
    // SAFETY: the test's strings and outputs.
    unsafe {
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut s), S_OK)?;
        c.hr(WindowsDuplicateString(s, null_mut()), E_INVALIDARG)?;
        c.hr(WindowsDuplicateString(s, &mut s2), S_OK)?;
        c.ok(s == s2, s2 as u64)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        c.hr(WindowsDeleteString(s2), S_OK)?;

        c.hr(WindowsCreateStringReference(ABCDEF.as_ptr(), 6, &mut header, &mut s), S_OK)?;
        c.hr(WindowsDuplicateString(s, &mut s2), S_OK)?;
        c.ok(s != s2, s2 as u64)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        c.hr(WindowsDeleteString(s2), S_OK)?;

        c.hr(WindowsDuplicateString(null_mut(), &mut s2), S_OK)?;
        c.ok(s2.is_null(), s2 as u64)?;

        check_string(c, null_mut(), &[], false)?;
        c.hr(WindowsCreateString(EMBED_NULL.as_ptr(), 6, &mut s), S_OK)?;
        check_string(c, s, &EMBED_NULL[..6], true)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        c.hr(WindowsCreateStringReference(EMBED_NULL.as_ptr(), 6, &mut header, &mut s), S_OK)?;
        check_string(c, s, &EMBED_NULL[..6], true)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        // The terminator counted in: an embedded NUL.
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 7, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..7], true)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        let mut null_flag = 0;
        c.hr(WindowsStringHasEmbeddedNull(s2, &mut null_flag), S_OK)?;
        c.hr(WindowsStringHasEmbeddedNull(s2, null_mut()), E_INVALIDARG)?;
    }
    Ok(())
}

/// The headers' layout as Windows has it (Wine's test_hstring_struct).
fn layout(c: &mut Checks) -> Result<(), u32> {
    let (mut s, mut s2): (HString, HString) = (null_mut(), null_mut());
    let mut header: Header = [0; _];
    c.ok(core::mem::size_of::<Header>() == core::mem::size_of::<HStringHeader>(), 0)?;
    // SAFETY: the test's strings; their headers read as Windows lays them
    // out, an allocated string's count and text after its header.
    unsafe {
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut s), S_OK)?;
        let h = &*(s as *const HStringHeader);
        let refs = (s as *const u8).add(core::mem::size_of::<HStringHeader>()).cast::<i32>();
        let text = refs.add(1).cast::<u16>();
        c.ok(h.flags == 0, u64::from(h.flags))?;
        c.ok(h.len == 6, u64::from(h.len))?;
        c.ok(h.text == text, h.text as u64)?;
        c.ok(*refs == 1, *refs as u64)?;
        c.ok(core::slice::from_raw_parts(text, 7) == &ABCDEF[..7], 0)?;
        c.hr(WindowsDuplicateString(s, &mut s2), S_OK)?;
        c.ok(*refs == 2, *refs as u64)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
        c.ok(*refs == 1, *refs as u64)?;
        c.hr(WindowsDeleteString(s2), S_OK)?;

        c.hr(WindowsCreateStringReference(ABCDEF.as_ptr(), 6, &mut header, &mut s), S_OK)?;
        c.ok(s == (&raw mut header).cast(), s as u64)?;
        let h = &*(s as *const HStringHeader);
        c.ok(h.flags == 1 && h.len == 6 && h.text == ABCDEF.as_ptr(), u64::from(h.flags))?;
        c.hr(WindowsDeleteString(s), S_OK)?;
    }
    Ok(())
}

fn buffers(c: &mut Checks) -> Result<(), u32> {
    let mut buf: HString = null_mut();
    let mut p: *mut u16 = null_mut();
    let mut s: HString = 1 as HString;
    // SAFETY: the test's strings and outputs; the buffer's units.
    unsafe {
        c.hr(WindowsPreallocateStringBuffer(0, &mut p, &mut buf), S_OK)?;
        c.ok(!p.is_null(), 0)?;
        c.hr(WindowsPromoteStringBuffer(buf, &mut s), S_OK)?;
        c.ok(s.is_null(), s as u64)?;
        c.hr(WindowsDeleteStringBuffer(null_mut()), S_OK)?;

        c.hr(WindowsPreallocateStringBuffer(6, &mut p, &mut buf), S_OK)?;
        c.hr(WindowsDeleteStringBuffer(buf), S_OK)?;

        c.hr(WindowsPreallocateStringBuffer(6, &mut p, &mut buf), S_OK)?;
        c.ok(*p.add(6) == 0, u64::from(*p.add(6)))?;
        core::ptr::copy_nonoverlapping(ABCDEF.as_ptr(), p, 6);
        c.hr(WindowsPromoteStringBuffer(buf, null_mut()), E_POINTER)?;
        c.hr(WindowsPromoteStringBuffer(buf, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..6], false)?;
        c.hr(WindowsDeleteString(s), S_OK)?;

        c.hr(WindowsPreallocateStringBuffer(6, null_mut(), &mut buf), E_POINTER)?;
        c.hr(WindowsPreallocateStringBuffer(6, &mut p, null_mut()), E_POINTER)?;

        c.hr(WindowsPreallocateStringBuffer(6, &mut p, &mut buf), S_OK)?;
        *p.add(6) = u16::from(b'a');
        c.hr(WindowsPromoteStringBuffer(buf, &mut s), E_INVALIDARG)?;
        c.hr(WindowsDeleteStringBuffer(buf), S_OK)?;

        c.hr(WindowsPreallocateStringBuffer(7, &mut p, &mut buf), S_OK)?;
        core::ptr::copy_nonoverlapping(ABCDEF.as_ptr(), p, 7);
        c.hr(WindowsPromoteStringBuffer(buf, &mut s), S_OK)?;
        check_string(c, s, &ABCDEF[..7], true)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
    }
    Ok(())
}

fn substrings(c: &mut Checks) -> Result<(), u32> {
    let (mut s, mut sub): (HString, HString) = (null_mut(), null_mut());
    let mut header: Header = [0; _];
    // SAFETY: the test's strings and outputs.
    unsafe {
        for reference in [false, true] {
            if reference {
                c.hr(WindowsCreateStringReference(ABCDEF.as_ptr(), 6, &mut header, &mut s), S_OK)?;
            } else {
                c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut s), S_OK)?;
            }
            c.hr(WindowsSubstring(s, 2, &mut sub), S_OK)?;
            check_string(c, sub, &ABCDEF[2..6], false)?;
            c.hr(WindowsDeleteString(sub), S_OK)?;
            c.hr(WindowsSubstringWithSpecifiedLength(s, 2, 3, &mut sub), S_OK)?;
            check_string(c, sub, &ABCDEF[2..5], false)?;
            c.hr(WindowsDeleteString(sub), S_OK)?;
            // The whole string: a new one all the same.
            c.hr(WindowsSubstring(s, 0, &mut sub), S_OK)?;
            c.ok(sub != s, sub as u64)?;
            check_string(c, sub, &ABCDEF[..6], false)?;
            c.hr(WindowsDeleteString(sub), S_OK)?;
            c.hr(WindowsSubstringWithSpecifiedLength(s, 0, 6, &mut sub), S_OK)?;
            c.ok(sub != s, sub as u64)?;
            c.hr(WindowsDeleteString(sub), S_OK)?;
            c.hr(WindowsDeleteString(s), S_OK)?;
        }
        c.hr(WindowsSubstring(null_mut(), 0, &mut sub), S_OK)?;
        c.ok(sub.is_null(), sub as u64)?;
        c.hr(WindowsSubstringWithSpecifiedLength(null_mut(), 0, 0, &mut sub), S_OK)?;
        c.ok(sub.is_null(), sub as u64)?;

        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut s), S_OK)?;
        c.hr(WindowsSubstring(s, 6, &mut sub), S_OK)?;
        c.ok(sub.is_null(), sub as u64)?;
        c.hr(WindowsSubstringWithSpecifiedLength(s, 6, 0, &mut sub), S_OK)?;
        c.ok(sub.is_null(), sub as u64)?;
        c.hr(WindowsSubstring(s, 7, &mut sub), E_BOUNDS)?;
        c.hr(WindowsSubstringWithSpecifiedLength(s, 7, 0, &mut sub), E_BOUNDS)?;
        c.hr(WindowsSubstringWithSpecifiedLength(s, 6, 1, &mut sub), E_BOUNDS)?;
        c.hr(WindowsSubstringWithSpecifiedLength(s, 7, !0, &mut sub), E_BOUNDS)?;
        c.hr(WindowsSubstring(s, 7, null_mut()), E_INVALIDARG)?;
        c.hr(WindowsSubstringWithSpecifiedLength(s, 7, 0, null_mut()), E_INVALIDARG)?;
        c.hr(WindowsDeleteString(s), S_OK)?;
    }
    Ok(())
}

fn concat_compare_trim(c: &mut Checks) -> Result<(), u32> {
    let (mut a, mut b, mut out): (HString, HString, HString) = (null_mut(), null_mut(), null_mut());
    let (mut h1, mut h2): (Header, Header) = ([0; _], [0; _]);
    let mut order = 5;
    // SAFETY: the test's strings and outputs.
    unsafe {
        for reference in [false, true] {
            if reference {
                c.hr(WindowsCreateStringReference(ABC.as_ptr(), 3, &mut h1, &mut a), S_OK)?;
                c.hr(WindowsCreateStringReference(DEF.as_ptr(), 3, &mut h2, &mut b), S_OK)?;
            } else {
                c.hr(WindowsCreateString(ABC.as_ptr(), 3, &mut a), S_OK)?;
                c.hr(WindowsCreateString(DEF.as_ptr(), 3, &mut b), S_OK)?;
            }
            c.hr(WindowsConcatString(a, null_mut(), null_mut()), E_INVALIDARG)?;
            c.hr(WindowsConcatString(a, null_mut(), &mut out), S_OK)?;
            // A copy is the same string; a reference gives a new one.
            c.ok((out == a) != reference, out as u64)?;
            check_string(c, out, &ABC[..3], false)?;
            c.hr(WindowsDeleteString(out), S_OK)?;
            c.hr(WindowsConcatString(null_mut(), b, &mut out), S_OK)?;
            c.ok((out == b) != reference, out as u64)?;
            check_string(c, out, &DEF[..3], false)?;
            c.hr(WindowsDeleteString(out), S_OK)?;
            c.hr(WindowsConcatString(a, b, &mut out), S_OK)?;
            check_string(c, out, &ABCDEF[..6], false)?;
            c.hr(WindowsDeleteString(out), S_OK)?;

            for (x, y, want) in
                [(a, a, 0), (a, b, -1), (b, a, 1), (a, null_mut(), 1), (null_mut(), b, -1)]
            {
                c.hr(WindowsCompareStringOrdinal(x, y, &mut order), S_OK)?;
                c.ok(order == want, order as u64)?;
            }
            c.hr(WindowsDeleteString(a), S_OK)?;
            c.hr(WindowsDeleteString(b), S_OK)?;
        }
        c.hr(WindowsConcatString(null_mut(), null_mut(), null_mut()), E_INVALIDARG)?;
        c.hr(WindowsConcatString(null_mut(), null_mut(), &mut out), S_OK)?;
        c.ok(out.is_null(), out as u64)?;
        c.hr(WindowsCompareStringOrdinal(null_mut(), null_mut(), null_mut()), E_INVALIDARG)?;
        c.hr(WindowsCompareStringOrdinal(null_mut(), null_mut(), &mut order), S_OK)?;
        c.ok(order == 0, order as u64)?;

        // Trimming "abcdef" by "abc" and by "def".
        c.hr(WindowsCreateString(ABCDEF.as_ptr(), 6, &mut a), S_OK)?;
        c.hr(WindowsCreateString(ABC.as_ptr(), 3, &mut b), S_OK)?;
        c.hr(WindowsTrimStringStart(a, b, &mut out), S_OK)?;
        check_string(c, out, &DEF[..3], false)?;
        c.hr(WindowsDeleteString(out), S_OK)?;
        c.hr(WindowsTrimStringEnd(a, b, &mut out), S_OK)?;
        c.ok(out == a, out as u64)?;
        c.hr(WindowsDeleteString(out), S_OK)?;
        c.hr(WindowsDeleteString(b), S_OK)?;
        c.hr(WindowsCreateString(DEF.as_ptr(), 3, &mut b), S_OK)?;
        c.hr(WindowsTrimStringStart(a, b, &mut out), S_OK)?;
        c.ok(out == a, out as u64)?;
        c.hr(WindowsDeleteString(out), S_OK)?;
        c.hr(WindowsTrimStringEnd(a, b, &mut out), S_OK)?;
        check_string(c, out, &ABC[..3], false)?;
        c.hr(WindowsDeleteString(out), S_OK)?;
        c.hr(WindowsDeleteString(b), S_OK)?;
        c.hr(WindowsTrimStringStart(null_mut(), null_mut(), null_mut()), E_INVALIDARG)?;
        c.hr(WindowsTrimStringStart(null_mut(), a, null_mut()), E_INVALIDARG)?;
        c.hr(WindowsTrimStringStart(null_mut(), null_mut(), &mut out), E_INVALIDARG)?;
        c.hr(WindowsTrimStringStart(null_mut(), a, &mut out), S_OK)?;
        c.ok(out.is_null(), out as u64)?;
        c.hr(WindowsTrimStringEnd(null_mut(), null_mut(), &mut out), E_INVALIDARG)?;
        c.hr(WindowsTrimStringEnd(null_mut(), a, &mut out), S_OK)?;
        c.ok(out.is_null(), out as u64)?;
        c.hr(WindowsDeleteString(a), S_OK)?;
    }
    Ok(())
}

/// The vtable entry `i` of COM object `obj`, called as `fn(obj, args)`.
macro_rules! com_call {
    ($obj:expr, $i:expr, fn($($t:ty),* $(,)?) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj as *mut *const usize;
        // SAFETY: a COM object whose vtable has entry $i of this type.
        #[allow(unused_unsafe)]
        let f: extern "system" fn(*mut *const usize $(, $t)*) -> $r =
            unsafe { core::mem::transmute(*(*obj).add($i)) };
        f(obj $(, $a)*)
    }};
}

fn release(obj: *mut c_void) -> u32 {
    com_call!(obj, 2, fn() -> u32)
}

static MISSING: [u16; 15] = wide(b"Does.Not.Exist");
static PACKAGE_MANAGER: [u16; 45] = wide(b"Windows.Management.Deployment.PackageManager");
/// The Store (MSIX) Office's package family.
static OFFICE_FAMILY: [u16; 39] = wide(b"Microsoft.Office.Desktop_8wekyb3d8bbwe");

/// An HSTRING reference to `text` (up to its NUL).
fn reference(text: &[u16], header: &mut Header) -> HString {
    let len = text.iter().position(|&c| c == 0).unwrap_or(text.len() - 1);
    let mut s = null_mut();
    // SAFETY: the caller's text, NUL-terminated at `len`.
    unsafe { WindowsCreateStringReference(text.as_ptr(), len as u32, header, &mut s) };
    s
}

fn activation(c: &mut Checks) -> Result<(), u32> {
    let mut header: Header = [0; _];
    let missing = reference(&MISSING, &mut header);
    let mut factory: *mut c_void = core::ptr::dangling_mut::<c_void>();
    let (mut kind, mut qualifier) = (5, 5);
    // SAFETY: the test's strings and outputs.
    unsafe {
        // No apartment yet (nor an MTA in the process).
        c.hr(
            RoGetActivationFactory(missing, &IID_IACTIVATIONFACTORY, &mut factory),
            CO_E_NOTINITIALIZED,
        )?;
        c.hr(RoInitialize(RO_INIT_MULTITHREADED), S_OK)?;
        c.hr(RoInitialize(RO_INIT_MULTITHREADED), S_FALSE)?;
        c.hr(RoInitialize(RO_INIT_SINGLETHREADED), RPC_E_CHANGED_MODE)?;
        factory = core::ptr::dangling_mut::<c_void>();
        c.hr(
            RoGetActivationFactory(missing, &IID_IACTIVATIONFACTORY, &mut factory),
            REGDB_E_CLASSNOTREG,
        )?;
        c.ok(factory.is_null(), factory as u64)?;
        let mut instance: *mut c_void = core::ptr::dangling_mut::<c_void>();
        c.hr(RoActivateInstance(missing, &mut instance), REGDB_E_CLASSNOTREG)?;
        c.ok(instance.is_null(), instance as u64)?;
        RoUninitialize();
        RoUninitialize();

        // CoIncrementMTAUsage: this thread, in no apartment, is in the
        // implicit MTA.
        c.hr(CoGetApartmentType(&mut kind, &mut qualifier), CO_E_NOTINITIALIZED)?;
        let mut cookie = 0;
        c.hr(CoIncrementMTAUsage(&mut cookie), S_OK)?;
        c.ok(cookie != 0, 0)?;
        c.hr(CoGetApartmentType(&mut kind, &mut qualifier), S_OK)?;
        c.ok(kind == APTTYPE_MTA && qualifier == APTTYPEQUALIFIER_IMPLICIT_MTA, kind as u64)?;
        c.hr(
            RoGetActivationFactory(missing, &IID_IACTIVATIONFACTORY, &mut factory),
            REGDB_E_CLASSNOTREG,
        )?;
        c.hr(CoDecrementMTAUsage(cookie), S_OK)?;
    }
    Ok(())
}

fn package_manager(c: &mut Checks) -> Result<(), u32> {
    let mut header: Header = [0; _];
    let mut family_header: Header = [0; _];
    let class = reference(&PACKAGE_MANAGER, &mut header);
    let family = reference(&OFFICE_FAMILY, &mut family_header);
    let mut factory: *mut c_void = null_mut();
    // SAFETY: the test's strings and outputs.
    unsafe {
        c.hr(RoInitialize(RO_INIT_MULTITHREADED), S_OK)?;
        c.hr(RoGetActivationFactory(class, &IID_IACTIVATIONFACTORY, &mut factory), S_OK)?;
        c.ok(!factory.is_null(), 0)?;
        // IActivationFactory::ActivateInstance.
        let mut instance: *mut c_void = null_mut();
        c.hr(com_call!(factory, 6, fn(*mut *mut c_void) -> HResult, &mut instance), S_OK)?;
        c.ok(!instance.is_null(), 0)?;
        release(factory);
        let mut manager: *mut c_void = null_mut();
        c.hr(
            com_call!(
                instance,
                0,
                fn(*const Guid, *mut *mut c_void) -> HResult,
                &IID_IPACKAGEMANAGER,
                &mut manager
            ),
            S_OK,
        )?;
        release(instance);
        // IInspectable::GetRuntimeClassName.
        let mut name: HString = null_mut();
        c.hr(com_call!(manager, 4, fn(*mut HString) -> HResult, &mut name), S_OK)?;
        let mut order = 5;
        c.hr(WindowsCompareStringOrdinal(name, class, &mut order), S_OK)?;
        c.ok(order == 0, order as u64)?;
        WindowsDeleteString(name);
        // FindPackagesByUserSecurityIdPackageFamilyName("", family): the
        // current user's; no Store Office.
        let mut packages: *mut c_void = null_mut();
        c.hr(
            com_call!(
                manager,
                20,
                fn(HString, HString, *mut *mut c_void) -> HResult,
                null_mut(),
                family,
                &mut packages
            ),
            S_OK,
        )?;
        c.ok(!packages.is_null(), 0)?;
        // IIterable::First, IIterator::get_HasCurrent.
        let mut iterator: *mut c_void = null_mut();
        c.hr(com_call!(packages, 6, fn(*mut *mut c_void) -> HResult, &mut iterator), S_OK)?;
        c.ok(!iterator.is_null(), 0)?;
        let mut has = 7u8;
        c.hr(com_call!(iterator, 7, fn(*mut u8) -> HResult, &mut has), S_OK)?;
        c.ok(has == 0, u64::from(has))?;
        release(iterator);
        release(packages);
        release(manager);

        let mut instance: *mut c_void = null_mut();
        c.hr(RoActivateInstance(class, &mut instance), S_OK)?;
        c.ok(!instance.is_null(), 0)?;
        release(instance);
        RoUninitialize();
    }
    Ok(())
}

fn errors(c: &mut Checks) -> Result<(), u32> {
    static MESSAGE: [u16; 10] = wide(b"gala test");
    let mut flags = 0;
    // SAFETY: the test's outputs; COM objects as documented.
    unsafe {
        c.hr(RoGetErrorReportingFlags(null_mut()), E_POINTER)?;
        c.hr(RoGetErrorReportingFlags(&mut flags), S_OK)?;
        c.ok(flags == RO_ERROR_REPORTING_USESETERRORINFO, u64::from(flags))?;
        c.ok(RoOriginateErrorW(S_OK, 0, MESSAGE.as_ptr()) == 0, 0)?;
        c.ok(RoOriginateErrorW(E_FAIL, 0, MESSAGE.as_ptr()) != 0, 0)?;
        let mut info: *mut c_void = null_mut();
        c.hr(GetRestrictedErrorInfo(&mut info), S_OK)?;
        c.ok(!info.is_null(), 0)?;
        let mut restricted: *mut c_void = null_mut();
        c.hr(
            com_call!(
                info,
                0,
                fn(*const Guid, *mut *mut c_void) -> HResult,
                &IID_IRESTRICTEDERRORINFO,
                &mut restricted
            ),
            S_OK,
        )?;
        // IRestrictedErrorInfo::GetErrorDetails.
        let (mut description, mut error, mut text, mut sid) =
            (null_mut::<u16>(), 0, null_mut::<u16>(), null_mut::<u16>());
        c.hr(
            com_call!(
                restricted,
                3,
                fn(*mut *mut u16, *mut HResult, *mut *mut u16, *mut *mut u16) -> HResult,
                &mut description,
                &mut error,
                &mut text,
                &mut sid
            ),
            S_OK,
        )?;
        c.hr(error, E_FAIL)?;
        c.ok(!text.is_null() && SysStringLen(text) == 9, 0)?;
        c.ok(core::slice::from_raw_parts(text, 9) == &MESSAGE[..9], 0)?;
        for b in [description, text, sid] {
            SysFreeString(b);
        }
        release(restricted);
        release(info);
        // Handed over: none left.
        let mut again: *mut c_void = core::ptr::dangling_mut::<c_void>();
        let hr = GetRestrictedErrorInfo(&mut again);
        c.ok(hr >= 0 && again.is_null(), hr as u32 as u64)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    create_delete(&mut c)?;
    duplicate_and_access(&mut c)?;
    layout(&mut c)?;
    buffers(&mut c)?;
    substrings(&mut c)?;
    concat_compare_trim(&mut c)?;
    activation(&mut c)?;
    package_manager(&mut c)?;
    errors(&mut c)?;
    cancellation(&mut c)?;
    Ok(())
}

/// ole32's call cancellation (ODT's setup.exe imports it): CoCancelCall
/// before CoEnableCallCancellation CO_E_CANCEL_DISABLED; two enables
/// S_OK; with no call pending CoCancelCall finds no cancel object
/// (E_NOINTERFACE); the first disable CO_E_CANCEL_DISABLED (an enable
/// remains), the second S_OK, and CoCancelCall is disabled again.
fn cancellation(c: &mut Checks) -> Result<(), u32> {
    const CO_E_CANCEL_DISABLED: HResult = 0x8001_0140u32 as i32;
    const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
    // SAFETY: plain calls.
    unsafe {
        c.hr(CoCancelCall(0, 0), CO_E_CANCEL_DISABLED)?;
        c.hr(CoEnableCallCancellation(null_mut()), S_OK)?;
        c.hr(CoEnableCallCancellation(null_mut()), S_OK)?;
        c.hr(CoCancelCall(0, 0), E_NOINTERFACE)?;
        c.hr(CoDisableCallCancellation(null_mut()), CO_E_CANCEL_DISABLED)?;
        c.hr(CoDisableCallCancellation(null_mut()), S_OK)?;
        c.hr(CoCancelCall(0, 0), CO_E_CANCEL_DISABLED)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

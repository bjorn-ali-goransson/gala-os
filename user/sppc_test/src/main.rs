//! sppc.dll, the Software Protection Platform's client (slpublic.h), as
//! Office's Click-to-Run integrator asks it while it installs Office's
//! licenses (ROADMAP OF). Expected values are those Microsoft's
//! documentation of the functions states and Windows answers:
//!
//! - SLOpen gives S_OK and a handle, SLClose S_OK (measured on Windows,
//!   ROADMAP issue #60);
//! - asking about an application, SKU, license file or product key that
//!   is not installed fails, its identifier among the questions, as
//!   does installing a blob that is not a
//!   license, uninstalling a license file that is not installed and
//!   installing a product key that is not one;
//! - a NULL output pointer fails;
//! - the policy getters answer SL_E_RIGHT_NOT_GRANTED while no right has
//!   been consumed on the handle, whatever the value name, and
//!   E_INVALIDARG for a NULL handle or a NULL required output
//!   (measured on Windows 11 24H2, build 26100; the data type of
//!   SLGetPolicyInformation is optional, and the two differ over the
//!   value name: the DWORD form refuses a NULL one, its sibling answers
//!   the right first and never looks).
//! - the application policies are missing, since nothing persists any:
//!   SLLoadApplicationPolicies answers SL_E_APPLICATION_POLICIES_MISSING
//!   and hands out no context, so SLGetApplicationPolicy and
//!   SLUnloadApplicationPolicies answer
//!   SL_E_APPLICATION_POLICIES_NOT_LOADED for whatever context they are
//!   given, after E_INVALIDARG for a NULL one, a NULL value name or a
//!   NULL required output -- the arguments are checked before the
//!   context is looked up, and the data type stays optional (measured on
//!   Windows 11 24H2, build 26100, where no application has persisted
//!   policies).
//!
//! - no plugin can be registered: SLRegisterPlugin and
//!   SLUnregisterPlugin, which no public header declares, answer
//!   SL_E_LUA_ACCESSDENIED once their pointers and their handle are
//!   good (measured on Windows 11 24H2, build 26100, as the standard
//!   user Office runs as -- an elevated caller is not measured, and
//!   this test is not one), E_INVALIDARG for a NULL handle or a NULL
//!   pointer, the pointers first, and RPC_NT_SS_CONTEXT_MISMATCH for a
//!   handle that is not one, their handles being RPC context handles.
//!
//! - authentication data is set but never read back:
//!   SLSetAuthenticationData, which no public header declares and
//!   which has no getter beside it, takes a count and a buffer and
//!   answers S_OK for a count of zero, ERROR_BUFFER_OVERFLOW above
//!   1024 bytes (before it looks at either the buffer or the handle)
//!   and E_INVALIDARG for a count with no buffer (also before the
//!   handle). Windows reads a blob of 1 to 1024 bytes and its service
//!   refuses filler with ERROR_INVALID_DATA; gala has no such service
//!   and accepts what it cannot refuse, so that one case is the only
//!   answer the two do not share and this test does not pin it to a
//!   code (measured on Windows 11 24H2, build 26100, as the standard
//!   user Office runs as).
//!
//! - and that last answer is every call's: a handle no SLOpen gave,
//!   never opened or closed, is RPC_NT_SS_CONTEXT_MISMATCH from each
//!   call that takes one, the arguments looked at first, while a NULL
//!   handle is E_INVALIDARG (measured on Windows 11 24H2, build
//!   26100, call by call).
//!
//! - offline activation stops at the store: for a SKU that is not
//!   installed SLGenerateOfflineInstallationId answers
//!   SL_E_VALUE_NOT_FOUND and leaves its output untouched, while
//!   SLDepositOfflineConfirmationId answers
//!   SL_E_PRODUCT_SKU_NOT_INSTALLED for that same SKU, before it reads
//!   either string; the Ex forms refuse every activation info that is
//!   not NULL with E_INVALIDARG (measured, this machine belonging to no
//!   Active Directory). What they answer for a SKU that is installed is
//!   gala's own (SL_E_PKEY_NOT_INSTALLED, no product key being
//!   installed), so it is checked in the round trip, which Windows
//!   skips.
//!
//! - a SKU the store does not hold is SL_E_PRODUCT_SKU_NOT_INSTALLED
//!   (0xC004F015) from SLGetLicensingStatusInformation and
//!   SLConsumeRight, which both require an application identifier
//!   (E_INVALIDARG for a NULL one, measured, where a NULL SKU is
//!   taken).
//!
//! The error codes of those failures are printed as `sppc_test: info`
//! lines (FAILED is checked), to be compared with a Windows run.
//!
//! The round trip of a license through the store (installed, found by
//! application, SKU, license and license file, its metadata read, its
//! status, uninstalled) uses a license made here, which is not signed:
//! Windows refuses it (it verifies XrML signatures, which gala does not:
//! ARCHITECTURE "sppc"), so that part runs only where the license is
//! taken and prints `sppc_test: info round trip skipped` elsewhere.
//!
//! Prints `sppc_test: ok`, or `sppc_test: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};

use ntrt as _;

const NAME: &str = "sppc_test";

type Handle = usize;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// SL_LICENSING_STATUS.
#[repr(C)]
struct LicensingStatus {
    sku: Guid,
    status: u32,
    grace_time: u32,
    total_grace_days: u32,
    reason: i32,
    validity_expiration: u64,
}

/// SL_ACTIVATION_INFO_HEADER (slpublic.h): what the Ex forms of the
/// offline activation calls take, and refuse outside an Active
/// Directory.
#[repr(C)]
struct ActivationInfo {
    size: u32,
    /// SL_ACTIVATION_TYPE: 0 default, 1 Active Directory.
    kind: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn LocalFree(p: *mut u8) -> *mut u8;
}

#[link(name = "sppc", kind = "raw-dylib")]
unsafe extern "system" {
    fn SLOpen(handle: *mut Handle) -> i32;
    fn SLClose(handle: Handle) -> i32;
    fn SLInstallLicense(h: Handle, size: u32, blob: *const u8, file_id: *mut Guid) -> i32;
    fn SLUninstallLicense(h: Handle, file_id: *const Guid) -> i32;
    fn SLGetSLIDList(
        h: Handle,
        query_type: u32,
        query: *const Guid,
        return_type: u32,
        count: *mut u32,
        ids: *mut *mut Guid,
    ) -> i32;
    fn SLGetLicenseInformation(
        h: Handle,
        license: *const Guid,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLGetProductSkuInformation(
        h: Handle,
        sku: *const Guid,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLGetLicensingStatusInformation(
        h: Handle,
        app: *const Guid,
        sku: *const Guid,
        right: *const u16,
        count: *mut u32,
        status: *mut *mut LicensingStatus,
    ) -> i32;
    fn SLGetPolicyInformationDWORD(h: Handle, name: *const u16, value: *mut u32) -> i32;
    fn SLGetPolicyInformation(
        h: Handle,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLLoadApplicationPolicies(
        app: *const Guid,
        sku: *const Guid,
        flags: u32,
        context: *mut Handle,
    ) -> i32;
    fn SLGetApplicationPolicy(
        context: Handle,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLUnloadApplicationPolicies(context: Handle, flags: u32) -> i32;
    fn SLGetInstalledProductKeyIds(
        h: Handle,
        sku: *const Guid,
        count: *mut u32,
        ids: *mut *mut Guid,
    ) -> i32;
    fn SLInstallProofOfPurchase(
        h: Handle,
        algorithm: *const u16,
        key: *const u16,
        size: u32,
        data: *const u8,
        pkey_id: *mut Guid,
    ) -> i32;
    fn SLGetPKeyId(
        h: Handle,
        algorithm: *const u16,
        key: *const u16,
        size: u32,
        data: *const u8,
        pkey_id: *mut Guid,
    ) -> i32;
    fn SLGetPKeyInformation(
        h: Handle,
        pkey: *const Guid,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLRegisterPlugin(h: Handle, plugin: *const u8, out: *mut u8) -> i32;
    fn SLUnregisterPlugin(h: Handle, plugin: *const u8) -> i32;
    fn SLSetAuthenticationData(h: Handle, cb: u32, data: *const u8) -> i32;
    fn SLConsumeRight(
        h: Handle,
        app: *const Guid,
        sku: *const Guid,
        right: *const u16,
        reserved: usize,
    ) -> i32;
    fn SLUninstallProofOfPurchase(h: Handle, pkey: *const Guid) -> i32;
    fn SLGenerateOfflineInstallationId(h: Handle, sku: *const Guid, out: *mut *mut u16) -> i32;
    fn SLGenerateOfflineInstallationIdEx(
        h: Handle,
        sku: *const Guid,
        activation: *const ActivationInfo,
        out: *mut *mut u16,
    ) -> i32;
    fn SLDepositOfflineConfirmationId(
        h: Handle,
        sku: *const Guid,
        installation: *const u16,
        confirmation: *const u16,
    ) -> i32;
    fn SLDepositOfflineConfirmationIdEx(
        h: Handle,
        sku: *const Guid,
        activation: *const ActivationInfo,
        installation: *const u16,
        confirmation: *const u16,
    ) -> i32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
/// ERROR_INVALID_DATA as an HRESULT: the service read the blob and refused it.
const E_INVALID_DATA: i32 = 0x8007_000du32 as i32;
/// ERROR_BUFFER_OVERFLOW as an HRESULT: a count sppc will not read that far.
const E_BUFFER_OVERFLOW: i32 = 0x8007_006fu32 as i32;
/// SL_E_VALUE_NOT_FOUND.
const SL_E_VALUE_NOT_FOUND: i32 = 0xc004_f012u32 as i32;
/// SL_E_RIGHT_NOT_GRANTED.
const SL_E_RIGHT_NOT_GRANTED: i32 = 0xc004_f013u32 as i32;
/// SL_E_PKEY_NOT_INSTALLED.
const SL_E_PKEY_NOT_INSTALLED: i32 = 0xc004_f014u32 as i32;
/// SL_E_PRODUCT_SKU_NOT_INSTALLED (not 0xC004F069, which is
/// SL_E_MISMATCHED_PRODUCT_SKU).
const SL_E_PRODUCT_SKU_NOT_INSTALLED: i32 = 0xc004_f015u32 as i32;
/// SL_E_APPLICATION_POLICIES_MISSING.
const SL_E_APPLICATION_POLICIES_MISSING: i32 = 0xc004_f072u32 as i32;
/// SL_E_APPLICATION_POLICIES_NOT_LOADED.
const SL_E_APPLICATION_POLICIES_NOT_LOADED: i32 = 0xc004_f073u32 as i32;
/// SL_E_LUA_ACCESSDENIED (slerror.h): the caller is not elevated.
const SL_E_LUA_ACCESSDENIED: i32 = 0xc004_f025u32 as i32;
/// RPC_NT_SS_CONTEXT_MISMATCH (ntstatus.h): not a context handle.
const RPC_NT_SS_CONTEXT_MISMATCH: i32 = 0xc003_0005u32 as i32;
/// SLIDTYPE.
const SL_ID_APPLICATION: u32 = 0;
const SL_ID_PRODUCT_SKU: u32 = 1;
const SL_ID_LICENSE_FILE: u32 = 2;
const SL_ID_LICENSE: u32 = 3;
/// SL_DATA_SZ (REG_SZ).
const SL_DATA_SZ: u32 = 1;
/// SL_LICENSING_STATUS_UNLICENSED.
const SL_LICENSING_STATUS_UNLICENSED: u32 = 0;

/// The application, SKU and licenses of the license made here (GUIDs
/// made for this test; no product uses them).
const APP: Guid =
    guid(0x5aa0_c7e1, 0x2f4e, 0x4d3b, [0x9a, 0x61, 0x1f, 0x52, 0x60, 0x7e, 0x3c, 0x01]);
const SKU: Guid =
    guid(0x5aa0_c7e1, 0x2f4e, 0x4d3b, [0x9a, 0x61, 0x1f, 0x52, 0x60, 0x7e, 0x3c, 0x02]);
const PUBLIC: Guid =
    guid(0x5aa0_c7e1, 0x2f4e, 0x4d3b, [0x9a, 0x61, 0x1f, 0x52, 0x60, 0x7e, 0x3c, 0x03]);
const PRIVATE: Guid =
    guid(0x5aa0_c7e1, 0x2f4e, 0x4d3b, [0x9a, 0x61, 0x1f, 0x52, 0x60, 0x7e, 0x3c, 0x04]);
/// An identifier nothing is installed under.
const NOBODY: Guid =
    guid(0x5aa0_c7e1, 0x2f4e, 0x4d3b, [0x9a, 0x61, 0x1f, 0x52, 0x60, 0x7e, 0x3c, 0xff]);

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
    Guid { data1, data2, data3, data4 }
}

/// A license group as Office's `-ul-oob.xrm-ms` files are: a public and
/// a private EUL license of one SKU, the metadata in `tm:infoStr`
/// (unsigned: Windows refuses it).
const LICENSE: &[u8] = b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<rg:licenseGroup xmlns:rg=\"urn:mpeg:mpeg21:2003:01-REL-R-NS\">\
<r:license xmlns:r=\"urn:mpeg:mpeg21:2003:01-REL-R-NS\" licenseId=\"{5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c03}\" \
xmlns:tm=\"http://www.microsoft.com/DRM/XrML2/TM/v2\"><r:title>sppc_test (Public)</r:title>\
<r:otherInfo><tm:infoTables><tm:infoList tag=\"#global\">\
<tm:infoStr name=\"licenseType\">msft:sl/EUL/GENERIC/PUBLIC</tm:infoStr>\
<tm:infoStr name=\"productSkuId\">{5AA0C7E1-2F4E-4D3B-9A61-1F52607E3C02}</tm:infoStr>\
<tm:infoStr name=\"applicationId\">{5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c01}</tm:infoStr>\
<tm:infoStr name=\"productName\">sppc_test edition</tm:infoStr>\
<tm:infoStr name=\"productDescription\">sppc_test, GALA channel</tm:infoStr>\
</tm:infoList></tm:infoTables></r:otherInfo></r:license>\
<r:license xmlns:r=\"urn:mpeg:mpeg21:2003:01-REL-R-NS\" licenseId=\"{5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c04}\" \
xmlns:tm=\"http://www.microsoft.com/DRM/XrML2/TM/v2\"><r:title>sppc_test (Private)</r:title>\
<r:otherInfo><tm:infoTables><tm:infoList tag=\"#global\">\
<tm:infoStr name=\"licenseType\">msft:sl/EUL/GENERIC/PRIVATE</tm:infoStr>\
<tm:infoStr name=\"productName\">sppc_test private edition</tm:infoStr>\
<tm:infoStr name=\"privateOnly\">private</tm:infoStr>\
<tm:infoStr name=\"productSkuId\">{5AA0C7E1-2F4E-4D3B-9A61-1F52607E3C02}</tm:infoStr>\
</tm:infoList></tm:infoTables></r:otherInfo></r:license>\
</rg:licenseGroup>";

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
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

/// `sppc_test: info <what> <hr>`.
fn info(what: &[u8], hr: i32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what)
        .s(b" ")
        .hex(hr as u32 as u64)
        .s(b"\r\n")
        .print();
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

    /// An HRESULT that must be exactly `want`, printed as `info <what>`.
    #[track_caller]
    fn code(&mut self, what: &[u8], hr: i32, want: i32) -> Result<(), u32> {
        info(what, hr);
        self.ok(hr == want, hr as u32 as u64)
    }

    /// A failed HRESULT, printed as `info <what>`.
    #[track_caller]
    fn failed(&mut self, what: &[u8], hr: i32) -> Result<(), u32> {
        info(what, hr);
        self.ok(hr < 0, hr as u32 as u64)
    }
}

/// The string value a getter gave: SL_DATA_SZ, as long as `expect`
/// with its NUL, and those characters; then freed.
#[track_caller]
fn string_is(c: &mut Checks, expect: &str, ty: u32, size: u32, value: *mut u8) -> Result<(), u32> {
    c.ok(ty == SL_DATA_SZ, ty.into())?;
    c.ok(size as usize == (expect.len() + 1) * 2, size.into())?;
    for (i, b) in expect.bytes().enumerate() {
        // SAFETY: size bytes were given.
        let ch = unsafe { *value.cast::<u16>().add(i) };
        c.ok(ch == u16::from(b), ch.into())?;
    }
    // SAFETY: allocated by the function with LocalAlloc.
    unsafe { LocalFree(value) };
    Ok(())
}

/// A NUL-terminated UTF-16 copy of `s` (at most 63 characters).
fn wide(s: &str) -> [u16; 64] {
    let mut w = [0u16; 64];
    for (i, b) in s.bytes().take(63).enumerate() {
        w[i] = b.into();
    }
    w
}

/// SLGetSLIDList: the HRESULT and the identifiers (freed).
fn ids(h: Handle, query: u32, id: *const Guid, ret: u32, out: &mut [Guid; 4]) -> (i32, u32) {
    let (mut n, mut list) = (0u32, null_mut::<Guid>());
    // SAFETY: our handle and outputs; the list is LocalAlloc'd.
    let hr = unsafe { SLGetSLIDList(h, query, id, ret, &mut n, &mut list) };
    if hr >= 0 && !list.is_null() {
        for (i, slot) in out.iter_mut().enumerate().take(n as usize) {
            // SAFETY: the function gave n identifiers.
            *slot = unsafe { *list.add(i) };
        }
        // SAFETY: allocated by the function with LocalAlloc.
        unsafe { LocalFree(list.cast()) };
    }
    (hr, n)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut h: Handle = 0;
    // SAFETY: our output.
    let hr = unsafe { SLOpen(&mut h) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(h != 0, 0)?;
    // SAFETY: a NULL output.
    let hr = unsafe { SLOpen(null_mut()) };
    c.failed(b"SLOpen with a NULL output", hr)?;

    // Nothing installed under NOBODY.
    let mut out = [NOBODY; 4];
    let (hr, _) = ids(h, SL_ID_APPLICATION, &NOBODY, SL_ID_PRODUCT_SKU, &mut out);
    c.failed(b"SKUs of an application not installed", hr)?;
    let (hr, _) = ids(h, SL_ID_PRODUCT_SKU, &NOBODY, SL_ID_LICENSE_FILE, &mut out);
    c.failed(b"license files of a SKU not installed", hr)?;
    // SAFETY: our identifier.
    let hr = unsafe { SLUninstallLicense(h, &NOBODY) };
    c.failed(b"uninstall a license file not installed", hr)?;
    let (mut n, mut list) = (0u32, null_mut::<Guid>());
    // SAFETY: our handle, identifier and outputs.
    let hr = unsafe { SLGetInstalledProductKeyIds(h, &NOBODY, &mut n, &mut list) };
    c.failed(b"product keys of a SKU not installed", hr)?;
    let (mut ty, mut size, mut value) = (0u32, 0u32, null_mut::<u8>());
    let name = wide("DigitalPID");
    // SAFETY: our handle, identifier, name and outputs.
    let hr =
        unsafe { SLGetPKeyInformation(h, &NOBODY, name.as_ptr(), &mut ty, &mut size, &mut value) };
    c.failed(b"information of a product key not installed", hr)?;
    let name = wide("productSkuId");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetLicenseInformation(h, &NOBODY, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.failed(b"information of a license not installed", hr)?;
    let name = wide("Name");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &NOBODY, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"information of a SKU not installed", hr, SL_E_VALUE_NOT_FOUND)?;
    // SAFETY: as above, with the optional data type left out.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &NOBODY, name.as_ptr(), null_mut(), &mut size, &mut value)
    };
    c.code(b"a SKU's information without the data type", hr, SL_E_VALUE_NOT_FOUND)?;
    // The arguments are looked at before the SKU.
    // SAFETY: our handle, identifier and outputs; a NULL name.
    let hr =
        unsafe { SLGetProductSkuInformation(h, &NOBODY, null(), &mut ty, &mut size, &mut value) };
    c.code(b"a SKU's information with a NULL value name", hr, E_INVALIDARG)?;
    // SAFETY: a NULL handle; our identifier, name and outputs.
    let hr = unsafe {
        SLGetProductSkuInformation(0, &NOBODY, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"a SKU's information on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: our handle, name and outputs; a NULL identifier.
    let hr = unsafe {
        SLGetProductSkuInformation(h, null(), name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"the information of a NULL SKU", hr, E_INVALIDARG)?;
    // The policies of a right nobody consumed: not a value that is
    // missing, a right that was never granted.
    let mut v = 0u32;
    let name = wide("sppc_test-no-such-policy");
    // SAFETY: our handle, name and output.
    let hr = unsafe { SLGetPolicyInformationDWORD(h, name.as_ptr(), &mut v) };
    c.code(b"a policy no right grants", hr, SL_E_RIGHT_NOT_GRANTED)?;
    // SAFETY: a NULL handle; our name and output.
    let hr = unsafe { SLGetPolicyInformationDWORD(0, name.as_ptr(), &mut v) };
    c.code(b"SLGetPolicyInformationDWORD on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: our handle and name; a NULL output.
    let hr = unsafe { SLGetPolicyInformationDWORD(h, name.as_ptr(), null_mut()) };
    c.code(b"SLGetPolicyInformationDWORD with a NULL output", hr, E_INVALIDARG)?;
    // SAFETY: our handle and output; a NULL name.
    let hr = unsafe { SLGetPolicyInformationDWORD(h, null(), &mut v) };
    c.code(b"SLGetPolicyInformationDWORD with a NULL value name", hr, E_INVALIDARG)?;
    // SAFETY: our handle, name and outputs.
    let hr = unsafe { SLGetPolicyInformation(h, name.as_ptr(), &mut ty, &mut size, &mut value) };
    c.code(b"a policy of any type no right grants", hr, SL_E_RIGHT_NOT_GRANTED)?;
    // SAFETY: our handle, name and outputs; the data type is optional.
    let hr = unsafe { SLGetPolicyInformation(h, name.as_ptr(), null_mut(), &mut size, &mut value) };
    c.code(b"a policy without the optional data type", hr, SL_E_RIGHT_NOT_GRANTED)?;
    // The name is looked at before the right, as in the DWORD sibling.
    // SAFETY: our handle and outputs; a NULL name.
    let hr = unsafe { SLGetPolicyInformation(h, null(), &mut ty, &mut size, &mut value) };
    c.code(b"a policy with a NULL value name", hr, E_INVALIDARG)?;
    // SAFETY: a NULL handle; our name and outputs.
    let hr = unsafe { SLGetPolicyInformation(0, name.as_ptr(), &mut ty, &mut size, &mut value) };
    c.code(b"SLGetPolicyInformation on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: our handle and name; NULL required outputs.
    let hr = unsafe { SLGetPolicyInformation(h, name.as_ptr(), &mut ty, null_mut(), null_mut()) };
    c.code(b"SLGetPolicyInformation with NULL outputs", hr, E_INVALIDARG)?;
    // The application policies: nothing persists any, so none is loaded
    // and no context exists to query.
    let mut context: Handle = 0;
    // SAFETY: our identifier and output; the SKU is optional.
    let hr = unsafe { SLLoadApplicationPolicies(&APP, null(), 0, &mut context) };
    c.code(b"load the policies of an application", hr, SL_E_APPLICATION_POLICIES_MISSING)?;
    c.ok(context == 0, context as u64)?;
    // SAFETY: our output; a NULL application identifier.
    let hr = unsafe { SLLoadApplicationPolicies(null(), null(), 0, &mut context) };
    c.code(b"load the policies of no application", hr, E_INVALIDARG)?;
    // SAFETY: our identifier; a NULL output.
    let hr = unsafe { SLLoadApplicationPolicies(&APP, null(), 0, null_mut()) };
    c.code(b"load the policies into a NULL context", hr, E_INVALIDARG)?;
    // A context that was never handed out: the arguments are checked
    // first, then the context is looked up.
    // SAFETY: not a context; our name and outputs.
    let hr = unsafe { SLGetApplicationPolicy(1, name.as_ptr(), &mut ty, &mut size, &mut value) };
    c.code(b"an application policy of no context", hr, SL_E_APPLICATION_POLICIES_NOT_LOADED)?;
    // SAFETY: not a context; our name and outputs; the type is optional.
    let hr = unsafe { SLGetApplicationPolicy(1, name.as_ptr(), null_mut(), &mut size, &mut value) };
    c.code(
        b"an application policy without the data type",
        hr,
        SL_E_APPLICATION_POLICIES_NOT_LOADED,
    )?;
    // SAFETY: our name and outputs; a NULL context.
    let hr = unsafe { SLGetApplicationPolicy(0, name.as_ptr(), &mut ty, &mut size, &mut value) };
    c.code(b"an application policy of a NULL context", hr, E_INVALIDARG)?;
    // SAFETY: not a context; our outputs; a NULL name.
    let hr = unsafe { SLGetApplicationPolicy(1, null(), &mut ty, &mut size, &mut value) };
    c.code(b"an application policy with a NULL value name", hr, E_INVALIDARG)?;
    // SAFETY: not a context; our name; NULL required outputs.
    let hr = unsafe { SLGetApplicationPolicy(1, name.as_ptr(), &mut ty, null_mut(), null_mut()) };
    c.code(b"an application policy with NULL outputs", hr, E_INVALIDARG)?;
    // SAFETY: not a context.
    let hr = unsafe { SLUnloadApplicationPolicies(1, 0) };
    c.code(b"unload the policies of no context", hr, SL_E_APPLICATION_POLICIES_NOT_LOADED)?;
    // SAFETY: a NULL context.
    let hr = unsafe { SLUnloadApplicationPolicies(0, 0) };
    c.code(b"unload the policies of a NULL context", hr, E_INVALIDARG)?;
    // SAFETY: our handle and identifier; no right named.
    let hr = unsafe {
        SLGetLicensingStatusInformation(h, &NOBODY, null(), null(), &mut n, &mut null_mut())
    };
    c.code(b"status of an application not installed", hr, SL_E_PRODUCT_SKU_NOT_INSTALLED)?;
    // An application identifier is required; the SKU is not.
    // SAFETY: our handle and identifier; a NULL application.
    let hr = unsafe {
        SLGetLicensingStatusInformation(h, null(), &NOBODY, null(), &mut n, &mut null_mut())
    };
    c.code(b"status of a NULL application", hr, E_INVALIDARG)?;
    // The right of a SKU that is not installed, which SLConsumeRight
    // answers the same way, and which also requires an application.
    // SAFETY: our handle and identifiers; no right named.
    let hr = unsafe { SLConsumeRight(h, &NOBODY, &NOBODY, null(), 0) };
    c.code(b"a right of a SKU not installed", hr, SL_E_PRODUCT_SKU_NOT_INSTALLED)?;
    // SAFETY: our handle and identifier; a NULL application.
    let hr = unsafe { SLConsumeRight(h, null(), &NOBODY, null(), 0) };
    c.code(b"a right of a NULL application", hr, E_INVALIDARG)?;
    // SAFETY: our handle and identifier.
    let hr = unsafe { SLUninstallProofOfPurchase(h, &NOBODY) };
    c.failed(b"uninstall a product key not installed", hr)?;

    // What is not a license, or not a product key.
    let mut file = NOBODY;
    let junk = b"this is not a license";
    // SAFETY: our handle, blob and output.
    let hr = unsafe { SLInstallLicense(h, junk.len() as u32, junk.as_ptr(), &mut file) };
    c.failed(b"install a blob that is not a license", hr)?;
    let alg = wide("msft:rm/algorithm/pkey/2009");
    let key = wide("BBBBB-BBBBB-BBBBB-BBBBB-BBBBB");
    let mut pkey = NOBODY;
    // SAFETY: our handle, strings and output.
    let hr =
        unsafe { SLInstallProofOfPurchase(h, alg.as_ptr(), key.as_ptr(), 0, null(), &mut pkey) };
    c.failed(b"install a product key no license configures", hr)?;
    // SAFETY: our handle, strings and output.
    let hr = unsafe { SLGetPKeyId(h, alg.as_ptr(), key.as_ptr(), 0, null(), &mut pkey) };
    c.failed(b"the identifier of a product key not installed", hr)?;
    // SAFETY: our handle and strings; a NULL output.
    let hr = unsafe { SLGetPKeyId(h, alg.as_ptr(), key.as_ptr(), 0, null(), null_mut()) };
    c.failed(b"SLGetPKeyId with a NULL output", hr)?;
    // A NULL output.
    // SAFETY: our handle; a NULL output.
    let hr = unsafe { SLInstallLicense(h, LICENSE.len() as u32, LICENSE.as_ptr(), null_mut()) };
    c.failed(b"SLInstallLicense with a NULL output", hr)?;

    // Offline activation: the Installation ID a telephone or web
    // activation is done with, and the Confirmation ID it answers.
    // Nothing is installed under NOBODY, so neither call reaches a
    // product key, and the two differ over the code for it; no failure
    // touches the output.
    let mut marker = 0u16;
    let sentinel = &raw mut marker;
    let mut iid_out = sentinel;
    let iid = wide("123456");
    let cid = wide("654321");
    let empty = wide("");
    let default_info = ActivationInfo { size: 8, kind: 0 };
    // SAFETY: our handle, identifier and output.
    let hr = unsafe { SLGenerateOfflineInstallationId(h, &NOBODY, &mut iid_out) };
    c.code(b"an installation id for a SKU not installed", hr, SL_E_VALUE_NOT_FOUND)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    // SAFETY: our handle and output; a NULL identifier.
    let hr = unsafe { SLGenerateOfflineInstallationId(h, null(), &mut iid_out) };
    c.code(b"an installation id for a NULL SKU", hr, E_INVALIDARG)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    // SAFETY: our handle and identifier; a NULL output.
    let hr = unsafe { SLGenerateOfflineInstallationId(h, &NOBODY, null_mut()) };
    c.code(b"an installation id into a NULL output", hr, E_INVALIDARG)?;
    // SAFETY: our identifier and output; a NULL handle.
    let hr = unsafe { SLGenerateOfflineInstallationId(0, &NOBODY, &mut iid_out) };
    c.code(b"an installation id on a NULL handle", hr, E_INVALIDARG)?;
    // The Ex form takes no activation info outside an Active Directory.
    // SAFETY: our handle, identifier and output; no activation info.
    let hr = unsafe { SLGenerateOfflineInstallationIdEx(h, &NOBODY, null(), &mut iid_out) };
    c.code(b"an installation id without activation info", hr, SL_E_VALUE_NOT_FOUND)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    // SAFETY: our handle, identifier, activation info and output.
    let hr = unsafe { SLGenerateOfflineInstallationIdEx(h, &NOBODY, &default_info, &mut iid_out) };
    c.code(b"an installation id with activation info", hr, E_INVALIDARG)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    // The deposit answers the other code for the same SKU, before it
    // reads either string.
    // SAFETY: our handle, identifier and strings.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, &NOBODY, iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id for a SKU not installed", hr, SL_E_PRODUCT_SKU_NOT_INSTALLED)?;
    // SAFETY: our handle and identifier; two empty strings.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, &NOBODY, empty.as_ptr(), empty.as_ptr()) };
    c.code(b"a confirmation id of empty strings", hr, SL_E_PRODUCT_SKU_NOT_INSTALLED)?;
    // SAFETY: our handle and strings; a NULL identifier.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, null(), iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id for a NULL SKU", hr, E_INVALIDARG)?;
    // SAFETY: our handle, identifier and string; a NULL installation id.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, &NOBODY, null(), cid.as_ptr()) };
    c.code(b"a confirmation id of a NULL installation id", hr, E_INVALIDARG)?;
    // SAFETY: our handle, identifier and string; a NULL confirmation id.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, &NOBODY, iid.as_ptr(), null()) };
    c.code(b"a NULL confirmation id", hr, E_INVALIDARG)?;
    // SAFETY: our identifier and strings; a NULL handle.
    let hr = unsafe { SLDepositOfflineConfirmationId(0, &NOBODY, iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: our handle, identifier and strings; no activation info.
    let hr =
        unsafe { SLDepositOfflineConfirmationIdEx(h, &NOBODY, null(), iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id without activation info", hr, SL_E_PRODUCT_SKU_NOT_INSTALLED)?;
    // SAFETY: our handle, identifier, activation info and strings.
    let hr = unsafe {
        SLDepositOfflineConfirmationIdEx(h, &NOBODY, &default_info, iid.as_ptr(), cid.as_ptr())
    };
    c.code(b"a confirmation id with activation info", hr, E_INVALIDARG)?;

    // A handle no SLOpen gave: every call that takes one answers
    // RPC_NT_SS_CONTEXT_MISMATCH once its arguments are good, the
    // handles of Windows' own sppc.dll being RPC context handles. One
    // that is not a multiple of eight is never read.
    const STALE: Handle = 1;
    let vname = wide("Name");
    // SAFETY: not a handle.
    let hr = unsafe { SLClose(STALE) };
    c.code(b"close what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our blob and output.
    let hr = unsafe { SLInstallLicense(STALE, LICENSE.len() as u32, LICENSE.as_ptr(), &mut file) };
    c.code(b"install a license on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier.
    let hr = unsafe { SLUninstallLicense(STALE, &NOBODY) };
    c.code(b"uninstall a license on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    let (hr, _) = ids(STALE, SL_ID_APPLICATION, &NOBODY, SL_ID_PRODUCT_SKU, &mut out);
    c.code(b"a SLID list on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier, name and outputs.
    let hr = unsafe {
        SLGetLicenseInformation(STALE, &NOBODY, vname.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"a license's information on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: as above.
    let hr = unsafe {
        SLGetProductSkuInformation(STALE, &NOBODY, vname.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"a SKU's information on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier and outputs.
    let hr = unsafe {
        SLGetLicensingStatusInformation(STALE, &NOBODY, null(), null(), &mut n, &mut null_mut())
    };
    c.code(b"a status on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifiers.
    let hr = unsafe { SLConsumeRight(STALE, &NOBODY, &NOBODY, null(), 0) };
    c.code(b"a right on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our name and outputs.
    let hr =
        unsafe { SLGetPolicyInformation(STALE, vname.as_ptr(), &mut ty, &mut size, &mut value) };
    c.code(b"a policy on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our name and output.
    let hr = unsafe { SLGetPolicyInformationDWORD(STALE, vname.as_ptr(), &mut v) };
    c.code(b"a DWORD policy on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our strings and output.
    let hr = unsafe {
        SLInstallProofOfPurchase(STALE, alg.as_ptr(), key.as_ptr(), 0, null(), &mut pkey)
    };
    c.code(b"install a product key on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our strings and output.
    let hr = unsafe { SLGetPKeyId(STALE, alg.as_ptr(), key.as_ptr(), 0, null(), &mut pkey) };
    c.code(b"a product key's identifier on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier.
    let hr = unsafe { SLUninstallProofOfPurchase(STALE, &NOBODY) };
    c.code(b"uninstall a product key on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier and outputs.
    let hr = unsafe { SLGetInstalledProductKeyIds(STALE, &NOBODY, &mut n, &mut list) };
    c.code(b"the product keys on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier, name and outputs.
    let hr = unsafe {
        SLGetPKeyInformation(STALE, &NOBODY, vname.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"a product key's information on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: not a handle; our identifier and output.
    let hr = unsafe { SLGenerateOfflineInstallationId(STALE, &NOBODY, &mut iid_out) };
    c.code(b"an installation id on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    // SAFETY: not a handle; our identifier and strings.
    let hr = unsafe { SLDepositOfflineConfirmationId(STALE, &NOBODY, iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;

    // The plugin registration Office's licensing asks for, which no
    // public header declares (sppc's SLRegisterPlugin records what was
    // measured of it). Nothing can be registered without elevation,
    // which the caller has not, and both pointers are checked before
    // the handle.
    let mut slot = 0usize;
    let out = (&raw mut slot).cast::<u8>();
    let name = wide("sppc_test-plugin");
    let plugin = name.as_ptr().cast::<u8>();
    // SAFETY: our handle, name and output.
    let hr = unsafe { SLRegisterPlugin(h, plugin, out) };
    c.code(b"register a plugin", hr, SL_E_LUA_ACCESSDENIED)?;
    // SAFETY: our handle and name; a NULL output.
    let hr = unsafe { SLRegisterPlugin(h, plugin, null_mut()) };
    c.code(b"register a plugin with a NULL output", hr, E_INVALIDARG)?;
    // SAFETY: our handle and output; a NULL name.
    let hr = unsafe { SLRegisterPlugin(h, null(), out) };
    c.code(b"register a NULL plugin", hr, E_INVALIDARG)?;
    // SAFETY: our name and output; a NULL handle.
    let hr = unsafe { SLRegisterPlugin(0, plugin, out) };
    c.code(b"register a plugin on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: not a handle; our output; a NULL name.
    let hr = unsafe { SLRegisterPlugin(1, null(), out) };
    c.code(b"a NULL plugin on what is not a handle", hr, E_INVALIDARG)?;
    // SAFETY: not a handle; our name and output.
    let hr = unsafe { SLRegisterPlugin(1, plugin, out) };
    c.code(b"register a plugin on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // SAFETY: our handle and name.
    let hr = unsafe { SLUnregisterPlugin(h, plugin) };
    c.code(b"unregister a plugin", hr, SL_E_LUA_ACCESSDENIED)?;
    // SAFETY: our handle; a NULL name.
    let hr = unsafe { SLUnregisterPlugin(h, null()) };
    c.code(b"unregister a NULL plugin", hr, E_INVALIDARG)?;
    // SAFETY: our name; a NULL handle.
    let hr = unsafe { SLUnregisterPlugin(0, plugin) };
    c.code(b"unregister a plugin on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: not a handle; our name.
    let hr = unsafe { SLUnregisterPlugin(1, plugin) };
    c.code(b"unregister a plugin on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;

    // The authentication data Office's licensing deposits, which no
    // public header declares either (sppc's SLSetAuthenticationData
    // records what was measured of it). The order the three arguments
    // are looked at in is the whole of what gala promises: a count
    // above 1024 is refused before the buffer and before the handle,
    // and a count with no buffer before the handle.
    let blob = [0x5au8; 64];
    let data = blob.as_ptr();
    // SAFETY: our handle; no data, which is not read.
    let hr = unsafe { SLSetAuthenticationData(h, 0, null()) };
    c.code(b"authentication data of no length", hr, S_OK)?;
    // SAFETY: our handle and buffer; a length of zero, so it is not read.
    let hr = unsafe { SLSetAuthenticationData(h, 0, data) };
    c.code(b"a buffer no length reads", hr, S_OK)?;
    // SAFETY: our handle; a length with no buffer.
    let hr = unsafe { SLSetAuthenticationData(h, 64, null()) };
    c.code(b"authentication data with no buffer", hr, E_INVALIDARG)?;
    // SAFETY: not a handle; a length with no buffer, which is looked at
    // first, so this is not the context mismatch the handle would earn.
    let hr = unsafe { SLSetAuthenticationData(STALE, 64, null()) };
    c.code(b"no buffer on what is not a handle", hr, E_INVALIDARG)?;
    // SAFETY: our handle and buffer; a length past what sppc reads.
    let hr = unsafe { SLSetAuthenticationData(h, 1025, data) };
    c.code(b"more authentication data than sppc reads", hr, E_BUFFER_OVERFLOW)?;
    // SAFETY: not a handle; our buffer; a length past what sppc reads,
    // which is looked at before the handle.
    let hr = unsafe { SLSetAuthenticationData(STALE, 1025, data) };
    c.code(b"too much data on what is not a handle", hr, E_BUFFER_OVERFLOW)?;
    // SAFETY: our buffer; a NULL handle, which is looked at first of all.
    let hr = unsafe { SLSetAuthenticationData(0, 1025, data) };
    c.code(b"authentication data on a NULL handle", hr, E_INVALIDARG)?;
    // SAFETY: not a handle; our buffer and a length it reads.
    let hr = unsafe { SLSetAuthenticationData(STALE, 64, data) };
    c.code(b"authentication data on what is not a handle", hr, RPC_NT_SS_CONTEXT_MISMATCH)?;
    // The one case Windows and gala answer differently: Windows reads
    // the blob and its service refuses filler, gala has no service and
    // keeps nothing, so only the success or failure is checked and not
    // the code. 1024 is read and 1025 is not, so this is the last
    // length that gets this far.
    // SAFETY: our handle and buffer, whose length sppc reads.
    let hr = unsafe { SLSetAuthenticationData(h, 1024, data) };
    c.ok(hr == S_OK || hr == E_INVALID_DATA, hr as u32 as u64)?;

    round_trip(&mut c, h)?;

    // SAFETY: our handle.
    let hr = unsafe { SLClose(h) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    Ok(())
}

/// The license made here through the store, where it is taken.
fn round_trip(c: &mut Checks, h: Handle) -> Result<(), u32> {
    let mut file = NOBODY;
    // SAFETY: our handle, blob and output.
    let hr = unsafe { SLInstallLicense(h, LICENSE.len() as u32, LICENSE.as_ptr(), &mut file) };
    if hr < 0 {
        info(b"round trip skipped: the unsigned license refused", hr);
        return Ok(());
    }
    c.ok(file != NOBODY, 0)?;
    // Installed again: the same license file.
    let mut again = NOBODY;
    // SAFETY: as above.
    let hr = unsafe { SLInstallLicense(h, LICENSE.len() as u32, LICENSE.as_ptr(), &mut again) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(again == file, u64::from(again.data1))?;

    let mut out = [NOBODY; 4];
    let (hr, n) = ids(h, SL_ID_APPLICATION, &APP, SL_ID_PRODUCT_SKU, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 1 && out[0] == SKU, n.into())?;
    let (hr, n) = ids(h, SL_ID_PRODUCT_SKU, &SKU, SL_ID_APPLICATION, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 1 && out[0] == APP, n.into())?;
    let (hr, n) = ids(h, SL_ID_PRODUCT_SKU, &SKU, SL_ID_LICENSE_FILE, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 1 && out[0] == file, n.into())?;
    let (hr, n) = ids(h, SL_ID_PRODUCT_SKU, &SKU, SL_ID_LICENSE, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 2 && out[..2].contains(&PUBLIC) && out[..2].contains(&PRIVATE), n.into())?;
    let (hr, n) = ids(h, SL_ID_LICENSE_FILE, &file, SL_ID_LICENSE, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 2, n.into())?;
    let (hr, n) = ids(h, SL_ID_LICENSE, &PRIVATE, SL_ID_LICENSE_FILE, &mut out);
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 1 && out[0] == file, n.into())?;

    // The license's metadata, a string with its NUL.
    let (mut ty, mut size, mut value) = (0u32, 0u32, null_mut::<u8>());
    let name = wide("productName");
    // SAFETY: our handle, identifier, name and outputs.
    let hr = unsafe {
        SLGetLicenseInformation(h, &PUBLIC, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    string_is(c, "sppc_test edition", ty, size, value)?;
    let name = wide("licenseId");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetLicenseInformation(h, &PUBLIC, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    string_is(c, "5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c03", ty, size, value)?;
    let name = wide("sppc_test-no-such-value");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetLicenseInformation(h, &PUBLIC, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.failed(b"a value the license has not", hr)?;

    // The SKU's information is its generic public end-user license's:
    // `Name` and `Description` are that license's productName and
    // productDescription, its identifier is the public license's, and
    // what only the private half has is not the SKU's.
    let name = wide("Name");
    // SAFETY: our handle, identifier, name and outputs.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &SKU, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    string_is(c, "sppc_test edition", ty, size, value)?;
    let name = wide("Description");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &SKU, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    string_is(c, "sppc_test, GALA channel", ty, size, value)?;
    let name = wide("licenseId");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &SKU, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    string_is(c, "5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c03", ty, size, value)?;
    let name = wide("privateOnly");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetProductSkuInformation(h, &SKU, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.code(b"a SKU's value only its private license has", hr, SL_E_VALUE_NOT_FOUND)?;

    // The SKU's status: licensed by no product key.
    let (mut n, mut status) = (0u32, null_mut::<LicensingStatus>());
    // SAFETY: our handle, identifiers and outputs.
    let hr = unsafe { SLGetLicensingStatusInformation(h, &APP, &SKU, null(), &mut n, &mut status) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(n == 1 && !status.is_null(), n.into())?;
    // SAFETY: one status was given.
    let (sku, state, reason) = unsafe { ((*status).sku, (*status).status, (*status).reason) };
    c.ok(sku == SKU, sku.data1.into())?;
    c.ok(state == SL_LICENSING_STATUS_UNLICENSED, state.into())?;
    info(b"reason of an unlicensed SKU", reason);
    // SAFETY: allocated by the function with LocalAlloc.
    unsafe { LocalFree(status.cast()) };

    // The SKU is installed, so offline activation gets past the store
    // and stops at the product key store, which is empty: the reason
    // the status gives, and what SLGetPKeyId and
    // SLGetInstalledProductKeyIds answer. This is gala's answer, not a
    // measured one -- asking Windows about an installed SKU would make
    // an Installation ID out of this machine's product key -- and it
    // runs only where the license was taken, as the rest of this round
    // trip does.
    let mut marker = 0u16;
    let sentinel = &raw mut marker;
    let mut iid_out = sentinel;
    // SAFETY: our handle, identifier and output.
    let hr = unsafe { SLGenerateOfflineInstallationId(h, &SKU, &mut iid_out) };
    c.code(b"an installation id for an installed SKU", hr, SL_E_PKEY_NOT_INSTALLED)?;
    c.ok(iid_out == sentinel, iid_out as usize as u64)?;
    let iid = wide("123456");
    let cid = wide("654321");
    // SAFETY: our handle, identifier and strings.
    let hr = unsafe { SLDepositOfflineConfirmationId(h, &SKU, iid.as_ptr(), cid.as_ptr()) };
    c.code(b"a confirmation id for an installed SKU", hr, SL_E_PKEY_NOT_INSTALLED)?;
    // The right its license grants nobody, no product key being
    // installed.
    // SAFETY: our handle and identifiers; no right named.
    let hr = unsafe { SLConsumeRight(h, &APP, &SKU, null(), 0) };
    c.code(b"a right of an installed SKU", hr, SL_E_RIGHT_NOT_GRANTED)?;

    // Another handle sees the store.
    let mut other: Handle = 0;
    // SAFETY: our output.
    let hr = unsafe { SLOpen(&mut other) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    let (hr, n) = ids(other, SL_ID_APPLICATION, &APP, SL_ID_PRODUCT_SKU, &mut out);
    c.ok(hr == S_OK && n == 1, hr as u32 as u64)?;
    // SAFETY: our handle.
    unsafe { SLClose(other) };

    // Uninstalled: gone.
    // SAFETY: our handle and identifier.
    let hr = unsafe { SLUninstallLicense(h, &file) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    let (hr, _) = ids(h, SL_ID_APPLICATION, &APP, SL_ID_PRODUCT_SKU, &mut out);
    c.failed(b"SKUs of an application uninstalled", hr)?;
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

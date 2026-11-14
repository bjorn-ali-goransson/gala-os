//! sppc.dll, the Software Protection Platform's client (slpublic.h), as
//! Office's Click-to-Run integrator asks it while it installs Office's
//! licenses (ROADMAP OF). Expected values are those Microsoft's
//! documentation of the functions states and Windows answers:
//!
//! - SLOpen gives S_OK and a handle, SLClose S_OK (measured on Windows,
//!   ROADMAP issue #60);
//! - asking about an application, SKU, license file or product key that
//!   is not installed fails, as does installing a blob that is not a
//!   license, uninstalling a license file that is not installed and
//!   installing a product key that is not one;
//! - a NULL output pointer fails.
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
    fn SLGetLicensingStatusInformation(
        h: Handle,
        app: *const Guid,
        sku: *const Guid,
        right: *const u16,
        count: *mut u32,
        status: *mut *mut LicensingStatus,
    ) -> i32;
    fn SLGetPolicyInformationDWORD(h: Handle, name: *const u16, value: *mut u32) -> i32;
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
    fn SLGetPKeyInformation(
        h: Handle,
        pkey: *const Guid,
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
}

const S_OK: i32 = 0;
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
</tm:infoList></tm:infoTables></r:otherInfo></r:license>\
<r:license xmlns:r=\"urn:mpeg:mpeg21:2003:01-REL-R-NS\" licenseId=\"{5aa0c7e1-2f4e-4d3b-9a61-1f52607e3c04}\" \
xmlns:tm=\"http://www.microsoft.com/DRM/XrML2/TM/v2\"><r:title>sppc_test (Private)</r:title>\
<r:otherInfo><tm:infoTables><tm:infoList tag=\"#global\">\
<tm:infoStr name=\"licenseType\">msft:sl/EUL/GENERIC/PRIVATE</tm:infoStr>\
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

    /// A failed HRESULT, printed as `info <what>`.
    #[track_caller]
    fn failed(&mut self, what: &[u8], hr: i32) -> Result<(), u32> {
        info(what, hr);
        self.ok(hr < 0, hr as u32 as u64)
    }
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
    let mut v = 0u32;
    let name = wide("sppc_test-no-such-policy");
    // SAFETY: our handle, name and output.
    let hr = unsafe { SLGetPolicyInformationDWORD(h, name.as_ptr(), &mut v) };
    c.failed(b"a policy no license has", hr)?;
    // SAFETY: our handle and identifier; no right named.
    let hr = unsafe {
        SLGetLicensingStatusInformation(h, &NOBODY, null(), null(), &mut n, &mut null_mut())
    };
    c.failed(b"status of an application not installed", hr)?;

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
    // A NULL output.
    // SAFETY: our handle; a NULL output.
    let hr = unsafe { SLInstallLicense(h, LICENSE.len() as u32, LICENSE.as_ptr(), null_mut()) };
    c.failed(b"SLInstallLicense with a NULL output", hr)?;

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
    c.ok(ty == SL_DATA_SZ, ty.into())?;
    let expect = "sppc_test edition";
    c.ok(size as usize == (expect.len() + 1) * 2, size.into())?;
    for (i, b) in expect.bytes().enumerate() {
        // SAFETY: size bytes were given.
        let ch = unsafe { *value.cast::<u16>().add(i) };
        c.ok(ch == u16::from(b), ch.into())?;
    }
    // SAFETY: allocated by the function with LocalAlloc.
    unsafe { LocalFree(value) };
    let name = wide("sppc_test-no-such-value");
    // SAFETY: as above.
    let hr = unsafe {
        SLGetLicenseInformation(h, &PUBLIC, name.as_ptr(), &mut ty, &mut size, &mut value)
    };
    c.failed(b"a value the license has not", hr)?;

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

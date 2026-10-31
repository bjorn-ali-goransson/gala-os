//! kernel32's locale and time functions, as the C runtime and the JVM use
//! them, with the results real Windows gives for the en-US locale in the
//! UTC time zone (documented contracts and the en-US locale data):
//!
//! - GetUserDefaultLCID/GetUserDefaultUILanguage/GetSystemDefaultLCID:
//!   0x409; IsValidLocale; EnumSystemLocalesW lists "00000409".
//! - GetLocaleInfoW/A: decimal and thousands separators, ISO language and
//!   country names, the OEM code page; counts include the NUL; a size
//!   query; ERROR_INSUFFICIENT_BUFFER; LOCALE_RETURN_NUMBER.
//! - LCMapStringW (upper and lower case), CompareStringW (linguistic
//!   order: "a" < "B", lower case first, NORM_IGNORECASE),
//!   GetStringTypeW (CT_CTYPE1), IsDBCSLeadByte.
//! - GetDateFormatW/GetTimeFormatW with pictures and the short date.
//! - GetLocalTime matches GetSystemTime (UTC); GetDynamicTimeZoneInformation
//!   (UTC, TIME_ZONE_ID_UNKNOWN); SystemTimeToTzSpecificLocalTime and back.
//! - GetUserGeoID (244, United States), GetGeoInfoA (GEO_ISO2 "US").
//! - Get{User,System}PreferredUILanguages: en-US, by name or id.
//!
//! Prints `kernel32_locale_test: ok` or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type LocaleEnumProc = extern "system" fn(*const u16) -> Bool;
type LocaleEnumProcEx = extern "system" fn(*const u16, u32, isize) -> Bool;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserPreferredUILanguages(
        flags: u32,
        count: *mut u32,
        buf: *mut u16,
        len: *mut u32,
    ) -> Bool;
    fn GetSystemPreferredUILanguages(
        flags: u32,
        count: *mut u32,
        buf: *mut u16,
        len: *mut u32,
    ) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetUserDefaultLCID() -> u32;
    fn GetSystemDefaultLCID() -> u32;
    fn GetUserDefaultUILanguage() -> u16;
    fn IsValidLocale(lcid: u32, flags: u32) -> Bool;
    fn EnumSystemLocalesW(f: LocaleEnumProc, flags: u32) -> Bool;
    fn GetLocaleInfoW(lcid: u32, kind: u32, buf: *mut u16, n: i32) -> i32;
    fn GetLocaleInfoA(lcid: u32, kind: u32, buf: *mut u8, n: i32) -> i32;
    fn LCMapStringW(lcid: u32, flags: u32, src: *const u16, n: i32, dst: *mut u16, m: i32) -> i32;
    fn CompareStringW(lcid: u32, flags: u32, a: *const u16, n: i32, b: *const u16, m: i32) -> i32;
    fn GetStringTypeW(kind: u32, s: *const u16, n: i32, out: *mut u16) -> Bool;
    fn IsDBCSLeadByte(b: u8) -> Bool;
    fn GetDateFormatW(
        lcid: u32,
        flags: u32,
        st: *const SystemTime,
        format: *const u16,
        buf: *mut u16,
        n: i32,
    ) -> i32;
    fn GetTimeFormatW(
        lcid: u32,
        flags: u32,
        st: *const SystemTime,
        format: *const u16,
        buf: *mut u16,
        n: i32,
    ) -> i32;
    fn GetSystemTime(st: *mut SystemTime);
    fn GetLocalTime(st: *mut SystemTime);
    fn GetDynamicTimeZoneInformation(info: *mut u8) -> u32;
    fn SystemTimeToTzSpecificLocalTime(
        tz: *const u8,
        utc: *const SystemTime,
        local: *mut SystemTime,
    ) -> Bool;
    fn TzSpecificLocalTimeToSystemTime(
        tz: *const u8,
        local: *const SystemTime,
        utc: *mut SystemTime,
    ) -> Bool;
    fn GetUserGeoID(class: u32) -> i32;
    fn LCIDToLocaleName(lcid: u32, buf: *mut u16, n: i32, flags: u32) -> i32;
    fn LocaleNameToLCID(name: *const u16, flags: u32) -> u32;
    fn IsValidLocaleName(name: *const u16) -> Bool;
    fn EnumSystemLocalesEx(
        f: LocaleEnumProcEx,
        flags: u32,
        param: isize,
        reserved: *mut c_void,
    ) -> Bool;
    fn GetCurrentPackageId(len: *mut u32, buf: *mut u8) -> i32;
    fn GetGeoInfoA(geo: i32, kind: u32, buf: *mut u8, n: i32, lang: u16) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// SYSTEMTIME: year, month, day of week, day, hour, minute, second, ms.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct SystemTime([u16; 8]);

const LOCALE_SDECIMAL: u32 = 0x0e;
const LOCALE_STHOUSAND: u32 = 0x0f;
const LOCALE_IDEFAULTCODEPAGE: u32 = 0x0b;
const LOCALE_SISO639LANGNAME: u32 = 0x59;
const LOCALE_SISO3166CTRYNAME: u32 = 0x5a;
const LOCALE_RETURN_NUMBER: u32 = 0x2000_0000;
const LOCALE_USER_DEFAULT: u32 = 0x400;
const LCID_SUPPORTED: u32 = 2;
const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;
const NORM_IGNORECASE: u32 = 1;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;
const CT_CTYPE1: u32 = 1;
const DATE_SHORTDATE: u32 = 1;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const GEOCLASS_NATION: u32 = 16;
const GEO_ISO2: u32 = 4;

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
        out(b"kernel32_locale_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// `got` holds `want` followed by a NUL.
    fn text(&mut self, got: &[u16], want: &str) -> Result<(), u32> {
        let n = want.len();
        let same = got.len() > n
            && got[..n].iter().zip(want.bytes()).all(|(&a, b)| a == u16::from(b))
            && got[n] == 0;
        self.ok(same, u64::from(got.first().copied().unwrap_or(0)))
    }
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 32] {
    let mut b = [0u16; 32];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

static SAW_409: AtomicU32 = AtomicU32::new(0);
static SAW_EN_US: AtomicU32 = AtomicU32::new(0);

extern "system" fn on_locale_ex(name: *const u16, _flags: u32, param: isize) -> Bool {
    let want = w("en-US");
    // SAFETY: a NUL-terminated locale name.
    let same = (0..6).all(|i| unsafe { *name.add(i) } == want[i]);
    if same && param == 77 {
        SAW_EN_US.fetch_add(1, Ordering::SeqCst);
    }
    1
}

extern "system" fn on_locale(name: *const u16) -> Bool {
    let want = w("00000409");
    // SAFETY: a NUL-terminated locale id string.
    let same = (0..9).all(|i| unsafe { *name.add(i) } == want[i]);
    if same {
        SAW_409.fetch_add(1, Ordering::SeqCst);
    }
    1
}

fn locales(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with buffers of ours.
    unsafe {
        c.eq(GetUserDefaultLCID().into(), 0x409)?;
        c.eq(GetSystemDefaultLCID().into(), 0x409)?;
        c.eq(GetUserDefaultUILanguage().into(), 0x409)?;
        c.ok(IsValidLocale(0x409, LCID_SUPPORTED) != 0, 0)?;
        c.eq(IsValidLocale(0x0012_3456, LCID_SUPPORTED) as u64, 0)?;
        c.ok(EnumSystemLocalesW(on_locale, LCID_SUPPORTED) != 0, GetLastError().into())?;
        c.eq(SAW_409.load(Ordering::SeqCst).into(), 1)?;
        // Locale names (the Vista API the C runtime prefers).
        let mut name = [0u16; 16];
        c.eq(LCIDToLocaleName(0x409, name.as_mut_ptr(), 16, 0) as u64, 6)?;
        c.ok(name[..6] == w("en-US")[..6], name[0].into())?;
        c.eq(LocaleNameToLCID(w("en-US").as_ptr(), 0).into(), 0x409)?;
        c.ok(IsValidLocaleName(w("en-US").as_ptr()) != 0, 0)?;
        c.eq(IsValidLocaleName(w("xx-QQ").as_ptr()) as u64, 0)?;
        c.ok(
            EnumSystemLocalesEx(on_locale_ex, 0, 77, core::ptr::null_mut()) != 0,
            GetLastError().into(),
        )?;
        c.eq(SAW_EN_US.load(Ordering::SeqCst).into(), 1)?;
        // Not a packaged (Store) app: APPMODEL_ERROR_NO_PACKAGE.
        let mut len = 0u32;
        c.eq(GetCurrentPackageId(&mut len, core::ptr::null_mut()) as u64, 15700)?;

        let mut buf = [0u16; 16];
        c.eq(GetLocaleInfoW(0x409, LOCALE_SDECIMAL, buf.as_mut_ptr(), 16) as u64, 2)?;
        c.text(&buf, ".")?;
        c.eq(GetLocaleInfoW(0x409, LOCALE_STHOUSAND, buf.as_mut_ptr(), 16) as u64, 2)?;
        c.text(&buf, ",")?;
        c.eq(
            GetLocaleInfoW(LOCALE_USER_DEFAULT, LOCALE_SISO639LANGNAME, buf.as_mut_ptr(), 16)
                as u64,
            3,
        )?;
        c.text(&buf, "en")?;
        c.eq(GetLocaleInfoW(0x409, LOCALE_SISO3166CTRYNAME, buf.as_mut_ptr(), 16) as u64, 3)?;
        c.text(&buf, "US")?;
        c.eq(GetLocaleInfoW(0x409, LOCALE_IDEFAULTCODEPAGE, buf.as_mut_ptr(), 16) as u64, 4)?;
        c.text(&buf, "437")?;
        // A size query, then a buffer too small.
        c.eq(GetLocaleInfoW(0x409, LOCALE_SISO639LANGNAME, core::ptr::null_mut(), 0) as u64, 3)?;
        SetLastError(0);
        c.eq(GetLocaleInfoW(0x409, LOCALE_SISO639LANGNAME, buf.as_mut_ptr(), 2) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        // A number: a DWORD in the buffer, the count in WCHARs (2).
        let mut number = 0u32;
        c.eq(
            GetLocaleInfoW(
                0x409,
                LOCALE_IDEFAULTCODEPAGE | LOCALE_RETURN_NUMBER,
                (&mut number as *mut u32).cast(),
                2,
            ) as u64,
            2,
        )?;
        c.eq(number.into(), 437)?;
        let mut abuf = [0u8; 16];
        c.eq(GetLocaleInfoA(0x409, LOCALE_SISO3166CTRYNAME, abuf.as_mut_ptr(), 16) as u64, 3)?;
        c.ok(&abuf[..3] == b"US\0", abuf[0].into())?;
    }
    Ok(())
}

fn strings(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with buffers of ours.
    unsafe {
        let mut buf = [0u16; 16];
        let abc = w("abc");
        c.eq(
            LCMapStringW(0x409, LCMAP_UPPERCASE, abc.as_ptr(), -1, buf.as_mut_ptr(), 16) as u64,
            4,
        )?;
        c.text(&buf, "ABC")?;
        let mixed = w("AbC");
        buf = [0xffff; 16];
        c.eq(
            LCMapStringW(0x409, LCMAP_LOWERCASE, mixed.as_ptr(), 3, buf.as_mut_ptr(), 16) as u64,
            3,
        )?;
        c.ok(buf[..3] == abc[..3] && buf[3] == 0xffff, buf[0].into())?; // no NUL added
        c.eq(
            LCMapStringW(0x409, LCMAP_UPPERCASE, abc.as_ptr(), -1, core::ptr::null_mut(), 0) as u64,
            4,
        )?;

        let cmp = |a: &str, b: &str, flags: u32| {
            let (a, b) = (w(a), w(b));
            CompareStringW(0x409, flags, a.as_ptr(), -1, b.as_ptr(), -1)
        };
        c.eq(cmp("abc", "abd", 0) as u64, CSTR_LESS_THAN as u64)?;
        c.eq(cmp("abc", "abc", 0) as u64, CSTR_EQUAL as u64)?;
        c.eq(cmp("b", "a", 0) as u64, CSTR_GREATER_THAN as u64)?;
        c.eq(cmp("a", "B", 0) as u64, CSTR_LESS_THAN as u64)?; // not ordinal
        c.eq(cmp("a", "A", 0) as u64, CSTR_LESS_THAN as u64)?; // lower case first
        c.eq(cmp("ABC", "abc", NORM_IGNORECASE) as u64, CSTR_EQUAL as u64)?;
        c.eq(cmp("ab", "abc", 0) as u64, CSTR_LESS_THAN as u64)?;

        let s = w("a1 A,");
        let mut types = [0u16; 5];
        c.ok(GetStringTypeW(CT_CTYPE1, s.as_ptr(), 5, types.as_mut_ptr()) != 0, 0)?;
        c.eq(types[0].into(), 0x382)?; // lower, xdigit, alpha, defined
        c.eq(types[1].into(), 0x284)?; // digit, xdigit, defined
        c.eq(types[2].into(), 0x248)?; // space, blank, defined
        c.eq(types[3].into(), 0x381)?; // upper, xdigit, alpha, defined
        c.eq(types[4].into(), 0x210)?; // punct, defined
        c.eq(IsDBCSLeadByte(b'A') as u64, 0)?;
    }
    Ok(())
}

fn times(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain kernel32 calls with buffers of ours.
    unsafe {
        // 2024-10-15 (a Tuesday) 13:05:09.
        let st = SystemTime([2024, 10, 2, 15, 13, 5, 9, 0]);
        let mut buf = [0u16; 32];
        let picture = w("yyyy-MM-dd");
        c.eq(GetDateFormatW(0x409, 0, &st, picture.as_ptr(), buf.as_mut_ptr(), 32) as u64, 11)?;
        c.text(&buf, "2024-10-15")?;
        c.eq(
            GetDateFormatW(0x409, DATE_SHORTDATE, &st, core::ptr::null(), buf.as_mut_ptr(), 32)
                as u64,
            11,
        )?;
        c.text(&buf, "10/15/2024")?;
        let picture = w("d MMM yy");
        c.eq(GetDateFormatW(0x409, 0, &st, picture.as_ptr(), buf.as_mut_ptr(), 32) as u64, 10)?;
        c.text(&buf, "15 Oct 24")?;
        let picture = w("HH':'mm':'ss");
        c.eq(GetTimeFormatW(0x409, 0, &st, picture.as_ptr(), buf.as_mut_ptr(), 32) as u64, 9)?;
        c.text(&buf, "13:05:09")?;
        let picture = w("h:mm tt");
        c.eq(GetTimeFormatW(0x409, 0, &st, picture.as_ptr(), buf.as_mut_ptr(), 32) as u64, 8)?;
        c.text(&buf, "1:05 PM")?;

        // The time zone is UTC: local time is system time.
        let (mut utc, mut local) = (SystemTime::default(), SystemTime::default());
        GetSystemTime(&mut utc);
        GetLocalTime(&mut local);
        c.ok(utc.0[..5] == local.0[..5], local.0[4].into())?; // to the hour
        let mut info = [0u8; 432];
        c.eq(GetDynamicTimeZoneInformation(info.as_mut_ptr()).into(), 0)?; // UNKNOWN
        c.eq(u32::from_le_bytes(info[..4].try_into().unwrap()).into(), 0)?; // Bias
        let key = &info[172..172 + 8];
        c.ok(key == [b'U', 0, b'T', 0, b'C', 0, 0, 0], key[0].into())?; // TimeZoneKeyName
        let mut back = SystemTime::default();
        c.ok(SystemTimeToTzSpecificLocalTime(core::ptr::null(), &st, &mut local) != 0, 0)?;
        c.ok(local == st, local.0[4].into())?;
        c.ok(TzSpecificLocalTimeToSystemTime(core::ptr::null(), &local, &mut back) != 0, 0)?;
        c.ok(back == st, back.0[4].into())?;

        c.eq(GetUserGeoID(GEOCLASS_NATION) as u64, 244)?;
        let mut abuf = [0u8; 8];
        c.eq(GetGeoInfoA(244, GEO_ISO2, abuf.as_mut_ptr(), 8, 0) as u64, 3)?;
        c.ok(&abuf[..3] == b"US\0", abuf[0].into())?;
    }
    Ok(())
}

/// The preferred UI languages: one, en-US, as a double-NUL-terminated
/// list of names (MUI_LANGUAGE_NAME) or hexadecimal ids (MUI_LANGUAGE_ID).
fn ui_languages(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers of ours.
    unsafe {
        let (mut count, mut len) = (0u32, 0u32);
        let ok = GetUserPreferredUILanguages(0x8, &mut count, core::ptr::null_mut(), &mut len);
        c.ok(ok != 0 && count == 1 && len == 7, len.into())?;
        let mut buf = [0xffffu16; 16];
        let mut len = 16u32;
        c.ok(GetUserPreferredUILanguages(0x8, &mut count, buf.as_mut_ptr(), &mut len) != 0, 0)?;
        c.eq(len.into(), 7)?;
        c.text(&buf, "en-US")?;
        c.eq(buf[6].into(), 0)?;
        let mut len = 16u32;
        c.ok(GetUserPreferredUILanguages(0x4, &mut count, buf.as_mut_ptr(), &mut len) != 0, 0)?;
        c.eq(len.into(), 6)?;
        c.text(&buf, "0409")?;
        let mut len = 3u32;
        c.eq(GetUserPreferredUILanguages(0x8, &mut count, buf.as_mut_ptr(), &mut len) as u64, 0)?;
        c.eq(GetLastError().into(), 122)?; // ERROR_INSUFFICIENT_BUFFER
        let mut len = 16u32;
        c.ok(GetSystemPreferredUILanguages(0x8, &mut count, buf.as_mut_ptr(), &mut len) != 0, 0)?;
        c.text(&buf, "en-US")?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    ui_languages(&mut c)?;
    locales(&mut c)?;
    strings(&mut c)?;
    times(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_locale_test: ok\r\n");
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

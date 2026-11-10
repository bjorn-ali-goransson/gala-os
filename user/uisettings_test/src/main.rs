//! Windows.UI.ViewManagement.UISettings, the Windows Runtime class
//! desktop programs ask for the system's colours (ODT's setup.exe colours
//! its window with it; ROADMAP OF). The checks are those of Wine's
//! dlls/windows.ui/tests/uisettings.c, which run on Windows:
//!
//! - RoGetActivationFactory gives a factory that is IUnknown and
//!   IInspectable but neither agile nor an IUISettings3; RoActivateInstance
//!   an object that is IInspectable, IAgileObject, IUISettings to
//!   IUISettings6 (the runtime class's interfaces) and
//!   IWeakReferenceSource, but no IWeakReference.
//! - TextScaleFactor is within 1.0 and 2.25; AnimationsEnabled is
//!   SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)'s answer.
//! - With `HKCU\...\Explorer\Accent` AccentPalette deleted,
//!   GetColorValue(Accent) writes it back, and the seven accent shades
//!   are its entries (Light3 the first ... Dark3 the seventh).
//! - With AppsUseLightTheme 1: Foreground opaque black, Background opaque
//!   white; with 0, the reverse. In both, UIElementColor gives the
//!   thirteen classic elements GetSysColor's colour with alpha 255 and
//!   zero for the others (AccentColor .. OverlayOutsidePopup, 12345).
//! - A weak reference is IUnknown and IWeakReference only; it resolves to
//!   the object (IUnknown, IInspectable and IUISettings the same pointer)
//!   while it lives and to NULL (S_OK) once released. The factory's
//!   Release after it all gives 1.
//! - ColorValuesChanged takes a handler (a token, S_OK) and gives it up
//!   (S_OK).
//!
//! The settings and the user's theme are put back as they were. The
//! other getters' values are printed (`<name>: info ...`), not checked.
//!
//! Prints `uisettings_test: ok` (`wow64_uisettings_test` for the x86
//! build), or `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "uisettings_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_uisettings_test";

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
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSysColor(index: i32) -> u32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, update: u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn RegCreateKeyExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *const c_void,
        out: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegDeleteValueW(key: usize, name: *const u16) -> i32;
    fn RegCloseKey(key: usize) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "combase", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "combase", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WindowsCreateString(text: *const u16, len: u32, out: *mut HString) -> HResult;
    fn WindowsDeleteString(s: HString) -> HResult;
    fn RoInitialize(kind: u32) -> HResult;
    fn RoUninitialize();
    fn RoGetActivationFactory(class: HString, iid: *const Guid, out: *mut *mut c_void) -> HResult;
    fn RoActivateInstance(class: HString, out: *mut *mut c_void) -> HResult;
}

const S_OK: HResult = 0;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const HKCU: usize = 0x8000_0001;
const KEY_ALL_ACCESS: u32 = 0xf003f;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IINSPECTABLE: Guid =
    guid(0xaf86e2e0, 0xb12d, 0x4c6a, [0x9c, 0x5a, 0xd7, 0xaa, 0x65, 0x10, 0x1e, 0x90]);
const IID_IAGILEOBJECT: Guid =
    guid(0x94ea2b94, 0xe9cc, 0x49e0, [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90]);
const IID_IACTIVATIONFACTORY: Guid = guid(0x35, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWEAKREFERENCE: Guid = guid(0x37, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWEAKREFERENCESOURCE: Guid = guid(0x38, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IUISETTINGS: Guid =
    guid(0x85361600, 0x1c63, 0x4627, [0xbc, 0xb1, 0x3a, 0x89, 0xe0, 0xbc, 0x9c, 0x55]);
const IID_IUISETTINGS2: Guid =
    guid(0xbad82401, 0x2721, 0x44f9, [0xbb, 0x91, 0x2b, 0xb2, 0x28, 0xbe, 0x44, 0x2f]);
const IID_IUISETTINGS3: Guid =
    guid(0x03021be4, 0x5254, 0x4781, [0x81, 0x94, 0x51, 0x68, 0xf7, 0xd0, 0x6d, 0x7b]);
const IID_IUISETTINGS4: Guid =
    guid(0x52bb3002, 0x919b, 0x4d6b, [0x9b, 0x78, 0x8d, 0xd6, 0x6f, 0xf4, 0xb9, 0x3b]);
const IID_IUISETTINGS5: Guid =
    guid(0x5349d588, 0x0cb5, 0x5f05, [0xbd, 0x34, 0x70, 0x6b, 0x32, 0x31, 0xf0, 0xbd]);
const IID_IUISETTINGS6: Guid =
    guid(0xaef19bd7, 0xfe31, 0x5a04, [0xad, 0xa4, 0x46, 0x9a, 0xae, 0xc6, 0xdf, 0xa9]);

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

fn info(what: &[u8], v: u64) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what).s(b" ").dec(v).s(b"\r\n").print();
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

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn hr(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

/// Method `index` of COM object `obj`, as a function of type F.
///
/// # Safety
/// `obj` is a live COM object whose vtable has that slot of type F.
unsafe fn method<F: Copy>(obj: *mut c_void, index: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vtable = *(obj as *const *const usize);
        core::mem::transmute_copy(&*vtable.add(index))
    }
}

type QueryFn = extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult;
type CountFn = extern "system" fn(*mut c_void) -> u32;

fn query(obj: *mut c_void, iid: &Guid) -> (HResult, *mut c_void) {
    let mut out = null_mut();
    // SAFETY: a live COM object.
    let hr = unsafe { method::<QueryFn>(obj, 0)(obj, iid, &mut out) };
    (hr, out)
}

fn release(obj: *mut c_void) -> u32 {
    // SAFETY: a live COM object we hold a reference to.
    unsafe { method::<CountFn>(obj, 2)(obj) }
}

/// QueryInterface for `iid` succeeds (S_OK, released again) or fails
/// with E_NOINTERFACE, as `yes` says.
#[track_caller]
fn has(c: &mut Checks, obj: *mut c_void, iid: &Guid, yes: bool) -> Result<(), u32> {
    let (hr, out) = query(obj, iid);
    c.hr(hr, if yes { S_OK } else { E_NOINTERFACE })?;
    if !out.is_null() {
        release(out);
    }
    Ok(())
}

/// A NUL-terminated UTF-16 copy of ASCII `text`.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// HKCU\`path`, opened (created if need be) for all access.
fn open_key(path: &str) -> usize {
    let k = wide::<128>(path);
    let mut h = 0;
    // SAFETY: a NUL-terminated name, our out-parameter.
    unsafe {
        RegCreateKeyExW(
            HKCU,
            k.as_ptr(),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut h,
            null_mut(),
        )
    };
    h
}

/// Value `name` of `key` into `buf`: (type, length), or None.
fn get_value(key: usize, name: &str, buf: &mut [u8]) -> Option<(u32, u32)> {
    let n = wide::<32>(name);
    let (mut kind, mut len) = (0u32, buf.len() as u32);
    // SAFETY: a NUL-terminated name, our buffer and out-parameters.
    let r = unsafe {
        RegQueryValueExW(key, n.as_ptr(), null_mut(), &mut kind, buf.as_mut_ptr(), &mut len)
    };
    (r == 0).then_some((kind, len))
}

fn set_value(key: usize, name: &str, kind: u32, data: &[u8]) {
    let n = wide::<32>(name);
    // SAFETY: a NUL-terminated name, our data.
    unsafe { RegSetValueExW(key, n.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
}

fn delete_value(key: usize, name: &str) {
    let n = wide::<32>(name);
    // SAFETY: a NUL-terminated name.
    unsafe { RegDeleteValueW(key, n.as_ptr()) };
}

type ColorFn = extern "system" fn(*mut c_void, u32, *mut [u8; 4]) -> HResult;

/// GetColorValue(kind) (IUISettings3) or UIElementColor(kind)
/// (IUISettings) as [A, R, G, B].
fn color(c: &mut Checks, obj: *mut c_void, index: usize, kind: u32) -> Result<[u8; 4], u32> {
    let mut v = [1u8; 4];
    // SAFETY: the interface's method `index`.
    let hr = unsafe { method::<ColorFn>(obj, index)(obj, kind, &mut v) };
    c.hr(hr, S_OK)?;
    Ok(v)
}

/// UIElementColor's answers match GetSysColor for the classic elements
/// and are zero for the rest.
fn check_elements(c: &mut Checks, settings: *mut c_void) -> Result<(), u32> {
    const CLASSIC: [i32; 13] = [2, 1, 15, 18, 9, 17, 13, 14, 26, 3, 19, 5, 8];
    for (kind, &index) in CLASSIC.iter().enumerate() {
        let v = color(c, settings, 18, kind as u32)?;
        // SAFETY: a plain call.
        let rgb = unsafe { GetSysColor(index) };
        let want = [255, rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8];
        c.eq(u32::from_be_bytes(v).into(), u32::from_be_bytes(want).into())?;
    }
    for kind in (1000..=1012).chain([12345]) {
        let v = color(c, settings, 18, kind)?;
        c.eq(u32::from_be_bytes(v).into(), 0)?;
    }
    Ok(())
}

/// A minimal event handler: counts its references, answers any
/// interface with itself.
#[repr(C)]
struct Handler {
    vtable: *const usize,
    refs: AtomicU32,
}

extern "system" fn h_query(
    this: *mut Handler,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: our object; the caller's out-parameter.
    unsafe {
        (*this).refs.fetch_add(1, Ordering::Relaxed);
        *out = this.cast();
    }
    S_OK
}
extern "system" fn h_add_ref(this: *mut Handler) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
extern "system" fn h_release(this: *mut Handler) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_sub(1, Ordering::Relaxed) - 1 }
}
extern "system" fn h_invoke(
    _this: *mut Handler,
    _sender: *mut c_void,
    _args: *mut c_void,
) -> HResult {
    S_OK
}

struct HandlerVtable([usize; 4]);

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: COM calls on the objects these calls hand out, our buffers.
    unsafe {
        c.hr(RoInitialize(1), S_OK)?;
        let name = wide::<64>("Windows.UI.ViewManagement.UISettings");
        let mut class = null_mut();
        c.hr(WindowsCreateString(name.as_ptr(), 36, &mut class), S_OK)?;

        // The factory.
        let mut factory = null_mut();
        c.hr(RoGetActivationFactory(class, &IID_IACTIVATIONFACTORY, &mut factory), S_OK)?;
        has(&mut c, factory, &IID_IUNKNOWN, true)?;
        has(&mut c, factory, &IID_IINSPECTABLE, true)?;
        has(&mut c, factory, &IID_IAGILEOBJECT, false)?;
        has(&mut c, factory, &IID_IUISETTINGS3, false)?;

        // An object and its interfaces.
        let mut inspectable = null_mut();
        c.hr(RoActivateInstance(class, &mut inspectable), S_OK)?;
        WindowsDeleteString(class);
        for iid in [
            &IID_IINSPECTABLE,
            &IID_IAGILEOBJECT,
            &IID_IUISETTINGS,
            &IID_IUISETTINGS2,
            &IID_IUISETTINGS3,
            &IID_IUISETTINGS4,
            &IID_IUISETTINGS5,
            &IID_IUISETTINGS6,
            &IID_IWEAKREFERENCESOURCE,
        ] {
            has(&mut c, inspectable, iid, true)?;
        }
        has(&mut c, inspectable, &IID_IWEAKREFERENCE, false)?;
        let (hr, settings) = query(inspectable, &IID_IUISETTINGS);
        c.hr(hr, S_OK)?;
        let (hr, settings2) = query(inspectable, &IID_IUISETTINGS2);
        c.hr(hr, S_OK)?;
        let (hr, settings3) = query(inspectable, &IID_IUISETTINGS3);
        c.hr(hr, S_OK)?;

        // TextScaleFactor and AnimationsEnabled.
        let mut scale = 12345.6f64;
        let get_f64: extern "system" fn(*mut c_void, *mut f64) -> HResult = method(settings2, 6);
        c.hr(get_f64(settings2, &mut scale), S_OK)?;
        c.ok((1.0..=2.25).contains(&scale), scale as u64)?;
        let mut animation = 0u32;
        let r =
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animation).cast(), 0);
        c.ok(r != 0, 0)?;
        let get_bool: extern "system" fn(*mut c_void, *mut u8) -> HResult = method(settings, 12);
        let mut enabled = 7u8;
        c.hr(get_bool(settings, &mut enabled), S_OK)?;
        c.eq(enabled.into(), u64::from(animation != 0))?;

        // The other getters, printed.
        let get_u32: extern "system" fn(*mut c_void, *mut u32) -> HResult = method(settings, 6);
        let mut v = 0u32;
        c.hr(get_u32(settings, &mut v), S_OK)?;
        info(b"HandPreference", v.into());
        let get_size: extern "system" fn(*mut c_void, *mut [f32; 2]) -> HResult =
            method(settings, 8);
        let mut size = [0f32; 2];
        c.hr(get_size(settings, &mut size), S_OK)?;
        info(b"ScrollBarSize width", size[0] as u64);
        for (index, what) in [
            (11, &b"MessageDuration"[..]),
            (14, b"CaretBlinkRate"),
            (15, b"CaretWidth"),
            (16, b"DoubleClickTime"),
            (17, b"MouseHoverTime"),
        ] {
            let get: extern "system" fn(*mut c_void, *mut u32) -> HResult = method(settings, index);
            c.hr(get(settings, &mut v), S_OK)?;
            info(what, v.into());
        }

        // The accent palette, written back when absent.
        let accent = open_key("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
        c.ok(accent != 0, 0)?;
        let mut saved = [0u8; 64];
        let saved_len = get_value(accent, "AccentPalette", &mut saved).map(|(_, n)| n);
        delete_value(accent, "AccentPalette");
        let mut palette = [0u8; 64];
        c.ok(get_value(accent, "AccentPalette", &mut palette).is_none(), 0)?;
        color(&mut c, settings3, 6, 5)?;
        let got = get_value(accent, "AccentPalette", &mut palette);
        c.ok(got.is_some_and(|(kind, n)| kind == REG_BINARY && n >= 28), 0)?;
        // UIColorType 2-8 (AccentDark3 .. AccentLight3) are entries 6 .. 0.
        for kind in 2..=8u32 {
            let entry = 8 - kind as usize;
            let e = &palette[4 * entry..4 * entry + 4];
            let v = color(&mut c, settings3, 6, kind)?;
            c.eq(
                u32::from_be_bytes([v[1], v[2], v[3], 0]).into(),
                u32::from_be_bytes([e[0], e[1], e[2], 0]).into(),
            )?;
        }
        match saved_len {
            Some(n) => set_value(accent, "AccentPalette", REG_BINARY, &saved[..n as usize]),
            None => delete_value(accent, "AccentPalette"),
        }
        RegCloseKey(accent);

        // The apps' theme.
        let theme = open_key("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        c.ok(theme != 0, 0)?;
        let mut old = [0u8; 4];
        let had = get_value(theme, "AppsUseLightTheme", &mut old).is_some();
        for light in [1u32, 0] {
            set_value(theme, "AppsUseLightTheme", REG_DWORD, &light.to_le_bytes());
            check_elements(&mut c, settings)?;
            let (fg, bg) = if light == 1 { (0, 0xff) } else { (0xff, 0) };
            let v = color(&mut c, settings3, 6, 1)?;
            c.eq(u32::from_be_bytes(v).into(), u32::from_be_bytes([255, fg, fg, fg]).into())?;
            let v = color(&mut c, settings3, 6, 0)?;
            c.eq(u32::from_be_bytes(v).into(), u32::from_be_bytes([255, bg, bg, bg]).into())?;
        }
        if had {
            set_value(theme, "AppsUseLightTheme", REG_DWORD, &old);
        } else {
            delete_value(theme, "AppsUseLightTheme");
        }
        RegCloseKey(theme);

        // An event handler.
        let vtable = HandlerVtable([
            h_query as *const () as usize,
            h_add_ref as *const () as usize,
            h_release as *const () as usize,
            h_invoke as *const () as usize,
        ]);
        let mut handler = Handler { vtable: vtable.0.as_ptr(), refs: AtomicU32::new(1) };
        let add: extern "system" fn(*mut c_void, *mut Handler, *mut i64) -> HResult =
            method(settings3, 7);
        let remove: extern "system" fn(*mut c_void, i64) -> HResult = method(settings3, 8);
        let mut token = 0i64;
        c.hr(add(settings3, &mut handler, &mut token), S_OK)?;
        c.ok(token != 0, 0)?;
        info(b"handler references while added", handler.refs.load(Ordering::Relaxed).into());
        c.hr(remove(settings3, token), S_OK)?;
        info(b"handler references after removal", handler.refs.load(Ordering::Relaxed).into());

        release(settings);
        release(settings2);
        release(settings3);

        // A weak reference.
        let (hr, source) = query(inspectable, &IID_IWEAKREFERENCESOURCE);
        c.hr(hr, S_OK)?;
        let mut weak = null_mut();
        let get_weak: extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult =
            method(source, 3);
        c.hr(get_weak(source, &mut weak), S_OK)?;
        release(source);
        has(&mut c, weak, &IID_IUNKNOWN, true)?;
        has(&mut c, weak, &IID_IWEAKREFERENCE, true)?;
        has(&mut c, weak, &IID_IINSPECTABLE, false)?;
        has(&mut c, weak, &IID_IAGILEOBJECT, false)?;
        has(&mut c, weak, &IID_IUISETTINGS, false)?;
        let resolve: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult =
            method(weak, 3);
        let (mut unknown, mut second, mut third) = (null_mut(), null_mut(), null_mut());
        c.hr(resolve(weak, &IID_IUNKNOWN, &mut unknown), S_OK)?;
        c.ok(!unknown.is_null(), 0)?;
        c.hr(resolve(weak, &IID_IINSPECTABLE, &mut second), S_OK)?;
        c.eq(second as u64, unknown as u64)?;
        release(second);
        c.hr(resolve(weak, &IID_IUISETTINGS, &mut third), S_OK)?;
        c.eq(third as u64, unknown as u64)?;
        release(third);
        release(unknown);
        release(inspectable);
        let mut gone = 0xdead_beef_usize as *mut c_void;
        c.hr(resolve(weak, &IID_IINSPECTABLE, &mut gone), S_OK)?;
        c.ok(gone.is_null(), gone as u64)?;
        release(weak);

        c.eq(release(factory).into(), 1)?;
        RoUninitialize();
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

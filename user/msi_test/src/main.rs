//! msi.dll's functions the Office Deployment Tool's setup.exe imports
//! (ROADMAP OF; it imports them by ordinal), with Windows' answers on a
//! machine where the products and components asked about are not
//! installed. The expected values are those of Wine's conformance tests
//! (dlls/msi/tests/msi.c, run against Windows), and of real Windows on
//! CI's runner (this program's run there):
//! - the ordinals: #8 MsiCloseHandle, #39 MsiEnumComponentQualifiersW,
//!   #45 MsiEnumProductsW, #90 MsiLocateComponentW, #113
//!   MsiQueryProductStateW, #141 MsiSetInternalUI, #173
//!   MsiGetComponentPathW, #190 MsiConfigureProductExW, #203
//!   MsiProvideQualifiedComponentExW (GetProcAddress by ordinal gives
//!   the named export);
//! - MsiCloseHandle(0) is ERROR_SUCCESS, a handle never opened
//!   ERROR_INVALID_HANDLE;
//! - MsiSetInternalUI returns the previous level: INSTALLUILEVEL_DEFAULT
//!   first, the level set after; INSTALLUILEVEL_NOCHANGE leaves it; an
//!   invalid level is INSTALLUILEVEL_NOCHANGE and changes nothing; the
//!   window handle passed is swapped with the previous one;
//! - MsiQueryProductStateW: INSTALLSTATE_INVALIDARG for NULL, "" and a
//!   string that is not a braced GUID; INSTALLSTATE_UNKNOWN with last
//!   error 0 for a product not installed;
//! - MsiEnumProductsW: ERROR_INVALID_PARAMETER without a buffer; from
//!   index 0 every product is a braced GUID of 38 characters, ending
//!   with ERROR_NO_MORE_ITEMS;
//! - MsiGetComponentPathW and MsiLocateComponentW: INSTALLSTATE_INVALIDARG
//!   without a product or component, with a malformed GUID, or with a
//!   buffer but no size; INSTALLSTATE_UNKNOWN for one not installed (the
//!   size unchanged), with or without a buffer;
//! - MsiConfigureProductExW: ERROR_INVALID_PARAMETER for NULL or a
//!   malformed product, ERROR_UNKNOWN_PRODUCT for one not installed;
//! - MsiProvideQualifiedComponentExW and MsiEnumComponentQualifiersW:
//!   ERROR_UNKNOWN_COMPONENT for a component not installed;
//!   MsiEnumComponentQualifiersW ERROR_INVALID_PARAMETER without one.
//!
//! The same source builds `wow64_msi_test`, the 32-bit (x86) program.
//! Prints `msi_test: ok` (`wow64_msi_test: ok`), or `<name>: check N got
//! X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "msi_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_msi_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "msi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "msi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn MsiCloseHandle(handle: u32) -> u32;
    fn MsiSetInternalUI(level: u32, window: *mut usize) -> u32;
    fn MsiQueryProductStateW(product: *const u16) -> i32;
    fn MsiEnumProductsW(index: u32, product: *mut u16) -> u32;
    fn MsiGetComponentPathW(
        product: *const u16,
        component: *const u16,
        path: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn MsiLocateComponentW(component: *const u16, path: *mut u16, size: *mut u32) -> i32;
    fn MsiConfigureProductExW(
        product: *const u16,
        level: i32,
        state: i32,
        command_line: *const u16,
    ) -> u32;
    fn MsiProvideQualifiedComponentExW(
        component: *const u16,
        qualifier: *const u16,
        mode: u32,
        product: *const u16,
        unused1: u32,
        unused2: u32,
        path: *mut u16,
        size: *mut u32,
    ) -> u32;
    fn MsiEnumComponentQualifiersW(
        component: *const u16,
        index: u32,
        qualifier: *mut u16,
        qualifier_size: *mut u32,
        data: *mut u16,
        data_size: *mut u32,
    ) -> u32;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_UNKNOWN_PRODUCT: u32 = 1605;
const ERROR_UNKNOWN_COMPONENT: u32 = 1607;
const INSTALLSTATE_INVALIDARG: i32 = -2;
const INSTALLSTATE_UNKNOWN: i32 = -1;
const INSTALLSTATE_DEFAULT: i32 = 5;
const INSTALLLEVEL_DEFAULT: i32 = 0;
const INSTALLUILEVEL_NOCHANGE: u32 = 0;
const INSTALLUILEVEL_DEFAULT: u32 = 1;
const INSTALLUILEVEL_NONE: u32 = 2;
const INSTALLUILEVEL_FULL: u32 = 5;
const INSTALLMODE_EXISTING: u32 = -1i32 as u32;

/// A product and a component no machine has (made-up GUIDs).
const PRODUCT: &str = "{6700E8CF-95AB-4D9C-BC2C-15840DEA7A5D}";
const COMPONENT: &str = "{A9D5E2C4-31B7-4F0E-9C6A-2E8D7B1F4A03}";

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

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// An INSTALLSTATE (a signed value) that should be `want`.
    #[track_caller]
    fn state(&mut self, got: i32, want: i32) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// The nine ordinals setup.exe imports: GetProcAddress by ordinal is the
/// export of that name.
fn ordinals(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: a NUL-terminated name.
    let module = unsafe { LoadLibraryW(wide::<8>("msi.dll").as_ptr()) };
    // SAFETY: plain call.
    c.ok(module != 0, u64::from(unsafe { GetLastError() }))?;
    let named: [(u16, &core::ffi::CStr); 9] = [
        (8, c"MsiCloseHandle"),
        (39, c"MsiEnumComponentQualifiersW"),
        (45, c"MsiEnumProductsW"),
        (90, c"MsiLocateComponentW"),
        (113, c"MsiQueryProductStateW"),
        (141, c"MsiSetInternalUI"),
        (173, c"MsiGetComponentPathW"),
        (190, c"MsiConfigureProductExW"),
        (203, c"MsiProvideQualifiedComponentExW"),
    ];
    for (ordinal, name) in named {
        // SAFETY: a loaded module; an ordinal (MAKEINTRESOURCE) and a name.
        let (by_ordinal, by_name) = unsafe {
            (
                GetProcAddress(module, usize::from(ordinal) as *const u8),
                GetProcAddress(module, name.as_ptr().cast()),
            )
        };
        c.ok(by_name != 0 && by_ordinal == by_name, u64::from(ordinal))?;
    }
    Ok(())
}

fn handles_and_ui(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented calls with our arguments.
    unsafe {
        c.eq(MsiCloseHandle(0).into(), ERROR_SUCCESS.into())?;
        c.eq(MsiCloseHandle(0x1234_5678).into(), ERROR_INVALID_HANDLE.into())?;

        c.eq(
            MsiSetInternalUI(INSTALLUILEVEL_FULL, null_mut()).into(),
            INSTALLUILEVEL_DEFAULT.into(),
        )?;
        c.eq(
            MsiSetInternalUI(INSTALLUILEVEL_DEFAULT, null_mut()).into(),
            INSTALLUILEVEL_FULL.into(),
        )?;
        c.eq(
            MsiSetInternalUI(INSTALLUILEVEL_NOCHANGE, null_mut()).into(),
            INSTALLUILEVEL_DEFAULT.into(),
        )?;
        c.eq(MsiSetInternalUI(0xdead_beef, null_mut()).into(), INSTALLUILEVEL_NOCHANGE.into())?;
        // The window: the previous owner comes back in its place. What a
        // process starts with is not documented (Windows on the CI runner
        // returns a nonzero owner, 0x2e1; gala none), and a value that is
        // not a window does not become the owner: the next call returns
        // the same previous owner again.
        let mut window = 0x1_0000usize;
        c.eq(
            MsiSetInternalUI(INSTALLUILEVEL_NONE, &mut window).into(),
            INSTALLUILEVEL_DEFAULT.into(),
        )?;
        let mut back = 0usize;
        c.eq(
            MsiSetInternalUI(INSTALLUILEVEL_DEFAULT, &mut back).into(),
            INSTALLUILEVEL_NONE.into(),
        )?;
        c.eq(back as u64, window as u64)?;
    }
    Ok(())
}

fn products(c: &mut Checks) -> Result<(), u32> {
    let product = wide::<39>(PRODUCT);
    let empty = [0u16; 1];
    let garbage = wide::<8>("garbage");
    let unbraced = wide::<37>("6700E8CF-95AB-4D9C-BC2C-15840DEA7A5D");
    // SAFETY: documented calls with our arguments.
    unsafe {
        SetLastError(0xdead_beef);
        c.state(MsiQueryProductStateW(null()), INSTALLSTATE_INVALIDARG)?;
        c.eq(GetLastError().into(), 0xdead_beef)?;
        c.state(MsiQueryProductStateW(empty.as_ptr()), INSTALLSTATE_INVALIDARG)?;
        c.state(MsiQueryProductStateW(garbage.as_ptr()), INSTALLSTATE_INVALIDARG)?;
        c.state(MsiQueryProductStateW(unbraced.as_ptr()), INSTALLSTATE_INVALIDARG)?;
        SetLastError(0xdead_beef);
        c.state(MsiQueryProductStateW(product.as_ptr()), INSTALLSTATE_UNKNOWN)?;
        c.eq(GetLastError().into(), ERROR_SUCCESS.into())?;

        c.eq(MsiEnumProductsW(0, null_mut()).into(), ERROR_INVALID_PARAMETER.into())?;
        let mut index = 0;
        loop {
            let mut guid = [0xffffu16; 39];
            let r = MsiEnumProductsW(index, guid.as_mut_ptr());
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            c.eq(r.into(), ERROR_SUCCESS.into())?;
            let braced = guid[0] == u16::from(b'{') && guid[37] == u16::from(b'}') && guid[38] == 0;
            c.ok(braced, index.into())?;
            index += 1;
            c.ok(index < 10_000, index.into())?;
        }

        c.eq(
            MsiConfigureProductExW(null(), INSTALLLEVEL_DEFAULT, INSTALLSTATE_DEFAULT, null())
                .into(),
            ERROR_INVALID_PARAMETER.into(),
        )?;
        c.eq(
            MsiConfigureProductExW(
                garbage.as_ptr(),
                INSTALLLEVEL_DEFAULT,
                INSTALLSTATE_DEFAULT,
                null(),
            )
            .into(),
            ERROR_INVALID_PARAMETER.into(),
        )?;
        c.eq(
            MsiConfigureProductExW(
                product.as_ptr(),
                INSTALLLEVEL_DEFAULT,
                INSTALLSTATE_DEFAULT,
                null(),
            )
            .into(),
            ERROR_UNKNOWN_PRODUCT.into(),
        )?;
    }
    Ok(())
}

fn components(c: &mut Checks) -> Result<(), u32> {
    let product = wide::<39>(PRODUCT);
    let component = wide::<39>(COMPONENT);
    let short = wide::<34>("{00000000-0000-0000-000000000000}");
    let qualifier = wide::<10>("qualifier");
    let mut path = [0u16; 260];
    // SAFETY: documented calls with our arguments.
    unsafe {
        let mut size = 260u32;
        c.state(
            MsiGetComponentPathW(null(), component.as_ptr(), path.as_mut_ptr(), &mut size),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiGetComponentPathW(product.as_ptr(), null(), path.as_mut_ptr(), &mut size),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiGetComponentPathW(product.as_ptr(), short.as_ptr(), path.as_mut_ptr(), &mut size),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiGetComponentPathW(
                product.as_ptr(),
                component.as_ptr(),
                path.as_mut_ptr(),
                null_mut(),
            ),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiGetComponentPathW(product.as_ptr(), component.as_ptr(), null_mut(), &mut size),
            INSTALLSTATE_UNKNOWN,
        )?;
        c.state(
            MsiGetComponentPathW(
                product.as_ptr(),
                component.as_ptr(),
                path.as_mut_ptr(),
                &mut size,
            ),
            INSTALLSTATE_UNKNOWN,
        )?;
        c.eq(size.into(), 260)?;

        c.state(
            MsiLocateComponentW(null(), path.as_mut_ptr(), &mut size),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiLocateComponentW(component.as_ptr(), path.as_mut_ptr(), null_mut()),
            INSTALLSTATE_INVALIDARG,
        )?;
        c.state(
            MsiLocateComponentW(component.as_ptr(), null_mut(), &mut size),
            INSTALLSTATE_UNKNOWN,
        )?;
        c.state(
            MsiLocateComponentW(component.as_ptr(), path.as_mut_ptr(), &mut size),
            INSTALLSTATE_UNKNOWN,
        )?;
        c.eq(size.into(), 260)?;

        c.eq(
            MsiProvideQualifiedComponentExW(
                component.as_ptr(),
                qualifier.as_ptr(),
                INSTALLMODE_EXISTING,
                product.as_ptr(),
                0,
                0,
                path.as_mut_ptr(),
                &mut size,
            )
            .into(),
            ERROR_UNKNOWN_COMPONENT.into(),
        )?;

        let (mut qualifier_buf, mut data) = ([0u16; 64], [0u16; 64]);
        let (mut qualifier_size, mut data_size) = (64u32, 64u32);
        c.eq(
            MsiEnumComponentQualifiersW(
                null(),
                0,
                qualifier_buf.as_mut_ptr(),
                &mut qualifier_size,
                data.as_mut_ptr(),
                &mut data_size,
            )
            .into(),
            ERROR_INVALID_PARAMETER.into(),
        )?;
        c.eq(
            MsiEnumComponentQualifiersW(
                component.as_ptr(),
                0,
                qualifier_buf.as_mut_ptr(),
                &mut qualifier_size,
                data.as_mut_ptr(),
                &mut data_size,
            )
            .into(),
            ERROR_UNKNOWN_COMPONENT.into(),
        )?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    ordinals(&mut c)?;
    handles_and_ui(&mut c)?;
    products(&mut c)?;
    components(&mut c)?;
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

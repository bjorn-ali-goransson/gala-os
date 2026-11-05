//! gala's GPU stub DLLs (ROADMAP GPU G0): with no Direct3D adapter,
//! the entry points Chromium's GPU process calls fail cleanly, as on a
//! Windows machine that cannot give a device (on Windows with a GPU these
//! calls succeed; this test is gala's no-adapter contract):
//!
//! 1. dxgi.dll: CreateDXGIFactory, CreateDXGIFactory1 and
//!    CreateDXGIFactory2 return DXGI_ERROR_UNSUPPORTED and set the factory
//!    to NULL; a NULL output is E_INVALIDARG;
//! 2. d3d11.dll: D3D11CreateDevice (hardware) returns
//!    DXGI_ERROR_UNSUPPORTED, device and context NULL, feature level 0;
//! 3. d3d12.dll: ordinal 101 is D3D12CreateDevice (chrome.dll imports it
//!    so), which returns DXGI_ERROR_UNSUPPORTED and a NULL device;
//!    D3D12GetDebugInterface is ordinal 102;
//! 4. dcomp.dll: DCompositionCreateDevice, DCompositionCreateDevice2 and
//!    DCompositionCreateDevice3 return DXGI_ERROR_UNSUPPORTED, device NULL.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Proc = *const c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> Proc;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const UNSUPPORTED: i32 = 0x887a_0004_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
/// A non-null placeholder output, overwritten with NULL by the stubs.
const JUNK: *mut c_void = 0x1234 as *mut c_void;
/// IDXGIFactory1's IID (dxgi.h; its value is not checked by the stubs).
const IID: [u8; 16] = [0; 16];

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

fn check(ok: bool, n: u32) {
    if !ok {
        let mut line = *b"gpu_stub_test: check 00 failed\r\n";
        line[21] = b'0' + (n / 10) as u8;
        line[22] = b'0' + (n % 10) as u8;
        out(&line);
        // SAFETY: ends the process.
        unsafe { ExitProcess(n) }
    }
}

/// `dll`'s export `name` (NUL-terminated), or by ordinal when `name` is
/// below 0x10000.
fn proc(dll: &[u8], name: *const u8, n: u32) -> Proc {
    // SAFETY: NUL-terminated names.
    let p = unsafe {
        let module = LoadLibraryA(dll.as_ptr());
        check(!module.is_null(), n);
        GetProcAddress(module, name)
    };
    check(!p.is_null(), n);
    p
}

type Factory = extern "system" fn(*const u8, *mut *mut c_void) -> i32;
type Factory2 = extern "system" fn(u32, *const u8, *mut *mut c_void) -> i32;
type D3d11 = extern "system" fn(
    *mut c_void,
    u32,
    *mut c_void,
    u32,
    *const u32,
    u32,
    u32,
    *mut *mut c_void,
    *mut u32,
    *mut *mut c_void,
) -> i32;
type D3d12 = extern "system" fn(*mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
type Dcomp = extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32;

fn tests() {
    // SAFETY (whole function): each export has the type it is cast to
    // (the SDK's prototypes).
    unsafe {
        // 1.
        for (i, name) in [&b"CreateDXGIFactory\0"[..], b"CreateDXGIFactory1\0"].iter().enumerate() {
            let f: Factory = core::mem::transmute(proc(b"dxgi.dll\0", name.as_ptr(), 1));
            let mut factory = JUNK;
            check(f(IID.as_ptr(), &mut factory) == UNSUPPORTED && factory.is_null(), 2 + i as u32);
            check(f(IID.as_ptr(), core::ptr::null_mut()) == E_INVALIDARG, 4);
        }
        let f2: Factory2 =
            core::mem::transmute(proc(b"dxgi.dll\0", c"CreateDXGIFactory2".as_ptr().cast(), 5));
        let mut factory = JUNK;
        check(f2(0, IID.as_ptr(), &mut factory) == UNSUPPORTED && factory.is_null(), 6);

        // 2. D3D_DRIVER_TYPE_HARDWARE (1), D3D11_SDK_VERSION (7).
        let d3d11: D3d11 =
            core::mem::transmute(proc(b"d3d11.dll\0", c"D3D11CreateDevice".as_ptr().cast(), 7));
        let (mut device, mut context, mut level) = (JUNK, JUNK, 0xb000u32);
        let hr = d3d11(
            core::ptr::null_mut(),
            1,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            0,
            7,
            &mut device,
            &mut level,
            &mut context,
        );
        check(hr == UNSUPPORTED && device.is_null() && context.is_null() && level == 0, 8);

        // 3. Ordinal 101 is D3D12CreateDevice; 102 D3D12GetDebugInterface.
        let by_ordinal = proc(b"d3d12.dll\0", 101 as *const u8, 9);
        check(by_ordinal == proc(b"d3d12.dll\0", c"D3D12CreateDevice".as_ptr().cast(), 10), 10);
        check(
            proc(b"d3d12.dll\0", 102 as *const u8, 11)
                == proc(b"d3d12.dll\0", c"D3D12GetDebugInterface".as_ptr().cast(), 11),
            11,
        );
        let d3d12: D3d12 = core::mem::transmute(by_ordinal);
        let mut device = JUNK;
        // D3D_FEATURE_LEVEL_11_0.
        check(d3d12(core::ptr::null_mut(), 0xb000, IID.as_ptr(), &mut device) == UNSUPPORTED, 12);
        check(device.is_null(), 13);

        // 4.
        for (i, name) in [
            &b"DCompositionCreateDevice\0"[..],
            b"DCompositionCreateDevice2\0",
            b"DCompositionCreateDevice3\0",
        ]
        .iter()
        .enumerate()
        {
            let f: Dcomp = core::mem::transmute(proc(b"dcomp.dll\0", name.as_ptr(), 14));
            let mut device = JUNK;
            check(
                f(core::ptr::null_mut(), IID.as_ptr(), &mut device) == UNSUPPORTED,
                15 + i as u32,
            );
            check(device.is_null(), 18);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    tests();
    out(b"gpu_stub_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(99) }
}

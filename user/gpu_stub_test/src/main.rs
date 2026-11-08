//! gala's GPU stub DLLs (ROADMAP GPU G0): with no Direct3D adapter,
//! the entry points Chromium's GPU process calls fail cleanly, as on a
//! Windows machine that cannot give a device (on Windows with a GPU these
//! calls succeed, and every Windows machine has at least the Basic Render
//! Driver's adapter; this test is gala's no-adapter contract):
//!
//! 1. dxgi.dll: CreateDXGIFactory and CreateDXGIFactory1 give a factory
//!    (IDXGIFactory, IDXGIFactory1; the Khronos Vulkan loader needs one,
//!    GPU G2) that has no adapters: EnumAdapters and EnumAdapters1 of 0
//!    are DXGI_ERROR_NOT_FOUND with the adapter NULL; QueryInterface
//!    gives IUnknown and IDXGIFactory1 (the same object) and refuses
//!    another interface with E_NOINTERFACE; the last Release returns 0.
//!    Another interface asked of CreateDXGIFactory1 is E_NOINTERFACE with
//!    the factory NULL; a NULL output is E_INVALIDARG. CreateDXGIFactory2
//!    gives the same factory (IDXGIFactory2 and later, which presenting
//!    needs, are E_NOINTERFACE);
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
const NOT_FOUND: i32 = 0x887a_0002_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
/// A non-null placeholder output, overwritten with NULL by the stubs.
const JUNK: *mut c_void = 0x1234 as *mut c_void;
/// An IID no DXGI object has (all zero: GUID_NULL).
const IID: [u8; 16] = [0; 16];

/// A GUID's bytes as in memory (Data1-3 little-endian).
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// dxgi.h's IIDs.
const IID_IUNKNOWN: [u8; 16] = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDXGIFACTORY: [u8; 16] =
    guid(0x7b71_66ec, 0x21c7, 0x44ae, [0xb2, 0x1a, 0xc9, 0xae, 0x32, 0x1a, 0xe3, 0x69]);
const IID_IDXGIFACTORY1: [u8; 16] =
    guid(0x770a_ae78, 0xf26f, 0x4dba, [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87]);

/// IDXGIFactory1's vtable slots: IUnknown (0-2), IDXGIObject (3-6),
/// IDXGIFactory (7-11: EnumAdapters 7), IDXGIFactory1 (EnumAdapters1 12).
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
const ENUM_ADAPTERS: usize = 7;
const ENUM_ADAPTERS1: usize = 12;

/// Method `slot` of COM object `object`.
///
/// # Safety
///
/// `object` is a live COM object with at least `slot + 1` methods.
unsafe fn method(object: *mut c_void, slot: usize) -> *const c_void {
    // SAFETY: as the caller promises: the vtable pointer, then the slot.
    unsafe { *(*(object as *const *const *const c_void)).add(slot) }
}

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
        type Query = extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32;
        type Release = extern "system" fn(*mut c_void) -> u32;
        type Enum = extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
        for (i, (name, iid)) in [
            (&b"CreateDXGIFactory\0"[..], IID_IDXGIFACTORY),
            (b"CreateDXGIFactory1\0", IID_IDXGIFACTORY1),
        ]
        .iter()
        .enumerate()
        {
            let f: Factory = core::mem::transmute(proc(b"dxgi.dll\0", name.as_ptr(), 1));
            let mut factory = JUNK;
            check(f(iid.as_ptr(), &mut factory) == 0 && !factory.is_null(), 2 + i as u32);
            let enum_slots: &[usize] =
                if i == 0 { &[ENUM_ADAPTERS] } else { &[ENUM_ADAPTERS, ENUM_ADAPTERS1] };
            for &slot in enum_slots {
                let enumerate: Enum = core::mem::transmute(method(factory, slot));
                let mut adapter = JUNK;
                check(enumerate(factory, 0, &mut adapter) == NOT_FOUND && adapter.is_null(), 19);
            }
            let query: Query = core::mem::transmute(method(factory, QUERY_INTERFACE));
            let mut same = JUNK;
            check(query(factory, IID_IUNKNOWN.as_ptr(), &mut same) == 0 && same == factory, 20);
            let mut other = JUNK;
            check(query(factory, IID.as_ptr(), &mut other) == E_NOINTERFACE && other.is_null(), 21);
            let release: Release = core::mem::transmute(method(factory, RELEASE));
            check(release(factory) == 1, 22);
            check(release(factory) == 0, 23);
            let mut none = JUNK;
            check(f(IID.as_ptr(), &mut none) == E_NOINTERFACE && none.is_null(), 24);
            check(f(iid.as_ptr(), core::ptr::null_mut()) == E_INVALIDARG, 4);
        }
        let f2: Factory2 =
            core::mem::transmute(proc(b"dxgi.dll\0", c"CreateDXGIFactory2".as_ptr().cast(), 5));
        let mut factory = JUNK;
        check(f2(0, IID_IDXGIFACTORY1.as_ptr(), &mut factory) == 0 && !factory.is_null(), 6);
        let release: Release = core::mem::transmute(method(factory, RELEASE));
        check(release(factory) == 0, 25);
        let mut factory = JUNK;
        check(f2(0, IID.as_ptr(), &mut factory) == E_NOINTERFACE && factory.is_null(), 26);

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

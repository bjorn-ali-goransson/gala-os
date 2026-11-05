//! setupapi's device information sets, as documented (setupapi.h), on
//! a machine without batteries (Chromium's battery-level provider asks
//! for them): SetupDiGetClassDevsW of GUID_DEVCLASS_BATTERY's device
//! interfaces (DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) is a valid, empty
//! set: SetupDiEnumDeviceInterfaces and SetupDiEnumDeviceInfo are FALSE
//! with ERROR_NO_MORE_ITEMS; SetupDiCreateDeviceInfoList makes an empty
//! set too; SetupDiGetClassDevsW without a class or DIGCF_ALLCLASSES is
//! ERROR_INVALID_PARAMETER; SetupDiDestroyDeviceInfoList frees a set, and
//! a freed set is ERROR_INVALID_HANDLE.
//!
//! Prints `setupapi_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// SP_DEVICE_INTERFACE_DATA.
#[repr(C)]
struct InterfaceData {
    size: u32,
    class: Guid,
    flags: u32,
    reserved: usize,
}

/// SP_DEVINFO_DATA.
#[repr(C)]
struct DevinfoData {
    size: u32,
    class: Guid,
    devinst: u32,
    reserved: usize,
}

#[link(name = "setupapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetupDiGetClassDevsW(
        class: *const Guid,
        enumerator: *const u16,
        parent: Handle,
        flags: u32,
    ) -> Handle;
    fn SetupDiCreateDeviceInfoList(class: *const Guid, parent: Handle) -> Handle;
    fn SetupDiEnumDeviceInterfaces(
        set: Handle,
        device: *mut DevinfoData,
        class: *const Guid,
        index: u32,
        data: *mut InterfaceData,
    ) -> i32;
    fn SetupDiEnumDeviceInfo(set: Handle, index: u32, data: *mut DevinfoData) -> i32;
    fn SetupDiDestroyDeviceInfoList(set: Handle) -> i32;
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

/// GUID_DEVCLASS_BATTERY {72631e54-78a4-11d0-bcf7-00aa00b7b32a}.
const BATTERY: Guid =
    Guid(0x7263_1e54, 0x78a4, 0x11d0, [0xbc, 0xf7, 0x00, 0xaa, 0x00, 0xb7, 0xb3, 0x2a]);
const DIGCF_PRESENT: u32 = 0x2;
const DIGCF_DEVICEINTERFACE: u32 = 0x10;
const INVALID: Handle = usize::MAX as Handle;
const ERROR_INVALID_HANDLE: u64 = 6;
const ERROR_INVALID_PARAMETER: u64 = 87;
const ERROR_NO_MORE_ITEMS: u64 = 259;

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

fn hex(v: u64) -> [u8; 16] {
    let mut s = [b'0'; 16];
    for (i, c) in s.iter_mut().enumerate() {
        *c = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 15) as usize];
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
        out(b"setupapi_test: check ");
        out(&hex(self.0.into()));
        out(b" failed, got 0x");
        out(&hex(got));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: setupapi calls with sets and buffers of ours.
    unsafe {
        let set = SetupDiGetClassDevsW(
            &BATTERY,
            core::ptr::null(),
            null,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        c.ok(set != INVALID && !set.is_null(), set as u64)?;
        let mut data = InterfaceData {
            size: size_of::<InterfaceData>() as u32,
            class: Guid(0, 0, 0, [0; 8]),
            flags: 0,
            reserved: 0,
        };
        SetLastError(0);
        c.eq(
            SetupDiEnumDeviceInterfaces(set, core::ptr::null_mut(), &BATTERY, 0, &mut data) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_NO_MORE_ITEMS)?;
        let mut info = DevinfoData {
            size: size_of::<DevinfoData>() as u32,
            class: Guid(0, 0, 0, [0; 8]),
            devinst: 0,
            reserved: 0,
        };
        c.eq(SetupDiEnumDeviceInfo(set, 0, &mut info) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NO_MORE_ITEMS)?;
        c.ok(SetupDiDestroyDeviceInfoList(set) != 0, GetLastError().into())?;
        SetLastError(0);
        c.eq(SetupDiDestroyDeviceInfoList(set) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE)?;

        let list = SetupDiCreateDeviceInfoList(core::ptr::null(), null);
        c.ok(list != INVALID && !list.is_null(), list as u64)?;
        c.eq(SetupDiEnumDeviceInfo(list, 0, &mut info) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NO_MORE_ITEMS)?;
        c.ok(SetupDiDestroyDeviceInfoList(list) != 0, GetLastError().into())?;

        SetLastError(0);
        let bad = SetupDiGetClassDevsW(core::ptr::null(), core::ptr::null(), null, DIGCF_PRESENT);
        c.eq(bad as u64, INVALID as u64)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"setupapi_test: ok\r\n");
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

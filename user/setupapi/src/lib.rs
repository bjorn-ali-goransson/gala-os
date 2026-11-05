//! gala's setupapi.dll: device information sets (setupapi.h) over the
//! device tree gala has, which is empty (no Plug and Play manager yet).
//! Every class has no devices, as a machine without that hardware: a set
//! from SetupDiGetClassDevsW or SetupDiCreateDeviceInfoList is valid and
//! empty, enumerating it is ERROR_NO_MORE_ITEMS, and opening a device or
//! interface by name finds none. Chromium's battery-level provider looks
//! for batteries this way. Each query of the tree is recorded once per
//! class in the ledger ("no device tree"). See user/setupapi_test.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::sync::atomic::{AtomicU32, Ordering};

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(e: u32);
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_NO_SUCH_DEVINST: u32 = 0xe000_020b;
const ERROR_NO_SUCH_DEVICE_INTERFACE: u32 = 0xe000_0225;
const INVALID_HANDLE_VALUE: usize = usize::MAX;
const DIGCF_ALLCLASSES: u32 = 0x4;

fn fail<T>(e: u32, v: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(e) };
    v
}

fn record(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// The sets: a bit per slot (live or not) and each slot's generation.
const SETS: usize = 32;
static LIVE: AtomicU32 = AtomicU32::new(0);
static GENERATIONS: [AtomicU32; SETS] = [const { AtomicU32::new(0) }; SETS];

/// A new empty set's handle: tag 0x5e7 in the low bits, the slot and its
/// generation above (never null or INVALID_HANDLE_VALUE).
fn new_set() -> usize {
    loop {
        let live = LIVE.load(Ordering::Acquire);
        let Some(slot) = (0..SETS).find(|&i| live & (1 << i) == 0) else {
            record(b"setupapi.dll!device information sets (all in use)", SETS as u64);
            return fail(ERROR_NOT_ENOUGH_MEMORY, INVALID_HANDLE_VALUE);
        };
        if LIVE
            .compare_exchange(live, live | (1 << slot), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let generation = GENERATIONS[slot].fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            return (generation as usize) << 20 | slot << 12 | 0x5e7;
        }
    }
}

/// The slot of live set `h`.
fn slot(h: usize) -> Option<usize> {
    let slot = (h >> 12) & 0xff;
    (h & 0xfff == 0x5e7
        && slot < SETS
        && LIVE.load(Ordering::Acquire) & (1 << slot) != 0
        && GENERATIONS[slot].load(Ordering::Acquire) as usize == h >> 20)
        .then_some(slot)
}

/// The first dword of a class GUID, for the ledger (0: none).
fn class_detail(class: *const u32) -> u64 {
    if class.is_null() {
        0
    } else {
        // SAFETY: the caller's GUID.
        u64::from(unsafe { class.read_unaligned() })
    }
}

/// SetupDiCreateDeviceInfoList(ClassGuid, hwndParent): an empty set.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiCreateDeviceInfoList(_class: *const u32, _parent: usize) -> usize {
    new_set()
}

/// SetupDiGetClassDevsW(ClassGuid, Enumerator, hwndParent, Flags): the
/// devices (or device interfaces) of a class, or of all with
/// DIGCF_ALLCLASSES: none on gala, an empty set.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetClassDevsW(
    class: *const u32,
    _enumerator: *const u16,
    _parent: usize,
    flags: u32,
) -> usize {
    if class.is_null() && flags & DIGCF_ALLCLASSES == 0 {
        return fail(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    record(b"setupapi.dll!SetupDiGetClassDevsW(no device tree)", class_detail(class));
    new_set()
}

/// SetupDiEnumDeviceInfo(DeviceInfoSet, MemberIndex, DeviceInfoData).
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiEnumDeviceInfo(set: usize, _index: u32, _data: *mut u8) -> i32 {
    match slot(set) {
        Some(_) => fail(ERROR_NO_MORE_ITEMS, 0),
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiEnumDeviceInterfaces(DeviceInfoSet, DeviceInfoData,
/// InterfaceClassGuid, MemberIndex, DeviceInterfaceData).
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiEnumDeviceInterfaces(
    set: usize,
    _device: *mut u8,
    _class: *const u32,
    _index: u32,
    _data: *mut u8,
) -> i32 {
    match slot(set) {
        Some(_) => fail(ERROR_NO_MORE_ITEMS, 0),
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiGetDeviceInterfaceDetailW: an empty set has no interface to
/// describe.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetDeviceInterfaceDetailW(
    set: usize,
    _interface: *mut u8,
    _detail: *mut u8,
    _size: u32,
    _required: *mut u32,
    _device: *mut u8,
) -> i32 {
    match slot(set) {
        Some(_) => fail(ERROR_NO_SUCH_DEVICE_INTERFACE, 0),
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiGetDevicePropertyW: an empty set has no device.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetDevicePropertyW(
    set: usize,
    _device: *mut u8,
    _key: *const u8,
    _kind: *mut u32,
    _buffer: *mut u8,
    _size: u32,
    _required: *mut u32,
    _flags: u32,
) -> i32 {
    match slot(set) {
        Some(_) => fail(ERROR_NO_SUCH_DEVINST, 0),
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiOpenDevRegKey: an empty set has no device.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDevRegKey(
    set: usize,
    _device: *mut u8,
    _scope: u32,
    _profile: u32,
    _kind: u32,
    _access: u32,
) -> usize {
    match slot(set) {
        Some(_) => fail(ERROR_NO_SUCH_DEVINST, INVALID_HANDLE_VALUE),
        None => fail(ERROR_INVALID_HANDLE, INVALID_HANDLE_VALUE),
    }
}

/// SetupDiOpenDeviceInfoW(DeviceInfoSet, DeviceInstanceId, ...): no
/// device has that id.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDeviceInfoW(
    set: usize,
    _id: *const u16,
    _parent: usize,
    _flags: u32,
    _data: *mut u8,
) -> i32 {
    match slot(set) {
        Some(_) => {
            record(b"setupapi.dll!SetupDiOpenDeviceInfoW(no device tree)", 0);
            fail(ERROR_NO_SUCH_DEVINST, 0)
        }
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiOpenDeviceInterfaceW(DeviceInfoSet, DevicePath, ...): no
/// interface has that path.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDeviceInterfaceW(
    set: usize,
    _path: *const u16,
    _flags: u32,
    _data: *mut u8,
) -> i32 {
    match slot(set) {
        Some(_) => {
            record(b"setupapi.dll!SetupDiOpenDeviceInterfaceW(no device tree)", 0);
            fail(ERROR_NO_SUCH_DEVICE_INTERFACE, 0)
        }
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// SetupDiDestroyDeviceInfoList(DeviceInfoSet).
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiDestroyDeviceInfoList(set: usize) -> i32 {
    match slot(set) {
        Some(s) => {
            LIVE.fetch_and(!(1 << s), Ordering::AcqRel);
            1
        }
        None => fail(ERROR_INVALID_HANDLE, 0),
    }
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

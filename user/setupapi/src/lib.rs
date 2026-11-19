//! gala's setupapi.dll: device information sets (setupapi.h) over the
//! device tree, which is in the registry where Windows keeps it
//! (`devices`, `kernel/src/nt/registry.seed`). It holds gala's display,
//! one monitor on one adapter; a class with no device there is a valid,
//! empty set, as on a machine without that hardware (Chromium's
//! battery-level provider asks for batteries this way), and that answer
//! goes to the ledger once per class.
//!
//! What the tree is for: a program that wants a monitor's EDID does not
//! ask the CCD for it. It enumerates GUID_DEVINTERFACE_MONITOR, matches
//! each interface's DevicePath against the one the CCD gave for its
//! target, opens that device's registry key and reads `EDID` out of it
//! -- DXVK's `getMonitorEdid`, and Word under it. See user/display_test
//! and user/setupapi_test.
//!
//! A set remembers only what it was asked for -- the class, the flags
//! and the enumerator -- and every call reads the tree again, so no
//! state here can go stale and nothing allocates. Two limits follow:
//! a device added to a hand-built set (SetupDiCreateDeviceInfoList,
//! then SetupDiOpenDeviceInfoW) is filled in for the caller but is not
//! a member the set enumerates, and the enumerator a set is limited to
//! is kept as sixteen ASCII bytes. Each is recorded when it bites.
//!
//! SetupIterateCabinetW (`cab`, user/setupcab_test): a cabinet's files
//! through the caller's callback, read by galapkg's cabinet reader.
//!
//! Built for x64 and x86 (ODT's 32-bit setup.exe unpacks its cabinets
//! with it).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use core::sync::atomic::{AtomicU32, Ordering};

use gala_heap as _;

// The 32-bit build takes the memory routines, 64-bit division helpers
// and unwind personality from ntrt's x86 runtime (ARCHITECTURE "Plan:
// WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

mod cab;
mod devices;

use devices::{Device, Wide};

#[cfg(not(target_arch = "x86"))]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(e: u32);
    fn ExitProcess(code: u32) -> !;
}

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(not(target_arch = "x86"))]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_FLAGS: u32 = 1004;
const ERROR_NOT_FOUND: u32 = 1168;
const ERROR_INVALID_USER_BUFFER: u32 = 1784;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_NO_SUCH_DEVINST: u32 = 0xe000_020b;
const ERROR_NO_SUCH_DEVICE_INTERFACE: u32 = 0xe000_0225;
const INVALID_HANDLE_VALUE: usize = usize::MAX;

const DIGCF_ALLCLASSES: u32 = 0x4;
const DIGCF_DEVICEINTERFACE: u32 = 0x10;

const DICS_FLAG_GLOBAL: u32 = 1;
const DIREG_DEV: u32 = 1;
const DIREG_DRV: u32 = 2;

/// SPINT_ACTIVE: an interface whose device is there.
const SPINT_ACTIVE: u32 = 1;

fn fail<T>(e: u32, v: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(e) };
    v
}

fn record(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Records an observation (the ledger's "other").
fn observed(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 5, detail) };
}

/// The sets: a bit per slot (live or not) and each slot's generation.
const SETS: usize = 32;
static LIVE: AtomicU32 = AtomicU32::new(0);
static GENERATIONS: [AtomicU32; SETS] = [const { AtomicU32::new(0) }; SETS];

/// And what each was asked for: whether it is the tree's answer to a
/// class (as against a list built by hand), the class itself, the flags,
/// and the enumerator it is limited to. One word per field per slot, so
/// a set costs no allocation and no lock.
static FROM_TREE: AtomicU32 = AtomicU32::new(0);
static HAS_CLASS: AtomicU32 = AtomicU32::new(0);
static HAS_ENUMERATOR: AtomicU32 = AtomicU32::new(0);
static CLASS_OF: [[AtomicU32; 4]; SETS] = [const { [const { AtomicU32::new(0) }; 4] }; SETS];
static FLAGS_OF: [AtomicU32; SETS] = [const { AtomicU32::new(0) }; SETS];
static ENUMERATOR_OF: [[AtomicU32; 4]; SETS] = [const { [const { AtomicU32::new(0) }; 4] }; SETS];

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

/// Sets a bit of one of the masks above.
fn mark(mask: &AtomicU32, s: usize, yes: bool) {
    if yes {
        mask.fetch_or(1 << s, Ordering::AcqRel);
    } else {
        mask.fetch_and(!(1 << s), Ordering::AcqRel);
    }
}

/// Keeps what a set was asked for.
fn remember(s: usize, class: Option<[u8; 16]>, enumerator: *const u16, flags: u32, tree: bool) {
    FLAGS_OF[s].store(flags, Ordering::Release);
    if let Some(class) = class {
        for (word, bytes) in CLASS_OF[s].iter().zip(class.chunks_exact(4)) {
            let four = <[u8; 4]>::try_from(bytes).unwrap_or_default();
            word.store(u32::from_le_bytes(four), Ordering::Release);
        }
    }
    mark(&HAS_CLASS, s, class.is_some());
    if !enumerator.is_null() {
        let bytes = enumerator_bytes(enumerator);
        for (word, four) in ENUMERATOR_OF[s].iter().zip(bytes.chunks_exact(4)) {
            let four = <[u8; 4]>::try_from(four).unwrap_or_default();
            word.store(u32::from_le_bytes(four), Ordering::Release);
        }
    }
    mark(&HAS_ENUMERATOR, s, !enumerator.is_null());
    mark(&FROM_TREE, s, tree);
}

fn class_of(s: usize) -> Option<[u8; 16]> {
    if HAS_CLASS.load(Ordering::Acquire) & (1 << s) == 0 {
        return None;
    }
    let mut class = [0u8; 16];
    for (word, bytes) in CLASS_OF[s].iter().zip(class.chunks_exact_mut(4)) {
        bytes.copy_from_slice(&word.load(Ordering::Acquire).to_le_bytes());
    }
    Some(class)
}

/// The enumerator a set is limited to, as ASCII bytes with no padding.
fn enumerator_of(s: usize) -> Option<[u8; 16]> {
    if HAS_ENUMERATOR.load(Ordering::Acquire) & (1 << s) == 0 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (word, four) in ENUMERATOR_OF[s].iter().zip(bytes.chunks_exact_mut(4)) {
        four.copy_from_slice(&word.load(Ordering::Acquire).to_le_bytes());
    }
    Some(bytes)
}

/// The caller's enumerator as up to sixteen ASCII bytes, NUL-padded. A
/// longer or non-ASCII one becomes a byte no instance id has, so such a
/// set is empty rather than wrong; it is recorded where it is taken.
fn enumerator_bytes(enumerator: *const u16) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let mut n = 0;
    loop {
        // SAFETY: the caller's NUL-ended name.
        let unit = unsafe { enumerator.add(n).read_unaligned() };
        if unit == 0 {
            return bytes;
        }
        if n == bytes.len() || unit >= 0x80 {
            return [0xff; 16];
        }
        bytes[n] = unit as u8;
        n += 1;
    }
}

/// Whether a device is below the enumerator a set is limited to.
fn under_enumerator(s: usize, device: &Device) -> bool {
    match enumerator_of(s) {
        Some(bytes) => {
            let n = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            device.instance.under(&bytes[..n])
        }
        None => true,
    }
}

/// Whether a device is one of a set's own: its class, unless the set is
/// of every class or of a device interface (where the class names the
/// interface and the devices are whichever have one).
fn in_set(s: usize, device: &Device) -> bool {
    let flags = FLAGS_OF[s].load(Ordering::Acquire);
    let by_class = flags & (DIGCF_ALLCLASSES | DIGCF_DEVICEINTERFACE) != 0
        || class_of(s).is_none_or(|class| class == device.class);
    by_class && under_enumerator(s, device)
}

/// SP_DEVICE_INTERFACE_DATA and SP_DEVINFO_DATA have the same shape -- a
/// cbSize, a GUID, a dword (Flags, or DevInst) and a pointer-wide
/// Reserved -- so one set of offsets reads and writes both.
const DATA_CB: u32 = if cfg!(target_pointer_width = "64") { 32 } else { 28 };
const DATA_GUID: usize = 4;
const DATA_WORD: usize = 20;
const DATA_RESERVED: usize = 24;
const POINTER: usize = core::mem::size_of::<usize>();

/// The caller's structure, if it is one of those two: its first dword is
/// its own size, which is how setupapi tells a caller of the wrong
/// bitness or vintage from one of its own.
fn data_in(p: *const u8) -> Option<[u8; 32]> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller's structure; its size is read first and the
    // rest only once that says the structure is as long as ours.
    unsafe {
        if p.cast::<u32>().read_unaligned() != DATA_CB {
            return None;
        }
        let mut bytes = [0u8; 32];
        core::ptr::copy_nonoverlapping(p, bytes.as_mut_ptr(), DATA_CB as usize);
        Some(bytes)
    }
}

/// Fills one, keeping its size: the GUID, the dword and Reserved.
fn data_out(p: *mut u8, guid: &[u8; 16], word: u32, reserved: usize) -> bool {
    if data_in(p).is_none() {
        return false;
    }
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&DATA_CB.to_le_bytes());
    bytes[DATA_GUID..DATA_GUID + 16].copy_from_slice(guid);
    bytes[DATA_WORD..DATA_WORD + 4].copy_from_slice(&word.to_le_bytes());
    bytes[DATA_RESERVED..DATA_RESERVED + POINTER].copy_from_slice(&reserved.to_ne_bytes());
    // SAFETY: the caller's structure, whose size data_in confirmed.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, DATA_CB as usize) };
    true
}

fn data_guid(bytes: &[u8; 32]) -> [u8; 16] {
    <[u8; 16]>::try_from(&bytes[DATA_GUID..DATA_GUID + 16]).unwrap_or_default()
}

fn data_word(bytes: &[u8; 32]) -> u32 {
    let four = <[u8; 4]>::try_from(&bytes[DATA_WORD..DATA_WORD + 4]).unwrap_or_default();
    u32::from_le_bytes(four)
}

fn data_reserved(bytes: &[u8; 32]) -> usize {
    let mut v = [0u8; POINTER];
    v.copy_from_slice(&bytes[DATA_RESERVED..DATA_RESERVED + POINTER]);
    usize::from_ne_bytes(v)
}

/// The caller's GUID, as the sixteen bytes a GUID is.
fn guid_in(p: *const u32) -> Option<[u8; 16]> {
    if p.is_null() {
        return None;
    }
    let mut guid = [0u8; 16];
    // SAFETY: the caller's GUID, read as its own bytes.
    unsafe { core::ptr::copy_nonoverlapping(p.cast::<u8>(), guid.as_mut_ptr(), 16) };
    Some(guid)
}

/// The caller's NUL-ended name, if it is no longer than the tree's own.
fn wide_in(p: *const u16) -> Option<Wide> {
    if p.is_null() {
        return None;
    }
    let mut w = Wide::new();
    for i in 0.. {
        // SAFETY: the caller's NUL-ended name.
        let unit = unsafe { p.add(i).read_unaligned() };
        if unit == 0 {
            break;
        }
        if i + 2 >= devices::MAX_WIDE {
            return None;
        }
        w.push(unit);
    }
    Some(w)
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

/// The device a filled-in SP_DEVINFO_DATA names, by the devnode id this
/// DLL gave it (`devices::Device`).
fn device_in(p: *const u8) -> Option<Device> {
    devices::by_devinst(data_word(&data_in(p)?))
}

/// SetupDiCreateDeviceInfoList(ClassGuid, hwndParent): an empty set of
/// that class, for the caller to put devices in.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiCreateDeviceInfoList(class: *const u32, _parent: usize) -> usize {
    let set = new_set();
    if let Some(s) = slot(set) {
        remember(s, guid_in(class), core::ptr::null(), 0, false);
    }
    set
}

/// SetupDiGetClassDevsW(ClassGuid, Enumerator, hwndParent, Flags): the
/// devices of a class, of every class with DIGCF_ALLCLASSES, or the
/// devices with an interface of a class with DIGCF_DEVICEINTERFACE --
/// whichever the tree has, which for a class gala has no device of is
/// none, a valid and empty set.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetClassDevsW(
    class: *const u32,
    enumerator: *const u16,
    _parent: usize,
    flags: u32,
) -> usize {
    if class.is_null() && flags & DIGCF_ALLCLASSES == 0 {
        return fail(ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let set = new_set();
    let Some(s) = slot(set) else { return set };
    remember(s, guid_in(class), enumerator, flags, true);
    if enumerator_of(s) == Some([0xff; 16]) {
        record(b"setupapi.dll!SetupDiGetClassDevsW(enumerator too long)", class_detail(class));
    }
    // Which class came back empty is worth the walk: it is the answer a
    // program that finds no hardware acted on.
    if member(s, 0).is_none() {
        observed(
            b"setupapi.dll!SetupDiGetClassDevsW: no device of that class",
            class_detail(class),
        );
    }
    set
}

/// The `index`th device of a set: the devices with an interface of its
/// class where it holds interfaces, and the devices of its class
/// otherwise.
fn member(s: usize, index: u32) -> Option<Device> {
    let flags = FLAGS_OF[s].load(Ordering::Acquire);
    if flags & DIGCF_DEVICEINTERFACE != 0 {
        let class = class_of(s)?;
        let found = devices::find_interface(&class, index, |i| under_enumerator(s, &i.device))?;
        Some(found.device)
    } else {
        devices::find_device(index, |device| in_set(s, device))
    }
}

/// SetupDiEnumDeviceInfo(DeviceInfoSet, MemberIndex, DeviceInfoData).
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiEnumDeviceInfo(set: usize, index: u32, data: *mut u8) -> i32 {
    let Some(s) = slot(set) else { return fail(ERROR_INVALID_HANDLE, 0) };
    if data_in(data).is_none() {
        return fail(ERROR_INVALID_USER_BUFFER, 0);
    }
    if FROM_TREE.load(Ordering::Acquire) & (1 << s) == 0 {
        // A hand-built set holds the devices the caller put in it, which
        // this DLL fills in for them but does not keep (see the module
        // doc): enumerating one finds nothing.
        record(b"setupapi.dll!SetupDiEnumDeviceInfo(a set built by hand)", u64::from(index));
        return fail(ERROR_NO_MORE_ITEMS, 0);
    }
    let Some(device) = member(s, index) else { return fail(ERROR_NO_MORE_ITEMS, 0) };
    if data_out(data, &device.class, device.devinst, 0) {
        1
    } else {
        fail(ERROR_INVALID_USER_BUFFER, 0)
    }
}

/// SetupDiEnumDeviceInterfaces(DeviceInfoSet, DeviceInfoData,
/// InterfaceClassGuid, MemberIndex, DeviceInterfaceData): the
/// interfaces of that class in the set, or of one device in it. The
/// interface's own place in its class goes in Reserved, which is how
/// SetupDiGetDeviceInterfaceDetailW finds it again.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiEnumDeviceInterfaces(
    set: usize,
    device: *mut u8,
    class: *const u32,
    index: u32,
    data: *mut u8,
) -> i32 {
    let Some(s) = slot(set) else { return fail(ERROR_INVALID_HANDLE, 0) };
    let (Some(class), true) = (guid_in(class), data_in(data).is_some()) else {
        return fail(ERROR_INVALID_PARAMETER, 0);
    };
    let only = if device.is_null() {
        None
    } else {
        match device_in(device) {
            Some(device) => Some(device.devinst),
            None => return fail(ERROR_INVALID_PARAMETER, 0),
        }
    };
    let found = devices::find_interface(&class, index, |i| {
        under_enumerator(s, &i.device) && only.is_none_or(|d| d == i.device.devinst)
    });
    let Some(found) = found else { return fail(ERROR_NO_MORE_ITEMS, 0) };
    if data_out(data, &class, SPINT_ACTIVE, found.nth as usize) {
        1
    } else {
        fail(ERROR_INVALID_USER_BUFFER, 0)
    }
}

/// SP_DEVICE_INTERFACE_DETAIL_DATA_W: a cbSize that is its own size,
/// then the path. The size a caller must give is the offset of the path
/// plus the path and its terminator, which is not cbSize -- the two
/// differ by the structure's own padding, and a caller that confuses
/// them is the classic setupapi bug.
const DETAIL_CB: u32 = if cfg!(target_pointer_width = "64") { 8 } else { 6 };
const DETAIL_PATH: u32 = 4;

/// SetupDiGetDeviceInterfaceDetailW(DeviceInfoSet, DeviceInterfaceData,
/// DeviceInterfaceDetailData, DeviceInterfaceDetailDataSize,
/// RequiredSize, DeviceInfoData): the interface's device path, and its
/// device. No buffer at all asks for the size.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetDeviceInterfaceDetailW(
    set: usize,
    interface: *mut u8,
    detail: *mut u8,
    size: u32,
    required: *mut u32,
    device: *mut u8,
) -> i32 {
    if slot(set).is_none() {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    let Some(data) = data_in(interface) else { return fail(ERROR_INVALID_PARAMETER, 0) };
    let class = data_guid(&data);
    let nth = data_reserved(&data) as u32;
    let found = devices::find_interface(&class, 0, |i| i.nth == nth);
    let Some(found) = found else { return fail(ERROR_NO_SUCH_DEVICE_INTERFACE, 0) };
    let need = DETAIL_PATH + 2 * (found.path.len() as u32 + 1);
    if !required.is_null() {
        // SAFETY: the caller's one dword.
        unsafe { required.write_unaligned(need) };
    }
    if detail.is_null() {
        // The size on its own is asked for with no buffer and no size;
        // a size with no buffer is the caller's mistake.
        return if size == 0 {
            fail(ERROR_INSUFFICIENT_BUFFER, 0)
        } else {
            fail(ERROR_INVALID_USER_BUFFER, 0)
        };
    }
    // SAFETY: the caller's structure, of which the first dword is its
    // own size (and must be, before anything is written).
    if unsafe { detail.cast::<u32>().read_unaligned() } != DETAIL_CB {
        return fail(ERROR_INVALID_USER_BUFFER, 0);
    }
    if size < need {
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    for (i, &unit) in found.path.units().iter().chain(&[0]).enumerate() {
        // SAFETY: the path and its terminator, inside the size the
        // caller gave, which is at least what the path needs.
        unsafe { detail.add(DETAIL_PATH as usize + 2 * i).cast::<u16>().write_unaligned(unit) };
    }
    if !device.is_null() && !data_out(device, &found.device.class, found.device.devinst, 0) {
        return fail(ERROR_INVALID_USER_BUFFER, 0);
    }
    1
}

/// SetupDiGetDevicePropertyW(DeviceInfoSet, DeviceInfoData, PropertyKey,
/// PropertyType, PropertyBuffer, PropertyBufferSize, RequiredSize,
/// Flags): the tree's devices have the values their keys hold, not the
/// unified device properties, so a device that is there has no property
/// -- which the ledger records, with the property key's first dword.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiGetDevicePropertyW(
    set: usize,
    device: *mut u8,
    key: *const u8,
    _kind: *mut u32,
    _buffer: *mut u8,
    _size: u32,
    _required: *mut u32,
    _flags: u32,
) -> i32 {
    if slot(set).is_none() {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if device_in(device).is_none() {
        return fail(ERROR_NO_SUCH_DEVINST, 0);
    }
    record(b"setupapi.dll!SetupDiGetDevicePropertyW", class_detail(key.cast()));
    fail(ERROR_NOT_FOUND, 0)
}

/// SetupDiOpenDevRegKey(DeviceInfoSet, DeviceInfoData, Scope, Profile,
/// KeyType, samDesired): the device's own key (DIREG_DEV, its `Device
/// Parameters`) or its driver's (DIREG_DRV), which is where a monitor's
/// EDID is. The caller closes it with RegCloseKey, since it is an NT key
/// handle like every other advapi32 gives out.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDevRegKey(
    set: usize,
    device: *mut u8,
    scope: u32,
    _profile: u32,
    kind: u32,
    access: u32,
) -> usize {
    if slot(set).is_none() {
        return fail(ERROR_INVALID_HANDLE, INVALID_HANDLE_VALUE);
    }
    if kind != DIREG_DEV && kind != DIREG_DRV {
        return fail(ERROR_INVALID_FLAGS, INVALID_HANDLE_VALUE);
    }
    if scope != DICS_FLAG_GLOBAL {
        // gala has one hardware profile, so a profile-specific key is
        // the same key; that it was asked for is worth recording.
        observed(b"setupapi.dll!SetupDiOpenDevRegKey(one hardware profile)", u64::from(scope));
    }
    let Some(device) = device_in(device) else {
        return fail(ERROR_NO_SUCH_DEVINST, INVALID_HANDLE_VALUE);
    };
    match devices::device_key(&device, kind == DIREG_DRV, access) {
        Some(key) => key,
        None => fail(ERROR_FILE_NOT_FOUND, INVALID_HANDLE_VALUE),
    }
}

/// SetupDiOpenDeviceInfoW(DeviceInfoSet, DeviceInstanceId, hwndParent,
/// OpenFlags, DeviceInfoData): the device with that instance id.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDeviceInfoW(
    set: usize,
    id: *const u16,
    _parent: usize,
    _flags: u32,
    data: *mut u8,
) -> i32 {
    if slot(set).is_none() {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    let Some(id) = wide_in(id) else { return fail(ERROR_INVALID_PARAMETER, 0) };
    let Some(device) = devices::by_instance(&id) else {
        record(b"setupapi.dll!SetupDiOpenDeviceInfoW(no such device)", u64::from(id.len() as u32));
        return fail(ERROR_NO_SUCH_DEVINST, 0);
    };
    // The caller may want nothing but to know it is there.
    if !data.is_null() && !data_out(data, &device.class, device.devinst, 0) {
        return fail(ERROR_INVALID_USER_BUFFER, 0);
    }
    1
}

/// SetupDiOpenDeviceInterfaceW(DeviceInfoSet, DevicePath, OpenFlags,
/// DeviceInterfaceData): the interface with that device path, whose
/// class is the path's own last field.
#[unsafe(no_mangle)]
pub extern "system" fn SetupDiOpenDeviceInterfaceW(
    set: usize,
    path: *const u16,
    _flags: u32,
    data: *mut u8,
) -> i32 {
    if slot(set).is_none() {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    let Some(path) = wide_in(path) else { return fail(ERROR_INVALID_PARAMETER, 0) };
    let Some(found) = devices::by_path(&path) else {
        record(
            b"setupapi.dll!SetupDiOpenDeviceInterfaceW(no such interface)",
            u64::from(path.len() as u32),
        );
        return fail(ERROR_NO_SUCH_DEVICE_INTERFACE, 0);
    };
    if !data.is_null() && !data_out(data, &found.class, SPINT_ACTIVE, found.nth as usize) {
        return fail(ERROR_INVALID_USER_BUFFER, 0);
    }
    1
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

/// A panic is a bug (or the heap ran out inside the cabinet reader): its
/// location goes to the ledger (`panic <file>`, the line as detail) and
/// the process ends with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as
/// gala's other DLLs do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    observed(&text[..n], line.into());
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}

//! gala's cfgmgr32.dll (the PnP configuration manager's API,
//! cfgmgr32.h): the device lists a program asks for, the devnodes their
//! ids name, and the properties those devnodes have. The tree is the
//! registry's, read by `gala_pnp` -- the same tree gala's setupapi.dll
//! answers device information sets out of, as Windows' setupapi.dll
//! answers both through cfgmgr32.dll, and a DEVINST here is the devnode
//! id a device has there.
//!
//! Who asks: the Khronos Vulkan loader lists the display adapters'
//! devices for their Vulkan ICDs before it reads
//! `HKLM\SOFTWARE\Khronos\Vulkan\Drivers` (where gala registers
//! lavapipe; ARCHITECTURE "Software 3D (G2)"), and Word asks for the
//! present devices of a class sixteen times before it draws anything.
//!
//! Every device the tree describes is present (`gala_pnp`), so a
//! present-only list is the whole list. The three filters the registry
//! answers are honoured -- the class, the enumerator and the service --
//! and the relation filters (eject, removal, power, bus, transport) ask
//! for the tree's shape, which `Enum` does not record: those answer an
//! empty list and say so in the ledger, as do CM_Get_Child and
//! CM_Get_Sibling (ARCHITECTURE "setupapi").
//!
//! Observability: every list asked for leaves a row naming what was
//! asked -- the class by its GUID, or the filter by name -- with the
//! number of devices answered as its detail, so a program looking for
//! hardware gala has none of shows up with the class it wanted. The
//! lot is measured against Windows (`user/cfgmgr32_test`).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use gala_pnp::{Device, ROOT_DEVINST, Wide};

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

// The x64 build's memory routines come from gala's ntdll (as the other
// DLLs that walk the tree take them).
#[cfg(not(target_arch = "x86"))]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(not(target_arch = "x86"))]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// CONFIGRET values (cfgmgr32.h).
const CR_SUCCESS: u32 = 0x00;
const CR_INVALID_POINTER: u32 = 0x03;
const CR_INVALID_FLAG: u32 = 0x04;
const CR_INVALID_DEVNODE: u32 = 0x05;
const CR_NO_SUCH_DEVNODE: u32 = 0x0d;
const CR_BUFFER_SMALL: u32 = 0x1a;
const CR_INVALID_DATA: u32 = 0x1f;
const CR_NO_SUCH_VALUE: u32 = 0x25;
const CR_NO_SUCH_REGISTRY_KEY: u32 = 0x2e;

/// What the registry says about a value asked for (winerror.h), as
/// `gala_pnp::query_value` passes it back.
const ERROR_SUCCESS: i32 = 0;
const ERROR_MORE_DATA: i32 = 234;

/// CM_GETIDLIST_FILTER_* (cfgmgr32.h).
const FILTER_ENUMERATOR: u32 = 0x001;
const FILTER_SERVICE: u32 = 0x002;
const FILTER_PRESENT: u32 = 0x100;
const FILTER_CLASS: u32 = 0x200;
/// The filters that ask for the tree's shape: eject, removal, power,
/// bus and transport relations.
const FILTER_RELATIONS: u32 = 0x004 | 0x008 | 0x010 | 0x020 | 0x080;
/// CM_GETIDLIST_DONOTGENERATE: the list need not be made afresh, which
/// no list here is -- every one of them is read out of the registry when
/// it is asked for.
const DONOTGENERATE: u32 = 0x1000_0040;
/// Every flag the call takes (CM_GETIDLIST_FILTER_BITS): one that is not
/// among them is CR_INVALID_FLAG, as Windows answers.
const FILTER_BITS: u32 = FILTER_ENUMERATOR
    | FILTER_SERVICE
    | FILTER_RELATIONS
    | FILTER_PRESENT
    | FILTER_CLASS
    | DONOTGENERATE;

/// DN_DRIVER_LOADED | DN_STARTED (cfg.h): what a device of gala's own
/// drivers is, the tree describing none that is not.
const STARTED: u32 = 0x02 | 0x08;

/// CM_REGISTRY_SOFTWARE (cfgmgr32.h): the driver's settings rather than
/// the device's own hardware key.
const CM_REGISTRY_SOFTWARE: u32 = 0x01;

/// A gap: something gala cannot answer.
fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Not a gap: a call that was answered, recorded because the answer is
/// the only thing that says the program asked.
fn reached(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "reached" (9).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 9, detail) };
}

/// The caller's NUL-ended wide string, or none if it gave none.
fn wide(text: *const u16) -> Option<Wide> {
    if text.is_null() {
        return None;
    }
    let mut w = Wide::new();
    for i in 0..gala_pnp::MAX_WIDE {
        // SAFETY: the caller says this is a NUL-ended string; it is read
        // only up to its NUL, and no further than a name the tree could
        // hold.
        let unit = unsafe { *text.add(i) };
        if unit == 0 {
            break;
        }
        w.push(unit);
    }
    Some(w)
}

/// Which devices a `CM_Get_Device_ID_List` call asks for.
enum Filter {
    /// Every device in the tree: CM_GETIDLIST_FILTER_NONE, and
    /// CM_GETIDLIST_FILTER_PRESENT, every device here being present.
    Every,
    /// The devices of a class.
    Class([u8; 16]),
    /// The devices under an enumerator: `ROOT`, or `PCI\VEN_8086&DEV_1234`.
    Under(Wide),
    /// The devices a service drives.
    Service(Wide),
    /// A filter the tree cannot answer: no device.
    Nothing,
}

impl Filter {
    /// The filter a call's flags and string make, or the CONFIGRET that
    /// says why they make none. The order is Windows': a flag that is no
    /// filter is refused before the string is looked at, a class filter
    /// wants a string, and a string that should be a GUID and is not is
    /// CR_INVALID_DATA (all three measured, `user/cfgmgr32_test`).
    fn of(filter: *const u16, flags: u32) -> Result<Self, u32> {
        if flags & !FILTER_BITS != 0 {
            missing(b"cfgmgr32.dll!CM_Get_Device_ID_List(flags)", flags.into());
            return Err(CR_INVALID_FLAG);
        }
        if flags & FILTER_RELATIONS != 0 {
            missing(b"cfgmgr32.dll!CM_Get_Device_ID_List(relations)", flags.into());
            return Ok(Filter::Nothing);
        }
        if flags & FILTER_CLASS != 0 {
            let text = wide(filter).ok_or(CR_INVALID_POINTER)?;
            let class = gala_pnp::guid_bytes(text.units()).ok_or(CR_INVALID_DATA)?;
            return Ok(Filter::Class(class));
        }
        if flags & FILTER_ENUMERATOR != 0 {
            return Ok(Filter::Under(wide(filter).ok_or(CR_INVALID_POINTER)?));
        }
        if flags & FILTER_SERVICE != 0 {
            return Ok(Filter::Service(wide(filter).ok_or(CR_INVALID_POINTER)?));
        }
        Ok(Filter::Every)
    }

    /// Whether this device is one of them.
    fn keeps(&self, device: &Device) -> bool {
        match self {
            Filter::Every => true,
            Filter::Class(class) => &device.class == class,
            // The filter may be the whole of the first component or the
            // first two of them, and a device is also under its own id.
            Filter::Under(text) => {
                device.instance.under_wide(text.units()) || device.instance.same(text)
            }
            Filter::Service(name) => gala_pnp::instance_key(device).is_some_and(|key| {
                let service = gala_pnp::string_value(key, b"Service");
                gala_pnp::close(key);
                service.is_some_and(|s| s.same(name))
            }),
            Filter::Nothing => false,
        }
    }

    /// The row that says this list was asked for. A class is named by
    /// its GUID, which is what says which hardware a program went
    /// looking for; the rest are named by the filter they are.
    fn note(&self, answered: u64) {
        const PREFIX: &[u8] = b"cfgmgr32.dll!CM_Get_Device_ID_List(";
        let mut name = [0u8; 96];
        name[..PREFIX.len()].copy_from_slice(PREFIX);
        let mut n = PREFIX.len();
        let mut put = |text: &[u8]| {
            let room = name.len() - 1 - n;
            let text = &text[..text.len().min(room)];
            name[n..n + text.len()].copy_from_slice(text);
            n += text.len();
        };
        match self {
            Filter::Every => put(b"all"),
            Filter::Class(class) => {
                let text = gala_pnp::guid_text(class);
                let mut ascii = [0u8; 40];
                for (to, &unit) in ascii.iter_mut().zip(text.units()) {
                    *to = u8::try_from(unit).unwrap_or(b'?');
                }
                put(&ascii[..text.len()]);
            }
            Filter::Under(_) => put(b"enumerator"),
            Filter::Service(_) => put(b"service"),
            // Its own row named the flags that could not be answered.
            Filter::Nothing => return,
        }
        name[n] = b')';
        reached(&name[..n + 1], answered);
    }
}

/// How many units a list of the devices a filter keeps takes, and how
/// many devices that is: every id and its NUL, and the NUL that ends the
/// list -- so an empty list is one unit, as Windows answers.
fn list_size(filter: &Filter) -> (u32, u64) {
    let mut units = 1u32;
    let mut devices = 0u64;
    gala_pnp::walk(|device| {
        if filter.keeps(device) {
            units += u32::try_from(device.instance.len()).unwrap_or(0) + 1;
            devices += 1;
        }
        true
    });
    (units, devices)
}

/// CM_Get_Device_ID_List_SizeW(pulLen, pszFilter, ulFlags): how many
/// units the list would take, which is what a caller sizes its buffer
/// by.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_ID_List_SizeW(
    len: *mut u32,
    filter: *const u16,
    flags: u32,
) -> u32 {
    if len.is_null() {
        return CR_INVALID_POINTER;
    }
    let filter = match Filter::of(filter, flags) {
        Ok(filter) => filter,
        Err(bad) => return bad,
    };
    let (units, devices) = list_size(&filter);
    filter.note(devices);
    // SAFETY: the caller's output.
    unsafe { *len = units };
    CR_SUCCESS
}

/// CM_Get_Device_ID_ListW(pszFilter, Buffer, BufferLen, ulFlags): the
/// ids themselves, one after another, each ended by a NUL and the list
/// by one more. A buffer that cannot hold them all is CR_BUFFER_SMALL
/// with nothing written past its end, which is what a caller whose size
/// went stale between the two calls is told.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_ID_ListW(
    filter: *const u16,
    buffer: *mut u16,
    len: u32,
    flags: u32,
) -> u32 {
    if buffer.is_null() {
        return CR_INVALID_POINTER;
    }
    let filter = match Filter::of(filter, flags) {
        Ok(filter) => filter,
        Err(bad) => return bad,
    };
    let room = len as usize;
    if room < 1 {
        filter.note(0);
        return CR_BUFFER_SMALL;
    }
    let mut at = 0usize;
    let mut written = 0u64;
    let mut short = false;
    gala_pnp::walk(|device| {
        if !filter.keeps(device) {
            return true;
        }
        let id = device.instance.units();
        // The id, its NUL, and room left for the one that ends the list.
        if at + id.len() + 2 > room {
            short = true;
            return false;
        }
        for &unit in id {
            // SAFETY: inside the caller's buffer of `len` units, the
            // room for this id and its NUL having been checked.
            unsafe { *buffer.add(at) = unit };
            at += 1;
        }
        // SAFETY: as above.
        unsafe { *buffer.add(at) = 0 };
        at += 1;
        written += 1;
        true
    });
    // A caller that asked for the size first hits this row twice, which
    // is what it did; one that did not is here all the same.
    filter.note(written);
    if short {
        return CR_BUFFER_SMALL;
    }
    // SAFETY: at most `room - 1`, the loop having kept a unit for this.
    unsafe { *buffer.add(at) = 0 };
    CR_SUCCESS
}

/// CM_Locate_DevNodeW(pdnDevInst, pDeviceID, ulFlags): the devnode an
/// instance id names. NULL (or "") is the root of the tree.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Locate_DevNodeW(dev: *mut u32, id: *const u16, _flags: u32) -> u32 {
    if dev.is_null() {
        return CR_INVALID_POINTER;
    }
    let Some(id) = wide(id).filter(|id| !id.is_empty()) else {
        // SAFETY: the caller's output.
        unsafe { *dev = ROOT_DEVINST };
        return CR_SUCCESS;
    };
    let Some(device) = gala_pnp::by_instance(&id) else {
        return CR_NO_SUCH_DEVNODE;
    };
    // SAFETY: the caller's output.
    unsafe { *dev = device.devinst };
    CR_SUCCESS
}

/// CM_Get_DevNode_Status(pulStatus, pulProblemNumber, dnDevInst,
/// ulFlags): a device of gala's own drivers is started, with no
/// problem, and so is the root.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_DevNode_Status(
    status: *mut u32,
    problem: *mut u32,
    dev: u32,
    _flags: u32,
) -> u32 {
    if status.is_null() || problem.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT_DEVINST && gala_pnp::by_devinst(dev).is_none() {
        return CR_INVALID_DEVNODE;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *status = STARTED;
        *problem = 0;
    }
    CR_SUCCESS
}

/// CM_Get_Child / CM_Get_Sibling: the shape of the tree, which is the
/// one thing the registry does not keep -- `Enum`'s three levels are
/// the enumerator, the hardware id and the instance, not a parent and
/// its children. A program that walks the tree this way instead of
/// asking for a list finds nothing and is in the ledger
/// (ARCHITECTURE "setupapi").
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Child(child: *mut u32, dev: u32, _flags: u32) -> u32 {
    if child.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT_DEVINST && gala_pnp::by_devinst(dev).is_none() {
        return CR_INVALID_DEVNODE;
    }
    missing(b"cfgmgr32.dll!CM_Get_Child(no tree shape)", dev.into());
    CR_NO_SUCH_DEVNODE
}

#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Sibling(sibling: *mut u32, dev: u32, _flags: u32) -> u32 {
    if sibling.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT_DEVINST && gala_pnp::by_devinst(dev).is_none() {
        return CR_INVALID_DEVNODE;
    }
    missing(b"cfgmgr32.dll!CM_Get_Sibling(no tree shape)", dev.into());
    CR_NO_SUCH_DEVNODE
}

/// CM_Get_Device_IDW(dnDevInst, Buffer, BufferLen, ulFlags): the
/// devnode's instance id -- the root's being `HTREE\ROOT\0`, as Windows
/// answers.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_IDW(dev: u32, buffer: *mut u16, len: u32, _flags: u32) -> u32 {
    if buffer.is_null() {
        return CR_INVALID_POINTER;
    }
    let id = if dev == ROOT_DEVINST {
        Wide::of(b"HTREE\\ROOT\\0")
    } else {
        match gala_pnp::by_devinst(dev) {
            Some(device) => device.instance,
            None => return CR_INVALID_DEVNODE,
        }
    };
    if (len as usize) < id.len() + 1 {
        return CR_BUFFER_SMALL;
    }
    for (i, &unit) in id.units().iter().enumerate() {
        // SAFETY: inside the caller's buffer of `len` units.
        unsafe { *buffer.add(i) = unit };
    }
    // SAFETY: as above.
    unsafe { *buffer.add(id.len()) = 0 };
    CR_SUCCESS
}

/// CM_Get_DevNode_Registry_PropertyW(dnDevInst, ulProperty,
/// pulRegDataType, Buffer, pulLength, ulFlags): a device's property,
/// which is a value of its own key (`gala_pnp::property_value`). The
/// root has none: it is not a device and has no key.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_DevNode_Registry_PropertyW(
    dev: u32,
    property: u32,
    kind: *mut u32,
    buffer: *mut u8,
    len: *mut u32,
    _flags: u32,
) -> u32 {
    if len.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev == ROOT_DEVINST {
        missing(b"cfgmgr32.dll!CM_Get_DevNode_Registry_PropertyW(root)", property.into());
        return CR_NO_SUCH_VALUE;
    }
    let Some(device) = gala_pnp::by_devinst(dev) else {
        return CR_INVALID_DEVNODE;
    };
    let Some(value) = gala_pnp::property_value(property) else {
        missing(b"cfgmgr32.dll!CM_Get_DevNode_Registry_PropertyW(property)", property.into());
        return CR_NO_SUCH_VALUE;
    };
    let Some(key) = gala_pnp::instance_key(&device) else {
        return CR_NO_SUCH_REGISTRY_KEY;
    };
    // SAFETY: the caller's outputs, as its own API gave them: `len`
    // holds the room in `buffer` and is written with what was used, and
    // `kind` and `buffer` may be null (a caller asking for the size).
    let r = unsafe { gala_pnp::query_value(key, value, kind, buffer, len) };
    gala_pnp::close(key);
    match r {
        ERROR_SUCCESS => CR_SUCCESS,
        ERROR_MORE_DATA => CR_BUFFER_SMALL,
        _ => CR_NO_SUCH_VALUE,
    }
}

/// CM_Open_DevNode_Key(dnDevNode, samDesired, ulHardwareProfile,
/// Disposition, phkDevice, ulFlags): a device's `Device Parameters`
/// (CM_REGISTRY_HARDWARE), or its driver's settings under
/// `Control\Class` (CM_REGISTRY_SOFTWARE). The root is not a device and
/// has neither. A key the tree does not hold is not created here,
/// whatever the disposition asks: the device's own INF would have made
/// it, and gala's devices have none.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Open_DevNode_Key(
    dev: u32,
    access: u32,
    _profile: u32,
    _disposition: u32,
    key: *mut usize,
    flags: u32,
) -> u32 {
    if key.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev == ROOT_DEVINST {
        return CR_NO_SUCH_REGISTRY_KEY;
    }
    let Some(device) = gala_pnp::by_devinst(dev) else {
        return CR_INVALID_DEVNODE;
    };
    match gala_pnp::device_key(&device, flags & CM_REGISTRY_SOFTWARE != 0, access) {
        Some(opened) => {
            // SAFETY: the caller's output.
            unsafe { *key = opened };
            CR_SUCCESS
        }
        None => CR_NO_SUCH_REGISTRY_KEY,
    }
}

/// The DLL entry point: nothing to set up.
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

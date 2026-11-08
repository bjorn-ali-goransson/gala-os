//! gala's cfgmgr32.dll (the PnP configuration manager's API, cfgmgr32.h).
//! gala has no Plug and Play device tree: its drivers are its own, not
//! devnodes. The tree is its root alone (`HTREE\ROOT\0`, started, no
//! children), and device lists are empty: the Khronos Vulkan loader,
//! which lists the display adapters for their drivers' Vulkan ICDs, finds
//! none and reads `HKLM\SOFTWARE\Khronos\Vulkan\Drivers` instead (where
//! gala registers lavapipe; ARCHITECTURE "Software 3D (G2)"). A list
//! asked for is in the ledger (`cfgmgr32.dll!CM_Get_Device_ID_List(no
//! device tree)`), so programs that look for hardware show up there.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
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
const CR_INVALID_DEVNODE: u32 = 0x05;
const CR_NO_SUCH_DEVNODE: u32 = 0x0d;
const CR_BUFFER_SMALL: u32 = 0x1a;
const CR_NO_SUCH_VALUE: u32 = 0x25;
const CR_NO_SUCH_REGISTRY_KEY: u32 = 0x2e;

/// The root devnode's handle (DEVINST), the only one there is.
const ROOT: u32 = 1;
/// DN_DRIVER_LOADED | DN_STARTED (cfg.h).
const ROOT_STATUS: u32 = 0x02 | 0x08;

fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// CM_Get_Device_ID_List_SizeW(pulLen, pszFilter, ulFlags): an empty
/// list's size, 1 (its final NUL).
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_ID_List_SizeW(
    len: *mut u32,
    _filter: *const u16,
    flags: u32,
) -> u32 {
    if len.is_null() {
        return CR_INVALID_POINTER;
    }
    missing(b"cfgmgr32.dll!CM_Get_Device_ID_List(no device tree)", flags.into());
    // SAFETY: the caller's output.
    unsafe { *len = 1 };
    CR_SUCCESS
}

/// CM_Get_Device_ID_ListW(pszFilter, Buffer, BufferLen, ulFlags): the
/// empty list (a single NUL).
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_ID_ListW(
    _filter: *const u16,
    buffer: *mut u16,
    len: u32,
    _flags: u32,
) -> u32 {
    if buffer.is_null() {
        return CR_INVALID_POINTER;
    }
    if len < 1 {
        return CR_BUFFER_SMALL;
    }
    // SAFETY: the caller's buffer of `len` (at least 1) characters.
    unsafe { *buffer = 0 };
    CR_SUCCESS
}

/// CM_Locate_DevNodeW(pdnDevInst, pDeviceID, ulFlags): NULL (or "") is
/// the root; no other device exists.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Locate_DevNodeW(dev: *mut u32, id: *const u16, _flags: u32) -> u32 {
    if dev.is_null() {
        return CR_INVALID_POINTER;
    }
    // SAFETY: the caller's NUL-terminated string, when given.
    if id.is_null() || unsafe { *id } == 0 {
        // SAFETY: the caller's output.
        unsafe { *dev = ROOT };
        return CR_SUCCESS;
    }
    CR_NO_SUCH_DEVNODE
}

/// CM_Get_DevNode_Status(pulStatus, pulProblemNumber, dnDevInst,
/// ulFlags): the root is started, with no problem.
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
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *status = ROOT_STATUS;
        *problem = 0;
    }
    CR_SUCCESS
}

/// CM_Get_Child / CM_Get_Sibling: the root has no children.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Child(child: *mut u32, dev: u32, _flags: u32) -> u32 {
    if child.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    CR_NO_SUCH_DEVNODE
}

#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Sibling(sibling: *mut u32, dev: u32, _flags: u32) -> u32 {
    if sibling.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    CR_NO_SUCH_DEVNODE
}

/// CM_Get_Device_IDW(dnDevInst, Buffer, BufferLen, ulFlags): the root's
/// instance ID, `HTREE\ROOT\0`.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_Device_IDW(dev: u32, buffer: *mut u16, len: u32, _flags: u32) -> u32 {
    const ID: &[u8] = b"HTREE\\ROOT\\0";
    if buffer.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    if (len as usize) < ID.len() + 1 {
        return CR_BUFFER_SMALL;
    }
    for (i, &b) in ID.iter().enumerate() {
        // SAFETY: inside the caller's buffer of `len` characters.
        unsafe { *buffer.add(i) = u16::from(b) };
    }
    // SAFETY: as above.
    unsafe { *buffer.add(ID.len()) = 0 };
    CR_SUCCESS
}

/// CM_Get_DevNode_Registry_PropertyW: the root has no properties gala
/// keeps.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Get_DevNode_Registry_PropertyW(
    dev: u32,
    property: u32,
    _kind: *mut u32,
    _buffer: *mut u8,
    _len: *mut u32,
    _flags: u32,
) -> u32 {
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    missing(b"cfgmgr32.dll!CM_Get_DevNode_Registry_PropertyW(root)", property.into());
    CR_NO_SUCH_VALUE
}

/// CM_Open_DevNode_Key: the root has no hardware or software key.
#[unsafe(no_mangle)]
pub extern "system" fn CM_Open_DevNode_Key(
    dev: u32,
    _access: u32,
    _profile: u32,
    _disposition: u32,
    key: *mut usize,
    _flags: u32,
) -> u32 {
    if key.is_null() {
        return CR_INVALID_POINTER;
    }
    if dev != ROOT {
        return CR_INVALID_DEVNODE;
    }
    CR_NO_SUCH_REGISTRY_KEY
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

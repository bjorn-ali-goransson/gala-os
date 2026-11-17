//! The firmware's tables: GetSystemFirmwareTable, which Office's shared
//! code (mso) asks the SMBIOS provider for the machine it runs on. The
//! table comes from NtQuerySystemInformation's
//! SystemFirmwareTableInformation, where the one buffer carries the
//! request in and the table out (ARCHITECTURE "Firmware tables").

use super::*;

/// SYSTEM_FIRMWARE_TABLE_INFORMATION: the provider's signature, the
/// action, the table's id and the length of the buffer after it, which
/// comes back as the length the table needs.
const HEADER: usize = 16;
const SYSTEM_FIRMWARE_TABLE_INFORMATION: u32 = 76;
/// SystemFirmwareTableGet.
const ACTION_GET: u32 = 1;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;

unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

/// GetSystemFirmwareTable(provider, id, buffer, size): a firmware table,
/// as much of it as fits, and the size it needs either way — which is how
/// a caller asks for the size first (a null buffer and 0). A provider the
/// machine has not got answers 0 with ERROR_INVALID_FUNCTION, and so does
/// a table it has not got (`office_k32_test`'s `firmware` measured both
/// on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemFirmwareTable(
    provider: u32,
    id: u32,
    buffer: *mut u8,
    size: u32,
) -> u32 {
    const MEM_COMMIT_RESERVE: u32 = 0x3000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 4;
    const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
    // The request and the table share one buffer, so it holds the header
    // and whatever room the caller has.
    let total = HEADER + size as usize;
    let info =
        VirtualAlloc(core::ptr::null_mut(), total, MEM_COMMIT_RESERVE, PAGE_READWRITE).cast::<u8>();
    if info.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, 0);
    }
    // SAFETY: our own buffer of `total` bytes, the header first.
    let (status, needed) = unsafe {
        for (at, value) in [(0, provider), (4, ACTION_GET), (8, id), (12, size)] {
            core::ptr::write_unaligned(info.add(at).cast::<u32>(), value);
        }
        let status = NtQuerySystemInformation(
            SYSTEM_FIRMWARE_TABLE_INFORMATION,
            info,
            total as u32,
            core::ptr::null_mut(),
        );
        // The length the table needs, which the call writes back whether
        // it fitted or not.
        (status, core::ptr::read_unaligned(info.add(12).cast::<u32>()))
    };
    if status == STATUS_SUCCESS && !buffer.is_null() && size != 0 {
        // SAFETY: the table the call wrote after the header, and the
        // caller's buffer, which has room for `size` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(info.add(HEADER), buffer, needed.min(size) as usize)
        };
    }
    VirtualFree(info.cast(), 0, MEM_RELEASE);
    match status {
        STATUS_SUCCESS => needed,
        // Too small is not a failure: the size is the answer.
        STATUS_BUFFER_TOO_SMALL => needed,
        _ => fail(status, 0),
    }
}

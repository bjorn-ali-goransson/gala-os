//! The display configuration (CCD, "Connecting and Configuring
//! Displays"): GetDisplayConfigBufferSizes, QueryDisplayConfig and
//! DisplayConfigGetDeviceInfo.
//!
//! gala has one display adapter with one source and one target, and that
//! path is always active, so the configuration is one path and two mode
//! records: the target's video signal and the source's desktop surface.
//! The numbers are the ones every other door answers with (`monitor`'s
//! screen, 32 bits per pixel and 60 Hz, as EnumDisplaySettingsW says), so
//! a program told the size twice is told the same size.
//!
//! What a real display has and gala's has not is an EDID: no manufacturer
//! or product code to report, and no name of the panel's own. So the
//! target's name is gala's (`friendlyNameForced`, which is what Windows
//! sets for a panel whose EDID carries no name, with the EDID ids left
//! zero and `edidIdsValid` clear) and its device path is a
//! device-interface path of gala's own making under the two public
//! interface classes (monitor, display adapter).
//!
//! Three things are deliberately not here, and each records a ledger row
//! rather than a wrong answer: a request about an adapter that is not
//! gala's one (ERROR_INVALID_PARAMETER, as Windows answers for an adapter
//! it does not know), an information type other than the three names
//! (ERROR_NOT_SUPPORTED), and DisplayConfigSetDeviceInfo, which would
//! change a configuration gala cannot change.

use core::ptr;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// ERROR_SUCCESS, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED and
/// ERROR_INSUFFICIENT_BUFFER: these functions answer with a Win32 error
/// and not with a handle or a flag, so they return it.
const OK: i32 = 0;
const INVALID_PARAMETER: i32 = 87;
const NOT_SUPPORTED: i32 = 50;
const INSUFFICIENT_BUFFER: i32 = 122;
/// What Windows answers DisplayConfigGetDeviceInfo with when there is no
/// packet at all (measured): the one place in these four calls where a
/// null pointer is not ERROR_INVALID_PARAMETER.
const GEN_FAILURE: i32 = 31;

/// The topologies QDC_* names, one of which every call must ask for.
const QDC_ALL_PATHS: u32 = 1;
const QDC_ONLY_ACTIVE_PATHS: u32 = 2;
const QDC_DATABASE_CURRENT: u32 = 4;
const QDC_TOPOLOGY: u32 = QDC_ALL_PATHS | QDC_ONLY_ACTIVE_PATHS | QDC_DATABASE_CURRENT;

/// DISPLAYCONFIG_TOPOLOGY_INTERNAL: the desktop is on the built-in
/// display, which is the only display gala has.
const TOPOLOGY_INTERNAL: u32 = 1;

/// gala's one display adapter, source and target. A LUID is opaque and
/// only has to be the same in every answer; an id is the adapter's own
/// numbering, and the target's appears in its device path as its UID, as
/// a monitor's does on Windows.
const ADAPTER_LUID_LOW: u32 = 0x0001_0001;
const ADAPTER_LUID_HIGH: u32 = 0;
const SOURCE_ID: u32 = 0;
const TARGET_ID: u32 = 1;

/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL: gala's display is a panel of
/// the machine and not something plugged into a socket.
const OUTPUT_TECHNOLOGY_INTERNAL: u32 = 0x8000_0000;

/// The refresh rate everything else here reports (EnumDisplaySettingsW's
/// dmDisplayFrequency, GetDeviceCaps's VREFRESH).
const REFRESH_HZ: u32 = 60;

/// D3DKMDT_VSS_OTHER: gala's signal follows no VESA or CEA standard.
const VIDEO_STANDARD_OTHER: u32 = 255;

/// The monitor's name and its device-interface path under
/// GUID_DEVINTERFACE_MONITOR, and the adapter's under
/// GUID_DEVINTERFACE_DISPLAY_ADAPTER. The instance in the monitor's path
/// ends in its target's id, as a monitor's UID does on Windows.
/// EnumDisplayDevicesW answers with these too (`session.rs`), so a
/// program that reaches gala's display through GDI and through the CCD
/// finds the same monitor.
pub(crate) const MONITOR_NAME: &[u8] = b"gala display";
pub(crate) const MONITOR_PATH: &[u8] =
    br"\\?\DISPLAY#GALA0001#1&gala&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const ADAPTER_PATH: &[u8] = br"\\?\ROOT#DISPLAY#0000#{5b45201d-f2f2-4f3b-85bb-30ff1f953599}";

/// The two devices as the Plug and Play tree would name them: the
/// monitor's instance id under GUID_DEVCLASS_MONITOR (whose hardware id
/// and instance are the ones in its device path above) and that class's
/// key, and the adapter's instance id -- gala's display hangs off the
/// root and not off a bus, as its device path says -- and the Video key
/// a display adapter's settings live under. Nothing in gala's registry
/// answers at either key yet, so a program that opens one gets the
/// ledger's `absent key` and not a wrong answer.
pub(crate) const MONITOR_DEVICE_ID: &[u8] =
    br"MONITOR\GALA0001\{4d36e96e-e325-11ce-bfc1-08002be10318}\0001";
pub(crate) const MONITOR_CLASS_KEY: &[u8] = b"\\Registry\\Machine\\System\\CurrentControlSet\\Control\\Class\\{4d36e96e-e325-11ce-bfc1-08002be10318}\\0001";
pub(crate) const ADAPTER_DEVICE_ID: &[u8] = br"ROOT\DISPLAY\0000";
pub(crate) const ADAPTER_VIDEO_KEY: &[u8] = b"\\Registry\\Machine\\System\\CurrentControlSet\\Control\\Video\\{67616c61-0001-4000-8000-000000000000}\\0000";

/// The screen, as every other answer in this DLL has it.
fn size() -> (u32, u32) {
    let w = crate::window::GetSystemMetrics(0);
    let h = crate::window::GetSystemMetrics(1);
    (w.max(0) as u32, h.max(0) as u32)
}

/// Write a word of a caller's structure.
fn put32(at: *mut u8, offset: usize, value: u32) {
    // SAFETY: the caller's structure, whose size its `size` field or the
    // call's counts have been checked against `offset`.
    unsafe { at.add(offset).cast::<u32>().write_unaligned(value) };
}

fn put64(at: *mut u8, offset: usize, value: u64) {
    // SAFETY: as put32.
    unsafe { at.add(offset).cast::<u64>().write_unaligned(value) };
}

/// Write a wide string and its terminator; `room` is the field's length
/// in characters, and a text that would not fit is not written.
fn put_wide(at: *mut u8, offset: usize, text: &[u8], room: usize) {
    if text.len() + 1 > room {
        return;
    }
    for (i, &c) in text.iter().chain(core::iter::once(&0)).enumerate() {
        // SAFETY: as put32; `room` characters are there.
        unsafe { at.add(offset + 2 * i).cast::<u16>().write_unaligned(u16::from(c)) };
    }
}

fn get32(at: *const u8, offset: usize) -> u32 {
    // SAFETY: the caller's structure; only its header is read before its
    // `size` field has said how much is there.
    unsafe { at.add(offset).cast::<u32>().read_unaligned() }
}

/// The one topology a call asks for, or none when it names no topology or
/// more than one.
fn topology(flags: u32) -> Option<u32> {
    match flags & QDC_TOPOLOGY {
        QDC_ALL_PATHS | QDC_ONLY_ACTIVE_PATHS | QDC_DATABASE_CURRENT => Some(flags & QDC_TOPOLOGY),
        _ => None,
    }
}

/// GetDisplayConfigBufferSizes(flags, numPathArrayElements,
/// numModeInfoArrayElements): one path and two modes, whichever topology
/// is asked for -- gala's one source and one target make one pairing, so
/// the active paths and all the paths are the same path.
#[unsafe(no_mangle)]
pub extern "system" fn GetDisplayConfigBufferSizes(
    flags: u32,
    paths: *mut u32,
    modes: *mut u32,
) -> i32 {
    if paths.is_null() || modes.is_null() || topology(flags).is_none() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's two words, not null.
    unsafe {
        paths.write_unaligned(1);
        modes.write_unaligned(2);
    }
    OK
}

/// QueryDisplayConfig(flags, numPathArrayElements, pathArray,
/// numModeInfoArrayElements, modeInfoArray, currentTopologyId): the one
/// path and its two modes. The counts are what the caller has room for
/// going in and what was written coming out, as Windows does; a topology
/// id is asked for and answered only with QDC_DATABASE_CURRENT.
#[unsafe(no_mangle)]
pub extern "system" fn QueryDisplayConfig(
    flags: u32,
    paths: *mut u32,
    path_array: *mut u8,
    modes: *mut u32,
    mode_array: *mut u8,
    topology_id: *mut u32,
) -> i32 {
    let Some(asked) = topology(flags) else {
        return INVALID_PARAMETER;
    };
    if paths.is_null() || modes.is_null() || path_array.is_null() || mode_array.is_null() {
        return INVALID_PARAMETER;
    }
    if (asked == QDC_DATABASE_CURRENT) != !topology_id.is_null() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's two counts, not null.
    let (room_paths, room_modes) = unsafe { (paths.read_unaligned(), modes.read_unaligned()) };
    if room_paths == 0 || room_modes == 0 {
        return INVALID_PARAMETER;
    }
    if room_modes < 2 {
        return INSUFFICIENT_BUFFER;
    }
    let (w, h) = size();
    // SAFETY: room for one path of 72 bytes and two modes of 64, as the
    // counts the caller passed say.
    unsafe {
        ptr::write_bytes(path_array, 0, 72);
        ptr::write_bytes(mode_array, 0, 128);
    }
    // DISPLAYCONFIG_PATH_INFO: the source (adapter, id, the mode record's
    // index, DISPLAYCONFIG_SOURCE_IN_USE), the target (the same adapter,
    // its id, its mode record, how it is attached and how it is driven)
    // and DISPLAYCONFIG_PATH_ACTIVE.
    put32(path_array, 0, ADAPTER_LUID_LOW);
    put32(path_array, 4, ADAPTER_LUID_HIGH);
    put32(path_array, 8, SOURCE_ID);
    put32(path_array, 12, 1);
    put32(path_array, 16, 1);
    put32(path_array, 20, ADAPTER_LUID_LOW);
    put32(path_array, 24, ADAPTER_LUID_HIGH);
    put32(path_array, 28, TARGET_ID);
    put32(path_array, 32, 0);
    put32(path_array, 36, OUTPUT_TECHNOLOGY_INTERNAL);
    put32(path_array, 40, 1); // DISPLAYCONFIG_ROTATION_IDENTITY
    put32(path_array, 44, 1); // DISPLAYCONFIG_SCALING_IDENTITY
    put32(path_array, 48, REFRESH_HZ);
    put32(path_array, 52, 1);
    put32(path_array, 56, 1); // DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE
    put32(path_array, 60, 1); // targetAvailable
    put32(path_array, 64, 1); // DISPLAYCONFIG_TARGET_IN_USE
    put32(path_array, 68, 1); // DISPLAYCONFIG_PATH_ACTIVE
    // Mode 0, the target's signal. gala's display has no blanking, so the
    // total size is the active size and the pixel rate is the pixels of a
    // frame sixty times a second; the horizontal rate follows from the
    // lines of a frame.
    put32(mode_array, 0, 2); // DISPLAYCONFIG_MODE_INFO_TYPE_TARGET
    put32(mode_array, 4, TARGET_ID);
    put32(mode_array, 8, ADAPTER_LUID_LOW);
    put32(mode_array, 12, ADAPTER_LUID_HIGH);
    put64(mode_array, 16, u64::from(w) * u64::from(h) * u64::from(REFRESH_HZ));
    put32(mode_array, 24, h * REFRESH_HZ);
    put32(mode_array, 28, 1);
    put32(mode_array, 32, REFRESH_HZ);
    put32(mode_array, 36, 1);
    put32(mode_array, 40, w);
    put32(mode_array, 44, h);
    put32(mode_array, 48, w);
    put32(mode_array, 52, h);
    put32(mode_array, 56, VIDEO_STANDARD_OTHER);
    put32(mode_array, 60, 1); // PROGRESSIVE
    // Mode 1, the source's surface: the desktop, 32 bits a pixel, at the
    // origin (gala's one display is the whole desktop).
    let second = 64;
    put32(mode_array, second, 1); // DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
    put32(mode_array, second + 4, SOURCE_ID);
    put32(mode_array, second + 8, ADAPTER_LUID_LOW);
    put32(mode_array, second + 12, ADAPTER_LUID_HIGH);
    put32(mode_array, second + 16, w);
    put32(mode_array, second + 20, h);
    put32(mode_array, second + 24, 4); // DISPLAYCONFIG_PIXELFORMAT_32BPP
    put32(mode_array, second + 28, 0);
    put32(mode_array, second + 32, 0);
    // SAFETY: the caller's counts and topology, not null.
    unsafe {
        paths.write_unaligned(1);
        modes.write_unaligned(2);
        if !topology_id.is_null() {
            topology_id.write_unaligned(TOPOLOGY_INTERNAL);
        }
    }
    OK
}

/// The information types DisplayConfigGetDeviceInfo answers.
const GET_SOURCE_NAME: u32 = 1;
const GET_TARGET_NAME: u32 = 2;
const GET_ADAPTER_NAME: u32 = 4;

/// DisplayConfigGetDeviceInfo(packet): the three names of gala's one
/// display. Every packet starts with a DISPLAYCONFIG_DEVICE_INFO_HEADER
/// (type, size, adapter, id), and `size` must be the structure's, which is
/// how a caller and this code agree on which structure it is.
#[unsafe(no_mangle)]
pub extern "system" fn DisplayConfigGetDeviceInfo(packet: *mut u8) -> i32 {
    if packet.is_null() {
        return GEN_FAILURE;
    }
    let kind = get32(packet, 0);
    let size = get32(packet, 4);
    let adapter = (get32(packet, 8), get32(packet, 12));
    let id = get32(packet, 16);
    if adapter != (ADAPTER_LUID_LOW, ADAPTER_LUID_HIGH) {
        missing(b"user32.dll!DisplayConfigGetDeviceInfo(adapter)", u64::from(adapter.0));
        return INVALID_PARAMETER;
    }
    match kind {
        // DISPLAYCONFIG_SOURCE_DEVICE_NAME: 20 + CCHDEVICENAME.
        GET_SOURCE_NAME => {
            if size != 84 || id != SOURCE_ID {
                return INVALID_PARAMETER;
            }
            // SAFETY: 84 bytes, as `size` says.
            unsafe { ptr::write_bytes(packet.add(20), 0, 64) };
            put_wide(packet, 20, crate::monitor::DISPLAY_NAME, 32);
            OK
        }
        // DISPLAYCONFIG_TARGET_DEVICE_NAME: the flags, how it is attached,
        // the EDID ids (none: gala's display has no EDID), the connector's
        // instance, a name of 64 characters and a device path of 128.
        GET_TARGET_NAME => {
            if size != 420 || id != TARGET_ID {
                return INVALID_PARAMETER;
            }
            // SAFETY: 420 bytes, as `size` says.
            unsafe { ptr::write_bytes(packet.add(20), 0, 400) };
            put32(packet, 20, 2); // friendlyNameForced
            put32(packet, 24, OUTPUT_TECHNOLOGY_INTERNAL);
            put_wide(packet, 36, MONITOR_NAME, 64);
            put_wide(packet, 164, MONITOR_PATH, 128);
            OK
        }
        // DISPLAYCONFIG_ADAPTER_NAME: 20 + a device path of 128.
        GET_ADAPTER_NAME => {
            if size != 276 {
                return INVALID_PARAMETER;
            }
            // SAFETY: 276 bytes, as `size` says.
            unsafe { ptr::write_bytes(packet.add(20), 0, 256) };
            put_wide(packet, 20, ADAPTER_PATH, 128);
            OK
        }
        _ => {
            missing(b"user32.dll!DisplayConfigGetDeviceInfo(type)", u64::from(kind));
            NOT_SUPPORTED
        }
    }
}

/// DisplayConfigSetDeviceInfo(packet): gala's display configuration is not
/// something a program can change.
#[unsafe(no_mangle)]
pub extern "system" fn DisplayConfigSetDeviceInfo(packet: *mut u8) -> i32 {
    let kind = if packet.is_null() { 0 } else { get32(packet, 0) };
    missing(b"user32.dll!DisplayConfigSetDeviceInfo", u64::from(kind));
    NOT_SUPPORTED
}

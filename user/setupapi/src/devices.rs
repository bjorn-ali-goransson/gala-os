//! The device tree, read where Windows keeps it: the registry. A device
//! is a key under `SYSTEM\CurrentControlSet\Enum\<enumerator>\<hardware
//! id>\<instance>` carrying its class and its `Device Parameters`, and a
//! device interface is a key under `Control\DeviceClasses\{class}`,
//! named after the interface's device path, pointing back at its device
//! and linked while that device is present.
//!
//! gala's display is the only device there so far
//! (`kernel/src/nt/registry.seed`): one monitor on one adapter, with the
//! EDID the kernel writes into the monitor's `Device Parameters` at boot
//! (`nt::desktop::edid`). A program that walks the tree for it --
//! DXVK's `getMonitorEdid`, and Word under it -- finds the same device
//! path the CCD and EnumDisplayDevicesW give (`user/user32/src/ccd.rs`).
//!
//! Nothing is cached: every call reads the tree again, through advapi32
//! as Windows' setupapi does, and nothing here allocates (a device
//! instance id has a bounded length, so the walk's buffers are fixed).

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: usize,
        sub: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegEnumKeyExW(
        key: usize,
        index: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        written: *mut u64,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const KEY_READ: u32 = 0x2_0019;
const SUCCESS: i32 = 0;
const REG_DWORD: u32 = 4;

/// The root of the walk below, and the two keys beside it.
const ENUM: &[u8] = br"SYSTEM\CurrentControlSet\Enum";
const DEVICE_CLASSES: &[u8] = br"SYSTEM\CurrentControlSet\Control\DeviceClasses\";
const CLASS: &[u8] = br"SYSTEM\CurrentControlSet\Control\Class\";

/// A device instance id is at most 200 characters (MAX_DEVICE_ID_LEN);
/// a device path and the registry paths below are a little longer.
pub const MAX_WIDE: usize = 320;

/// A wide string in a buffer of its own, always ended by a NUL (nothing
/// below fills the last unit), so it can be passed as a name.
#[derive(Clone, Copy)]
pub struct Wide {
    units: [u16; MAX_WIDE],
    len: usize,
}

impl Wide {
    pub const fn new() -> Self {
        Self { units: [0; MAX_WIDE], len: 0 }
    }

    /// The wide string of ASCII text.
    pub fn of(text: &[u8]) -> Self {
        let mut w = Self::new();
        w.push_ascii(text);
        w
    }

    pub fn push_ascii(&mut self, text: &[u8]) {
        for &c in text {
            self.push(u16::from(c));
        }
    }

    pub fn push_wide(&mut self, units: &[u16]) {
        for &u in units {
            self.push(u);
        }
    }

    /// Appends one unit, dropping it if the buffer is full: a name that
    /// long is not one the tree has, so the lookup fails either way.
    pub fn push(&mut self, unit: u16) {
        if self.len + 1 < MAX_WIDE {
            self.units[self.len] = unit;
            self.len += 1;
        }
    }

    pub fn units(&self) -> &[u16] {
        &self.units[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn ptr(&self) -> *const u16 {
        self.units.as_ptr()
    }

    /// Whether two names are the same, as the registry compares them.
    pub fn same(&self, other: &Wide) -> bool {
        self.len == other.len
            && self.units().iter().zip(other.units()).all(|(&a, &b)| fold(a) == fold(b))
    }

    /// Whether this name's first component is that ASCII text, so that
    /// `DISPLAY\GALA0001\1&gala&0&UID1` is under `DISPLAY`:
    /// `DISPLAY\GALA0001\...` is under `DISPLAY`, which is what
    /// SetupDiGetClassDevsW's Enumerator limits a set to.
    pub fn under(&self, text: &[u8]) -> bool {
        self.len > text.len()
            && self.units[text.len()] == u16::from(b'\\')
            && self.units[..text.len()]
                .iter()
                .zip(text)
                .all(|(&u, &c)| fold(u) == fold(u16::from(c)))
    }
}

impl Default for Wide {
    fn default() -> Self {
        Self::new()
    }
}

/// A unit as the registry folds it for a comparison (ASCII is enough:
/// every name in the tree is a hardware id or a GUID).
fn fold(unit: u16) -> u16 {
    if unit < 0x80 { u16::from((unit as u8).to_ascii_lowercase()) } else { unit }
}

/// A key under HKEY_LOCAL_MACHINE, or none if the tree has no such key.
fn open(path: &Wide) -> Option<usize> {
    let mut key = 0;
    // SAFETY: our NUL-ended path and one handle out.
    let r = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.ptr(), 0, KEY_READ, &mut key) };
    (r == SUCCESS).then_some(key)
}

/// That key with more path below it.
fn open_below(path: &Wide, below: &[u16]) -> Option<usize> {
    let mut p = *path;
    p.push(u16::from(b'\\'));
    p.push_wide(below);
    open(&p)
}

fn close(key: usize) {
    // SAFETY: a key this module opened.
    unsafe { RegCloseKey(key) };
}

/// The `index`th subkey's name.
fn subkey(key: usize, index: u32) -> Option<Wide> {
    let mut name = [0u16; MAX_WIDE];
    let mut len = MAX_WIDE as u32;
    // SAFETY: our buffer, whose length the call is given and updates.
    let r = unsafe {
        RegEnumKeyExW(
            key,
            index,
            name.as_mut_ptr(),
            &mut len,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    if r != SUCCESS || len as usize >= MAX_WIDE {
        return None;
    }
    let mut w = Wide::new();
    w.push_wide(&name[..len as usize]);
    Some(w)
}

/// A string value of a key, with its terminator dropped.
fn string_value(key: usize, name: &[u8]) -> Option<Wide> {
    let name = Wide::of(name);
    let mut data = [0u16; MAX_WIDE];
    let mut size = (2 * MAX_WIDE) as u32;
    let mut kind = 0;
    // SAFETY: our name and buffer, whose size the call is given.
    let r = unsafe {
        RegQueryValueExW(
            key,
            name.ptr(),
            core::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != SUCCESS {
        return None;
    }
    let mut w = Wide::new();
    for &u in &data[..(size as usize / 2).min(MAX_WIDE)] {
        if u == 0 {
            break;
        }
        w.push(u);
    }
    Some(w)
}

/// A REG_DWORD value of a key.
fn dword_value(key: usize, name: &[u8]) -> Option<u32> {
    let name = Wide::of(name);
    let mut data = 0u32;
    let mut size = 4;
    let mut kind = 0;
    // SAFETY: our name and one dword out.
    let r = unsafe {
        RegQueryValueExW(
            key,
            name.ptr(),
            core::ptr::null_mut(),
            &mut kind,
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (r == SUCCESS && kind == REG_DWORD).then_some(data)
}

/// A class GUID as the registry spells it, `{xxxxxxxx-xxxx-xxxx-xxxx-
/// xxxxxxxxxxxx}`: the first three fields little-endian, the last eight
/// bytes in order, with a dash after the fourth byte of the text and
/// every second one after that.
const ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
const DASHES: [usize; 4] = [4, 6, 8, 10];

/// Its text, which is always 38 units long.
pub fn guid_text(guid: &[u8; 16]) -> Wide {
    let mut w = Wide::new();
    w.push(u16::from(b'{'));
    for (i, &at) in ORDER.iter().enumerate() {
        if DASHES.contains(&i) {
            w.push(u16::from(b'-'));
        }
        for nibble in [guid[at] >> 4, guid[at] & 0xf] {
            w.push(u16::from(b"0123456789abcdef"[nibble as usize]));
        }
    }
    w.push(u16::from(b'}'));
    w
}

/// The sixteen bytes of such a text, if it is one.
pub fn guid_bytes(text: &[u16]) -> Option<[u8; 16]> {
    if text.len() != 38 || text[0] != u16::from(b'{') || text[37] != u16::from(b'}') {
        return None;
    }
    let mut guid = [0u8; 16];
    let mut at = 1;
    for (i, &to) in ORDER.iter().enumerate() {
        if DASHES.contains(&i) {
            if text[at] != u16::from(b'-') {
                return None;
            }
            at += 1;
        }
        let mut byte = 0u8;
        for _ in 0..2 {
            let digit = match u8::try_from(text[at]).ok()? {
                c @ b'0'..=b'9' => c - b'0',
                c @ b'a'..=b'f' => c - b'a' + 10,
                c @ b'A'..=b'F' => c - b'A' + 10,
                _ => return None,
            };
            byte = (byte << 4) | digit;
            at += 1;
        }
        guid[to] = byte;
    }
    Some(guid)
}

/// A device: the instance id that names it in the tree, its class, and
/// the devnode id this module gives it -- its place in the whole tree,
/// counted from one, so that a SP_DEVINFO_DATA can be turned back into a
/// device without a table of them.
#[derive(Clone, Copy)]
pub struct Device {
    pub instance: Wide,
    pub class: [u8; 16],
    pub devinst: u32,
}

/// A device interface: the path a program opens it by, its class, its
/// device, and its own place among the interfaces of that class, which
/// is what a SP_DEVICE_INTERFACE_DATA carries so that it can be found
/// again without a table of them.
#[derive(Clone, Copy)]
pub struct Interface {
    pub path: Wide,
    pub class: [u8; 16],
    pub device: Device,
    pub nth: u32,
}

/// The path under HKEY_LOCAL_MACHINE of `Enum\<instance>`.
fn enum_path(instance: &Wide) -> Wide {
    let mut path = Wide::of(ENUM);
    path.push(u16::from(b'\\'));
    path.push_wide(instance.units());
    path
}

/// The class an instance id's key gives, if the tree has that device.
fn class_at(instance: &Wide) -> Option<[u8; 16]> {
    let key = open(&enum_path(instance))?;
    let class = string_value(key, b"ClassGUID").and_then(|t| guid_bytes(t.units()));
    close(key);
    Some(class.unwrap_or([0; 16]))
}

/// Every device in the tree, in order, to `keep`, until it says to stop:
/// `Enum` is three levels deep -- the enumerator, the hardware id and
/// the instance -- and a device is a key at the third. The devnode ids
/// this hands out count every device, so they do not depend on a filter.
fn walk(mut keep: impl FnMut(&Device) -> bool) {
    let root = Wide::of(ENUM);
    let Some(top) = open(&root) else { return };
    let mut devinst = 0;
    let mut going = true;
    for a in 0.. {
        let Some(first) = subkey(top, a) else { break };
        let Some(enumerator) = open_below(&root, first.units()) else { continue };
        for b in 0.. {
            let Some(second) = subkey(enumerator, b) else { break };
            let mut below = first;
            below.push(u16::from(b'\\'));
            below.push_wide(second.units());
            let Some(hardware) = open_below(&root, below.units()) else { continue };
            for c in 0.. {
                let Some(third) = subkey(hardware, c) else { break };
                let mut instance = below;
                instance.push(u16::from(b'\\'));
                instance.push_wide(third.units());
                let Some(class) = class_at(&instance) else { continue };
                devinst += 1;
                if !keep(&Device { instance, class, devinst }) {
                    going = false;
                    break;
                }
            }
            close(hardware);
            if !going {
                break;
            }
        }
        close(enumerator);
        if !going {
            break;
        }
    }
    close(top);
}

/// The `index`th device the filter keeps.
pub fn find_device(index: u32, keep: impl Fn(&Device) -> bool) -> Option<Device> {
    let mut seen = 0;
    let mut found = None;
    walk(|device| {
        if !keep(device) {
            return true;
        }
        if seen == index {
            found = Some(*device);
            return false;
        }
        seen += 1;
        true
    });
    found
}

/// The device a devnode id names.
pub fn by_devinst(devinst: u32) -> Option<Device> {
    find_device(0, |device| device.devinst == devinst)
}

/// The device an instance id names.
pub fn by_instance(instance: &Wide) -> Option<Device> {
    find_device(0, |device| device.instance.same(instance))
}

/// The `index`th device interface of a class that the filter keeps: a
/// linked subkey of `Control\DeviceClasses\{class}`, whose name is the
/// device path with `##?#` where the path has `\\?\`.
pub fn find_interface(
    class: &[u8; 16],
    index: u32,
    keep: impl Fn(&Interface) -> bool,
) -> Option<Interface> {
    let mut path = Wide::of(DEVICE_CLASSES);
    path.push_wide(guid_text(class).units());
    let key = open(&path)?;
    let mut seen = 0;
    let mut nth = 0;
    let mut found = None;
    for at in 0.. {
        let Some(name) = subkey(key, at) else { break };
        // An interface is linked while its device is there, which is
        // what a DIGCF_PRESENT set holds -- and what every set gala's
        // tree answers holds, a described device being a present one.
        let mut control = name;
        control.push_ascii(br"\#\Control");
        let linked = open_below(&path, control.units()).is_some_and(|k| {
            let linked = dword_value(k, b"Linked") == Some(1);
            close(k);
            linked
        });
        if !linked {
            continue;
        }
        let instance = open_below(&path, name.units()).and_then(|k| {
            let instance = string_value(k, b"DeviceInstance");
            close(k);
            instance
        });
        let Some(device) = instance.and_then(|i| by_instance(&i)) else { continue };
        // `##?#DISPLAY#...#{class}` is the path `\\?\DISPLAY#...#{class}`.
        let mut device_path = Wide::new();
        if name.units().starts_with(&[0x23, 0x23, 0x3f, 0x23]) {
            device_path.push_ascii(b"\\\\?\\");
            device_path.push_wide(&name.units()[4..]);
        } else {
            device_path.push_wide(name.units());
        }
        // Its place among the class's own, counted before the filter so
        // that it is the same number whoever is asking.
        let candidate = Interface { path: device_path, class: *class, device, nth };
        nth += 1;
        if !keep(&candidate) {
            continue;
        }
        if seen == index {
            found = Some(candidate);
            break;
        }
        seen += 1;
    }
    close(key);
    found
}

/// The interface a device path names: its class is the last field of the
/// path itself, so one class is searched rather than all of them.
pub fn by_path(path: &Wide) -> Option<Interface> {
    let units = path.units();
    let class = guid_bytes(units.get(units.len().checked_sub(38)?..)?)?;
    find_interface(&class, 0, |candidate| candidate.path.same(path))
}

/// A device's own registry key: its `Device Parameters` (DIREG_DEV), or
/// the key its driver's settings are in (DIREG_DRV, the `Driver` value
/// under `Control\Class`). The caller owns the handle.
pub fn device_key(device: &Device, driver: bool, access: u32) -> Option<usize> {
    let path = if driver {
        let key = open(&enum_path(&device.instance))?;
        let value = string_value(key, b"Driver");
        close(key);
        let mut path = Wide::of(CLASS);
        path.push_wide(value?.units());
        path
    } else {
        let mut path = enum_path(&device.instance);
        path.push_ascii(br"\Device Parameters");
        path
    };
    let mut key = 0;
    // SAFETY: our NUL-ended path and one handle out.
    let r = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.ptr(), 0, access, &mut key) };
    (r == SUCCESS).then_some(key)
}

//! What a component's registration says, read out of the registry
//! Windows Installer writes it to.
//!
//! An installer registers a component as a key named for the
//! component's packed ("squished") GUID under
//! `\Registry\Machine\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Components`,
//! holding one value per product that installed it: the value's name is
//! the product's squished GUID and its data the component's key path --
//! the file (or registry value) whose presence stands for the
//! component's. That is what MsiGetProductCodeW, MsiEnumClientsW,
//! MsiGetComponentPathW and MsiLocateComponentW answer out of, and both
//! `msi_test`'s fixture (`components.regseed`) and Office's own
//! registration -- which Click-to-Run writes into gala's registry
//! during `office_install`, where it survives as a replayed registry
//! change -- have that shape. Word resolves its resource files through
//! it: MSO asks which product installed the component whose key file is
//! WINWORD.EXE, then where that component is, and lays its resource
//! paths out beside it.
//!
//! The limits, measured rather than guessed at:
//!
//! * Only the per-machine registrations under the local system
//!   account's UserData are read. Windows keeps per-user ones as well,
//!   under `HKEY_CURRENT_USER\Software\Microsoft\Installer\Components`;
//!   which of the two answers when both have the component was not
//!   measured, and nothing gala runs has been seen to need the per-user
//!   key.
//! * Whether the registered file is still there is not asked, so a
//!   component whose key file has been deleted answers
//!   INSTALLSTATE_LOCAL here where Windows may answer otherwise. The
//!   absent-file answer could not be measured: of the 27886 plain
//!   file registrations on the machine the expectations come from, every
//!   one names a file that is there (`msi_test` says so as its own
//!   limit).
//! * A registration longer than `MAX_KEYPATH` characters is not handed
//!   back. The call records a ledger row naming the function that could
//!   not read it, so a program that fails for it leaves the reason
//!   behind instead of looking like a component nothing installed.
//!
//! Reading a value's name and data out of the information structure
//! assumes a little-endian machine, as the rest of gala does.

use crate::CODE_LEN;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const UnicodeString,
    attributes: u32,
    security: usize,
    qos: usize,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtOpenKey(out: *mut usize, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtEnumerateValueKey(
        key: usize,
        index: u32,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(key: usize) -> u32;
}

const KEY_QUERY_VALUE: u32 = 0x1;

/// The registrations are read in the 64-bit view, whatever the process.
/// `SOFTWARE\Microsoft\Windows\CurrentVersion\Installer` is one of the
/// keys WOW64 redirects (it is not in Microsoft Learn's shared list, and
/// a 32-bit process's redirected view has no `UserData` under it at all,
/// measured: `WOW6432Node\...\Installer` holds only `Secure`), and yet
/// Windows' own 32-bit msi.dll answers a 32-bit caller out of the
/// machine's registrations -- MsiGetProductCodeW and
/// MsiGetComponentPathW measured from a 32-bit process name the product
/// and the path a 64-bit installer registered. So it opens the 64-bit
/// view, and so does this (`wow64_msi_test` is the same checks as
/// `msi_test` in a 32-bit process, and has the same expectations).
const KEY_WOW64_64KEY: u32 = 0x100;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xC000_0023;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;

/// KEY_VALUE_FULL_INFORMATION: a title index, the kind, the offset of
/// the data from the start of the structure, the data's length in bytes
/// and the name's, then the name and, at that offset, the data. The one
/// class that carries both the value's name (the product) and its data
/// (the key path), so one enumeration answers every question here.
const KEY_VALUE_FULL_INFORMATION: u32 = 1;

/// The five words before the name.
const FULL_FIXED: usize = 20;

/// The hexadecimal digits of a GUID, which is what its packed form is
/// written as and how long a value's name under a component key is.
const PACKED_LEN: usize = 32;

/// The longest key path gala hands back. Longer than any Windows
/// Installer writes (a file path is bounded by MAX_PATH and a registry
/// one by the key-name limits), and the bound the enumeration buffer is
/// sized from.
const MAX_KEYPATH: usize = 512;

/// The information structure, in 32-bit words: the fixed part, a
/// 32-character name, the alignment between them and the longest key
/// path, with room to spare.
const VALUE_WORDS: usize = 288;

/// Where a component's registrations live. Spelled as the registry
/// takes it; the lookup is case-insensitive either way
/// (OBJ_CASE_INSENSITIVE), which is why a component code of either case
/// finds the same key (`msi_test`).
const COMPONENTS: &str = r"\Registry\Machine\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Components\";

/// The name of a component's key: that path and the packed code.
const KEY_CHARS: usize = COMPONENTS.len() + PACKED_LEN;

/// The digit permutation Windows Installer's packed ("squished") form
/// is: the first eight hexadecimal digits reversed, then the next four,
/// then the next four, then each remaining pair swapped. Reversal and
/// pair-swapping are each their own inverse, so this both packs a GUID
/// and unpacks one. The case of every digit is carried through
/// unchanged, because Windows hands a product code back in the case it
/// was registered with (`msi_test`).
fn permute(digits: &[u8; PACKED_LEN]) -> [u8; PACKED_LEN] {
    let mut out = [0; PACKED_LEN];
    for (slot, &d) in out[..8].iter_mut().zip(digits[..8].iter().rev()) {
        *slot = d;
    }
    for (slot, &d) in out[8..12].iter_mut().zip(digits[8..12].iter().rev()) {
        *slot = d;
    }
    for (slot, &d) in out[12..16].iter_mut().zip(digits[12..16].iter().rev()) {
        *slot = d;
    }
    for (pair, from) in out[16..].chunks_mut(2).zip(digits[16..].chunks(2)) {
        pair[0] = from[1];
        pair[1] = from[0];
    }
    out
}

/// The packed form of a caller's braced GUID, or nothing if it is not
/// one. One body for both string widths, as `is_guid`: the ANSI
/// functions ask it of their own bytes.
fn squish<T: Copy + Into<u32>>(code: *const T) -> Option<[u8; PACKED_LEN]> {
    if !crate::is_guid(code) {
        return None;
    }
    let mut digits = [0; PACKED_LEN];
    let mut n = 0;
    for i in 1..CODE_LEN - 1 {
        // SAFETY: `is_guid` read the whole 38-character string and its
        // NUL already, so every index up to 36 is the caller's.
        let c: u32 = unsafe { *code.add(i) }.into();
        if c == u32::from(b'-') {
            continue;
        }
        digits[n] = c as u8;
        n += 1;
    }
    (n == PACKED_LEN).then(|| permute(&digits))
}

/// A packed code written back as a braced GUID, in the case it is
/// stored in.
fn unsquish(packed: &[u8; PACKED_LEN]) -> [u8; CODE_LEN] {
    let digits = permute(packed);
    let mut out = [0; CODE_LEN];
    out[0] = b'{';
    out[CODE_LEN - 1] = b'}';
    for at in [9, 14, 19, 24] {
        out[at] = b'-';
    }
    for (at, group) in [(1, 0..8), (10, 8..12), (15, 12..16), (20, 16..20), (25, 20..32)] {
        out[at..at + group.len()].copy_from_slice(&digits[group]);
    }
    out
}

/// The `i`th byte of an information structure.
fn byte_at(words: &[u32; VALUE_WORDS], i: usize) -> u8 {
    (words[i / 4] >> (8 * (i % 4))) as u8
}

/// The 16-bit unit at byte `i` of an information structure.
fn unit_at(words: &[u32; VALUE_WORDS], i: usize) -> u16 {
    u16::from(byte_at(words, i)) | (u16::from(byte_at(words, i + 1)) << 8)
}

/// A component's key path, as a registration stores it and as Windows
/// hands it back: with Windows Installer's `?` marker right after the
/// drive taken away (a path stored as `C:\?WINDOWS\SysWOW64\x.dll` is
/// answered as `C:\WINDOWS\SysWOW64\x.dll`, measured), and a registry
/// key path -- two digits for the root, a colon, then the key and value
/// -- exactly as stored.
pub(crate) struct KeyPath {
    text: [u16; MAX_KEYPATH],
    len: usize,
}

impl KeyPath {
    /// The stored data as it is handed back, or nothing if it does not
    /// fit.
    fn of(stored: &[u16]) -> Option<KeyPath> {
        if stored.len() > MAX_KEYPATH {
            return None;
        }
        let marked = stored.len() > 3
            && stored[1] == u16::from(b':')
            && stored[2] == u16::from(b'\\')
            && stored[3] == u16::from(b'?');
        let mut path = KeyPath { text: [0; MAX_KEYPATH], len: 0 };
        for (i, &c) in stored.iter().enumerate() {
            if marked && i == 3 {
                continue;
            }
            path.text[path.len] = c;
            path.len += 1;
        }
        Some(path)
    }

    fn chars(&self) -> &[u16] {
        &self.text[..self.len]
    }
}

/// A component's registration: the open key the products that installed
/// it are listed under, and the name of the function reading it, so a
/// registration gala cannot read is recorded against the call that
/// wanted it.
pub(crate) struct Component {
    key: usize,
    name: &'static [u8],
}

impl Drop for Component {
    fn drop(&mut self) {
        // SAFETY: a key this process opened and has not closed.
        unsafe { NtClose(self.key) };
    }
}

impl Component {
    /// Opens the registration of `code`, or nothing if the code is not a
    /// braced GUID or no product has registered the component.
    pub(crate) fn open<T: Copy + Into<u32>>(
        code: *const T,
        name: &'static [u8],
    ) -> Option<Component> {
        let packed = squish(code)?;
        let mut key_name = [0u16; KEY_CHARS];
        let spelled = COMPONENTS.bytes().chain(packed.iter().copied());
        for (slot, b) in key_name.iter_mut().zip(spelled) {
            *slot = u16::from(b);
        }
        let bytes = u16::try_from(KEY_CHARS * 2).ok()?;
        let us = UnicodeString { length: bytes, maximum: bytes, buffer: key_name.as_ptr() };
        let attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root: 0,
            name: &us,
            attributes: OBJ_CASE_INSENSITIVE,
            security: 0,
            qos: 0,
        };
        let mut key = 0;
        // SAFETY: an output and attributes of this frame's, naming a
        // string that outlives the call.
        let access = KEY_QUERY_VALUE | KEY_WOW64_64KEY;
        let status = unsafe { NtOpenKey(&mut key, access, &attributes) };
        (status == 0).then_some(Component { key, name })
    }

    /// The product that registered the component at `index`, as the
    /// registration spells it, or nothing past the last one.
    pub(crate) fn client(&self, index: u32) -> Option<[u8; CODE_LEN]> {
        let mut seen = 0;
        self.each_value(|product, _| {
            if seen == index {
                return Some(unsquish(product));
            }
            seen += 1;
            None
        })
    }

    /// Where the component is, as `product` registered it.
    pub(crate) fn key_path<T: Copy + Into<u32>>(&self, product: *const T) -> Option<KeyPath> {
        let want = squish(product)?;
        self.each_value(|stored, path| {
            let same = stored.iter().zip(want.iter()).all(|(a, b)| a.eq_ignore_ascii_case(b));
            same.then(|| KeyPath::of(path)).flatten()
        })
    }

    /// Where the component is, whichever product registered it there --
    /// what MsiLocateComponentW answers.
    pub(crate) fn any_key_path(&self) -> Option<KeyPath> {
        self.each_value(|_, path| KeyPath::of(path))
    }

    /// Walks the registration's values in the order the registry lists
    /// them, handing each one's product (packed) and its key path to
    /// `each` until it answers.
    fn each_value<R>(
        &self,
        mut each: impl FnMut(&[u8; PACKED_LEN], &[u16]) -> Option<R>,
    ) -> Option<R> {
        let mut index = 0;
        loop {
            let mut words = [0u32; VALUE_WORDS];
            let mut got = 0;
            // SAFETY: a key this process opened, and a buffer of this
            // frame's named with its own size.
            let status = unsafe {
                NtEnumerateValueKey(
                    self.key,
                    index,
                    KEY_VALUE_FULL_INFORMATION,
                    words.as_mut_ptr().cast(),
                    (VALUE_WORDS * 4) as u32,
                    &mut got,
                )
            };
            index += 1;
            if status == STATUS_BUFFER_OVERFLOW || status == STATUS_BUFFER_TOO_SMALL {
                self.too_long(u64::from(got));
                continue;
            }
            if status != 0 {
                return None;
            }
            let got = got as usize;
            let (kind, data_at, data_bytes, name_bytes) =
                (words[1], words[2] as usize, words[3] as usize, words[4] as usize);
            let fits = data_at + data_bytes <= got && FULL_FIXED + name_bytes <= got;
            if (kind != REG_SZ && kind != REG_EXPAND_SZ)
                || name_bytes != PACKED_LEN * 2
                || !fits
                || !data_bytes.is_multiple_of(2)
            {
                continue;
            }
            let mut product = [0; PACKED_LEN];
            let mut hex = true;
            for (i, slot) in product.iter_mut().enumerate() {
                match u8::try_from(unit_at(&words, FULL_FIXED + i * 2)) {
                    Ok(b) if b.is_ascii_hexdigit() => *slot = b,
                    _ => hex = false,
                }
            }
            if !hex {
                continue;
            }
            let mut path = [0u16; MAX_KEYPATH];
            let mut len = 0;
            let mut whole = true;
            for i in 0..data_bytes / 2 {
                let c = unit_at(&words, data_at + i * 2);
                if c == 0 {
                    break;
                }
                if len == MAX_KEYPATH {
                    whole = false;
                    break;
                }
                path[len] = c;
                len += 1;
            }
            if !whole {
                self.too_long(data_bytes as u64);
                continue;
            }
            if let Some(answer) = each(&product, &path[..len]) {
                return Some(answer);
            }
        }
    }

    /// Records a registration gala could not read, the detail its length
    /// in bytes.
    fn too_long(&self, bytes: u64) {
        crate::record_gap(self.name, b": a registration too long to read", 2, bytes);
    }
}

/// Hands a key path to a MsiGetComponentPathW-shaped caller and answers
/// its INSTALLSTATE. Measured: the size is the length in characters
/// without the NUL, a buffer too small for the whole path and its NUL
/// is INSTALLSTATE_MOREDATA with the size still set to the whole
/// length, and no buffer at all is INSTALLSTATE_LOCAL with that length.
pub(crate) fn hand_back(path: &KeyPath, out: *mut u16, size: *mut u32) -> i32 {
    let text = path.chars();
    let room = if size.is_null() {
        0
    } else {
        // SAFETY: the caller's size, which it has to have given with a
        // buffer (`MsiGetComponentPathW`).
        unsafe { *size as usize }
    };
    let whole = out.is_null() || room > text.len();
    if !out.is_null() && whole {
        for (i, &c) in text.iter().enumerate() {
            // SAFETY: the caller's buffer, which it says has room for
            // the path and its NUL.
            unsafe { *out.add(i) = c };
        }
        // SAFETY: as above.
        unsafe { *out.add(text.len()) = 0 };
    }
    if !size.is_null() {
        // SAFETY: the caller's size.
        unsafe { *size = u32::try_from(text.len()).unwrap_or(u32::MAX) };
    }
    if whole { crate::INSTALLSTATE_LOCAL } else { crate::INSTALLSTATE_MOREDATA }
}

/// The same for the ANSI form, with anything outside ASCII written as
/// `?`: gala's own paths are ASCII, and converting by the process's code
/// page would ask kernel32 for what msi.dll does not import.
pub(crate) fn hand_back_ansi(path: &KeyPath, out: *mut u8, size: *mut u32) -> i32 {
    let text = path.chars();
    let room = if size.is_null() {
        0
    } else {
        // SAFETY: the caller's size (`MsiGetComponentPathA`).
        unsafe { *size as usize }
    };
    let whole = out.is_null() || room > text.len();
    if !out.is_null() && whole {
        for (i, &c) in text.iter().enumerate() {
            let b = u8::try_from(c).map_or(b'?', |b| if b.is_ascii() { b } else { b'?' });
            // SAFETY: the caller's buffer, which it says has room for
            // the path and its NUL.
            unsafe { *out.add(i) = b };
        }
        // SAFETY: as above.
        unsafe { *out.add(text.len()) = 0 };
    }
    if !size.is_null() {
        // SAFETY: the caller's size.
        unsafe { *size = u32::try_from(text.len()).unwrap_or(u32::MAX) };
    }
    if whole { crate::INSTALLSTATE_LOCAL } else { crate::INSTALLSTATE_MOREDATA }
}

/// Writes a product code and its NUL into a caller's buffer, which
/// MsiGetProductCodeW and MsiEnumClientsW take on trust to hold the 39
/// characters a braced GUID needs (both are documented that way and
/// neither is given a size).
pub(crate) fn write_code(out: *mut u16, code: &[u8; CODE_LEN]) {
    for (i, &b) in code.iter().enumerate() {
        // SAFETY: the caller's buffer, 39 characters by the contract of
        // the functions that call this.
        unsafe { *out.add(i) = u16::from(b) };
    }
    // SAFETY: as above.
    unsafe { *out.add(CODE_LEN) = 0 };
}

/// The same for the ANSI form. A product code is hexadecimal digits,
/// braces and dashes, so there is nothing to convert.
pub(crate) fn write_code_ansi(out: *mut u8, code: &[u8; CODE_LEN]) {
    for (i, &b) in code.iter().enumerate() {
        // SAFETY: the caller's buffer, 39 bytes by the contract of
        // MsiGetProductCodeA.
        unsafe { *out.add(i) = b };
    }
    // SAFETY: as above.
    unsafe { *out.add(CODE_LEN) = 0 };
}

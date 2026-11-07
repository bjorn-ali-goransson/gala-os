//! The keyboard layout (US English, the one gala's PS/2 keyboard maps):
//! VkKeyScanW (a character's virtual key and shift state), MapVirtualKeyW
//! (virtual keys, scan codes and characters), GetKeyboardLayout,
//! LoadKeyboardLayoutW (only US's KLID) and ToUnicode(Ex) (a key and a
//! key state to characters).

/// Set 1 scan codes of the virtual keys that have one (index: VK).
fn scan_of(vk: u32) -> u32 {
    const LETTERS: &[u8; 26] = b"\x1e\x30\x2e\x20\x12\x21\x22\x23\x17\x24\x25\x26\x32\x31\x18\x19\x10\x13\x1f\x14\x16\x2f\x11\x2d\x15\x2c";
    match vk {
        0x41..=0x5a => u32::from(LETTERS[(vk - 0x41) as usize]),
        0x31..=0x39 => vk - 0x31 + 2,
        0x30 => 0x0b,
        0x08 => 0x0e,
        0x09 => 0x0f,
        0x0d => 0x1c,
        0x10 => 0x2a,
        0x11 => 0x1d,
        0x12 => 0x38,
        0x14 => 0x3a,
        0x1b => 0x01,
        0x20 => 0x39,
        0x70..=0x79 => vk - 0x70 + 0x3b,
        0x7a => 0x57,
        0x7b => 0x58,
        0xba => 0x27,
        0xbb => 0x0d,
        0xbc => 0x33,
        0xbd => 0x0c,
        0xbe => 0x34,
        0xbf => 0x35,
        0xc0 => 0x29,
        0xdb => 0x1a,
        0xdc => 0x2b,
        0xdd => 0x1b,
        0xde => 0x28,
        // Extended keys share their scan codes with the keypad's.
        0x25 => 0x4b,
        0x26 => 0x48,
        0x27 => 0x4d,
        0x28 => 0x50,
        0x21 => 0x49,
        0x22 => 0x51,
        0x23 => 0x4f,
        0x24 => 0x47,
        0x2d => 0x52,
        0x2e => 0x53,
        _ => 0,
    }
}

/// The unshifted character of a virtual key (0: none).
fn char_of(vk: u32) -> u32 {
    match vk {
        0x41..=0x5a => vk + 32,
        0x30..=0x39 | 0x20 => vk,
        0x08 | 0x09 | 0x0d | 0x1b => vk,
        0xba => u32::from(b';'),
        0xbb => u32::from(b'='),
        0xbc => u32::from(b','),
        0xbd => u32::from(b'-'),
        0xbe => u32::from(b'.'),
        0xbf => u32::from(b'/'),
        0xc0 => u32::from(b'`'),
        0xdb => u32::from(b'['),
        0xdc => u32::from(b'\\'),
        0xdd => u32::from(b']'),
        0xde => u32::from(b'\''),
        _ => 0,
    }
}

/// VkKeyScanW: the virtual key in the low byte, the shift state in the
/// high (1 Shift, 2 Ctrl, 4 Alt); -1 for a character the layout lacks.
#[unsafe(no_mangle)]
pub extern "system" fn VkKeyScanW(ch: u16) -> i16 {
    let c = u32::from(ch);
    const SHIFTED: &[u8] = b")!@#$%^&*(";
    let (vk, shift) = match c {
        0x61..=0x7a => (c - 32, 0),
        0x41..=0x5a => (c, 1),
        0x30..=0x39 | 0x20 | 0x08 | 0x09 | 0x0d | 0x1b => (c, 0),
        _ => {
            if let Some(k) = SHIFTED.iter().position(|&b| u32::from(b) == c) {
                (0x30 + k as u32, 1)
            } else if (1..=26).contains(&c) {
                (0x40 + c, 2) // Ctrl+letter
            } else {
                let pairs: [(u8, u8, u32); 11] = [
                    (b';', b':', 0xba),
                    (b'=', b'+', 0xbb),
                    (b',', b'<', 0xbc),
                    (b'-', b'_', 0xbd),
                    (b'.', b'>', 0xbe),
                    (b'/', b'?', 0xbf),
                    (b'`', b'~', 0xc0),
                    (b'[', b'{', 0xdb),
                    (b'\\', b'|', 0xdc),
                    (b']', b'}', 0xdd),
                    (b'\'', b'"', 0xde),
                ];
                match pairs.iter().find(|p| u32::from(p.0) == c || u32::from(p.1) == c) {
                    Some(&(plain, _, vk)) => (vk, u32::from(c != u32::from(plain))),
                    None => return -1,
                }
            }
        }
    };
    (shift << 8 | vk) as i16
}

#[unsafe(no_mangle)]
pub extern "system" fn VkKeyScanExW(ch: u16, _layout: usize) -> i16 {
    VkKeyScanW(ch)
}

#[unsafe(no_mangle)]
pub extern "system" fn VkKeyScanA(ch: u8) -> i16 {
    VkKeyScanW(u16::from(ch))
}

/// MapVirtualKeyW: MAPVK_VK_TO_VSC (0), MAPVK_VSC_TO_VK (1, 3),
/// MAPVK_VK_TO_CHAR (2: the unshifted character, letters upper-case);
/// 0 when there is none.
#[unsafe(no_mangle)]
pub extern "system" fn MapVirtualKeyW(code: u32, kind: u32) -> u32 {
    match kind {
        0 | 4 => scan_of(code),
        1 | 3 => (0x08..=0xde)
            .find(|&vk| scan_of(vk) == code && !(0x21..=0x2e).contains(&vk))
            .unwrap_or(0),
        2 => match char_of(code) {
            c @ 0x61..=0x7a => c - 32,
            c => c,
        },
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn MapVirtualKeyExW(code: u32, kind: u32, _layout: usize) -> u32 {
    MapVirtualKeyW(code, kind)
}

#[unsafe(no_mangle)]
pub extern "system" fn MapVirtualKeyA(code: u32, kind: u32) -> u32 {
    MapVirtualKeyW(code, kind)
}

/// GetKeyboardLayout: US English (0x04090409).
#[unsafe(no_mangle)]
pub extern "system" fn GetKeyboardLayout(_thread: u32) -> usize {
    US
}

/// The US layout's handle (its HKL: language and device both 0x0409).
const US: usize = 0x0409_0409;

/// LoadKeyboardLayoutW: the layout named by its KLID (eight hex digits).
/// Only US English (00000409) is there: it is the thread's layout already
/// (KLF_ACTIVATE changes nothing); any other is recorded and fails (NULL,
/// ERROR_FILE_NOT_FOUND: no layout DLL).
#[unsafe(no_mangle)]
pub extern "system" fn LoadKeyboardLayoutW(klid: *const u16, _flags: u32) -> usize {
    let mut id = 0u32;
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string, read up to 9 characters.
    while n < 9 && !klid.is_null() && unsafe { *klid.add(n) } != 0 {
        // SAFETY: as above.
        let c = unsafe { *klid.add(n) };
        let Some(d) = char::from_u32(u32::from(c)).and_then(|c| c.to_digit(16)) else {
            id = u32::MAX;
            break;
        };
        id = id.wrapping_shl(4) | d;
        n += 1;
    }
    if n == 8 && id == 0x0000_0409 {
        return US;
    }
    crate::window::unsupported(b"user32.dll!LoadKeyboardLayoutW(layout)", u64::from(id));
    crate::window::set_last_error(2);
    0
}

const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_MENU: usize = 0x12;
const VK_CAPITAL: usize = 0x14;

/// The character of `vk` with Shift (0: none).
fn shifted_of(vk: u32) -> u32 {
    const DIGITS: &[u8; 10] = b")!@#$%^&*(";
    match vk {
        0x41..=0x5a => vk,
        0x30..=0x39 => u32::from(DIGITS[(vk - 0x30) as usize]),
        0xba => u32::from(b':'),
        0xbb => u32::from(b'+'),
        0xbc => u32::from(b'<'),
        0xbd => u32::from(b'_'),
        0xbe => u32::from(b'>'),
        0xbf => u32::from(b'?'),
        0xc0 => u32::from(b'~'),
        0xdb => u32::from(b'{'),
        0xdc => u32::from(b'|'),
        0xdd => u32::from(b'}'),
        0xde => u32::from(b'"'),
        _ => char_of(vk),
    }
}

/// The keypad's characters (NumLock's digits and the operators).
fn keypad_of(vk: u32) -> u32 {
    match vk {
        0x60..=0x69 => vk - 0x60 + u32::from(b'0'),
        0x6a => u32::from(b'*'),
        0x6b => u32::from(b'+'),
        0x6d => u32::from(b'-'),
        0x6e => u32::from(b'.'),
        0x6f => u32::from(b'/'),
        _ => 0,
    }
}

/// ToUnicodeEx: the character `vk` types in the US layout with the key
/// state (high bit: down; VK_CAPITAL's low bit: Caps Lock on): letters
/// follow Shift and Caps Lock, the other keys Shift; Ctrl with a letter
/// gives its control character, Ctrl+[ \ ] ESC FS GS, Ctrl+Enter LF,
/// Ctrl+Backspace DEL; Ctrl+Alt (AltGr: none in US) and keys without a
/// character give 0. No dead keys. 1 with the character written, 0 else.
#[unsafe(no_mangle)]
pub extern "system" fn ToUnicodeEx(
    vk: u32,
    _scan: u32,
    state: *const u8,
    buf: *mut u16,
    len: i32,
    _flags: u32,
    _layout: usize,
) -> i32 {
    if state.is_null() {
        return 0;
    }
    // SAFETY: the caller's 256-byte key state.
    let down = |k: usize| unsafe { *state.add(k) } & 0x80 != 0;
    // SAFETY: as above.
    let caps = unsafe { *state.add(VK_CAPITAL) } & 1 != 0;
    let (shift, ctrl, alt) = (down(VK_SHIFT), down(VK_CONTROL), down(VK_MENU));
    let letter = (0x41..=0x5a).contains(&vk);
    let c = if ctrl && alt {
        0
    } else if ctrl {
        match vk {
            0x41..=0x5a => vk - 0x40,
            0xdb => 0x1b,
            0xdc => 0x1c,
            0xdd => 0x1d,
            0x0d => 0x0a,
            0x08 => 0x7f,
            0x20 => 0x20,
            _ => 0,
        }
    } else if letter {
        if shift != caps { vk } else { vk + 32 }
    } else if shift {
        shifted_of(vk)
    } else {
        match keypad_of(vk) {
            0 => char_of(vk),
            k => k,
        }
    };
    if c == 0 || len < 1 || buf.is_null() {
        return 0;
    }
    // SAFETY: the caller's buffer of `len` characters.
    unsafe {
        *buf = c as u16;
        if len > 1 {
            *buf.add(1) = 0;
        }
    }
    1
}

/// ToUnicode: ToUnicodeEx with the thread's layout.
#[unsafe(no_mangle)]
pub extern "system" fn ToUnicode(
    vk: u32,
    scan: u32,
    state: *const u8,
    buf: *mut u16,
    len: i32,
    flags: u32,
) -> i32 {
    ToUnicodeEx(vk, scan, state, buf, len, flags, US)
}

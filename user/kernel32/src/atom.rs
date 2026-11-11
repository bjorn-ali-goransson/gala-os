//! The atom tables, as documented: the global one (GlobalAddAtomW/A,
//! GlobalFindAtomW/A, GlobalDeleteAtom, GlobalGetAtomNameW/A) and the
//! process's local one (AddAtomW/A, FindAtomW/A, DeleteAtom,
//! GetAtomNameW/A, InitAtomTable). An integer atom (MAKEINTATOM, or a
//! "#1234" string) is its own value (1..0xBFFF); a string atom is 0xC000
//! plus its slot, found case-insensitively, counted by references (each add
//! one, each delete one; the last delete frees it). GetAtomName copies as
//! much of the name as fits with its NUL (Wine's atom.c tests record that
//! of Windows' local table) and returns the characters copied. ANSI names
//! are the ANSI code page's (UTF-8 in gala). Delphi's VCL makes its
//! window-property atoms with the global table as it starts; Office's
//! React Native UI (OfficeC2RClient.exe) imports the local functions.
//!
//! Limit: the global table is the process's own too; Windows keeps one
//! per session (NtAddAtom), so another process does not see its atoms
//! (TODO: the session's table in the kernel, ROADMAP AG). The local table
//! has the same fixed number of slots whatever InitAtomTable asks.

use super::*;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const SLOTS: usize = 128;
const MAX_NAME: usize = 255;
const STRING_BASE: u16 = 0xC000;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

#[derive(Clone, Copy)]
struct Slot {
    refs: u32,
    len: u16,
    name: [u16; MAX_NAME],
}

struct Table {
    lock: AtomicBool,
    slots: UnsafeCell<[Slot; SLOTS]>,
    /// The ledger's name when the table is full.
    full: &'static [u8],
}

// SAFETY: `slots` is only used under `lock`.
unsafe impl Sync for Table {}

const EMPTY: Slot = Slot { refs: 0, len: 0, name: [0; MAX_NAME] };

static GLOBAL: Table = Table {
    lock: AtomicBool::new(false),
    slots: UnsafeCell::new([EMPTY; SLOTS]),
    full: b"kernel32.dll!GlobalAddAtomW(table full)",
};

static LOCAL: Table = Table {
    lock: AtomicBool::new(false),
    slots: UnsafeCell::new([EMPTY; SLOTS]),
    full: b"kernel32.dll!AddAtomW(table full)",
};

impl Table {
    fn with<R>(&self, f: impl FnOnce(&mut [Slot; SLOTS]) -> R) -> R {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: under the lock.
        let r = f(unsafe { &mut *self.slots.get() });
        self.lock.store(false, Ordering::Release);
        r
    }

    /// AddAtom: the atom of `name`, added (or its count raised).
    fn add(&self, name: Option<Name<'_>>) -> u16 {
        match name {
            None => fail_with(ERROR_INVALID_PARAMETER, 0),
            Some(Name::Integer(v)) => v,
            Some(Name::Text(text)) => {
                let atom = self.with(|t| {
                    if let Some(i) = t.iter().position(|s| same(s, text)) {
                        t[i].refs += 1;
                        return Some(i);
                    }
                    let i = t.iter().position(|s| s.refs == 0)?;
                    t[i].refs = 1;
                    t[i].len = text.len() as u16;
                    t[i].name[..text.len()].copy_from_slice(text);
                    Some(i)
                });
                match atom {
                    Some(i) => STRING_BASE + i as u16,
                    None => {
                        unsupported(self.full, SLOTS as u64);
                        fail_with(ERROR_NOT_ENOUGH_MEMORY, 0)
                    }
                }
            }
        }
    }

    /// FindAtom: the atom of `name`, or 0 (ERROR_FILE_NOT_FOUND).
    fn find(&self, name: Option<Name<'_>>) -> u16 {
        match name {
            None => fail_with(ERROR_INVALID_PARAMETER, 0),
            Some(Name::Integer(v)) => v,
            Some(Name::Text(text)) => match self.with(|t| t.iter().position(|s| same(s, text))) {
                Some(i) => STRING_BASE + i as u16,
                None => fail_with(ERROR_FILE_NOT_FOUND, 0),
            },
        }
    }

    /// DeleteAtom: one reference less (an integer atom: nothing); 0 on
    /// success, the atom itself if it is not in the table.
    fn delete(&self, atom: u16) -> u16 {
        if atom < STRING_BASE {
            return 0;
        }
        let i = usize::from(atom - STRING_BASE);
        let ok = self.with(|t| match t.get_mut(i) {
            Some(s) if s.refs > 0 => {
                s.refs -= 1;
                true
            }
            _ => false,
        });
        if ok { 0 } else { fail_with(ERROR_INVALID_HANDLE, atom) }
    }

    /// The atom's name ("#<n>" for an integer atom) into `text`; its
    /// length, or None (last error set).
    fn name(&self, atom: u16, text: &mut [u16; MAX_NAME]) -> Option<usize> {
        if atom == 0 {
            return fail_with(ERROR_INVALID_PARAMETER, None);
        }
        if atom < STRING_BASE {
            let mut digits = [0u8; 6];
            let mut v = atom;
            let mut k = digits.len();
            loop {
                k -= 1;
                digits[k] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            text[0] = u16::from(b'#');
            for (d, &c) in text[1..].iter_mut().zip(&digits[k..]) {
                *d = c.into();
            }
            return Some(1 + digits.len() - k);
        }
        let i = usize::from(atom - STRING_BASE);
        let found = self.with(|t| {
            t.get(i).filter(|s| s.refs > 0).map(|s| {
                text[..usize::from(s.len)].copy_from_slice(&s.name[..usize::from(s.len)]);
                usize::from(s.len)
            })
        });
        match found {
            Some(n) => Some(n),
            None => fail_with(ERROR_INVALID_HANDLE, None),
        }
    }

    /// GetAtomNameW: as much of the name as fits in `size` characters
    /// with its NUL; the characters copied, or 0.
    fn name_w(&self, atom: u16, buf: *mut u16, size: i32) -> u32 {
        if buf.is_null() || size <= 0 {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        let mut text = [0u16; MAX_NAME];
        let Some(n) = self.name(atom, &mut text) else { return 0 };
        let n = n.min(size as usize - 1);
        // SAFETY: the caller's buffer of `size` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
            *buf.add(n) = 0;
        }
        n as u32
    }

    /// GetAtomNameA: the same in the ANSI code page, `size` bytes.
    fn name_a(&self, atom: u16, buf: *mut u8, size: i32) -> u32 {
        if buf.is_null() || size <= 0 {
            return fail_with(ERROR_INVALID_PARAMETER, 0);
        }
        let mut text = [0u16; MAX_NAME];
        let Some(n) = self.name(atom, &mut text) else { return 0 };
        let mut bytes = [0u8; 3 * MAX_NAME];
        let mut len = 0;
        for c in char::decode_utf16(text[..n].iter().copied()) {
            let mut b = [0u8; 4];
            let s = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
            bytes[len..len + s.len()].copy_from_slice(s.as_bytes());
            len += s.len();
        }
        let len = len.min(size as usize - 1);
        // SAFETY: the caller's buffer of `size` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
            *buf.add(len) = 0;
        }
        len as u32
    }
}

/// What an atom argument names.
enum Name<'a> {
    Integer(u16),
    Text(&'a [u16]),
}

/// MAKEINTATOM or "#<decimal>" (an integer atom), else the string;
/// None if invalid (0, too large, too long).
fn parse(text: &[u16]) -> Option<Name<'_>> {
    let digits = text.get(1..).unwrap_or(&[]);
    if text.first() == Some(&u16::from(b'#'))
        && !digits.is_empty()
        && digits.iter().all(|&c| (u16::from(b'0')..=u16::from(b'9')).contains(&c))
    {
        let mut v: u32 = 0;
        for &c in digits {
            v = v.saturating_mul(10).saturating_add(u32::from(c - u16::from(b'0')));
        }
        return (1..u32::from(STRING_BASE)).contains(&v).then_some(Name::Integer(v as u16));
    }
    (!text.is_empty() && text.len() <= MAX_NAME).then_some(Name::Text(text))
}

/// An integer atom passed as a pointer (MAKEINTATOM), if it is one.
fn integer(p: usize) -> Option<Option<Name<'static>>> {
    if p >> 16 != 0 {
        return None;
    }
    let v = p as u16;
    Some((1..STRING_BASE).contains(&v).then_some(Name::Integer(v)))
}

fn parse_w<'a>(name: *const u16) -> Option<Name<'a>> {
    if name.is_null() {
        return None;
    }
    if let Some(n) = integer(name as usize) {
        return n;
    }
    parse(wide_slice(name))
}

/// An ANSI name widened into `buf`, then parsed.
fn parse_a(name: *const u8, buf: &mut [u16; MAX_NAME + 1]) -> Option<Name<'_>> {
    if name.is_null() {
        return None;
    }
    if let Some(n) = integer(name as usize) {
        return n;
    }
    let n = super::sync::ansi_to_wide(name, buf)?;
    parse(&buf[..n])
}

fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn same(slot: &Slot, text: &[u16]) -> bool {
    slot.refs > 0
        && usize::from(slot.len) == text.len()
        && slot.name[..text.len()].iter().zip(text).all(|(&a, &b)| fold(a) == fold(b))
}

// ---- The global table -------------------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn GlobalAddAtomW(name: *const u16) -> u16 {
    GLOBAL.add(parse_w(name))
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalAddAtomA(name: *const u8) -> u16 {
    let mut buf = [0u16; MAX_NAME + 1];
    GLOBAL.add(parse_a(name, &mut buf))
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalFindAtomW(name: *const u16) -> u16 {
    GLOBAL.find(parse_w(name))
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalFindAtomA(name: *const u8) -> u16 {
    let mut buf = [0u16; MAX_NAME + 1];
    GLOBAL.find(parse_a(name, &mut buf))
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalDeleteAtom(atom: u16) -> u16 {
    GLOBAL.delete(atom)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalGetAtomNameW(atom: u16, buf: *mut u16, size: i32) -> u32 {
    GLOBAL.name_w(atom, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GlobalGetAtomNameA(atom: u16, buf: *mut u8, size: i32) -> u32 {
    GLOBAL.name_a(atom, buf, size)
}

// ---- The process's local table --------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn AddAtomW(name: *const u16) -> u16 {
    LOCAL.add(parse_w(name))
}

#[unsafe(no_mangle)]
pub extern "system" fn AddAtomA(name: *const u8) -> u16 {
    let mut buf = [0u16; MAX_NAME + 1];
    LOCAL.add(parse_a(name, &mut buf))
}

#[unsafe(no_mangle)]
pub extern "system" fn FindAtomW(name: *const u16) -> u16 {
    LOCAL.find(parse_w(name))
}

#[unsafe(no_mangle)]
pub extern "system" fn FindAtomA(name: *const u8) -> u16 {
    let mut buf = [0u16; MAX_NAME + 1];
    LOCAL.find(parse_a(name, &mut buf))
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteAtom(atom: u16) -> u16 {
    LOCAL.delete(atom)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetAtomNameW(atom: u16, buf: *mut u16, size: i32) -> u32 {
    LOCAL.name_w(atom, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetAtomNameA(atom: u16, buf: *mut u8, size: i32) -> u32 {
    LOCAL.name_a(atom, buf, size)
}

/// InitAtomTable(nSize): the local table's hash size, which gala's
/// fixed table does not need; TRUE as documented.
#[unsafe(no_mangle)]
pub extern "system" fn InitAtomTable(_size: u32) -> Bool {
    1
}

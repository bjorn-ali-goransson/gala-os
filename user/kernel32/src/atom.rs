//! The global atom table (GlobalAddAtomW, GlobalFindAtomW,
//! GlobalDeleteAtom, GlobalGetAtomNameW), as documented: an integer atom
//! (MAKEINTATOM, or a "#1234" string) is its own value (1..0xBFFF);
//! a string atom is 0xC000 + its slot, found case-insensitively, counted
//! by references (each add one, each delete one; the last delete frees
//! it). Delphi's VCL makes its window-property atoms with it as it
//! starts.
//!
//! Limit: the table is the process's own; Windows keeps one per session
//! (NtAddAtom), so another process does not see these atoms (TODO: the
//! session's table in the kernel, ROADMAP AG).

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
}

// SAFETY: `slots` is only used under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    slots: UnsafeCell::new([Slot { refs: 0, len: 0, name: [0; MAX_NAME] }; SLOTS]),
};

fn with_table<R>(f: impl FnOnce(&mut [Slot; SLOTS]) -> R) -> R {
    while TABLE
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.slots.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// What an atom argument names.
enum Name<'a> {
    Integer(u16),
    Text(&'a [u16]),
}

/// MAKEINTATOM or "#<decimal>" (an integer atom), else the string;
/// None if invalid (0, too large, too long).
fn parse<'a>(name: *const u16) -> Option<Name<'a>> {
    if name.is_null() {
        return None;
    }
    if (name as usize) >> 16 == 0 {
        let v = name as usize as u16;
        return (1..STRING_BASE).contains(&v).then_some(Name::Integer(v));
    }
    let text = wide_slice(name);
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

fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn same(slot: &Slot, text: &[u16]) -> bool {
    slot.refs > 0
        && usize::from(slot.len) == text.len()
        && slot.name[..text.len()].iter().zip(text).all(|(&a, &b)| fold(a) == fold(b))
}

/// GlobalAddAtomW: the atom of `name`, added (or its count raised).
#[unsafe(no_mangle)]
pub extern "system" fn GlobalAddAtomW(name: *const u16) -> u16 {
    match parse(name) {
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
        Some(Name::Integer(v)) => v,
        Some(Name::Text(text)) => {
            let atom = with_table(|t| {
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
                    unsupported(b"kernel32.dll!GlobalAddAtomW(table full)", SLOTS as u64);
                    fail_with(ERROR_NOT_ENOUGH_MEMORY, 0)
                }
            }
        }
    }
}

/// GlobalFindAtomW: the atom of `name`, or 0 (ERROR_FILE_NOT_FOUND).
#[unsafe(no_mangle)]
pub extern "system" fn GlobalFindAtomW(name: *const u16) -> u16 {
    match parse(name) {
        None => fail_with(ERROR_INVALID_PARAMETER, 0),
        Some(Name::Integer(v)) => v,
        Some(Name::Text(text)) => match with_table(|t| t.iter().position(|s| same(s, text))) {
            Some(i) => STRING_BASE + i as u16,
            None => fail_with(ERROR_FILE_NOT_FOUND, 0),
        },
    }
}

/// GlobalDeleteAtom: one reference less (an integer atom: nothing); 0 on
/// success, the atom itself if it is not in the table.
#[unsafe(no_mangle)]
pub extern "system" fn GlobalDeleteAtom(atom: u16) -> u16 {
    if atom < STRING_BASE {
        return 0;
    }
    let i = usize::from(atom - STRING_BASE);
    let ok = with_table(|t| match t.get_mut(i) {
        Some(s) if s.refs > 0 => {
            s.refs -= 1;
            true
        }
        _ => false,
    });
    if ok { 0 } else { fail_with(ERROR_INVALID_HANDLE, atom) }
}

/// GlobalGetAtomNameW: the atom's name ("#<n>" for an integer atom) in
/// `buf` of `size` characters, truncated to fit; its length, or 0.
#[unsafe(no_mangle)]
pub extern "system" fn GlobalGetAtomNameW(atom: u16, buf: *mut u16, size: i32) -> u32 {
    if buf.is_null() || size <= 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut text = [0u16; MAX_NAME];
    let n = if atom == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    } else if atom < STRING_BASE {
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
        1 + digits.len() - k
    } else {
        let i = usize::from(atom - STRING_BASE);
        let found = with_table(|t| {
            t.get(i).filter(|s| s.refs > 0).map(|s| {
                text[..usize::from(s.len)].copy_from_slice(&s.name[..usize::from(s.len)]);
                usize::from(s.len)
            })
        });
        match found {
            Some(n) => n,
            None => return fail_with(ERROR_INVALID_HANDLE, 0),
        }
    };
    let n = n.min(size as usize - 1);
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
    n as u32
}

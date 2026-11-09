//! The UCRT's locale interface. gala's C runtime has one locale, "C",
//! with UTF-8 as the multibyte code page (gala's ANSI code page): these
//! answer for it. `setlocale` (msvcrt's) names "C" whatever is asked;
//! locale objects (`_create_locale`) are that one locale. What a program
//! asks beyond it is recorded in the ledger.

use crate::Lock;
use crate::wide::classes;
use core::sync::atomic::{AtomicI32, Ordering};

/// `_pctype`'s table: the "C" locale's class bits of EOF (-1, index 0)
/// and the bytes 0-255 (none above 127), built on first use.
static mut PCTYPE: [u16; 257] = [0; 257];
static mut WCTYPE: [u16; 256] = [0; 256];
static mut BUILT: bool = false;
static BUILD: Lock = Lock::new();

fn tables() {
    BUILD.with(|| {
        // SAFETY: our statics, written once under BUILD.
        unsafe {
            if BUILT {
                return;
            }
            for c in 0..128u16 {
                PCTYPE[1 + c as usize] = classes(c);
            }
            for c in 0..256u16 {
                WCTYPE[c as usize] = classes(c);
            }
            BUILT = true;
        }
    });
}

/// `__pctype_func()`: the class table indexed by a character (-1 to 255).
#[unsafe(no_mangle)]
pub extern "C" fn __pctype_func() -> *const u16 {
    tables();
    // SAFETY: inside the table; index -1 is EOF's entry.
    unsafe { (&raw const PCTYPE).cast::<u16>().add(1) }
}

/// `__pwctype_func()`: the wide class table for 0-255.
#[unsafe(no_mangle)]
pub extern "C" fn __pwctype_func() -> *const u16 {
    tables();
    (&raw const WCTYPE).cast()
}

/// The "C" locale's names: six NULLs (LC_ALL to LC_TIME).
static LOCALE_NAMES: [usize; 6] = [0; 6];

/// `___lc_locale_name_func()`: the locale names by category (NULL: "C").
#[unsafe(no_mangle)]
pub extern "C" fn ___lc_locale_name_func() -> *const usize {
    LOCALE_NAMES.as_ptr()
}

/// `___lc_collate_cp_func()`: the collation code page ("C": 0).
#[unsafe(no_mangle)]
pub extern "C" fn ___lc_collate_cp_func() -> u32 {
    0
}

/// `__initialize_lconv_for_unsigned_char()`: lconv's char fields for
/// /J programs (unsigned char): already CHAR_MAX-free here; 0.
#[unsafe(no_mangle)]
pub extern "C" fn __initialize_lconv_for_unsigned_char() -> i32 {
    0
}

static THREAD_LOCALE: AtomicI32 = AtomicI32::new(2);

/// `_configthreadlocale(mode)`: whether setlocale changes the thread's
/// locale (_ENABLE_PER_THREAD_LOCALE, 1) or the process's
/// (_DISABLE_PER_THREAD_LOCALE, 2, the default); 0 queries, -1 resets.
/// The previous setting. (Kept per process: with one locale both mean
/// the same.)
#[unsafe(no_mangle)]
pub extern "C" fn _configthreadlocale(mode: i32) -> i32 {
    match mode {
        0 => THREAD_LOCALE.load(Ordering::Relaxed),
        1 | 2 => THREAD_LOCALE.swap(mode, Ordering::Relaxed),
        -1 => THREAD_LOCALE.swap(2, Ordering::Relaxed),
        _ => {
            super::invalid_parameter();
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _lock_locales() {}

#[unsafe(no_mangle)]
pub extern "C" fn _unlock_locales() {}

/// A `_locale_t`: the "C" locale's (two pointers, as `__crt_locale_pointers`).
#[repr(C)]
pub struct LocaleT {
    info: usize,
    mb: usize,
}

static C_LOCALE: LocaleT = LocaleT { info: 0, mb: 0 };

/// `_create_locale(category, name)`: the "C" locale for "C" (and ""
/// recorded: the user's locale is "C" here too).
#[unsafe(no_mangle)]
pub extern "C" fn _create_locale(_category: i32, name: *const u8) -> *const LocaleT {
    // SAFETY: the caller's NUL-terminated name (or NULL).
    let n = if name.is_null() { &b""[..] } else { unsafe { crate::c_str(name) } };
    if n != b"C" {
        crate::missing(b"ucrtbase.dll!_create_locale(not C)", n.len() as u64);
    }
    &C_LOCALE
}

/// `_wcreate_locale(category, name)`.
#[unsafe(no_mangle)]
pub extern "C" fn _wcreate_locale(category: i32, _name: *const u16) -> *const LocaleT {
    _create_locale(category, b"C\0".as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_current_locale() -> *const LocaleT {
    &C_LOCALE
}

#[unsafe(no_mangle)]
pub extern "C" fn _free_locale(_locale: *const LocaleT) {}

/// `_wsetlocale(category, name)`: "C", as `setlocale`.
#[unsafe(no_mangle)]
pub extern "C" fn _wsetlocale(_category: i32, _name: *const u16) -> *const u16 {
    static C: [u16; 2] = [b'C' as u16, 0];
    C.as_ptr()
}

/// `___lc_codepage_func`'s companion `__acrt_get_qualified_locale` is not
/// exported; `_getmbcp()`: the multibyte code page (UTF-8, 65001).
#[unsafe(no_mangle)]
pub extern "C" fn _getmbcp() -> i32 {
    65001
}

//! gala's resutils.dll: the cluster resource utility functions programs
//! resolve at run time (SQL Server): looking properties up in a property
//! list (the documented CLUSPROP format: a count, then per property a
//! CLUSPROP_SYNTAX_NAME entry, its values and CLUSPROP_SYNTAX_ENDMARK;
//! every entry is {Syntax, cbLength, data padded to 4 bytes}) and
//! comparing resource type names.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, size: usize) -> *mut u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;

const SYNTAX_NAME: u32 = 0x0004_0003;
const SYNTAX_ENDMARK: u32 = 0;
const FORMAT_DWORD: u32 = 2;
const FORMAT_SZ: u32 = 3;
const FORMAT_EXPAND_SZ: u32 = 4;
const FORMAT_MULTI_SZ: u32 = 5;
const FORMAT_LONG: u32 = 7;
const FORMAT_EXPANDED_SZ: u32 = 8;

/// ASCII case folding for property names.
fn fold(c: u16) -> u16 {
    if (0x61..=0x7a).contains(&c) { c - 32 } else { c }
}

/// The NUL-terminated string at `p` (empty for null).
fn wide<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// The first value (syntax, data) of property `name` in the list.
fn find(list: *const u8, size: u32, name: *const u16) -> Result<(u32, &'static [u8]), u32> {
    if list.is_null() || name.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's property list of `size` bytes.
    let list: &[u8] = unsafe { core::slice::from_raw_parts(list, size as usize) };
    let word = |at: usize| -> Result<u32, u32> {
        let b = list.get(at..at + 4).ok_or(ERROR_INVALID_DATA)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // One entry: (syntax, data, the offset after it).
    let entry = |at: usize| -> Result<(u32, &[u8], usize), u32> {
        let (syntax, length) = (word(at)?, word(at + 4)? as usize);
        let data = list.get(at + 8..at + 8 + length).ok_or(ERROR_INVALID_DATA)?;
        Ok((syntax, data, at + 8 + length.div_ceil(4) * 4))
    };
    let want = wide(name);
    let count = word(0)?;
    let mut at = 4;
    for _ in 0..count {
        let (syntax, text, next) = entry(at)?;
        if syntax != SYNTAX_NAME {
            return Err(ERROR_INVALID_DATA);
        }
        let units = text.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        let same = units.take_while(|&c| c != 0).map(fold).eq(want.iter().map(|&c| fold(c)));
        at = next;
        let (syntax, data, _) = entry(at)?;
        if same {
            return Ok((syntax, data));
        }
        // Skip the values up to the end mark.
        loop {
            let (syntax, _, next) = entry(at)?;
            at = next;
            if syntax == SYNTAX_ENDMARK {
                break;
            }
        }
    }
    Err(ERROR_FILE_NOT_FOUND)
}

/// ResUtilFindDwordProperty: the DWORD value of property `name`.
#[unsafe(no_mangle)]
pub extern "system" fn ResUtilFindDwordProperty(
    list: *const u8,
    size: u32,
    name: *const u16,
    out: *mut u32,
) -> u32 {
    match find(list, size, name) {
        Ok((syntax, data)) if matches!(syntax & 0xffff, FORMAT_DWORD | FORMAT_LONG) => {
            let Some(b) = data.get(..4) else { return ERROR_INVALID_DATA };
            if !out.is_null() {
                // SAFETY: the caller's output.
                unsafe { out.write(u32::from_le_bytes([b[0], b[1], b[2], b[3]])) };
            }
            ERROR_SUCCESS
        }
        Ok(_) => ERROR_INVALID_DATA,
        Err(e) => e,
    }
}

/// Copies `data` (UTF-16 bytes) into LocalAlloc'ed memory, NUL-terminated
/// twice (enough for a MULTI_SZ too).
fn copy_out(data: &[u8], out: *mut *mut u16) -> u32 {
    if out.is_null() {
        return ERROR_SUCCESS;
    }
    // SAFETY: plain allocation (zeroed: LMEM_ZEROINIT).
    let p = unsafe { LocalAlloc(0x40, data.len() + 4) };
    if p.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: `p` has room for the data; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        out.write(p.cast());
    }
    ERROR_SUCCESS
}

/// ResUtilFindSzProperty: a copy (LocalAlloc) of the string value.
#[unsafe(no_mangle)]
pub extern "system" fn ResUtilFindSzProperty(
    list: *const u8,
    size: u32,
    name: *const u16,
    out: *mut *mut u16,
) -> u32 {
    match find(list, size, name) {
        Ok((syntax, data))
            if matches!(syntax & 0xffff, FORMAT_SZ | FORMAT_EXPAND_SZ | FORMAT_EXPANDED_SZ) =>
        {
            copy_out(data, out)
        }
        Ok(_) => ERROR_INVALID_DATA,
        Err(e) => e,
    }
}

/// ResUtilFindMultiSzProperty: a copy (LocalAlloc) of the MULTI_SZ value
/// and its size in bytes.
#[unsafe(no_mangle)]
pub extern "system" fn ResUtilFindMultiSzProperty(
    list: *const u8,
    size: u32,
    name: *const u16,
    out: *mut *mut u16,
    out_size: *mut u32,
) -> u32 {
    match find(list, size, name) {
        Ok((syntax, data)) if syntax & 0xffff == FORMAT_MULTI_SZ => {
            if !out_size.is_null() {
                // SAFETY: the caller's output.
                unsafe { out_size.write(data.len() as u32) };
            }
            copy_out(data, out)
        }
        Ok(_) => ERROR_INVALID_DATA,
        Err(e) => e,
    }
}

/// ResUtilResourceTypesEqual: resource type names compared ignoring case.
#[unsafe(no_mangle)]
pub extern "system" fn ResUtilResourceTypesEqual(a: *const u16, b: *const u16) -> i32 {
    let (a, b) = (wide(a), wide(b));
    i32::from(a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y)))
}

/// DllMain (the entry point our link flags name).
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

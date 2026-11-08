//! NeedCurrentDirectoryForExePathW. (The PathCch functions are
//! kernelbase's alone: user/kernelbase/src/path.rs.)

use super::*;

const SLASH: u16 = b'\\' as u16;

/// The characters of the NUL-terminated `p` (empty for null), at most
/// 0x8000.
fn chars<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < 0x8000 && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` characters were just read.
    unsafe { core::slice::from_raw_parts(p, n) }
}

/// NeedCurrentDirectoryForExePathW(ExeName): whether the current directory
/// is searched for it: always for a name with a backslash; otherwise
/// unless NoDefaultCurrentDirectoryInExePath is set (to anything).
#[unsafe(no_mangle)]
pub extern "system" fn NeedCurrentDirectoryForExePathW(name: *const u16) -> Bool {
    const ERROR_ENVVAR_NOT_FOUND: u32 = 203;
    if chars(name).contains(&SLASH) {
        return TRUE;
    }
    let var: [u16; 35] = {
        let s = b"NoDefaultCurrentDirectoryInExePath\0";
        let mut w = [0u16; 35];
        for (d, &c) in w.iter_mut().zip(s.iter()) {
            *d = u16::from(c);
        }
        w
    };
    SetLastError(0);
    let n = GetEnvironmentVariableW(var.as_ptr(), core::ptr::null_mut(), 0);
    let set = n != 0 || GetLastError() != ERROR_ENVVAR_NOT_FOUND;
    if set { FALSE } else { TRUE }
}

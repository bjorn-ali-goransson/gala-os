//! Console functions beyond output: gala's console is a log (the serial
//! line), not a screen, so its handles are not consoles to programs
//! (GetConsoleMode fails, as for redirected output). The code pages are
//! UTF-8; screen buffers, console input and control events fail as for a
//! non-console handle and are recorded.

use super::*;

const CP_UTF8: u32 = 65001;

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleCP() -> u32 {
    CP_UTF8
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleOutputCP() -> u32 {
    CP_UTF8
}

/// No console window.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleWindow() -> Handle {
    core::ptr::null_mut()
}

/// GetConsoleTitleA: no title (0 characters, error set).
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleTitleA(_title: *mut u8, _size: u32) -> u32 {
    fail_with(ERROR_INVALID_HANDLE, 0)
}

/// Functions that fail on a handle that is not a console.
macro_rules! not_a_console {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> Bool {
            $(let _ = $arg;)*
            unsupported(concat!("kernel32.dll!", stringify!($name)).as_bytes(), 0);
            fail_with(ERROR_INVALID_HANDLE, FALSE)
        }
    )*};
}

not_a_console! {
    GetConsoleScreenBufferInfo(h: Handle, info: *mut u8);
    SetConsoleActiveScreenBuffer(h: Handle);
    FillConsoleOutputAttribute(h: Handle, attribute: u16, length: u32, at: u32, written: *mut u32);
    FillConsoleOutputCharacterA(h: Handle, c: u8, length: u32, at: u32, written: *mut u32);
    GetNumberOfConsoleInputEvents(h: Handle, count: *mut u32);
    PeekConsoleInputW(h: Handle, buffer: *mut u8, length: u32, read: *mut u32);
    ReadConsoleInputA(h: Handle, buffer: *mut u8, length: u32, read: *mut u32);
    ReadConsoleInputW(h: Handle, buffer: *mut u8, length: u32, read: *mut u32);
    GenerateConsoleCtrlEvent(event: u32, group: u32);
}

/// CreateConsoleScreenBuffer: no screen to buffer.
#[unsafe(no_mangle)]
pub extern "system" fn CreateConsoleScreenBuffer(
    _access: u32,
    _share: u32,
    _attributes: *const c_void,
    _flags: u32,
    _data: *mut c_void,
) -> Handle {
    unsupported(b"kernel32.dll!CreateConsoleScreenBuffer", 0);
    fail_with(ERROR_NOT_SUPPORTED, INVALID_HANDLE_VALUE)
}

/// SetConsoleCP/SetConsoleOutputCP: UTF-8 is the only code page.
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleCP(code_page: u32) -> Bool {
    if code_page == CP_UTF8 { TRUE } else { fail_with(ERROR_INVALID_PARAMETER, FALSE) }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleOutputCP(code_page: u32) -> Bool {
    SetConsoleCP(code_page)
}

not_a_console! {
    SetConsoleCursorPosition(h: Handle, at: u32);
    SetConsoleScreenBufferSize(h: Handle, size: u32);
    SetConsoleTextAttribute(h: Handle, attributes: u16);
    SetConsoleTitleA(title: *const u8);
}

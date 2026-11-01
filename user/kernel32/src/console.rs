//! The console. gala's standard handles are the serial line: a character
//! device that is not a console (as redirected output on Windows; GetFileType
//! is FILE_TYPE_CHAR, GetConsoleMode fails). The console itself is reached
//! as CONOUT$ and CONIN$ (CreateFileW), as a program whose output is
//! redirected reaches it on Windows. Its screen buffer has no cells: what
//! is written goes to the serial line (UTF-8), and kernel32 keeps what the
//! console functions report - the modes, a 120-column buffer with a 30-line
//! window that follows the cursor, the cursor (moved by what WriteConsole*
//! and WriteFile write through these handles), text attributes, the title.
//! There is no keyboard: input has no events.
//!
//! Console handles are the ones opened as CONOUT$/CONIN$ (a table here;
//! duplicates of them are not recognized yet, TODO).

use core::sync::atomic::{AtomicBool, Ordering};

use super::*;

const CP_UTF8: u32 = 65001;

/// The width and height of the screen buffer and of its window.
const WIDTH: i16 = 120;
const HEIGHT: i16 = 9001;
const WINDOW_HEIGHT: i16 = 30;

const MAX_HANDLES: usize = 32;

struct State {
    /// Console handles: (handle, is input).
    handles: [(usize, bool); MAX_HANDLES],
    output_mode: u32,
    input_mode: u32,
    cursor: (i16, i16),
    attributes: u16,
    cursor_size: u32,
    cursor_visible: bool,
    title: [u16; 256],
    title_len: usize,
}

struct Shared {
    lock: AtomicBool,
    state: core::cell::UnsafeCell<State>,
}

// SAFETY: `state` is only touched with `lock` held.
unsafe impl Sync for Shared {}

static CONSOLE: Shared = Shared {
    lock: AtomicBool::new(false),
    state: core::cell::UnsafeCell::new(State {
        handles: [(0, false); MAX_HANDLES],
        // ENABLE_PROCESSED_OUTPUT | ENABLE_WRAP_AT_EOL_OUTPUT.
        output_mode: 0x3,
        // PROCESSED_INPUT | LINE_INPUT | ECHO_INPUT | INSERT_MODE |
        // QUICK_EDIT_MODE | EXTENDED_FLAGS | AUTO_POSITION.
        input_mode: 0x1f7,
        cursor: (0, 0),
        attributes: 0x07,
        cursor_size: 25,
        cursor_visible: true,
        title: [0; 256],
        title_len: 0,
    }),
};

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    while CONSOLE
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: the lock is held.
    let r = f(unsafe { &mut *CONSOLE.state.get() });
    CONSOLE.lock.store(false, Ordering::Release);
    r
}

/// Whether `name` is CONOUT$ (Some(false)) or CONIN$ (Some(true)), any case.
pub(crate) fn console_name(name: &[u16]) -> Option<bool> {
    let is = |text: &[u8]| {
        name.len() == text.len()
            && name
                .iter()
                .zip(text)
                .all(|(&c, &t)| u8::try_from(c).is_ok_and(|c| c.eq_ignore_ascii_case(&t)))
    };
    if is(b"CONOUT$") {
        Some(false)
    } else if is(b"CONIN$") {
        Some(true)
    } else {
        None
    }
}

/// Remembers a console handle.
pub(crate) fn register(h: Handle, input: bool) {
    let full = with_state(|s| match s.handles.iter_mut().find(|e| e.0 == 0) {
        Some(slot) => {
            *slot = (h as usize, input);
            false
        }
        None => true,
    });
    if full {
        unsupported(b"kernel32.dll!CreateFileW(too many console handles)", MAX_HANDLES as u64);
    }
}

/// Forgets a closed handle.
pub(crate) fn forget(h: Handle) {
    with_state(|s| {
        if let Some(slot) = s.handles.iter_mut().find(|e| e.0 == h as usize) {
            *slot = (0, false);
        }
    });
}

/// Whether `h` is a console handle, and if so whether it is the input.
pub(crate) fn kind(h: Handle) -> Option<bool> {
    if h.is_null() {
        return None;
    }
    with_state(|s| s.handles.iter().find(|e| e.0 == h as usize).map(|e| e.1))
}

/// For a handle not opened as CONIN$/CONOUT$ here (a standard or inherited
/// handle): whether the kernel says it is a console (FILE_DEVICE_CONSOLE),
/// and then whether it is input (a zero-length write fails). The console
/// functions that write accept such handles; GetConsoleMode keeps saying
/// gala's standard handles are not consoles (redirected output, which the
/// JDK and the Go runtime then treat as such).
fn console_device(h: Handle) -> Option<bool> {
    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtQueryVolumeInformationFile(
            h: Handle,
            iosb: *mut [usize; 2],
            buf: *mut u8,
            len: u32,
            class: u32,
        ) -> u32;
    }
    let mut iosb = [0usize; 2];
    let mut device = [0u32; 2];
    // SAFETY: our buffers (FileFsDeviceInformation, 4).
    let status =
        unsafe { NtQueryVolumeInformationFile(h, &mut iosb, device.as_mut_ptr().cast(), 8, 4) };
    if status != 0 || device[0] != 0x50 {
        return None;
    }
    // SAFETY: a zero-length write from our (unused) buffer.
    let written = unsafe {
        super::NtWriteFile(
            h,
            0,
            0,
            0,
            &mut iosb,
            [0u8; 1].as_ptr(),
            0,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
    Some(written != 0)
}

/// `Some(())` for a console output handle, else the error is set.
fn output(h: Handle) -> Option<()> {
    match kind(h).or_else(|| console_device(h)) {
        Some(false) => Some(()),
        _ => {
            SetLastError(ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// Moves the cursor over `text` (UTF-16 or bytes as code units).
fn advance(units: impl Iterator<Item = u32>) {
    with_state(|s| {
        let newline_returns = s.output_mode & 0x8 == 0; // DISABLE_NEWLINE_AUTO_RETURN
        let (mut x, mut y) = s.cursor;
        for c in units {
            match c {
                0x0d => x = 0,
                0x0a => {
                    y += 1;
                    if newline_returns {
                        x = 0;
                    }
                }
                0x08 => x = (x - 1).max(0),
                0x09 => x = ((x / 8 + 1) * 8).min(WIDTH - 1),
                0x80..=0xbf if c < 0x100 => {} // UTF-8 continuation bytes
                _ => {
                    x += 1;
                    if x >= WIDTH {
                        x = 0;
                        y += 1;
                    }
                }
            }
        }
        s.cursor = (x, y.min(HEIGHT - 1));
    });
}

/// WriteFile through a console output handle moves the cursor too.
pub(crate) fn wrote(h: Handle, bytes: &[u8]) {
    if kind(h) == Some(false) {
        advance(bytes.iter().map(|&b| u32::from(b)));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleMode(h: Handle, mode: *mut u32) -> Bool {
    let Some(input) = kind(h) else { return fail_with(ERROR_INVALID_HANDLE, FALSE) };
    if mode.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let m = with_state(|s| if input { s.input_mode } else { s.output_mode });
    // SAFETY: the caller's output.
    unsafe { *mode = m };
    TRUE
}

/// SetConsoleMode: output modes 0x1f (processed, wrap, virtual terminal,
/// no auto return, LVB grid), input modes 0x3ff.
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleMode(h: Handle, mode: u32) -> Bool {
    let Some(input) = kind(h) else { return fail_with(ERROR_INVALID_HANDLE, FALSE) };
    let valid = if input { 0x3ff } else { 0x1f };
    if mode & !valid != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    with_state(|s| {
        if input {
            s.input_mode = mode;
        } else {
            s.output_mode = mode;
        }
    });
    TRUE
}

/// Writes UTF-8 to the console's device (the serial line).
fn write_utf8(h: Handle, bytes: &[u8]) -> bool {
    let mut iosb = [0usize; 2];
    // SAFETY: our buffer; the handle is a console output handle.
    let status = unsafe {
        NtWriteFile(
            h,
            0,
            0,
            0,
            &mut iosb,
            bytes.as_ptr(),
            bytes.len() as u32,
            core::ptr::null(),
            core::ptr::null(),
        )
    };
    status == STATUS_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn WriteConsoleW(
    h: Handle,
    buffer: *const u16,
    length: u32,
    written: *mut u32,
    _reserved: *mut c_void,
) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    if buffer.is_null() && length > 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's `length` characters.
    let text = if length == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(buffer, length as usize) }
    };
    let mut chunk = [0u8; 512];
    let mut n = 0;
    for c in char::decode_utf16(text.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        if n + 4 > chunk.len() {
            if !write_utf8(h, &chunk[..n]) {
                return fail_with(ERROR_INVALID_HANDLE, FALSE);
            }
            n = 0;
        }
        n += c.encode_utf8(&mut chunk[n..]).len();
    }
    if n > 0 && !write_utf8(h, &chunk[..n]) {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    advance(text.iter().map(|&c| u32::from(c)));
    if !written.is_null() {
        // SAFETY: the caller's output.
        unsafe { *written = length };
    }
    TRUE
}

/// WriteConsoleA: the text is in the console's code page (UTF-8).
#[unsafe(no_mangle)]
pub extern "system" fn WriteConsoleA(
    h: Handle,
    buffer: *const u8,
    length: u32,
    written: *mut u32,
    _reserved: *mut c_void,
) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    // SAFETY: the caller's `length` bytes.
    let text = if length == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(buffer, length as usize) }
    };
    if !write_utf8(h, text) {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    advance(text.iter().map(|&b| u32::from(b)));
    if !written.is_null() {
        // SAFETY: the caller's output.
        unsafe { *written = length };
    }
    TRUE
}

/// The window: 30 lines ending at the cursor's line (or the top 30).
fn window(cursor_y: i16) -> [i16; 4] {
    let top = (cursor_y - (WINDOW_HEIGHT - 1)).max(0);
    [0, top, WIDTH - 1, top + WINDOW_HEIGHT - 1]
}

/// CONSOLE_SCREEN_BUFFER_INFO (22 bytes) at `out`.
fn screen_info(out: *mut u8) {
    let (cursor, attributes) = with_state(|s| (s.cursor, s.attributes));
    let words: [i16; 11] = {
        let w = window(cursor.1);
        [WIDTH, HEIGHT, cursor.0, cursor.1, attributes as i16, w[0], w[1], w[2], w[3], WIDTH, 60]
    };
    for (i, v) in words.iter().enumerate() {
        // SAFETY: the caller's 22-byte structure.
        unsafe { (out.add(2 * i) as *mut i16).write_unaligned(*v) };
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleScreenBufferInfo(h: Handle, info: *mut u8) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    screen_info(info);
    TRUE
}

/// The standard 16-color console palette (COLORREF 0x00BBGGRR).
const PALETTE: [u32; 16] = [
    0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0, 0x808080,
    0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
];

/// GetConsoleScreenBufferInfoEx: CONSOLE_SCREEN_BUFFER_INFOEX (cbSize 96):
/// the same, the popup attributes, not full screen, the palette.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleScreenBufferInfoEx(h: Handle, info: *mut u8) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    // SAFETY: the caller's structure: cbSize first.
    if info.is_null() || unsafe { (info as *const u32).read_unaligned() } != 96 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's 96-byte structure.
    unsafe {
        screen_info(info.add(4));
        (info.add(26) as *mut u16).write_unaligned(0xf5); // wPopupAttributes
        (info.add(28) as *mut u32).write_unaligned(0); // bFullscreenSupported
        for (i, c) in PALETTE.iter().enumerate() {
            (info.add(32 + 4 * i) as *mut u32).write_unaligned(*c);
        }
    }
    TRUE
}

/// SetConsoleScreenBufferInfoEx: the attributes are kept; the size, the
/// window and the palette are fixed here.
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleScreenBufferInfoEx(h: Handle, info: *const u8) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's structure (wAttributes at +12).
    let attributes = unsafe { (info.add(12) as *const u16).read_unaligned() };
    with_state(|s| s.attributes = attributes);
    TRUE
}

/// SetConsoleCursorPosition: COORD (X low, Y high) inside the buffer.
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleCursorPosition(h: Handle, at: u32) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    let (x, y) = (at as u16 as i16, (at >> 16) as u16 as i16);
    if !(0..WIDTH).contains(&x) || !(0..HEIGHT).contains(&y) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    with_state(|s| s.cursor = (x, y));
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleTextAttribute(h: Handle, attributes: u16) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    with_state(|s| s.attributes = attributes);
    TRUE
}

/// CONSOLE_CURSOR_INFO { dwSize, bVisible }.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleCursorInfo(h: Handle, info: *mut u32) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    let (size, visible) = with_state(|s| (s.cursor_size, s.cursor_visible));
    // SAFETY: the caller's 8-byte structure.
    unsafe {
        *info = size;
        *info.add(1) = u32::from(visible);
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleCursorInfo(h: Handle, info: *const u32) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    // SAFETY: the caller's 8-byte structure.
    let (size, visible) = unsafe { (*info, *info.add(1)) };
    if !(1..=100).contains(&size) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    with_state(|s| {
        s.cursor_size = size;
        s.cursor_visible = visible != 0;
    });
    TRUE
}

/// The buffer keeps no cells: filling reports the cells as written.
fn filled(h: Handle, length: u32, written: *mut u32) -> Bool {
    if output(h).is_none() {
        return FALSE;
    }
    if !written.is_null() {
        // SAFETY: the caller's output.
        unsafe { *written = length };
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn FillConsoleOutputCharacterW(
    h: Handle,
    _c: u16,
    length: u32,
    _at: u32,
    written: *mut u32,
) -> Bool {
    filled(h, length, written)
}

#[unsafe(no_mangle)]
pub extern "system" fn FillConsoleOutputCharacterA(
    h: Handle,
    _c: u8,
    length: u32,
    _at: u32,
    written: *mut u32,
) -> Bool {
    filled(h, length, written)
}

#[unsafe(no_mangle)]
pub extern "system" fn FillConsoleOutputAttribute(
    h: Handle,
    _a: u16,
    length: u32,
    _at: u32,
    written: *mut u32,
) -> Bool {
    filled(h, length, written)
}

/// The buffer's size and window are fixed: requests are accepted as long
/// as they are valid shapes.
#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleScreenBufferSize(h: Handle, _size: u32) -> Bool {
    if output(h).is_none() { FALSE } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleWindowInfo(
    h: Handle,
    _absolute: Bool,
    _window: *const i16,
) -> Bool {
    if output(h).is_none() { FALSE } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleActiveScreenBuffer(h: Handle) -> Bool {
    if output(h).is_none() { FALSE } else { TRUE }
}

/// GetLargestConsoleWindowSize: COORD (columns low, rows high).
#[unsafe(no_mangle)]
pub extern "system" fn GetLargestConsoleWindowSize(h: Handle) -> u32 {
    if output(h).is_none() {
        return 0;
    }
    (WIDTH as u32) | (60 << 16)
}

/// `Some(())` for the console input handle, else the error is set.
fn input(h: Handle) -> Option<()> {
    match kind(h) {
        Some(true) => Some(()),
        _ => {
            SetLastError(ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// No keyboard: there are never input events.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumberOfConsoleInputEvents(h: Handle, count: *mut u32) -> Bool {
    if input(h).is_none() {
        return FALSE;
    }
    // SAFETY: the caller's output.
    unsafe { *count = 0 };
    TRUE
}

fn no_events(h: Handle, read: *mut u32) -> Bool {
    if input(h).is_none() {
        return FALSE;
    }
    if !read.is_null() {
        // SAFETY: the caller's output.
        unsafe { *read = 0 };
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn PeekConsoleInputW(
    h: Handle,
    _buffer: *mut u8,
    _length: u32,
    read: *mut u32,
) -> Bool {
    no_events(h, read)
}

#[unsafe(no_mangle)]
pub extern "system" fn PeekConsoleInputA(
    h: Handle,
    _buffer: *mut u8,
    _length: u32,
    read: *mut u32,
) -> Bool {
    no_events(h, read)
}

/// ReadConsoleInput/ReadConsole wait for input on Windows; there is no
/// keyboard here, so they return nothing at once (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn ReadConsoleInputW(
    h: Handle,
    _buffer: *mut u8,
    _length: u32,
    read: *mut u32,
) -> Bool {
    unsupported(b"kernel32.dll!ReadConsoleInputW(no keyboard)", 0);
    no_events(h, read)
}

#[unsafe(no_mangle)]
pub extern "system" fn ReadConsoleInputA(
    h: Handle,
    _buffer: *mut u8,
    _length: u32,
    read: *mut u32,
) -> Bool {
    unsupported(b"kernel32.dll!ReadConsoleInputA(no keyboard)", 0);
    no_events(h, read)
}

#[unsafe(no_mangle)]
pub extern "system" fn ReadConsoleW(
    h: Handle,
    _buf: *mut c_void,
    _len: u32,
    read: *mut u32,
    _control: *mut c_void,
) -> Bool {
    unsupported(b"kernel32.dll!ReadConsoleW(no keyboard)", 0);
    no_events(h, read)
}

#[unsafe(no_mangle)]
pub extern "system" fn ReadConsoleA(
    h: Handle,
    _buf: *mut c_void,
    _len: u32,
    read: *mut u32,
    _control: *mut c_void,
) -> Bool {
    unsupported(b"kernel32.dll!ReadConsoleA(no keyboard)", 0);
    no_events(h, read)
}

#[unsafe(no_mangle)]
pub extern "system" fn FlushConsoleInputBuffer(h: Handle) -> Bool {
    if input(h).is_none() { FALSE } else { TRUE }
}

/// GenerateConsoleCtrlEvent: no console process groups to signal.
#[unsafe(no_mangle)]
pub extern "system" fn GenerateConsoleCtrlEvent(event: u32, _group: u32) -> Bool {
    unsupported(b"kernel32.dll!GenerateConsoleCtrlEvent", event.into());
    fail_with(ERROR_INVALID_HANDLE, FALSE)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleCP() -> u32 {
    CP_UTF8
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleOutputCP() -> u32 {
    CP_UTF8
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

/// No console window.
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleWindow() -> Handle {
    core::ptr::null_mut()
}

/// GetConsoleTitleW: the title (length without the NUL; truncated).
#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleTitleW(title: *mut u16, size: u32) -> u32 {
    with_state(|s| {
        if title.is_null() || size == 0 {
            return s.title_len as u32;
        }
        let n = s.title_len.min(size as usize - 1);
        // SAFETY: the caller's buffer of `size` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(s.title.as_ptr(), title, n);
            *title.add(n) = 0;
        }
        n as u32
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn GetConsoleTitleA(title: *mut u8, size: u32) -> u32 {
    let mut wide = [0u16; 256];
    let n = GetConsoleTitleW(wide.as_mut_ptr(), 256) as usize;
    let mut k = 0;
    for c in char::decode_utf16(wide[..n].iter().copied()) {
        let c = c.unwrap_or('?');
        let mut b = [0u8; 4];
        let bytes = c.encode_utf8(&mut b).as_bytes();
        if title.is_null() || k + bytes.len() + 1 > size as usize {
            break;
        }
        // SAFETY: the caller's buffer of `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), title.add(k), bytes.len()) };
        k += bytes.len();
    }
    if !title.is_null() && size > 0 {
        // SAFETY: as above.
        unsafe { *title.add(k) = 0 };
    }
    k as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleTitleW(title: *const u16) -> Bool {
    if title.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    with_state(|s| {
        let mut n = 0;
        // SAFETY: the caller's NUL-terminated string (up to 255 kept).
        unsafe {
            while n < 255 && *title.add(n) != 0 {
                s.title[n] = *title.add(n);
                n += 1;
            }
        }
        s.title_len = n;
    });
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn SetConsoleTitleA(title: *const u8) -> Bool {
    if title.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let mut wide = [0u16; 256];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string (ASCII kept as is).
    unsafe {
        while n < 255 && *title.add(n) != 0 {
            wide[n] = u16::from(*title.add(n));
            n += 1;
        }
    }
    SetConsoleTitleW(wide.as_ptr())
}

/// CreateConsoleScreenBuffer: one screen buffer only.
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

//! Pseudo consoles (ConPTY), as Microsoft documents them ("Creating a
//! Pseudoconsole session", CreatePseudoConsole, ResizePseudoConsole,
//! ClosePseudoConsole): a console whose host carries what its programs
//! write to the caller's output pipe as UTF-8 text and VT sequences, and
//! turns what the caller writes to the input pipe (text, VT sequences for
//! special keys) into the console's key events. As on Windows the host is
//! a process, `conhost.exe` in the system directory (gala's own,
//! user/conhost, in pseudo-console mode); the console is one of the
//! kernel's console sessions (GalaConsoleCreate, ARCHITECTURE "Pseudo
//! consoles").
//!
//! An HPCON points at a `PseudoConsole` here. Its first field is the
//! session's id: CreateProcess passes it to the kernel for
//! PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE (gala's conhost.exe, starting a
//! program in a console it made itself, passes a block with the same first
//! field).

use super::*;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaConsoleCreate(flags: u32, size: u32, info: *mut [u32; 2]) -> u32;
    fn GalaConsoleSize(id: u32, size: *mut u32, set: u32) -> u32;
    fn GalaConsoleClose(id: u32) -> u32;
}

/// What an HPCON points at.
#[repr(C)]
pub(crate) struct PseudoConsole {
    /// The console session (first: CreateProcess reads it).
    pub(crate) session: u32,
    _reserved: u32,
    /// The write end of the host's signal pipe (closed: the host ends).
    signal: Handle,
    /// The host process.
    host: Handle,
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000eu32 as i32;
/// PSEUDOCONSOLE_INHERIT_CURSOR: the host asks the terminal where its
/// cursor is (ESC [ 6 n) before it starts.
const PSEUDOCONSOLE_INHERIT_CURSOR: u32 = 1;
const HANDLE_FLAG_INHERIT: u32 = 1;
const STARTF_USESTDHANDLES: u32 = 0x100;
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x2_0002;

/// HRESULT_FROM_WIN32 of the thread's last error.
fn last_hresult() -> i32 {
    match GetLastError() {
        0 => E_INVALIDARG,
        e => (0x8007_0000 | (e & 0xffff)) as i32,
    }
}

/// Appends ASCII `text` to `out` at `*at`.
fn put(out: &mut [u16], at: &mut usize, text: &[u8]) {
    for &b in text {
        if *at < out.len() - 1 {
            out[*at] = u16::from(b);
            *at += 1;
        }
    }
}

fn put_number(out: &mut [u16], at: &mut usize, mut n: u64, hex: bool) {
    let base = if hex { 16 } else { 10 };
    let mut digits = [0u8; 20];
    let mut d = 0;
    loop {
        let v = (n % base) as u8;
        digits[d] = if v < 10 { b'0' + v } else { b'a' + v - 10 };
        d += 1;
        n /= base;
        if n == 0 {
            break;
        }
    }
    while d > 0 {
        d -= 1;
        put(out, at, &digits[d..d + 1]);
    }
}

/// CreatePseudoConsole(size, hInput, hOutput, dwFlags, *phPC).
#[unsafe(no_mangle)]
pub extern "system" fn CreatePseudoConsole(
    size: u32,
    input: Handle,
    output: Handle,
    flags: u32,
    pc: *mut usize,
) -> i32 {
    let (cols, rows) = (size as u16 as i16, (size >> 16) as u16 as i16);
    let bad = |h: Handle| h.is_null() || h == INVALID_HANDLE_VALUE;
    if pc.is_null() || cols <= 0 || rows <= 0 || bad(input) || bad(output) {
        return E_INVALIDARG;
    }
    if flags & !PSEUDOCONSOLE_INHERIT_CURSOR != 0 {
        unsupported(b"kernel32.dll!CreatePseudoConsole(flags)", u64::from(flags));
    }
    let mut info = [0u32; 2];
    // SAFETY: our output buffer.
    let status = unsafe { GalaConsoleCreate(1, size, &mut info) };
    if status != STATUS_SUCCESS {
        return fail(status, E_OUTOFMEMORY);
    }
    let session = info[0];
    let closed = |hr: i32| {
        // SAFETY: the session we made.
        unsafe { GalaConsoleClose(session) };
        hr
    };
    // The host's signal pipe: it ends when we close our end.
    let (mut signal_read, mut signal_write) = (core::ptr::null_mut(), core::ptr::null_mut());
    if super::pipe::CreatePipe(&mut signal_read, &mut signal_write, core::ptr::null_mut(), 0)
        == FALSE
    {
        return closed(last_hresult());
    }
    super::pipe::SetHandleInformation(signal_read, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    // "<system>\conhost.exe" --headless --width W --height H --signal 0xS
    // --session N [--inheritcursor]
    let mut exe = [0u16; 300];
    let n = super::files::GetSystemDirectoryW(exe.as_mut_ptr(), 260) as usize;
    let mut at = n.min(260);
    put(&mut exe, &mut at, b"\\conhost.exe");
    exe[at] = 0;
    let mut line = [0u16; 512];
    let mut l = 0;
    put(&mut line, &mut l, b"\"");
    line[l..l + at].copy_from_slice(&exe[..at]);
    l += at;
    put(&mut line, &mut l, b"\" --headless --width ");
    put_number(&mut line, &mut l, cols as u64, false);
    put(&mut line, &mut l, b" --height ");
    put_number(&mut line, &mut l, rows as u64, false);
    put(&mut line, &mut l, b" --signal 0x");
    put_number(&mut line, &mut l, signal_read as u64, true);
    put(&mut line, &mut l, b" --session ");
    put_number(&mut line, &mut l, u64::from(session), false);
    if flags & PSEUDOCONSOLE_INHERIT_CURSOR != 0 {
        put(&mut line, &mut l, b" --inheritcursor");
    }
    line[l] = 0;
    // Only the signal pipe is inherited (besides the standard handles):
    // the caller's other inheritable handles stay out of the host.
    let inherited = [signal_read];
    let mut list = core::mem::MaybeUninit::<super::process::AttributeList>::uninit();
    let mut list_size = core::mem::size_of::<super::process::AttributeList>();
    let list = list.as_mut_ptr();
    if super::process::InitializeProcThreadAttributeList(list, 1, 0, &mut list_size) == FALSE
        || super::process::UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherited.as_ptr() as usize,
            size_of_val(&inherited),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        ) == FALSE
    {
        CloseHandle(signal_read);
        CloseHandle(signal_write);
        return closed(E_INVALIDARG);
    }
    // STARTUPINFOEXW: the standard handles and the attribute list.
    // SAFETY: an all-zero STARTUPINFOEXW is a valid bit pattern.
    let mut startup: super::process::StartupInfoEx = unsafe { core::mem::zeroed() };
    startup.cb = size_of_val(&startup) as u32;
    startup.flags = STARTF_USESTDHANDLES;
    startup.std = [input as usize, output as usize, output as usize];
    startup.attribute_list = list as usize;
    let mut process = [0usize; 3];
    let ok = super::process::CreateProcessW(
        exe.as_ptr(),
        line.as_mut_ptr(),
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        TRUE,
        EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
        core::ptr::null_mut(),
        core::ptr::null(),
        (&raw const startup).cast(),
        process.as_mut_ptr(),
    );
    CloseHandle(signal_read);
    if ok == FALSE {
        let hr = last_hresult();
        CloseHandle(signal_write);
        return closed(hr);
    }
    CloseHandle(process[1] as Handle);
    let block = super::heap::HeapAlloc(
        super::heap::GetProcessHeap(),
        0,
        core::mem::size_of::<PseudoConsole>(),
    )
    .cast::<PseudoConsole>();
    if block.is_null() {
        super::mapping::TerminateProcess(process[0] as Handle, 1);
        CloseHandle(process[0] as Handle);
        CloseHandle(signal_write);
        return closed(E_OUTOFMEMORY);
    }
    // SAFETY: our new allocation; the caller's output.
    unsafe {
        block.write(PseudoConsole {
            session,
            _reserved: 0,
            signal: signal_write,
            host: process[0] as Handle,
        });
        *pc = block as usize;
    }
    S_OK
}

/// The pseudo console an HPCON names (None: null).
fn console(pc: usize) -> Option<&'static PseudoConsole> {
    // SAFETY: an HPCON from CreatePseudoConsole, not yet closed.
    (pc != 0).then(|| unsafe { &*(pc as *const PseudoConsole) })
}

/// ResizePseudoConsole(hPC, size): the console's programs see the new
/// size (GetConsoleScreenBufferInfo; a WINDOW_BUFFER_SIZE_EVENT with
/// ENABLE_WINDOW_INPUT).
#[unsafe(no_mangle)]
pub extern "system" fn ResizePseudoConsole(pc: usize, size: u32) -> i32 {
    let Some(c) = console(pc) else { return E_INVALIDARG };
    let (cols, rows) = (size as u16 as i16, (size >> 16) as u16 as i16);
    if cols <= 0 || rows <= 0 {
        return E_INVALIDARG;
    }
    let mut size = size;
    // SAFETY: our size; the session we made.
    match unsafe { GalaConsoleSize(c.session, &mut size, 1) } {
        STATUS_SUCCESS => S_OK,
        status => fail(status, E_INVALIDARG),
    }
}

/// ClosePseudoConsole(hPC): the console closes, its programs end, and the
/// host (which carries a last output to the pipe) is waited for a while,
/// as on Windows (the caller drains the output pipe meanwhile).
#[unsafe(no_mangle)]
pub extern "system" fn ClosePseudoConsole(pc: usize) {
    let Some(c) = console(pc) else { return };
    // SAFETY: the session we made.
    unsafe { GalaConsoleClose(c.session) };
    CloseHandle(c.signal);
    WaitForSingleObject(c.host, 5000);
    CloseHandle(c.host);
    super::heap::HeapFree(super::heap::GetProcessHeap(), 0, pc as *mut c_void);
}

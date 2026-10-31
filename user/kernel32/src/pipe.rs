//! Anonymous pipes and handle flags. CreatePipe makes a uniquely named
//! byte-mode pipe below `\Device\NamedPipe` (`Win32Pipes.<pid>.<n>`, the
//! name Windows uses): the server end, created with NtCreateNamedPipeFile,
//! is the read end; the client end, opened with NtOpenFile, the write end.
//! Both are synchronous, so ReadFile/WriteFile on them block.

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateNamedPipeFile(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        share: u32,
        disposition: u32,
        options: u32,
        kind: u32,
        read_mode: u32,
        completion: u32,
        instances: u32,
        inbound: u32,
        outbound: u32,
        timeout: *const i64,
    ) -> u32;
    fn NtOpenFile(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        iosb: *mut [usize; 2],
        share: u32,
        options: u32,
    ) -> u32;
    fn NtFsControlFile(
        h: Handle,
        event: usize,
        apc: usize,
        context: usize,
        iosb: *mut [usize; 2],
        code: u32,
        input: *const u8,
        input_length: u32,
        output: *mut u8,
        output_length: u32,
    ) -> u32;
    fn NtQueryObject(h: Handle, class: u32, buffer: *mut u8, length: u32, ret: *mut u32) -> u32;
    fn NtSetInformationObject(h: Handle, class: u32, buffer: *const u8, length: u32) -> u32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const FILE_CREATE: u32 = 2;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const OBJ_INHERIT: u32 = 0x02;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const FSCTL_PIPE_PEEK: u32 = 0x0011_400c;
const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const OBJECT_HANDLE_FLAG_INFORMATION: u32 = 4;
const HANDLE_FLAG_INHERIT: u32 = 1;
const HANDLE_FLAG_PROTECT_FROM_CLOSE: u32 = 2;

/// Numbers the pipes of this process (the name's second part).
static PIPE_SERIAL: AtomicU32 = AtomicU32::new(0);

/// `\Device\NamedPipe\Win32Pipes.<pid>.<serial>` (hex, 8 digits each).
fn pipe_name(out: &mut [u16; 64]) -> usize {
    let mut at = 0;
    let mut put = |s: &[u8]| {
        for &b in s {
            out[at] = u16::from(b);
            at += 1;
        }
    };
    put(b"\\Device\\NamedPipe\\Win32Pipes.");
    let hex8 = |v: u32| {
        let mut s = [0u8; 8];
        for (i, c) in s.iter_mut().enumerate() {
            let d = (v >> (28 - 4 * i)) & 0xf;
            *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
        }
        s
    };
    put(&hex8(GetCurrentProcessId()));
    put(b".");
    put(&hex8(PIPE_SERIAL.fetch_add(1, Ordering::Relaxed)));
    at
}

/// CreatePipe(*hReadPipe, *hWritePipe, lpPipeAttributes, nSize).
#[unsafe(no_mangle)]
pub extern "system" fn CreatePipe(
    read: *mut Handle,
    write: *mut Handle,
    attributes: *mut c_void,
    size: u32,
) -> Bool {
    if read.is_null() || write.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let mut name = [0u16; 64];
    let len = pipe_name(&mut name);
    let unicode = UnicodeString {
        length: (len * 2) as u16,
        maximum_length: (len * 2) as u16,
        buffer: name.as_ptr(),
    };
    let inherit = super::sync::inheritable(attributes);
    let object = ObjectAttributes {
        length: core::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &unicode,
        attributes: OBJ_CASE_INSENSITIVE | if inherit { OBJ_INHERIT } else { 0 },
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let (mut server, mut client) = (core::ptr::null_mut(), core::ptr::null_mut());
    let mut iosb = [0usize; 2];
    let timeout: i64 = -1_200_000_000; // 120 s, as Windows passes
    // SAFETY: valid output pointers and attributes.
    let status = unsafe {
        NtCreateNamedPipeFile(
            &mut server,
            GENERIC_READ | FILE_WRITE_ATTRIBUTES | SYNCHRONIZE,
            &object,
            &mut iosb,
            3, // FILE_SHARE_READ | FILE_SHARE_WRITE
            FILE_CREATE,
            FILE_SYNCHRONOUS_IO_NONALERT,
            0, // byte stream
            0, // byte read mode
            0, // blocking
            1, // one instance
            size,
            size,
            &timeout,
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    // SAFETY: as above.
    let status = unsafe {
        NtOpenFile(
            &mut client,
            GENERIC_WRITE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &object,
            &mut iosb,
            1, // FILE_SHARE_READ
            FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE,
        )
    };
    if status != STATUS_SUCCESS {
        CloseHandle(server);
        return fail(status, FALSE);
    }
    // SAFETY: the caller's output pointers (checked above).
    unsafe {
        *read = server;
        *write = client;
    }
    TRUE
}

/// PeekNamedPipe(hNamedPipe, lpBuffer, nBufferSize, *lpBytesRead,
/// *lpTotalBytesAvail, *lpBytesLeftThisMessage): FSCTL_PIPE_PEEK.
#[unsafe(no_mangle)]
pub extern "system" fn PeekNamedPipe(
    h: Handle,
    buffer: *mut u8,
    size: u32,
    read: *mut u32,
    available: *mut u32,
    left: *mut u32,
) -> Bool {
    let size = if buffer.is_null() { 0 } else { size };
    let total = 16 + size as usize;
    // FILE_PIPE_PEEK_BUFFER: on the stack when small, else allocated.
    let mut small = [0u8; 1024];
    let peek = if total <= small.len() {
        small.as_mut_ptr()
    } else {
        let p = VirtualAlloc(core::ptr::null_mut(), total, 0x3000, 4).cast::<u8>();
        if p.is_null() {
            return FALSE;
        }
        p
    };
    let mut iosb = [0usize; 2];
    // SAFETY: an output buffer of `total` bytes.
    let status = unsafe {
        NtFsControlFile(
            h,
            0,
            0,
            0,
            &mut iosb,
            FSCTL_PIPE_PEEK,
            core::ptr::null(),
            0,
            peek,
            total as u32,
        )
    };
    let ok = status == STATUS_SUCCESS || status == STATUS_BUFFER_OVERFLOW;
    if ok {
        let copied = iosb[1].saturating_sub(16).min(size as usize);
        // SAFETY: `peek` holds the 16-byte header and `copied` bytes; the
        // caller's buffer and output pointers.
        unsafe {
            if copied > 0 {
                core::ptr::copy_nonoverlapping(peek.add(16), buffer, copied);
            }
            if !read.is_null() {
                *read = copied as u32;
            }
            if !available.is_null() {
                *available = peek.add(4).cast::<u32>().read_unaligned();
            }
            if !left.is_null() {
                // Byte mode: no message, so nothing is left of one.
                *left = 0;
            }
        }
    }
    if total > small.len() {
        VirtualFree(peek.cast(), 0, 0x8000);
    }
    if ok { TRUE } else { fail(status, FALSE) }
}

fn handle_flags(h: Handle) -> Result<[u8; 2], u32> {
    let mut flags = [0u8; 2];
    // SAFETY: a 2-byte OBJECT_HANDLE_FLAG_INFORMATION buffer.
    let status = unsafe {
        NtQueryObject(
            h,
            OBJECT_HANDLE_FLAG_INFORMATION,
            flags.as_mut_ptr(),
            2,
            core::ptr::null_mut(),
        )
    };
    if status == STATUS_SUCCESS { Ok(flags) } else { Err(status) }
}

/// GetHandleInformation(hObject, *lpdwFlags): HANDLE_FLAG_INHERIT and
/// HANDLE_FLAG_PROTECT_FROM_CLOSE.
#[unsafe(no_mangle)]
pub extern "system" fn GetHandleInformation(h: Handle, flags: *mut u32) -> Bool {
    match handle_flags(h) {
        Ok(f) => {
            if flags.is_null() {
                return fail_with(ERROR_INVALID_PARAMETER, FALSE);
            }
            let value = if f[0] != 0 { HANDLE_FLAG_INHERIT } else { 0 }
                | if f[1] != 0 { HANDLE_FLAG_PROTECT_FROM_CLOSE } else { 0 };
            // SAFETY: the caller's output pointer.
            unsafe { *flags = value };
            TRUE
        }
        Err(status) => fail(status, FALSE),
    }
}

/// SetHandleInformation(hObject, dwMask, dwFlags): the masked flags.
#[unsafe(no_mangle)]
pub extern "system" fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool {
    let mut f = match handle_flags(h) {
        Ok(f) => f,
        Err(status) => return fail(status, FALSE),
    };
    if mask & HANDLE_FLAG_INHERIT != 0 {
        f[0] = u8::from(flags & HANDLE_FLAG_INHERIT != 0);
    }
    if mask & HANDLE_FLAG_PROTECT_FROM_CLOSE != 0 {
        f[1] = u8::from(flags & HANDLE_FLAG_PROTECT_FROM_CLOSE != 0);
    }
    // SAFETY: a 2-byte OBJECT_HANDLE_FLAG_INFORMATION buffer.
    let status =
        unsafe { NtSetInformationObject(h, OBJECT_HANDLE_FLAG_INFORMATION, f.as_ptr(), 2) };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

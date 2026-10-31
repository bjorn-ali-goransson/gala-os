//! FormatMessageW/A with FORMAT_MESSAGE_FROM_SYSTEM: the system message
//! texts (US English, as Microsoft documents them in "System Error Codes"
//! and "Windows Sockets Error Codes", each ending in CRLF) of the errors
//! programs have needed. Other numbers fail with ERROR_MR_MID_NOT_FOUND
//! and go to the ledger, so the table grows where it matters.

use super::*;

const FORMAT_MESSAGE_ALLOCATE_BUFFER: u32 = 0x100;
const FORMAT_MESSAGE_FROM_STRING: u32 = 0x400;
const FORMAT_MESSAGE_FROM_HMODULE: u32 = 0x800;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x1000;
const FORMAT_MESSAGE_MAX_WIDTH_MASK: u32 = 0xff;
const ERROR_MR_MID_NOT_FOUND: u32 = 317;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

const MESSAGES: &[(u32, &str)] = &[
    (0, "The operation completed successfully."),
    (1, "Incorrect function."),
    (2, "The system cannot find the file specified."),
    (3, "The system cannot find the path specified."),
    (5, "Access is denied."),
    (6, "The handle is invalid."),
    (8, "Not enough memory resources are available to process this command."),
    (32, "The process cannot access the file because it is being used by another process."),
    (38, "Reached the end of the file."),
    (50, "The request is not supported."),
    (80, "The file exists."),
    (87, "The parameter is incorrect."),
    (122, "The data area passed to a system call is too small."),
    (126, "The specified module could not be found."),
    (127, "The specified procedure could not be found."),
    (183, "Cannot create a file when that file already exists."),
    (203, "The system could not find the environment option that was entered."),
    (258, "The wait operation timed out."),
    (
        995,
        "The I/O operation has been aborted because of either a thread exit or an application request.",
    ),
    (996, "Overlapped I/O event is not in a signaled state."),
    (997, "Overlapped I/O operation is in progress."),
    (1168, "Element not found."),
    (10004, "A blocking operation was interrupted by a call to WSACancelBlockingCall."),
    (
        10014,
        "The system detected an invalid pointer address in attempting to use a pointer argument in a call.",
    ),
    (10022, "An invalid argument was supplied."),
    (10035, "A non-blocking socket operation could not be completed immediately."),
    (10038, "An operation was attempted on something that is not a socket."),
    (
        10042,
        "An unknown, invalid, or unsupported option or level was specified in a getsockopt or setsockopt call.",
    ),
    (10045, "The attempted operation is not supported for the type of object referenced."),
    (10047, "An address incompatible with the requested protocol was used."),
    (
        10048,
        "Only one usage of each socket address (protocol/network address/port) is normally permitted.",
    ),
    (10049, "The requested address is not valid in its context."),
    (10051, "A socket operation was attempted to an unreachable network."),
    (10053, "An established connection was aborted by the software in your host machine."),
    (10054, "An existing connection was forcibly closed by the remote host."),
    (
        10055,
        "An operation on a socket could not be performed because the system lacked sufficient buffer space or because a queue was full.",
    ),
    (
        10057,
        "A request to send or receive data was disallowed because the socket is not connected and (when sending on a datagram socket using a sendto call) no address was supplied.",
    ),
    (
        10060,
        "A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond.",
    ),
    (10061, "No connection could be made because the target machine actively refused it."),
    (10093, "Either the application has not called WSAStartup, or WSAStartup failed."),
    (11001, "No such host is known."),
];

/// The message for `id` (with its CRLF unless `one_line`), as UTF-16 into
/// `out`; the length, or the Win32 error.
fn system_message(flags: u32, id: u32, out: &mut [u16]) -> Result<usize, u32> {
    if flags
        & (FORMAT_MESSAGE_FROM_STRING
            | FORMAT_MESSAGE_FROM_HMODULE
            | FORMAT_MESSAGE_ALLOCATE_BUFFER)
        != 0
        || flags & FORMAT_MESSAGE_FROM_SYSTEM == 0
    {
        unsupported(b"kernel32.dll!FormatMessage(flags)", flags.into());
        return Err(ERROR_NOT_SUPPORTED);
    }
    let Some(&(_, text)) = MESSAGES.iter().find(|&&(n, _)| n == id) else {
        unsupported(b"kernel32.dll!FormatMessage(message)", id.into());
        return Err(ERROR_MR_MID_NOT_FOUND);
    };
    // FORMAT_MESSAGE_MAX_WIDTH_MASK: no line breaks.
    let one_line = flags & FORMAT_MESSAGE_MAX_WIDTH_MASK == FORMAT_MESSAGE_MAX_WIDTH_MASK;
    let ending: &str = if one_line { "" } else { "\r\n" };
    let mut n = 0;
    for c in text.encode_utf16().chain(ending.encode_utf16()) {
        // Room for the terminator too.
        if n + 1 >= out.len() {
            return Err(ERROR_INSUFFICIENT_BUFFER);
        }
        out[n] = c;
        n += 1;
    }
    out[n] = 0;
    Ok(n)
}

/// FormatMessageW: FORMAT_MESSAGE_FROM_SYSTEM (inserts are not expanded;
/// the system texts above have none).
#[unsafe(no_mangle)]
pub extern "system" fn FormatMessageW(
    flags: u32,
    _source: *const c_void,
    message: u32,
    _language: u32,
    buffer: *mut u16,
    size: u32,
    _args: *mut c_void,
) -> u32 {
    if buffer.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's buffer of `size` characters.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    match system_message(flags, message, out) {
        Ok(n) => n as u32,
        Err(e) => fail_with(e, 0),
    }
}

/// FormatMessageA: the same text in the ANSI code page (ASCII here).
#[unsafe(no_mangle)]
pub extern "system" fn FormatMessageA(
    flags: u32,
    _source: *const c_void,
    message: u32,
    _language: u32,
    buffer: *mut u8,
    size: u32,
    _args: *mut c_void,
) -> u32 {
    if buffer.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut wide = [0u16; 512];
    let room = (size as usize).min(wide.len());
    match system_message(flags, message, &mut wide[..room]) {
        Ok(n) => {
            // SAFETY: the caller's buffer of `size` >= n + 1 bytes.
            unsafe {
                for (i, &c) in wide[..=n].iter().enumerate() {
                    *buffer.add(i) = c as u8; // the texts are ASCII
                }
            }
            n as u32
        }
        Err(e) => fail_with(e, 0),
    }
}

//! FormatMessageW/A: the system message texts (US English, as Microsoft
//! documents them in "System Error Codes" and "Windows Sockets Error
//! Codes", each ending in CRLF) of the errors programs have needed, or
//! the caller's string, with inserts. Other numbers fail with ERROR_MR_MID_NOT_FOUND
//! and go to the ledger, so the table grows where it matters.

use super::*;

const FORMAT_MESSAGE_ALLOCATE_BUFFER: u32 = 0x100;
const FORMAT_MESSAGE_FROM_STRING: u32 = 0x400;
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

const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x200;
const FORMAT_MESSAGE_ARGUMENT_ARRAY: u32 = 0x2000;

/// The longest message formatted.
const MAX_MESSAGE: usize = 4096;

/// A message being built (UTF-16).
struct Out {
    text: [u16; MAX_MESSAGE],
    len: usize,
}

impl Out {
    fn push(&mut self, c: u16) -> Result<(), u32> {
        *self.text.get_mut(self.len).ok_or(ERROR_INSUFFICIENT_BUFFER)? = c;
        self.len += 1;
        Ok(())
    }

    fn push_str(&mut self, s: &str) -> Result<(), u32> {
        s.encode_utf16().try_for_each(|c| self.push(c))
    }
}

/// The source text: the system message for `id` (with its CRLF unless
/// the line-width mask is set), or the caller's string.
enum Source {
    System(&'static str),
    Wide(*const u16),
    Ansi(*const u8),
}

fn source(flags: u32, source: *const c_void, id: u32, wide: bool) -> Result<Source, u32> {
    if flags & FORMAT_MESSAGE_FROM_STRING != 0 {
        if source.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        return Ok(if wide { Source::Wide(source.cast()) } else { Source::Ansi(source.cast()) });
    }
    if flags & FORMAT_MESSAGE_FROM_SYSTEM == 0 {
        unsupported(b"kernel32.dll!FormatMessage(flags)", flags.into());
        return Err(ERROR_NOT_SUPPORTED);
    }
    match MESSAGES.iter().find(|&&(n, _)| n == id) {
        Some(&(_, text)) => Ok(Source::System(text)),
        None => {
            unsupported(b"kernel32.dll!FormatMessage(message)", id.into());
            Err(ERROR_MR_MID_NOT_FOUND)
        }
    }
}

/// The source's code units (UTF-16; ANSI is ASCII/UTF-8 bytes).
fn source_units(src: &Source, out: &mut [u16; MAX_MESSAGE]) -> usize {
    let mut n = 0;
    match *src {
        Source::System(text) => {
            for c in text.encode_utf16() {
                out[n] = c;
                n += 1;
            }
        }
        // SAFETY: the caller's NUL-terminated string.
        Source::Wide(p) => unsafe {
            while n < MAX_MESSAGE && *p.add(n) != 0 {
                out[n] = *p.add(n);
                n += 1;
            }
        },
        // SAFETY: as above.
        Source::Ansi(p) => unsafe {
            while n < MAX_MESSAGE && *p.add(n) != 0 {
                out[n] = u16::from(*p.add(n));
                n += 1;
            }
        },
    }
    n
}

/// Argument `i` (from 1): from the array, or the va_list (an array of
/// 8-byte slots behind a pointer).
fn argument(flags: u32, args: *const c_void, i: usize) -> Result<usize, u32> {
    if args.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's arguments (as many as the message inserts).
    unsafe {
        let base = if flags & FORMAT_MESSAGE_ARGUMENT_ARRAY != 0 {
            args.cast::<usize>()
        } else {
            *args.cast::<*const usize>()
        };
        Ok(*base.add(i - 1))
    }
}

/// Formats one insert's value with its printf-style format (`s` when none).
fn insert(out: &mut Out, value: usize, spec: &[u16], wide: bool) -> Result<(), u32> {
    let spec: [u8; 16] = {
        let mut b = [0u8; 16];
        for (d, &c) in b.iter_mut().zip(spec) {
            *d = c as u8;
        }
        b
    };
    let spec = &spec[..spec.iter().position(|&b| b == 0).unwrap_or(16)];
    // Flags and width ("08"), then size prefixes (l, h, I64, ll), then the type.
    let zero = spec.first() == Some(&b'0');
    let digits = spec.iter().take_while(|b| b.is_ascii_digit()).count();
    let width: usize =
        core::str::from_utf8(&spec[..digits]).ok().and_then(|w| w.parse().ok()).unwrap_or(0);
    let kind = spec.last().copied().unwrap_or(b's');
    let wide_string = match kind {
        b's' => !spec.contains(&b'h') && (wide || spec.contains(&b'l')),
        b'S' => !wide,
        _ => false,
    };
    let mut digits_buf = [0u8; 24];
    let text: &[u8] = match kind {
        b's' | b'S' => {
            if value == 0 {
                return out.push_str("(null)");
            }
            let mut n = 0;
            // SAFETY: the caller's NUL-terminated string argument.
            unsafe {
                if wide_string {
                    let p = value as *const u16;
                    while *p.add(n) != 0 {
                        out.push(*p.add(n))?;
                        n += 1;
                    }
                } else {
                    let p = value as *const u8;
                    while *p.add(n) != 0 {
                        out.push((*p.add(n)).into())?;
                        n += 1;
                    }
                }
            }
            return Ok(());
        }
        b'c' => return out.push(value as u16),
        b'd' | b'i' => {
            let v = if spec.contains(&b'6') { value as i64 } else { i64::from(value as i32) };
            let mut i = digits_buf.len();
            let mut m = v.unsigned_abs();
            loop {
                i -= 1;
                digits_buf[i] = b'0' + (m % 10) as u8;
                m /= 10;
                if m == 0 {
                    break;
                }
            }
            if v < 0 {
                i -= 1;
                digits_buf[i] = b'-';
            }
            &digits_buf[i..]
        }
        b'u' | b'x' | b'X' => {
            let v = if spec.contains(&b'6') { value as u64 } else { u64::from(value as u32) };
            let (base, digits): (u64, &[u8]) = match kind {
                b'u' => (10, b"0123456789"),
                b'x' => (16, b"0123456789abcdef"),
                _ => (16, b"0123456789ABCDEF"),
            };
            let mut i = digits_buf.len();
            let mut m = v;
            loop {
                i -= 1;
                digits_buf[i] = digits[(m % base) as usize];
                m /= base;
                if m == 0 {
                    break;
                }
            }
            &digits_buf[i..]
        }
        _ => {
            unsupported(b"kernel32.dll!FormatMessage(insert format)", kind.into());
            return Err(ERROR_INVALID_PARAMETER);
        }
    };
    for _ in text.len()..width {
        out.push(if zero { u16::from(b'0') } else { u16::from(b' ') })?;
    }
    text.iter().try_for_each(|&b| out.push(b.into()))
}

/// The formatted message (UTF-16, not terminated).
fn format(
    flags: u32,
    src: *const c_void,
    id: u32,
    args: *const c_void,
    wide: bool,
    out: &mut Out,
) -> Result<(), u32> {
    let source = source(flags, src, id, wide)?;
    let system = matches!(source, Source::System(_));
    let mut units = [0u16; MAX_MESSAGE];
    let n = source_units(&source, &mut units);
    let units = &units[..n];
    let one_line = flags & FORMAT_MESSAGE_MAX_WIDTH_MASK == FORMAT_MESSAGE_MAX_WIDTH_MASK;
    if flags & FORMAT_MESSAGE_IGNORE_INSERTS != 0 {
        units.iter().try_for_each(|&c| out.push(c))?;
    } else {
        let mut i = 0;
        while i < units.len() {
            let c = units[i];
            i += 1;
            if c != u16::from(b'%') || i == units.len() {
                out.push(c)?;
                continue;
            }
            let next = units[i] as u8;
            i += 1;
            match next {
                b'0' => return Ok(()), // the message ends here, no line break
                b'1'..=b'9' => {
                    let mut number = usize::from(next - b'0');
                    if i < units.len() && (u16::from(b'0')..=u16::from(b'9')).contains(&units[i]) {
                        number = number * 10 + usize::from(units[i] as u8 - b'0');
                        i += 1;
                    }
                    let mut spec: &[u16] = &[u16::from(b's')];
                    if i < units.len() && units[i] == u16::from(b'!') {
                        let end = units[i + 1..].iter().position(|&c| c == u16::from(b'!'));
                        if let Some(end) = end {
                            spec = &units[i + 1..i + 1 + end];
                            i += end + 2;
                        }
                    }
                    insert(out, argument(flags, args, number)?, spec, wide)?;
                }
                b'n' => {
                    out.push(13)?;
                    out.push(10)?;
                }
                b'r' => out.push(13)?,
                b't' => out.push(9)?,
                b'b' => out.push(32)?,
                other => out.push(other.into())?, // %% %. %! and the like
            }
        }
    }
    if system && !one_line {
        out.push_str("\r\n")?;
    }
    Ok(())
}

/// Delivers `text` (UTF-16 code units, `wide`, or bytes): into the caller's
/// buffer of `size` units, or with FORMAT_MESSAGE_ALLOCATE_BUFFER into a
/// LocalAlloc'ed one stored at `buffer` (at least `size` units).
fn deliver(flags: u32, text: &[u16], buffer: *mut c_void, size: u32, wide: bool) -> u32 {
    let unit = if wide { 2 } else { 1 };
    let write = |dst: *mut u8| {
        // SAFETY: a destination of at least text.len() + 1 units.
        unsafe {
            for (i, &c) in text.iter().chain(core::iter::once(&0)).enumerate() {
                if wide {
                    (dst.cast::<u16>()).add(i).write_unaligned(c);
                } else {
                    *dst.add(i) = c as u8;
                }
            }
        }
    };
    if flags & FORMAT_MESSAGE_ALLOCATE_BUFFER != 0 {
        let units = (text.len() + 1).max(size as usize);
        let p = super::heap::LocalAlloc(0, units * unit);
        if p.is_null() {
            return fail_with(8, 0); // ERROR_NOT_ENOUGH_MEMORY
        }
        write(p.cast());
        // SAFETY: the caller's pointer to a buffer pointer.
        unsafe { *buffer.cast::<*mut c_void>() = p };
        return text.len() as u32;
    }
    if (size as usize) < text.len() + 1 {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    write(buffer.cast());
    text.len() as u32
}

/// FormatMessageW: system messages (FORMAT_MESSAGE_FROM_SYSTEM) or the
/// caller's string (FROM_STRING), with inserts (%1..%99 and their
/// printf-style formats, %n %r %t %b %0 %%) unless IGNORE_INSERTS, into
/// the caller's buffer or an allocated one (ALLOCATE_BUFFER). Message
/// tables in modules (FROM_HMODULE) are not supported yet (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn FormatMessageW(
    flags: u32,
    src: *const c_void,
    message: u32,
    _language: u32,
    buffer: *mut u16,
    size: u32,
    args: *mut c_void,
) -> u32 {
    if buffer.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut out = Out { text: [0; MAX_MESSAGE], len: 0 };
    match format(flags, src, message, args, true, &mut out) {
        Ok(()) => deliver(flags, &out.text[..out.len], buffer.cast(), size, true),
        Err(e) => fail_with(e, 0),
    }
}

/// FormatMessageA: the same in the ANSI code page (the system texts are
/// ASCII; strings are passed through as bytes).
#[unsafe(no_mangle)]
pub extern "system" fn FormatMessageA(
    flags: u32,
    src: *const c_void,
    message: u32,
    _language: u32,
    buffer: *mut u8,
    size: u32,
    args: *mut c_void,
) -> u32 {
    if buffer.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut out = Out { text: [0; MAX_MESSAGE], len: 0 };
    match format(flags, src, message, args, false, &mut out) {
        Ok(()) => deliver(flags, &out.text[..out.len], buffer.cast(), size, false),
        Err(e) => fail_with(e, 0),
    }
}

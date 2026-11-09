//! The Wayland wire format (ROADMAP LD step 2; ARCHITECTURE "Linux
//! applications on the desktop (Wayland)"): what travels on a Wayland
//! client's socket, as plain data the in-kernel server parses and builds.
//!
//! - A message is a header of two 32-bit words, the object id, then the
//!   message's size in bytes (high 16 bits, header included, a multiple of
//!   4) and its opcode (low 16 bits), followed by its arguments, each
//!   32-bit aligned, in host byte order: `int`, `uint`, `fixed` (signed
//!   24.8), `object` (an id, 0 for null), `new_id` (the id the sender
//!   chose; untyped, as `wl_registry.bind` takes it, it is preceded by the
//!   interface's name as a string and its version), `string` (a length
//!   counting the NUL, 0 for null, then the bytes padded to 4) and `array`
//!   (a length, then the bytes padded to 4). An `fd` takes no bytes: it
//!   travels as SCM_RIGHTS beside the message.
//! - [`INTERFACES`]: every interface of wayland.xml, xdg-shell.xml and
//!   xdg-decoration-unstable-v1.xml with its requests and events (their
//!   opcodes are their positions), generated at build time from the XML.
//! - [`decode`] checks a message against its signature and gives its
//!   arguments; [`encode`] builds one. Anything malformed is a
//!   [`WireError`], which the server answers with `wl_display.error` and a
//!   disconnect, as the protocol specifies; nothing here panics on a
//!   client's bytes.
//! - Limits as libwayland's: a message is at most [`MAX_MESSAGE`] bytes
//!   and carries at most [`MAX_FDS`] file descriptors.
//!
//! Sources (CLEAN_ROOM.md "Linux"; THIRD_PARTY.md): the Wayland
//! protocol's documentation ("Wire Format") and its XML, read as
//! specifications. `no_std`, no allocation, no `unsafe`; host-tested.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods, clippy::disallowed_macros))]

/// The header's size: object id, then size and opcode.
pub const HEADER: usize = 8;
/// The largest message, header included (libwayland's limit).
pub const MAX_MESSAGE: usize = 4096;
/// The most file descriptors one message carries (libwayland's limit).
pub const MAX_FDS: usize = 28;

/// An argument's wire type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    Int,
    Uint,
    Fixed,
    String,
    Object,
    NewId,
    Array,
    Fd,
}

/// An argument of a request or event as the protocol declares it.
#[derive(Debug)]
pub struct ArgSpec {
    pub name: &'static str,
    pub kind: ArgKind,
    /// The interface of an `object` or `new_id`; None for any (an untyped
    /// `new_id` carries its interface's name and version on the wire).
    pub interface: Option<&'static str>,
    /// Whether an `object` or `string` may be null.
    pub nullable: bool,
}

/// A request or event; its opcode is its position in its interface's list.
#[derive(Debug)]
pub struct Message {
    pub name: &'static str,
    /// The interface version that added it.
    pub since: u32,
    /// Whether it destroys the object it is sent on.
    pub destructor: bool,
    pub args: &'static [ArgSpec],
}

/// An interface: its name, highest version, requests and events.
#[derive(Debug)]
pub struct Interface {
    pub name: &'static str,
    pub version: u32,
    pub requests: &'static [Message],
    pub events: &'static [Message],
}

include!(concat!(env!("OUT_DIR"), "/protocol.rs"));

/// The interface called `name`.
pub fn interface(name: &[u8]) -> Option<&'static Interface> {
    INTERFACES.iter().find(|i| i.name.as_bytes() == name)
}

/// What is wrong with a message (each a protocol error for the server).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Fewer bytes than the header, or than the header's size.
    Truncated,
    /// A size below the header's, not a multiple of 4, or above
    /// MAX_MESSAGE.
    BadSize,
    /// An opcode the interface does not have.
    BadOpcode,
    /// The arguments need more bytes than the message has.
    ArgumentsTruncated,
    /// Bytes left after the last argument.
    TrailingBytes,
    /// A string without its NUL, or with one before its end.
    BadString,
    /// Null where the protocol does not allow it (object, string, new_id).
    NullNotAllowed,
    /// More file descriptors than came with the message, or than
    /// MAX_FDS.
    MissingFd,
    /// An encoded argument that does not match the signature.
    WrongArgument,
    /// The output buffer is too small.
    NoRoom,
}

/// A message's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub object: u32,
    pub opcode: u16,
    /// The whole message's size in bytes, header included.
    pub size: u16,
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    let w = b.get(at..at + 4)?;
    Some(u32::from_ne_bytes([w[0], w[1], w[2], w[3]]))
}

fn padded(len: usize) -> usize {
    (len + 3) & !3
}

impl Header {
    /// The header at the start of `bytes`, its size checked (at least the
    /// header, a multiple of 4, at most MAX_MESSAGE). Whether the whole
    /// message has arrived is the caller's to check (`size`).
    pub fn parse(bytes: &[u8]) -> Result<Header, WireError> {
        let (Some(object), Some(second)) = (word(bytes, 0), word(bytes, 4)) else {
            return Err(WireError::Truncated);
        };
        let size = (second >> 16) as u16;
        if usize::from(size) < HEADER || !size.is_multiple_of(4) || usize::from(size) > MAX_MESSAGE
        {
            return Err(WireError::BadSize);
        }
        Ok(Header { object, opcode: second as u16, size })
    }

    pub fn to_bytes(self) -> [u8; HEADER] {
        let mut b = [0u8; HEADER];
        b[..4].copy_from_slice(&self.object.to_ne_bytes());
        b[4..]
            .copy_from_slice(&(u32::from(self.size) << 16 | u32::from(self.opcode)).to_ne_bytes());
        b
    }
}

/// A decoded (or to be encoded) argument. Strings are without their NUL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg<'a> {
    Int(i32),
    Uint(u32),
    /// Signed 24.8 fixed point.
    Fixed(i32),
    String(Option<&'a [u8]>),
    Object(Option<u32>),
    NewId(u32),
    /// An untyped new_id: the interface's name, its version, the id.
    UntypedNewId {
        interface: &'a [u8],
        version: u32,
        id: u32,
    },
    Array(&'a [u8]),
    /// A file descriptor: the next one of those that came with the message.
    Fd,
}

/// A string's wire form at `at`: its bytes without the NUL (None for
/// null) and where the next argument starts.
fn string_at(msg: &[u8], at: usize) -> Result<(Option<&[u8]>, usize), WireError> {
    let len = word(msg, at).ok_or(WireError::ArgumentsTruncated)? as usize;
    let start = at + 4;
    if len == 0 {
        return Ok((None, start));
    }
    let end = start.checked_add(padded(len)).ok_or(WireError::ArgumentsTruncated)?;
    let bytes = msg.get(start..start + len).ok_or(WireError::ArgumentsTruncated)?;
    if end > msg.len() {
        return Err(WireError::ArgumentsTruncated);
    }
    // The NUL is the last byte and the only one.
    let (text, nul) = bytes.split_at(len - 1);
    if nul != [0] || text.contains(&0) {
        return Err(WireError::BadString);
    }
    Ok((Some(text), end))
}

/// Decodes the message `msg` (header included, exactly `Header::size`
/// bytes) against `sig`, with `fds` file descriptors received with it:
/// the arguments in `out`, how many, and how many file descriptors it
/// takes.
pub fn decode<'a>(
    msg: &'a [u8],
    sig: &Message,
    fds: usize,
    out: &mut [Arg<'a>; MAX_ARGS],
) -> Result<(usize, usize), WireError> {
    let header = Header::parse(msg)?;
    if msg.len() != usize::from(header.size) {
        return Err(WireError::Truncated);
    }
    let mut at = HEADER;
    let mut used_fds = 0;
    for (slot, spec) in out.iter_mut().zip(sig.args) {
        let next = |at: usize| word(msg, at).ok_or(WireError::ArgumentsTruncated);
        *slot = match spec.kind {
            ArgKind::Int => Arg::Int(next(at)? as i32),
            ArgKind::Uint => Arg::Uint(next(at)?),
            ArgKind::Fixed => Arg::Fixed(next(at)? as i32),
            ArgKind::Object => {
                let id = next(at)?;
                if id == 0 && !spec.nullable {
                    return Err(WireError::NullNotAllowed);
                }
                Arg::Object((id != 0).then_some(id))
            }
            ArgKind::NewId if spec.interface.is_none() => {
                let (name, after) = string_at(msg, at)?;
                let name = name.ok_or(WireError::NullNotAllowed)?;
                let version = next(after)?;
                let id = next(after + 4)?;
                if id == 0 {
                    return Err(WireError::NullNotAllowed);
                }
                at = after + 8;
                *slot = Arg::UntypedNewId { interface: name, version, id };
                continue;
            }
            ArgKind::NewId => {
                let id = next(at)?;
                if id == 0 {
                    return Err(WireError::NullNotAllowed);
                }
                Arg::NewId(id)
            }
            ArgKind::String => {
                let (s, after) = string_at(msg, at)?;
                if s.is_none() && !spec.nullable {
                    return Err(WireError::NullNotAllowed);
                }
                at = after;
                *slot = Arg::String(s);
                continue;
            }
            ArgKind::Array => {
                let len = next(at)? as usize;
                let start = at + 4;
                let end = start.checked_add(padded(len)).ok_or(WireError::ArgumentsTruncated)?;
                if end > msg.len() {
                    return Err(WireError::ArgumentsTruncated);
                }
                at = end;
                *slot = Arg::Array(&msg[start..start + len]);
                continue;
            }
            ArgKind::Fd => {
                used_fds += 1;
                if used_fds > fds || used_fds > MAX_FDS {
                    return Err(WireError::MissingFd);
                }
                *slot = Arg::Fd;
                continue;
            }
        };
        at += 4;
    }
    if at != msg.len() {
        return Err(WireError::TrailingBytes);
    }
    Ok((sig.args.len(), used_fds))
}

/// A little writer over the caller's buffer.
struct Out<'b> {
    buf: &'b mut [u8],
    at: usize,
}

impl Out<'_> {
    fn bytes(&mut self, b: &[u8]) -> Result<(), WireError> {
        let end = self.at + padded(b.len());
        if end > self.buf.len() || end > MAX_MESSAGE {
            return Err(WireError::NoRoom);
        }
        self.buf[self.at..self.at + b.len()].copy_from_slice(b);
        self.buf[self.at + b.len()..end].fill(0);
        self.at = end;
        Ok(())
    }

    fn word(&mut self, w: u32) -> Result<(), WireError> {
        self.bytes(&w.to_ne_bytes())
    }

    fn string(&mut self, s: Option<&[u8]>) -> Result<(), WireError> {
        match s {
            None => self.word(0),
            Some(s) => {
                if s.contains(&0) {
                    return Err(WireError::BadString);
                }
                self.word(s.len() as u32 + 1)?;
                let end = self.at + padded(s.len() + 1);
                if end > self.buf.len() || end > MAX_MESSAGE {
                    return Err(WireError::NoRoom);
                }
                self.buf[self.at..self.at + s.len()].copy_from_slice(s);
                self.buf[self.at + s.len()..end].fill(0);
                self.at = end;
                Ok(())
            }
        }
    }
}

/// Encodes message `opcode` (`sig`) on `object` with `args` into `out`:
/// how many bytes, and how many file descriptors go beside it (one per
/// `Arg::Fd`, in order).
pub fn encode(
    object: u32,
    opcode: u16,
    sig: &Message,
    args: &[Arg<'_>],
    out: &mut [u8],
) -> Result<(usize, usize), WireError> {
    if args.len() != sig.args.len() || out.len() < HEADER {
        return Err(if args.len() != sig.args.len() {
            WireError::WrongArgument
        } else {
            WireError::NoRoom
        });
    }
    let mut w = Out { buf: out, at: HEADER };
    let mut fds = 0;
    for (arg, spec) in args.iter().zip(sig.args) {
        match (spec.kind, *arg) {
            (ArgKind::Int, Arg::Int(v)) | (ArgKind::Fixed, Arg::Fixed(v)) => w.word(v as u32)?,
            (ArgKind::Uint, Arg::Uint(v)) => w.word(v)?,
            (ArgKind::Object, Arg::Object(id)) => {
                if id.is_none() && !spec.nullable {
                    return Err(WireError::NullNotAllowed);
                }
                w.word(id.unwrap_or(0))?
            }
            (ArgKind::NewId, Arg::NewId(id)) if spec.interface.is_some() && id != 0 => {
                w.word(id)?
            }
            (ArgKind::NewId, Arg::UntypedNewId { interface, version, id })
                if spec.interface.is_none() && id != 0 =>
            {
                w.string(Some(interface))?;
                w.word(version)?;
                w.word(id)?
            }
            (ArgKind::String, Arg::String(s)) => {
                if s.is_none() && !spec.nullable {
                    return Err(WireError::NullNotAllowed);
                }
                w.string(s)?
            }
            (ArgKind::Array, Arg::Array(a)) => {
                w.word(a.len() as u32)?;
                w.bytes(a)?
            }
            (ArgKind::Fd, Arg::Fd) => {
                fds += 1;
                if fds > MAX_FDS {
                    return Err(WireError::MissingFd);
                }
            }
            _ => return Err(WireError::WrongArgument),
        }
    }
    let size = w.at;
    let header = Header { object, opcode, size: size as u16 };
    out[..HEADER].copy_from_slice(&header.to_bytes());
    Ok((size, fds))
}

/// A fixed-point value from an integer, and back (rounding toward zero).
pub const fn fixed_from_int(v: i32) -> i32 {
    v.wrapping_mul(256)
}

pub const fn fixed_to_int(f: i32) -> i32 {
    f / 256
}

#[cfg(test)]
mod tests;

//! WinHttpCrackUrl and InternetCrackUrlW/A: a URL's components into
//! URL_COMPONENTS (one layout in both APIs; URL_COMPONENTSA the same
//! with byte strings), with the documented
//! conventions per component: length 0 leaves it out; a null
//! pointer with a nonzero length (often -1) gets a pointer into the URL
//! and the component's length; a buffer with its length in characters
//! gets a NUL-terminated copy and the copied length, or
//! ERROR_INSUFFICIENT_BUFFER with the length needed (with the NUL).

use crate::err::{INSUFFICIENT_BUFFER, INVALID_PARAMETER, INVALID_URL, UNRECOGNIZED_SCHEME};
use crate::{Dll, missing};

/// URL_COMPONENTSW (`T` u16) and URL_COMPONENTSA (`T` u8), in the
/// caller's width (the pointers are 4 or 8 bytes).
#[repr(C)]
pub struct UrlComponentsOf<T> {
    size: u32,
    scheme: *mut T,
    scheme_length: u32,
    n_scheme: u32,
    host: *mut T,
    host_length: u32,
    port: u16,
    user: *mut T,
    user_length: u32,
    password: *mut T,
    password_length: u32,
    path: *mut T,
    path_length: u32,
    extra: *mut T,
    extra_length: u32,
}

/// URL_COMPONENTS(W).
pub type UrlComponents = UrlComponentsOf<u16>;
/// URL_COMPONENTSA.
pub type UrlComponentsA = UrlComponentsOf<u8>;

/// A character of a URL the caller gives: UTF-16 or an ANSI byte. The
/// delimiters are ASCII, so the byte offsets of an ANSI URL widened byte
/// by byte are its components' offsets whatever its code page.
pub trait Unit: Copy + Default + PartialEq {
    fn unit(self) -> u16;
}

impl Unit for u16 {
    fn unit(self) -> u16 {
        self
    }
}

impl Unit for u8 {
    fn unit(self) -> u16 {
        self.into()
    }
}

const ICU_DECODE: u32 = 0x1000_0000;
const ICU_ESCAPE: u32 = 0x8000_0000;

/// A URL split into ranges of its characters.
pub struct Parts {
    pub scheme: (usize, usize),
    /// https (otherwise http).
    pub secure: bool,
    pub user: (usize, usize),
    pub password: (usize, usize),
    pub host: (usize, usize),
    pub port: u16,
    pub path: (usize, usize),
    pub extra: (usize, usize),
}

fn find(u: &[u16], from: usize, any: &[u8]) -> usize {
    u[from..].iter().position(|&c| any.iter().any(|&a| c == a.into())).map_or(u.len(), |i| from + i)
}

/// Splits `scheme://[user[:password]@]host[:port][/path][?extra|#extra]`.
pub fn parse(u: &[u16]) -> Result<Parts, u32> {
    let colon = u.iter().position(|&c| c == b':'.into()).ok_or(UNRECOGNIZED_SCHEME)?;
    let scheme = (0, colon);
    let is = |want: &[u8]| {
        colon == want.len()
            && u[..colon]
                .iter()
                .zip(want)
                .all(|(&c, &w)| c < 0x80 && (c as u8).eq_ignore_ascii_case(&w))
    };
    let (secure, default_port) = if is(b"http") {
        (false, 80)
    } else if is(b"https") {
        (true, 443)
    } else {
        return Err(UNRECOGNIZED_SCHEME);
    };
    let slash = [b'/' as u16, b'/' as u16];
    if u.len() < colon + 3 || u[colon + 1..colon + 3] != slash {
        return Err(INVALID_URL);
    }
    let start = colon + 3;
    let end = find(u, start, b"/?#");
    // Credentials: up to the last '@' of the authority.
    let at = u[start..end].iter().rposition(|&c| c == b'@'.into()).map(|i| start + i);
    let (user, password, host_start) = match at {
        Some(at) => {
            let c = find(&u[..at], start, b":");
            if c < at { ((start, c), (c + 1, at), at + 1) } else { ((start, at), (at, at), at + 1) }
        }
        None => ((start, start), (start, start), start),
    };
    // The host: a bracketed IPv6 literal keeps its brackets.
    let host_end = if u.get(host_start) == Some(&(b'[' as u16)) {
        let close = find(&u[..end], host_start, b"]");
        if close == end {
            return Err(INVALID_URL);
        }
        close + 1
    } else {
        find(&u[..end], host_start, b":")
    };
    if host_end == host_start {
        return Err(INVALID_URL);
    }
    let port = if host_end < end {
        if u[host_end] != b':'.into() {
            return Err(INVALID_URL);
        }
        let digits = &u[host_end + 1..end];
        let mut p: u32 = 0;
        if digits.is_empty() {
            p = default_port;
        }
        for &d in digits {
            if !(b'0'.into()..=b'9'.into()).contains(&d) {
                return Err(INVALID_URL);
            }
            p = p * 10 + u32::from(d - u16::from(b'0'));
            if p > 65535 {
                return Err(INVALID_URL);
            }
        }
        p as u16
    } else {
        default_port as u16
    };
    let extra_start = find(u, end, b"?#");
    Ok(Parts {
        scheme,
        secure,
        user,
        password,
        host: (host_start, host_end),
        port,
        path: (end, extra_start),
        extra: (extra_start, u.len()),
    })
}

/// Fills one component by the documented conventions.
///
/// # Safety
/// `ptr` and `len` are the caller's component fields; `url` the URL.
unsafe fn component<T: Unit>(
    url: *const T,
    range: (usize, usize),
    ptr: *mut *mut T,
    len: *mut u32,
) -> Result<(), u32> {
    let n = range.1 - range.0;
    // SAFETY: the caller's fields and, for a copy, its buffer of `*len`
    // characters; `range` is inside the URL.
    unsafe {
        if *len == 0 {
            return Ok(());
        }
        if (*ptr).is_null() {
            *ptr = url.add(range.0).cast_mut();
            *len = n as u32;
            return Ok(());
        }
        if (*len as usize) < n + 1 {
            *len = n as u32 + 1;
            return Err(INSUFFICIENT_BUFFER);
        }
        core::ptr::copy_nonoverlapping(url.add(range.0), *ptr, n);
        *(*ptr).add(n) = T::default();
        *len = n as u32;
    }
    Ok(())
}

/// Cracks `url` (`length` characters, 0: NUL-terminated) into the
/// caller's URL_COMPONENTS(W) or URL_COMPONENTSA; `schemes` are the API's
/// numbers for http and https (nScheme).
pub fn crack<T: Unit>(
    dll: Dll,
    url: *const T,
    length: u32,
    flags: u32,
    components: *mut UrlComponentsOf<T>,
    schemes: [u32; 2],
) -> Result<(), u32> {
    if url.is_null() || components.is_null() {
        return Err(INVALID_PARAMETER);
    }
    // SAFETY: the caller's structure.
    let c = unsafe { &mut *components };
    if c.size as usize != size_of::<UrlComponentsOf<T>>() {
        return Err(INVALID_PARAMETER);
    }
    if flags & (ICU_DECODE | ICU_ESCAPE) != 0 {
        // The components are given as they are in the URL.
        missing(dll, b"CrackUrl(ICU_DECODE/ICU_ESCAPE)", flags.into());
    }
    let n = if length == 0 {
        // SAFETY: the caller's NUL-terminated URL.
        (0..).take_while(|&i| unsafe { *url.add(i) } != T::default()).count()
    } else {
        length as usize
    };
    let mut text = alloc::vec::Vec::new();
    text.try_reserve(n).map_err(|_| crate::err::NOT_ENOUGH_MEMORY)?;
    // SAFETY: `n` characters of the caller's URL.
    text.extend((0..n).map(|i| unsafe { *url.add(i) }.unit()));
    let p = parse(&text)?;
    let mut short = false;
    // SAFETY: the caller's fields; the ranges are inside `url`.
    unsafe {
        for (range, ptr, len) in [
            (p.scheme, &raw mut c.scheme, &raw mut c.scheme_length),
            (p.host, &raw mut c.host, &raw mut c.host_length),
            (p.user, &raw mut c.user, &raw mut c.user_length),
            (p.password, &raw mut c.password, &raw mut c.password_length),
            (p.path, &raw mut c.path, &raw mut c.path_length),
            (p.extra, &raw mut c.extra, &raw mut c.extra_length),
        ] {
            match component(url, range, ptr, len) {
                Ok(()) => {}
                Err(INSUFFICIENT_BUFFER) => short = true,
                Err(e) => return Err(e),
            }
        }
    }
    c.n_scheme = schemes[usize::from(p.secure)];
    c.port = p.port;
    if short { Err(INSUFFICIENT_BUFFER) } else { Ok(()) }
}

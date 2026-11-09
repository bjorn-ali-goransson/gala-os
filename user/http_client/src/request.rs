//! A request: its headers, sending it (on an idle connection to the same
//! server kept from an earlier request, `pool`, else a new one) with a
//! body from the send call and later writes, the response's status line
//! and headers (1xx responses skipped), the header queries and the body
//! by Content-Length, chunked transfer coding or until the server closes;
//! HEAD, 204 and 304 have none. WinHttpSendRequest / HttpSendRequest,
//! WinHttpReceiveResponse, WinHttpQueryHeaders / HttpQueryInfo,
//! WinHttpQueryDataAvailable / InternetQueryDataAvailable and
//! WinHttpReadData / InternetReadFile are thin wrappers of these.
//!
//! Redirects are followed while the response is received, as the API's
//! policy says (`Settings::redirect_policy`, `redirect_rules`): the
//! Location (absolute, scheme-relative or relative to the request's
//! path) becomes the request's URL and the request is sent again, on a
//! new connection, up to `max_redirects` times. A redirect refused (https
//! to http under the default policy, a scheme other than http(s), one
//! too many) is `err::REDIRECT_FAILED` with the reason in the ledger
//! (`<dll>!redirect: ...`).
//!
//! Connections are reused (HTTP/1.1 keep-alive): once a response has
//! been read to its end, its connection goes back to the pool unless the
//! request or the response said `Connection: close`, the response was
//! HTTP/1.0 without `Connection: keep-alive`, or its body ended with the
//! connection. A request sent on a reused connection that the server
//! closes before answering is sent again on a new one (unless part of
//! its body came through WinHttpWriteData, which is not kept).
//!
//! Each step is told to the request's observer (`Event`, set by the DLL):
//! sending and sent, receiving and received (each response's header
//! block, a redirect's included), a redirect followed (between the 3xx's
//! received and the new request's connection), and the connection's own
//! (`net`).

use alloc::vec::Vec;

use crate::err::{
    CONNECTION_ERROR, HEADER_ALREADY_EXISTS, HEADER_NOT_FOUND, HEADER_SIZE_OVERFLOW,
    INCORRECT_HANDLE_STATE, INSUFFICIENT_BUFFER, INVALID_HEADER, INVALID_PARAMETER,
    INVALID_QUERY_REQUEST, INVALID_SERVER_RESPONSE, NOT_ENOUGH_MEMORY, NOT_SUPPORTED,
    REDIRECT_FAILED,
};
use crate::net::{Conn, Target};
use crate::pool::{self, Key};
use crate::{
    Event, Observer, REDIRECT_DISALLOW_HTTPS_TO_HTTP, REDIRECT_NEVER, RedirectRules, Settings,
    failed, missing, narrow, put, url, wide,
};

/// The modifiers of WinHttpAddRequestHeaders and HttpAddRequestHeaders
/// (the same bits).
pub const ADDREQ_FLAG_ADD_IF_NEW: u32 = 0x1000_0000;
pub const ADDREQ_FLAG_ADD: u32 = 0x2000_0000;
const ADDREQ_FLAG_COALESCE_WITH_COMMA: u32 = 0x4000_0000;
const ADDREQ_FLAG_COALESCE_WITH_SEMICOLON: u32 = 0x0100_0000;
const ADDREQ_FLAG_REPLACE: u32 = 0x8000_0000;

/// Query levels and flags (WINHTTP_QUERY_* and HTTP_QUERY_*: the same
/// numbers).
const QUERY_VERSION: u32 = 18;
const QUERY_STATUS_CODE: u32 = 19;
const QUERY_STATUS_TEXT: u32 = 20;
const QUERY_RAW_HEADERS: u32 = 21;
const QUERY_RAW_HEADERS_CRLF: u32 = 22;
const QUERY_REQUEST_METHOD: u32 = 45;
pub const QUERY_CUSTOM: u32 = 65535;
const QUERY_FLAG_REQUEST_HEADERS: u32 = 0x8000_0000;
const QUERY_FLAG_SYSTEMTIME: u32 = 0x4000_0000;
const QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const QUERY_FLAG_NUMBER64: u32 = 0x0800_0000;

/// The headers the query levels name ("Query Info Flags"); the others
/// are the status line's parts and the raw block.
const QUERY_NAMES: [&[u8]; 79] = [
    b"Mime-Version",
    b"Content-Type",
    b"Content-Transfer-Encoding",
    b"Content-ID",
    b"Content-Description",
    b"Content-Length",
    b"Content-Language",
    b"Allow",
    b"Public",
    b"Date",
    b"Expires",
    b"Last-Modified",
    b"Message-ID",
    b"URI",
    b"Derived-From",
    b"Cost",
    b"Link",
    b"Pragma",
    b"", // VERSION
    b"", // STATUS_CODE
    b"", // STATUS_TEXT
    b"", // RAW_HEADERS
    b"", // RAW_HEADERS_CRLF
    b"Connection",
    b"Accept",
    b"Accept-Charset",
    b"Accept-Encoding",
    b"Accept-Language",
    b"Authorization",
    b"Content-Encoding",
    b"Forwarded",
    b"From",
    b"If-Modified-Since",
    b"Location",
    b"Orig-URI",
    b"Referer",
    b"Retry-After",
    b"Server",
    b"Title",
    b"User-Agent",
    b"WWW-Authenticate",
    b"Proxy-Authenticate",
    b"Accept-Ranges",
    b"Set-Cookie",
    b"Cookie",
    b"", // REQUEST_METHOD
    b"Refresh",
    b"Content-Disposition",
    b"Age",
    b"Cache-Control",
    b"Content-Base",
    b"Content-Location",
    b"Content-MD5",
    b"Content-Range",
    b"ETag",
    b"Host",
    b"If-Match",
    b"If-None-Match",
    b"If-Range",
    b"If-Unmodified-Since",
    b"Max-Forwards",
    b"Proxy-Authorization",
    b"Range",
    b"Transfer-Encoding",
    b"Upgrade",
    b"Vary",
    b"Via",
    b"Warning",
    b"Expect",
    b"Proxy-Connection",
    b"Unless-Modified-Since",
    b"",
    b"",
    b"",
    b"",
    b"Proxy-Support",
    b"Authentication-Info",
    b"", // PASSPORT_URLS
    b"", // PASSPORT_CONFIG
];

/// `read_head`: a reused connection closed before any of the response
/// came (never returned to the caller: the request is sent again).
const STALE: u32 = u32::MAX;

/// The largest response header block (WinHTTP's default
/// MAX_RESPONSE_HEADER_SIZE, 64 KB; WinINet's too).
const MAX_HEADERS: usize = 64 * 1024;

type Header = (Vec<u8>, Vec<u8>);

#[derive(PartialEq, Eq)]
enum State {
    Open,
    Sent,
    Received,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chunk {
    /// The chunk-size line.
    Size,
    Data(u64),
    /// The CRLF after a chunk's data.
    DataEnd,
    /// Trailer fields up to the empty line.
    Trailer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Framing {
    Length(u64),
    Chunked(Chunk),
    UntilClose,
    Done,
}

pub struct Request {
    pub settings: Settings,
    /// SECURITY_FLAG_IGNORE_* for the TLS handshake.
    pub security_flags: u32,
    host: Vec<u16>,
    port: u16,
    secure: bool,
    verb: Vec<u8>,
    path: Vec<u8>,
    version: Vec<u8>,
    /// Headers added by the caller, then (once sent) the ones the API adds.
    headers: Vec<Header>,
    /// The names of the headers the API added (`build`), replaced on
    /// every send and redirect.
    api_added: Vec<Vec<u8>>,
    state: State,
    conn: Option<Conn>,
    /// `conn` came from the pool and has received nothing for this
    /// request yet (a close then means it went stale while idle).
    reused: bool,
    /// The connection may carry another request once the body ends.
    keep: bool,
    /// Body bytes still to come through WinHttpWriteData.
    to_write: u64,
    /// The body given to the send call, kept for a redirect that sends
    /// it again (307, 308).
    sent_body: Vec<u8>,
    /// More of the body came through WinHttpWriteData (not kept).
    streamed: bool,
    /// Redirects followed since the send.
    redirects: u32,
    status: u16,
    reason: Vec<u8>,
    response_version: Vec<u8>,
    response: Vec<Header>,
    /// Received plaintext not yet taken (headers, then body framing).
    rx: Vec<u8>,
    /// The decoded body not yet read, from `body_at`.
    body: Vec<u8>,
    body_at: usize,
    framing: Framing,
    /// Who is told the steps (None: nobody).
    pub observer: Option<Observer>,
}

/// Lowercase-insensitive header name comparison.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t' | b'\r'] = s {
        s = rest;
    }
    s
}

fn copy(s: &[u8]) -> Result<Vec<u8>, u32> {
    let mut v = Vec::new();
    put(&mut v, s)?;
    Ok(v)
}

fn hex(b: u8) -> u8 {
    b"0123456789ABCDEF"[b as usize & 15]
}

/// The decimal digits of `n`.
fn decimal(mut n: u64, out: &mut Vec<u8>) -> Result<(), u32> {
    let mut d = [0u8; 20];
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    put(out, &d[i..])
}

/// Where a redirect goes.
struct Next {
    secure: bool,
    host: Vec<u16>,
    port: u16,
    path: Vec<u8>,
}

/// `s` with spaces and non-ASCII bytes %-escaped (a path on the wire).
fn escaped(s: &[u8]) -> Result<Vec<u8>, u32> {
    let mut v = Vec::new();
    for &b in s {
        if b == b' ' || b >= 0x80 {
            put(&mut v, &[b'%', hex(b >> 4), hex(b)])?;
        } else {
            put(&mut v, &[b])?;
        }
    }
    Ok(v)
}

/// RFC 3986's remove_dot_segments for an absolute path (and its query).
fn without_dots(path: &[u8]) -> Result<Vec<u8>, u32> {
    let (path, query) = match path.iter().position(|&b| b == b'?') {
        Some(i) => path.split_at(i),
        None => (path, &b""[..]),
    };
    let mut kept: Vec<&[u8]> = Vec::new();
    let parts = path.strip_prefix(b"/").unwrap_or(path);
    let n = parts.split(|&b| b == b'/').count();
    for (i, seg) in parts.split(|&b| b == b'/').enumerate() {
        let last = i + 1 == n;
        kept.try_reserve(1).map_err(|_| NOT_ENOUGH_MEMORY)?;
        match seg {
            b"." if last => kept.push(b""),
            b"." => {}
            b".." => {
                kept.pop();
                if last {
                    kept.push(b"");
                }
            }
            s => kept.push(s),
        }
    }
    let mut out = Vec::new();
    for seg in kept {
        put(&mut out, b"/")?;
        put(&mut out, seg)?;
    }
    if out.is_empty() {
        put(&mut out, b"/")?;
    }
    put(&mut out, query)?;
    Ok(out)
}

/// Where a request goes and how (WinHttpOpenRequest's and
/// HttpOpenRequest's parameters, the strings as the caller gave them).
pub struct Open<'a> {
    pub host: &'a [u16],
    /// 0: the scheme's (INTERNET_DEFAULT_PORT).
    pub port: u16,
    pub verb: *const u16,
    pub object: *const u16,
    pub version: *const u16,
    pub referrer: *const u16,
    /// A NULL-terminated array of strings, or null.
    pub accept: *const *const u16,
    pub secure: bool,
    /// Escape spaces and non-ASCII in the path (as UTF-8).
    pub escape: bool,
}

impl Request {
    pub fn new(settings: Settings, o: &Open) -> Result<Request, u32> {
        let dll = settings.dll;
        let verb = narrow(dll, &wide(o.verb, None)?)?;
        let verb = if verb.is_empty() { copy(b"GET")? } else { verb };
        let version = narrow(dll, &wide(o.version, None)?)?;
        let version = if version.is_empty() { copy(b"HTTP/1.1")? } else { version };
        // The path on the wire: UTF-8, spaces and non-ASCII escaped
        // (unless the caller asked not to).
        let object = wide(o.object, None)?;
        let mut path = Vec::new();
        if object.first() != Some(&u16::from(b'/')) && !object.starts_with(&[u16::from(b'*')]) {
            put(&mut path, b"/")?;
        }
        for c in char::decode_utf16(object.iter().copied()) {
            let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut utf8 = [0u8; 4];
            for &b in c.encode_utf8(&mut utf8).as_bytes() {
                if (b == b' ' || b >= 0x80) && o.escape {
                    put(&mut path, &[b'%', hex(b >> 4), hex(b)])?;
                } else {
                    put(&mut path, &[b])?;
                }
            }
        }
        let mut r = Request {
            settings,
            security_flags: 0,
            host: Vec::new(),
            port: o.port,
            secure: o.secure,
            verb,
            path,
            version,
            headers: Vec::new(),
            api_added: Vec::new(),
            state: State::Open,
            conn: None,
            reused: false,
            keep: false,
            to_write: 0,
            sent_body: Vec::new(),
            streamed: false,
            redirects: 0,
            status: 0,
            reason: Vec::new(),
            response_version: Vec::new(),
            response: Vec::new(),
            rx: Vec::new(),
            body: Vec::new(),
            body_at: 0,
            framing: Framing::Done,
            observer: None,
        };
        put(&mut r.host, o.host)?;
        if r.port == 0 {
            // INTERNET_DEFAULT_PORT: the scheme's.
            r.port = if r.secure { 443 } else { 80 };
        }
        let referrer = narrow(dll, &wide(o.referrer, None)?)?;
        if !referrer.is_empty() {
            r.add(b"Referer", &referrer)?;
        }
        let accept = o.accept;
        if !accept.is_null() {
            let mut list = Vec::new();
            for i in 0.. {
                // SAFETY: the caller's NULL-terminated array of strings.
                let p = unsafe { *accept.add(i) };
                if p.is_null() {
                    break;
                }
                if !list.is_empty() {
                    put(&mut list, b", ")?;
                }
                put(&mut list, &narrow(dll, &wide(p, None)?)?)?;
            }
            if !list.is_empty() {
                r.add(b"Accept", &list)?;
            }
        }
        Ok(r)
    }

    fn add(&mut self, name: &[u8], value: &[u8]) -> Result<(), u32> {
        let h = (copy(name)?, copy(value)?);
        self.headers.try_reserve(1).map_err(|_| NOT_ENOUGH_MEMORY)?;
        self.headers.push(h);
        Ok(())
    }

    /// A header the API adds (Host, User-Agent, ...): replaced by the
    /// next send's or redirect's.
    fn add_own(&mut self, name: &[u8], value: &[u8]) -> Result<(), u32> {
        let n = copy(name)?;
        self.api_added.try_reserve(1).map_err(|_| NOT_ENOUGH_MEMORY)?;
        self.add(name, value)?;
        self.api_added.push(n);
        Ok(())
    }

    fn has(&self, name: &[u8]) -> bool {
        self.headers.iter().any(|h| same(&h.0, name))
    }

    /// The added headers' lines ("Name: value", CRLF-separated).
    pub fn add_headers(&mut self, text: &[u16], modifiers: u32) -> Result<(), u32> {
        let text = narrow(self.settings.dll, text)?;
        if modifiers & (ADDREQ_FLAG_COALESCE_WITH_COMMA | ADDREQ_FLAG_COALESCE_WITH_SEMICOLON) != 0
        {
            // Added as further fields instead of merged into one.
            missing(self.settings.dll, b"ADDREQ_FLAG_COALESCE", modifiers.into());
        }
        for line in text.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() {
                continue;
            }
            let colon = line.iter().position(|&b| b == b':').ok_or(INVALID_HEADER)?;
            let (name, value) = (trim(&line[..colon]), trim(&line[colon + 1..]));
            if name.is_empty() {
                return Err(INVALID_HEADER);
            }
            if modifiers & ADDREQ_FLAG_REPLACE != 0 {
                self.headers.retain(|h| !same(&h.0, name));
                if value.is_empty() {
                    continue;
                }
            } else if modifiers & ADDREQ_FLAG_ADD_IF_NEW != 0 && self.has(name) {
                return Err(HEADER_ALREADY_EXISTS);
            }
            // ADDREQ_FLAG_ADD is the default: added.
            self.add(name, value)?;
        }
        Ok(())
    }

    /// The request line and headers, with the ones the API adds (Host,
    /// User-Agent, Content-Length, Connection) unless the caller gave them.
    fn build(&mut self, total: u64) -> Result<Vec<u8>, u32> {
        let added = core::mem::take(&mut self.api_added);
        self.headers.retain(|h| !added.iter().any(|n| same(n, &h.0)));
        let mut host = narrow(self.settings.dll, &self.host)?;
        if self.port != if self.secure { 443 } else { 80 } {
            put(&mut host, b":")?;
            decimal(self.port.into(), &mut host)?;
        }
        if !self.has(b"Host") {
            self.add_own(b"Host", &host)?;
        }
        if !self.settings.agent.is_empty() && !self.has(b"User-Agent") {
            let agent = copy(&self.settings.agent)?;
            self.add_own(b"User-Agent", &agent)?;
        }
        let bodyless = same(&self.verb, b"GET") || same(&self.verb, b"HEAD");
        if (total > 0 || !bodyless)
            && !self.has(b"Content-Length")
            && !self.has(b"Transfer-Encoding")
        {
            let mut n = Vec::new();
            decimal(total, &mut n)?;
            self.add_own(b"Content-Length", &n)?;
        }
        if !self.has(b"Connection") {
            self.add_own(b"Connection", b"Keep-Alive")?;
        }
        let mut out = Vec::new();
        for part in [&self.verb[..], b" ", &self.path, b" ", &self.version, b"\r\n"] {
            put(&mut out, part)?;
        }
        for (name, value) in &self.headers {
            for part in [&name[..], b": ", value, b"\r\n"] {
                put(&mut out, part)?;
            }
        }
        put(&mut out, b"\r\n")?;
        Ok(out)
    }

    /// Sends the request with `headers` added, `optional` as the body's
    /// start and `total` bytes of body in all (the rest by `write_data`).
    pub fn send(&mut self, headers: &[u16], optional: &[u8], total: u64) -> Result<(), u32> {
        if !headers.is_empty() {
            self.add_headers(headers, ADDREQ_FLAG_ADD)?;
        }
        let total = total.max(optional.len() as u64);
        self.sent_body.clear();
        put(&mut self.sent_body, optional)?;
        self.streamed = total > optional.len() as u64;
        self.redirects = 0;
        self.transmit(total, true)
    }

    /// The pool's key for this request's server.
    fn key(&self) -> Result<Key, u32> {
        let mut host = Vec::new();
        put(&mut host, &self.host)?;
        let (security_flags, protocols) =
            if self.secure { (self.security_flags, self.settings.protocols) } else { (0, 0) };
        Ok(Key { host, port: self.port, secure: self.secure, security_flags, protocols })
    }

    /// Sends the request line, the headers and `sent_body` (`total` bytes
    /// of body in all) on an idle connection to the server if `pooled`
    /// and one waits, else on a new one.
    fn transmit(&mut self, total: u64, pooled: bool) -> Result<(), u32> {
        // A new send starts over (a body not read to its end leaves its
        // connection unusable: it goes).
        self.conn = None;
        self.reused = false;
        self.keep = false;
        self.state = State::Open;
        self.response.clear();
        self.rx.clear();
        self.body.clear();
        self.body_at = 0;
        let mut message = self.build(total)?;
        put(&mut message, &self.sent_body)?;
        let t = self.settings.timeouts;
        if pooled && let Some(mut conn) = pool::take(&self.key()?) {
            conn.set_timeouts(t.send, t.receive);
            Observer::tell(self.observer.as_ref(), Event::Sending);
            // A write that fails found the connection closed: a new one.
            if conn.write(&message).is_ok() {
                let sent = message.len().min(u32::MAX as usize) as u32;
                Observer::tell(self.observer.as_ref(), Event::Sent(sent));
                self.conn = Some(conn);
                self.reused = true;
                self.to_write = total - self.sent_body.len() as u64;
                self.state = State::Sent;
                return Ok(());
            }
        }
        // IPv6 literals go to the resolver without their brackets.
        let host = match &self.host[..] {
            [b, inner @ .., e] if *b == u16::from(b'[') && *e == u16::from(b']') => inner,
            h => h,
        };
        let mut conn = Conn::open(&Target {
            dll: self.settings.dll,
            host,
            port: self.port,
            secure: self.secure,
            security_flags: self.security_flags,
            protocols: self.settings.protocols,
            connect_timeout: t.connect,
            send_timeout: t.send,
            receive_timeout: t.receive,
            observer: self.observer,
        })?;
        Observer::tell(self.observer.as_ref(), Event::Sending);
        conn.write(&message)?;
        let sent = message.len().min(u32::MAX as usize) as u32;
        Observer::tell(self.observer.as_ref(), Event::Sent(sent));
        self.conn = Some(conn);
        self.to_write = total - self.sent_body.len() as u64;
        self.state = State::Sent;
        Ok(())
    }

    /// The request's URL (WINHTTP_OPTION_URL): the scheme, the host, the
    /// port unless the scheme's, the path as sent.
    pub fn url(&self) -> Result<Vec<u16>, u32> {
        let mut v: Vec<u16> = Vec::new();
        let scheme: &[u8] = if self.secure { b"https://" } else { b"http://" };
        let mut port = Vec::new();
        if self.port != if self.secure { 443 } else { 80 } {
            put(&mut port, b":")?;
            decimal(self.port.into(), &mut port)?;
        }
        v.try_reserve(scheme.len() + self.host.len() + port.len() + self.path.len())
            .map_err(|_| NOT_ENOUGH_MEMORY)?;
        v.extend(scheme.iter().map(|&b| u16::from(b)));
        v.extend_from_slice(&self.host);
        v.extend(port.iter().chain(&self.path).map(|&b| u16::from(b)));
        Ok(v)
    }

    /// The URL a redirect's Location names: absolute (http or https),
    /// scheme-relative, an absolute path, a query, or a path relative to
    /// the request's (RFC 3986 section 5.2); the fragment dropped.
    fn resolve(&self, location: &[u8]) -> Result<Next, u32> {
        let dll = self.settings.dll;
        let location = match location.iter().position(|&b| b == b'#') {
            Some(i) => &location[..i],
            None => location,
        };
        let scheme_end = location.iter().position(|&b| matches!(b, b':' | b'/' | b'?'));
        let has_scheme = scheme_end.is_some_and(|i| {
            i > 0
                && location[i] == b':'
                && location[..i].iter().all(|&b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        });
        if has_scheme || location.starts_with(b"//") {
            let mut wide_url: Vec<u16> = Vec::new();
            let scheme: &[u8] = match (has_scheme, self.secure) {
                (true, _) => b"",
                (false, true) => b"https:",
                (false, false) => b"http:",
            };
            wide_url.try_reserve(scheme.len() + location.len()).map_err(|_| NOT_ENOUGH_MEMORY)?;
            wide_url.extend(scheme.iter().chain(location).map(|&b| u16::from(b)));
            let p = url::parse(&wide_url).map_err(|e| {
                // Another scheme (ftp:, ...) or no host.
                failed(dll, b"redirect: Location is no http(s) URL", e.into());
                REDIRECT_FAILED
            })?;
            let mut host = Vec::new();
            put(&mut host, &wide_url[p.host.0..p.host.1])?;
            let rest: Vec<u8> = narrow(dll, &wide_url[p.path.0..])?;
            let mut path = Vec::new();
            if !rest.starts_with(b"/") {
                put(&mut path, b"/")?;
            }
            put(&mut path, &rest)?;
            return Ok(Next { secure: p.secure, host, port: p.port, path: escaped(&path)? });
        }
        let current = match self.path.iter().position(|&b| b == b'?') {
            Some(i) => &self.path[..i],
            None => &self.path[..],
        };
        let mut path = Vec::new();
        match location.first() {
            Some(b'/') => {}
            Some(b'?') | None => put(&mut path, current)?,
            Some(_) => {
                let dir = current.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
                put(&mut path, &current[..dir])?;
                if path.is_empty() {
                    put(&mut path, b"/")?;
                }
            }
        }
        put(&mut path, location)?;
        let mut host = Vec::new();
        put(&mut host, &self.host)?;
        Ok(Next {
            secure: self.secure,
            host,
            port: self.port,
            path: escaped(&without_dots(&path)?)?,
        })
    }

    /// Where the response just received redirects to, if the policy
    /// follows it; a redirect it refuses is REDIRECT_FAILED.
    fn redirect(&self) -> Result<Option<Next>, u32> {
        let dll = self.settings.dll;
        let rules = self.settings.redirect_rules;
        let redirects = match self.status {
            301 | 302 | 307 | 308 => true,
            303 => rules == RedirectRules::WinInet,
            _ => false,
        };
        if !redirects || self.settings.redirect_policy == REDIRECT_NEVER {
            return Ok(None);
        }
        let Some(location) = self.header(b"Location") else {
            return Ok(None);
        };
        if self.redirects >= self.settings.max_redirects {
            failed(dll, b"redirect: more than the maximum", self.redirects.into());
            return Err(REDIRECT_FAILED);
        }
        let next = self.resolve(trim(location))?;
        if self.secure
            && !next.secure
            && self.settings.redirect_policy == REDIRECT_DISALLOW_HTTPS_TO_HTTP
        {
            failed(dll, b"redirect: https to http (REDIRECT_POLICY)", self.status.into());
            return Err(REDIRECT_FAILED);
        }
        if matches!(self.status, 307 | 308) && self.streamed {
            // The body written with WriteData is not kept to send again:
            // the 3xx response is returned.
            missing(dll, b"redirect of a body written with WriteData", self.status.into());
            return Ok(None);
        }
        Ok(Some(next))
    }

    /// Sends the request again to where `next` says.
    fn follow(&mut self, next: Next) -> Result<(), u32> {
        self.redirects += 1;
        let to_get = match self.settings.redirect_rules {
            RedirectRules::WinHttp => matches!(self.status, 301 | 302) && same(&self.verb, b"POST"),
            RedirectRules::WinInet => {
                !matches!(self.status, 307 | 308)
                    && !same(&self.verb, b"GET")
                    && !same(&self.verb, b"HEAD")
            }
        };
        if to_get {
            self.verb = copy(b"GET")?;
            self.sent_body.clear();
            self.streamed = false;
        }
        // The Host is the new one's, even one the caller gave.
        self.headers.retain(|h| !same(&h.0, b"Host"));
        self.secure = next.secure;
        self.host = next.host;
        self.port = next.port;
        self.path = next.path;
        if self.observer.is_some() {
            let mut url = self.url()?;
            put(&mut url, &[0])?;
            Observer::tell(self.observer.as_ref(), Event::Redirect(&url));
        }
        let total = self.sent_body.len() as u64;
        self.transmit(total, true)
    }

    pub fn write_data(&mut self, data: &[u8]) -> Result<(), u32> {
        if self.state != State::Sent {
            return Err(INCORRECT_HANDLE_STATE);
        }
        let conn = self.conn.as_mut().ok_or(INCORRECT_HANDLE_STATE)?;
        conn.write(data)?;
        self.to_write = self.to_write.saturating_sub(data.len() as u64);
        Ok(())
    }

    /// Reads into `rx` until it holds a whole header block; its length.
    fn read_head(&mut self) -> Result<usize, u32> {
        let mut scanned = 0;
        loop {
            // The end: an empty line (CRLF CRLF, or bare LFs).
            let mut i = scanned;
            while i < self.rx.len() {
                if self.rx[i] == b'\n' {
                    if self.rx[i + 1..].starts_with(b"\r\n") {
                        return Ok(i + 3);
                    }
                    if self.rx[i + 1..].starts_with(b"\n") {
                        return Ok(i + 2);
                    }
                }
                i += 1;
            }
            scanned = self.rx.len().saturating_sub(2);
            if self.rx.len() > MAX_HEADERS {
                failed(self.settings.dll, b"response headers too large", self.rx.len() as u64);
                return Err(HEADER_SIZE_OVERFLOW);
            }
            let conn = self.conn.as_mut().ok_or(INCORRECT_HANDLE_STATE)?;
            let closed = match conn.read(&mut self.rx) {
                // A reused connection that ends (or fails) before any of
                // the response: the server closed it while it was idle.
                Err(_) | Ok(true) if self.reused && self.rx.is_empty() => return Err(STALE),
                r => r?,
            };
            self.reused = false;
            if closed {
                let n = self.rx.len() as u64;
                failed(self.settings.dll, b"closed before the response headers", n);
                return Err(if self.rx.is_empty() {
                    CONNECTION_ERROR
                } else {
                    INVALID_SERVER_RESPONSE
                });
            }
        }
    }

    /// Parses a header block (status line, fields; obs-fold continued).
    fn parse_head(&mut self, head: &[u8]) -> Result<(), u32> {
        let mut lines = head.split(|&b| b == b'\n');
        let status = trim(lines.next().unwrap_or(b""));
        let dll = self.settings.dll;
        let invalid = || {
            failed(dll, b"invalid status line", 0);
            INVALID_SERVER_RESPONSE
        };
        if !status.starts_with(b"HTTP/") {
            return Err(invalid());
        }
        let sp = status.iter().position(|&b| b == b' ').ok_or_else(invalid)?;
        let rest = &status[sp + 1..];
        let code = rest.get(..3).ok_or_else(invalid)?;
        if !code.iter().all(u8::is_ascii_digit) || rest.get(3).is_some_and(|&b| b != b' ') {
            return Err(invalid());
        }
        self.status = code.iter().fold(0, |n, &d| n * 10 + u16::from(d - b'0'));
        self.response_version = copy(&status[..sp])?;
        self.reason = copy(trim(rest.get(4..).unwrap_or(b"")))?;
        self.response.clear();
        for line in lines {
            let raw = line.strip_suffix(b"\r").unwrap_or(line);
            if raw.is_empty() {
                continue;
            }
            if matches!(raw[0], b' ' | b'\t') {
                // obs-fold: continues the previous field's value.
                if let Some(last) = self.response.last_mut() {
                    put(&mut last.1, b" ")?;
                    put(&mut last.1, trim(raw))?;
                }
                continue;
            }
            let Some(colon) = raw.iter().position(|&b| b == b':') else {
                failed(dll, b"invalid response header", 0);
                return Err(INVALID_SERVER_RESPONSE);
            };
            let h = (copy(trim(&raw[..colon]))?, copy(trim(&raw[colon + 1..]))?);
            self.response.try_reserve(1).map_err(|_| NOT_ENOUGH_MEMORY)?;
            self.response.push(h);
        }
        Ok(())
    }

    fn header(&self, name: &[u8]) -> Option<&[u8]> {
        self.response.iter().find(|h| same(&h.0, name)).map(|h| &h.1[..])
    }

    /// Reads the response's status line and headers.
    pub fn receive(&mut self) -> Result<(), u32> {
        if self.state != State::Sent {
            return Err(INCORRECT_HANDLE_STATE);
        }
        if self.to_write > 0 {
            // The body is not all written yet.
            return Err(INCORRECT_HANDLE_STATE);
        }
        loop {
            self.receive_head()?;
            match self.redirect()? {
                Some(next) => self.follow(next)?,
                None => break,
            }
        }
        self.state = State::Received;
        if self.framing == Framing::Done {
            self.finish();
        }
        Ok(())
    }

    /// Reads the final response's status line and headers on the
    /// current connection and decides the body's framing.
    fn receive_head(&mut self) -> Result<(), u32> {
        if let Some(c) = self.conn.as_mut() {
            c.receive_timeout = self.settings.timeouts.response;
        }
        Observer::tell(self.observer.as_ref(), Event::Receiving);
        let mut received = 0usize;
        loop {
            let end = match self.read_head() {
                Err(STALE) if !self.streamed => {
                    // Sent again on a new connection.
                    let total = self.sent_body.len() as u64;
                    self.transmit(total, false)?;
                    if let Some(c) = self.conn.as_mut() {
                        c.receive_timeout = self.settings.timeouts.response;
                    }
                    self.read_head()?
                }
                Err(STALE) => {
                    failed(self.settings.dll, b"reused connection closed", 0);
                    return Err(CONNECTION_ERROR);
                }
                r => r?,
            };
            received += end;
            let head = copy(&self.rx[..end])?;
            self.rx.drain(..end);
            self.parse_head(&head)?;
            // Interim responses (100 Continue, 103 ...) are skipped;
            // 101 is final.
            if self.status < 100 || self.status >= 200 || self.status == 101 {
                break;
            }
        }
        let received = received.min(u32::MAX as usize) as u32;
        Observer::tell(self.observer.as_ref(), Event::Received(received));
        if let Some(c) = self.conn.as_mut() {
            c.receive_timeout = self.settings.timeouts.receive;
        }
        let chunked = self.header(b"Transfer-Encoding").is_some_and(|v| {
            v.rsplit(|&b| b == b',').next().is_some_and(|last| same(trim(last), b"chunked"))
        });
        self.framing = if same(&self.verb, b"HEAD")
            || self.status == 204
            || self.status == 304
            || self.status == 101
        {
            Framing::Done
        } else if chunked {
            Framing::Chunked(Chunk::Size)
        } else if let Some(v) = self.header(b"Content-Length") {
            let v = trim(v);
            if v.is_empty() || !v.iter().all(u8::is_ascii_digit) || v.len() > 19 {
                failed(self.settings.dll, b"invalid Content-Length", 0);
                return Err(INVALID_SERVER_RESPONSE);
            }
            Framing::Length(v.iter().fold(0, |n, &d| n * 10 + u64::from(d - b'0')))
        } else {
            Framing::UntilClose
        };
        self.keep = self.framing != Framing::UntilClose && self.status != 101 && self.persistent();
        Ok(())
    }

    /// Whether the connection persists after this exchange (RFC 9112
    /// section 9.3): neither side said `Connection: close`, and the
    /// response is HTTP/1.1 or later, or HTTP/1.0 with `keep-alive`.
    fn persistent(&self) -> bool {
        let has = |list: Option<&[u8]>, token: &[u8]| {
            list.is_some_and(|v| v.split(|&b| b == b',').any(|t| same(trim(t), token)))
        };
        let asked =
            self.headers.iter().filter(|h| same(&h.0, b"Connection")).map(|h| &h.1[..]).next();
        if has(asked, b"close") || has(self.header(b"Connection"), b"close") {
            return false;
        }
        if same(&self.response_version, b"HTTP/1.0") {
            return has(self.header(b"Connection"), b"keep-alive");
        }
        self.response_version.len() > 5
    }

    /// The body is at its end: the connection goes back to the pool if it
    /// persists and nothing past the response came, else it closes.
    fn finish(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        if self.keep
            && self.rx.is_empty()
            && let Ok(key) = self.key()
        {
            pool::put(key, conn);
        }
    }

    /// Moves body bytes from `rx` to `body` by the framing.
    fn decode(&mut self) -> Result<(), u32> {
        let mut at = 0;
        let dll = self.settings.dll;
        let bad = |what: &[u8]| {
            failed(dll, what, 0);
            INVALID_SERVER_RESPONSE
        };
        loop {
            let rest = &self.rx[at..];
            match self.framing {
                Framing::Done => break,
                Framing::UntilClose => {
                    put(&mut self.body, rest)?;
                    at = self.rx.len();
                    break;
                }
                Framing::Length(n) | Framing::Chunked(Chunk::Data(n)) => {
                    let k = (rest.len() as u64).min(n) as usize;
                    put(&mut self.body, &rest[..k])?;
                    at += k;
                    let left = n - k as u64;
                    self.framing = match self.framing {
                        Framing::Length(_) if left == 0 => Framing::Done,
                        Framing::Length(_) => Framing::Length(left),
                        _ if left == 0 => Framing::Chunked(Chunk::DataEnd),
                        _ => Framing::Chunked(Chunk::Data(left)),
                    };
                    if left > 0 {
                        break;
                    }
                }
                Framing::Chunked(state) => {
                    let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
                        if rest.len() > 4096 {
                            return Err(bad(b"invalid chunk line"));
                        }
                        break;
                    };
                    let line = trim(&rest[..nl]);
                    at += nl + 1;
                    self.framing = Framing::Chunked(match state {
                        Chunk::Size => {
                            let digits = line.split(|&b| b == b';').next().unwrap_or(b"");
                            let digits = trim(digits);
                            if digits.is_empty() || digits.len() > 15 {
                                return Err(bad(b"invalid chunk size"));
                            }
                            let mut n: u64 = 0;
                            for &d in digits {
                                let v = (d as char)
                                    .to_digit(16)
                                    .ok_or_else(|| bad(b"invalid chunk size"))?;
                                n = n * 16 + u64::from(v);
                            }
                            if n == 0 { Chunk::Trailer } else { Chunk::Data(n) }
                        }
                        Chunk::DataEnd => {
                            if !line.is_empty() {
                                return Err(bad(b"no CRLF after a chunk"));
                            }
                            Chunk::Size
                        }
                        Chunk::Trailer if line.is_empty() => {
                            self.framing = Framing::Done;
                            continue;
                        }
                        // A trailer field (not kept).
                        Chunk::Trailer => Chunk::Trailer,
                        Chunk::Data(_) => unreachable!(),
                    });
                }
            }
        }
        self.rx.drain(..at);
        if self.framing == Framing::Done {
            self.finish();
        }
        Ok(())
    }

    /// Makes body bytes available (reading if none are); 0 at the end.
    pub fn available(&mut self) -> Result<usize, u32> {
        if self.state != State::Received {
            return Err(INCORRECT_HANDLE_STATE);
        }
        loop {
            if self.body_at == self.body.len() {
                self.body.clear();
                self.body_at = 0;
            }
            self.decode()?;
            if self.body.len() > self.body_at || self.framing == Framing::Done {
                return Ok(self.body.len() - self.body_at);
            }
            let Some(conn) = self.conn.as_mut() else {
                return Ok(0);
            };
            if conn.read(&mut self.rx)? {
                self.conn = None;
                if self.framing == Framing::UntilClose {
                    self.framing = Framing::Done;
                } else {
                    failed(self.settings.dll, b"closed before the end of the body", 0);
                    self.framing = Framing::Done;
                    return Err(CONNECTION_ERROR);
                }
            }
        }
    }

    /// Reads body bytes (those available, at least one unless at the
    /// end); 0 at the end.
    pub fn read_body(&mut self, out: &mut [u8]) -> Result<usize, u32> {
        if out.is_empty() {
            return Ok(0);
        }
        let n = self.available()?.min(out.len());
        out[..n].copy_from_slice(&self.body[self.body_at..self.body_at + n]);
        self.body_at += n;
        Ok(n)
    }

    /// The text a query level names (the `index`th field for names;
    /// `next` is the index after it).
    fn query(&self, level: u32, name: &[u16], index: u32) -> Result<(Vec<u8>, u32), u32> {
        let query = level & 0xffff;
        let request = level & QUERY_FLAG_REQUEST_HEADERS != 0;
        let fields = if request { &self.headers } else { &self.response };
        let fixed = |v: &[u8]| -> Result<(Vec<u8>, u32), u32> {
            if index > 0 {
                return Err(HEADER_NOT_FOUND);
            }
            Ok((copy(v)?, 1))
        };
        match query {
            QUERY_REQUEST_METHOD if request => return fixed(&self.verb),
            QUERY_VERSION if request => return fixed(&self.version),
            QUERY_VERSION => return fixed(&self.response_version),
            QUERY_STATUS_TEXT if !request => return fixed(&self.reason),
            QUERY_STATUS_CODE if !request => {
                let mut v = Vec::new();
                decimal(self.status.into(), &mut v)?;
                return fixed(&v);
            }
            QUERY_RAW_HEADERS | QUERY_RAW_HEADERS_CRLF => {
                let (sep, end): (&[u8], &[u8]) =
                    if query == QUERY_RAW_HEADERS { (b"\0", b"\0") } else { (b"\r\n", b"\r\n") };
                let mut v = Vec::new();
                if request {
                    for part in [&self.verb[..], b" ", &self.path, b" ", &self.version, sep] {
                        put(&mut v, part)?;
                    }
                } else {
                    put(&mut v, &self.response_version)?;
                    put(&mut v, b" ")?;
                    decimal(self.status.into(), &mut v)?;
                    for part in [b" ", &self.reason[..], sep] {
                        put(&mut v, part)?;
                    }
                }
                for (n, value) in fields {
                    for part in [&n[..], b": ", value, sep] {
                        put(&mut v, part)?;
                    }
                }
                put(&mut v, end)?;
                return fixed(&v);
            }
            _ => {}
        }
        let custom;
        let wanted: &[u8] = if query == QUERY_CUSTOM {
            custom = narrow(self.settings.dll, name)?;
            &custom
        } else {
            match QUERY_NAMES.get(query as usize) {
                Some(n) if !n.is_empty() => n,
                _ => {
                    missing(self.settings.dll, b"query level", level.into());
                    return Err(INVALID_QUERY_REQUEST);
                }
            }
        };
        match fields.iter().filter(|h| same(&h.0, wanted)).nth(index as usize) {
            Some(h) => Ok((copy(&h.1)?, index + 1)),
            None => Err(HEADER_NOT_FOUND),
        }
    }

    /// Answers a header query (WinHttpQueryHeaders, HttpQueryInfoW/A): the
    /// value as UTF-16 (`ansi`: as its bytes, HttpQueryInfoA) with its NUL
    /// (`*length` then its bytes without the NUL; ERROR_INSUFFICIENT_BUFFER
    /// with the bytes needed, the NUL included), or with QUERY_FLAG_NUMBER
    /// / NUMBER64 as a number.
    /// `name` is QUERY_CUSTOM's header name; `index` (in: which
    /// occurrence, out: the next) may be null.
    ///
    /// # Safety
    /// `length` is the caller's DWORD and `buffer` its buffer of
    /// `*length` bytes (or null); `index` null or the caller's DWORD.
    pub unsafe fn query_info(
        &self,
        level: u32,
        name: &[u16],
        buffer: *mut u8,
        length: *mut u32,
        index: *mut u32,
        ansi: bool,
    ) -> Result<(), u32> {
        if length.is_null() {
            return Err(INVALID_PARAMETER);
        }
        let request = level & QUERY_FLAG_REQUEST_HEADERS != 0;
        if !request && self.state != State::Received {
            return Err(INCORRECT_HANDLE_STATE);
        }
        if level & QUERY_FLAG_SYSTEMTIME != 0 {
            missing(self.settings.dll, b"QUERY_FLAG_SYSTEMTIME", level.into());
            return Err(NOT_SUPPORTED);
        }
        if level & 0xffff == QUERY_CUSTOM && name.is_empty() {
            return Err(INVALID_PARAMETER);
        }
        // SAFETY: the caller's DWORD (in: which occurrence).
        let at = if index.is_null() { 0 } else { unsafe { *index } };
        let (value, next) = self.query(level, name, at)?;
        // SAFETY: the caller's length, buffer of `*length` bytes and index.
        unsafe {
            if level & (QUERY_FLAG_NUMBER | QUERY_FLAG_NUMBER64) != 0 {
                let wide64 = level & QUERY_FLAG_NUMBER64 != 0;
                let size = if wide64 { 8 } else { 4 };
                let digits = trim(&value);
                if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                    return Err(INVALID_QUERY_REQUEST);
                }
                let n = digits
                    .iter()
                    .fold(0u64, |n, &d| n.wrapping_mul(10).wrapping_add(u64::from(d - b'0')));
                if buffer.is_null() || *length < size {
                    *length = size;
                    return Err(INSUFFICIENT_BUFFER);
                }
                if wide64 {
                    buffer.cast::<u64>().write_unaligned(n);
                } else {
                    buffer.cast::<u32>().write_unaligned(n as u32);
                }
                *length = size;
            } else if ansi {
                let needed = value.len() as u32 + 1;
                if buffer.is_null() || *length < needed {
                    *length = needed;
                    return Err(INSUFFICIENT_BUFFER);
                }
                core::ptr::copy_nonoverlapping(value.as_ptr(), buffer, value.len());
                buffer.add(value.len()).write(0);
                *length = value.len() as u32;
            } else {
                let needed = (value.len() as u32 + 1) * 2;
                if buffer.is_null() || *length < needed {
                    *length = needed;
                    return Err(INSUFFICIENT_BUFFER);
                }
                let out = buffer.cast::<u16>();
                for (i, &b) in value.iter().enumerate() {
                    out.add(i).write_unaligned(b.into());
                }
                out.add(value.len()).write_unaligned(0);
                *length = value.len() as u32 * 2;
            }
            if !index.is_null() {
                *index = next;
            }
        }
        Ok(())
    }
}

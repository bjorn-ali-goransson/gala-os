//! A request: its headers, WinHttpSendRequest/WriteData (the request on
//! its own connection), WinHttpReceiveResponse (the status line and
//! headers, 1xx responses skipped), WinHttpQueryHeaders and the body
//! (WinHttpQueryDataAvailable/ReadData) by Content-Length, chunked
//! transfer coding or until the server closes; HEAD, 204 and 304 have
//! none.

use alloc::vec::Vec;

use crate::net::{Conn, Target};
use crate::{
    Connect, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_WINHTTP_CONNECTION_ERROR,
    ERROR_WINHTTP_HEADER_ALREADY_EXISTS, ERROR_WINHTTP_HEADER_NOT_FOUND,
    ERROR_WINHTTP_HEADER_SIZE_OVERFLOW, ERROR_WINHTTP_INCORRECT_HANDLE_STATE,
    ERROR_WINHTTP_INVALID_HEADER, ERROR_WINHTTP_INVALID_QUERY_REQUEST,
    ERROR_WINHTTP_INVALID_SERVER_RESPONSE, Settings, WINHTTP_FLAG_ESCAPE_DISABLE,
    WINHTTP_FLAG_SECURE, done, failed, missing, narrow, put, request_of, wide,
};

const WINHTTP_ADDREQ_FLAG_ADD_IF_NEW: u32 = 0x1000_0000;
const WINHTTP_ADDREQ_FLAG_ADD: u32 = 0x2000_0000;
const WINHTTP_ADDREQ_FLAG_COALESCE_WITH_COMMA: u32 = 0x4000_0000;
const WINHTTP_ADDREQ_FLAG_COALESCE_WITH_SEMICOLON: u32 = 0x0100_0000;
const WINHTTP_ADDREQ_FLAG_REPLACE: u32 = 0x8000_0000;

const WINHTTP_QUERY_VERSION: u32 = 18;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_STATUS_TEXT: u32 = 20;
const WINHTTP_QUERY_RAW_HEADERS: u32 = 21;
const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
const WINHTTP_QUERY_REQUEST_METHOD: u32 = 45;
const WINHTTP_QUERY_CUSTOM: u32 = 65535;
const WINHTTP_QUERY_FLAG_REQUEST_HEADERS: u32 = 0x8000_0000;
const WINHTTP_QUERY_FLAG_SYSTEMTIME: u32 = 0x4000_0000;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const WINHTTP_QUERY_FLAG_NUMBER64: u32 = 0x0800_0000;

/// The headers WINHTTP_QUERY_* levels name ("Query Info Flags"); the
/// others are the status line's parts and the raw block.
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

/// The largest response header block (WinHTTP's default
/// MAX_RESPONSE_HEADER_SIZE, 64 KB).
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
    pub security_flags: u32,
    host: Vec<u16>,
    port: u16,
    secure: bool,
    verb: Vec<u8>,
    path: Vec<u8>,
    version: Vec<u8>,
    /// Headers added by the caller, then (once sent) the ones WinHTTP adds.
    headers: Vec<Header>,
    state: State,
    conn: Option<Conn>,
    /// Body bytes still to come through WinHttpWriteData.
    to_write: u64,
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

impl Request {
    pub fn new(
        c: &Connect,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept: *const *const u16,
        flags: u32,
    ) -> Result<Request, u32> {
        let verb = narrow(&wide(verb, None)?)?;
        let verb = if verb.is_empty() { copy(b"GET")? } else { verb };
        let version = narrow(&wide(version, None)?)?;
        let version = if version.is_empty() { copy(b"HTTP/1.1")? } else { version };
        // The path on the wire: UTF-8, spaces and non-ASCII escaped
        // (unless WINHTTP_FLAG_ESCAPE_DISABLE).
        let object = wide(object, None)?;
        let mut path = Vec::new();
        if object.first() != Some(&u16::from(b'/')) && !object.starts_with(&[u16::from(b'*')]) {
            put(&mut path, b"/")?;
        }
        for c in char::decode_utf16(object.iter().copied()) {
            let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut utf8 = [0u8; 4];
            for &b in c.encode_utf8(&mut utf8).as_bytes() {
                if (b == b' ' || b >= 0x80) && flags & WINHTTP_FLAG_ESCAPE_DISABLE == 0 {
                    put(&mut path, &[b'%', hex(b >> 4), hex(b)])?;
                } else {
                    put(&mut path, &[b])?;
                }
            }
        }
        let mut r = Request {
            settings: c.settings.clone(),
            security_flags: 0,
            host: Vec::new(),
            port: c.port,
            secure: flags & WINHTTP_FLAG_SECURE != 0,
            verb,
            path,
            version,
            headers: Vec::new(),
            state: State::Open,
            conn: None,
            to_write: 0,
            status: 0,
            reason: Vec::new(),
            response_version: Vec::new(),
            response: Vec::new(),
            rx: Vec::new(),
            body: Vec::new(),
            body_at: 0,
            framing: Framing::Done,
        };
        put(&mut r.host, &c.host)?;
        if r.port == 0 {
            // INTERNET_DEFAULT_PORT: the scheme's.
            r.port = if r.secure { 443 } else { 80 };
        }
        let referrer = narrow(&wide(referrer, None)?)?;
        if !referrer.is_empty() {
            r.add(b"Referer", &referrer)?;
        }
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
                put(&mut list, &narrow(&wide(p, None)?)?)?;
            }
            if !list.is_empty() {
                r.add(b"Accept", &list)?;
            }
        }
        Ok(r)
    }

    fn add(&mut self, name: &[u8], value: &[u8]) -> Result<(), u32> {
        let h = (copy(name)?, copy(value)?);
        self.headers.try_reserve(1).map_err(|_| crate::ERROR_NOT_ENOUGH_MEMORY)?;
        self.headers.push(h);
        Ok(())
    }

    fn has(&self, name: &[u8]) -> bool {
        self.headers.iter().any(|h| same(&h.0, name))
    }

    /// WinHttpAddRequestHeaders' lines ("Name: value", CRLF-separated).
    pub fn add_headers(&mut self, text: &[u16], modifiers: u32) -> Result<(), u32> {
        let text = narrow(text)?;
        if modifiers
            & (WINHTTP_ADDREQ_FLAG_COALESCE_WITH_COMMA
                | WINHTTP_ADDREQ_FLAG_COALESCE_WITH_SEMICOLON)
            != 0
        {
            // Added as further fields instead of merged into one.
            missing(b"winhttp.dll!WINHTTP_ADDREQ_FLAG_COALESCE", modifiers.into());
        }
        for line in text.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() {
                continue;
            }
            let colon = line.iter().position(|&b| b == b':').ok_or(ERROR_WINHTTP_INVALID_HEADER)?;
            let (name, value) = (trim(&line[..colon]), trim(&line[colon + 1..]));
            if name.is_empty() {
                return Err(ERROR_WINHTTP_INVALID_HEADER);
            }
            if modifiers & WINHTTP_ADDREQ_FLAG_REPLACE != 0 {
                self.headers.retain(|h| !same(&h.0, name));
                if value.is_empty() {
                    continue;
                }
            } else if modifiers & WINHTTP_ADDREQ_FLAG_ADD_IF_NEW != 0 && self.has(name) {
                return Err(ERROR_WINHTTP_HEADER_ALREADY_EXISTS);
            }
            let _ = WINHTTP_ADDREQ_FLAG_ADD; // the default: added.
            self.add(name, value)?;
        }
        Ok(())
    }

    /// The request line and headers, with the ones WinHTTP adds (Host,
    /// User-Agent, Content-Length, Connection) unless the caller gave them.
    fn build(&mut self, total: u64) -> Result<Vec<u8>, u32> {
        let mut host = narrow(&self.host)?;
        if self.port != if self.secure { 443 } else { 80 } {
            put(&mut host, b":")?;
            decimal(self.port.into(), &mut host)?;
        }
        if !self.has(b"Host") {
            self.add(b"Host", &host)?;
        }
        if !self.settings.agent.is_empty() && !self.has(b"User-Agent") {
            let agent = copy(&self.settings.agent)?;
            self.add(b"User-Agent", &agent)?;
        }
        let bodyless = same(&self.verb, b"GET") || same(&self.verb, b"HEAD");
        if (total > 0 || !bodyless)
            && !self.has(b"Content-Length")
            && !self.has(b"Transfer-Encoding")
        {
            let mut n = Vec::new();
            decimal(total, &mut n)?;
            self.add(b"Content-Length", &n)?;
        }
        if !self.has(b"Connection") {
            // Every request has its own connection (no reuse yet).
            self.add(b"Connection", b"Keep-Alive")?;
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

    fn send(&mut self, headers: &[u16], optional: &[u8], total: u64) -> Result<(), u32> {
        if !headers.is_empty() {
            self.add_headers(headers, WINHTTP_ADDREQ_FLAG_ADD)?;
        }
        // A new send starts over (its own connection).
        self.conn = None;
        self.state = State::Open;
        self.response.clear();
        self.rx.clear();
        self.body.clear();
        self.body_at = 0;
        let total = total.max(optional.len() as u64);
        let mut message = self.build(total)?;
        put(&mut message, optional)?;
        // IPv6 literals go to the resolver without their brackets.
        let host = match &self.host[..] {
            [b, inner @ .., e] if *b == u16::from(b'[') && *e == u16::from(b']') => inner,
            h => h,
        };
        let t = self.settings.timeouts;
        let mut conn = Conn::open(&Target {
            host,
            port: self.port,
            secure: self.secure,
            security_flags: self.security_flags,
            protocols: self.settings.protocols,
            send_timeout: t.send,
            receive_timeout: t.receive,
        })?;
        conn.write(&message)?;
        self.conn = Some(conn);
        self.to_write = total - optional.len() as u64;
        self.state = State::Sent;
        Ok(())
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), u32> {
        if self.state != State::Sent {
            return Err(ERROR_WINHTTP_INCORRECT_HANDLE_STATE);
        }
        let conn = self.conn.as_mut().ok_or(ERROR_WINHTTP_INCORRECT_HANDLE_STATE)?;
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
                failed(b"winhttp.dll!response headers too large", self.rx.len() as u64);
                return Err(ERROR_WINHTTP_HEADER_SIZE_OVERFLOW);
            }
            let conn = self.conn.as_mut().ok_or(ERROR_WINHTTP_INCORRECT_HANDLE_STATE)?;
            if conn.read(&mut self.rx)? {
                failed(b"winhttp.dll!closed before the response headers", self.rx.len() as u64);
                return Err(if self.rx.is_empty() {
                    ERROR_WINHTTP_CONNECTION_ERROR
                } else {
                    ERROR_WINHTTP_INVALID_SERVER_RESPONSE
                });
            }
        }
    }

    /// Parses a header block (status line, fields; obs-fold continued).
    fn parse_head(&mut self, head: &[u8]) -> Result<(), u32> {
        let mut lines = head.split(|&b| b == b'\n');
        let status = trim(lines.next().unwrap_or(b""));
        let invalid = || {
            failed(b"winhttp.dll!invalid status line", 0);
            ERROR_WINHTTP_INVALID_SERVER_RESPONSE
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
                failed(b"winhttp.dll!invalid response header", 0);
                return Err(ERROR_WINHTTP_INVALID_SERVER_RESPONSE);
            };
            let h = (copy(trim(&raw[..colon]))?, copy(trim(&raw[colon + 1..]))?);
            self.response.try_reserve(1).map_err(|_| crate::ERROR_NOT_ENOUGH_MEMORY)?;
            self.response.push(h);
        }
        Ok(())
    }

    fn header(&self, name: &[u8]) -> Option<&[u8]> {
        self.response.iter().find(|h| same(&h.0, name)).map(|h| &h.1[..])
    }

    fn receive(&mut self) -> Result<(), u32> {
        if self.state != State::Sent {
            return Err(ERROR_WINHTTP_INCORRECT_HANDLE_STATE);
        }
        if self.to_write > 0 {
            // The body is not all written yet.
            return Err(ERROR_WINHTTP_INCORRECT_HANDLE_STATE);
        }
        if let Some(c) = self.conn.as_mut() {
            c.receive_timeout = self.settings.timeouts.response;
        }
        loop {
            let end = self.read_head()?;
            let head = copy(&self.rx[..end])?;
            self.rx.drain(..end);
            self.parse_head(&head)?;
            // Interim responses (100 Continue, 103 ...) are skipped;
            // 101 is final.
            if self.status < 100 || self.status >= 200 || self.status == 101 {
                break;
            }
        }
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
                failed(b"winhttp.dll!invalid Content-Length", 0);
                return Err(ERROR_WINHTTP_INVALID_SERVER_RESPONSE);
            }
            Framing::Length(v.iter().fold(0, |n, &d| n * 10 + u64::from(d - b'0')))
        } else {
            Framing::UntilClose
        };
        if matches!(self.status, 301 | 302 | 303 | 307 | 308)
            && self.settings.redirect_policy != 0
            && self.header(b"Location").is_some()
        {
            // Windows follows it (WINHTTP_OPTION_REDIRECT_POLICY); gala
            // returns the redirect itself.
            missing(b"winhttp.dll!automatic redirect", self.status.into());
        }
        self.state = State::Received;
        if self.framing == Framing::Done {
            self.conn = None;
        }
        Ok(())
    }

    /// Moves body bytes from `rx` to `body` by the framing.
    fn decode(&mut self) -> Result<(), u32> {
        let mut at = 0;
        let bad = |what: &[u8]| {
            failed(what, 0);
            ERROR_WINHTTP_INVALID_SERVER_RESPONSE
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
                            return Err(bad(b"winhttp.dll!invalid chunk line"));
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
                                return Err(bad(b"winhttp.dll!invalid chunk size"));
                            }
                            let mut n: u64 = 0;
                            for &d in digits {
                                let v = (d as char)
                                    .to_digit(16)
                                    .ok_or_else(|| bad(b"winhttp.dll!invalid chunk size"))?;
                                n = n * 16 + u64::from(v);
                            }
                            if n == 0 { Chunk::Trailer } else { Chunk::Data(n) }
                        }
                        Chunk::DataEnd => {
                            if !line.is_empty() {
                                return Err(bad(b"winhttp.dll!no CRLF after a chunk"));
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
            // No reuse yet: the connection ends with the body.
            self.conn = None;
        }
        Ok(())
    }

    /// Makes body bytes available (reading if none are); 0 at the end.
    fn available(&mut self) -> Result<usize, u32> {
        if self.state != State::Received {
            return Err(ERROR_WINHTTP_INCORRECT_HANDLE_STATE);
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
                    failed(b"winhttp.dll!closed before the end of the body", 0);
                    self.framing = Framing::Done;
                    return Err(ERROR_WINHTTP_CONNECTION_ERROR);
                }
            }
        }
    }

    fn read_body(&mut self, out: &mut [u8]) -> Result<usize, u32> {
        if out.is_empty() {
            return Ok(0);
        }
        let n = self.available()?.min(out.len());
        out[..n].copy_from_slice(&self.body[self.body_at..self.body_at + n]);
        self.body_at += n;
        Ok(n)
    }

    /// The text a WinHttpQueryHeaders level names (the `index`th field
    /// for names; `next` is the index after it).
    fn query(&self, level: u32, name: &[u16], index: u32) -> Result<(Vec<u8>, u32), u32> {
        let query = level & 0xffff;
        let request = level & WINHTTP_QUERY_FLAG_REQUEST_HEADERS != 0;
        let fields = if request { &self.headers } else { &self.response };
        let fixed = |v: &[u8]| -> Result<(Vec<u8>, u32), u32> {
            if index > 0 {
                return Err(ERROR_WINHTTP_HEADER_NOT_FOUND);
            }
            Ok((copy(v)?, 1))
        };
        match query {
            WINHTTP_QUERY_REQUEST_METHOD if request => return fixed(&self.verb),
            WINHTTP_QUERY_VERSION if request => return fixed(&self.version),
            WINHTTP_QUERY_VERSION => return fixed(&self.response_version),
            WINHTTP_QUERY_STATUS_TEXT if !request => return fixed(&self.reason),
            WINHTTP_QUERY_STATUS_CODE if !request => {
                let mut v = Vec::new();
                decimal(self.status.into(), &mut v)?;
                return fixed(&v);
            }
            WINHTTP_QUERY_RAW_HEADERS | WINHTTP_QUERY_RAW_HEADERS_CRLF => {
                let (sep, end): (&[u8], &[u8]) = if query == WINHTTP_QUERY_RAW_HEADERS {
                    (b"\0", b"\0")
                } else {
                    (b"\r\n", b"\r\n")
                };
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
        let wanted: &[u8] = if query == WINHTTP_QUERY_CUSTOM {
            custom = narrow(name)?;
            &custom
        } else {
            match QUERY_NAMES.get(query as usize) {
                Some(n) if !n.is_empty() => n,
                _ => {
                    missing(b"winhttp.dll!WinHttpQueryHeaders(level)", level.into());
                    return Err(ERROR_WINHTTP_INVALID_QUERY_REQUEST);
                }
            }
        };
        match fields.iter().filter(|h| same(&h.0, wanted)).nth(index as usize) {
            Some(h) => Ok((copy(&h.1)?, index + 1)),
            None => Err(ERROR_WINHTTP_HEADER_NOT_FOUND),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpSendRequest(
    h: usize,
    headers: *const u16,
    headers_length: u32,
    optional: *const u8,
    optional_length: u32,
    total_length: u32,
    _context: usize,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        let headers = if headers.is_null() || headers_length == 0 {
            Vec::new()
        } else {
            wide(
                headers,
                if headers_length == u32::MAX { None } else { Some(headers_length as usize) },
            )?
        };
        let optional: &[u8] = if optional.is_null() || optional_length == 0 {
            &[]
        } else {
            // SAFETY: the caller's buffer of `optional_length` bytes.
            unsafe { core::slice::from_raw_parts(optional, optional_length as usize) }
        };
        r.send(&headers, optional, total_length.into())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpWriteData(
    h: usize,
    data: *const u8,
    len: u32,
    written: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if data.is_null() && len > 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let data: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller's buffer of `len` bytes.
            unsafe { core::slice::from_raw_parts(data, len as usize) }
        };
        r.write_data(data)?;
        if !written.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *written = len };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpReceiveResponse(h: usize, reserved: usize) -> i32 {
    done((|| {
        if reserved != 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        request_of(h)?.receive()
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpQueryDataAvailable(h: usize, available: *mut u32) -> i32 {
    done((|| {
        let n = request_of(h)?.available()?;
        if !available.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *available = n.min(u32::MAX as usize) as u32 };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpReadData(h: usize, buffer: *mut u8, len: u32, read: *mut u32) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if buffer.is_null() && len > 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let out: &mut [u8] = if len == 0 {
            &mut []
        } else {
            // SAFETY: the caller's buffer of `len` bytes.
            unsafe { core::slice::from_raw_parts_mut(buffer, len as usize) }
        };
        let n = r.read_body(out)?;
        if !read.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *read = n as u32 };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpQueryHeaders(
    h: usize,
    level: u32,
    name: *const u16,
    buffer: *mut u8,
    length: *mut u32,
    index: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if length.is_null() {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let request = level & WINHTTP_QUERY_FLAG_REQUEST_HEADERS != 0;
        if !request && r.state != State::Received {
            return Err(ERROR_WINHTTP_INCORRECT_HANDLE_STATE);
        }
        if level & WINHTTP_QUERY_FLAG_SYSTEMTIME != 0 {
            missing(b"winhttp.dll!WINHTTP_QUERY_FLAG_SYSTEMTIME", level.into());
            return Err(crate::ERROR_NOT_SUPPORTED);
        }
        let name = if level & 0xffff == WINHTTP_QUERY_CUSTOM {
            if name.is_null() {
                return Err(ERROR_INVALID_PARAMETER);
            }
            wide(name, None)?
        } else {
            Vec::new()
        };
        // SAFETY: the caller's DWORD (in: which occurrence).
        let at = if index.is_null() { 0 } else { unsafe { *index } };
        let (value, next) = r.query(level, &name, at)?;
        // SAFETY: the caller's length, buffer of `*length` bytes and index.
        unsafe {
            if level & (WINHTTP_QUERY_FLAG_NUMBER | WINHTTP_QUERY_FLAG_NUMBER64) != 0 {
                let wide64 = level & WINHTTP_QUERY_FLAG_NUMBER64 != 0;
                let size = if wide64 { 8 } else { 4 };
                let digits = trim(&value);
                if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                    return Err(ERROR_WINHTTP_INVALID_QUERY_REQUEST);
                }
                let n = digits
                    .iter()
                    .fold(0u64, |n, &d| n.wrapping_mul(10).wrapping_add(u64::from(d - b'0')));
                if buffer.is_null() || *length < size {
                    *length = size;
                    return Err(ERROR_INSUFFICIENT_BUFFER);
                }
                if wide64 {
                    buffer.cast::<u64>().write_unaligned(n);
                } else {
                    buffer.cast::<u32>().write_unaligned(n as u32);
                }
                *length = size;
            } else {
                // UTF-16 with its NUL; the length in bytes without it.
                let needed = (value.len() as u32 + 1) * 2;
                if buffer.is_null() || *length < needed {
                    *length = needed;
                    return Err(ERROR_INSUFFICIENT_BUFFER);
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
    })())
}

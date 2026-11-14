//! A request's connection: TCP through ws2_32 (GetAddrInfoW, each
//! address in turn), and for https a TLS client through SSPI's Schannel
//! (secur32), as "Using SSPI with a Windows Sockets Client" and "Creating
//! a Secure Connection Using Schannel" describe it: the
//! InitializeSecurityContextW loop with the host as target name (SNI and
//! the name check) and Schannel's automatic validation against the
//! machine root store, then EncryptMessage (STREAM_HEADER, DATA,
//! STREAM_TRAILER) and DecryptMessage in place (DATA, EXTRA;
//! SEC_I_RENEGOTIATE's post-handshake messages back to the token loop;
//! close_notify as SEC_I_CONTEXT_EXPIRED). Time-outs: a non-blocking
//! connect waited for with WSAPoll (the connect time-out, each address
//! in turn), WSAPoll before each send and receive. The steps are told to
//! the request's observer (`Event`): resolving, each address tried, the
//! connection made (after the handshake for https), a handshake failure.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use crate::cert::{
    CERT_E_CHAINING, CERT_E_CN_NO_MATCH, CERT_E_EXPIRED, CERT_E_REVOKED, CERT_E_UNTRUSTEDCA,
    CERT_E_UNTRUSTEDROOT, SEC_E_CERT_EXPIRED, SEC_E_CERT_UNKNOWN, SEC_E_CERT_WRONG_USAGE,
    SEC_E_UNTRUSTED_ROOT, SEC_E_WRONG_PRINCIPAL,
};
use crate::err::{
    CANNOT_CONNECT, CONNECTION_ERROR, NAME_NOT_RESOLVED, NOT_ENOUGH_MEMORY, SECURE_CHANNEL_ERROR,
    TIMEOUT,
};
use crate::{
    Dll, Event, IGNORE_CN_INVALID, IGNORE_DATE_INVALID, IGNORE_UNKNOWN_CA, IGNORE_WRONG_USAGE,
    Observer, failed, missing, put,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct SecHandle {
    lower: usize,
    upper: usize,
}

const NO_HANDLE: SecHandle = SecHandle { lower: 0, upper: 0 };

#[repr(C)]
#[derive(Clone, Copy)]
struct SecBuffer {
    size: u32,
    kind: u32,
    data: *mut u8,
}

const EMPTY_BUFFER: SecBuffer = SecBuffer { size: 0, kind: SECBUFFER_EMPTY, data: null_mut() };

#[repr(C)]
struct SecBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut SecBuffer,
}

fn desc(buffers: &mut [SecBuffer]) -> SecBufferDesc {
    SecBufferDesc { version: 0, count: buffers.len() as u32, buffers: buffers.as_mut_ptr() }
}

/// SCHANNEL_CRED (version 4).
#[repr(C)]
struct SchannelCred {
    version: u32,
    creds: u32,
    cred: *const *const c_void,
    root_store: *mut c_void,
    mappers: u32,
    mapper: *const c_void,
    algs: u32,
    alg: *const u32,
    enabled_protocols: u32,
    min_strength: u32,
    max_strength: u32,
    lifespan: u32,
    flags: u32,
    format: u32,
}

#[repr(C)]
struct WsaData {
    version: u16,
    high_version: u16,
    max_sockets: u16,
    max_udp_dg: u16,
    vendor_info: *mut u8,
    description: [u8; 257],
    system_status: [u8; 129],
}

/// ADDRINFOW (x64).
#[repr(C)]
struct AddrInfo {
    flags: i32,
    family: i32,
    socktype: i32,
    protocol: i32,
    addrlen: usize,
    canonname: *const u16,
    addr: *const c_void,
    next: *const AddrInfo,
}

/// WSAPOLLFD.
#[repr(C)]
struct PollFd {
    fd: usize,
    events: i16,
    revents: i16,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ws2_32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ws2_32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
    fn GetAddrInfoW(
        node: *const u16,
        service: *const u16,
        hints: *const AddrInfo,
        result: *mut *const AddrInfo,
    ) -> i32;
    fn FreeAddrInfoW(ai: *const AddrInfo);
    fn socket(af: i32, kind: i32, protocol: i32) -> usize;
    fn connect(s: usize, addr: *const c_void, len: i32) -> i32;
    fn ioctlsocket(s: usize, cmd: i32, arg: *mut u32) -> i32;
    fn getsockopt(s: usize, level: i32, name: i32, value: *mut u8, len: *mut i32) -> i32;
    fn InetNtopW(family: i32, addr: *const c_void, text: *mut u16, size: usize) -> *const u16;
    fn send(s: usize, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: usize, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn closesocket(s: usize) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "secur32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "secur32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn AcquireCredentialsHandleW(
        principal: *const u16,
        package: *const u16,
        usage: u32,
        logon_id: *mut c_void,
        auth: *const c_void,
        get_key: usize,
        arg: usize,
        credential: *mut SecHandle,
        expiry: *mut i64,
    ) -> i32;
    fn FreeCredentialsHandle(credential: *mut SecHandle) -> i32;
    fn InitializeSecurityContextW(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        target: *const u16,
        flags: u32,
        reserved: u32,
        representation: u32,
        input: *mut SecBufferDesc,
        reserved2: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        attributes: *mut u32,
        expiry: *mut i64,
    ) -> i32;
    fn DeleteSecurityContext(context: *mut SecHandle) -> i32;
    fn FreeContextBuffer(buffer: *mut c_void) -> i32;
    fn QueryContextAttributesW(context: *mut SecHandle, attribute: u32, buffer: *mut c_void)
    -> i32;
    fn EncryptMessage(
        context: *mut SecHandle,
        qop: u32,
        message: *mut SecBufferDesc,
        seq: u32,
    ) -> i32;
    fn DecryptMessage(
        context: *mut SecHandle,
        message: *mut SecBufferDesc,
        seq: u32,
        qop: *mut u32,
    ) -> i32;
}

const SEC_E_OK: u32 = 0;
const SEC_I_CONTINUE_NEEDED: u32 = 0x0009_0312;
const SEC_I_CONTEXT_EXPIRED: u32 = 0x0009_0317;
const SEC_I_RENEGOTIATE: u32 = 0x0009_0321;
const SEC_E_INCOMPLETE_MESSAGE: u32 = 0x8009_0318;

const SECPKG_CRED_OUTBOUND: u32 = 2;
const SECBUFFER_EMPTY: u32 = 0;
const SECBUFFER_DATA: u32 = 1;
const SECBUFFER_TOKEN: u32 = 2;
const SECBUFFER_EXTRA: u32 = 5;
const SECBUFFER_STREAM_TRAILER: u32 = 6;
const SECBUFFER_STREAM_HEADER: u32 = 7;
const SECPKG_ATTR_STREAM_SIZES: u32 = 4;

/// ISC_REQ_REPLAY_DETECT, SEQUENCE_DETECT, CONFIDENTIALITY,
/// ALLOCATE_MEMORY, STREAM: the documented client's request.
const ISC_FLAGS: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x8000;

const SCHANNEL_CRED_VERSION: u32 = 4;
const SCH_CRED_NO_SERVERNAME_CHECK: u32 = 0x4;
const SCH_CRED_MANUAL_CRED_VALIDATION: u32 = 0x8;
const SCH_CRED_NO_DEFAULT_CREDS: u32 = 0x10;
const SCH_CRED_AUTO_CRED_VALIDATION: u32 = 0x20;

const POLLRDNORM: i16 = 0x100;
const POLLWRNORM: i16 = 0x10;
const POLLERR: i16 = 0x1;
const POLLHUP: i16 = 0x2;
const WSAETIMEDOUT: i32 = 10060;
const WSAEWOULDBLOCK: i32 = 10035;
const WSAECONNREFUSED: i32 = 10061;
const FIONBIO: i32 = 0x8004_667e_u32 as i32;
const SOL_SOCKET: i32 = 0xffff;
const SO_ERROR: i32 = 0x1007;
const AF_INET: i32 = 2;
const AF_INET6: i32 = 23;

/// "Microsoft Unified Security Protocol Provider" (UNISP_NAME_W).
const UNISP: &[u8] = b"Microsoft Unified Security Protocol Provider\0";

struct Tls {
    cred: SecHandle,
    ctx: SecHandle,
    header: usize,
    trailer: usize,
    max: usize,
    /// close_notify came (SEC_I_CONTEXT_EXPIRED).
    closed: bool,
}

pub struct Conn {
    dll: Dll,
    socket: usize,
    tls: Option<Tls>,
    /// Received bytes not yet decrypted (TLS).
    raw: Vec<u8>,
    /// The TLS target name (the host), NUL-terminated.
    target: Vec<u16>,
    send_timeout: i32,
    pub receive_timeout: i32,
}

/// Where a connection goes and how.
pub struct Target<'a> {
    pub dll: Dll,
    /// The host, without IPv6 brackets.
    pub host: &'a [u16],
    pub port: u16,
    pub secure: bool,
    /// SECURITY_FLAG_IGNORE_* (the request's security flags).
    pub security_flags: u32,
    /// SP_PROT_* (0: Schannel's defaults).
    pub protocols: u32,
    /// Per address tried (-1: the stack's own).
    pub connect_timeout: i32,
    pub send_timeout: i32,
    pub receive_timeout: i32,
    /// Who is told the steps.
    pub observer: Option<Observer>,
}

impl Conn {
    /// Resolves the host, connects to its addresses in turn and, for
    /// https, makes the TLS handshake.
    pub fn open(t: &Target) -> Result<Conn, u32> {
        // SAFETY: Winsock's start, balanced by Drop's WSACleanup.
        let started = unsafe {
            let mut data: WsaData = core::mem::zeroed();
            WSAStartup(0x202, &mut data)
        };
        if started != 0 {
            failed(t.dll, b"WSAStartup", started as u64);
            return Err(CANNOT_CONNECT);
        }
        let mut conn = Conn {
            dll: t.dll,
            socket: usize::MAX,
            tls: None,
            raw: Vec::new(),
            target: Vec::new(),
            send_timeout: t.send_timeout,
            receive_timeout: t.receive_timeout,
        };
        let (socket, address) = connect_to(t)?;
        conn.socket = socket;
        if t.secure
            && let Err(e) = conn.handshake(t)
        {
            if crate::is_certificate(e) || e == SECURE_CHANNEL_ERROR {
                Observer::tell(t.observer.as_ref(), Event::SecureFailure(e));
            }
            return Err(e);
        }
        Observer::tell(t.observer.as_ref(), Event::Connected(address.text()));
        Ok(conn)
    }

    fn send_all(&self, data: &[u8]) -> Result<(), u32> {
        send_all(self.dll, self.socket, self.send_timeout, data)
    }

    /// The send and receive time-outs of the request now using it (a
    /// connection taken from the pool).
    pub fn set_timeouts(&mut self, send: i32, receive: i32) {
        self.send_timeout = send;
        self.receive_timeout = receive;
    }

    /// Whether an idle connection can carry another request: nothing
    /// arrived while it waited (a readable socket is the server's close,
    /// or bytes no request asked for), no TLS bytes are left over, and
    /// no close_notify came.
    pub fn idle_ok(&self) -> bool {
        if !self.raw.is_empty() || self.tls.as_ref().is_some_and(|t| t.closed) {
            return false;
        }
        let mut fd = PollFd { fd: self.socket, events: POLLRDNORM, revents: 0 };
        // SAFETY: one WSAPOLLFD of ours; no wait.
        unsafe { WSAPoll(&mut fd, 1, 0) == 0 }
    }

    /// Appends what the server sends next to `out`; Ok(0) when it closed.
    fn recv_into(&self, out: &mut Vec<u8>) -> Result<usize, u32> {
        wait(self.dll, self.socket, POLLRDNORM, self.receive_timeout)?;
        const CHUNK: usize = 16 * 1024;
        out.try_reserve(CHUNK).map_err(|_| NOT_ENOUGH_MEMORY)?;
        let at = out.len();
        // SAFETY: `CHUNK` reserved bytes after `at`; `recv` writes at most
        // that many, and only those it wrote are made part of `out`.
        unsafe {
            let n = recv(self.socket, out.as_mut_ptr().add(at), CHUNK as i32, 0);
            if n < 0 {
                return Err(socket_error(self.dll, b"recv"));
            }
            out.set_len(at + n as usize);
            Ok(n as usize)
        }
    }

    /// InitializeSecurityContextW with `input` (None: the first call).
    fn isc(&mut self, input: *mut SecBufferDesc, out: &mut SecBuffer) -> u32 {
        let tls = self.tls.as_mut().expect("a TLS connection");
        let first = tls.ctx.lower == 0 && tls.ctx.upper == 0;
        let ctx: *mut SecHandle = if first { null_mut() } else { &mut tls.ctx };
        let mut ob = [SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: null_mut() }];
        let mut output = desc(&mut ob);
        let mut attributes = 0;
        // SAFETY: handles and descriptors of ours; a NUL-terminated target.
        let r = unsafe {
            InitializeSecurityContextW(
                &mut tls.cred,
                ctx,
                self.target.as_ptr(),
                ISC_FLAGS,
                0,
                0,
                input,
                0,
                &mut tls.ctx,
                &mut output,
                &mut attributes,
                null_mut(),
            )
        } as u32;
        *out = ob[0];
        r
    }

    /// Sends a token the package allocated, and frees it.
    fn send_token(&self, b: &SecBuffer) -> Result<(), u32> {
        if b.data.is_null() {
            return Ok(());
        }
        // SAFETY: the package's buffer of `size` bytes.
        let r = self.send_all(unsafe { core::slice::from_raw_parts(b.data, b.size as usize) });
        // SAFETY: allocated by the package (ISC_REQ_ALLOCATE_MEMORY).
        unsafe { FreeContextBuffer(b.data.cast()) };
        r
    }

    /// Feeds the received handshake bytes in `raw` to the token loop;
    /// keeps SECBUFFER_EXTRA. Ok(status) for OK / CONTINUE_NEEDED /
    /// INCOMPLETE_MESSAGE.
    fn feed(&mut self) -> Result<u32, u32> {
        let mut ib = [
            SecBuffer {
                size: self.raw.len() as u32,
                kind: SECBUFFER_TOKEN,
                data: self.raw.as_mut_ptr(),
            },
            EMPTY_BUFFER,
        ];
        let mut input = desc(&mut ib);
        let mut token = EMPTY_BUFFER;
        let r = self.isc(&mut input, &mut token);
        if r == SEC_E_INCOMPLETE_MESSAGE {
            return Ok(r);
        }
        if r != SEC_E_OK && r != SEC_I_CONTINUE_NEEDED {
            if !token.data.is_null() {
                // An alert for the server, if any: the connection ends anyway.
                // SAFETY: allocated by the package.
                unsafe { FreeContextBuffer(token.data.cast()) };
            }
            failed(self.dll, b"TLS handshake", r.into());
            return Err(secure_error(r));
        }
        self.send_token(&token)?;
        let extra = if ib[1].kind == SECBUFFER_EXTRA { ib[1].size as usize } else { 0 };
        let extra = extra.min(self.raw.len());
        self.raw.drain(..self.raw.len() - extra);
        Ok(r)
    }

    fn handshake(&mut self, t: &Target) -> Result<(), u32> {
        let ignore = t.security_flags;
        let mut flags = SCH_CRED_NO_DEFAULT_CREDS;
        if ignore & (IGNORE_UNKNOWN_CA | IGNORE_DATE_INVALID | IGNORE_WRONG_USAGE) != 0 {
            // Schannel's manual validation skips every certificate check,
            // more than the flags ask for: recorded.
            if ignore
                & (IGNORE_UNKNOWN_CA | IGNORE_DATE_INVALID | IGNORE_WRONG_USAGE | IGNORE_CN_INVALID)
                != IGNORE_UNKNOWN_CA | IGNORE_DATE_INVALID | IGNORE_WRONG_USAGE | IGNORE_CN_INVALID
            {
                missing(t.dll, b"SECURITY_FLAG_IGNORE_* (some)", ignore.into());
            }
            flags |= SCH_CRED_MANUAL_CRED_VALIDATION;
        } else {
            flags |= SCH_CRED_AUTO_CRED_VALIDATION;
            if ignore & IGNORE_CN_INVALID != 0 {
                flags |= SCH_CRED_NO_SERVERNAME_CHECK;
            }
        }
        let auth = SchannelCred {
            version: SCHANNEL_CRED_VERSION,
            creds: 0,
            cred: null(),
            root_store: null_mut(),
            mappers: 0,
            mapper: null(),
            algs: 0,
            alg: null(),
            enabled_protocols: t.protocols,
            min_strength: 0,
            max_strength: 0,
            lifespan: 0,
            flags,
            format: 0,
        };
        let mut package = [0u16; UNISP.len()];
        for (d, &b) in package.iter_mut().zip(UNISP) {
            *d = b.into();
        }
        let mut cred = NO_HANDLE;
        // SAFETY: a credentials structure of ours.
        let r = unsafe {
            AcquireCredentialsHandleW(
                null(),
                package.as_ptr(),
                SECPKG_CRED_OUTBOUND,
                null_mut(),
                (&auth as *const SchannelCred).cast(),
                0,
                0,
                &mut cred,
                null_mut(),
            )
        } as u32;
        if r != SEC_E_OK {
            failed(self.dll, b"AcquireCredentialsHandleW", r.into());
            return Err(SECURE_CHANNEL_ERROR);
        }
        self.tls = Some(Tls { cred, ctx: NO_HANDLE, header: 0, trailer: 0, max: 0, closed: false });
        put(&mut self.target, t.host)?;
        put(&mut self.target, &[0])?;
        let mut token = EMPTY_BUFFER;
        let r = self.isc(null_mut(), &mut token);
        if r != SEC_I_CONTINUE_NEEDED {
            failed(self.dll, b"TLS handshake", r.into());
            return Err(secure_error(r));
        }
        self.send_token(&token)?;
        let mut need = true;
        loop {
            if need || self.raw.is_empty() {
                let mut raw = core::mem::take(&mut self.raw);
                let n = self.recv_into(&mut raw);
                self.raw = raw;
                if n? == 0 {
                    failed(self.dll, b"TLS handshake: closed", 0);
                    return Err(SECURE_CHANNEL_ERROR);
                }
            }
            match self.feed()? {
                SEC_E_INCOMPLETE_MESSAGE => need = true,
                SEC_E_OK => break,
                _ => need = self.raw.is_empty(),
            }
        }
        let mut sizes = [0u32; 5];
        let tls = self.tls.as_mut().expect("a TLS connection");
        // SAFETY: SecPkgContext_StreamSizes (five DWORDs).
        let r = unsafe {
            QueryContextAttributesW(
                &mut tls.ctx,
                SECPKG_ATTR_STREAM_SIZES,
                sizes.as_mut_ptr().cast(),
            )
        } as u32;
        if r != SEC_E_OK {
            failed(self.dll, b"QueryContextAttributesW(STREAM_SIZES)", r.into());
            return Err(SECURE_CHANNEL_ERROR);
        }
        (tls.header, tls.trailer, tls.max) =
            (sizes[0] as usize, sizes[1] as usize, sizes[2] as usize);
        Ok(())
    }

    /// Sends `data` (as TLS records on https).
    pub fn write(&mut self, data: &[u8]) -> Result<(), u32> {
        let Some(tls) = self.tls.as_mut() else {
            return self.send_all(data);
        };
        let (header, trailer) = (tls.header, tls.trailer);
        let dll = self.dll;
        let mut record = Vec::new();
        for chunk in data.chunks(tls.max.max(1)) {
            record.clear();
            record.try_reserve(header + chunk.len() + trailer).map_err(|_| NOT_ENOUGH_MEMORY)?;
            record.resize(header + chunk.len() + trailer, 0);
            record[header..header + chunk.len()].copy_from_slice(chunk);
            let base = record.as_mut_ptr();
            // SAFETY: inside `record`.
            let mut b = unsafe {
                [
                    SecBuffer { size: header as u32, kind: SECBUFFER_STREAM_HEADER, data: base },
                    SecBuffer {
                        size: chunk.len() as u32,
                        kind: SECBUFFER_DATA,
                        data: base.add(header),
                    },
                    SecBuffer {
                        size: trailer as u32,
                        kind: SECBUFFER_STREAM_TRAILER,
                        data: base.add(header + chunk.len()),
                    },
                    EMPTY_BUFFER,
                ]
            };
            let mut d = desc(&mut b);
            // SAFETY: our context and buffers.
            let r = unsafe { EncryptMessage(&mut tls.ctx, 0, &mut d, 0) } as u32;
            if r != SEC_E_OK {
                failed(dll, b"EncryptMessage", r.into());
                return Err(SECURE_CHANNEL_ERROR);
            }
            let total = (b[0].size + b[1].size + b[2].size) as usize;
            send_all(dll, self.socket, self.send_timeout, &record[..total])?;
        }
        Ok(())
    }

    /// Appends the next plaintext the server sends to `out`; Ok(true)
    /// when it closed (or sent close_notify) instead.
    pub fn read(&mut self, out: &mut Vec<u8>) -> Result<bool, u32> {
        if self.tls.is_none() {
            return Ok(self.recv_into(out)? == 0);
        }
        let mut need = self.raw.is_empty();
        let dll = self.dll;
        loop {
            if self.tls.as_ref().is_some_and(|t| t.closed) {
                return Ok(true);
            }
            if need {
                let mut raw = core::mem::take(&mut self.raw);
                let n = self.recv_into(&mut raw);
                self.raw = raw;
                if n? == 0 {
                    // Closed without close_notify: the end of the stream as
                    // far as HTTP framing is concerned.
                    return Ok(true);
                }
            }
            let tls = self.tls.as_mut().expect("a TLS connection");
            let mut b = [
                SecBuffer {
                    size: self.raw.len() as u32,
                    kind: SECBUFFER_DATA,
                    data: self.raw.as_mut_ptr(),
                },
                EMPTY_BUFFER,
                EMPTY_BUFFER,
                EMPTY_BUFFER,
            ];
            let mut d = desc(&mut b);
            // SAFETY: our context and buffers.
            let r = unsafe { DecryptMessage(&mut tls.ctx, &mut d, 0, null_mut()) } as u32;
            if r == SEC_E_INCOMPLETE_MESSAGE {
                need = true;
                continue;
            }
            if r != SEC_E_OK && r != SEC_I_RENEGOTIATE && r != SEC_I_CONTEXT_EXPIRED {
                failed(dll, b"DecryptMessage", r.into());
                return Err(SECURE_CHANNEL_ERROR);
            }
            let before = out.len();
            let mut extra = 0;
            for buffer in &b {
                if buffer.kind == SECBUFFER_DATA && buffer.size > 0 {
                    // SAFETY: decrypted in place, inside `raw`.
                    let text =
                        unsafe { core::slice::from_raw_parts(buffer.data, buffer.size as usize) };
                    put(out, text)?;
                }
                if buffer.kind == SECBUFFER_EXTRA {
                    extra = buffer.size as usize;
                }
            }
            let extra = extra.min(self.raw.len());
            self.raw.drain(..self.raw.len() - extra);
            if r == SEC_I_CONTEXT_EXPIRED {
                tls.closed = true;
            }
            if r == SEC_I_RENEGOTIATE {
                // A post-handshake message (TLS 1.3 NewSessionTicket,
                // KeyUpdate) in the extra bytes: back to the token loop.
                match self.feed()? {
                    SEC_E_OK => {}
                    s => {
                        failed(dll, b"TLS post-handshake", s.into());
                        return Err(SECURE_CHANNEL_ERROR);
                    }
                }
            }
            if out.len() > before {
                return Ok(false);
            }
            need = self.raw.is_empty();
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: our handles.
        unsafe {
            if let Some(tls) = self.tls.as_mut() {
                if tls.ctx.lower != 0 || tls.ctx.upper != 0 {
                    DeleteSecurityContext(&mut tls.ctx);
                }
                FreeCredentialsHandle(&mut tls.cred);
            }
            if self.socket != usize::MAX {
                closesocket(self.socket);
            }
            WSACleanup();
        }
    }
}

/// Waits until the socket can take `events`, for at most `timeout` ms
/// (-1: no limit); err::TIMEOUT when it cannot.
fn wait(dll: Dll, socket: usize, events: i16, timeout: i32) -> Result<(), u32> {
    if timeout < 0 {
        return Ok(());
    }
    let mut fd = PollFd { fd: socket, events, revents: 0 };
    // SAFETY: one WSAPOLLFD of ours.
    let n = unsafe { WSAPoll(&mut fd, 1, timeout) };
    if n == 0 {
        failed(dll, b"timed out", timeout as u64);
        return Err(TIMEOUT);
    }
    // An error shows in the send or recv that follows.
    Ok(())
}

fn send_all(dll: Dll, socket: usize, timeout: i32, data: &[u8]) -> Result<(), u32> {
    let mut done = 0;
    while done < data.len() {
        wait(dll, socket, POLLWRNORM, timeout)?;
        let chunk = (data.len() - done).min(1 << 30) as i32;
        // SAFETY: our buffer of `chunk` bytes at `done`.
        let n = unsafe { send(socket, data[done..].as_ptr(), chunk, 0) };
        if n <= 0 {
            return Err(socket_error(dll, b"send"));
        }
        done += n as usize;
    }
    Ok(())
}

/// The error for a failed handshake: a certificate problem is its status
/// itself, for the DLL to translate into its API's code
/// (`crate::is_certificate`); other SSPI failures are
/// err::SECURE_CHANNEL_ERROR.
fn secure_error(status: u32) -> u32 {
    let certificate = matches!(
        status,
        SEC_E_WRONG_PRINCIPAL
            | SEC_E_UNTRUSTED_ROOT
            | SEC_E_CERT_UNKNOWN
            | SEC_E_CERT_EXPIRED
            | SEC_E_CERT_WRONG_USAGE
            | CERT_E_EXPIRED
            | CERT_E_UNTRUSTEDROOT
            | CERT_E_CHAINING
            | CERT_E_REVOKED
            | CERT_E_CN_NO_MATCH
            | CERT_E_UNTRUSTEDCA
    ) || status & 0xffff_0000 == 0x800b_0000; // other CERT_E_*, TRUST_E_*
    if certificate { status } else { SECURE_CHANNEL_ERROR }
}

/// Records a failed send or recv; the error for it.
fn socket_error(dll: Dll, what: &[u8]) -> u32 {
    // SAFETY: a plain call.
    let e = unsafe { WSAGetLastError() };
    failed(dll, what, e as u64);
    if e == WSAETIMEDOUT { TIMEOUT } else { CONNECTION_ERROR }
}

/// An address as text (InetNtopW: no port), NUL-terminated.
struct Address {
    text: [u16; 48],
    len: usize,
}

impl Address {
    /// `a`'s address; empty for a family other than IPv4 and IPv6.
    fn of(a: &AddrInfo) -> Address {
        let mut out = Address { text: [0; 48], len: 0 };
        // The address within SOCKADDR_IN (offset 4) or SOCKADDR_IN6 (8).
        let at = match a.family {
            AF_INET => 4,
            AF_INET6 => 8,
            _ => return out,
        };
        // SAFETY: GetAddrInfoW's sockaddr of the family; our buffer of
        // `text.len()` characters.
        let r = unsafe {
            InetNtopW(
                a.family,
                a.addr.cast::<u8>().add(at).cast(),
                out.text.as_mut_ptr(),
                out.text.len(),
            )
        };
        if !r.is_null() {
            out.len = out.text.iter().position(|&c| c == 0).unwrap_or(0);
        }
        out
    }

    /// With its NUL.
    fn text(&self) -> &[u16] {
        &self.text[..=self.len]
    }
}

/// Connects `s` to `a`, waiting at most `timeout` ms (-1: the stack's
/// own wait); the Winsock error otherwise (WSAETIMEDOUT for the time-out).
fn attempt(s: usize, a: &AddrInfo, timeout: i32) -> Result<(), i32> {
    // SAFETY: our socket and GetAddrInfoW's address; plain calls.
    unsafe {
        if timeout < 0 {
            return if connect(s, a.addr, a.addrlen as i32) == 0 {
                Ok(())
            } else {
                Err(WSAGetLastError())
            };
        }
        let mut on = 1u32;
        if ioctlsocket(s, FIONBIO, &mut on) != 0 {
            return Err(WSAGetLastError());
        }
        if connect(s, a.addr, a.addrlen as i32) != 0 {
            let e = WSAGetLastError();
            if e != WSAEWOULDBLOCK {
                return Err(e);
            }
            let mut fd = PollFd { fd: s, events: POLLWRNORM, revents: 0 };
            match WSAPoll(&mut fd, 1, timeout) {
                0 => return Err(WSAETIMEDOUT),
                n if n < 0 => return Err(WSAGetLastError()),
                _ => {}
            }
            let mut error = 0i32;
            let mut len = 4i32;
            if getsockopt(s, SOL_SOCKET, SO_ERROR, (&raw mut error).cast(), &mut len) != 0 {
                return Err(WSAGetLastError());
            }
            if error != 0 {
                return Err(error);
            }
            if fd.revents & (POLLERR | POLLHUP) != 0 && fd.revents & POLLWRNORM == 0 {
                return Err(WSAECONNREFUSED);
            }
        }
        let mut off = 0u32;
        if ioctlsocket(s, FIONBIO, &mut off) != 0 {
            return Err(WSAGetLastError());
        }
    }
    Ok(())
}

/// Resolves the target's host and connects to its addresses in the
/// order given, each within the connect time-out; the socket and the
/// address it connected to. All failed: err::TIMEOUT when the last one
/// timed out, else CANNOT_CONNECT.
fn connect_to(t: &Target) -> Result<(usize, Address), u32> {
    let (dll, host, port) = (t.dll, t.host, t.port);
    let observer = t.observer.as_ref();
    let mut name = Vec::new();
    put(&mut name, host)?;
    put(&mut name, &[0])?;
    let mut service = [0u16; 6];
    let mut digits = [0u8; 5];
    let mut n = 0;
    let mut p = port;
    loop {
        digits[n] = b'0' + (p % 10) as u8;
        n += 1;
        p /= 10;
        if p == 0 {
            break;
        }
    }
    for i in 0..n {
        service[i] = digits[n - 1 - i].into();
    }
    // SAFETY: plain data.
    let mut hints: AddrInfo = unsafe { core::mem::zeroed() };
    hints.socktype = 1; // SOCK_STREAM
    hints.protocol = 6; // IPPROTO_TCP
    let mut list: *const AddrInfo = null();
    Observer::tell(observer, Event::Resolving(&name));
    // SAFETY: NUL-terminated strings and hints of ours.
    let r = unsafe { GetAddrInfoW(name.as_ptr(), service.as_ptr(), &hints, &mut list) };
    if r != 0 {
        failed(dll, b"GetAddrInfoW", r as u64);
        return Err(NAME_NOT_RESOLVED);
    }
    if !list.is_null() {
        // SAFETY: the list GetAddrInfoW made.
        Observer::tell(observer, Event::Resolved(Address::of(unsafe { &*list }).text()));
    }
    let mut last = 0;
    let mut ai = list;
    let mut found = None;
    // SAFETY: the list GetAddrInfoW made, freed below.
    unsafe {
        while !ai.is_null() {
            let a = &*ai;
            let address = Address::of(a);
            Observer::tell(observer, Event::Connecting(address.text()));
            let s = socket(a.family, 1, 6);
            if s == usize::MAX {
                last = WSAGetLastError();
            } else {
                match attempt(s, a, t.connect_timeout) {
                    Ok(()) => {
                        found = Some((s, address));
                        break;
                    }
                    Err(e) => {
                        last = e;
                        closesocket(s);
                    }
                }
            }
            ai = a.next;
        }
        FreeAddrInfoW(list);
    }
    found.ok_or_else(|| {
        if last == WSAETIMEDOUT {
            failed(dll, b"connect timed out", t.connect_timeout as u64);
            TIMEOUT
        } else {
            failed(dll, b"connect", last as u64);
            CANNOT_CONNECT
        }
    })
}

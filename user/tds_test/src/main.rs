//! A SQL Server client of our own, speaking TDS 7.4 as Microsoft's
//! [MS-TDS] specification describes it, over the named pipe SQL Server
//! LocalDB listens on. It starts the instance as SqlLocalDB does (the
//! first start of an instance: SqlUserInstance.dll's `-m -K -T1617 -w5`
//! arguments, `-w1` here), waits for `\\.\pipe\LOCALDB#GALA0001\tsql\query`,
//! then:
//! - PRELOGIN without encryption (ENCRYPT_NOT_SUP); the server must not
//!   require it;
//! - LOGIN7 with integrated security (fIntSecurity): the SSPI data is a
//!   Negotiate token from InitializeSecurityContextW, the server's SSPI
//!   token is answered with an SSPI message, until LOGINACK;
//! - SQL_BATCH (with the ALL_HEADERS transaction descriptor) `SELECT
//!   @@VERSION, SUSER_SNAME()`: COLMETADATA, ROW (NVARCHAR) and DONE;
//!   prints the row;
//! - disconnects and waits for sqlservr.exe to end itself after its idle
//!   time (`-w1`).
//!
//! Prints `tds_test: ok` or exits with the number of the failing step.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct SecHandle {
    lower: usize,
    upper: usize,
}

#[repr(C)]
struct SecBuffer {
    size: u32,
    kind: u32,
    data: *mut u8,
}

#[repr(C)]
struct SecBufferDesc {
    version: u32,
    count: u32,
    buffers: *mut SecBuffer,
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: i32,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn Sleep(ms: u32);
    fn GetLastError() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "secur32", kind = "raw-dylib")]
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
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn strlen(s: *const u8) -> usize;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

/// Reports failing step `step` with value `got` and returns it.
fn fail(step: u32, what: &[u8], got: u32) -> u32 {
    out(b"tds_test: step ");
    out(&hex(step));
    out(b" (");
    out(what);
    out(b") got ");
    out(&hex(got));
    out(b"\r\n");
    step
}

/// `text` as NUL-terminated UTF-16 in `buf`; its length without the NUL.
fn wide(text: &str, buf: &mut [u16]) -> usize {
    let mut n = 0;
    for c in text.encode_utf16() {
        buf[n] = c;
        n += 1;
    }
    buf[n] = 0;
    n
}

const PIPE: &str = "\\\\.\\pipe\\LOCALDB#GALA0001\\tsql\\query";
const SQLSERVR: &str = "C:\\sql\\Binn\\sqlservr.exe";
const ARGS: &str = "sqlservr.exe -m -K -T1617 -w1 -c -SMSSQL16E.LOCALDB -sLOCALDB#GALA0001 \
     -dC:\\sql\\data\\master.mdf -lC:\\sql\\data\\mastlog.ldf -eC:\\sql\\data\\ERRORLOG";

// TDS packet types ([MS-TDS] 2.2.3.1.1).
const SQL_BATCH: u8 = 0x01;
const TABULAR_RESULT: u8 = 0x04;
const LOGIN7: u8 = 0x10;
const SSPI: u8 = 0x11;
const PRELOGIN: u8 = 0x12;

/// The pipe and the packet numbering.
struct Conn {
    pipe: Handle,
    packet_id: u8,
}

/// A growable-enough buffer for one message.
struct Buf {
    data: [u8; 16384],
    len: usize,
}

impl Buf {
    fn new() -> Buf {
        Buf { data: [0; 16384], len: 0 }
    }
    fn put(&mut self, b: &[u8]) {
        let n = b.len().min(self.data.len() - self.len);
        self.data[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
    }
    fn u8(&mut self, v: u8) {
        self.put(&[v]);
    }
    fn u16le(&mut self, v: u16) {
        self.put(&v.to_le_bytes());
    }
    fn u32le(&mut self, v: u32) {
        self.put(&v.to_le_bytes());
    }
    fn bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

impl Conn {
    fn write_all(&self, data: &[u8]) -> bool {
        let mut n = 0;
        // SAFETY: our buffer; the pipe handle.
        let ok = unsafe {
            WriteFile(self.pipe, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut())
        };
        ok != 0 && n == data.len() as u32
    }

    fn read_exact(&self, buf: &mut [u8]) -> bool {
        let mut at = 0;
        while at < buf.len() {
            let mut n = 0;
            // SAFETY: our buffer; the pipe handle.
            let ok = unsafe {
                ReadFile(
                    self.pipe,
                    buf[at..].as_mut_ptr(),
                    (buf.len() - at) as u32,
                    &mut n,
                    core::ptr::null_mut(),
                )
            };
            // ERROR_MORE_DATA (a message larger than the buffer) still
            // delivered `n` bytes.
            // SAFETY: a plain call.
            if (ok == 0 && unsafe { GetLastError() } != 234) || n == 0 {
                return false;
            }
            at += n as usize;
        }
        true
    }

    /// Sends `payload` as one message of `kind` (packets of at most
    /// 4096 bytes, the last with END_OF_MESSAGE).
    fn send(&mut self, kind: u8, payload: &[u8]) -> bool {
        let mut rest = payload;
        loop {
            let n = rest.len().min(4096 - 8);
            let last = n == rest.len();
            let total = (n + 8) as u16;
            let header =
                [kind, u8::from(last), (total >> 8) as u8, total as u8, 0, 0, self.packet_id, 0];
            self.packet_id = self.packet_id.wrapping_add(1);
            let mut packet = [0u8; 4096];
            packet[..8].copy_from_slice(&header);
            packet[8..8 + n].copy_from_slice(&rest[..n]);
            if !self.write_all(&packet[..8 + n]) {
                return false;
            }
            rest = &rest[n..];
            if last {
                return true;
            }
        }
    }

    /// Receives one message: (its packet type, the payload).
    fn receive(&self, into: &mut Buf) -> Option<u8> {
        into.len = 0;
        loop {
            let mut header = [0u8; 8];
            if !self.read_exact(&mut header) {
                return None;
            }
            let total = usize::from(u16::from_be_bytes([header[2], header[3]]));
            let mut body = [0u8; 4096];
            if !(8..=4096).contains(&total) || !self.read_exact(&mut body[..total - 8]) {
                return None;
            }
            into.put(&body[..total - 8]);
            if header[1] & 1 != 0 {
                return Some(header[0]);
            }
        }
    }
}

/// Starts sqlservr.exe; its process handle.
fn start_server() -> Result<Handle, u32> {
    let (mut app, mut cmd, mut dir) = ([0u16; 64], [0u16; 512], [0u16; 32]);
    wide(SQLSERVR, &mut app);
    wide(ARGS, &mut cmd);
    wide("C:\\sql\\Binn", &mut dir);
    let startup = StartupInfo { cb: 104, reserved: [0; 100] };
    let mut info = ProcessInformation {
        process: core::ptr::null_mut(),
        thread: core::ptr::null_mut(),
        pid: 0,
        tid: 0,
    };
    // SAFETY: our strings and structures.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmd.as_mut_ptr(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
            0,
            core::ptr::null_mut(),
            dir.as_ptr(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        // SAFETY: a plain call.
        return Err(fail(1, b"CreateProcessW", unsafe { GetLastError() }));
    }
    // SAFETY: the thread handle we do not need.
    unsafe { CloseHandle(info.thread) };
    Ok(info.process)
}

/// Opens the server's pipe, retrying while it does not exist yet (or is
/// busy), up to about four minutes.
fn connect() -> Result<Handle, u32> {
    let mut name = [0u16; 64];
    wide(PIPE, &mut name);
    let mut last = 0;
    for _ in 0..1200 {
        // SAFETY: our name; GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                0xc000_0000,
                0,
                core::ptr::null_mut(),
                3,
                0,
                core::ptr::null_mut(),
            )
        };
        if h as isize != -1 {
            return Ok(h);
        }
        // SAFETY: plain calls.
        unsafe {
            last = GetLastError();
            Sleep(200);
        }
    }
    Err(fail(2, b"open the pipe", last))
}

/// PRELOGIN ([MS-TDS] 2.2.6.5): VERSION, ENCRYPTION (not supported),
/// INSTOPT, THREADID, MARS; the server's ENCRYPTION answer.
fn prelogin(c: &mut Conn) -> Result<(), u32> {
    let mut p = Buf::new();
    let options: [(u8, u16); 5] = [(0, 6), (1, 1), (2, 1), (3, 4), (4, 1)];
    let mut offset = 5 * 5 + 1u16;
    for (token, len) in options {
        p.u8(token);
        p.put(&offset.to_be_bytes());
        p.put(&len.to_be_bytes());
        offset += len;
    }
    p.u8(0xff);
    p.put(&[0x10, 0, 0, 0, 0, 0]); // version (informational)
    p.u8(2); // ENCRYPT_NOT_SUP
    p.u8(0); // INSTOPT: the default instance name
    // SAFETY: a plain call.
    p.put(&unsafe { GetCurrentProcessId() }.to_be_bytes());
    p.u8(0); // MARS off
    if !c.send(PRELOGIN, p.bytes()) {
        return Err(fail(3, b"send PRELOGIN", 0));
    }
    let mut r = Buf::new();
    match c.receive(&mut r) {
        Some(TABULAR_RESULT) => {}
        Some(other) => return Err(fail(4, b"PRELOGIN response type", other.into())),
        // SAFETY: a plain call.
        None => return Err(fail(4, b"PRELOGIN response read", unsafe { GetLastError() })),
    }
    let d = r.bytes();
    let mut i = 0;
    while i + 5 <= d.len() && d[i] != 0xff {
        let off = usize::from(u16::from_be_bytes([d[i + 1], d[i + 2]]));
        if d[i] == 1 {
            let e = d.get(off).copied().unwrap_or(0xff);
            if e == 3 || e == 1 {
                // ENCRYPT_REQ / ENCRYPT_ON: the server wants TLS.
                return Err(fail(5, b"server requires encryption", e.into()));
            }
        }
        i += 5;
    }
    Ok(())
}

/// Appends UTF-16 `text` to the LOGIN7 data part; (offset, length).
fn login_string(data: &mut Buf, base: usize, text: &str) -> (u16, u16) {
    let at = (base + data.len) as u16;
    let mut n = 0u16;
    for ch in text.encode_utf16() {
        data.u16le(ch);
        n += 1;
    }
    (at, n)
}

/// Gets the next SSPI token: InitializeSecurityContextW with `input`.
fn sspi_step(
    cred: &mut SecHandle,
    ctx: &mut SecHandle,
    first: bool,
    input: &[u8],
    token: &mut [u8; 4096],
) -> Result<usize, u32> {
    let mut target = [0u16; 32];
    wide("MSSQLSvc/gala", &mut target);
    let mut in_copy = [0u8; 4096];
    in_copy[..input.len()].copy_from_slice(input);
    let mut in_buf = SecBuffer { size: input.len() as u32, kind: 2, data: in_copy.as_mut_ptr() };
    let mut in_desc = SecBufferDesc { version: 0, count: 1, buffers: &mut in_buf };
    let mut out_buf = SecBuffer { size: 4096, kind: 2, data: token.as_mut_ptr() };
    let mut out_desc = SecBufferDesc { version: 0, count: 1, buffers: &mut out_buf };
    let (mut attrs, mut expiry) = (0u32, 0i64);
    // ISC_REQ_CONNECTION | ISC_REQ_DELEGATE-free defaults: mutual auth off.
    let flags = 0x0000_0800; // ISC_REQ_CONNECTION
    // SAFETY: our handles and buffers.
    let status = unsafe {
        InitializeSecurityContextW(
            cred,
            if first { core::ptr::null_mut() } else { ctx },
            target.as_ptr(),
            flags,
            0,
            0x10, // SECURITY_NATIVE_DREP
            if first { core::ptr::null_mut() } else { &mut in_desc },
            0,
            ctx,
            &mut out_desc,
            &mut attrs,
            &mut expiry,
        )
    };
    // SEC_E_OK or SEC_I_CONTINUE_NEEDED.
    if status != 0 && status != 0x0009_0312 {
        return Err(fail(6, b"InitializeSecurityContextW", status as u32));
    }
    Ok(out_buf.size as usize)
}

/// Reads a B_VARCHAR / US_VARCHAR as ASCII into `out` (non-ASCII as '?').
fn text_of(units: &[u8], out: &mut [u8; 512]) -> usize {
    let mut n = 0;
    for c in units.chunks_exact(2) {
        let u = u16::from_le_bytes([c[0], c[1]]);
        if n < out.len() {
            out[n] = if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' };
            n += 1;
        }
    }
    n
}

/// What a token stream holds that the caller wants.
#[derive(Default)]
struct Seen {
    login_ack: bool,
    sspi: Option<(usize, usize)>,
    rows: u32,
    error: Option<u32>,
    done: bool,
}

/// Walks the tokens of a tabular result ([MS-TDS] 2.2.7): prints INFO,
/// ERROR and ROW values; records LOGINACK, SSPI, DONE.
fn tokens(d: &[u8]) -> Result<Seen, u32> {
    let mut seen = Seen::default();
    let mut i = 0;
    // Column types of the current COLMETADATA.
    let mut columns: [u8; 16] = [0; 16];
    let mut count = 0usize;
    let u16_at = |i: usize| usize::from(u16::from_le_bytes([d[i], d[i + 1]]));
    while i < d.len() {
        let t = d[i];
        i += 1;
        match t {
            0xaa | 0xab => {
                // ERROR / INFO: Number, State, Class, MsgText (US_VARCHAR).
                let len = u16_at(i);
                let number = u32::from_le_bytes([d[i + 2], d[i + 3], d[i + 4], d[i + 5]]);
                let chars = u16_at(i + 8);
                let mut text = [0u8; 512];
                let n = text_of(&d[i + 10..i + 10 + 2 * chars], &mut text);
                out(if t == 0xaa { b"tds_test: server error " } else { b"tds_test: server info " });
                out(&hex(number));
                out(b": ");
                out(&text[..n]);
                out(b"\r\n");
                if t == 0xaa {
                    seen.error = Some(number);
                }
                i += 2 + len;
            }
            0xad | 0xe3 | 0xa9 | 0xa4 | 0xa5 => {
                // LOGINACK, ENVCHANGE, ORDER, TABNAME, COLINFO: USHORT length.
                if t == 0xad {
                    seen.login_ack = true;
                }
                i += 2 + u16_at(i);
            }
            0xed => {
                let len = u16_at(i);
                seen.sspi = Some((i + 2, len));
                i += 2 + len;
            }
            0xe4 => {
                // SESSIONSTATE: DWORD length.
                let len = u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
                i += 4 + len;
            }
            0xae => {
                // FEATUREEXTACK: id, DWORD length, data; 0xFF ends.
                while d[i] != 0xff {
                    let len = u32::from_le_bytes([d[i + 1], d[i + 2], d[i + 3], d[i + 4]]) as usize;
                    i += 5 + len;
                }
                i += 1;
            }
            0x79 => i += 4, // RETURNSTATUS
            0xfd..=0xff => {
                // DONE / DONEPROC / DONEINPROC: Status, CurCmd, RowCount.
                if t == 0xfd {
                    seen.done = true;
                }
                i += 12;
            }
            0x81 => {
                // COLMETADATA: count, then UserType, Flags, TYPE_INFO, name.
                count = u16_at(i);
                i += 2;
                if count == 0xffff {
                    count = 0;
                    continue;
                }
                for col in 0..count {
                    i += 4 + 2; // UserType, Flags
                    let ty = d[i];
                    i += 1;
                    if col < columns.len() {
                        columns[col] = ty;
                    }
                    match ty {
                        0xe7 | 0xef | 0xa7 | 0xaf => i += 2 + 5, // max length, collation
                        0x38 | 0x34 | 0x30 | 0x7f => {}          // INT4, INT2, INT1, INT8
                        0x26 => i += 1,                          // INTN: length
                        _ => return Err(fail(20, b"column type", ty.into())),
                    }
                    let name = usize::from(d[i]);
                    i += 1 + 2 * name;
                }
            }
            0xd1 => {
                // ROW: each column's value.
                seen.rows += 1;
                out(b"tds_test: row:");
                for &ty in &columns[..count.min(columns.len())] {
                    out(b" [");
                    match ty {
                        0xe7 | 0xef | 0xa7 | 0xaf => {
                            let len = u16_at(i);
                            i += 2;
                            if len != 0xffff {
                                if ty == 0xe7 || ty == 0xef {
                                    let mut text = [0u8; 512];
                                    let n = text_of(&d[i..i + len], &mut text);
                                    out(&text[..n]);
                                } else {
                                    out(&d[i..i + len]);
                                }
                                i += len;
                            }
                        }
                        0x38 => {
                            out(&hex(u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])));
                            i += 4;
                        }
                        0x26 => {
                            let len = usize::from(d[i]);
                            i += 1 + len;
                        }
                        _ => return Err(fail(21, b"row type", ty.into())),
                    }
                    out(b"]");
                }
                out(b"\r\n");
            }
            _ => return Err(fail(22, b"token", t.into())),
        }
    }
    Ok(seen)
}

/// LOGIN7 ([MS-TDS] 2.2.6.4) with integrated security, then the SSPI
/// exchange until LOGINACK.
fn login(c: &mut Conn) -> Result<(), u32> {
    let mut cred = SecHandle { lower: 0, upper: 0 };
    let mut ctx = SecHandle { lower: 0, upper: 0 };
    let mut package = [0u16; 16];
    wide("Negotiate", &mut package);
    let mut expiry = 0i64;
    // SAFETY: our outputs; SECPKG_CRED_OUTBOUND (2), the caller's logon.
    let status = unsafe {
        AcquireCredentialsHandleW(
            core::ptr::null(),
            package.as_ptr(),
            2,
            core::ptr::null_mut(),
            core::ptr::null(),
            0,
            0,
            &mut cred,
            &mut expiry,
        )
    };
    if status != 0 {
        return Err(fail(7, b"AcquireCredentialsHandleW", status as u32));
    }
    let mut token = [0u8; 4096];
    let first = sspi_step(&mut cred, &mut ctx, true, &[], &mut token)?;

    const FIXED: usize = 94;
    let mut data = Buf::new();
    let host = login_string(&mut data, FIXED, "GALA");
    let user = login_string(&mut data, FIXED, "");
    let password = login_string(&mut data, FIXED, "");
    let app = login_string(&mut data, FIXED, "gala tds_test");
    let server = login_string(&mut data, FIXED, "np:\\\\.\\pipe\\LOCALDB#GALA0001\\tsql\\query");
    let unused = ((FIXED + data.len) as u16, 0u16);
    let library = login_string(&mut data, FIXED, "gala");
    let language = login_string(&mut data, FIXED, "");
    let database = login_string(&mut data, FIXED, "master");
    let sspi_at = (FIXED + data.len) as u16;
    data.put(&token[..first]);
    let attach = ((FIXED + data.len) as u16, 0u16);
    let change = ((FIXED + data.len) as u16, 0u16);

    let mut l = Buf::new();
    l.u32le((FIXED + data.len) as u32);
    l.u32le(0x7400_0004); // TDS 7.4
    l.u32le(4096); // packet size
    l.u32le(0x0700_0000); // client program version
    // SAFETY: a plain call.
    l.u32le(unsafe { GetCurrentProcessId() });
    l.u32le(0); // connection id
    l.u8(0xe0); // fUseDB | fDatabase | fSetLang
    l.u8(0x80 | 0x02 | 0x01); // fIntSecurity | fODBC | fLanguage
    l.u8(0); // type flags
    l.u8(0); // option flags 3
    l.u32le(0); // time zone
    l.u32le(0x0409); // LCID
    for (at, n) in [host, user, password, app, server, unused, library, language, database] {
        l.u16le(at);
        l.u16le(n);
    }
    l.put(&[0x00, 0x15, 0x5d, 0x00, 0x00, 0x01]); // client id (MAC)
    l.u16le(sspi_at);
    l.u16le(first as u16);
    for (at, n) in [attach, change] {
        l.u16le(at);
        l.u16le(n);
    }
    l.u32le(0); // cbSSPILong
    l.put(data.bytes());
    if !c.send(LOGIN7, l.bytes()) {
        return Err(fail(8, b"send LOGIN7", 0));
    }
    for round in 0..4 {
        let mut r = Buf::new();
        if c.receive(&mut r) != Some(TABULAR_RESULT) {
            return Err(fail(9, b"login response", round));
        }
        let seen = tokens(r.bytes())?;
        if let Some(e) = seen.error {
            return Err(fail(10, b"login failed", e));
        }
        if seen.login_ack {
            out(b"tds_test: logged in\r\n");
            return Ok(());
        }
        let Some((at, len)) = seen.sspi else { return Err(fail(11, b"no SSPI token", 0)) };
        let challenge = &r.bytes()[at..at + len];
        let n = sspi_step(&mut cred, &mut ctx, false, challenge, &mut token)?;
        if !c.send(SSPI, &token[..n]) {
            return Err(fail(12, b"send SSPI", 0));
        }
    }
    Err(fail(13, b"too many SSPI rounds", 0))
}

/// Runs a SQL batch ([MS-TDS] 2.2.6.7, with ALL_HEADERS); its result.
fn batch(c: &mut Conn, sql: &str, step: u32) -> Result<Seen, u32> {
    let mut p = Buf::new();
    p.u32le(22); // ALL_HEADERS total length
    p.u32le(18); // header length
    p.u16le(2); // transaction descriptor
    p.put(&[0; 8]);
    p.u32le(1); // outstanding requests
    for ch in sql.encode_utf16() {
        p.u16le(ch);
    }
    if !c.send(SQL_BATCH, p.bytes()) {
        return Err(fail(step, b"send SQL_BATCH", 0));
    }
    let mut r = Buf::new();
    if c.receive(&mut r) != Some(TABULAR_RESULT) {
        return Err(fail(step + 1, b"batch response", 0));
    }
    tokens(r.bytes())
}

fn run() -> Result<(), u32> {
    let server = start_server()?;
    let pipe = connect()?;
    out(b"tds_test: connected\r\n");
    let mut c = Conn { pipe, packet_id: 1 };
    prelogin(&mut c)?;
    login(&mut c)?;
    let seen = batch(&mut c, "SELECT @@VERSION, SUSER_SNAME()", 30)?;
    if let Some(e) = seen.error {
        return Err(fail(32, b"query failed", e));
    }
    if seen.rows != 1 || !seen.done {
        return Err(fail(33, b"rows", seen.rows));
    }
    // Disconnect; the instance ends itself after its idle time (-w1).
    // (SHUTDOWN is accepted, but sqlservr then fails while terminating:
    // ROADMAP.)
    // SAFETY: our handles.
    unsafe {
        CloseHandle(pipe);
        if WaitForSingleObject(server, 180_000) != 0 {
            return Err(fail(42, b"sqlservr did not exit", 0));
        }
        let mut code = 0;
        GetExitCodeProcess(server, &mut code);
        out(b"tds_test: sqlservr exited with ");
        out(&hex(code));
        out(b"\r\n");
        CloseHandle(server);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"tds_test: ok\r\n");
            0
        }
        Err(step) => step,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

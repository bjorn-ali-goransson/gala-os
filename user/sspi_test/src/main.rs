//! SSPI's NTLM package as a program on one machine uses it for both
//! sides of a connection, with the results Windows documents: the
//! package's information (QuerySecurityPackageInfoW: RPC id 10,
//! cbMaxToken 2888; integrity, privacy, connection), its listing,
//! credentials of the current user both ways, the three-message exchange
//! (NEGOTIATE from InitializeSecurityContextW, CHALLENGE from
//! AcceptSecurityContext, AUTHENTICATE, accepted), the client's name as
//! the server sees it, signatures (MakeSignature/VerifySignature;
//! SEC_E_MESSAGE_ALTERED when the message changed), sealing
//! (EncryptMessage/DecryptMessage), and a wrong password
//! (SEC_E_LOGON_DENIED). Kerberos on a machine in a workgroup: the
//! package (RPC id 16) and credentials exist, but a context cannot start
//! (SEC_E_NO_AUTHENTICATING_AUTHORITY: no domain controller). Negotiate
//! (RPC id 9) authenticates with NTLM. Schannel's package information
//! (RPC id 14). security.dll (the older name) forwards to the same
//! functions.
//!
//! Prints `sspi_test: ok` or exits with the number of the failing check.

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

const NO_HANDLE: SecHandle = SecHandle { lower: 0, upper: 0 };

#[repr(C)]
struct SecPkgInfo {
    capabilities: u32,
    version: u16,
    rpc_id: u16,
    max_token: u32,
    name: *const u16,
    comment: *const u16,
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

/// SEC_WINNT_AUTH_IDENTITY_W.
#[repr(C)]
struct Identity {
    user: *const u16,
    user_length: u32,
    domain: *const u16,
    domain_length: u32,
    password: *const u16,
    password_length: u32,
    flags: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn QuerySecurityPackageInfoW(name: *const u16, info: *mut *mut SecPkgInfo) -> i32;
    fn EnumerateSecurityPackagesW(count: *mut u32, info: *mut *mut SecPkgInfo) -> i32;
    fn FreeContextBuffer(buffer: *mut c_void) -> i32;
    fn AcquireCredentialsHandleW(
        principal: *const u16,
        package: *const u16,
        usage: u32,
        logon_id: *mut c_void,
        auth: *const Identity,
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
    fn AcceptSecurityContext(
        credential: *mut SecHandle,
        context: *mut SecHandle,
        input: *mut SecBufferDesc,
        flags: u32,
        representation: u32,
        new_context: *mut SecHandle,
        output: *mut SecBufferDesc,
        attributes: *mut u32,
        expiry: *mut i64,
    ) -> i32;
    fn DeleteSecurityContext(context: *mut SecHandle) -> i32;
    fn QueryContextAttributesW(context: *mut SecHandle, attribute: u32, buffer: *mut c_void)
    -> i32;
    fn MakeSignature(
        context: *mut SecHandle,
        qop: u32,
        message: *mut SecBufferDesc,
        seq: u32,
    ) -> i32;
    fn VerifySignature(
        context: *mut SecHandle,
        message: *mut SecBufferDesc,
        seq: u32,
        qop: *mut u32,
    ) -> i32;
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

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // The stack probe for frames over a page (ntdll exports it).
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

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"sspi_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

const SEC_E_OK: u64 = 0;
const SEC_I_CONTINUE_NEEDED: u64 = 0x0009_0312;
const SEC_E_LOGON_DENIED: u64 = 0x8009_030c;
const SEC_E_MESSAGE_ALTERED: u64 = 0x8009_030f;
const SECPKG_CRED_INBOUND: u32 = 1;
const SECPKG_CRED_OUTBOUND: u32 = 2;
const SECBUFFER_DATA: u32 = 1;
const SECBUFFER_TOKEN: u32 = 2;
const SECURITY_NATIVE_DREP: u32 = 0x10;
/// ISC_REQ_/ASC_REQ_ CONFIDENTIALITY | INTEGRITY | CONNECTION.
const ISC_FLAGS: u32 = 0x10 | 0x1_0000 | 0x800;
const ASC_FLAGS: u32 = 0x10 | 0x2_0000 | 0x800;

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 48] {
    let mut b = [0u16; 48];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

/// The NUL-terminated string at `p` equals `want`.
fn same(p: *const u16, want: &str) -> bool {
    // SAFETY: callers pass NUL-terminated strings.
    unsafe {
        want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) && *p.add(want.len()) == 0
    }
}

/// A token buffer and its descriptor.
struct Token {
    bytes: [u8; 2888],
    buffer: SecBuffer,
}

impl Token {
    fn new() -> Token {
        Token {
            bytes: [0; 2888],
            buffer: SecBuffer { size: 0, kind: SECBUFFER_TOKEN, data: core::ptr::null_mut() },
        }
    }

    /// A descriptor for output (the whole buffer) or input (`size` bytes).
    fn desc(&mut self, size: u32) -> SecBufferDesc {
        self.buffer = SecBuffer { size, kind: SECBUFFER_TOKEN, data: self.bytes.as_mut_ptr() };
        SecBufferDesc { version: 0, count: 1, buffers: &mut self.buffer }
    }

    fn message_type(&self) -> u32 {
        if &self.bytes[..8] != b"NTLMSSP\0" {
            return 0;
        }
        u32::from_le_bytes([self.bytes[8], self.bytes[9], self.bytes[10], self.bytes[11]])
    }
}

/// Runs the exchange with client credentials `client`; returns the final
/// AcceptSecurityContext status and the two contexts.
fn exchange(
    c: &mut Checks,
    client: &mut SecHandle,
    server: &mut SecHandle,
) -> Result<(u64, SecHandle, SecHandle), u32> {
    let null: *mut c_void = core::ptr::null_mut();
    let target = w("gala");
    let (mut ctx_c, mut ctx_s) = (NO_HANDLE, NO_HANDLE);
    let (mut attrs, mut expiry) = (0u32, 0i64);
    let (mut t1, mut t2, mut t3) = (Token::new(), Token::new(), Token::new());
    // SAFETY: SSPI calls with buffers and handles of ours.
    unsafe {
        let mut out = t1.desc(2888);
        let r = InitializeSecurityContextW(
            client,
            null.cast(),
            target.as_ptr(),
            ISC_FLAGS,
            0,
            SECURITY_NATIVE_DREP,
            null.cast(),
            0,
            &mut ctx_c,
            &mut out,
            &mut attrs,
            &mut expiry,
        );
        c.eq(r as u32 as u64, SEC_I_CONTINUE_NEEDED)?;
        let n1 = t1.buffer.size;
        c.eq(t1.message_type().into(), 1)?;

        let mut input = t1.desc(n1);
        let mut out = t2.desc(2888);
        let r = AcceptSecurityContext(
            server,
            null.cast(),
            &mut input,
            ASC_FLAGS,
            SECURITY_NATIVE_DREP,
            &mut ctx_s,
            &mut out,
            &mut attrs,
            &mut expiry,
        );
        c.eq(r as u32 as u64, SEC_I_CONTINUE_NEEDED)?;
        let n2 = t2.buffer.size;
        c.eq(t2.message_type().into(), 2)?;

        let mut input = t2.desc(n2);
        let mut out = t3.desc(2888);
        let r = InitializeSecurityContextW(
            client,
            &mut ctx_c,
            target.as_ptr(),
            ISC_FLAGS,
            0,
            SECURITY_NATIVE_DREP,
            &mut input,
            0,
            &mut ctx_c,
            &mut out,
            &mut attrs,
            &mut expiry,
        );
        c.eq(r as u32 as u64, SEC_E_OK)?;
        let n3 = t3.buffer.size;
        c.eq(t3.message_type().into(), 3)?;

        let mut input = t3.desc(n3);
        let mut out = t2.desc(2888);
        let r = AcceptSecurityContext(
            server,
            &mut ctx_s,
            &mut input,
            ASC_FLAGS,
            SECURITY_NATIVE_DREP,
            &mut ctx_s,
            &mut out,
            &mut attrs,
            &mut expiry,
        );
        Ok((r as u32 as u64, ctx_c, ctx_s))
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: SSPI calls with buffers and handles of ours; the package
    // information is what secur32 returns.
    unsafe {
        let ntlm = w("NTLM");
        let mut info: *mut SecPkgInfo = null.cast();
        c.eq(QuerySecurityPackageInfoW(ntlm.as_ptr(), &mut info) as u64, SEC_E_OK)?;
        c.ok(same((*info).name, "NTLM"), 0)?;
        c.eq((*info).rpc_id.into(), 10)?; // RPC_C_AUTHN_WINNT
        c.eq((*info).max_token.into(), 2888)?;
        c.eq((*info).version.into(), 1)?;
        // SECPKG_FLAG_INTEGRITY | PRIVACY | CONNECTION.
        c.eq(((*info).capabilities & 0x13).into(), 0x13)?;
        c.eq(FreeContextBuffer(info.cast()) as u64, SEC_E_OK)?;

        let mut count = 0;
        c.eq(EnumerateSecurityPackagesW(&mut count, &mut info) as u64, SEC_E_OK)?;
        let listed = (0..count as usize).any(|i| same((*info.add(i)).name, "NTLM"));
        c.ok(listed, count.into())?;
        c.eq(FreeContextBuffer(info.cast()) as u64, SEC_E_OK)?;

        // Credentials of the current user, both ways.
        let (mut client, mut server) = (NO_HANDLE, NO_HANDLE);
        let mut expiry = 0i64;
        let r = AcquireCredentialsHandleW(
            null.cast(),
            ntlm.as_ptr(),
            SECPKG_CRED_OUTBOUND,
            null,
            null.cast(),
            0,
            0,
            &mut client,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;
        let r = AcquireCredentialsHandleW(
            null.cast(),
            ntlm.as_ptr(),
            SECPKG_CRED_INBOUND,
            null,
            null.cast(),
            0,
            0,
            &mut server,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;

        let (status, mut ctx_c, mut ctx_s) = exchange(&mut c, &mut client, &mut server)?;
        c.eq(status, SEC_E_OK)?;

        // The client's name, as the server sees it.
        let mut name: *mut u16 = null.cast();
        c.eq(QueryContextAttributesW(&mut ctx_s, 1, (&raw mut name).cast()) as u64, SEC_E_OK)?;
        c.ok(same(name, "GALA\\gala"), 0)?;
        FreeContextBuffer(name.cast());
        // SecPkgContext_Sizes: cbMaxToken, cbMaxSignature, cbBlockSize,
        // cbSecurityTrailer.
        let mut sizes = [0u32; 4];
        c.eq(QueryContextAttributesW(&mut ctx_c, 0, sizes.as_mut_ptr().cast()) as u64, SEC_E_OK)?;
        c.ok(sizes[1] == 16 && sizes[3] == 16, sizes[1].into())?;

        // A signature, checked by the other side; a changed message is not.
        let mut data = *b"hello, gala";
        let mut sig = [0u8; 16];
        let mut buffers = [
            SecBuffer { size: data.len() as u32, kind: SECBUFFER_DATA, data: data.as_mut_ptr() },
            SecBuffer { size: 16, kind: SECBUFFER_TOKEN, data: sig.as_mut_ptr() },
        ];
        let mut desc = SecBufferDesc { version: 0, count: 2, buffers: buffers.as_mut_ptr() };
        c.eq(MakeSignature(&mut ctx_c, 0, &mut desc, 0) as u64, SEC_E_OK)?;
        let mut qop = 0;
        c.eq(VerifySignature(&mut ctx_s, &mut desc, 0, &mut qop) as u64, SEC_E_OK)?;
        c.eq(MakeSignature(&mut ctx_c, 0, &mut desc, 1) as u64, SEC_E_OK)?;
        *buffers[0].data = b'j';
        c.eq(
            VerifySignature(&mut ctx_s, &mut desc, 1, &mut qop) as u32 as u64,
            SEC_E_MESSAGE_ALTERED,
        )?;

        // Sealed: the data changes, and the other side recovers it.
        let mut data = *b"secret data";
        let mut buffers = [
            SecBuffer { size: 16, kind: SECBUFFER_TOKEN, data: sig.as_mut_ptr() },
            SecBuffer { size: data.len() as u32, kind: SECBUFFER_DATA, data: data.as_mut_ptr() },
        ];
        let mut desc = SecBufferDesc { version: 0, count: 2, buffers: buffers.as_mut_ptr() };
        c.eq(EncryptMessage(&mut ctx_c, 0, &mut desc, 2) as u64, SEC_E_OK)?;
        c.ok(&data != b"secret data", 0)?;
        c.eq(DecryptMessage(&mut ctx_s, &mut desc, 2, &mut qop) as u64, SEC_E_OK)?;
        c.ok(&data == b"secret data", data[0].into())?;

        c.eq(DeleteSecurityContext(&mut ctx_c) as u64, SEC_E_OK)?;
        c.eq(DeleteSecurityContext(&mut ctx_s) as u64, SEC_E_OK)?;

        // Explicit credentials with a wrong password: denied.
        let (user, domain, password) = (w("gala"), w("GALA"), w("not the password"));
        let identity = Identity {
            user: user.as_ptr(),
            user_length: 4,
            domain: domain.as_ptr(),
            domain_length: 4,
            password: password.as_ptr(),
            password_length: 16,
            flags: 2, // SEC_WINNT_AUTH_IDENTITY_UNICODE
        };
        let mut wrong = NO_HANDLE;
        let r = AcquireCredentialsHandleW(
            null.cast(),
            ntlm.as_ptr(),
            SECPKG_CRED_OUTBOUND,
            null,
            &identity,
            0,
            0,
            &mut wrong,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;
        let (status, mut ctx_c, mut ctx_s) = exchange(&mut c, &mut wrong, &mut server)?;
        c.eq(status, SEC_E_LOGON_DENIED)?;
        DeleteSecurityContext(&mut ctx_c);
        DeleteSecurityContext(&mut ctx_s);

        c.eq(FreeCredentialsHandle(&mut wrong) as u64, SEC_E_OK)?;

        let kerberos = w("Kerberos");
        c.eq(QuerySecurityPackageInfoW(kerberos.as_ptr(), &mut info) as u64, SEC_E_OK)?;
        c.ok(same((*info).name, "Kerberos"), 0)?;
        c.eq((*info).rpc_id.into(), 16)?; // RPC_C_AUTHN_GSS_KERBEROS
        FreeContextBuffer(info.cast());
        let mut krb = NO_HANDLE;
        let r = AcquireCredentialsHandleW(
            null.cast(),
            kerberos.as_ptr(),
            SECPKG_CRED_OUTBOUND,
            null,
            null.cast(),
            0,
            0,
            &mut krb,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;
        let mut t = Token::new();
        let mut out = t.desc(2888);
        let (mut ctx, mut attrs) = (NO_HANDLE, 0u32);
        let target = w("MSSQLSvc/gala");
        let r = InitializeSecurityContextW(
            &mut krb,
            null.cast(),
            target.as_ptr(),
            ISC_FLAGS,
            0,
            SECURITY_NATIVE_DREP,
            null.cast(),
            0,
            &mut ctx,
            &mut out,
            &mut attrs,
            &mut expiry,
        );
        c.eq(r as u32 as u64, 0x8009_0311)?; // SEC_E_NO_AUTHENTICATING_AUTHORITY
        c.eq(FreeCredentialsHandle(&mut krb) as u64, SEC_E_OK)?;

        let negotiate = w("Negotiate");
        c.eq(QuerySecurityPackageInfoW(negotiate.as_ptr(), &mut info) as u64, SEC_E_OK)?;
        c.ok(same((*info).name, "Negotiate"), 0)?;
        c.eq((*info).rpc_id.into(), 9)?; // RPC_C_AUTHN_GSS_NEGOTIATE
        FreeContextBuffer(info.cast());
        let (mut nc, mut ns) = (NO_HANDLE, NO_HANDLE);
        let r = AcquireCredentialsHandleW(
            null.cast(),
            negotiate.as_ptr(),
            SECPKG_CRED_OUTBOUND,
            null,
            null.cast(),
            0,
            0,
            &mut nc,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;
        let r = AcquireCredentialsHandleW(
            null.cast(),
            negotiate.as_ptr(),
            SECPKG_CRED_INBOUND,
            null,
            null.cast(),
            0,
            0,
            &mut ns,
            &mut expiry,
        );
        c.eq(r as u64, SEC_E_OK)?;
        let (status, mut ctx_c, mut ctx_s) = exchange(&mut c, &mut nc, &mut ns)?;
        c.eq(status, SEC_E_OK)?;
        DeleteSecurityContext(&mut ctx_c);
        DeleteSecurityContext(&mut ctx_s);
        FreeCredentialsHandle(&mut nc);
        FreeCredentialsHandle(&mut ns);

        let schannel = w("Microsoft Unified Security Protocol Provider");
        c.eq(QuerySecurityPackageInfoW(schannel.as_ptr(), &mut info) as u64, SEC_E_OK)?;
        c.ok(same((*info).name, "Microsoft Unified Security Protocol Provider"), 0)?;
        c.eq((*info).rpc_id.into(), 14)?; // RPC_C_AUTHN_GSS_SCHANNEL
        FreeContextBuffer(info.cast());

        let security = LoadLibraryA(c"security.dll".as_ptr().cast());
        let secur32 = LoadLibraryA(c"secur32.dll".as_ptr().cast());
        c.ok(security != 0 && secur32 != 0, 0)?;
        for name in [c"InitSecurityInterfaceW", c"AcquireCredentialsHandleW", c"FreeContextBuffer"]
        {
            let a = GetProcAddress(security, name.as_ptr().cast());
            c.ok(a != 0 && a == GetProcAddress(secur32, name.as_ptr().cast()), a as u64)?;
        }
        c.eq(FreeCredentialsHandle(&mut client) as u64, SEC_E_OK)?;
        c.eq(FreeCredentialsHandle(&mut server) as u64, SEC_E_OK)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"sspi_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

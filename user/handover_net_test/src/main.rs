//! A TCP server carried across a kernel handover while it serves
//! (ROADMAP R9 milestone (e), docs/ARCHITECTURE.md "Sockets across a
//! handover"): the peer sees at most a pause, never a reset.
//!
//! It listens on port 7 of every address (backlog 5), prints
//! `handover_net_test: listening on port 7` and serves three connections
//! one after the other, all on that listening socket: each line it
//! receives is answered with `<n> <line>`, n counting every line of every
//! connection (the program's state). The line `handover` asks gala for a
//! kernel handover (GalaRequestHandover, test-only) and is answered
//! `<n> handover requested` at once; the program then blocks in recv
//! (a pending AFD receive) while the kernel hands over. The line `bye` is
//! answered and the connection closed.
//!
//! It also binds UDP port 7 of every address and leaves it alone until a
//! connection sends the line `udp`: then it answers every datagram queued
//! there with `udp <datagram>` to its sender and the line with
//! `<n> udp <k>`, k the datagrams answered. The client sends one datagram
//! before the handover (it waits in the socket's queue across it) and one
//! after: the port, the queued datagram and new ones survive.
//!
//! xtask's client (the `handover_net` scenarios) talks to it through
//! QEMU's port forward: the first connection before, across and after the
//! handover; a second one it opened before the handover, which waits
//! unaccepted in the listener's backlog with its first line unread until
//! the program accepts it after the handover; a third one opened after
//! it, which also asks for the UDP datagrams. The client checks every
//! answer and that no connection was reset.
//!
//! With the argument `self` a thread of its own plays xtask's client over
//! loopback (127.0.0.1:7), so the program also runs alone, on Windows
//! (user/bin/run-all.ps1) and in the `handover_net_self` scenario: the
//! answer to `handover` is then `<n> handover skipped (not gala)` on
//! Windows (GalaRequestHandover is gala's own: not called there) or
//! `<n> handover skipped (none allowed)` on gala without `handover=`, and
//! the rest is the same. Prints `handover_net_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetCommandLineW() -> *const u16;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> i32;
    fn Sleep(ms: u32);
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn sendto(
        s: Socket,
        buf: *const u8,
        len: i32,
        flags: i32,
        to: *const SockaddrIn,
        to_len: i32,
    ) -> i32;
    fn recvfrom(
        s: Socket,
        buf: *mut u8,
        len: i32,
        flags: i32,
        from: *mut SockaddrIn,
        from_len: *mut i32,
    ) -> i32;
    fn select(
        n: i32,
        read: *mut FdSet,
        write: *mut FdSet,
        except: *mut FdSet,
        timeout: *const [i32; 2],
    ) -> i32;
    fn closesocket(s: Socket) -> i32;
}

/// fd_set (x64): the count, then the sockets at offset 8.
#[repr(C)]
struct FdSet {
    count: u32,
    sockets: [Socket; 1],
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// WSADATA (x64 layout: the version fields, then the pointers).
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

/// SOCKADDR_IN: family, port and address in network byte order.
#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;
const INVALID_SOCKET: Socket = !0;
const PORT: u16 = 7;
const CONNECTIONS: u32 = 3;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;

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

fn decimal(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return &buf[i..];
        }
    }
}

/// Fails check `n`, printing the Winsock error.
fn fail(n: u32) -> u32 {
    // SAFETY: plain Winsock call.
    let error = unsafe { WSAGetLastError() };
    let (mut a, mut b) = ([0u8; 20], [0u8; 20]);
    out_line(&[
        b"handover_net_test: check ",
        decimal(u64::from(n), &mut a),
        b" failed, WSAGetLastError ",
        decimal(error as u64, &mut b),
        b"\r\n",
    ]);
    n
}

/// Writes `parts` as one line in one write: the self client's thread
/// prints too, and pieces written apart interleave with its line.
fn out_line(parts: &[&[u8]]) {
    let mut line = [0u8; 160];
    let mut n = 0;
    for p in parts {
        let k = p.len().min(line.len() - n);
        line[n..n + k].copy_from_slice(&p[..k]);
        n += k;
    }
    out(&line[..n]);
}

/// GalaRequestHandover (gala's test-only service 0x7f5a).
fn request_handover() -> u32 {
    let status: u32;
    // SAFETY: a system call with no arguments; the registers it may
    // change are declared.
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("eax") 0x7f5au32 => status,
            inout("r10") 0usize => _,
            lateout("rcx") _,
            lateout("rdx") _,
            lateout("r8") _,
            lateout("r9") _,
            lateout("r11") _,
            options(nostack),
        )
    };
    status
}

/// Sends all of `data`.
fn send_all(s: Socket, data: &[u8]) -> Result<(), u32> {
    let mut sent = 0;
    while sent < data.len() {
        // SAFETY: a valid buffer of the length given.
        let n = unsafe { send(s, data[sent..].as_ptr(), (data.len() - sent) as i32, 0) };
        if n <= 0 {
            return Err(fail(10));
        }
        sent += n as usize;
    }
    Ok(())
}

/// Whether `s` is readable within `ms` milliseconds.
fn readable(s: Socket, ms: i32) -> bool {
    let mut set = FdSet { count: 1, sockets: [s] };
    let timeout = [ms / 1000, ms % 1000 * 1000];
    // SAFETY: a valid fd_set and timeval.
    unsafe { select(0, &mut set, core::ptr::null_mut(), core::ptr::null_mut(), &timeout) == 1 }
}

/// `udp`: answers every datagram queued on the UDP socket with
/// `udp <datagram>`; returns how many.
fn answer_datagrams(udp: Socket) -> Result<u64, u32> {
    let mut answered = 0;
    let mut wait = 2000;
    while readable(udp, wait) {
        wait = 300;
        let mut buf = [0u8; 256];
        buf[..4].copy_from_slice(b"udp ");
        let mut from = SockaddrIn { family: 0, port: 0, addr: [0; 4], zero: [0; 8] };
        let mut len = size_of::<SockaddrIn>() as i32;
        // SAFETY: valid buffers of the lengths given.
        let n = unsafe { recvfrom(udp, buf[4..].as_mut_ptr(), 252, 0, &mut from, &mut len) };
        if n < 0 {
            return Err(fail(14));
        }
        // SAFETY: as above.
        let sent = unsafe { sendto(udp, buf.as_ptr(), n + 4, 0, &from, len) };
        if sent != n + 4 {
            return Err(fail(15));
        }
        answered += 1;
    }
    Ok(answered)
}

/// Serves one connection; returns how many lines it answered. `count`
/// numbers every line of every connection.
fn serve(s: Socket, udp: Socket, count: &mut u64) -> Result<u64, u32> {
    let mut buf = [0u8; 512];
    let mut have = 0usize;
    let mut lines = 0;
    loop {
        // A whole line in the buffer?
        if let Some(end) = buf[..have].iter().position(|&b| b == b'\n') {
            let line = &buf[..end];
            *count += 1;
            lines += 1;
            let mut reply = [0u8; 600];
            let mut len = 0;
            let mut put = |bytes: &[u8]| {
                reply[len..len + bytes.len()].copy_from_slice(bytes);
                len += bytes.len();
            };
            let mut d = [0u8; 20];
            put(decimal(*count, &mut d));
            put(b" ");
            let bye = line == b"bye";
            if line == b"handover" {
                put(if !on_gala() {
                    b"handover skipped (not gala)"
                } else {
                    match request_handover() {
                        0 => b"handover requested",
                        STATUS_ACCESS_DENIED => b"handover skipped (none allowed)",
                        _ => b"handover failed",
                    }
                });
            } else if line == b"udp" {
                put(b"udp ");
                put(decimal(answer_datagrams(udp)?, &mut d));
            } else {
                put(line);
            }
            put(b"\n");
            send_all(s, &reply[..len])?;
            buf.copy_within(end + 1..have, 0);
            have -= end + 1;
            if bye {
                return Ok(lines);
            }
            continue;
        }
        if have == buf.len() {
            return Err(11);
        }
        // SAFETY: the rest of the buffer.
        let n = unsafe { recv(s, buf[have..].as_mut_ptr(), (buf.len() - have) as i32, 0) };
        match n {
            0 => return Ok(lines),
            n if n < 0 => return Err(fail(12)),
            n => have += n as usize,
        }
    }
}

/// Whether the command line ends with the word `self`.
fn wants_self() -> bool {
    // SAFETY: GetCommandLineW returns a NUL-terminated string that lives
    // as long as the process.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let word = b"self".map(u16::from);
    let mut end = line.len();
    while end > 0 && line[end - 1] == u16::from(b' ') {
        end -= 1;
    }
    end >= word.len() && line[end - word.len()..end] == word
}

/// The `self` client's side of a connection: lines in, lines out.
struct Peer {
    s: Socket,
}

impl Peer {
    fn open() -> Result<Self, u32> {
        let to = SockaddrIn {
            family: AF_INET as u16,
            port: PORT.to_be(),
            addr: [127, 0, 0, 1],
            zero: [0; 8],
        };
        // SAFETY: documented Winsock calls with valid arguments.
        unsafe {
            let s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
            if s == INVALID_SOCKET || connect(s, &to, size_of::<SockaddrIn>() as i32) != 0 {
                return Err(fail(40));
            }
            Ok(Peer { s })
        }
    }

    fn send(&self, line: &[u8]) -> Result<(), u32> {
        send_all(self.s, line)?;
        send_all(self.s, b"\n")
    }

    /// The next line (without its newline); empty at the end of stream.
    fn line<'a>(&self, buf: &'a mut [u8; 128]) -> Result<&'a [u8], u32> {
        let mut n = 0;
        loop {
            let mut b = 0u8;
            // SAFETY: a one-byte buffer.
            match unsafe { recv(self.s, &mut b, 1, 0) } {
                1 if b == b'\n' => return Ok(&buf[..n]),
                1 if n < buf.len() => {
                    buf[n] = b;
                    n += 1;
                }
                0 if n == 0 => return Ok(&[]),
                _ => return Err(fail(41)),
            }
        }
    }

    /// Expects the answer `<n> <text>` (only its start if `prefix`).
    fn expect(&self, n: u64, text: &[u8], prefix: bool, check: u32) -> Result<(), u32> {
        let mut want = [0u8; 64];
        let mut d = [0u8; 20];
        let number = decimal(n, &mut d);
        let len = number.len() + 1 + text.len();
        want[..number.len()].copy_from_slice(number);
        want[number.len()] = b' ';
        want[number.len() + 1..len].copy_from_slice(text);
        let mut buf = [0u8; 128];
        let got = self.line(&mut buf)?;
        let same = if prefix { got.starts_with(&want[..len]) } else { got == &want[..len] };
        if !same {
            out(b"handover_net_test: self client got \"");
            out(got);
            out(b"\"\r\n");
            return Err(check);
        }
        Ok(())
    }

    fn ask(&self, line: &[u8], n: u64, check: u32) -> Result<(), u32> {
        self.send(line)?;
        self.expect(n, line, false, check)
    }

    /// After `bye`: the end of the stream, not a reset.
    fn closed(self, check: u32) -> Result<(), u32> {
        let mut buf = [0u8; 128];
        if !self.line(&mut buf)?.is_empty() {
            return Err(check);
        }
        // SAFETY: the socket is ours.
        if unsafe { closesocket(self.s) } != 0 {
            return Err(fail(check + 1));
        }
        Ok(())
    }
}

/// The `self` client's datagrams: one to the server's UDP port.
fn datagram_to_server(s: Socket, data: &[u8]) -> Result<(), u32> {
    let to = SockaddrIn {
        family: AF_INET as u16,
        port: PORT.to_be(),
        addr: [127, 0, 0, 1],
        zero: [0; 8],
    };
    // SAFETY: valid buffers.
    let n = unsafe {
        sendto(s, data.as_ptr(), data.len() as i32, 0, &to, size_of::<SockaddrIn>() as i32)
    };
    if n != data.len() as i32 { Err(fail(65)) } else { Ok(()) }
}

/// The `self` client: xtask's conversation (`handover_net_client`) over
/// loopback; 0 or the number of the failing check.
extern "system" fn self_client(_: *mut c_void) -> u32 {
    let run = || -> Result<(), u32> {
        // SAFETY: plain Winsock call.
        let udp = unsafe { socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP) };
        if udp == INVALID_SOCKET {
            return Err(fail(66));
        }
        let first = Peer::open()?;
        first.ask(b"hello", 1, 50)?;
        first.ask(b"before", 2, 51)?;
        let second = Peer::open()?;
        second.send(b"early")?;
        datagram_to_server(udp, b"early-udp")?;
        // SAFETY: plain call.
        unsafe { Sleep(200) };
        first.send(b"handover")?;
        first.expect(3, b"handover ", true, 52)?;
        first.ask(b"during", 4, 53)?;
        for i in 0..10u8 {
            let line = [b'l', b'i', b'n', b'e', b' ', b'0' + i];
            first.ask(&line, 5 + u64::from(i), 54)?;
        }
        first.ask(b"bye", 15, 55)?;
        first.closed(56)?;
        second.expect(16, b"early", false, 58)?;
        second.ask(b"bye", 17, 59)?;
        second.closed(60)?;
        let third = Peer::open()?;
        third.ask(b"after", 18, 62)?;
        datagram_to_server(udp, b"late-udp")?;
        // SAFETY: plain call.
        unsafe { Sleep(200) };
        third.send(b"udp")?;
        third.expect(19, b"udp 2", false, 67)?;
        for want in [&b"udp early-udp"[..], b"udp late-udp"] {
            let mut buf = [0u8; 64];
            // SAFETY: a valid buffer; the sender is not asked for.
            let n = unsafe {
                recvfrom(udp, buf.as_mut_ptr(), 64, 0, core::ptr::null_mut(), core::ptr::null_mut())
            };
            if n < 0 || &buf[..n as usize] != want {
                return Err(68);
            }
        }
        // SAFETY: our socket.
        if unsafe { closesocket(udp) } != 0 {
            return Err(fail(69));
        }
        third.ask(b"bye", 20, 63)?;
        third.closed(64)
    };
    match run() {
        Ok(()) => {
            out(b"handover_net_test: self client ok\r\n");
            0
        }
        Err(n) => n,
    }
}

fn run() -> Result<(), u32> {
    // SAFETY: documented Winsock calls with valid arguments.
    unsafe {
        let mut data: WsaData = core::mem::zeroed();
        if WSAStartup(0x0202, &mut data) != 0 {
            return Err(1);
        }
        let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if listener == INVALID_SOCKET {
            return Err(fail(2));
        }
        let any =
            SockaddrIn { family: AF_INET as u16, port: PORT.to_be(), addr: [0; 4], zero: [0; 8] };
        if bind(listener, &any, size_of::<SockaddrIn>() as i32) != 0 {
            return Err(fail(3));
        }
        if listen(listener, 5) != 0 {
            return Err(fail(4));
        }
        let udp = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
        if udp == INVALID_SOCKET || bind(udp, &any, size_of::<SockaddrIn>() as i32) != 0 {
            return Err(fail(16));
        }
        out(b"handover_net_test: listening on port 7\r\n");
        let client = if wants_self() {
            let t = CreateThread(
                core::ptr::null_mut(),
                0,
                self_client,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            );
            if t.is_null() {
                return Err(9);
            }
            Some(t)
        } else {
            None
        };
        let mut count = 0u64;
        for i in 1..=CONNECTIONS {
            let conn = accept(listener, core::ptr::null_mut(), core::ptr::null_mut());
            if conn == INVALID_SOCKET {
                return Err(fail(5));
            }
            let lines = serve(conn, udp, &mut count)?;
            if closesocket(conn) != 0 {
                return Err(fail(6));
            }
            let (mut a, mut b, mut d) = ([0u8; 20], [0u8; 20], [0u8; 20]);
            out_line(&[
                b"handover_net_test: connection ",
                decimal(u64::from(i), &mut a),
                b": ",
                decimal(lines, &mut b),
                b" line(s), ",
                decimal(count, &mut d),
                b" in all\r\n",
            ]);
        }
        if closesocket(listener) != 0 || closesocket(udp) != 0 {
            return Err(fail(7));
        }
        if let Some(t) = client {
            let mut code = 1;
            if WaitForSingleObject(t, u32::MAX) != 0 || GetExitCodeThread(t, &mut code) == 0 {
                return Err(13);
            }
            if code != 0 {
                return Err(code);
            }
        }
        if WSACleanup() != 0 {
            return Err(fail(8));
        }
    }
    Ok(())
}

/// Whether this runs on gala: no ntdll is loaded, or the loaded one is
/// gala's (it exports GalaConsoleCreate), as `ntrt::on_gala` decides.
/// Elsewhere gala's own services are not called: their numbers would reach
/// Windows' win32k table and the first call faults.
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaConsoleCreate".as_ptr().cast()).is_null()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"handover_net_test: ok\r\n");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

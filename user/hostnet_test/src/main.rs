//! A page from the host: connects to 10.0.2.100 port 80 (QEMU's user
//! network forwards that address to a program on the host for each
//! connection: xtask's `serve-page`), sends `GET /`, reads the response
//! until the server closes and checks it is a 200 whose body is
//! `HOST_PAGE`. Prints `hostnet_test: ok` or exits with the number of the
//! failing step (1 WSAStartup, 2 socket, 3 connect, 4 send, 5 recv, 6 the
//! status line, 7 the body).

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type Socket = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn send(s: Socket, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: Socket, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn closesocket(s: Socket) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // Stack probe for the 4 KiB buffer (ntdll exports it, as on Windows).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

/// The host's page (xtask's HOST_PAGE).
const HOST_PAGE: &[u8] = b"<html><body><h1>hello from the host</h1></body></html>\n";

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
        );
    }
}

fn fail(step: u32) -> ! {
    out(b"hostnet_test: failed at step ");
    out(&[b'0' + step as u8, b'\n']);
    // SAFETY: ends the process.
    unsafe { ExitProcess(step) }
}

/// What was received, on a failure: its length and first 60 bytes
/// (control bytes as dots).
fn show(response: &[u8]) {
    let mut line = [0u8; 120];
    let mut n = 0;
    for &b in b"hostnet_test: got " {
        line[n] = b;
        n += 1;
    }
    let mut len = response.len();
    let mut digits = [0u8; 10];
    let mut d = 0;
    loop {
        digits[d] = b'0' + (len % 10) as u8;
        d += 1;
        len /= 10;
        if len == 0 {
            break;
        }
    }
    while d > 0 {
        d -= 1;
        line[n] = digits[d];
        n += 1;
    }
    for &b in b" bytes: " {
        line[n] = b;
        n += 1;
    }
    for &b in response.iter().take(60) {
        line[n] = if (0x20..0x7f).contains(&b) { b } else { b'.' };
        n += 1;
    }
    line[n] = b'\n';
    out(&line[..=n]);
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut wsa = [0u8; 512];
    let mut response = [0u8; 4096];
    let mut got = 0usize;
    // SAFETY: plain Winsock calls with valid buffers.
    unsafe {
        if WSAStartup(0x0202, wsa.as_mut_ptr()) != 0 {
            fail(1);
        }
        let s = socket(2, 1, 6);
        if s == !0 {
            fail(2);
        }
        let to = SockaddrIn { family: 2, port: 80u16.to_be(), addr: [10, 0, 2, 100], zero: [0; 8] };
        if connect(s, &to, 16) != 0 {
            let _ = WSAGetLastError();
            fail(3);
        }
        let request = b"GET / HTTP/1.0\r\nHost: 10.0.2.100\r\n\r\n";
        if send(s, request.as_ptr(), request.len() as i32, 0) != request.len() as i32 {
            fail(4);
        }
        loop {
            let n = recv(s, response[got..].as_mut_ptr(), (response.len() - got) as i32, 0);
            if n < 0 {
                fail(5);
            }
            if n == 0 || got + n as usize == response.len() {
                got += n as usize;
                break;
            }
            got += n as usize;
        }
        closesocket(s);
    }
    let response = &response[..got];
    if !response.starts_with(b"HTTP/1.0 200") && !response.starts_with(b"HTTP/1.1 200") {
        show(response);
        fail(6);
    }
    match find(response, b"\r\n\r\n") {
        Some(i) if &response[i + 4..] == HOST_PAGE => {}
        _ => {
            show(response);
            fail(7)
        }
    }
    out(b"hostnet_test: ok\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

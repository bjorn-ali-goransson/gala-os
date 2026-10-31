//! gala's mswsock.dll: Microsoft's Winsock extensions. AcceptEx and
//! GetAcceptExSockaddrs are export forwarders to gala's ws2_32 (build.rs),
//! where the AFD code is; programs such as Go import them from here.

#![no_std]

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSASetLastError(e: i32);
}

/// TransmitFile: not supported (WSAEOPNOTSUPP, recorded); callers such as
/// the JDK fall back to reading the file and sending it.
#[unsafe(no_mangle)]
pub extern "system" fn TransmitFile(
    _socket: usize,
    _file: usize,
    _bytes: u32,
    _per_send: u32,
    _overlapped: usize,
    _buffers: usize,
    _flags: u32,
) -> i32 {
    let name = b"mswsock.dll!TransmitFile";
    // SAFETY: plain calls with a valid buffer.
    unsafe {
        GalaRecordMissing(name.as_ptr(), name.len(), 2, 0);
        WSASetLastError(10045); // WSAEOPNOTSUPP
    }
    0
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

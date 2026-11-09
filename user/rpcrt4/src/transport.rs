//! The transport: named pipes in message mode, one PDU per message.
//! - ncacn_np's endpoint `\pipe\<name>` is the pipe `\\.\pipe\<name>`
//!   (local machine only);
//! - ncalrpc's endpoint `<name>` is the pipe `\\.\pipe\LRPC-<name>`
//!   (Windows carries ncalrpc over ALPC ports, a mechanism gala does not
//!   have; the pipe is private to the runtime: both ends are gala's).
//!
//! A server keeps a listening instance at all times: the listener creates
//! the next instance before it waits on the current one, so a client
//! never finds the name without one (gala has no WaitNamedPipe).

use alloc::vec::Vec;

use crate::binding::{
    Protseq, RPC_S_INVALID_ENDPOINT_FORMAT, RPC_S_OUT_OF_MEMORY, RPC_S_SERVER_TOO_BUSY,
    RPC_S_SERVER_UNAVAILABLE,
};
use crate::sys::{self, Handle, INVALID_HANDLE};

const PIPE_ACCESS_DUPLEX: u32 = 3;
const PIPE_TYPE_MESSAGE: u32 = 4;
const PIPE_READMODE_MESSAGE: u32 = 2;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const QUOTA: u32 = 0x1_0000;
const GENERIC_READ_WRITE: u32 = 0xc000_0000;
const OPEN_EXISTING: u32 = 3;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_PIPE_CONNECTED: u32 = 535;

/// The pipe path (NUL-terminated) of `endpoint` over `protseq`, or
/// RPC_S_INVALID_ENDPOINT_FORMAT.
pub fn pipe_path(protseq: Protseq, endpoint: &[u16]) -> Result<Vec<u16>, u32> {
    let mut path = Vec::new();
    path.try_reserve_exact(16 + endpoint.len()).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
    path.extend("\\\\.".encode_utf16());
    match protseq {
        Protseq::Np => {
            let prefix = "\\pipe\\";
            let ok = endpoint.len() > prefix.len()
                && sys::eq_ignore_case(&endpoint[..prefix.len()], prefix);
            if !ok {
                return Err(RPC_S_INVALID_ENDPOINT_FORMAT);
            }
            path.extend_from_slice(endpoint);
        }
        Protseq::Lrpc => {
            if endpoint.is_empty() || endpoint.contains(&u16::from(b'\\')) {
                return Err(RPC_S_INVALID_ENDPOINT_FORMAT);
            }
            path.extend("\\pipe\\LRPC-".encode_utf16());
            path.extend_from_slice(endpoint);
        }
    }
    path.push(0);
    Ok(path)
}

/// A new listening instance of the pipe `path`; `first`: it must be the
/// name's first (FILE_FLAG_FIRST_PIPE_INSTANCE: an endpoint another
/// server holds fails).
pub fn create_instance(path: &[u16], first: bool) -> Option<Handle> {
    let flag = if first { 0x0008_0000 } else { 0 };
    // SAFETY: a NUL-terminated name.
    let h = unsafe {
        sys::CreateNamedPipeW(
            path.as_ptr(),
            PIPE_ACCESS_DUPLEX | flag,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE,
            PIPE_UNLIMITED_INSTANCES,
            QUOTA,
            QUOTA,
            0,
            0,
        )
    };
    (h != INVALID_HANDLE && h != 0).then_some(h)
}

/// Waits for a client on `pipe`; false if it failed.
pub fn accept(pipe: Handle) -> bool {
    // SAFETY: our pipe; a synchronous wait.
    unsafe { sys::ConnectNamedPipe(pipe, 0) != 0 || sys::GetLastError() == ERROR_PIPE_CONNECTED }
}

/// Connects to the server at `path`.
pub fn connect(path: &[u16]) -> Result<Handle, u32> {
    // SAFETY: a NUL-terminated name.
    let h =
        unsafe { sys::CreateFileW(path.as_ptr(), GENERIC_READ_WRITE, 0, 0, OPEN_EXISTING, 0, 0) };
    if h == INVALID_HANDLE {
        // SAFETY: plain call.
        return Err(match unsafe { sys::GetLastError() } {
            ERROR_PIPE_BUSY => RPC_S_SERVER_TOO_BUSY,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => RPC_S_SERVER_UNAVAILABLE,
            _ => RPC_S_SERVER_UNAVAILABLE,
        });
    }
    let mode = PIPE_READMODE_MESSAGE;
    // SAFETY: our pipe.
    if unsafe { sys::SetNamedPipeHandleState(h, &mode, core::ptr::null(), core::ptr::null()) } == 0
    {
        // SAFETY: our handle.
        unsafe { sys::CloseHandle(h) };
        return Err(RPC_S_SERVER_UNAVAILABLE);
    }
    Ok(h)
}

/// Sends one PDU.
pub fn send(pipe: Handle, pdu: &[u8]) -> Result<(), u32> {
    let mut written = 0;
    // SAFETY: our buffer.
    let ok = unsafe { sys::WriteFile(pipe, pdu.as_ptr(), pdu.len() as u32, &mut written, 0) };
    if ok == 0 || written as usize != pdu.len() {
        return Err(crate::binding::RPC_S_CALL_FAILED);
    }
    Ok(())
}

/// Receives one PDU into `buf` (resized to it); Err when the other end is
/// gone or the message is larger than any PDU.
pub fn receive(pipe: Handle, buf: &mut Vec<u8>) -> Result<(), u32> {
    if buf.len() < crate::pdu::MAX_FRAG {
        buf.try_reserve(crate::pdu::MAX_FRAG - buf.len()).map_err(|_| RPC_S_OUT_OF_MEMORY)?;
        buf.resize(crate::pdu::MAX_FRAG, 0);
    }
    let mut read = 0;
    // SAFETY: our buffer of its length.
    let ok = unsafe { sys::ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut read, 0) };
    if ok == 0 {
        // SAFETY: plain call.
        let e = unsafe { sys::GetLastError() };
        if e == ERROR_MORE_DATA {
            sys::unsupported(b"rpcrt4.dll!receive(fragment larger than MAX_FRAG)", read.into());
        }
        return Err(crate::binding::RPC_S_CALL_FAILED);
    }
    buf.truncate(read as usize);
    Ok(())
}

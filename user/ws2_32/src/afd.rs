//! AFD's request buffers as this build lays them out: the caller's own
//! pointer width (afd_abi; the kernel reads a WOW64 process's as x86
//! ones). Each builder returns the largest (x64) size; send its first
//! `Layout`-given bytes.

pub use afd_abi::Layout;

/// This build's layouts.
pub const L: Layout = Layout::NATIVE;

/// A WSABUF {ULONG len; CHAR *buf}.
pub fn wsabuf(len: u32, buf: usize) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&len.to_le_bytes());
    L.put_ptr(&mut b, L.wsabuf_buf(), buf as u64);
    b
}

/// AFD_SEND_INFO / AFD_RECV_INFO: the buffers, their count, the TDI
/// (message) flags; `L.info_size()` bytes.
pub fn info(buffers: usize, count: u32, tdi_flags: u32) -> [u8; 24] {
    let mut b = [0u8; 24];
    L.put_ptr(&mut b, 0, buffers as u64);
    b[L.info_count()..][..4].copy_from_slice(&count.to_le_bytes());
    b[L.info_tdi_flags()..][..4].copy_from_slice(&tdi_flags.to_le_bytes());
    b
}

/// AFD_RECV_INFO_UDP: `info`'s fields, then where the sender's address
/// and its length go; `L.recv_dgram_size()` bytes.
pub fn recv_dgram(buffers: usize, count: u32, tdi_flags: u32, from: usize, len: usize) -> [u8; 40] {
    let mut b = [0u8; 40];
    b[..24].copy_from_slice(&info(buffers, count, tdi_flags));
    L.put_ptr(&mut b, L.recv_dgram_address(), from as u64);
    L.put_ptr(&mut b, L.recv_dgram_address_len(), len as u64);
    b
}

/// AFD_SEND_INFO_UDP: the buffers and the destination's TRANSPORT_ADDRESS
/// (`address`, `len` bytes); `L.send_dgram_size()` bytes.
pub fn send_dgram(buffers: usize, count: u32, address: usize, len: u32) -> [u8; 104] {
    let mut b = [0u8; 104];
    L.put_ptr(&mut b, 0, buffers as u64);
    b[L.info_count()..][..4].copy_from_slice(&count.to_le_bytes());
    b[L.send_dgram_address_len()..][..4].copy_from_slice(&len.to_le_bytes());
    L.put_ptr(&mut b, L.send_dgram_address(), address as u64);
    b
}

/// AFD_POLL_INFO entry `i`'s offset (after the 16-byte header).
pub fn poll_entry(i: usize) -> usize {
    Layout::POLL_HEADER + L.poll_entry() * i
}

/// Sets poll entry `i`: the socket and the events wanted.
pub fn put_poll(b: &mut [u8], i: usize, socket: usize, events: u32) {
    let at = poll_entry(i);
    L.put_ptr(b, at, socket as u64);
    b[at + L.poll_events()..][..4].copy_from_slice(&events.to_le_bytes());
}

/// Poll entry `i`: (socket, events).
pub fn get_poll(b: &[u8], i: usize) -> (usize, u32) {
    let at = poll_entry(i);
    let e = at + L.poll_events();
    (L.get_ptr(b, at) as usize, u32::from_le_bytes([b[e], b[e + 1], b[e + 2], b[e + 3]]))
}

/// The input of a poll of `n` entries.
pub fn poll_len(n: usize) -> usize {
    poll_entry(n)
}

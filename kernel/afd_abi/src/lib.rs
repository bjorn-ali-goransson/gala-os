//! AFD's request structures (the IOCTL_AFD_* inputs gala's ws2_32 builds
//! and the kernel's AFD reads), for both pointer sizes. Their pointers and
//! handles are the caller's width: a 32-bit (WOW64) process's ws2_32
//! writes the x86 layouts and the kernel reads them as such, as Windows'
//! AFD reads a 32-bit caller's (IoIs32bitProcess); wow64.dll passes AFD's
//! IOCTLs through unconverted. Each layout is computed from the pointer
//! size with the C rules (a field aligned to its size), so one definition
//! serves both sides and both widths; the field order is the structures'
//! as ReactOS's `drivers/network/afd/include/afd/shared.h` documents them
//! (ARCHITECTURE "AFD"), except where noted as gala's own.

#![no_std]

/// The layouts for one pointer size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The pointer (and handle) size: 8 or 4.
    pub ptr: usize,
}

const fn align(n: usize, to: usize) -> usize {
    n.div_ceil(to) * to
}

impl Layout {
    pub const X64: Layout = Layout { ptr: 8 };
    pub const X86: Layout = Layout { ptr: 4 };
    /// This build's own.
    pub const NATIVE: Layout = Layout { ptr: core::mem::size_of::<usize>() };

    // ---- WSABUF {ULONG len; CHAR *buf} ----

    pub const fn wsabuf_buf(self) -> usize {
        self.ptr
    }
    pub const fn wsabuf_size(self) -> usize {
        2 * self.ptr
    }

    // ---- AFD_SEND_INFO / AFD_RECV_INFO {LPWSABUF buffers; ULONG count;
    // ULONG afd_flags; ULONG tdi_flags} ----

    pub const fn info_count(self) -> usize {
        self.ptr
    }
    pub const fn info_afd_flags(self) -> usize {
        self.ptr + 4
    }
    pub const fn info_tdi_flags(self) -> usize {
        self.ptr + 8
    }
    pub const fn info_size(self) -> usize {
        align(self.ptr + 12, self.ptr)
    }

    // ---- AFD_RECV_INFO_UDP: AFD_RECV_INFO's fields, then PVOID address;
    // PINT address_length ----

    pub const fn recv_dgram_address(self) -> usize {
        align(self.ptr + 12, self.ptr)
    }
    pub const fn recv_dgram_address_len(self) -> usize {
        self.recv_dgram_address() + self.ptr
    }
    pub const fn recv_dgram_size(self) -> usize {
        self.recv_dgram_address() + 2 * self.ptr
    }

    // ---- AFD_SEND_INFO_UDP {LPWSABUF buffers; ULONG count; ULONG
    // afd_flags; TDI_REQUEST_SEND_DATAGRAM (TDI_REQUEST {HANDLE; PVOID;
    // PVOID; TDI_STATUS} and a PTDI_CONNECTION_INFORMATION);
    // TDI_CONNECTION_INFORMATION {LONG UserDataLength; PVOID UserData;
    // LONG OptionsLength; PVOID Options; LONG RemoteAddressLength; PVOID
    // RemoteAddress}} ----

    const fn tdi_request(self) -> usize {
        align(self.ptr + 8, self.ptr)
    }
    const fn tdi_request_send_datagram_size(self) -> usize {
        align(3 * self.ptr + 4, self.ptr) + self.ptr
    }
    const fn connection_information(self) -> usize {
        self.tdi_request() + self.tdi_request_send_datagram_size()
    }
    /// TDI_CONNECTION_INFORMATION.RemoteAddressLength.
    pub const fn send_dgram_address_len(self) -> usize {
        self.connection_information() + 4 * self.ptr
    }
    /// TDI_CONNECTION_INFORMATION.RemoteAddress: the TRANSPORT_ADDRESS
    /// (AFD copies it during the call).
    pub const fn send_dgram_address(self) -> usize {
        self.connection_information() + 5 * self.ptr
    }
    pub const fn send_dgram_size(self) -> usize {
        self.send_dgram_address() + self.ptr
    }

    // ---- AFD_POLL_INFO {LARGE_INTEGER timeout; ULONG count; ULONG
    // exclusive; AFD_POLL_HANDLE_INFO handles[] {HANDLE; ULONG events;
    // NTSTATUS status}} ----

    pub const POLL_HEADER: usize = 16;
    pub const fn poll_events(self) -> usize {
        self.ptr
    }
    pub const fn poll_status(self) -> usize {
        self.ptr + 4
    }
    pub const fn poll_entry(self) -> usize {
        align(self.ptr + 8, self.ptr)
    }

    // ---- AFD_ACCEPT_DATA {ULONG use_san; ULONG sequence; HANDLE
    // accept} ----

    pub const ACCEPT_SEQUENCE: usize = 4;
    pub const ACCEPT_HANDLE: usize = 8;
    pub const fn accept_size(self) -> usize {
        align(8 + self.ptr, self.ptr)
    }

    // ---- AFD_SUPER_ACCEPT (gala's AcceptEx request, after ReactOS's
    // AFD_ACCEPT_DATA): {BOOLEAN use_san; BOOLEAN fix_alignment; HANDLE
    // accept; ULONG receive_length; ULONG local_length; ULONG
    // remote_length} ----

    pub const fn super_accept_handle(self) -> usize {
        self.ptr
    }
    pub const fn super_accept_receive(self) -> usize {
        2 * self.ptr
    }
    pub const fn super_accept_local(self) -> usize {
        2 * self.ptr + 4
    }
    pub const fn super_accept_remote(self) -> usize {
        2 * self.ptr + 8
    }
    pub const fn super_accept_size(self) -> usize {
        2 * self.ptr + 12
    }

    // ---- afd_event_select_params {HANDLE event; int mask} ----

    pub const fn event_select_mask(self) -> usize {
        self.ptr
    }
    pub const fn event_select_size(self) -> usize {
        align(self.ptr + 4, self.ptr)
    }

    /// A pointer of this width at `at` (zero-extended).
    pub fn get_ptr(self, b: &[u8], at: usize) -> u64 {
        let mut v = [0u8; 8];
        v[..self.ptr].copy_from_slice(&b[at..at + self.ptr]);
        u64::from_le_bytes(v)
    }

    /// A handle of this width at `at` (sign-extended, as a 32-bit
    /// process's pseudo handles are).
    pub fn get_handle(self, b: &[u8], at: usize) -> u64 {
        if self.ptr == 4 {
            i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as i64 as u64
        } else {
            self.get_ptr(b, at)
        }
    }

    /// Writes a pointer or handle of this width at `at`.
    pub fn put_ptr(self, b: &mut [u8], at: usize, v: u64) {
        b[at..at + self.ptr].copy_from_slice(&v.to_le_bytes()[..self.ptr]);
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn x64_layouts() {
        // The offsets gala's x64 ws2_32 and the kernel have always used.
        let l = Layout::X64;
        assert_eq!((l.wsabuf_buf(), l.wsabuf_size()), (8, 16));
        assert_eq!((l.info_count(), l.info_tdi_flags(), l.info_size()), (8, 16, 24));
        assert_eq!((l.recv_dgram_address(), l.recv_dgram_address_len()), (24, 32));
        assert_eq!(l.recv_dgram_size(), 40);
        assert_eq!((l.send_dgram_address_len(), l.send_dgram_address()), (88, 96));
        assert_eq!(l.send_dgram_size(), 104);
        assert_eq!((l.poll_events(), l.poll_entry()), (8, 16));
        assert_eq!(l.accept_size(), 16);
        assert_eq!((l.super_accept_handle(), l.super_accept_receive()), (8, 16));
        assert_eq!(l.super_accept_size(), 28);
        assert_eq!((l.event_select_mask(), l.event_select_size()), (8, 16));
    }

    #[test]
    fn x86_layouts() {
        let l = Layout::X86;
        assert_eq!((l.wsabuf_buf(), l.wsabuf_size()), (4, 8));
        assert_eq!((l.info_count(), l.info_tdi_flags(), l.info_size()), (4, 12, 16));
        assert_eq!((l.recv_dgram_address(), l.recv_dgram_address_len()), (16, 20));
        assert_eq!(l.recv_dgram_size(), 24);
        // TDI_REQUEST_SEND_DATAGRAM at 12 (20 bytes), the connection
        // information at 32: RemoteAddressLength at +16, RemoteAddress +20.
        assert_eq!((l.send_dgram_address_len(), l.send_dgram_address()), (48, 52));
        assert_eq!(l.send_dgram_size(), 56);
        assert_eq!((l.poll_events(), l.poll_entry()), (4, 12));
        assert_eq!(l.accept_size(), 12);
        assert_eq!((l.super_accept_handle(), l.super_accept_receive()), (4, 8));
        assert_eq!(l.super_accept_size(), 20);
        assert_eq!((l.event_select_mask(), l.event_select_size()), (4, 8));
    }

    #[test]
    fn handles_and_pointers() {
        let mut b = [0u8; 8];
        Layout::X86.put_ptr(&mut b, 0, 0xffff_fffe);
        assert_eq!(Layout::X86.get_handle(&b, 0), (-2i64) as u64);
        assert_eq!(Layout::X86.get_ptr(&b, 0), 0xffff_fffe);
        Layout::X64.put_ptr(&mut b, 0, 0x1234_5678_9abc);
        assert_eq!(Layout::X64.get_ptr(&b, 0), 0x1234_5678_9abc);
    }
}

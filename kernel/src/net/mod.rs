//! Networking: a virtio-net card and the smoltcp TCP/IP stack (0BSD, see
//! THIRD_PARTY.md), with the static address QEMU's user-mode network
//! expects (10.0.2.15/24, gateway 10.0.2.2).
//!
//! N1a has one user: the `net_echo` boot flag, a TCP echo server on port 7
//! that serves one connection on the boot thread. N1b moves the stack into
//! a polling kernel thread behind the AFD device that Winsock uses.

pub mod virtio_net;

use crate::{sched, time};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::tcp;
use smoltcp::wire::{EthernetAddress, IpCidr, Ipv4Address};
use virtio_net::VirtioNet;

pub const ADDRESS: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub const PREFIX: u8 = 24;
pub const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

const ECHO_PORT: u16 = 7;
const SOCKET_BUFFER: usize = 16 * 1024;
const ECHO_TIMEOUT_NS: u64 = 60_000_000_000;

fn now() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros((time::monotonic_ns() / 1000) as i64)
}

fn zeroed(len: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

/// Starts the card and an interface with the static address.
fn bring_up() -> Option<(VirtioNet, Interface)> {
    let mut dev = match VirtioNet::probe() {
        Ok(dev) => dev,
        Err(e) => {
            warn!("net: no usable virtio-net card ({e:?})");
            return None;
        }
    };
    let m = dev.mac;
    info!(
        "net: virtio-net at {}, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        dev.at, m[0], m[1], m[2], m[3], m[4], m[5]
    );
    let mut config = Config::new(EthernetAddress(dev.mac).into());
    config.random_seed = time::monotonic_ns() ^ 0x9e37_79b9_7f4a_7c15;
    let mut iface = Interface::new(config, &mut dev, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(ADDRESS.into(), PREFIX));
    });
    let _ = iface.routes_mut().add_default_ipv4_route(GATEWAY);
    info!("net: link up, {ADDRESS}/{PREFIX}, gateway {GATEWAY}");
    Some((dev, iface))
}

/// `net_echo`: serves one TCP connection on port 7, echoing everything,
/// until the peer closes (or a minute passes).
pub fn echo_test() {
    let Some((mut dev, mut iface)) = bring_up() else { return };
    let (Some(mut rx), Some(mut tx)) = (zeroed(SOCKET_BUFFER), zeroed(SOCKET_BUFFER)) else {
        warn!("net: out of memory for socket buffers");
        return;
    };
    let mut storage = [SocketStorage::EMPTY; 1];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let socket =
        tcp::Socket::new(tcp::SocketBuffer::new(&mut rx[..]), tcp::SocketBuffer::new(&mut tx[..]));
    let handle = sockets.add(socket);
    if sockets.get_mut::<tcp::Socket>(handle).listen(ECHO_PORT).is_err() {
        warn!("net: listen failed");
        return;
    }
    info!("net: echo listening on port {ECHO_PORT}");
    let start = time::monotonic_ns();
    let (mut echoed, mut connected) = (0usize, false);
    let mut buf = [0u8; 4096];
    loop {
        iface.poll(now(), &mut dev, &mut sockets);
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        connected |= socket.is_active();
        if socket.can_recv() {
            let room = socket.send_capacity() - socket.send_queue();
            let len = room.min(buf.len());
            let n = socket.recv_slice(&mut buf[..len]).unwrap_or(0);
            echoed += socket.send_slice(&buf[..n]).unwrap_or(0);
        }
        // The peer half-closed and everything is echoed: close our side.
        if connected && !socket.may_recv() && socket.may_send() && socket.send_queue() == 0 {
            socket.close();
        }
        if connected && socket.state() == tcp::State::Closed {
            break;
        }
        if time::monotonic_ns() - start > ECHO_TIMEOUT_NS || dev.failed() {
            warn!("net_echo: gave up (state {}, device failed: {})", socket.state(), dev.failed());
            break;
        }
        match iface.poll_delay(now(), &sockets) {
            Some(d) if d.total_millis() == 0 => sched::yield_now(),
            _ => sched::sleep_ms(1),
        }
    }
    info!("net_echo: echoed {echoed} bytes");
    info!(
        "net: {} frames received, {} sent, {} dropped (transmit queue full)",
        virtio_net::RX_FRAMES.load(Ordering::Relaxed),
        virtio_net::TX_FRAMES.load(Ordering::Relaxed),
        virtio_net::TX_FULL.load(Ordering::Relaxed),
    );
}

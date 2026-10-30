//! Networking: a virtio-net card, a loopback device and the smoltcp TCP/IP
//! stack (0BSD, see THIRD_PARTY.md), with the static address QEMU's
//! user-mode network expects (10.0.2.15/24, gateway 10.0.2.2). `tcp` is
//! the kernel's socket API; `nt::afd` builds Windows sockets on it.
//! Design: docs/ARCHITECTURE.md, "Networking".

pub mod loopback;
pub mod tcp;
pub mod virtio_net;

use crate::{sched, time};
use core::sync::atomic::Ordering;
use smoltcp::wire::Ipv4Address;
use tcp::{Addr, NetError};
use virtio_net::VirtioNet;

pub const ADDRESS: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub const PREFIX: u8 = 24;
pub const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

const ECHO_PORT: u16 = 7;
const ECHO_TIMEOUT_NS: u64 = 60_000_000_000;

/// Finds the card (if any) and starts the stack and the `net` thread.
pub fn init() {
    let card = match VirtioNet::probe() {
        Ok(dev) => {
            let m = dev.mac;
            info!(
                "net: virtio-net at {}, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                dev.at, m[0], m[1], m[2], m[3], m[4], m[5]
            );
            Some(dev)
        }
        Err(virtio_net::Error::NotFound) => None,
        Err(e) => {
            warn!("net: virtio-net card not usable ({e:?})");
            None
        }
    };
    let has_card = card.is_some();
    match tcp::start(card) {
        Ok(()) if has_card => info!("net: link up, {ADDRESS}/{PREFIX}, gateway {GATEWAY}"),
        Ok(()) => {}
        Err(e) => warn!("net: not started ({e:?})"),
    }
}

/// Retries `f` while it would block (polling: kernel callers have no
/// endpoint object to wait on), for at most the echo timeout.
fn blocking<R>(start: u64, mut f: impl FnMut() -> Result<R, NetError>) -> Result<R, NetError> {
    loop {
        match f() {
            Err(NetError::WouldBlock) if time::monotonic_ns() - start < ECHO_TIMEOUT_NS => {
                sched::sleep_ms(1)
            }
            other => return other,
        }
    }
}

/// `net_echo`: serves one TCP connection on port 7 through `net::tcp`,
/// echoing everything until the peer closes (or a minute passes).
pub fn echo_test() {
    let start = time::monotonic_ns();
    let result = (|| {
        let listener = tcp::bind(Addr { ip: [0; 4], port: ECHO_PORT }, 0)?;
        tcp::listen(listener, 1)?;
        info!("net: echo listening on port {ECHO_PORT}");
        let (sequence, _) = blocking(start, || tcp::pending(listener))?;
        let conn = tcp::accept(listener, sequence, 0)?;
        tcp::close(listener, false);
        let mut buf = [0u8; 4096];
        let mut echoed = 0usize;
        loop {
            let n = blocking(start, || tcp::recv(conn, &mut buf))?;
            if n == 0 {
                break;
            }
            let mut sent = 0;
            while sent < n {
                sent += blocking(start, || tcp::send(conn, &buf[sent..n]))?;
            }
            echoed += n;
        }
        tcp::close(conn, false);
        Ok::<usize, NetError>(echoed)
    })();
    match result {
        Ok(echoed) => info!("net_echo: echoed {echoed} bytes"),
        Err(e) => warn!("net_echo: failed ({e:?})"),
    }
    // Let the FIN go out before the VM ends.
    sched::sleep_ms(50);
}

/// Logs the network counters.
pub fn log_stats() {
    let (polls, lo_packets, lo_dropped, sockets) = tcp::stats();
    info!(
        "net: {} frames received, {} sent, {} dropped (transmit queue full); loopback {} packets, {} dropped; {} polls, {} sockets open",
        virtio_net::RX_FRAMES.load(Ordering::Relaxed),
        virtio_net::TX_FRAMES.load(Ordering::Relaxed),
        virtio_net::TX_FULL.load(Ordering::Relaxed),
        lo_packets,
        lo_dropped,
        polls,
        sockets,
    );
}

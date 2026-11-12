//! Networking: a virtio-net card, a loopback device and the smoltcp TCP/IP
//! stack (0BSD, see THIRD_PARTY.md), with the static address QEMU's
//! user-mode network expects (10.0.2.15/24, gateway 10.0.2.2). `tcp` is
//! the kernel's socket API; `nt::afd` builds Windows sockets on it.
//! Design: docs/ARCHITECTURE.md, "Networking".

pub mod loopback;
pub mod tcp;
pub mod udp;
pub mod virtio_net;

use crate::{sched, time};
use core::sync::atomic::Ordering;
use smoltcp::wire::Ipv4Address;
use tcp::{Addr, NetError};
use virtio_net::VirtioNet;

pub const ADDRESS: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub const PREFIX: u8 = 24;
pub const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

/// An IPv4 or IPv6 address (N3: ARCHITECTURE "Plan: IPv6"), the stack's
/// own type (smoltcp's `IpAddress` at its edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ip {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl Default for Ip {
    fn default() -> Self {
        Ip::V4([0; 4])
    }
}

impl From<[u8; 4]> for Ip {
    fn from(a: [u8; 4]) -> Self {
        Ip::V4(a)
    }
}

impl Ip {
    /// ::1.
    pub const LOOPBACK6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

    /// 0.0.0.0 or ::.
    pub fn is_unspecified(self) -> bool {
        match self {
            Ip::V4(a) => a == [0; 4],
            Ip::V6(a) => a == [0; 16],
        }
    }

    /// 127.0.0.0/8 or ::1.
    pub fn is_loopback(self) -> bool {
        match self {
            Ip::V4(a) => a[0] == 127,
            Ip::V6(a) => a == Self::LOOPBACK6,
        }
    }

    pub fn is_v6(self) -> bool {
        matches!(self, Ip::V6(_))
    }

    /// An IPv4 address as IPv4-mapped IPv6 (::ffff:a.b.c.d), what an
    /// IPv6 socket sees of an IPv4 peer; IPv6 addresses unchanged.
    pub fn mapped(self) -> Ip {
        match self {
            Ip::V4([a, b, c, d]) => Ip::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d]),
            v6 => v6,
        }
    }

    /// An IPv4-mapped IPv6 address as the IPv4 address it stands for
    /// (the stack routes and binds it as IPv4); others unchanged.
    pub fn unmapped(self) -> Ip {
        match self {
            Ip::V6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d]) => Ip::V4([a, b, c, d]),
            other => other,
        }
    }

    /// The unspecified address of this family.
    pub fn unspecified_like(self) -> Ip {
        match self {
            Ip::V4(_) => Ip::V4([0; 4]),
            Ip::V6(_) => Ip::V6([0; 16]),
        }
    }

    /// The loopback address of this family.
    pub fn loopback_like(self) -> Ip {
        match self {
            Ip::V4(_) => Ip::V4([127, 0, 0, 1]),
            Ip::V6(_) => Ip::V6(Self::LOOPBACK6),
        }
    }

    pub fn to_smoltcp(self) -> smoltcp::wire::IpAddress {
        use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};
        match self {
            Ip::V4(a) => IpAddress::Ipv4(Ipv4Address::from(a)),
            Ip::V6(a) => IpAddress::Ipv6(Ipv6Address::from(a)),
        }
    }

    pub fn from_smoltcp(a: smoltcp::wire::IpAddress) -> Ip {
        use smoltcp::wire::IpAddress;
        match a {
            IpAddress::Ipv4(a) => Ip::V4(a.octets()),
            IpAddress::Ipv6(a) => Ip::V6(a.octets()),
        }
    }

    /// The address a listen or bind endpoint takes: None for unspecified.
    pub fn listen_addr(self) -> Option<smoltcp::wire::IpAddress> {
        (!self.is_unspecified()).then(|| self.to_smoltcp())
    }
}

/// Which families a socket takes (N3 step 2): an IPv4 socket's, an
/// IPv6 socket's with IPV6_V6ONLY (Windows' default) or without it (dual
/// stack: IPv4 peers too). Ports are per family: two sockets conflict
/// only when their scopes overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    V4,
    V6,
    Dual,
}

impl Scope {
    /// An address's own family (a socket bound without a stated scope).
    pub fn of(ip: Ip) -> Scope {
        if ip.is_v6() { Scope::V6 } else { Scope::V4 }
    }

    pub fn overlaps(self, other: Scope) -> bool {
        self == other || self == Scope::Dual || other == Scope::Dual
    }
}

/// `a.b.c.d` or the IPv6 text form (RFC 5952: the longest run of zero
/// groups as `::`).
impl core::fmt::Display for Ip {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Ip::V4([a, b, c, d]) => write!(f, "{a}.{b}.{c}.{d}"),
            Ip::V6(a) => write!(f, "{}", smoltcp::wire::Ipv6Address::from(a)),
        }
    }
}

/// The machine has a network card (the task manager's network graph).
static CARD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether a network card was found.
pub fn has_card() -> bool {
    CARD.load(core::sync::atomic::Ordering::Relaxed)
}

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
    CARD.store(has_card, core::sync::atomic::Ordering::Relaxed);
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
        let listener = tcp::bind(Addr::v4([0; 4], ECHO_PORT), 0)?;
        tcp::listen(listener, 1, tcp::WhenFull::Refuse)?;
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

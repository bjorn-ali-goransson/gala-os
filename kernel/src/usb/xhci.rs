//! An xHCI USB host controller (the eXtensible Host Controller Interface
//! specification, revision 1.2; USB 2.0 chapter 9 for the requests; see
//! THIRD_PARTY.md): the devices on its root ports at boot, each set up
//! through its first HID interface (a keyboard in the boot protocol, a
//! mouse in the report protocol as its report descriptor describes it),
//! then read through that interface's interrupt IN endpoint.
//!
//! Polled: no interrupter is enabled; `Xhci::poll` reads the event ring.
//! One command ring, one event ring (one segment), a transfer ring for
//! each device's endpoint 0 and one for its interrupt endpoint, each a
//! page of 256 TRBs ending in a Link TRB back to the start. Control
//! transfers move at most `DATA_LEN` bytes, in to the host or none.
//! Limits: root ports only (no hubs), no hotplug (devices present when
//! the controller is started), the first HID interface of a device, the
//! default port speed IDs. Design: docs/ARCHITECTURE.md, "USB".
//!
//! The driver is generic over `hw::Platform` so that its logic runs on
//! the host against a model of a controller (`tests`).

use super::hid;
use crate::disk::hw::{self, MS, Mem, Platform, SECOND};

// Capability registers.
const CAPLENGTH: usize = 0x00;
const HCSPARAMS1: usize = 0x04;
const HCSPARAMS2: usize = 0x08;
const HCCPARAMS1: usize = 0x10;
const DBOFF: usize = 0x14;
const RTSOFF: usize = 0x18;

// Operational registers (from CAPLENGTH).
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const PAGESIZE: usize = 0x08;
const CRCR: usize = 0x18;
const DCBAAP: usize = 0x30;
const CONFIG: usize = 0x38;
const PORTSC: usize = 0x400;

const CMD_RS: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const STS_HCH: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;

const PORT_CCS: u32 = 1 << 0;
const PORT_PED: u32 = 1 << 1;
const PORT_PR: u32 = 1 << 4;
const PORT_PP: u32 = 1 << 9;
const PORT_PRC: u32 = 1 << 21;
/// The change bits (write 1 to clear).
const PORT_CHANGES: u32 = 0x7f << 17;
/// The bits a write must give back as read: power, indicator, wake
/// enables. The others are either write-1-to-clear (PED: writing it back
/// would disable the port) or act when written.
const PORT_PRESERVE: u32 = PORT_PP | 3 << 14 | 7 << 25;

// Interrupter 0 (runtime registers + 0x20).
const IR0: usize = 0x20;
const ERSTSZ: usize = 0x08;
const ERSTBA: usize = 0x10;
const ERDP: usize = 0x18;
const ERDP_EHB: u64 = 1 << 3;

// TRB types.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_EVALUATE_CONTEXT: u32 = 13;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_COMPLETION: u32 = 33;
const TRB_PORT_STATUS_CHANGE: u32 = 34;

const TRB_CYCLE: u32 = 1 << 0;
const TRB_TC: u32 = 1 << 1;
const TRB_ISP: u32 = 1 << 2;
const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6;
const TRB_DIR_IN: u32 = 1 << 16;

const CC_SUCCESS: u8 = 1;
const CC_SHORT_PACKET: u8 = 13;

const PAGE: usize = 4096;
/// TRBs in a ring (a page), the last a Link TRB.
const RING_TRBS: usize = PAGE / 16;
/// Devices this driver keeps (the slots it enables).
pub const MAX_DEVICES: usize = 8;
/// The most scratchpad buffers accepted (each a page).
const MAX_SCRATCHPADS: u32 = 256;

// A device's buffer (a page): the interrupt transfers' slices first,
// the data of control transfers in its second half.
const SLICE: usize = 64;
const INTERRUPT_TRBS: usize = 16;
const DATA_AT: usize = PAGE / 2;
pub const DATA_LEN: usize = PAGE / 2;

// Port speed IDs (the default Protocol Speed ID mapping).
const SPEED_FULL: u32 = 1;
const SPEED_LOW: u32 = 2;
const SPEED_HIGH: u32 = 3;

/// Why the controller, or one device on it, is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The controller kept a bit set (or clear) past its timeout.
    Timeout(&'static str),
    NoMemory,
    /// Memory above 4 GiB for a controller that cannot address it.
    Above4G,
    PageSize(u32),
    Scratchpads(u32),
    /// A command's completion code.
    Command(u32, u8),
    /// A transfer's completion code.
    Transfer(u8),
    /// No event for a command or transfer within a second.
    NoEvent(&'static str),
    /// A descriptor that does not say what it should.
    Descriptor(&'static str),
    /// No interface this driver reads.
    NotHid,
    NoSlot,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Timeout(what) => write!(f, "timed out waiting for {what}"),
            Error::NoMemory => f.write_str("out of memory"),
            Error::Above4G => f.write_str("memory above 4 GiB and the controller is 32-bit"),
            Error::PageSize(p) => write!(f, "no 4 KiB page size (PAGESIZE {p:#x})"),
            Error::Scratchpads(n) => write!(f, "{n} scratchpad buffers asked for"),
            Error::Command(t, cc) => write!(f, "command {t} failed (completion code {cc})"),
            Error::Transfer(cc) => write!(f, "transfer failed (completion code {cc})"),
            Error::NoEvent(what) => write!(f, "no completion for {what}"),
            Error::Descriptor(what) => write!(f, "bad descriptor: {what}"),
            Error::NotHid => f.write_str("no HID keyboard or mouse interface"),
            Error::NoSlot => f.write_str("no device slot left"),
        }
    }
}

/// A transfer or command ring: TRBs written by the driver, the last a
/// Link TRB back to the first that flips the cycle bit.
struct Ring<M> {
    mem: M,
    enq: usize,
    cycle: u32,
}

impl<M: Mem> Ring<M> {
    fn new(mut mem: M) -> Self {
        let link = (RING_TRBS - 1) * 16;
        let phys = mem.phys();
        mem.write32(link, phys as u32);
        mem.write32(link + 4, (phys >> 32) as u32);
        mem.write32(link + 8, 0);
        mem.write32(link + 12, TRB_LINK << 10 | TRB_TC);
        Self { mem, enq: 0, cycle: TRB_CYCLE }
    }

    /// The dequeue pointer the controller starts from (with the cycle).
    fn start(&self) -> u64 {
        self.mem.phys() | 1
    }

    /// Writes a TRB (its cycle bit last); its physical address.
    fn push(&mut self, trb: [u32; 4]) -> u64 {
        let at = self.enq * 16;
        self.mem.write32(at, trb[0]);
        self.mem.write32(at + 4, trb[1]);
        self.mem.write32(at + 8, trb[2]);
        self.mem.write32(at + 12, trb[3] & !TRB_CYCLE | self.cycle);
        let phys = self.mem.phys() + at as u64;
        self.enq += 1;
        if self.enq == RING_TRBS - 1 {
            let link = self.enq * 16;
            self.mem.write32(link + 12, TRB_LINK << 10 | TRB_TC | self.cycle);
            self.cycle ^= TRB_CYCLE;
            self.enq = 0;
        }
        phys
    }

    /// The TRB at `phys` if it is one of this ring's.
    fn trb(&self, phys: u64) -> Option<[u32; 4]> {
        let o = phys.checked_sub(self.mem.phys())? as usize;
        if o >= (RING_TRBS - 1) * 16 || !o.is_multiple_of(16) {
            return None;
        }
        Some([
            self.mem.read32(o),
            self.mem.read32(o + 4),
            self.mem.read32(o + 8),
            self.mem.read32(o + 12),
        ])
    }
}

fn trb_type(trb: &[u32; 4]) -> u32 {
    trb[3] >> 10 & 0x3f
}

fn completion(trb: &[u32; 4]) -> u8 {
    (trb[2] >> 24) as u8
}

fn param(trb: &[u32; 4]) -> u64 {
    u64::from(trb[0]) | u64::from(trb[1]) << 32
}

/// What a device is read as.
pub enum Kind {
    Keyboard(hid::Keyboard),
    Mouse(hid::MouseLayout),
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Keyboard(_) => "keyboard",
            Kind::Mouse(_) => "mouse",
        }
    }
}

/// A device set up on a slot.
struct Device<M> {
    slot: u8,
    port: u8,
    speed: u32,
    ep0: Ring<M>,
    /// The device context the controller writes (its DCBAA entry).
    _output: M,
    input: M,
    buf: M,
    mps0: u32,
    /// The interrupt endpoint: its context index, its ring, packet size.
    dci: u8,
    intr: Option<Ring<M>>,
    intr_mps: usize,
    kind: Option<Kind>,
    /// A transfer on its interrupt endpoint failed; it is not read.
    dead: bool,
    vendor: u16,
    product: u16,
}

/// A HID interface of a configuration descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidInterface {
    pub configuration: u8,
    pub number: u8,
    pub subclass: u8,
    pub protocol: u8,
    /// The interrupt IN endpoint's address, packet size and interval.
    pub endpoint: u8,
    pub max_packet: u16,
    pub interval: u8,
    /// The report descriptor's length (the HID descriptor's).
    pub report_len: u16,
}

/// The first HID interface (alternate setting 0) with an interrupt IN
/// endpoint in a configuration descriptor and those that follow it.
pub fn find_hid(desc: &[u8]) -> Option<HidInterface> {
    let mut configuration = None;
    let mut current: Option<HidInterface> = None;
    let mut at = 0;
    while at + 2 <= desc.len() {
        let len = desc[at] as usize;
        if len < 2 || at + len > desc.len() {
            break;
        }
        let d = &desc[at..at + len];
        match d[1] {
            2 if len >= 9 => configuration = Some(d[5]),
            4 if len >= 9 => {
                if let Some(i) = current
                    && i.endpoint != 0
                    && i.report_len != 0
                {
                    return Some(i);
                }
                current = (d[5] == 3 && d[3] == 0).then_some(HidInterface {
                    configuration: configuration?,
                    number: d[2],
                    subclass: d[6],
                    protocol: d[7],
                    endpoint: 0,
                    max_packet: 0,
                    interval: 0,
                    report_len: 0,
                });
            }
            0x21 if len >= 9 => {
                if let Some(i) = current.as_mut() {
                    // The class descriptors that follow: the report
                    // descriptor's type and length.
                    let n = d[5] as usize;
                    for k in 0..n {
                        let o = 6 + 3 * k;
                        if o + 3 <= len && d[o] == 0x22 {
                            i.report_len = u16::from_le_bytes([d[o + 1], d[o + 2]]);
                        }
                    }
                }
            }
            5 if len >= 7 => {
                if let Some(i) = current.as_mut()
                    && i.endpoint == 0
                    && d[2] & 0x80 != 0
                    && d[3] & 3 == 3
                {
                    i.endpoint = d[2];
                    i.max_packet = u16::from_le_bytes([d[4], d[5]]) & 0x7ff;
                    i.interval = d[6];
                }
            }
            _ => {}
        }
        at += len;
    }
    current.filter(|i| i.endpoint != 0 && i.report_len != 0)
}

/// The Interval field of an interrupt endpoint's context: 2^Interval
/// 125-microsecond frames.
fn interval(speed: u32, b_interval: u8) -> u32 {
    if speed == SPEED_FULL || speed == SPEED_LOW {
        // bInterval in milliseconds (1 to 255): floor(log2(ms * 8)).
        let frames = u32::from(b_interval.max(1)) * 8;
        (31 - frames.leading_zeros()).clamp(3, 10)
    } else {
        // 2^(bInterval-1) frames.
        u32::from(b_interval.clamp(1, 16)) - 1
    }
}

/// The controller with its rings; the devices are `Xhci`'s.
struct Hc<P: Platform> {
    p: P,
    op: usize,
    rt: usize,
    db: usize,
    ac64: bool,
    csz: usize,
    ports: u8,
    slots: u8,
    dcbaa: P::Mem,
    _scratch: Option<(P::Mem, P::Mem)>,
    cmd: Ring<P::Mem>,
    events: P::Mem,
    deq: usize,
    ccs: u32,
    /// Events no one waited for (the log says what they were).
    strays: u64,
}

impl<P: Platform> Hc<P> {
    fn alloc(&mut self, bytes: usize) -> Result<P::Mem, Error> {
        let m = self.p.alloc(bytes).ok_or(Error::NoMemory)?;
        if !self.ac64 && m.phys() + m.len() as u64 > 1 << 32 {
            return Err(Error::Above4G);
        }
        Ok(m)
    }

    fn portsc(&self, port: u8) -> usize {
        self.op + PORTSC + 0x10 * (usize::from(port) - 1)
    }

    fn ring(&mut self, slot: u8, target: u32) {
        self.p.write32(self.db + 4 * usize::from(slot), target);
    }

    /// The next event, if the controller wrote one (the dequeue pointer
    /// moves past it).
    fn next_event(&mut self) -> Option<[u32; 4]> {
        let at = self.deq * 16;
        let d3 = self.events.read32(at + 12);
        if d3 & TRB_CYCLE != self.ccs {
            return None;
        }
        let trb =
            [self.events.read32(at), self.events.read32(at + 4), self.events.read32(at + 8), d3];
        self.deq += 1;
        if self.deq == RING_TRBS {
            self.deq = 0;
            self.ccs ^= TRB_CYCLE;
        }
        let erdp = (self.events.phys() + (self.deq * 16) as u64) | ERDP_EHB;
        self.p.write64(self.rt + IR0 + ERDP, erdp);
        Some(trb)
    }

    /// Waits a second for the event `want` picks; the others are logged
    /// and dropped (none is expected while a device is set up).
    fn wait(
        &mut self,
        what: &'static str,
        mut want: impl FnMut(&[u32; 4]) -> bool,
    ) -> Result<[u32; 4], Error> {
        let start = self.p.now_ns();
        loop {
            if let Some(ev) = self.next_event() {
                if want(&ev) {
                    return Ok(ev);
                }
                self.stray(&ev);
                continue;
            }
            if self.p.now_ns().saturating_sub(start) > SECOND {
                return Err(Error::NoEvent(what));
            }
            core::hint::spin_loop();
        }
    }

    fn stray(&mut self, ev: &[u32; 4]) {
        match trb_type(ev) {
            TRB_PORT_STATUS_CHANGE => {
                let port = ev[0] >> 24;
                log!(
                    info,
                    "usb: xhci: port {port} changed (devices attached later are not set up)"
                );
            }
            t => {
                self.strays += 1;
                log!(
                    warn,
                    "usb: xhci: unexpected event type {t} (completion code {}, slot {}, parameter {:#x})",
                    completion(ev),
                    ev[3] >> 24,
                    param(ev)
                );
            }
        }
    }

    /// Runs a command; its completion event.
    fn command(&mut self, trb: [u32; 4]) -> Result<[u32; 4], Error> {
        let t = trb_type(&trb);
        let at = self.cmd.push(trb);
        self.ring(0, 0);
        let ev =
            self.wait("a command", |ev| trb_type(ev) == TRB_COMMAND_COMPLETION && param(ev) == at)?;
        match completion(&ev) {
            CC_SUCCESS => Ok(ev),
            cc => Err(Error::Command(t, cc)),
        }
    }

    /// A control transfer on a device's endpoint 0, with `len` bytes (at
    /// most `DATA_LEN`) in to the host at `buf[DATA_AT..]`, or none.
    fn control(
        &mut self,
        dev: &mut Device<P::Mem>,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        len: u16,
    ) -> Result<(), Error> {
        let len = len.min(DATA_LEN as u16);
        dev.buf.zero(DATA_AT, DATA_LEN);
        let transfer_type = if len == 0 { 0 } else { 3 };
        dev.ep0.push([
            u32::from(request_type) | u32::from(request) << 8 | u32::from(value) << 16,
            u32::from(index) | u32::from(len) << 16,
            8,
            TRB_SETUP << 10 | TRB_IDT | transfer_type << 16,
        ]);
        if len != 0 {
            let data = dev.buf.phys() + DATA_AT as u64;
            dev.ep0.push([
                data as u32,
                (data >> 32) as u32,
                u32::from(len),
                TRB_DATA << 10 | TRB_DIR_IN,
            ]);
        }
        let status_dir = if len == 0 { TRB_DIR_IN } else { 0 };
        let status = dev.ep0.push([0, 0, 0, TRB_STATUS << 10 | TRB_IOC | status_dir]);
        let slot = dev.slot;
        self.ring(slot, 1);
        let ev = self.wait("a control transfer", |ev| {
            trb_type(ev) == TRB_TRANSFER_EVENT
                && (ev[3] >> 24) as u8 == slot
                && (param(ev) == status || !matches!(completion(ev), CC_SUCCESS | CC_SHORT_PACKET))
        })?;
        match completion(&ev) {
            CC_SUCCESS | CC_SHORT_PACKET => Ok(()),
            cc => Err(Error::Transfer(cc)),
        }
    }

    fn get_descriptor(
        &mut self,
        dev: &mut Device<P::Mem>,
        value: u16,
        len: u16,
    ) -> Result<(), Error> {
        self.control(dev, 0x80, 6, value, 0, len)
    }

    /// The input context's offset of context `i` (0 the slot, then the
    /// endpoints by their DCI).
    fn ctx(&self, i: usize) -> usize {
        (1 + i) * self.csz
    }

    fn write_slot_context(&self, dev: &mut Device<P::Mem>, entries: u32) {
        let s = self.ctx(0);
        dev.input.write32(s, dev.speed << 20 | entries << 27);
        dev.input.write32(s + 4, u32::from(dev.port) << 16);
    }

    fn write_ep0_context(&self, dev: &mut Device<P::Mem>) {
        let e = self.ctx(1);
        // Error count 3, type control.
        dev.input.write32(e + 4, 3 << 1 | 4 << 3 | dev.mps0 << 16);
        let start = dev.ep0.start();
        dev.input.write32(e + 8, start as u32);
        dev.input.write32(e + 12, (start >> 32) as u32);
        dev.input.write32(e + 16, 8);
    }

    /// A slot for the device on `port`, addressed, with its endpoint 0
    /// working.
    fn address(&mut self, port: u8, speed: u32) -> Result<Device<P::Mem>, Error> {
        let ev = self.command([0, 0, 0, TRB_ENABLE_SLOT << 10])?;
        let slot = (ev[3] >> 24) as u8;
        if slot == 0 || slot > self.slots {
            return Err(Error::NoSlot);
        }
        let output = self.alloc(PAGE)?;
        let input = self.alloc(PAGE)?;
        let ring = self.alloc(PAGE)?;
        let buf = self.alloc(PAGE)?;
        let mps0 = match speed {
            SPEED_LOW | SPEED_FULL => 8,
            SPEED_HIGH => 64,
            _ => 512,
        };
        let mut dev = Device {
            slot,
            port,
            speed,
            ep0: Ring::new(ring),
            _output: output,
            input,
            buf,
            mps0,
            dci: 0,
            intr: None,
            intr_mps: 0,
            kind: None,
            dead: false,
            vendor: 0,
            product: 0,
        };
        let out = dev._output.phys();
        self.dcbaa.write32(usize::from(slot) * 8, out as u32);
        self.dcbaa.write32(usize::from(slot) * 8 + 4, (out >> 32) as u32);
        // Add the slot and endpoint 0.
        dev.input.write32(4, 0b11);
        self.write_slot_context(&mut dev, 1);
        self.write_ep0_context(&mut dev);
        let at = dev.input.phys();
        self.command([
            at as u32,
            (at >> 32) as u32,
            0,
            TRB_ADDRESS_DEVICE << 10 | u32::from(slot) << 24,
        ])?;
        // The first 8 bytes of the device descriptor say endpoint 0's
        // packet size (a power of 2 at SuperSpeed).
        self.get_descriptor(&mut dev, 0x0100, 8)?;
        let raw = dev.buf.read32(DATA_AT + 4) >> 24;
        let mps0 = if speed > SPEED_HIGH { 1 << raw.min(12) } else { raw };
        if mps0 == 0 {
            return Err(Error::Descriptor("endpoint 0 packet size 0"));
        }
        if mps0 != dev.mps0 {
            dev.mps0 = mps0;
            dev.input.zero(0, PAGE);
            dev.input.write32(4, 0b10);
            self.write_ep0_context(&mut dev);
            self.command([
                at as u32,
                (at >> 32) as u32,
                0,
                TRB_EVALUATE_CONTEXT << 10 | u32::from(slot) << 24,
            ])?;
        }
        self.get_descriptor(&mut dev, 0x0100, 18)?;
        let ids = dev.buf.read32(DATA_AT + 8);
        dev.vendor = ids as u16;
        dev.product = (ids >> 16) as u16;
        Ok(dev)
    }

    /// Configures the device for its HID interface; what it is read as.
    fn configure(&mut self, dev: &mut Device<P::Mem>) -> Result<Kind, Error> {
        self.get_descriptor(dev, 0x0200, 9)?;
        let total = (dev.buf.read32(DATA_AT) >> 16) as u16;
        if total < 9 {
            return Err(Error::Descriptor("configuration shorter than its header"));
        }
        let total = total.min(DATA_LEN as u16);
        self.get_descriptor(dev, 0x0200, total)?;
        let mut desc = [0u8; DATA_LEN];
        dev.buf.copy_out(DATA_AT, &mut desc[..usize::from(total)]);
        let hid = find_hid(&desc[..usize::from(total)]).ok_or(Error::NotHid)?;
        let keyboard = hid.protocol == 1 && hid.subclass == 1;
        if hid.protocol == 1 && !keyboard {
            // A keyboard's reports are read in the boot protocol only.
            return Err(Error::NotHid);
        }
        let mps = usize::from(hid.max_packet);
        if mps == 0 {
            return Err(Error::Descriptor("interrupt endpoint packet size 0"));
        }
        // The interrupt endpoint's context.
        let dci = (hid.endpoint & 0xf) * 2 + 1;
        let ring = Ring::new(self.alloc(PAGE)?);
        dev.input.zero(0, PAGE);
        dev.input.write32(4, 1 | 1 << dci);
        self.write_slot_context(dev, u32::from(dci));
        let e = self.ctx(usize::from(dci));
        let mps32 = u32::from(hid.max_packet);
        dev.input.write32(e, interval(dev.speed, hid.interval) << 16);
        // Error count 3, type interrupt IN.
        dev.input.write32(e + 4, 3 << 1 | 7 << 3 | mps32 << 16);
        let start = ring.start();
        dev.input.write32(e + 8, start as u32);
        dev.input.write32(e + 12, (start >> 32) as u32);
        dev.input.write32(e + 16, mps32 | mps32 << 16);
        let at = dev.input.phys();
        self.command([
            at as u32,
            (at >> 32) as u32,
            0,
            TRB_CONFIGURE_ENDPOINT << 10 | u32::from(dev.slot) << 24,
        ])?;
        dev.dci = dci;
        dev.intr = Some(ring);
        dev.intr_mps = mps.min(SLICE);
        // SET_CONFIGURATION.
        self.control(dev, 0x00, 9, u16::from(hid.configuration), 0, 0)?;
        let iface = u16::from(hid.number);
        if keyboard {
            // SET_PROTOCOL (boot), SET_IDLE (a report only on a change).
            self.control(dev, 0x21, 0x0b, 0, iface, 0)?;
            self.control(dev, 0x21, 0x0a, 0, iface, 0)?;
            return Ok(Kind::Keyboard(hid::Keyboard::default()));
        }
        // A mouse: its report descriptor says where its fields are. No
        // SET_IDLE: a mouse may stall it, and a stall halts endpoint 0.
        self.control(dev, 0x81, 6, 0x2200, iface, hid.report_len)?;
        let n = usize::from(hid.report_len).min(DATA_LEN);
        let mut report = [0u8; DATA_LEN];
        dev.buf.copy_out(DATA_AT, &mut report[..n]);
        match hid::parse_mouse(&report[..n]) {
            Ok(layout) => Ok(Kind::Mouse(layout)),
            Err(e) if hid.subclass == 1 && hid.protocol == 2 => {
                log!(
                    info,
                    "usb: port {}: report descriptor not understood ({e:?}); the boot protocol instead",
                    dev.port
                );
                self.control(dev, 0x21, 0x0b, 0, iface, 0)?;
                Ok(Kind::Mouse(hid::MouseLayout::boot()))
            }
            Err(e) => {
                log!(info, "usb: port {}: report descriptor not understood ({e:?})", dev.port);
                Err(Error::NotHid)
            }
        }
    }

    /// Queues a transfer into slice `k` of the device's buffer.
    fn queue_interrupt(&mut self, dev: &mut Device<P::Mem>, k: usize) {
        let Some(ring) = dev.intr.as_mut() else { return };
        let at = dev.buf.phys() + (k * SLICE) as u64;
        ring.push([
            at as u32,
            (at >> 32) as u32,
            dev.intr_mps as u32,
            TRB_NORMAL << 10 | TRB_IOC | TRB_ISP,
        ]);
    }
}

/// Counters for the log and the `usb:` line at exit.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub reports: u64,
    pub errors: u64,
}

pub struct Xhci<P: Platform> {
    hc: Hc<P>,
    devices: [Option<Device<P::Mem>>; MAX_DEVICES],
    pub version: u16,
    pub stats: Stats,
}

impl<P: Platform> Xhci<P> {
    /// Resets and starts the controller whose registers `p` reaches
    /// (`regs_len` bytes of them) and sets up the devices on its ports.
    pub fn start(mut p: P, regs_len: usize) -> Result<Self, Error> {
        let caplength = (p.read32(CAPLENGTH) & 0xff) as usize;
        let version = (p.read32(CAPLENGTH) >> 16) as u16;
        let hcs1 = p.read32(HCSPARAMS1);
        let hcs2 = p.read32(HCSPARAMS2);
        let hcc1 = p.read32(HCCPARAMS1);
        let op = caplength;
        let db = (p.read32(DBOFF) & !3) as usize;
        let rt = (p.read32(RTSOFF) & !0x1f) as usize;
        let slots = (hcs1 & 0xff).min(MAX_DEVICES as u32) as u8;
        let ports = (hcs1 >> 24) as u8;
        let ac64 = hcc1 & 1 != 0;
        let csz = if hcc1 & 1 << 2 != 0 { 64 } else { 32 };

        bios_handoff(&mut p, (hcc1 >> 16) as usize * 4, regs_len);

        if !hw::wait_for(&p, SECOND, |p| p.read32(op + USBSTS) & STS_CNR == 0) {
            return Err(Error::Timeout("controller ready"));
        }
        if p.read32(op + USBCMD) & CMD_RS != 0 {
            let cmd = p.read32(op + USBCMD);
            p.write32(op + USBCMD, cmd & !CMD_RS);
            if !hw::wait_for(&p, 100 * MS, |p| p.read32(op + USBSTS) & STS_HCH != 0) {
                return Err(Error::Timeout("halt"));
            }
        }
        p.write32(op + USBCMD, CMD_HCRST);
        hw::delay(&p, MS);
        if !hw::wait_for(&p, SECOND, |p| {
            p.read32(op + USBCMD) & CMD_HCRST == 0 && p.read32(op + USBSTS) & STS_CNR == 0
        }) {
            return Err(Error::Timeout("reset"));
        }
        let pagesize = p.read32(op + PAGESIZE);
        if pagesize & 1 == 0 {
            return Err(Error::PageSize(pagesize));
        }

        let alloc = |p: &mut P, bytes| -> Result<P::Mem, Error> {
            let m = p.alloc(bytes).ok_or(Error::NoMemory)?;
            if !ac64 && m.phys() + m.len() as u64 > 1 << 32 {
                return Err(Error::Above4G);
            }
            Ok(m)
        };
        let mut dcbaa = alloc(&mut p, PAGE)?;
        let scratchpads = (hcs2 >> 27 & 0x1f) | (hcs2 >> 21 & 0x1f) << 5;
        let scratch = if scratchpads == 0 {
            None
        } else if scratchpads > MAX_SCRATCHPADS {
            return Err(Error::Scratchpads(scratchpads));
        } else {
            let mut array = alloc(&mut p, scratchpads as usize * 8)?;
            let bufs = alloc(&mut p, scratchpads as usize * PAGE)?;
            for i in 0..scratchpads as usize {
                let at = bufs.phys() + (i * PAGE) as u64;
                array.write32(i * 8, at as u32);
                array.write32(i * 8 + 4, (at >> 32) as u32);
            }
            dcbaa.write32(0, array.phys() as u32);
            dcbaa.write32(4, (array.phys() >> 32) as u32);
            Some((array, bufs))
        };
        let cmd = Ring::new(alloc(&mut p, PAGE)?);
        // The event ring's segment, then its one-entry segment table.
        let mut events = alloc(&mut p, 2 * PAGE)?;
        let seg = events.phys();
        events.write32(PAGE, seg as u32);
        events.write32(PAGE + 4, (seg >> 32) as u32);
        events.write32(PAGE + 8, RING_TRBS as u32);

        p.write32(op + CONFIG, u32::from(slots));
        p.write64(op + DCBAAP, dcbaa.phys());
        p.write64(op + CRCR, cmd.start());
        p.write32(rt + IR0 + ERSTSZ, 1);
        p.write64(rt + IR0 + ERDP, seg);
        p.write64(rt + IR0 + ERSTBA, seg + PAGE as u64);
        p.write32(op + USBCMD, CMD_RS);
        if !hw::wait_for(&p, 100 * MS, |p| p.read32(op + USBSTS) & STS_HCH == 0) {
            return Err(Error::Timeout("run"));
        }

        let mut hc = Hc {
            p,
            op,
            rt,
            db,
            ac64,
            csz,
            ports,
            slots,
            dcbaa,
            _scratch: scratch,
            cmd,
            events,
            deq: 0,
            ccs: TRB_CYCLE,
            strays: 0,
        };
        log!(
            info,
            "usb: xhci: version {}.{:02x}, {} ports, {} slots used, {}-byte contexts, {}-bit, {} scratchpads",
            version >> 8,
            version & 0xff,
            ports,
            slots,
            csz,
            if ac64 { 64 } else { 32 },
            scratchpads
        );
        let mut devices: [Option<Device<P::Mem>>; MAX_DEVICES] = core::array::from_fn(|_| None);
        let mut used = 0;
        for port in connected_ports(&mut hc).iter().flatten() {
            let (port, speed) = *port;
            if used == MAX_DEVICES {
                log!(warn, "usb: port {port}: more than {MAX_DEVICES} devices; not set up");
                continue;
            }
            match setup(&mut hc, port, speed) {
                Ok(dev) => {
                    devices[used] = Some(dev);
                    used += 1;
                }
                Err(e) => log!(warn, "usb: port {port}: device not used: {e}"),
            }
        }
        let mut xhci = Self { hc, devices, version, stats: Stats::default() };
        // The interrupt transfers, queued once every device is set up
        // (their events would otherwise come among the commands').
        for dev in xhci.devices.iter_mut().flatten() {
            for k in 0..INTERRUPT_TRBS {
                xhci.hc.queue_interrupt(dev, k);
            }
            let (slot, dci) = (dev.slot, dev.dci);
            xhci.hc.ring(slot, u32::from(dci));
        }
        Ok(xhci)
    }

    /// The devices set up: (port, what it is read as).
    pub fn devices(&self) -> impl Iterator<Item = (u8, &'static str)> + '_ {
        self.devices.iter().flatten().map(|d| (d.port, d.kind.as_ref().map_or("?", Kind::name)))
    }

    /// Reads what the devices reported, at most `limit` events of the
    /// controller's (at least one, at most a ring's worth: a controller
    /// writing events without end cannot keep the caller here); `f` gets
    /// each key and pointer change. The reports read (keyboards', mice's),
    /// counted once their transfers are queued again.
    pub fn poll(&mut self, limit: usize, mut f: impl FnMut(hid::Event)) -> (u64, u64) {
        let mut n = (0, 0);
        for _ in 0..limit.clamp(1, RING_TRBS) {
            let Some(ev) = self.hc.next_event() else { break };
            if trb_type(&ev) != TRB_TRANSFER_EVENT {
                self.hc.stray(&ev);
                continue;
            }
            let slot = (ev[3] >> 24) as u8;
            let dci = (ev[3] >> 16 & 0x1f) as u8;
            let Some(dev) =
                self.devices.iter_mut().flatten().find(|d| d.slot == slot && d.dci == dci)
            else {
                self.hc.stray(&ev);
                continue;
            };
            if dev.dead {
                continue;
            }
            let cc = completion(&ev);
            if !matches!(cc, CC_SUCCESS | CC_SHORT_PACKET) {
                self.stats.errors += 1;
                dev.dead = true;
                log!(
                    warn,
                    "usb: port {}: {} transfer failed (completion code {cc}); the device is no longer read",
                    dev.port,
                    dev.kind.as_ref().map_or("?", Kind::name)
                );
                continue;
            }
            let Some(trb) = dev.intr.as_ref().and_then(|r| r.trb(param(&ev))) else {
                self.hc.stray(&ev);
                continue;
            };
            let k = (param(&trb).wrapping_sub(dev.buf.phys()) as usize) / SLICE;
            if k >= INTERRUPT_TRBS {
                self.hc.stray(&ev);
                continue;
            }
            let asked = (trb[2] & 0x1ffff) as usize;
            let len = asked.saturating_sub((ev[2] & 0xff_ffff) as usize).min(SLICE);
            let mut report = [0u8; SLICE];
            dev.buf.copy_out(k * SLICE, &mut report[..len]);
            match dev.kind.as_mut() {
                Some(Kind::Keyboard(kb)) => {
                    kb.report(&report[..len], &mut f);
                    n.0 += 1;
                }
                Some(Kind::Mouse(layout)) => {
                    if let Some(e) = layout.decode(&report[..len]) {
                        f(e);
                    }
                    n.1 += 1;
                }
                None => {}
            }
            self.stats.reports += 1;
            self.hc.queue_interrupt(dev, k);
            let (slot, dci) = (dev.slot, dev.dci);
            self.hc.ring(slot, u32::from(dci));
        }
        n
    }

    /// Events no one asked for, since the start.
    pub fn strays(&self) -> u64 {
        self.hc.strays
    }
}

/// Takes the controller from the firmware (the USB Legacy Support
/// extended capability), if the firmware has it, and turns off the
/// firmware's SMIs.
fn bios_handoff<P: Platform>(p: &mut P, first: usize, regs_len: usize) {
    let mut at = first;
    while at != 0 && at + 8 <= regs_len {
        let cap = p.read32(at);
        if cap & 0xff == 1 {
            if cap & 1 << 16 != 0 {
                p.write32(at, cap | 1 << 24);
                if !hw::wait_for(p, SECOND, |p| p.read32(at) & 1 << 16 == 0) {
                    log!(
                        warn,
                        "usb: xhci: the firmware did not hand the controller over; taken anyway"
                    );
                }
            }
            let ctl = p.read32(at + 4);
            p.write32(at + 4, ctl & !(1 | 1 << 4 | 1 << 13 | 1 << 14 | 1 << 15));
            return;
        }
        let next = (cap >> 8 & 0xff) as usize * 4;
        if next == 0 {
            return;
        }
        at += next;
    }
}

/// Powers the ports, resets those with a device, and says which are
/// enabled: (port, speed ID).
fn connected_ports<P: Platform>(hc: &mut Hc<P>) -> [Option<(u8, u32)>; 32] {
    let mut out = [None; 32];
    let mut powered = false;
    for port in 1..=hc.ports {
        let r = hc.portsc(port);
        let v = hc.p.read32(r);
        if v & PORT_PP == 0 {
            hc.p.write32(r, v & PORT_PRESERVE | PORT_PP);
            powered = true;
        }
    }
    if powered {
        hw::delay(&hc.p, 100 * MS);
    }
    let mut n = 0;
    for port in 1..=hc.ports {
        let r = hc.portsc(port);
        let v = hc.p.read32(r);
        if v & PORT_CCS == 0 {
            continue;
        }
        if v & PORT_PED == 0 {
            // A USB 2 port: reset it to enable it (a USB 3 port enables
            // itself when its link trains).
            hc.p.write32(r, v & PORT_PRESERVE | PORT_PR);
            if !hw::wait_for(&hc.p, 500 * MS, |p| p.read32(r) & PORT_PRC != 0) {
                log!(warn, "usb: port {port}: reset did not complete");
                continue;
            }
            // The reset recovery time (USB 2.0, 7.1.7.5).
            hw::delay(&hc.p, 10 * MS);
        }
        let v = hc.p.read32(r);
        hc.p.write32(r, v & PORT_PRESERVE | PORT_CHANGES);
        if v & PORT_PED == 0 {
            log!(warn, "usb: port {port}: device present but the port is not enabled");
            continue;
        }
        let speed = v >> 10 & 0xf;
        log!(info, "usb: port {port}: device connected, speed {}", speed_name(speed));
        if n < out.len() {
            out[n] = Some((port, speed));
            n += 1;
        }
    }
    out
}

fn speed_name(speed: u32) -> &'static str {
    match speed {
        SPEED_FULL => "full (12 Mb/s)",
        SPEED_LOW => "low (1.5 Mb/s)",
        SPEED_HIGH => "high (480 Mb/s)",
        4 => "super (5 Gb/s)",
        5 => "super+ (10 Gb/s)",
        _ => "unknown",
    }
}

fn setup<P: Platform>(hc: &mut Hc<P>, port: u8, speed: u32) -> Result<Device<P::Mem>, Error> {
    let mut dev = hc.address(port, speed)?;
    log!(
        info,
        "usb: port {port}: device {:04x}:{:04x} on slot {}",
        dev.vendor,
        dev.product,
        dev.slot
    );
    let kind = hc.configure(&mut dev)?;
    log!(info, "usb: {} on port {port}", kind.name());
    dev.kind = Some(kind);
    Ok(dev)
}

#[cfg(test)]
mod tests;

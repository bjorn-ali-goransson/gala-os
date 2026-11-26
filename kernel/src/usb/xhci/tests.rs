//! The driver against a model of an xHCI controller: its registers, a
//! command ring and an event ring it walks and writes in guest memory, and
//! two full-speed HID devices (a boot keyboard on port 1, a wheel mouse on
//! port 2) that answer endpoint 0's requests and send the reports a test
//! gives them.

use super::*;
use crate::disk::hw::fake::{Clock, FakeMem, Phys, Shared};
use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

const OP: usize = 0x40;
const RT: usize = 0x1000;
const DB: usize = 0x2000;
const REGS_LEN: usize = 0x3000;
/// The extended capabilities, when modelled: a protocol capability
/// whose next pointer leads to the USB Legacy Support one.
const XECP: usize = 0x2800;
const LEGACY: usize = XECP + 0x10;

const WHEEL_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x03,
    0x81, 0x06, 0xc0, 0xc0,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum What {
    Keyboard,
    Mouse,
}

/// A ring the model reads: where it is and the cycle it expects.
#[derive(Default, Clone, Copy)]
struct Cursor {
    deq: u64,
    ccs: u32,
}

impl Cursor {
    fn from(ptr: u64) -> Self {
        Self { deq: ptr & !0xf, ccs: (ptr & 1) as u32 }
    }

    /// The next TRB the driver handed over (Link TRBs followed); its
    /// address and contents.
    fn next(&mut self, phys: &Shared) -> Option<(u64, [u32; 4])> {
        loop {
            let at = self.deq;
            let mut p = phys.borrow_mut();
            let trb = [p.read32(at), p.read32(at + 4), p.read32(at + 8), p.read32(at + 12)];
            if trb[3] & 1 != self.ccs {
                return None;
            }
            if trb_type(&trb) == TRB_LINK {
                self.deq = param(&trb) & !0xf;
                if trb[3] & TRB_TC != 0 {
                    self.ccs ^= 1;
                }
                continue;
            }
            self.deq += 16;
            return Some((at, trb));
        }
    }
}

struct FakeDevice {
    what: What,
    port: u8,
    connected: bool,
    slot: u8,
    ep0: Cursor,
    mps0: u32,
    /// Endpoint 0 stalls every request (a broken device).
    stalls: bool,
    halted: bool,
    setup: [u32; 2],
    intr: Cursor,
    dci: u8,
    interval: u32,
    /// Requests that changed state: (request type, request, value).
    requests: Vec<(u8, u8, u16)>,
}

#[derive(Default)]
struct Model {
    usbcmd: u32,
    config: u32,
    crcr: u64,
    dcbaap: u64,
    erstsz: u32,
    erstba: u64,
    erdp: u64,
    portsc: [u32; 4],
    cmd: Cursor,
    ev_seg: u64,
    ev_size: usize,
    ev_enq: usize,
    ev_pcs: u32,
    next_slot: u8,
    devices: Vec<FakeDevice>,
    /// Command types run, in order.
    commands: Vec<u32>,
    legacy: Option<Legacy>,
}

/// The USB Legacy Support capability and the firmware behind it.
#[derive(Clone, Copy)]
struct Legacy {
    cap: u32,
    ctl: u32,
    /// The firmware never lets go of the controller.
    stubborn: bool,
    /// The capability and control words when the controller was reset.
    at_reset: Option<(u32, u32)>,
}

impl Legacy {
    const BIOS: u32 = 1 << 16;
    const OS: u32 = 1 << 24;
    /// The SMI enables the firmware set: USB, host system error, OS
    /// ownership change, PCI command, BAR; and a status bit (29) that
    /// the driver must not touch.
    const SMIS: u32 = 1 | 1 << 4 | 1 << 13 | 1 << 14 | 1 << 15;

    fn new(stubborn: bool) -> Self {
        Self { cap: 1 | Self::BIOS, ctl: Self::SMIS | 1 << 29, stubborn, at_reset: None }
    }
}

struct Fake {
    m: Rc<RefCell<Model>>,
    phys: Shared,
    clock: Clock,
}

fn set_lo(r: &mut u64, v: u32) {
    *r = *r & !0xffff_ffff | u64::from(v);
}

fn set_hi(r: &mut u64, v: u32) {
    *r = *r & 0xffff_ffff | u64::from(v) << 32;
}

impl Model {
    fn event(&mut self, phys: &Shared, trb: [u32; 4]) {
        assert!(self.ev_size > 0, "an event before the event ring was set up");
        let at = self.ev_seg + (self.ev_enq * 16) as u64;
        let mut p = phys.borrow_mut();
        p.write32(at, trb[0]);
        p.write32(at + 4, trb[1]);
        p.write32(at + 8, trb[2]);
        p.write32(at + 12, trb[3] & !1 | self.ev_pcs);
        self.ev_enq += 1;
        if self.ev_enq == self.ev_size {
            self.ev_enq = 0;
            self.ev_pcs ^= 1;
        }
    }

    fn run(&mut self, phys: &Shared) {
        self.cmd = Cursor::from(self.crcr);
        let mut p = phys.borrow_mut();
        self.ev_seg = p.read64(self.erstba);
        self.ev_size = p.read32(self.erstba + 8) as usize;
        assert_eq!(self.erstsz, 1);
        assert_eq!(self.erdp & !0xf, self.ev_seg);
        self.ev_enq = 0;
        self.ev_pcs = 1;
    }

    fn device(&mut self, slot: u8) -> &mut FakeDevice {
        self.devices.iter_mut().find(|d| d.slot == slot).expect("no device on the slot")
    }

    fn commands(&mut self, phys: &Shared) {
        while let Some((at, trb)) = self.cmd.next(phys) {
            let t = trb_type(&trb);
            self.commands.push(t);
            let input = param(&trb);
            let mut slot = (trb[3] >> 24) as u8;
            let read = |o: u64| phys.borrow_mut().read32(input + o);
            let cc = match t {
                TRB_ENABLE_SLOT => {
                    self.next_slot += 1;
                    slot = self.next_slot;
                    1
                }
                TRB_ADDRESS_DEVICE => {
                    assert_eq!(read(4), 0b11, "address: add flags");
                    let port = (read(32 + 4) >> 16 & 0xff) as u8;
                    let deq = u64::from(read(64 + 8)) | u64::from(read(64 + 12)) << 32;
                    let mps0 = read(64 + 4) >> 16;
                    let out = phys.borrow_mut().read64(self.dcbaap + u64::from(slot) * 8);
                    assert_ne!(out, 0, "no output context for slot {slot}");
                    let d = self
                        .devices
                        .iter_mut()
                        .find(|d| d.port == port)
                        .expect("no device on the port");
                    d.slot = slot;
                    d.ep0 = Cursor::from(deq);
                    d.mps0 = mps0;
                    1
                }
                TRB_EVALUATE_CONTEXT => {
                    let mps0 = read(64 + 4) >> 16;
                    self.device(slot).mps0 = mps0;
                    1
                }
                TRB_CONFIGURE_ENDPOINT => {
                    let add = read(4);
                    assert_eq!(add & 1, 1, "configure: the slot context's add flag");
                    let dci = 31 - (add & !1).leading_zeros();
                    let e = u64::from((1 + dci) * 32);
                    assert_eq!(read(e + 4) >> 3 & 7, 7, "an interrupt IN endpoint");
                    assert_eq!(read(32) >> 27, dci, "context entries");
                    let interval = read(e) >> 16 & 0xff;
                    let deq = u64::from(read(e + 8)) | u64::from(read(e + 12)) << 32;
                    let d = self.device(slot);
                    d.dci = dci as u8;
                    d.interval = interval;
                    d.intr = Cursor::from(deq);
                    1
                }
                _ => 9, // TRB error
            };
            self.event(
                phys,
                [
                    at as u32,
                    (at >> 32) as u32,
                    cc << 24,
                    TRB_COMMAND_COMPLETION << 10 | u32::from(slot) << 24,
                ],
            );
        }
    }

    fn control(&mut self, phys: &Shared, slot: u8) {
        let mut events = Vec::new();
        {
            let d = self.device(slot);
            while !d.halted {
                let Some((at, trb)) = d.ep0.next(phys) else { break };
                match trb_type(&trb) {
                    TRB_SETUP => {
                        assert_ne!(trb[3] & TRB_IDT, 0);
                        d.setup = [trb[0], trb[1]];
                        if d.stalls {
                            d.halted = true;
                            events.push([at as u32, (at >> 32) as u32, 6 << 24]);
                        }
                    }
                    TRB_DATA => {
                        let len = (trb[2] & 0x1ffff) as usize;
                        let reply = d.reply();
                        let n = reply.len().min(len);
                        phys.borrow_mut().write(param(&trb), &reply[..n]);
                        if trb[3] & (TRB_IOC | TRB_ISP) != 0 && n < len {
                            events.push([
                                at as u32,
                                (at >> 32) as u32,
                                13 << 24 | (len - n) as u32,
                            ]);
                        }
                    }
                    TRB_STATUS => {
                        if d.setup[0] & 0x80 == 0 {
                            let request = d.setup[0];
                            d.requests.push((
                                request as u8,
                                (request >> 8) as u8,
                                (request >> 16) as u16,
                            ));
                        }
                        if trb[3] & TRB_IOC != 0 {
                            events.push([at as u32, (at >> 32) as u32, 1 << 24]);
                        }
                    }
                    t => panic!("TRB type {t} on endpoint 0"),
                }
            }
        }
        for e in events {
            self.event(
                phys,
                [e[0], e[1], e[2], TRB_TRANSFER_EVENT << 10 | 1 << 16 | u32::from(slot) << 24],
            );
        }
    }
}

impl FakeDevice {
    fn new(what: What, port: u8) -> Self {
        Self {
            what,
            port,
            connected: true,
            slot: 0,
            ep0: Cursor::default(),
            mps0: 0,
            stalls: false,
            halted: false,
            setup: [0; 2],
            intr: Cursor::default(),
            dci: 0,
            interval: 0,
            requests: Vec::new(),
        }
    }

    fn config(&self) -> Vec<u8> {
        let (protocol, report_len, mps) = match self.what {
            What::Keyboard => (1, 63u16, 8),
            What::Mouse => (2, WHEEL_MOUSE.len() as u16, 4),
        };
        let mut c = std::vec![9, 2, 0, 0, 1, 1, 0, 0xa0, 50];
        c.extend_from_slice(&[9, 4, 0, 0, 1, 3, 1, protocol, 0]);
        c.extend_from_slice(&[
            9,
            0x21,
            0x11,
            0x01,
            0,
            1,
            0x22,
            report_len as u8,
            (report_len >> 8) as u8,
        ]);
        c.extend_from_slice(&[7, 5, 0x81, 3, mps, 0, 10]);
        let total = c.len() as u16;
        c[2..4].copy_from_slice(&total.to_le_bytes());
        c
    }

    /// The data stage of the request set up.
    fn reply(&self) -> Vec<u8> {
        let request_type = self.setup[0] as u8;
        let request = (self.setup[0] >> 8) as u8;
        let value = (self.setup[0] >> 16) as u16;
        match (request_type, request, value) {
            (0x80, 6, 0x0100) => {
                let product = if self.what == What::Keyboard { 0x0001 } else { 0x0002 };
                let mut d = std::vec![18, 1, 0x00, 0x02, 0, 0, 0, 8, 0x34, 0x12];
                d.extend_from_slice(&[product as u8, 0, 0, 1, 0, 0, 0, 1]);
                d
            }
            (0x80, 6, 0x0200) => self.config(),
            (0x81, 6, 0x2200) if self.what == What::Mouse => WHEEL_MOUSE.to_vec(),
            r => panic!("{:?}: request {r:x?} not modelled", self.what),
        }
    }
}

impl Fake {
    fn new(devices: Vec<FakeDevice>) -> (Self, Rc<RefCell<Model>>) {
        let mut model = Model { devices, ..Model::default() };
        for i in 0..4 {
            let connected = model.devices.iter().any(|d| d.port == i as u8 + 1 && d.connected);
            model.portsc[i] = PORT_PP | if connected { PORT_CCS | 1 << 17 } else { 0 };
        }
        let m = Rc::new(RefCell::new(model));
        (Self { m: m.clone(), phys: Phys::shared(), clock: Clock::default() }, m)
    }
}

impl Platform for Fake {
    type Mem = FakeMem;

    fn read32(&self, offset: usize) -> u32 {
        let m = self.m.borrow();
        match offset {
            CAPLENGTH => 0x40 | 0x0110 << 16,
            HCSPARAMS1 => 32 | 1 << 8 | 4 << 24,
            HCSPARAMS2 => 0,
            HCCPARAMS1 => {
                if m.legacy.is_some() {
                    1 | (XECP as u32 / 4) << 16
                } else {
                    1
                }
            }
            // A Supported Protocol capability (ID 2), four dwords long.
            XECP if m.legacy.is_some() => 2 | 4 << 8,
            o if (XECP + 4..LEGACY).contains(&o) && m.legacy.is_some() => 0,
            LEGACY if m.legacy.is_some() => m.legacy.unwrap().cap,
            o if o == LEGACY + 4 && m.legacy.is_some() => m.legacy.unwrap().ctl,
            DBOFF => DB as u32,
            RTSOFF => RT as u32,
            o if o == OP + USBCMD => m.usbcmd,
            o if o == OP + USBSTS => {
                if m.usbcmd & CMD_RS == 0 {
                    STS_HCH
                } else {
                    0
                }
            }
            o if o == OP + PAGESIZE => 1,
            o if o == OP + CONFIG => m.config,
            o if (OP + PORTSC..OP + PORTSC + 0x40).contains(&o) && o % 0x10 == 0 => {
                m.portsc[(o - OP - PORTSC) / 0x10]
            }
            o => panic!("read of register {o:#x}"),
        }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        let mut m = self.m.borrow_mut();
        let m = &mut *m;
        match offset {
            o if o == OP + USBCMD => {
                if value & CMD_HCRST != 0 {
                    assert_eq!(m.usbcmd & CMD_RS, 0, "reset while running");
                    let legacy = m.legacy.map(|l| Legacy { at_reset: Some((l.cap, l.ctl)), ..l });
                    *m = Model {
                        devices: core::mem::take(&mut m.devices),
                        portsc: m.portsc,
                        legacy,
                        ..Model::default()
                    };
                    return;
                }
                if value & CMD_RS != 0 && m.usbcmd & CMD_RS == 0 {
                    m.run(&self.phys);
                }
                m.usbcmd = value;
            }
            LEGACY if m.legacy.is_some() => {
                let l = m.legacy.as_mut().unwrap();
                // Only the OS-owned semaphore is the driver's to write.
                assert_eq!(value & !Legacy::OS, l.cap & !Legacy::OS, "capability word changed");
                l.cap = value;
                if value & Legacy::OS != 0 && !l.stubborn {
                    l.cap &= !Legacy::BIOS;
                }
            }
            o if o == LEGACY + 4 && m.legacy.is_some() => {
                let l = m.legacy.as_mut().unwrap();
                l.ctl = value;
            }
            o if o == OP + CONFIG => m.config = value,
            o if o == OP + CRCR => set_lo(&mut m.crcr, value),
            o if o == OP + CRCR + 4 => set_hi(&mut m.crcr, value),
            o if o == OP + DCBAAP => set_lo(&mut m.dcbaap, value),
            o if o == OP + DCBAAP + 4 => set_hi(&mut m.dcbaap, value),
            o if (OP + PORTSC..OP + PORTSC + 0x40).contains(&o) && o % 0x10 == 0 => {
                let r = &mut m.portsc[(o - OP - PORTSC) / 0x10];
                assert_eq!(value & PORT_PED, 0, "a write of PED disables the port");
                *r &= !(value & PORT_CHANGES);
                if value & PORT_PR != 0 && *r & PORT_CCS != 0 {
                    *r |= PORT_PED | PORT_PRC | SPEED_FULL << 10;
                }
            }
            o if o == RT + IR0 + ERSTSZ => m.erstsz = value,
            o if o == RT + IR0 + ERSTBA => set_lo(&mut m.erstba, value),
            o if o == RT + IR0 + ERSTBA + 4 => set_hi(&mut m.erstba, value),
            o if o == RT + IR0 + ERDP => set_lo(&mut m.erdp, value),
            o if o == RT + IR0 + ERDP + 4 => set_hi(&mut m.erdp, value),
            DB => {
                assert_ne!(m.usbcmd & CMD_RS, 0, "a doorbell while halted");
                m.commands(&self.phys);
            }
            o if (DB + 4..DB + 4 * 33).contains(&o) => {
                let slot = ((o - DB) / 4) as u8;
                match value {
                    1 => m.control(&self.phys, slot),
                    t => assert_eq!(
                        t,
                        u32::from(m.device(slot).dci),
                        "a doorbell for another endpoint"
                    ),
                }
            }
            o => panic!("write of {value:#x} to register {o:#x}"),
        }
    }

    fn alloc(&mut self, bytes: usize) -> Option<FakeMem> {
        let at = self.phys.borrow_mut().alloc(bytes);
        Some(FakeMem { phys: self.phys.clone(), at, len: bytes.div_ceil(4096) * 4096 })
    }

    fn now_ns(&self) -> u64 {
        self.clock.now()
    }
}

/// What a test does to the model while the driver holds the platform.
struct Bus {
    m: Rc<RefCell<Model>>,
    phys: Shared,
}

impl Bus {
    /// The device on `port` completes its oldest interrupt transfer with
    /// `data` (or fails it with `cc`).
    fn complete(&self, port: u8, data: &[u8], cc: u32) {
        let mut m = self.m.borrow_mut();
        let d = m.devices.iter_mut().find(|d| d.port == port).unwrap();
        let (at, trb) = d.intr.next(&self.phys).expect("no interrupt transfer queued");
        assert_eq!(trb_type(&trb), TRB_NORMAL);
        let len = (trb[2] & 0x1ffff) as usize;
        let n = data.len().min(len);
        self.phys.borrow_mut().write(param(&trb), &data[..n]);
        let cc = if cc == 1 && n < len { 13 } else { cc };
        let (slot, dci) = (d.slot, d.dci);
        m.event(
            &self.phys,
            [
                at as u32,
                (at >> 32) as u32,
                cc << 24 | (len - n) as u32,
                TRB_TRANSFER_EVENT << 10 | u32::from(dci) << 16 | u32::from(slot) << 24,
            ],
        );
    }

    fn report(&self, port: u8, data: &[u8]) {
        self.complete(port, data, 1);
    }
}

fn start(devices: Vec<FakeDevice>) -> (Xhci<Fake>, Bus) {
    let (fake, m) = Fake::new(devices);
    let phys = fake.phys.clone();
    let x = Xhci::start(fake, REGS_LEN).expect("start");
    (x, Bus { m, phys })
}

fn both() -> Vec<FakeDevice> {
    std::vec![FakeDevice::new(What::Keyboard, 1), FakeDevice::new(What::Mouse, 2)]
}

fn poll(x: &mut Xhci<Fake>) -> Vec<hid::Event> {
    let mut out = Vec::new();
    x.poll(usize::MAX, |e| out.push(e));
    out
}

#[test]
fn both_devices_are_set_up() {
    let (x, bus) = start(both());
    let devices: Vec<_> = x.devices().collect();
    assert_eq!(devices, [(1, "keyboard"), (2, "mouse")]);
    let m = bus.m.borrow();
    assert_eq!(m.config, 32.min(MAX_DEVICES as u32));
    let kb = &m.devices[0];
    // SET_CONFIGURATION 1, SET_PROTOCOL boot, SET_IDLE.
    assert_eq!(kb.requests, [(0x00, 9, 1), (0x21, 0x0b, 0), (0x21, 0x0a, 0)]);
    assert_eq!((kb.dci, kb.mps0), (3, 8));
    // 10 ms at full speed: 2^6 125-microsecond frames.
    assert_eq!(kb.interval, 6);
    let mouse = &m.devices[1];
    // The report protocol (no SET_PROTOCOL) and no SET_IDLE.
    assert_eq!(mouse.requests, [(0x00, 9, 1)]);
    assert_eq!(x.strays(), 0);
}

#[test]
fn keyboard_reports_are_key_presses() {
    let (mut x, bus) = start(both());
    bus.report(1, &[0, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(poll(&mut x), [hid::Event::Key { usage: 0x04, down: true }]);
    bus.report(1, &[0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(poll(&mut x), [hid::Event::Key { usage: 0xe1, down: true }]);
    bus.report(1, &[0; 8]);
    let up = poll(&mut x);
    assert!(up.contains(&hid::Event::Key { usage: 0x04, down: false }), "{up:?}");
    assert!(up.contains(&hid::Event::Key { usage: 0xe1, down: false }), "{up:?}");
    assert_eq!(x.stats.reports, 3);
}

#[test]
fn many_reports_wrap_the_rings() {
    // More reports than the interrupt and event rings hold: the driver
    // requeues each transfer and follows both rings round.
    let (mut x, bus) = start(both());
    let mut events = 0;
    for i in 0..3 * RING_TRBS {
        let key = if i % 2 == 0 { 0x05 } else { 0 };
        bus.report(1, &[0, 0, key, 0, 0, 0, 0, 0]);
        events += poll(&mut x).len();
    }
    assert_eq!(events, 3 * RING_TRBS);
    assert_eq!(x.stats.reports, 3 * RING_TRBS as u64);
    assert_eq!(x.strays(), 0);
}

#[test]
fn reports_queued_together_are_all_read() {
    let (mut x, bus) = start(both());
    for _ in 0..INTERRUPT_TRBS / 2 {
        bus.report(1, &[0, 0, 0x06, 0, 0, 0, 0, 0]);
        bus.report(1, &[0; 8]);
    }
    assert_eq!(poll(&mut x).len(), INTERRUPT_TRBS);
    // The transfers were queued again.
    bus.report(1, &[0, 0, 0x07, 0, 0, 0, 0, 0]);
    assert_eq!(poll(&mut x), [hid::Event::Key { usage: 0x07, down: true }]);
}

#[test]
fn mouse_reports_are_pointer_moves() {
    let (mut x, bus) = start(both());
    bus.report(2, &[0b001, 0xfe, 3, 0xff]);
    assert_eq!(poll(&mut x), [hid::Event::Pointer { dx: -2, dy: 3, buttons: 1, dz: -1 }]);
    // A short report: no wheel byte.
    bus.report(2, &[0b100, 5, 0]);
    assert_eq!(poll(&mut x), [hid::Event::Pointer { dx: 5, dy: 0, buttons: 4, dz: 0 }]);
}

#[test]
fn a_failed_transfer_stops_reading_the_device() {
    let (mut x, bus) = start(both());
    bus.complete(1, &[], 4);
    assert_eq!(poll(&mut x), []);
    assert_eq!(x.stats.errors, 1);
    bus.report(1, &[0, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(poll(&mut x), []);
    // The mouse is still read.
    bus.report(2, &[0, 1, 1, 0]);
    assert_eq!(poll(&mut x).len(), 1);
}

#[test]
fn a_device_that_stalls_is_left_out() {
    let mut devices = both();
    devices[0].stalls = true;
    let (mut x, bus) = start(devices);
    assert_eq!(x.devices().collect::<Vec<_>>(), [(2, "mouse")]);
    bus.report(2, &[0, 1, 1, 0]);
    assert_eq!(poll(&mut x).len(), 1);
}

fn start_with_firmware(stubborn: bool) -> Legacy {
    let (fake, m) = Fake::new(both());
    m.borrow_mut().legacy = Some(Legacy::new(stubborn));
    let x = Xhci::start(fake, REGS_LEN).expect("start");
    assert_eq!(x.devices().count(), 2);
    m.borrow().legacy.expect("the capability")
}

#[test]
fn firmware_hands_the_controller_over_before_the_reset() {
    let l = start_with_firmware(false);
    let (cap, ctl) = l.at_reset.expect("the controller was reset");
    assert_eq!(cap & (Legacy::BIOS | Legacy::OS), Legacy::OS);
    // The SMI enables are off; the status bit is left alone.
    assert_eq!(ctl, 1 << 29);
}

#[test]
fn a_firmware_that_holds_on_is_overridden() {
    let l = start_with_firmware(true);
    let (cap, ctl) = l.at_reset.expect("the controller was reset");
    assert_eq!(cap & (Legacy::BIOS | Legacy::OS), Legacy::BIOS | Legacy::OS);
    assert_eq!(ctl, 1 << 29);
}

#[test]
fn no_devices() {
    let (mut x, bus) = start(std::vec![]);
    assert_eq!(x.devices().count(), 0);
    assert_eq!(poll(&mut x), []);
    assert_eq!(bus.m.borrow().commands, []);
}

#[test]
fn find_hid_skips_other_interfaces() {
    let desc: &[u8] = &[
        9, 2, 0, 0, 2, 3, 0, 0xa0, 50, // configuration 3
        9, 4, 0, 0, 1, 8, 6, 0x50, 0, // mass storage
        7, 5, 0x81, 2, 0, 2, 0, // its bulk IN
        9, 4, 1, 1, 1, 3, 1, 1, 0, // HID, alternate setting 1
        9, 0x21, 0x11, 1, 0, 1, 0x22, 40, 0, //
        7, 5, 0x83, 3, 8, 0, 4, //
        9, 4, 1, 0, 1, 3, 0, 0, 0, // HID, setting 0, report protocol
        9, 0x21, 0x11, 1, 0, 1, 0x22, 77, 0, //
        7, 5, 0x02, 3, 8, 0, 1, // an OUT endpoint first
        7, 5, 0x84, 3, 16, 0, 2, //
    ];
    assert_eq!(
        find_hid(desc),
        Some(HidInterface {
            configuration: 3,
            number: 1,
            subclass: 0,
            protocol: 0,
            endpoint: 0x84,
            max_packet: 16,
            interval: 2,
            report_len: 77,
        })
    );
    assert_eq!(find_hid(&desc[..37]), None);
    // A truncated descriptor ends the walk.
    assert_eq!(find_hid(&[9, 2, 0, 0, 1, 1, 0, 0, 0, 9, 4]), None);
}

#[test]
fn intervals() {
    assert_eq!(interval(SPEED_FULL, 1), 3);
    assert_eq!(interval(SPEED_FULL, 10), 6);
    assert_eq!(interval(SPEED_LOW, 255), 10);
    assert_eq!(interval(SPEED_HIGH, 4), 3);
    assert_eq!(interval(4, 1), 0);
}

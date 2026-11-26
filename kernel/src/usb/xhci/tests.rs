//! The driver against a model of an xHCI controller: its registers, a
//! command ring and an event ring it walks and writes in guest memory, and
//! full-speed devices that answer endpoint 0's requests: HID devices (a
//! boot keyboard, a wheel mouse) that send the reports a test gives them,
//! and a CDC Ethernet adapter (ECM or NCM, in its second configuration)
//! whose bulk endpoints carry the frames a test gives and takes.

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
    Net(cdc::Model),
}

/// The adapter's MAC address and its string (iMACAddress 3).
const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const MAC_STRING: &str = "525400123456";
/// The adapter's bulk endpoints (0x81 IN, 0x02 OUT): their DCIs.
const NET_IN_DCI: u8 = 3;
const NET_OUT_DCI: u8 = 4;
const NET_MPS: usize = 64;

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
    /// The data of requests out (SET_NTB_INPUT_SIZE's).
    data_out: Vec<Vec<u8>>,
    /// Endpoint 0 stalls SET_ETHERNET_PACKET_FILTER.
    filter_stalls: bool,
    /// Reset Endpoint ran on endpoint 0 since it halted.
    ep0_reset: bool,
    /// The bulk endpoints of an adapter.
    bulk_in: Cursor,
    bulk_out: Cursor,
    /// Bulk OUT transfers are left queued (a busy device) until
    /// `Bus::take_out`.
    out_paused: bool,
    /// The bytes of the transfer out being read, the transfers out
    /// whole (a TRB with IOC ends one), and each TRB's length.
    out_partial: Vec<u8>,
    sent: Vec<Vec<u8>>,
    out_trbs: Vec<usize>,
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
                TRB_CONFIGURE_ENDPOINT if read(4).count_ones() == 3 => {
                    // An adapter's two bulk endpoints.
                    let add = read(4);
                    let (i, o) = (u32::from(NET_IN_DCI), u32::from(NET_OUT_DCI));
                    assert_eq!(add, 1 | 1 << i | 1 << o, "configure: add flags");
                    assert_eq!(read(32) >> 27, o, "context entries");
                    let mut cursors = [Cursor::default(); 2];
                    for (n, (dci, ty)) in [(i, 6), (o, 2)].into_iter().enumerate() {
                        let e = u64::from((1 + dci) * 32);
                        assert_eq!(read(e + 4) >> 3 & 7, ty, "endpoint type of DCI {dci}");
                        assert_eq!(read(e + 4) >> 16, NET_MPS as u32, "max packet");
                        let deq = u64::from(read(e + 8)) | u64::from(read(e + 12)) << 32;
                        cursors[n] = Cursor::from(deq);
                    }
                    let d = self.device(slot);
                    assert!(matches!(d.what, What::Net(_)));
                    (d.bulk_in, d.bulk_out) = (cursors[0], cursors[1]);
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
                TRB_RESET_ENDPOINT => {
                    assert_eq!(trb[3] >> 16 & 0x1f, 1, "reset of endpoint 0");
                    let d = self.device(slot);
                    assert!(d.halted, "reset of an endpoint not halted");
                    d.ep0_reset = true;
                    1
                }
                TRB_SET_TR_DEQUEUE => {
                    assert_eq!(trb[3] >> 16 & 0x1f, 1, "dequeue of endpoint 0");
                    let d = self.device(slot);
                    assert!(d.ep0_reset, "dequeue set before the endpoint was reset");
                    d.ep0 = Cursor::from(param(&trb));
                    d.halted = false;
                    d.ep0_reset = false;
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
                        let filter = trb[0] & 0xffff == 0x4321;
                        if d.stalls || d.filter_stalls && filter {
                            d.halted = true;
                            events.push([at as u32, (at >> 32) as u32, 6 << 24]);
                        }
                    }
                    TRB_DATA if d.setup[0] & 0x80 == 0 => {
                        let len = (trb[2] & 0x1ffff) as usize;
                        let mut data = std::vec![0u8; len];
                        phys.borrow_mut().read(param(&trb), &mut data);
                        d.data_out.push(data);
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

    /// An adapter reads the transfers queued on its bulk OUT endpoint.
    fn bulk_out(&mut self, phys: &Shared, slot: u8) {
        let mut events = Vec::new();
        {
            let d = self.device(slot);
            if d.out_paused {
                return;
            }
            while let Some((at, trb)) = d.bulk_out.next(phys) {
                assert_eq!(trb_type(&trb), TRB_NORMAL);
                let len = (trb[2] & 0x1ffff) as usize;
                // A packet short of the largest ends the transfer; a
                // zero-length one is that short packet.
                let mut data = std::vec![0u8; len];
                if len != 0 {
                    phys.borrow_mut().read(param(&trb), &mut data);
                }
                d.out_partial.extend_from_slice(&data);
                d.out_trbs.push(len);
                if trb[3] & TRB_IOC != 0 {
                    d.sent.push(core::mem::take(&mut d.out_partial));
                    events.push(at);
                }
            }
        }
        for at in events {
            self.event(
                phys,
                [
                    at as u32,
                    (at >> 32) as u32,
                    1 << 24,
                    TRB_TRANSFER_EVENT << 10 | u32::from(NET_OUT_DCI) << 16 | u32::from(slot) << 24,
                ],
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
            data_out: Vec::new(),
            filter_stalls: false,
            ep0_reset: false,
            bulk_in: Cursor::default(),
            bulk_out: Cursor::default(),
            out_paused: false,
            out_partial: Vec::new(),
            sent: Vec::new(),
            out_trbs: Vec::new(),
        }
    }

    /// An adapter's configurations: the first a vendor's (RNDIS-like,
    /// which gala leaves alone), the second the CDC one.
    fn net_config(model: cdc::Model, index: u16) -> Vec<u8> {
        let mps = NET_MPS as u8;
        let mut c = std::vec![9, 2, 0, 0, 2, index as u8 + 1, 0, 0xc0, 50];
        let (subclass, data_protocol) = match (index, model) {
            (0, _) => (0x02, 0),
            (_, cdc::Model::Ecm) => (0x06, 0),
            (_, cdc::Model::Ncm) => (0x0d, 1),
        };
        c.extend_from_slice(&[9, 4, 0, 0, 1, 2, subclass, if index == 0 { 0xff } else { 0 }, 0]);
        c.extend_from_slice(&[5, 0x24, 0x00, 0x10, 0x01]); // header
        c.extend_from_slice(&[5, 0x24, 0x06, 0, 1]); // union: data interface 1
        if index != 0 {
            c.extend_from_slice(&[13, 0x24, 0x0f, 3, 0, 0, 0, 0, 0xea, 0x05, 0, 0, 0]);
        }
        if subclass == 0x0d {
            c.extend_from_slice(&[6, 0x24, 0x1a, 0x00, 0x01, 0]);
        }
        c.extend_from_slice(&[7, 5, 0x83, 3, 16, 0, 32]); // notifications
        if index != 0 {
            // The data interface's setting without endpoints.
            c.extend_from_slice(&[9, 4, 1, 0, 0, 0x0a, 0, data_protocol, 0]);
        }
        let alt = if index == 0 { 0 } else { 1 };
        c.extend_from_slice(&[9, 4, 1, alt, 2, 0x0a, 0, data_protocol, 0]);
        c.extend_from_slice(&[7, 5, 0x81, 2, mps, 0, 0]);
        c.extend_from_slice(&[7, 5, 0x02, 2, mps, 0, 0]);
        let total = c.len() as u16;
        c[2..4].copy_from_slice(&total.to_le_bytes());
        c
    }

    fn config(&self) -> Vec<u8> {
        let (protocol, report_len, mps) = match self.what {
            What::Keyboard => (1, 63u16, 8),
            What::Mouse => (2, WHEEL_MOUSE.len() as u16, 4),
            What::Net(model) => {
                return Self::net_config(model, (self.setup[0] >> 16) as u16 & 0xff);
            }
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
        let net = matches!(self.what, What::Net(_));
        match (request_type, request, value) {
            (0x80, 6, 0x0100) => {
                let (product, configurations) = match self.what {
                    What::Keyboard => (0x0001, 1),
                    What::Mouse => (0x0002, 1),
                    What::Net(_) => (0x0003, 2),
                };
                let mut d = std::vec![18, 1, 0x00, 0x02, 0, 0, 0, 8, 0x34, 0x12];
                d.extend_from_slice(&[product as u8, 0, 0, 1, 0, 0, 0, configurations]);
                d
            }
            (0x80, 6, 0x0200) => self.config(),
            (0x80, 6, 0x0201) if net => self.config(),
            (0x80, 6, 0x0303) if net => {
                assert_eq!(self.setup[1] & 0xffff, 0x0409, "the string's language");
                let mut d = std::vec![2 + 2 * MAC_STRING.len() as u8, 3];
                for c in MAC_STRING.encode_utf16() {
                    d.extend_from_slice(&c.to_le_bytes());
                }
                d
            }
            // GET_NTB_PARAMETERS: 16 KiB blocks in, 2 KiB out, datagrams
            // out aligned to 4.
            (0xa1, 0x80, 0) if matches!(self.what, What::Net(cdc::Model::Ncm)) => {
                let mut d = std::vec![0u8; 28];
                d[0..2].copy_from_slice(&28u16.to_le_bytes());
                d[2..4].copy_from_slice(&1u16.to_le_bytes());
                d[4..8].copy_from_slice(&16384u32.to_le_bytes());
                d[8..10].copy_from_slice(&4u16.to_le_bytes());
                d[16..20].copy_from_slice(&2048u32.to_le_bytes());
                d[20..22].copy_from_slice(&4u16.to_le_bytes());
                d[26..28].copy_from_slice(&0u16.to_le_bytes());
                d
            }
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
                let net = matches!(m.device(slot).what, What::Net(_));
                match value {
                    1 => m.control(&self.phys, slot),
                    t if net && t == u32::from(NET_OUT_DCI) => m.bulk_out(&self.phys, slot),
                    t if net => assert_eq!(t, u32::from(NET_IN_DCI), "a doorbell for DCI {t}"),
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

    /// The adapter on `port` completes its oldest bulk IN transfer with
    /// `data` (or fails it with `cc`).
    fn receive_cc(&self, port: u8, data: &[u8], cc: u32) {
        let mut m = self.m.borrow_mut();
        let d = m.devices.iter_mut().find(|d| d.port == port).unwrap();
        let (at, trb) = d.bulk_in.next(&self.phys).expect("no bulk IN transfer queued");
        assert_eq!(trb_type(&trb), TRB_NORMAL);
        let len = (trb[2] & 0x1ffff) as usize;
        assert!(data.len() <= len, "a frame longer than the transfer");
        self.phys.borrow_mut().write(param(&trb), data);
        let cc = if cc == 1 && data.len() < len { 13 } else { cc };
        let slot = d.slot;
        m.event(
            &self.phys,
            [
                at as u32,
                (at >> 32) as u32,
                cc << 24 | (len - data.len()) as u32,
                TRB_TRANSFER_EVENT << 10 | u32::from(NET_IN_DCI) << 16 | u32::from(slot) << 24,
            ],
        );
    }

    fn receive(&self, port: u8, data: &[u8]) {
        self.receive_cc(port, data, 1);
    }

    /// The adapter on `port` stops or resumes reading its bulk OUT
    /// transfers (and reads those queued meanwhile).
    fn pause_out(&self, port: u8, paused: bool) {
        let mut m = self.m.borrow_mut();
        let d = m.devices.iter_mut().find(|d| d.port == port).unwrap();
        d.out_paused = paused;
        let slot = d.slot;
        if !paused {
            m.bulk_out(&self.phys, slot);
        }
    }

    fn device<T>(&self, port: u8, f: impl FnOnce(&FakeDevice) -> T) -> T {
        let m = self.m.borrow();
        f(m.devices.iter().find(|d| d.port == port).unwrap())
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

fn net(model: cdc::Model) -> FakeDevice {
    FakeDevice::new(What::Net(model), 1)
}

/// A frame of `len` bytes whose bytes count from `seed`.
fn frame(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

fn receive(x: &mut Xhci<Fake>) -> Option<Vec<u8>> {
    let mut buf = [0u8; 2048];
    x.net_receive(&mut buf).map(|n| buf[..n].to_vec())
}

/// An NTB16 the way a device may build one: the datagrams first, each
/// aligned to four, then one NDP16 listing them.
fn ntb(frames: &[&[u8]]) -> Vec<u8> {
    let mut b = std::vec![0u8; 12];
    let mut entries = Vec::new();
    for f in frames {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        entries.push((b.len() as u16, f.len() as u16));
        b.extend_from_slice(f);
    }
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let ndp = b.len();
    // At least one entry and the terminator (NCM 1.0 table 3-3).
    let ndp_len = 8 + 4 * (entries.len() + 1).max(2);
    b.extend_from_slice(&cdc::NDP16_SIGNATURE.to_le_bytes());
    b.extend_from_slice(&(ndp_len as u16).to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    for (index, len) in entries {
        b.extend_from_slice(&index.to_le_bytes());
        b.extend_from_slice(&len.to_le_bytes());
    }
    b.resize(ndp + ndp_len, 0);
    let total = b.len() as u16;
    b[0..4].copy_from_slice(&cdc::NTH16_SIGNATURE.to_le_bytes());
    b[4..6].copy_from_slice(&12u16.to_le_bytes());
    b[8..10].copy_from_slice(&total.to_le_bytes());
    b[10..12].copy_from_slice(&(ndp as u16).to_le_bytes());
    b
}

#[test]
fn an_ecm_adapter_is_set_up_in_its_cdc_configuration() {
    let (x, bus) = start(std::vec![net(cdc::Model::Ecm)]);
    assert_eq!(x.devices().collect::<Vec<_>>(), [(1, "CDC-ECM")]);
    assert_eq!(x.net_mac(), Some((MAC, cdc::Model::Ecm)));
    bus.device(1, |d| {
        // SET_CONFIGURATION 2, SET_INTERFACE 1 of interface 1, the
        // packet filter (directed, broadcast, multicast).
        assert_eq!(d.requests, [(0x00, 9, 2), (0x01, 11, 1), (0x21, 0x43, 0x0e)]);
    });
    assert_eq!(x.strays(), 0);
}

#[test]
fn ecm_frames_are_received_round_the_ring() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ecm)]);
    assert_eq!(receive(&mut x), None);
    // One at a time, past the slots: each is queued again once read.
    for i in 0..3 * NET_SLOTS {
        let f = frame(60 + i, i as u8);
        bus.receive(1, &f);
        assert_eq!(receive(&mut x), Some(f));
    }
    // A slot's worth at once: read in order.
    let frames: Vec<_> = (0..NET_SLOTS).map(|i| frame(1514, 7 * i as u8)).collect();
    for f in &frames {
        bus.receive(1, f);
    }
    for f in &frames {
        assert_eq!(receive(&mut x).as_ref(), Some(f));
    }
    assert_eq!(receive(&mut x), None);
    assert_eq!((x.stats.errors, x.strays()), (0, 0));
}

#[test]
fn ecm_frames_are_sent_with_a_zero_length_packet_when_whole() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ecm)]);
    let (a, b) = (frame(60, 1), frame(2 * NET_MPS, 2));
    assert!(x.net_send(&a));
    assert!(x.net_send(&b));
    bus.device(1, |d| {
        assert_eq!(d.sent, [a.clone(), b.clone()]);
        assert_eq!(d.out_trbs, [60, 2 * NET_MPS, 0]);
    });
    // Too long for a slot: dropped.
    assert!(!x.net_send(&frame(NET_SLOT + 1, 0)));
}

#[test]
fn sends_stop_while_every_slot_is_in_flight() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ecm)]);
    bus.pause_out(1, true);
    for i in 0..NET_SLOTS {
        assert!(x.net_send(&frame(100, i as u8)), "frame {i}");
    }
    assert!(!x.net_send(&frame(100, 0xff)));
    bus.pause_out(1, false);
    assert!(x.net_send(&frame(100, 0xee)));
    bus.device(1, |d| {
        assert_eq!(d.sent.len(), NET_SLOTS + 1);
        assert_eq!(d.sent[NET_SLOTS], frame(100, 0xee));
    });
}

#[test]
fn an_ncm_adapter_sets_its_block_size() {
    let (x, bus) = start(std::vec![net(cdc::Model::Ncm)]);
    assert_eq!(x.devices().collect::<Vec<_>>(), [(1, "CDC-NCM")]);
    assert_eq!(x.net_mac(), Some((MAC, cdc::Model::Ncm)));
    bus.device(1, |d| {
        assert_eq!(d.requests, [(0x00, 9, 2), (0x21, 0x86, 0), (0x01, 11, 1), (0x21, 0x43, 0x0e)]);
        // SET_NTB_INPUT_SIZE: no larger than a slot.
        assert_eq!(d.data_out, [(NET_SLOT as u32).to_le_bytes().to_vec()]);
    });
}

#[test]
fn ncm_blocks_are_read_a_datagram_at_a_time() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ncm)]);
    let (a, b, c) = (frame(60, 1), frame(99, 2), frame(1514, 3));
    bus.receive(1, &ntb(&[&a, &b]));
    bus.receive(1, &ntb(&[]));
    bus.receive(1, &ntb(&[&c]));
    assert_eq!(receive(&mut x), Some(a));
    assert_eq!(receive(&mut x), Some(b));
    // The empty block is passed over.
    assert_eq!(receive(&mut x), Some(c));
    assert_eq!(receive(&mut x), None);
    // Blocks keep coming round the slots.
    for i in 0..2 * NET_SLOTS {
        let f = frame(70, i as u8);
        bus.receive(1, &ntb(&[&f]));
        assert_eq!(receive(&mut x), Some(f));
    }
    assert_eq!(x.stats.errors, 0);
}

#[test]
fn a_bad_ncm_block_is_counted_and_passed_over() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ncm)]);
    let mut bad = ntb(&[&frame(60, 1)]);
    // A datagram beyond the block's end.
    let ndp = usize::from(u16::from_le_bytes([bad[10], bad[11]]));
    bad[ndp + 10..ndp + 12].copy_from_slice(&4000u16.to_le_bytes());
    bus.receive(1, &bad);
    let good = frame(80, 9);
    bus.receive(1, &ntb(&[&good]));
    assert_eq!(receive(&mut x), Some(good));
    assert_eq!(x.stats.errors, 1);
}

#[test]
fn ncm_frames_are_sent_in_blocks() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ncm)]);
    let (a, b) = (frame(60, 1), frame(1514, 2));
    assert!(x.net_send(&a));
    assert!(x.net_send(&b));
    bus.device(1, |d| {
        assert_eq!(d.sent.len(), 2);
        for (block, want) in d.sent.iter().zip([&a, &b]) {
            let mut got = Vec::new();
            assert_eq!(cdc::ntb16_frames(block, |f| got.push(f.to_vec())), Ok(1));
            assert_eq!(got, core::slice::from_ref(want));
        }
        // The sequence numbers count up.
        assert_eq!(&d.sent[0][6..8], &[0, 0]);
        assert_eq!(&d.sent[1][6..8], &[1, 0]);
    });
}

#[test]
fn a_failed_bulk_transfer_stops_using_the_adapter() {
    let (mut x, bus) = start(std::vec![net(cdc::Model::Ecm)]);
    bus.receive_cc(1, &[], 4);
    assert_eq!(receive(&mut x), None);
    assert_eq!(x.stats.errors, 1);
    assert_eq!(x.net_mac(), None);
    assert!(!x.net_send(&frame(60, 0)));
    bus.device(1, |d| assert!(d.sent.is_empty()));
}

#[test]
fn a_stalled_packet_filter_leaves_the_default() {
    let mut d = net(cdc::Model::Ecm);
    d.filter_stalls = true;
    let (mut x, bus) = start(std::vec![d]);
    assert_eq!(x.net_mac(), Some((MAC, cdc::Model::Ecm)));
    let commands = bus.m.borrow().commands.clone();
    assert!(commands.ends_with(&[TRB_RESET_ENDPOINT, TRB_SET_TR_DEQUEUE]), "{commands:?}");
    bus.device(1, |d| assert_eq!(d.requests, [(0x00, 9, 2), (0x01, 11, 1)]));
    // Endpoint 0 and the bulk endpoints still work.
    let f = frame(64, 5);
    bus.receive(1, &f);
    assert_eq!(receive(&mut x), Some(f));
}

#[test]
fn a_keyboard_and_an_adapter_together() {
    let mut devices = std::vec![FakeDevice::new(What::Keyboard, 1)];
    devices.push(FakeDevice::new(What::Net(cdc::Model::Ecm), 2));
    let (mut x, bus) = start(devices);
    assert_eq!(x.devices().collect::<Vec<_>>(), [(1, "keyboard"), (2, "CDC-ECM")]);
    bus.receive(2, &frame(60, 1));
    bus.report(1, &[0, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(poll(&mut x), [hid::Event::Key { usage: 0x04, down: true }]);
    assert_eq!(receive(&mut x), Some(frame(60, 1)));
    assert!(x.net_send(&frame(60, 2)));
    bus.device(2, |d| assert_eq!(d.sent, [frame(60, 2)]));
    assert_eq!(x.strays(), 0);
}

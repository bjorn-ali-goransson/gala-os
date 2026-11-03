//! The driver against a fake virtio-gpu: a host-side model of the device
//! (resources, backing, scanouts, the cursor, display events, resets) that
//! decodes the driver's requests and answers them as the specification
//! says, reading "guest memory" the fake transport hands out.

use super::proto::{self, Rect};
use super::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// Guest memory: chunk base address -> bytes.
type Phys = Rc<RefCell<BTreeMap<u64, Vec<u8>>>>;

struct FakeMemory {
    phys: Phys,
    chunks: Vec<(u64, u32)>,
    len: usize,
}

impl Memory for FakeMemory {
    fn len(&self) -> usize {
        self.len
    }
    fn chunks(&self) -> usize {
        self.chunks.len()
    }
    fn chunk(&self, index: usize) -> (u64, u32) {
        self.chunks[index]
    }
    fn write(&mut self, offset: usize, pixels: &[u32]) {
        let mut phys = self.phys.borrow_mut();
        let mut at = offset;
        for p in pixels {
            for (i, b) in p.to_le_bytes().iter().enumerate() {
                let (mut o, mut c) = (at + i, 0);
                while o >= self.chunks[c].1 as usize {
                    o -= self.chunks[c].1 as usize;
                    c += 1;
                }
                phys.get_mut(&self.chunks[c].0).unwrap()[o] = *b;
            }
            at += 4;
        }
    }
}

impl Drop for FakeMemory {
    fn drop(&mut self) {
        let mut phys = self.phys.borrow_mut();
        for (addr, _) in &self.chunks {
            phys.remove(addr);
        }
    }
}

#[derive(Default)]
struct Resource {
    format: u32,
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    backing: Vec<(u64, u32)>,
}

/// The device's state.
#[derive(Default)]
struct Device {
    phys: Phys,
    modes: Vec<(u32, u32, bool)>,
    events: u32,
    features: u64,
    resources: HashMap<u32, Resource>,
    scanouts: [u32; proto::MAX_SCANOUTS],
    /// What each scanout shows after the last flush.
    shown: HashMap<u32, Vec<u32>>,
    cursor: Option<(u32, u32, u32, (u32, u32))>,
    cursor_moves: Vec<(u32, u32)>,
    commands: Vec<u32>,
    /// Reset under the driver: commands fail until reinit.
    reset: bool,
    reinits: u32,
    /// Answer this command with this error once.
    fail: Option<(u32, u32)>,
    next_addr: u64,
}

impl Device {
    fn read_phys(&self, backing: &[(u64, u32)], offset: usize, out: &mut [u8]) {
        let phys = self.phys.borrow();
        for (i, b) in out.iter_mut().enumerate() {
            let (mut o, mut c) = (offset + i, 0);
            while o >= backing[c].1 as usize {
                o -= backing[c].1 as usize;
                c += 1;
            }
            // Reading freed memory would be the bug the field order and
            // DETACH_BACKING prevent.
            *b = phys.get(&backing[c].0).expect("device read freed memory")[o];
        }
    }

    fn handle(&mut self, req: &[u8], resp: &mut [u8]) -> usize {
        let kind = proto::le32(req, 0);
        self.commands.push(kind);
        let mut answer = |t: u32| {
            resp[..proto::HEADER].fill(0);
            resp[..4].copy_from_slice(&t.to_le_bytes());
            proto::HEADER
        };
        if let Some((k, err)) = self.fail
            && k == kind
        {
            self.fail = None;
            return answer(err);
        }
        let a = |i: usize| proto::le32(req, proto::HEADER + 4 * i);
        match kind {
            proto::CMD_GET_DISPLAY_INFO => {
                answer(proto::RESP_OK_DISPLAY_INFO);
                for (i, &(w, h, on)) in self.modes.iter().enumerate() {
                    let at = proto::HEADER + 24 * i;
                    for (j, v) in [0, 0, w, h, u32::from(on), 0].iter().enumerate() {
                        resp[at + 4 * j..at + 4 * j + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
                proto::DISPLAY_INFO_SIZE
            }
            proto::CMD_GET_EDID => {
                answer(proto::RESP_OK_EDID);
                let e = proto::tests::sample_edid();
                resp[24..28].copy_from_slice(&128u32.to_le_bytes());
                resp[32..160].copy_from_slice(&e);
                proto::EDID_RESPONSE_SIZE
            }
            proto::CMD_RESOURCE_CREATE_2D => {
                let (id, format, w, h) = (a(0), a(1), a(2), a(3));
                if id == 0 || self.resources.contains_key(&id) {
                    return answer(proto::RESP_ERR_INVALID_RESOURCE_ID);
                }
                let pixels = vec![0; (w * h) as usize];
                let r = Resource { format, width: w, height: h, pixels, backing: Vec::new() };
                self.resources.insert(id, r);
                answer(proto::RESP_OK_NODATA)
            }
            proto::CMD_RESOURCE_ATTACH_BACKING => {
                let (id, n) = (a(0), a(1) as usize);
                let Some(r) = self.resources.get_mut(&id) else {
                    return answer(proto::RESP_ERR_INVALID_RESOURCE_ID);
                };
                r.backing = (0..n)
                    .map(|i| {
                        let at = proto::HEADER + 8 + 16 * i;
                        (proto::le64(req, at), proto::le32(req, at + 8))
                    })
                    .collect();
                answer(proto::RESP_OK_NODATA)
            }
            proto::CMD_RESOURCE_DETACH_BACKING => match self.resources.get_mut(&a(0)) {
                Some(r) => {
                    r.backing.clear();
                    answer(proto::RESP_OK_NODATA)
                }
                None => answer(proto::RESP_ERR_INVALID_RESOURCE_ID),
            },
            proto::CMD_RESOURCE_UNREF => match self.resources.remove(&a(0)) {
                Some(_) => answer(proto::RESP_OK_NODATA),
                None => answer(proto::RESP_ERR_INVALID_RESOURCE_ID),
            },
            proto::CMD_SET_SCANOUT => {
                let (scanout, id) = (a(4), a(5));
                if scanout as usize >= self.modes.len() {
                    return answer(proto::RESP_ERR_INVALID_SCANOUT_ID);
                }
                if id != 0 && !self.resources.contains_key(&id) {
                    return answer(proto::RESP_ERR_INVALID_RESOURCE_ID);
                }
                self.scanouts[scanout as usize] = id;
                answer(proto::RESP_OK_NODATA)
            }
            proto::CMD_TRANSFER_TO_HOST_2D => {
                let r = proto::rect_at(req, proto::HEADER);
                let offset = proto::le64(req, proto::HEADER + 16) as usize;
                let id = proto::le32(req, proto::HEADER + 24);
                let Some(res) = self.resources.get(&id) else {
                    return answer(proto::RESP_ERR_INVALID_RESOURCE_ID);
                };
                if res.backing.is_empty() || r.clip(res.width, res.height) != Some(r) {
                    return answer(proto::RESP_ERR_INVALID_PARAMETER);
                }
                let (stride, backing) = (res.width as usize * 4, res.backing.clone());
                let mut rows = Vec::new();
                for j in 0..r.height as usize {
                    let mut row = vec![0u8; r.width as usize * 4];
                    self.read_phys(&backing, offset + j * stride, &mut row);
                    rows.push(row);
                }
                let res = self.resources.get_mut(&id).unwrap();
                for (j, row) in rows.iter().enumerate() {
                    for (i, px) in row.chunks(4).enumerate() {
                        let at = (r.y as usize + j) * res.width as usize + r.x as usize + i;
                        res.pixels[at] = u32::from_le_bytes(px.try_into().unwrap());
                    }
                }
                answer(proto::RESP_OK_NODATA)
            }
            proto::CMD_RESOURCE_FLUSH => {
                let id = proto::le32(req, proto::HEADER + 16);
                let Some(res) = self.resources.get(&id) else {
                    return answer(proto::RESP_ERR_INVALID_RESOURCE_ID);
                };
                for (s, &on) in self.scanouts.iter().enumerate() {
                    if on == id {
                        self.shown.insert(s as u32, res.pixels.clone());
                    }
                }
                answer(proto::RESP_OK_NODATA)
            }
            _ => answer(proto::RESP_ERR_UNSPEC),
        }
    }
}

#[derive(Clone)]
struct Fake {
    dev: Rc<RefCell<Device>>,
    /// Bytes per memory chunk (to make the backing several chunks).
    chunk: usize,
    max_control: usize,
}

impl Fake {
    fn new(modes: &[(u32, u32, bool)]) -> Self {
        let dev = Device {
            modes: modes.to_vec(),
            features: proto::F_EDID,
            next_addr: 0x10_0000,
            ..Device::default()
        };
        Self { dev: Rc::new(RefCell::new(dev)), chunk: 64 * 1024, max_control: 4096 }
    }

    fn shown(&self, scanout: u32) -> Vec<u32> {
        self.dev.borrow().shown.get(&scanout).cloned().unwrap_or_default()
    }

    fn commands(&self) -> Vec<&'static str> {
        self.dev.borrow().commands.iter().map(|&k| proto::name(k)).collect()
    }

    fn clear_commands(&self) {
        self.dev.borrow_mut().commands.clear();
    }

    /// Bytes of guest memory still allocated.
    fn allocated(&self) -> usize {
        self.dev.borrow().phys.borrow().values().map(Vec::len).sum()
    }
}

impl Transport for Fake {
    type Memory = FakeMemory;
    fn features(&self) -> u64 {
        self.dev.borrow().features
    }
    fn control(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, TransportError> {
        let mut dev = self.dev.borrow_mut();
        if dev.reset {
            return Err(TransportError::DeviceReset);
        }
        if req.len() + resp.len() > self.max_control {
            return Err(TransportError::TooLarge);
        }
        Ok(dev.handle(req, resp))
    }
    fn cursor(&mut self, req: &[u8]) -> Result<(), TransportError> {
        let mut dev = self.dev.borrow_mut();
        if dev.reset {
            return Err(TransportError::DeviceReset);
        }
        let kind = proto::le32(req, 0);
        dev.commands.push(kind);
        let a = |i: usize| proto::le32(req, proto::HEADER + 4 * i);
        match kind {
            proto::CMD_UPDATE_CURSOR => dev.cursor = Some((a(1), a(2), a(4), (a(5), a(6)))),
            proto::CMD_MOVE_CURSOR => dev.cursor_moves.push((a(1), a(2))),
            _ => panic!("not a cursor command"),
        }
        Ok(())
    }
    fn max_control(&self) -> usize {
        self.max_control
    }
    fn needs_reset(&self) -> bool {
        self.dev.borrow().reset
    }
    fn reinit(&mut self) -> Result<(), TransportError> {
        let mut dev = self.dev.borrow_mut();
        dev.reset = false;
        dev.reinits += 1;
        dev.resources.clear();
        dev.scanouts = [0; proto::MAX_SCANOUTS];
        dev.cursor = None;
        Ok(())
    }
    fn config_u32(&self, offset: u16) -> u32 {
        match offset {
            proto::CONFIG_EVENTS_READ => self.dev.borrow().events,
            proto::CONFIG_NUM_SCANOUTS => self.dev.borrow().modes.len() as u32,
            _ => 0,
        }
    }
    fn set_config_u32(&mut self, offset: u16, value: u32) {
        if offset == proto::CONFIG_EVENTS_CLEAR {
            self.dev.borrow_mut().events &= !value;
        }
    }
    fn alloc(&mut self, bytes: usize) -> Option<FakeMemory> {
        let mut dev = self.dev.borrow_mut();
        let phys = dev.phys.clone();
        let mut chunks = Vec::new();
        let mut left = bytes;
        while left > 0 {
            let n = left.min(self.chunk);
            let addr = dev.next_addr;
            dev.next_addr += n as u64 + 0x1000;
            phys.borrow_mut().insert(addr, vec![0; n]);
            chunks.push((addr, n as u32));
            left -= n;
        }
        Some(FakeMemory { phys, chunks, len: bytes })
    }
}

fn started(modes: &[(u32, u32, bool)]) -> (Fake, Gpu<Fake>) {
    let fake = Fake::new(modes);
    let gpu = Gpu::start(fake.clone(), false).expect("start");
    (fake, gpu)
}

#[test]
fn start_shows_the_preferred_mode() {
    let (fake, gpu) = started(&[(0, 0, false), (1280, 800, true)]);
    assert_eq!(gpu.size(), Some((1280, 800)));
    assert_eq!(
        fake.commands(),
        [
            "GET_DISPLAY_INFO",
            "GET_EDID",
            "RESOURCE_CREATE_2D",
            "RESOURCE_ATTACH_BACKING",
            "SET_SCANOUT",
            "TRANSFER_TO_HOST_2D",
            "RESOURCE_FLUSH",
        ]
    );
    let dev = fake.dev.borrow();
    let id = dev.scanouts[1];
    assert_ne!(id, 0);
    let r = &dev.resources[&id];
    assert_eq!((r.format, r.width, r.height), (proto::FORMAT_B8G8R8X8_UNORM, 1280, 800));
    // 4000 KiB in 64 KiB chunks.
    assert_eq!(r.backing.len(), 63);
    assert_eq!(dev.shown[&1].len(), 1280 * 800);
    let edid = gpu.edid.expect("EDID read");
    assert_eq!((&edid.manufacturer, edid.preferred), (b"QEM", Some((1280, 800))));
}

#[test]
fn no_edid_without_the_feature_and_a_fallback_mode() {
    let fake = Fake::new(&[(0, 0, false)]);
    fake.dev.borrow_mut().features = 0;
    let gpu = Gpu::start(fake.clone(), false).unwrap();
    assert!(gpu.edid.is_none());
    assert!(!fake.commands().contains(&"GET_EDID"));
    assert_eq!(gpu.size(), Some(FALLBACK_MODE));
    assert_ne!(fake.dev.borrow().scanouts[0], 0);
}

#[test]
fn rows_reach_the_screen_on_flush() {
    let (fake, mut gpu) = started(&[(100, 50, true)]);
    // A row, and one clipped at the right edge.
    gpu.write_row(10, 3, &[0x00ff_0000, 0x0000_ff00, 0x0000_00ff]);
    gpu.write_row(98, 49, &[1, 2, 3, 4]);
    assert_eq!(fake.shown(0)[3 * 100 + 10], 0, "not shown before a flush");
    gpu.flush(Rect::new(10, 3, 3, 1)).unwrap();
    let shown = fake.shown(0);
    assert_eq!(&shown[3 * 100 + 10..3 * 100 + 13], &[0x00ff_0000, 0x0000_ff00, 0x0000_00ff]);
    assert_eq!(shown[49 * 100 + 98], 0, "outside the flushed area");
    // A flush past the edge is clipped.
    gpu.flush(Rect::new(90, 40, 50, 50)).unwrap();
    assert_eq!(&fake.shown(0)[49 * 100 + 98..49 * 100 + 100], &[1, 2]);
    gpu.flush(Rect::new(500, 0, 1, 1)).unwrap();
    assert_eq!(gpu.stats.errors, 0);
}

#[test]
fn rows_across_chunks() {
    let mut fake = Fake::new(&[(300, 20, true)]);
    fake.chunk = 4096; // a 1200-byte row crosses chunks often
    let mut gpu = Gpu::start(fake.clone(), false).unwrap();
    let row: Vec<u32> = (0..300).map(|i| 0x10000 + i).collect();
    for y in 0..20 {
        gpu.write_row(0, y, &row);
    }
    gpu.flush(Rect::new(0, 0, 300, 20)).unwrap();
    let shown = fake.shown(0);
    for y in 0..20 {
        assert_eq!(&shown[y * 300..y * 300 + 300], &row[..], "row {y}");
    }
}

#[test]
fn a_display_change_switches_the_mode_and_frees_the_old_screen() {
    let (fake, mut gpu) = started(&[(1280, 800, true)]);
    let old = fake.dev.borrow().scanouts[0];
    assert_eq!(gpu.poll(), Ok(None), "no event");
    let before = fake.allocated();
    {
        let mut dev = fake.dev.borrow_mut();
        dev.modes[0] = (1024, 768, true);
        dev.events = proto::EVENT_DISPLAY;
    }
    fake.clear_commands();
    assert_eq!(gpu.poll(), Ok(Some((1024, 768))));
    assert_eq!(gpu.size(), Some((1024, 768)));
    assert_eq!(fake.dev.borrow().events, 0, "event cleared");
    assert_eq!(
        fake.commands(),
        [
            "GET_DISPLAY_INFO",
            "RESOURCE_CREATE_2D",
            "RESOURCE_ATTACH_BACKING",
            "SET_SCANOUT",
            "TRANSFER_TO_HOST_2D",
            "RESOURCE_FLUSH",
            "RESOURCE_DETACH_BACKING",
            "RESOURCE_UNREF",
        ]
    );
    let dev = fake.dev.borrow();
    assert!(!dev.resources.contains_key(&old));
    assert_ne!(dev.scanouts[0], old);
    drop(dev);
    assert_eq!(fake.allocated(), before - 1280 * 800 * 4 + 1024 * 768 * 4);
    // The same size again: nothing to do.
    fake.dev.borrow_mut().events = proto::EVENT_DISPLAY;
    assert_eq!(gpu.poll(), Ok(None));
    assert_eq!(gpu.stats.mode_changes, 2);
}

#[test]
fn a_display_turned_off_keeps_the_screen() {
    let (fake, mut gpu) = started(&[(800, 600, true)]);
    {
        let mut dev = fake.dev.borrow_mut();
        dev.modes[0] = (0, 0, false);
        dev.events = proto::EVENT_DISPLAY;
    }
    // Nothing usable: scanout 0 at the fallback size (shasha keeps a
    // screen to draw on).
    assert_eq!(gpu.poll(), Ok(Some(FALLBACK_MODE)));
}

#[test]
fn recovery_after_a_device_reset_keeps_the_picture() {
    let (fake, mut gpu) = started(&[(64, 32, true)]);
    gpu.write_row(0, 0, &[0x123456; 64]);
    gpu.flush(Rect::new(0, 0, 64, 1)).unwrap();
    // The device is reset under the driver: it forgets everything.
    {
        let mut dev = fake.dev.borrow_mut();
        dev.reset = true;
        dev.resources.clear();
        dev.shown.clear();
    }
    gpu.write_row(0, 1, &[0x654321; 64]);
    gpu.flush(Rect::new(0, 1, 64, 1)).unwrap();
    assert_eq!(gpu.stats.resets, 1);
    assert_eq!(fake.dev.borrow().reinits, 1);
    let shown = fake.shown(0);
    assert_eq!(shown[0], 0x123456, "the earlier picture is back");
    assert_eq!(shown[64], 0x654321, "and the new row");
}

#[test]
fn recovery_from_poll() {
    let (fake, mut gpu) = started(&[(64, 32, true)]);
    fake.dev.borrow_mut().reset = true;
    assert_eq!(gpu.poll(), Ok(None));
    assert_eq!(gpu.stats.resets, 1);
    assert_ne!(fake.dev.borrow().scanouts[0], 0);
}

#[test]
fn the_hardware_cursor() {
    let (fake, mut gpu) = started(&[(640, 480, true)]);
    gpu.move_cursor(5, 5).unwrap();
    assert!(fake.dev.borrow().cursor_moves.is_empty(), "no cursor yet: nothing sent");
    let image = vec![0xff00_ff00u32; 64 * 64];
    gpu.set_cursor(Some((&image, (3, 4)))).unwrap();
    let (x, y, id, hot) = fake.dev.borrow().cursor.unwrap();
    assert_eq!((x, y, hot), (5, 5, (3, 4)));
    {
        let dev = fake.dev.borrow();
        let r = &dev.resources[&id];
        assert_eq!((r.format, r.width, r.height), (proto::FORMAT_B8G8R8A8_UNORM, 64, 64));
        assert_eq!(r.pixels[64 * 64 - 1], 0xff00_ff00, "uploaded");
    }
    gpu.move_cursor(100, 200).unwrap();
    assert_eq!(fake.dev.borrow().cursor_moves, [(100, 200)]);
    // A reset brings the cursor back.
    fake.dev.borrow_mut().reset = true;
    gpu.poll().unwrap();
    let (x, y, id2, _) = fake.dev.borrow().cursor.unwrap();
    assert_eq!((x, y), (100, 200));
    assert!(fake.dev.borrow().resources.contains_key(&id2));
    // Hidden: resource 0, the image released.
    gpu.set_cursor(None).unwrap();
    assert_eq!(fake.dev.borrow().cursor.unwrap().2, 0);
    assert!(!fake.dev.borrow().resources.contains_key(&id2));
}

#[test]
fn device_errors_are_reported_and_counted() {
    let (fake, mut gpu) = started(&[(320, 200, true)]);
    fake.dev.borrow_mut().fail =
        Some((proto::CMD_RESOURCE_CREATE_2D, proto::RESP_ERR_OUT_OF_MEMORY));
    assert_eq!(gpu.set_mode(0, 640, 480), Err(Error::Device(proto::DeviceError::OutOfMemory)));
    assert_eq!(gpu.size(), Some((320, 200)), "the old screen stays");
    assert_eq!(gpu.stats.errors, 1);
    assert_eq!(
        gpu.stats.last_error,
        Some((proto::CMD_RESOURCE_CREATE_2D, Error::Device(proto::DeviceError::OutOfMemory)))
    );
    // A failed SET_SCANOUT: the new resource is released, its memory freed.
    let before = fake.allocated();
    fake.dev.borrow_mut().fail = Some((proto::CMD_SET_SCANOUT, proto::RESP_ERR_INVALID_SCANOUT_ID));
    assert!(gpu.set_mode(0, 640, 480).is_err());
    assert_eq!(fake.allocated(), before);
    assert_eq!(fake.dev.borrow().resources.len(), 1);
    assert_eq!(gpu.set_mode(0, 0, 480), Err(Error::Device(proto::DeviceError::InvalidParameter)));
}

#[test]
fn a_backing_in_too_many_chunks_is_refused() {
    let mut fake = Fake::new(&[(320, 200, true)]);
    fake.chunk = 4096;
    fake.max_control = 1024; // (1024 - 56) / 16 = 60 chunks at most
    assert_eq!(Gpu::start(fake.clone(), false).err(), Some(Error::TooManyChunks));
    // Nothing is left allocated.
    assert_eq!(fake.allocated(), 0);
}

#[test]
fn the_screen_memory_outlives_the_device() {
    // Dropping the driver drops the transport (the kernel's resets the
    // device) before the memory the device read.
    let (fake, gpu) = started(&[(32, 32, true)]);
    assert!(fake.allocated() > 0);
    drop(gpu);
    assert_eq!(fake.allocated(), 0);
}

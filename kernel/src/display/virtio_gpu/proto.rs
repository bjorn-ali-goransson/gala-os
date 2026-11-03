//! The virtio-gpu wire protocol as plain data (OASIS *Virtual I/O Device
//! (VIRTIO) Version 1.x*, "GPU Device"; see THIRD_PARTY.md): requests are
//! encoded into byte buffers and responses decoded from them, so all of it
//! is testable on the host. Every field is little-endian.
//!
//! A request starts with the 24-byte control header (type, flags,
//! fence_id, ctx_id, ring_idx, padding); a response starts with the same
//! header whose type says OK_* or ERR_*.

/// Commands (`virtio_gpu_ctrl_type`).
pub const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const CMD_SET_SCANOUT: u32 = 0x0103;
pub const CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
pub const CMD_GET_EDID: u32 = 0x010a;
pub const CMD_UPDATE_CURSOR: u32 = 0x0300;
pub const CMD_MOVE_CURSOR: u32 = 0x0301;

/// Responses.
pub const RESP_OK_NODATA: u32 = 0x1100;
pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
pub const RESP_OK_EDID: u32 = 0x1104;
pub const RESP_ERR_UNSPEC: u32 = 0x1200;
pub const RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
pub const RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
pub const RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
pub const RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
pub const RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;

/// Feature bits (device-specific, the low 24).
pub const F_VIRGL: u64 = 1 << 0;
pub const F_EDID: u64 = 1 << 1;
pub const F_RESOURCE_UUID: u64 = 1 << 2;
pub const F_RESOURCE_BLOB: u64 = 1 << 3;
pub const F_CONTEXT_INIT: u64 = 1 << 4;

/// The device configuration: events the device raised (read), events to
/// clear (write), scanouts, capability sets.
pub const CONFIG_EVENTS_READ: u16 = 0;
pub const CONFIG_EVENTS_CLEAR: u16 = 4;
pub const CONFIG_NUM_SCANOUTS: u16 = 8;
pub const CONFIG_NUM_CAPSETS: u16 = 12;
/// The display configuration changed: ask GET_DISPLAY_INFO again.
pub const EVENT_DISPLAY: u32 = 1;

/// `VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM`: bytes B, G, R, X in memory, i.e. a
/// little-endian `0x00RRGGBB` u32: the desktop's pixels as they are.
pub const FORMAT_B8G8R8X8_UNORM: u32 = 2;
/// `VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM`: little-endian `0xAARRGGBB` (the
/// cursor, which has alpha).
pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;

pub const MAX_SCANOUTS: usize = 16;
/// Cursor images are 64 x 64.
pub const CURSOR_SIZE: u32 = 64;

pub const HEADER: usize = 24;
pub const DISPLAY_INFO_SIZE: usize = HEADER + MAX_SCANOUTS * 24;
pub const EDID_RESPONSE_SIZE: usize = HEADER + 8 + 1024;
/// One `virtio_gpu_mem_entry`: address (u64), length (u32), padding.
pub const MEM_ENTRY: usize = 16;

/// The size of an ATTACH_BACKING request with `entries` entries.
pub const fn attach_backing_size(entries: usize) -> usize {
    HEADER + 8 + entries * MEM_ENTRY
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    /// The part of `self` inside a `w` x `h` surface (None: nothing).
    pub fn clip(self, w: u32, h: u32) -> Option<Rect> {
        let x2 = self.x.saturating_add(self.width).min(w);
        let y2 = self.y.saturating_add(self.height).min(h);
        (self.x < x2 && self.y < y2).then(|| Rect::new(self.x, self.y, x2 - self.x, y2 - self.y))
    }
}

/// A request being encoded (fixed-size buffer; `len` bytes used).
#[derive(Clone)]
pub struct Request {
    pub bytes: [u8; Self::MAX],
    pub len: usize,
}

impl Request {
    /// Large enough for every request but ATTACH_BACKING with its entries,
    /// which `attach_backing` writes into the caller's buffer.
    pub const MAX: usize = 64;

    fn new(kind: u32) -> Self {
        let mut r = Self { bytes: [0; Self::MAX], len: HEADER };
        r.bytes[..4].copy_from_slice(&kind.to_le_bytes());
        r
    }

    fn u32(mut self, v: u32) -> Self {
        self.bytes[self.len..self.len + 4].copy_from_slice(&v.to_le_bytes());
        self.len += 4;
        self
    }

    fn u64(mut self, v: u64) -> Self {
        self.bytes[self.len..self.len + 8].copy_from_slice(&v.to_le_bytes());
        self.len += 8;
        self
    }

    fn rect(self, r: Rect) -> Self {
        self.u32(r.x).u32(r.y).u32(r.width).u32(r.height)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The command type (for logs and fakes).
    pub fn kind(&self) -> u32 {
        le32(&self.bytes, 0)
    }
}

pub fn get_display_info() -> Request {
    Request::new(CMD_GET_DISPLAY_INFO)
}

pub fn get_edid(scanout: u32) -> Request {
    Request::new(CMD_GET_EDID).u32(scanout).u32(0)
}

pub fn resource_create_2d(resource: u32, format: u32, width: u32, height: u32) -> Request {
    Request::new(CMD_RESOURCE_CREATE_2D).u32(resource).u32(format).u32(width).u32(height)
}

pub fn resource_unref(resource: u32) -> Request {
    Request::new(CMD_RESOURCE_UNREF).u32(resource).u32(0)
}

pub fn resource_detach_backing(resource: u32) -> Request {
    Request::new(CMD_RESOURCE_DETACH_BACKING).u32(resource).u32(0)
}

/// SET_SCANOUT: `resource` (0: disable the scanout) shown on `scanout`,
/// its area `r`.
pub fn set_scanout(scanout: u32, resource: u32, r: Rect) -> Request {
    Request::new(CMD_SET_SCANOUT).rect(r).u32(scanout).u32(resource)
}

pub fn resource_flush(resource: u32, r: Rect) -> Request {
    Request::new(CMD_RESOURCE_FLUSH).rect(r).u32(resource).u32(0)
}

/// TRANSFER_TO_HOST_2D: area `r` of `resource` from its backing, which
/// holds the area's first pixel at byte `offset`.
pub fn transfer_to_host_2d(resource: u32, r: Rect, offset: u64) -> Request {
    Request::new(CMD_TRANSFER_TO_HOST_2D).rect(r).u64(offset).u32(resource).u32(0)
}

/// UPDATE_CURSOR (`update`: a new image from `resource`, hot spot `hot`;
/// resource 0 hides the cursor) or MOVE_CURSOR (only the position) on
/// scanout `scanout`.
pub fn cursor(
    update: bool,
    scanout: u32,
    x: u32,
    y: u32,
    resource: u32,
    hot: (u32, u32),
) -> Request {
    let kind = if update { CMD_UPDATE_CURSOR } else { CMD_MOVE_CURSOR };
    Request::new(kind).u32(scanout).u32(x).u32(y).u32(0).u32(resource).u32(hot.0).u32(hot.1).u32(0)
}

/// RESOURCE_ATTACH_BACKING into `out`: `resource`'s backing is the
/// guest-physical `entries` (address, length). The request's length, or
/// None if `out` is too small.
pub fn attach_backing(
    out: &mut [u8],
    resource: u32,
    entries: impl ExactSizeIterator<Item = (u64, u32)>,
) -> Option<usize> {
    let len = attach_backing_size(entries.len());
    let out = out.get_mut(..len)?;
    out.fill(0);
    out[..4].copy_from_slice(&CMD_RESOURCE_ATTACH_BACKING.to_le_bytes());
    out[HEADER..HEADER + 4].copy_from_slice(&resource.to_le_bytes());
    out[HEADER + 4..HEADER + 8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    for (i, (addr, size)) in entries.enumerate() {
        let at = HEADER + 8 + i * MEM_ENTRY;
        out[at..at + 8].copy_from_slice(&addr.to_le_bytes());
        out[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
    }
    Some(len)
}

/// The little-endian u32 at `at` (0 past the end).
pub fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn le64(b: &[u8], at: usize) -> u64 {
    u64::from(le32(b, at)) | u64::from(le32(b, at + 4)) << 32
}

pub fn rect_at(b: &[u8], at: usize) -> Rect {
    Rect::new(le32(b, at), le32(b, at + 4), le32(b, at + 8), le32(b, at + 12))
}

/// A response's type (0 if it is shorter than a header).
pub fn response_type(resp: &[u8]) -> u32 {
    if resp.len() < HEADER { 0 } else { le32(resp, 0) }
}

/// What the device said about a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// Fewer bytes than a header came back.
    Short,
    /// A response of this type when another was expected.
    Unexpected(u32),
    Unspec,
    OutOfMemory,
    InvalidScanout,
    InvalidResource,
    InvalidContext,
    InvalidParameter,
}

/// Checks that `resp` is of type `want`.
pub fn check(resp: &[u8], want: u32) -> Result<(), DeviceError> {
    match response_type(resp) {
        0 => Err(DeviceError::Short),
        t if t == want => Ok(()),
        RESP_ERR_UNSPEC => Err(DeviceError::Unspec),
        RESP_ERR_OUT_OF_MEMORY => Err(DeviceError::OutOfMemory),
        RESP_ERR_INVALID_SCANOUT_ID => Err(DeviceError::InvalidScanout),
        RESP_ERR_INVALID_RESOURCE_ID => Err(DeviceError::InvalidResource),
        RESP_ERR_INVALID_CONTEXT_ID => Err(DeviceError::InvalidContext),
        RESP_ERR_INVALID_PARAMETER => Err(DeviceError::InvalidParameter),
        t => Err(DeviceError::Unexpected(t)),
    }
}

/// One scanout of GET_DISPLAY_INFO's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub rect: Rect,
    pub enabled: bool,
    pub flags: u32,
}

/// GET_DISPLAY_INFO's answer: the host's preferred mode per scanout.
pub fn display_info(resp: &[u8]) -> Result<[Mode; MAX_SCANOUTS], DeviceError> {
    check(resp, RESP_OK_DISPLAY_INFO)?;
    if resp.len() < DISPLAY_INFO_SIZE {
        return Err(DeviceError::Short);
    }
    let mut modes = [Mode::default(); MAX_SCANOUTS];
    for (i, m) in modes.iter_mut().enumerate() {
        let at = HEADER + 24 * i;
        *m = Mode {
            rect: rect_at(resp, at),
            enabled: le32(resp, at + 16) != 0,
            flags: le32(resp, at + 20),
        };
    }
    Ok(modes)
}

/// GET_EDID's answer: the EDID bytes.
pub fn edid(resp: &[u8]) -> Result<&[u8], DeviceError> {
    check(resp, RESP_OK_EDID)?;
    let size = (le32(resp, HEADER) as usize).min(1024);
    resp.get(HEADER + 8..HEADER + 8 + size).ok_or(DeviceError::Short)
}

/// What gala reads from an EDID block (VESA E-EDID 1.3/1.4 base block):
/// the manufacturer's three letters, the product code, and the first
/// detailed timing's active size (the preferred mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdidInfo {
    pub manufacturer: [u8; 3],
    pub product: u16,
    pub preferred: Option<(u32, u32)>,
}

pub fn parse_edid(e: &[u8]) -> Option<EdidInfo> {
    const MAGIC: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
    if e.len() < 128 || e[..8] != MAGIC {
        return None;
    }
    // Bytes 8-9: three 5-bit letters (1 = 'A'), big-endian.
    let id = u16::from_be_bytes([e[8], e[9]]);
    let letter = |shift: u16| b'A' - 1 + ((id >> shift) & 0x1f) as u8;
    let manufacturer = [letter(10), letter(5), letter(0)];
    let product = u16::from_le_bytes([e[10], e[11]]);
    // The first 18-byte descriptor (at 54) is a detailed timing when its
    // pixel clock is not 0.
    let d = &e[54..72];
    let preferred = (d[0] != 0 || d[1] != 0).then(|| {
        let w = u32::from(d[2]) | u32::from(d[4] >> 4) << 8;
        let h = u32::from(d[5]) | u32::from(d[7] >> 4) << 8;
        (w, h)
    });
    Some(EdidInfo { manufacturer, product, preferred })
}

/// A request's name in logs and traces.
pub fn name(kind: u32) -> &'static str {
    match kind {
        CMD_GET_DISPLAY_INFO => "GET_DISPLAY_INFO",
        CMD_RESOURCE_CREATE_2D => "RESOURCE_CREATE_2D",
        CMD_RESOURCE_UNREF => "RESOURCE_UNREF",
        CMD_SET_SCANOUT => "SET_SCANOUT",
        CMD_RESOURCE_FLUSH => "RESOURCE_FLUSH",
        CMD_TRANSFER_TO_HOST_2D => "TRANSFER_TO_HOST_2D",
        CMD_RESOURCE_ATTACH_BACKING => "RESOURCE_ATTACH_BACKING",
        CMD_RESOURCE_DETACH_BACKING => "RESOURCE_DETACH_BACKING",
        CMD_GET_EDID => "GET_EDID",
        CMD_UPDATE_CURSOR => "UPDATE_CURSOR",
        CMD_MOVE_CURSOR => "MOVE_CURSOR",
        _ => "?",
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn header_and_sizes() {
        // Sizes from the specification's structs.
        assert_eq!(get_display_info().len, 24);
        assert_eq!(resource_create_2d(1, 2, 3, 4).len, 40);
        assert_eq!(resource_unref(1).len, 32);
        assert_eq!(set_scanout(0, 1, Rect::default()).len, 48);
        assert_eq!(resource_flush(1, Rect::default()).len, 48);
        assert_eq!(transfer_to_host_2d(1, Rect::default(), 0).len, 56);
        assert_eq!(cursor(true, 0, 0, 0, 0, (0, 0)).len, 56);
        assert_eq!(get_edid(0).len, 32);
        assert_eq!(DISPLAY_INFO_SIZE, 408);
        assert_eq!(attach_backing_size(2), 64);
    }

    #[test]
    fn encodings() {
        let r = resource_create_2d(7, FORMAT_B8G8R8X8_UNORM, 1280, 800);
        let b = r.as_bytes();
        assert_eq!(&b[..4], &[0x01, 0x01, 0, 0]);
        assert!(b[4..24].iter().all(|&x| x == 0));
        assert_eq!((le32(b, 24), le32(b, 28), le32(b, 32), le32(b, 36)), (7, 2, 1280, 800));

        let r = set_scanout(1, 7, Rect::new(0, 0, 1280, 800));
        let b = r.as_bytes();
        assert_eq!(r.kind(), CMD_SET_SCANOUT);
        assert_eq!((le32(b, 32), le32(b, 36), le32(b, 40), le32(b, 44)), (1280, 800, 1, 7));

        let r = transfer_to_host_2d(7, Rect::new(10, 20, 30, 40), 0x1_0000_0004);
        let b = r.as_bytes();
        assert_eq!((le32(b, 24), le32(b, 28), le32(b, 32), le32(b, 36)), (10, 20, 30, 40));
        assert_eq!(le64(b, 40), 0x1_0000_0004);
        assert_eq!(le32(b, 48), 7);

        let r = cursor(false, 0, 5, 6, 9, (1, 2));
        let b = r.as_bytes();
        assert_eq!(r.kind(), CMD_MOVE_CURSOR);
        assert_eq!((le32(b, 24), le32(b, 28), le32(b, 32)), (0, 5, 6));
        assert_eq!((le32(b, 40), le32(b, 44), le32(b, 48)), (9, 1, 2));

        let mut out = [0xaau8; 80];
        let n = attach_backing(&mut out, 3, [(0x1000, 4096), (0x8000, 8192)].into_iter());
        assert_eq!(n, Some(64));
        assert_eq!(le32(&out, 0), CMD_RESOURCE_ATTACH_BACKING);
        assert_eq!((le32(&out, 24), le32(&out, 28)), (3, 2));
        assert_eq!((le64(&out, 32), le32(&out, 40), le32(&out, 44)), (0x1000, 4096, 0));
        assert_eq!((le64(&out, 48), le32(&out, 56)), (0x8000, 8192));
        assert_eq!(out[64], 0xaa);
        assert_eq!(attach_backing(&mut out[..63], 3, [(0, 0), (0, 0)].into_iter()), None);
    }

    #[test]
    fn responses() {
        let mut resp = [0u8; DISPLAY_INFO_SIZE];
        resp[..4].copy_from_slice(&RESP_OK_DISPLAY_INFO.to_le_bytes());
        let at = HEADER + 24;
        for (i, v) in [0u32, 0, 1920, 1080, 1, 0].iter().enumerate() {
            resp[at + 4 * i..at + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        let modes = display_info(&resp).unwrap();
        assert!(!modes[0].enabled);
        assert_eq!(modes[1], Mode { rect: Rect::new(0, 0, 1920, 1080), enabled: true, flags: 0 });
        assert_eq!(display_info(&resp[..100]), Err(DeviceError::Short));

        let mut err = [0u8; HEADER];
        err[..4].copy_from_slice(&RESP_ERR_INVALID_RESOURCE_ID.to_le_bytes());
        assert_eq!(check(&err, RESP_OK_NODATA), Err(DeviceError::InvalidResource));
        assert_eq!(check(&err[..10], RESP_OK_NODATA), Err(DeviceError::Short));
        err[..4].copy_from_slice(&RESP_OK_NODATA.to_le_bytes());
        assert_eq!(check(&err, RESP_OK_NODATA), Ok(()));
        assert_eq!(check(&err, RESP_OK_EDID), Err(DeviceError::Unexpected(RESP_OK_NODATA)));
    }

    /// A minimal EDID base block: "QEM", product 0x1234, 1280 x 800.
    pub fn sample_edid() -> [u8; 128] {
        let mut e = [0u8; 128];
        e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        // Q=17, E=5, M=13: 0b0_10001_00101_01101.
        let id: u16 = 17 << 10 | 5 << 5 | 13;
        e[8..10].copy_from_slice(&id.to_be_bytes());
        e[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
        // Detailed timing: pixel clock 71.00 MHz, 1280 (0x500) x 800 (0x320).
        e[54] = 0xbc;
        e[55] = 0x1b;
        e[56] = 0x00;
        e[58] = 0x50;
        e[59] = 0x20;
        e[61] = 0x30;
        e
    }

    #[test]
    fn edid_parsing() {
        let e = sample_edid();
        let info = parse_edid(&e).unwrap();
        assert_eq!(&info.manufacturer, b"QEM");
        assert_eq!(info.product, 0x1234);
        assert_eq!(info.preferred, Some((1280, 800)));
        assert_eq!(parse_edid(&e[..100]), None);
        let mut bad = e;
        bad[0] = 1;
        assert_eq!(parse_edid(&bad), None);

        let mut resp = [0u8; EDID_RESPONSE_SIZE];
        resp[..4].copy_from_slice(&RESP_OK_EDID.to_le_bytes());
        resp[HEADER..HEADER + 4].copy_from_slice(&128u32.to_le_bytes());
        resp[HEADER + 8..HEADER + 8 + 128].copy_from_slice(&e);
        assert_eq!(edid(&resp).unwrap(), &e[..]);
    }

    #[test]
    fn clipping() {
        assert_eq!(Rect::new(10, 10, 100, 100).clip(50, 60), Some(Rect::new(10, 10, 40, 50)));
        assert_eq!(Rect::new(60, 0, 10, 10).clip(50, 60), None);
        assert_eq!(Rect::new(0, 0, 0, 10).clip(50, 60), None);
        assert_eq!(Rect::new(u32::MAX, 0, 10, 10).clip(50, 60), None);
    }
}

//! USB communications-class Ethernet: what a CDC-ECM or CDC-NCM function
//! looks like in a configuration descriptor (CDC 1.2 ch. 5, ECM 1.2 ch. 5,
//! NCM 1.0 ch. 5), its MAC address string, and NCM's 16-bit transfer
//! blocks (NCM 1.0 ch. 3). The controller side (the bulk endpoints, the
//! requests) is `xhci`'s. ARCHITECTURE "USB".

/// The two Ethernet control models gala drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Ethernet Control Model: one frame per bulk transfer.
    Ecm,
    /// Network Control Model: frames packed in transfer blocks (NTB16).
    Ncm,
}

impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Model::Ecm => "CDC-ECM",
            Model::Ncm => "CDC-NCM",
        }
    }
}

/// A bulk endpoint: its address and largest packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bulk {
    pub address: u8,
    pub mps: u16,
}

/// What a configuration says of its Ethernet function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub configuration: u8,
    pub model: Model,
    /// The communications interface (the class requests go to it).
    pub comm: u8,
    /// The data interface and the alternate setting with the endpoints.
    pub data: u8,
    pub data_alt: u8,
    pub bulk_in: Bulk,
    pub bulk_out: Bulk,
    /// The string descriptor holding the MAC address (iMACAddress).
    pub mac_string: u8,
    /// The largest Ethernet frame the function carries (wMaxSegmentSize).
    pub max_segment: u16,
}

const DESC_CONFIGURATION: u8 = 2;
const DESC_INTERFACE: u8 = 4;
const DESC_ENDPOINT: u8 = 5;
const DESC_CS_INTERFACE: u8 = 0x24;

const CLASS_COMM: u8 = 0x02;
const CLASS_DATA: u8 = 0x0a;
const SUBCLASS_ECM: u8 = 0x06;
const SUBCLASS_NCM: u8 = 0x0d;

const CS_UNION: u8 = 0x06;
const CS_ETHERNET: u8 = 0x0f;

/// Finds the Ethernet function in one configuration descriptor (its
/// whole `wTotalLength`): a communications interface of the ECM or NCM
/// subclass, its union naming the data interface, the Ethernet
/// functional descriptor, and the data interface's alternate setting
/// with a bulk IN and a bulk OUT endpoint. None if any part is missing.
pub fn find(desc: &[u8]) -> Option<Function> {
    let mut configuration = None;
    let mut model = None;
    let mut comm = None;
    let mut data = None;
    let mut mac_string = None;
    let mut max_segment = 0;
    // The interface the following descriptors belong to: (number, alt,
    // class).
    let mut current: Option<(u8, u8, u8)> = None;
    let (mut bulk_in, mut bulk_out) = (None, None);
    let mut data_alt = None;
    let mut at = 0;
    while at + 2 <= desc.len() {
        let len = desc[at] as usize;
        if len < 2 || at + len > desc.len() {
            break;
        }
        let d = &desc[at..at + len];
        match d[1] {
            DESC_CONFIGURATION if len >= 9 => configuration = Some(d[5]),
            DESC_INTERFACE if len >= 9 => {
                let (number, alt, class, subclass) = (d[2], d[3], d[5], d[6]);
                current = Some((number, alt, class));
                if class == CLASS_COMM && comm.is_none() {
                    model = match subclass {
                        SUBCLASS_ECM => Some(Model::Ecm),
                        SUBCLASS_NCM => Some(Model::Ncm),
                        _ => None,
                    };
                    if model.is_some() {
                        comm = Some(number);
                    }
                }
                // A new alternate setting of the data interface: its
                // endpoints are counted afresh.
                if class == CLASS_DATA && Some(number) == data && data_alt.is_none() {
                    bulk_in = None;
                    bulk_out = None;
                }
            }
            DESC_CS_INTERFACE if len >= 3 => {
                let in_comm = matches!(current, Some((n, _, _)) if Some(n) == comm);
                match d[2] {
                    CS_UNION if in_comm && len >= 5 => data = Some(d[4]),
                    CS_ETHERNET if in_comm && len >= 13 => {
                        mac_string = Some(d[3]);
                        max_segment = u16::from_le_bytes([d[8], d[9]]);
                    }
                    _ => {}
                }
            }
            DESC_ENDPOINT if len >= 7 => {
                let Some((number, alt, class)) = current else {
                    at += len;
                    continue;
                };
                if class == CLASS_DATA
                    && Some(number) == data
                    && data_alt.is_none()
                    && d[3] & 3 == 2
                {
                    let bulk =
                        Bulk { address: d[2], mps: u16::from_le_bytes([d[4], d[5]]) & 0x7ff };
                    if d[2] & 0x80 != 0 {
                        bulk_in.get_or_insert(bulk);
                    } else {
                        bulk_out.get_or_insert(bulk);
                    }
                    if bulk_in.is_some() && bulk_out.is_some() {
                        data_alt = Some(alt);
                    }
                }
            }
            _ => {}
        }
        at += len;
    }
    Some(Function {
        configuration: configuration?,
        model: model?,
        comm: comm?,
        data: data?,
        data_alt: data_alt?,
        bulk_in: bulk_in?,
        bulk_out: bulk_out?,
        mac_string: mac_string?,
        max_segment: if max_segment == 0 { 1514 } else { max_segment },
    })
}

/// The MAC address out of its string descriptor: twelve hexadecimal
/// digits in UTF-16 (ECM 1.2 table 3, iMACAddress).
pub fn parse_mac(string: &[u8]) -> Option<[u8; 6]> {
    if string.len() < 2 + 24 || string[1] != 3 {
        return None;
    }
    let mut mac = [0u8; 6];
    for i in 0..12 {
        let unit = u16::from_le_bytes([string[2 + 2 * i], string[3 + 2 * i]]);
        let digit = char::from_u32(unit as u32)?.to_digit(16)? as u8;
        mac[i / 2] = mac[i / 2] << 4 | digit;
    }
    Some(mac)
}

pub const NTH16_SIGNATURE: u32 = 0x484d_434e; // "NCMH"
pub const NDP16_SIGNATURE: u32 = 0x304d_434e; // "NCM0", no CRC
const NTH16_LEN: usize = 12;
/// The NDP16 gala writes: its header and two entries (the frame, the
/// terminator).
const NDP16_LEN: usize = 8 + 2 * 4;

/// Why a received transfer block was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtbError {
    Header,
    Length,
    Pointer,
    Datagram,
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Walks a received NTB16 (`block`, as long as the transfer was) and
/// hands each datagram to `f`; the datagrams found. Every length and
/// index is checked against the block: a device's error loses the
/// block, never reads beyond it.
pub fn ntb16_frames(block: &[u8], mut f: impl FnMut(&[u8])) -> Result<usize, NtbError> {
    if u32_at(block, 0) != Some(NTH16_SIGNATURE) || u16_at(block, 4) != Some(NTH16_LEN) {
        return Err(NtbError::Header);
    }
    let block_len = u16_at(block, 8).ok_or(NtbError::Header)?;
    if block_len > block.len() || block_len < NTH16_LEN {
        return Err(NtbError::Length);
    }
    let block = &block[..block_len];
    let mut ndp = u16_at(block, 10).ok_or(NtbError::Header)?;
    let mut n = 0;
    // NDPs chain; a bound keeps a looping chain finite.
    for _ in 0..16 {
        if ndp == 0 {
            return Ok(n);
        }
        if ndp < NTH16_LEN || ndp % 4 != 0 || u32_at(block, ndp) != Some(NDP16_SIGNATURE) {
            return Err(NtbError::Pointer);
        }
        let ndp_len = u16_at(block, ndp + 4).ok_or(NtbError::Pointer)?;
        if ndp_len < 16 || ndp_len % 4 != 0 || ndp + ndp_len > block.len() {
            return Err(NtbError::Pointer);
        }
        let next = u16_at(block, ndp + 6).ok_or(NtbError::Pointer)?;
        let mut entry = ndp + 8;
        while entry + 4 <= ndp + ndp_len {
            let (index, len) =
                (u16_at(block, entry).unwrap_or(0), u16_at(block, entry + 2).unwrap_or(0));
            if index == 0 || len == 0 {
                break;
            }
            let frame = block.get(index..index + len).ok_or(NtbError::Datagram)?;
            f(frame);
            n += 1;
            entry += 4;
        }
        ndp = next;
    }
    Err(NtbError::Pointer)
}

/// Builds an NTB16 carrying one frame into `out`: the header, the
/// frame at offset 12, then the NDP16 aligned to four; the block's
/// length, or None if it does not fit.
pub fn ntb16_build(frame: &[u8], sequence: u16, out: &mut [u8]) -> Option<usize> {
    let ndp = (NTH16_LEN + frame.len()).next_multiple_of(4);
    let total = ndp + NDP16_LEN;
    if total > out.len() || total > u16::MAX as usize {
        return None;
    }
    let out = &mut out[..total];
    out.fill(0);
    out[0..4].copy_from_slice(&NTH16_SIGNATURE.to_le_bytes());
    out[4..6].copy_from_slice(&(NTH16_LEN as u16).to_le_bytes());
    out[6..8].copy_from_slice(&sequence.to_le_bytes());
    out[8..10].copy_from_slice(&(total as u16).to_le_bytes());
    out[10..12].copy_from_slice(&(ndp as u16).to_le_bytes());
    out[NTH16_LEN..NTH16_LEN + frame.len()].copy_from_slice(frame);
    out[ndp..ndp + 4].copy_from_slice(&NDP16_SIGNATURE.to_le_bytes());
    out[ndp + 4..ndp + 6].copy_from_slice(&(NDP16_LEN as u16).to_le_bytes());
    // wNextNdpIndex stays 0.
    out[ndp + 8..ndp + 10].copy_from_slice(&(NTH16_LEN as u16).to_le_bytes());
    out[ndp + 10..ndp + 12].copy_from_slice(&(frame.len() as u16).to_le_bytes());
    Some(total)
}

#[cfg(test)]
mod tests;

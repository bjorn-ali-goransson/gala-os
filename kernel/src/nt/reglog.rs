//! The registry's change log (D3): what programs change in the registry,
//! as records keyed by the key's full name (`\REGISTRY\...`), kept in the
//! file `C:\Windows\System32\config\gala.reglog`, which the disk keeps
//! like any file on C: (D2, `nt::persist`). Replayed after registry.seed
//! and the boot's own keys; volatile keys are never recorded.
//!
//! A record: magic `GREG`, payload length (u32), kind (u8, 3 reserved
//! bytes), CRC-32 of the header (CRC field zero) and payload, then the
//! payload, padded to 8 bytes; a record that does not check out ends the
//! log. Names are u32 length + UTF-16 units; data u32 length + bytes.

use crate::fs::log::crc32;
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegOp {
    /// The key exists (made with the keys above it if missing); `reset`
    /// (snapshots) drops the values it had.
    CreateKey {
        key: Vec<u16>,
        class: Vec<u16>,
        last_write: u64,
        reset: bool,
    },
    SetValue {
        key: Vec<u16>,
        name: Vec<u16>,
        kind: u32,
        data: Vec<u8>,
    },
    DeleteValue {
        key: Vec<u16>,
        name: Vec<u16>,
    },
    /// The key goes, with everything below it.
    DeleteKey {
        key: Vec<u16>,
    },
    SetSecurity {
        key: Vec<u16>,
        descriptor: Vec<u8>,
    },
}

const MAGIC: u32 = u32::from_le_bytes(*b"GREG");
const HEADER: usize = 16;
/// Values hold at most about a megabyte on Windows; records a bit more.
const MAX_PAYLOAD: usize = 2 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMemory;

fn put(out: &mut Vec<u8>, b: &[u8]) -> Result<(), NoMemory> {
    out.try_extend_from_slice(b).map_err(|_| NoMemory)
}

fn put_name(out: &mut Vec<u8>, name: &[u16]) -> Result<(), NoMemory> {
    put(out, &(name.len() as u32).to_le_bytes())?;
    for u in name {
        put(out, &u.to_le_bytes())?;
    }
    Ok(())
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) -> Result<(), NoMemory> {
    put(out, &(b.len() as u32).to_le_bytes())?;
    put(out, b)
}

impl RegOp {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), NoMemory> {
        let start = out.len();
        put(out, &[0; HEADER])?;
        let kind = match self {
            RegOp::CreateKey { key, class, last_write, reset } => {
                put_name(out, key)?;
                put_name(out, class)?;
                put(out, &last_write.to_le_bytes())?;
                put(out, &[u8::from(*reset)])?;
                1
            }
            RegOp::SetValue { key, name, kind, data } => {
                put_name(out, key)?;
                put_name(out, name)?;
                put(out, &kind.to_le_bytes())?;
                put_bytes(out, data)?;
                2
            }
            RegOp::DeleteValue { key, name } => {
                put_name(out, key)?;
                put_name(out, name)?;
                3
            }
            RegOp::DeleteKey { key } => {
                put_name(out, key)?;
                4
            }
            RegOp::SetSecurity { key, descriptor } => {
                put_name(out, key)?;
                put_bytes(out, descriptor)?;
                5
            }
        };
        let len = out.len() - start - HEADER;
        out[start..start + 4].copy_from_slice(&MAGIC.to_le_bytes());
        out[start + 4..start + 8].copy_from_slice(&(len as u32).to_le_bytes());
        out[start + 8] = kind;
        let crc = crc32(0, &out[start..]);
        out[start + 12..start + 16].copy_from_slice(&crc.to_le_bytes());
        while !(out.len() - start).is_multiple_of(8) {
            put(out, &[0])?;
        }
        Ok(())
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn name(&mut self) -> Option<Result<Vec<u16>, NoMemory>> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(2)?)?;
        let mut v = Vec::new();
        if v.try_reserve_exact(n).is_err() {
            return Some(Err(NoMemory));
        }
        v.extend(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
        Some(Ok(v))
    }
    fn bytes(&mut self) -> Option<Result<Vec<u8>, NoMemory>> {
        let n = self.u32()? as usize;
        let raw = self.take(n)?;
        let mut v = Vec::new();
        Some(v.try_extend_from_slice(raw).map(|()| v).map_err(|_| NoMemory))
    }
}

/// Decodes all of `bytes` up to the first record that does not check out.
pub fn decode_all(bytes: &[u8]) -> Result<Vec<RegOp>, NoMemory> {
    let mut ops = Vec::new();
    let mut at = 0;
    while let Some(rest) = bytes.get(at..) {
        if rest.len() < HEADER {
            break;
        }
        let field = |i: usize| u32::from_le_bytes([rest[i], rest[i + 1], rest[i + 2], rest[i + 3]]);
        let len = field(4) as usize;
        if field(0) != MAGIC || len > MAX_PAYLOAD || rest.len() < HEADER + len {
            break;
        }
        let mut header = [0u8; HEADER];
        header.copy_from_slice(&rest[..HEADER]);
        header[12..16].fill(0);
        if crc32(crc32(0, &header), &rest[HEADER..HEADER + len]) != field(12) {
            break;
        }
        let mut r = Reader { b: &rest[HEADER..HEADER + len], at: 0 };
        let Some(op) = parse(rest[8], &mut r) else { break };
        ops.try_push(op?).map_err(|_| NoMemory)?;
        at += (HEADER + len).next_multiple_of(8);
    }
    Ok(ops)
}

fn parse(kind: u8, r: &mut Reader<'_>) -> Option<Result<RegOp, NoMemory>> {
    let key = match r.name()? {
        Ok(k) => k,
        Err(e) => return Some(Err(e)),
    };
    Some(Ok(match kind {
        1 => {
            let class = match r.name()? {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            let last_write = r.u64()?;
            let reset = r.take(1)?[0] != 0;
            RegOp::CreateKey { key, class, last_write, reset }
        }
        2 => {
            let name = match r.name()? {
                Ok(n) => n,
                Err(e) => return Some(Err(e)),
            };
            let kind = r.u32()?;
            match r.bytes()? {
                Ok(data) => RegOp::SetValue { key, name, kind, data },
                Err(e) => return Some(Err(e)),
            }
        }
        3 => match r.name()? {
            Ok(name) => RegOp::DeleteValue { key, name },
            Err(e) => return Some(Err(e)),
        },
        4 => RegOp::DeleteKey { key },
        5 => match r.bytes()? {
            Ok(descriptor) => RegOp::SetSecurity { key, descriptor },
            Err(e) => return Some(Err(e)),
        },
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn registry_records_round_trip_and_end_at_damage() {
        let ops = vec![
            RegOp::CreateKey {
                key: w("\\REGISTRY\\USER\\x"),
                class: w("c"),
                last_write: 3,
                reset: true,
            },
            RegOp::SetValue {
                key: w("\\REGISTRY\\USER\\x"),
                name: w("v"),
                kind: 4,
                data: vec![1, 0, 0, 0],
            },
            RegOp::DeleteValue { key: w("\\REGISTRY\\USER\\x"), name: w("v") },
            RegOp::SetSecurity { key: w("\\REGISTRY\\USER\\x"), descriptor: vec![1, 0, 4, 0x80] },
            RegOp::DeleteKey { key: w("\\REGISTRY\\USER\\x") },
        ];
        let mut bytes = Vec::new();
        for op in &ops {
            op.encode(&mut bytes).unwrap();
        }
        assert_eq!(decode_all(&bytes).unwrap(), ops);
        // A torn last record is dropped, nothing before it.
        let cut = bytes.len() - 10; // past the padding, into the payload
        assert_eq!(decode_all(&bytes[..cut]).unwrap(), ops[..4]);
        bytes[HEADER + 1] ^= 1;
        assert_eq!(decode_all(&bytes).unwrap(), vec![]);
    }
}

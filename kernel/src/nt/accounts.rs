//! Local accounts and logons (docs/SECURITY.md, "Accounts and logon";
//! phase S3's minimal part): the boot user `gala` (RID 1001, empty
//! password) and the accounts under `\Registry\Machine\SAM\Gala\Users`
//! (one subkey per user name: `Rid` REG_DWORD, `NtHash` REG_BINARY, the
//! MD4 of the UTF-16LE password, the NT hash NTLM uses too). A logon
//! checks the password and makes a primary token in a new logon session
//! with the groups Windows gives that logon type.
//!
//! TODO: the account key is readable by programs (on Windows the SAM is
//! SYSTEM's); managing accounts (NetUserAdd); LsaLogonUser and S4U (S4).

use alloc::vec::Vec;

use super::registry;
use super::status::NtStatus;
use super::token::{self, Group, Sid, TokenData};
use crate::mm::heap::TryVec;

pub const LOGON_FAILURE: NtStatus = NtStatus(0xc000_006d);

// LOGON32_LOGON_* (LogonUserW's dwLogonType).
pub const INTERACTIVE: u32 = 2;
pub const NETWORK: u32 = 3;
pub const BATCH: u32 = 4;
pub const SERVICE: u32 = 5;
pub const NETWORK_CLEARTEXT: u32 = 8;

/// The machine GALA's domain sub-authorities (S-1-5-21-a-b-c).
const MACHINE: [u32; 4] = [21, 1_000_000_001, 1_000_000_002, 1_000_000_003];
const BOOT_USER_RID: u32 = 1001;

/// MD4 (RFC 1320).
pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut h: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut block = [0u8; 64];
    let process = |block: &[u8; 64], h: &mut [u32; 4]| {
        let mut x = [0u32; 16];
        for (i, w) in x.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        let [mut a, mut b, mut c, mut d] = *h;
        let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
        let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
        let hh = |x: u32, y: u32, z: u32| x ^ y ^ z;
        for k in 0..16 {
            let s = [3, 7, 11, 19][k % 4];
            let t = a.wrapping_add(f(b, c, d)).wrapping_add(x[k]).rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        const R2: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
        for k in 0..16 {
            let s = [3, 5, 9, 13][k % 4];
            let t = a
                .wrapping_add(g(b, c, d))
                .wrapping_add(x[R2[k]])
                .wrapping_add(0x5a82_7999)
                .rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        const R3: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
        for k in 0..16 {
            let s = [3, 9, 11, 15][k % 4];
            let t = a
                .wrapping_add(hh(b, c, d))
                .wrapping_add(x[R3[k]])
                .wrapping_add(0x6ed9_eba1)
                .rotate_left(s);
            (a, b, c, d) = (d, t, b, c);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
    };
    let mut chunks = data.chunks_exact(64);
    for chunk in &mut chunks {
        block.copy_from_slice(chunk);
        process(&block, &mut h);
    }
    let rest = chunks.remainder();
    block = [0; 64];
    block[..rest.len()].copy_from_slice(rest);
    block[rest.len()] = 0x80;
    if rest.len() >= 56 {
        process(&block, &mut h);
        block = [0; 64];
    }
    block[56..].copy_from_slice(&bits.to_le_bytes());
    process(&block, &mut h);
    let mut out = [0u8; 16];
    for (i, w) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// The NT hash of a UTF-16 password.
pub fn nt_hash(password: &[u16]) -> Result<[u8; 16], NtStatus> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(2 * password.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    for &c in password {
        bytes.try_extend_from_slice(&c.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(md4(&bytes))
}

fn same_ascii(a: &[u16], b: &str) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.bytes()).all(|(&c, b)| c < 128 && (c as u8).eq_ignore_ascii_case(&b))
}

/// The account `name`: (RID, NT hash).
fn account(name: &[u16]) -> Result<(u32, [u8; 16]), NtStatus> {
    if same_ascii(name, "gala") {
        return Ok((BOOT_USER_RID, nt_hash(&[])?));
    }
    if name.is_empty() || name.contains(&u16::from(b'\\')) {
        return Err(LOGON_FAILURE);
    }
    let mut path = Vec::new();
    for c in "\\Registry\\Machine\\SAM\\Gala\\Users\\".encode_utf16() {
        path.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    path.try_extend_from_slice(name).map_err(|_| NtStatus::NO_MEMORY)?;
    let rid = [u16::from(b'R'), u16::from(b'i'), u16::from(b'd')];
    let hash = b"NtHash".map(u16::from);
    registry::with(|r| {
        let relative = registry::Registry::absolute(&path)?;
        let id = r.lookup(registry::ROOT, relative).map_err(|_| LOGON_FAILURE)?;
        let rid = r.value(id, &rid).map_err(|_| LOGON_FAILURE)?;
        let hash = r.value(id, &hash).map_err(|_| LOGON_FAILURE)?;
        let rid = u32::from_le_bytes(
            rid.data.get(..4).and_then(|b| b.try_into().ok()).ok_or(LOGON_FAILURE)?,
        );
        let hash: [u8; 16] =
            hash.data.get(..16).and_then(|b| b.try_into().ok()).ok_or(LOGON_FAILURE)?;
        Ok((rid, hash))
    })
}

/// The SID of local account RID `rid`.
pub fn user_sid(rid: u32) -> Sid {
    Sid::new(5, &[MACHINE[0], MACHINE[1], MACHINE[2], MACHINE[3], rid])
}

/// LogonUserW's logon: the password checked (STATUS_LOGON_FAILURE for an
/// unknown user, a wrong password or another domain), a primary token
/// in a new logon session. With no password: an S4U logon (the caller
/// checked SeTcbPrivilege).
pub fn logon(
    name: &[u16],
    domain: &[u16],
    password: Option<&[u16]>,
    kind: u32,
) -> Result<TokenData, NtStatus> {
    let local = domain.is_empty() || same_ascii(domain, ".") || same_ascii(domain, "GALA");
    if !local {
        return Err(LOGON_FAILURE);
    }
    // The logon type's group: INTERACTIVE, NETWORK, BATCH or SERVICE.
    let type_sid = match kind {
        INTERACTIVE => Sid::new(5, &[4]),
        NETWORK | NETWORK_CLEARTEXT => Sid::new(5, &[2]),
        BATCH => Sid::new(5, &[3]),
        SERVICE => Sid::new(5, &[6]),
        _ => return Err(NtStatus::INVALID_PARAMETER),
    };
    let (rid, hash) = account(name)?;
    if let Some(password) = password
        && nt_hash(password)? != hash
    {
        return Err(LOGON_FAILURE);
    }
    // The standard user's groups, with the logon type's instead of
    // INTERACTIVE and without CONSOLE LOGON (not the console session's
    // logon); LOCAL only for logons on this machine's console or batch.
    let mut groups = Vec::new();
    for g in token::GROUPS {
        let console = g.sid == Sid::new(2, &[1]);
        let interactive = g.sid == Sid::new(5, &[4]);
        let local_group = g.sid == Sid::new(2, &[0]);
        if console || (local_group && matches!(kind, NETWORK | NETWORK_CLEARTEXT)) {
            continue;
        }
        let g = if interactive { Group { sid: type_sid, attributes: g.attributes } } else { g };
        groups.try_push(g).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    TokenData::standard_user(user_sid(rid), token::next_luid(), &groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> alloc::string::String {
        let digits = b"0123456789abcdef";
        let mut s = alloc::string::String::new();
        for x in b {
            s.push(char::from(digits[usize::from(x >> 4)]));
            s.push(char::from(digits[usize::from(x & 15)]));
        }
        s
    }

    #[test]
    fn md4_vectors() {
        // RFC 1320's test suite.
        assert_eq!(hex(&md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(&md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(hex(&md4(b"message digest")), "d9130a8164549fe818874806e1c7014b");
        assert_eq!(
            hex(&md4(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "e33b4ddc9c38f2199c3e7b164fcc0536"
        );
        let mut password = Vec::new();
        for c in "Gala-Test-1".encode_utf16() {
            password.try_push(c).unwrap();
        }
        assert_eq!(hex(&nt_hash(&password).unwrap()), "cf21ae340740f8506d56b0bfd82d7a8e");
    }
}

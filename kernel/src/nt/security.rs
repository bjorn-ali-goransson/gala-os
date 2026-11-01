//! Security descriptors kept on objects (docs/SECURITY.md, phase S2), as
//! documented in "Security Descriptors", "ACE Inheritance Rules" and
//! "Creating a Security Descriptor for a New Object" (Microsoft Learn):
//!
//! - Objects store a self-relative descriptor (files on their ramfs node,
//!   keys on their registry node, other objects on the KObject).
//! - At creation (`assign`): the owner and group are the creator's (the
//!   given ones, or the token's owner and primary group); the DACL is the
//!   given one (with the parent's inheritable ACEs after it when the
//!   object type uses automatic inheritance and it is not protected), else
//!   the parent's inheritable ACEs, else the token's default DACL.
//! - Inherited ACEs follow the documented rules for containers and
//!   objects (OI, CI, NP, IO), get INHERITED_ACE, and an effective ACE has
//!   generic rights mapped and CREATOR OWNER / CREATOR GROUP replaced
//!   (with an inherit-only copy of the original when it also propagates).
//! - `set` merges NtSetSecurityObject's parts; an owner must be the
//!   caller's user or one of its groups with SE_GROUP_OWNER.
//!
//! Nothing checks access against these yet (phase S3); handles still
//! carry their granted access (READ_CONTROL, WRITE_DAC, WRITE_OWNER are
//! checked by the services).

use alloc::vec::Vec;

use super::object::GenericMapping;
use super::status::NtStatus;
use super::token::{Sid, TokenData};
use crate::mm::heap::TryVec;

// SECURITY_DESCRIPTOR_CONTROL.
pub const SE_OWNER_DEFAULTED: u16 = 0x0001;
pub const SE_GROUP_DEFAULTED: u16 = 0x0002;
pub const SE_DACL_PRESENT: u16 = 0x0004;
pub const SE_DACL_DEFAULTED: u16 = 0x0008;
pub const SE_SACL_PRESENT: u16 = 0x0010;
pub const SE_SACL_DEFAULTED: u16 = 0x0020;
pub const SE_DACL_AUTO_INHERIT_REQ: u16 = 0x0100;
pub const SE_SACL_AUTO_INHERIT_REQ: u16 = 0x0200;
pub const SE_DACL_AUTO_INHERITED: u16 = 0x0400;
pub const SE_SACL_AUTO_INHERITED: u16 = 0x0800;
pub const SE_DACL_PROTECTED: u16 = 0x1000;
pub const SE_SACL_PROTECTED: u16 = 0x2000;
pub const SE_SELF_RELATIVE: u16 = 0x8000;

const DACL_BITS: u16 = SE_DACL_PRESENT
    | SE_DACL_DEFAULTED
    | SE_DACL_AUTO_INHERIT_REQ
    | SE_DACL_AUTO_INHERITED
    | SE_DACL_PROTECTED;
const SACL_BITS: u16 = SE_SACL_PRESENT
    | SE_SACL_DEFAULTED
    | SE_SACL_AUTO_INHERIT_REQ
    | SE_SACL_AUTO_INHERITED
    | SE_SACL_PROTECTED;

// SECURITY_INFORMATION.
pub const OWNER_SECURITY_INFORMATION: u32 = 0x1;
pub const GROUP_SECURITY_INFORMATION: u32 = 0x2;
pub const DACL_SECURITY_INFORMATION: u32 = 0x4;
pub const SACL_SECURITY_INFORMATION: u32 = 0x8;
pub const LABEL_SECURITY_INFORMATION: u32 = 0x10;
pub const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
pub const PROTECTED_SACL_SECURITY_INFORMATION: u32 = 0x4000_0000;
pub const UNPROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x2000_0000;
pub const UNPROTECTED_SACL_SECURITY_INFORMATION: u32 = 0x1000_0000;

// ACE flags.
pub const OBJECT_INHERIT_ACE: u8 = 0x01;
pub const CONTAINER_INHERIT_ACE: u8 = 0x02;
pub const NO_PROPAGATE_INHERIT_ACE: u8 = 0x04;
pub const INHERIT_ONLY_ACE: u8 = 0x08;
pub const INHERITED_ACE: u8 = 0x10;

const ACL_REVISION: u8 = 2;

/// CREATOR OWNER (S-1-3-0) and CREATOR GROUP (S-1-3-1).
const CREATOR_OWNER: Sid = Sid::new(3, &[0]);
const CREATOR_GROUP: Sid = Sid::new(3, &[1]);

const GENERIC_BITS: u32 = 0xf000_0000;

/// A descriptor's parts. `dacl`/`sacl` are ACL bytes; a present NULL DACL
/// is `SE_DACL_PRESENT` with `dacl` None.
#[derive(Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub control: u16,
    pub owner: Option<Sid>,
    pub group: Option<Sid>,
    pub dacl: Option<Vec<u8>>,
    pub sacl: Option<Vec<u8>>,
}

fn nomem<T>(_: T) -> NtStatus {
    NtStatus::NO_MEMORY
}

fn copy(b: &[u8]) -> Result<Vec<u8>, NtStatus> {
    let mut v = Vec::new();
    v.try_extend_from_slice(b).map_err(nomem)?;
    Ok(v)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

/// The ACL at the start of `b` if well formed (revision, size, ACE sizes):
/// its bytes (AclSize long).
pub fn valid_acl(b: &[u8]) -> Option<&[u8]> {
    let size = usize::from(u16_at(b, 2)?);
    let count = usize::from(u16_at(b, 4)?);
    if !(2..=4).contains(b.first()?) || size < 8 || size > b.len() {
        return None;
    }
    let mut at = 8;
    for _ in 0..count {
        let len = usize::from(u16_at(b, at + 2)?);
        if len < 4 || at + len > size {
            return None;
        }
        at += len;
    }
    Some(&b[..size])
}

/// One ACE of an ACL: (type, flags, its bytes).
fn aces(acl: &[u8]) -> impl Iterator<Item = (u8, u8, &[u8])> {
    let count = u16_at(acl, 4).unwrap_or(0);
    let mut at = 8usize;
    (0..count).map_while(move |_| {
        let len = usize::from(u16_at(acl, at + 2)?);
        let ace = acl.get(at..at + len)?;
        at += len;
        Some((ace[0], ace[1], ace))
    })
}

/// An ACL from ACE bytes.
fn build_acl(revision: u8, ace_bytes: &[u8], count: usize) -> Result<Vec<u8>, NtStatus> {
    let size = 8 + ace_bytes.len();
    if size > usize::from(u16::MAX) {
        return Err(NtStatus::INVALID_ACL);
    }
    let mut b = Vec::new();
    b.try_reserve_exact(size).map_err(nomem)?;
    b.try_extend_from_slice(&[revision, 0]).map_err(nomem)?;
    b.try_extend_from_slice(&(size as u16).to_le_bytes()).map_err(nomem)?;
    b.try_extend_from_slice(&(count as u16).to_le_bytes()).map_err(nomem)?;
    b.try_extend_from_slice(&[0, 0]).map_err(nomem)?;
    b.try_extend_from_slice(ace_bytes).map_err(nomem)?;
    Ok(b)
}

/// Whether the ACE type has a mask and a SID right after its header
/// (ACCESS_ALLOWED, ACCESS_DENIED, SYSTEM_AUDIT, SYSTEM_ALARM,
/// SYSTEM_MANDATORY_LABEL...).
fn simple(kind: u8) -> bool {
    matches!(kind, 0..=3 | 0x11)
}

/// Generic rights in an ACE mask mapped to specific ones (others kept).
pub fn map_mask(mask: u32, mapping: &GenericMapping) -> u32 {
    let mut m = mask & !GENERIC_BITS;
    if mask & 0x8000_0000 != 0 {
        m |= mapping.read;
    }
    if mask & 0x4000_0000 != 0 {
        m |= mapping.write;
    }
    if mask & 0x2000_0000 != 0 {
        m |= mapping.execute;
    }
    if mask & 0x1000_0000 != 0 {
        m |= mapping.all;
    }
    m
}

/// Appends a simple ACE.
fn push_ace(out: &mut Vec<u8>, kind: u8, flags: u8, mask: u32, sid: &Sid) -> Result<(), NtStatus> {
    let len = 8 + sid.len();
    let mut s = [0u8; 68];
    sid.write(&mut s);
    out.try_extend_from_slice(&[kind, flags]).map_err(nomem)?;
    out.try_extend_from_slice(&(len as u16).to_le_bytes()).map_err(nomem)?;
    out.try_extend_from_slice(&mask.to_le_bytes()).map_err(nomem)?;
    out.try_extend_from_slice(&s[..sid.len()]).map_err(nomem)
}

/// An ACE copied with new flags (any type).
fn push_copy(out: &mut Vec<u8>, ace: &[u8], flags: u8) -> Result<(), NtStatus> {
    let at = out.len();
    out.try_extend_from_slice(ace).map_err(nomem)?;
    out[at + 1] = flags;
    Ok(())
}

/// The ACEs `parent_acl` passes to a new child (a container or not),
/// as ACE bytes and their count.
pub fn inherited_aces(
    parent_acl: &[u8],
    container: bool,
    owner: &Sid,
    group: &Sid,
    mapping: &GenericMapping,
) -> Result<(Vec<u8>, usize), NtStatus> {
    let (mut out, mut count) = (Vec::new(), 0);
    for (kind, flags, ace) in aces(parent_acl) {
        let oi = flags & OBJECT_INHERIT_ACE != 0;
        let ci = flags & CONTAINER_INHERIT_ACE != 0;
        let np = flags & NO_PROPAGATE_INHERIT_ACE != 0;
        let (effective, propagate) = if container { (ci, (oi || ci) && !np) } else { (oi, false) };
        if !effective && !propagate {
            continue;
        }
        let inheritable = flags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE);
        // The effective ACE: generic rights mapped, creator SIDs replaced.
        let (changed, mask, sid) = if simple(kind) && ace.len() >= 8 {
            let mask = u32_at(ace, 4).unwrap_or(0);
            let sid = Sid::parse(&ace[8..]);
            let replaced = match sid {
                Some(s) if s == CREATOR_OWNER => Some(*owner),
                Some(s) if s == CREATOR_GROUP => Some(*group),
                _ => None,
            };
            let mapped = map_mask(mask, mapping);
            (mapped != mask || replaced.is_some(), mapped, replaced.or(sid))
        } else {
            (false, 0, None)
        };
        if effective {
            match sid {
                Some(sid) if changed => {
                    push_ace(&mut out, kind, INHERITED_ACE, mask, &sid)?;
                    count += 1;
                    if propagate {
                        push_copy(&mut out, ace, inheritable | INHERIT_ONLY_ACE | INHERITED_ACE)?;
                        count += 1;
                    }
                }
                _ => {
                    let keep = if propagate { inheritable } else { 0 };
                    push_copy(&mut out, ace, keep | INHERITED_ACE)?;
                    count += 1;
                }
            }
        } else {
            push_copy(&mut out, ace, inheritable | INHERIT_ONLY_ACE | INHERITED_ACE)?;
            count += 1;
        }
    }
    Ok((out, count))
}

/// An ACL of `aces` (mask, SID, flags) as ACCESS_ALLOWED ACEs.
pub fn allow_acl(list: &[(u32, Sid, u8)]) -> Result<Vec<u8>, NtStatus> {
    let mut b = Vec::new();
    for (mask, sid, flags) in list {
        push_ace(&mut b, 0, *flags, *mask, sid)?;
    }
    build_acl(ACL_REVISION, &b, list.len())
}

/// The token's default DACL with generic rights mapped (what an object
/// created without a DACL or inheritable ACEs gets).
fn default_dacl(token: &TokenData, mapping: &GenericMapping) -> Result<Option<Vec<u8>>, NtStatus> {
    let Some(acl) = valid_acl(&token.default_dacl) else { return Ok(None) };
    let mut out = Vec::new();
    let mut count = 0;
    for (kind, flags, ace) in aces(acl) {
        let at = out.len();
        push_copy(&mut out, ace, flags)?;
        if simple(kind) && ace.len() >= 8 {
            let mask = map_mask(u32_at(ace, 4).unwrap_or(0), mapping);
            out[at + 4..at + 8].copy_from_slice(&mask.to_le_bytes());
        }
        count += 1;
    }
    Ok(Some(build_acl(acl[0], &out, count)?))
}

/// Whether `owner` may be set by the holder of `token`: its user, or a
/// group with SE_GROUP_OWNER (or with SeRestorePrivilege enabled).
pub fn valid_owner(token: &TokenData, owner: &Sid) -> bool {
    const SE_GROUP_OWNER: u32 = 0x8;
    const SE_RESTORE: u64 = 18;
    *owner == token.user.sid
        || token.groups.iter().any(|g| g.sid == *owner && g.attributes & SE_GROUP_OWNER != 0)
        || token.privileges.iter().any(|p| p.luid == SE_RESTORE && p.attributes & 2 != 0)
}

/// The descriptor a new object gets (see the module comment).
/// `auto_inherit`: the object type merges inherited ACEs after a given
/// DACL and marks inherited DACLs auto-inherited (files and keys).
pub fn assign(
    parent: Option<&Descriptor>,
    given: Option<Descriptor>,
    container: bool,
    token: &TokenData,
    mapping: &GenericMapping,
    auto_inherit: bool,
) -> Result<Descriptor, NtStatus> {
    let given = given.unwrap_or(Descriptor {
        control: 0,
        owner: None,
        group: None,
        dacl: None,
        sacl: None,
    });
    let (owner, owner_defaulted) = match given.owner {
        Some(o) => {
            if !valid_owner(token, &o) {
                return Err(NtStatus::INVALID_OWNER);
            }
            (o, given.control & SE_OWNER_DEFAULTED)
        }
        None => (token.user.sid, SE_OWNER_DEFAULTED),
    };
    let (group, group_defaulted) = match given.group {
        Some(g) => (g, given.control & SE_GROUP_DEFAULTED),
        None => (token.primary_group, SE_GROUP_DEFAULTED),
    };
    let inherited = match parent.and_then(|p| p.dacl.as_deref()) {
        Some(acl) => inherited_aces(acl, container, &owner, &group, mapping)?,
        None => (Vec::new(), 0),
    };
    let mut control = owner_defaulted | group_defaulted;
    let dacl = if given.control & SE_DACL_PRESENT != 0 {
        let protected = given.control & SE_DACL_PROTECTED != 0;
        control |= SE_DACL_PRESENT | (given.control & (SE_DACL_PROTECTED | SE_DACL_DEFAULTED));
        match given.dacl {
            Some(acl) if auto_inherit && !protected => {
                // The given (non-inherited) ACEs, then the inherited ones.
                let mut b = Vec::new();
                let mut count = 0;
                for (_, flags, ace) in aces(&acl) {
                    if flags & INHERITED_ACE == 0 {
                        b.try_extend_from_slice(ace).map_err(nomem)?;
                        count += 1;
                    }
                }
                b.try_extend_from_slice(&inherited.0).map_err(nomem)?;
                control |= SE_DACL_AUTO_INHERITED;
                Some(build_acl(acl[0], &b, count + inherited.1)?)
            }
            other => other,
        }
    } else if inherited.1 > 0 {
        control |= SE_DACL_PRESENT;
        if auto_inherit {
            control |= SE_DACL_AUTO_INHERITED;
        }
        Some(build_acl(ACL_REVISION, &inherited.0, inherited.1)?)
    } else {
        let acl = default_dacl(token, mapping)?;
        if acl.is_some() {
            control |= SE_DACL_PRESENT | SE_DACL_DEFAULTED;
        }
        acl
    };
    let (sacl, sacl_control) = if given.control & SE_SACL_PRESENT != 0 {
        (given.sacl, given.control & SACL_BITS)
    } else {
        (None, 0)
    };
    Ok(Descriptor {
        control: control | sacl_control,
        owner: Some(owner),
        group: Some(group),
        dacl,
        sacl,
    })
}

/// NtSetSecurityObject's merge of `info`'s parts of `new` into `current`.
pub fn set(
    current: &mut Descriptor,
    info: u32,
    new: Descriptor,
    token: &TokenData,
) -> Result<(), NtStatus> {
    if info & OWNER_SECURITY_INFORMATION != 0 {
        let owner = new.owner.ok_or(NtStatus::INVALID_OWNER)?;
        if !valid_owner(token, &owner) {
            return Err(NtStatus::INVALID_OWNER);
        }
        current.owner = Some(owner);
        current.control &= !SE_OWNER_DEFAULTED;
    }
    if info & GROUP_SECURITY_INFORMATION != 0 {
        current.group = Some(new.group.ok_or(NtStatus::INVALID_PRIMARY_GROUP)?);
        current.control &= !SE_GROUP_DEFAULTED;
    }
    if info & DACL_SECURITY_INFORMATION != 0 {
        current.control = (current.control & !DACL_BITS) | (new.control & DACL_BITS);
        current.dacl = if new.control & SE_DACL_PRESENT != 0 { new.dacl } else { None };
        if info & PROTECTED_DACL_SECURITY_INFORMATION != 0 {
            current.control |= SE_DACL_PROTECTED;
        } else if info & UNPROTECTED_DACL_SECURITY_INFORMATION != 0 {
            current.control &= !SE_DACL_PROTECTED;
        }
    }
    if info & SACL_SECURITY_INFORMATION != 0 {
        current.control = (current.control & !SACL_BITS) | (new.control & SACL_BITS);
        current.sacl = if new.control & SE_SACL_PRESENT != 0 { new.sacl } else { None };
        if info & PROTECTED_SACL_SECURITY_INFORMATION != 0 {
            current.control |= SE_SACL_PROTECTED;
        } else if info & UNPROTECTED_SACL_SECURITY_INFORMATION != 0 {
            current.control &= !SE_SACL_PROTECTED;
        }
    }
    Ok(())
}

impl Descriptor {
    /// The self-relative form with the parts `info` asks for (all:
    /// u32::MAX): header, owner, group, SACL, DACL.
    pub fn to_bytes(&self, info: u32) -> Result<Vec<u8>, NtStatus> {
        let mut b = Vec::new();
        b.try_extend_from_slice(&[0u8; 20]).map_err(nomem)?;
        b[0] = 1;
        let mut control = SE_SELF_RELATIVE;
        let sid_part = |b: &mut Vec<u8>, sid: &Option<Sid>, field: usize| -> Result<(), NtStatus> {
            if let Some(sid) = sid {
                let at = b.len() as u32;
                let mut s = [0u8; 68];
                sid.write(&mut s);
                b.try_extend_from_slice(&s[..sid.len()]).map_err(nomem)?;
                b[field..field + 4].copy_from_slice(&at.to_le_bytes());
            }
            Ok(())
        };
        if info & OWNER_SECURITY_INFORMATION != 0 {
            sid_part(&mut b, &self.owner, 4)?;
            control |= self.control & SE_OWNER_DEFAULTED;
        }
        if info & GROUP_SECURITY_INFORMATION != 0 {
            sid_part(&mut b, &self.group, 8)?;
            control |= self.control & SE_GROUP_DEFAULTED;
        }
        let acl_part =
            |b: &mut Vec<u8>, acl: &Option<Vec<u8>>, field: usize| -> Result<(), NtStatus> {
                if let Some(acl) = acl {
                    let at = b.len() as u32;
                    b.try_extend_from_slice(acl).map_err(nomem)?;
                    b[field..field + 4].copy_from_slice(&at.to_le_bytes());
                }
                Ok(())
            };
        if info & SACL_SECURITY_INFORMATION != 0 {
            acl_part(&mut b, &self.sacl, 12)?;
            control |= self.control & SACL_BITS;
        }
        if info & DACL_SECURITY_INFORMATION != 0 {
            acl_part(&mut b, &self.dacl, 16)?;
            control |= self.control & DACL_BITS;
        }
        b[2..4].copy_from_slice(&control.to_le_bytes());
        Ok(b)
    }

    /// A self-relative descriptor's parts (as stored).
    pub fn parse(b: &[u8]) -> Result<Descriptor, NtStatus> {
        let bad = NtStatus::INVALID_SECURITY_DESCR;
        let control = u16_at(b, 2).ok_or(bad)?;
        if b.first() != Some(&1) || control & SE_SELF_RELATIVE == 0 {
            return Err(bad);
        }
        let offset = |field: usize| u32_at(b, field).map(|o| o as usize).ok_or(bad);
        let sid = |field: usize| -> Result<Option<Sid>, NtStatus> {
            match offset(field)? {
                0 => Ok(None),
                o => Ok(Some(b.get(o..).and_then(Sid::parse).ok_or(bad)?)),
            }
        };
        let acl = |field: usize, present: bool| -> Result<Option<Vec<u8>>, NtStatus> {
            match offset(field)? {
                0 => Ok(None),
                _ if !present => Ok(None),
                o => Ok(Some(copy(b.get(o..).and_then(valid_acl).ok_or(NtStatus::INVALID_ACL)?)?)),
            }
        };
        Ok(Descriptor {
            control: control & !SE_SELF_RELATIVE,
            owner: sid(4)?,
            group: sid(8)?,
            sacl: acl(12, control & SE_SACL_PRESENT != 0)?,
            dacl: acl(16, control & SE_DACL_PRESENT != 0)?,
        })
    }

    /// A caller's descriptor (absolute or self-relative) at `ptr`, read
    /// with `read(address, buffer)` from its memory.
    pub fn capture(
        ptr: u64,
        read: impl Fn(u64, &mut [u8]) -> Result<(), NtStatus>,
    ) -> Result<Descriptor, NtStatus> {
        let bad = NtStatus::INVALID_SECURITY_DESCR;
        let mut head = [0u8; 40];
        read(ptr, &mut head[..20])?;
        let control = u16::from_le_bytes([head[2], head[3]]);
        if head[0] != 1 {
            return Err(NtStatus::UNKNOWN_REVISION);
        }
        // The addresses of the owner, group, SACL and DACL (0: none).
        let addresses: [u64; 4] = if control & SE_SELF_RELATIVE != 0 {
            let o = |f: usize| {
                let v = u32::from_le_bytes([head[f], head[f + 1], head[f + 2], head[f + 3]]);
                if v == 0 { 0 } else { ptr + u64::from(v) }
            };
            [o(4), o(8), o(12), o(16)]
        } else {
            read(ptr + 20, &mut head[20..40])?;
            let p = |f: usize| u64::from_le_bytes(head[f..f + 8].try_into().unwrap_or([0; 8]));
            [p(8), p(16), p(24), p(32)]
        };
        let sid = |at: u64| -> Result<Option<Sid>, NtStatus> {
            if at == 0 {
                return Ok(None);
            }
            let mut s = [0u8; 68];
            read(at, &mut s[..8])?;
            let n = 8 + 4 * usize::from(s[1].min(15));
            read(at + 8, &mut s[8..n])?;
            Ok(Some(Sid::parse(&s[..n]).ok_or(NtStatus::INVALID_SID)?))
        };
        let acl = |at: u64, present: bool| -> Result<Option<Vec<u8>>, NtStatus> {
            if at == 0 || !present {
                return Ok(None);
            }
            let mut h = [0u8; 8];
            read(at, &mut h)?;
            let size = usize::from(u16::from_le_bytes([h[2], h[3]]));
            if size < 8 {
                return Err(NtStatus::INVALID_ACL);
            }
            let mut v = Vec::new();
            v.try_reserve_exact(size).map_err(nomem)?;
            v.resize(size, 0u8);
            read(at, &mut v)?;
            if valid_acl(&v).is_none() {
                return Err(NtStatus::INVALID_ACL);
            }
            Ok(Some(v))
        };
        let _ = bad;
        Ok(Descriptor {
            control: control & !SE_SELF_RELATIVE,
            owner: sid(addresses[0])?,
            group: sid(addresses[1])?,
            sacl: acl(addresses[2], control & SE_SACL_PRESENT != 0)?,
            dacl: acl(addresses[3], control & SE_DACL_PRESENT != 0)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nt::token::{TokenData, USER};

    const FILE_MAPPING: GenericMapping = GenericMapping {
        read: 0x0012_0089,
        write: 0x0012_0116,
        execute: 0x0012_00a0,
        all: 0x001f_01ff,
    };
    const EVERYONE: Sid = Sid::new(1, &[0]);
    const SYSTEM: Sid = Sid::new(5, &[18]);

    fn ace_list(acl: &[u8]) -> Vec<(u8, u8, u32, Sid)> {
        aces(acl)
            .map(|(k, f, a)| (k, f, u32_at(a, 4).unwrap(), Sid::parse(&a[8..]).unwrap()))
            .collect()
    }

    fn parent(list: &[(u32, Sid, u8)]) -> Descriptor {
        Descriptor {
            control: SE_DACL_PRESENT | SE_DACL_PROTECTED,
            owner: Some(USER),
            group: Some(USER),
            dacl: Some(allow_acl(list).unwrap()),
            sacl: None,
        }
    }

    #[test]
    fn inheritance_into_files_and_directories() {
        let token = TokenData::boot_user().unwrap();
        let p = parent(&[
            (0x1f01ff, EVERYONE, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE),
            (0x120089, SYSTEM, 0),
            (
                0x1000_0000,
                Sid::new(3, &[0]),
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
            ),
            (0x120089, SYSTEM, CONTAINER_INHERIT_ACE | NO_PROPAGATE_INHERIT_ACE),
        ]);
        let file = assign(Some(&p), None, false, &token, &FILE_MAPPING, true).unwrap();
        assert_eq!(
            file.control,
            SE_DACL_PRESENT | SE_DACL_AUTO_INHERITED | SE_OWNER_DEFAULTED | SE_GROUP_DEFAULTED
        );
        assert_eq!(
            ace_list(file.dacl.as_ref().unwrap()),
            [(0, INHERITED_ACE, 0x1f01ff, EVERYONE), (0, INHERITED_ACE, 0x1f01ff, USER)]
        );
        let dir = assign(Some(&p), None, true, &token, &FILE_MAPPING, true).unwrap();
        assert_eq!(
            ace_list(dir.dacl.as_ref().unwrap()),
            [
                (0, 0x13, 0x1f01ff, EVERYONE),
                (0, INHERITED_ACE, 0x1f01ff, USER),
                (0, 0x1b, 0x1000_0000, Sid::new(3, &[0])),
                (0, INHERITED_ACE, 0x120089, SYSTEM),
            ]
        );
    }

    #[test]
    fn given_dacls_and_defaults() {
        let token = TokenData::boot_user().unwrap();
        let p = parent(&[(0x1f01ff, EVERYONE, OBJECT_INHERIT_ACE)]);
        let protected = parent(&[(0x120089, SYSTEM, 0)]);
        let d = assign(Some(&p), Some(protected), false, &token, &FILE_MAPPING, true).unwrap();
        assert_eq!(ace_list(d.dacl.as_ref().unwrap()), [(0, 0, 0x120089, SYSTEM)]);
        assert!(d.control & SE_DACL_PROTECTED != 0);
        let mut open = parent(&[(0x120089, SYSTEM, 0)]);
        open.control = SE_DACL_PRESENT;
        let d = assign(Some(&p), Some(open), false, &token, &FILE_MAPPING, true).unwrap();
        assert_eq!(ace_list(d.dacl.as_ref().unwrap()).len(), 2);
        // No parent: the token's default DACL, mapped.
        let d = assign(None, None, false, &token, &FILE_MAPPING, false).unwrap();
        let list = ace_list(d.dacl.as_ref().unwrap());
        assert_eq!(list[0], (0, 0, 0x1f01ff, USER));
        assert_eq!(list.len(), 3);
        // Another owner is refused.
        let mut other = parent(&[]);
        other.owner = Some(EVERYONE);
        assert_eq!(
            assign(None, Some(other), false, &token, &FILE_MAPPING, false),
            Err(NtStatus::INVALID_OWNER)
        );
    }

    #[test]
    fn bytes_round_trip_and_parts() {
        let d = parent(&[(0x1f01ff, EVERYONE, 0)]);
        let b = d.to_bytes(u32::MAX).unwrap();
        assert_eq!(Descriptor::parse(&b).unwrap(), d);
        let only = Descriptor::parse(&d.to_bytes(DACL_SECURITY_INFORMATION).unwrap()).unwrap();
        assert_eq!(only.owner, None);
        assert_eq!(only.dacl, d.dacl);
        // Captured from "user memory" at 0x1000.
        let read = |at: u64, out: &mut [u8]| {
            let at = at.checked_sub(0x1000).ok_or(NtStatus::ACCESS_VIOLATION)? as usize;
            out.copy_from_slice(b.get(at..at + out.len()).ok_or(NtStatus::ACCESS_VIOLATION)?);
            Ok(())
        };
        assert_eq!(Descriptor::capture(0x1000, read).unwrap(), d);
    }

    #[test]
    fn setting_parts() {
        let token = TokenData::boot_user().unwrap();
        let mut d = parent(&[(0x1f01ff, EVERYONE, 0)]);
        let mut new = parent(&[(0x120089, SYSTEM, 0)]);
        new.control = SE_DACL_PRESENT;
        set(&mut d, DACL_SECURITY_INFORMATION, new, &token).unwrap();
        assert_eq!(d.control & SE_DACL_PROTECTED, 0);
        assert_eq!(ace_list(d.dacl.as_ref().unwrap()), [(0, 0, 0x120089, SYSTEM)]);
        let mut owner = parent(&[]);
        owner.owner = Some(EVERYONE);
        assert_eq!(
            set(&mut d, OWNER_SECURITY_INFORMATION, owner, &token),
            Err(NtStatus::INVALID_OWNER)
        );
    }
}

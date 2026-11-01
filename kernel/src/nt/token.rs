//! Access tokens (docs/SECURITY.md, "Tokens"): a kernel object type
//! holding a security context as documented in "Access Tokens" (Microsoft
//! Learn): the user, groups with attributes, privileges with attributes,
//! restricting SIDs, the owner and primary group, the default DACL, the
//! logon session, the token type and impersonation level.
//!
//! A process holds a primary token (`Process::token`); a thread may hold
//! an impersonation token (`NtThread::impersonation`). Processes the
//! kernel starts get the boot user's token (`gala`, a standard user of
//! the interactive logon session); a child gets a copy of its creator's
//! primary token. Nothing is enforced yet (phase S3): tokens are kept,
//! queried, adjusted, duplicated and filtered, and handles to them carry
//! granted access like any other object's.
//!
//! The information classes are laid out as `NtQueryInformationToken`
//! returns them (the documented TOKEN_* structures, x64): pointers inside
//! the data point into the caller's buffer, so `query` takes its address.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::object::{GenericMapping, READ_CONTROL, STANDARD_RIGHTS_REQUIRED};
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

pub const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
pub const TOKEN_DUPLICATE: u32 = 0x0002;
pub const TOKEN_IMPERSONATE: u32 = 0x0004;
pub const TOKEN_QUERY: u32 = 0x0008;
pub const TOKEN_QUERY_SOURCE: u32 = 0x0010;
pub const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
pub const TOKEN_ADJUST_GROUPS: u32 = 0x0040;
pub const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
pub const TOKEN_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x01ff;

/// TOKEN_READ, TOKEN_WRITE, TOKEN_EXECUTE (winnt.h).
pub const TOKEN_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | TOKEN_QUERY,
    write: READ_CONTROL | TOKEN_ADJUST_PRIVILEGES | TOKEN_ADJUST_GROUPS | TOKEN_ADJUST_DEFAULT,
    execute: READ_CONTROL,
    all: TOKEN_ALL_ACCESS,
};

// SECURITY_IMPERSONATION_LEVEL.
pub const SECURITY_ANONYMOUS: u32 = 0;
pub const SECURITY_IDENTIFICATION: u32 = 1;
pub const SECURITY_IMPERSONATION: u32 = 2;
pub const SECURITY_DELEGATION: u32 = 3;

// Group attributes (SE_GROUP_*).
pub const SE_GROUP_MANDATORY: u32 = 0x0000_0001;
pub const SE_GROUP_ENABLED_BY_DEFAULT: u32 = 0x0000_0002;
pub const SE_GROUP_ENABLED: u32 = 0x0000_0004;
pub const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;
pub const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
pub const SE_GROUP_INTEGRITY_ENABLED: u32 = 0x0000_0040;
pub const SE_GROUP_LOGON_ID: u32 = 0xc000_0000;

// Privilege attributes (SE_PRIVILEGE_*).
pub const SE_PRIVILEGE_ENABLED_BY_DEFAULT: u32 = 0x0000_0001;
pub const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;
pub const SE_PRIVILEGE_REMOVED: u32 = 0x0000_0004;
pub const SE_PRIVILEGE_USED_FOR_ACCESS: u32 = 0x8000_0000;

/// SeChangeNotifyPrivilege's LUID: the one DISABLE_MAX_PRIVILEGE keeps.
pub const SE_CHANGE_NOTIFY: u64 = 23;

// NtFilterToken flags.
pub const DISABLE_MAX_PRIVILEGE: u32 = 0x1;
pub const SANDBOX_INERT: u32 = 0x2;

/// The logon session of the boot user (an interactive logon's LUID; the
/// logon SID is S-1-5-5-0-LOGON_ID).
pub const LOGON_ID: u64 = 0x0002_a3b4;

/// A SID (revision 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sid {
    authority: [u8; 6],
    count: u8,
    subs: [u32; 15],
}

impl Sid {
    /// A SID whose identifier authority fits in its last byte (all the
    /// well-known ones do: 1 world, 2 local, 5 NT, 16 mandatory label).
    pub const fn new(authority: u8, subs: &[u32]) -> Sid {
        let mut s = [0u32; 15];
        let mut i = 0;
        while i < subs.len() && i < 15 {
            s[i] = subs[i];
            i += 1;
        }
        Sid { authority: [0, 0, 0, 0, 0, authority], count: i as u8, subs: s }
    }

    #[allow(clippy::len_without_is_empty)] // a SID is never empty
    pub fn len(&self) -> usize {
        8 + 4 * usize::from(self.count)
    }

    /// The SID's bytes, at the start of `out` (at least `len()` long).
    pub fn write(&self, out: &mut [u8]) {
        out[0] = 1;
        out[1] = self.count;
        out[2..8].copy_from_slice(&self.authority);
        for i in 0..usize::from(self.count) {
            out[8 + 4 * i..12 + 4 * i].copy_from_slice(&self.subs[i].to_le_bytes());
        }
    }

    /// A SID from its bytes (the start of `b`), if valid.
    pub fn parse(b: &[u8]) -> Option<Sid> {
        if b.len() < 8 || b[0] != 1 || b[1] > 15 || b.len() < 8 + 4 * usize::from(b[1]) {
            return None;
        }
        let mut sid = Sid { authority: [0; 6], count: b[1], subs: [0; 15] };
        sid.authority.copy_from_slice(&b[2..8]);
        for i in 0..usize::from(b[1]) {
            let at = 8 + 4 * i;
            sid.subs[i] = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        }
        Some(sid)
    }

    fn is_label(&self) -> bool {
        self.authority == [0, 0, 0, 0, 0, 16] && self.count == 1
    }
}

/// A SID with its attributes (SID_AND_ATTRIBUTES).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub sid: Sid,
    pub attributes: u32,
}

/// A privilege (LUID_AND_ATTRIBUTES).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Privilege {
    pub luid: u64,
    pub attributes: u32,
}

/// TOKEN_TYPE, with the impersonation level of impersonation tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Primary,
    Impersonation(u32),
}

/// What a token holds (all but its id).
pub struct TokenData {
    pub kind: TokenType,
    /// The user (attributes 0, or deny-only once filtered).
    pub user: Group,
    pub groups: Vec<Group>,
    pub privileges: Vec<Privilege>,
    /// Restricting SIDs (CreateRestrictedToken); empty: not restricted.
    pub restricted: Vec<Group>,
    pub primary_group: Sid,
    /// The default DACL (an ACL), or empty: none.
    pub default_dacl: Vec<u8>,
    pub authentication_id: u64,
    pub modified_id: u64,
    pub session: u32,
    /// TOKEN_ELEVATION_TYPE: 1 default (not a split token).
    pub elevation_type: u32,
    /// TOKEN_SOURCE: the source name and its id.
    pub source: ([u8; 8], u64),
    /// Created with SANDBOX_INERT.
    pub sandbox_inert: bool,
}

/// A token object.
pub struct Token {
    /// TokenStatistics.TokenId.
    pub id: u64,
    pub data: IrqMutex<TokenData>,
}

/// A token object holding `data`.
pub fn object(data: TokenData) -> Result<super::object::ObjectRef, NtStatus> {
    super::object::KObject::new(super::object::ObjectBody::Token(Token::new(data)))
        .map_err(|_| NtStatus::NO_MEMORY)
}

/// A primary token for a new process: a copy of `parent`'s (its creator's
/// primary token), or the boot user's for processes the kernel starts.
pub fn for_new_process(
    parent: Option<&super::object::ObjectRef>,
) -> Result<super::object::ObjectRef, NtStatus> {
    let data = match parent.map(|p| &p.body) {
        Some(super::object::ObjectBody::Token(t)) => {
            let mut data = t.data.lock().try_clone()?;
            data.kind = TokenType::Primary;
            data
        }
        _ => TokenData::boot_user()?,
    };
    object(data)
}

static NEXT_LUID: AtomicU64 = AtomicU64::new(0x0010_0000);

/// A new locally unique id (above the well-known logon session LUIDs).
pub fn next_luid() -> u64 {
    NEXT_LUID.fetch_add(1, Ordering::Relaxed)
}

/// The boot user `gala`: S-1-5-21-1000000001-1000000002-1000000003-1001
/// (the machine GALA's domain, RID 1001; advapi32 knows the same SID).
pub const USER: Sid = Sid::new(5, &[21, 1_000_000_001, 1_000_000_002, 1_000_000_003, 1001]);
pub const LOCAL_SYSTEM: Sid = Sid::new(5, &[18]);
pub const LOGON_SID: Sid = Sid::new(5, &[5, 0, LOGON_ID as u32]);

/// The groups of a standard interactive user's token, as Windows gives
/// them (SE_GROUP_MANDATORY | ENABLED_BY_DEFAULT | ENABLED = 7; the
/// logon SID adds SE_GROUP_LOGON_ID; the label is SE_GROUP_INTEGRITY |
/// SE_GROUP_INTEGRITY_ENABLED).
const GROUPS: [Group; 10] = [
    Group { sid: Sid::new(1, &[0]), attributes: 7 }, // Everyone
    Group { sid: Sid::new(2, &[0]), attributes: 7 }, // LOCAL
    Group { sid: Sid::new(2, &[1]), attributes: 7 }, // CONSOLE LOGON
    Group { sid: Sid::new(5, &[4]), attributes: 7 }, // INTERACTIVE
    Group { sid: Sid::new(5, &[32, 545]), attributes: 7 }, // BUILTIN\Users
    Group { sid: Sid::new(5, &[11]), attributes: 7 }, // Authenticated Users
    Group { sid: Sid::new(5, &[15]), attributes: 7 }, // This Organization
    Group { sid: LOGON_SID, attributes: SE_GROUP_LOGON_ID | 7 },
    Group { sid: Sid::new(5, &[64, 10]), attributes: 7 }, // NTLM Authentication
    Group {
        sid: Sid::new(16, &[8192]),
        attributes: SE_GROUP_INTEGRITY | SE_GROUP_INTEGRITY_ENABLED,
    }, // Medium
];

/// The privileges a standard user holds: SeShutdown (19), SeChangeNotify
/// (23, enabled by default), SeUndock (25), SeIncreaseWorkingSet (33),
/// SeTimeZone (34).
const PRIVILEGES: [Privilege; 5] = [
    Privilege { luid: 19, attributes: 0 },
    Privilege { luid: 23, attributes: SE_PRIVILEGE_ENABLED_BY_DEFAULT | SE_PRIVILEGE_ENABLED },
    Privilege { luid: 25, attributes: 0 },
    Privilege { luid: 33, attributes: 0 },
    Privilege { luid: 34, attributes: 0 },
];

fn copy<T: Copy>(from: &[T]) -> Result<Vec<T>, NtStatus> {
    let mut v = Vec::new();
    v.try_extend_from_slice(from).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(v)
}

/// ACCESS_ALLOWED_ACE entries as an ACL (revision 2).
fn acl(aces: &[(u32, Sid)]) -> Result<Vec<u8>, NtStatus> {
    let size = 8 + aces.iter().map(|(_, s)| 8 + s.len()).sum::<usize>();
    let mut b = Vec::new();
    b.try_reserve_exact(size).map_err(|_| NtStatus::NO_MEMORY)?;
    b.resize(size, 0u8);
    b[0] = 2;
    b[2..4].copy_from_slice(&(size as u16).to_le_bytes());
    b[4..6].copy_from_slice(&(aces.len() as u16).to_le_bytes());
    let mut at = 8;
    for (mask, sid) in aces {
        let len = 8 + sid.len();
        b[at + 2..at + 4].copy_from_slice(&(len as u16).to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&mask.to_le_bytes());
        sid.write(&mut b[at + 8..]);
        at += len;
    }
    Ok(b)
}

impl TokenData {
    /// The boot user's primary token: the standard user's groups and
    /// privileges, owner the user, the default DACL Windows gives such a
    /// user (GENERIC_ALL to the user and SYSTEM, GENERIC_READ |
    /// GENERIC_EXECUTE to the logon SID). TODO(S2): the primary group of a
    /// local account is the machine's "None" (RID 513); it is the user
    /// here, as advapi32 answered before.
    pub fn boot_user() -> Result<TokenData, NtStatus> {
        Ok(TokenData {
            kind: TokenType::Primary,
            user: Group { sid: USER, attributes: 0 },
            groups: copy(&GROUPS)?,
            privileges: copy(&PRIVILEGES)?,
            restricted: Vec::new(),
            primary_group: USER,
            default_dacl: acl(&[
                (0x1000_0000, USER),
                (0x1000_0000, LOCAL_SYSTEM),
                (0xa000_0000, LOGON_SID),
            ])?,
            authentication_id: LOGON_ID,
            modified_id: next_luid(),
            session: 1,
            elevation_type: 1,
            source: (*b"User32 \0", LOGON_ID),
            sandbox_inert: false,
        })
    }

    pub fn try_clone(&self) -> Result<TokenData, NtStatus> {
        Ok(TokenData {
            kind: self.kind,
            user: self.user,
            groups: copy(&self.groups)?,
            privileges: copy(&self.privileges)?,
            restricted: copy(&self.restricted)?,
            primary_group: self.primary_group,
            default_dacl: copy(&self.default_dacl)?,
            authentication_id: self.authentication_id,
            modified_id: self.modified_id,
            session: self.session,
            elevation_type: self.elevation_type,
            source: self.source,
            sandbox_inert: self.sandbox_inert,
        })
    }

    pub fn level(&self) -> Option<u32> {
        match self.kind {
            TokenType::Primary => None,
            TokenType::Impersonation(level) => Some(level),
        }
    }
}

impl Token {
    pub fn new(data: TokenData) -> Token {
        Token { id: next_luid(), data: IrqMutex::new(data) }
    }

    /// NtDuplicateToken's copy: of type `kind`; with `effective_only`,
    /// without the disabled groups and privileges. An impersonation token
    /// duplicates at most at its own level, and only one at impersonation
    /// level or above becomes a primary token.
    pub fn duplicate(&self, kind: TokenType, effective_only: bool) -> Result<Token, NtStatus> {
        let mut data = self.data.lock().try_clone()?;
        if let Some(have) = data.level() {
            let enough = match kind {
                TokenType::Primary => have >= SECURITY_IMPERSONATION,
                TokenType::Impersonation(want) => want <= have,
            };
            if !enough {
                return Err(NtStatus::BAD_IMPERSONATION_LEVEL);
            }
        }
        data.kind = kind;
        if effective_only {
            data.groups.retain(|g| g.attributes & SE_GROUP_ENABLED != 0 || g.sid.is_label());
            data.privileges.retain(|p| p.attributes & SE_PRIVILEGE_ENABLED != 0);
        }
        Ok(Token::new(data))
    }

    /// NtFilterToken's restricted copy (same type): `disable` SIDs (the
    /// user or groups) become deny-only; `delete` privileges go (with
    /// DISABLE_MAX_PRIVILEGE all but SeChangeNotify); `restrict` SIDs are
    /// the restricting SIDs (intersected with the existing ones if the
    /// token is restricted already, as documented).
    pub fn filter(
        &self,
        flags: u32,
        disable: &[Sid],
        delete: &[u64],
        restrict: &[Sid],
    ) -> Result<Token, NtStatus> {
        let mut data = self.data.lock().try_clone()?;
        let deny_only = |g: &mut Group| {
            if disable.contains(&g.sid) {
                g.attributes &= !(SE_GROUP_ENABLED | SE_GROUP_ENABLED_BY_DEFAULT);
                g.attributes |= SE_GROUP_USE_FOR_DENY_ONLY;
            }
        };
        deny_only(&mut data.user);
        data.groups.iter_mut().for_each(deny_only);
        if flags & DISABLE_MAX_PRIVILEGE != 0 {
            data.privileges.retain(|p| p.luid == SE_CHANGE_NOTIFY);
        }
        data.privileges.retain(|p| !delete.contains(&p.luid));
        if !restrict.is_empty() {
            let attributes = SE_GROUP_MANDATORY | SE_GROUP_ENABLED_BY_DEFAULT | SE_GROUP_ENABLED;
            let mut list = Vec::new();
            for sid in restrict {
                let keep =
                    data.restricted.is_empty() || data.restricted.iter().any(|g| g.sid == *sid);
                if keep && !list.iter().any(|g: &Group| g.sid == *sid) {
                    list.try_push(Group { sid: *sid, attributes })
                        .map_err(|_| NtStatus::NO_MEMORY)?;
                }
            }
            data.restricted = list;
        }
        data.sandbox_inert |= flags & SANDBOX_INERT != 0;
        data.modified_id = next_luid();
        Ok(Token::new(data))
    }
}

/// The result of NtAdjustPrivilegesToken's changes.
pub struct Adjusted {
    /// The privileges whose state changed, as they were (PreviousState).
    pub previous: Vec<Privilege>,
    /// Some privilege asked for is not held (STATUS_NOT_ALL_ASSIGNED).
    pub not_all_assigned: bool,
}

/// NtAdjustPrivilegesToken: with `disable_all`, every enabled privilege is
/// disabled (`new` ignored); otherwise each privilege of `new` held by the
/// token is enabled or disabled as its SE_PRIVILEGE_ENABLED bit says, or
/// removed (SE_PRIVILEGE_REMOVED). `previous_room` is the PreviousState
/// buffer's size (None: not asked for): if the previous state does not fit,
/// nothing changes (Err(needed size)).
pub fn adjust_privileges(
    data: &mut TokenData,
    disable_all: bool,
    new: &[Privilege],
    previous_room: Option<usize>,
) -> Result<Result<Adjusted, usize>, NtStatus> {
    let mut changes: Vec<(usize, u32)> = Vec::new(); // (index, new attributes)
    let mut not_all_assigned = false;
    let mut remove = Vec::new();
    for (i, p) in data.privileges.iter().enumerate() {
        let wanted = if disable_all {
            Some(p.attributes & !SE_PRIVILEGE_ENABLED)
        } else {
            new.iter().rev().find(|n| n.luid == p.luid).map(|n| {
                if n.attributes & SE_PRIVILEGE_REMOVED != 0 {
                    u32::MAX
                } else {
                    (p.attributes & !SE_PRIVILEGE_ENABLED) | (n.attributes & SE_PRIVILEGE_ENABLED)
                }
            })
        };
        match wanted {
            Some(u32::MAX) => remove.try_push(i).map_err(|_| NtStatus::NO_MEMORY)?,
            Some(a) if a != p.attributes => {
                changes.try_push((i, a)).map_err(|_| NtStatus::NO_MEMORY)?
            }
            _ => {}
        }
    }
    if !disable_all {
        not_all_assigned = new.iter().any(|n| !data.privileges.iter().any(|p| p.luid == n.luid));
    }
    let mut previous = Vec::new();
    for &(i, _) in &changes {
        previous.try_push(data.privileges[i]).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    for &i in &remove {
        previous.try_push(data.privileges[i]).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    let needed = 4 + 12 * previous.len();
    if previous_room.is_some_and(|room| room < needed) {
        return Ok(Err(needed));
    }
    for &(i, a) in &changes {
        data.privileges[i].attributes = a;
    }
    for &i in remove.iter().rev() {
        data.privileges.remove(i);
    }
    if !changes.is_empty() || !remove.is_empty() {
        data.modified_id = next_luid();
    }
    Ok(Ok(Adjusted { previous, not_all_assigned }))
}

/// NtPrivilegeCheck: whether the enabled privileges satisfy `set` (all of
/// them, or any with `all` false); the enabled ones are marked
/// SE_PRIVILEGE_USED_FOR_ACCESS.
pub fn privilege_check(data: &TokenData, set: &mut [Privilege], all: bool) -> bool {
    let mut held = 0;
    for p in set.iter_mut() {
        let enabled = data
            .privileges
            .iter()
            .any(|t| t.luid == p.luid && t.attributes & SE_PRIVILEGE_ENABLED != 0);
        if enabled {
            p.attributes |= SE_PRIVILEGE_USED_FOR_ACCESS;
            held += 1;
        }
    }
    if all { held == set.len() } else { held > 0 }
}

/// Builds an information class's bytes for a buffer at `base`.
struct Out {
    b: Vec<u8>,
    base: u64,
}

impl Out {
    fn put(&mut self, bytes: &[u8]) -> Result<(), NtStatus> {
        self.b.try_extend_from_slice(bytes).map_err(|_| NtStatus::NO_MEMORY)
    }

    fn u32(&mut self, v: u32) -> Result<(), NtStatus> {
        self.put(&v.to_le_bytes())
    }

    fn u64(&mut self, v: u64) -> Result<(), NtStatus> {
        self.put(&v.to_le_bytes())
    }

    fn align(&mut self, to: usize) -> Result<(), NtStatus> {
        while !self.b.len().is_multiple_of(to) {
            self.put(&[0])?;
        }
        Ok(())
    }

    fn set_u32(&mut self, at: usize, v: u32) {
        self.b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn set_u64(&mut self, at: usize, v: u64) {
        self.b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn here(&self) -> u64 {
        self.base.wrapping_add(self.b.len() as u64)
    }

    fn sid(&mut self, sid: &Sid) -> Result<(), NtStatus> {
        let mut s = [0u8; 68];
        sid.write(&mut s);
        self.put(&s[..sid.len()])
    }

    /// SID_AND_ATTRIBUTES entries for `list` here (8-aligned), the SIDs
    /// after them.
    fn sid_list(&mut self, list: &[Group]) -> Result<(), NtStatus> {
        let start = self.b.len();
        for _ in list {
            self.put(&[0; 16])?;
        }
        for (i, g) in list.iter().enumerate() {
            let e = start + 16 * i;
            let at = self.here();
            self.set_u64(e, at);
            self.set_u32(e + 8, g.attributes);
            self.sid(&g.sid)?;
        }
        Ok(())
    }

    /// TOKEN_GROUPS: GroupCount (padded to 8), then the list.
    fn groups(&mut self, list: &[Group]) -> Result<(), NtStatus> {
        self.u32(list.len() as u32)?;
        self.u32(0)?;
        self.sid_list(list)
    }

    /// A pointer to the SID that follows it.
    fn sid_pointer(&mut self, sid: &Sid) -> Result<(), NtStatus> {
        let at = self.here() + 8;
        self.u64(at)?;
        self.sid(sid)
    }
}

fn privileges(o: &mut Out, list: &[Privilege]) -> Result<(), NtStatus> {
    for p in list {
        o.u64(p.luid)?;
        o.u32(p.attributes)?;
    }
    Ok(())
}

// Token information classes (TOKEN_INFORMATION_CLASS).
pub const TOKEN_USER: u32 = 1;
pub const TOKEN_GROUPS: u32 = 2;
pub const TOKEN_PRIVILEGES: u32 = 3;
pub const TOKEN_OWNER: u32 = 4;
pub const TOKEN_PRIMARY_GROUP: u32 = 5;
pub const TOKEN_DEFAULT_DACL: u32 = 6;
pub const TOKEN_SOURCE: u32 = 7;
pub const TOKEN_TYPE: u32 = 8;
pub const TOKEN_IMPERSONATION_LEVEL: u32 = 9;
pub const TOKEN_STATISTICS: u32 = 10;
pub const TOKEN_RESTRICTED_SIDS: u32 = 11;
pub const TOKEN_SESSION_ID: u32 = 12;
pub const TOKEN_GROUPS_AND_PRIVILEGES: u32 = 13;
pub const TOKEN_SANDBOX_INERT: u32 = 15;
pub const TOKEN_ELEVATION_TYPE: u32 = 18;
pub const TOKEN_ELEVATION: u32 = 20;
pub const TOKEN_HAS_RESTRICTIONS: u32 = 21;
pub const TOKEN_VIRTUALIZATION_ALLOWED: u32 = 23;
pub const TOKEN_VIRTUALIZATION_ENABLED: u32 = 24;
pub const TOKEN_INTEGRITY_LEVEL: u32 = 25;
pub const TOKEN_UI_ACCESS: u32 = 26;
pub const TOKEN_MANDATORY_POLICY: u32 = 27;
pub const TOKEN_LOGON_SID: u32 = 28;
pub const TOKEN_IS_APP_CONTAINER: u32 = 29;
pub const TOKEN_CAPABILITIES: u32 = 30;
pub const TOKEN_APP_CONTAINER_SID: u32 = 31;
pub const TOKEN_APP_CONTAINER_NUMBER: u32 = 32;

/// The access a class needs (TOKEN_QUERY, TokenSource TOKEN_QUERY_SOURCE).
pub fn query_access(class: u32) -> u32 {
    if class == TOKEN_SOURCE { TOKEN_QUERY_SOURCE } else { TOKEN_QUERY }
}

/// Information class `class` of the token `id` holding `data`, laid out
/// for a buffer at `base`. INVALID_INFO_CLASS for classes gala does not
/// answer and for TokenImpersonationLevel of a primary token.
pub fn query(id: u64, data: &TokenData, class: u32, base: u64) -> Result<Vec<u8>, NtStatus> {
    let mut o = Out { b: Vec::new(), base };
    let dword = |v: u32| -> Result<Vec<u8>, NtStatus> {
        let mut b = Vec::new();
        b.try_extend_from_slice(&v.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(b)
    };
    match class {
        TOKEN_USER => o.sid_list(core::slice::from_ref(&data.user))?,
        TOKEN_GROUPS => o.groups(&data.groups)?,
        TOKEN_RESTRICTED_SIDS => o.groups(&data.restricted)?,
        TOKEN_LOGON_SID => {
            let mut logon = [Group { sid: LOGON_SID, attributes: 0 }; 1];
            let found =
                data.groups.iter().find(|g| g.attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID);
            match found {
                Some(g) => logon[0] = *g,
                None => return Err(NtStatus::NOT_FOUND),
            }
            o.groups(&logon)?
        }
        TOKEN_CAPABILITIES => o.groups(&[])?,
        TOKEN_PRIVILEGES => {
            o.u32(data.privileges.len() as u32)?;
            privileges(&mut o, &data.privileges)?;
        }
        TOKEN_OWNER => o.sid_pointer(&data.user.sid)?,
        TOKEN_PRIMARY_GROUP => o.sid_pointer(&data.primary_group)?,
        TOKEN_DEFAULT_DACL => {
            if data.default_dacl.is_empty() {
                o.u64(0)?;
            } else {
                let at = o.here() + 8;
                o.u64(at)?;
                o.put(&data.default_dacl)?;
            }
        }
        TOKEN_SOURCE => {
            o.put(&data.source.0)?;
            o.u64(data.source.1)?;
        }
        TOKEN_TYPE => return dword(if data.level().is_some() { 2 } else { 1 }),
        TOKEN_IMPERSONATION_LEVEL => match data.level() {
            Some(level) => return dword(level),
            None => return Err(NtStatus::INVALID_INFO_CLASS),
        },
        TOKEN_STATISTICS => {
            o.u64(id)?; // TokenId
            o.u64(data.authentication_id)?;
            o.u64(i64::MAX as u64)?; // ExpirationTime: never
            o.u32(if data.level().is_some() { 2 } else { 1 })?;
            o.u32(data.level().unwrap_or(0))?;
            o.u32(4096)?; // DynamicCharged
            o.u32(4096 - data.default_dacl.len() as u32)?; // DynamicAvailable
            o.u32(data.groups.len() as u32)?;
            o.u32(data.privileges.len() as u32)?;
            o.u64(data.modified_id)?;
        }
        TOKEN_GROUPS_AND_PRIVILEGES => {
            // SidCount, SidLength, Sids, RestrictedSidCount,
            // RestrictedSidLength, RestrictedSids, PrivilegeCount,
            // PrivilegeLength, Privileges, AuthenticationId.
            o.put(&[0; 56])?;
            let sids_at = o.b.len();
            let mut all = Vec::new();
            all.try_push(data.user).map_err(|_| NtStatus::NO_MEMORY)?;
            all.try_extend_from_slice(&data.groups).map_err(|_| NtStatus::NO_MEMORY)?;
            o.sid_list(&all)?;
            let sids_len = o.b.len() - sids_at;
            o.align(8)?;
            let restricted_at = o.b.len();
            o.sid_list(&data.restricted)?;
            let restricted_len = o.b.len() - restricted_at;
            o.align(4)?;
            let privileges_at = o.b.len();
            privileges(&mut o, &data.privileges)?;
            let privileges_len = o.b.len() - privileges_at;
            o.set_u32(0, all.len() as u32);
            o.set_u32(4, sids_len as u32);
            o.set_u64(8, base.wrapping_add(sids_at as u64));
            o.set_u32(16, data.restricted.len() as u32);
            o.set_u32(20, restricted_len as u32);
            let restricted_ptr = if data.restricted.is_empty() {
                0
            } else {
                base.wrapping_add(restricted_at as u64)
            };
            o.set_u64(24, restricted_ptr);
            o.set_u32(32, data.privileges.len() as u32);
            o.set_u32(36, privileges_len as u32);
            o.set_u64(40, base.wrapping_add(privileges_at as u64));
            o.set_u64(48, data.authentication_id);
        }
        TOKEN_SESSION_ID => return dword(data.session),
        TOKEN_SANDBOX_INERT => return dword(u32::from(data.sandbox_inert)),
        TOKEN_ELEVATION_TYPE => return dword(data.elevation_type),
        TOKEN_HAS_RESTRICTIONS => return dword(u32::from(!data.restricted.is_empty())),
        TOKEN_ELEVATION
        | TOKEN_VIRTUALIZATION_ALLOWED
        | TOKEN_VIRTUALIZATION_ENABLED
        | TOKEN_UI_ACCESS
        | TOKEN_IS_APP_CONTAINER
        | TOKEN_APP_CONTAINER_NUMBER => return dword(0),
        // TOKEN_MANDATORY_POLICY_NO_WRITE_UP | NEW_PROCESS_MIN.
        TOKEN_MANDATORY_POLICY => return dword(3),
        TOKEN_APP_CONTAINER_SID => o.u64(0)?,
        TOKEN_INTEGRITY_LEVEL => {
            // TOKEN_MANDATORY_LABEL: the label with SE_GROUP_INTEGRITY.
            let label = data.groups.iter().find(|g| g.sid.is_label()).map_or(
                Group { sid: Sid::new(16, &[8192]), attributes: SE_GROUP_INTEGRITY },
                |g| Group { sid: g.sid, attributes: SE_GROUP_INTEGRITY },
            );
            o.sid_list(&[label])?
        }
        _ => return Err(NtStatus::INVALID_INFO_CLASS),
    }
    Ok(o.b)
}

/// The SIDs of a TOKEN_GROUPS-style list: `entries` is its bytes up to
/// the end of its `count` SID_AND_ATTRIBUTES; `read(address, buffer)`
/// reads the caller's memory (the SIDs may be anywhere in it).
pub fn parse_groups(
    entries: &[u8],
    count: usize,
    read: impl Fn(u64, &mut [u8]) -> Result<(), NtStatus>,
) -> Result<Vec<Sid>, NtStatus> {
    let mut sids = Vec::new();
    for i in 0..count {
        let e = 8 + 16 * i;
        let ptr = entries.get(e..e + 8).ok_or(NtStatus::INVALID_PARAMETER)?;
        let ptr = u64::from_le_bytes(ptr.try_into().map_err(|_| NtStatus::INVALID_PARAMETER)?);
        let mut b = [0u8; 68];
        read(ptr, &mut b[..8])?;
        let n = 8 + 4 * usize::from(b[1].min(15));
        read(ptr + 8, &mut b[8..n])?;
        let sid = Sid::parse(&b[..n]).ok_or(NtStatus::INVALID_SID)?;
        sids.try_push(sid).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(sids)
}

/// LUID_AND_ATTRIBUTES entries from their bytes.
pub fn parse_privileges(raw: &[u8], count: usize) -> Result<Vec<Privilege>, NtStatus> {
    let mut list = Vec::new();
    for i in 0..count {
        let e = raw.get(12 * i..12 * i + 12).ok_or(NtStatus::INVALID_PARAMETER)?;
        let luid = u64::from_le_bytes(e[0..8].try_into().map_err(|_| NtStatus::INVALID_PARAMETER)?);
        let attributes = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        list.try_push(Privilege { luid, attributes }).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    fn rd64(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn sid_round_trip() {
        let mut b = [0u8; 68];
        USER.write(&mut b);
        assert_eq!(&b[..8], &[1, 5, 0, 0, 0, 0, 0, 5]);
        assert_eq!(Sid::parse(&b[..USER.len()]), Some(USER));
        assert_eq!(Sid::parse(&b[..USER.len() - 1]), None);
    }

    #[test]
    fn user_and_groups_point_into_the_buffer() {
        let data = TokenData::boot_user().unwrap();
        let base = 0x1000;
        let b = query(7, &data, TOKEN_USER, base).unwrap();
        assert_eq!(rd64(&b, 0), base + 16);
        assert_eq!(Sid::parse(&b[16..]), Some(USER));
        let b = query(7, &data, TOKEN_GROUPS, base).unwrap();
        assert_eq!(rd32(&b, 0), 10);
        let first = (rd64(&b, 8) - base) as usize;
        assert_eq!(first, 8 + 16 * 10);
        assert_eq!(Sid::parse(&b[first..]), Some(Sid::new(1, &[0])));
        let b = query(7, &data, TOKEN_STATISTICS, base).unwrap();
        assert_eq!(b.len(), 56);
        assert_eq!(rd64(&b, 0), 7);
        assert_eq!(rd64(&b, 8), LOGON_ID);
        assert_eq!(rd32(&b, 24), 1);
        assert!(query(7, &data, TOKEN_IMPERSONATION_LEVEL, base).is_err());
    }

    #[test]
    fn groups_and_privileges_lengths() {
        let data = TokenData::boot_user().unwrap();
        let base = 0x2000;
        let b = query(1, &data, TOKEN_GROUPS_AND_PRIVILEGES, base).unwrap();
        assert_eq!(rd32(&b, 0), 11);
        assert_eq!(rd64(&b, 8), base + 56);
        assert_eq!(rd32(&b, 32), 5);
        let p = (rd64(&b, 40) - base) as usize;
        assert_eq!(rd32(&b, 36), 60);
        assert_eq!(p + 60, b.len());
        assert_eq!(rd64(&b, p + 12), 23);
    }

    #[test]
    fn adjusting_privileges() {
        let mut data = TokenData::boot_user().unwrap();
        let before = data.modified_id;
        let on = [Privilege { luid: 19, attributes: SE_PRIVILEGE_ENABLED }];
        // Too small a previous state: nothing changes.
        assert_eq!(adjust_privileges(&mut data, false, &on, Some(4)).unwrap().err(), Some(16));
        assert_eq!(data.privileges[0].attributes, 0);
        let a = adjust_privileges(&mut data, false, &on, Some(16)).unwrap().ok().unwrap();
        assert_eq!(a.previous, [Privilege { luid: 19, attributes: 0 }]);
        assert!(!a.not_all_assigned);
        assert_eq!(data.privileges[0].attributes, SE_PRIVILEGE_ENABLED);
        assert_ne!(data.modified_id, before);
        // Again: no change, nothing previous.
        let a = adjust_privileges(&mut data, false, &on, Some(4)).unwrap().ok().unwrap();
        assert!(a.previous.is_empty());
        // Not held.
        let debug = [Privilege { luid: 20, attributes: SE_PRIVILEGE_ENABLED }];
        assert!(
            adjust_privileges(&mut data, false, &debug, None)
                .unwrap()
                .ok()
                .unwrap()
                .not_all_assigned
        );
        // Disable all: SeShutdown and SeChangeNotify.
        let a = adjust_privileges(&mut data, true, &[], None).unwrap().ok().unwrap();
        assert_eq!(a.previous.len(), 2);
        assert!(data.privileges.iter().all(|p| p.attributes & SE_PRIVILEGE_ENABLED == 0));
        // Removed.
        let gone = [Privilege { luid: 25, attributes: SE_PRIVILEGE_REMOVED }];
        adjust_privileges(&mut data, false, &gone, None).unwrap().ok().unwrap();
        assert!(!data.privileges.iter().any(|p| p.luid == 25));
    }

    #[test]
    fn checking_privileges() {
        let data = TokenData::boot_user().unwrap();
        let mut set =
            [Privilege { luid: 23, attributes: 0 }, Privilege { luid: 19, attributes: 0 }];
        assert!(!privilege_check(&data, &mut set, true));
        assert_eq!(set[0].attributes, SE_PRIVILEGE_USED_FOR_ACCESS);
        assert_eq!(set[1].attributes, 0);
        assert!(privilege_check(&data, &mut set, false));
    }

    #[test]
    fn duplicating_and_filtering() {
        let token = Token::new(TokenData::boot_user().unwrap());
        let imp =
            token.duplicate(TokenType::Impersonation(SECURITY_IDENTIFICATION), false).unwrap();
        assert_ne!(imp.id, token.id);
        assert!(matches!(
            imp.duplicate(TokenType::Impersonation(SECURITY_IMPERSONATION), false),
            Err(NtStatus::BAD_IMPERSONATION_LEVEL)
        ));
        assert!(imp.duplicate(TokenType::Primary, false).is_err());
        let effective = token.duplicate(TokenType::Primary, true).unwrap();
        assert_eq!(effective.data.lock().privileges.len(), 1);

        let everyone = Sid::new(1, &[0]);
        let filtered = token.filter(DISABLE_MAX_PRIVILEGE, &[everyone], &[], &[everyone]).unwrap();
        let d = filtered.data.lock();
        assert_eq!(d.privileges.len(), 1);
        assert_eq!(
            d.groups[0].attributes & (SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY),
            SE_GROUP_USE_FOR_DENY_ONLY
        );
        assert_eq!(d.restricted.len(), 1);
        // Restricting again intersects.
        drop(d);
        let local = Sid::new(2, &[0]);
        let again = filtered.filter(0, &[], &[], &[local, everyone]).unwrap();
        assert_eq!(again.data.lock().restricted.len(), 1);
        assert_eq!(token.data.lock().restricted.len(), 0);
    }

    #[test]
    fn parsing_user_lists() {
        let mut memory = [0u8; 12];
        Sid::new(1, &[0]).write(&mut memory);
        let read = |at: u64, out: &mut [u8]| {
            let at = at.checked_sub(0x5000).ok_or(NtStatus::ACCESS_VIOLATION)? as usize;
            let from = memory.get(at..at + out.len()).ok_or(NtStatus::ACCESS_VIOLATION)?;
            out.copy_from_slice(from);
            Ok(())
        };
        let mut entries = [0u8; 8 + 16];
        entries[0] = 1;
        entries[8..16].copy_from_slice(&0x5000u64.to_le_bytes());
        assert_eq!(parse_groups(&entries, 1, read).unwrap(), [Sid::new(1, &[0])]);
        entries[8..16].copy_from_slice(&0x4000u64.to_le_bytes());
        assert!(parse_groups(&entries, 1, read).is_err());
        let p = parse_privileges(&[19, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0], 1).unwrap();
        assert_eq!(p, [Privilege { luid: 19, attributes: 2 }]);
    }
}

//! Who a Linux program is (ARCHITECTURE "Linux identity"; user/lx/id_test
//! is the spec, recorded on Linux): the uid, gid and groups the Linux
//! calls report are a function of the process's NT token, so a Linux
//! program runs as the account its token is, as a Windows program does,
//! and `/etc/passwd` and `/etc/group` are made from gala's account
//! database (`nt::accounts`), not shipped.
//!
//! - **uid**: SYSTEM (S-1-5-18) is root, 0. A local account of this
//!   machine with RID 1001 or more (gala's accounts start at the boot
//!   user's 1001) is RID - 1, so the boot user `gala` is 1000, Linux's
//!   first user. Every other SID (another machine's or a service's
//!   account, a local RID under 1001) is `nobody`, 65534, as Linux shows
//!   an id it cannot map.
//! - **gid**: a user's own group (Linux's user private groups) of the
//!   same number; root's is 0, nobody's `nogroup` 65534.
//! - **Supplementary groups**: the primary gid, then each enabled token
//!   group that has a Linux name: BUILTIN\Administrators is `sudo` (27),
//!   BUILTIN\Users `users` (100).
//! - **The user database**: [`publish`] writes `/etc/passwd` and
//!   `/etc/group` from the accounts at boot, as files of a volume of
//!   their own that the Linux walk binds over `/etc/passwd` and
//!   `/etc/group` (`lx::file`'s mounts: root's 0644 files, never in the
//!   change log, never `C:\etc\passwd`). A user's home is their profile,
//!   `/Users/<name>` (`C:\Users\<name>`, made for the boot user if no
//!   module made it); root's is SYSTEM's,
//!   `/Windows/System32/config/systemprofile`; the shell `/bin/sh`.
//!
//! Observability: `publish` logs what it wrote (`lx: identity: ...`, the
//! boot user's uid), and the `lx_id` scenario expects that line.

use alloc::vec::Vec;

use crate::mm::heap::TryVec;
use crate::nt::accounts;
use crate::nt::object::{ObjectBody, ObjectRef};
use crate::nt::process::Process;
use crate::nt::status::NtStatus;
use crate::nt::token::{self, SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY, Sid, TokenData};

/// Linux's id for what it cannot map (and `nogroup`'s).
pub const NOBODY: u32 = 65534;
/// The first local RID with a Linux uid (the boot user's; uid 1000).
const FIRST_RID: u32 = 1001;
/// The groups with Linux names: (alias, gid, name).
const NAMED_GROUPS: [(Sid, u32, &str); 2] =
    [(Sid::new(5, &[32, 544]), 27, "sudo"), (Sid::new(5, &[32, 545]), 100, "users")];
/// The most supplementary groups reported (the primary and the named).
pub const MAX_GROUPS: usize = 1 + NAMED_GROUPS.len();

/// A Linux program's ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub uid: u32,
    pub gid: u32,
    groups: [u32; MAX_GROUPS],
    count: usize,
}

impl Ids {
    /// The supplementary groups (getgroups), the primary first.
    pub fn groups(&self) -> &[u32] {
        &self.groups[..self.count]
    }
}

/// The uid of account SID `sid`.
pub fn uid_of(sid: &Sid) -> u32 {
    if *sid == token::LOCAL_SYSTEM {
        return 0;
    }
    match accounts::local_rid(sid) {
        Some(rid) if rid >= FIRST_RID => rid - 1,
        _ => NOBODY,
    }
}

/// The ids of a token's holder.
pub fn of_token(t: &TokenData) -> Ids {
    let uid = uid_of(&t.user.sid);
    let gid = uid;
    let mut ids = Ids { uid, gid, groups: [0; MAX_GROUPS], count: 1 };
    ids.groups[0] = gid;
    for (sid, g, _) in NAMED_GROUPS {
        let held = t.groups.iter().any(|x| {
            x.sid == sid
                && x.attributes & SE_GROUP_ENABLED != 0
                && x.attributes & SE_GROUP_USE_FOR_DENY_ONLY == 0
        });
        if held && ids.count < MAX_GROUPS && !ids.groups().contains(&g) {
            ids.groups[ids.count] = g;
            ids.count += 1;
        }
    }
    ids
}

/// The ids of token object `token` (nobody's if it is not a token).
pub fn of_token_object(token: &ObjectRef) -> Ids {
    match &token.body {
        ObjectBody::Token(t) => of_token(&t.data.lock()),
        _ => Ids { uid: NOBODY, gid: NOBODY, groups: [NOBODY; MAX_GROUPS], count: 1 },
    }
}

/// Ids of `uid` with the one group `gid` (host tests).
#[cfg(test)]
pub fn test_ids(uid: u32, gid: u32) -> Ids {
    let mut groups = [0; MAX_GROUPS];
    groups[0] = gid;
    Ids { uid, gid, groups, count: 1 }
}

/// The ids process `p` runs as (its primary token's).
pub fn of_process(p: &Process) -> Ids {
    let token = p.token.lock().clone();
    of_token_object(&token)
}

/// The boot user's ids (what the kernel's own Linux-facing servers, the
/// Wayland server, present as).
pub fn boot_user() -> Ids {
    match TokenData::boot_user() {
        Ok(t) => of_token(&t),
        Err(_) => Ids { uid: NOBODY, gid: NOBODY, groups: [NOBODY; MAX_GROUPS], count: 1 },
    }
}

/// The user database's two files, made from the accounts.
pub struct Database {
    pub passwd: Vec<u8>,
    pub group: Vec<u8>,
}

fn push(out: &mut Vec<u8>, s: &[u8]) -> Result<(), NtStatus> {
    out.try_extend_from_slice(s).map_err(|_| NtStatus::NO_MEMORY)
}

fn push_num(out: &mut Vec<u8>, mut v: u32) -> Result<(), NtStatus> {
    let mut digits = [0u8; 10];
    let mut n = 0;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    digits[..n].reverse();
    push(out, &digits[..n])
}

/// An account name as Linux shows it: ASCII as is (lower case kept as
/// given), anything else `_`.
fn push_name(out: &mut Vec<u8>, name: &[u16]) -> Result<(), NtStatus> {
    for &c in name {
        let b = if c < 128 && c != u16::from(b':') && c > 32 { c as u8 } else { b'_' };
        out.try_push(b).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(())
}

/// `/etc/passwd` and `/etc/group` for `users` ((name, RID), the local
/// accounts in RID order).
pub fn database(users: &[(Vec<u16>, u32)]) -> Result<Database, NtStatus> {
    let mut passwd = Vec::new();
    let mut group = Vec::new();
    push(&mut passwd, b"root:x:0:0:root:/Windows/System32/config/systemprofile:/bin/sh\n")?;
    push(&mut group, b"root:x:0:\n")?;
    let mapped = || users.iter().filter(|(_, rid)| *rid >= FIRST_RID);
    for (name, rid) in mapped() {
        let id = rid - 1;
        for (out, gecos) in [(&mut passwd, true), (&mut group, false)] {
            push_name(out, name)?;
            push(out, b":x:")?;
            push_num(out, id)?;
            push(out, b":")?;
            if gecos {
                push_num(out, id)?;
                push(out, b":")?;
                push_name(out, name)?;
                push(out, b":/Users/")?;
                push_name(out, name)?;
                push(out, b":/bin/sh")?;
            }
            push(out, b"\n")?;
        }
    }
    for (_, gid, gname) in NAMED_GROUPS {
        push(&mut group, gname.as_bytes())?;
        push(&mut group, b":x:")?;
        push_num(&mut group, gid)?;
        push(&mut group, b":")?;
        if gid == 100 {
            // Every local user is in BUILTIN\Users.
            for (i, (name, _)) in mapped().enumerate() {
                if i > 0 {
                    push(&mut group, b",")?;
                }
                push_name(&mut group, name)?;
            }
        }
        push(&mut group, b"\n")?;
    }
    push(&mut passwd, b"nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n")?;
    push(&mut group, b"nogroup:x:65534:\n")?;
    Ok(Database { passwd, group })
}

/// Makes the user database from the accounts and publishes it at
/// `/etc/passwd` and `/etc/group` (`lx::file::publish_identity`), and
/// the boot user's profile directory if no module made it. At boot,
/// after the registry's modules (the accounts) are in.
pub fn publish() -> Result<(), NtStatus> {
    let users = accounts::local_users()?;
    let db = database(&users)?;
    super::file::publish_identity(&db.passwd, &db.group)?;
    super::file::ensure_profile(b"gala")?;
    crate::info!(
        "lx: identity: {} local accounts in /etc/passwd ({} bytes), the boot user uid {}",
        users.len(),
        db.passwd.len(),
        boot_user().uid
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nt::token::Group;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn accounts_map_to_linux_ids() {
        assert_eq!(uid_of(&token::LOCAL_SYSTEM), 0);
        assert_eq!(uid_of(&token::USER), 1000);
        assert_eq!(uid_of(&accounts::user_sid(1002)), 1001);
        assert_eq!(uid_of(&accounts::user_sid(500)), NOBODY);
        // LocalService, another machine's account.
        assert_eq!(uid_of(&Sid::new(5, &[19])), NOBODY);
        assert_eq!(uid_of(&Sid::new(5, &[21, 1, 2, 3, 1001])), NOBODY);
    }

    #[test]
    fn the_boot_user_is_1000_in_users() {
        let ids = boot_user();
        assert_eq!((ids.uid, ids.gid), (1000, 1000));
        assert_eq!(ids.groups(), [1000, 100]);
        let system = of_token(&TokenData::system().unwrap());
        assert_eq!((system.uid, system.gid), (0, 0));
        assert_eq!(system.groups(), [0, 27]);
    }

    #[test]
    fn a_deny_only_group_is_not_a_linux_group() {
        let mut t = TokenData::boot_user().unwrap();
        for g in t.groups.iter_mut() {
            if g.sid == Sid::new(5, &[32, 545]) {
                *g = Group { sid: g.sid, attributes: SE_GROUP_USE_FOR_DENY_ONLY };
            }
        }
        assert_eq!(of_token(&t).groups(), [1000]);
    }

    #[test]
    fn the_database_lists_root_the_users_and_nobody() {
        let users = [(w("gala"), 1001), (w("galatest"), 1002), (w("Administrator"), 500)];
        let db = database(&users).unwrap();
        assert_eq!(
            core::str::from_utf8(&db.passwd).unwrap(),
            "root:x:0:0:root:/Windows/System32/config/systemprofile:/bin/sh\n\
             gala:x:1000:1000:gala:/Users/gala:/bin/sh\n\
             galatest:x:1001:1001:galatest:/Users/galatest:/bin/sh\n\
             nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n"
        );
        assert_eq!(
            core::str::from_utf8(&db.group).unwrap(),
            "root:x:0:\ngala:x:1000:\ngalatest:x:1001:\nsudo:x:27:\n\
             users:x:100:gala,galatest\nnogroup:x:65534:\n"
        );
    }
}

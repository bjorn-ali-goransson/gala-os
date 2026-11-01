//! Security descriptors on objects (docs/SECURITY.md, phase S2):
//! NtQuerySecurityObject and NtSetSecurityObject over `nt::security`,
//! and what object creation assigns. Files keep their descriptor on the
//! ramfs node, keys on the registry node, other objects on the KObject.
//! Objects nothing assigned one to (boot files and keys, processes,
//! threads, unnamed objects created without one) report a default.
//! Service numbers are gala's until verified (reached through ntdll).

use super::super::file;
use super::super::object::{FILE_MAPPING, Handle, KObject, ObjectBody, ObjectRef};
use super::super::registry::{self, KEY_MAPPING};
use super::super::security::{
    self, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, Descriptor, GROUP_SECURITY_INFORMATION,
    LABEL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    SACL_SECURITY_INFORMATION, SE_DACL_PRESENT,
};
use super::super::status::NtStatus;
use super::super::thread;
use super::super::token::{LOCAL_SYSTEM, Sid, TokenData};
use super::Call;
use crate::fs::ramfs::NodeId;

const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;

const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const ADMINISTRATORS: Sid = Sid::new(5, &[32, 544]);
const USERS: Sid = Sid::new(5, &[32, 545]);

/// Runs `f` on the effective token of the caller: its thread's
/// impersonation token, else its process's primary token.
fn with_token<R>(call: &Call<'_>, f: impl FnOnce(&TokenData) -> R) -> R {
    let impersonation = thread::current().and_then(|me| match &me.body {
        ObjectBody::Thread(t) => t.impersonation.lock().clone(),
        _ => None,
    });
    let token = impersonation.unwrap_or_else(|| call.process.token.lock().clone());
    match &token.body {
        ObjectBody::Token(t) => f(&t.data.lock()),
        _ => unreachable!("tokens are token objects"),
    }
}

/// What files and keys without a descriptor of their own (those the
/// boot puts there) have: fixed, whoever asks (as Windows' system files
/// are): owner Administrators, all access for the boot user `gala`,
/// Administrators and SYSTEM, reading and executing for BUILTIN\\Users
/// (inherited by children with `flags`).
fn default_for(all: u32, read: u32, flags: u8) -> Result<Descriptor, NtStatus> {
    let user = super::super::token::USER;
    let dacl = security::allow_acl(&[
        (all, user, flags),
        (all, ADMINISTRATORS, flags),
        (all, LOCAL_SYSTEM, flags),
        (read, USERS, flags),
    ])?;
    Ok(Descriptor {
        control: SE_DACL_PRESENT,
        owner: Some(ADMINISTRATORS),
        group: Some(LOCAL_SYSTEM),
        dacl: Some(dacl),
        sacl: None,
    })
}

fn file_default(directory: bool) -> Result<Descriptor, NtStatus> {
    const FILE_GENERIC_READ_EXECUTE: u32 = 0x0012_00a9;
    let flags = if directory { OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE } else { 0 };
    default_for(FILE_ALL_ACCESS, FILE_GENERIC_READ_EXECUTE, flags)
}

fn key_default() -> Result<Descriptor, NtStatus> {
    const KEY_READ: u32 = 0x0002_0019;
    default_for(KEY_ALL_ACCESS, KEY_READ, CONTAINER_INHERIT_ACE)
}

/// The descriptor of file system node `node` (its own or the default).
fn node_descriptor(_token: &TokenData, node: NodeId) -> Result<Descriptor, NtStatus> {
    let stored = file::with_fs(|fs| {
        fs.node(node).map(|n| (n.security.as_deref().map(Descriptor::parse), n.is_dir()))
    });
    match stored {
        Some((Some(d), _)) => d,
        Some((None, directory)) => file_default(directory),
        None => Err(NtStatus::FILE_DELETED),
    }
}

fn key_descriptor(_token: &TokenData, id: registry::KeyId) -> Result<Descriptor, NtStatus> {
    let stored = registry::with(|r| Ok(r.security(id)?.0.map(Descriptor::parse)))?;
    match stored {
        Some(d) => d,
        None => key_default(),
    }
}

/// The descriptor `object` has (its own or the default).
fn descriptor_of(token: &TokenData, object: &ObjectRef) -> Result<Descriptor, NtStatus> {
    match &object.body {
        ObjectBody::File(f) => node_descriptor(token, f.node),
        ObjectBody::Key(k) => key_descriptor(token, k.id),
        _ => match object.security.lock().as_deref() {
            Some(b) => Descriptor::parse(b),
            None => security::assign(None, None, false, token, &object.mapping(), false),
        },
    }
}

fn store(object: &ObjectRef, d: &Descriptor) -> Result<(), NtStatus> {
    let bytes = d.to_bytes(u32::MAX)?;
    match &object.body {
        ObjectBody::File(f) => {
            file::with_fs(|fs| fs.set_security(f.node, bytes)).map_err(|_| NtStatus::FILE_DELETED)
        }
        ObjectBody::Key(k) => registry::with(|r| r.set_security(k.id, bytes)),
        _ => {
            *object.security.lock() = Some(bytes);
            Ok(())
        }
    }
}

/// The access NtQuerySecurityObject (`set` false) or
/// NtSetSecurityObject needs for `info`.
fn needed_access(info: u32, set: bool) -> u32 {
    let mut access = 0;
    if info & (OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION)
        != 0
    {
        access |= if set { WRITE_OWNER } else { READ_CONTROL };
    }
    if info & DACL_SECURITY_INFORMATION != 0 {
        access |= if set { WRITE_DAC } else { READ_CONTROL };
    }
    if info & SACL_SECURITY_INFORMATION != 0 {
        access |= ACCESS_SYSTEM_SECURITY;
    }
    access
}

impl Call<'_> {
    /// The descriptor at `ptr` in the caller's memory.
    fn capture_descriptor(&self, ptr: u64) -> Result<Descriptor, NtStatus> {
        Descriptor::capture(ptr, |at, out| {
            self.process.read(at, out).map_err(|_| NtStatus::ACCESS_VIOLATION)
        })
    }

    /// OBJECT_ATTRIBUTES.SecurityDescriptor, captured (None: none given).
    pub(super) fn attributes_descriptor(
        &self,
        attrs_ptr: u64,
    ) -> Result<Option<Descriptor>, NtStatus> {
        if attrs_ptr == 0 {
            return Ok(None);
        }
        match self.read_u64(attrs_ptr + 32)? {
            0 => Ok(None),
            sd => Ok(Some(self.capture_descriptor(sd)?)),
        }
    }
}

/// Assigns a new kernel object (not a file or key) its descriptor: the
/// given one completed with the creator's owner and group, else the
/// token's default DACL (kept for named objects, which others may open by
/// name; an unnamed one reports it when asked).
pub(super) fn assign_object(
    call: &Call<'_>,
    object: &KObject,
    attrs_ptr: u64,
) -> Result<(), NtStatus> {
    let given = call.attributes_descriptor(attrs_ptr)?;
    let named = attrs_ptr != 0 && call.read_u64(attrs_ptr + 16)? != 0;
    if given.is_none() && !named {
        return Ok(());
    }
    let mapping = object.mapping();
    let d = with_token(call, |t| security::assign(None, given, false, t, &mapping, false))?;
    *object.security.lock() = Some(d.to_bytes(u32::MAX)?);
    Ok(())
}

/// Assigns a file or directory just created its descriptor, inheriting
/// from its parent directory (automatic inheritance).
pub(super) fn assign_file(
    call: &Call<'_>,
    object: &ObjectRef,
    attrs_ptr: u64,
) -> Result<(), NtStatus> {
    let ObjectBody::File(f) = &object.body else { return Ok(()) };
    let given = call.attributes_descriptor(attrs_ptr)?;
    let (parent, directory) = file::with_fs(|fs| fs.node(f.node).map(|n| (n.parent(), n.is_dir())))
        .ok_or(NtStatus::FILE_DELETED)?;
    let d = with_token(call, |t| -> Result<Descriptor, NtStatus> {
        let parent = node_descriptor(t, parent)?;
        security::assign(Some(&parent), given, directory, t, &FILE_MAPPING, true)
    })?;
    store(object, &d)
}

/// Assigns a registry key just created its descriptor, inheriting from
/// its parent key.
pub(super) fn assign_key(
    call: &Call<'_>,
    id: registry::KeyId,
    attrs_ptr: u64,
) -> Result<(), NtStatus> {
    let given = call.attributes_descriptor(attrs_ptr)?;
    let parent = registry::with(|r| Ok(r.security(id)?.1))?;
    let d = with_token(call, |t| -> Result<Descriptor, NtStatus> {
        let parent = key_descriptor(t, parent)?;
        security::assign(Some(&parent), given, true, t, &KEY_MAPPING, true)
    })?;
    let bytes = d.to_bytes(u32::MAX)?;
    registry::with(|r| r.set_security(id, bytes))
}

/// NtQuerySecurityObject(Handle, SecurityInformation, SecurityDescriptor,
/// Length, *LengthNeeded): the parts asked for, self-relative;
/// STATUS_BUFFER_TOO_SMALL with the size when they do not fit.
pub(super) fn nt_query_security_object(call: &mut Call<'_>) -> NtStatus {
    let (h, info, buffer, length, needed) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3) as u32, arg!(call, 4));
    let entry = tri!(call.handle(Handle(h)));
    tri!(entry.require(needed_access(info, false)));
    let d = tri!(with_token(call, |t| descriptor_of(t, &entry.object)));
    let bytes = tri!(d.to_bytes(info));
    if needed != 0 {
        tri!(call.write_u32(needed, bytes.len() as u32));
    }
    if (length as usize) < bytes.len() {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    match call.process.write(buffer, &bytes) {
        Ok(()) => NtStatus::SUCCESS,
        Err(_) => NtStatus::ACCESS_VIOLATION,
    }
}

/// NtSetSecurityObject(Handle, SecurityInformation, SecurityDescriptor):
/// the parts `info` names replace the object's.
pub(super) fn nt_set_security_object(call: &mut Call<'_>) -> NtStatus {
    let (h, info, ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let entry = tri!(call.handle(Handle(h)));
    tri!(entry.require(needed_access(info, true)));
    if ptr == 0 {
        return NtStatus::ACCESS_VIOLATION;
    }
    let new = tri!(call.capture_descriptor(ptr));
    let result = with_token(call, |t| -> Result<Descriptor, NtStatus> {
        let mut d = descriptor_of(t, &entry.object)?;
        security::set(&mut d, info, new, t)?;
        Ok(d)
    });
    let d = tri!(result);
    tri!(store(&entry.object, &d));
    NtStatus::SUCCESS
}

/// Runs `f` on the token opens are checked against: the thread's
/// impersonation token (STATUS_BAD_IMPERSONATION_LEVEL below impersonation
/// level), else the process's.
fn with_access_token<R>(
    call: &Call<'_>,
    f: impl FnOnce(&TokenData) -> Result<R, NtStatus>,
) -> Result<R, NtStatus> {
    with_token(call, |t| {
        if t.level().is_some_and(|l| l < super::super::token::SECURITY_IMPERSONATION) {
            return Err(NtStatus::BAD_IMPERSONATION_LEVEL);
        }
        f(t)
    })
}

/// The access an open of `object` (not a file or key) with `desired` is
/// granted (STATUS_ACCESS_DENIED).
pub(super) fn check_object(
    call: &Call<'_>,
    object: &ObjectRef,
    desired: u32,
) -> Result<u32, NtStatus> {
    with_access_token(call, |t| {
        let d = descriptor_of(t, object)?;
        security::access_check(&d, t, desired, &object.mapping(), false)
    })
}

/// The access an open of key `id` is granted.
pub(super) fn check_key(
    call: &Call<'_>,
    id: registry::KeyId,
    desired: u32,
    backup: bool,
) -> Result<u32, NtStatus> {
    with_access_token(call, |t| {
        let d = key_descriptor(t, id)?;
        security::access_check(&d, t, desired, &KEY_MAPPING, backup)
    })
}

/// Creating a subkey of `parent` needs KEY_CREATE_SUB_KEY on it.
pub(super) fn check_key_create(call: &Call<'_>, parent: registry::KeyId) -> Result<(), NtStatus> {
    const KEY_CREATE_SUB_KEY: u32 = 0x0004;
    check_key(call, parent, KEY_CREATE_SUB_KEY, false).map(drop)
}

// Directory and file rights the checks use.
const FILE_LIST_DIRECTORY: u32 = 0x0001;
const FILE_ADD_FILE: u32 = 0x0002;
const FILE_ADD_SUBDIRECTORY: u32 = 0x0004;
const FILE_DELETE_CHILD: u32 = 0x0040;
const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const DELETE: u32 = 0x0001_0000;
const FILE_DIRECTORY_FILE: u32 = 0x0001;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;

/// The access NtCreateFile/NtOpenFile of `path` with `desired` is granted:
/// an existing file's descriptor decides, where its parent directory
/// adds DELETE (FILE_DELETE_CHILD) and FILE_READ_ATTRIBUTES
/// (FILE_LIST_DIRECTORY), as documented; creating one needs FILE_ADD_FILE
/// (FILE_ADD_SUBDIRECTORY for a directory) on the parent and grants what
/// was asked. Backup intent lets SeBackup/SeRestore grant access.
pub(super) fn file_access(
    call: &Call<'_>,
    path: &file::FilePath,
    desired: u32,
    disposition: u32,
    options: u32,
) -> Result<u32, NtStatus> {
    let (node, parent) = file::lookup_path(path)?;
    let backup = options & FILE_OPEN_FOR_BACKUP_INTENT != 0;
    let maximum = desired & security::MAXIMUM_ALLOWED != 0;
    with_access_token(call, |t| {
        let parent_grants = match parent {
            Some(p) => {
                let pd = node_descriptor(t, p)?;
                security::access_check(&pd, t, security::MAXIMUM_ALLOWED, &FILE_MAPPING, backup)
                    .unwrap_or(0)
            }
            None => 0,
        };
        let Some(node) = node else {
            if parent.is_none() || matches!(disposition, file::FILE_OPEN | file::FILE_OVERWRITE) {
                // Missing: the open reports it.
                return Ok(FILE_MAPPING.map(desired));
            }
            let need = if options & FILE_DIRECTORY_FILE != 0 {
                FILE_ADD_SUBDIRECTORY
            } else {
                FILE_ADD_FILE
            };
            if parent_grants & need == 0 {
                return Err(NtStatus::ACCESS_DENIED);
            }
            return Ok(FILE_MAPPING.map(desired));
        };
        let d = node_descriptor(t, node)?;
        let mut extra = 0;
        if parent_grants & FILE_DELETE_CHILD != 0 {
            extra |= DELETE;
        }
        if parent_grants & FILE_LIST_DIRECTORY != 0 {
            extra |= FILE_READ_ATTRIBUTES;
        }
        let wanted = security::map_mask(desired & !security::MAXIMUM_ALLOWED, &FILE_MAPPING);
        let rest = (desired & security::MAXIMUM_ALLOWED) | (wanted & !extra);
        match security::access_check(&d, t, rest, &FILE_MAPPING, backup) {
            Ok(g) => Ok(g | (extra & if maximum { u32::MAX } else { wanted })),
            Err(NtStatus::ACCESS_DENIED)
                if wanted & !extra == 0 && (extra & wanted != 0 || maximum && extra != 0) =>
            {
                Ok(extra & if maximum { u32::MAX } else { wanted })
            }
            Err(e) => Err(e),
        }
    })
}

//! The registry services (Nt*Key) over `nt::registry`.
//!
//! A key is named by OBJECT_ATTRIBUTES: an absolute `\Registry\...` name,
//! or a name relative to the key handle in RootDirectory. Information
//! classes follow the documented KEY_*_INFORMATION and
//! KEY_VALUE_*_INFORMATION layouts; a buffer smaller than the structure's
//! fixed part is STATUS_BUFFER_TOO_SMALL, one smaller than the whole is
//! STATUS_BUFFER_OVERFLOW (with what fits), and ResultLength is always the
//! size needed.

use alloc::vec::Vec;

use super::super::object::{DELETE, Handle, KObject, ObjectBody, ObjectRef};
use super::super::platform;
use super::super::registry::{
    self, KEY_CREATE_SUB_KEY, KEY_ENUMERATE_SUB_KEYS, KEY_NOTIFY, KEY_QUERY_VALUE, KEY_SET_VALUE,
    KEY_WOW64_RES, KeyId, KeyObject, REG_CREATED_NEW_KEY, REG_OPENED_EXISTING_KEY, Registry,
};
use super::super::regview;
use super::super::status::NtStatus;
use super::{Call, HANDLE_ATTRIBUTES};
use crate::mm::heap::TryVec;

/// What a key's OBJECT_ATTRIBUTES name (Call::key_attributes).
struct KeyName {
    /// The RootDirectory key, kept while the name is resolved.
    _root: Option<ObjectRef>,
    start: KeyId,
    /// The name below `start`.
    name: Vec<u16>,
    attributes: u32,
    /// The whole name as the caller spelled it.
    spelled: Vec<u16>,
}

/// The longest value data accepted (NtSetValueKey).
const MAX_VALUE_DATA: usize = 16 << 20;

impl Call<'_> {
    /// The key a handle refers to, if it grants `access`. The object keeps
    /// the key while the caller works with it.
    fn key(&self, h: u64, access: u32) -> Result<(ObjectRef, KeyId), NtStatus> {
        let entry = self.handle(Handle(h))?;
        let object = entry.require(access)?.clone();
        let ObjectBody::Key(k) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
        let id = k.id;
        Ok((object, id))
    }

    /// The key OBJECT_ATTRIBUTES start from and the rest of the name: a
    /// RootDirectory key, or the root for an absolute `\Registry` name.
    /// The last part is the whole name as the caller spelled it (the
    /// RootDirectory key's name as it was opened, then this name), which
    /// the opened key's KeyNameInformation echoes.
    fn key_attributes(&self, ptr: u64) -> Result<KeyName, NtStatus> {
        let (root, mut name, attributes) =
            self.raw_object_attributes(ptr)?.ok_or(NtStatus::INVALID_PARAMETER)?;
        if root == 0 {
            let spelled = registry::copy_name(&name)?;
            let rest = Registry::absolute(&name)?;
            let skip = name.len() - rest.len();
            name.drain(..skip);
            return Ok(KeyName { _root: None, start: registry::ROOT, name, attributes, spelled });
        }
        if name.first() == Some(&(b'\\' as u16)) {
            return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
        }
        let (object, id) = self.key(root, 0)?;
        let ObjectBody::Key(k) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
        let mut spelled = registry::copy_name(&k.name)?;
        if spelled.is_empty() {
            spelled = registry::with(|r| r.full_name(id))?;
        }
        if !name.is_empty() {
            spelled.try_push(u16::from(b'\\')).map_err(|_| NtStatus::NO_MEMORY)?;
            spelled.try_extend_from_slice(&name).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(KeyName { _root: Some(object), start: id, name, attributes, spelled })
    }

    /// `name` below `start` in the caller's view of the registry
    /// (nt::regview): a WOW64 process's names are in the 32-bit view
    /// unless `access` has KEY_WOW64_64KEY, any process's with
    /// KEY_WOW64_32KEY. The redirected name is relative to the root; the
    /// third part is the length of its prefix through WOW6432Node (0 if
    /// it has none).
    fn key_view(
        &self,
        access: u32,
        start: KeyId,
        name: Vec<u16>,
    ) -> Result<(KeyId, Vec<u16>, usize), NtStatus> {
        const KEY_WOW64_64KEY: u32 = 0x0100;
        const KEY_WOW64_32KEY: u32 = 0x0200;
        let wow64 = self.process.wow64.is_some();
        if access & KEY_WOW64_32KEY == 0 && (!wow64 || access & KEY_WOW64_64KEY != 0) {
            return Ok((start, name, 0));
        }
        let full = registry::with(|r| {
            let mut full = r.full_name(start)?;
            if !name.is_empty() {
                full.try_push(u16::from(b'\\')).map_err(|_| NtStatus::NO_MEMORY)?;
                full.try_extend_from_slice(&name).map_err(|_| NtStatus::NO_MEMORY)?;
            }
            Ok(full)
        })?;
        match regview::redirect(&full).map_err(|_| NtStatus::NO_MEMORY)? {
            Some(r) => Ok((registry::ROOT, r.path, r.node)),
            None => Ok((start, name, 0)),
        }
    }

    /// Makes a key object for `id` (already retained) and returns a handle.
    /// return_key after the access check of an open of an existing key (on
    /// failure the key is released).
    fn return_checked_key(
        &self,
        out: u64,
        id: KeyId,
        access: u32,
        attributes: u32,
        spelled: &[u16],
    ) -> NtStatus {
        match super::security_calls::check_key(self, id, access & !KEY_WOW64_RES, false) {
            Ok(granted) => self.return_key(out, id, granted, attributes, spelled),
            Err(e) => {
                let _ = registry::with(|r| {
                    r.release(id);
                    Ok(())
                });
                e
            }
        }
    }

    fn return_key(
        &self,
        out: u64,
        id: KeyId,
        access: u32,
        attributes: u32,
        spelled: &[u16],
    ) -> NtStatus {
        // The name it was opened by, where it names this key's own
        // components (registry::display_name). Without memory for it the
        // key is still opened, under its stored name.
        let name = registry::with(|r| registry::display_name(&r.full_name(id)?, spelled))
            .unwrap_or_default();
        // On failure the key object is dropped here, outside the registry
        // lock, which releases the node.
        let Ok(object) = KObject::new(ObjectBody::Key(KeyObject { id, name })) else {
            return NtStatus::NO_MEMORY;
        };
        self.return_handle(out, object, access & !KEY_WOW64_RES, attributes & HANDLE_ATTRIBUTES)
    }

    /// Writes as much of an information structure as `length` allows.
    fn return_partial(
        &self,
        data: &[u8],
        fixed: usize,
        buffer: u64,
        length: u64,
        ret_ptr: u64,
    ) -> NtStatus {
        if ret_ptr != 0 {
            tri!(self.write_u32(ret_ptr, data.len() as u32));
        }
        let (n, status) = registry::fit(data.len(), fixed, length as usize);
        if n > 0 && self.process.write(buffer, &data[..n]).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
        status
    }

    /// A value name (an absent UNICODE_STRING is the unnamed value).
    fn value_name(&self, ptr: u64) -> Result<Vec<u16>, NtStatus> {
        if ptr == 0 { Ok(Vec::new()) } else { self.unicode_string(ptr) }
    }
}

/// NtOpenKey(*KeyHandle, DesiredAccess, ObjectAttributes)
pub(super) fn nt_open_key(call: &mut Call<'_>) -> NtStatus {
    open_key(call)
}

/// NtOpenKeyEx(*KeyHandle, DesiredAccess, ObjectAttributes, OpenOptions)
pub(super) fn nt_open_key_ex(call: &mut Call<'_>) -> NtStatus {
    open_key(call)
}

fn open_key(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let KeyName { _root, start, name, attributes, spelled } =
        tri!(call.key_attributes(arg!(call, 2)));
    let (start, name, _) = tri!(call.key_view(access, start, name));
    let found = registry::with(|r| {
        let id = r.lookup(start, &name)?;
        r.retain(id);
        Ok(id)
    });
    let id = match found {
        Ok(id) => id,
        Err(status) => {
            if status == NtStatus::OBJECT_NAME_NOT_FOUND {
                note_absent(start, &name);
            }
            emit_key(call, start, &name, status);
            return status;
        }
    };
    let status = call.return_checked_key(out, id, access, attributes, &spelled);
    emit_key(call, start, &name, status);
    status
}

/// The `key` event of a key open (`events=file`): the key it is relative
/// to (`base`, its full name), the name and the status, in order with the file opens (the ledger keeps only each
/// absent key's first time: which open came last before a failure needs
/// this).
fn emit_key(call: &Call<'_>, start: KeyId, name: &[u16], status: NtStatus) {
    use crate::events::{Category, Utf16, emit, enabled};
    if !enabled(Category::File) {
        return;
    }
    let base = registry::with(|r| r.full_name(start)).unwrap_or_default();
    emit(
        Category::File,
        "key",
        format_args!(
            r#""pid":{},"base":{},"path":{},"status":{}"#,
            call.process.id,
            Utf16(&base),
            Utf16(name),
            status.0
        ),
    );
}

/// Records an absent key in the ledger ("registry:MACHINE/SOFTWARE/...",
/// the end of the name if it is long): the keys real programs look for.
fn note_absent(start: KeyId, name: &[u16]) {
    let mut path = [0u8; 160];
    let mut n = 0;
    let mut push = |c: u16| {
        if n < path.len() {
            let b = match c {
                0x5c => b'/',
                0x20..=0x7e if c != 0x22 => c as u8,
                _ => b'?',
            };
            path[n] = b;
            n += 1;
        }
    };
    let base = registry::with(|r| r.full_name(start)).unwrap_or_default();
    // Without the leading "\REGISTRY\".
    for &c in base.iter().skip(10) {
        push(c);
    }
    if !base.is_empty() && base.len() > 9 {
        push(0x5c);
    }
    for &c in name {
        push(c);
    }
    const PREFIX: &[u8] = b"registry:";
    let keep = 96 - PREFIX.len();
    let tail = &path[n.saturating_sub(keep)..n];
    let mut entry = [0u8; 96];
    entry[..PREFIX.len()].copy_from_slice(PREFIX);
    entry[PREFIX.len()..PREFIX.len() + tail.len()].copy_from_slice(tail);
    super::super::ledger::record_user(&entry[..PREFIX.len() + tail.len()], "absent key", 0);
}

/// NtCreateKey(*KeyHandle, DesiredAccess, ObjectAttributes, TitleIndex,
///             *Class, CreateOptions, *Disposition)
pub(super) fn nt_create_key(call: &mut Call<'_>) -> NtStatus {
    let (out, access) = (arg!(call, 0), arg!(call, 1) as u32);
    let (class_ptr, options, disposition_ptr) =
        (arg!(call, 4), arg!(call, 5) as u32, arg!(call, 6));
    let KeyName { _root, start, name, attributes, spelled } =
        tri!(call.key_attributes(arg!(call, 2)));
    let (start, name, node) = tri!(call.key_view(access, start, name));
    let class = if class_ptr == 0 { Vec::new() } else { tri!(call.unicode_string(class_ptr)) };
    let now = platform::system_time();
    // A name in the 32-bit view: its WOW6432Node exists (as on Windows,
    // where the views' keys are always there).
    if node > 0 {
        tri!(registry::with(|r| r.create(start, &name[..node], 0, &[], now).map(|_| ())));
    }
    // A new key needs KEY_CREATE_SUB_KEY on its parent (the access check,
    // docs/SECURITY.md S3); an existing one is opened as NtOpenKey would.
    let existing = registry::with(|r| Ok(r.lookup(start, &name).ok()));
    if let Ok(None) = existing {
        let split = name.iter().rposition(|&c| c == u16::from(b'\\'));
        let parent = match split {
            Some(i) => registry::with(|r| r.lookup(start, &name[..i])),
            None => Ok(start),
        };
        if let Ok(parent) = parent {
            tri!(super::security_calls::check_key_create(call, parent));
        }
    }
    let (id, created) = tri!(registry::with(|r| {
        let (id, created) = r.create(start, &name, options, &class, now)?;
        r.retain(id);
        Ok((id, created))
    }));
    if created {
        tri!(super::security_calls::assign_key(call, id, arg!(call, 2)));
    }
    let status = if created {
        call.return_key(out, id, access, attributes, &spelled)
    } else {
        call.return_checked_key(out, id, access, attributes, &spelled)
    };
    if status == NtStatus::SUCCESS && disposition_ptr != 0 {
        let disposition = if created { REG_CREATED_NEW_KEY } else { REG_OPENED_EXISTING_KEY };
        tri!(call.write_u32(disposition_ptr, disposition));
    }
    status
}

/// NtQueryKey(KeyHandle, KeyInformationClass, KeyInformation, Length,
///            *ResultLength). KeyNameInformation needs no access right.
pub(super) fn nt_query_key(call: &mut Call<'_>) -> NtStatus {
    let class = arg!(call, 1) as u32;
    let access = if matches!(class, 3 | 7) { 0 } else { KEY_QUERY_VALUE };
    let (object, id) = tri!(call.key(arg!(call, 0), access));
    let ObjectBody::Key(k) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let (data, fixed) = if class == 3 && !k.name.is_empty() {
        // KEY_NAME_INFORMATION: the name as the key was opened (measured
        // on 26200, issue #59), as far as it still names the key's own
        // components (a key renamed since: its new name).
        let name = tri!(registry::with(|r| registry::display_name(&r.full_name(id)?, &k.name)));
        tri!(registry::name_information(&name))
    } else {
        tri!(registry::with(|r| r.key_information(id, class)))
    };
    call.return_partial(&data, fixed, arg!(call, 2), arg!(call, 3) as u32 as u64, arg!(call, 4))
}

/// NtEnumerateKey(KeyHandle, Index, KeyInformationClass, KeyInformation,
///                Length, *ResultLength): subkeys in name order.
pub(super) fn nt_enumerate_key(call: &mut Call<'_>) -> NtStatus {
    let (index, class) = (arg!(call, 1) as u32 as usize, arg!(call, 2) as u32);
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_ENUMERATE_SUB_KEYS));
    let (data, fixed) = tri!(registry::with(|r| {
        let sub = r.subkey(id, index)?;
        r.key_information(sub, class)
    }));
    call.return_partial(&data, fixed, arg!(call, 3), arg!(call, 4) as u32 as u64, arg!(call, 5))
}

/// NtQueryValueKey(KeyHandle, *ValueName, KeyValueInformationClass,
///                 KeyValueInformation, Length, *ResultLength)
pub(super) fn nt_query_value_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_QUERY_VALUE));
    let name = tri!(call.value_name(arg!(call, 1)));
    let class = arg!(call, 2) as u32;
    let (data, fixed) =
        tri!(registry::with(|r| registry::value_information(r.value(id, &name)?, class)));
    call.return_partial(&data, fixed, arg!(call, 3), arg!(call, 4) as u32 as u64, arg!(call, 5))
}

/// NtEnumerateValueKey(KeyHandle, Index, KeyValueInformationClass,
///                     KeyValueInformation, Length, *ResultLength)
pub(super) fn nt_enumerate_value_key(call: &mut Call<'_>) -> NtStatus {
    let (index, class) = (arg!(call, 1) as u32 as usize, arg!(call, 2) as u32);
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_QUERY_VALUE));
    let (data, fixed) =
        tri!(registry::with(|r| registry::value_information(r.value_at(id, index)?, class)));
    call.return_partial(&data, fixed, arg!(call, 3), arg!(call, 4) as u32 as u64, arg!(call, 5))
}

/// NtSetValueKey(KeyHandle, *ValueName, TitleIndex, Type, Data, DataSize)
pub(super) fn nt_set_value_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_SET_VALUE));
    let name = tri!(call.value_name(arg!(call, 1)));
    let (kind, data_ptr, size) = (arg!(call, 3) as u32, arg!(call, 4), arg!(call, 5) as u32);
    let size = size as usize;
    if size > MAX_VALUE_DATA {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut data = Vec::new();
    if data.try_reserve_exact(size).is_err() {
        return NtStatus::NO_MEMORY;
    }
    data.resize(size, 0u8);
    if size > 0 && call.process.read(data_ptr, &mut data).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let now = platform::system_time();
    tri!(registry::with(|r| r.set_value(id, &name, kind, &data, now)));
    NtStatus::SUCCESS
}

/// NtDeleteKey(KeyHandle): the key must have no subkeys; handles to it
/// stay open and get STATUS_KEY_DELETED.
pub(super) fn nt_delete_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), DELETE));
    let now = platform::system_time();
    tri!(registry::with(|r| r.delete_key(id, now)));
    NtStatus::SUCCESS
}

/// NtDeleteValueKey(KeyHandle, *ValueName)
pub(super) fn nt_delete_value_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_SET_VALUE));
    let name = tri!(call.value_name(arg!(call, 1)));
    let now = platform::system_time();
    tri!(registry::with(|r| r.delete_value(id, &name, now)));
    NtStatus::SUCCESS
}

/// NtSetInformationKey(KeyHandle, KeySetInformationClass, KeySetInformation,
/// KeySetInformationLength), with KEY_SET_VALUE on the handle (as
/// ZwSetInformationKey's documentation asks; a KEY_READ handle is
/// STATUS_ACCESS_DENIED for an administrator too). What each class
/// answers was measured on 26200 for a standard user and an administrator
/// alike (issue #60): KeyWriteTimeInformation (0) sets the key's
/// LastWriteTime to the 8 bytes given. KeyWow64FlagsInformation (1),
/// KeyControlFlagsInformation (2), KeySetVirtualizationInformation (3),
/// KeySetDebugInformation (4) and KeySetHandleTagsInformation (5) are a
/// ULONG (another length is STATUS_INFO_LENGTH_MISMATCH: measured for 2)
/// and succeed without a privilege; they steer registry virtualization,
/// debugging and handle tags, which gala has not (every process writes
/// where it names): accepted, nothing to change, recorded with the value
/// (Office's Click-to-Run client sets REG_KEY_DONT_VIRTUALIZE |
/// REG_KEY_RECURSE_FLAG). KeySetLayerInformation (6) is no class on that
/// build: STATUS_INVALID_INFO_CLASS, as for any other.
pub(super) fn nt_set_information_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_SET_VALUE));
    let (class, data, len) = (arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3) as u32);
    match class {
        0 => {
            if len != 8 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let mut time = [0u8; 8];
            if call.process.read(data, &mut time).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            tri!(registry::with(|r| r.set_write_time(id, u64::from_le_bytes(time))));
            NtStatus::SUCCESS
        }
        1..=5 => {
            if len != 4 {
                return NtStatus::INFO_LENGTH_MISMATCH;
            }
            let mut value = [0u8; 4];
            if call.process.read(data, &mut value).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let what = match class {
                1 => "wow64 flags: no registry virtualization",
                2 => "control flags: no registry virtualization",
                3 => "virtualization: no registry virtualization",
                4 => "debug information: no registry debugging",
                _ => "handle tags: no handle tags",
            };
            super::super::ledger::record(
                "NtSetInformationKey",
                what,
                u64::from(u32::from_le_bytes(value)),
            );
            // Windows keeps these flags with the key: a change of its
            // attributes for the notifications.
            tri!(registry::with(|r| r.touch_attributes(id)));
            NtStatus::SUCCESS
        }
        _ => NtStatus::INVALID_INFO_CLASS,
    }
}

/// NtFlushKey(KeyHandle): the registry's changes reach the disk before
/// the call returns (`persist`'s lazy flush, at once).
pub(super) fn nt_flush_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), 0));
    tri!(registry::with(|r| r.key_information(id, 7).map(|_| ())));
    // On the disk before the call returns.
    crate::nt::persist::flush(crate::nt::persist::Cause::FlushKey);
    NtStatus::SUCCESS
}

/// NtRenameKey(KeyHandle, NewName): the key takes the name NewName (one
/// component) under the same parent; the handle needs KEY_WRITE
/// (RegRenameKey's documentation; a KEY_READ handle is
/// STATUS_ACCESS_DENIED in Wine's conformance test, which passes on
/// Windows). Logged (`events=file`: `key rename`) and kept in the change
/// log (nt::reglog).
pub(super) fn nt_rename_key(call: &mut Call<'_>) -> NtStatus {
    let name_ptr = arg!(call, 1);
    if name_ptr == 0 {
        return NtStatus::ACCESS_VIOLATION;
    }
    let name = tri!(call.unicode_string(name_ptr));
    // An empty name before the handle: RegRenameKey(NULL, NULL, NULL) is
    // ERROR_INVALID_PARAMETER, (NULL, NULL, L"x") ERROR_INVALID_HANDLE
    // (Wine's conformance test, which passes on Windows).
    if name.is_empty() {
        return NtStatus::INVALID_PARAMETER;
    }
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_SET_VALUE | KEY_CREATE_SUB_KEY));
    let old = registry::with(|r| r.full_name(id)).unwrap_or_default();
    let now = platform::system_time();
    let status = match registry::with(|r| r.rename(id, &name, now)) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    };
    use crate::events::{Category, Utf16, emit, enabled};
    if enabled(Category::File) {
        emit(
            Category::File,
            "key rename",
            format_args!(
                r#""pid":{},"key":{},"name":{},"status":{}"#,
                call.process.id,
                Utf16(&old),
                Utf16(&name),
                status.0
            ),
        );
    }
    status
}

/// NtNotifyChangeKey(KeyHandle, Event, ApcRoutine, ApcContext,
/// IoStatusBlock, CompletionFilter, WatchTree, Buffer, BufferSize,
/// Asynchronous): see nt::regnotify.
pub(super) fn nt_notify_change_key(call: &mut Call<'_>) -> NtStatus {
    let args = Notify {
        event: arg!(call, 1),
        apc: arg!(call, 2),
        apc_context: arg!(call, 3),
        iosb: arg!(call, 4),
        filter: arg!(call, 5) as u32,
        tree: arg!(call, 6) as u8 != 0,
        asynchronous: arg!(call, 9) as u8 != 0,
    };
    notify_change(call, arg!(call, 0), None, args)
}

/// NtNotifyChangeMultipleKeys(MasterKeyHandle, Count, SubordinateObjects,
/// Event, ApcRoutine, ApcContext, IoStatusBlock, CompletionFilter,
/// WatchTree, Buffer, BufferSize, Asynchronous): as NtNotifyChangeKey,
/// with at most one subordinate key (OBJECT_ATTRIBUTES, opened for
/// KEY_NOTIFY in the caller's registry view) whose changes count as the
/// master's; a Count over 1 is STATUS_INVALID_PARAMETER, as is a
/// subordinate in the master's hive (`registry::Registry::hive`; Windows
/// refuses a sibling key of the master, regnotify_test).
pub(super) fn nt_notify_change_multiple_keys(call: &mut Call<'_>) -> NtStatus {
    let count = arg!(call, 1) as u32;
    if count > 1 {
        return NtStatus::INVALID_PARAMETER;
    }
    let subordinate = if count == 1 {
        let KeyName { _root, start, name, .. } = tri!(call.key_attributes(arg!(call, 2)));
        let (start, name, _) = tri!(call.key_view(KEY_NOTIFY, start, name));
        let id = tri!(registry::with(|r| {
            let id = r.lookup(start, &name)?;
            r.retain(id);
            Ok(id)
        }));
        // The object releases the key when it goes (on a failure here too).
        let Ok(object) = KObject::new(ObjectBody::Key(KeyObject { id, name: Vec::new() })) else {
            return NtStatus::NO_MEMORY;
        };
        tri!(super::security_calls::check_key(call, id, KEY_NOTIFY, false));
        Some((object, id))
    } else {
        None
    };
    let args = Notify {
        event: arg!(call, 3),
        apc: arg!(call, 4),
        apc_context: arg!(call, 5),
        iosb: arg!(call, 6),
        filter: arg!(call, 7) as u32,
        tree: arg!(call, 8) as u8 != 0,
        asynchronous: arg!(call, 11) as u8 != 0,
    };
    notify_change(call, arg!(call, 0), subordinate, args)
}

/// The arguments the two notification services share.
struct Notify {
    event: u64,
    apc: u64,
    apc_context: u64,
    iosb: u64,
    filter: u32,
    tree: bool,
    asynchronous: bool,
}

/// Arms a notification on the key handle `h` (KEY_NOTIFY) and, for a
/// synchronous call, waits for it. The filter may be 0 (Wine's
/// conformance test, which passes on Windows: such a watch never fires)
/// but no bit outside REG_LEGAL_CHANGE_FILTER. Buffer and BufferSize are
/// not used (the registry returns no data).
fn notify_change(
    call: &mut Call<'_>,
    h: u64,
    subordinate: Option<(ObjectRef, KeyId)>,
    a: Notify,
) -> NtStatus {
    use super::super::object::{Event, EventType};
    use super::super::regnotify::{self, LEGAL_FILTER, THREAD_AGNOSTIC};
    use super::super::thread;
    use super::super::wait::{self, Dispatcher, Timeout};
    let (object, id) = tri!(call.key(h, KEY_NOTIFY));
    let ObjectBody::Key(k) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if a.filter & !LEGAL_FILTER != 0 {
        return NtStatus::INVALID_PARAMETER;
    }
    if a.iosb == 0 {
        return NtStatus::ACCESS_VIOLATION;
    }
    // A key deleted since it was opened: STATUS_KEY_DELETED.
    tri!(registry::with(|r| r.key_name(id).map(drop)));
    if let Some((_, sub)) = &subordinate {
        let same = tri!(registry::with(|r| Ok(r.hive(id)? == r.hive(*sub)?)));
        if same {
            return NtStatus::INVALID_PARAMETER;
        }
    }
    let event = match a.event {
        0 => None,
        e => Some(tri!(super::event_object(call, Handle(e), super::EVENT_MODIFY_STATE))),
    };
    if let Some(e) = &event {
        wait::set_signal(e, 0);
    }
    let me = thread::current();
    let apc = match (a.apc, &me) {
        (0, _) | (_, None) => None,
        (routine, Some(t)) => {
            Some((t.clone(), thread::UserApc { routine, args: [a.apc_context, a.iosb, 0] }))
        }
    };
    let done = if a.asynchronous {
        None
    } else {
        let e = Event { kind: EventType::Notification, dispatcher: Dispatcher::new(0) };
        match KObject::new(ObjectBody::Event(e)) {
            Ok(o) => Some(o),
            Err(_) => return NtStatus::NO_MEMORY,
        }
    };
    let agnostic = a.filter & THREAD_AGNOSTIC != 0;
    let request = regnotify::Request {
        process: call.process.clone(),
        iosb: a.iosb,
        event,
        apc,
        done: done.clone(),
        thread: if agnostic { None } else { thread::current_id() },
        reserved: None,
    };
    let request = tri!(request.reserve());
    let owner = k.owner();
    let status = regnotify::arm(owner, call.process.id, id, subordinate, a.filter, a.tree, request);
    drop(object);
    match (status, done) {
        (NtStatus::PENDING, Some(done)) => {
            let objects = [done];
            wait::wait_for(&objects, false, Timeout::Infinite);
            // The result is in the caller's IO_STATUS_BLOCK.
            let mut b = [0u8; 4];
            match call.process.read(a.iosb, &mut b) {
                Ok(()) => NtStatus(u32::from_le_bytes(b)),
                Err(_) => NtStatus::ACCESS_VIOLATION,
            }
        }
        (status, _) => status,
    }
}

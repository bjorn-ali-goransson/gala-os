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
    self, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_RES, KeyId, KeyObject,
    REG_CREATED_NEW_KEY, REG_OPENED_EXISTING_KEY, Registry,
};
use super::super::status::NtStatus;
use super::{Call, HANDLE_ATTRIBUTES};

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
    fn key_attributes(
        &self,
        ptr: u64,
    ) -> Result<(Option<ObjectRef>, KeyId, Vec<u16>, u32), NtStatus> {
        let (root, mut name, attributes) =
            self.raw_object_attributes(ptr)?.ok_or(NtStatus::INVALID_PARAMETER)?;
        if root == 0 {
            let rest = Registry::absolute(&name)?;
            let skip = name.len() - rest.len();
            name.drain(..skip);
            return Ok((None, registry::ROOT, name, attributes));
        }
        if name.first() == Some(&(b'\\' as u16)) {
            return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
        }
        let (object, id) = self.key(root, 0)?;
        Ok((Some(object), id, name, attributes))
    }

    /// Makes a key object for `id` (already retained) and returns a handle.
    fn return_key(&self, out: u64, id: KeyId, access: u32, attributes: u32) -> NtStatus {
        // On failure the key object is dropped here, outside the registry
        // lock, which releases the node.
        let Ok(object) = KObject::new(ObjectBody::Key(KeyObject { id })) else {
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
    let (_root, start, name, attributes) = tri!(call.key_attributes(arg!(call, 2)));
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
            return status;
        }
    };
    call.return_key(out, id, access, attributes)
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
    let (_root, start, name, attributes) = tri!(call.key_attributes(arg!(call, 2)));
    let class = if class_ptr == 0 { Vec::new() } else { tri!(call.unicode_string(class_ptr)) };
    let now = platform::system_time();
    let (id, created) = tri!(registry::with(|r| {
        let (id, created) = r.create(start, &name, options, &class, now)?;
        r.retain(id);
        Ok((id, created))
    }));
    if created {
        tri!(super::security_calls::assign_key(call, id, arg!(call, 2)));
    }
    let status = call.return_key(out, id, access, attributes);
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
    let (_object, id) = tri!(call.key(arg!(call, 0), access));
    let (data, fixed) = tri!(registry::with(|r| r.key_information(id, class)));
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

/// NtFlushKey(KeyHandle): the registry is in memory; nothing to write.
pub(super) fn nt_flush_key(call: &mut Call<'_>) -> NtStatus {
    let (_object, id) = tri!(call.key(arg!(call, 0), 0));
    tri!(registry::with(|r| r.key_information(id, 7).map(|_| ())));
    NtStatus::SUCCESS
}

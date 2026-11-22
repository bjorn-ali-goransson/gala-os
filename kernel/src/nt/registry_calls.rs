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
use core::sync::atomic::{AtomicUsize, Ordering};

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

/// The `key` event of a key opened or created (`events=file`): the key
/// it is relative to (`base`, its full name), the name and the status,
/// in order with the file opens. The ledger keeps only each absent key's
/// first time, so which open came last before a failure needs this.
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

/// What a value event says (`emit_value`).
struct Valued<'a> {
    /// The event's name: `value` read, `value set` written, `value
    /// deleted` removed.
    what: &'static str,
    key: KeyId,
    /// The name the call asked for, or the one an index held.
    name: &'a [u16],
    /// The index an enumerate asked for; a call by name has none.
    index: Option<usize>,
    /// What the value held, where the call had anything: its type, its
    /// size and the first [`PEEK`] bytes of its data, or as many as
    /// `events_data=` asked for. An empty value is a different answer
    /// from an absent one, and this is the difference.
    held: Option<Peek>,
    status: NtStatus,
}

/// The index an enumerate asked for, written where there was one.
struct At(Option<usize>);

impl core::fmt::Display for At {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(index) => write!(f, r#","index":{index}"#),
            None => Ok(()),
        }
    }
}

/// Value types as the registry stores them, the ones a value event
/// reads its data as.
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const REG_DWORD: u32 = 4;
const REG_DWORD_BIG_ENDIAN: u32 = 5;
const REG_MULTI_SZ: u32 = 7;
const REG_QWORD: u32 = 11;

/// How much of a value's data a value event carries unless
/// `events_data=` says otherwise. A number of any width fits whole and
/// a name or a short path very nearly; longer data is cut here, and the
/// event's `bytes` says how much there was, so a cut needs no field of
/// its own to be seen. The default is small because a program that
/// walks a hive of long values would otherwise turn the stream into a
/// second copy of the hive.
const PEEK: usize = 64;

/// The most `events_data=` may ask for. A record a program writes about
/// itself is what the raised peek is for -- Word's session record is
/// 401 bytes of JSON -- and this is the room for one, bounded because
/// the bytes are copied into a field of [`Peek`] and so onto the stack
/// of every registry call that reads a value.
const MAX_PEEK: usize = 512;

/// Bytes of a value's data a value event carries (`events_data=`).
static PEEK_BYTES: AtomicUsize = AtomicUsize::new(PEEK);

/// Parses an `events_data=` value: a number of bytes up to [`MAX_PEEK`].
fn parse_peek(text: &str) -> Option<usize> {
    text.parse::<usize>().ok().filter(|n| (1..=MAX_PEEK).contains(n))
}

/// Sets how much of a value's data a value event carries (an
/// `events_data=` value). A number that is none, or past the bound,
/// leaves the default standing and says so: a peek that had quietly
/// become the default would be read as the one that was asked for.
pub fn peek_bytes(text: &str) {
    let Some(n) = parse_peek(text) else {
        warn!(
            "events: events_data={text} is not a number from 1 to \
             {MAX_PEEK}: the default {PEEK} bytes stand"
        );
        return;
    };
    if !crate::events::enabled(crate::events::Category::File) {
        warn!("events: events_data={text} without events=file: no value event to carry it");
        return;
    }
    PEEK_BYTES.store(n, Ordering::Relaxed);
    info!("events: {n} bytes of each value's data are carried (events_data=)");
}

/// What a value held: its type, how much data there was, and the first
/// [`PEEK`] bytes of it, or as many as `events_data=` asked for. The
/// bytes are copied while the registry is still locked, because the
/// event is written after the lock is let go.
#[derive(Clone, Copy)]
struct Peek {
    kind: u32,
    bytes: usize,
    head: [u8; MAX_PEEK],
    /// How much of `head` the value filled: `bytes`, or the peek's size
    /// where it was cut.
    len: usize,
}

impl Peek {
    /// What a value event will carry of this data, or `None` when no
    /// stream is reading one: the copy is what the field costs, so a
    /// read nothing is watching pays one relaxed load and nothing else.
    fn of(kind: u32, data: &[u8]) -> Option<Self> {
        if !crate::events::enabled(crate::events::Category::File) {
            return None;
        }
        Some(Self::cut(kind, data, PEEK_BYTES.load(Ordering::Relaxed)))
    }

    /// The first `n` bytes of it, whatever is watching.
    fn cut(kind: u32, data: &[u8], n: usize) -> Self {
        let len = data.len().min(n).min(MAX_PEEK);
        let mut head = [0u8; MAX_PEEK];
        head[..len].copy_from_slice(&data[..len]);
        Peek { kind, bytes: data.len(), head, len }
    }

    /// The data as its type says to read it: a number for the integer
    /// types that are the width they claim, the text of a string with
    /// the NULs at its end trimmed (a multi-string keeps the ones
    /// between its parts, so the one field shows where they end), and
    /// the bytes as hex for everything else -- a type gala does not
    /// know, and an integer narrower than it claims, which is not a
    /// number and is shown as what it is instead of being guessed at.
    fn write_data(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let head = &self.head[..self.len];
        match self.kind {
            REG_DWORD if head.len() >= 4 => {
                write!(f, "{}", u32::from_le_bytes([head[0], head[1], head[2], head[3]]))
            }
            REG_DWORD_BIG_ENDIAN if head.len() >= 4 => {
                write!(f, "{}", u32::from_be_bytes([head[0], head[1], head[2], head[3]]))
            }
            REG_QWORD if head.len() >= 8 => {
                let mut eight = [0u8; 8];
                eight.copy_from_slice(&head[..8]);
                write!(f, "{}", u64::from_le_bytes(eight))
            }
            REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => {
                let mut units = [0u16; MAX_PEEK / 2];
                let mut n = head.len() / 2;
                for (i, unit) in units.iter_mut().take(n).enumerate() {
                    *unit = u16::from_le_bytes([head[2 * i], head[2 * i + 1]]);
                }
                while n > 0 && units[n - 1] == 0 {
                    n -= 1;
                }
                write!(f, "{}", crate::events::Utf16(&units[..n]))
            }
            _ => {
                use core::fmt::Write as _;
                f.write_char('"')?;
                for byte in head {
                    write!(f, "{byte:02x}")?;
                }
                f.write_char('"')
            }
        }
    }
}

/// What a value held, written where the call had any.
struct Held(Option<Peek>);

impl core::fmt::Display for Held {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Some(peek) = self.0 else { return Ok(()) };
        write!(f, r#","kind":{},"bytes":{},"data":"#, peek.kind, peek.bytes)?;
        peek.write_data(f)
    }
}

/// The event of a registry value read, written or deleted
/// (`events=file`): the key it is in (its full name), the name asked
/// for, what the value held where the call had it, and the status -- in
/// order with the file opens and the `key` events.
///
/// The ledger cannot answer this. It keeps one row an absent key and
/// nothing at all about values, so the value a program read last before
/// it gave up is in this stream or nowhere. A call refused before its
/// key is resolved -- a handle that is not a key, or one without the
/// access right -- leaves no event here, only the system call's own
/// status under `events=syscall`.
fn emit_value(call: &Call<'_>, said: Valued<'_>) {
    use crate::events::{Category, Utf16, emit, enabled};
    if !enabled(Category::File) {
        return;
    }
    let key = registry::with(|r| r.full_name(said.key)).unwrap_or_default();
    emit(
        Category::File,
        said.what,
        format_args!(
            r#""pid":{},"key":{}{},"name":{}{},"status":{}"#,
            call.process.id,
            Utf16(&key),
            At(said.index),
            Utf16(said.name),
            Held(said.held),
            said.status.0
        ),
    );
}

/// Records an absent key in the ledger: the keys real programs look for.
fn note_absent(start: KeyId, name: &[u16]) {
    let base = registry::with(|r| r.full_name(start)).unwrap_or_default();
    let mut entry = [0u8; 96];
    let n = absent_name(&base, name, &mut entry);
    super::super::ledger::record_user_bulk(&entry[..n], "absent key", 0);
}

/// `registry:<key>` in `out`, how many bytes it took: the key the open
/// was relative to, without the `\REGISTRY\` every name starts with, and
/// then the name that was not found.
fn absent_name(base: &[u16], name: &[u16], out: &mut [u8; 96]) -> usize {
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
    // A user hive is named by the SID of the user whose it is: 40 of the
    // 87 bytes a name has, and the same 40 on every row, which left the
    // name itself starting mid-SID. The row says USER and leaves the SID
    // out; the `key` events carry the full name (`emit_key`) for whoever
    // wants the hive and not the key.
    if let Some(at) = sid_at(&path[..n]) {
        let after = path[at..n].iter().position(|&c| c == b'/').map_or(n, |i| at + i + 1);
        path.copy_within(after..n, at);
        n -= after - at;
    }
    const PREFIX: &[u8] = b"registry:";
    const ELIDED: &[u8] = b"/.../";
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut at = PREFIX.len();
    if n <= out.len() - at {
        out[at..at + n].copy_from_slice(&path[..n]);
        return at + n;
    }
    // Still too long: the hive it is in and as much of the end as fits,
    // cut at a component boundary where one is near the cut. The end
    // alone did not say which hive the key was in.
    let hive = path[..n].iter().position(|&c| c == b'/').unwrap_or(0);
    if hive > 0 && hive + ELIDED.len() + 32 <= out.len() - at {
        out[at..at + hive].copy_from_slice(&path[..hive]);
        at += hive;
        out[at..at + ELIDED.len()].copy_from_slice(ELIDED);
        at += ELIDED.len();
    }
    let mut from = n - (out.len() - at);
    if let Some(i) = path[from..n].iter().take(16).position(|&c| c == b'/') {
        from += i + 1;
    }
    out[at..at + (n - from)].copy_from_slice(&path[from..n]);
    at + (n - from)
}

/// Where a SID component starts in `path`, if one does: a SID is `S-1-`
/// and then nothing but digits and the dashes between them, so a key
/// whose own name begins that way is not mistaken for one.
fn sid_at(path: &[u8]) -> Option<usize> {
    let mut at = 0;
    while at < path.len() {
        let end = path[at..].iter().position(|&c| c == b'/').map_or(path.len(), |i| at + i);
        let comp = &path[at..end];
        if comp.len() > 4
            && comp.starts_with(b"S-1-")
            && comp[4..].iter().all(|&c| c.is_ascii_digit() || c == b'-')
        {
            return Some(at);
        }
        at = end + 1;
    }
    None
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
    let made = registry::with(|r| {
        let (id, created) = r.create(start, &name, options, &class, now)?;
        r.retain(id);
        Ok((id, created))
    });
    let (id, created) = match made {
        Ok(made) => made,
        Err(status) => {
            emit_key(call, start, &name, status);
            return status;
        }
    };
    if created {
        tri!(super::security_calls::assign_key(call, id, arg!(call, 2)));
    }
    let status = if created {
        call.return_key(out, id, access, attributes, &spelled)
    } else {
        call.return_checked_key(out, id, access, attributes, &spelled)
    };
    emit_key(call, start, &name, status);
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
    let found = registry::with(|r| {
        let value = r.value(id, &name)?;
        let held = Peek::of(value.kind, &value.data);
        Ok((registry::value_information(value, class)?, held))
    });
    let ((data, fixed), held) = match found {
        Ok(found) => found,
        Err(status) => {
            let said =
                Valued { what: "value", key: id, name: &name, index: None, held: None, status };
            emit_value(call, said);
            return status;
        }
    };
    let status = call.return_partial(
        &data,
        fixed,
        arg!(call, 3),
        arg!(call, 4) as u32 as u64,
        arg!(call, 5),
    );
    let said = Valued { what: "value", key: id, name: &name, index: None, held, status };
    emit_value(call, said);
    status
}

/// NtEnumerateValueKey(KeyHandle, Index, KeyValueInformationClass,
///                     KeyValueInformation, Length, *ResultLength)
pub(super) fn nt_enumerate_value_key(call: &mut Call<'_>) -> NtStatus {
    let (index, class) = (arg!(call, 1) as u32 as usize, arg!(call, 2) as u32);
    let (_object, id) = tri!(call.key(arg!(call, 0), KEY_QUERY_VALUE));
    // The name is the index's answer and not an argument, so the event
    // needs a copy of it -- taken only when there is an event to write,
    // for a program that walks a key to cost nothing more for this.
    let naming = crate::events::enabled(crate::events::Category::File);
    let found = registry::with(|r| {
        let value = r.value_at(id, index)?;
        let held = Peek::of(value.kind, &value.data);
        let name = if naming { registry::copy_name(&value.name)? } else { Vec::new() };
        Ok((registry::value_information(value, class)?, held, name))
    });
    let ((data, fixed), held, name) = match found {
        Ok(found) => found,
        Err(status) => {
            let said = Valued {
                what: "value",
                key: id,
                name: &[],
                index: Some(index),
                held: None,
                status,
            };
            emit_value(call, said);
            return status;
        }
    };
    let status = call.return_partial(
        &data,
        fixed,
        arg!(call, 3),
        arg!(call, 4) as u32 as u64,
        arg!(call, 5),
    );
    let said = Valued { what: "value", key: id, name: &name, index: Some(index), held, status };
    emit_value(call, said);
    status
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
    let status = match registry::with(|r| r.set_value(id, &name, kind, &data, now)) {
        Ok(()) => NtStatus::SUCCESS,
        Err(status) => status,
    };
    // What the program meant to write, whether or not it landed: the
    // status says which.
    let said = Valued {
        what: "value set",
        key: id,
        name: &name,
        index: None,
        held: Peek::of(kind, &data),
        status,
    };
    emit_value(call, said);
    status
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
    let status = match registry::with(|r| r.delete_value(id, &name, now)) {
        Ok(()) => NtStatus::SUCCESS,
        Err(status) => status,
    };
    let said =
        Valued { what: "value deleted", key: id, name: &name, index: None, held: None, status };
    emit_value(call, said);
    status
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
        (routine, Some(t)) => Some((
            t.clone(),
            thread::UserApc { routine, args: [a.apc_context, a.iosb, 0], special: false },
        )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Raw bytes. The reading above names no constant for it -- bytes are
    /// what every type it does not know falls back to -- so the tests that
    /// check that fallback name it here.
    const REG_BINARY: u32 = 3;

    /// What a value event writes for a value of this type and data.
    fn held(kind: u32, data: &[u8]) -> String {
        format!("{}", Held(Some(Peek::cut(kind, data, PEEK))))
    }

    /// UTF-16 bytes with the NUL a string value ends with.
    fn sz(s: &str) -> Vec<u8> {
        let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        out.push(0);
        out.push(0);
        out
    }

    #[test]
    fn a_number_is_read_at_the_width_its_type_claims() {
        assert_eq!(
            held(REG_DWORD, &0x1234_5678u32.to_le_bytes()),
            r#","kind":4,"bytes":4,"data":305419896"#
        );
        assert_eq!(
            held(REG_DWORD_BIG_ENDIAN, &0x1234_5678u32.to_be_bytes()),
            r#","kind":5,"bytes":4,"data":305419896"#
        );
        assert_eq!(
            held(REG_QWORD, &0x1122_3344_5566_7788u64.to_le_bytes()),
            r#","kind":11,"bytes":8,"data":1234605616436508552"#
        );
    }

    #[test]
    fn an_integer_narrower_than_its_type_is_the_bytes_it_is_and_not_a_guess() {
        assert_eq!(held(REG_DWORD, &[1, 2, 3]), r#","kind":4,"bytes":3,"data":"010203""#);
        assert_eq!(held(REG_QWORD, &[1, 2, 3, 4]), r#","kind":11,"bytes":4,"data":"01020304""#);
    }

    #[test]
    fn a_string_loses_the_nuls_at_its_end() {
        assert_eq!(held(REG_SZ, &sz("hello")), r#","kind":1,"bytes":12,"data":"hello""#);
        assert_eq!(held(REG_EXPAND_SZ, &sz("%TMP%")), r#","kind":2,"bytes":12,"data":"%TMP%""#);
    }

    #[test]
    fn a_multi_string_keeps_the_nuls_between_its_parts() {
        // Where its parts end is the only thing that says how many
        // there are, so the separators are part of the answer.
        assert_eq!(
            held(REG_MULTI_SZ, &sz("a\0bc\0")),
            r#","kind":7,"bytes":12,"data":"a\u0000bc""#
        );
    }

    #[test]
    fn bytes_that_are_not_text_are_hex() {
        assert_eq!(held(REG_BINARY, &[1, 2, 3]), r#","kind":3,"bytes":3,"data":"010203""#);
        // A type gala does not know is not read as anything.
        assert_eq!(held(999, &[0xab, 0xcd]), r#","kind":999,"bytes":2,"data":"abcd""#);
    }

    #[test]
    fn an_empty_value_is_not_an_absent_one() {
        assert_eq!(held(REG_SZ, &[]), r#","kind":1,"bytes":0,"data":"""#);
        assert_eq!(format!("{}", Held(None)), "");
    }

    #[test]
    fn data_past_the_peek_is_cut_and_bytes_says_how_much_there_was() {
        let long: Vec<u8> = (0..100u8).collect();
        // The first PEEK bytes, whole, and nothing of the rest. Spelled out
        // rather than probed for, because consecutive bytes spell each
        // other's digits: 0x04 0x05 is "0405", which holds "40".
        let head: String = long[..PEEK].iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(held(REG_BINARY, &long), format!(r#","kind":3,"bytes":100,"data":"{head}""#));
    }

    #[test]
    fn a_string_past_the_peek_is_cut_on_a_whole_character() {
        let long: Vec<u8> = sz(&"ab".repeat(40));
        let written = held(REG_SZ, &long);
        // PEEK is even, so the cut never lands inside a UTF-16 unit.
        assert!(written.starts_with(r#","kind":1,"bytes":162,"data":"abab"#), "{written}");
        assert_eq!(written.len(), r#","kind":1,"bytes":162,"data":"""#.len() + PEEK / 2);
    }

    #[test]
    fn a_raised_peek_carries_more_of_the_same_value() {
        let long: Vec<u8> = sz(&"ab".repeat(40));
        // All 80 characters, where the default peek above shows 32 of them.
        let whole = format!("{}", Held(Some(Peek::cut(REG_SZ, &long, 256))));
        assert_eq!(whole, format!(r#","kind":1,"bytes":162,"data":"{}""#, "ab".repeat(40)));
    }

    #[test]
    fn a_peek_past_the_bound_or_no_number_at_all_is_refused() {
        assert_eq!(parse_peek("128"), Some(128));
        assert_eq!(parse_peek(&format!("{MAX_PEEK}")), Some(MAX_PEEK));
        // Past the bound the default stands: the bytes are copied onto the
        // stack of every registry call that reads a value.
        assert_eq!(parse_peek(&format!("{}", MAX_PEEK + 1)), None);
        assert_eq!(parse_peek("0"), None);
        assert_eq!(parse_peek("-1"), None);
        assert_eq!(parse_peek("lots"), None);
    }
    /// What a key that was not found is named in the ledger.
    fn absent(base: &str, name: &str) -> String {
        let base: Vec<u16> = base.encode_utf16().collect();
        let name: Vec<u16> = name.encode_utf16().collect();
        let mut out = [0u8; 96];
        let n = absent_name(&base, &name, &mut out);
        String::from(core::str::from_utf8(&out[..n]).unwrap())
    }

    /// The user hive of the one interactive user there is
    /// (docs/SECURITY.md), named by that user's SID.
    const USER_HIVE: &str = r"\REGISTRY\USER\S-1-5-21-1000000001-1000000002-1000000003-1001";

    #[test]
    fn a_key_short_enough_is_named_in_full() {
        assert_eq!(
            absent(r"\REGISTRY\MACHINE\SOFTWARE\Microsoft", "Direct3D"),
            "registry:MACHINE/SOFTWARE/Microsoft/Direct3D"
        );
    }

    #[test]
    fn a_user_key_is_named_without_the_sid_that_is_the_same_on_every_row() {
        // The SID is 40 of the 87 bytes a name has, and the same 40 on
        // every row: keeping it, the name began mid-SID.
        assert_eq!(
            absent(&format!(r"{USER_HIVE}\Software\Microsoft\Office\16.0\Word\Options"), "Vpref"),
            "registry:USER/Software/Microsoft/Office/16.0/Word/Options/Vpref"
        );
    }

    #[test]
    fn a_component_that_is_not_a_sid_is_kept_whatever_it_looks_like() {
        assert_eq!(
            absent(r"\REGISTRY\MACHINE\SOFTWARE\S-1-5-21-Vendor", "Build"),
            "registry:MACHINE/SOFTWARE/S-1-5-21-Vendor/Build"
        );
    }

    #[test]
    fn a_key_too_long_keeps_the_hive_it_is_in_and_the_end_that_names_it() {
        let long = format!(
            "{USER_HIVE}{}",
            r"\Software\Microsoft\Office\16.0\Common\Licensing\Store\Tenants\Default"
        );
        let got = absent(&long, "ProductReleaseId");
        assert!(got.len() <= 96, "{got}");
        assert!(got.starts_with("registry:USER/.../"), "{got}");
        assert!(got.ends_with("/ProductReleaseId"), "{got}");
    }

    #[test]
    fn a_name_that_is_not_plain_text_is_not_mistaken_for_one() {
        assert_eq!(absent(r"\REGISTRY\MACHINE", "a\"b\u{1f600}c"), "registry:MACHINE/a?b??c");
    }
}

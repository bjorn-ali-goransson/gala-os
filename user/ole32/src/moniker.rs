//! Monikers and the running object table, as Windows 11 answers them
//! (measured by `user/moniker_test`). Word's Mso20win32client.dll
//! delay-loads `MkParseDisplayName`, and until this module existed the
//! delay-load raised an exception no handler took.
//!
//! A moniker here is its kind and its display name, and every operation
//! is on that name: that is enough for what programs do with monikers
//! outside binding -- parse a name, join two, ask whether an object is
//! running under one, hand one to the running object table.
//!
//! - `MkParseDisplayName` takes a path to a file that exists (a file
//!   moniker), `clsid:<guid>:` (a class moniker), and refuses everything
//!   else the way Windows does: MK_E_CANTOPENFILE for a file that is not
//!   there and for a directory, MK_E_SYNTAX for a bare word or a URL,
//!   E_NOINTERFACE for a `@ProgID`, E_INVALIDARG for a NULL argument or
//!   an empty name. A file followed by an extension it cannot parse is
//!   MK_E_INVALIDEXTENSION with the file part eaten *and* the file's
//!   moniker handed back, which is Windows' answer and not what the
//!   documentation says.
//! - `CreateFileMoniker` keeps the path exactly as given, relative or
//!   empty; `CreateItemMoniker` makes `<delimiter><name>`;
//!   `CreateAntiMoniker` makes `\..`; `CreateClassMoniker` makes
//!   `clsid:<guid>:`.
//! - Names compare and hash without regard to case, so a moniker found
//!   in the running object table is found whatever case it was written
//!   in. Composing a file with an item gives a generic composite whose
//!   name is the two joined; composing anything with an anti moniker
//!   annihilates it, which is S_OK with a NULL moniker. Two file
//!   monikers make no composite at all: a relative right-hand path
//!   merges into the left as one file moniker, each of its leading `..`
//!   taking a component, and an absolute one is MK_E_SYNTAX.
//! - `CreateGenericComposite` is `ComposeWith` with the ends handled: a
//!   NULL on either side hands back the other moniker, referenced.
//! - `Inverse` of a file or an item is an anti moniker;
//!   `CommonPrefixWith` of two paths is their shared directory with its
//!   separator still on (and of a moniker with itself, MK_S_US);
//!   `RelativePathTo` steps out of the source's own last component
//!   before descending.
//! - The running object table is one object for the process, handed back
//!   by `GetRunningObjectTable` and by `IBindCtx::GetRunningObjectTable`
//!   alike. It holds a reference on each registered object and moniker,
//!   gives a second registration of one name its own key and
//!   MK_S_MONIKERALREADYREGISTERED, finds entries by the moniker's value
//!   rather than its pointer, and after a revoke answers S_FALSE,
//!   MK_E_UNAVAILABLE and E_INVALIDARG as Windows does.
//!
//! Limits, with a ledger row each where a program meets them
//! (ARCHITECTURE "COM: ole32 and oleaut32"):
//!
//! - Binding is not implemented. A file moniker whose file is absent is
//!   MK_E_CANTOPENFILE and `BindToStorage` is E_NOINTERFACE, both as
//!   measured; binding a file that *is* there would have to find the
//!   file's class and gala has no such registration, so it is recorded
//!   and refused. `Reduce`, `Load` and `Save` are recorded too.
//! - The table is this process's. Windows keeps one per logon session in
//!   RPCSS, so a moniker registered by one program is not visible to
//!   another here.
//! - A moniker's serialized size (`GetSizeMax`) and its hash are gala's
//!   own numbers, not Windows'. Only their consistency is promised: two
//!   monikers that are equal hash alike, and an anti moniker hashes to
//!   0x80000001 as on Windows because programs test for it.
//! - Case folding is ASCII. Two paths differing only in the case of a
//!   non-ASCII letter are two monikers here and one on Windows.
//! - Names are at most 320 characters; a longer one is recorded and
//!   refused with E_OUTOFMEMORY.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use gala_lock::Lock;

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    S_FALSE, S_OK, VTable, add_ref, alloc_zeroed, com_call, free, missing, ole_iid, put, release,
};

const MK_E_NEEDGENERIC: HResult = 0x8004_01e2u32 as i32;
const MK_E_UNAVAILABLE: HResult = 0x8004_01e3u32 as i32;
const MK_E_SYNTAX: HResult = 0x8004_01e4u32 as i32;
const MK_E_INVALIDEXTENSION: HResult = 0x8004_01e6u32 as i32;
const MK_E_NOINVERSE: HResult = 0x8004_01ecu32 as i32;
const MK_E_CANTOPENFILE: HResult = 0x8004_01eau32 as i32;
const MK_E_NOPREFIX: HResult = 0x8004_01eeu32 as i32;
const MK_S_US: HResult = 0x0004_01e6;
const MK_S_MONIKERALREADYREGISTERED: HResult = 0x0004_01e7;

const IID_IPERSIST: Guid = ole_iid(0x10c);
const IID_IPERSISTSTREAM: Guid = ole_iid(0x109);
const IID_IMONIKER: Guid = ole_iid(0xf);
const IID_IRUNNINGOBJECTTABLE: Guid = ole_iid(0x10);
const IID_IENUMMONIKER: Guid = ole_iid(0x102);
const IID_ISTORAGE: Guid = ole_iid(0xb);
const IID_ISTREAM: Guid = ole_iid(0xc);
const IID_ILOCKBYTES: Guid = ole_iid(0xa);
/// {F29F6BC0-5021-11CE-AA15-00006901293F}: IROTData, the only one of
/// these that is not an OLE interface id.
const IID_IROTDATA: Guid = [
    0xc0, 0x6b, 0x9f, 0xf2, 0x21, 0x50, 0xce, 0x11, 0xaa, 0x15, 0x00, 0x00, 0x69, 0x01, 0x29, 0x3f,
];

/// MKSYS_*, which is also this module's tag for what a moniker is.
const MKSYS_GENERICCOMPOSITE: u32 = 1;
const MKSYS_FILEMONIKER: u32 = 2;
const MKSYS_ANTIMONIKER: u32 = 3;
const MKSYS_ITEMMONIKER: u32 = 4;
const MKSYS_CLASSMONIKER: u32 = 7;

/// The class id of each kind (CLSID_FileMoniker and friends: OLE class
/// ids, so `ole_iid` builds them).
fn class_of(kind: u32) -> Guid {
    match kind {
        MKSYS_GENERICCOMPOSITE => ole_iid(0x309),
        MKSYS_FILEMONIKER => ole_iid(0x303),
        MKSYS_ANTIMONIKER => ole_iid(0x305),
        MKSYS_ITEMMONIKER => ole_iid(0x304),
        _ => ole_iid(0x31a),
    }
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetFileAttributesW(name: *const u16) -> u32;
}

const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// The longest display name a moniker may carry here.
const NAME: usize = 320;

// ---- Names ----

/// ASCII case folding (the limit this module documents).
fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn same_name(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| fold(*x) == fold(*y))
}

/// FNV-1a over the folded name, so equal monikers hash alike.
fn hash_name(name: &[u16]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &c in name {
        let c = fold(c);
        for byte in c.to_le_bytes() {
            h = (h ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// The caller's NUL-terminated string into `out`, or None when it does
/// not fit.
fn read_wide(p: *const u16, out: &mut [u16]) -> Option<usize> {
    if p.is_null() {
        return None;
    }
    for i in 0..=out.len() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let c = unsafe { *p.add(i) };
        if c == 0 {
            return Some(i);
        }
        if i == out.len() {
            return None;
        }
        out[i] = c;
    }
    None
}

/// `text` (ASCII) appended to `out` at `at`, giving the new length, or
/// None when it does not fit.
fn push_ascii(out: &mut [u16], at: usize, text: &[u8]) -> Option<usize> {
    if at + text.len() > out.len() {
        return None;
    }
    for (i, &b) in text.iter().enumerate() {
        out[at + i] = u16::from(b);
    }
    Some(at + text.len())
}

fn push(out: &mut [u16], at: usize, text: &[u16]) -> Option<usize> {
    if at + text.len() > out.len() {
        return None;
    }
    out[at..at + text.len()].copy_from_slice(text);
    Some(at + text.len())
}

fn is_separator(c: u16) -> bool {
    c == u16::from(b'\\') || c == u16::from(b'/')
}

// ---- The object ----

/// One moniker. `rot_vtable` is the second interface's pointer: an
/// IROTData handed out as `&self.rot_vtable`, which `rot_of` turns back
/// into the object (IMoniker's own first eight slots are
/// IPersistStream's, so that interface is this same pointer).
#[repr(C)]
struct Moniker {
    vtable: *const usize,
    rot_vtable: *const usize,
    refs: AtomicU32,
    kind: u32,
    /// The class a class moniker names; zero otherwise.
    clsid: Guid,
    len: u32,
    name: [u16; NAME],
}

fn mk<'a>(this: *mut Moniker) -> &'a mut Moniker {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &mut *this }
}

impl Moniker {
    fn text(&self) -> &[u16] {
        &self.name[..self.len as usize]
    }
}

/// A new moniker of `kind` carrying `name`, or null (recorded) when the
/// name is too long or the heap is spent.
fn new_moniker(kind: u32, clsid: Guid, name: &[u16]) -> *mut Moniker {
    if name.len() > NAME {
        missing(b"ole32.dll!moniker(name longer than 320)", name.len() as u64);
        return core::ptr::null_mut();
    }
    let p = alloc_zeroed(core::mem::size_of::<Moniker>()).cast::<Moniker>();
    if p.is_null() {
        return p;
    }
    let mut buffer = [0u16; NAME];
    buffer[..name.len()].copy_from_slice(name);
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(Moniker {
            vtable: VTABLE.0.as_ptr().cast(),
            rot_vtable: ROTDATA_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            kind,
            clsid,
            len: name.len() as u32,
            name: buffer,
        })
    };
    p
}

/// Hands `p` to the caller as S_OK, or turns a failed `new_moniker` into
/// its HRESULT.
fn handed(p: *mut Moniker, out: *mut *mut c_void) -> HResult {
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

/// Whether `obj` is one of this module's monikers (its vtable is ours).
fn ours(obj: *mut c_void) -> bool {
    if obj.is_null() {
        return false;
    }
    // SAFETY: a COM object: its first field is its vtable pointer.
    let vtable = unsafe { *obj.cast::<*const usize>() };
    vtable == VTABLE.0.as_ptr().cast()
}

/// A moniker's kind and display name, whoever made it. For one of ours
/// that is a read; for a foreign one it is two calls (and a bind context
/// to make them with), which is recorded, since gala has met none yet.
fn described(obj: *mut c_void) -> Option<(u32, [u16; NAME], usize)> {
    if obj.is_null() {
        return None;
    }
    if ours(obj) {
        let m = mk(obj.cast::<Moniker>());
        return Some((m.kind, m.name, m.len as usize));
    }
    missing(b"ole32.dll!IMoniker(a moniker from elsewhere)", 0);
    let mut kind = 0u32;
    if com_call!(obj, 22, fn(*mut u32) -> HResult, &mut kind) < 0 {
        return None;
    }
    let mut bc: *mut c_void = core::ptr::null_mut();
    if super::bindctx::CreateBindCtx(0, &mut bc) < 0 {
        return None;
    }
    let mut text: *mut u16 = core::ptr::null_mut();
    let hr = com_call!(
        obj,
        20,
        fn(*mut c_void, *mut c_void, *mut *mut u16) -> HResult,
        bc,
        core::ptr::null_mut(),
        &mut text
    );
    release(bc);
    if hr < 0 || text.is_null() {
        return None;
    }
    let mut name = [0u16; NAME];
    let len = read_wide(text, &mut name);
    super::CoTaskMemFree(text.cast());
    Some((kind, name, len?))
}

extern "system" fn query(this: *mut Moniker, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN
        || iid == IID_IMONIKER
        || iid == IID_IPERSIST
        || iid == IID_IPERSISTSTREAM
    {
        add(this);
        put(out, this.cast());
        return S_OK;
    }
    if iid == IID_IROTDATA {
        add(this);
        put(out, (&raw mut mk(this).rot_vtable).cast());
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    E_NOINTERFACE
}

extern "system" fn add(this: *mut Moniker) -> u32 {
    mk(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn rel(this: *mut Moniker) -> u32 {
    let left = mk(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(this.cast());
    }
    left
}

extern "system" fn get_class_id(this: *mut Moniker, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, class_of(mk(this).kind));
    S_OK
}

/// A moniker made here has never been changed since it was made.
extern "system" fn is_dirty(_: *mut Moniker) -> HResult {
    S_FALSE
}

extern "system" fn load(_: *mut Moniker, _stream: *mut c_void) -> HResult {
    missing(b"ole32.dll!IMoniker::Load(no serialized monikers)", 0);
    E_NOTIMPL
}

extern "system" fn save(_: *mut Moniker, _stream: *mut c_void, _clear: i32) -> HResult {
    missing(b"ole32.dll!IMoniker::Save(no serialized monikers)", 0);
    E_NOTIMPL
}

/// GetSizeMax: room for the name and a header. gala's own number, as
/// nothing can read back what Save does not write.
extern "system" fn get_size_max(this: *mut Moniker, out: *mut u64) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, u64::from(mk(this).len) * 2 + 64);
    S_OK
}

/// BindToObject: a file moniker whose file is not there cannot be bound
/// (MK_E_CANTOPENFILE, as measured); binding one that is there needs the
/// file's class, which gala does not register, so it is recorded.
extern "system" fn bind_to_object(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    _iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let m = mk(this);
    if m.kind == MKSYS_FILEMONIKER && !file_is_there(m.text()) {
        return MK_E_CANTOPENFILE;
    }
    missing(b"ole32.dll!IMoniker::BindToObject(nothing to bind to)", u64::from(m.kind));
    E_NOTIMPL
}

/// BindToStorage: gala has no IStorage, and a moniker that is asked for
/// an interface a storage would not give answers E_NOINTERFACE as
/// Windows does.
extern "system" fn bind_to_storage(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_ISTORAGE || iid == IID_ISTREAM || iid == IID_ILOCKBYTES {
        missing(b"ole32.dll!IMoniker::BindToStorage(no storage)", u64::from(mk(this).kind));
    }
    E_NOINTERFACE
}

extern "system" fn reduce(
    _this: *mut Moniker,
    _bc: *mut c_void,
    _how: u32,
    left: *mut *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if !left.is_null() {
        put(left, core::ptr::null_mut());
    }
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    missing(b"ole32.dll!IMoniker::Reduce", 0);
    E_NOTIMPL
}

/// The part of a path no `..` may climb out of: the drive and its
/// separator, or a UNC `\\server\share\`, and nothing at all for a
/// relative path.
fn root_of(path: &[u16]) -> usize {
    if path.len() >= 2 && is_separator(path[0]) && is_separator(path[1]) {
        let mut i = 2;
        // the server, then the share, each with its separator
        for _ in 0..2 {
            while i < path.len() && !is_separator(path[i]) {
                i += 1;
            }
            if i < path.len() {
                i += 1;
            }
        }
        return i;
    }
    if path.len() >= 2
        && path[1] == u16::from(b':')
        && (u16::from(b'A')..=u16::from(b'Z')).contains(&fold(path[0]))
    {
        return if path.len() >= 3 && is_separator(path[2]) { 3 } else { 2 };
    }
    0
}

/// A relative path merged into the one it is composed onto, as Windows'
/// file moniker does it (measured by `user/moniker_test`).
///
/// Each leading `..` of `right` takes one component of `left` and leaves
/// the separator behind it, so `C:\a\b.txt` and `..\up.txt` make
/// `C:\a\up.txt`; a leading `.` followed by a separator is dropped and
/// everything else is appended as written, inner `..` and `.` included.
/// Asking for more components than the path has above its root is
/// MK_E_SYNTAX -- `C:\` and `..` have nowhere to go -- and so is any
/// right-hand path at all when the left is empty.
fn merge_paths(left: &[u16], right: &[u16], out: &mut [u16; NAME]) -> Result<usize, HResult> {
    if left.is_empty() && !right.is_empty() {
        return Err(MK_E_SYNTAX);
    }
    let root = root_of(left);
    let mut cut = left.len();
    let mut rest = right;
    loop {
        let dots = rest.iter().take_while(|&&c| c == u16::from(b'.')).count();
        if dots == 0 || dots > 2 {
            break;
        }
        let ends = dots == rest.len();
        // ".." is a segment at the end of the name as well; a lone "."
        // there is kept, as Windows keeps it
        if (ends && dots == 1) || (!ends && !is_separator(rest[dots])) {
            break;
        }
        if dots == 2 {
            if cut <= root {
                return Err(MK_E_SYNTAX);
            }
            let end = if is_separator(left[cut - 1]) { cut - 1 } else { cut };
            cut = if end <= root {
                root
            } else {
                match left[root..end].iter().rposition(|&c| is_separator(c)) {
                    Some(i) => root + i + 1,
                    None => root,
                }
            };
        }
        rest = &rest[dots..];
        if !rest.is_empty() {
            rest = &rest[1..];
        }
    }
    let mut at = push(out, 0, &left[..cut]).ok_or(E_OUTOFMEMORY)?;
    if !rest.is_empty() && at > 0 && !is_separator(out[at - 1]) {
        at = push_ascii(out, at, b"\\").ok_or(E_OUTOFMEMORY)?;
    }
    push(out, at, rest).ok_or(E_OUTOFMEMORY)
}

/// ComposeWith: an anti moniker annihilates what it is composed onto,
/// which is S_OK with no moniker at all; otherwise the two names joined
/// in a generic composite, refused with MK_E_NEEDGENERIC when the caller
/// said it would not take one.
///
/// Two file monikers are the exception: they make no composite, so the
/// right path merges into the left (`merge_paths`) whether or not the
/// caller allows one, and an absolute right-hand path -- a drive or a
/// UNC share -- is MK_E_SYNTAX.
extern "system" fn compose_with(
    this: *mut Moniker,
    right: *mut c_void,
    only_if_not_generic: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if right.is_null() {
        return E_INVALIDARG;
    }
    let Some((kind, name, len)) = described(right) else { return E_INVALIDARG };
    if kind == MKSYS_ANTIMONIKER {
        return S_OK;
    }
    let m = mk(this);
    if m.kind == MKSYS_FILEMONIKER && kind == MKSYS_FILEMONIKER {
        if looks_like_a_path(&name[..len]) {
            return MK_E_SYNTAX;
        }
        let mut merged = [0u16; NAME];
        return match merge_paths(m.text(), &name[..len], &mut merged) {
            Ok(end) => handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], &merged[..end]), out),
            Err(E_OUTOFMEMORY) => {
                missing(b"ole32.dll!IMoniker::ComposeWith(name longer than 320)", 0);
                E_OUTOFMEMORY
            }
            Err(e) => e,
        };
    }
    if only_if_not_generic != 0 {
        return MK_E_NEEDGENERIC;
    }
    let mut joined = [0u16; NAME];
    let Some(at) = push(&mut joined, 0, m.text()) else {
        missing(b"ole32.dll!IMoniker::ComposeWith(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    let Some(end) = push(&mut joined, at, &name[..len]) else {
        missing(b"ole32.dll!IMoniker::ComposeWith(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    handed(new_moniker(MKSYS_GENERICCOMPOSITE, [0; 16], &joined[..end]), out)
}

/// Enum: a moniker that is not a composite has no parts to walk, which
/// Windows answers with S_OK and no enumerator at all.
extern "system" fn enum_parts(this: *mut Moniker, _forward: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if mk(this).kind == MKSYS_GENERICCOMPOSITE {
        missing(b"ole32.dll!IMoniker::Enum(a composite's parts)", 0);
        return E_NOTIMPL;
    }
    S_OK
}

extern "system" fn is_equal(this: *mut Moniker, other: *mut c_void) -> HResult {
    if other.is_null() {
        return E_INVALIDARG;
    }
    let Some((kind, name, len)) = described(other) else { return S_FALSE };
    let m = mk(this);
    if m.kind == kind && same_name(m.text(), &name[..len]) { S_OK } else { S_FALSE }
}

/// Hash: a number equal monikers share. An anti moniker's is Windows'
/// 0x80000001, which programs test for.
extern "system" fn hash(this: *mut Moniker, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let m = mk(this);
    if m.kind == MKSYS_ANTIMONIKER {
        put(out, 0x8000_0001);
        return S_OK;
    }
    put(out, hash_name(m.text()));
    S_OK
}

/// IsRunning: whether the running object table holds this name. A class
/// moniker does not answer the question at all on Windows (E_NOTIMPL),
/// and programs branch on that.
extern "system" fn is_running(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    _newly_running: *mut c_void,
) -> HResult {
    let m = mk(this);
    if m.kind == MKSYS_CLASSMONIKER {
        return E_NOTIMPL;
    }
    if registered(m.text()).is_some() { S_OK } else { S_FALSE }
}

extern "system" fn time_of_last_change(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    out: *mut u64,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let m = mk(this);
    if let Some(entry) = registered(m.text()) {
        put(out, with_slot(entry.0, |e| e.time));
        return S_OK;
    }
    missing(b"ole32.dll!IMoniker::GetTimeOfLastChange(not running)", u64::from(m.kind));
    MK_E_UNAVAILABLE
}

/// Inverse: the anti moniker that undoes one step, for the kinds that
/// have one. An anti moniker has no inverse (MK_E_NOINVERSE).
extern "system" fn inverse(this: *mut Moniker, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let m = mk(this);
    match m.kind {
        MKSYS_FILEMONIKER | MKSYS_ITEMMONIKER => {
            let mut name = [0u16; NAME];
            let end = push_ascii(&mut name, 0, b"\\..").unwrap_or(0);
            handed(new_moniker(MKSYS_ANTIMONIKER, [0; 16], &name[..end]), out)
        }
        MKSYS_ANTIMONIKER => MK_E_NOINVERSE,
        kind => {
            missing(b"ole32.dll!IMoniker::Inverse", u64::from(kind));
            E_NOTIMPL
        }
    }
}

/// The length of the shared prefix of two paths, up to and including the
/// separator that ends it (Windows keeps that separator), or None when
/// they share no directory.
fn shared_directory(a: &[u16], b: &[u16]) -> Option<usize> {
    let mut end = None;
    for i in 0..a.len().min(b.len()) {
        if fold(a[i]) != fold(b[i]) {
            break;
        }
        if is_separator(a[i]) {
            end = Some(i + 1);
        }
    }
    end
}

extern "system" fn common_prefix_with(
    this: *mut Moniker,
    other: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if other.is_null() {
        return E_INVALIDARG;
    }
    let Some((kind, name, len)) = described(other) else { return E_INVALIDARG };
    let m = mk(this);
    if m.kind == kind && same_name(m.text(), &name[..len]) {
        // The prefix of a moniker and itself is itself, said with MK_S_US.
        let p = new_moniker(m.kind, m.clsid, m.text());
        let hr = handed(p, out);
        return if hr == S_OK { MK_S_US } else { hr };
    }
    if m.kind != MKSYS_FILEMONIKER || kind != MKSYS_FILEMONIKER {
        missing(b"ole32.dll!IMoniker::CommonPrefixWith", u64::from(m.kind));
        return MK_E_NOPREFIX;
    }
    let Some(end) = shared_directory(m.text(), &name[..len]) else { return MK_E_NOPREFIX };
    handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], &m.text()[..end]), out)
}

/// RelativePathTo: one ".." for every component of this moniker's path
/// past the shared directory -- it steps out of its own last component
/// too -- and then the rest of the other's.
extern "system" fn relative_path_to(
    this: *mut Moniker,
    other: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if other.is_null() {
        return E_INVALIDARG;
    }
    let Some((kind, name, len)) = described(other) else { return E_INVALIDARG };
    let m = mk(this);
    if m.kind != MKSYS_FILEMONIKER || kind != MKSYS_FILEMONIKER {
        missing(b"ole32.dll!IMoniker::RelativePathTo", u64::from(m.kind));
        return MK_E_NOPREFIX;
    }
    let mine = m.text();
    let theirs = &name[..len];
    let Some(shared) = shared_directory(mine, theirs) else { return MK_E_NOPREFIX };
    let ups = 1 + mine[shared..].iter().filter(|c| is_separator(**c)).count();
    let mut path = [0u16; NAME];
    let mut at = 0;
    for _ in 0..ups {
        match push_ascii(&mut path, at, b"..\\") {
            Some(next) => at = next,
            None => {
                missing(b"ole32.dll!IMoniker::RelativePathTo(name longer than 320)", 0);
                return E_OUTOFMEMORY;
            }
        }
    }
    let Some(end) = push(&mut path, at, &theirs[shared..]) else {
        missing(b"ole32.dll!IMoniker::RelativePathTo(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], &path[..end]), out)
}

extern "system" fn get_display_name(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let m = mk(this);
    let text = super::CoTaskMemAlloc((m.len as usize + 1) * 2).cast::<u16>();
    if text.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a block of len + 1 units, just allocated.
    unsafe {
        core::ptr::copy_nonoverlapping(m.name.as_ptr(), text, m.len as usize);
        text.add(m.len as usize).write(0);
    }
    put(out, text);
    S_OK
}

/// ParseDisplayName: continuing a name past this moniker needs the
/// object it names, which gala cannot bind. Windows refuses an extension
/// it cannot parse with MK_E_INVALIDEXTENSION and, unlike
/// `MkParseDisplayName`, leaves the caller's count alone -- measured,
/// and programs read that count only on success.
extern "system" fn parse_display_name(
    this: *mut Moniker,
    _bc: *mut c_void,
    _left: *mut c_void,
    _name: *const u16,
    _eaten: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    missing(b"ole32.dll!IMoniker::ParseDisplayName", u64::from(mk(this).kind));
    MK_E_INVALIDEXTENSION
}

extern "system" fn is_system_moniker(this: *mut Moniker, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, mk(this).kind);
    S_OK
}

static VTABLE: VTable<23> = VTable([
    query as *const (),
    add as *const (),
    rel as *const (),
    get_class_id as *const (),
    is_dirty as *const (),
    load as *const (),
    save as *const (),
    get_size_max as *const (),
    bind_to_object as *const (),
    bind_to_storage as *const (),
    reduce as *const (),
    compose_with as *const (),
    enum_parts as *const (),
    is_equal as *const (),
    hash as *const (),
    is_running as *const (),
    time_of_last_change as *const (),
    inverse as *const (),
    common_prefix_with as *const (),
    relative_path_to as *const (),
    get_display_name as *const (),
    parse_display_name as *const (),
    is_system_moniker as *const (),
]);

// ---- IROTData, the moniker's second interface ----

/// The object behind an IROTData pointer, which is the address of its
/// `rot_vtable` field.
fn rot_of(this: *mut c_void) -> *mut Moniker {
    // SAFETY: a pointer to our object's second vtable field, one pointer
    // past the object's start.
    unsafe { this.cast::<u8>().sub(core::mem::size_of::<*const usize>()).cast::<Moniker>() }
}

extern "system" fn rotdata_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    query(rot_of(this), iid, out)
}

extern "system" fn rotdata_add(this: *mut c_void) -> u32 {
    add(rot_of(this))
}

extern "system" fn rotdata_rel(this: *mut c_void) -> u32 {
    rel(rot_of(this))
}

/// IROTData::GetComparisonData: the bytes the running object table
/// compares monikers by. gala's table compares display names instead, so
/// nothing here reads this; a program that asks is recorded.
extern "system" fn comparison_data(
    _this: *mut c_void,
    _data: *mut u8,
    _max: u32,
    written: *mut u32,
) -> HResult {
    if !written.is_null() {
        put(written, 0);
    }
    missing(b"ole32.dll!IROTData::GetComparisonData", 0);
    E_NOTIMPL
}

static ROTDATA_VTABLE: VTable<4> = VTable([
    rotdata_query as *const (),
    rotdata_add as *const (),
    rotdata_rel as *const (),
    comparison_data as *const (),
]);

// ---- The makers ----

/// CreateFileMoniker: a moniker for `path`, kept exactly as given
/// (relative and empty paths included). A NULL path is MK_E_SYNTAX.
///
/// Windows does not check the out pointer and faults on a NULL one
/// (measured); gala refuses it.
#[unsafe(no_mangle)]
pub extern "system" fn CreateFileMoniker(path: *const u16, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if path.is_null() {
        return MK_E_SYNTAX;
    }
    let mut name = [0u16; NAME];
    let Some(len) = read_wide(path, &mut name) else {
        missing(b"ole32.dll!CreateFileMoniker(path longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], &name[..len]), out)
}

/// CreateItemMoniker: `<delimiter><item>`, the delimiter optional.
#[unsafe(no_mangle)]
pub extern "system" fn CreateItemMoniker(
    delimiter: *const u16,
    item: *const u16,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if item.is_null() {
        return E_INVALIDARG;
    }
    let mut name = [0u16; NAME];
    let mut at = 0;
    if !delimiter.is_null() {
        let mut d = [0u16; NAME];
        let Some(len) = read_wide(delimiter, &mut d) else {
            missing(b"ole32.dll!CreateItemMoniker(name longer than 320)", 0);
            return E_OUTOFMEMORY;
        };
        at = push(&mut name, 0, &d[..len]).unwrap_or(0);
    }
    let mut i = [0u16; NAME];
    let Some(len) = read_wide(item, &mut i) else {
        missing(b"ole32.dll!CreateItemMoniker(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    let Some(end) = push(&mut name, at, &i[..len]) else {
        missing(b"ole32.dll!CreateItemMoniker(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    handed(new_moniker(MKSYS_ITEMMONIKER, [0; 16], &name[..end]), out)
}

/// CreateAntiMoniker: the one that undoes a step, whose display name is
/// `\..`.
#[unsafe(no_mangle)]
pub extern "system" fn CreateAntiMoniker(out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let mut name = [0u16; NAME];
    let end = push_ascii(&mut name, 0, b"\\..").unwrap_or(0);
    handed(new_moniker(MKSYS_ANTIMONIKER, [0; 16], &name[..end]), out)
}

/// `clsid:<guid>:`, the display name of a class moniker, in upper case
/// as Windows writes it.
/// CreateGenericComposite: the left moniker's own `ComposeWith` with a
/// generic composite allowed, and a NULL on either side handing back the
/// other moniker with a reference of its own (measured on Windows 11).
///
/// Windows reads `out` without checking it and faults on a NULL; gala
/// answers E_POINTER, which is the one deliberate difference here.
#[unsafe(no_mangle)]
pub extern "system" fn CreateGenericComposite(
    first: *mut c_void,
    rest: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let only = if first.is_null() {
        rest
    } else if rest.is_null() {
        first
    } else {
        core::ptr::null_mut()
    };
    if !only.is_null() {
        add_ref(only);
        put(out, only);
        return S_OK;
    }
    if first.is_null() {
        return S_OK;
    }
    com_call!(first, 11, fn(*mut c_void, i32, *mut *mut c_void) -> HResult, rest, 0, out)
}

fn class_name(clsid: &Guid, out: &mut [u16]) -> usize {
    let braced = super::guid_text(clsid);
    let at = push_ascii(out, 0, b"clsid:").unwrap_or(0);
    // Without the braces.
    let at = push(out, at, &braced[1..37]).unwrap_or(at);
    push_ascii(out, at, b":").unwrap_or(at)
}

/// CreateClassMoniker: a moniker naming a class, whose display name is
/// `clsid:<guid>:`.
#[unsafe(no_mangle)]
pub extern "system" fn CreateClassMoniker(clsid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if clsid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's CLSID.
    let clsid = unsafe { *clsid };
    let mut name = [0u16; NAME];
    let end = class_name(&clsid, &mut name);
    handed(new_moniker(MKSYS_CLASSMONIKER, clsid, &name[..end]), out)
}

// ---- Parsing a display name ----

fn file_is_there(path: &[u16]) -> bool {
    attributes(path).is_some_and(|a| a & FILE_ATTRIBUTE_DIRECTORY == 0)
}

fn attributes(path: &[u16]) -> Option<u32> {
    if path.is_empty() || path.len() >= NAME {
        return None;
    }
    let mut z = [0u16; NAME];
    z[..path.len()].copy_from_slice(path);
    // SAFETY: a NUL-terminated buffer of ours.
    let a = unsafe { GetFileAttributesW(z.as_ptr()) };
    if a == INVALID_FILE_ATTRIBUTES { None } else { Some(a) }
}

/// Whether `name` is shaped like a path: a drive letter and a colon, or
/// a UNC `\\server\share`.
fn looks_like_a_path(name: &[u16]) -> bool {
    if name.len() >= 2 && is_separator(name[0]) && is_separator(name[1]) {
        return true;
    }
    name.len() >= 2
        && name[1] == u16::from(b':')
        && (u16::from(b'A')..=u16::from(b'Z')).contains(&fold(name[0]))
}

/// `clsid:<guid>:` parsed, the GUID unbraced and in either case.
fn parse_class_name(name: &[u16]) -> Option<Guid> {
    const PREFIX: &[u8] = b"clsid:";
    if name.len() != PREFIX.len() + 37 {
        return None;
    }
    if !name[..PREFIX.len()].iter().zip(PREFIX).all(|(c, p)| fold(*c) == fold(u16::from(*p))) {
        return None;
    }
    if name[name.len() - 1] != u16::from(b':') {
        return None;
    }
    let mut braced = [0u16; 39];
    braced[0] = u16::from(b'{');
    braced[1..37].copy_from_slice(&name[PREFIX.len()..PREFIX.len() + 36]);
    braced[37] = u16::from(b'}');
    super::parse_guid(braced.as_ptr())
}

/// MkParseDisplayName: the moniker a display name names, and how much of
/// the name it took.
///
/// Measured on Windows 11 (`user/moniker_test`): a NULL argument and an
/// empty name are E_INVALIDARG; a path to a file that is there is S_OK
/// with the whole name eaten; a file that is not there and a directory
/// are both MK_E_CANTOPENFILE with nothing eaten; a bare word and a URL
/// are MK_E_SYNTAX; a name beginning `@` is E_NOINTERFACE; and a file
/// followed by an extension that cannot be parsed is
/// MK_E_INVALIDEXTENSION with the file part eaten and the file's moniker
/// handed back -- a failure that still gives the caller an object.
#[unsafe(no_mangle)]
pub extern "system" fn MkParseDisplayName(
    bc: *mut c_void,
    display_name: *const u16,
    eaten: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    if bc.is_null() || display_name.is_null() || eaten.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let mut name = [0u16; NAME];
    let Some(len) = read_wide(display_name, &mut name) else {
        missing(b"ole32.dll!MkParseDisplayName(name longer than 320)", 0);
        return E_OUTOFMEMORY;
    };
    if len == 0 {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    // A name that is refused has nothing eaten, which Windows writes
    // rather than leaving the caller's count alone -- measured, and
    // unlike IMoniker::ParseDisplayName, which leaves it. Only the
    // arguments themselves being wrong (E_INVALIDARG above) leaves it.
    put(eaten, 0);
    let name = &name[..len];

    if let Some(clsid) = parse_class_name(name) {
        let mut text = [0u16; NAME];
        let end = class_name(&clsid, &mut text);
        let hr = handed(new_moniker(MKSYS_CLASSMONIKER, clsid, &text[..end]), out);
        if hr == S_OK {
            put(eaten, len as u32);
        }
        return hr;
    }

    // "@<ProgID>" names a class by its ProgID, which gala does not
    // register; Windows answers a ProgID that names no moniker class
    // E_NOINTERFACE, and so does gala.
    if name[0] == u16::from(b'@') {
        missing(b"ole32.dll!MkParseDisplayName(@ProgID)", 0);
        return E_NOINTERFACE;
    }

    if !looks_like_a_path(name) {
        missing(b"ole32.dll!MkParseDisplayName(not a path)", len as u64);
        return MK_E_SYNTAX;
    }

    if file_is_there(name) {
        let hr = handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], name), out);
        if hr == S_OK {
            put(eaten, len as u32);
        }
        return hr;
    }

    // A file and then something else: the part up to the first "!" is
    // the file, and what follows is an extension nothing here can parse.
    if let Some(bang) = name.iter().position(|c| *c == u16::from(b'!'))
        && file_is_there(&name[..bang])
    {
        let hr = handed(new_moniker(MKSYS_FILEMONIKER, [0; 16], &name[..bang]), out);
        if hr != S_OK {
            return hr;
        }
        put(eaten, bang as u32);
        missing(b"ole32.dll!MkParseDisplayName(an extension past the file)", 0);
        return MK_E_INVALIDEXTENSION;
    }

    MK_E_CANTOPENFILE
}

// ---- The running object table ----

/// How many objects a process may have running at once. Windows has no
/// such limit; one past this is E_OUTOFMEMORY with a row to say so.
const ENTRIES: usize = 64;

struct Entry {
    /// Zero for a free slot.
    key: u32,
    object: *mut c_void,
    moniker: *mut c_void,
    name: [u16; NAME],
    len: u32,
    time: u64,
    /// How many look-ups are using this entry outside the lock.
    borrows: u32,
    /// Revoked while borrowed: out of reach, its references still held.
    retired: bool,
}

const FREE: Entry = Entry {
    key: 0,
    object: core::ptr::null_mut(),
    moniker: core::ptr::null_mut(),
    name: [0; NAME],
    len: 0,
    time: 0,
    borrows: 0,
    retired: false,
};

struct Running {
    lock: Lock,
    slots: UnsafeCell<[Entry; ENTRIES]>,
    /// The next key. It only rises, so a key is never reused and a stale
    /// one names nothing.
    next_key: AtomicU32,
}

// SAFETY: `slots` is only touched under `lock`.
unsafe impl Sync for Running {}

static RUNNING: Running = Running {
    lock: Lock::new(b"ole32.dll!the running object table"),
    slots: UnsafeCell::new([FREE; ENTRIES]),
    next_key: AtomicU32::new(1),
};

/// A borrow on a table entry, given back when it goes out of scope.
///
/// A forgotten borrow loses no memory but a great deal of meaning: a
/// revoke that lands while one is outstanding leaves the entry out of
/// reach with its references still held, so the object stays alive for
/// ever and its owner cannot tell why. That happened once, to
/// `IMoniker::IsRunning`, which asked the table a question and dropped
/// the answer's borrow on the floor. A borrow is a value the compiler
/// insists on giving back rather than a call to remember.
struct Borrowed(usize);

impl Drop for Borrowed {
    fn drop(&mut self) {
        give_back(self.0);
    }
}

/// The slot holding `name`, with a borrow taken on it so a revoke
/// meanwhile cannot free what the caller is about to read.
fn registered(name: &[u16]) -> Option<Borrowed> {
    let at = RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *RUNNING.slots.get() };
        let at = slots
            .iter()
            .position(|e| e.key != 0 && !e.retired && same_name(&e.name[..e.len as usize], name))?;
        slots[at].borrows += 1;
        Some(at)
    })?;
    Some(Borrowed(at))
}

/// The slot with `key`, borrowed as `registered` borrows.
fn by_key(key: u32) -> Option<Borrowed> {
    if key == 0 {
        return None;
    }
    let at = RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *RUNNING.slots.get() };
        let at = slots.iter().position(|e| e.key == key && !e.retired)?;
        slots[at].borrows += 1;
        Some(at)
    })?;
    Some(Borrowed(at))
}

/// Reads something out of a borrowed slot under the lock.
fn with_slot<T>(slot: usize, read: impl FnOnce(&mut Entry) -> T) -> T {
    RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        read(&mut unsafe { &mut *RUNNING.slots.get() }[slot])
    })
}

/// Gives back a borrow, and finishes the revoke that was waiting on it
/// (`Borrowed`'s Drop, which is the only caller).
fn give_back(slot: usize) {
    let (object, moniker) = RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let e = &mut unsafe { &mut *RUNNING.slots.get() }[slot];
        e.borrows -= 1;
        if e.borrows == 0 && e.retired {
            let held = (e.object, e.moniker);
            *e = FREE;
            return held;
        }
        (core::ptr::null_mut(), core::ptr::null_mut())
    });
    // Released with the lock let go: Release is the program's code.
    if !object.is_null() {
        release(object);
    }
    if !moniker.is_null() {
        release(moniker);
    }
}

/// A moniker's display name, for the table to key on. The call into the
/// moniker is made before any lock is taken.
fn name_of(moniker: *mut c_void) -> Option<([u16; NAME], usize)> {
    let (_, name, len) = described(moniker)?;
    Some((name, len))
}

#[repr(C)]
struct Rot {
    vtable: *const VTable<10>,
}

// SAFETY: a pointer to a static vtable.
unsafe impl Sync for Rot {}

static ROT: Rot = Rot { vtable: &ROT_VTABLE };

extern "system" fn rot_query(this: usize, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IRUNNINGOBJECTTABLE {
        put(out, this as *mut c_void);
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    E_NOINTERFACE
}

/// The table is the process's and outlives every caller, so its
/// reference count is a formality (as the task allocator's is).
extern "system" fn rot_add(_this: usize) -> u32 {
    1
}

extern "system" fn rot_release(_this: usize) -> u32 {
    1
}

/// IRunningObjectTable::Register: the object is now running under that
/// moniker, and the table holds a reference on both until it is revoked.
/// Registering a name that is already there is a success of its own
/// (MK_S_MONIKERALREADYREGISTERED) with a second key, as measured.
extern "system" fn rot_register(
    _this: usize,
    flags: u32,
    object: *mut c_void,
    moniker: *mut c_void,
    key: *mut u32,
) -> HResult {
    if key.is_null() {
        return E_POINTER;
    }
    put(key, 0);
    if object.is_null() || moniker.is_null() {
        return E_INVALIDARG;
    }
    if flags != 0 {
        // ROTFLAGS_REGISTRATIONKEEPSALIVE and ALLOWANYCLIENT: gala holds
        // a strong reference either way and has no other client.
        missing(b"ole32.dll!IRunningObjectTable::Register(flags)", u64::from(flags));
    }
    let Some((name, len)) = name_of(moniker) else { return E_INVALIDARG };
    let mut now = 0u64;
    // SAFETY: a u64 of ours.
    unsafe { super::GetSystemTimeAsFileTime(&mut now) };
    // The references are taken before the lock: AddRef is the program's
    // code (gala_lock: nothing is held across a call into a program).
    add_ref(object);
    add_ref(moniker);
    let new_key = RUNNING.next_key.fetch_add(1, Ordering::Relaxed).max(1);
    let (taken, already) = RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *RUNNING.slots.get() };
        let already = slots.iter().any(|e| {
            e.key != 0 && !e.retired && same_name(&e.name[..e.len as usize], &name[..len])
        });
        match slots.iter_mut().position(|e| e.key == 0) {
            Some(at) => {
                slots[at] = Entry {
                    key: new_key,
                    object,
                    moniker,
                    name,
                    len: len as u32,
                    time: now,
                    borrows: 0,
                    retired: false,
                };
                (true, already)
            }
            None => (false, already),
        }
    });
    if !taken {
        release(object);
        release(moniker);
        missing(b"ole32.dll!IRunningObjectTable::Register(more than 64)", 0);
        return E_OUTOFMEMORY;
    }
    put(key, new_key);
    if already { MK_S_MONIKERALREADYREGISTERED } else { S_OK }
}

/// IRunningObjectTable::Revoke: the references given back. A key that
/// names nothing -- one already revoked included -- is E_INVALIDARG, as
/// measured.
extern "system" fn rot_revoke(_this: usize, key: u32) -> HResult {
    if key == 0 {
        return E_INVALIDARG;
    }
    let (object, moniker, found) = RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *RUNNING.slots.get() };
        let Some(at) = slots.iter().position(|e| e.key == key && !e.retired) else {
            return (core::ptr::null_mut(), core::ptr::null_mut(), false);
        };
        let e = &mut slots[at];
        if e.borrows > 0 {
            // Someone is reading it: out of reach now, freed by the last
            // borrow's `give_back`.
            e.retired = true;
            e.key = 0;
            return (core::ptr::null_mut(), core::ptr::null_mut(), true);
        }
        let held = (e.object, e.moniker, true);
        *e = FREE;
        held
    });
    if !found {
        return E_INVALIDARG;
    }
    if !object.is_null() {
        release(object);
    }
    if !moniker.is_null() {
        release(moniker);
    }
    S_OK
}

/// IRunningObjectTable::IsRunning: by the moniker's value, not its
/// pointer -- a separate but equal moniker finds the entry.
extern "system" fn rot_is_running(_this: usize, moniker: *mut c_void) -> HResult {
    if moniker.is_null() {
        return E_INVALIDARG;
    }
    let Some((name, len)) = name_of(moniker) else { return E_INVALIDARG };
    if registered(&name[..len]).is_some() { S_OK } else { S_FALSE }
}

/// IRunningObjectTable::GetObject: the registered object, AddRef'd;
/// MK_E_UNAVAILABLE when nothing is running under that name.
extern "system" fn rot_get_object(
    _this: usize,
    moniker: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if moniker.is_null() {
        return E_INVALIDARG;
    }
    let Some((name, len)) = name_of(moniker) else { return E_INVALIDARG };
    let Some(entry) = registered(&name[..len]) else { return MK_E_UNAVAILABLE };
    let object = with_slot(entry.0, |e| e.object);
    // AddRef with the lock let go; the borrow keeps the entry alive.
    add_ref(object);
    put(out, object);
    S_OK
}

extern "system" fn rot_note_change_time(_this: usize, key: u32, time: *const u64) -> HResult {
    if time.is_null() {
        return E_INVALIDARG;
    }
    let Some(entry) = by_key(key) else { return E_INVALIDARG };
    // SAFETY: the caller's FILETIME.
    let now = unsafe { *time };
    with_slot(entry.0, |e| e.time = now);
    S_OK
}

extern "system" fn rot_time_of_last_change(
    _this: usize,
    moniker: *mut c_void,
    out: *mut u64,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if moniker.is_null() {
        return E_INVALIDARG;
    }
    let Some((name, len)) = name_of(moniker) else { return E_INVALIDARG };
    let Some(entry) = registered(&name[..len]) else { return MK_E_UNAVAILABLE };
    put(out, with_slot(entry.0, |e| e.time));
    S_OK
}

extern "system" fn rot_enum_running(_this: usize, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let p = alloc_zeroed(core::mem::size_of::<EnumMoniker>()).cast::<EnumMoniker>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(EnumMoniker {
            vtable: ENUM_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            at: 0,
            count: 0,
            items: [core::ptr::null_mut(); ENTRIES],
        })
    };
    // A snapshot of the monikers, each borrowed under the lock and
    // AddRef'd outside it.
    let mut borrowed = [usize::MAX; ENTRIES];
    let mut found = 0;
    RUNNING.lock.with(|| {
        // SAFETY: under the lock.
        let slots = unsafe { &mut *RUNNING.slots.get() };
        for (at, e) in slots.iter_mut().enumerate() {
            if e.key != 0 && !e.retired {
                e.borrows += 1;
                borrowed[found] = at;
                found += 1;
            }
        }
    });
    let e = enum_of(p);
    for &slot in borrowed.iter().take(found) {
        // Each slot was borrowed under the lock above; taking it as a
        // `Borrowed` here gives it back at the end of the turn.
        let entry = Borrowed(slot);
        let moniker = with_slot(entry.0, |held| held.moniker);
        add_ref(moniker);
        e.items[e.count as usize] = moniker;
        e.count += 1;
    }
    put(out, p.cast());
    S_OK
}

static ROT_VTABLE: VTable<10> = VTable([
    rot_query as *const (),
    rot_add as *const (),
    rot_release as *const (),
    rot_register as *const (),
    rot_revoke as *const (),
    rot_is_running as *const (),
    rot_get_object as *const (),
    rot_note_change_time as *const (),
    rot_time_of_last_change as *const (),
    rot_enum_running as *const (),
]);

/// GetRunningObjectTable: the process's table, the same object every
/// time (measured: two calls give one pointer).
///
/// Windows does not check the out pointer and faults on a NULL one
/// (measured); gala refuses it. `reserved` is ignored, as Windows
/// ignores it.
#[unsafe(no_mangle)]
pub extern "system" fn GetRunningObjectTable(_reserved: u32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, (&raw const ROT).cast_mut().cast());
    S_OK
}

/// What `IBindCtx::GetRunningObjectTable` hands back: the same table.
pub fn bind_ctx_table(out: *mut *mut c_void) -> HResult {
    GetRunningObjectTable(0, out)
}

// ---- IEnumMoniker ----

#[repr(C)]
struct EnumMoniker {
    vtable: *const usize,
    refs: AtomicU32,
    at: u32,
    count: u32,
    items: [*mut c_void; ENTRIES],
}

fn enum_of<'a>(this: *mut EnumMoniker) -> &'a mut EnumMoniker {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &mut *this }
}

extern "system" fn enum_query(
    this: *mut EnumMoniker,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IENUMMONIKER {
        enum_add(this);
        put(out, this.cast());
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    E_NOINTERFACE
}

extern "system" fn enum_add(this: *mut EnumMoniker) -> u32 {
    enum_of(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn enum_rel(this: *mut EnumMoniker) -> u32 {
    let left = enum_of(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let e = enum_of(this);
        for i in 0..e.count as usize {
            release(e.items[i]);
        }
        free(this.cast());
    }
    left
}

/// IEnumMoniker::Next: up to `count` monikers, each AddRef'd. S_OK when
/// all were there, S_FALSE otherwise -- and S_FALSE still writes the
/// count it managed, as measured. `fetched` may be NULL when one is
/// asked for.
extern "system" fn enum_next(
    this: *mut EnumMoniker,
    count: u32,
    out: *mut *mut c_void,
    fetched: *mut u32,
) -> HResult {
    if out.is_null() || (fetched.is_null() && count != 1) {
        return E_INVALIDARG;
    }
    let e = enum_of(this);
    let mut given = 0;
    while given < count && e.at < e.count {
        let moniker = e.items[e.at as usize];
        add_ref(moniker);
        // SAFETY: the caller's array of `count` pointers.
        unsafe { out.add(given as usize).write(moniker) };
        e.at += 1;
        given += 1;
    }
    if !fetched.is_null() {
        put(fetched, given);
    }
    if given == count { S_OK } else { S_FALSE }
}

extern "system" fn enum_skip(this: *mut EnumMoniker, count: u32) -> HResult {
    let e = enum_of(this);
    let left = e.count - e.at;
    e.at += count.min(left);
    if count <= left { S_OK } else { S_FALSE }
}

extern "system" fn enum_reset(this: *mut EnumMoniker) -> HResult {
    enum_of(this).at = 0;
    S_OK
}

extern "system" fn enum_clone(this: *mut EnumMoniker, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let e = enum_of(this);
    let p = alloc_zeroed(core::mem::size_of::<EnumMoniker>()).cast::<EnumMoniker>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(EnumMoniker {
            vtable: ENUM_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            at: e.at,
            count: e.count,
            items: e.items,
        })
    };
    for i in 0..e.count as usize {
        add_ref(e.items[i]);
    }
    put(out, p.cast());
    S_OK
}

static ENUM_VTABLE: VTable<7> = VTable([
    enum_query as *const (),
    enum_add as *const (),
    enum_rel as *const (),
    enum_next as *const (),
    enum_skip as *const (),
    enum_reset as *const (),
    enum_clone as *const (),
]);

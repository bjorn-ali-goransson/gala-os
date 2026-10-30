//! The object manager namespace: a tree of directory objects rooted at `\`.
//!
//! Lookup rules follow the documented behaviour of the NT object manager:
//! absolute paths start with `\`; paths relative to a RootDirectory handle
//! must not; a missing intermediate component is
//! STATUS_OBJECT_PATH_NOT_FOUND, a missing final one
//! STATUS_OBJECT_NAME_NOT_FOUND.

use alloc::vec::Vec;

use super::object::{
    DirEntry, Directory, KObject, ObjectBody, ObjectName, ObjectRef, SymbolicLink,
};
use super::status::NtStatus;
use crate::mm::heap::{AllocError, TryVec};
use crate::sync::IrqMutex;

const BACKSLASH: u16 = b'\\' as u16;

/// Serializes structural changes (link/unlink) and lookups.
static NAMESPACE: IrqMutex<Option<ObjectRef>> = IrqMutex::new(None);

/// Directories that exist at boot (Windows has these and more).
const SYSTEM_DIRECTORIES: &[&str] = &[
    "??",
    "BaseNamedObjects",
    "Device",
    "KernelObjects",
    "Sessions",
    "Sessions\\1",
    "Sessions\\1\\BaseNamedObjects",
];

fn wide(s: &str) -> Result<Vec<u16>, AllocError> {
    let mut v = Vec::new();
    v.try_reserve(s.len()).map_err(|_| AllocError)?;
    for b in s.bytes() {
        v.try_push(u16::from(b))?;
    }
    Ok(v)
}

fn new_directory() -> Result<ObjectRef, AllocError> {
    KObject::with_permanence(ObjectBody::Directory(Directory::default()), true)
}

pub fn init() -> Result<(), NtStatus> {
    let root = new_directory().map_err(|_| NtStatus::NO_MEMORY)?;
    *NAMESPACE.lock() = Some(root);
    for path in SYSTEM_DIRECTORIES {
        let mut full = wide("\\").map_err(|_| NtStatus::NO_MEMORY)?;
        full.try_extend_from_slice(&wide(path).map_err(|_| NtStatus::NO_MEMORY)?)
            .map_err(|_| NtStatus::NO_MEMORY)?;
        let dir = new_directory().map_err(|_| NtStatus::NO_MEMORY)?;
        create(None, &full, dir, true, false)?;
    }
    for (name, target) in SYSTEM_LINKS {
        let target = wide(target).map_err(|_| NtStatus::NO_MEMORY)?;
        let link = new_symbolic_link(&target)?;
        create(None, &wide(name).map_err(|_| NtStatus::NO_MEMORY)?, link, true, false)?;
    }
    Ok(())
}

/// Symbolic links that exist at boot.
const SYSTEM_LINKS: &[(&str, &str)] =
    &[("\\Sessions\\1\\BaseNamedObjects\\Global", "\\BaseNamedObjects")];

/// Compares names, ignoring ASCII case if `ci`.
// TODO: full Unicode upcase table, as the object manager uses.
fn same_name(a: &[u16], b: &[u16], ci: bool) -> bool {
    let fold =
        |c: u16| if ci && (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

fn find(dir: &ObjectRef, name: &[u16], ci: bool) -> Option<ObjectRef> {
    let d = dir.as_directory()?;
    d.entries.lock().iter().find(|e| same_name(&e.name, name, ci)).map(|e| e.object.clone())
}

/// How many symbolic links one lookup may follow.
const MAX_REPARSE: usize = 32;

/// What a walk of the namespace should produce.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// The directory and final component (to create an object there).
    Parent,
    /// The object named by the path; a final symbolic link is followed
    /// unless `follow_final` is false (opening the link itself).
    Object { follow_final: bool },
    /// The first object that is not a directory or link (e.g. a device),
    /// with the rest of the path.
    Parse,
}

enum Found {
    Parent(ObjectRef, Vec<u16>),
    Object(ObjectRef),
    Parsed(ObjectRef, Vec<u16>),
}

fn owned(s: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_extend_from_slice(s).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(v)
}

/// Walks `path` (relative to `start`, if given) from `root`, following
/// symbolic links by substituting their target for the components walked
/// so far and starting again. Must be called with the namespace lock held.
fn walk(
    root: &ObjectRef,
    start: Option<&ObjectRef>,
    path: &[u16],
    ci: bool,
    want: Want,
) -> Result<Found, NtStatus> {
    let mut path = owned(path)?;
    let mut start = start.cloned();
    for _ in 0..=MAX_REPARSE {
        let (mut dir, body_start) = match &start {
            None => match path.first() {
                Some(&BACKSLASH) => (root.clone(), 1),
                _ => return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD),
            },
            Some(s) => {
                if path.first() == Some(&BACKSLASH) {
                    return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
                }
                if want != Want::Parse && s.as_directory().is_none() {
                    return Err(NtStatus::OBJECT_TYPE_MISMATCH);
                }
                (s.clone(), 0)
            }
        };
        let rest = &path[body_start..];
        if rest.is_empty() {
            return Ok(match want {
                Want::Parent => Found::Parent(dir, Vec::new()),
                Want::Object { .. } => Found::Object(dir),
                Want::Parse => Found::Parsed(dir, Vec::new()),
            });
        }
        let mut at = 0;
        let reparse = loop {
            let end =
                rest[at..].iter().position(|&c| c == BACKSLASH).map_or(rest.len(), |p| at + p);
            let part = &rest[at..end];
            let last = end == rest.len();
            if part.is_empty() {
                return Err(NtStatus::OBJECT_NAME_INVALID);
            }
            if last && want == Want::Parent {
                return Ok(Found::Parent(dir, owned(part)?));
            }
            let Some(next) = find(&dir, part, ci) else {
                return Err(if last {
                    NtStatus::OBJECT_NAME_NOT_FOUND
                } else {
                    NtStatus::OBJECT_PATH_NOT_FOUND
                });
            };
            if let Some(target) = next.as_symbolic_link() {
                if last && want == (Want::Object { follow_final: false }) {
                    return Ok(Found::Object(next));
                }
                // Continue at the target, with what is left of the path.
                let mut new = owned(target)?;
                new.try_extend_from_slice(&rest[end..]).map_err(|_| NtStatus::NO_MEMORY)?;
                break new;
            }
            if last {
                return Ok(match want {
                    Want::Parse => Found::Parsed(next, Vec::new()),
                    _ => Found::Object(next),
                });
            }
            if next.as_directory().is_some() {
                dir = next;
                at = end + 1;
                continue;
            }
            return match want {
                // Hand the remainder (with its leading `\`) to the object.
                Want::Parse => Ok(Found::Parsed(next, owned(&rest[end..])?)),
                _ => Err(NtStatus::OBJECT_PATH_NOT_FOUND),
            };
        };
        path = reparse;
        start = None;
    }
    // Too many links (a loop).
    Err(NtStatus::OBJECT_PATH_NOT_FOUND)
}

fn with_root<R>(f: impl FnOnce(&ObjectRef) -> Result<R, NtStatus>) -> Result<R, NtStatus> {
    let guard = NAMESPACE.lock();
    let root = guard.as_ref().ok_or(NtStatus::OBJECT_PATH_NOT_FOUND)?;
    f(root)
}

/// Finds the object named `path` (relative to `start`, if given),
/// following symbolic links.
pub fn lookup(start: Option<&ObjectRef>, path: &[u16], ci: bool) -> Result<ObjectRef, NtStatus> {
    lookup_with(start, path, ci, true)
}

/// Like `lookup`, but a symbolic link named by the full path is returned
/// itself (NtOpenSymbolicLinkObject).
pub fn lookup_link(
    start: Option<&ObjectRef>,
    path: &[u16],
    ci: bool,
) -> Result<ObjectRef, NtStatus> {
    lookup_with(start, path, ci, false)
}

fn lookup_with(
    start: Option<&ObjectRef>,
    path: &[u16],
    ci: bool,
    follow_final: bool,
) -> Result<ObjectRef, NtStatus> {
    with_root(|root| match walk(root, start, path, ci, Want::Object { follow_final })? {
        Found::Object(o) => Ok(o),
        _ => Err(NtStatus::OBJECT_NAME_NOT_FOUND),
    })
}

/// Resolves `path` until it reaches an object that is not a directory
/// (e.g. a device), returning it and the rest of the path (starting with
/// `\`, or empty). This is how `\??\C:\dir\file` reaches the file system
/// (through the `\??\C:` link to the volume).
pub fn lookup_parse(
    start: Option<&ObjectRef>,
    path: &[u16],
    ci: bool,
) -> Result<(ObjectRef, Vec<u16>), NtStatus> {
    with_root(|root| match walk(root, start, path, ci, Want::Parse)? {
        Found::Parsed(o, rest) => Ok((o, rest)),
        _ => Err(NtStatus::OBJECT_NAME_NOT_FOUND),
    })
}

/// A permanent symbolic link object pointing at `target`.
pub fn new_symbolic_link(target: &[u16]) -> Result<ObjectRef, NtStatus> {
    let link = SymbolicLink { target: owned(target)? };
    KObject::with_permanence(ObjectBody::SymbolicLink(link), true).map_err(|_| NtStatus::NO_MEMORY)
}

/// The entries of `dir` from index `from`: (name, type name) pairs.
pub fn directory_entries(
    dir: &ObjectRef,
    from: usize,
) -> Result<Vec<(Vec<u16>, &'static str)>, NtStatus> {
    let _guard = NAMESPACE.lock();
    let d = dir.as_directory().ok_or(NtStatus::OBJECT_TYPE_MISMATCH)?;
    let entries = d.entries.lock();
    let mut out = Vec::new();
    for e in entries.iter().skip(from) {
        out.try_push((owned(&e.name)?, e.object.type_name())).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// One OBJECT_DIRECTORY_INFORMATION entry: two UNICODE_STRINGs.
const DIR_INFO_SIZE: usize = 32;

/// Encodes `entries` for NtQueryDirectoryObject into at most `length`
/// bytes for a buffer at user address `base`: an array of
/// OBJECT_DIRECTORY_INFORMATION ending with a zeroed entry, then the
/// strings (each NUL-terminated). Takes one entry if `single`. Returns the
/// bytes and how many entries they hold, or None if not even one fits.
pub fn encode_directory(
    entries: &[(Vec<u16>, &str)],
    single: bool,
    length: usize,
    base: u64,
) -> Result<Option<(Vec<u8>, usize)>, NtStatus> {
    let size = |n: usize| -> usize {
        let strings: usize =
            entries[..n].iter().map(|(name, kind)| (name.len() + 1 + kind.len() + 1) * 2).sum();
        (n + 1) * DIR_INFO_SIZE + strings
    };
    let max = if single { entries.len().min(1) } else { entries.len() };
    let mut count = 0;
    while count < max && size(count + 1) <= length {
        count += 1;
    }
    if count == 0 && !entries.is_empty() {
        return Ok(None);
    }
    let total = size(count);
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| NtStatus::NO_MEMORY)?;
    out.resize(total, 0u8);
    let mut text = (count + 1) * DIR_INFO_SIZE;
    for (i, (name, kind)) in entries[..count].iter().enumerate() {
        let mut put_string = |field: usize, units: &mut dyn Iterator<Item = u16>| {
            let start = text;
            for u in units {
                out[text..text + 2].copy_from_slice(&u.to_le_bytes());
                text += 2;
            }
            let bytes = (text - start) as u16;
            text += 2; // NUL
            out[field..field + 2].copy_from_slice(&bytes.to_le_bytes());
            out[field + 2..field + 4].copy_from_slice(&(bytes + 2).to_le_bytes());
            out[field + 8..field + 16].copy_from_slice(&(base + start as u64).to_le_bytes());
        };
        put_string(i * DIR_INFO_SIZE, &mut name.iter().copied());
        put_string(i * DIR_INFO_SIZE + 16, &mut kind.bytes().map(u16::from));
    }
    Ok(Some((out, count)))
}

/// Links `object` under `path`. If the name exists and `open_if` is set,
/// returns the existing object instead (`true` = existed); the caller checks
/// its type. Without `open_if` that is STATUS_OBJECT_NAME_COLLISION.
pub fn create(
    start: Option<&ObjectRef>,
    path: &[u16],
    object: ObjectRef,
    ci: bool,
    open_if: bool,
) -> Result<(ObjectRef, bool), NtStatus> {
    let guard = NAMESPACE.lock();
    let root = guard.as_ref().ok_or(NtStatus::OBJECT_PATH_NOT_FOUND)?;
    let Found::Parent(dir, last) = walk(root, start, path, ci, Want::Parent)? else {
        return Err(NtStatus::OBJECT_NAME_INVALID);
    };
    let last = last.as_slice();
    if last.is_empty() {
        return Err(NtStatus::OBJECT_NAME_INVALID);
    }
    if let Some(existing) = find(&dir, last, ci) {
        return if open_if { Ok((existing, true)) } else { Err(NtStatus::OBJECT_NAME_COLLISION) };
    }
    let mut name = Vec::new();
    name.try_extend_from_slice(last).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut entry_name = Vec::new();
    entry_name.try_extend_from_slice(last).map_err(|_| NtStatus::NO_MEMORY)?;
    let d = dir.as_directory().expect("resolved parent is a directory");
    d.entries
        .lock()
        .try_push(DirEntry { name: entry_name, object: object.clone() })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    *object.name.lock() = Some(ObjectName { parent: dir.clone(), name });
    Ok((object, false))
}

/// Removes a named object from its directory (last handle closed).
pub fn unlink(object: &ObjectRef) {
    let _guard = NAMESPACE.lock();
    let Some(name) = object.name.lock().take() else { return };
    if let Some(d) = name.parent.as_directory() {
        d.entries.lock().retain(|e| !crate::mm::heap::KArc::ptr_eq(&e.object, object));
    }
}

/// The full path of a named object (`\A\B`), or empty if unnamed.
pub fn full_name(object: &ObjectRef) -> Result<Vec<u16>, AllocError> {
    let guard = NAMESPACE.lock();
    let mut parts: Vec<Vec<u16>> = Vec::new();
    let mut current = object.clone();
    loop {
        let next = {
            let name = current.name.lock();
            match name.as_ref() {
                Some(n) => {
                    let mut part = Vec::new();
                    part.try_extend_from_slice(&n.name)?;
                    parts.try_push(part)?;
                    n.parent.clone()
                }
                None => break,
            }
        };
        current = next;
    }
    let mut out = Vec::new();
    for part in parts.iter().rev() {
        out.try_push(BACKSLASH)?;
        out.try_extend_from_slice(part)?;
    }
    // The root directory is named `\`.
    if out.is_empty()
        && guard.as_ref().is_some_and(|root| crate::mm::heap::KArc::ptr_eq(root, object))
    {
        out.try_push(BACKSLASH)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::heap::KArc;
    use crate::nt::object::{HandleTable, KObject, ObjectBody};

    fn setup() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| init().unwrap());
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn console() -> ObjectRef {
        KObject::new(ObjectBody::ConsoleOutput).unwrap()
    }

    #[test]
    fn path_rules() {
        setup();
        assert!(lookup(None, &w("\\BaseNamedObjects"), false).unwrap().as_directory().is_some());
        assert!(lookup(None, &w("\\"), false).unwrap().as_directory().is_some());
        assert_eq!(
            lookup(None, &w("BaseNamedObjects"), false).err(),
            Some(NtStatus::OBJECT_PATH_SYNTAX_BAD)
        );
        assert_eq!(
            lookup(None, &w("\\basenamedobjects"), false).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        assert!(lookup(None, &w("\\basenamedobjects"), true).is_ok());
        assert_eq!(
            lookup(None, &w("\\Nope\\x"), true).err(),
            Some(NtStatus::OBJECT_PATH_NOT_FOUND)
        );
        assert_eq!(
            lookup(None, &w("\\BaseNamedObjects\\\\x"), true).err(),
            Some(NtStatus::OBJECT_NAME_INVALID)
        );
        let dir = lookup(None, &w("\\Sessions"), false).unwrap();
        assert!(lookup(Some(&dir), &w("1\\BaseNamedObjects"), false).is_ok());
        assert_eq!(
            lookup(Some(&dir), &w("\\1"), false).err(),
            Some(NtStatus::OBJECT_PATH_SYNTAX_BAD)
        );
    }

    #[test]
    fn symbolic_links_are_followed() {
        setup();
        let target = w("\\BaseNamedObjects");
        let link = new_symbolic_link(&target).unwrap();
        create(None, &w("\\KernelObjects\\host_link"), link.clone(), false, false).unwrap();
        let obj = console();
        create(None, &w("\\BaseNamedObjects\\host_link_obj"), obj.clone(), false, false).unwrap();
        // In the middle of a path, and at its end.
        let via = lookup(None, &w("\\KernelObjects\\host_link\\host_link_obj"), false).unwrap();
        assert!(KArc::ptr_eq(&via, &obj));
        let dir = lookup(None, &w("\\KernelObjects\\host_link"), false).unwrap();
        assert!(dir.as_directory().is_some());
        let itself = lookup_link(None, &w("\\KernelObjects\\host_link"), false).unwrap();
        assert!(KArc::ptr_eq(&itself, &link));
        // A link to a link, and the session's Global link.
        let link2 = new_symbolic_link(&w("\\KernelObjects\\host_link")).unwrap();
        create(None, &w("\\KernelObjects\\host_link2"), link2, false, false).unwrap();
        let via2 = lookup(None, &w("\\KernelObjects\\host_link2\\host_link_obj"), false).unwrap();
        assert!(KArc::ptr_eq(&via2, &obj));
        let global = w("\\Sessions\\1\\BaseNamedObjects\\Global\\host_link_obj");
        assert!(KArc::ptr_eq(&lookup(None, &global, false).unwrap(), &obj));
        // A loop is cut off.
        let a = new_symbolic_link(&w("\\KernelObjects\\host_loop")).unwrap();
        create(None, &w("\\KernelObjects\\host_loop"), a, false, false).unwrap();
        assert_eq!(
            lookup(None, &w("\\KernelObjects\\host_loop\\x"), false).err(),
            Some(NtStatus::OBJECT_PATH_NOT_FOUND)
        );
        // Parse goes through links to the first non-directory object.
        let dev = console();
        create(None, &w("\\Device\\host_dev"), dev.clone(), false, false).unwrap();
        let dl = new_symbolic_link(&w("\\Device\\host_dev")).unwrap();
        create(None, &w("\\??\\HOSTDEV:"), dl, false, false).unwrap();
        let (found, rest) = lookup_parse(None, &w("\\??\\HOSTDEV:\\a\\b"), true).unwrap();
        assert!(KArc::ptr_eq(&found, &dev));
        assert_eq!(rest, w("\\a\\b"));
    }

    #[test]
    fn directory_information_layout() {
        let entries = [(w("ev"), "Event"), (w("link"), "SymbolicLink")];
        let (b, n) = encode_directory(&entries, false, 4096, 0x1000).unwrap().unwrap();
        assert_eq!(n, 2);
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let ptr_at = |o: usize| u64::from_le_bytes(b[o + 8..o + 16].try_into().unwrap()) - 0x1000;
        assert_eq!(u16_at(0), 4); // "ev"
        assert_eq!(u16_at(16), 10); // "Event"
        assert_eq!(u16_at(32), 8); // "link"
        assert!(b[64..96].iter().all(|&x| x == 0), "terminating entry");
        let name = ptr_at(0) as usize;
        assert_eq!(&b[name..name + 6], &[b'e', 0, b'v', 0, 0, 0]);
        let (_, one) = encode_directory(&entries, true, 4096, 0).unwrap().unwrap();
        assert_eq!(one, 1);
        assert!(encode_directory(&entries, true, 40, 0).unwrap().is_none());
    }

    #[test]
    fn create_collide_open_if_and_unlink() {
        setup();
        let path = w("\\BaseNamedObjects\\host_test_obj");
        let obj = console();
        let (created, existed) = create(None, &path, obj.clone(), false, false).unwrap();
        assert!(!existed && KArc::ptr_eq(&created, &obj));
        assert_eq!(
            create(None, &path, console(), false, false).err(),
            Some(NtStatus::OBJECT_NAME_COLLISION)
        );
        let (found, existed) = create(None, &path, console(), false, true).unwrap();
        assert!(existed && KArc::ptr_eq(&found, &obj));
        assert_eq!(full_name(&obj).unwrap(), path);
        // Last handle closed -> unlinked.
        let mut table = HandleTable::default();
        let h = table.insert_object(obj.clone()).unwrap();
        table.close(h);
        assert_eq!(lookup(None, &path, false).err(), Some(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert!(full_name(&obj).unwrap().is_empty());
    }
}

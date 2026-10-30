//! The object manager namespace: a tree of directory objects rooted at `\`.
//!
//! Lookup rules follow the documented behaviour of the NT object manager:
//! absolute paths start with `\`; paths relative to a RootDirectory handle
//! must not; a missing intermediate component is
//! STATUS_OBJECT_PATH_NOT_FOUND, a missing final one
//! STATUS_OBJECT_NAME_NOT_FOUND.

use alloc::vec::Vec;

use super::object::{DirEntry, Directory, KObject, ObjectBody, ObjectName, ObjectRef};
use super::status::NtStatus;
use crate::mm::heap::{AllocError, TryVec};
use crate::sync::IrqMutex;

const BACKSLASH: u16 = b'\\' as u16;

/// Serializes structural changes (link/unlink) and lookups.
static NAMESPACE: IrqMutex<Option<ObjectRef>> = IrqMutex::new(None);

/// Directories that exist at boot (Windows has these and more).
const SYSTEM_DIRECTORIES: &[&str] = &[
    "BaseNamedObjects",
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
    Ok(())
}

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

/// Splits `path` into its directory and final component, resolving every
/// intermediate directory. Must be called with the namespace lock held.
fn resolve_parent<'p>(
    root: &ObjectRef,
    start: Option<&ObjectRef>,
    path: &'p [u16],
    ci: bool,
) -> Result<(ObjectRef, &'p [u16]), NtStatus> {
    let (mut dir, rest) = match start {
        None => match path.split_first() {
            Some((&BACKSLASH, rest)) => (root.clone(), rest),
            _ => return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD),
        },
        Some(s) => {
            if path.first() == Some(&BACKSLASH) {
                return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
            }
            if s.as_directory().is_none() {
                return Err(NtStatus::OBJECT_TYPE_MISMATCH);
            }
            (s.clone(), path)
        }
    };
    if rest.is_empty() {
        return Ok((dir, rest));
    }
    let mut parts = rest.split(|&c| c == BACKSLASH).peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() {
            return Err(NtStatus::OBJECT_NAME_INVALID);
        }
        if parts.peek().is_none() {
            return Ok((dir, part));
        }
        dir = match find(&dir, part, ci) {
            Some(next) if next.as_directory().is_some() => next,
            _ => return Err(NtStatus::OBJECT_PATH_NOT_FOUND),
        };
    }
    unreachable!("split yields at least one part")
}

/// Finds the object named `path` (relative to `start`, if given).
pub fn lookup(start: Option<&ObjectRef>, path: &[u16], ci: bool) -> Result<ObjectRef, NtStatus> {
    let guard = NAMESPACE.lock();
    let root = guard.as_ref().ok_or(NtStatus::OBJECT_PATH_NOT_FOUND)?;
    let (dir, last) = resolve_parent(root, start, path, ci)?;
    if last.is_empty() {
        return Ok(dir);
    }
    find(&dir, last, ci).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)
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
    let (dir, last) = resolve_parent(root, start, path, ci)?;
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

//! Known folders: the definitions of the folders Windows names by
//! KNOWNFOLDERID, where each one is (SHGetKnownFolderPath,
//! SHGetFolderPathW), and the Known Folder Manager over them
//! (CLSID_KnownFolderManager: IKnownFolderManager and IKnownFolder),
//! which App-V's package configuration asks while the Click-to-Run
//! service installs Office.
//!
//! The definitions (`FOLDERS`) are Windows' (name, category, parent,
//! relative path, parsing name, attributes, KFDF_ flags, CSIDL) as Wine's
//! conformance test lists them (dlls/shell32/tests/shellpath.c), the
//! Windows 8 and later alternatives where it gives two, and build
//! 26100's where it differs (measured: the `shell:::{...}` parsing names
//! of Documents, Downloads, Music, Pictures and Videos, no parsing name
//! for Public, no Sidebar gadget folders); the virtual
//! folders (no path), the libraries and the sample folders are not
//! defined. Windows keeps them under `FolderDescriptions` in the
//! registry; gala's are compiled in, so RegisterFolder,
//! UnregisterFolder, SetPath and Redirect (redirection) are recorded as
//! unsupported, and so are GetShellItem, GetIDList, GetFolderType and
//! FindFolderFromIDList.
//!
//! A folder's path is its root's (an environment variable, the Windows
//! or system directory) or its parent's path, a backslash and its
//! relative path, so a folder is always below its parent. The path is
//! given whether or not the folder exists (gala's profile does not hold
//! every folder Windows makes with a profile); KF_FLAG_CREATE creates it.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_FAIL, E_FILE_NOT_FOUND, E_INVALIDARG, E_OUTOFMEMORY, Folder, MAX_PATH, S_OK, create_folder,
    guid, resolve, unsupported,
};
use crate::link::{
    VTable, factory_add_ref, factory_lock, factory_query, factory_release, iid, put,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

type HResult = i32;
type Guid = [u8; 16];

const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110u32 as i32;

const OLE: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const IID_IUNKNOWN: Guid = guid(0, 0, 0, OLE);
pub const CLSID_KNOWN_FOLDER_MANAGER: Guid =
    guid(0x4df0_c730, 0xdf9d, 0x4ae3, [0x91, 0x53, 0xaa, 0x6b, 0x82, 0xe9, 0x79, 0x5a]);
const IID_IKNOWN_FOLDER_MANAGER: Guid =
    guid(0x8be2_d872, 0x86aa, 0x4d47, [0xb7, 0x76, 0x32, 0xcc, 0xa4, 0x0c, 0x70, 0x18]);
const IID_IKNOWN_FOLDER: Guid =
    guid(0x3aa7_af7e, 0x9b36, 0x420c, [0xa8, 0xe3, 0xf7, 0x7d, 0x46, 0x74, 0xa4, 0x88]);

const KF_FLAG_CREATE: u32 = 0x8000;
const KF_CATEGORY_FIXED: u32 = 2;
const FFFP_NEARESTPARENTMATCH: u32 = 1;
/// CSIDL_DESKTOPDIRECTORY: the desktop's folder, FOLDERID_Desktop (whose
/// own CSIDL is CSIDL_DESKTOP).
const CSIDL_DESKTOPDIRECTORY: i32 = 0x10;

/// A known folder's definition (KNOWNFOLDER_DEFINITION's fields gala
/// has), and where its path starts when it has no parent to start from.
pub struct Def {
    pub id: Guid,
    /// -1 when it has no CSIDL.
    csidl: i32,
    /// The canonical name (GetFolderByName's).
    name: &'static str,
    category: u32,
    parent: Option<Guid>,
    relative: Option<&'static str>,
    parsing: Option<&'static str>,
    attributes: u32,
    flags: u32,
    root: Option<Folder>,
}

/// The definition of folder `id`.
pub fn by_id(id: &Guid) -> Option<&'static Def> {
    FOLDERS.iter().find(|d| d.id == *id)
}

/// The definition of CSIDL `csidl` (its flags masked off).
pub fn by_csidl(csidl: i32) -> Option<&'static Def> {
    let csidl = match csidl & 0xff {
        CSIDL_DESKTOPDIRECTORY => 0,
        c => c,
    };
    FOLDERS.iter().find(|d| d.csidl == csidl)
}

/// Folder `def`'s path in `out` (NUL-terminated); its length.
pub fn path(def: &Def, out: &mut [u16; MAX_PATH]) -> Option<usize> {
    path_at(def, out, 0)
}

fn path_at(def: &Def, out: &mut [u16; MAX_PATH], depth: usize) -> Option<usize> {
    if let Some(root) = def.root {
        return resolve(root, out);
    }
    // Definitions nest a few levels; a cycle would be a bug in FOLDERS.
    let parent = by_id(&def.parent?).filter(|_| depth < 8)?;
    let n = path_at(parent, out, depth + 1)?;
    let relative = def.relative?;
    if n + 1 + relative.len() >= MAX_PATH {
        return None;
    }
    out[n] = u16::from(b'\\');
    for (i, c) in relative.encode_utf16().enumerate() {
        out[n + 1 + i] = c;
    }
    let n = n + 1 + relative.len();
    out[n] = 0;
    Some(n)
}

/// `n` units of `s` and a NUL in CoTaskMemAlloc'd memory (gala's
/// CoTaskMemAlloc is the process heap).
fn task_string(s: &[u16]) -> Option<*mut u16> {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, 2 * (s.len() + 1)) }.cast::<u16>();
    if p.is_null() {
        return None;
    }
    // SAFETY: a block of s.len() + 1 units.
    unsafe {
        core::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
        *p.add(s.len()) = 0;
    }
    Some(p)
}

fn task_str(s: Option<&str>) -> Result<*mut u16, ()> {
    let Some(s) = s else { return Ok(core::ptr::null_mut()) };
    let mut buf = [0u16; MAX_PATH];
    let mut n = 0;
    for c in s.encode_utf16().take(MAX_PATH) {
        buf[n] = c;
        n += 1;
    }
    task_string(&buf[..n]).ok_or(())
}

fn task_free(p: *mut u16) {
    if !p.is_null() {
        // SAFETY: a block of task_string's.
        unsafe { HeapFree(GetProcessHeap(), 0, p.cast()) };
    }
}

/// Folder `def`'s path in CoTaskMemAlloc'd memory, created first with
/// KF_FLAG_CREATE: SHGetKnownFolderPath and IKnownFolder::GetPath.
pub fn task_path(def: &Def, flags: u32, out: *mut *mut u16) -> HResult {
    put(out, core::ptr::null_mut());
    let mut buf = [0u16; MAX_PATH];
    let Some(n) = path(def, &mut buf) else { return E_FILE_NOT_FOUND };
    if flags & KF_FLAG_CREATE != 0 && !create_folder(&mut buf, n) {
        return E_FAIL;
    }
    let Some(p) = task_string(&buf[..n]) else { return E_OUTOFMEMORY };
    put(out, p);
    S_OK
}

fn d1(id: &Guid) -> u64 {
    u32::from_le_bytes([id[0], id[1], id[2], id[3]]).into()
}

/// Records an unknown folder id asked for (its first 32 bits).
fn unknown_id(what: &[u8], id: &Guid) {
    unsupported(what, d1(id));
}

// ---- IKnownFolder ----

/// A known folder object: its vtable, references and definition.
#[repr(C)]
struct KnownFolder {
    vtable: *const VTable<12>,
    refs: AtomicU32,
    def: &'static Def,
}

fn folder<'a>(this: *mut c_void) -> &'a KnownFolder {
    // SAFETY: COM calls our methods with the pointer we gave out, a live
    // KnownFolder.
    unsafe { &*this.cast::<KnownFolder>() }
}

/// A new folder object for `def` with one reference, in `out`.
fn new_folder(def: &'static Def, out: *mut *mut c_void) -> HResult {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<KnownFolder>()) }
        .cast::<KnownFolder>();
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block of the object's size (the process heap aligns
    // to 16).
    unsafe { p.write(KnownFolder { vtable: &FOLDER_VTABLE, refs: AtomicU32::new(1), def }) };
    put(out, p.cast());
    S_OK
}

extern "system" fn kf_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid(riid) {
        IID_IUNKNOWN | IID_IKNOWN_FOLDER => {
            folder(this).refs.fetch_add(1, Ordering::Relaxed);
            put(out, this);
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn kf_add_ref(this: *mut c_void) -> u32 {
    folder(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn kf_release(this: *mut c_void) -> u32 {
    let left = folder(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the object's own block, no longer referenced.
        unsafe { HeapFree(GetProcessHeap(), 0, this.cast()) };
    }
    left
}

extern "system" fn kf_get_id(this: *mut c_void, id: *mut Guid) -> HResult {
    if id.is_null() {
        return E_INVALIDARG;
    }
    put(id, folder(this).def.id);
    S_OK
}

extern "system" fn kf_get_category(this: *mut c_void, category: *mut u32) -> HResult {
    if category.is_null() {
        return E_INVALIDARG;
    }
    put(category, folder(this).def.category);
    S_OK
}

extern "system" fn kf_get_shell_item(
    this: *mut c_void,
    _flags: u32,
    _riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    unsupported(b"shell32.dll!IKnownFolder::GetShellItem", d1(&folder(this).def.id));
    E_NOTIMPL
}

extern "system" fn kf_get_path(this: *mut c_void, flags: u32, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    task_path(folder(this).def, flags, out)
}

/// SetPath: redirection, which gala does not do; a fixed folder's path
/// cannot be set (E_INVALIDARG, as Windows answers).
extern "system" fn kf_set_path(this: *mut c_void, _flags: u32, _path: *const u16) -> HResult {
    let def = folder(this).def;
    if def.category == KF_CATEGORY_FIXED {
        return E_INVALIDARG;
    }
    unsupported(b"shell32.dll!IKnownFolder::SetPath", d1(&def.id));
    E_NOTIMPL
}

extern "system" fn kf_get_id_list(
    this: *mut c_void,
    _flags: u32,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    unsupported(b"shell32.dll!IKnownFolder::GetIDList", d1(&folder(this).def.id));
    E_NOTIMPL
}

extern "system" fn kf_get_folder_type(this: *mut c_void, _kind: *mut Guid) -> HResult {
    unsupported(b"shell32.dll!IKnownFolder::GetFolderType", d1(&folder(this).def.id));
    E_NOTIMPL
}

/// No redirection capabilities: gala does not redirect folders.
extern "system" fn kf_get_redirection_capabilities(_this: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, 0);
    S_OK
}

/// KNOWNFOLDER_DEFINITION (shobjidl.h); the caller frees its strings
/// (FreeKnownFolderDefinitionFields: CoTaskMemFree).
#[repr(C)]
struct Definition {
    category: u32,
    name: *mut u16,
    description: *mut u16,
    parent: Guid,
    relative: *mut u16,
    parsing: *mut u16,
    tooltip: *mut u16,
    localized: *mut u16,
    icon: *mut u16,
    security: *mut u16,
    attributes: u32,
    flags: u32,
    folder_type: Guid,
}

extern "system" fn kf_get_folder_definition(this: *mut c_void, out: *mut Definition) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let def = folder(this).def;
    let null = core::ptr::null_mut();
    let mut d = Definition {
        category: def.category,
        name: null,
        description: null,
        parent: def.parent.unwrap_or([0; 16]),
        relative: null,
        parsing: null,
        tooltip: null,
        localized: null,
        icon: null,
        security: null,
        attributes: def.attributes,
        flags: def.flags,
        folder_type: [0; 16],
    };
    let strings = (|| {
        d.name = task_str(Some(def.name))?;
        d.relative = task_str(def.relative)?;
        d.parsing = task_str(def.parsing)?;
        Ok::<(), ()>(())
    })();
    if strings.is_err() {
        for p in [d.name, d.relative, d.parsing] {
            task_free(p);
        }
        // SAFETY: the caller's structure, zeroed as on success.
        unsafe { out.write_bytes(0, 1) };
        return E_OUTOFMEMORY;
    }
    put(out, d);
    S_OK
}

static FOLDER_VTABLE: VTable<12> = VTable([
    kf_query as *const (),
    kf_add_ref as *const (),
    kf_release as *const (),
    kf_get_id as *const (),
    kf_get_category as *const (),
    kf_get_shell_item as *const (),
    kf_get_path as *const (),
    kf_set_path as *const (),
    kf_get_id_list as *const (),
    kf_get_folder_type as *const (),
    kf_get_redirection_capabilities as *const (),
    kf_get_folder_definition as *const (),
]);

// ---- IKnownFolderManager ----

/// The manager: its vtable and references (it has no state of its own).
#[repr(C)]
struct Manager {
    vtable: *const VTable<13>,
    refs: AtomicU32,
}

fn manager<'a>(this: *mut c_void) -> &'a Manager {
    // SAFETY: COM calls our methods with the pointer we gave out, a live
    // Manager.
    unsafe { &*this.cast::<Manager>() }
}

extern "system" fn m_query(this: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid(riid) {
        IID_IUNKNOWN | IID_IKNOWN_FOLDER_MANAGER => {
            manager(this).refs.fetch_add(1, Ordering::Relaxed);
            put(out, this);
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn m_add_ref(this: *mut c_void) -> u32 {
    manager(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn m_release(this: *mut c_void) -> u32 {
    let left = manager(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the object's own block, no longer referenced.
        unsafe { HeapFree(GetProcessHeap(), 0, this.cast()) };
    }
    left
}

extern "system" fn m_folder_id_from_csidl(
    _this: *mut c_void,
    csidl: i32,
    id: *mut Guid,
) -> HResult {
    if id.is_null() {
        return E_INVALIDARG;
    }
    match by_csidl(csidl) {
        Some(def) => {
            put(id, def.id);
            S_OK
        }
        None => {
            unsupported(b"shell32.dll!IKnownFolderManager::FolderIdFromCsidl", csidl as u32 as u64);
            E_INVALIDARG
        }
    }
}

extern "system" fn m_folder_id_to_csidl(
    _this: *mut c_void,
    id: *const Guid,
    csidl: *mut i32,
) -> HResult {
    if id.is_null() || csidl.is_null() {
        return E_INVALIDARG;
    }
    let id = iid(id);
    match by_id(&id) {
        Some(def) if def.csidl >= 0 => {
            put(csidl, def.csidl);
            S_OK
        }
        Some(_) => E_INVALIDARG,
        None => {
            unknown_id(b"shell32.dll!IKnownFolderManager::FolderIdToCsidl(unknown folder)", &id);
            E_INVALIDARG
        }
    }
}

extern "system" fn m_get_folder_ids(
    _this: *mut c_void,
    ids: *mut *mut Guid,
    count: *mut u32,
) -> HResult {
    if ids.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, FOLDERS.len() * 16) }.cast::<Guid>();
    if p.is_null() {
        put(ids, core::ptr::null_mut());
        put(count, 0);
        return E_OUTOFMEMORY;
    }
    for (i, def) in FOLDERS.iter().enumerate() {
        // SAFETY: inside the block of FOLDERS.len() ids.
        unsafe { p.add(i).write_unaligned(def.id) };
    }
    put(ids, p);
    put(count, FOLDERS.len() as u32);
    S_OK
}

extern "system" fn m_get_folder(
    _this: *mut c_void,
    id: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let id = iid(id);
    match by_id(&id) {
        Some(def) => new_folder(def, out),
        None => {
            unknown_id(b"shell32.dll!IKnownFolderManager::GetFolder(unknown folder)", &id);
            E_INVALIDARG
        }
    }
}

/// Whether the NUL-terminated `s` is `name`, ASCII case aside.
fn named(s: *const u16, name: &str) -> bool {
    let mut i = 0;
    for b in name.bytes() {
        // SAFETY: read up to the string's NUL (a mismatch stops first).
        let c = unsafe { *s.add(i) };
        if c > 0x7f || !(c as u8).eq_ignore_ascii_case(&b) {
            return false;
        }
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *s.add(i) == 0 }
}

extern "system" fn m_get_folder_by_name(
    _this: *mut c_void,
    name: *const u16,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if name.is_null() {
        return E_INVALIDARG;
    }
    match FOLDERS.iter().find(|d| named(name, d.name)) {
        Some(def) => new_folder(def, out),
        None => {
            // The name, as far as it is ASCII, in the record.
            let mut what = [0u8; 120];
            let head = b"shell32.dll!IKnownFolderManager::GetFolderByName(";
            what[..head.len()].copy_from_slice(head);
            let mut n = head.len();
            let mut i = 0;
            loop {
                // SAFETY: read up to the name's NUL.
                let c = unsafe { *name.add(i) };
                if c == 0 || n + 2 > what.len() {
                    break;
                }
                what[n] = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
                n += 1;
                i += 1;
            }
            what[n] = b')';
            unsupported(&what[..n + 1], 0);
            E_FILE_NOT_FOUND
        }
    }
}

extern "system" fn m_register_folder(
    _this: *mut c_void,
    id: *const Guid,
    _def: *const c_void,
) -> HResult {
    unsupported(b"shell32.dll!IKnownFolderManager::RegisterFolder", d1(&iid(id)));
    E_NOTIMPL
}

extern "system" fn m_unregister_folder(_this: *mut c_void, id: *const Guid) -> HResult {
    unsupported(b"shell32.dll!IKnownFolderManager::UnregisterFolder", d1(&iid(id)));
    E_NOTIMPL
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// FindFolderFromPath: the folder whose path is `path` (case aside, a
/// trailing backslash aside), or with FFFP_NEARESTPARENTMATCH the one
/// whose path is the longest above it; the first defined of folders
/// with the same path.
extern "system" fn m_find_folder_from_path(
    _this: *mut c_void,
    path: *const u16,
    mode: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if path.is_null() {
        return E_INVALIDARG;
    }
    let mut n = 0;
    // SAFETY: a NUL-terminated string, read up to its NUL.
    while unsafe { *path.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: n units.
    let path = unsafe { core::slice::from_raw_parts(path, n) };
    let path = path.strip_suffix(&[u16::from(b'\\')]).unwrap_or(path);
    let mut best: Option<(&'static Def, usize)> = None;
    let mut buf = [0u16; MAX_PATH];
    for def in FOLDERS {
        let Some(len) = self::path(def, &mut buf) else { continue };
        let folder = &buf[..len];
        let exact = path.len() == len;
        let below =
            mode == FFFP_NEARESTPARENTMATCH && path.len() > len && path[len] == u16::from(b'\\');
        if !(exact || below) || !folder.iter().zip(path).all(|(&a, &b)| upper(a) == upper(b)) {
            continue;
        }
        if best.is_none_or(|(_, l)| len > l) {
            best = Some((def, len));
        }
    }
    match best {
        Some((def, _)) => new_folder(def, out),
        None => E_FILE_NOT_FOUND,
    }
}

extern "system" fn m_find_folder_from_id_list(
    _this: *mut c_void,
    _list: *const c_void,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    unsupported(b"shell32.dll!IKnownFolderManager::FindFolderFromIDList", 0);
    E_NOTIMPL
}

extern "system" fn m_redirect(
    _this: *mut c_void,
    id: *const Guid,
    _window: usize,
    _flags: u32,
    _target: *const u16,
    _count: u32,
    _exclusions: *const Guid,
    error: *mut *mut u16,
) -> HResult {
    if !error.is_null() {
        put(error, core::ptr::null_mut());
    }
    unsupported(b"shell32.dll!IKnownFolderManager::Redirect", d1(&iid(id)));
    E_NOTIMPL
}

static MANAGER_VTABLE: VTable<13> = VTable([
    m_query as *const (),
    m_add_ref as *const (),
    m_release as *const (),
    m_folder_id_from_csidl as *const (),
    m_folder_id_to_csidl as *const (),
    m_get_folder_ids as *const (),
    m_get_folder as *const (),
    m_get_folder_by_name as *const (),
    m_register_folder as *const (),
    m_unregister_folder as *const (),
    m_find_folder_from_path as *const (),
    m_find_folder_from_id_list as *const (),
    m_redirect as *const (),
]);

// ---- The class object ----

extern "system" fn factory_create(
    _this: *mut c_void,
    outer: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<Manager>()) }
        .cast::<Manager>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block of the object's size.
    unsafe { p.write(Manager { vtable: &MANAGER_VTABLE, refs: AtomicU32::new(1) }) };
    let r = m_query(p.cast(), riid, out);
    m_release(p.cast());
    r
}

static FACTORY_VTABLE: VTable<5> = VTable([
    factory_query as *const (),
    factory_add_ref as *const (),
    factory_release as *const (),
    factory_create as *const (),
    factory_lock as *const (),
]);

#[repr(C)]
pub struct Factory {
    vtable: *const VTable<5>,
}
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Factory {}
/// CLSID_KnownFolderManager's class object (DllGetClassObject's).
pub static FACTORY: Factory = Factory { vtable: &FACTORY_VTABLE };

// ---- The definitions ----

/// Windows' known folders that have a path (Wine's conformance table,
/// see the module's documentation). Roots: the folders the environment
/// names, the Windows and system directories.
static FOLDERS: &[Def] = &[
    Def {
        // FOLDERID_AdminTools
        id: guid(0x724ef170, 0xa42d, 0x4fef, [0x9f, 0x26, 0xb6, 0x0e, 0x84, 0x6f, 0xba, 0x4f]),
        csidl: 48,
        name: "Administrative Tools",
        category: 4,
        parent: Some(guid(
            0xa77f5d77,
            0x2e2b,
            0x44c3,
            [0xa6, 0xa2, 0xab, 0xa6, 0x01, 0x05, 0x4a, 0x51],
        )),
        relative: Some("Administrative Tools"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_CDBurning
        id: guid(0x9e52ab10, 0xf80d, 0x49df, [0xac, 0xb8, 0x43, 0x30, 0xf5, 0x68, 0x78, 0x55]),
        csidl: 59,
        name: "CD Burning",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\Burn\\Burn"),
        parsing: None,
        attributes: 0x1,
        flags: 0x2,
        root: None,
    },
    Def {
        // FOLDERID_CommonAdminTools
        id: guid(0xd0384e7d, 0xbac3, 0x4797, [0x8f, 0x14, 0xcb, 0xa2, 0x29, 0xb3, 0x92, 0xb5]),
        csidl: 47,
        name: "Common Administrative Tools",
        category: 3,
        parent: Some(guid(
            0x0139d44e,
            0x6afe,
            0x49f2,
            [0x86, 0x90, 0x3d, 0xaf, 0xca, 0xe6, 0xff, 0xb8],
        )),
        relative: Some("Administrative Tools"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_CommonOEMLinks
        id: guid(0xc1bae2d0, 0x10df, 0x4334, [0xbe, 0xdd, 0x7a, 0xa2, 0x0b, 0x22, 0x7a, 0x9d]),
        csidl: 58,
        name: "OEM Links",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("OEM Links"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_CommonPrograms
        id: guid(0x0139d44e, 0x6afe, 0x49f2, [0x86, 0x90, 0x3d, 0xaf, 0xca, 0xe6, 0xff, 0xb8]),
        csidl: 23,
        name: "Common Programs",
        category: 3,
        parent: Some(guid(
            0xa4115719,
            0xd62e,
            0x491d,
            [0xaa, 0x7c, 0xe7, 0x4b, 0x8b, 0xe3, 0xb0, 0x67],
        )),
        relative: Some("Programs"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_CommonStartMenu
        id: guid(0xa4115719, 0xd62e, 0x491d, [0xaa, 0x7c, 0xe7, 0x4b, 0x8b, 0xe3, 0xb0, 0x67]),
        csidl: 22,
        name: "Common Start Menu",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("Microsoft\\Windows\\Start Menu"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_CommonStartup
        id: guid(0x82a5ea35, 0xd9cd, 0x47c5, [0x96, 0x29, 0xe1, 0x5d, 0x2f, 0x71, 0x4e, 0x6e]),
        csidl: 24,
        name: "Common Startup",
        category: 3,
        parent: Some(guid(
            0x0139d44e,
            0x6afe,
            0x49f2,
            [0x86, 0x90, 0x3d, 0xaf, 0xca, 0xe6, 0xff, 0xb8],
        )),
        relative: Some("StartUp"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_CommonTemplates
        id: guid(0xb94237e7, 0x57ac, 0x4347, [0x91, 0x51, 0xb0, 0x8c, 0x6c, 0x32, 0xd1, 0xf7]),
        csidl: 45,
        name: "Common Templates",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("Microsoft\\Windows\\Templates"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Contacts
        id: guid(0x56784854, 0xc6cb, 0x462b, [0x81, 0x69, 0x88, 0xe3, 0x50, 0xac, 0xb8, 0x82]),
        csidl: -1,
        name: "Contacts",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Contacts"),
        parsing: Some(
            "::{59031a47-3f72-44a7-89c5-5595fe6b30ee}\\{56784854-C6CB-462B-8169-88E350ACB882}",
        ),
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_Cookies
        id: guid(0x2b0f765d, 0xc0e9, 0x4171, [0x90, 0x8e, 0x08, 0xa6, 0x11, 0xb8, 0x4f, 0xf6]),
        csidl: 33,
        name: "Cookies",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\INetCookies"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Desktop
        id: guid(0xb4bfcc3a, 0xdb2c, 0x424c, [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41]),
        csidl: 0,
        name: "Desktop",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Desktop"),
        parsing: None,
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_DeviceMetadataStore
        id: guid(0x5ce4a5e9, 0xe4eb, 0x479d, [0xb8, 0x9f, 0x13, 0x0c, 0x02, 0x88, 0x61, 0x55]),
        csidl: -1,
        name: "Device Metadata Store",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("Microsoft\\Windows\\DeviceMetadataStore"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Documents
        id: guid(0xfdd39ad0, 0x238f, 0x46af, [0xad, 0xb4, 0x6c, 0x85, 0x48, 0x03, 0x69, 0xc7]),
        csidl: 5,
        name: "Personal",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Documents"),
        parsing: Some("shell:::{A8CDFF1C-4878-43be-B5FD-F8091C1C60D0}"),
        attributes: 0x1,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_Downloads
        id: guid(0x374de290, 0x123f, 0x4565, [0x91, 0x64, 0x39, 0xc4, 0x92, 0x5e, 0x46, 0x7b]),
        csidl: -1,
        name: "Downloads",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Downloads"),
        parsing: Some("shell:::{374DE290-123F-4565-9164-39C4925E467B}"),
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_Favorites
        id: guid(0x1777f761, 0x68ad, 0x4d8a, [0x87, 0xbd, 0x30, 0xb7, 0x59, 0xfa, 0x33, 0xdd]),
        csidl: 6,
        name: "Favorites",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Favorites"),
        parsing: None,
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_Fonts
        id: guid(0xfd228cb7, 0xae11, 0x4ae3, [0x86, 0x4c, 0x16, 0xf3, 0x91, 0x0a, 0xb8, 0xfe]),
        csidl: 20,
        name: "Fonts",
        category: 2,
        parent: Some(guid(
            0xf38bf404,
            0x1d43,
            0x42f2,
            [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23],
        )),
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("windir", "\\Fonts")),
    },
    Def {
        // FOLDERID_GameTasks
        id: guid(0x054fae61, 0x4dd8, 0x4787, [0x80, 0xb6, 0x09, 0x02, 0x20, 0xc4, 0xb7, 0x00]),
        csidl: -1,
        name: "GameTasks",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\GameExplorer"),
        parsing: None,
        attributes: 0x0,
        flags: 0x2,
        root: None,
    },
    Def {
        // FOLDERID_History
        id: guid(0xd9dc8a3b, 0xb784, 0x432e, [0xa7, 0x81, 0x5a, 0x11, 0x30, 0xa7, 0x59, 0x63]),
        csidl: 34,
        name: "History",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\History"),
        parsing: None,
        attributes: 0x0,
        flags: 0x2,
        root: None,
    },
    Def {
        // FOLDERID_ImplicitAppShortcuts
        id: guid(0xbcb5256f, 0x79f6, 0x4cee, [0xb7, 0x25, 0xdc, 0x34, 0xe4, 0x02, 0xfd, 0x46]),
        csidl: -1,
        name: "ImplicitAppShortcuts",
        category: 4,
        parent: Some(guid(
            0x9e3995ab,
            0x1f9c,
            0x4f13,
            [0xb8, 0x27, 0x48, 0xb2, 0x4b, 0x6c, 0x71, 0x74],
        )),
        relative: Some("ImplicitAppShortcuts"),
        parsing: None,
        attributes: 0x0,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_InternetCache
        id: guid(0x352481e8, 0x33be, 0x4251, [0xba, 0x85, 0x60, 0x07, 0xca, 0xed, 0xcf, 0x9d]),
        csidl: 32,
        name: "Cache",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\INetCache"),
        parsing: None,
        attributes: 0x0,
        flags: 0x2,
        root: None,
    },
    Def {
        // FOLDERID_Libraries
        id: guid(0x1b3ea5dc, 0xb587, 0x4786, [0xb4, 0xef, 0xbd, 0x1d, 0xc3, 0x32, 0xae, 0xae]),
        csidl: -1,
        name: "Libraries",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Libraries"),
        parsing: None,
        attributes: 0x0,
        flags: 0x28,
        root: None,
    },
    Def {
        // FOLDERID_Links
        id: guid(0xbfb9d5e0, 0xc6a9, 0x404c, [0xb2, 0xb2, 0xae, 0x6d, 0xb6, 0xaf, 0x49, 0x68]),
        csidl: -1,
        name: "Links",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Links"),
        parsing: Some(
            "::{59031a47-3f72-44a7-89c5-5595fe6b30ee}\\{bfb9d5e0-c6a9-404c-b2b2-ae6db6af4968}",
        ),
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_LocalAppData
        id: guid(0xf1b32785, 0x6fba, 0x4fcf, [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91]),
        csidl: 28,
        name: "Local AppData",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("AppData\\Local"),
        parsing: None,
        attributes: 0x0,
        flags: 0x22,
        root: None,
    },
    Def {
        // FOLDERID_LocalAppDataLow
        id: guid(0xa520a1a4, 0x1780, 0x4ff6, [0xbd, 0x18, 0x16, 0x73, 0x43, 0xc5, 0xaf, 0x16]),
        csidl: -1,
        name: "LocalAppDataLow",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("AppData\\LocalLow"),
        parsing: None,
        attributes: 0x2000,
        flags: 0x2a,
        root: None,
    },
    Def {
        // FOLDERID_Music
        id: guid(0x4bd8d571, 0x6d19, 0x48d3, [0xbe, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0e, 0x43]),
        csidl: 13,
        name: "My Music",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Music"),
        parsing: Some("shell:::{1CF1260C-4DD0-4EBB-811F-33C572699FDE}"),
        attributes: 0x1,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_NetHood
        id: guid(0xc5abbf53, 0xe17f, 0x4121, [0x89, 0x00, 0x86, 0x62, 0x6f, 0xc2, 0xc9, 0x73]),
        csidl: 19,
        name: "NetHood",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Network Shortcuts"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_OriginalImages
        id: guid(0x2c36c0aa, 0x5812, 0x4b87, [0xbf, 0xd0, 0x4c, 0xd0, 0xdf, 0xb1, 0x9b, 0x39]),
        csidl: -1,
        name: "Original Images",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows Photo Gallery\\Original Images"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_PhotoAlbums
        id: guid(0x69d2cf90, 0xfc33, 0x4fb7, [0x9a, 0x0c, 0xeb, 0xb0, 0xf0, 0xfc, 0xb4, 0x3c]),
        csidl: -1,
        name: "PhotoAlbums",
        category: 4,
        parent: Some(guid(
            0x33e28130,
            0x4e1e,
            0x4676,
            [0x83, 0x5a, 0x98, 0x39, 0x5c, 0x3b, 0xc3, 0xbb],
        )),
        relative: Some("Slide Shows"),
        parsing: None,
        attributes: 0x1,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Pictures
        id: guid(0x33e28130, 0x4e1e, 0x4676, [0x83, 0x5a, 0x98, 0x39, 0x5c, 0x3b, 0xc3, 0xbb]),
        csidl: 39,
        name: "My Pictures",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Pictures"),
        parsing: Some("shell:::{3ADD1653-EB32-4CB0-BBD7-DFA0ABB5ACCA}"),
        attributes: 0x1,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_Playlists
        id: guid(0xde92c1c7, 0x837f, 0x4f69, [0xa3, 0xbb, 0x86, 0xe6, 0x31, 0x20, 0x4a, 0x23]),
        csidl: -1,
        name: "Playlists",
        category: 4,
        parent: Some(guid(
            0x4bd8d571,
            0x6d19,
            0x48d3,
            [0xbe, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0e, 0x43],
        )),
        relative: Some("Playlists"),
        parsing: None,
        attributes: 0x1,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_PrintHood
        id: guid(0x9274bd8d, 0xcfd1, 0x41c3, [0xb3, 0x5e, 0xb1, 0x3f, 0x55, 0xa7, 0x58, 0xf4]),
        csidl: 27,
        name: "PrintHood",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Printer Shortcuts"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Profile
        id: guid(0x5e6c858f, 0x0e22, 0x4760, [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73]),
        csidl: 40,
        name: "Profile",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("USERPROFILE", "")),
    },
    Def {
        // FOLDERID_ProgramData
        id: guid(0x62ab5d82, 0xfdc1, 0x4dc3, [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97]),
        csidl: 35,
        name: "Common AppData",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("ProgramData", "")),
    },
    Def {
        // FOLDERID_ProgramFiles
        id: guid(0x905e63b6, 0xc1bf, 0x494e, [0xb2, 0x9c, 0x65, 0xb7, 0x32, 0xd3, 0xd2, 0x1a]),
        csidl: 38,
        name: "ProgramFiles",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: Some(Folder::Env("ProgramFiles", "")),
    },
    Def {
        // FOLDERID_ProgramFilesCommon
        id: guid(0xf7f1ed05, 0x9f6d, 0x47a2, [0xaa, 0xae, 0x29, 0xd3, 0x17, 0xc6, 0xf0, 0x66]),
        csidl: 43,
        name: "ProgramFilesCommon",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("CommonProgramFiles", "")),
    },
    #[cfg(target_arch = "x86_64")]
    Def {
        // FOLDERID_ProgramFilesCommonX64
        id: guid(0x6365d5a7, 0x0f0d, 0x45e5, [0x87, 0xf6, 0x0d, 0xa5, 0x6b, 0x6a, 0x4f, 0x7d]),
        csidl: -1,
        name: "ProgramFilesCommonX64",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("CommonProgramW6432", "")),
    },
    Def {
        // FOLDERID_ProgramFilesCommonX86
        id: guid(0xde974d24, 0xd9c6, 0x4d3e, [0xbf, 0x91, 0xf4, 0x45, 0x51, 0x20, 0xb9, 0x17]),
        csidl: 44,
        name: "ProgramFilesCommonX86",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("CommonProgramFiles(x86)", "")),
    },
    #[cfg(target_arch = "x86_64")]
    Def {
        // FOLDERID_ProgramFilesX64
        id: guid(0x6d809377, 0x6af0, 0x444b, [0x89, 0x57, 0xa3, 0x77, 0x3f, 0x02, 0x20, 0x0e]),
        csidl: -1,
        name: "ProgramFilesX64",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("ProgramW6432", "")),
    },
    Def {
        // FOLDERID_ProgramFilesX86
        id: guid(0x7c5a40ef, 0xa0fb, 0x4bfc, [0x87, 0x4a, 0xc0, 0xf2, 0xe0, 0xb9, 0xfa, 0x8e]),
        csidl: 42,
        name: "ProgramFilesX86",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: Some(Folder::Env("ProgramFiles(x86)", "")),
    },
    Def {
        // FOLDERID_Programs
        id: guid(0xa77f5d77, 0x2e2b, 0x44c3, [0xa6, 0xa2, 0xab, 0xa6, 0x01, 0x05, 0x4a, 0x51]),
        csidl: 2,
        name: "Programs",
        category: 4,
        parent: Some(guid(
            0x625b53c3,
            0xab48,
            0x4ec1,
            [0xba, 0x1f, 0xa1, 0xef, 0x41, 0x46, 0xfc, 0x19],
        )),
        relative: Some("Programs"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_Public
        id: guid(0xdfdf76a2, 0xc82a, 0x4d63, [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85]),
        csidl: -1,
        name: "Public",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: Some(Folder::Env("PUBLIC", "")),
    },
    Def {
        // FOLDERID_PublicDesktop
        id: guid(0xc4aa340d, 0xf20f, 0x4863, [0xaf, 0xef, 0xf8, 0x7e, 0xf2, 0xe6, 0xba, 0x25]),
        csidl: 25,
        name: "Common Desktop",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Desktop"),
        parsing: None,
        attributes: 0x3,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicDocuments
        id: guid(0xed4824af, 0xdce4, 0x45a8, [0x81, 0xe2, 0xfc, 0x79, 0x65, 0x08, 0x36, 0x34]),
        csidl: 46,
        name: "Common Documents",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Documents"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicDownloads
        id: guid(0x3d644c9b, 0x1fb8, 0x4f30, [0x9b, 0x45, 0xf6, 0x70, 0x23, 0x5f, 0x79, 0xc0]),
        csidl: -1,
        name: "CommonDownloads",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Downloads"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicGameTasks
        id: guid(0xdebf2536, 0xe1a8, 0x4c59, [0xb6, 0xa2, 0x41, 0x45, 0x86, 0x47, 0x6a, 0xea]),
        csidl: -1,
        name: "PublicGameTasks",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("Microsoft\\Windows\\GameExplorer"),
        parsing: None,
        attributes: 0x0,
        flags: 0x2,
        root: None,
    },
    Def {
        // FOLDERID_PublicLibraries
        id: guid(0x48daf80b, 0xe6cf, 0x4f4e, [0xb8, 0x00, 0x0e, 0x69, 0xd8, 0x4e, 0xe3, 0x84]),
        csidl: -1,
        name: "PublicLibraries",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Libraries"),
        parsing: None,
        attributes: 0x3,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicMusic
        id: guid(0x3214fab5, 0x9757, 0x4298, [0xbb, 0x61, 0x92, 0xa9, 0xde, 0xaa, 0x44, 0xff]),
        csidl: 53,
        name: "CommonMusic",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Music"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicPictures
        id: guid(0xb6ebfb86, 0x6907, 0x413c, [0x9a, 0xf7, 0x4f, 0xc2, 0xab, 0xf0, 0x7c, 0xc5]),
        csidl: 54,
        name: "CommonPictures",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Pictures"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicRingtones
        id: guid(0xe555ab60, 0x153b, 0x4d17, [0x9f, 0x04, 0xa5, 0xfe, 0x99, 0xfc, 0x15, 0xec]),
        csidl: -1,
        name: "CommonRingtones",
        category: 3,
        parent: Some(guid(
            0x62ab5d82,
            0xfdc1,
            0x4dc3,
            [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97],
        )),
        relative: Some("Microsoft\\Windows\\Ringtones"),
        parsing: None,
        attributes: 0x0,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_PublicVideos
        id: guid(0x2400183a, 0x6185, 0x49fb, [0xa2, 0xd8, 0x4a, 0x39, 0x2a, 0x60, 0x2b, 0xa3]),
        csidl: 55,
        name: "CommonVideo",
        category: 3,
        parent: Some(guid(
            0xdfdf76a2,
            0xc82a,
            0x4d63,
            [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85],
        )),
        relative: Some("Videos"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_QuickLaunch
        id: guid(0x52a4f021, 0x7b75, 0x48a9, [0x9f, 0x6b, 0x4b, 0x87, 0xa2, 0x10, 0xbc, 0x8f]),
        csidl: -1,
        name: "Quick Launch",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Internet Explorer\\Quick Launch"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Recent
        id: guid(0xae50c081, 0xebd2, 0x438a, [0x86, 0x55, 0x8a, 0x09, 0x2e, 0x34, 0x98, 0x7a]),
        csidl: 8,
        name: "Recent",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Recent"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_ResourceDir
        id: guid(0x8ad10c31, 0x2adb, 0x4296, [0xa8, 0xf7, 0xe4, 0x70, 0x12, 0x32, 0xc9, 0x72]),
        csidl: 56,
        name: "ResourceDir",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("windir", "\\Resources")),
    },
    Def {
        // FOLDERID_Ringtones
        id: guid(0xc870044b, 0xf49e, 0x4126, [0xa9, 0xc3, 0xb5, 0x2a, 0x1f, 0xf4, 0x11, 0xe8]),
        csidl: -1,
        name: "Ringtones",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Microsoft\\Windows\\Ringtones"),
        parsing: None,
        attributes: 0x0,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_RoamingAppData
        id: guid(0x3eb685db, 0x65f9, 0x4cf6, [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d]),
        csidl: 26,
        name: "AppData",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("AppData\\Roaming"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_SavedGames
        id: guid(0x4c5c32ff, 0xbb9d, 0x43b0, [0xb5, 0xb4, 0x2d, 0x72, 0xe5, 0x4e, 0xaa, 0xa4]),
        csidl: -1,
        name: "SavedGames",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Saved Games"),
        parsing: Some(
            "::{59031a47-3f72-44a7-89c5-5595fe6b30ee}\\{4C5C32FF-BB9D-43b0-B5B4-2D72E54EAAA4}",
        ),
        attributes: 0x1,
        flags: 0x2c,
        root: None,
    },
    Def {
        // FOLDERID_SavedSearches
        id: guid(0x7d1d3a04, 0xdebb, 0x4115, [0x95, 0xcf, 0x2f, 0x29, 0xda, 0x29, 0x20, 0xda]),
        csidl: -1,
        name: "Searches",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Searches"),
        parsing: Some(
            "::{59031a47-3f72-44a7-89c5-5595fe6b30ee}\\{7d1d3a04-debb-4115-95cf-2f29da2920da}",
        ),
        attributes: 0x1,
        flags: 0x28,
        root: None,
    },
    Def {
        // FOLDERID_SendTo
        id: guid(0x8983036c, 0x27c0, 0x404b, [0x8f, 0x08, 0x10, 0x2d, 0x10, 0xdc, 0xfd, 0x74]),
        csidl: 9,
        name: "SendTo",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\SendTo"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_StartMenu
        id: guid(0x625b53c3, 0xab48, 0x4ec1, [0xba, 0x1f, 0xa1, 0xef, 0x41, 0x46, 0xfc, 0x19]),
        csidl: 11,
        name: "Start Menu",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Start Menu"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_Startup
        id: guid(0xb97d20bb, 0xf46a, 0x4c97, [0xba, 0x10, 0x5e, 0x36, 0x08, 0x43, 0x08, 0x54]),
        csidl: 7,
        name: "Startup",
        category: 4,
        parent: Some(guid(
            0xa77f5d77,
            0x2e2b,
            0x44c3,
            [0xa6, 0xa2, 0xab, 0xa6, 0x01, 0x05, 0x4a, 0x51],
        )),
        relative: Some("StartUp"),
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_System
        id: guid(0x1ac14e77, 0x02e7, 0x4e5d, [0xb7, 0x44, 0x2e, 0xb1, 0xae, 0x51, 0x98, 0xb7]),
        csidl: 37,
        name: "System",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::System),
    },
    Def {
        // FOLDERID_SystemX86
        id: guid(0xd65231b0, 0xb2f1, 0x4857, [0xa4, 0xce, 0xa8, 0xe7, 0xc6, 0xea, 0x7d, 0x27]),
        csidl: 41,
        name: "SystemX86",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Env("windir", "\\SysWOW64")),
    },
    Def {
        // FOLDERID_Templates
        id: guid(0xa63293e8, 0x664e, 0x48db, [0xa0, 0x79, 0xdf, 0x75, 0x9e, 0x05, 0x09, 0xf7]),
        csidl: 21,
        name: "Templates",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\Templates"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_UserPinned
        id: guid(0x9e3995ab, 0x1f9c, 0x4f13, [0xb8, 0x27, 0x48, 0xb2, 0x4b, 0x6c, 0x71, 0x74]),
        csidl: -1,
        name: "User Pinned",
        category: 4,
        parent: Some(guid(
            0x52a4f021,
            0x7b75,
            0x48a9,
            [0x9f, 0x6b, 0x4b, 0x87, 0xa2, 0x10, 0xbc, 0x8f],
        )),
        relative: Some("User Pinned"),
        parsing: None,
        attributes: 0x2,
        flags: 0x8,
        root: None,
    },
    Def {
        // FOLDERID_UserProfiles
        id: guid(0x0762d272, 0xc50a, 0x4bb0, [0xa3, 0x82, 0x69, 0x7d, 0xcd, 0x72, 0x9b, 0x80]),
        csidl: -1,
        name: "UserProfiles",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x1,
        flags: 0x8,
        root: Some(Folder::Env("SystemDrive", "\\Users")),
    },
    Def {
        // FOLDERID_UserProgramFiles
        id: guid(0x5cd7aee2, 0x2219, 0x4a67, [0xb8, 0x5d, 0x6c, 0x9c, 0xe1, 0x56, 0x60, 0xcb]),
        csidl: -1,
        name: "UserProgramFiles",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Programs"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_UserProgramFilesCommon
        id: guid(0xbcbd3057, 0xca5c, 0x4622, [0xb4, 0x2d, 0xbc, 0x56, 0xdb, 0x0a, 0xe5, 0x16]),
        csidl: -1,
        name: "UserProgramFilesCommon",
        category: 4,
        parent: Some(guid(
            0x5cd7aee2,
            0x2219,
            0x4a67,
            [0xb8, 0x5d, 0x6c, 0x9c, 0xe1, 0x56, 0x60, 0xcb],
        )),
        relative: Some("Common"),
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: None,
    },
    Def {
        // FOLDERID_Videos
        id: guid(0x18989b1d, 0x99b5, 0x455b, [0x84, 0x1c, 0xab, 0x7c, 0x74, 0xe4, 0xdd, 0xfc]),
        csidl: 14,
        name: "My Video",
        category: 4,
        parent: Some(guid(
            0x5e6c858f,
            0x0e22,
            0x4760,
            [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73],
        )),
        relative: Some("Videos"),
        parsing: Some("shell:::{A0953C92-50DC-43BF-BE83-3742FED03C9C}"),
        attributes: 0x1,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_Windows
        id: guid(0xf38bf404, 0x1d43, 0x42f2, [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23]),
        csidl: 36,
        name: "Windows",
        category: 2,
        parent: None,
        relative: None,
        parsing: None,
        attributes: 0x0,
        flags: 0x0,
        root: Some(Folder::Windows),
    },
    Def {
        // FOLDERID_AccountPictures
        id: guid(0x008ca0b1, 0x55b4, 0x4c56, [0xb8, 0xa8, 0x4d, 0xe4, 0xb2, 0x99, 0xd3, 0xbe]),
        csidl: -1,
        name: "AccountPictures",
        category: 4,
        parent: Some(guid(
            0x3eb685db,
            0x65f9,
            0x4cf6,
            [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d],
        )),
        relative: Some("Microsoft\\Windows\\AccountPictures"),
        parsing: None,
        attributes: 0x1,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_Screenshots
        id: guid(0xb7bede81, 0xdf94, 0x4682, [0xa7, 0xd8, 0x57, 0xa5, 0x26, 0x20, 0xb8, 0x6f]),
        csidl: -1,
        name: "Screenshots",
        category: 4,
        parent: Some(guid(
            0x33e28130,
            0x4e1e,
            0x4676,
            [0x83, 0x5a, 0x98, 0x39, 0x5c, 0x3b, 0xc3, 0xbb],
        )),
        relative: Some("Screenshots"),
        parsing: None,
        attributes: 0x0,
        flags: 0xc,
        root: None,
    },
    Def {
        // FOLDERID_AppDataDocuments
        id: guid(0x7be16610, 0x1f7f, 0x44ac, [0xbf, 0xf0, 0x83, 0xe1, 0x5f, 0x2f, 0xfc, 0xa1]),
        csidl: -1,
        name: "AppDataDocuments",
        category: 4,
        parent: Some(guid(
            0xf1b32785,
            0x6fba,
            0x4fcf,
            [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
        )),
        relative: Some("Documents"),
        parsing: None,
        attributes: 0x0,
        flags: 0xc,
        root: None,
    },
];

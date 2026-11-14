//! Shell links: CLSID_ShellLink's object (IShellLinkW, IPersistFile,
//! IShellLinkDataList, IPropertyStore) and the `.lnk` file it reads and
//! writes, as [MS-SHLLINK] lays it out (the property store's block as
//! [MS-PROPSTORE] does). ole32's CoCreateInstance finds the class through
//! `HKLM\SOFTWARE\Classes\CLSID\{00021401-...}\InprocServer32` (the
//! registry seed) and this DLL's DllGetClassObject.
//!
//! What a saved link holds: the header (the target's attributes, times
//! and size as SetPath found them, the icon index, show command and hot
//! key), the target as a LinkInfo (a drive path's volume and local base
//! path, or a UNC path's share and suffix), the strings (Unicode), and
//! the extra blocks: the unexpanded target and icon of paths with
//! environment variables, the blocks a loaded link had that gala does not
//! interpret (kept as they were), the property store. No IDList is
//! written: Windows' shell item IDs are not documented, and gala's own
//! item ID lists mean nothing to another system; a loaded link's IDList
//! is written back while its target is unchanged. A loaded link whose
//! only target is an IDList has no path here (recorded). IShellLinkA,
//! Darwin (MSI) targets and link tracking (Resolve of a target that
//! moved) are recorded as unsupported.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{E_INVALIDARG, E_OUTOFMEMORY, MAX_PATH, S_OK, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u16;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(h: usize, buf: *const u8, n: u32, written: *mut u32, overlapped: usize) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
    fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut [u32; 9]) -> i32;
    fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        ext: *const u16,
        size: u32,
        buf: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
    fn GetFullPathNameW(name: *const u16, size: u32, buf: *mut u16, part: *mut *mut u16) -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        label: *mut u16,
        label_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs: *mut u16,
        fs_size: u32,
    ) -> i32;
}

type HResult = i32;
pub type Guid = [u8; 16];

const S_FALSE: HResult = 1;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;

const fn hresult_from_win32(e: u32) -> HResult {
    if e == 0 { E_FAIL } else { (0x8007_0000 | (e & 0xffff)) as i32 }
}

/// A GUID's bytes as in memory (Data1-3 little endian).
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const OLE: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
pub const CLSID_SHELL_LINK: Guid = guid(0x0002_1401, 0, 0, OLE);
const IID_IUNKNOWN: Guid = guid(0, 0, 0, OLE);
const IID_ICLASSFACTORY: Guid = guid(1, 0, 0, OLE);
const IID_IPERSIST: Guid = guid(0x10c, 0, 0, OLE);
const IID_IPERSISTFILE: Guid = guid(0x10b, 0, 0, OLE);
const IID_ISHELLLINKW: Guid = guid(0x0002_14f9, 0, 0, OLE);
const IID_ISHELLLINKDATALIST: Guid =
    guid(0x45e2_b4ae, 0xb1c3, 0x11d0, [0xb9, 0x2f, 0x00, 0xa0, 0xc9, 0x03, 0x12, 0xe1]);
const IID_IPROPERTYSTORE: Guid =
    guid(0x886d_8eeb, 0x8cf2, 0x4446, [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99]);

// ---- Heap buffers ----

/// A growable buffer on the process heap (fallible: a failed growth
/// leaves it as it was and says so).
pub struct HBuf<T: Copy> {
    ptr: *mut T,
    len: usize,
    cap: usize,
}

impl<T: Copy> HBuf<T> {
    pub const fn new() -> Self {
        HBuf { ptr: core::ptr::null_mut(), len: 0, cap: 0 }
    }

    pub fn as_slice(&self) -> &[T] {
        if self.ptr.is_null() {
            return &[];
        }
        // SAFETY: `len` initialized elements at `ptr`.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn reserve(&mut self, more: usize) -> bool {
        let Some(need) = self.len.checked_add(more) else { return false };
        if need <= self.cap {
            return true;
        }
        let cap = need.max(self.cap.saturating_mul(2)).max(16);
        let Some(bytes) = cap.checked_mul(core::mem::size_of::<T>()) else { return false };
        // SAFETY: the process heap; a block of ours (or none yet).
        let p = unsafe {
            if self.ptr.is_null() {
                HeapAlloc(GetProcessHeap(), 0, bytes)
            } else {
                HeapReAlloc(GetProcessHeap(), 0, self.ptr.cast(), bytes)
            }
        };
        if p.is_null() {
            return false;
        }
        self.ptr = p.cast();
        self.cap = cap;
        true
    }

    pub fn extend(&mut self, items: &[T]) -> bool {
        if !self.reserve(items.len()) {
            return false;
        }
        // SAFETY: room for `items.len()` more after `len`.
        unsafe {
            core::ptr::copy_nonoverlapping(items.as_ptr(), self.ptr.add(self.len), items.len())
        };
        self.len += items.len();
        true
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Replaces the contents (unchanged if that fails).
    pub fn set(&mut self, items: &[T]) -> bool {
        let keep = self.len;
        self.len = 0;
        if !self.reserve(items.len()) {
            self.len = keep;
            return false;
        }
        self.extend(items)
    }

    /// Drops `range` from the buffer.
    fn remove(&mut self, start: usize, end: usize) {
        let end = end.min(self.len);
        if start >= end {
            return;
        }
        // SAFETY: both ranges are inside the buffer.
        unsafe { core::ptr::copy(self.ptr.add(end), self.ptr.add(start), self.len - end) };
        self.len -= end - start;
    }

    fn free(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: our block.
            unsafe { HeapFree(GetProcessHeap(), 0, self.ptr.cast()) };
        }
        *self = HBuf::new();
    }
}

/// A NUL-terminated string of the caller's (empty for NULL).
pub fn caller_str<'a>(s: *const u16) -> &'a [u16] {
    if s.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: its `n` units.
    unsafe { core::slice::from_raw_parts(s, n) }
}

/// `s` NUL-terminated in `out` (MAX_PATH-sized buffers and up); None if
/// it does not fit.
fn z<const N: usize>(s: &[u16]) -> Option<[u16; N]> {
    let mut out = [0u16; N];
    if s.len() >= N {
        return None;
    }
    out[..s.len()].copy_from_slice(s);
    Some(out)
}

/// Copies `s` into the caller's buffer of `cch` characters, cut to fit
/// and NUL-terminated.
fn give(s: &[u16], buf: *mut u16, cch: i32) {
    if buf.is_null() || cch <= 0 {
        return;
    }
    let n = s.len().min(cch as usize - 1);
    // SAFETY: the caller's buffer of `cch` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(s.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
}

fn is(c: u16, ch: u8) -> bool {
    c == u16::from(ch)
}

// ---- The object ----

#[repr(transparent)]
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: tables of function pointers, never written.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// A shell link: four interface pointers (IShellLinkW first, the
/// object's identity), a reference count and the link's data.
#[repr(C)]
struct Link {
    shell_link: *const VTable<21>,
    persist_file: *const VTable<9>,
    data_list: *const VTable<8>,
    property_store: *const VTable<8>,
    refs: AtomicU32,
    /// The target (environment variables expanded), and as given when it
    /// had some (SLDF_HAS_EXP_SZ).
    path: HBuf<u16>,
    raw_path: HBuf<u16>,
    description: HBuf<u16>,
    workdir: HBuf<u16>,
    args: HBuf<u16>,
    icon: HBuf<u16>,
    /// The icon as given when it had environment variables (HasExpIcon).
    raw_icon: HBuf<u16>,
    relative: HBuf<u16>,
    icon_index: i32,
    show: i32,
    hotkey: u16,
    /// LinkFlags (SLDF_*): what SetFlags or a loaded file set; the
    /// structure bits are recomputed when saving.
    flags: u32,
    /// The target's attributes, creation, access and write times and size.
    attributes: u32,
    times: [u64; 3],
    size: u32,
    /// A loaded link's IDList (kept while its target is unchanged).
    idlist: HBuf<u8>,
    /// Extra data blocks kept as they were (whole blocks, headers included).
    extra: HBuf<u8>,
    /// Properties: records of FMTID (16), pid (4), byte count (4) and a
    /// TypedPropertyValue; and whole serialized storages gala does not
    /// address (string names), written back as they were.
    props: HBuf<u8>,
    named_storages: HBuf<u8>,
    file: HBuf<u16>,
    dirty: bool,
}

/// An interface pointer's object: its vtable slot is `slot` pointers in.
fn object<'a>(this: *mut c_void, slot: usize) -> &'a mut Link {
    // SAFETY: COM calls this interface's methods with the pointer we gave
    // out, `slot` pointers into a live Link.
    unsafe { &mut *(this.cast::<*const ()>().sub(slot).cast::<Link>()) }
}

pub(crate) fn iid(p: *const Guid) -> Guid {
    // SAFETY: the caller's GUID (null checked).
    if p.is_null() { [0; 16] } else { unsafe { p.read_unaligned() } }
}

pub(crate) fn put<T>(out: *mut T, v: T) {
    // SAFETY: the caller's output (null checked by the callers).
    unsafe { out.write_unaligned(v) };
}

fn query(link: &mut Link, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let base = link as *mut Link as *mut *const ();
    let slot = match iid(riid) {
        IID_IUNKNOWN | IID_ISHELLLINKW => 0,
        IID_IPERSIST | IID_IPERSISTFILE => 1,
        IID_ISHELLLINKDATALIST => 2,
        IID_IPROPERTYSTORE => 3,
        other => {
            put(out, core::ptr::null_mut());
            let d1 = u32::from_le_bytes([other[0], other[1], other[2], other[3]]);
            unsupported(b"shell32.dll!ShellLink(QueryInterface)", d1.into());
            return E_NOINTERFACE;
        }
    };
    link.refs.fetch_add(1, Ordering::Relaxed);
    // SAFETY: one of the four interface slots at the object's start.
    put(out, unsafe { base.add(slot) }.cast());
    S_OK
}

fn add_ref(link: &mut Link) -> u32 {
    link.refs.fetch_add(1, Ordering::Relaxed) + 1
}

fn release(link: &mut Link) -> u32 {
    let left = link.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        for b in [
            &mut link.path,
            &mut link.raw_path,
            &mut link.description,
            &mut link.workdir,
            &mut link.args,
            &mut link.icon,
            &mut link.raw_icon,
            &mut link.relative,
            &mut link.file,
        ] {
            b.free();
        }
        link.idlist.free();
        link.extra.free();
        link.props.free();
        link.named_storages.free();
        // SAFETY: the object's own block, no longer referenced.
        unsafe { HeapFree(GetProcessHeap(), 0, (link as *mut Link).cast()) };
    }
    left
}

/// A new link with one reference.
fn new_link() -> Option<*mut Link> {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<Link>()) }.cast::<Link>();
    if p.is_null() {
        return None;
    }
    // SAFETY: a fresh block of the Link's size (the process heap aligns
    // to 16).
    unsafe {
        p.write(Link {
            shell_link: &SHELL_LINK_VTABLE,
            persist_file: &PERSIST_FILE_VTABLE,
            data_list: &DATA_LIST_VTABLE,
            property_store: &PROPERTY_STORE_VTABLE,
            refs: AtomicU32::new(1),
            path: HBuf::new(),
            raw_path: HBuf::new(),
            description: HBuf::new(),
            workdir: HBuf::new(),
            args: HBuf::new(),
            icon: HBuf::new(),
            raw_icon: HBuf::new(),
            relative: HBuf::new(),
            icon_index: 0,
            show: SW_SHOWNORMAL,
            hotkey: 0,
            flags: 0,
            attributes: 0,
            times: [0; 3],
            size: 0,
            idlist: HBuf::new(),
            extra: HBuf::new(),
            props: HBuf::new(),
            named_storages: HBuf::new(),
            file: HBuf::new(),
            dirty: false,
        })
    };
    Some(p)
}

const SW_SHOWNORMAL: i32 = 1;

// LinkFlags (SLDF_*).
const HAS_ID_LIST: u32 = 0x1;
const HAS_LINK_INFO: u32 = 0x2;
const HAS_NAME: u32 = 0x4;
const HAS_RELPATH: u32 = 0x8;
const HAS_WORKINGDIR: u32 = 0x10;
const HAS_ARGS: u32 = 0x20;
const HAS_ICONLOCATION: u32 = 0x40;
const UNICODE: u32 = 0x80;
const FORCE_NO_LINKINFO: u32 = 0x100;
const HAS_EXP_SZ: u32 = 0x200;
const HAS_DARWINID: u32 = 0x1000;
const HAS_EXP_ICON_SZ: u32 = 0x4000;
const DISABLE_LINK_PATH_TRACKING: u32 = 0x10_0000;
/// The bits that say which parts the file has (recomputed on Save).
const STRUCTURE: u32 = HAS_ID_LIST
    | HAS_LINK_INFO
    | HAS_NAME
    | HAS_RELPATH
    | HAS_WORKINGDIR
    | HAS_ARGS
    | HAS_ICONLOCATION
    | UNICODE
    | HAS_DARWINID;

// Extra data block signatures.
const EXP_SZ_LINK_SIG: u32 = 0xa000_0001;
const EXP_SZ_ICON_SIG: u32 = 0xa000_0007;
const PROPERTY_STORE_SIG: u32 = 0xa000_0009;
/// The size of EXP_SZ_LINK (header, 260 ANSI and 260 wide characters).
const EXP_SZ_SIZE: usize = 0x314;

impl Link {
    /// Sets the target as SetPath does; the HRESULT.
    fn set_path(&mut self, given: &[u16]) -> HResult {
        let mut given = given;
        // Quotes around the path are dropped.
        if given.len() >= 2 && is(given[0], b'"') && is(given[given.len() - 1], b'"') {
            given = &given[1..given.len() - 1];
        }
        self.idlist.clear();
        self.attributes = 0;
        self.times = [0; 3];
        self.size = 0;
        self.dirty = true;
        if given.is_empty() {
            self.path.clear();
            self.raw_path.clear();
            self.flags &= !HAS_EXP_SZ;
            return S_OK;
        }
        if given.starts_with(&[0x3a, 0x3a]) {
            // "::{CLSID}..." : a shell namespace object or a Darwin id.
            unsupported(b"shell32.dll!IShellLinkW::SetPath(namespace or Darwin target)", 0);
        }
        let Some(src) = z::<{ 2 * MAX_PATH }>(given) else { return E_INVALIDARG };
        let mut expanded = [0u16; 2 * MAX_PATH];
        let has_env = given.iter().any(|&c| is(c, b'%'));
        let name: &[u16] = if has_env {
            // SAFETY: NUL-terminated input, a buffer of its size.
            let n = unsafe {
                ExpandEnvironmentStringsW(src.as_ptr(), expanded.as_mut_ptr(), 2 * MAX_PATH as u32)
            };
            if n == 0 || n as usize > 2 * MAX_PATH {
                return E_INVALIDARG;
            }
            &expanded[..n as usize - 1]
        } else {
            given
        };
        let full = full_path(name);
        let Some(full) = full else { return E_INVALIDARG };
        let mut n = full.iter().position(|&c| c == 0).unwrap_or(full.len());
        // A trailing separator goes (not a drive's root's).
        while n > 3 && (is(full[n - 1], b'\\') || is(full[n - 1], b'/')) {
            n -= 1;
        }
        if !self.path.set(&full[..n]) {
            return E_OUTOFMEMORY;
        }
        if has_env {
            if !self.raw_path.set(given) {
                return E_OUTOFMEMORY;
            }
            self.flags |= HAS_EXP_SZ;
        } else {
            self.raw_path.clear();
            self.flags &= !HAS_EXP_SZ;
        }
        self.find_target();
        S_OK
    }

    /// The target's attributes, times and size, if it is there.
    fn find_target(&mut self) {
        let Some(p) = z::<{ 2 * MAX_PATH }>(self.path.as_slice()) else { return };
        let mut info = [0u32; 9];
        // SAFETY: a NUL-terminated path; WIN32_FILE_ATTRIBUTE_DATA is 36 bytes.
        if unsafe { GetFileAttributesExW(p.as_ptr(), 0, &mut info) } == 0 {
            return;
        }
        self.attributes = info[0];
        for (k, t) in self.times.iter_mut().enumerate() {
            *t = u64::from(info[1 + 2 * k]) | (u64::from(info[2 + 2 * k]) << 32);
        }
        self.size = info[8];
    }

    fn set_icon(&mut self, given: &[u16], index: i32) -> HResult {
        self.icon_index = index;
        self.dirty = true;
        if given.iter().any(|&c| is(c, b'%')) {
            let Some(src) = z::<{ 2 * MAX_PATH }>(given) else { return E_INVALIDARG };
            let mut out = [0u16; 2 * MAX_PATH];
            // SAFETY: NUL-terminated input, a buffer of its size.
            let n = unsafe {
                ExpandEnvironmentStringsW(src.as_ptr(), out.as_mut_ptr(), 2 * MAX_PATH as u32)
            };
            if n == 0 || n as usize > 2 * MAX_PATH {
                return E_INVALIDARG;
            }
            if !self.raw_icon.set(given) || !self.icon.set(&out[..n as usize - 1]) {
                return E_OUTOFMEMORY;
            }
            self.flags |= HAS_EXP_ICON_SZ;
        } else {
            self.raw_icon.clear();
            self.flags &= !HAS_EXP_ICON_SZ;
            if !self.icon.set(given) {
                return E_OUTOFMEMORY;
            }
        }
        S_OK
    }
}

/// The full form of a target path: a file found by SearchPath (".exe"
/// added to a name without an extension, a bare name along the search
/// path), else the path made absolute.
fn full_path(name: &[u16]) -> Option<[u16; 2 * MAX_PATH]> {
    let src = z::<{ 2 * MAX_PATH }>(name)?;
    let mut out = [0u16; 2 * MAX_PATH];
    let exe: [u16; 5] = [0x2e, 0x65, 0x78, 0x65, 0];
    let size = 2 * MAX_PATH as u32;
    let mut part = core::ptr::null_mut();
    // SAFETY: NUL-terminated strings and a buffer of `size` characters.
    let n = unsafe {
        SearchPathW(
            core::ptr::null(),
            src.as_ptr(),
            exe.as_ptr(),
            size,
            out.as_mut_ptr(),
            &mut part,
        )
    };
    if n > 0 && n < size {
        return Some(out);
    }
    // SAFETY: as above.
    let n = unsafe { GetFullPathNameW(src.as_ptr(), size, out.as_mut_ptr(), &mut part) };
    (n > 0 && n < size).then_some(out)
}

// ---- IShellLinkW ----

extern "system" fn sl_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    query(object(this, 0), riid, out)
}
extern "system" fn sl_add_ref(this: *mut c_void) -> u32 {
    add_ref(object(this, 0))
}
extern "system" fn sl_release(this: *mut c_void) -> u32 {
    release(object(this, 0))
}

/// The WIN32_FIND_DATAW of the target (zeroed when there is none).
fn find_data(link: &Link, fd: *mut u8) {
    if fd.is_null() {
        return;
    }
    // SAFETY: the caller's WIN32_FIND_DATAW (592 bytes).
    unsafe {
        core::ptr::write_bytes(fd, 0, 592);
        if link.path.as_slice().is_empty() {
            return;
        }
        (fd as *mut u32).write_unaligned(link.attributes);
        for (k, t) in link.times.iter().enumerate() {
            (fd.add(4 + 8 * k) as *mut u64).write_unaligned(*t);
        }
        (fd.add(32) as *mut u32).write_unaligned(link.size);
        let path = link.path.as_slice();
        let start = path.iter().rposition(|&c| is(c, b'\\') || is(c, b'/')).map_or(0, |i| i + 1);
        let name = &path[start..];
        let n = name.len().min(MAX_PATH - 1);
        for (i, &c) in name[..n].iter().enumerate() {
            (fd.add(44 + 2 * i) as *mut u16).write_unaligned(c);
        }
    }
}

extern "system" fn sl_get_path(
    this: *mut c_void,
    buf: *mut u16,
    cch: i32,
    fd: *mut u8,
    flags: u32,
) -> HResult {
    const SLGP_RAWPATH: u32 = 0x4;
    let link = object(this, 0);
    find_data(link, fd);
    if link.path.as_slice().is_empty() {
        give(&[], buf, cch);
        return S_FALSE;
    }
    let raw = flags & SLGP_RAWPATH != 0
        && link.flags & HAS_EXP_SZ != 0
        && !link.raw_path.as_slice().is_empty();
    give(if raw { link.raw_path.as_slice() } else { link.path.as_slice() }, buf, cch);
    S_OK
}

extern "system" fn sl_get_id_list(this: *mut c_void, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let link = object(this, 0);
    if link.path.as_slice().is_empty() {
        put(out, core::ptr::null_mut());
        return S_FALSE;
    }
    let list = super::namespace::simple_list(link.path.as_slice());
    put(out, list);
    if list.is_null() { S_FALSE } else { S_OK }
}

extern "system" fn sl_set_id_list(this: *mut c_void, list: *const u8) -> HResult {
    if list.is_null() {
        return E_INVALIDARG;
    }
    let mut path = [0u16; MAX_PATH];
    if super::namespace::SHGetPathFromIDListW(list, path.as_mut_ptr()) == 0 {
        unsupported(b"shell32.dll!IShellLinkW::SetIDList(not a file system item)", 0);
        return E_INVALIDARG;
    }
    let n = path.iter().position(|&c| c == 0).unwrap_or(MAX_PATH);
    object(this, 0).set_path(&path[..n])
}

fn get_string(s: &HBuf<u16>, buf: *mut u16, cch: i32) -> HResult {
    give(s.as_slice(), buf, cch);
    S_OK
}

/// One of a link's strings.
type Field = fn(&mut Link) -> &mut HBuf<u16>;

fn set_string(link: &mut Link, which: Field, s: *const u16) -> HResult {
    let s = caller_str(s);
    link.dirty = true;
    if which(link).set(s) { S_OK } else { E_OUTOFMEMORY }
}

extern "system" fn sl_get_description(this: *mut c_void, buf: *mut u16, cch: i32) -> HResult {
    get_string(&object(this, 0).description, buf, cch)
}
extern "system" fn sl_set_description(this: *mut c_void, s: *const u16) -> HResult {
    set_string(object(this, 0), |l| &mut l.description, s)
}
extern "system" fn sl_get_workdir(this: *mut c_void, buf: *mut u16, cch: i32) -> HResult {
    get_string(&object(this, 0).workdir, buf, cch)
}
extern "system" fn sl_set_workdir(this: *mut c_void, s: *const u16) -> HResult {
    set_string(object(this, 0), |l| &mut l.workdir, s)
}
extern "system" fn sl_get_args(this: *mut c_void, buf: *mut u16, cch: i32) -> HResult {
    get_string(&object(this, 0).args, buf, cch)
}
extern "system" fn sl_set_args(this: *mut c_void, s: *const u16) -> HResult {
    set_string(object(this, 0), |l| &mut l.args, s)
}
extern "system" fn sl_get_hotkey(this: *mut c_void, out: *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, object(this, 0).hotkey);
    S_OK
}
extern "system" fn sl_set_hotkey(this: *mut c_void, key: u16) -> HResult {
    let link = object(this, 0);
    link.hotkey = key;
    link.dirty = true;
    S_OK
}
extern "system" fn sl_get_show(this: *mut c_void, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, object(this, 0).show);
    S_OK
}
extern "system" fn sl_set_show(this: *mut c_void, show: i32) -> HResult {
    let link = object(this, 0);
    link.show = show;
    link.dirty = true;
    S_OK
}
extern "system" fn sl_get_icon(
    this: *mut c_void,
    buf: *mut u16,
    cch: i32,
    index: *mut i32,
) -> HResult {
    let link = object(this, 0);
    give(link.icon.as_slice(), buf, cch);
    if !index.is_null() {
        put(index, link.icon_index);
    }
    S_OK
}
extern "system" fn sl_set_icon(this: *mut c_void, path: *const u16, index: i32) -> HResult {
    object(this, 0).set_icon(caller_str(path), index)
}
extern "system" fn sl_set_relative(this: *mut c_void, s: *const u16, _reserved: u32) -> HResult {
    set_string(object(this, 0), |l| &mut l.relative, s)
}
extern "system" fn sl_resolve(this: *mut c_void, _hwnd: usize, _flags: u32) -> HResult {
    let link = object(this, 0);
    let Some(p) = z::<{ 2 * MAX_PATH }>(link.path.as_slice()) else { return S_OK };
    let mut info = [0u32; 9];
    // SAFETY: a NUL-terminated path.
    if link.path.as_slice().is_empty()
        || unsafe { GetFileAttributesExW(p.as_ptr(), 0, &mut info) } != 0
    {
        return S_OK;
    }
    // Windows would look for a target that moved (link tracking).
    unsupported(b"shell32.dll!IShellLinkW::Resolve(target not found)", 0);
    hresult_from_win32(2) // ERROR_FILE_NOT_FOUND
}
extern "system" fn sl_set_path(this: *mut c_void, path: *const u16) -> HResult {
    if path.is_null() {
        return E_INVALIDARG;
    }
    object(this, 0).set_path(caller_str(path))
}

static SHELL_LINK_VTABLE: VTable<21> = VTable([
    sl_query as *const (),
    sl_add_ref as *const (),
    sl_release as *const (),
    sl_get_path as *const (),
    sl_get_id_list as *const (),
    sl_set_id_list as *const (),
    sl_get_description as *const (),
    sl_set_description as *const (),
    sl_get_workdir as *const (),
    sl_set_workdir as *const (),
    sl_get_args as *const (),
    sl_set_args as *const (),
    sl_get_hotkey as *const (),
    sl_set_hotkey as *const (),
    sl_get_show as *const (),
    sl_set_show as *const (),
    sl_get_icon as *const (),
    sl_set_icon as *const (),
    sl_set_relative as *const (),
    sl_resolve as *const (),
    sl_set_path as *const (),
]);

// ---- IPersistFile ----

extern "system" fn pf_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    query(object(this, 1), riid, out)
}
extern "system" fn pf_add_ref(this: *mut c_void) -> u32 {
    add_ref(object(this, 1))
}
extern "system" fn pf_release(this: *mut c_void) -> u32 {
    release(object(this, 1))
}
extern "system" fn pf_class_id(_this: *mut c_void, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, CLSID_SHELL_LINK);
    S_OK
}
extern "system" fn pf_is_dirty(this: *mut c_void) -> HResult {
    if object(this, 1).dirty { S_OK } else { S_FALSE }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
const INVALID_HANDLE: usize = usize::MAX;
/// The largest link file read (links are a few KiB).
const MAX_FILE: i64 = 1 << 20;

extern "system" fn pf_load(this: *mut c_void, name: *const u16, _mode: u32) -> HResult {
    let link = object(this, 1);
    if name.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's NUL-terminated name.
    let h = unsafe {
        CreateFileW(
            name,
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h == INVALID_HANDLE {
        // SAFETY: plain call.
        return hresult_from_win32(unsafe { GetLastError() });
    }
    let mut data: HBuf<u8> = HBuf::new();
    let mut size = 0i64;
    // SAFETY: our handle.
    let ok = unsafe { GetFileSizeEx(h, &mut size) } != 0;
    let mut result = if !ok {
        // SAFETY: plain call.
        hresult_from_win32(unsafe { GetLastError() })
    } else if size > MAX_FILE {
        unsupported(b"shell32.dll!IPersistFile::Load(link over 1 MiB)", size as u64);
        E_FAIL
    } else if !data.reserve(size as usize) {
        E_OUTOFMEMORY
    } else {
        let mut read = 0u32;
        // SAFETY: room for `size` bytes.
        if unsafe { ReadFile(h, data.ptr, size as u32, &mut read, core::ptr::null_mut()) } == 0 {
            // SAFETY: plain call.
            hresult_from_win32(unsafe { GetLastError() })
        } else {
            data.len = read as usize;
            S_OK
        }
    };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if result == S_OK {
        result = parse(link, data.as_slice());
    }
    data.free();
    if result == S_OK {
        if !link.file.set(caller_str(name)) {
            return E_OUTOFMEMORY;
        }
        link.dirty = false;
    }
    result
}

extern "system" fn pf_save(this: *mut c_void, name: *const u16, remember: i32) -> HResult {
    let link = object(this, 1);
    let mut target = [0u16; 2 * MAX_PATH];
    let given = caller_str(name);
    let path = if given.is_empty() { link.file.as_slice() } else { given };
    if path.is_empty() {
        // Nothing to save to (Windows 7 and later answer S_OK).
        return S_OK;
    }
    if path.len() >= target.len() {
        return E_INVALIDARG;
    }
    target[..path.len()].copy_from_slice(path);
    let mut out: HBuf<u8> = HBuf::new();
    if !serialize(link, &mut out) {
        out.free();
        return E_OUTOFMEMORY;
    }
    // SAFETY: a NUL-terminated name.
    let h = unsafe {
        CreateFileW(
            target.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            core::ptr::null(),
            CREATE_ALWAYS,
            0,
            0,
        )
    };
    let result = if h == INVALID_HANDLE {
        // SAFETY: plain call.
        hresult_from_win32(unsafe { GetLastError() })
    } else {
        let mut written = 0u32;
        let bytes = out.as_slice();
        // SAFETY: our handle and buffer.
        let ok = unsafe { WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, 0) } != 0;
        // SAFETY: plain call (before CloseHandle changes it).
        let error = unsafe { GetLastError() };
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        if ok && written as usize == bytes.len() { S_OK } else { hresult_from_win32(error) }
    };
    out.free();
    if result == S_OK && (remember != 0 || given.is_empty()) {
        if !given.is_empty() && !link.file.set(given) {
            return E_OUTOFMEMORY;
        }
        link.dirty = false;
    }
    result
}

extern "system" fn pf_save_completed(_this: *mut c_void, _name: *const u16) -> HResult {
    S_OK
}

extern "system" fn pf_get_cur_file(this: *mut c_void, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let link = object(this, 1);
    let name = link.file.as_slice();
    if name.is_empty() {
        put(out, core::ptr::null_mut());
        return S_FALSE;
    }
    // The task allocator's memory (gala's CoTaskMemAlloc is the process heap).
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, 2 * (name.len() + 1)) }.cast::<u16>();
    if p.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: a block of len + 1 characters.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), p, name.len());
        *p.add(name.len()) = 0;
    }
    put(out, p);
    S_OK
}

static PERSIST_FILE_VTABLE: VTable<9> = VTable([
    pf_query as *const (),
    pf_add_ref as *const (),
    pf_release as *const (),
    pf_class_id as *const (),
    pf_is_dirty as *const (),
    pf_load as *const (),
    pf_save as *const (),
    pf_save_completed as *const (),
    pf_get_cur_file as *const (),
]);

// ---- IShellLinkDataList ----

extern "system" fn dl_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    query(object(this, 2), riid, out)
}
extern "system" fn dl_add_ref(this: *mut c_void) -> u32 {
    add_ref(object(this, 2))
}
extern "system" fn dl_release(this: *mut c_void) -> u32 {
    release(object(this, 2))
}

/// An EXP_SZ_LINK block of `raw` (for the target or the icon).
fn exp_sz(sig: u32, raw: &[u16]) -> HBuf<u8> {
    let mut b: HBuf<u8> = HBuf::new();
    let mut block = [0u8; EXP_SZ_SIZE];
    block[..4].copy_from_slice(&(EXP_SZ_SIZE as u32).to_le_bytes());
    block[4..8].copy_from_slice(&sig.to_le_bytes());
    for (i, &c) in raw.iter().take(MAX_PATH - 1).enumerate() {
        block[8 + i] = if c < 0x80 { c as u8 } else { b'?' };
        block[8 + MAX_PATH + 2 * i..8 + MAX_PATH + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
    }
    if !b.extend(&block) {
        b.free();
    }
    b
}

/// The wide string of an EXP_SZ_LINK block (the ANSI one if empty).
fn exp_sz_target(block: &[u8], out: &mut HBuf<u16>) -> bool {
    if block.len() < EXP_SZ_SIZE {
        return false;
    }
    let wide = &block[8 + MAX_PATH..8 + 3 * MAX_PATH];
    let mut units = [0u16; MAX_PATH];
    let mut n = 0;
    while n < MAX_PATH && (wide[2 * n] != 0 || wide[2 * n + 1] != 0) {
        units[n] = u16::from_le_bytes([wide[2 * n], wide[2 * n + 1]]);
        n += 1;
    }
    if n == 0 {
        while n < MAX_PATH && block[8 + n] != 0 {
            units[n] = block[8 + n].into();
            n += 1;
        }
    }
    out.set(&units[..n])
}

/// The kept extra block with `sig`: its byte range.
fn find_block(extra: &[u8], sig: u32) -> Option<(usize, usize)> {
    let mut at = 0;
    while at + 8 <= extra.len() {
        let size = rd32(extra, at)? as usize;
        if size < 8 || at + size > extra.len() {
            return None;
        }
        if rd32(extra, at + 4)? == sig {
            return Some((at, at + size));
        }
        at += size;
    }
    None
}

extern "system" fn dl_add_block(this: *mut c_void, block: *const u8) -> HResult {
    let link = object(this, 2);
    if block.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's DATABLOCK_HEADER and the block it sizes.
    let (size, sig) = unsafe {
        ((block as *const u32).read_unaligned(), (block.add(4) as *const u32).read_unaligned())
    };
    if size < 8 {
        return E_INVALIDARG;
    }
    // SAFETY: `size` bytes of the caller's block.
    let bytes = unsafe { core::slice::from_raw_parts(block, size as usize) };
    link.dirty = true;
    match sig {
        EXP_SZ_LINK_SIG => {
            let mut raw: HBuf<u16> = HBuf::new();
            let r = if exp_sz_target(bytes, &mut raw) {
                link.set_path(raw.as_slice())
            } else {
                E_INVALIDARG
            };
            raw.free();
            r
        }
        EXP_SZ_ICON_SIG => {
            let mut raw: HBuf<u16> = HBuf::new();
            let index = link.icon_index;
            let r = if exp_sz_target(bytes, &mut raw) {
                link.set_icon(raw.as_slice(), index)
            } else {
                E_INVALIDARG
            };
            raw.free();
            r
        }
        PROPERTY_STORE_SIG => {
            if read_store(link, &bytes[8..]) {
                S_OK
            } else {
                E_INVALIDARG
            }
        }
        _ => {
            if let Some((s, e)) = find_block(link.extra.as_slice(), sig) {
                link.extra.remove(s, e);
            }
            if link.extra.extend(bytes) { S_OK } else { E_OUTOFMEMORY }
        }
    }
}

extern "system" fn dl_copy_block(this: *mut c_void, sig: u32, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let link = object(this, 2);
    let mut made: HBuf<u8> = HBuf::new();
    let bytes: &[u8] = match sig {
        EXP_SZ_LINK_SIG if link.flags & HAS_EXP_SZ != 0 && !link.raw_path.as_slice().is_empty() => {
            made = exp_sz(sig, link.raw_path.as_slice());
            made.as_slice()
        }
        EXP_SZ_ICON_SIG
            if link.flags & HAS_EXP_ICON_SZ != 0 && !link.raw_icon.as_slice().is_empty() =>
        {
            made = exp_sz(sig, link.raw_icon.as_slice());
            made.as_slice()
        }
        _ => match find_block(link.extra.as_slice(), sig) {
            Some((s, e)) => &link.extra.as_slice()[s..e],
            None => &[],
        },
    };
    if bytes.is_empty() {
        made.free();
        put(out, core::ptr::null_mut());
        return E_FAIL;
    }
    // The caller frees it with LocalFree.
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0, bytes.len()) }.cast::<u8>();
    if !p.is_null() {
        // SAFETY: a block of that size.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    }
    made.free();
    put(out, p);
    if p.is_null() { E_OUTOFMEMORY } else { S_OK }
}

extern "system" fn dl_remove_block(this: *mut c_void, sig: u32) -> HResult {
    let link = object(this, 2);
    link.dirty = true;
    match sig {
        EXP_SZ_LINK_SIG => link.flags &= !HAS_EXP_SZ,
        EXP_SZ_ICON_SIG => link.flags &= !HAS_EXP_ICON_SZ,
        PROPERTY_STORE_SIG => {
            link.props.clear();
            link.named_storages.clear();
        }
        _ => match find_block(link.extra.as_slice(), sig) {
            Some((s, e)) => link.extra.remove(s, e),
            None => return E_FAIL,
        },
    }
    S_OK
}

extern "system" fn dl_get_flags(this: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, object(this, 2).flags);
    S_OK
}

extern "system" fn dl_set_flags(this: *mut c_void, flags: u32) -> HResult {
    let link = object(this, 2);
    link.flags = flags;
    link.dirty = true;
    S_OK
}

static DATA_LIST_VTABLE: VTable<8> = VTable([
    dl_query as *const (),
    dl_add_ref as *const (),
    dl_release as *const (),
    dl_add_block as *const (),
    dl_copy_block as *const (),
    dl_remove_block as *const (),
    dl_get_flags as *const (),
    dl_set_flags as *const (),
]);

// ---- IPropertyStore ----

// Property records in `props`: FMTID (16), pid (4), length (4), then a
// TypedPropertyValue ([MS-OLEPS] 2.15) of that many bytes.

const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;
const VT_UI4: u16 = 19;
const VT_I8: u16 = 20;
const VT_UI8: u16 = 21;
const VT_LPWSTR: u16 = 31;
const VT_FILETIME: u16 = 64;
const VT_CLSID: u16 = 72;

fn rd32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn rd16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// The records as (start, key, value range).
fn records(props: &[u8]) -> impl Iterator<Item = (usize, [u8; 20], usize, usize)> + '_ {
    let mut at = 0;
    core::iter::from_fn(move || {
        let len = rd32(props, at + 20)? as usize;
        let start = at;
        let mut key = [0u8; 20];
        key.copy_from_slice(props.get(at..at + 20)?);
        let value = (at + 24, at + 24 + len);
        props.get(value.0..value.1)?;
        at = value.1;
        Some((start, key, value.0, value.1))
    })
}

fn find_prop(props: &[u8], key: &[u8; 20]) -> Option<(usize, usize, usize)> {
    records(props).find(|r| &r.1 == key).map(|(s, _, a, b)| (s, a, b))
}

fn key_at(key: *const u8) -> [u8; 20] {
    let mut k = [0u8; 20];
    // SAFETY: the caller's PROPERTYKEY (20 bytes).
    unsafe { core::ptr::copy_nonoverlapping(key, k.as_mut_ptr(), 20) };
    k
}

/// Adds or replaces the record of `key` with `typed`.
fn store_prop(link: &mut Link, key: &[u8; 20], typed: &[u8]) -> bool {
    if let Some((s, _, e)) = find_prop(link.props.as_slice(), key) {
        link.props.remove(s, e);
    }
    link.props.extend(key)
        && link.props.extend(&(typed.len() as u32).to_le_bytes())
        && link.props.extend(typed)
}

extern "system" fn ps_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    query(object(this, 3), riid, out)
}
extern "system" fn ps_add_ref(this: *mut c_void) -> u32 {
    add_ref(object(this, 3))
}
extern "system" fn ps_release(this: *mut c_void) -> u32 {
    release(object(this, 3))
}
extern "system" fn ps_get_count(this: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, records(object(this, 3).props.as_slice()).count() as u32);
    S_OK
}
extern "system" fn ps_get_at(this: *mut c_void, index: u32, out: *mut [u8; 20]) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match records(object(this, 3).props.as_slice()).nth(index as usize) {
        Some((_, key, _, _)) => {
            put(out, key);
            S_OK
        }
        None => E_INVALIDARG,
    }
}

/// A PROPVARIANT (24 bytes: vt, three reserved words, the value).
fn write_variant(pv: *mut u8, vt: u16, value: u64) {
    // SAFETY: the caller's PROPVARIANT.
    unsafe {
        core::ptr::write_bytes(pv, 0, 24);
        (pv as *mut u16).write_unaligned(vt);
        (pv.add(8) as *mut u64).write_unaligned(value);
    }
}

/// A copy of `bytes` from the task allocator (the process heap on gala).
fn task_copy(bytes: &[u8]) -> *mut u8 {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, bytes.len().max(1)) };
    if !p.is_null() {
        // SAFETY: a block of that size.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    }
    p
}

extern "system" fn ps_get_value(this: *mut c_void, key: *const u8, pv: *mut u8) -> HResult {
    if key.is_null() || pv.is_null() {
        return E_POINTER;
    }
    let link = object(this, 3);
    let props = link.props.as_slice();
    let Some((_, a, b)) = find_prop(props, &key_at(key)) else {
        write_variant(pv, VT_EMPTY, 0);
        return S_OK;
    };
    let typed = &props[a..b];
    let vt = rd16(typed, 0).unwrap_or(0);
    let v = &typed[4.min(typed.len())..];
    let num = |n: usize| {
        v.get(..n).map(|s| s.iter().rev().fold(0u64, |acc, &x| (acc << 8) | u64::from(x)))
    };
    match vt {
        VT_I4 | VT_UI4 | VT_BOOL | VT_I8 | VT_UI8 | VT_FILETIME => {
            let n = match vt {
                VT_BOOL => 2,
                VT_I4 | VT_UI4 => 4,
                _ => 8,
            };
            write_variant(pv, vt, num(n).unwrap_or(0));
        }
        VT_LPWSTR => {
            let chars = rd32(v, 0).unwrap_or(0) as usize;
            let Some(text) = v.get(4..4 + 2 * chars) else { return E_FAIL };
            let p = task_copy(text);
            if p.is_null() {
                return E_OUTOFMEMORY;
            }
            if chars > 0 {
                // SAFETY: the copy's last character is its NUL.
                unsafe { (p.add(2 * chars - 2) as *mut u16).write_unaligned(0) };
            }
            write_variant(pv, VT_LPWSTR, p as u64);
        }
        VT_CLSID => {
            let Some(g) = v.get(..16) else { return E_FAIL };
            let p = task_copy(g);
            if p.is_null() {
                return E_OUTOFMEMORY;
            }
            write_variant(pv, VT_CLSID, p as u64);
        }
        other => {
            unsupported(b"shell32.dll!ShellLink IPropertyStore::GetValue(type)", other.into());
            write_variant(pv, VT_EMPTY, 0);
        }
    }
    S_OK
}

extern "system" fn ps_set_value(this: *mut c_void, key: *const u8, pv: *const u8) -> HResult {
    if key.is_null() || pv.is_null() {
        return E_POINTER;
    }
    let link = object(this, 3);
    let key = key_at(key);
    // SAFETY: the caller's PROPVARIANT (vt at 0, the value at 8).
    let (vt, value) = unsafe {
        ((pv as *const u16).read_unaligned(), (pv.add(8) as *const u64).read_unaligned())
    };
    let mut typed = [0u8; 4 + 4 + 2 * 1024];
    typed[..2].copy_from_slice(&vt.to_le_bytes());
    let len = match vt {
        VT_EMPTY => {
            if let Some((s, _, e)) = find_prop(link.props.as_slice(), &key) {
                link.props.remove(s, e);
            }
            link.dirty = true;
            return S_OK;
        }
        VT_BOOL => {
            typed[4..6].copy_from_slice(&(value as u16).to_le_bytes());
            8
        }
        VT_I4 | VT_UI4 => {
            typed[4..8].copy_from_slice(&(value as u32).to_le_bytes());
            8
        }
        VT_I8 | VT_UI8 | VT_FILETIME => {
            typed[4..12].copy_from_slice(&value.to_le_bytes());
            12
        }
        VT_CLSID => {
            if value == 0 {
                return E_INVALIDARG;
            }
            // SAFETY: the caller's CLSID.
            unsafe {
                core::ptr::copy_nonoverlapping(value as *const u8, typed.as_mut_ptr().add(4), 16)
            };
            20
        }
        // Strings are kept as VT_LPWSTR, the shell's type for them.
        VT_LPWSTR | VT_BSTR => {
            let s = caller_str(value as *const u16);
            if s.len() >= 1024 {
                unsupported(
                    b"shell32.dll!ShellLink IPropertyStore::SetValue(string over 1023)",
                    s.len() as u64,
                );
                return E_INVALIDARG;
            }
            typed[..2].copy_from_slice(&VT_LPWSTR.to_le_bytes());
            typed[4..8].copy_from_slice(&(s.len() as u32 + 1).to_le_bytes());
            for (i, &c) in s.iter().enumerate() {
                typed[8 + 2 * i..10 + 2 * i].copy_from_slice(&c.to_le_bytes());
            }
            // The characters and their NUL, padded to four bytes.
            (8 + 2 * (s.len() + 1)).next_multiple_of(4)
        }
        other => {
            unsupported(b"shell32.dll!ShellLink IPropertyStore::SetValue(type)", other.into());
            return E_INVALIDARG;
        }
    };
    link.dirty = true;
    if store_prop(link, &key, &typed[..len]) { S_OK } else { E_OUTOFMEMORY }
}

extern "system" fn ps_commit(_this: *mut c_void) -> HResult {
    // The values are the link's own already; IPersistFile::Save writes them.
    S_OK
}

static PROPERTY_STORE_VTABLE: VTable<8> = VTable([
    ps_query as *const (),
    ps_add_ref as *const (),
    ps_release as *const (),
    ps_get_count as *const (),
    ps_get_at as *const (),
    ps_get_value as *const (),
    ps_set_value as *const (),
    ps_commit as *const (),
]);

/// FMTID of the storages whose values have string names ([MS-PROPSTORE] 2.2).
const NAMED_FMTID: Guid =
    guid(0xd5cd_d505, 0x2e9c, 0x101b, [0x93, 0x97, 0x08, 0x00, 0x2b, 0x2c, 0xf9, 0xae]);
const SPS_VERSION: u32 = 0x5350_5331;

/// Reads a PropertyStoreDataBlock's storages into the link's properties.
fn read_store(link: &mut Link, store: &[u8]) -> bool {
    link.props.clear();
    link.named_storages.clear();
    let mut at = 0;
    while let Some(size) = rd32(store, at) {
        let size = size as usize;
        if size == 0 {
            return true;
        }
        let Some(storage) = store.get(at..at + size) else { return false };
        if rd32(storage, 4) != Some(SPS_VERSION) || size < 24 {
            return false;
        }
        let fmtid = &storage[8..24];
        if fmtid == NAMED_FMTID {
            if !link.named_storages.extend(storage) {
                return false;
            }
        } else {
            let mut v = 24;
            while let Some(vsize) = rd32(storage, v) {
                let vsize = vsize as usize;
                if vsize == 0 {
                    break;
                }
                let (Some(id), Some(typed)) = (rd32(storage, v + 4), storage.get(v + 9..v + vsize))
                else {
                    return false;
                };
                let mut key = [0u8; 20];
                key[..16].copy_from_slice(fmtid);
                key[16..].copy_from_slice(&id.to_le_bytes());
                if !store_prop(link, &key, typed) {
                    return false;
                }
                v += vsize;
            }
        }
        at += size;
    }
    true
}

/// Appends the PropertyStoreDataBlock (if there are properties).
fn write_store(link: &Link, out: &mut HBuf<u8>) -> bool {
    let props = link.props.as_slice();
    let named = link.named_storages.as_slice();
    if props.is_empty() && named.is_empty() {
        return true;
    }
    let block_start = out.as_slice().len();
    let mut ok =
        out.extend(&[0; 4]) && out.extend(&PROPERTY_STORE_SIG.to_le_bytes()) && out.extend(named);
    // One storage per FMTID, in the order the FMTIDs first appear.
    for (i, (_, key, _, _)) in records(props).enumerate() {
        if records(props).take(i).any(|r| r.1[..16] == key[..16]) {
            continue;
        }
        let start = out.as_slice().len();
        ok = ok
            && out.extend(&[0; 4])
            && out.extend(&SPS_VERSION.to_le_bytes())
            && out.extend(&key[..16]);
        for (_, k, a, b) in records(props).filter(|r| r.1[..16] == key[..16]) {
            let size = (9 + b - a) as u32;
            ok = ok
                && out.extend(&size.to_le_bytes())
                && out.extend(&k[16..])
                && out.extend(&[0])
                && out.extend(&props[a..b]);
        }
        ok = ok && out.extend(&[0; 4]);
        if ok {
            let size = (out.as_slice().len() - start) as u32;
            patch32(out, start, size);
        }
    }
    ok = ok && out.extend(&[0; 4]);
    if ok {
        let size = (out.as_slice().len() - block_start) as u32;
        patch32(out, block_start, size);
    }
    ok
}

fn patch32(out: &mut HBuf<u8>, at: usize, v: u32) {
    if at + 4 <= out.len {
        // SAFETY: inside the buffer.
        unsafe { core::ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), out.ptr.add(at), 4) };
    }
}

// ---- The file ([MS-SHLLINK]) ----

const HEADER_SIZE: usize = 0x4c;

/// Reads a link file into `link`.
fn parse(link: &mut Link, b: &[u8]) -> HResult {
    if rd32(b, 0) != Some(HEADER_SIZE as u32) || b.get(4..20) != Some(&CLSID_SHELL_LINK[..]) {
        return E_FAIL;
    }
    let r32 = |at| rd32(b, at).unwrap_or(0);
    let r64 = |at| u64::from(r32(at)) | (u64::from(r32(at + 4)) << 32);
    let flags = r32(0x14);
    link.flags = flags;
    link.attributes = r32(0x18);
    link.times = [r64(0x1c), r64(0x24), r64(0x2c)];
    link.size = r32(0x34);
    link.icon_index = r32(0x38) as i32;
    link.show = match r32(0x3c) {
        s @ (1 | 3 | 7) => s as i32,
        _ => SW_SHOWNORMAL,
    };
    link.hotkey = rd16(b, 0x40).unwrap_or(0);
    for s in [
        &mut link.path,
        &mut link.raw_path,
        &mut link.description,
        &mut link.workdir,
        &mut link.args,
    ] {
        s.clear();
    }
    for s in [&mut link.icon, &mut link.raw_icon, &mut link.relative] {
        s.clear();
    }
    link.idlist.clear();
    link.extra.clear();
    link.props.clear();
    link.named_storages.clear();
    let mut at = HEADER_SIZE;
    if flags & HAS_ID_LIST != 0 {
        let Some(n) = rd16(b, at) else { return E_FAIL };
        let Some(list) = b.get(at + 2..at + 2 + usize::from(n)) else { return E_FAIL };
        if !link.idlist.set(list) {
            return E_OUTOFMEMORY;
        }
        at += 2 + usize::from(n);
    }
    if flags & HAS_LINK_INFO != 0 {
        let Some(size) = rd32(b, at) else { return E_FAIL };
        let Some(info) = b.get(at..at + size as usize) else { return E_FAIL };
        if flags & FORCE_NO_LINKINFO == 0 && !link_info_path(info, &mut link.path) {
            return E_FAIL;
        }
        at += size as usize;
    }
    let unicode = flags & UNICODE != 0;
    let fields: [(u32, Field); 5] = [
        (HAS_NAME, |l| &mut l.description),
        (HAS_RELPATH, |l| &mut l.relative),
        (HAS_WORKINGDIR, |l| &mut l.workdir),
        (HAS_ARGS, |l| &mut l.args),
        (HAS_ICONLOCATION, |l| &mut l.icon),
    ];
    for (bit, field) in fields {
        if flags & bit == 0 {
            continue;
        }
        let Some(count) = rd16(b, at) else { return E_FAIL };
        let count = usize::from(count);
        let bytes = if unicode { 2 * count } else { count };
        let Some(s) = b.get(at + 2..at + 2 + bytes) else { return E_FAIL };
        let buf = field(link);
        buf.clear();
        for i in 0..count {
            let c =
                if unicode { u16::from_le_bytes([s[2 * i], s[2 * i + 1]]) } else { s[i].into() };
            if !buf.extend(&[c]) {
                return E_OUTOFMEMORY;
            }
        }
        at += 2 + bytes;
    }
    // The extra data blocks, up to the terminal block.
    while let Some(size) = rd32(b, at) {
        let size = size as usize;
        if size < 4 {
            break;
        }
        let Some(block) = b.get(at..at + size) else { return E_FAIL };
        let sig = rd32(block, 4).unwrap_or(0);
        let ok = match sig {
            EXP_SZ_LINK_SIG => {
                flags & DISABLE_LINK_PATH_TRACKING != 0 || {
                    exp_sz_target(block, &mut link.raw_path) && {
                        // The target is the expanded path.
                        let mut expanded = [0u16; 2 * MAX_PATH];
                        let src = z::<{ 2 * MAX_PATH }>(link.raw_path.as_slice())
                            .unwrap_or([0; 2 * MAX_PATH]);
                        // SAFETY: NUL-terminated input, a buffer of its size.
                        let n = unsafe {
                            ExpandEnvironmentStringsW(
                                src.as_ptr(),
                                expanded.as_mut_ptr(),
                                2 * MAX_PATH as u32,
                            )
                        };
                        n > 0
                            && (n as usize) <= expanded.len()
                            && link.path.set(&expanded[..n as usize - 1])
                    }
                }
            }
            EXP_SZ_ICON_SIG => exp_sz_target(block, &mut link.raw_icon),
            PROPERTY_STORE_SIG => read_store(link, &block[8.min(block.len())..]),
            _ => link.extra.extend(block),
        };
        if !ok {
            return E_FAIL;
        }
        at += size;
    }
    if link.path.as_slice().is_empty() && !link.idlist.as_slice().is_empty() {
        unsupported(b"shell32.dll!IPersistFile::Load(target only as an IDList)", 0);
    }
    S_OK
}

/// The target path of a LinkInfo: the local base path and the common
/// suffix, or the network share's name and the suffix.
fn link_info_path(info: &[u8], out: &mut HBuf<u16>) -> bool {
    let r = |at| rd32(info, at).unwrap_or(0) as usize;
    let header = r(4);
    let flags = r(8);
    let ansi = |at: usize, out: &mut HBuf<u16>| -> bool {
        let mut i = at;
        while let Some(&c) = info.get(i) {
            if c == 0 {
                return true;
            }
            if !out.extend(&[c.into()]) {
                return false;
            }
            i += 1;
        }
        false
    };
    let wide = |at: usize, out: &mut HBuf<u16>| -> bool {
        let mut i = at;
        while let Some(c) = rd16(info, i) {
            if c == 0 {
                return true;
            }
            if !out.extend(&[c]) {
                return false;
            }
            i += 2;
        }
        false
    };
    out.clear();
    let unicode = header >= 0x24;
    let base_ok = if flags & 1 != 0 {
        if unicode && r(0x1c) != 0 { wide(r(0x1c), out) } else { ansi(r(0x10), out) }
    } else if flags & 2 != 0 {
        // CommonNetworkRelativeLink: its NetName (Unicode when offered).
        let net = r(0x14);
        let name_at = rd32(info, net + 8).unwrap_or(0) as usize;
        let ok = if name_at > 0x14 {
            wide(net + rd32(info, net + 0x14).unwrap_or(0) as usize, out)
        } else {
            ansi(net + name_at, out)
        };
        ok && (out.as_slice().is_empty() || out.extend(&[0x5c]))
    } else {
        true
    };
    if !base_ok {
        return false;
    }
    let start = out.as_slice().len();
    let ok = if unicode && r(0x20) != 0 { wide(r(0x20), out) } else { ansi(r(0x18), out) };
    // A network path's separator stays only before a suffix.
    if ok && flags & 2 != 0 && out.as_slice().len() == start && start > 0 {
        out.len -= 1;
    }
    ok
}

/// Appends a StringData (count, then UTF-16).
fn string_data(out: &mut HBuf<u8>, s: &[u16]) -> bool {
    let n = s.len().min(0xffff);
    if !out.extend(&(n as u16).to_le_bytes()) {
        return false;
    }
    s[..n].iter().all(|c| out.extend(&c.to_le_bytes()))
}

/// The LinkInfo of a drive or UNC path (None: the path is neither).
fn write_link_info(path: &[u16], out: &mut HBuf<u8>) -> Option<bool> {
    let ascii = path.iter().all(|&c| c < 0x80);
    let ansi_of = |s: &[u16], out: &mut HBuf<u8>| {
        s.iter().all(|&c| out.extend(&[if c < 0x80 { c as u8 } else { b'?' }])) && out.extend(&[0])
    };
    let wide_of = |s: &[u16], out: &mut HBuf<u8>| {
        s.iter().all(|&c| out.extend(&c.to_le_bytes())) && out.extend(&[0, 0])
    };
    let header: usize = if ascii { 0x1c } else { 0x24 };
    let start = out.as_slice().len();
    let drive = path.len() >= 3 && is(path[1], b':') && is(path[2], b'\\');
    let unc = path.len() > 2 && is(path[0], b'\\') && is(path[1], b'\\') && !is(path[2], b'?');
    if !drive && !unc {
        return None;
    }
    let mut ok = out.extend(&[0u8; 0x24][..header]);
    let rel = |out: &HBuf<u8>| (out.as_slice().len() - start) as u32;
    let mut fields = [0u32; 9]; // size, header, flags, volume, base, net, suffix, base U, suffix U
    fields[1] = header as u32;
    if drive {
        fields[2] = 1;
        let root = [path[0], 0x3a, 0x5c, 0];
        // SAFETY: a NUL-terminated root.
        let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
        let mut label = [0u16; MAX_PATH + 1];
        let mut serial = 0u32;
        // SAFETY: NUL-terminated root, buffers of the sizes given.
        let have = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                0,
            )
        } != 0;
        let label_n = if have { label.iter().position(|&c| c == 0).unwrap_or(0) } else { 0 };
        let label = &label[..label_n];
        fields[3] = rel(out);
        let vol = out.as_slice().len();
        let label_ascii = label.iter().all(|&c| c < 0x80);
        ok = ok
            && out.extend(&[0; 4])
            && out.extend(&kind.to_le_bytes())
            && out.extend(&serial.to_le_bytes());
        if label_ascii {
            ok = ok && out.extend(&0x10u32.to_le_bytes()) && ansi_of(label, out);
        } else {
            ok = ok
                && out.extend(&0x14u32.to_le_bytes())
                && out.extend(&0x14u32.to_le_bytes())
                && wide_of(label, out);
        }
        let vol_size = (out.as_slice().len() - vol) as u32;
        patch32(out, vol, vol_size);
        fields[4] = rel(out);
        ok = ok && ansi_of(path, out);
        fields[6] = rel(out);
        ok = ok && out.extend(&[0]);
        if !ascii {
            fields[7] = rel(out);
            ok = ok && wide_of(path, out);
            fields[8] = rel(out);
            ok = ok && out.extend(&[0, 0]);
        }
    } else {
        // \\server\share, then the rest as the common path suffix.
        let share_end = path[2..]
            .iter()
            .enumerate()
            .filter(|(_, c)| is(**c, b'\\'))
            .nth(1)
            .map_or(path.len(), |(i, _)| i + 2);
        let (share, rest) = (&path[..share_end], path.get(share_end + 1..).unwrap_or(&[]));
        fields[2] = 2;
        fields[5] = rel(out);
        let net = out.as_slice().len();
        // Size, flags (no device, no provider type), NetNameOffset,
        // DeviceNameOffset, NetworkProviderType, then the Unicode offsets
        // when the names are not ASCII.
        let name_at: u32 = if ascii { 0x14 } else { 0x1c };
        ok = ok && out.extend(&[0; 8]) && out.extend(&name_at.to_le_bytes()) && out.extend(&[0; 8]);
        if !ascii {
            ok = ok && out.extend(&[0; 8]);
        }
        ok = ok && ansi_of(share, out);
        if !ascii {
            let at = (out.as_slice().len() - net) as u32;
            patch32(out, net + 0x14, at);
            ok = ok && wide_of(share, out);
        }
        let net_size = (out.as_slice().len() - net) as u32;
        patch32(out, net, net_size);
        fields[6] = rel(out);
        ok = ok && ansi_of(rest, out);
        if !ascii {
            fields[8] = rel(out);
            ok = ok && wide_of(rest, out);
        }
    }
    fields[0] = rel(out);
    for (i, f) in fields.iter().enumerate().take(header / 4) {
        patch32(out, start + 4 * i, *f);
    }
    Some(ok)
}

/// The link file of `link`.
fn serialize(link: &Link, out: &mut HBuf<u8>) -> bool {
    let path = link.path.as_slice();
    let mut flags = (link.flags & !STRUCTURE) | UNICODE;
    let exp = link.flags & HAS_EXP_SZ != 0 && !link.raw_path.as_slice().is_empty();
    let exp_icon = link.flags & HAS_EXP_ICON_SZ != 0 && !link.raw_icon.as_slice().is_empty();
    if !exp {
        flags &= !HAS_EXP_SZ;
    }
    if !exp_icon {
        flags &= !HAS_EXP_ICON_SZ;
    }
    let idlist = link.idlist.as_slice();
    if !idlist.is_empty() {
        flags |= HAS_ID_LIST;
    }
    let mut info: HBuf<u8> = HBuf::new();
    if !path.is_empty() && link.flags & FORCE_NO_LINKINFO == 0 {
        match write_link_info(path, &mut info) {
            Some(true) => flags |= HAS_LINK_INFO,
            Some(false) => {
                info.free();
                return false;
            }
            None => {
                info.clear();
                unsupported(
                    b"shell32.dll!IPersistFile::Save(target neither a drive nor a UNC path)",
                    0,
                );
            }
        }
    }
    for (s, bit) in [
        (&link.description, HAS_NAME),
        (&link.relative, HAS_RELPATH),
        (&link.workdir, HAS_WORKINGDIR),
        (&link.args, HAS_ARGS),
        (&link.icon, HAS_ICONLOCATION),
    ] {
        if !s.as_slice().is_empty() {
            flags |= bit;
        }
    }
    let mut header = [0u8; HEADER_SIZE];
    header[..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    header[4..20].copy_from_slice(&CLSID_SHELL_LINK);
    header[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
    if !path.is_empty() {
        header[0x18..0x1c].copy_from_slice(&link.attributes.to_le_bytes());
        for (k, t) in link.times.iter().enumerate() {
            header[0x1c + 8 * k..0x24 + 8 * k].copy_from_slice(&t.to_le_bytes());
        }
        header[0x34..0x38].copy_from_slice(&link.size.to_le_bytes());
    }
    header[0x38..0x3c].copy_from_slice(&link.icon_index.to_le_bytes());
    header[0x3c..0x40].copy_from_slice(&link.show.to_le_bytes());
    header[0x40..0x42].copy_from_slice(&link.hotkey.to_le_bytes());
    let mut ok = out.extend(&header);
    if !idlist.is_empty() {
        ok = ok && out.extend(&(idlist.len() as u16).to_le_bytes()) && out.extend(idlist);
    }
    ok = ok && out.extend(info.as_slice());
    info.free();
    for s in [&link.description, &link.relative, &link.workdir, &link.args, &link.icon] {
        if !s.as_slice().is_empty() {
            ok = ok && string_data(out, s.as_slice());
        }
    }
    for (on, sig, raw) in
        [(exp, EXP_SZ_LINK_SIG, &link.raw_path), (exp_icon, EXP_SZ_ICON_SIG, &link.raw_icon)]
    {
        if on {
            let mut block = exp_sz(sig, raw.as_slice());
            ok = ok && !block.as_slice().is_empty() && out.extend(block.as_slice());
            block.free();
        }
    }
    ok = ok && out.extend(link.extra.as_slice());
    ok = ok && write_store(link, out);
    ok && out.extend(&[0; 4])
}

// ---- The class object ----

pub(crate) extern "system" fn factory_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid(riid) {
        IID_IUNKNOWN | IID_ICLASSFACTORY => {
            put(out, this);
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

/// The class object is static: its references are not counted.
pub(crate) extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}
pub(crate) extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

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
    let Some(link) = new_link() else { return E_OUTOFMEMORY };
    // SAFETY: the new object, one reference (ours).
    let link = unsafe { &mut *link };
    let r = query(link, riid, out);
    release(link);
    r
}

pub(crate) extern "system" fn factory_lock(_this: *mut c_void, _lock: i32) -> HResult {
    S_OK
}

static FACTORY_VTABLE: VTable<5> = VTable([
    factory_query as *const (),
    factory_add_ref as *const (),
    factory_release as *const (),
    factory_create as *const (),
    factory_lock as *const (),
]);

#[repr(C)]
struct Factory {
    vtable: *const VTable<5>,
}
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Factory {}
static FACTORY: Factory = Factory { vtable: &FACTORY_VTABLE };

/// DllGetClassObject: the class objects of CLSID_ShellLink and
/// CLSID_KnownFolderManager (other classes are recorded:
/// CLASS_E_CLASSNOTAVAILABLE).
#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(
    clsid: *const Guid,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let c = iid(clsid);
    if c == crate::knownfolder::CLSID_KNOWN_FOLDER_MANAGER {
        let factory = &crate::knownfolder::FACTORY as *const crate::knownfolder::Factory;
        return factory_query(factory as *mut c_void, riid, out);
    }
    if c != CLSID_SHELL_LINK {
        let d1 = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        unsupported(b"shell32.dll!DllGetClassObject(class)", d1.into());
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    factory_query(&FACTORY as *const Factory as *mut c_void, riid, out)
}

/// DllCanUnloadNow: S_FALSE (gala does not unload shell32).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    S_FALSE
}

/// Makes and saves a link to `target` at `file` (SHAddToRecentDocs);
/// the HRESULT.
pub fn save_link(target: &[u16], file: &[u16]) -> HResult {
    let Some(link) = new_link() else { return E_OUTOFMEMORY };
    // SAFETY: the new object, one reference (ours).
    let link = unsafe { &mut *link };
    let mut r = link.set_path(target);
    if r == S_OK {
        match z::<{ 2 * MAX_PATH }>(file) {
            Some(name) => {
                r = pf_save(
                    (link as *mut Link).cast::<*const ()>().wrapping_add(1).cast(),
                    name.as_ptr(),
                    1,
                )
            }
            None => r = E_INVALIDARG,
        }
    }
    release(link);
    r
}

// ---- Recent documents ----

use super::shellops::FindData;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(h: usize, data: *mut FindData) -> i32;
    fn FindClose(h: usize) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
}

/// `parts` joined into a NUL-terminated path (None if too long).
fn join(parts: &[&[u16]]) -> Option<[u16; 2 * MAX_PATH]> {
    let mut out = [0u16; 2 * MAX_PATH];
    let mut n = 0;
    for p in parts {
        out.get_mut(n..n + p.len())?.copy_from_slice(p);
        n += p.len();
    }
    (n < out.len()).then_some(out)
}

fn ascii<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, &b) in w.iter_mut().zip(s) {
        *d = b.into();
    }
    w
}

/// SHAddToRecentDocs: a link to the document (SHARD_PIDL, SHARD_PATHA,
/// SHARD_PATHW) in the user's Recent folder, named after the file with
/// ".lnk" added; NULL clears the folder's links. The other forms
/// (application ids, IShellItem, IShellLink) are recorded. The policies
/// that turn the list off (NoRecentDocsHistory, Start_TrackDocs) are not
/// read: gala keeps the list.
#[unsafe(no_mangle)]
pub extern "system" fn SHAddToRecentDocs(flags: u32, pv: *const c_void) {
    const SHARD_PIDL: u32 = 1;
    const SHARD_PATHA: u32 = 2;
    const SHARD_PATHW: u32 = 3;
    const CSIDL_RECENT_CREATE: i32 = 0x08 | 0x8000;
    let mut recent = [0u16; MAX_PATH];
    if super::SHGetFolderPathW(0, CSIDL_RECENT_CREATE, 0, 0, recent.as_mut_ptr()) != S_OK {
        unsupported(b"shell32.dll!SHAddToRecentDocs(no Recent folder)", 0);
        return;
    }
    let rn = recent.iter().position(|&c| c == 0).unwrap_or(0);
    let recent = &recent[..rn];
    let sep = [0x5cu16];
    if pv.is_null() {
        let Some(pattern) = join(&[recent, &sep, &ascii(b"*.lnk")]) else { return };
        // SAFETY: zeroed is a valid FindData.
        let mut fd: FindData = unsafe { core::mem::zeroed() };
        // SAFETY: a NUL-terminated pattern, our FindData.
        let h = unsafe { FindFirstFileW(pattern.as_ptr(), &mut fd) };
        if h == INVALID_HANDLE {
            return;
        }
        loop {
            let n = fd.name.iter().position(|&c| c == 0).unwrap_or(0);
            if let Some(file) = join(&[recent, &sep, &fd.name[..n]]) {
                // SAFETY: a NUL-terminated path.
                unsafe { DeleteFileW(file.as_ptr()) };
            }
            // SAFETY: our search handle and FindData.
            if unsafe { FindNextFileW(h, &mut fd) } == 0 {
                break;
            }
        }
        // SAFETY: our search handle.
        unsafe { FindClose(h) };
        return;
    }
    let mut target = [0u16; MAX_PATH];
    let n = match flags & 0xff {
        SHARD_PATHW => {
            let s = caller_str(pv.cast());
            if s.len() >= MAX_PATH {
                return;
            }
            target[..s.len()].copy_from_slice(s);
            s.len()
        }
        SHARD_PATHA => {
            // SAFETY: the caller's NUL-terminated string, our buffer.
            let n = unsafe {
                MultiByteToWideChar(0, 0, pv.cast(), -1, target.as_mut_ptr(), MAX_PATH as i32)
            };
            (n as usize).saturating_sub(1)
        }
        SHARD_PIDL => {
            if super::namespace::SHGetPathFromIDListW(pv.cast(), target.as_mut_ptr()) == 0 {
                unsupported(b"shell32.dll!SHAddToRecentDocs(not a file system item)", 0);
                return;
            }
            target.iter().position(|&c| c == 0).unwrap_or(0)
        }
        other => {
            unsupported(b"shell32.dll!SHAddToRecentDocs(flags)", other.into());
            return;
        }
    };
    let target = &target[..n];
    let start = target.iter().rposition(|&c| is(c, b'\\') || is(c, b'/')).map_or(0, |i| i + 1);
    let name = &target[start..];
    if name.is_empty() {
        return;
    }
    let Some(file) = join(&[recent, &sep, name, &ascii(b".lnk")]) else { return };
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    let r = save_link(target, &file[..len]);
    if r != S_OK {
        unsupported(b"shell32.dll!SHAddToRecentDocs(saving the link failed)", r as u32 as u64);
    }
}

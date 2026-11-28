//! Compound files (MS-CFB), read-only: StgOpenStorage and
//! StgIsStorageFile over a file on disk, with the IStorage, IStream and
//! IEnumSTATSTG objects they hand out. Office's Mso20win32client
//! delay-loads StgOpenStorage and WWLIB calls it on start-up.
//!
//! The format: 512-byte sectors (4096 in version 4), a FAT of sector
//! chains reached through the DIFAT, 128-byte directory entries in a
//! tree, and a mini FAT of 64-byte sectors inside the root entry's
//! stream for everything below the header's cutoff. A stream's size
//! alone decides which chain it lives in: at or above the cutoff the
//! FAT, below it the mini FAT.
//!
//! What `stg_test` measures on Windows 11 and this builds:
//!
//! - StgOpenStorage works before CoInitialize; the mode is checked
//!   hard (a share mode is required, write access demands
//!   STGM_SHARE_EXCLUSIVE, STGM_DELETEONRELEASE is
//!   STG_E_INVALIDFUNCTION and an unknown bit STG_E_INVALIDFLAG), and
//!   a non-zero `reserved` leaves the out pointer untouched where
//!   every other refusal writes NULL through it.
//! - A file that is not a compound file, or is too short for the
//!   header, is STG_E_FILEALREADYEXISTS; a missing one
//!   STG_E_FILENOTFOUND; a directory STG_E_ACCESSDENIED; a missing
//!   parent, a trailing separator and the empty name
//!   STG_E_PATHNOTFOUND. These are the file system's own errors
//!   mapped into the STG_E_ facility, so gala's answers follow its
//!   CreateFileW.
//! - The root storage's Stat gives the file's full path (whatever name
//!   it was opened by), STGTY_STORAGE (1, not the directory entry's
//!   5), size 0, the mode it was opened with and the file's times;
//!   an element's Stat gives the bare entry name and the directory
//!   entry's times, which are zero in a file nothing has set them in.
//! - Names fold case-insensitively (RtlUpcaseUnicodeChar) but are
//!   neither trimmed nor prefix-matched, and the directory tree is
//!   ordered by name length first, so enumeration is in the format's
//!   order and not alphabetical.
//! - An element may be open once: a second open of it is
//!   STG_E_ACCESSDENIED until the first is released (a Clone shares
//!   the hold). A child open takes STGM_READ with
//!   STGM_SHARE_EXCLUSIVE; a deny-write share is
//!   STG_E_INVALIDFUNCTION and anything else STG_E_INVALIDFLAG.
//! - Reads stop at the end of the stream with S_OK and a short count,
//!   Seek may go past it, and a negative result or an unknown origin
//!   is STG_E_INVALIDFUNCTION.
//!
//! Read-only, so every call that would change a compound file is
//! STG_E_ACCESSDENIED with its own ledger row (Windows answers that
//! for a storage opened for reading; what gala lacks is the
//! write-capable open behind it, which hands out a read-only storage
//! and records that it did). The other recorded gaps: IStorage::CopyTo
//! and MoveElementTo, the IPropertySetStorage, IRootStorage and
//! IMarshal interfaces Windows' storage answers, an exclusion list
//! (refused), and a priority storage (ignored).

use core::cmp::Ordering as Order;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, Guid, HResult, IID_IUNKNOWN, S_FALSE, S_OK, VTable,
    alloc, com_call, free, missing, ole_iid, put, reached,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(
        file: usize,
        buffer: *mut u8,
        length: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn SetFilePointerEx(file: usize, distance: i64, new: *mut i64, method: u32) -> i32;
    fn GetFileSizeEx(file: usize, size: *mut i64) -> i32;
    fn GetFileTime(file: usize, created: *mut u64, accessed: *mut u64, written: *mut u64) -> i32;
    fn GetFullPathNameW(path: *const u16, size: u32, buf: *mut u16, part: *mut *mut u16) -> u32;
    fn CloseHandle(file: usize) -> i32;
    fn GetLastError() -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlUpcaseUnicodeChar(c: u16) -> u16;
}

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE: usize = usize::MAX;

const STG_E_INVALIDFUNCTION: HResult = 0x8003_0001u32 as i32;
const STG_E_FILENOTFOUND: HResult = 0x8003_0002u32 as i32;
const STG_E_ACCESSDENIED: HResult = 0x8003_0005u32 as i32;
const STG_E_INVALIDPOINTER: HResult = 0x8003_0009u32 as i32;
const STG_E_FILEALREADYEXISTS: HResult = 0x8003_0050u32 as i32;
const STG_E_INVALIDPARAMETER: HResult = 0x8003_0057u32 as i32;
const STG_E_INVALIDNAME: HResult = 0x8003_00fcu32 as i32;
const STG_E_INVALIDFLAG: HResult = 0x8003_00ffu32 as i32;
const STG_E_DOCFILECORRUPT: HResult = 0x8003_0109u32 as i32;

const STGM_ACCESS: u32 = 0x3;
const STGM_SHARE: u32 = 0x70;
const STGM_SHARE_EXCLUSIVE: u32 = 0x10;
const STGM_SHARE_DENY_WRITE: u32 = 0x20;
const STGM_DELETEONRELEASE: u32 = 0x400_0000;
const STGM_TRANSACTED: u32 = 0x1_0000;
const STGM_DIRECT_SWMR: u32 = 0x40_0000;
/// Flags Windows' storage takes and this does not build: STGM_SIMPLE,
/// STGM_PRIORITY, STGM_NOSCRATCH and STGM_NOSNAPSHOT. They are refused
/// like any unknown bit, with a row of their own so a program asking
/// for one is named.
const STGM_NOT_BUILT: u32 = 0x800_0000 | 0x4_0000 | 0x10_0000 | 0x20_0000;

const STGTY_STORAGE: u32 = 1;
const STGTY_STREAM: u32 = 2;
const STATFLAG_NONAME: u32 = 1;

const IID_ISTORAGE: Guid = ole_iid(0xb);
const IID_ISTREAM: Guid = ole_iid(0xc);
const IID_IENUMSTATSTG: Guid = ole_iid(0xd);
const IID_IROOTSTORAGE: Guid = ole_iid(0x12);
const IID_IMARSHAL: Guid = ole_iid(0x3);
const IID_IPROPERTYSETSTORAGE: Guid = ole_iid(0x13a);

const SIGNATURE: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
/// The last sector number that is a sector; above it are the special
/// values (DIFSECT, FATSECT, ENDOFCHAIN, FREESECT) and NOSTREAM.
const MAXREGSECT: u32 = 0xffff_fffa;
const NOSTREAM: u32 = 0xffff_ffff;
/// A directory entry's size.
const ENTRY: usize = 128;
/// The longest path this walks down the directory tree, and the
/// deepest its enumeration stack goes: a bound against a cycle in a
/// malformed file.
const MAX_DEPTH: usize = 64;
/// Elements one storage can hold open at a time.
const MAX_HELD: usize = 64;
/// The largest sector this reads (version 4's).
const MAX_SECTOR: usize = 4096;

// STATSTG's fields: a pointer, a DWORD, then members aligned to 8, so
// x86 and x64 differ by the padding after the type.
const PTR: usize = core::mem::size_of::<usize>();
const ST_TYPE: usize = PTR;
const ST_CBSIZE: usize = if PTR == 8 { 16 } else { 8 };
const ST_MTIME: usize = ST_CBSIZE + 8;
const ST_CTIME: usize = ST_CBSIZE + 16;
const ST_ATIME: usize = ST_CBSIZE + 24;
const ST_MODE: usize = ST_CBSIZE + 32;
const ST_CLSID: usize = ST_MODE + 8;
const ST_STATE: usize = ST_CLSID + 16;
const STATSTG_SIZE: usize = ST_STATE + 8;

/// A file system error in the STG_E_ facility, the way the storage
/// errors for a file that is missing, held or not reachable are laid
/// out (STG_E_FILENOTFOUND is ERROR_FILE_NOT_FOUND, and so on).
fn from_win32(err: u32) -> HResult {
    if err == 0 {
        return STG_E_ACCESSDENIED;
    }
    (0x8003_0000 | (err & 0xffff)) as i32
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

/// Reads `len` bytes at `offset` into `buf`; false on a short read or
/// any failure (a truncated or corrupt file).
fn read_at(file: usize, offset: i64, buf: *mut u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    // SAFETY: the document's handle and a buffer of `len` bytes.
    unsafe {
        if SetFilePointerEx(file, offset, core::ptr::null_mut(), 0) == 0 {
            return false;
        }
        let mut at = 0usize;
        while at < len {
            let mut done = 0u32;
            let want = (len - at).min(u32::MAX as usize) as u32;
            if ReadFile(file, buf.add(at), want, &mut done, core::ptr::null_mut()) == 0 {
                return false;
            }
            if done == 0 {
                return false;
            }
            at += done as usize;
        }
    }
    true
}

/// Case folding for a directory entry's name, as the format's tree is
/// ordered and looked up.
fn fold(c: u16) -> u16 {
    // SAFETY: a plain call.
    unsafe { RtlUpcaseUnicodeChar(c) }
}

/// The directory tree's order: name length first, then the folded
/// characters.
fn compare(a: &[u16], b: &[u16]) -> Order {
    match a.len().cmp(&b.len()) {
        Order::Equal => {}
        other => return other,
    }
    for i in 0..a.len() {
        let (x, y) = (fold(a[i]), fold(b[i]));
        if x != y {
            return x.cmp(&y);
        }
    }
    Order::Equal
}

/// A directory entry's name: its characters without the terminator.
fn entry_name(e: &[u8; ENTRY]) -> ([u16; 32], usize) {
    let mut out = [0u16; 32];
    let mut n = (le16(e, 0x40) as usize / 2).min(32);
    for (i, c) in out.iter_mut().enumerate().take(n) {
        *c = le16(e, i * 2);
    }
    while n > 0 && out[n - 1] == 0 {
        n -= 1;
    }
    (out, n)
}

fn entry_kind(e: &[u8; ENTRY]) -> u8 {
    e[0x42]
}

fn entry_left(e: &[u8; ENTRY]) -> u32 {
    le32(e, 0x44)
}

fn entry_right(e: &[u8; ENTRY]) -> u32 {
    le32(e, 0x48)
}

fn entry_child(e: &[u8; ENTRY]) -> u32 {
    le32(e, 0x4c)
}

fn entry_clsid(e: &[u8; ENTRY]) -> Guid {
    let mut g = [0u8; 16];
    g.copy_from_slice(&e[0x50..0x60]);
    g
}

fn entry_state(e: &[u8; ENTRY]) -> u32 {
    le32(e, 0x60)
}

/// A stream's times: the entry's creation and modification times (zero
/// in a file nothing has set them in), as Stat reports them.
fn entry_times(e: &[u8; ENTRY]) -> [u64; 3] {
    [le64(e, 0x6c), le64(e, 0x64), 0]
}

fn entry_start(e: &[u8; ENTRY]) -> u32 {
    le32(e, 0x74)
}

/// A stream's size. Version 3 writes only the low 32 bits.
fn entry_size(e: &[u8; ENTRY], sector: u32) -> u64 {
    let size = le64(e, 0x78);
    if sector == 512 { size & 0xffff_ffff } else { size }
}

/// An open compound file: the handle, the allocation tables read once,
/// and which elements are open. Every object handed out holds a
/// reference; the file is closed with the last of them.
#[repr(C)]
struct Doc {
    refs: AtomicU32,
    file: usize,
    mode: u32,
    /// The file's full path, for the root storage's Stat.
    path: *mut u16,
    path_len: usize,
    sector: u32,
    mini_sector: u32,
    cutoff: u32,
    fat: *mut u32,
    fat_len: usize,
    mini_fat: *mut u32,
    mini_fat_len: usize,
    /// The first sector of the directory chain.
    dir: u32,
    /// The root entry's stream: the mini sectors live in it.
    mini_stream: u32,
    root_clsid: Guid,
    /// The file's modification, creation and access times.
    times: [u64; 3],
    held: [(u32, u32); MAX_HELD],
    held_len: usize,
}

impl Doc {
    fn fat_at(&self, n: u32) -> u32 {
        if (n as usize) < self.fat_len {
            // SAFETY: the table this read, within its length.
            unsafe { *self.fat.add(n as usize) }
        } else {
            NOSTREAM
        }
    }

    fn mini_at(&self, n: u32) -> u32 {
        if (n as usize) < self.mini_fat_len {
            // SAFETY: the table this read, within its length.
            unsafe { *self.mini_fat.add(n as usize) }
        } else {
            NOSTREAM
        }
    }

    /// Where sector `n` starts in the file (the header is sector -1).
    fn sector_offset(&self, n: u32) -> i64 {
        (i64::from(n) + 1) * i64::from(self.sector)
    }

    /// Reads `len` bytes from `offset` within the FAT chain at `start`.
    fn read_chain(&self, start: u32, offset: u64, buf: *mut u8, len: usize) -> bool {
        let size = u64::from(self.sector);
        let mut sector = start;
        let mut skip = offset / size;
        let mut within = (offset % size) as usize;
        // A cycle in a malformed file must not spin: the chain cannot
        // be longer than the table it is in.
        let mut budget = self.fat_len as u64 + 2;
        while skip > 0 {
            if sector > MAXREGSECT || budget == 0 {
                return false;
            }
            sector = self.fat_at(sector);
            skip -= 1;
            budget -= 1;
        }
        let mut done = 0usize;
        while done < len {
            if sector > MAXREGSECT || budget == 0 {
                return false;
            }
            let n = (len - done).min(self.sector as usize - within);
            // SAFETY: a buffer of `len` bytes, filled from `done`.
            let at = unsafe { buf.add(done) };
            if !read_at(self.file, self.sector_offset(sector) + within as i64, at, n) {
                return false;
            }
            done += n;
            within = 0;
            sector = self.fat_at(sector);
            budget -= 1;
        }
        true
    }

    /// Reads from a mini FAT chain, which lives inside the root
    /// entry's stream.
    fn read_mini(&self, start: u32, offset: u64, buf: *mut u8, len: usize) -> bool {
        let size = u64::from(self.mini_sector);
        let mut sector = start;
        let mut skip = offset / size;
        let mut within = (offset % size) as usize;
        let mut budget = self.mini_fat_len as u64 + 2;
        while skip > 0 {
            if sector > MAXREGSECT || budget == 0 {
                return false;
            }
            sector = self.mini_at(sector);
            skip -= 1;
            budget -= 1;
        }
        let mut done = 0usize;
        while done < len {
            if sector > MAXREGSECT || budget == 0 {
                return false;
            }
            let n = (len - done).min(self.mini_sector as usize - within);
            let at = u64::from(sector) * size + within as u64;
            // SAFETY: a buffer of `len` bytes, filled from `done`.
            if !self.read_chain(self.mini_stream, at, unsafe { buf.add(done) }, n) {
                return false;
            }
            done += n;
            within = 0;
            sector = self.mini_at(sector);
            budget -= 1;
        }
        true
    }

    /// Reads a stream's bytes, from whichever chain its size puts it
    /// in (the header's cutoff decides, the size alone).
    fn read_stream(&self, start: u32, size: u64, offset: u64, buf: *mut u8, len: usize) -> bool {
        if size >= u64::from(self.cutoff) {
            self.read_chain(start, offset, buf, len)
        } else {
            self.read_mini(start, offset, buf, len)
        }
    }

    fn entry(&self, index: u32) -> Option<[u8; ENTRY]> {
        let mut e = [0u8; ENTRY];
        let at = u64::from(index) * ENTRY as u64;
        if self.read_chain(self.dir, at, e.as_mut_ptr(), ENTRY) { Some(e) } else { None }
    }

    /// Takes the one hold on element `index`; false when something
    /// already has it (a second open is STG_E_ACCESSDENIED).
    fn hold(&mut self, index: u32) -> bool {
        for i in 0..self.held_len {
            if self.held[i].0 == index {
                return false;
            }
        }
        if self.held_len == MAX_HELD {
            missing(b"ole32.dll!IStorage::OpenStream(too many elements open)", MAX_HELD as u64);
            return false;
        }
        self.held[self.held_len] = (index, 1);
        self.held_len += 1;
        true
    }

    /// One more object on a hold already taken (a Clone).
    fn share_hold(&mut self, index: u32) {
        for i in 0..self.held_len {
            if self.held[i].0 == index {
                self.held[i].1 += 1;
                return;
            }
        }
    }

    fn drop_hold(&mut self, index: u32) {
        for i in 0..self.held_len {
            if self.held[i].0 == index {
                self.held[i].1 -= 1;
                if self.held[i].1 == 0 {
                    self.held[i] = self.held[self.held_len - 1];
                    self.held_len -= 1;
                }
                return;
            }
        }
    }

    /// Looks `want` up in the tree under `child`.
    fn find(&self, child: u32, want: &[u16]) -> Option<(u32, [u8; ENTRY])> {
        let mut node = child;
        for _ in 0..MAX_DEPTH {
            if node > MAXREGSECT {
                return None;
            }
            let e = self.entry(node)?;
            let (name, len) = entry_name(&e);
            match compare(want, &name[..len]) {
                Order::Equal => return Some((node, e)),
                Order::Less => node = entry_left(&e),
                Order::Greater => node = entry_right(&e),
            }
        }
        None
    }

    /// Walks the tree under `child` in order, filling `out` (when
    /// given) and returning how many entries there are.
    fn walk(&self, child: u32, mut out: Option<&mut [u32]>) -> usize {
        let mut stack = [0u32; MAX_DEPTH];
        let mut sp = 0usize;
        let mut node = child;
        let mut n = 0usize;
        let mut budget = (MAX_HELD * MAX_DEPTH) as u32;
        loop {
            while node <= MAXREGSECT {
                if sp == MAX_DEPTH || budget == 0 {
                    return n;
                }
                budget -= 1;
                stack[sp] = node;
                sp += 1;
                node = match self.entry(node) {
                    Some(e) => entry_left(&e),
                    None => NOSTREAM,
                };
            }
            if sp == 0 {
                return n;
            }
            sp -= 1;
            let at = stack[sp];
            if let Some(items) = out.as_deref_mut()
                && n < items.len()
            {
                items[n] = at;
            }
            n += 1;
            if budget == 0 {
                return n;
            }
            budget -= 1;
            node = match self.entry(at) {
                Some(e) => entry_right(&e),
                None => NOSTREAM,
            };
        }
    }
}

fn doc_add_ref(doc: *mut Doc) {
    // SAFETY: a document an object of ours holds.
    unsafe { (*doc).refs.fetch_add(1, Ordering::Relaxed) };
}

/// Lets go of a document, closing the file with the last object on it.
fn doc_release(doc: *mut Doc) {
    // SAFETY: a document an object of ours holds, freed with its last
    // reference.
    unsafe {
        if (*doc).refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        CloseHandle((*doc).file);
        free((*doc).path.cast());
        free((*doc).fat.cast());
        free((*doc).mini_fat.cast());
        free(doc.cast());
    }
}

/// A copy of `chars` on the task allocator, as STATSTG's name is
/// handed out (the caller frees it with CoTaskMemFree).
fn task_name(chars: &[u16]) -> *mut u16 {
    let p = alloc((chars.len() + 1) * 2).cast::<u16>();
    if p.is_null() {
        return p;
    }
    for (i, &c) in chars.iter().enumerate() {
        // SAFETY: memory for len + 1 characters.
        unsafe { p.add(i).write(c) };
    }
    // SAFETY: as above.
    unsafe { p.add(chars.len()).write(0) };
    p
}

/// What a STATSTG says about one element: the name the caller is given
/// (NULL for STATFLAG_NONAME), what kind of element it is, and the
/// readings behind it.
struct Stat {
    name: *mut u16,
    kind: u32,
    size: u64,
    times: [u64; 3],
    mode: u32,
    clsid: Guid,
    state: u32,
}

/// Fills the caller's STATSTG.
fn write_stat(out: *mut u8, stat: &Stat) {
    let &Stat { name, kind, size, times, mode, clsid, state } = stat;
    // SAFETY: the caller's STATSTG, checked non-null.
    unsafe {
        core::ptr::write_bytes(out, 0, STATSTG_SIZE);
        out.cast::<*mut u16>().write_unaligned(name);
        out.add(ST_TYPE).cast::<u32>().write_unaligned(kind);
        out.add(ST_CBSIZE).cast::<u64>().write_unaligned(size);
        out.add(ST_MTIME).cast::<u64>().write_unaligned(times[0]);
        out.add(ST_CTIME).cast::<u64>().write_unaligned(times[1]);
        out.add(ST_ATIME).cast::<u64>().write_unaligned(times[2]);
        out.add(ST_MODE).cast::<u32>().write_unaligned(mode);
        core::ptr::copy_nonoverlapping(clsid.as_ptr(), out.add(ST_CLSID), 16);
        out.add(ST_STATE).cast::<u32>().write_unaligned(state);
    }
}

// ---- Opening the file ----

/// The mode StgOpenStorage takes.
fn open_mode(mode: u32) -> HResult {
    if mode & STGM_DELETEONRELEASE != 0 {
        return STG_E_INVALIDFUNCTION;
    }
    let known =
        STGM_ACCESS | STGM_SHARE | STGM_TRANSACTED | STGM_DIRECT_SWMR | STGM_DELETEONRELEASE;
    if mode & !known != 0 {
        if mode & STGM_NOT_BUILT != 0 {
            missing(b"ole32.dll!StgOpenStorage(flags gala has not built)", u64::from(mode));
        }
        return STG_E_INVALIDFLAG;
    }
    if mode & STGM_ACCESS == STGM_ACCESS {
        return STG_E_INVALIDFLAG;
    }
    let share = mode & STGM_SHARE;
    if share != STGM_SHARE_EXCLUSIVE && share != STGM_SHARE_DENY_WRITE {
        return STG_E_INVALIDFLAG;
    }
    // Write access is only ever granted exclusively.
    if mode & STGM_ACCESS != 0 && share != STGM_SHARE_EXCLUSIVE {
        return STG_E_INVALIDFLAG;
    }
    S_OK
}

/// The mode an element is opened with: read-only and exclusive.
fn child_mode(mode: u32) -> HResult {
    if mode & !(STGM_ACCESS | STGM_SHARE | STGM_TRANSACTED | STGM_DIRECT_SWMR) != 0 {
        return STG_E_INVALIDFLAG;
    }
    if mode & STGM_SHARE == STGM_SHARE_DENY_WRITE {
        return STG_E_INVALIDFUNCTION;
    }
    if mode & STGM_SHARE != STGM_SHARE_EXCLUSIVE {
        return STG_E_INVALIDFLAG;
    }
    if mode & STGM_ACCESS != 0 {
        return STG_E_INVALIDFLAG;
    }
    S_OK
}

/// Reads the allocation table the header's DIFAT names; None when the
/// file's own numbers do not hold together.
fn read_fat(file: usize, head: &[u8; 512], sector: u32, sectors: u64) -> Option<(*mut u32, usize)> {
    let per = sector as usize / 4;
    let count = le32(head, 0x2c) as usize;
    if count == 0 || count as u64 > sectors + 2 {
        return None;
    }
    let fat = alloc(count * per * 4).cast::<u32>();
    if fat.is_null() {
        return None;
    }
    let mut difat = [0u8; MAX_SECTOR];
    let mut difat_sector = le32(head, 0x44);
    let mut difat_at = per; // nothing loaded yet
    let mut raw = [0u8; MAX_SECTOR];
    for i in 0..count {
        let which = if i < 109 {
            le32(head, 0x4c + i * 4)
        } else {
            if difat_at >= per - 1 {
                if difat_at != per {
                    difat_sector = le32(&difat, (per - 1) * 4);
                }
                if difat_sector > MAXREGSECT {
                    free(fat.cast());
                    return None;
                }
                let at = (i64::from(difat_sector) + 1) * i64::from(sector);
                if !read_at(file, at, difat.as_mut_ptr(), sector as usize) {
                    free(fat.cast());
                    return None;
                }
                difat_at = 0;
            }
            let v = le32(&difat, difat_at * 4);
            difat_at += 1;
            v
        };
        if which > MAXREGSECT {
            free(fat.cast());
            return None;
        }
        let at = (i64::from(which) + 1) * i64::from(sector);
        if !read_at(file, at, raw.as_mut_ptr(), sector as usize) {
            free(fat.cast());
            return None;
        }
        for j in 0..per {
            // SAFETY: the table just allocated, within count * per.
            unsafe { fat.add(i * per + j).write(le32(&raw, j * 4)) };
        }
    }
    Some((fat, count * per))
}

/// Reads the mini FAT, whose sectors are a chain in the FAT.
fn read_mini_fat(
    file: usize,
    head: &[u8; 512],
    sector: u32,
    fat: *mut u32,
    fat_len: usize,
    sectors: u64,
) -> Option<(*mut u32, usize)> {
    let per = sector as usize / 4;
    let count = le32(head, 0x40) as usize;
    if count == 0 {
        return Some((core::ptr::null_mut(), 0));
    }
    if count as u64 > sectors + 2 {
        return None;
    }
    let mini = alloc(count * per * 4).cast::<u32>();
    if mini.is_null() {
        return None;
    }
    let mut raw = [0u8; MAX_SECTOR];
    let mut which = le32(head, 0x3c);
    for i in 0..count {
        if which > MAXREGSECT {
            free(mini.cast());
            return None;
        }
        let at = (i64::from(which) + 1) * i64::from(sector);
        if !read_at(file, at, raw.as_mut_ptr(), sector as usize) {
            free(mini.cast());
            return None;
        }
        for j in 0..per {
            // SAFETY: the table just allocated, within count * per.
            unsafe { mini.add(i * per + j).write(le32(&raw, j * 4)) };
        }
        which = if (which as usize) < fat_len {
            // SAFETY: the FAT this read, within its length.
            unsafe { *fat.add(which as usize) }
        } else {
            NOSTREAM
        };
    }
    Some((mini, count * per))
}

/// Opens `name` as a compound file: the handle, the allocation tables
/// and the root entry, or the error the caller gets.
fn open_doc(name: *const u16, mode: u32) -> Result<*mut Doc, HResult> {
    let share = if mode & STGM_SHARE == STGM_SHARE_EXCLUSIVE { 0 } else { FILE_SHARE_READ };
    // SAFETY: the caller's name, checked non-null.
    let file = unsafe {
        CreateFileW(name, GENERIC_READ, share, core::ptr::null_mut(), OPEN_EXISTING, 0, 0)
    };
    if file == INVALID_HANDLE {
        // SAFETY: a plain call.
        return Err(from_win32(unsafe { GetLastError() }));
    }
    let close = |file: usize, err: HResult| -> Result<*mut Doc, HResult> {
        // SAFETY: the handle just opened.
        unsafe { CloseHandle(file) };
        Err(err)
    };
    let mut head = [0u8; 512];
    if !read_at(file, 0, head.as_mut_ptr(), 512) {
        // Too short for a header: not a compound file.
        return close(file, STG_E_FILEALREADYEXISTS);
    }
    if head[..8] != SIGNATURE || le16(&head, 0x1c) != 0xfffe {
        return close(file, STG_E_FILEALREADYEXISTS);
    }
    let shift = le16(&head, 0x1e);
    if (shift != 9 && shift != 12) || le16(&head, 0x20) != 6 {
        return close(file, STG_E_FILEALREADYEXISTS);
    }
    let sector = 1u32 << shift;
    let mut size = 0i64;
    // SAFETY: the handle just opened.
    if unsafe { GetFileSizeEx(file, &mut size) } == 0 {
        // SAFETY: a plain call.
        let err = unsafe { GetLastError() };
        return close(file, from_win32(err));
    }
    let sectors = size.max(0) as u64 / u64::from(sector);
    let Some((fat, fat_len)) = read_fat(file, &head, sector, sectors) else {
        missing(b"ole32.dll!StgOpenStorage(corrupt allocation table)", u64::from(mode));
        return close(file, STG_E_DOCFILECORRUPT);
    };
    let mini = read_mini_fat(file, &head, sector, fat, fat_len, sectors);
    let Some((mini_fat, mini_fat_len)) = mini else {
        free(fat.cast());
        missing(b"ole32.dll!StgOpenStorage(corrupt mini allocation table)", u64::from(mode));
        return close(file, STG_E_DOCFILECORRUPT);
    };
    let mut times = [0u64; 3];
    // SAFETY: the handle just opened; the file's three times.
    unsafe { GetFileTime(file, &mut times[1], &mut times[2], &mut times[0]) };
    // The name Stat gives is the file's full path, whatever name the
    // caller opened it by.
    // SAFETY: the caller's name; the length asked for first.
    let (path, path_len) = unsafe {
        let need = GetFullPathNameW(name, 0, core::ptr::null_mut(), core::ptr::null_mut());
        let p = alloc((need as usize + 1) * 2).cast::<u16>();
        if p.is_null() || need == 0 {
            (p, 0)
        } else {
            let got = GetFullPathNameW(name, need, p, core::ptr::null_mut());
            (p, got as usize)
        }
    };
    let doc = alloc(core::mem::size_of::<Doc>()).cast::<Doc>();
    if doc.is_null() {
        free(path.cast());
        free(fat.cast());
        free(mini_fat.cast());
        return close(file, E_OUTOFMEMORY);
    }
    // SAFETY: fresh memory of the document's size.
    unsafe {
        doc.write(Doc {
            refs: AtomicU32::new(1),
            file,
            mode,
            path,
            path_len,
            sector,
            mini_sector: 1 << le16(&head, 0x20),
            cutoff: le32(&head, 0x38),
            fat,
            fat_len,
            mini_fat,
            mini_fat_len,
            dir: le32(&head, 0x30),
            mini_stream: 0,
            root_clsid: [0; 16],
            times,
            held: [(0, 0); MAX_HELD],
            held_len: 0,
        });
    }
    // SAFETY: the document just built; the root entry names the mini
    // stream and carries the storage's class.
    let root = unsafe { (*doc).entry(0) };
    match root {
        Some(e) if entry_kind(&e) == 5 => {
            // SAFETY: as above.
            unsafe {
                (*doc).mini_stream = entry_start(&e);
                (*doc).root_clsid = entry_clsid(&e);
            }
            Ok(doc)
        }
        _ => {
            doc_release(doc);
            missing(b"ole32.dll!StgOpenStorage(no root entry)", u64::from(mode));
            Err(STG_E_DOCFILECORRUPT)
        }
    }
}

// ---- IStorage ----

#[repr(C)]
struct Storage {
    vtable: *const usize,
    refs: AtomicU32,
    doc: *mut Doc,
    /// The directory entry this storage is (0 for the root).
    entry: u32,
    /// The root of its children's tree.
    child: u32,
    mode: u32,
    root: bool,
    name: [u16; 32],
    name_len: usize,
    clsid: Guid,
    times: [u64; 3],
}

/// What a storage object keeps of its directory entry: the entry
/// itself, the root of its children's tree, and the readings its Stat
/// answers from (all zero for the root, whose Stat reads the file).
struct StorageEntry {
    entry: u32,
    child: u32,
    name: [u16; 32],
    name_len: usize,
    clsid: Guid,
    times: [u64; 3],
}

fn new_storage(doc: *mut Doc, of: StorageEntry, mode: u32, root: bool) -> *mut Storage {
    let StorageEntry { entry, child, name, name_len, clsid, times } = of;
    let p = alloc(core::mem::size_of::<Storage>()).cast::<Storage>();
    if p.is_null() {
        return p;
    }
    doc_add_ref(doc);
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Storage {
            vtable: STORAGE_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            doc,
            entry,
            child,
            mode,
            root,
            name,
            name_len,
            clsid,
            times,
        });
    }
    p
}

extern "system" fn stg_query(
    this: *mut Storage,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    match iid {
        IID_IUNKNOWN | IID_ISTORAGE => {
            stg_add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            // Windows' own storage answers these three; gala's does
            // not, so a program that wants one is named here instead
            // of failing somewhere else.
            let gap: Option<&[u8]> = match iid {
                IID_IPROPERTYSETSTORAGE => {
                    Some(b"ole32.dll!IStorage::QueryInterface(IPropertySetStorage)")
                }
                IID_IROOTSTORAGE => Some(b"ole32.dll!IStorage::QueryInterface(IRootStorage)"),
                IID_IMARSHAL => Some(b"ole32.dll!IStorage::QueryInterface(IMarshal)"),
                _ => None,
            };
            if let Some(what) = gap {
                missing(what, 0);
            }
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn stg_add_ref(this: *mut Storage) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn stg_release(this: *mut Storage) -> u32 {
    // SAFETY: our object, freed with its last reference; a substorage
    // lets go of its hold, and the document of one of its objects.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            let doc = (*this).doc;
            if !(*this).root {
                (*doc).drop_hold((*this).entry);
            }
            doc_release(doc);
            free(this.cast());
        }
        left
    }
}

/// A call that would change the file: refused, and recorded, since
/// what gala lacks is the write-capable open behind it.
fn read_only(what: &[u8], out: *mut *mut c_void) -> HResult {
    missing(what, 0);
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    STG_E_ACCESSDENIED
}

extern "system" fn stg_create_stream(
    _this: *mut Storage,
    _name: *const u16,
    _mode: u32,
    _r1: u32,
    _r2: u32,
    out: *mut *mut c_void,
) -> HResult {
    read_only(b"ole32.dll!IStorage::CreateStream(read-only storage)", out)
}

extern "system" fn stg_create_storage(
    _this: *mut Storage,
    _name: *const u16,
    _mode: u32,
    _r1: u32,
    _r2: u32,
    out: *mut *mut c_void,
) -> HResult {
    read_only(b"ole32.dll!IStorage::CreateStorage(read-only storage)", out)
}

extern "system" fn stg_destroy(_this: *mut Storage, _name: *const u16) -> HResult {
    read_only(b"ole32.dll!IStorage::DestroyElement(read-only storage)", core::ptr::null_mut())
}

extern "system" fn stg_rename(_this: *mut Storage, _old: *const u16, _new: *const u16) -> HResult {
    read_only(b"ole32.dll!IStorage::RenameElement(read-only storage)", core::ptr::null_mut())
}

extern "system" fn stg_set_times(
    _this: *mut Storage,
    _name: *const u16,
    _ctime: *const u64,
    _atime: *const u64,
    _mtime: *const u64,
) -> HResult {
    read_only(b"ole32.dll!IStorage::SetElementTimes(read-only storage)", core::ptr::null_mut())
}

extern "system" fn stg_set_class(_this: *mut Storage, _clsid: *const Guid) -> HResult {
    read_only(b"ole32.dll!IStorage::SetClass(read-only storage)", core::ptr::null_mut())
}

extern "system" fn stg_set_state(_this: *mut Storage, _bits: u32, _mask: u32) -> HResult {
    read_only(b"ole32.dll!IStorage::SetStateBits(read-only storage)", core::ptr::null_mut())
}

extern "system" fn stg_commit(_this: *mut Storage, _flags: u32) -> HResult {
    read_only(b"ole32.dll!IStorage::Commit(read-only storage)", core::ptr::null_mut())
}

/// Revert: nothing was changed, so there is nothing to undo (S_OK, as
/// Windows answers for a storage opened for reading).
extern "system" fn stg_revert(_this: *mut Storage) -> HResult {
    S_OK
}

extern "system" fn stg_copy_to(
    _this: *mut Storage,
    _count: u32,
    _iids: *const Guid,
    _exclude: *mut *mut u16,
    _dest: *mut c_void,
) -> HResult {
    missing(b"ole32.dll!IStorage::CopyTo", 0);
    E_NOTIMPL
}

extern "system" fn stg_move_to(
    _this: *mut Storage,
    _name: *const u16,
    _dest: *mut c_void,
    _new: *const u16,
    _flags: u32,
) -> HResult {
    missing(b"ole32.dll!IStorage::MoveElementTo", 0);
    E_NOTIMPL
}

/// The name a child is looked up by: NULL is a name, not "this
/// storage".
fn child_name(name: *const u16) -> Option<([u16; 32], usize)> {
    if name.is_null() {
        return None;
    }
    let mut out = [0u16; 32];
    let mut n = 0usize;
    // SAFETY: the caller's name, terminated; a longer one cannot be a
    // directory entry's and is simply not found.
    unsafe {
        while n < 32 {
            let c = *name.add(n);
            if c == 0 {
                break;
            }
            out[n] = c;
            n += 1;
        }
    }
    Some((out, n))
}

extern "system" fn stg_open_stream(
    this: *mut Storage,
    name: *const u16,
    reserved1: *mut c_void,
    mode: u32,
    reserved2: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    if !reserved1.is_null() || reserved2 != 0 {
        return STG_E_INVALIDPARAMETER;
    }
    let Some((want, len)) = child_name(name) else {
        return STG_E_INVALIDNAME;
    };
    let bad = child_mode(mode);
    if bad != S_OK {
        return bad;
    }
    // SAFETY: our object.
    let (doc, child) = unsafe { ((*this).doc, (*this).child) };
    // SAFETY: the document this object holds.
    let found = unsafe { (*doc).find(child, &want[..len]) };
    let Some((index, e)) = found else {
        return STG_E_FILENOTFOUND;
    };
    if entry_kind(&e) != 2 {
        return STG_E_FILENOTFOUND;
    }
    // SAFETY: as above.
    if !unsafe { (*doc).hold(index) } {
        return STG_E_ACCESSDENIED;
    }
    let (name, name_len) = entry_name(&e);
    // SAFETY: as above.
    let sector = unsafe { (*doc).sector };
    let p = new_stream(
        doc,
        index,
        entry_start(&e),
        entry_size(&e, sector),
        mode,
        name,
        name_len,
        entry_times(&e),
    );
    if p.is_null() {
        // SAFETY: as above.
        unsafe { (*doc).drop_hold(index) };
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

extern "system" fn stg_open_storage(
    this: *mut Storage,
    name: *const u16,
    priority: *mut c_void,
    mode: u32,
    exclude: *mut *mut u16,
    reserved: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    if !exclude.is_null() || reserved != 0 {
        return STG_E_INVALIDPARAMETER;
    }
    let Some((want, len)) = child_name(name) else {
        return STG_E_INVALIDNAME;
    };
    let bad = child_mode(mode);
    if bad != S_OK {
        return bad;
    }
    if !priority.is_null() {
        missing(b"ole32.dll!IStorage::OpenStorage(priority storage)", u64::from(mode));
    }
    // SAFETY: our object.
    let (doc, child) = unsafe { ((*this).doc, (*this).child) };
    // SAFETY: the document this object holds.
    let found = unsafe { (*doc).find(child, &want[..len]) };
    let Some((index, e)) = found else {
        return STG_E_FILENOTFOUND;
    };
    if entry_kind(&e) != 1 {
        return STG_E_FILENOTFOUND;
    }
    // SAFETY: as above.
    if !unsafe { (*doc).hold(index) } {
        return STG_E_ACCESSDENIED;
    }
    let (name, name_len) = entry_name(&e);
    let p = new_storage(
        doc,
        StorageEntry {
            entry: index,
            child: entry_child(&e),
            name,
            name_len,
            clsid: entry_clsid(&e),
            times: entry_times(&e),
        },
        mode,
        false,
    );
    if p.is_null() {
        // SAFETY: as above.
        unsafe { (*doc).drop_hold(index) };
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

extern "system" fn stg_stat(this: *mut Storage, out: *mut u8, flag: u32) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: our object; the root reports the file's path and times,
    // an element its entry's name and times.
    unsafe {
        let s = &*this;
        let name = if flag & STATFLAG_NONAME != 0 {
            core::ptr::null_mut()
        } else if s.root {
            task_name(core::slice::from_raw_parts((*s.doc).path, (*s.doc).path_len))
        } else {
            task_name(&s.name[..s.name_len])
        };
        let times = if s.root { (*s.doc).times } else { s.times };
        write_stat(
            out,
            &Stat {
                name,
                kind: STGTY_STORAGE,
                size: 0,
                times,
                mode: s.mode,
                clsid: s.clsid,
                state: 0,
            },
        );
    }
    S_OK
}

extern "system" fn stg_enum(
    this: *mut Storage,
    reserved1: u32,
    reserved2: *mut c_void,
    reserved3: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    if reserved1 != 0 || !reserved2.is_null() || reserved3 != 0 {
        return STG_E_INVALIDPARAMETER;
    }
    // SAFETY: our object.
    let (doc, child) = unsafe { ((*this).doc, (*this).child) };
    // SAFETY: the document this object holds; the walk is counted
    // first, then filled, so the snapshot does not change under it.
    let len = unsafe { (*doc).walk(child, None) };
    let items = if len == 0 { core::ptr::null_mut() } else { alloc(len * 4).cast::<u32>() };
    if len > 0 && items.is_null() {
        return E_OUTOFMEMORY;
    }
    if len > 0 {
        // SAFETY: the array just allocated, of `len` entries.
        unsafe { (*doc).walk(child, Some(core::slice::from_raw_parts_mut(items, len))) };
    }
    let p = new_enum(doc, items, len, 0);
    if p.is_null() {
        free(items.cast());
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

static STORAGE_VTABLE: VTable<18> = VTable([
    stg_query as *const (),
    stg_add_ref as *const (),
    stg_release as *const (),
    stg_create_stream as *const (),
    stg_open_stream as *const (),
    stg_create_storage as *const (),
    stg_open_storage as *const (),
    stg_copy_to as *const (),
    stg_move_to as *const (),
    stg_commit as *const (),
    stg_revert as *const (),
    stg_enum as *const (),
    stg_destroy as *const (),
    stg_rename as *const (),
    stg_set_times as *const (),
    stg_set_class as *const (),
    stg_set_state as *const (),
    stg_stat as *const (),
]);

// ---- IStream ----

#[repr(C)]
struct Stream {
    vtable: *const usize,
    refs: AtomicU32,
    doc: *mut Doc,
    entry: u32,
    start: u32,
    size: u64,
    mode: u32,
    position: u64,
    name: [u16; 32],
    name_len: usize,
    times: [u64; 3],
}

#[allow(clippy::too_many_arguments)]
fn new_stream(
    doc: *mut Doc,
    entry: u32,
    start: u32,
    size: u64,
    mode: u32,
    name: [u16; 32],
    name_len: usize,
    times: [u64; 3],
) -> *mut Stream {
    let p = alloc(core::mem::size_of::<Stream>()).cast::<Stream>();
    if p.is_null() {
        return p;
    }
    doc_add_ref(doc);
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Stream {
            vtable: STREAM_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            doc,
            entry,
            start,
            size,
            mode,
            position: 0,
            name,
            name_len,
            times,
        });
    }
    p
}

extern "system" fn str_query(
    this: *mut Stream,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's IID. A compound file's stream answers
    // IStream and IUnknown only: ISequentialStream is not one of its
    // interfaces, as Windows' is not either.
    match unsafe { *iid } {
        IID_IUNKNOWN | IID_ISTREAM => {
            str_add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn str_add_ref(this: *mut Stream) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn str_release(this: *mut Stream) -> u32 {
    // SAFETY: our object, freed with its last reference; the element's
    // hold goes with it, and the document with its last object.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            let doc = (*this).doc;
            (*doc).drop_hold((*this).entry);
            doc_release(doc);
            free(this.cast());
        }
        left
    }
}

extern "system" fn str_read(this: *mut Stream, buf: *mut u8, size: u32, done: *mut u32) -> HResult {
    if !done.is_null() {
        put(done, 0);
    }
    if buf.is_null() && size > 0 {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    let left = s.size.saturating_sub(s.position);
    let n = left.min(u64::from(size)) as usize;
    if n > 0 {
        // SAFETY: the document this object holds; a buffer of at least
        // `size` bytes.
        let ok = unsafe { (*s.doc).read_stream(s.start, s.size, s.position, buf, n) };
        if !ok {
            missing(b"ole32.dll!IStream::Read(chain ends early)", s.size);
            return STG_E_DOCFILECORRUPT;
        }
        s.position += n as u64;
    }
    if !done.is_null() {
        put(done, n as u32);
    }
    S_OK
}

extern "system" fn str_write(
    _this: *mut Stream,
    _buf: *const u8,
    _size: u32,
    done: *mut u32,
) -> HResult {
    if !done.is_null() {
        put(done, 0);
    }
    missing(b"ole32.dll!IStream::Write(read-only storage)", 0);
    STG_E_ACCESSDENIED
}

extern "system" fn str_seek(
    this: *mut Stream,
    move_by: i64,
    origin: u32,
    out: *mut u64,
) -> HResult {
    // SAFETY: our object.
    let s = unsafe { &mut *this };
    let base = match origin {
        0 => 0,
        1 => s.position,
        2 => s.size,
        _ => return STG_E_INVALIDFUNCTION,
    };
    let Some(at) = base.checked_add_signed(move_by) else {
        return STG_E_INVALIDFUNCTION;
    };
    s.position = at;
    if !out.is_null() {
        put(out, at);
    }
    S_OK
}

extern "system" fn str_set_size(_this: *mut Stream, _size: u64) -> HResult {
    missing(b"ole32.dll!IStream::SetSize(read-only storage)", 0);
    STG_E_ACCESSDENIED
}

/// CopyTo: reads from here and writes to `target`, reporting what each
/// side did.
extern "system" fn str_copy_to(
    this: *mut Stream,
    target: *mut c_void,
    count: u64,
    read_out: *mut u64,
    written_out: *mut u64,
) -> HResult {
    if target.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    let (mut total_read, mut total_written, mut left) = (0u64, 0u64, count);
    let mut chunk = [0u8; 4096];
    let mut r = S_OK;
    while left > 0 {
        let mut n = 0u32;
        let want = left.min(chunk.len() as u64) as u32;
        let got = str_read(this, chunk.as_mut_ptr(), want, &mut n);
        if got < 0 {
            r = got;
            break;
        }
        if n == 0 {
            break;
        }
        total_read += u64::from(n);
        left -= u64::from(n);
        let mut w = 0u32;
        r = com_call!(
            target,
            4,
            fn(*const u8, u32, *mut u32) -> HResult,
            chunk.as_ptr(),
            n,
            &mut w
        );
        total_written += u64::from(w);
        if r < 0 {
            break;
        }
    }
    if !read_out.is_null() {
        put(read_out, total_read);
    }
    if !written_out.is_null() {
        put(written_out, total_written);
    }
    r
}

/// Commit and Revert on a stream opened for reading: nothing to do.
extern "system" fn str_commit(_this: *mut Stream, _flags: u32) -> HResult {
    S_OK
}

extern "system" fn str_revert(_this: *mut Stream) -> HResult {
    S_OK
}

extern "system" fn str_lock(_this: *mut Stream, _offset: u64, _length: u64, _kind: u32) -> HResult {
    STG_E_INVALIDFUNCTION
}

extern "system" fn str_stat(this: *mut Stream, out: *mut u8, flag: u32) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: our object; the caller's STATSTG.
    unsafe {
        let s = &*this;
        let name = if flag & STATFLAG_NONAME != 0 {
            core::ptr::null_mut()
        } else {
            task_name(&s.name[..s.name_len])
        };
        write_stat(
            out,
            &Stat {
                name,
                kind: STGTY_STREAM,
                size: s.size,
                times: s.times,
                mode: s.mode,
                clsid: [0; 16],
                state: 0,
            },
        );
    }
    S_OK
}

/// Clone: another stream on the same element at the same position,
/// sharing its hold.
extern "system" fn str_clone(this: *mut Stream, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: our object.
    let s = unsafe { &*this };
    let p = new_stream(s.doc, s.entry, s.start, s.size, s.mode, s.name, s.name_len, s.times);
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: our object and the document it holds.
    unsafe {
        (*p).position = s.position;
        (*s.doc).share_hold(s.entry);
    }
    put(out, p.cast());
    S_OK
}

static STREAM_VTABLE: VTable<14> = VTable([
    str_query as *const (),
    str_add_ref as *const (),
    str_release as *const (),
    str_read as *const (),
    str_write as *const (),
    str_seek as *const (),
    str_set_size as *const (),
    str_copy_to as *const (),
    str_commit as *const (),
    str_revert as *const (),
    str_lock as *const (),
    str_lock as *const (),
    str_stat as *const (),
    str_clone as *const (),
]);

// ---- IEnumSTATSTG ----

#[repr(C)]
struct Enum {
    vtable: *const usize,
    refs: AtomicU32,
    doc: *mut Doc,
    items: *mut u32,
    len: usize,
    at: usize,
}

fn new_enum(doc: *mut Doc, items: *mut u32, len: usize, at: usize) -> *mut Enum {
    let p = alloc(core::mem::size_of::<Enum>()).cast::<Enum>();
    if p.is_null() {
        return p;
    }
    doc_add_ref(doc);
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Enum {
            vtable: ENUM_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            doc,
            items,
            len,
            at,
        });
    }
    p
}

extern "system" fn enum_query(this: *mut Enum, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: the caller's IID.
    match unsafe { *iid } {
        IID_IUNKNOWN | IID_IENUMSTATSTG => {
            enum_add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn enum_add_ref(this: *mut Enum) -> u32 {
    // SAFETY: our object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn enum_release(this: *mut Enum) -> u32 {
    // SAFETY: our object, freed with its last reference.
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            free((*this).items.cast());
            doc_release((*this).doc);
            free(this.cast());
        }
        left
    }
}

extern "system" fn enum_next(
    this: *mut Enum,
    count: u32,
    out: *mut u8,
    fetched: *mut u32,
) -> HResult {
    if fetched.is_null() && count > 1 {
        return STG_E_INVALIDPARAMETER;
    }
    if !fetched.is_null() {
        put(fetched, 0);
    }
    if out.is_null() && count > 0 {
        return STG_E_INVALIDPOINTER;
    }
    // SAFETY: our object.
    let e = unsafe { &mut *this };
    let mut n = 0usize;
    while n < count as usize && e.at < e.len {
        // SAFETY: the snapshot array, within its length; the
        // document this object holds.
        let entry = unsafe { (*e.doc).entry(*e.items.add(e.at)) };
        let Some(entry) = entry else {
            break;
        };
        let (name, name_len) = entry_name(&entry);
        // SAFETY: as above.
        let sector = unsafe { (*e.doc).sector };
        let kind = match entry_kind(&entry) {
            1 | 5 => STGTY_STORAGE,
            _ => STGTY_STREAM,
        };
        let size = if kind == STGTY_STREAM { entry_size(&entry, sector) } else { 0 };
        // SAFETY: the caller's array of `count` STATSTGs.
        let at = unsafe { out.add(n * STATSTG_SIZE) };
        write_stat(
            at,
            &Stat {
                name: task_name(&name[..name_len]),
                kind,
                size,
                times: entry_times(&entry),
                mode: 0,
                clsid: entry_clsid(&entry),
                state: entry_state(&entry),
            },
        );
        e.at += 1;
        n += 1;
    }
    if !fetched.is_null() {
        put(fetched, n as u32);
    }
    if n == count as usize { S_OK } else { S_FALSE }
}

extern "system" fn enum_skip(this: *mut Enum, count: u32) -> HResult {
    // SAFETY: our object.
    let e = unsafe { &mut *this };
    let left = e.len - e.at;
    let n = (count as usize).min(left);
    e.at += n;
    if n == count as usize { S_OK } else { S_FALSE }
}

extern "system" fn enum_reset(this: *mut Enum) -> HResult {
    // SAFETY: our object.
    unsafe { (*this).at = 0 };
    S_OK
}

extern "system" fn enum_clone(this: *mut Enum, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: our object; the clone takes a snapshot of its own.
    let e = unsafe { &*this };
    let items = if e.len == 0 { core::ptr::null_mut() } else { alloc(e.len * 4).cast::<u32>() };
    if e.len > 0 && items.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: two arrays of `len` entries.
    unsafe { core::ptr::copy_nonoverlapping(e.items, items, e.len) };
    let p = new_enum(e.doc, items, e.len, e.at);
    if p.is_null() {
        free(items.cast());
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    S_OK
}

static ENUM_VTABLE: VTable<7> = VTable([
    enum_query as *const (),
    enum_add_ref as *const (),
    enum_release as *const (),
    enum_next as *const (),
    enum_skip as *const (),
    enum_reset as *const (),
    enum_clone as *const (),
]);

// ---- The exports ----

/// StgIsStorageFile: whether the file starts with the compound file
/// signature. Nothing else of it is read, so a file truncated after
/// the first sector still answers S_OK (as Windows does).
#[unsafe(no_mangle)]
pub extern "system" fn StgIsStorageFile(name: *const u16) -> HResult {
    if name.is_null() {
        return STG_E_INVALIDNAME;
    }
    // SAFETY: the caller's name.
    let file = unsafe {
        CreateFileW(name, GENERIC_READ, FILE_SHARE_READ, core::ptr::null_mut(), OPEN_EXISTING, 0, 0)
    };
    if file == INVALID_HANDLE {
        // SAFETY: a plain call.
        return from_win32(unsafe { GetLastError() });
    }
    let mut sig = [0u8; 8];
    let ok = read_at(file, 0, sig.as_mut_ptr(), 8);
    // SAFETY: the handle just opened.
    unsafe { CloseHandle(file) };
    if ok && sig == SIGNATURE { S_OK } else { S_FALSE }
}

/// StgOpenStorage: the root storage of a compound file, read-only.
/// `priority` is ignored and an exclusion list refused, both recorded.
#[unsafe(no_mangle)]
pub extern "system" fn StgOpenStorage(
    name: *const u16,
    priority: *mut c_void,
    mode: u32,
    exclude: *mut *mut u16,
    reserved: u32,
    out: *mut *mut c_void,
) -> HResult {
    // A non-zero `reserved` is refused before the out pointer is
    // touched, which is what Windows leaves behind.
    if reserved != 0 {
        return STG_E_INVALIDPARAMETER;
    }
    if out.is_null() {
        return STG_E_INVALIDPOINTER;
    }
    put(out, core::ptr::null_mut());
    if name.is_null() {
        return STG_E_INVALIDNAME;
    }
    let bad = open_mode(mode);
    if bad != S_OK {
        return bad;
    }
    if !exclude.is_null() {
        // Windows takes an exclusion list with a transacted
        // read-write open and refuses it otherwise; gala has neither,
        // and refuses it as Windows refuses the direct case.
        missing(b"ole32.dll!StgOpenStorage(exclusion list)", u64::from(mode));
        return STG_E_ACCESSDENIED;
    }
    if !priority.is_null() {
        missing(b"ole32.dll!StgOpenStorage(priority storage)", u64::from(mode));
    }
    if mode & STGM_ACCESS != 0 {
        // The storage handed out is read-only whatever was asked for:
        // writing a compound file is not built.
        missing(b"ole32.dll!StgOpenStorage(write access)", u64::from(mode));
    }
    let doc = match open_doc(name, mode) {
        Ok(doc) => doc,
        Err(err) => return err,
    };
    // SAFETY: the document just opened; the root entry's children are
    // the storage's.
    let root = unsafe { (*doc).entry(0) };
    let Some(e) = root else {
        doc_release(doc);
        return STG_E_DOCFILECORRUPT;
    };
    // SAFETY: as above.
    let clsid = unsafe { (*doc).root_clsid };
    let times = unsafe { (*doc).times };
    let p = new_storage(
        doc,
        StorageEntry { entry: 0, child: entry_child(&e), name: [0; 32], name_len: 0, clsid, times },
        mode,
        true,
    );
    // The document started with one reference, which the storage took
    // over; new_storage counted its own.
    doc_release(doc);
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    put(out, p.cast());
    reached(b"ole32.dll!StgOpenStorage(opened)", u64::from(mode));
    S_OK
}

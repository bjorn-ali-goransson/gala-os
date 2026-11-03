//! PE image helpers (dbghelp.h), as documented and as the PE format
//! defines the headers: ImageNtHeader on a mapped image, and
//! SymSrvGetFileIndexInfoW, which reads the values a symbol server
//! indexes an image file by (its TimeDateStamp and SizeOfImage, and its
//! CodeView record's PDB name, GUID and age).

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn SetFilePointerEx(h: usize, distance: i64, new: *mut i64, method: u32) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn SetLastError(e: u32);
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; 2 is GalaRecordMissing's "unsupported".
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

const MZ: u16 = 0x5a4d;
const PE: u32 = 0x4550;

/// ImageNtHeader: the IMAGE_NT_HEADERS of the image mapped at `base`, or
/// NULL when it is not one ("MZ", then "PE\0\0" at e_lfanew).
#[unsafe(no_mangle)]
pub extern "system" fn ImageNtHeader(base: *const u8) -> *const u8 {
    if base.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the caller's mapped image (or memory at least as large as
    // a DOS header, as Windows' function reads it too).
    unsafe {
        if (base as *const u16).read_unaligned() != MZ {
            return core::ptr::null();
        }
        let lfanew = (base.add(0x3c) as *const u32).read_unaligned();
        if lfanew == 0 || lfanew > 0x1000_0000 {
            return core::ptr::null();
        }
        let nt = base.add(lfanew as usize);
        if (nt as *const u32).read_unaligned() != PE {
            return core::ptr::null();
        }
        nt
    }
}

/// SYMSRV_INDEX_INFOW (dbghelp.h).
#[repr(C)]
pub struct IndexInfo {
    size: u32,
    file: [u16; 261],
    stripped: i32,
    timestamp: u32,
    image_size: u32,
    dbg_file: [u16; 261],
    pdb_file: [u16; 261],
    guid: [u8; 16],
    sig: u32,
    age: u32,
}

const GENERIC_READ: u32 = 0x8000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_READ_WRITE_DELETE: u32 = 7;
const INVALID: usize = usize::MAX;
const ERROR_BAD_EXE_FORMAT: u32 = 193;
const ERROR_INVALID_PARAMETER: u32 = 87;
const IMAGE_FILE_DEBUG_STRIPPED: u16 = 0x200;
const IMAGE_DEBUG_TYPE_CODEVIEW: u32 = 2;

/// An open file read at offsets.
struct File(usize);

impl File {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Option<usize> {
        let mut read = 0u32;
        // SAFETY: our handle and buffer.
        unsafe {
            if SetFilePointerEx(self.0, offset as i64, core::ptr::null_mut(), 0) == 0 {
                return None;
            }
            if ReadFile(
                self.0,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                core::ptr::null_mut(),
            ) == 0
            {
                return None;
            }
        }
        Some(read as usize)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: our handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The index values of the image file `f` into `info`; None if it is not
/// a PE image.
fn read_index(f: &File, info: &mut IndexInfo) -> Option<()> {
    let mut headers = [0u8; 4096];
    let n = f.read(0, &mut headers)?;
    let h = &headers[..n];
    if u16_at(h, 0)? != MZ {
        return None;
    }
    let nt = u32_at(h, 0x3c)? as usize;
    if u32_at(h, nt)? != PE {
        return None;
    }
    let file_header = nt + 4;
    let sections = u16_at(h, file_header + 2)? as usize;
    info.timestamp = u32_at(h, file_header + 4)?;
    let optional_size = u16_at(h, file_header + 16)? as usize;
    let characteristics = u16_at(h, file_header + 18)?;
    info.stripped = i32::from(characteristics & IMAGE_FILE_DEBUG_STRIPPED != 0);
    let optional = file_header + 20;
    let magic = u16_at(h, optional)?;
    // SizeOfImage is at 56 in both forms; the data directories start at
    // 96 (PE32) or 112 (PE32+); the debug directory is entry 6.
    info.image_size = u32_at(h, optional + 56)?;
    let directories = match magic {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return None,
    };
    let count = u32_at(h, directories - 4)?;
    if count <= 6 {
        return Some(());
    }
    let (debug_rva, debug_size) = (u32_at(h, directories + 48)?, u32_at(h, directories + 52)?);
    if debug_rva == 0 || debug_size == 0 {
        return Some(());
    }
    // The section holding the debug directory: its file offset.
    let table = optional + optional_size;
    let offset = (0..sections).find_map(|i| {
        let s = table + i * 40;
        let (size, rva, raw) = (u32_at(h, s + 8)?, u32_at(h, s + 12)?, u32_at(h, s + 20)?);
        (debug_rva >= rva && debug_rva < rva + size).then(|| u64::from(raw + (debug_rva - rva)))
    })?;
    let mut entries = [0u8; 28 * 16];
    let len = (debug_size as usize).min(entries.len());
    let n = f.read(offset, &mut entries[..len])?;
    for e in entries[..n].chunks_exact(28) {
        if u32_at(e, 12)? != IMAGE_DEBUG_TYPE_CODEVIEW {
            continue;
        }
        let (size, raw) = (u32_at(e, 16)? as usize, u32_at(e, 24)?);
        let mut cv = [0u8; 24 + 520];
        let want = size.min(cv.len());
        let n = f.read(raw.into(), &mut cv[..want])?;
        let cv = &cv[..n];
        if cv.get(..4) == Some(b"RSDS") {
            info.guid.copy_from_slice(cv.get(4..20)?);
            info.age = u32_at(cv, 20)?;
            let name = &cv[24..];
            let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
            // The PDB's file name, without the build machine's directory.
            let base = name.rsplit(|&b| b == b'\\' || b == b'/').next().unwrap_or(name);
            for (d, &b) in info.pdb_file.iter_mut().zip(base.iter().take(260)) {
                *d = b.into();
            }
        } else {
            // NB10 (old CodeView with a signature) and others.
            unsupported(b"dbghelp.dll!SymSrvGetFileIndexInfoW(CodeView format)", 0);
        }
        break;
    }
    Some(())
}

/// SymSrvGetFileIndexInfoW: the symbol-server index values of the image
/// file `path` (its name, TimeDateStamp, SizeOfImage, whether its debug
/// information is stripped, and its CodeView record's PDB name, GUID and
/// age). FALSE with the file's error when it cannot be opened, or
/// ERROR_BAD_EXE_FORMAT for a file that is not an image (PDB and DBG
/// files are not read yet: recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SymSrvGetFileIndexInfoW(
    path: *const u16,
    info: *mut IndexInfo,
    _flags: u32,
) -> i32 {
    if path.is_null() || info.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    // SAFETY: the caller's structure.
    let info = unsafe { &mut *info };
    if info.size as usize != core::mem::size_of::<IndexInfo>() {
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    // SAFETY: the caller's NUL-terminated path.
    let h = unsafe {
        CreateFileW(
            path,
            GENERIC_READ,
            FILE_SHARE_READ_WRITE_DELETE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h == INVALID {
        return 0; // CreateFileW's last error
    }
    let f = File(h);
    let size = info.size;
    // SAFETY: plain data; zeroed, then filled in.
    *info = unsafe { core::mem::zeroed() };
    info.size = size;
    // The file name, without its directory.
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated path.
    while unsafe { *path.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: as above.
    let p = unsafe { core::slice::from_raw_parts(path, n) };
    let name = p.rsplit(|&c| c == u16::from(b'\\') || c == u16::from(b'/')).next().unwrap_or(p);
    for (d, &c) in info.file.iter_mut().zip(name.iter().take(260)) {
        *d = c;
    }
    if read_index(&f, info).is_none() {
        let ext = name.rsplit(|&c| c == u16::from(b'.')).next().unwrap_or(&[]);
        let pdb = ext.len() == 3
            && ext.iter().zip(b"pdb").all(|(&c, &b)| (c as u8).eq_ignore_ascii_case(&b));
        let dbg = ext.len() == 3
            && ext.iter().zip(b"dbg").all(|(&c, &b)| (c as u8).eq_ignore_ascii_case(&b));
        if pdb || dbg {
            unsupported(b"dbghelp.dll!SymSrvGetFileIndexInfoW(PDB or DBG file)", 0);
        }
        // SAFETY: plain call.
        unsafe { SetLastError(ERROR_BAD_EXE_FORMAT) };
        return 0;
    }
    1
}

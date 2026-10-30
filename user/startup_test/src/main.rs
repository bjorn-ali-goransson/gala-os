//! Process startup through ntdll: every process has ntdll.dll mapped from
//! `\Windows\System32` as an image (found by scanning the address space
//! with NtQueryVirtualMemory and MemoryMappedFilenameInformation), exporting
//! LdrInitializeThunk and NtContinue. Results are expected to match real
//! Windows; when the ntdll is gala's test ntdll (`user/testntdll`), the
//! test also checks that its LdrInitializeThunk ran before our entry point
//! with the documented arguments. Prints `startup_test: ok` or exits with
//! the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

unsafe extern "C" {
    static __ImageBase: u8;
}

const MEMORY_MAPPED_FILENAME_INFORMATION: u32 = 2;
const PAGE_EXECUTE_READ: u32 = 0x20;
const USER_TOP: usize = 0x7fff_ffff_0000;
const TEST_NTDLL_MAGIC: u64 = 0x6761_6c61_6e74_646c;

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("startup_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

fn ends_with_ci(s: &[u16], suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s[s.len() - suffix.len()..]
            .iter()
            .zip(suffix.bytes())
            .all(|(&a, b)| upper(a) == upper(u16::from(b)))
}

/// The mapped file name of `addr` (MemoryMappedFilenameInformation).
fn mapped_name(addr: usize, buf: &mut [u8; 600]) -> Result<&[u16], NtStatus> {
    let (s, _) = query_virtual_raw(addr, MEMORY_MAPPED_FILENAME_INFORMATION, buf);
    if s != STATUS_SUCCESS {
        return Err(s);
    }
    let len = usize::from(u16::from_le_bytes([buf[0], buf[1]]));
    let ptr = usize::from_le_bytes(buf[8..16].try_into().unwrap()) as *const u16;
    // SAFETY: the name buffer is inside `buf`, `len` bytes long.
    Ok(unsafe { core::slice::from_raw_parts(ptr, len / 2) })
}

/// Finds the image mapped from `...\System32\ntdll.dll`.
fn find_ntdll() -> Option<usize> {
    let mut addr = 0;
    let mut buf = [0u8; 600];
    while addr < USER_TOP {
        let (s, mbi) = query(addr);
        if s != STATUS_SUCCESS || mbi.region_size == 0 {
            return None;
        }
        if mbi.kind == MEM_IMAGE
            && mbi.base_address == mbi.allocation_base
            && mapped_name(addr, &mut buf).is_ok_and(|n| ends_with_ci(n, "\\System32\\ntdll.dll"))
        {
            return Some(addr);
        }
        addr = mbi.base_address + mbi.region_size;
    }
    None
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    c.ok(true, 0)?;
    // SAFETY: the start of a mapped image.
    let mz = unsafe { *(ntdll as *const [u8; 2]) };
    c.ok(&mz == b"MZ", 0)?;
    c.ok(export_dll_name(ntdll).is_some_and(|n| n.eq_ignore_ascii_case(b"ntdll.dll")), 0)?;
    let thunk = find_export(ntdll, b"LdrInitializeThunk");
    c.ok(thunk.is_some(), 0)?;
    c.ok(find_export(ntdll, b"NtContinue").is_some(), 0)?;
    let (s, mbi) = query(thunk.unwrap_or(0));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    c.eq(mbi.protect.into(), PAGE_EXECUTE_READ.into())?;
    c.eq(mbi.allocation_base as u64, ntdll as u64)?;

    // Our own image is mapped from our file; private memory has no name.
    let image = &raw const __ImageBase as usize;
    let mut buf = [0u8; 600];
    c.ok(mapped_name(image, &mut buf).is_ok_and(|n| ends_with_ci(n, "\\startup_test.exe")), 0)?;
    let local = 0u8;
    c.ok(mapped_name(&raw const local as usize, &mut buf).is_err(), 0)?;

    // gala's test ntdll records its LdrInitializeThunk call.
    if let Some(state) = find_export(ntdll, b"GalaTestNtdllState") {
        // SAFETY: the exported [u64; 4].
        let state = unsafe { (state as *const [u64; 4]).read_volatile() };
        c.eq(state[0], TEST_NTDLL_MAGIC)?;
        c.eq(state[1], ntdll as u64)?; // NtdllBase argument
        let pe = image + read_u32(image + 0x3c) as usize;
        let entry = image + read_u32(pe + 24 + 16) as usize; // AddressOfEntryPoint
        c.eq(state[2], entry as u64)?; // Context->Rip
        c.eq(state[3], 1)?;
    }
    Ok(())
}

fn read_u32(addr: usize) -> u32 {
    // SAFETY: callers read inside mapped image headers.
    unsafe { (addr as *const u32).read_unaligned() }
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("startup_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

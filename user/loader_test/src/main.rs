//! The dynamic loader, through ntdll's exports that LoadLibrary and
//! GetProcAddress build on: LdrLoadDll, LdrGetProcedureAddress,
//! LdrGetDllHandle and LdrUnloadDll. testdll.dll and testdll2.dll (which
//! imports from testdll) sit next to the exe; neither is imported, so the
//! loader maps them on demand. Checks reference counting, lookup by name
//! and by ordinal, the error statuses, that DllMain sees one
//! DLL_PROCESS_ATTACH, and that a DLL's own imports are bound. Prints
//! `loader_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_PROCEDURE_NOT_FOUND: NtStatus = 0xc000_007a;
const STATUS_DLL_NOT_FOUND: NtStatus = 0xc000_0135;
const INITIAL_VALUE: u64 = 0x1234_5678_9abc;

#[repr(C)]
struct AnsiString {
    length: u16,
    maximum_length: u16,
    buffer: *const u8,
}

impl AnsiString {
    fn new(s: &[u8]) -> Self {
        AnsiString { length: s.len() as u16, maximum_length: s.len() as u16, buffer: s.as_ptr() }
    }
}

type LoadDll =
    extern "system" fn(*const u16, *const u32, *const UnicodeString, *mut usize) -> NtStatus;
type GetProc = extern "system" fn(usize, *const AnsiString, u32, *mut usize) -> NtStatus;
type GetHandle =
    extern "system" fn(*const u16, *const u32, *const UnicodeString, *mut usize) -> NtStatus;
type Unload = extern "system" fn(usize) -> NtStatus;

struct Api {
    load: LoadDll,
    get_proc: GetProc,
    get_handle: GetHandle,
    unload: Unload,
}

impl Api {
    fn load(&self, name: &[u16]) -> (NtStatus, usize) {
        let mut base = 0;
        let s =
            (self.load)(core::ptr::null(), core::ptr::null(), &UnicodeString::new(name), &mut base);
        (s, base)
    }

    fn handle(&self, name: &[u16]) -> (NtStatus, usize) {
        let mut base = 0;
        let s = (self.get_handle)(
            core::ptr::null(),
            core::ptr::null(),
            &UnicodeString::new(name),
            &mut base,
        );
        (s, base)
    }

    fn proc(&self, module: usize, name: &[u8]) -> (NtStatus, usize) {
        let mut addr = 0;
        let s = (self.get_proc)(module, &AnsiString::new(name), 0, &mut addr);
        (s, addr)
    }

    fn proc_by_ordinal(&self, module: usize, ordinal: u32) -> (NtStatus, usize) {
        let mut addr = 0;
        let s = (self.get_proc)(module, core::ptr::null(), ordinal, &mut addr);
        (s, addr)
    }
}

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
        print("loader_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(addr: usize) -> u32 {
    // SAFETY: callers read inside a mapped image.
    unsafe { (addr as *const u32).read_unaligned() }
}

fn c_str(addr: usize) -> &'static [u8] {
    let mut n = 0;
    // SAFETY: export names are NUL-terminated strings in the image.
    unsafe {
        while *((addr + n) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(addr as *const u8, n)
    }
}

/// The ordinal of export `name` of the image at `base` (Base plus its
/// index in the address table).
fn ordinal_of(base: usize, name: &[u8]) -> Option<u32> {
    let pe = base + u32_at(base + 0x3c) as usize;
    let dir = base + u32_at(pe + 24 + 112) as usize; // DataDirectory[0]
    let names = base + u32_at(dir + 32) as usize;
    let ordinals = base + u32_at(dir + 36) as usize;
    let i = (0..u32_at(dir + 24) as usize)
        .find(|&i| c_str(base + u32_at(names + 4 * i) as usize) == name)?;
    // SAFETY: inside the export directory.
    let index = unsafe { ((ordinals + 2 * i) as *const u16).read_unaligned() };
    Some(u32_at(dir + 16) + u32::from(index))
}

fn resolve(c: &mut Checks) -> Result<Api, u32> {
    let Some(ntdll) = find_ntdll() else {
        c.0 += 1;
        return Err(c.0);
    };
    let mut get = |name: &[u8]| {
        let f = find_export(ntdll, name);
        c.ok(f.is_some(), 0).map(|()| f.unwrap_or(0))
    };
    let (load, get_proc, get_handle, unload) = (
        get(b"LdrLoadDll")?,
        get(b"LdrGetProcedureAddress")?,
        get(b"LdrGetDllHandle")?,
        get(b"LdrUnloadDll")?,
    );
    // SAFETY: ntdll's exports with these signatures.
    unsafe {
        Ok(Api {
            load: core::mem::transmute::<usize, LoadDll>(load),
            get_proc: core::mem::transmute::<usize, GetProc>(get_proc),
            get_handle: core::mem::transmute::<usize, GetHandle>(get_handle),
            unload: core::mem::transmute::<usize, Unload>(unload),
        })
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let api = resolve(&mut c)?;

    // Loading by file name, then again without the extension.
    let (s, dll) = api.load(&wide!("testdll.dll"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    let (s, mbi) = query(dll);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    c.eq(mbi.allocation_base as u64, dll as u64)?;
    let mut buf = [0u8; 600];
    c.ok(mapped_file_name(dll, &mut buf).is_ok_and(|name| ends_with_ci(name, "\\testdll.dll")), 0)?;
    let (s, again) = api.load(&wide!("testdll"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(again as u64, dll as u64)?;

    // Exports by name and by ordinal.
    let (s, add) = api.proc(dll, b"gala_add");
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.ok(find_export(dll, b"gala_add") == Some(add), add as u64)?;
    // SAFETY: testdll's gala_add.
    let add_fn = unsafe { core::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(add) };
    c.eq(add_fn(40, 2), 42)?;
    let Some(ordinal) = ordinal_of(dll, b"gala_add") else { return c.ok(false, 0) };
    let (s, by_ordinal) = api.proc_by_ordinal(dll, ordinal);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(by_ordinal as u64, add as u64)?;
    let (s, _) = api.proc(dll, b"no_such_export");
    c.eq(s.into(), STATUS_PROCEDURE_NOT_FOUND.into())?;
    let (s, count) = api.proc(dll, b"GALA_ATTACH_COUNT");
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    // SAFETY: testdll's exported counter.
    let attaches = || unsafe { core::ptr::read_volatile(count as *const u64) };
    c.eq(attaches(), 1)?;

    // Handles of loaded modules; missing DLLs.
    let (s, ntdll) = api.handle(&wide!("ntdll.dll"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.ok(find_ntdll() == Some(ntdll), ntdll as u64)?;
    let (s, h) = api.handle(&wide!("TESTDLL.DLL"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(h as u64, dll as u64)?;
    let (s, _) = api.load(&wide!("no_such_dll.dll"));
    c.eq(s.into(), STATUS_DLL_NOT_FOUND.into())?;
    let (s, _) = api.handle(&wide!("testdll2.dll"));
    c.eq(s.into(), STATUS_DLL_NOT_FOUND.into())?;

    // A DLL with imports of its own (from the already loaded testdll).
    let (s, dll2) = api.load(&wide!("testdll2.dll"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.ok(dll2 != 0 && dll2 != dll, dll2 as u64)?;
    let (s, add3) = api.proc(dll2, b"gala2_add3");
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    // SAFETY: testdll2's gala2_add3.
    let add3 = unsafe { core::mem::transmute::<usize, extern "C" fn(u64, u64, u64) -> u64>(add3) };
    c.eq(add3(2, 3, 4), 9)?;
    let (s, value) = api.proc(dll2, b"gala2_get_value");
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    // SAFETY: testdll2's gala2_get_value.
    let value = unsafe { core::mem::transmute::<usize, extern "C" fn() -> u64>(value) };
    c.eq(value(), INITIAL_VALUE)?;
    c.eq(attaches(), 1)?;

    // Unloading: testdll2 goes at once; testdll after its second unload.
    c.eq((api.unload)(dll2).into(), STATUS_SUCCESS.into())?;
    let (s, _) = api.handle(&wide!("testdll2.dll"));
    c.eq(s.into(), STATUS_DLL_NOT_FOUND.into())?;
    c.eq((api.unload)(dll).into(), STATUS_SUCCESS.into())?;
    let (s, h) = api.handle(&wide!("testdll.dll"));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(h as u64, dll as u64)?;
    c.eq((api.unload)(dll).into(), STATUS_SUCCESS.into())?;
    let (s, _) = api.handle(&wide!("testdll.dll"));
    c.eq(s.into(), STATUS_DLL_NOT_FOUND.into())?;
    let (s, _) = query(dll);
    c.ok(s != STATUS_SUCCESS || query(dll).1.kind != MEM_IMAGE, 0)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("loader_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

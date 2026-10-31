//! Image sections (SEC_IMAGE) from a DLL file, with results expected to
//! match real Windows. The test DLL (user/testdll) is embedded and written
//! to `C:\Users\Public` first, so the binary is self-contained; so is a
//! PE32 IL-only assembly (user/bin/dotnet_hello.dll). Prints
//! `image_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

static TEST_DLL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/x86_64-pc-windows-msvc/release/testdll.dll"
));

/// A 32-bit (PE32, i386) IL-only .NET assembly: 64-bit Windows maps such
/// images as image sections too (WOW64 and .NET rely on it).
static IL_DLL: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../bin/dotnet_hello.dll"));

const STATUS_IMAGE_NOT_AT_BASE: NtStatus = 0x4000_0003;
const STATUS_INVALID_IMAGE_NOT_MZ: NtStatus = 0xc000_012f;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_WRITECOPY: u32 = 0x08;
const INITIAL_VALUE: u64 = 0x1234_5678_9abc;

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
        print("image_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn st(s: NtStatus) -> u64 {
    s.into()
}

fn read_u32(addr: usize) -> u32 {
    // SAFETY: callers only read inside a mapped image.
    unsafe { (addr as *const u32).read_unaligned() }
}

/// Finds an export by name by walking the image's export directory.
fn export(base: usize, name: &[u8]) -> Option<usize> {
    let pe = base + read_u32(base + 0x3c) as usize;
    let dir_rva = read_u32(pe + 24 + 112) as usize; // DataDirectory[0]
    if dir_rva == 0 {
        return None;
    }
    let dir = base + dir_rva;
    let count = read_u32(dir + 24) as usize; // NumberOfNames
    let functions = base + read_u32(dir + 28) as usize;
    let names = base + read_u32(dir + 32) as usize;
    let ordinals = base + read_u32(dir + 36) as usize;
    for i in 0..count {
        let s = base + read_u32(names + 4 * i) as usize;
        // SAFETY: export names are NUL-terminated strings in the image.
        let matches = (0..=name.len()).all(|j| {
            let b = unsafe { *((s + j) as *const u8) };
            if j == name.len() { b == 0 } else { b == name[j] }
        });
        if matches {
            // SAFETY: inside the export directory.
            let ordinal = unsafe { ((ordinals + 2 * i) as *const u16).read_unaligned() } as usize;
            return Some(base + read_u32(functions + 4 * ordinal) as usize);
        }
    }
    None
}

fn write_whole_file(attrs: &ObjectAttributes, data: &[u8]) -> NtStatus {
    let (s, h, _) = create_file(
        attrs,
        GENERIC_WRITE,
        0,
        FILE_OVERWRITE_IF,
        FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE,
    );
    if s != STATUS_SUCCESS {
        return s;
    }
    let (s, _) = write_file_info(h, data);
    close(h);
    s
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = st(STATUS_SUCCESS);
    let opts = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;

    // A file that is not an image.
    let text = wide!("\\??\\C:\\Users\\Public\\gala_image_test.txt");
    let text = UnicodeString::new(&text);
    let text = ObjectAttributes::new(&text, Handle(0), OBJ_CASE_INSENSITIVE);
    c.eq(st(write_whole_file(&text, b"not an image")), ok)?;
    let (_, f, _) = open_file(&text, GENERIC_READ | GENERIC_EXECUTE, FILE_SHARE_READ, opts);
    c.eq(st(create_image_section(f).0), st(STATUS_INVALID_IMAGE_NOT_MZ))?;
    close(f);

    // The test DLL.
    let path = wide!("\\??\\C:\\Users\\Public\\gala_image_test.dll");
    let path = UnicodeString::new(&path);
    let dll = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    c.eq(st(write_whole_file(&dll, TEST_DLL)), ok)?;
    let (s, f, _) = open_file(&dll, GENERIC_READ | GENERIC_EXECUTE, FILE_SHARE_READ, opts);
    c.eq(st(s), ok)?;
    let (s, sec) = create_image_section(f);
    c.eq(st(s), ok)?;

    // First view.
    let (s, a, size) = map_view(sec, 0, 0, PAGE_READONLY);
    c.ok(s == STATUS_SUCCESS || s == STATUS_IMAGE_NOT_AT_BASE, st(s))?;
    c.ok(size >= 0x1000, size as u64)?;
    c.eq(read_u32(a) as u64 & 0xffff, 0x5a4d)?; // "MZ"
    let (_, mbi) = query(a);
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    c.eq(mbi.protect.into(), PAGE_READONLY.into())?;
    c.eq(mbi.allocation_base as u64, a as u64)?;

    let add = export(a, b"gala_add").ok_or(100u32)?;
    let get = export(a, b"gala_get_value").ok_or(101u32)?;
    let value = export(a, b"GALA_VALUE").ok_or(102u32)?;
    c.eq(query(add).1.protect.into(), PAGE_EXECUTE_READ.into())?;
    c.eq(query(value).1.protect.into(), PAGE_WRITECOPY.into())?;
    // SAFETY: the exports have these signatures (see user/testdll).
    let (add_fn, get_fn): (extern "C" fn(u64, u64) -> u64, extern "C" fn() -> u64) = unsafe {
        (
            core::mem::transmute::<usize, extern "C" fn(u64, u64) -> u64>(add),
            core::mem::transmute::<usize, extern "C" fn() -> u64>(get),
        )
    };
    c.eq(add_fn(2, 3), 5)?;
    c.eq(get_fn(), INITIAL_VALUE)?;

    // Second view: another base, relocated to point at its own data.
    let (s, b, _) = map_view(sec, 0, 0, PAGE_READONLY);
    c.ok(s == STATUS_SUCCESS || s == STATUS_IMAGE_NOT_AT_BASE, st(s))?;
    c.ok(b != a, b as u64)?;
    let get_b = export(b, b"gala_get_value").ok_or(103u32)?;
    // SAFETY: as above.
    let get_b_fn: extern "C" fn() -> u64 = unsafe { core::mem::transmute(get_b) };
    c.eq(get_b_fn(), INITIAL_VALUE)?;

    // Writable image pages are private to each view (copy on write).
    // SAFETY: GALA_VALUE is a writable u64 in view a.
    unsafe { (value as *mut u64).write_volatile(7) };
    c.eq(get_fn(), 7)?;
    c.eq(get_b_fn(), INITIAL_VALUE)?;

    c.eq(st(unmap_view(a)), ok)?;
    c.eq(st(unmap_view(b)), ok)?;
    close(sec);
    close(f);

    // A PE32 IL-only image: laid out by its section table like any image.
    let path = wide!("\\??\\C:\\Users\\Public\\gala_image_test_il.dll");
    let path = UnicodeString::new(&path);
    let il = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    c.eq(st(write_whole_file(&il, IL_DLL)), ok)?;
    let (s, f, _) = open_file(&il, GENERIC_READ | GENERIC_EXECUTE, FILE_SHARE_READ, opts);
    c.eq(st(s), ok)?;
    let (s, sec) = create_image_section(f);
    c.eq(st(s), ok)?;
    let (s, a, size) = map_view(sec, 0, 0, PAGE_READONLY);
    c.ok(s == STATUS_SUCCESS || s == STATUS_IMAGE_NOT_AT_BASE, st(s))?;
    let pe = a + read_u32(a + 0x3c) as usize;
    c.eq(read_u32(pe + 24) as u64 & 0xffff, 0x10b)?; // PE32
    c.eq(read_u32(pe + 24 + 56).into(), size as u64)?; // SizeOfImage
    // The CLI header (data directory 14) at its RVA: cb = 72.
    let cli = read_u32(pe + 24 + 96 + 14 * 8) as usize;
    c.ok(cli != 0 && cli < size, cli as u64)?;
    c.eq(read_u32(a + cli).into(), 72)?;
    c.eq(st(unmap_view(a)), ok)?;
    close(sec);
    close(f);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("image_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

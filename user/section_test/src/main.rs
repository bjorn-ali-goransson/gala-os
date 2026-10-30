//! Pagefile-backed sections and views, with results expected to match real
//! Windows. Prints `section_test: ok` or exits with the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const MEM_MAPPED: u32 = 0x4_0000;
const STATUS_NOT_MAPPED_VIEW: NtStatus = 0xc000_0019;
const STATUS_UNABLE_TO_DELETE_SECTION: NtStatus = 0xc000_001b;
const STATUS_INVALID_VIEW_SIZE: NtStatus = 0xc000_001f;
const STATUS_SECTION_PROTECTION: NtStatus = 0xc000_004e;

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
        print("section_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn peek(addr: usize) -> u64 {
    // SAFETY: callers only pass addresses inside mapped views.
    unsafe { (addr as *const u64).read_volatile() }
}

fn poke(addr: usize, v: u64) {
    // SAFETY: callers only pass addresses inside writable views.
    unsafe { (addr as *mut u64).write_volatile(v) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    // Size is required for pagefile-backed sections.
    c.eq(create_section(None, None, PAGE_READWRITE).0.into(), STATUS_INVALID_PARAMETER_4.into())?;

    let (st, sec) = create_section(None, Some(0x3000), PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    c.eq(query_section(sec).0.into(), ok)?;
    c.eq(query_section(sec).1.into(), SEC_COMMIT.into())?;
    c.eq(query_section(sec).2, 0x3000)?;

    // Two views of the same memory.
    let (st, a, size) = map_view(sec, 0, 0, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    c.eq((a % 0x10000) as u64, 0)?;
    c.eq(size as u64, 0x3000)?;
    let (st, b, _) = map_view(sec, 0, 0, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    c.ok(a != b, b as u64)?;
    c.eq(peek(a + 0x2ff8), 0)?;
    poke(a, 0x1111);
    poke(a + 0x2000, 0x2222);
    c.eq(peek(b), 0x1111)?;
    c.eq(peek(b + 0x2000), 0x2222)?;
    poke(b + 0x1000, 0x3333);
    c.eq(peek(a + 0x1000), 0x3333)?;

    let (st, mbi) = query(a + 0x10);
    c.eq(st.into(), ok)?;
    c.eq(mbi.kind.into(), MEM_MAPPED.into())?;
    c.eq(mbi.state.into(), MEM_COMMIT.into())?;
    c.eq(mbi.protect.into(), PAGE_READWRITE.into())?;
    c.eq(mbi.allocation_base as u64, a as u64)?;
    c.eq(mbi.region_size as u64, 0x3000)?;

    // A read-only view sees the same data.
    let (st, r, _) = map_view(sec, 0, 0, PAGE_READONLY);
    c.eq(st.into(), ok)?;
    c.eq(peek(r + 0x1000), 0x3333)?;
    c.eq(query(r).1.protect.into(), PAGE_READONLY.into())?;

    // Views are not freed with NtFreeVirtualMemory.
    c.eq(free(a, 0, MEM_RELEASE).0.into(), STATUS_UNABLE_TO_DELETE_SECTION.into())?;

    // Unmapping one view leaves the others; views outlive the handle.
    c.eq(unmap_view(a + 0x1234).into(), ok)?;
    c.eq(query(a).1.state.into(), MEM_FREE.into())?;
    c.eq(unmap_view(a).into(), STATUS_NOT_MAPPED_VIEW.into())?;
    c.eq(close(sec).into(), ok)?;
    c.eq(peek(b), 0x1111)?;
    poke(b + 8, 0x4444);
    c.eq(peek(r + 8), 0x4444)?;
    c.eq(unmap_view(b).into(), ok)?;
    c.eq(unmap_view(r).into(), ok)?;

    // Protection and size limits.
    let (_, ro) = create_section(None, Some(0x1000), PAGE_READONLY);
    c.eq(map_view(ro, 0, 0, PAGE_READWRITE).0.into(), STATUS_SECTION_PROTECTION.into())?;
    c.eq(map_view(ro, 0, 0x2000, PAGE_READONLY).0.into(), STATUS_INVALID_VIEW_SIZE.into())?;
    close(ro);

    // A view at an offset.
    let (_, big) = create_section(None, Some(0x20000), PAGE_READWRITE);
    let (st, whole, _) = map_view(big, 0, 0, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    poke(whole + 0x10008, 0x5555);
    let (st, part, part_size) = map_view(big, 0x10000, 0x1000, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    c.eq(part_size as u64, 0x1000)?;
    c.eq(peek(part + 8), 0x5555)?;
    unmap_view(part);
    unmap_view(whole);
    close(big);

    // Named sections are shared through the object namespace.
    let name = wide!("\\BaseNamedObjects\\gala_section_test");
    let name = UnicodeString::new(&name);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let (st, named) = create_section(Some(&attrs), Some(0x1000), PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    let (st, v1, _) = map_view(named, 0, 0, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    poke(v1, 0x6666);
    let (st, opened) = open_section(&attrs);
    c.eq(st.into(), ok)?;
    let (st, v2, _) = map_view(opened, 0, 0, PAGE_READONLY);
    c.eq(st.into(), ok)?;
    c.eq(peek(v2), 0x6666)?;
    unmap_view(v1);
    unmap_view(v2);
    close(named);
    close(opened);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("section_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

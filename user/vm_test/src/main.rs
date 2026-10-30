//! Virtual memory services, with results expected to match real Windows.
//! Prints `vm_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

struct Checks(u32);

impl Checks {
    fn eq<T: PartialEq + Copy + Into<u64>>(&mut self, got: T, want: T) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        print("vm_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got.into() as u32));
        print(" want ");
        write_file(stdout(), &hex32(want.into() as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // Reserve + commit 64 KiB anywhere.
    let (st, base, size) = allocate(0, 0x10000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq((base % 0x10000) as u64, 0)?;
    c.eq(size as u64, 0x10000)?;

    // Fresh memory reads as zero and is writable.
    let mem = base as *mut u64;
    for i in 0..(0x10000 / 8) {
        // SAFETY: inside the committed read-write region.
        unsafe {
            if mem.add(i).read_volatile() != 0 {
                return Err(100);
            }
            mem.add(i).write_volatile(i as u64);
        }
    }
    // SAFETY: as above.
    c.eq(unsafe { mem.add(0x1fff).read_volatile() }, 0x1fff)?;

    let (st, mbi) = query(base + 0x10);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(mbi.base_address as u64, base as u64)?;
    c.eq(mbi.allocation_base as u64, base as u64)?;
    c.eq(mbi.region_size as u64, 0x10000)?;
    c.eq(mbi.state, MEM_COMMIT)?;
    c.eq(mbi.protect, PAGE_READWRITE)?;
    c.eq(mbi.kind, MEM_PRIVATE)?;

    // Read-only first page; the old protection comes back.
    let (st, old) = protect(base, 0x1000, PAGE_READONLY);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(old, PAGE_READWRITE)?;
    let (_, mbi) = query(base);
    c.eq(mbi.protect, PAGE_READONLY)?;
    c.eq(mbi.region_size as u64, 0x1000)?;
    // SAFETY: still readable.
    c.eq(unsafe { mem.read_volatile() }, 0)?;

    // Decommit the second page.
    let (st, _, _) = free(base + 0x1000, 0x1000, MEM_DECOMMIT);
    c.eq(st, STATUS_SUCCESS)?;
    let (_, mbi) = query(base + 0x1000);
    c.eq(mbi.state, MEM_RESERVE)?;
    c.eq(mbi.region_size as u64, 0x1000)?;

    // Recommitting gives zeroed memory again.
    let (st, _, _) = allocate(base + 0x1000, 0x1000, MEM_COMMIT, PAGE_READWRITE);
    c.eq(st, STATUS_SUCCESS)?;
    // SAFETY: committed again.
    c.eq(unsafe { mem.add(0x1000 / 8).read_volatile() }, 0)?;

    // Release the whole reservation.
    let (st, _, _) = free(base, 0, MEM_RELEASE);
    c.eq(st, STATUS_SUCCESS)?;
    let (_, mbi) = query(base);
    c.eq(mbi.state, MEM_FREE)?;

    // Fixed-address reservations and conflicts.
    let (st, b, _) = allocate(0x5000_0000, 0x20000, MEM_RESERVE, PAGE_READWRITE);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(b as u64, 0x5000_0000)?;
    let (st, _, _) = allocate(0x5001_0000, 0x1000, MEM_RESERVE, PAGE_READWRITE);
    c.eq(st, STATUS_CONFLICTING_ADDRESSES)?;
    let (st, _, _) = free(0x5000_0000, 0, MEM_RELEASE);
    c.eq(st, STATUS_SUCCESS)?;

    // Invalid protection.
    let (st, _, _) = allocate(0, 0x1000, MEM_RESERVE | MEM_COMMIT, 0);
    c.eq(st, STATUS_INVALID_PAGE_PROTECTION)?;

    // The image is MEM_IMAGE; its headers are read-only.
    let (st, mbi) = query(image_base());
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(mbi.kind, MEM_IMAGE)?;
    c.eq(mbi.state, MEM_COMMIT)?;
    c.eq(mbi.protect, PAGE_READONLY)?;
    c.eq(mbi.allocation_protect, PAGE_EXECUTE_WRITECOPY)?;

    // Our own stack is committed private read-write memory.
    let local = 0u64;
    let (_, mbi) = query(&raw const local as usize);
    c.eq(mbi.state, MEM_COMMIT)?;
    c.eq(mbi.protect, PAGE_READWRITE)?;
    c.eq(mbi.kind, MEM_PRIVATE)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("vm_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

//! Mutants and semaphores, with results expected to match real Windows.
//! Their system call numbers depend on the build, so the services are
//! called through ntdll's exports (found in the address space): the real
//! ntdll's stubs on Windows, gala's test ntdll on gala. Prints
//! `sync_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
const STATUS_ABANDONED_WAIT_0: NtStatus = 0x0000_0080;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const STATUS_MUTANT_NOT_OWNED: NtStatus = 0xc000_0046;
const STATUS_SEMAPHORE_LIMIT_EXCEEDED: NtStatus = 0xc000_0047;
const MUTANT_ALL_ACCESS: u32 = 0x1f_0001;
const SEMAPHORE_ALL_ACCESS: u32 = 0x1f_0003;

type Create = extern "system" fn(*mut Handle, u32, *const ObjectAttributes, u8) -> NtStatus;
type CreateSemaphore =
    extern "system" fn(*mut Handle, u32, *const ObjectAttributes, i32, i32) -> NtStatus;
type Open = extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> NtStatus;
type ReleaseMutant = extern "system" fn(Handle, *mut i32) -> NtStatus;
type ReleaseSemaphore = extern "system" fn(Handle, i32, *mut i32) -> NtStatus;
type Query = extern "system" fn(Handle, u32, *mut u8, u32, *mut u32) -> NtStatus;

struct Api {
    create_mutant: Create,
    open_mutant: Open,
    release_mutant: ReleaseMutant,
    query_mutant: Query,
    create_semaphore: CreateSemaphore,
    open_semaphore: Open,
    release_semaphore: ReleaseSemaphore,
    query_semaphore: Query,
}

/// The ntdll exports, or the number of the missing one.
fn api() -> Result<Api, u32> {
    let ntdll = find_ntdll().ok_or(1u32)?;
    let get = |name: &[u8], n: u32| find_export(ntdll, name).ok_or(n);
    // SAFETY: the documented signatures of these ntdll exports.
    unsafe {
        Ok(Api {
            create_mutant: core::mem::transmute::<usize, Create>(get(b"NtCreateMutant", 2)?),
            open_mutant: core::mem::transmute::<usize, Open>(get(b"NtOpenMutant", 3)?),
            release_mutant: core::mem::transmute::<usize, ReleaseMutant>(get(
                b"NtReleaseMutant",
                4,
            )?),
            query_mutant: core::mem::transmute::<usize, Query>(get(b"NtQueryMutant", 5)?),
            create_semaphore: core::mem::transmute::<usize, CreateSemaphore>(get(
                b"NtCreateSemaphore",
                6,
            )?),
            open_semaphore: core::mem::transmute::<usize, Open>(get(b"NtOpenSemaphore", 7)?),
            release_semaphore: core::mem::transmute::<usize, ReleaseSemaphore>(get(
                b"NtReleaseSemaphore",
                8,
            )?),
            query_semaphore: core::mem::transmute::<usize, Query>(get(b"NtQuerySemaphore", 9)?),
        })
    }
}

/// MUTANT_BASIC_INFORMATION: (CurrentCount, OwnedByCaller, AbandonedState).
fn mutant_info(api: &Api, h: Handle) -> (NtStatus, i32, u8, u8) {
    let mut b = [0xffu8; 8];
    let mut ret = 0;
    let s = (api.query_mutant)(h, 0, b.as_mut_ptr(), 8, &mut ret);
    (s, i32::from_le_bytes(b[0..4].try_into().unwrap()), b[4], b[5])
}

/// SEMAPHORE_BASIC_INFORMATION: (CurrentCount, MaximumCount).
fn semaphore_info(api: &Api, h: Handle) -> (NtStatus, i32, i32) {
    let mut b = [0xffu8; 8];
    let mut ret = 0;
    let s = (api.query_semaphore)(h, 0, b.as_mut_ptr(), 8, &mut ret);
    (
        s,
        i32::from_le_bytes(b[0..4].try_into().unwrap()),
        i32::from_le_bytes(b[4..8].try_into().unwrap()),
    )
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn st(&mut self, got: NtStatus, want: NtStatus) -> Result<(), u32> {
        self.eq(got.into(), want.into())
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("sync_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn i(v: i32) -> u64 {
    v as u32 as u64
}

static SHARED: AtomicUsize = AtomicUsize::new(0);
static RELEASE_FN: AtomicUsize = AtomicUsize::new(0);
static WORKER_RESULT: AtomicU64 = AtomicU64::new(0xffff_ffff);

/// Takes the mutant and exits without releasing it (abandoning it).
extern "C" fn take_and_exit(_: usize) -> ! {
    let s = wait(Handle(SHARED.load(Ordering::SeqCst)), None);
    WORKER_RESULT.store(s.into(), Ordering::SeqCst);
    exit_thread(0)
}

/// Tries to release a mutant it does not own.
extern "C" fn release_foreign(_: usize) -> ! {
    // SAFETY: stored from the resolved NtReleaseMutant export.
    let release =
        unsafe { core::mem::transmute::<usize, ReleaseMutant>(RELEASE_FN.load(Ordering::SeqCst)) };
    let mut prev = 0;
    let s = release(Handle(SHARED.load(Ordering::SeqCst)), &mut prev);
    WORKER_RESULT.store(s.into(), Ordering::SeqCst);
    exit_thread(0)
}

fn run_thread(c: &mut Checks, entry: extern "C" fn(usize) -> !) -> Result<(), u32> {
    WORKER_RESULT.store(0xffff_ffff, Ordering::SeqCst);
    let (s, t, _) = create_thread(entry, 0, false);
    c.st(s, STATUS_SUCCESS)?;
    c.st(wait(t, None), STATUS_SUCCESS)?;
    close(t);
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let api = api().inspect_err(|&n| {
        let _ = c.ok(false, n.into());
    })?;
    let ok = STATUS_SUCCESS;
    let null = core::ptr::null();

    // A mutant created owned: recursion counts down from 0.
    let mut m = Handle(0);
    c.st((api.create_mutant)(&mut m, MUTANT_ALL_ACCESS, null, 1), ok)?;
    let (s, count, owned, abandoned) = mutant_info(&api, m);
    c.st(s, ok)?;
    c.ok(count == 0 && owned == 1 && abandoned == 0, i(count))?;
    c.st(wait(m, Some(0)), ok)?; // the owner may take it again
    c.eq(i(mutant_info(&api, m).1), i(-1))?;
    let mut prev = 7;
    c.st((api.release_mutant)(m, &mut prev), ok)?;
    c.eq(i(prev), i(-1))?;
    c.st((api.release_mutant)(m, &mut prev), ok)?;
    c.eq(i(prev), 0)?;
    let (_, count, owned, _) = mutant_info(&api, m);
    c.ok(count == 1 && owned == 0, i(count))?;
    c.st((api.release_mutant)(m, &mut prev), STATUS_MUTANT_NOT_OWNED)?;

    // Abandoned by a thread that exits while owning it.
    SHARED.store(m.0, Ordering::SeqCst);
    run_thread(&mut c, take_and_exit)?;
    c.eq(WORKER_RESULT.load(Ordering::SeqCst), 0)?;
    let (_, count, owned, abandoned) = mutant_info(&api, m);
    c.ok(count == 1 && owned == 0 && abandoned == 1, i(count))?;
    c.st(wait(m, Some(0)), STATUS_ABANDONED_WAIT_0)?;
    c.eq(mutant_info(&api, m).2.into(), 1)?; // ours now

    // Only the owner may release it.
    RELEASE_FN.store(api.release_mutant as usize, Ordering::SeqCst);
    run_thread(&mut c, release_foreign)?;
    c.eq(WORKER_RESULT.load(Ordering::SeqCst), STATUS_MUTANT_NOT_OWNED.into())?;
    c.st((api.release_mutant)(m, &mut prev), ok)?;

    // Named mutants open by name.
    let name = wide!("\\BaseNamedObjects\\gala_sync_test_mutant");
    let name = UnicodeString::new(&name);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let (mut named, mut opened) = (Handle(0), Handle(0));
    c.st((api.create_mutant)(&mut named, MUTANT_ALL_ACCESS, &attrs, 0), ok)?;
    c.st((api.open_mutant)(&mut opened, MUTANT_ALL_ACCESS, &attrs), ok)?;
    c.st(wait(opened, Some(0)), ok)?;
    c.eq(mutant_info(&api, named).2.into(), 1)?; // the same mutant, owned
    c.st((api.release_mutant)(named, &mut prev), ok)?;
    close(opened);
    close(named);

    // Semaphores.
    let mut sem = Handle(0);
    c.st((api.create_semaphore)(&mut sem, SEMAPHORE_ALL_ACCESS, null, 2, 3), ok)?;
    let (s, count, max) = semaphore_info(&api, sem);
    c.st(s, ok)?;
    c.ok(count == 2 && max == 3, i(count))?;
    c.st(wait(sem, Some(0)), ok)?;
    c.st(wait(sem, Some(0)), ok)?;
    c.st(wait(sem, Some(0)), STATUS_TIMEOUT)?;
    c.eq(i(semaphore_info(&api, sem).1), 0)?;
    c.st((api.release_semaphore)(sem, 2, &mut prev), ok)?;
    c.eq(i(prev), 0)?;
    c.st((api.release_semaphore)(sem, 2, &mut prev), STATUS_SEMAPHORE_LIMIT_EXCEEDED)?;
    c.eq(i(semaphore_info(&api, sem).1), 2)?;
    c.st((api.release_semaphore)(sem, 0, &mut prev), STATUS_INVALID_PARAMETER)?;
    let mut bad = Handle(0);
    c.st(
        (api.create_semaphore)(&mut bad, SEMAPHORE_ALL_ACCESS, null, 4, 3),
        STATUS_INVALID_PARAMETER,
    )?;
    c.st(
        (api.create_semaphore)(&mut bad, SEMAPHORE_ALL_ACCESS, null, 0, 0),
        STATUS_INVALID_PARAMETER,
    )?;
    let sname = wide!("\\BaseNamedObjects\\gala_sync_test_semaphore");
    let sname = UnicodeString::new(&sname);
    let sattrs = ObjectAttributes::new(&sname, Handle(0), OBJ_CASE_INSENSITIVE);
    let (mut s1, mut s2) = (Handle(0), Handle(0));
    c.st((api.create_semaphore)(&mut s1, SEMAPHORE_ALL_ACCESS, &sattrs, 1, 5), ok)?;
    c.st((api.open_semaphore)(&mut s2, SEMAPHORE_ALL_ACCESS, &sattrs), ok)?;
    c.eq(i(semaphore_info(&api, s2).2), 5)?;
    close(s2);
    close(s1);

    // Wait-all on a mutant and a semaphore takes both.
    c.st(wait_multiple(&[m, sem], 0, Some(0)), ok)?; // WaitAll
    c.eq(mutant_info(&api, m).2.into(), 1)?;
    c.eq(i(semaphore_info(&api, sem).1), 1)?;
    c.st((api.release_mutant)(m, &mut prev), ok)?;
    close(sem);
    close(m);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("sync_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

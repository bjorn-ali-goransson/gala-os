//! Micro-benchmarks that run unchanged on Windows and gala (ROADMAP P1;
//! image_load: H8).
//! Each prints `bench: <name> ns_per_op <n>` (wall time from the
//! performance counter, divided by the operations); `cargo xtask bench`
//! collects them into target/report/bench.json and user/bin/run-all.ps1
//! runs the same binary on Windows. Numbers under emulation (QEMU TCG) are
//! only comparable with each other, not with hardware. Ends with
//! `bench: done`.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;

/// gala's kernel32.dll (87 KiB): the image that `image_load` loads, as
/// the loader loads each DLL of each new process.
static IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/x86_64-pc-windows-msvc/release/kernel32.dll"
));

/// Runs `op` `n` times and prints ns per operation.
fn bench(name: &str, n: u64, mut op: impl FnMut()) {
    let (start, freq) = performance_counter();
    for _ in 0..n {
        op();
    }
    let (end, _) = performance_counter();
    report(name, (end - start) as u64, freq as u64, n);
}

fn report(name: &str, ticks: u64, freq: u64, n: u64) {
    let ns = (u128::from(ticks) * 1_000_000_000 / u128::from(freq.max(1))) as u64 / n.max(1);
    print("bench: ");
    print(name);
    print(" ns_per_op ");
    let mut digits = [0u8; 20];
    let (mut v, mut i) = (ns, digits.len());
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    write_file(stdout(), &digits[i..]);
    print("\r\n");
}

fn pin(h: Handle, cpu: u32) {
    set_thread(h, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes());
}

static PING: AtomicU64 = AtomicU64::new(0);
static PONG: AtomicU64 = AtomicU64::new(0);
const ROUNDS: u64 = 500;

extern "C" fn ponger(cpu: usize) -> ! {
    pin(CURRENT_THREAD, cpu as u32);
    let (ping, pong) = (
        Handle(PING.load(Ordering::Relaxed) as usize),
        Handle(PONG.load(Ordering::Relaxed) as usize),
    );
    for _ in 0..ROUNDS {
        wait(ping, None);
        set_event(pong);
    }
    exit_thread(STATUS_SUCCESS)
}

/// Event round trips between this thread (on CPU 0) and one on `cpu`.
fn ping_pong(name: &str, cpu: u32) {
    pin(CURRENT_THREAD, 0);
    let (_, ping) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
    let (_, pong) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
    PING.store(ping.0 as u64, Ordering::Relaxed);
    PONG.store(pong.0 as u64, Ordering::Relaxed);
    let (_, h, _) = create_thread(ponger, cpu as usize, false);
    // Let it start and pin itself first.
    delay(-100_000);
    let (start, freq) = performance_counter();
    for _ in 0..ROUNDS {
        set_event(ping);
        wait(pong, None);
    }
    let (end, _) = performance_counter();
    report(name, (end - start) as u64, freq as u64, ROUNDS);
    wait(h, None);
    for handle in [h, ping, pong] {
        close(handle);
    }
}

extern "C" fn exit_now(_: usize) -> ! {
    exit_thread(STATUS_SUCCESS)
}

fn main() -> u32 {
    let mut b = [0u8; 64];
    query_system(0, &mut b);
    let cpus = u32::from(b[56]);

    bench("null_syscall", 5000, || {
        performance_counter();
    });
    bench("handle_create_close", 1000, || {
        let (_, e) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
        close(e);
    });
    bench("virtual_alloc_free_64k", 300, || {
        let (_, base, _) = allocate(0, 0x10000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        free(base, 0, MEM_RELEASE);
    });
    // Demand-zero faults: first touch of committed pages.
    const PAGES: usize = 256;
    let (_, base, _) = allocate(0, PAGES * 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    let mut page = 0;
    bench("demand_zero_fault", PAGES as u64, || {
        // SAFETY: committed read-write pages, each touched once.
        unsafe { ((base + page * 0x1000) as *mut u8).write_volatile(1) };
        page += 1;
    });
    free(base, 0, MEM_RELEASE);
    let (_, section) = create_section(None, Some(0x10000), PAGE_READWRITE);
    bench("section_map_unmap_64k", 300, || {
        let (_, view, _) = map_view(section, 0, 0, PAGE_READWRITE);
        unmap_view(view);
    });
    close(section);
    // Loading a DLL as the loader does: open the file, an image section,
    // a view at its preferred base, then unmap and close everything.
    let path = wide!("\\??\\C:\\Users\\Public\\gala_bench_image.dll");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let opts = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    let (_, f, _) = create_file(&attrs, GENERIC_WRITE | DELETE, 0, FILE_OVERWRITE_IF, opts);
    write_file_info(f, IMAGE);
    close(f);
    bench("image_load", 50, || {
        let share = FILE_SHARE_READ | FILE_SHARE_DELETE;
        let (_, f, _) = open_file(&attrs, GENERIC_READ | GENERIC_EXECUTE, share, opts);
        let (_, section) = create_image_section(f);
        let (_, view, _) = map_view(section, 0, 0, PAGE_EXECUTE_WRITECOPY);
        unmap_view(view);
        close(section);
        close(f);
    });
    let (_, f, _) = open_file(&attrs, DELETE, FILE_SHARE_READ | FILE_SHARE_DELETE, opts);
    set_file(f, 13, &[1]); // FileDispositionInformation: delete
    close(f);
    bench("thread_create_exit", 100, || {
        let (_, h, _) = create_thread(exit_now, 0, false);
        wait(h, None);
        close(h);
    });
    ping_pong("event_pingpong_same_cpu", 0);
    if cpus >= 2 {
        ping_pong("event_pingpong_cross_cpu", 1);
    }
    print("bench: done\r\n");
    0
}

entry!(main);

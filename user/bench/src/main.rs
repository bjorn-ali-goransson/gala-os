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

/// The DLL `image_load` loads, as the loader loads each DLL of each new
/// process: made here (`make_image`), not taken from the build, so that
/// bench.exe depends on its own source only (it embedded the built
/// kernel32.dll, and every change to kernel32 changed bench.exe).
/// Sections: .text (256 KiB), .rdata (256 KiB, a 64-bit address every
/// 256 bytes), .data (64 KiB), .reloc (those addresses' fixups).
const IMAGE_BASE: u64 = 0x1_8000_0000;
const TEXT_SIZE: usize = 0x4_0000;
const RDATA_SIZE: usize = 0x4_0000;
const DATA_SIZE: usize = 0x1_0000;
/// Fixups per 4 KiB page of .rdata, and the size of its block (header,
/// entries).
const FIXUPS_PER_PAGE: usize = 16;
const RELOC_BLOCK: usize = 8 + 2 * FIXUPS_PER_PAGE;
const RELOC_SIZE: usize = RDATA_SIZE / 0x1000 * RELOC_BLOCK;
const HEADERS: usize = 0x400;
/// Sections: name, RVA, size, file offset, characteristics.
const SECTIONS: [(&[u8; 8], usize, usize, usize, u32); 4] = [
    (b".text\0\0\0", 0x1000, TEXT_SIZE, HEADERS, 0x6000_0020),
    (b".rdata\0\0", 0x1000 + TEXT_SIZE, RDATA_SIZE, HEADERS + TEXT_SIZE, 0x4000_0040),
    (
        b".data\0\0\0",
        0x1000 + TEXT_SIZE + RDATA_SIZE,
        DATA_SIZE,
        HEADERS + TEXT_SIZE + RDATA_SIZE,
        0xc000_0040,
    ),
    (
        b".reloc\0\0",
        0x1000 + TEXT_SIZE + RDATA_SIZE + DATA_SIZE,
        RELOC_SIZE,
        HEADERS + TEXT_SIZE + RDATA_SIZE + DATA_SIZE,
        0x4200_0040,
    ),
];
/// The file's size (RELOC_SIZE is a multiple of the file alignment).
const IMAGE_FILE: usize = HEADERS + TEXT_SIZE + RDATA_SIZE + DATA_SIZE + RELOC_SIZE;

/// Writes the DLL into `pe` (IMAGE_FILE bytes, zeroed): a PE32+ DLL as
/// the PE format's documentation lays one out, with no entry point.
fn make_image(pe: &mut [u8]) {
    let put =
        |pe: &mut [u8], at: usize, bytes: &[u8]| pe[at..at + bytes.len()].copy_from_slice(bytes);
    let (reloc_rva, size_of_image) =
        (SECTIONS[3].1, (SECTIONS[3].1 + RELOC_SIZE).div_ceil(0x1000) * 0x1000);
    put(pe, 0, b"MZ");
    put(pe, 0x3c, &0x80u32.to_le_bytes());
    put(pe, 0x80, b"PE\0\0");
    // The file header: AMD64, 4 sections, the optional header's size,
    // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE | DLL.
    put(pe, 0x84, &0x8664u16.to_le_bytes());
    put(pe, 0x86, &(SECTIONS.len() as u16).to_le_bytes());
    put(pe, 0x94, &240u16.to_le_bytes());
    put(pe, 0x96, &0x2022u16.to_le_bytes());
    let opt = 0x98;
    put(pe, opt, &0x20bu16.to_le_bytes());
    put(pe, opt + 4, &(TEXT_SIZE as u32).to_le_bytes());
    put(pe, opt + 8, &((RDATA_SIZE + DATA_SIZE + RELOC_SIZE) as u32).to_le_bytes());
    put(pe, opt + 20, &(SECTIONS[0].1 as u32).to_le_bytes());
    put(pe, opt + 24, &IMAGE_BASE.to_le_bytes());
    put(pe, opt + 32, &0x1000u32.to_le_bytes());
    put(pe, opt + 36, &0x200u32.to_le_bytes());
    // Operating system and subsystem versions 6.0.
    put(pe, opt + 40, &6u16.to_le_bytes());
    put(pe, opt + 48, &6u16.to_le_bytes());
    put(pe, opt + 56, &(size_of_image as u32).to_le_bytes());
    put(pe, opt + 60, &(HEADERS as u32).to_le_bytes());
    // Windows GUI; HIGH_ENTROPY_VA | DYNAMIC_BASE | NX_COMPAT.
    put(pe, opt + 68, &2u16.to_le_bytes());
    put(pe, opt + 70, &0x0160u16.to_le_bytes());
    for (i, v) in [0x10_0000u64, 0x1000, 0x10_0000, 0x1000].iter().enumerate() {
        put(pe, opt + 72 + 8 * i, &v.to_le_bytes());
    }
    put(pe, opt + 108, &16u32.to_le_bytes());
    // Data directory 5: the base relocations.
    put(pe, opt + 112 + 5 * 8, &(reloc_rva as u32).to_le_bytes());
    put(pe, opt + 112 + 5 * 8 + 4, &(RELOC_SIZE as u32).to_le_bytes());
    for (i, &(name, rva, size, file, flags)) in SECTIONS.iter().enumerate() {
        let h = opt + 240 + 40 * i;
        put(pe, h, name);
        put(pe, h + 8, &(size as u32).to_le_bytes());
        put(pe, h + 12, &(rva as u32).to_le_bytes());
        put(pe, h + 16, &(size as u32).to_le_bytes());
        put(pe, h + 20, &(file as u32).to_le_bytes());
        put(pe, h + 36, &flags.to_le_bytes());
    }
    // .text: int3; .data: a pattern.
    pe[SECTIONS[0].3..SECTIONS[0].3 + TEXT_SIZE].fill(0xcc);
    for (i, b) in pe[SECTIONS[2].3..SECTIONS[2].3 + DATA_SIZE].iter_mut().enumerate() {
        *b = i as u8;
    }
    // .rdata's addresses (each the address of the next) and their fixups
    // (IMAGE_REL_BASED_DIR64), a block per page.
    let (rdata_rva, rdata) = (SECTIONS[1].1, SECTIONS[1].3);
    for page in 0..RDATA_SIZE / 0x1000 {
        let block = SECTIONS[3].3 + page * RELOC_BLOCK;
        put(pe, block, &((rdata_rva + page * 0x1000) as u32).to_le_bytes());
        put(pe, block + 4, &(RELOC_BLOCK as u32).to_le_bytes());
        for k in 0..FIXUPS_PER_PAGE {
            let offset = k * 0x100;
            let target = IMAGE_BASE + (rdata_rva + page * 0x1000 + offset + 0x100) as u64;
            put(pe, rdata + page * 0x1000 + offset, &target.to_le_bytes());
            put(pe, block + 8 + 2 * k, &((10u16 << 12) | offset as u16).to_le_bytes());
        }
    }
}

/// Grows a PE image's last section by `extra` bytes of virtual size (and
/// SizeOfImage with it): pages past the file's data, zero when mapped.
fn grow_last_section(pe: &mut [u8], extra: u32) {
    let u16_at = |pe: &[u8], o: usize| u16::from_le_bytes([pe[o], pe[o + 1]]) as usize;
    let u32_at = |pe: &[u8], o: usize| u32::from_le_bytes([pe[o], pe[o + 1], pe[o + 2], pe[o + 3]]);
    let file = u32_at(pe, 0x3c) as usize + 4;
    let sections = u16_at(pe, file + 2);
    let opt = file + 20;
    let last = opt + u16_at(pe, file + 16) + (sections - 1) * 40;
    let size_of_image = u32_at(pe, opt + 56) + extra;
    pe[opt + 56..opt + 60].copy_from_slice(&size_of_image.to_le_bytes());
    let virtual_size = u32_at(pe, last + 8) + extra;
    pe[last + 8..last + 12].copy_from_slice(&virtual_size.to_le_bytes());
}

/// Runs `op` `n` times and prints ns per operation.
fn bench(name: &str, n: u64, mut op: impl FnMut()) {
    let (start, freq) = performance_counter();
    for _ in 0..n {
        op();
    }
    let (end, _) = performance_counter();
    report(name, (end - start) as u64, freq as u64, n);
}

/// Says so if `make_image`'s DLL did not load (its numbers would measure
/// a failure): `bench: <name> did not load: section <status> view
/// <status>`.
fn check_load(name: &str, (section, view): (NtStatus, NtStatus)) {
    if section as i32 >= 0 && view as i32 >= 0 {
        return;
    }
    print("bench: ");
    print(name);
    print(" did not load: section ");
    hex(section);
    print(" view ");
    hex(view);
    print("\r\n");
}

fn hex(v: u32) {
    let mut digits = *b"0x00000000";
    for i in 0..8 {
        digits[2 + i] = b"0123456789abcdef"[(v >> (28 - 4 * i)) as usize & 0xf];
    }
    write_file(stdout(), &digits);
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

/// Waits on the event `handle` names, then exits.
extern "C" fn wait_then_exit(handle: usize) -> ! {
    wait(Handle(handle), None);
    exit_thread(STATUS_SUCCESS)
}

/// How many other threads `null_syscall_200_threads` keeps alive (a
/// browser has hundreds).
const MANY: usize = 200;

fn main() -> u32 {
    let mut b = [0u8; 64];
    query_system(0, &mut b);
    let cpus = u32::from(b[56]);

    // Pure user-mode work (a xorshift step per operation, no system
    // call): the CPU's own speed, and user time the sampling profiler's
    // scenario can count on (events_sample).
    let mut x = 0x2545_f491_4f6c_dd1du64;
    bench("cpu_loop", 64_000_000, || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x = core::hint::black_box(x);
    });
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
    // Committing and decommitting one page of a reservation: the same
    // in a fresh one and in one split into 4096 runs (every other page
    // committed, as a JIT's or allocator's reservations become). The cost
    // must not grow with the runs; compare the two in one run.
    const RESERVED: usize = 4096 * 0x1000;
    for (name, split) in [("commit_decommit_page", false), ("commit_decommit_page_4096_runs", true)]
    {
        let (_, r, _) = allocate(0, RESERVED, MEM_RESERVE, PAGE_READWRITE);
        if split {
            for p in (0..RESERVED / 0x1000).step_by(2) {
                allocate(r + p * 0x1000, 0x1000, MEM_COMMIT, PAGE_READWRITE);
            }
        }
        let middle = r + RESERVED / 2 + 0x1000;
        bench(name, 300, || {
            allocate(middle, 0x1000, MEM_COMMIT, PAGE_READWRITE);
            free(middle, 0x1000, MEM_DECOMMIT);
        });
        free(r, 0, MEM_RELEASE);
    }
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
    let (_, image_buf, _) = allocate(0, IMAGE_FILE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    // SAFETY: a fresh (zeroed) read-write allocation of IMAGE_FILE bytes.
    let image = unsafe { core::slice::from_raw_parts_mut(image_buf as *mut u8, IMAGE_FILE) };
    make_image(image);
    write_file_info(f, image);
    close(f);
    // The section's and the view's statuses (a view not at the preferred
    // base is STATUS_IMAGE_NOT_AT_BASE, a success).
    let load = || {
        let share = FILE_SHARE_READ | FILE_SHARE_DELETE;
        let (_, f, _) = open_file(&attrs, GENERIC_READ | GENERIC_EXECUTE, share, opts);
        let (made, section) = create_image_section(f);
        let (mapped, view, _) = map_view(section, 0, 0, PAGE_EXECUTE_WRITECOPY);
        unmap_view(view);
        close(section);
        close(f);
        (made, mapped)
    };
    // Once untimed: the first load lays the file out (an OS caches that,
    // as Windows does its image sections); the bench measures the loads
    // after it.
    check_load("image_load", load());
    bench("image_load", 50, || {
        load();
    });
    let (_, f, _) = open_file(&attrs, DELETE, FILE_SHARE_READ | FILE_SHARE_DELETE, opts);
    set_file(f, 13, &[1]); // FileDispositionInformation: delete
    close(f);
    // The same with the image 32 MiB larger (its last section's virtual
    // size grown: zero pages), not touched after mapping: a view's cost
    // must not grow with the image (chrome.dll is 340 MB; its processes
    // touch a fraction of it).
    grow_last_section(image, 32 << 20);
    let (_, f, _) = create_file(&attrs, GENERIC_WRITE | DELETE, 0, FILE_OVERWRITE_IF, opts);
    write_file_info(f, image);
    close(f);
    check_load("image_load_32m_untouched", load());
    bench("image_load_32m_untouched", 20, || {
        load();
    });
    let (_, f, _) = open_file(&attrs, DELETE, FILE_SHARE_READ | FILE_SHARE_DELETE, opts);
    set_file(f, 13, &[1]); // FileDispositionInformation: delete
    close(f);
    free(image_buf, 0, MEM_RELEASE);
    bench("thread_create_exit", 100, || {
        let (_, h, _) = create_thread(exit_now, 0, false);
        wait(h, None);
        close(h);
    });
    // The same call with many other threads alive: what a system call
    // costs in a process like a browser (its cost must not grow with the
    // number of threads).
    let (_, gate) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    let mut waiters = [Handle(0); MANY];
    for w in waiters.iter_mut() {
        let (_, h, _) = create_thread(wait_then_exit, gate.0, false);
        *w = h;
    }
    bench("null_syscall_200_threads", 5000, || {
        performance_counter();
    });
    set_event(gate);
    for w in waiters {
        wait(w, None);
        close(w);
    }
    close(gate);
    ping_pong("event_pingpong_same_cpu", 0);
    if cpus >= 2 {
        ping_pong("event_pingpong_cross_cpu", 1);
    }
    print("bench: done\r\n");
    0
}

entry!(main);

# Where the time goes

Measurements of gala and how to take them. Under QEMU TCG every
instruction is translated, so only ratios between gala runs mean
something; Windows numbers come from `user/bin/run-all.ps1` on a real
machine. Each measurement below keeps its date and how it was taken.

## How to measure

- **The `nt:` line** at the end of every run (`kernel/src/nt/stats.rs`,
  always on): system calls (count, time; waits counted apart, as their
  time is mostly sleeping), user faults the kernel resolves, processes
  created. Two clock reads per event, so it does not change what it
  measures. It is in the scenario's serial log,
  `target/<profile>/test-<scenario>.serial.log` (examples under H8).
- **The event stream** (`XTASK_EVENTS=syscall,... cargo xtask boot-test
  <scenario>`; `kernel/src/events.rs`, docs/OBSERVABILITY.md "Event
  stream"): one JSON line per event with arguments and per-call time
  (`ns`). `cargo xtask events <events.jsonl>` sums system calls by
  service; for sizes or other arguments, group the lines yourself by
  `name` and the argument. Each line is a debug-port write, slow under
  emulation (with `all` the lua scenario takes 3x as long, faults
  dominating), so use it for shares and arguments, not totals.
- **Micro-benchmarks**: `user/bench` runs unchanged on Windows and gala
  and prints `bench: <name> ns_per_op <n>` (wall time from the
  performance counter): `cpu_loop` (a xorshift step in user mode, no
  system call: the CPU's own speed; also the user time `events_sample`'s
  profiler check counts on), `null_syscall`, `null_syscall_200_threads`
  (the same with 200 other threads alive: a system call's cost must not
  grow with the threads, as a browser has hundreds; compare the two in
  one run, absolute numbers move ±30% between runs under TCG),
  `handle_create_close`,
  `virtual_alloc_free_64k`, `commit_decommit_page` and
  `commit_decommit_page_4096_runs` (one page of a fresh reservation and
  of one split into 4096 runs: the cost must not grow with the runs;
  about 23 and 76 us under TCG, the latter 2.6 ms when each change
  rebuilt the run list), `demand_zero_fault`, `section_map_unmap_64k`,
  `image_load`, `image_load_32m_untouched` (both after one untimed load
  that lays the file out; the second must cost what the first does),
  `thread_create_exit`, `event_pingpong_same_cpu`,
  `event_pingpong_cross_cpu` (with two CPUs). `cargo xtask bench
  --release [bench-windows.txt]` boots it (`run=bench.exe`, BIOS, no
  screen), writes `target/report/bench.json` (platform `gala qemu-tcg`
  or `qemu-kvm`) and prints a table next to Windows' numbers
  (`run-all.ps1` saves `bench-windows.txt`, unless antivirus quarantined
  `bench.exe`: WORKFLOW, "To compare with Windows"). The kernel is a
  release build only with `--release`.
- **Without kernel changes**: QEMU TCG plugins (`-plugin`) count
  instructions per address range (needs a QEMU built with plugins;
  Ubuntu's has none); under KVM, `perf kvm` on the host.
- **The sampling profiler**: `events=sample,image,thread,process` (or
  `XTASK_EVENTS=...`) and `cargo xtask profile` (docs/OBSERVABILITY.md
  "Event stream"): where the CPU time goes, per process, thread, module
  and function. A Chromium run found 36% of all samples in
  `nt::thread::current`, a scan of every thread on each system call.

Rules of thumb:

- Never benchmark a debug kernel: it is about 30x slower per system
  call (boot tests use debug builds on purpose).
- Under TCG, code in shared physical frames is translated once (QEMU's
  translation cache is per physical page): sharing pages between
  processes helps the emulator as much as it saves copying.
- Pipe reads and writes that take hundreds of microseconds are usually
  blocking (a parent waiting on its children), not work.

## P1: first benchmark numbers

The first `cargo xtask bench` run (date not recorded; before H8, so
`image_load` is not in it): QEMU TCG on a 4-CPU container, release
kernel; only relative.

| Benchmark | Time per op |
|---|---|
| null system call | 2.2 us |
| event create + close | 6 us |
| 64 KiB alloc + free | 17 us |
| demand-zero fault | 7 us (6.8-11 us across runs) |
| section map + unmap (64 KiB) | 13 us |
| event round trip, one CPU | 20 us |
| event round trip, across CPUs | 124 us |
| thread create + exit | 369 us |

Thread creation and cross-CPU wakes are the outliers to look at first.

## T1: boot-test speed

Before and after item T1 (BIOS boots, shared VMs, parallel ISO
creation; WORKFLOW "Test speed and environments"): software emulation
on a 4-CPU container, 40 scenarios (date not recorded).

| | before | after |
|---|---|---|
| VMs | 40 | 13 |
| firmware + bootloader (summed over VMs) | 193 s (OVMF) | 24 s (SeaBIOS) |
| kernel + tests (summed) | 38 s | 18 s |
| ISO creation | 5.7 s (sequential) | 0.5 s (parallel) |
| boot-test wall time | 66 s | 14 s |

Single-threaded TCG costs about 1.8x: java_hello ran 84 s instead of
47 s in the kernel with `-accel tcg,thread=single` (about 35 s
locally).

## H8: building Lua (2026-10-31)

The `lua` scenario: busybox sh unpacks TinyCC and Lua, tcc builds its
runtime (15 runs), make builds Lua (37 tcc runs), lua.exe runs a script;
119 processes in all. Release kernel, no events.

**Before: 12.7 s** in the kernel.

    nt: 50145 system calls in 3095 ms (and 4614 waits, 26580 ms),
        34034 user faults in 753 ms, 119 processes created in 641 ms

The NT layer's own work was about 3.9 s; the rest was user code (tcc
compiling, under TCG) and idle time. The largest kernel items, from the
event stream (system calls only):

| Service | Calls | Median | Total |
|---|---|---|---|
| GalaCreateProcess | 118 | 5.8 ms | 0.90 s |
| NtCreateFile | 6321 | 46 us | 0.45 s (mostly tcc's include path probes) |
| NtMapViewOfSection | 1496 | 28 us (p90 392 us) | 0.34 s |
| NtCreateSection | 617 | 168 us | 0.21 s |

Faults: 561 per tcc run, about 5 ms a run. Loading images was the
largest avoidable cost: each process re-read and re-laid-out every DLL
(NtCreateSection SEC_IMAGE read the whole file), copied the layout again
per view, and copied the whole executable into fresh frames
(GalaCreateProcess).

**Changes**:

- Image layouts are cached per file and content version
  (`section::image_layout`, 16 entries); a cached layout keeps its
  shared read-only frames, so later loads map them directly.
- A view at the preferred base maps the shared frames without copying
  and "relocating" the layout.
- Executables are mapped from the same cached layout: every page borrows
  the shared frame (copy-on-write for writable sections), as image views
  do.

**After: 7.9 s** in the kernel (-38 %).

    nt: 50143 system calls in 2095 ms (and 2846 waits, 15364 ms),
        34034 user faults in 614 ms, 119 processes created in 266 ms

| | Before | After |
|---|---|---|
| bench `image_load` (open, SEC_IMAGE section, map, unmap, close of an 87 KiB DLL) | 2.09 ms | 0.32 ms |

Later: image views are demand-paged (a page is mapped at its first
touch, not with the view). Warmed up (the layout made once, untimed),
release kernel, TCG:

| | Eager | Demand-paged |
|---|---|---|
| bench `image_load` (87 KiB DLL) | 0.50 ms | 0.30 ms |
| bench `image_load_32m_untouched` (the same, 32 MiB larger, not touched) | 22.4 ms | 0.29 ms |
| process creation (119) | 641 ms | 266 ms |
| lua scenario, kernel time | 12.7 s | 7.9 s |
| tcc / tcc_make / tcc_self (full suite run) | 6.3 / 9.0 / 8.5 s | 4.3 / 6.7 / 6.8 s |
| whole suite, summed kernel time | 64.3 s | 53.0 s |

The scenario gained more than the kernel counters lost (4.8 s against
about 1.3 s): with each tcc run's code in the same physical frames,
QEMU reuses the translated code instead of translating tcc again in
every process.

Next candidates: NtCreateFile path resolution (0.45 s, include probes),
and the user time itself (tcc under TCG), which only a faster emulator
or KVM changes.

## H9: SQLite (2026-10-31)

The `sqlite` scenario (tcc runtime, unzip, one tcc run compiling the
9.5 MB sqlite3.c with the driver, three SQLite runs): 6.9 s in the
kernel. No breakdown taken.

    nt: 30254 system calls in 1970 ms (and 2928 waits, 10126 ms),
        24443 user faults in 746 ms, 46 processes created in 95 ms

# Where the time goes

Measurements of gala under QEMU (TCG: every instruction is translated,
so only ratios between gala runs mean something; Windows numbers come
from `run-all.ps1`). Tools:

- **The `nt:` line** at the end of every run (`nt::stats`, always on):
  system calls (count, time; waits apart, as their time is sleeping),
  user faults the kernel resolves, processes created. Two clock reads
  per event, so it does not change what it measures.
- **The event stream** (`XTASK_EVENTS=syscall,...`, `events.rs`): one
  JSON line per event with arguments and per-call time. Each line is a
  debug-port write, slow under emulation: with `all` the lua scenario
  takes 3x as long (faults dominate), so use it for shares and
  arguments, not totals. Grouping script idea: count and sum `ns` by
  `name`, split by argument (sizes) where it matters.
- **`cargo xtask bench`**: micro-benchmarks also run on Windows.

## H8: building Lua (2026-10-31)

The `lua` scenario: busybox sh unpacks TinyCC and Lua, tcc builds its
runtime (15 runs), make builds Lua (37 tcc runs), lua.exe runs a script;
119 processes in all.

Before (release kernel, no events): **12.7 s** in the kernel.

    nt: 50145 system calls in 3095 ms (and 4614 waits, 26580 ms),
        34034 user faults in 753 ms, 119 processes created in 641 ms

So the NT layer's own work was about 3.9 s; the rest was user code
(tcc compiling, under TCG) and idle time. From the event stream (system
calls only), the largest kernel items:

| Service | Calls | Median | Total |
|---|---|---|---|
| GalaCreateProcess | 118 | 5.8 ms | 0.90 s |
| NtCreateFile | 6321 | 46 us | 0.45 s (mostly tcc's include path probes) |
| NtMapViewOfSection | 1496 | 28 us (p90 392 us) | 0.34 s |
| NtCreateSection | 617 | 168 us | 0.21 s |

Small pipe reads and writes average hundreds of microseconds, but that
is blocking (busybox waiting on its children), not work. Faults: 561 per
tcc run, 6.8-11 us each (bench `demand_zero_fault`): about 5 ms a run.

Loading images was the largest avoidable cost: each process re-read and
re-laid-out every DLL (NtCreateSection SEC_IMAGE read the whole file),
copied the layout again per view, and copied the whole executable into
fresh frames (GalaCreateProcess).

Changes:
- Image layouts are cached per file and content version
  (`section::image_layout`, 16 entries); a cached layout keeps its
  shared read-only frames, so later loads map them directly.
- A view at the preferred base no longer copies and "relocates" the
  layout (it maps the shared frames anyway).
- Executables are mapped from the same cached layout: every page
  borrows the shared frame (copy-on-write for writable sections), as
  image views do.

After: **7.9 s** in the kernel for the lua scenario (-38 %).

    nt: 50143 system calls in 2095 ms (and 2846 waits, 15364 ms),
        34034 user faults in 614 ms, 119 processes created in 266 ms

| | Before | After |
|---|---|---|
| bench `image_load` (open, SEC_IMAGE section, map, unmap, close of an 87 KiB DLL) | 2.09 ms | 0.32 ms |
| process creation (119) | 641 ms | 266 ms |
| lua scenario, kernel time | 12.7 s | 7.9 s |
| tcc / tcc_make / tcc_self (full suite run) | 6.3 / 9.0 / 8.5 s | 4.3 / 6.7 / 6.8 s |
| whole suite, summed kernel time | 64.3 s | 53.0 s |

The scenario gained more than the kernel counters lost (4.8 s against
about 1.3 s): with each tcc run's code in the same physical frames,
QEMU's translation cache (kept per physical page) reuses the translated
code instead of translating tcc again in every process. Sharing pages
helps the emulator as much as it saves copying.

Next candidates: NtCreateFile path resolution (0.45 s, include
probes), and the user time itself (tcc under TCG), which only a faster
emulator or KVM changes.

## H9: SQLite (2026-10-31)

The `sqlite` scenario (tcc runtime, unzip, one tcc run compiling the
9.5 MB sqlite3.c with the driver, three SQLite runs): 6.9 s in the
kernel.

    nt: 30254 system calls in 1970 ms (and 2928 waits, 10126 ms),
        24443 user faults in 746 ms, 46 processes created in 95 ms

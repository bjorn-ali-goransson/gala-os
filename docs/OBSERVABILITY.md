# Observability: tracing, telemetry, profiling (plan)

gala-os is developed mostly by agents, which can only act on what they can
read. The goal is that every boot produces machine-readable answers to:
what did the program ask for, what did we not implement, where did the
time go, and what was the state when it broke. Status per item: **done**,
**next**, **later**.

## 1. Structured event stream — done (`events`, scenario `events`)

- A second output channel next to the human serial log: QEMU's
  `isa-debugcon` (port 0xe9, one `out` per byte, no UART handshake) written
  to a file by xtask, carrying JSON lines:
  `{"t":ns,"cpu":0,"pid":4,"tid":8,"ev":"syscall","name":"NtCreateFile",
  "args":[...],"status":"0xc0000034","dur_ns":1200}`.
- Events: system calls (the `trace` flag already formats them; done as
  text), exceptions delivered to user mode, page faults resolved/raised,
  object create/close, thread create/exit/switch, process start/exit, image
  maps (base, size, name — needed to symbolize user addresses), panics.
- Cheap when off: a per-category mask from the command line
  (`events=syscall,image`), checked with one relaxed atomic load.

Implemented: `kernel/src/events.rs` writes to QEMU's debug console
(port 0xe9, `arch::debugcon`); xtask always attaches it to
`<iso>.events.jsonl` next to the serial log. Categories: `syscall`
(pid, tid, nr, name, args, status, ns), `image` (pid, NT path, base,
size: the exe and every image view), `thread` (create with start and TEB,
exit with status), `process` (start, exit), `fault` (every user-mode CPU
exception, before it is resolved or delivered). `XTASK_EVENTS=all cargo
xtask boot-test` turns it on for every scenario; `EVENT_EXPECT` in xtask
checks a scenario's stream. Next: process-relative times, a `cargo xtask
events` summary (top services by count and time), user-address
symbolization from the image events.

## 2. "What is missing" ledger — done (`nt::ledger`, `ledger_test`)

Every STATUS_NOT_IMPLEMENTED, STATUS_INVALID_SYSTEM_SERVICE, unknown
information class, unsupported flag or TODO path increments a counter keyed
by (service, detail). At process exit the kernel prints the ledger as
JSON; `cargo xtask report` merges it over all scenarios and real programs.
When running a real program (Go, Nginx), this list *is* the work queue,
ranked by how often it was hit.

Implemented: a fixed 64-entry table keyed by (service, what, detail);
the dispatcher records unknown services and, generically, any
STATUS_INVALID_INFO_CLASS (with the class argument, per
`ledger::class_argument`) and STATUS_NOT_IMPLEMENTED. Other code calls
`nt::ledger::record(service, what, detail)` for unsupported flags and TODO
paths. Lines are `ledger: {"service":..,"what":..,"detail":..,"hits":..}`,
dumped after each program and before `boot ok`. `cargo xtask report`
writes `target/report/ledger.md`, ranked by programs and hits.

## 3. Trace diffing against Windows — later

`cargo xtask diff-trace gala.jsonl windows.jsonl` aligns two syscall
traces of the same program and shows the first divergence (service,
arguments, status). The Windows side comes from a user-mode tracer (an
NtTrace-like tool, or ETW syscall events) run by `user/bin/run-all.ps1`.
The Windows results we collect are black-box behaviour, so this stays
clean-room.

## 4. Profiling — benchmarks done (P1); sampling profiler next

**Benchmarks**: `user/bench` (one program, unchanged on Windows) prints
`bench: <name> ns_per_op <n>`; `cargo xtask bench [bench-windows.txt]`
boots it on a release kernel, writes `target/report/bench.json` and
prints a table next to Windows' numbers (run-all.ps1 saves
`bench-windows.txt`). First numbers (QEMU TCG on a 4-CPU container, so
only relative): null syscall 2.2 us, event create+close 6 us, 64 KiB
alloc+free 17 us, demand-zero fault 7 us, section map+unmap 13 us, event
round trip 20 us on one CPU and 124 us across CPUs, thread create+exit
369 us. Lessons: a debug kernel is ~30x slower per system call (boot tests
use debug builds on purpose; never benchmark them); thread creation and
cross-CPU wakes are the outliers to look at first. QEMU instruction
counts (TCG plugins) need a QEMU built with plugins; Ubuntu's has none.


- **Sampling**: the LAPIC timer interrupt already runs on every CPU. At
  each tick, record the interrupted RIP (and, with frame pointers, a short
  stack) into a per-CPU ring buffer; dump it at exit. xtask symbolizes
  kernel addresses with the kernel ELF (`addr2line`/the `object` crate)
  and user addresses with the image-map events plus PE exports, and writes
  folded stacks for a flame graph. Build kernels with
  `-C force-frame-pointers=yes` for this.
- **Counters**: per-service call counts and TSC time, spin iterations on
  each `IrqMutex` (lock contention), heap and frame statistics, context
  switches and IPIs per CPU. Printed at exit and queryable at run time
  (below).
- **Without kernel changes**: QEMU TCG plugins (`-plugin`) can count
  executed instructions/basic blocks per address range, which gives
  coverage and an instruction-count profile of both kernel and user code,
  deterministic and independent of timing. Under KVM, `perf kvm` on the
  host samples the guest.

## 5. Post-mortem — backtraces done; gdb and record/replay next

- Panics print a backtrace (`backtrace: 0x... 0x...`, return addresses
  from a frame-pointer walk, `arch::backtrace`; the kernel is built with
  `-C force-frame-pointers=yes`). xtask symbolizes it with
  `llvm-addr2line`/`addr2line` whenever it shows a failed boot's log, and
  `cargo xtask symbolize <log>` does it for any log. Still to do: the
  current thread/process and the last N events in the panic output.
- `cargo xtask debug name` boots a scenario with QEMU's gdbstub (`-s -S`)
  and a gdb script, so an agent can get backtraces and inspect memory in
  batch mode.
- Fault injection (ROADMAP R2, `failpoint=`): `failtrace` prints a
  backtrace at each injected failure, so the code that mishandled it is
  named even when the symptom (a hang, a wrong status) comes much later
  (WORKFLOW "Fault injection").
- Flaky SMP bugs: QEMU record/replay (`-icount shift=auto,rr=record`)
  makes a failing run reproducible.
- **Flight recorder** (done 2026-11-02, `kernel/src/flight.rs`): an
  always-on ring per CPU of the last 256 records (time << 8 | kind, two
  values; one atomic increment and three stores each), kept whether the
  event stream is on or not: system calls (number | pid << 32, thread
  id) and their status, process exits, the watchdog's marks. A panic
  prints the last 48 across CPUs, merged by time (`flight: ...` lines
  after the backtrace). TODO: context switches, page faults, interrupts.
- **Hang watchdog** (done 2026-11-02 for stuck CPUs): every timer
  interrupt counts its CPU's heartbeat; once a second each CPU looks at
  the next online one's, and one that has not moved for 5 s (interrupts
  off: a lock never released, a loop in a handler) is reported once,
  with its last flight-recorder events (scenario `watchdog`: `hangtest`
  stalls CPU 1). And a thread that stays in the kernel: each thread's
  last progress (it blocked in `schedule`, or a tick found it in user
  mode) is kept; a tick that finds a thread in kernel code with no
  progress for 20 s (preemption is not progress) reports it with its
  name, rip and the CPU's last events, again every 20 s while it stays
  (scenario `kernel_watchdog`: `spintest` makes the boot thread spin
  25 s). Progress is: returning from a system call, a tick in user mode,
  a voluntary yield (a polling wait); blocking marks the thread so that
  its count starts afresh when it runs again (a long wait is not time in
  the kernel); preemption is not progress. First finding: java.exe's
  NtCreateSection on the JDK's `modules` file stays in the kernel over
  20 s (a boot-module file is copied page by page into the ram file
  system under its lock when first mapped): fixed, such a file now turns
  into pages lazily, a page when it is written or mapped (java_hello 38 s
  -> 25 s). TODO: the stuck CPU's own
  stack (an NMI to it).
- **Crash record across reboots** (done 2026-11-02, `kernel/src/crash.rs`):
  a panic writes the panic message, the backtrace and the flight
  recorder's last 48 events to the crash area (the last 16 sectors of a
  version 2 volume: `GCRASH\0\x01`, length, CRC-32, the text), with
  `disk::write_in_panic` (never waits: the device busy means no record);
  the next boot that mounts the volume prints it (`crash: ...` lines)
  and clears it. It stays on the machine (privacy by design). Scenario
  `crash_record` (two boots: `crashtest` panics on purpose, then the
  record is printed). TODO: the screen (a blue screen with the record),
  symbols in the record (addresses only now).
- **Deterministic simulation** (done 2026-11-02, R5): `um --sim=<seed>`
  runs one NT thread at a time, chosen by a seeded PRNG at every system
  call, wait, yield and lock release, with a virtual clock; the run
  ends with `sim: seed S trace H` (the same for the same seed) and a
  deadlock is reported with the seed and every thread's wait. `cargo
  xtask sim-test` runs the multi-threaded programs under several seeds,
  twice each (ARCHITECTURE "Deterministic simulation", WORKFLOW).
  TODO: RDRAND (Go's runtime) from the seed.

## 6. Live introspection — later

A command channel (second serial port or virtio-vsock) with read-only
queries: threads and their states/CPUs, a process's handle table and VADs,
object namespace, counters. Agents can ask a hung system what it is
waiting on instead of guessing. Later read-write for tests (ROADMAP R7,
live mocking): inject a fault, jump the clock, swap a device for a
wrapper; off unless booted with an explicit flag, every change logged
to the event stream.

## 6b. Screen — done

The kernel's screen console mirrors the serial log on the framebuffer, and
`cargo xtask screenshot` captures it as PNG at chosen times (docs/WORKFLOW.md).
Later: a status area (CPU load, threads, memory) drawn by the kernel.

## 7. Fuzzing — later

A user-mode fuzzer program calling services with random/mutated arguments
in CI (the kernel must never panic on user input), and `cargo fuzz` on the
host for pure parsers (PE, paths, PE resources, registry hives later).

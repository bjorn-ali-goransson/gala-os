# Observability: tracing, telemetry, profiling (plan)

gala-os is developed mostly by agents, which can only act on what they can
read. The goal is that every boot produces machine-readable answers to:
what did the program ask for, what did we not implement, where did the
time go, and what was the state when it broke. Status per item: **done**,
**next**, **later**.

**Rule (the owner, 2026-11-03): observability carries great weight.**
Every new subsystem, driver, boot stage, format or test ships with the
means to explain its own failures: what it logs, which events it emits,
what a failure leaves behind (panic line, flight records, crash record,
the failed log's extra sections), and how an agent reads it. A failure
whose cause has to be guessed is a gap to fill before (or together with)
the fix. Reviews ask: "if this broke silently at 3 a.m., what would we
have?"

## Gaps found 2026-11-03 (audit, ranked by the time they cost)

An audit of what failures left behind, against the failures that cost the
most agent time (ROADMAP "Priorities", WORKFLOW "Known intermittent").
Each: what failed, what we lacked, what would have explained it, status.

1. **Timeouts that said nothing.** java_http, java_hello, postgres and
   smpsched_test failed under load with only "timed out after 300s"; the
   one-minute hang dump came long before the end (and only for `run=`
   programs), and nothing was taken at the moment of the kill. Lacked:
   the state when xtask gave up. Would have explained: which threads
   wait on what, which CPU spins where, which locks are held. **Done**:
   xtask injects an NMI (QEMU monitor `nmi`, every CPU) before killing a
   timed-out VM and waits up to 20 s for the kernel's state dump
   (`kernel/src/statedump.rs`): each CPU's rip and kernel backtrace
   (symbolized in the failed log), the thread each CPU runs, whether the
   scheduler, heap, frame and console locks are free (a lock held for the
   whole dump is the deadlock), heap and frames, the flight recorder;
   then, from the boot thread, every NT thread's system call and wait
   object, every scheduler thread, the ledger and `statedump: end`.
   xtask's line says how far the dump got ("did not answer the NMI": the
   machine is wedged or the handler is not installed yet). gala-um: SIGQUIT
   (`timeout -s QUIT`) prints the same threads part. Scenario
   `statedump_test` (a program that hangs on purpose; also in um-test).
   `XTASK_TIMEOUT=<s>` shortens any scenario's timeout to get the dump
   of a hang quickly. Found on the way: the LAPIC's LINT1 (the NMI line)
   was masked, and an NMI arriving in user mode would have been handed to
   the program as an exception.
2. **A failed log had to be read whole.** Agents scrolled thousands of
   lines (JVM runs) for the verdict. Lacked: a summary at the tail.
   **Done**: every failed log (`target/failed/<scenario>.log`, and the
   console) ends with a triage block (`xtask/src/triage.rs`): scenario,
   verdict, duration, host CPUs/load/free disk, guest CPUs, heap (peak)
   and frames (fewest free), warning/error counts and the first error,
   panic/exception/watchdog/leak/out-of-memory/missing-function lines,
   programs begun and not ended, processes that failed, the ledger's top
   five, the state dump's CPUs, locks and threads, each thread's last
   system calls (event stream, else flight recorder), user faults, the
   last lines. `cargo xtask triage <log> [events.jsonl]` prints it for any
   log. The event stream is copied next to the failed log
   (`<scenario>.events.jsonl`) when it was on.
3. **No summary of an event stream** (§1's "next"). **Done**: `cargo xtask
   events <events.jsonl>`: events by kind, system calls by total time,
   failing statuses by service, each thread's last calls, faults,
   processes, images.
4. **Flakiness was found by people rerunning.** Which scenarios fail 1 in
   20, and since when, was in agents' memories. Lacked: a record across
   runs. **Done**: every `cargo xtask test`/`boot-test` appends one line
   per scenario to `target/report/history.jsonl` (time, commit, host
   load, pass/fail, VM seconds, reason) and ends with `xtask: health:`
   lines: scenarios that both passed and failed in their last 30 runs
   (with the last failure's reason) and scenarios whose latest pass (alone
   in a VM) took over 1.5 times, and 10 s more than, the median of their
   last ten. Still open: XTASK_REPEAT boots and um-test into the history.
5. **The disk filled up mid-suite** ("No space left on device" from a copy
   or xorriso, read as a build failure). **Done**: an image is not started
   with less than 300 MiB free, nor packed into an ISO without its tree's
   size free; the error names the free space, the need, and what to
   delete (WORKFLOW "Disk space"). Out-of-space errors on the way get the
   same hint.
6. **Programs died with only an exit status.** "exited with status
   0xc0000005"; for the JVM we had only its own hs_err file, for others
   nothing (a dispatched exception that no handler took said nothing at
   all). **Done**: crash reports (`nt::crashreport`): every user exception
   keeps a fixed-size note in its process; when the process ends with
   that code, `crash report:` lines give the code and its name, the thread, the
   address as module+offset (the executable or an image view), what was
   read/written/executed, the registers and the top eight stack words
   (each named by module when it points into an image). Scenarios
   `crash` and `crash_dispatched`. Still open: a real stack walk (unwind
   data or frame pointers) and export names (nearest exported symbol).
7. **Memory high-water marks were missing.** Out of frames looks like a
   bad image (WORKFLOW), and the `stats:` line showed only the end state.
   **Done**: the heap's peak and the fewest free frames since boot, on the
   `stats:` line, in the state dump and after a panic.
8. **The flight recorder showed numbers.** `syscall 0x400000004 0x8` had
   to be decoded by hand. **Done**: service names, pid and tid on each
   system-call record; the triage lists them per thread.
9. **A panic did not say which thread** (§5's TODO). **Done**: after the
   flight recorder, the panic prints the memory line and each CPU's thread
   (when the scheduler's lock is free).
10. **The failpoint panic's backtrace was unreadable** (addresses of a
    build since rebuilt). Done before (symbolized into the failed log when
    the boot fails); the state dump's per-CPU backtraces now go the same
    way, and the symbolized blocks say whose backtrace they are.
11. **"address space still referenced"**: the holders are printed (done
    before); the triage names the leak. Still open: who took each
    reference (a per-reference tag or backtrace in debug builds).
12. **gala_cli's split output lines**: a program's output and the
    kernel's log share the serial port. The full suite of 2026-11-03
    showed the cause of one: the watchdog's report (written without the
    port's lock, as interrupt-time output must be) landed inside the
    program's line "gala_cli_test target: WM_CLOSE". **Done**: emergency
    output takes the port's lock when it comes free within 10 ms (untracked
    by lockdep), so whole lines no longer mix; without the lock (the
    interrupted code holds it) it still writes. Still open: program output
    on its own channel (an `out` event per write with the pid, or a second
    serial port) so that a test can match a program's lines alone. The
    watchdog fired there because the boot under a host load of 10 measured
    the TSC at 15 MHz (`apic:` line): a calibration that went wrong under
    load, for the early-boot work (and the triage shows the `apic:` line's
    figures in the guest line's neighbourhood: look there first when
    watchdogs fire at boot).
13. **Unexplained net client failures** ("Connection refused" from the
    host port forward once in a full suite). Still open: the client's
    attempts, the port and QEMU's forward in the failed log.
14. **QEMU exiting at once on a loaded host with an empty log** (java_hello
    12 of 30 never started), early boot visibility: the other agent's work
    (QEMU exit reason, debugcon boot phases); not in this audit.
15. **A stuck CPU's own stack** (§5 watchdog TODO). **Done**: the watchdog
    sends an NMI to the CPU it found stuck, which prints `watchdog: CPU N
    kernel rip ...` and `watchdog: CPU N backtrace: ...` (symbolized in the
    failed log); scenario `watchdog` expects them.
16. **Diagnostics that took locks confused lockdep**: a `try_lock` from an
    interrupt or NMI records an order between the interrupted code's locks
    and the one tried, which no real path has. **Done**:
    `IrqMutex::try_lock_untracked` for the dump, the panic's figures and
    emergency output.
17. **A clock calibrated wrong under load went unnoticed.** Two full
    suites (host load 10-15) had boots that calibrated the TSC at 15, 31
    and 135 MHz (`apic:` line; right is ~2.8 GHz): guest time then runs
    20-200x fast, so sleeps end early (handover_idle_uefi's check 0x16),
    the watchdog reports CPUs "stuck" for "5 s" every quarter second
    (postgres: dozens of reports, which slow the boot further; gala_cli's
    split line). **Done**: the triage flags a TSC under 500 MHz as a
    suspicious clock. **Done** (2026-11-03, root cause and fix in
    ARCHITECTURE "Interrupts and time"): the vCPU was descheduled between
    starting the PIT one-shot and reading the TSC, so the TSC and the
    LAPIC timer were both measured over a fraction of the window (the
    135 MHz boot had its timer at 3043 kHz of 62748: the same 4.8%).
    Calibration now uses bracketed samples against the HPET / PM timer
    with a median and logs how (`tsc: ... from HPET ..., 5 samples (n
    paused, n outliers), spread x%`, `phase tsc_mhz`, a `clock_cal`
    event); a kernel thread checks the TSC against the CMOS RTC after
    boot and recalibrates when they disagree (`clock: RTC check: agrees`
    or `clock: WRONG`, `clock_wrong` event); xtask times every
    `uptime_ms=` mark against the host and adds `xtask: clock: <source>:
    guest N ms over host M ms` or `xtask: clock: WRONG: ...`; the triage
    reports both (`triage: clock WRONG`), and a failure in a boot whose
    clock was wrong says so in its verdict. Scenarios `clockcal_test`
    (host CPUs kept busy, `calstall`), `clockcal_recal` (`calwrong`).
18. **First fruits of the state dump** (full suite, 2026-11-03): java_http
    timed out with a correct clock, and its dump shows the JVM's
    threads 16, 20 and 40 in NtYieldExecution (spinning), 32 and 36
    waiting on semaphores with count 0, the others in delays or waits on
    events: the signature of a lost semaphore release at HotSpot's wait
    barrier (ROADMAP "Priorities"), which the NtReleaseSemaphore fix was
    meant to end. The failed log (`target/failed/java_http.log`) is the
    starting point for the next look.

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
checks a scenario's stream. `cargo xtask events <file>` summarizes a
stream (done 2026-11-03: system calls by time, failing statuses, each
thread's last calls, faults, processes, images). Next: process-relative
times, user-address symbolization from the image events.
Since 2026-11-03 also `net` (TCP accept, connect, close and reset with
local and remote addresses), `logon` (LogonUser/LSA logons: pid, user,
domain, type, how, status; never the password) and `console` (console
sessions and pseudo consoles created, resized, closed with how many
programs it ended); `process start` carries the command line. Since RS
B1 (sessions) `process start` and `process exit` carry `"session"`, a
session made after boot is an event (`"ev":"session"`, id and its
BaseNamedObjects; category `process`) and a log line, the start line
says the session (`process x.exe (pid 4, session 1): starting at`), and
the state dump's thread lines end with `session N (name)`. Since RS
B2 `window` (`"op":"create"` with hwnd, parent, pid and session;
`"describe"` with tid, class and text once user32 knows them;
`"destroy"`; `"other session"` when a process asks
about, sends or posts to another session's HWND, also logged the first
32 times as `window: pid P (session S) ... not visible across
sessions`), and the state dump has a sessions part (`statedump: session
N: ... windows ...`, then its top-level windows). Also `display`
(2026-11-03: virtio-gpu modes set, device resets, failed commands with
their names); `gputrace` logs every virtio-gpu command and its result,
and a boot ends with a `display:` line (commands, flushes, mode changes,
errors, resets, the last error). Server
logs: `logtee=C:/dir` copies what programs write below a directory to the
serial log (`logtee: <file>: <line>`; sshd's and sshd-session's logs in
the sshd scenarios), and `conpty=trace` logs a pseudo console's VT
traffic in both directions.

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
  `cargo xtask symbolize <log>` does it for any log. After the
  backtrace: the flight recorder's last events, the heap and frame figures
  and each CPU's thread (done 2026-11-03).
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
  -> 25 s). The stuck CPU's own rip and backtrace follow, by an NMI
  to it (done 2026-11-03).
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
  Go programs repeat too (RDRAND hidden by CPUID faulting, ProcessPrng
  from the seed, spin loops preempted at a canonical point).

## 5b. Early boot — done (2026-11-03)

A boot that dies before or without a panic used to leave a serial log
that just stops and a QEMU exit status of 0 (an AP that triple-faulted
reset the machine: ARCHITECTURE "Processors"). Every boot test now
records, always on and cheap:
- **QEMU's side** (xtask, `xtask/src/telemetry.rs`), next to the image
  (`target/<profile>/<image>.*`): `.qemu.log` (QEMU `-d
  cpu_reset,guest_errors -D`: each CPU reset with its registers, "Triple
  fault", bad guest accesses), `.bios.log` (SeaBIOS's debug port 0x402:
  the CPUs it counted, the boot device), `.phases.log` (the kernel's
  phase markers, below). After the run a few `xtask:` lines end the serial
  log (and so `target/failed/<scenario>.log`): how QEMU ended (status 33 /
  35 = the kernel's exit codes, 0 = a reset or power-off without them,
  which with `-no-reboot` is what a triple fault looks like, a signal, a
  timeout), the vCPUs QEMU created (QMP `query-cpus-fast`, on a socket in
  the temp directory), the host's load average and available memory at the
  start and the end. After an abnormal end (not 33/35, not stopped by
  xtask) also the last 12 phase markers and the last 30 lines of QEMU's
  log.
- **CPU count**: xtask compares the kernel's `smp: N CPUs online` with
  the vCPUs it asked for (2, or 1 for `onecpu_*`); a boot short of CPUs
  is booted once more (not with a disk), and a failure on one says first
  `VM got 1 of 2 CPUs (host overloaded?)`.
- **Boot phase markers** (`arch::bootphase`): a line per step on their own
  debug-console port 0x403 (the event stream has 0xe9), lock-free and
  allocation-free: `phase <name> cpu <APIC id> tsc <TSC>` for entry,
  boot_info, xsave, gdt_idt, memory_map, frames, paging, apic, sched,
  smp_start, `ap_online <index>` (from each AP), smp_done, drivers,
  manifest, program (each `run=` program), boot_ok. The last line in the
  file is how far the boot got, even when the machine reset before the
  serial log said anything.
- **Faults**: every CPU exception in kernel mode first writes `fault
  <vector> rip <addr> error <code> cpu <id> tsc <n>` to the phase port,
  then panics as before (message, backtrace, flight records, crash
  record); a double fault runs on its own IST stack (gdt::IST_DOUBLE_FAULT)
  and goes the same way. A panic writes `panic cpu <id>`. Before the IDT
  is loaded (the first few hundred instructions) a fault still
  triple-faults: the last phase is then `entry` or `boot_info`.
- **SMP detail** in the boot log: Limine's MP response (count, BSP, each
  APIC id), the MADT's enabled CPUs and which of them the bootloader did
  not start, how long start-up took, CPUs the kernel started itself.

## 6. Live introspection — first slice done (state dump on NMI)

Done 2026-11-03: a read-only "what are you doing" query with no channel
of its own: an NMI (QEMU monitor `nmi`; xtask sends it when a boot test
times out) makes the kernel print a state dump (`kernel/src/statedump.rs`,
"Gaps found 2026-11-03" item 1); SIGQUIT does the same in gala-um.
Later: a command channel (second serial port or virtio-vsock) with read-only
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

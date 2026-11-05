# Observability: logs, events, telemetry, post-mortem

gala-os is developed mostly by agents, which can only act on what they can
read. Every boot should give machine-readable answers to: what did the
program ask for, what did we not implement, where did the time go, and
what was the state when it broke. This document is the reference for the
facilities that answer those questions, the gaps still open and the
facilities planned.

**Rule: observability carries great weight.** Every new subsystem,
driver, boot stage, format or test ships with the means to explain its
own failures: what it logs, which events it emits, what a failure leaves
behind (panic line, flight records, crash record, the failed log's extra
sections), and how an agent reads it. A failure whose cause has to be
guessed is a gap to fill before (or together with) the fix. Reviews ask:
"if this broke silently at 3 a.m., what would we have?"

**Target model: OpenTelemetry** (ROADMAP OT). New telemetry is designed
to map onto OTel's traces, spans, logs and metrics with W3C trace
context, so that a span opened in a program can be followed through
ntdll into the kernel and back. The kernel keeps compact per-CPU binary
records; an exporter produces OTLP. New events carry the fields that
mapping needs (thread, process, a span or parent when there is one)
rather than free text.

## What exists

| Facility | Where | How to read it |
|---|---|---|
| Serial log | `log` macros, `kernel/src/console.rs` | `target/<profile>/test-<scenario>.serial.log`; `target/failed/<scenario>.log` |
| Event stream (JSON lines) | `kernel/src/events.rs` | `events=` / `XTASK_EVENTS`; `<iso>.events.jsonl`; `cargo xtask events` |
| OpenTelemetry export | `xtask/src/otel.rs` | `cargo xtask otel`, `XTASK_OTEL=1` |
| Missing-features ledger | `kernel/src/nt/ledger.rs` | `ledger:` lines; `cargo xtask report` |
| Triage of a failed log | `xtask/src/triage.rs` | end of every failed log; `cargo xtask triage` |
| Run history, health | `xtask/src/main.rs` (`record_history`) | `target/report/history.jsonl`, `xtask: health:` lines |
| Backtraces, panic output | `arch::backtrace`, `cargo xtask symbolize` | `backtrace:` lines, symbolized in failed logs |
| Flight recorder | `kernel/src/flight.rs` | `flight:` lines after a panic, watchdog report, state dump |
| Hang watchdog | `kernel/src/flight.rs` | `watchdog:` lines |
| State dump on NMI / SIGQUIT | `kernel/src/statedump.rs` | `statedump:` lines on a timeout |
| Crash reports (programs) | `kernel/src/nt/crashreport.rs` | `crash report:` lines |
| Crash record across reboots | `kernel/src/crash.rs` | `crash:` lines on the next boot |
| Clock checks | `kernel/src/time.rs`, `kernel/src/clockcal.rs`, `xtask/src/clockcheck.rs` | `tsc:`, `clock:`, `xtask: clock:` lines |
| Early boot (QEMU side, phases) | `xtask/src/telemetry.rs`, `arch::bootphase` | `.qemu.log`, `.bios.log`, `.phases.log`, `xtask:` lines |
| Screen | kernel screen console | `cargo xtask screenshot` |
| Benchmarks | `user/bench` | `cargo xtask bench` (docs/PERF.md) |

Fault injection, deterministic simulation and record/replay are test
tools that also explain failures: ARCHITECTURE "Fault injection",
"Deterministic simulation", "Record/replay"; WORKFLOW "Fault injection",
"Deterministic simulation"; gala-um's side in docs/GALA_UM.md.

### Serial log

The human log, shared by the kernel and program output on the serial
port. Lines that tools and agents rely on:

- `stats:` (heap in use and its peak, free and total frames and the
  fewest free since boot) and `nt:` (`nt::stats`: system calls, waits,
  user faults resolved, processes created, with their time; always on;
  docs/PERF.md) at the end of a run.
- `process x.exe (pid 4, session 1): starting at ..., personality nt`
  and the exit line; the unknown-service line names its dispatcher
  (`unimplemented system service 0x.. (dispatcher nt::syscalls,
  personality nt)`); a thread whose personality has no dispatcher logs
  `syscall: no dispatcher for personality ..`.
- `session` lines for a session made after boot; `window: pid P
  (session S) ... not visible across sessions` (the first 32 times).
- `console N: Ctrl+C k` and `console N: pid P tid T (name) takes Ctrl+C
  k after m ms`, with a warning `took Ctrl+C k late` from 250 ms: when
  each process's control handler took the event.
- `display:` at the end of a boot (virtio-gpu commands, flushes, mode
  changes, errors, resets, the last error).
- Kernel command-line options that add lines: `trace` (every system
  call; `um --trace` in gala-um), `gputrace` (every virtio-gpu command
  and its result), `logtee=C:/dir` (what programs write below a
  directory, as `logtee: <file>: <line>`; sshd's and sshd-session's logs
  in the sshd scenarios), `conpty=trace` (a pseudo console's VT traffic
  in both directions).

Emergency output (panic, watchdog, state dump) takes the port's lock
when it comes free within 10 ms, so whole lines do not mix with a
program's; if the interrupted code holds the lock it writes anyway.
Diagnostics that may run from an interrupt or NMI use
`IrqMutex::try_lock_untracked`, so lockdep does not record an order no
real path has.

### Event stream

A second output channel for tools: QEMU's `isa-debugcon` on port 0xe9
(`arch::debugcon`; one `out` per byte, no UART handshake), which xtask
always writes to `<iso>.events.jsonl` next to the serial log. One JSON
object per line, starting with `"t"` (ns since boot), `"cpu"`, `"by"`
(the client id of the thread whose work emitted it: an NT thread id, a
Linux tid, 0 for a kernel thread; the scheduler keeps it per CPU, so
`emit` reads it with one load) and `"ev"`; `pid`/`tid` in the rest name
the event's subject, which may be another thread:

    {"t":ns,"cpu":0,"by":8,"ev":"syscall","pid":4,"tid":8,"name":"NtCreateFile",
     "args":[...],"status":"0xc0000034","ns":1200}

Off by default and cheap when off (one relaxed atomic load of the
category mask). `events=<categories>` or `events=all` on the kernel
command line; `XTASK_EVENTS=all cargo xtask boot-test` for every
scenario; `EVENT_EXPECT` in xtask checks a scenario's stream (scenario
`events`).

| Category | Events |
|---|---|
| `syscall` | each call (pid, tid, nr, name, args, status, ns); `wait` on entry to a wait, so a thread blocked for good shows, naming the object waited on (`obj`: its type, `oid`: its identity, the same in every process; the first of several). `events_pid=a,b,..` limits system calls and waits to those processes (scenario `events_pid`; `XTASK_CMDLINE=events_pid=4` for a boot test), and `events_match=<text>` adds every process whose command line contains the text when it starts (a program's processes chosen by what they are: `events_match=network.mojom.NetworkService` for Chromium's network service; scenario `events_match`); 64 processes at most |
| `image` | the exe and every image view (pid, NT path, base, size) |
| `thread` | create (start address, TEB), exit (status) |
| `process` | start (command line, `"session"`, `"personality":"nt"`, `"ppid"` and `"ptid"`: the parent process and the thread that created it, 0 for processes the kernel starts; scenario `events_process`), exit; `session` (a session made after boot: id, its BaseNamedObjects) |
| `fault` | every user-mode CPU exception, before it is resolved or delivered (pid, vector, rip, cr2, error) |
| `sample` | the sampling profiler: every `sample_hz=` (default 100) a busy CPU's timer tick writes the interrupted thread (`by`), `mode` (`user`/`kernel`) and `rip`. `cargo xtask profile <events.jsonl \| scenario> [--pid N] [--tid N] [--from S --to S] [--top N] [--symbols DIR]` attributes them to processes (Chromium's `--type` and utility sub-type shown), threads, modules (the `image` events; record `image,thread,process` with it) and functions: kernel addresses from the kernel ELF, a module's from `DIR/<module>` and its symbols (llvm-symbolizer at its preferred base; `target/prebuilt/chromium-syms` for Chromium), else 256-byte blocks. Scenario `events_sample` |
| `ipc` | pipes: `pipe create` (pipe number, name, message type, instance), `pipe connect`, `pipe write` (`end`, `to`, `off`: the offset of its first byte in the stream to that end, `bytes`, `queued`), `pipe read` (`end`, `off`, `bytes`, `left`; `by` may be the writer completing a waiting read), `pipe disconnect` (`unread`: bytes discarded), `pipe close`; I/O completion ports: `iocp post` (port number, key, ctx, status, bytes, depth) and `iocp dequeue` (the same with `queued_t` and `queued_by`: when and by which thread the packet was queued). Scenario `events_ipc` |
| `mock` | live mocking's changes (clock control, record/replay) |
| `net` | TCP accept, connect, close, reset, with local and remote addresses |
| `logon` | LogonUser and LSA logons: pid, user, domain, type, how, status (never the password) |
| `console` | console sessions and pseudo consoles created, resized, closed (with how many programs it ended); `console ctrl`, `console ctrl taken` |
| `display` | virtio-gpu modes set, device resets, failed commands with their names |
| `clock` | `clock_cal` (how the TSC was calibrated), the RTC check, `clock_recalibrated`; a wrong clock (`clock_wrong`) is written whatever the mask |
| `file` | `open`: each NtCreateFile/NtOpenFile with the name as given, its root directory handle, the handle made and the status (the opens no ledger line names) |
| `window` | `create` (hwnd, parent, pid, session), `describe` (tid, class, text once user32 knows them), `destroy`, `other session` (a process asks about, sends or posts to another session's HWND) |

A `record` boot's recording also goes to this port (`"ev":"rec"`,
whatever the mask; extracted to `<iso>.rec`).

`cargo xtask events <events.jsonl>` summarizes a stream: events by kind,
system calls by total time, failing statuses by service, each thread's
last calls, faults, processes, images. When the stream was on, it is
copied next to a failed log as `<scenario>.events.jsonl`.

### OpenTelemetry export

`xtask/src/otel.rs` turns one boot's event stream and serial log into
OTLP/JSON (stage OT1 of ROADMAP OT): `cargo xtask otel <serial log |
events.jsonl | scenario> [<events.jsonl>] [--out <prefix>] [--endpoint
http://host:4318]` (or `OTEL_EXPORTER_OTLP_ENDPOINT`), and `XTASK_OTEL=1`
makes boot-test write it for every boot. How to view it: WORKFLOW
"OpenTelemetry". The kernel is unchanged: this maps what the stream has
today. A hand-written encoder (no OTel crate) follows the OTLP/JSON
mapping of opentelemetry-proto: ids in hex, 64-bit integers as decimal
strings, enums as numbers.

| Source | OTel | Names (semantic conventions first, else `gala.`) |
|---|---|---|
| the boot | one resource; one trace whose root span is `boot <scenario>` (0 to the last event) | `service.name` gala-os, `service.version` (the banner), `service.instance.id` (= the trace id), `gala.scenario`, `gala.kernel.build` (profile and commit), `gala.cmdline`, `gala.cpus` |
| `process start` / `process exit` | span `process <name>`, child of its parent process (`ppid`; the boot for processes the kernel starts), with a link `created by` to the creating thread's span (`ptid`) | `process.pid`, `process.parent_pid`, `gala.creator.tid`, `process.executable.name`, `process.command_line`, `process.exit.code`, `gala.exit_status`, `gala.session`, `gala.image_base`, `gala.entry`; status ERROR for an NTSTATUS error |
| `thread create` / `thread exit` | span `thread <tid>`, child of its process | `thread.id`, `process.pid`, `gala.start_address`, `gala.teb`, `gala.exit_status` |
| `syscall` (end time `t`, duration `ns`) | span named by the service, child of its thread | `code.function.name`, `gala.syscall.nr`, `gala.syscall.status`, `gala.syscall.args` (hex), `gala.cpu`, `thread.id`, `process.pid`; status ERROR for 0xC... |
| `wait` (a wait's entry) | merged into its `syscall` span; one that never returns: span event `<service> (entered, no return)` on its thread | as the stream has them |
| `fault` | span event `fault #PF page fault` (by vector) on the thread, else the process |
| `pipe write` / `pipe read` | a zero-length span on the thread that did it; a read links each write whose bytes it took (`data from`) | `gala.pipe.id`, `.name`, `.offset`, `.end`/`.to_end`, `.waited_ns` (from the first such write), `messaging.message.body.size` |
| `iocp post` / `iocp dequeue` | a zero-length span on the thread that did it; a dequeue links its post (`completion of`, FIFO per port and context) | `gala.iocp.port`, `.key`, `.context`, `.bytes`, `.status`, `.queued_ns`, `.queued_by` | `gala.fault.vector`, `.rip`, `.address` (cr2), `.error` |
| `image` | span event `image load <file>` on the process | `gala.image.name`, `.base`, `.size` |
| every other kind (`console *`, `clock_*`, `tcp *`, `logon`, `session`, `display *`, ...) | span event of that name on its thread (`tid`, else the emitting thread `by`), process, or the boot | `gala.<field>`; `pid`/`tid` as `process.pid`/`thread.id` |
| `sample` (the sampling profiler) | **profiles** (`<prefix>.profiles.json`, an ExportProfilesServiceRequest of opentelemetry-proto profiles/v1development; POSTed to `/v1development/profiles`): one Profile (sample type `samples`/`count`, period `cpu`/`nanoseconds`, 10 ms); a sample per (location, attributes, link) with its timestamps; a location per address in the mapping of the module there (the `image` events; `kernel` for kernel addresses), with its function where known (the kernel's from its ELF, a program's from `XTASK_SYMBOLS=<dir>`: the heaviest 400 blocks' addresses); a link to the sample's thread span, so a profile view and the trace meet | `thread.id`, `process.pid`, `process.executable.name`, `gala.cpu.mode` |
| `rec` (record/replay bytes) | not mapped (counted) | |
| serial log lines | one log record each, severity from `[info ]`/`[warn ]`/`[error]` (none: program output, severity 0), linked to the process span when the line names a pid, else to the boot | `gala.log.line`, `gala.time.estimated`, `process.pid`, `gala.log.source` |
| `crash report:` block | one log record (ERROR) and an `exception` span event on the crashing thread (status ERROR) | `exception.type` (code and name), `exception.message`, `exception.stacktrace` (registers, stack words), `gala.crash.address`, `gala.crash.location` (module+offset) |
| `statedump: begin` .. `end` | one log record (WARN) and a `state dump` span event on the boot | `gala.statedump` |
| `KERNEL PANIC:` | log record (FATAL) and an `exception` event on the boot (status ERROR) | `exception.*` |
| `stats:` lines | gauges, one point per line | `gala.stats.<words>` (`heap_in_use` KiBy, `frames_free` and `.total`, ...) |
| system calls | sums per service over the boot | `gala.syscall.calls`, `gala.syscall.duration` (ns), by `code.function.name` |
| the converter itself | a log record `otel: ...` on the boot; `gala.otel.gaps`, `gala.otel.not_mapped` on the root span | |

- **Times**: the stream's `t` is ns since boot; wall time is the log's
  modification time (the boot's end) minus the last event's `t`. Serial
  lines carry no time: each takes the time of the latest line before it
  that can be placed (a process start or exit line matched to the
  stream's event, an `uptime_ms=` mark, a state dump's `now N ms since
  boot`) and says so (`gala.time.estimated`).
- **Ids** are derived from the boot's content, so converting the same
  boot twice gives the same ids.
- **The slowest hand-offs**: `otel: slow pipe data:` (the five longest
  times from a write to the read that took it: pipe, name, end, bytes,
  writer and reader threads) and `otel: slow completion:` (the five
  longest times a packet waited in its port: port, context, the thread
  that queued it and the one that took it).
- **Its own observability**: every conversion prints `otel:` lines: what
  each event kind became, what was **not mapped** (per kind, e.g. `rec`,
  unparsed lines) and the **gaps in the stream** (per kind: a fault
  without a thread, a wait without return, a thread or process without
  an exit or seen before its create event, a system call without a
  thread); the same text is a log record in the export.
- Scale: sshd_shell with `XTASK_EVENTS=all` gives about 9500 spans and
  2000 span events from 13000 events. The stream's own gaps (OT2's list)
  are under "Open gaps".

### Missing-features ledger

Every STATUS_NOT_IMPLEMENTED, STATUS_INVALID_SYSTEM_SERVICE, unknown
information class, unsupported flag or TODO path increments a counter
keyed by (service, what, detail) in a fixed 64-entry table. When running
a real program, this list is the work queue, ranked by how often it was
hit (scenario `ledger_test`).

- The dispatcher records unknown services and, generically, any
  STATUS_INVALID_INFO_CLASS (with the class argument, per
  `ledger::class_argument`) and STATUS_NOT_IMPLEMENTED. Other code calls
  `nt::ledger::record(service, what, detail)`.
- Lines `ledger: {"service":..,"what":..,"detail":..,"hits":..}` are
  dumped after each program, before `boot ok` and in a state dump.
- `cargo xtask report` merges them over all scenarios and real programs
  into `target/report/ledger.md`, ranked by programs and hits.

### Failed logs: triage and history

Every failed boot test leaves `target/failed/<scenario>.log` (repeats
keep one each: `target/failed/<name>.r<i>.log`), which ends with:

- **QEMU and host lines** (see "Early boot"): how QEMU ended, its vCPUs,
  host load and memory; for network scenarios `xtask: net: host
  127.0.0.1:<port> forwarded to the guest's port <n>`, and QEMU's
  standard error (`<iso>.qemu.stderr`, e.g. a forward it could not set
  up).
- **Symbolized backtraces** (panic, state dump, watchdog), each block
  saying whose backtrace it is.
- **A triage block** (`xtask/src/triage.rs`, also printed on the
  console): scenario, verdict, duration, host CPUs/load/free disk, guest
  CPUs, heap (peak) and frames (fewest free), warning/error counts and
  the first error, panic/exception/watchdog/leak/out-of-memory/
  missing-function lines, programs begun and not ended, processes that
  failed, the ledger's top five, the state dump's CPUs, locks and
  threads, each thread's last system calls (event stream, else flight
  recorder), user faults, a TSC under 500 MHz as a suspicious clock,
  `triage: clock WRONG`, the last lines. `cargo xtask triage <log>
  [events.jsonl]` prints it for any log.

**History**: every `cargo xtask test`/`boot-test` appends one line per
scenario to `target/report/history.jsonl` (time, commit, host load,
pass/fail, VM seconds, whether the VM was shared, reason; the last 20000
runs are kept) and ends with `xtask: health:` lines: scenarios that both
passed and failed in their last 30 runs (with the last failure's reason)
and scenarios whose latest pass, alone in a VM, took over 1.5 times, and
10 s more than, the median of their last ten.

**Disk space**: an image is not started with less than 300 MiB free,
nor packed into an ISO without its tree's size free; the error names the
free space, the need and what to delete (WORKFLOW "Disk space").
Out-of-space errors on the way get the same hint.

### Backtraces and panic output

- A kernel panic prints the message, a backtrace (`backtrace: 0x...`,
  return addresses from a frame-pointer walk, `arch::backtrace`; the
  kernel is built with `-C force-frame-pointers=yes`), the flight
  recorder's last 48 records across CPUs, the memory line (heap and its
  peak, frames and the fewest free) and each CPU's thread (when the
  scheduler's lock is free), then writes the crash record.
- xtask symbolizes backtraces with `llvm-addr2line`/`addr2line`
  whenever it shows a failed boot's log; `cargo xtask symbolize <log>
  [--release]` does it for any log.
- Fault injection's `failtrace` prints a backtrace at each injected
  failure, so the code that mishandled it is named even when the
  symptom comes much later (WORKFLOW "Fault injection").

### Flight recorder

`kernel/src/flight.rs` (ROADMAP R4): an always-on ring per CPU of the
last 256 records, kept whether the event stream is on or not. A record
is three words (time << 8 | kind, two values): one atomic increment and
three stores. Kinds: `syscall` (number | pid << 32, thread id),
`syscall-done` (number, status), `process-exit` (pid, status),
`watchdog` (the CPU found stuck). Printed merged by time as `flight:`
lines (service names, pid and tid on system calls) after a panic, in a
watchdog report and in the state dump.

### Hang watchdog

- **Stuck CPUs**: every timer interrupt counts its CPU's heartbeat; once
  a second each CPU looks at the next online one's. One that has not
  moved for 5 s (interrupts off: a lock never released, a loop in a
  handler) is reported once with its last flight records, and gets an
  NMI that prints `watchdog: CPU N kernel rip ...` and `watchdog: CPU N
  backtrace: ...` (symbolized in the failed log). Scenario `watchdog`
  (`hangtest` stalls CPU 1).
- **Threads stuck in the kernel**: each thread's last progress is kept;
  a tick that finds a thread in kernel code with no progress for 20 s is
  reported with its name, rip and the CPU's last records, again every
  20 s while it stays. Progress is returning from a system call, a tick
  in user mode or a voluntary yield (a polling wait); blocking restarts
  the count when the thread runs again (a long wait is not time in the
  kernel); preemption is not progress. Scenario `kernel_watchdog`
  (`spintest` spins the boot thread 25 s).

### State dump

A read-only "what are you doing" with no channel of its own
(`kernel/src/statedump.rs`; cheap: nothing runs until asked). When a
boot test times out, xtask injects an NMI (QEMU monitor `nmi`, every
CPU) before killing the VM and waits up to 20 s for the dump:

1. From the NMI (only atomics and `try_lock`): each CPU's rip and kernel
   backtrace (`statedump: cpu N ...`, symbolized in the failed log), the
   thread each CPU runs, whether the scheduler, heap, frame and console
   locks are free (a lock held for the whole dump is the deadlock), heap
   and frame figures, the flight recorder.
2. From the boot thread (may take locks): every NT thread's system call
   and wait object (with `session N personality nt (name)`), every
   scheduler thread, the ledger, the statistics, each session's
   windows (`statedump: session N: ... windows ...`), then `statedump:
   end`. A machine too stuck for this still has part 1.

xtask's line says how far the dump got (`did not answer the NMI`: the
machine is wedged or the handler is not installed yet).
`XTASK_TIMEOUT=<s>` shortens any scenario's timeout to get a hang's dump
quickly. In gala-um, SIGQUIT (`timeout -s QUIT`) prints part 2. Scenario
`statedump_test` (a program that hangs on purpose; also in um-test).

### Crash reports (programs)

`nt::crashreport`: every user exception keeps a fixed-size note in its
process. When the process ends with that exception's code (unhandled),
`crash report:` lines give the code and its name, the thread, the
personality and session, the address as module+offset (the executable or
an image view), what was read/written/executed, the registers and the
top eight stack words, each named by module when it points into an
image. ntdll reports an exception no handler took before it calls the
top-level filter (GalaUnhandledException, its record and context), so a
filter that ends the process with its own status (Chromium's Crashpad:
0xffff7002 when its handler fails) still leaves the report, headed
"ended with status S after an unhandled exception C"; a filter that
continues execution withdraws it. Scenarios `crash`, `crash_dispatched`
and `crash_filtered`.

### Crash record across reboots

`kernel/src/crash.rs` (ROADMAP R4): a panic writes its message, the
backtrace and the flight recorder's last 48 records to the crash area
(the last 16 sectors of a version 2 volume, `fs::volume::CRASH_SECTORS`:
`GCRASH\0\x01`, length, CRC-32, the text) with `disk::write_in_panic`,
which never waits (a busy device means no record). The next boot that
mounts the volume prints it (`crash: ...` lines) and clears it. It stays
on the machine (privacy by design). Scenario `crash_record` (two boots:
`crashtest` panics on purpose, then the record is printed).

### Clock checks

A TSC calibrated wrong (under host load: the vCPU descheduled during
the calibration window) makes guest time run fast: sleeps end early and
the watchdog reports CPUs "stuck". Design: ARCHITECTURE "Interrupts and
time". What it leaves behind:

- `tsc: ... from HPET ..., 5 samples (n paused, n outliers), spread x%`,
  the phase `tsc_mhz` and a `clock_cal` event: how the TSC was
  calibrated (bracketed samples against the HPET / PM timer, median).
- A kernel thread checks the TSC against the CMOS RTC after boot and
  recalibrates when they disagree: `clock: RTC check: agrees` or `clock:
  WRONG`, and `clock_wrong` / `clock_recalibrated` events.
- xtask (`xtask/src/clockcheck.rs`) times every `uptime_ms=` mark against
  the host, in a thread of its own that reads the serial log every 20 ms:
  `xtask: clock: <source>: guest N ms over host A..B ms` (the mark's poll
  window) or `xtask: clock: WRONG: ...`. The triage reports both, and a
  failure in a boot whose clock was wrong says so in its verdict.
- Every boot ends with `xtask: vm time: <run> run of <wall> wall (<w>
  waiting for a host CPU)`: what its time limit counted
  (`xtask/src/vmtime.rs`); a timeout says which limit it hit.
- Scenarios `clockcal_test` (host CPUs kept busy, `calstall`) and
  `clockcal_recal` (`calwrong`).

### Early boot

A boot that dies before or without a panic (an AP that triple-faults
resets the machine: ARCHITECTURE "Processors") would leave a serial log
that just stops and a QEMU exit status of 0. Every boot test records,
always on and cheap:

- **QEMU's side** (`xtask/src/telemetry.rs`), next to the image
  (`target/<profile>/<image>.*`): `.qemu.log` (QEMU `-d
  cpu_reset,guest_errors -D`: each CPU reset with its registers, "Triple
  fault", bad guest accesses), `.bios.log` (SeaBIOS's debug port 0x402:
  the CPUs it counted, the boot device), `.phases.log` (the phase
  markers below). After the run, `xtask:` lines end the serial log (and
  so the failed log): how QEMU ended (status 33 / 35 = the kernel's exit
  codes; 0 = a reset or power-off without them, which with `-no-reboot`
  is what a triple fault looks like; a signal; a timeout), the vCPUs QEMU
  created (QMP `query-cpus-fast` on a socket in the temp directory), the
  host's load average and available memory at start and end. After an
  abnormal end (not 33/35, not stopped by xtask) also the last 12 phase
  markers and the last 30 lines of QEMU's log.
- **CPU count**: xtask compares the kernel's `smp: N CPUs online` with
  the vCPUs it asked for (2, or 1 for `onecpu_*`); a boot short of CPUs
  is booted once more (not with a disk), and a failure on one says first
  `VM got 1 of 2 CPUs (host overloaded?)`.
- **Boot phase markers** (`arch::bootphase`): one line per step on
  debug-console port 0x403 (the event stream has 0xe9), lock-free and
  allocation-free, written only under QEMU (CPUID's hypervisor leaf; on
  a PC that port may be the chipset's): `phase <name> cpu <APIC id> tsc
  <TSC>` for entry, boot_info, xsave, gdt_idt, memory_map, frames,
  paging, apic, tsc_mhz, sched, smp_start, `ap_online <index>` (from
  each AP), smp_done, drivers, manifest, program (each `run=` program),
  boot_ok. The last line is how far the boot got, even when the machine
  reset before the serial log said anything.
- **Faults**: every CPU exception in kernel mode first writes `fault
  <vector> rip <addr> error <code> cpu <id> tsc <n>` to the phase port,
  then panics as usual; a double fault runs on its own IST stack
  (`gdt::IST_DOUBLE_FAULT`) and goes the same way. A panic writes `panic
  cpu <id>`. Before the IDT is loaded (the first few hundred
  instructions) a fault still triple-faults: the last phase is then
  `entry` or `boot_info`.
- **SMP detail** in the boot log: Limine's MP response (count, BSP, each
  APIC id), the MADT's enabled CPUs and which of them the bootloader did
  not start, how long start-up took, CPUs the kernel started itself.

### Screen

The kernel's screen console mirrors the serial log on the framebuffer;
`cargo xtask screenshot [--uefi] [--at 1,3,5] [--name N] [kernel
cmdline...]` saves PNGs at chosen times in `target/screenshots/`
(WORKFLOW "Screenshots").

### Benchmarks

`user/bench` (one program, unchanged on Windows) prints `bench: <name>
ns_per_op <n>`; `cargo xtask bench [bench-windows.txt]` writes
`target/report/bench.json` and a table next to Windows' numbers
(ROADMAP P1). Method and numbers: docs/PERF.md.

## Open gaps

What failures still leave unexplained, ranked by the time they cost.

| Gap | What is missing | Would explain |
|---|---|---|
| Program output shares the serial port | an `out` event per write with the pid, or a second serial port | tests matching a program's lines alone |
| Crash reports show stack words only | a real stack walk (unwind data or frame pointers) and export names (nearest exported symbol) | where in a program or DLL it died |
| "address space still referenced" | who took each reference (a per-reference tag or backtrace in debug builds) | which object leaks the space; the holders are printed, the triage names the leak |
| History covers only test/boot-test | `XTASK_REPEAT` boots and um-test in `history.jsonl` | flakiness found by repeats and in gala-um |
| Flight recorder kinds | context switches, page faults, interrupts | what a CPU did just before a hang |
| Crash record | symbols (addresses only now), the screen (a blue screen with the record) | a panic on a machine without serial |
| Event stream (OT2's list) | an entry event for every system call (only waits have one); trace context propagated across processes and IPC (OT3); a time on serial lines; the boot's wall-clock start (guessed from the log's modification time) | cross-process traces, calls that never return |
| Event stream, tools | process-relative times; user-address symbolization from the image events | user addresses without manual lookup |

## Planned facilities

- **Trace context in the kernel** (ROADMAP OT2..OT5): system calls,
  waits, faults, I/O and network as spans in per-CPU rings; propagation
  through ntdll/kernel32, process creation, pipes, ALPC/RPC, IOCP; an
  ETW bridge; an in-guest OTLP exporter controlled from the `gala` CLI.
  Design rules:
  - The kernel writes compact binary records to per-CPU ring buffers:
    fixed size, no allocation, never blocking, with a drop counter. An
    exporter turns them into OTLP outside the hot path (xtask over
    debugcon/virtio-serial; in a running system a user-mode service
    sending OTLP/HTTP). Telemetry must not perturb record/replay (R7).
  - Context propagation is the core: each thread carries a trace
    context (W3C traceparent semantics) in a gala-private TEB slot that
    the kernel reads at system-call entry; kernel work becomes child
    spans; the context follows work across process creation, pipes,
    ALPC/RPC, I/O completion ports and work queues.
  - Windows programs speak ETW: the ETW-to-OTel bridge (EventRegister/
    EventWrite, TraceLogging in advapi32) turns what software already
    emits into logs and spans; later the same for the Linux
    personality.
  - What exists maps onto it: events to span events and logs, `stats:`
    to metrics, crash reports and dumps to logs carrying the trace
    context they happened in; `cargo xtask triage` reads spans.
  - One clock (R1): timestamps from the kernel's monotonic clock, mapped
    to wall time once per export; ring state survives a kernel handover.
  - Sources: the OpenTelemetry specification and opentelemetry-proto
    (Apache-2.0), W3C Trace Context.
- **Trace diffing against Windows**: `cargo xtask diff-trace gala.jsonl
  windows.jsonl` aligns two system-call traces of the same program and
  shows the first divergence (service, arguments, status). The Windows
  side comes from a user-mode tracer (an NtTrace-like tool, or ETW
  system-call events) run by `user/bin/run-all.ps1`; those results are
  black-box behaviour, so this stays clean-room.
- **Sampling profiler, next steps**: a short stack per sample (unwind
  data or frame pointers) and folded stacks for a flame graph; samples
  into a per-CPU ring instead of the debug console (each sample costs
  its bytes' port writes under TCG).
- **Counters**: per-service call counts and TSC time (the `nt:` line
  has totals), spin iterations on each `IrqMutex` (contention), context
  switches and IPIs per CPU; printed at exit and queryable at run time.
- **Without kernel changes**: QEMU TCG plugins (`-plugin`) count
  executed instructions or basic blocks per address range (coverage and
  a timing-independent profile; needs a QEMU built with plugins,
  Ubuntu's has none); under KVM, `perf kvm` on the host samples the
  guest.
- **Debugger**: a xtask command that boots a scenario with QEMU's
  gdbstub (`-s -S`) and a gdb script, for backtraces and memory in batch
  mode (not built; gala-um processes take `gdb -p`, WORKFLOW "gala-um").
  QEMU record/replay (`-icount shift=auto,rr=record`) to make a failing
  SMP run reproducible.
- **Live introspection channel**: a command channel (second serial port
  or virtio-vsock) with read-only queries (threads and their states and
  CPUs, a process's handle table and VADs, the object namespace,
  counters); later read-write for tests (ROADMAP R7, live mocking:
  inject a fault, jump the clock, swap a device for a wrapper), off
  unless booted with an explicit flag, every change an event.
- **Screen status area**: CPU load, threads, memory drawn by the kernel.
- **Fuzzing**: a user-mode fuzzer program calling services with random
  or mutated arguments in CI (the kernel must never panic on user
  input), and `cargo fuzz` on the host for pure parsers (PE, paths, PE
  resources, registry hives).

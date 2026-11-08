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
| Screen | kernel screen console; Test presenter frames (`presenter=test`, GalaSession) | `cargo xtask screenshot`; `test-<scenario>.screen<n>.png`; ARCHITECTURE "Presenters (B3a)" |
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
  (with `, 32-bit x86 (WOW64)` for a 32-bit program; `wow64:` lines say
  which wow64.dll is used, or why a 32-bit program cannot start, e.g.
  `wow64: x.exe imports DLLs, and there is no \Windows\SysWOW64\ntdll.dll
  to bind them`; `ntdll: using \Windows\SysWOW64\ntdll.dll` names the
  32-bit ntdll) and the exit line; a service or information class gala's
  wow64.dll cannot convert yet is in the ledger as `wow64.dll!service
  0x....` (with the case: `wow64.dll!service 0x0007 (AFD IOCTL)`) or
  `wow64.dll!<service>(class)`, an exception or APC it cannot hand to
  32-bit code as `wow64.dll!exception (32-bit stack full)` or
  `wow64.dll!APC (no 32-bit ntdll|32-bit stack full|64-bit routine)`,
  and the 32-bit msvcrt's longjmp over a `__try` level it cannot unwind
  as `msvcrt.dll!longjmp(try level)` (a missing msvcrt export stops the
  load as `missing export`);
  `wow64: ... cannot free an exited
  thread's 64-bit stack|TEB32` if a WOW64 thread's exit leaks them; the unknown-service line names its dispatcher
  (`unimplemented system service 0x.. (dispatcher nt::syscalls,
  personality nt)`); a thread whose personality has no dispatcher logs
  `syscall: no dispatcher for personality ..`.
- `session` lines for a session made after boot; `window: pid P
  (session S) ... not visible across sessions` (the first 32 times).
- `winsta: pid P (x.exe, session S) is on WinSta0\Default (why)` at a
  process's first window station or desktop call (a warning when the
  name it asked for does not exist), `winsta: ... asked for window
  station "x": 0xc0000034` for an open of an unknown name (the first
  32), `winsta: made window station Service-0x...$` (ARCHITECTURE
  "Window stations and desktops (B3a)").
- `desktop: session S is live (WxH), shown by none|console|test`, `desktop:
  session S is now shown by test (WxH; was none)`, `presenter: test frame
  for session S: WxH at 0x... (...)` (where xtask's headless screen checks
  read it), a bare desktop (nothing can show it) going live: `desktop:
  session S keeps its windows (WxH), shown by none (no screen and no
  presenter=)` or `(server profile)`, a presenter refused there
  `desktop: session S cannot be shown by test: server profile`; `desktop: the
  console now shows session S (WxH; was session T, which keeps its N
  windows shown by none)` (a console switch; a warning `the console
  could not move ...; it stays` when it fails), `desktop: pid P's input
  for session S refused: 0x... (...)` (GalaSession input to a desktop no
  Test frame shows; the first 8), `desktop: session S's start menu
  asked for ...: not started` (ARCHITECTURE "Presenters (B3a)"). The
  state dump's sessions line says which session the console shows, and
  each session's line what shows its desktop (a bare one `presenter none;
  never shown: <why>`).
- `console N: Ctrl+C k` and `console N: pid P tid T (name) takes Ctrl+C
  k after m ms`, with a warning `took Ctrl+C k late` from 250 ms: when
  each process's control handler took the event.
- `display:` at the end of a boot (virtio-gpu commands, flushes, mode
  changes, errors, resets, the last error).
- Pool and handle quotas (`nt::quota`; ARCHITECTURE "Pool and handle
  quotas"): `quota: pid P (x.exe) over its nonpaged pool: U in use + N >
  limit L` (or `paged pool`, `handle limit`) when a process is refused,
  its first four times (the rest counted); `quota: <spec>` for a
  `quota=` boot option; and at the end of a run (gala-um too) `quota:
  most per process: paged pool K KiB (x.exe pid P), nonpaged pool ...,
  handles N (...); limits ...`, the largest peaks any process reached
  against the limits new processes get (how much headroom the defaults
  leave); during the run `quota: new most per process: nonpaged pool N
  KiB (x.exe pid P)` each time that largest peak reaches a new power of
  two (from 256 KiB, or 1024 handles), so a server xtask stops before
  the end still shows how far it went; `quota: pid P (x.exe) NonPaged
  pool limit set to N (in use U)` when NtSetInformationProcess
  (ProcessQuotaLimits) changes one. A socket refused for its buffers
  shows as the owner's `over its nonpaged pool` line (a listener refused
  a listening socket logs it once per stretch); one refused by the
  network stack's own budget or for memory as `net: a loopback socket is
  refused: its N KiB of buffers are over the budget (... their buffers
  .. KiB of the stack's .. KiB ...)`, once until a socket goes.
- Kernel command-line options that add lines: `trace` (every system
  call; `um --trace` in gala-um), `gputrace` (every virtio-gpu command
  and its result), `logtee=C:/dir` (what programs write below a
  directory, as `logtee: <file>: <line>`; sshd's and sshd-session's logs
  in the sshd scenarios), `conpty=trace` (a pseudo console's VT traffic
  in both directions), `wayland=trace` (every Wayland request and event
  as `WAYLAND_DEBUG=1` prints them: `wayland: pid P
  wl_display#1.get_registry(new id wl_registry#2)`, events after `->`,
  and each window message the server takes from the desktop for a
  client's windows: `wayland: pid P input: window W message M wparam X
  lparam Y`; scenarios `lx_wl_pie`, `lx_wl_input_pie`).

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
| `thread` | create (start address: the Win32 start routine, TEB, `teb32`: a WOW64 thread's 32-bit TEB, else 0), name (`thread name`: SetThreadDescription's, as NtSetInformationThread ThreadNameInformation sets it), exit (status) |
| `process` | start (command line, `"session"`, `"personality":"nt"`, `"machine"`: `"x86"` for a 32-bit process running through WOW64, `"x64"` otherwise, `"ppid"` and `"ptid"`: the parent process and the thread that created it, 0 for processes the kernel starts; scenario `events_process`), exit (with the process's pool and handle peaks: `paged_peak`, `nonpaged_peak` in bytes, `handles_peak`); `session` (a session made after boot: id, its BaseNamedObjects); `quota exceeded` (pid, `kind`: `paged pool`, `nonpaged pool` or `handle limit`, `usage`, `want`, `limit`, `count`: the process's refusals so far; every refusal, unlike the log line) |
| `fault` | every user-mode CPU exception, before it is resolved or delivered (pid, vector, rip, cr2, error) |
| `sample` | the sampling profiler: every `sample_hz=` (default 100) each busy CPU is sampled by an NMI that the next CPU's timer tick sends it (round robin), so samples land inside code that runs with interrupts off (every `IrqMutex` section) too; the NMI records the interrupted thread (`by`), `mode` (`user`/`kernel`) and `rip` into a per-CPU ring of 8 that the CPU's own tick writes out (the `events:` line at exit counts the samples asked of another CPU, written in user mode and in the kernel, and lost to a full ring, interrupts off for over 8 sample periods, and the last sample before a loss carries `lost` with their number: `cargo xtask profile` lists those stretches by call path; scenario `events_sample` with `sampleprobe` checks that samples land inside a half second with interrupts off); a kernel sample also its `stack` (the return addresses of the interrupted code's frames, innermost first, at most 24: frame pointers, the chain ending at the first user-mode frame pointer). `cargo xtask profile <events.jsonl \| scenario> [--pid N] [--tid N] [--from S --to S] [--top N] [--symbols DIR] [--release]` attributes them to processes (Chromium's `--type` and utility sub-type shown), threads (each with where it starts, `starts at module+offset`, from its `thread create` event's Win32 start address), modules (the `image` events; record `image,thread,process` with it) and functions: kernel addresses from the kernel ELF, a module's from `DIR/<module>` and its symbols (llvm-symbolizer at its preferred base; `target/prebuilt/chromium-syms` for Chromium), else 256-byte blocks; from the kernel stacks, each kernel function's share including what it calls and the heaviest call paths (`leaf <- caller <- ...`), which name who calls a hot function. With one CPU the tick samples itself, and a tick held back while interrupts are off is taken where they come back on: there a lock guard's drop (`IrqGuard`, `SpaceGuard`) as a leaf stands for the whole section the lock was held. Scenario `events_sample` |
| `ipc` | pipes: `pipe create` (pipe number, name, message type, instance), `pipe connect`, `pipe write` (`end`, `to`, `off`: the offset of its first byte in the stream to that end, `bytes`, `queued`), `pipe read` (`end`, `off`, `bytes`, `left`; `by` may be the writer completing a waiting read), `pipe disconnect` (`unread`: bytes discarded), `pipe close`; I/O completion ports: `iocp post` (port number, key, ctx, status, bytes, depth) and `iocp dequeue` (the same with `queued_t` and `queued_by`: when and by which thread the packet was queued); Linux memfds: `lx memfd` (pid, fd, name, flags, node) and `lx memfd seal` (pid, fd, seals asked for, errno or 0). Linux AF_UNIX sockets: `lx unix` (pid, fd, op: socketpair, bind, listen, connect, accept with `new_fd` and `peer_pid`, close of a named socket; the name, a path or `@` and an abstract name; errno or 0: a refused bind, listen or connect is here) and `lx unix rights` (pid, fd, what: sent, received, dropped; count). Scenario `events_ipc` |
| `mock` | live mocking's changes (clock control, record/replay) |
| `net` | TCP accept, connect, close, reset, with local and remote addresses; shutdown (`tcp shutdown`), close and reset with the connection's byte counts (`sent`, `received` by its owner, `unsent`, `unread` then); a listener's backlog filling (`tcp backlog full`: further connections refused, or ignored on Linux sockets) and each SYN ignored over it (`tcp syn ignored`, the client's address); AFD's `afd shutdown after sends`, `afd send failed`, `afd shutdown failed` |
| `logon` | LogonUser and LSA logons: pid, user, domain, type, how, status (never the password) |
| `console` | console sessions and pseudo consoles created, resized, closed (with how many programs it ended); `console ctrl`, `console ctrl taken` |
| `display` | virtio-gpu modes set, device resets, failed commands with their names |
| `clock` | `clock_cal` (how the TSC was calibrated), the RTC check, `clock_recalibrated`; a wrong clock (`clock_wrong`) is written whatever the mask |
| `file` | `open`: each NtCreateFile/NtOpenFile with the name as given, its root directory handle, the handle made and the status (the opens no ledger line names) |
| `wayland` | the Wayland server (`wayland=on`): `wayland connect` (pid, client number), `wayland disconnect` (`why`: closed, protocol error, output full, no memory; `requests` handled), `wayland role` (surface, role, its object), `wayland map` (surface, window or -1), `wayland commit` (surface, `buffer`: whether one came, its width and height, `rows` copied into the window, window, `placed` with `x`/`y`: the outer position the window was put at for the configure this commit answers), `wayland configure` (surface, the `width` and `height` asked for, 0x0 the client's choice, `states` as bits 1 << state: maximized 2, activated 16, tiled 32-256, the xdg_surface's `serial`), `wayland error` (object, interface, code, message) and `wayland input` (`what`: enter, leave, button, axis, key, focus, unfocus, close, maximize, restore, place, or dropped for a window the client shows no surface in; surface, window, `code` the button or evdev key, `state` pressed or released, an axis's delta in WHEEL_DELTA units; motion is not evented): each with the client's pid and number. Scenarios `lx_wl`, `lx_wl_input` (with `window`: the desktop's `button` and `key` say where input went before the server took it) |
| `window` | `create` (hwnd, parent, pid, session), `describe` (tid, class, text once user32 knows them), `destroy`, `button` and `key` (the desktop's delivery: message, window, owner; the screen point or virtual key), `input taken` (a button or key its program took from the queue: message, window, owner; `by` the taking thread), `other session` (a process asks about, sends or posts to another session's HWND); `winsta`: `connect` (a process's window station and desktop at its first window station call: pid, session, station, desktop, `from`: `logon`, `startupinfo`, `parent`, `handle`), `connect failed` (the name it asked for, the status), `made` (a logon session's station). Scenario `winsta_test`; `presenter`: `live` (a session's desktop laid out: session, presenter, w, h; a bare one, which nothing can show, `bare`: why), `attach` (GalaSession attached a presenter: session, presenter, was, w, h), `console` (a console switch: session, from (-1: none), was, kept windows, w, h). Scenarios `presenter_test`, `server_windows` (the server kernel's `live`, presenter none, `bare`), `noscreen_windows` and `server_profile_windows` (the desktop kernel's bare `live` without a screen and in the server profile) |

A `record` boot's recording also goes to this port (`"ev":"rec"`,
whatever the mask; extracted to `<iso>.rec`).

`cargo xtask events <events.jsonl>` summarizes a stream: events by kind,
system calls by total time, failing statuses by service, each thread's
last calls, faults, processes, images. When the stream was on, it is
copied next to a failed log as `<scenario>.events.jsonl`.

### OpenTelemetry export

`xtask/src/otel.rs` turns one boot's event stream and serial log into
OpenTelemetry (stage OT1 of ROADMAP OT): `cargo xtask otel <serial log |
events.jsonl | scenario> [<events.jsonl>] [--out <prefix>] [--endpoint
http://host:4318]` (or `OTEL_EXPORTER_OTLP_ENDPOINT`), and `XTASK_OTEL=1`
makes boot-test write it for every boot. How to view it: WORKFLOW
"OpenTelemetry". The kernel is unchanged: this maps what the stream has
today. The conversion builds one model of the export (`otel/wire.rs`
`Data`: spans, log records, data points, in absolute times) that two
hand-written encoders (no protoc, no OTel crate) write from
opentelemetry-proto's definitions:

- **Files are OTLP/JSON** (for reading and diffing): ids in hex, 64-bit
  integers as decimal strings, enums as numbers; one request a line, at
  most 5000 spans or records a request.
- **What is sent is OTLP/HTTP binary protobuf** (`application/x-protobuf`)
  by the field numbers of common/v1, resource/v1, trace/v1, logs/v1,
  metrics/v1 and the collector's `Export*ServiceRequest`: ids as raw
  bytes (16 and 8), times fixed64, enums varints, `as_int` sfixed64,
  proto3 defaults left out except inside a oneof (an `AnyValue` of 0,
  false or "" is written). `OTEL_EXPORTER_OTLP_PROTOCOL=http/json` sends
  the same batches as OTLP/JSON instead; any other value (`grpc`) is
  refused. The unit tests decode the protobuf back, by the field
  numbers, and compare it with the JSON form.
- **Profiles stay OTLP/JSON** on the wire too (POSTed as
  `application/json`, which OTLP/HTTP receivers take beside protobuf):
  their dictionary is built as JSON tables, and profiles/v1development's
  field numbers still change between releases.
- **Compression**: gzip (`Content-Encoding: gzip`, flate2 over
  miniz_oxide) unless `OTEL_EXPORTER_OTLP_COMPRESSION=none`.
- **Batching** (`otel/send.rs`, built for the in-guest exporter of OT5
  as much as for the export after a boot): each signal has a `Batcher`,
  a bounded queue (4096 items) that gives a batch of at most 512 items
  (spans, log records or data points; a request groups consecutive
  points of one metric) when one is full or a second after the last
  batch was done. A push to a full queue is refused and counted. The
  export after a boot queues items as a live exporter would and sends
  what is due (`tick`), so a full batch leaves before the next item is
  queued, then flushes the rest.
- **Retries**: a request that fails to connect, or gets 429, 502, 503 or
  504, is retried up to 5 attempts in all, waiting 1 s and doubling up
  to 16 s; a `Retry-After` of seconds on 429 or 503 replaces the wait
  (at most 60 s; an HTTP-date is not read, the backoff applies). Other
  statuses are not retried. A batch that fails for good stops the
  export: its items count as FAILED and those never sent as NOT SENT. An
  accepted request's partial success (`rejected_*`, `error_message`) is
  read in either encoding and counted as REJECTED.
- **Its report**: `otel: sent to <endpoint>: http/protobuf, gzip; traces
  N requests, N spans, N KiB (N KiB before gzip); logs ...; metrics ...;
  profiles ... (JSON); N retries (last: why, which path, how long); N
  dropped`, with DROPPED, FAILED, NOT SENT and REJECTED per signal when
  not zero; a failure prints the error followed by the same line.

| Source | OTel | Names (semantic conventions first, else `gala.`) |
|---|---|---|
| the boot | one resource; one trace whose root span is `boot <scenario>` (0 to the last event) | `service.name` gala-os, `service.version` (the banner), `service.instance.id` (= the trace id), `gala.scenario`, `gala.kernel.build` (profile and commit), `gala.cmdline`, `gala.cpus` |
| `process start` / `process exit` | span `process <name>`, child of its parent process (`ppid`; the boot for processes the kernel starts), with a link `created by` to the creating thread's span (`ptid`) | `process.pid`, `process.parent_pid`, `gala.creator.tid`, `process.executable.name`, `process.command_line`, `process.exit.code`, `gala.exit_status`, `gala.session`, `gala.image_base`, `gala.entry`; status ERROR for an NTSTATUS error |
| `thread create` / `thread exit` | span `thread <tid>`, child of its process | `thread.id`, `process.pid`, `gala.start_address`, `gala.teb`, `gala.exit_status` |
| `syscall` (end time `t`, duration `ns`) | span named by the service, child of its thread | `code.function.name`, `gala.syscall.nr`, `gala.syscall.status`, `gala.syscall.args` (hex), `gala.cpu`, `thread.id`, `process.pid`; status ERROR for 0xC... |
| `wait` (a wait's entry) | merged into its `syscall` span; one that never returns: span event `<service> (entered, no return)` on its thread | as the stream has them |
| `fault` | span event `fault #PF page fault` (by vector) on the thread, else the process |
| `pipe write` / `pipe read` | a zero-length span on the thread that did it; a read links each write whose bytes it took (`data from`) | `gala.pipe.id`, `.name`, `.offset`, `.end`/`.to_end`, `.waited_ns` (from the first such write), `messaging.message.body.size` |
| `button` / `key` / `input taken` | a zero-length span on the thread that did it (a delivery: the boot, the desktop being the kernel's); a taking links its delivery (`input from`, FIFO per window and message: a window gets its input in order) and `otel: slow input` names the five longest waits | `gala.input.window`, `.message`, `.owner`, `.x`/`.y` or `.vk`, `.queued_ns` |
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
- The Linux personality records with the same call: `linux syscall
  missing` (a call it lacks, by number), `linux syscall stub` (accepted
  without its effect: ioctl, wait4's rusage), `linux
  option missing` (a flag, family, option or case not supported: open
  flags, socket families and options, CPU-time clocks, an unlink or
  rename an NT program's open refuses, process groups...), `linux clone flags missing` (clone flags;
  vfork's CLONE_VM, its memory being a copy). ARCHITECTURE "The lx
  module", "Observability", lists them with the `lx:` log lines (`lx:
  pid <p> execve <path>`, `lx: pid <p> vfork: child pid <c> ...`).

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
  (`spintest` spins the boot thread 25 s). A kernel thread with nothing
  more to do ends (`sched::exit`) rather than halting in a loop, which
  the watchdog would rightly count as time in the kernel: the boot
  thread ends after `gala: boot ok` when there is no `qemu_exit`, so an
  idle machine reports nothing.

### State dump

A read-only "what are you doing" with no channel of its own
(`kernel/src/statedump.rs`; cheap: nothing runs until asked). When a
boot test times out, xtask injects an NMI (QEMU monitor `nmi`, every
CPU) before killing the VM and waits up to 20 s for the dump:

1. From the NMI (only atomics and `try_lock`): each CPU's rip and kernel
   backtrace (`statedump: cpu N ...`, symbolized in the failed log;
   `user 32-bit` for a CPU interrupted in a WOW64 thread's 32-bit code), the
   thread each CPU runs, whether the scheduler, heap, frame and console
   locks are free (a lock held for the whole dump is the deadlock), heap
   and frame figures, the flight recorder.
2. From the boot thread (may take locks): every NT thread's start
   routine (`start 0x...`, its Win32 start address: which thread it is,
   a pool worker or the program's own; also in triage's thread lines)
   and its name if the program gave it one (` name "CrBrowserMain"`,
   SetThreadDescription; also in triage and, from the `thread name`
   event, in `cargo xtask profile`'s threads),
   system call and wait object (with `session N personality nt (name)`; for a WOW64
   thread also the 32-bit code that made the call, from its CPU area:
   `called from 32-bit code: eip=... esp=...`; for a 64-bit one the
   caller of the system call stub, the return address on top of its user
   stack: `called from 0x... (kernel32.dll+0x...)`, which names the lock
   or wait a hang sits in; for a keyed-event wait, what critical sections,
   SRW locks, condition variables and WaitOnAddress sleep on, the key, the
   value there and who is queued for it: `key 0x... (value there 0x...);
   queued for it: (waiters, releasers, all entries) Some((1, 0, 1))`, so a
   release waiting with no waiter, or a waiter whose value changed, shows),
   every
   running process's commit charge and pool and handle quotas
   (`process: pid P (x.exe) commit N KiB, R reservations, G regions; paged U/L (peak P) nonpaged
   U/L (peak P) handles N/L (peak P) refused R`, bytes), every
   scheduler thread (a waiting one with where its wait began:
   `sched: thread N (x) Waiting { until_tick: None } at
   kernel/src/lx/futex.rs:304`, the caller of `prepare_wait` through
   `#[track_caller]` wrappers; the only clue for a Linux thread, which
   has no NT system-call record), the ledger, each Linux thread (`lx: pid P thread
   T (core C): mask .. pending .. process pending .. blocked|running`),
   each epoll set's entries (`lx: pid P epoll fd N: fd ..`) and every
   futex waiter (`lx: futex waiter: pid P address A bitset B thread
   T`), every named AF_UNIX socket (`dump: unix: stream by pid P,
   node N|abstract NAME, listening (K of B waiting)|connected (side S:
   Q queued, shutdown F, peer open|gone), W waiting`), the Wayland
   server's clients and their objects (`statedump: wayland client N (pid
   P): K objects, R requests, B bytes of events waiting`, then a line per
   object: a surface's window, size and pending frame callbacks, a
   buffer's size and stride, an xdg_surface's configure state, a
   toplevel's title and decoration, a decoration's mode asked for; then
   its seat: the pointer's surface and place, buttons held, the
   keyboard's surface, keys down, modifiers), the statistics,
   each session's
   windows (`statedump: session N: ... windows ...`) and window stations
   (`statedump: session N window stations: WinSta0 ...`), its input
   queue (`statedump: session N input queue: K waiting, D dropped
   (full)`) and the last 16 mouse buttons and keys its desktop
   delivered, each with what became of it (`statedump: session N button
   0x201 at (x, y) to window W ... ms ago, taken after M ms`, `key
   0x100 vk 0x47 ...`, `not taken by its program`, `dropped (queue
   full)`: kept whether or not `events=window` is on, so input a
   program ignored shows where it went and whether the program took it,
   in a run whose timing a trace would change), then `statedump: end`. A machine too stuck for this still has part 1.

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
and `crash_filtered`. An exception in 32-bit code (a WOW64 process, CS
0x23) says `32-bit x86 (WOW64)` after the personality and shows eax..esp,
eip and the stack's 4-byte words (`[esp+0x..]`); scenario `wow64_crash`
(no 32-bit ntdll: the kernel ends the process) and `wow64_seh_crash`
(the 32-bit ntdll's dispatcher found no handler: wow64.dll passes its
record and the 32-bit registers on through GalaUnhandledException, and
the ledger has `ntdll.dll!unhandled exception` as for 64-bit code).
An exception in wow64.dll's own 64-bit code other than a fault on a
caller's invalid 32-bit pointer (which fails that call with
STATUS_ACCESS_VIOLATION) is a bug there: its dispatcher reports it
through GalaUnhandledException, so the report shows the 64-bit registers
at `wow64.dll+0x...`.

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

### Visualization: a live view of hot paths and transactions

The goal: watch a running gala (a QEMU boot, later real hardware) the
way state-of-the-art observability dashboards show a distributed
system: where the time goes (hot paths, impact = time x occurrences),
over time, and which "transactions" (causal chains across threads and
processes) are slow, with each step's owner and caller. Three layers,
standard tools first, gala's own views only where gala knows more:

1. **Live, on the standard stack** (ROADMAP OT6): a streaming export
   (xtask follows the debug console, or the kernel's per-CPU rings,
   and posts OTLP batches every second) into an OpenTelemetry
   Collector feeding Grafana's stack: Tempo (traces), Pyroscope
   (profiles), Loki (logs), Prometheus (metrics). `cargo xtask observe
   <scenario>` starts the stack (a compose file in `tools/observe/`,
   pinned images) and the boot, with provisioned dashboards:
   - **Overview**: processes and threads alive, CPU by process (the
     samples), system calls and waits per second, the ledger's new
     entries, crash reports, IPC throughput.
   - **System** (Task Manager's Performance tab, Process Explorer):
     per CPU its busy share split into user, kernel and interrupt
     time and what ran there (process and thread per CPU over time);
     per process CPU, commit charge and working set, threads, handles,
     I/O bytes, context switches; per thread its start address, state,
     times and a sampled stack. Exact numbers from the kernel's own
     accounting (metrics), not from samples. The same accounting feeds
     NtQuerySystemInformation (SystemProcessorPerformanceInformation,
     SystemProcessInformation) and ThreadTimes, so real Task Manager
     and Process Explorer show it inside gala too. Needs: kernel and
     user time per thread and idle, kernel, user and interrupt time per
     CPU, taken at context switch and at kernel entry and exit
     (ThreadTimes returns zeros for both today).

     samples carry whole stacks, user-mode ones their leaf until they
     are unwound), filterable by
     process, thread and mode, and a "from the trace" jump (each sample
     links its thread's span).
   - **Impact table**: per system call, wait object, function and IPC
     channel: count, total time (impact), p50/p95/max, and a sparkline
     of when it happened, sorted by impact (derived metrics).
   - **Service map**: processes as nodes, IPC as edges (pipes, sockets,
     completion ports) weighted by messages, bytes and hand-off
     latency (Tempo's service graph from the link spans).
2. **Deep timeline, Perfetto** (ROADMAP OT6): `cargo xtask perfetto
   <events.jsonl | scenario> [--out FILE]` writes a boot as a Perfetto
   trace (Chrome's JSON trace-event format; default
   `target/events/<scenario>.perfetto.json`): a track per process and thread
   with system calls and waits as slices (the wait's object named),
   CPU samples as on-CPU marks, and **flows** (arrows) for causality:
   process creation, pipe data (write to the read that took it),
   completion packets (post to dequeue). ui.perfetto.dev opens it
   offline; it answers "what exactly happened in these three seconds"
   with every thread side by side.
3. **gala's own analyses** (in xtask, shown in both): what only the
   kernel's view gives:
   - **Transactions**: a chain of work linked across threads and
     processes ("data from", "completion of", "created by", later trace
     context carried in the TEB): a child's Mojo ping, the browser IO
     thread's read, the UI thread's answer. Each step has its **owner**
     (process, thread, module/function from the samples) and its
     **caller** (the step before it). Each chain gets a
     **critical-path** breakdown: on CPU in X, queued in port Y, blocked
     in write Z, waiting for object W (OT1's "slow pipe data" and "slow
     completion" lines are its first form).
   - **Impact ranking** with a time axis: "time x occurrences" per key,
     and its histogram over the boot, so a burst and a steady cost are
     told apart.
   - **Gaps and failures in context**: the ledger's entries, crash
     reports and state dumps placed on the timeline and on the spans
     they happened in.

**Volume: what is sent, and how it is sampled.** Each signal keeps its
cost bounded in its own way, and every reduction is visible (the
observability rule: a dropped or sampled-out item is counted, never
silent):

- **Metrics are never sampled; they are aggregated.** Counters and
  histograms (per CPU, process, thread, system call, wait object, IPC
  channel) are kept in the kernel and exported once per interval: the
  cost is fixed whatever the load. A Task Manager view, call rates and
  latency percentiles need nothing else. High-frequency events
  (system calls, waits, pipe writes) become metrics first, and spans
  only when sampled.
- **Exemplars join the two**: each histogram bucket keeps a few
  sampled spans' ids (OTel exemplars), so a p99 bar jumps to a trace
  that was actually that slow.
- **Traces: one decision per transaction, made at its root and carried
  with it.** The decision travels in the trace context (W3C
  traceparent's sampled flag, the TEB slot, across process creation,
  pipes, ALPC and completion ports), so a chain is kept whole or not at
  all. Head sampling is a ratio per root kind (a system call from a
  given process, a network accept), with the adjusted count recorded
  (OTel's probability sampling in tracestate) so the backend can still
  scale counts.
- **Decide before the cost**: recording everything and choosing at the
  end (a collector's tail sampling) spends the most on what is thrown
  away: at millions of transactions a second most of the work would
  be discarded. A rule decides where the item is made, before it is
  formatted or buffered, one of three:
  - **drop**: counted only (a per-CPU counter and a latency histogram
    bucket: the metrics), nothing formatted, buffered or sent;
  - **keep**: recorded and exported;
  - **watch**: recorded provisionally and kept only if it turns out
    failed or slow.
  A single-span item (a system call, a wait, a fault) costs nothing to
  watch: the kernel already notes its start (accounting), and at its
  end status and duration are known where it ends, so "keep if failed
  or slower than X" is a comparison there, with nothing buffered; it
  can apply to every call. Only a multi-span transaction (a chain across
  threads and processes) has to be held until its root ends, so
  "watch" for a chain is itself a head decision with its own ratio,
  carried in the trace context, and only watched chains take ring
  space (a full ring drops and counts, as the profiler's does).
- **Failures are never sampled out.** Unhandled exceptions and crashes
  (with their crash report), a process ending with an error, the
  ledger's unsupported and missing calls, kernel errors and watchdog
  reports, and whatever gala itself finds wrong (`emit_always`, the
  clock check) bypass the rules and the byte budget: rare, and the
  thing one debugs. Error statuses are another matter: many are a
  program's normal control flow (NtOpenKey's OBJECT_NAME_NOT_FOUND for
  keys probed, thousands at Chromium's start; BUFFER_OVERFLOW from a
  size query; WSAEWOULDBLOCK; first-chance exceptions: guard pages, the
  CLR's and V8's own use of SEH). Every error status is counted (per
  call, status and process: a metric, never lost) and kept by default,
  except where a rule calls a (call, status) expected (count only), and
  repeats are limited: the first N of each (call, status, process) or
  (exception code, address) kept whole, the rest counted, as the ledger
  dedupes its entries. Every failure is thus visible: the real ones in
  full, the expected-looking ones as counts with their first examples.
- **Sampling rules**: what is sampled how is a list of rules, the first
  match deciding (a default rule last). A rule matches on the
  transaction's or span's **name** (a glob: `NtWriteFile`,
  `chrome.exe/net*`), its **type** (system call, wait, fault, file,
  IPC, network accept or connect, process start, profiler sample, log),
  the **process** (image name, command-line text, pid) and thread, and
  on what is only known at the end (status: failed; duration over a
  threshold). Its action, per signal: drop, keep or watch (above) with
  a ratio for traces (0 to 1), the watch condition (if failed, if
  slower than X), a profiler
  rate (samples per second for those threads, 0: none), and
  metrics-only (counted and timed, never a span). Examples: every
  failed system call kept; `NtWriteFile` of the console metrics-only;
  1% of `NtWaitFor*` unless slower than 100 ms; Chromium's network
  service at 1000 samples per second, everything else at 100. Each
  kept item carries the rule that kept it (`gala.sampling.rule`, with
  its ratio for the adjusted count) and each rule's matched, kept and
  dropped counts are metrics, so a view knows what it is not seeing.
  The rules are kernel state: set at boot (a rules file the command
  line names) and changed at run time (the `gala` CLI, OT5), evaluated
  where the event is made (the kernel for its spans and samples).
  `events_pid=`/`events_match=` (the system-call stream of chosen
  processes) and `sample_hz=` are the first, fixed forms of such rules.
- **Tail sampling at the source.** The kernel records every span into
  its per-CPU rings anyway (cheap, fixed size, like the flight
  recorder); what decides is the end of the root: a transaction that
  was slow (over a per-kind threshold), failed (an error status, a
  crash, a ledger entry) or was asked for is exported whole, the rest
  only counted. Always record, rarely send; a collector's tail
  sampler is the fallback for streams that leave the machine unsampled.
- **Profiles are aggregated**: per interval, each distinct stack and its
  count (OTLP profiles allow values without per-sample timestamps), so
  the volume follows the number of hot stacks, not `sample_hz`. The
  rate itself stays adjustable (`sample_hz=`, and at run time).
- **Logs**: by severity; a repeated line goes once with a count (as the
  ledger does).
- **A budget**: the exporter has bytes per second to spend. Over
  budget, it lowers head ratios and the profile rate (never the
  metrics), and says so: dropped and sampled-out counts per signal are
  themselves metrics, and the export says what ratio was in force.
- **Defaults: always on, cheap; deep on demand.** A running gala keeps,
  always: the metrics (aggregated, fixed cost: they say something is
  wrong and where), failures (above), the ledger, crash reports, the
  flight recorder (an in-memory ring written out only when something
  goes wrong), and the free watch of single calls (failed or slow).
  Traces (head ratios), watched chains and the profiler are off by
  default and raised at run time, narrowly (one process, one call type,
  for a while), through the rules (the `gala` CLI), when a metric or a
  failure points somewhere: no reboot, no permanent cost. Windows'
  ETW works the same way (a few always-on loggers, providers enabled
  for an investigation).
- Debugging a test boot stays unsampled (`events=`, everything to the
  file); sampling is for long-running systems and the live view.
- **Prometheus: pushed.** Prometheus' own model is pull (it scrapes,
  and a target that stops answering shows as down), but a test boot
  lives seconds (a scrape interval misses most of it) and a gala server
  may sit behind NAT or a firewall. gala pushes: OTLP metrics straight
  to Prometheus' OTLP receiver (Prometheus 3, `/api/v1/otlp/v1/metrics`;
  remote-write as the alternative), no collector or Pushgateway (that
  is for batch jobs) in between. Prometheus runs on the same host as
  the gala it watches, and this is for diagnosing symptoms, not for
  checking availability (a fleet's uptime is another tool's job): no
  heartbeat. A `/metrics` endpoint in the text format stays as an
  option for those who scrape.
- **Anomalies suggest an investigation** (later): the always-on
  histograms give each key (a call per process, a wait object, an IPC
  channel) a baseline (its p50 and p99 over a recent window). A duration
  far outside its own baseline (say 10 times its p99, or the whole
  distribution shifting) flags the key, and the live view or the `gala`
  CLI suggests tracing it ("NtReadFile in pid 1352 takes 40 times its
  usual time: trace it for 5 minutes?"); accepting raises that key's
  rule (keep, the profiler) for a limited time. Once trusted, it can
  switch itself on within a budget (rate, time, bytes), saying so in the
  stream: nothing changes silently.

Order: the Perfetto export first (done), then the live stack, then
transactions as their own view. The kernel side the live view needs
(per-CPU rings instead of the debug console, which costs a port write
per byte under TCG; user-mode stacks on samples) is OT2's.

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

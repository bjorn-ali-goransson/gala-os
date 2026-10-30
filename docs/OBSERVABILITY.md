# Observability: tracing, telemetry, profiling (plan)

gala-os is developed mostly by agents, which can only act on what they can
read. The goal is that every boot produces machine-readable answers to:
what did the program ask for, what did we not implement, where did the
time go, and what was the state when it broke. Status per item: **done**,
**next**, **later**.

## 1. Structured event stream — next

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

## 2. "What is missing" ledger — next

Every STATUS_NOT_IMPLEMENTED, STATUS_INVALID_SYSTEM_SERVICE, unknown
information class, unsupported flag or TODO path increments a counter keyed
by (service, detail). At process exit the kernel prints the ledger as
JSON; `cargo xtask report` merges it over all scenarios and real programs.
When running a real program (Go, Nginx), this list *is* the work queue,
ranked by how often it was hit.

## 3. Trace diffing against Windows — later

`cargo xtask diff-trace gala.jsonl windows.jsonl` aligns two syscall
traces of the same program and shows the first divergence (service,
arguments, status). The Windows side comes from a user-mode tracer (an
NtTrace-like tool, or ETW syscall events) run by `user/bin/run-all.ps1`.
The Windows results we collect are black-box behaviour, so this stays
clean-room.

## 4. Profiling — next (kernel), later (user)

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

## 5. Post-mortem — next

- Panics print a symbolized backtrace (frame-pointer walk in the kernel,
  symbolized by xtask from the serial log), the current thread/process,
  and the last N events from the event ring.
- `cargo xtask debug name` boots a scenario with QEMU's gdbstub (`-s -S`)
  and a gdb script, so an agent can get backtraces and inspect memory in
  batch mode.
- Flaky SMP bugs: QEMU record/replay (`-icount shift=auto,rr=record`)
  makes a failing run reproducible.

## 6. Live introspection — later

A command channel (second serial port or virtio-vsock) with read-only
queries: threads and their states/CPUs, a process's handle table and VADs,
object namespace, counters. Agents can ask a hung system what it is
waiting on instead of guessing.

## 6b. Screen — done

The kernel's screen console mirrors the serial log on the framebuffer, and
`cargo xtask screenshot` captures it as PNG at chosen times (docs/WORKFLOW.md).
Later: a status area (CPU load, threads, memory) drawn by the kernel.

## 7. Fuzzing — later

A user-mode fuzzer program calling services with random/mutated arguments
in CI (the kernel must never panic on user input), and `cargo fuzz` on the
host for pure parsers (PE, paths, PE resources, registry hives later).

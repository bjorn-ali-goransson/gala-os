# Working on gala-os (handbook for agents and humans)

How work is done: the test-first loop, the commands, the tools, program
recipes, debugging recipes and the pitfalls met so far. Read `CLAUDE.md`
first, then this, then the parts of `ARCHITECTURE.md` you touch.
`ROADMAP.md` says what is done and what comes next (its "Current focus" is
the order to work in). README.md is the short quick start; this is the
full reference.

Contents: [The loop](#the-loop) ·
[Building, running and testing](#building-running-and-testing) ·
[Writing tests and adding functions](#writing-tests-and-adding-functions) ·
[Program recipes](#program-recipes) · [Tools](#tools) ·
[Debugging and troubleshooting](#debugging-and-troubleshooting) ·
[Things that are easy to get wrong](#things-that-are-easy-to-get-wrong) ·
[Hygiene sweeps](#hygiene-sweeps) · [Where things are](#where-things-are)

## The loop

Every piece of work starts from a test program whose expected results
are those of real Windows (pinned build: Windows 11 24H2, build 26100),
or of real Linux for the Linux personality.

### One chunk of work

1. **Test first.** Create `user/<feature>_test/` (copy `Cargo.toml` from
   another test with `sed 's/file_test/<feature>_test/g'`). Its `main`
   returns 0 or the number of the failing check and prints
   `<feature>_test: ok`, or on a failure one line with the check's
   number, the value it got and its source line (`check N got X (line
   L)`: `#[track_caller]` on every method of its `Checks`, the line from
   `core::panic::Location::caller()`), so a report from any machine
   names the line without counting checks. Expected values are what real Windows returns;
   say in the doc comment what is assumed.
2. **Wire it up:**
   - `xtask/src/main.rs`: add to `USER_PROGRAMS`, add a `Scenario`
     (`cmdline: "qemu_exit run=<feature>_test.exe [args=...]"`), and, if
     it needs the test ntdll or DLLs, `EXTRA_MODULES` entries
     (`("x_test", "ntdll.dll", "Windows/System32")`).
   - Root `Cargo.toml` workspace members.
   - `.github/workflows/ci.yml`: append `-p <feature>_test` to the
     Windows-target clippy line; test DLLs go on the DLL build/clippy
     lines.
   - `user/bin/run-all.ps1` only if the program needs arguments on
     Windows.
   - A multi-threaded program also goes in `SIM_PROGRAMS`, and in
     `FAULT_PROGRAMS` once its fault sweep passes. A program that cannot
     run in gala-um goes in `NOT_IN_UM` with the reason.
3. `cargo xtask boot-test <feature>_test`: see it fail for the right
   reason.
4. Implement. Host unit tests for pure logic (`#[cfg(test)]` in the
   module).
5. Run the checks below; update the docs; commit with a message saying
   what the test checks and what changed; push.

### Before committing

All must pass:

```sh
cargo fmt --all
# every clippy/build line CI runs (selected by content, not line number)
grep -E '^ +cargo (clippy|build --release)' .github/workflows/ci.yml | sed 's/^ *//' | bash -e
cargo xtask test            # stub-audit, host tests, alloc-audit, gala-um, replay, every boot scenario
cargo xtask export-tests    # refresh user/bin (CI compares)
```

- CI (`.github/workflows/ci.yml`, GitHub's hosted runners; the
  repository is public) runs on every pull request and on main after a
  merge (a branch without a pull request gets no run; a newer push to a
  pull request cancels the older run). Work reaches main through a pull
  request once its checks are green, or by a direct push after the local
  gate (`cargo xtask test`) passed; main's ruleset allows both for now.
  Its jobs:
  - `linux` (Ubuntu 24.04): the format and clippy lines, `cargo xtask
    lx-expect && git diff --exit-code user/lx`, `cargo xtask test
    --no-boot` (everything but the boots) and `cargo xtask export-tests &&
    git diff --exit-code user/bin` (checked-in binaries and transcripts
    must match their sources).
  - `boot (i/6)`: every boot scenario, dealt out over six runners with
    `XTASK_SHARD=i/n` (`cargo xtask boot-test` boots the i-th of n
    round-robin shares of the planned VMs; a batch stays in one share,
    and the shard prints what it took). A failed shard keeps
    `target/failed/` (each failed boot's log and triage) as an artifact.
    Each shard (and the `linux` job, for what its host tests and
    gala-um use) keeps its pinned downloads (`target/prebuilt`, without
    Chromium's symbols) in the Actions cache between runs, keyed by
    `xtask/src/main.rs` with the shard's older caches restored: every
    download is checked against its pinned checksum at each use, so only
    what is missing is fetched (a release host's outage fails only a run
    that needs a new download).
    The runners have hardware virtualization: a step opens /dev/kvm to
    the runner's user, so the boots run under KVM (the boot log's
    `xtask: vm time` line, and a QEMU standard error without "falling
    back to tcg", show it). A step deletes the runner's preinstalled
    Android, .NET and GHC toolchains for disk space.
  - `windows` (Windows Server 2025, build 26100: the pinned build): the
    checked-in `user/bin` through `run-all.ps1`, elevated, with the
    `galatest` account and Defender excluded from the checkout, as the
    sweep brief asks. The report (`windows.json`,
    `windows-output.txt`) is the job's `windows-report` artifact, in
    `cargo xtask compare`'s schema; `user/windows/ci_summary.py` writes
    the job summary (passes, and each failing program's exit and line).
    With `user/windows/ci-passing.txt` (program names) the job fails
    when a listed program does not pass on Windows: a test's expectation
    stopped being Windows' answer. Each program that ended with an
    exception status is run again under `user/windows/fault.py --report`
    (a debugger: exception code, module+offset, address, the first-chance
    exceptions before it), into the summary and `faults.md` in the
    artifact; one that does not crash again says so.
- Commit messages pass through the shell: do not put backticks in a
  double-quoted `-m`; use a heredoc with `-F -`.
- Borrowed code or a new dependency gets a `THIRD_PARTY.md` entry in the
  same commit.

### Documents to update (same commit)

- `docs/ROADMAP.md`: move the item to done, with the test name.
- `docs/ARCHITECTURE.md`: how it works, invariants, TODOs.
- `docs/WORKFLOW.md`: a new convention, tool or pitfall.
- `docs/OBSERVABILITY.md`: new logs, events or failure reports.

### Rules for every change

- `cargo xtask lint` passes (the pre-commit hook in `hooks/` runs it;
  any xtask run enables the hook with `core.hooksPath`): the analyzers
  of `xtask/src/lint.rs` and `cargo fmt --check`. A finding names its
  file, line and analyzer; fix the code, or for `event-reset` justify
  the site with `// lint(event-reset): <reason>`, or for `wait-site`
  add the reviewed site to `WAIT_SITES` with the review's reason.
- Before a push the pre-push hook runs `cargo xtask lint --full`: the
  same, then every command of CI's Clippy step, read from
  `.github/workflows/ci.yml` (add a crate's line there and the hook
  runs it too). It names the commands that failed; fix them rather than
  pushing with `--no-verify`.

- **Observability is part of every change** (OBSERVABILITY.md): a new
  subsystem, driver, boot stage, format or test comes with its
  logs/events and what a failure leaves behind; when a failure's cause
  had to be guessed, add the missing telemetry with the fix.
- **Fallible allocation only** (`try_box`, `try_push`, `try_reserve`,
  `mm::heap::try_to_vec`); clippy bans the rest (`collect` and `to_vec`
  included) and `cargo xtask alloc-audit` finds what clippy cannot see
  ("Fault injection"). `unsafe` only where `CLAUDE.md` allows it, each
  block with a `SAFETY:` comment.
- **Unsupported paths are recorded**: when you leave a flag, class or
  case unimplemented, call `nt::ledger::record(service, what, detail)`
  there (in DLLs: `unsupported(b"dll!Name(case)", detail)`), so real
  programs that need it show up in `target/report/ledger.md`.
- **Constants come from headers**: values typed in by hand go wrong
  silently (a wrong STATUS_OBJECT_NAME_EXISTS once matched a test that
  agreed with it). With mingw-w64 installed (`apt install
  mingw-w64-common`, or the cross compiler), `cargo test -p xtask`
  compares every NTSTATUS in `kernel/src/nt/status.rs` with
  `ntstatus.h`; check other constants the same way when you add them.
- **System call numbers** live in `kernel/src/nt/syscalls.rs` and
  `user/ntrt/src/lib.rs` and must match ("Test programs").
- **Architecture-specific code only under `kernel/src/arch`**
  (ARCHITECTURE "The architecture seam"): elsewhere use its neutral
  accessors (`UserRegs::ip`, `SyscallFrame::number`, `TrapFrame::kind`,
  `cpu::cycles`, `arch::timer`, ...); what is Windows' or Linux's ABI on
  x86-64 goes in `nt/abi/x64/` or `lx/machine/x86_64.rs`, with a stub in
  their aarch64 counterpart. The aarch64 check enforces it: `cargo clippy
  -p kernel --target aarch64-unknown-none -- -D warnings` (CI; `rustup
  target add aarch64-unknown-none` if the toolchain lacks it). A new
  arch function used outside `arch` needs its aarch64 stub (it may end
  in `unported`).
- **shasha features: the `gala` command first.** Everything the
  graphical shell does can be done from a terminal. Make a new feature a
  named action (`nt::desktop::Action`, or a new GalaDesktopAction
  request in `nt/action.rs` and `user/gala`), test it through `gala` as
  `gala_cli_test` does (start the program with `gala start`, wait on
  `gala windows`, check what the program saw), and only then bind the
  mouse or a key to the same action. A `gala` command is also the easy
  way to put a screen test's window where its pixel checks expect it.

## Building, running and testing

### Commands

`cargo xtask <command> [--release]`; `--release` builds an optimized
kernel (user programs are always release builds). Running `cargo xtask`
alone prints the usage.

| Command | What it does |
|---|---|
| `build` | kernel, DLLs, programs, `target/<profile>/gala-os.iso` |
| `run [--headless]` | build, then boot in QEMU (UEFI) with the serial log on stdio |
| `test` | `stub-audit`, host unit tests, `alloc-audit`, `um-test`, `replay-test`, then every boot scenario not in `PENDING` |
| `boot-test [filter]` | boot scenarios only; a filter matches names containing it, `name$` exactly that one, `a,b,c` exactly those |
| `export-tests` | copy the built test programs and DLLs to `user/bin/`, then remake derived test data |
| `iso [--name N] [--scenario S] [--file PATH[=DIR]]... [cmdline...]` | a bootable image ("Bootable images and USB sticks") |
| `screenshot [--uefi] [--at 1,3,5] [--name N] [--scenario S] [cmdline...]` | PNGs of the screen ("Screenshots") |
| `serve <scenario> [port]` | boot a network scenario and keep it running ("Serving to a browser", "SSH into a running gala") |
| `um-test` | the program scenarios that `runs_in_um`, in gala-um |
| `fault-test [filter] [--sweep[=frame\|net_tx]]` | gala-um under injected failures ("Fault injection") |
| `sim-test [filter] [--seeds N] [--seed S] [--all]` | gala-um's deterministic simulation ("Deterministic simulation") |
| `replay-test` | record programs' inputs in gala-um and replay them, live and simulated |
| `lx-expect [--record] [--no-build] [--runs N]` | the Linux test programs on this host against their transcripts |
| `alloc-audit [kernel ELF] [--all]` | infallible allocations in the kernel's machine code |
| `stub-audit` | the 32-bit ntdll's stubs agree with every DLL's declaration of them and with wow64.dll's table (argument sizes, words) |
| `symbolize <log> [--release]` | resolve kernel backtraces in a serial log |
| `triage <serial log> [events.jsonl]` | the triage block of a log ("When a test fails") |
| `events <events.jsonl>` | summarize an event stream |
| `otel <log\|events.jsonl\|scenario> [events.jsonl] [--out P] [--endpoint URL]` | OTLP/JSON ("OpenTelemetry") |
| `profile <events.jsonl\|scenario> [--pid N] [--tid N] [--from S --to S] [--top N] [--symbols DIR] [--release]` | the sampling profiler's report (OBSERVABILITY `sample`) |
| `perfetto <events.jsonl\|scenario> [--out FILE]` | a Perfetto trace for ui.perfetto.dev (OBSERVABILITY "Visualization") |
| `bench [bench-windows.txt]` | `user/bench` on gala ("Benchmarks") |
| `report` | every test program on gala -> `target/report/gala.{json,md}` |
| `compare <windows.json> [gala.json]` | against `run-all.ps1`'s report from Windows |
| `docker-test [xtask args]` | run `cargo xtask test` (or the args) in `docker/test.Dockerfile` |
| `qemu` | build the pinned QEMU ("QEMU") |
| `bake-fonts`, `bake-icons` | rasterize the desktop's fonts and icons ("Screenshots") |

To compare with Windows: copy `user/bin` to a Windows 11 machine, run
`powershell -ExecutionPolicy Bypass -File run-all.ps1 -Out windows.json`
there, then `cargo xtask compare windows.json` here. Before the run
there: a local account `galatest` with password `Gala-Test-1` (access,
disk, logon, lsa, profile, session and system tests log on as it) and
an administrator prompt for `schannel_test` (its test CA goes into
LocalMachine\Root; it is skipped otherwise, and so are `winhttp_test`
and `wininet_test` and their WOW64 twins, which fetch from the Nginx
that setup starts).
A program that crashes gets a `fault` field
(`<code> <name> at module+offset`) from `user/windows/fault.py`, which
the runner looks for at `..\windowsault.py`: it re-runs the program as
its debugger, because Windows Error Reporting's event 1000 is never
written for these -- the runner sets `SEM_NOGPFAULTERRORBOX`, and a
process with that error mode does not report its fault to WER at all.
Map the offset to a function with the build's PDB of that module. A
program whose non-zero exit is the result rather than a failure (`crash`,
`go_exit`, `busybox`, and gala's own `cmd`, `conhost` and `gala`, which
real Windows cannot load) carries an `expected` field beside `exit`. `cargo xtask
report` also lists programs that pass in QEMU but not in gala-um, or the
other way round (one of the two platforms is then wrong).

`user/bin` alone is not the whole sweep. The seven script scenarios
(`busybox_files`, `busybox_script`, `tcc`, `tcc_make`, `tcc_self`, `lua`,
`sqlite`) run `busybox.exe sh ..\<dir>\<script>.sh`, so they also need
`user/busybox`, `user/tcc`, `user/lua`, `user/sqlite` and
`user/msvcrt_test` beside it. run-all.ps1 reports a scenario whose
script or `busybox.exe` is missing as skipped with that reason (a round
that copied only `user/bin` has seven such skips). Copy `user/` (or
those five directories) and check the count.

Run it from an ordinary directory, never one under `%TEMP%`: Windows
treats a temporary path differently, so a test that works from its
current directory reads that instead of Windows. shell32_test's check
174 is what caught it: `SHAddToRecentDocs(SHARD_PATHW, <a file in the
current directory>)` creates the Recent `.lnk` within about 200 ms for
an ordinary path and never for one under `%TEMP%`, so the check also
spent its full ten-second wait.

Antivirus is what goes wrong there: the test binaries are unsigned and
freshly built, which Microsoft Defender's machine-learning heuristic
takes for malware (`bench.exe` as `Trojan:Win32/Bearfoos.A!ml`). A
program Windows refuses to start, and one already quarantined out of the
directory, are both reported as `skipped` with the reason (for a
quarantined one the threat name, read from Defender's own detection
history) and the run goes on, so one such program costs its own result
and no other; `cargo xtask compare` shows them as missing on the Windows
side, never as a check that passed or failed. An administrator's
`Add-MpPreference -ExclusionPath <user/bin>` brings them back, for as
long as that exclusion stands.

Windows' firewall prompts for two programs, `ipv6_test` and
`handover_net_test`, the first time each one listens: the dialog creates
a `TCP Query User{<guid>}<path>` and a `UDP Query User{...}` rule **keyed
on the absolute path**, so a sweep from a new directory asks again (an
old clone's rules do not carry over). Answer Allow. Answering No writes
a *Block* rule instead, which is permanent and would fail those two
silently in every later round -- check with `Get-NetFirewallRule -Action
Block | Get-NetFirewallApplicationFilter` if either starts failing. The
permission is not actually needed: both are in CI's passing baseline
(`user/windows/ci-passing.txt`), and CI's runner is headless with nobody
to answer the prompt, so they talk over loopback, which the firewall
does not filter. The prompt only costs the time it sits unanswered.

**Probing a divergence from a script, rather than from the test
program, has two traps that silently answer a different question.** A
probe host brings its own environment:

- **comctl32's version.** A process with a v6 manifest (python.exe has
  one) loads 6.10 from WinSxS; the test programs have no manifest and
  get 5.82, which behaves differently -- it rejects `TOOLINFOW` with
  `cbSize` 72 where 6.10 accepts it. Print the module's path before
  trusting a comctl32 result: `GetModuleFileNameW(GetModuleHandleW(
  L"comctl32.dll"))` names the WinSxS directory, version and all.
- **UI Automation in-process or out.** A client in another process sees
  these controls as `ControlType.Pane` and does not see the non-client
  title bar at all (three children where the test sees four); the same
  query from inside the process that owns the window gives Button, Edit,
  Text and a TitleBar. uia_test queries in-process, so only an
  in-process client answers its checks (Python can, with `comtypes`).

Where the environment is the question, instrument the test program
instead: build it from the same sources, add prints, run it, revert.
That is how the MXCSR, static-TLS and comctl32 answers were measured.

Every program's whole output goes to `windows-output.txt` beside the
report, with a header per program. The report keeps one line, the
failing check; triaging often needs what the program printed before it
(which call it had reached, the values it had found), and this saves
asking for one program at a time.

Each result carries `ms`, how long the program ran, on both sides: on
Windows the runner's stopwatch around the process (printed per
program), on gala the process's lifetime from creation to exit (the
kernel's `process ... exited with status 0x... after N ms` line, which
`cargo xtask report` reads). Both leave out what is not the program: a
gala boot's firmware, bootloader and kernel start are xtask's summary's
own lines. `cargo xtask compare` prints both times per program, gala's
as a multiple of Windows', and the totals over the programs timed on
both: how much slower the same tests run under QEMU (TCG without KVM)
than on the Windows machine. `cargo xtask bench` against
`bench-windows.txt` stays the operation-by-operation comparison
("Benchmarks", docs/PERF.md).

A program that leaves a child of its own running holds the pipe its
standard output goes through, and the end of that output never comes:
quota_test creates its children suspended and leaks them when a check
fails before it resumes them, which costs nothing under gala (the
machine goes away with the scenario) and strands them on a real one.
The per-program timeout does not cover this, since the program itself
exited; the runner bounds the wait for the output too, kills what the
program left running and goes on, so that program still reaches the
report with its exit status. A program whose output is lost that way
says so in place of its report line.

Requirements (README "Requirements"): Linux, rustup (the pinned
toolchain in `rust-toolchain.toml` installs itself), `git`, `xorriso`,
QEMU, OVMF for UEFI boots (`OVMF_CODE`, `OVMF_VARS` if it is somewhere
unusual), Linux 5.11+ for gala-um. The first build fetches the pinned
Limine into `target/`. The `handover_next` kernel is built into
`target/next`.

### Environment variables

| Variable | Effect |
|---|---|
| `XTASK_VERBOSE=1` | print the serial log while booting |
| `XTASK_EVENTS=all` or `syscall,image,thread,process,fault,window,display,...` | write the event stream to `target/<profile>/<iso>.events.jsonl` |
| `XTASK_CMDLINE="words"` | add kernel command-line words to every boot (the kernel takes the last `args=`, so this can replace a scenario's arguments) |
| `XTASK_QEMU_ARGS="..."` | extra QEMU arguments; an `-accel` here replaces xtask's own |
| `XTASK_QEMU_LOG=int,cpu_reset` | QEMU's `-d` log into `<image>.qemu.log` |
| `XTASK_QEMU=<path>` | the QEMU binary to use |
| `XTASK_FIRMWARE=uefi\|bios` | force one firmware for every boot |
| `XTASK_NO_BATCH=1` | every scenario in its own VM |
| `XTASK_REPEAT=n` | boot one scenario's image n times ("Flaky tests and repeats") |
| `XTASK_REPEAT_JOBS=n` | VMs at once for repeats (default one per two host CPUs) |
| `XTASK_TIMEOUT=<s>` | every scenario's time limit (a hang's state dump without the wait) |
| `XTASK_RECORD=1` | record every boot's outside inputs ("Clock control, record/replay") |
| `XTASK_OTEL=1` | write OTLP files for every boot |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | where `otel` and `XTASK_OTEL=1` post |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/protobuf` (default) or `http/json` for what is posted |
| `OTEL_EXPORTER_OTLP_COMPRESSION` | `gzip` (default) or `none` for what is posted |
| `XTASK_UNSIGNED=1` | unsigned kernels and images ("Signed images") |
| `XTASK_DOCKER_CA` (or `SSL_CERT_FILE`) | a CA for `docker-test` to trust (behind a proxy) |
| `GALAPKG_BLESS=1` | regenerate galapkg's winget golden file ("Installing software") |

Examples: `XTASK_QEMU_ARGS="-smp 1"` runs on one processor,
`XTASK_QEMU_ARGS="-cpu Nehalem"` without AVX/BMI/XSAVE;
`XTASK_CMDLINE="checkuser tlbparanoid"` adds diagnostic flags
(`checkuser`, `tlbparanoid`, `nomigrate`, `framecheck`, `eagercommit`;
ARCHITECTURE "Kernel command line").

### Scenarios

A boot scenario (`SCENARIOS` in xtask) is a kernel command line, the
boot modules, the expected exit status and the lines its serial log must
contain. Expected lines are only searched for, so a program that prints
several similar steps labels each line with its step. An expected line
never pins a measured number (a duration's leading digit): the program
checks its own bounds (handover_test bounds a sleep by the measured
pause) and the scenario expects only its label. The lists in
xtask that change how a scenario boots:

| List | Effect |
|---|---|
| `USER_PROGRAMS`, `USER_DLLS`, `GO_PROGRAMS` | what is built and exported |
| `EXTRA_MODULES` | extra files for a scenario: `(scenario, file, directory on C:)`; repository paths and `target/prebuilt/...` paths work too |
| `MODULE_SETS` | a named set of modules shared by scenarios (and so a VM); a scenario listed twice gets the union |
| `WRITABLE_TREES` | directory modules programs may write to (`sql/data`, `fzs`, `ftp`) |
| `PENDING` | still being brought up: `boot-test <name>` runs it, `test` and CI do not |
| `SLOW_SCENARIOS` | 300 s instead of `BOOT_TEST_TIMEOUT` (120 s) |
| `UEFI_SCENARIOS` | boot by UEFI (the rest by BIOS) |
| `NET_SCENARIOS` | a network card and a host-side client ("Network tests") |
| `HOST_PAGE_SCENARIOS` | a network card whose 10.0.2.100:80 is a page served from the host (`xtask serve-page`; "Network tests") |
| `DISK_SCENARIOS` | a virtio-blk disk ("Disk tests") |
| `GPU_SCENARIOS` | a virtio-gpu screen ("GPU tests") |
| `SCREEN_SCENARIOS` | pixel checks through the QEMU monitor ("Screenshots") |
| `SINGLE_THREAD_TCG` | `-accel tcg,thread=single` without KVM ("Known intermittent") |
| `HOST_LOAD_SCENARIOS` | run with a busy thread on every host CPU |
| `NOT_IN_UM` | not run by `um-test`, with the reason |
| `SIM_PROGRAMS`, `FAULT_PROGRAMS`, `FAULT_RULES` | sim-test and fault-test |
| `trust::SCENARIO_SIGNING`, `EARLIER_SIGNING` | images signed differently ("Signed images") |

- **Time limits**: 120 s by default; `SLOW_SCENARIOS` (JVMs, .NET,
  PowerShell, FlaUI, PostgreSQL, FileZilla) 300 s; elasticsearch,
  java_stress and sqllocaldb* 900 s; dump scenarios 25 s. A
  `SINGLE_THREAD_TCG` scenario's limit doubles without KVM. Every limit
  counts the VM's *run time*, not wall time (`xtask/src/vmtime.rs`): the
  wall time less the run-queue wait of its most-starved vCPU thread
  (`/proc/<pid>/task/<tid>/schedstat`). A VM starved by a loaded host is
  not killed, and a hung one is caught as before (halted vCPUs sleep, so
  its run time is the wall time); a wall ceiling of 4x the limit
  remains. The guest's own clock cannot do this: under TCG it follows
  the host's even when starved (ARCHITECTURE "What a guest's clock
  follows"). Every log has `xtask: vm time: <run> run of <wall> wall
  (<w> waiting for a host CPU)`, the triage repeats it, and the state
  dump's wait after a timeout counts run time too.
- **A scenario that runs many processes** (a database, a runtime) can
  pass alone and time out under the full suite (2-3x slower); its client
  then reports a dropped connection, not a timeout. Look for "timed out
  after" first; such scenarios go in `SLOW_SCENARIOS`.
- **Memory**: QEMU gets 512 MiB, plus twice the image size when the
  image is over 64 MiB (boot modules stay in memory); elasticsearch,
  sqllocaldb and postgres get 3 GiB (`qemu()` in xtask).
- **Servers**: a server never exits; scenarios with exit status
  `XTASK_STOPS` are stopped by xtask once their net client has finished,
  and pass if it succeeded.
- **A server and a client at once**: `run=` starts programs one after
  another, so the client starts the server itself (CreateProcessW) and
  waits for it, as `tds_test` does with sqlservr; or the scenario starts
  it with `start=` (`schannel_test`).
- **A program that ends while its children work on** needs the boot
  option `wait=all`.
- Code that derives a scenario from an image's name must go through
  `iso_scenario`, which knows the `.r<i>` of a repeat's copy.

Logs: `target/<profile>/test-<name>.serial.log` (shared VMs:
`target/debug/batch-<first program>.serial.log`), the event stream next
to it, a failed boot's copy with its triage in
`target/failed/<scenario>.log` (overwritten by the next failure of the
same scenario: after a pass it is stale). Timing per scenario (or VM) and
a summary go to `target/report/timing.txt`; outcomes to
`target/report/history.jsonl`.

### Network tests

VMs have no network card unless the scenario is in `NET_SCENARIOS`
or `HOST_PAGE_SCENARIOS` (`-nic none` otherwise).

- A host-page scenario's guest fetches `http://10.0.2.100/`: QEMU runs
  `xtask serve-page` (the running xtask's own binary) per connection,
  which answers with `HOST_PAGE`. Nothing is leased; its requests show
  as `xtask: serve-page: "GET / ..."` in the log. To serve something
  else, change `HOST_PAGE` (user/hostnet_test checks it byte for byte).

- A net scenario gets a virtio-net card on QEMU's user network with a
  host port forwarded to guest port 7 (TCP and UDP, the same port number
  on both; sshd's 22 for `SSH_PORT_22`). User networking needs no
  privileges, so this works in containers and CI.
- Each VM leases its own host port (`PortLease`: free for both
  protocols, below the host's ephemeral range so no outgoing connection
  takes it before QEMU binds it, never given to another VM of the same
  xtask while held); a QEMU that still cannot bind it is booted again
  with another. The failed log says which port the VM had (`xtask: net:
  host 127.0.0.1:<port> forwarded to ...`) and has QEMU's standard
  error. Net scenarios therefore run in parallel (the suite,
  `XTASK_REPEAT_JOBS`).
- xtask plays the remote side (`NetClient`: echo, HTTP, Nginx, TLS with
  the host's `curl -k`, ssh with key or password, sshd server, an
  interactive shell with `ssh -tt`, sftp, psql, ...) once the guest logs
  that it listens; the verdict is appended to the serial log as an
  `xtask: net client: ...` line for `expect` to check. The client's own
  files (ssh's key copy and `-v` trace, sftp's transfer, the askpass
  script) are per port in `target/net-client/<port>/`, removed after the
  boot. On failure the end of curl's or ssh's `-v` trace is in the log.
- A guest that only connects out (`office_install` fetching from
  Microsoft's CDN) is a net scenario with `NetClient::Outbound`: the card
  and QEMU's NAT to the host's network, no client run.
- A UDP client resends until it gets its answer (the guest's first reply
  may wait for ARP) and must skip late duplicates of earlier answers.
- QEMU's DNS (10.0.2.3) forwards to the host's resolver: a check that
  needs it accepts "no DNS" when the host has none (`udp_echo` resolves
  example.com only if the host can).
- Servers start late: a program may log that it started before it
  listens (Nginx's master logs, then its worker opens the socket). The
  HTTP client retries for 10 s while it gets no response at all.

### Disk tests

VMs have no disk unless the scenario is in `DISK_SCENARIOS` (name, size,
what xtask writes into the fresh raw image, what the image must hold
afterwards).

- The image is `target/.../<scenario>.disk.img` next to the ISO,
  attached as a legacy virtio-blk device; such scenarios boot alone and
  not in gala-um.
- Raw disk access needs an administrator, so their programs run
  `as=system`.
- `earlier_boots` boots the same image first (each with the lines its
  log must have; logs kept as `<iso>.boot<n>.serial.log`): that is how a
  test checks what survives a reboot (`persist_test`, which tells the
  boots apart by what it finds on C:).
- `persist` keeps C: on the disk; without it a disk is left alone (the
  raw disk tests).
- A slow disk: `failpoint=disk_io:delay:<ms>` (with `persist`).

### GPU tests

A scenario in `GPU_SCENARIOS` boots with `-vga none -device
virtio-gpu-pci,xres=1280,yres=800,edid=on` (UEFI) and its command line
says `display=virtio`; QEMU's `screendump` then shows the virtio-gpu
scanout, so the usual `SCREEN_SCENARIOS` checks work. The device is
modern-only (no `disable-legacy=off` as for virtio-blk/net): its driver
uses `arch::virtio::ModernDevice`. With such a display the boot splash
is not visible (the firmware's framebuffer is no longer shown): read the
serial log. The kernel flag `gputrace` logs every command; `events=display`
the modes, resets and errors.

Software 3D (GPU G2) needs no device: its scenarios run Mesa (PREBUILT
`mesa`) on the CPU and check a headless desktop's frame
(`presenter=test`, BIOS). `gl_test` (user/gl_test, C, `build.sh`) is
OpenGL on llvmpipe, `vk_test` (user/vk_test, C; its `build.sh` needs
`glslangValidator`, apt `glslang-tools`, and clones the pinned
Vulkan-Headers) Vulkan on lavapipe through the Khronos loader (PREBUILT
`vulkan-runtime`), about 10 s each under TCG. When one fails:

- Read the ledger first: `missing export` stops a load; `missing dll`
  for d3d12/vulkan-1 is Mesa looking for drivers, D3DKMT* and
  IDXGIFactory6 the Vulkan loader probing (not failures).
- A crash report inside `vulkan-1.dll`: `llvm-symbolizer
  --obj=target/prebuilt/vulkan-runtime/VulkanRT-X64-1.4.341.0-Components/x64/vulkan-1.dll
  0x180000000+<offset>` names the loader's source line (its PDB is
  unpacked beside it; the loader's source is
  github.com/KhronosGroup/Vulkan-Loader at the same version).
- vk_test printing `skipped (no Vulkan driver)` means the loader found
  no driver: the Drivers key (the scenario's `registry` module), the
  manifest and `vulkan_lvp.dll` in System32, and `dxgi.dll` (the loader
  takes no driver from the key without a DXGI factory).
- Mesa's own `MESA:` lines are in the serial log.
- A frame that renders (the read-back pixels are right) but is not on
  the screen is user32's presentation of a kept DC (ARCHITECTURE
  "Software 3D (G2)").

### Test speed and environments

- **BIOS boot by default.** ISOs are hybrid (BIOS and UEFI). SeaBIOS
  reaches the kernel in about 0.9 s against about 4.5 s for OVMF; the
  `UEFI_SCENARIOS` (hello, selftest, fault-pagefault, the handover
  `_uefi` ones, the screen ones, ...) keep the UEFI path tested.
- **Shared VMs.** Scenarios that just run one program with no other
  flags, expect success, boot by BIOS and expect no process ids and no
  `desktop: session ...` line (logged once per session and boot, so it
  lands in whichever program's section came first) are grouped by identical boot modules and run in shared VMs
  (`run=a.exe,b.exe`). Each program is judged on its own section of the
  log (between `run: begin` and `run: end`, plus the boot preamble);
  scenarios running the same program (two `dotnet.exe` runs) are judged
  on that program's n-th section, in group order. `XTASK_NO_BATCH=1`
  gives every scenario its own VM; `boot-test` with one match always
  boots it alone.
- **Boot tests pass `noscreen`** (serial only) except in the UEFI
  scenarios: drawing text roughly doubles kernel time under emulation.
- **KVM**: QEMU uses KVM when `/dev/kvm` is available (near-native
  speed); otherwise TCG with one host thread per processor (multi-threaded
  TCG), except `SINGLE_THREAD_TCG` scenarios. TCG executes the
  instructions the Intel way whatever vendor the guest's CPUID names, so
  behaviour that differs between Intel and AMD only shows under KVM on
  that vendor's host (CI's runners are AMD: an AMD-only bug is CI-only
  here). The boot log's `cpu:` line names the vendor.
- **Docker**: `cargo xtask docker-test [xtask args]` builds
  `docker/test.Dockerfile` (Ubuntu 24.04, QEMU, OVMF, xorriso, the
  pinned toolchain) and runs `cargo xtask test` (or the given arguments)
  inside it with the repository at /work and build output in
  `target/docker`. It passes `/dev/kvm` through when the host has it and
  forwards the `XTASK_*` variables and proxy settings. In a cloud
  container without a running daemon, start one with `dockerd &`.
- **Never run two xtask boot runs at once** (a full `cargo xtask test`
  and a single `boot-test`): they share ISO and serial-log files under
  `target/` and fail each other.
- **Fresh checkouts**: CI starts without `target/`; anything fetched or
  built on first use (Limine, prebuilt programs) must also work when
  several threads need it at once.

### QEMU

xtask runs a pinned QEMU (`QEMU_VERSION` in xtask, 11.1.3), built by
`cargo xtask qemu` into `~/.cache/gala-qemu/<version>` (downloaded from
download.qemu.org, SHA-256 checked, only `x86_64-softmmu` with user
networking).

- `XTASK_QEMU=<path>` overrides it; without either, xtask uses
  `qemu-system-x86_64` from PATH and says so once. Every run prints which
  QEMU it uses.
- Build dependencies (Ubuntu): `libglib2.0-dev libpixman-1-dev
  libslirp-dev flex bison ninja-build python3-venv libfdt-dev zlib1g-dev
  pkg-config bzip2`. CI builds it once and caches it. `python3-venv` is
  what provides `ensurepip`, which configure's own venv needs, and
  `bzip2` is needed although nothing unpacks a `.bz2` here. Configure
  does report "Compiler for language cpp ... not found" and "Program
  iasl found: NO" on a machine without `g++` or `acpica-tools` and
  carries on regardless, so those two are not required; it fails on a
  missing dependency rather than warning, one at a time, so read the
  last line of its output rather than the first.
- Why not the distribution's: QEMU 8.1.0-8.2.5 (Ubuntu 24.04 ships
  8.2.2) keeps running stale translations of code rewritten on a
  translated block's second page, which corrupted JIT-compiled code
  ("Known intermittent"). `straddle_test` (single-threaded TCG) rewrites
  code across a page boundary and runs it: it passes on the pinned QEMU
  and fails on 8.2.2 (check 2, round 1 gets the pre-rewrite value), so
  it is the quick way to tell whether a QEMU build has the bug. On the
  pinned QEMU `java_stress` passed 15 of 15 boots; on 8.2.2, 2 of 6
  failed. A newer distribution QEMU is no safer: on 10.2.1 (Ubuntu
  26.04's) the `hello` scenario wedges **under KVM** before the kernel
  logs a phase, three runs of three timing out at 120 s with CPU 1 at
  the reset vector (`EIP=0000fff0 CS=f000`, `HLT=1`) and no state dump,
  while `lx_hello`, `lx_hello_pie` and `go_hello` passed in the same
  runs; on the pinned 11.1.3 `hello` boots under KVM in 9-16 s. So a
  distribution QEMU can produce what looks exactly like a kernel that
  never starts, and the first question about any such failure is which
  QEMU ran.
- QEMU TCG (8.x) has no x2APIC: boot tests exercise only the xAPIC path;
  the x2APIC path runs only under KVM.
- A cloud container is ephemeral: run `cargo xtask qemu` in the
  environment's setup script, or once per new container (about 10
  minutes).
- In manual QEMU runs use `-cpu max`: QEMU's default CPU model has no
  RDRAND, and Go programs then fail fast (0xc0000409, ledger:
  `ProcessPrng(no RDRAND)`).

### gala-um

gala-um runs the real NT layer (the kernel crate with `--features
hosted`) as a Linux process (docs/GALA_UM.md). `cargo xtask test` runs
`um-test` right after the unit tests: every program scenario that
`runs_in_um` (xtask), checked against the scenario's `expect` lines;
about 50 programs in about 3 s. Run it alone with `cargo xtask um-test`
while changing `kernel/src/nt`: it is the fastest check there is. A new
test program runs in gala-um by default; if it cannot (child processes,
a network card, a disk), add it to `NOT_IN_UM` with the reason. A
scenario with a kernel flag gala-um does not apply (anything not in
`UM_FLAGS`: `start=`, `as=`, `events=`, ...) is left out by rule, and
`um_command` refuses it; um-test's last lines list every program
scenario it did not run and why (`um_gap`).

- **A "hang" in gala-um is first a question of setup**: read the
  SIGQUIT state dump and `um --trace` before naming a cause. A program
  run without the server its scenario starts retries its connect
  (STATUS_CONNECTION_REFUSED from AFD's connect IOCTL 0x12007, in a
  loop in the trace) until um's 30 s timeout, which looks like a hang
  in the sockets.

Run a program directly:

    target/debug/um [--trace] [--cpus=2] \
        --module=Windows/System32/ntdll.dll=user/bin/testntdll.dll \
        user/bin/<name>.exe [arguments]

- `--module=<path on C:>=<host file>` adds a file to C: as a boot module
  would (DLLs under their `test*` names in user/bin); `--dir=<dir>` sets
  the program's directory on C:; `--trace` logs every system call;
  `--cpus` caps the processors (um-test uses 2, as QEMU);
  `--failpoint=`, `--failseed=` and `--failtrace` inject failures;
  `--quota=` sets the pool and handle limits (the kernel's `quota=`);
  `--sim=<seed>` runs deterministically; `--record=<f>` and
  `--replay=<f>` record and replay outside inputs.
- The kernel's log goes to stderr, the program's output to stdout.
- Program code must stay below 0x5000_0000_0000 (above is the host's,
  where `syscall` is not caught). Needs Linux 5.11+ (syscall user
  dispatch).
- gala-um re-executes itself without ASLR under the name `exe`: `gdb -p`
  that process (`handle SIGSYS nostop noprint`, `handle SIGURG nostop
  noprint`), and `thread apply all bt` shows where NT threads are inside
  the NT layer.
- A run that hangs gets SIGQUIT at its timeout (`timeout -s QUIT`,
  SIGKILL 5 s later) and prints the threads part of the state dump to
  stderr; for a live one, `kill -QUIT <pid of exe>`.
- A failure that reproduces in gala-um is in the NT layer or the
  user-mode DLLs; one that does not points at the kernel's machine code
  (paging, switches, interrupts) or at the emulator.
- A builtin or in-process program step can be tried in milliseconds
  (`um ... user/bin/busybox.exe 'sh -c "..."'`: pass the command line
  pre-quoted, as one argument); anything that starts a child needs QEMU.

#### Running the JDK in gala-um

`um --dir=jdk/bin` puts the program in C:\jdk\bin (the JDK finds its
home from its own path). Pass every file of
`target/prebuilt/jdk/jdk-21.0.5+11/` as `--module=jdk/<path>=<host
file>`, the scenario's DLLs from user/bin as
`--module=Windows/System32/<name>=user/bin/<exported_as>` and
`--module=gala/Hello.java=user/java/Hello.java`, then
`um --cpus=2 --dir=jdk/bin <modules> .../bin/java.exe C:/gala/Hello.java`:
javac and the program in about 3.5 s, so hundreds of runs take minutes.

### Disk space

ISOs of the runtime scenarios are 120-470 MB each, about 50 MB for the
rest.

- `cargo xtask test` makes each VM's ISO just before it boots and
  deletes it after a pass (a failed one stays to boot by hand), so only
  the running VMs' images use disk. xtask deletes each `*-iso-root` once
  its ISO is made. `XTASK_REPEAT` still makes its images first.
- xtask refuses to start an image with less than 300 MiB free
  (`MIN_FREE_MIB`), or to pack an ISO without its tree's size free, and
  says what to delete: images (`target/*/*.iso`, kept for failed boots),
  `target/debug/incremental`, and other worktrees' `target/*/test-*` and
  `*.iso`. The session's disk allowance is shared with any agents'
  worktrees (`.claude/worktrees/*/target`); `du -sh target/*
  .claude/worktrees/*/target` shows who uses it.
- An agent's worktree whose `target` is a symlink to the main
  checkout's (to save disk) shares `target/debug/xtask` with it: whichever
  checkout built last owns it, and `cargo xtask` may run the other's
  sources (paths under `.claude/worktrees/...` in its output, or
  `export-tests` changing nothing after a test changed). Build and copy
  the binary (`touch xtask/src/main.rs && cargo build -p xtask && cp
  target/debug/xtask <scratch>/`) and run that copy while an agent works.
- Low on disk, a few user programs can be built alone into a scratch
  target directory with xtask's flags (`cargo build --release --target
  x86_64-pc-windows-msvc --config 'target.x86_64-pc-windows-msvc.rustflags=
  ["--remap-path-prefix=<cargo home>=/cargo"]' -p gala -p ...`); the
  output is byte-identical to `export-tests`'s when no other program of
  the batch shares a dependency with different features (`cargo tree
  --workspace -i <crate>` tells).

### Bootable images and USB sticks

`cargo xtask iso [--name N] [--scenario S] [--file PATH[=DIR]]... [cmdline...]`
writes `target/<profile>/gala-<N>.iso` (N defaults to `demo`) with every
program and DLL, a scenario's files and command line (without
`qemu_exit`), host files as `C:\DIR\<name>` (DIR defaults to `gala`,
also the programs' current directory), and the given command line, or
the demo (`run=hello.exe,go_hello.exe,go_goroutines.exe`). It prints the
QEMU and `dd` commands. The ISO is hybrid, so `dd` to a USB stick boots
on BIOS and UEFI (README.md has the steps); checked in QEMU as `-drive
file=...,format=raw` under SeaBIOS and OVMF and as `usb-storage` behind
`qemu-xhci`, not yet on physical PCs.

To try a new program: `cargo xtask iso --file path/to/app.exe
run=app.exe "args=..."` and boot it with QEMU (`-cpu max`), then follow
the ledger and the program's own error messages.

### Signed images

Every image xtask makes is sealed (R8; ARCHITECTURE "Chain of trust";
code `xtask/src/trust.rs`).

- **The test key** is made on first use in `target/trust/`
  (`test-signing.seed`, the private Ed25519 seed, mode 0600, never
  committed; `test-signing.pub`). Every kernel xtask builds trusts it
  (`GALA_TRUST_KEY` = its public key in hex, read at compile time).
  Deleting `target/trust/` makes a new key: the next build recompiles
  the kernels with it. A kernel from another checkout refuses this
  checkout's manifests: `manifest: ... refused: signed by key X, this
  kernel trusts Y`.
- **The manifest** `/boot/gala.manifest` lists every file in `/boot`
  (size, SHA-256), signed as release 1 with floor 1. A scenario that
  needs another release or a file left out goes in
  `trust::SCENARIO_SIGNING` (or `EARLIER_SIGNING` for the earlier boots
  of a disk scenario).
- **limine.conf** has `#<BLAKE2B>` after every path and its own hash
  enrolled in the Limine executables: editing it on a USB stick (the
  command line) makes Limine refuse to boot (`CHECKSUM MISMATCH FOR
  CONFIG FILE`); make a new image with `cargo xtask iso ... <cmdline>`
  instead. A module changed after sealing: `PANIC: Blake2b hash for URI
  ... does not match!` (on the serial port only with `serial: yes`,
  which the boot-chain scenarios set). Under UEFI Limine writes each
  character with a cursor sequence: for `boot_config_edited` xtask
  strips them and stops the VM at Limine's "Press a key to return to
  menu" / "System halted.".
- **An unsigned build** (a developer's own kernel, a quick experiment):
  `XTASK_UNSIGNED=1 cargo xtask ...` builds the kernels without a key and
  makes images without manifest, hashes or enrolment; the kernel logs
  `manifest: unsigned build (no release key built in): nothing is
  checked`, hands over to any image and never raises a disk's rollback
  floor. The handover and rollback scenarios expect a signed build.
  Switching between the two rebuilds the kernel crate (the key is a
  compile-time input).
- **The kernel on an image is stripped** of debug information (xtask's
  `copy_kernel`); `cargo xtask symbolize` uses the build's ELF in
  `target/` (symbols and the GALA note are kept). Limine hashes every
  file it loads (about 4 s more per BIOS boot under TCG with a 30 MB
  debug kernel) and the kernel SHA-256s the next image before a
  handover. A handover that comes seconds late (its program finished
  before) is the SHA-256 of a large image: check the image's kernel
  size.
- **A disk that refuses the build** (`rollback: refused: release 1 is
  below the rollback floor 2`) was used by a newer release; make a fresh
  one (the floor lives in sector 0 at byte 64).

### The update feed

`cargo xtask feed` writes and checks the update feed (R8; ARCHITECTURE
"The update feed"; `xtask/src/feed.rs` over `user/tuf`):

- `cargo xtask feed publish <files dir> <feed dir> [--release N]
  [--floor F]`: a release of every file under `<files dir>` (paths
  relative to it). The first publish writes `1.root.json`; the release
  number defaults to the last one plus 1, the floor to the last floor;
  a release or floor below the last is refused. Blobs already in the
  feed are kept (and must hash to their name).
- `cargo xtask feed refresh <feed dir>`: new snapshot and timestamp
  over the same targets (what a scheduled CI job runs, more often than
  the timestamp's 1-day expiry), and targets signed again (same
  release, new version) when it expires within 15 days.
- `cargo xtask feed renew-root <feed dir>`: root N+1, the same keys, a
  fresh 90-day expiry (CI runs it monthly).
- `cargo xtask feed check <feed> [--over <source>] [--root <trusted
  root.json>]`: the feed as gala's client sees it now, every blob
  hashed; prints the versions and release, or the first refusal
  (`timestamp.json: refused: timestamp: expired at ...`). `<feed>` is a
  directory or an http(s) URL (read with curl); `--over` reads what the
  directory lacks from a second source (a work directory over the
  bucket: the feed as it will be once pushed).
- `cargo xtask feed pull <source> <empty dir>`: the metadata a client
  follows, verified, into a new directory (no blobs), with `.pulled`
  listing what came; `cargo xtask feed push <dir> <s3://bucket | dir>`:
  what changed since, blobs first and `timestamp.json` last, refusing to
  overwrite a published version. `s3://` runs the `aws` CLI with its
  usual environment (`AWS_ENDPOINT_URL`, credentials).
- `cargo xtask release-tree <dir> [--release]`: the files of a release
  (kernel, system DLLs, Limine, sealed config, manifest; no test
  programs), what CI publishes.
- Keys: `GALA_FEED_KEYS` (one `<role> <64 hex>` line each for `root-1`,
  `root-2`, `root-3`, `targets`, `snapshot`, `timestamp`) when set;
  otherwise test keys made on first use in `target/trust/feed-root-{1,2,3}`,
  `feed-targets`, `feed-snapshot`, `feed-timestamp` (`.seed`, mode 0600;
  `.pub`). A new `target/trust/` means a new root: start a new feed
  directory. Signing a feed whose root lists other keys is refused.
- **CI** (`.github/workflows/release.yml`, environment `release`):
  `publish` after CI passes on main (main's head only), `refresh` every
  6 hours, `renew-root` on the 3rd of each month; each pulls from the
  public URL (`FEED_URL`, the one place it is written), signs, checks,
  pushes to `s3://$R2_BUCKET` through R2's S3 endpoint, and checks the
  public URL. The job summary lists the release, files, new blobs and
  each role's expiry. A manual run: Actions, "Release", "Run workflow",
  choose publish (from main), refresh or renew-root.
- **One-time setup** (the owner): make the keys and store them as the
  secret without them touching the screen or a file: `cargo xtask feed
  keygen | gh secret set GALA_FEED_KEYS --env release` (the key ids go
  to standard error: keep them to recognise the keys later). Besides
  it, the environment `release` (or the repository) needs the secrets
  `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and the variables
  `R2_ACCOUNT_ID`, `R2_BUCKET`. The first publish against the empty
  bucket makes root 1. A lost secret means a new root 1 for a new
  bucket (clients trust root 1 or a later root they took): keep a copy
  offline. GitHub disables the schedules of a public repository without
  activity for 60 days (the feed then freezes within a day): re-enable
  the workflow on the Actions page.
- Locally, against a directory standing in for the bucket:
  `GALA_FEED_KEYS="$(cargo xtask feed keygen)"` exported, then `feed
  pull bucket w1`, `feed publish <release tree> w1`, `feed check w1
  --over bucket`, `feed push w1 bucket`, `feed check bucket`; the host
  test `feed::tests::workflows_against_a_local_bucket` runs that
  sequence (publish, refresh, renew-root, publish).
- Cross-check with python-tuf (optional, host only): `python3 -m venv
  <dir> && <dir>/bin/pip install tuf==6.0.0`, then load each role with
  `Metadata.from_bytes`, `root.verify_delegate(role, md)` and
  `meta.verify_length_and_hashes(bytes)`.

## Writing tests and adding functions

### Test programs

Test programs are freestanding Windows executables on `user/ntrt` (or
kernel32 through `raw-dylib`), run under gala and under Windows
(`user/bin/run-all.ps1`).

- **System call numbers.** Long-stable x64 numbers are used directly
  (list in `kernel/src/nt/syscalls.rs`). Numbers that change between
  builds are gala's own choices, unverified: tests must call those
  services through ntdll exports (`find_ntdll()` + `find_export()` in
  ntrt), and `user/testntdll` provides stubs with gala's numbers
  (`syscall_stubs!`). On Windows the same test then uses the real ntdll.
  ntrt does so itself where it wraps such a service (`create_timer`,
  `set_timer`, `create_semaphore`, `query_semaphore` on x64, through
  `ntdll_export`; on x86 `nt_call` takes the 32-bit ntdll's export for
  the numbers that are gala's own), falling back to gala's number only
  when no ntdll is mapped (gala scenarios that ship none). A raw
  `mov eax, N` in a test is for long-stable numbers only.
- **A test must hold on an ordinary user's machine too** (run-all.ps1
  runs as a non-admin user in session 1, with many CPUs, a battery, its
  own UI language): files and directories go in `%TEMP%` (GetTempPathW),
  not C:\ (a user may not create files there) or C:\gala, and an
  earlier run's leftovers are removed first; values that depend on the
  machine (the processor count, devices, the system locale, the build,
  the volume's number) are read from another API, not typed in; per-user
  state another process may hold (the clipboard) is skipped, naming the
  holder, when it is held (a test that changes the clipboard also
  retries, for a moment, an OpenClipboard meant to succeed: clipboard
  history and remote-desktop agents open it after each change,
  `clipboard_test`); named sections and symbolic links go in the session's
  `\Sessions\<n>\BaseNamedObjects` (`\BaseNamedObjects` in session 0):
  creating them in the global one needs SeCreateGlobalPrivilege outside
  session 0, and DIRECTORY_ALL_ACCESS on it is not granted (ask
  TRAVERSE | CREATE_OBJECT); information buffers are aligned for their
  class (NtSetInformationThread checks alignment before length:
  STATUS_DATATYPE_MISALIGNMENT for a `[u8; 4]` at an odd address); and
  `cmd /c` with more than two quotes strips the first and the last, so
  such a line is wrapped in one more pair.
- **A test that needs what an ordinary user lacks** (the `galatest`
  account, an administrator, SYSTEM) checks for it with a real query
  (LookupAccountNameW's ERROR_NONE_MAPPED, TokenElevation, TokenUser,
  CheckTokenMembership) and without it prints one line `<test>: skipped
  (<reason>)` and exits 0; never a guess from an error alone. gala's
  scenario provides it (`as=system`, the account), so there the test runs
  whole. What only gala has (its Gala* services, gala.exe, `C:\gala`)
  runs only when `ntrt::on_gala()` (kernel32 tests ask GetProcAddress for
  ntdll's GalaConsoleCreate): on Windows gala's private service numbers
  have bit 12 set, reach win32k's table, and the first call faults
  (STATUS_FATAL_USER_CALLBACK_EXCEPTION), so a status check comes too late.
- **testntdll is gala's stand-in ntdll**, shipped only in scenarios that
  list it in `EXTRA_MODULES`: a small loader (LdrLoadDll and friends),
  the exception/APC dispatchers and the stubs. Adding an export there
  changes `user/bin/testntdll.dll`.
- **Write each expected output line with one WriteFile call**: the
  program's output and the kernel's log share the serial port, and a
  kernel log line ("process ... exited") can land between the pieces of
  a line written in parts, so the expected substring is missing. In
  shell scripts, `echo` a captured value in one piece.
- **Bad-argument checks are those Windows survives**: a test calls an
  API with a NULL handle or NULL out-pointer only as Wine's conformance
  tests do (they run on Windows and note the calls that crash there);
  others may fault inside the system DLLs (Windows' service functions
  faulted in ntdll on NULL handles Wine never passes them, `svc_test`).
  A test of many sections prints `<name>: <section>` before each, so a
  crash on Windows names the section: run-all.ps1 keeps the last
  `<name>: ` line of a crashed program as well.
- **Ask for exactly the access Windows would grant**: kernel32's
  CreateFileW adds SYNCHRONIZE for synchronous handles (open overlapped
  to avoid it); an object's owner only gets READ_CONTROL and WRITE_DAC
  implicitly; deleting a key needs DELETE on it; FILE_GENERIC_WRITE
  includes SYNCHRONIZE and READ_CONTROL (a deny ACE with FW denies
  reading too). To find a denial: gala-um's `--trace` (or
  `events=syscall`) shows the call that returned 0xc0000022.
- **A process at Low integrity** (uipi_test, gala_cli_test) cannot
  signal or open the parent's named events (no write up to a Medium
  object's default label): pass inherited handles instead.
- **Asynchronous Windows APIs**: SuspendThread returns before a thread
  running on another CPU has stopped, on Windows as on gala; a test that
  needs it stopped calls GetThreadContext first.
- **Handle reuse**: a handle value freed by NtClose is handed out again
  at once (the lowest free slot), so "use after close" tests may hit a
  new object of another type (ws2_32 maps "not a socket" device requests
  to WSAENOTSOCK).
- **Terminating a thread**: let it reach its own code first; ended
  during its start-up it holds ntdll's loader lock, and the next
  LoadLibrary (or DLL notification) spins in NtYieldExecution for ever.
- **Timing checks measure the host as much as the guest**: under TCG a
  loaded host stops the VM's processors for milliseconds at a time while
  the guest's clocks go on, so "this took under 2 s" fails. Check what the
  guest did instead (smpsched_test counts its threads' ContextSwitches);
  keep elapsed-time bounds as generous give-ups only. A sleep that spans a
  kernel handover is bounded by the pause the kernel measured
  (GalaHandoverPause: 200-350 ms under TCG, seconds on a loaded host;
  ARCHITECTURE "Programs across a handover"), never by a guessed constant.
  To reproduce a timing failure, run QEMU on one host CPU:
  `XTASK_QEMU=<script>` with a script that runs `taskset -c 3
  ~/.cache/gala-qemu/<version>/bin/qemu-system-x86_64 "$@"`, plus busy
  loops pinned to that CPU (`taskset -c 3 sh -c 'while :; do :; done'`).
  The same holds for KUSER_SHARED_DATA's clocks (a snapshot written at
  clock interrupts, which come late under load: compare them with a
  counter read on each side of a fresh snapshot, as clockcal_test does)
  and for screen checks: a program must not end or move on after a
  guessed time while xtask looks; it waits for xtask's input
  (console_window's "q"), and a step whose input acts on output the
  program is still printing waits for that output (`ScreenStep::after`).
  A program that ends on xtask's input ends on the last thing xtask
  sends: a click counts on WM_LBUTTONUP, not on the press, or under KVM
  the program and QEMU are gone before xtask writes the release
  ("screen: Broken pipe"; presenter_test, gl_test and vk_test count the
  release). On the host side, timing xtask's own observations in a loop that does
  other work is the same mistake: xtask times `uptime_ms=` marks in a
  thread of its own.
- **Single-CPU tests that measure ticks** pin themselves
  (`sched::set_affinity`).
- **Cross-processor races**: a test program that makes the two sides
  meet often (threads pinned to processors 0 and 1, kept in step:
  `smpcow_test`, `smpsem_test`, `smprace_test`), repeated with
  `XTASK_REPEAT`. Cross-CPU memory tests need the victim thread pinned
  to another CPU and nothing else flushing meanwhile (let reaping settle
  first, as `smpmem_test` does), or the bug is masked.
- **Vector registers**: a test that looks at YMM registers must declare
  the xmm registers clobbered (Rust has no ymm operands without the avx
  target feature) and check only what Windows keeps across a system call
  (the upper halves, and ymm6-15 whole: xmm0-5 are volatile).
- **Hover without a screen**: under gala the cursor is at (0, 0) and
  GetCursorPos follows moves over the program's own windows only; tests
  that hover put their window under GetCursorPos's point, so the same
  check holds on Windows (comctl32_test).
- **Windows in another session**: a GUI test run with `as=system` is in
  session 0: on a machine with a screen its windows are kept with their
  pixels but nothing shows them and they get no input (`desktop: session
  0 is live (1280x800), shown by none (the console shows session 1)`), so
  screen checks need the boot user (session 1), or the program attaches a
  Test presenter to its session, reads the frame back and brings it input
  (GalaSession's SESSION_FRAME and SESSION_INPUT, `presenter_test`), or
  moves the console there (`gala attach <session>` as SYSTEM). On a BIOS boot without `presenter=` every
  session's desktop is bare (`desktop: session 0 keeps its windows
  (1280x800), shown by none (no screen and no presenter=)`): windows
  work, nothing can show them, no Test frame can be attached. On real Windows a child
  started in another session needs STARTUPINFO.lpDesktop
  `winsta0\default` and a token that may open it (SYSTEM's, moved with
  SetTokenInformation(TokenSessionId), as session_test does), or user32
  fails to initialize.
- **Window stations depend on the account**: a program run with
  `as=system` is on `Service-0x0-3e7$\Default` (not visible: OpenSSL
  calls that a service), the boot user's on `WinSta0\Default`; a child
  inherits its parent's unless STARTUPINFO.lpDesktop names one, and a
  name its session lacks leaves it with none (`winsta:` warning; on
  Windows user32 fails to start). On Windows such a test runs as SYSTEM
  through `psexec -s` (winsta_test's `service` argument).
- **Window station and desktop handles block a kernel handover**: once a
  program asked user32 for its window station or thread desktop it
  holds handles a handover refuses ("a window station or desktop
  handle"); handover tests keep away from those calls.
- **A resource** (a manifest: `manifest_test`): the test's build.rs
  writes a .res file (the documented compiled resource format) and
  passes it to the linker; lld-link converts it itself (its
  /MANIFEST:EMBED would need Microsoft's mt.exe).

### Adding a function to gala's ntdll or kernel32

1. A test program that imports it (`raw-dylib` against `kernel32` or
   `ntdll`, as `kernel32_test` does), with the results real Windows
   gives, last-error codes included. Its scenario lists the DLLs in
   `EXTRA_MODULES` (`ntdll.dll` and `kernel32.dll` to
   `Windows/System32`).
2. In `user/kernel32/src/lib.rs`: `#[unsafe(no_mangle)] pub extern
   "system" fn Name(...)` with the documented signature. Call ntdll's
   exports (add them to the `raw-dylib` block); set the last error
   through `fail(status, value)` / `fail_with(error, value)`; report
   unsupported cases with `unsupported(b"kernel32.dll!Name(case)",
   detail)`.
3. A new system call goes into testntdll's `syscall_stubs!` and the
   kernel's table, with a stable x64 number or gala's own.
4. A new NTSTATUS the kernel returns needs a row in testntdll's
   `STATUS_TO_ERROR` (unmapped ones give 317 and are reported).
5. Run the program you are bringing up and read the `ledger:` lines:
   they list every missing import by name.
6. Which DLL: what Windows' kernel32 exports goes in kernel32; what only
   kernelbase has (its documentation says "DLL: Kernelbase.dll", Wine's
   kernel32.spec lacks it, or GetProcAddress on Windows says so: add it
   to `export_split.rs`) goes in `user/kernelbase/src/` (`lib.rs`,
   `path.rs`, `address_wait.rs`), named in its build.rs's `OWN`. A name
   Windows' kernelbase has that Wine's spec lacks goes in
   `tools/kernelbase_exports.py`'s MEASURED. Programs reach either through the core
   API sets: kernelbase forwards every name of Windows' list
   (`user/kernelbase/exports.txt`) to the DLL that has it, so a new
   kernel32 function is reachable through its set with nothing more.
   kernelbase ships wherever kernel32 does (xtask's `extra_modules`):
   scenarios need not list it. `exports.txt` is regenerated with
   `python3 -I tools/kernelbase_exports.py <dir of Wine's .spec files>
   <commit>` (record the commit in THIRD_PARTY.md).

### Programs that import functions gala lacks

- A missing export stops the load (as on Windows), so add it to the
  DLL's `src/unimplemented.rs` (`gala_stubs::unimplemented_exports!`)
  to get the program running. The stub logs at once (`[warn ]
  <program>: <dll>!<function> not implemented, ending the process`),
  records `dll!function` in the ledger and ends the process with
  STATUS_NOT_IMPLEMENTED when called. Implementing one means removing it
  from that list.
- GetProcAddress returns NULL for stubs, so a test that checks a
  function is *exported* imports it statically (a call in a branch never
  taken).
- An export forwarder (`/EXPORT:f=kernel32.f`) to a stub fails to bind
  as a "missing export": implement the function where it is forwarded
  from instead (normaliz's IdnToUnicode).
- List a program's missing imports by comparing its import tables
  (`llvm-readobj --coff-imports`, or `x86_64-w64-mingw32-objdump -p
  app.exe | grep "DLL Name"` for a first survey) with the
  `--coff-exports` of `user/bin/test*.dll`, ordinals included (an
  export with RVA 0 is an empty ordinal slot, not an export). For a
  program with many DLLs, walk the whole import closure (imports and
  delay imports of every DLL it loads, following export forwarders such
  as the api-ms-win-* sets) before booting, and fix the gaps in batches.
  DLLs loaded at run time (LoadLibrary: the JDK's launcher loads msvcp140
  and jvm.dll) are not in the closure: boot and read the ledger
  (`"what":"missing dll"`, `missing export`).
- A DLL Windows names other than `.dll` (`winspool.drv`): its
  USER_DLLS entry's `file` is that name; cargo builds `<package>.dll`
  and xtask's `built_dll` ships it under the right name.
- Exports by ordinal (ws2_32's) come from a build.rs printing
  `/EXPORT:name,@n`; lld warns that rustc's .def names them too and
  keeps the ordinal one (the warning cannot be silenced).
- A program importing advapi32 needs `bcryptprimitives.dll` in its
  scenario's modules too (advapi32 imports it), or it ends with
  0xc0000135 (the ledger names the missing DLL).
- Registry keys a program needs: boot it and read the ledger's
  `"what":"absent key"` lines (`registry:MACHINE/...`, the end of long
  names). Keys a clean Windows has go in `kernel/src/nt/registry.seed`
  with Windows' values; keys a clean Windows lacks too (policies, other
  products) stay absent.

### Tests in C, C++ and C#

- **DLL tests in C** (`user/<dll>_test/main.c`, mingw; oleaut32_test,
  shlwapi_test, ole32_test, shell32_test, comdlg32_test, dbghelp_test,
  msvcrt_test; the 32-bit wow64_crt_test, "32-bit test programs"):
  `build.sh` rebuilds the checked-in `user/bin/<name>.exe` (xtask does
  not; run it after editing main.c and commit the binary).
  `user/msvcrt_test` builds against msvcrt's own stdio, and its main.c
  must also build with TinyCC's headers (the `tcc_self` scenario).
- **C++ exception tests**: `user/cxx_eh_test` is C++ built with clang
  for x86_64-pc-windows-msvc and lld-link (`build.sh`, import libraries
  from .def files with llvm-dlltool), no CRT, against Microsoft's
  vcruntime140 (the JDK's, app-local); the binary is checked in and the
  build is reproducible. `throw.cpp` is `cxx_throw.exe` (a throw nothing
  catches: scenario `cxx_throw` checks the crash report's decoding of it;
  run-all.ps1 expects its 0xe06d7363). Pitfall: Ubuntu 24.04's clang
  18.1.3 does not finish main.cpp at -O1 with -fasync-exceptions (it
  loops); `build.sh throw` builds cxx_throw.exe alone.
- **Managed (.NET) tests** are C# in `user/<name>/` built by its
  `build.sh` with Mono's `mcs` (apt package `mono-mcs`) into `user/bin`
  (reproducible: the script pins the MVID and timestamp). CI does not
  rebuild them, as for the other third-party-tool builds. FlaUI tests:
  "FlaUI".

### 32-bit test programs (WOW64)

32-bit (x86) programs test WOW64 (ARCHITECTURE "Plan: WOW64"): no_std
Rust built for `i686-pc-windows-msvc` (in `rust-toolchain.toml`; linked
as the x64 ones, `.cargo/config.toml`) on ntrt's x86 runtime
(`user/ntrt/src/x86.rs`: the 32-bit stubs, TEB32/PEB32 access, print,
exit, memory, a few NT services; any service through its generic stub
`ntrt::syscall(nr::NT_..., &[args])` with the x64 numbers in `nr`). On
Windows they run from user/bin through Windows' own WOW64 (gala's own
numbers, 0x7f.., fail there).

- List the program in `USER_PROGRAMS_X86` (not USER_PROGRAMS: xtask
  builds that list for the i686 target) and add `-p <name>` to CI's
  i686 clippy line (`cargo clippy --release --target
  i686-pc-windows-msvc -p ntrt -p ...`); it cannot build for x64.
- Its scenario ships gala's wow64.dll: `("<scenario>", "wow64.dll",
  "Windows/System32")` in EXTRA_MODULES (any scenario whose program
  starts a 32-bit child too). Without it the process is refused (`wow64:
  ... there is no ...wow64.dll`).
- gala-um cannot run 32-bit code: such scenarios go in NOT_IN_UM.
- A 32-bit program may import from gala's 32-bit ntdll and from 32-bit
  DLLs of its own (`wow64_import_test`): its scenario then ships
  `("<scenario>", "x86/ntdll.dll", "Windows/SysWOW64")` too (the `x86/`
  prefix names a USER_DLLS_X86 DLL: built for i686, under `boot/x86`
  on the ISO). Without it the kernel refuses a program with imports
  (`wow64: <name> imports DLLs, and there is no ...`); with it, every
  32-bit program starts through it (its exceptions and APCs need it).
  One on kernel32 ships `("<scenario>", "x86/kernel32.dll",
  "Windows/SysWOW64")` as well (`wow64_k32_test`). Check imports with
  `llvm-readobj --coff-imports`.
- Imports on x86 are `#[link(name = "...", kind = "raw-dylib",
  import_name_type = "undecorated")]` (without it a stdcall import is
  named `_Name@N`, which no DLL exports; that holds for a dependency's
  imports too, e.g. `vendor/getrandom`'s ProcessPrng: check a new
  32-bit DLL with `llvm-readobj --coff-imports` for `@`). Keep data imports (`static`)
  in an `extern "C"` block: one in an `extern "system"` raw-dylib block
  crashes rustc 1.97 ("Ty::fn_sig() called on non-fn type"). The
  memory routines the compiler calls (`memcpy`) do not come from a
  raw-dylib import: take ntrt's x86 runtime as a dependency (`use ntrt as
  _;`) even when the program calls only kernel32 (a 32-bit DLL too: the
  32-bit kernel32 takes `memcpy`, MSVC's 64-bit division helpers,
  `__chkstk` and `strlen` from it; its no_mangle ones become extra
  exports).
- A cdecl (`extern "C"`) import on x86 with `import_name_type =
  "undecorated"` loses one leading underscore (rustc takes it for the C
  decoration: `__acrt_iob_func` is imported as `_acrt_iob_func`): give
  such names one more with `link_name` (`ucrt_test`'s `ucrt_imports!`
  macro adds `_` to every name; `malloc` stays `malloc`). stdcall
  imports are not affected.
- One source for an x64 and an x86 test program: the x86 package's
  `[[bin]] path` points at the other's `main.rs` (`wow64_ucrt_test`
  builds `user/ucrt_test/src/main.rs`); one source for two DLLs: the
  second package's `[lib] path` and `build` point at the first's
  (`user/ucrtbase` is msvcrt's source with `cfg(ucrt)`).
- `raw-dylib` blocks of a crate built for both: `import_name_type` is
  an error on x64, so they carry `#[cfg_attr(target_arch = "x86",
  link(name = "...", kind = "raw-dylib", import_name_type =
  "undecorated"))]` and `#[cfg_attr(not(target_arch = "x86"), link(name
  = "...", kind = "raw-dylib"))]` (kernel32, gala_stubs).
- A structure with pointer-sized fields (handles, SIZE_Ts, pointers)
  is a `repr(C)` struct with `usize` fields, its offsets `offset_of!`,
  never byte offsets of the x64 layout; a query of an information class
  with such fields passes the struct's size (wow64.dll answers only the
  x86 length). TEB and PEB fields: the arch module's named offsets
  (`teb_read(TEB_LAST_ERROR)`), never `gs:[0x68]`.
- A 32-bit C program: `user/wow64_c_test` (main.c, `build.sh`: clang
  for i686-pc-windows-msvc, an import library from a .def file with the
  stdcall names `Name@N` and `llvm-dlltool -m i386 --kill-at`, lld-link
  without a CRT); the binary is checked in (`user/bin/wow64_c_test.exe`,
  shipped from there by its scenario); rerun `build.sh` after editing
  main.c.
- A 32-bit C program on msvcrt: `user/wow64_crt_test` (main.c,
  `build.sh`: mingw-w64's i686 GCC, apt package
  `gcc-mingw-w64-i686-win32`, with mingw's own startup code and
  `-D__USE_MINGW_ANSI_STDIO=0` for msvcrt's stdio); its scenario ships
  `x86/msvcrt.dll` to SysWOW64 too. mingw's i686 headers define
  `_USE_32BIT_TIME_T` (time_t is 32 bits and `time()` imports msvcrt's
  `time`), make `setjmp` `_setjmp3(buf, NULL)`, reach stdin/stdout
  through the `_iob` data import, and link mingw's own `_fpreset`
  (fninit: the x87 at 64-bit precision, `_controlfp(0, 0)` 0x8001f, not
  msvcrt's 0x9001f): a test of msvcrt's `_fpreset` calls it through
  GetProcAddress.
- One C source for an x64 test and its x86 twin: `build.sh` compiles
  main.c with both mingw compilers, the twin with
  `-DNAME='"wow64_<name>"'` for its output lines (`user/wininet_test`:
  `wininet_test.exe` and `wow64_wininet_test.exe`); both binaries are
  checked in, and the twin's scenario ships the 32-bit DLLs it loads
  (for one on the network, Schannel's chain: ws2_32, secur32, schannel,
  crypt32, ncrypt, bcrypt, advapi32, bcryptprimitives).
- A 32-bit DLL (USER_DLLS_X86, exported as `test<name>32.dll` or its
  own `<name>32.dll`): its exports keep their undecorated names
  (`#[unsafe(no_mangle)] extern "system"`, naked functions too); symbols
  of `global_asm!` need `/EXPORT:` in its build.rs. testntdll is one
  crate for both ntdlls: what differs by architecture goes in its
  `x64.rs` and `x86.rs` (offsets, stubs, assembly), x86 SEH in
  `seh_x86.rs`; add `-p <name>` to CI's i686 clippy line.
- crypt32's caller structures with pointers live in
  `user/crypt32/src/layout.rs`: read a cbSize-versioned one's field with
  `get!` after `has!(size, Type, field)`, never against a byte count of
  one width (CERT_CHAIN_PARA's "32" is 16 on x86). Its `check!` lines
  hold each type to wincrypt.h's size and offsets for both targets: take
  the values from the header, not by hand (a C file of `int size_X =
  sizeof(X);` compiled with `clang -target i686-w64-mingw32 -S` and
  `x86_64-w64-mingw32` prints them in the assembly; COFF objects carry no
  symbol sizes for `llvm-nm -S`). A 32-bit DLL's exports' argument sizes
  can be checked against mingw's i686 import library: its `_Name@N`
  decorations (`llvm-nm /usr/i686-w64-mingw32/lib/lib<dll>.a`) give
  each stdcall function's bytes, which the Rust declaration must pop.
- advapi32's structures with pointer-sized fields live in
  `user/advapi32/src/layout.rs` (SID_AND_ATTRIBUTES, TOKEN_GROUPS'
  array offset `GROUPS_AT`, the absolute SECURITY_DESCRIPTOR and
  `layout::part`, SECURITY_ATTRIBUTES, TRUSTEE_W, EXPLICIT_ACCESS_W),
  with compile-time size checks for both targets: read and build them
  through it, never at x64 byte offsets.
- A 32-bit program reads HKLM\SOFTWARE through the registry's 32-bit
  view (`nt/regview.rs`): a key the seed puts under `SOFTWARE` that
  32-bit programs need (and that is not a shared key) needs its copy
  under `SOFTWARE\WOW6432Node` in `registry.seed` too, with the 32-bit
  values Windows has there.
- A service a 32-bit program calls needs its conversion in wow64.dll
  (`user/wow64`: a row of argument kinds in `generic::TABLE`, or an
  information-class case in `info.rs`); one without fails with
  STATUS_NOT_IMPLEMENTED and names itself in the ledger
  (`wow64.dll!service 0x...`). A class whose x86 layout differs must be
  converted, never passed through: check it against the x86 headers
  (pointer-sized fields, handles, SIZE_Ts, alignment).
- A DLL's declaration of a system call has the argument types of the
  stub in `user/testntdll/src/lib.rs` (a handle is `usize` there, a u64
  only a u64): the 32-bit stub pops what its declaration names, so a
  different size corrupts the caller's stack (locals read wrong values
  after the call, nowhere near it), and wow64.dll's row has one entry per
  4-byte word of it. `cargo xtask stub-audit` names any mismatch.
- Making a gala DLL 32-bit too: its raw-dylib blocks get the paired
  `cfg_attr`s (above); its runtime block (memcpy, `__chkstk`,
  `__CxxFrameHandler3` from ntdll) and `_fltused` become
  `#[cfg(target_arch = "x86_64")]`, with `#[cfg(target_arch = "x86")] use
  ntrt as _;` and ntrt as an x86-only dependency in its Cargo.toml (the
  runtime functions it calls itself declared in an x86 `extern "C"`
  block); a structure it reads at byte offsets becomes a `repr(C)` type
  with pointer-sized fields (comctl32's `layout.rs`), TEB reads and
  writes go through `fs:` and the x86 offsets (grep the crate for `gs:[`:
  ws2_32's `gs:[0x68]` LastErrorValue wrote outside the 32-bit TEB); add it to `USER_DLLS_X86` and
  to CI's i686 clippy line. Find byte-offset code with a grep for
  `.add(8).cast` / `.add(16).cast` / `[u8; N]` buffers: a structure with
  a pointer has other offsets and another size on x86 (WNDCLASSW 40 bytes,
  not 72; WSADATA's fields in another order). A COM class a 32-bit DLL
  serves needs its registration under `SOFTWARE\Classes\WOW6432Node` in
  `registry.seed`.
- An AFD request structure (ws2_32 to the kernel) is laid out by
  `kernel/afd_abi`'s `Layout` on both sides, never at byte offsets of
  one width: a new field or IOCTL goes there, with its x64 and x86
  offsets in its host tests (`cargo test -p afd_abi`).
- A 32-bit C GUI program: `user/wow64_gui_test` (mingw's i686 GCC with
  `-municode`, `-luser32 -lgdi32`); its scenario ships `x86/user32.dll`,
  `x86/gdi32.dll` (and kernel32, msvcrt, ntdll) to SysWOW64. gala's own
  window messages carry 32-bit WPARAM/LPARAM values (pack with 16-bit
  halves, never `<< 32`).
- wow64.dll touches a caller's memory only through `mem` (fault-safe):
  a plain dereference of a 32-bit pointer would crash the process
  instead of failing the call with STATUS_ACCESS_VIOLATION.
- A FUTEX_WAIT loop in a test reads the word once and passes that value
  (`for (int v; (v = word) != 0;) futex(&word, FUTEX_WAIT, v)`): reading
  it again for the argument can pass the value a wake already set, and
  the wait then sleeps with no wake to come (lx_epoll's join hung so).
- A recursive test function meant to use stack must keep its frame live
  across the call (`core::hint::black_box(&mut frame)` after it), or
  LLVM turns the recursion into a loop.

### Derived test data

Test data built from exported binaries is remade with them: `cargo xtask
export-tests` runs `user/pk_install_test/make_source.py` after exporting,
because that package source zips `user/bin/hello.exe` (a change to ntrt
changes every ntrt-based exe). New derived data goes in the same place.

### Reproducible user binaries

`cargo xtask export-tests` output must be the same on every machine (CI
compares it). Workspace crates get relative source paths, but crates.io
dependencies (libm in msvcrt) get absolute paths under the cargo home,
which end up in panic locations: xtask's `user_build` remaps the cargo
home to `/cargo` (`--remap-path-prefix`, merged into the target's
rustflags with `--config`). A new dependency needs nothing more.
Third-party builds (`user/<name>/build.sh`) pin their source and
checksum and strip build ids and timestamps so two builds give the same
hash. Inputs' file times count too: export-tests rewrites `user/bin`, so
a packer that stores them (NSIS's `File` without `SetDateSave off`)
gives a new binary on every run. Check a generated binary by building it
twice with a `touch` of its inputs in between.

### The Office survey (ROADMAP OF)

The `office-survey` workflow (`.github/workflows/office-survey.yml`; runs
on a push that changes it or `tools/office_survey.py`, or by hand)
installs Microsoft 365 Apps unactivated on a Windows runner with the
Office Deployment Tool (the owner accepted the licence terms for this)
and uploads only metadata (`tools/office_survey.py`: imports, the
export lists of Office's DLLs, COM classes, services, scheduled tasks;
never a binary). Download the `office-survey` artifact, then rank it
against gala's DLLs: `python3 -I tools/import_survey.py
target/x86_64-pc-windows-msvc/release --dumps <artifact>/imports
<artifact>/exports`; `--roots WINWORD.EXE,EXCEL.EXE,POWERPNT.EXE` keeps only
what those programs load (static imports through Office's own DLLs,
then delay imports), `--static` after it only what loading needs.

### Linux test programs (user/lx, ROADMAP LX)

Test first against real Linux (CLEAN_ROOM.md "Linux": man-pages, the
ABI/ELF specs, observed behaviour, gVisor; never the Linux kernel
source).

- **A shell's report of a killed background job depends on timing**:
  `kill $pid; wait $pid` prints "Terminated" through `wait` only when
  the job had not been reaped before; otherwise nothing (on Linux too).
  Discard `wait`'s stderr in transcripts (`wait $pid 2>/dev/null`),
  as `user/lx/busybox/httpd.sh` does.
- **The program**: `user/lx/<name>_test/main.c`, built with musl
  (`apt install musl-tools`), makes the system calls it checks itself
  (`user/lx/common/lx.h`: `sys6` and friends with the `syscall`
  instruction; failures are `-errno`), so it tests the kernel, not libc.
  `common/more.h` (on top of lx.h) adds musl's `SYS_*` numbers and
  struct definitions, `say_ret` (the errno's name appended), `lx_init`
  (closes every fd above 2 so fd numbers are deterministic and printed;
  clears the signal mask, SIGPIPE/SIGCHLD to default, umask 022),
  `set_action` (rt_sigaction with SA_RESTORER and an rt_sigreturn
  restorer, as musl does) and wait-status printing (without the core
  flag, which depends on core limits).
- **Output**: `name = value` lines, one write per line, ending with
  `<name>_test: ok`. Never an address or anything that varies between
  runs, kernel versions (auxv entries come and go: print only the ones
  you check), users (print "AT_UID is getuid() = yes", not the number)
  or build modes. Print relations, not pids/addresses/times/inodes; sort
  directory listings; directory sizes and nlinks are file-system
  specific and not printed; nothing depends on root (EACCES checks
  would). A test that lists or names its directory's entries works in
  a directory it makes (fs_test's `w`): gala runs it in `/lx`, beside
  every other binary, and only a directory a Linux program makes is
  case-sensitive there.
- **A test of a mount the host may not have** (run_test needs `/run`
  a tmpfs of mode 0755, as on systemd systems and CI; a container's
  `/run` is often a plain directory): record it in a mount namespace
  that has it, `unshare -m sh -c 'mount -t tmpfs -o mode=755 none /run
  && cargo xtask lx-expect --record'` (as root). Check what it prints
  only before permissions, so the runner's user does not matter.
- **Determinism**: children report through exit status or a pipe and
  the parent waits before printing on; never rely on timing for an
  ordering: a FUTEX_WAKE is retried until it wakes the thread known to be
  about to wait, a signal that must arrive while a child waits is
  blocked and taken with rt_sigtimedwait (a handler plus a blocking read
  raced and hung under load), echoes are read until the expected byte
  count. Accessibility without signals: write(2) from an address fails
  with EFAULT if it is unreadable, read(2) into it if it is unwritable
  (brk_test, mmap_test).
- **Threads**: a raw clone'd thread (thread_test's and epoll_test's
  `clone_thread`) has its own stack and TLS block and runs only raw
  system calls (no musl: its TLS is not musl's), leaving results in
  globals for the main thread to print after joining it (CLEARTID's
  futex wake); only the main thread prints. musl's pthreads may be used
  as such (thread_test's last part). A signal for a thread that must
  arrive while it waits is sent until its wait returns, or blocked and
  taken with rt_sigtimedwait.
- **Waiting in the kernel** (`lx::wait`): prepare before the last look
  at what is waited for (under its lock, or after joining its queue),
  then `block`; a wake can be spurious, so loop on the condition. A
  check made before `prepare` and not repeated after loses the wake
  that comes between (the thread then sleeps until its deadline or for
  ever).
- **Build**: `user/lx/build.sh` builds each as `user/lx/bin/<name>_test`
  (static, ET_EXEC) and `<name>_test-pie` (static-pie, ET_DYN),
  reproducibly (stripped, no build id, `-fno-stack-protector`: the
  canary is read from %fs:0x28, so a test that moves FS would crash).
  The tests in its `GLIBC_TESTS` (dyn_test) are built with the host's
  gcc against glibc instead, dynamically: `<name>_test` (ET_EXEC with an
  interpreter) and `<name>_test-pie` (a PIE), each `lib*.c` beside the
  test a `user/lx/bin/lib*.so` linked with `--as-needed` (one only
  dlopen()ed is not DT_NEEDED), found through DT_RUNPATH `$ORIGIN`.
  They use libc as a program does (printf, malloc, dlopen), not raw
  system calls: what they check is that glibc's programs run. CI's
  rebuild (ubuntu-24.04: gcc 13.3, glibc 2.39) must give the same bytes:
  their start-up files (`crt1.o`, `crti.o`) come from the host's
  libc6-dev, so the binaries are rebuilt on that Ubuntu release.
  The binaries are checked in under `user/lx/bin`, not `user/bin`
  (`export-tests` copies only the Windows build's outputs); the kernel's
  host tests `include_bytes!` them.
- **Transcripts**: `cargo xtask lx-expect` rebuilds them (when musl-gcc
  is there; else it runs the checked-in ones), runs each binary on this
  host as `./<name>_test first '' 'third arg'` from
  `target/lx-expect/<binary>/` (with `user/lx/bin/lib*.so` copied
  beside it) with only `A=1 EMPTY= LONGER_NAME=some value` in its
  environment, and compares stdout + `-- exit status N`
  with `user/lx/<name>_test/expected.txt` (both binaries against the
  same file). A mismatch prints a line diff and leaves `actual.txt`
  there. The Wayland clients (lx_expect.rs `WAYLAND_TESTS`: wl_test,
  wl_popup_test) get a
  weston of their own (`weston --backend=headless`, apt package `weston`)
  on a socket in a runtime directory of its own in the system's
  temporary directory (`gala-wl-<pid>`, removed after; a socket's path
  must fit sun_path's 108 bytes, which the run directory of a deep
  checkout does not), named by `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`
  added to their environment; its log is `weston.log` in their run
  directory. A headless weston 13 differs from the protocol in places: it
  has no wl_seat, adjusts no constrained popup, accepts set_gravity and
  set_anchor out of their enums and an xdg_surface destroyed before its
  popup, and answers an incomplete positioner and get_popup on a
  toplevel's xdg_surface with codes on other objects than the protocol
  names. A recorded test leaves those cases out (wl_popup_test's header
  lists them); gala follows the specification there, checked by host
  tests, and wl_input_test (written from the specification) covers what
  needs a seat or a known window place. A client that needs a seat to
  behave as on a desktop (GTK 3 warns without one: lx_expect.rs
  `SEATED_TESTS`, gtk_test) gets weston's X11 backend instead
  (`--backend=x11 --use-pixman`) in an Xvfb of its own (apt package
  `xvfb`; Xvfb picks a free display and names it with `-displayfd`;
  its log is `xvfb.log` in the run directory): Xvfb's keyboard and
  pointer make weston's seat, and nothing types or clicks there, so
  such a program drives itself when run by lx-expect (gtk_test clicks
  its own button: its header). In gala their scenarios (`lx_wl`) start the kernel's server
  with `wayland=on` and pass `XDG_RUNTIME_DIR=/run/user/1000
  WAYLAND_DISPLAY=wayland-0` with `lxenv=`; `wayland=trace` logs both
  directions as `WAYLAND_DEBUG=1` prints them, to compare with the same
  client run against weston under `WAYLAND_DEBUG=1`. A test real Linux
  cannot record (`INPUT_TESTS` in lx_expect.rs, with the reason:
  wl_input_test, as weston's headless backend has no input devices)
  is built and checked in like the others but not run by lx-expect; its
  expected.txt is written from the specification and its header says
  so, and its gala scenarios drive it: `lx_wl_input` runs on a headless
  desktop (`presenter=test`) with a screen check (`WL_INPUT_SCREEN`)
  whose steps each wait for the line the program printed for the step
  before (`after:`), then send PS/2 input with QEMU's monitor and check
  pixels; a step's moves avoid crossing a window's client area on the
  way (an enter or leave on the path would add lines), and only where a
  button lands is printed (how many motion events come depends on how
  the host's packets arrive). The server's keymap,
  `kernel/src/wayland/keymap.xkb`, is checked after a change with the
  host's `xkbcomp -w 10 -xkb keymap.xkb out.xkb` (x11-xkb-utils) and by
  compiling it with libxkbcommon (`xkb_keymap_new_from_string` from a
  scratch C program linked against `libxkbcommon.so.0`; no headers
  needed); the host test `every_key_the_scan_codes_give_is_in_the_keymap`
  checks that each key the scan codes give has a keycode there.
  `--runs 5` runs each binary 5 times and fails if a transcript
  varies (do it before recording); `--record` writes expected.txt (on
  real Linux only, and check the diff: it is the spec); `--no-build`
  skips the rebuild. Transcripts avoid version-dependent values because
  CI's runner kernel differs from the recording machine's, and so does its
  CPU: GitHub's runners are a mix of AMD and Intel, some with 5-level
  paging (user space up to 2^56), so a test finds such a boundary on its
  host (arch_prctl_test probes a mapping above 2^47) instead of assuming
  it. Nor may a transcript assert what Linux leaves to timing: a chain of
  exactly 40 symbolic links reads or fails with ELOOP, depending on
  whether the lookup's lockless walk fell back (fs_test asserts 20 and
  41, and "opens or ELOOP" for 40). lx-expect
  closes the descriptors it inherited first (it says how many): fork
  sizes a program's descriptor table by the parent's highest open
  descriptor, and answers such as select's on a descriptor beyond the
  table depend on that size.
- **Skips**: pty_test and proc_test print a single `SKIP:
  open(/dev/ptmx) = -errno` (or readlink /proc/self) line when that is
  unavailable, so a machine without them shows a mismatch whose diff
  names the reason.
- **Kernel side**: `kernel/src/lx/` (ARCHITECTURE "The lx module"),
  `cargo test -p kernel --lib lx`; the ABI data crate `kernel/lx_abi`
  (ARCHITECTURE "The lx_abi crate"), `cargo test -p lx_abi`, whose tests
  read the transcripts: changing a test program's output can break them
  (rerun both).
- **Boot scenarios** `lx_<name>` and `lx_<name>_pie` (one per binary)
  run it in gala: `qemu_exit lxrun=/lx/<binary> lxargs=<argv>
  lxenv=<env>` (both percent-encoded shell words; argv[0] first,
  `./<name>_test` as lx-expect gives it). The binary is shipped as
  `C:\lx\<binary>` by an EXTRA_MODULES entry with its repository path.
  xtask rebuilds the transcript from the kernel's `lx: pid N stdout: ..`
  / `-- exit status` lines and requires it to equal expected.txt (a diff
  on mismatch; `lx_expect::check_boot`). The transcript joins the
  stdout lines of every pid (a forked child's too) and ends with the
  last `--` line logged, so a test's children print nothing and end
  before it (pipe_test waits for each). A new user/lx test that should
  boot needs its two entries in EXTRA_MODULES and SCENARIOS (copy an
  `lx_` pair).
- **Bring-up of a new Linux program**: run it with `lxrun=`, then read
  the `unimplemented system call <name>` lines and the `ledger:` lines
  (`"what":"linux syscall missing"`, by hits) after `run: end`: that is
  the next work list.
- **Dynamic programs in gala** (`lx_dyn`, `lx_dyn_pie`): the image holds
  the test and its libraries in `/lx` and glibc where Ubuntu's ld.so
  finds it: `ld-linux-x86-64.so.2`, `libc.so.6` and `libm.so.6` from
  the pinned `libc6` .deb (`PREBUILT` "glibc", unpacked with dpkg-deb)
  in `/lib/x86_64-linux-gnu`, ld.so also in `/lib64` (PT_INTERP's path;
  a copy, boot modules make no links). The scenarios ship them as the
  module set "glibc" (MODULE_SETS); a new glibc test adds its program
  and libraries to its own scenario and that set. ld.so needs
  `/proc/self/exe` for `$ORIGIN` (served by exact path) and reads
  `/etc/ld.so.cache` if there is one (there is none: it searches its
  system directories). Bring-up: `lx: exec interpreter ...` shows where
  ld.so went; the ledger's `linux syscall missing` lines (rseq is the
  one glibc 2.39 reaches and does without) are the next calls.
- **GTK programs** (`user/lx/gtk_test`, build.sh's `GLIBC_TESTS` with
  `GTK_LIBS`): no GTK development package is needed; the program
  declares the functions it calls (from docs.gtk.org) and links against
  the runtime libraries by soname (`-l:libgtk-3.so.0`), so the host
  needs only `libgtk-3-0t64` (CI installs it, with `xvfb`); the bytes
  depend only on the sonames and symbol names, so any 3.24 build links
  the same binary. In gala it runs on the Linux root "gtk3" (below).
  Bring-up of such a program: run it on Linux under `strace -f -e
  trace=%file` (its own files are the root's list: what it opens and
  finds) and `WAYLAND_DEBUG=1` (what it binds, compared with
  `wayland=trace`); then in gala read stderr (GLib's warnings,
  fontconfig's errors), the crash report and the ledger.
- **Linux roots** (`LINUX_ROOTS` in xtask, ARCHITECTURE "GTK programs
  and Linux roots"): a distribution's packages assembled into
  `target/prebuilt/<name>/` on first use, shipped as trees (`usr/`,
  `etc/`, `home/`; a module set such as "gtk3"). To add a library or
  data file, find its package on Ubuntu 24.04 (`dpkg -S <file>`), take
  the release pocket's version, path and SHA-256 from the signed index
  (`dists/noble/InRelease`, checked with `gpgv --keyring
  /usr/share/keyrings/ubuntu-archive-keyring.gpg`, then
  `dists/noble/main/binary-amd64/Packages.xz`, whose hash InRelease
  lists) and add a `Deb` row with the files used; a THIRD_PARTY row too.
  The packages are kept in `target/prebuilt/debs/` (shared, checked at
  each use), so a re-assembly or an interrupted fetch downloads only
  what is missing. Assembly runs the host's `update-mime-database`
  (shared-mime-info) for a root with the MIME XML. A root carries no
  user database: the kernel makes `/etc/passwd` and `/etc/group` from
  gala's accounts (ARCHITECTURE "The lx module", Linux identity).
- **Linux programs that are not tests** (busybox): fetched as a
  `PREBUILT` (never run on the host), shipped to `C:\lx` by an
  EXTRA_MODULES entry, driven by a script of ours in
  `user/lx/<program>/` (`lx_busybox` runs `busybox sh /lx/shell.sh` with
  `lxenv=PATH=/lx`). Their transcript is per scenario,
  `user/lx/<program>/<scenario>.txt` (`lx_expect::check_boot`; a
  Wayland client's, such as `weston-simple-shm/lx_simple_shm.txt`, is
  its run against the host's weston headless backend, the same build),
  written from POSIX's and the program's documented output, one labelled line per
  check; cross-check it with the host's dash and coreutils (shell.sh's
  part after its setup, or httpd.sh behind a `busybox() { "$@"; }`
  shim; busybox is a test subject: it runs inside gala, where its
  behaviour is what is tested; see "Third-party code on the host"). busybox.net's build has no standalone shell, so `shell.sh`
  first makes `/lx/bb.bin/<applet>` symbolic links to busybox and puts
  that directory first in PATH (an applet it needs goes in that list);
  httpd.sh still calls `busybox <applet>`. To try a command line quickly, change `lx_busybox`'s
  `lxargs=` (percent-encoded) and read the `lx: pid N stdout:` lines.
- **Pitfalls met**: a C argument evaluated twice in one call
  (`say(name, poll(&rev), rev)`) reads in an unspecified order: compute
  first, print after (socket_test's `say_poll`); a test that must give one
  transcript for both binaries cannot print argv[0] (the `_pie` scenarios
  run `./<name>_test-pie`): print relations (exec_test); Linux answers a
  non-blocking TCP connect with EINPROGRESS even on loopback (the next
  connect returns 0 once). Rust's `Command` passes the environment sorted by
  name (auxv_test's envp order); a static-pie's initial break is not
  above its bss (brk_test prints only what holds for both); a pipe write
  whose buffer runs into an unmapped page fails with EFAULT, not a
  partial count (write_exit_test); a store to a never-touched bss page
  after mprotect(PROT_READ) faults with error code 6 (not present), so
  signal_test touches it first (7). A MAP_PRIVATE page never written
  shows later writes to its file (the page cache; mmap_test asserts it),
  one written keeps its copy. A relation over a path from getcwd must
  count its NUL: the system call's length includes it (exec_test's
  `/proc/self/exe` checks). Kernel code a host test calls must not log
  (`crate::info!` is the serial port: SIGSEGV on the host): exec's
  `prepare` returns its plan and the caller logs it. An address length
  passed to accept, getsockname or recvfrom is value-result: reusing
  the variable for a later bind or connect passes what the kernel
  wrote (2 for an unnamed peer: an autobind, or EINVAL), so unix_test
  keeps separate ones. A Linux process's memory goes when it ends,
  before its parent's wait4 returns, so whatever the parent does next
  (sealing a memfd against writing: EBUSY while a writable mapping is
  left) must not see the child's mappings (`lx::process_ended`).

## Program recipes

Third-party programs are either built reproducibly by a checked-in
script (the binary is checked in under `user/bin`; CI uses it and never
runs the script) or fetched as pinned releases (`PREBUILT`). Keep
downloaded files in their own directory and run them only inside QEMU.
Read the source of the exact version of an open-source program before
deciding what it does (tags: `git ls-remote --tags`; a sparse,
blob-less clone of one directory is quick).

### Go programs

`user/<name>/` with a `main.go` and `go.mod`, listed in `GO_PROGRAMS` in
xtask: built for windows/amd64 with the pinned toolchain (`GO_TOOLCHAIN`,
fetched by version; `-trimpath`, empty build id, so the binary is
reproducible and checked in under `user/bin`). Without `go` installed,
xtask uses the checked-in binary. Bring-up loop: run it
(`XTASK_EVENTS=all cargo xtask boot-test <name>` once it has a scenario,
or `cargo xtask screenshot run=<name>.exe`) and read the `ledger:` lines;
Go programs also print which DLL or procedure they could not find.

### Nginx

`user/nginx/build.sh` cross-compiles Nginx (pinned version and SHA-256)
with mingw-w64 (`apt install gcc-mingw-w64-x86-64`) into
`user/bin/nginx.exe`, reproducibly (fixed build directory, no debug
info, no link timestamp). Its configuration, page and placeholder files
(boot modules only create the directories they are in) are in
`user/nginx/` and shipped by EXTRA_MODULES entries with repository
paths. It runs with `-p C:/nginx/ -c conf/nginx.conf`; the image path is
then `C:\nginx\nginx.exe` and the loader looks for DLLs next to it, then
in System32. `cargo xtask screenshot run=nginx.exe
"args=-p%20C:/nginx/%20-c%20conf/nginx.conf"` shows it; `cargo xtask
serve nginx` serves it to a browser. Nginx relies on an accepted socket
inheriting the listener's non-blocking mode.

### TLS tests (Schannel)

`schannel_test` is a TLS client of Nginx (OpenSSL) inside the guest: the
scenario starts `nginx.exe` with `start=` and
`user/schannel_test/nginx.conf` (ports 8443-8449 on 127.0.0.1, so no
xtask network client is involved: 8443 the issued certificate, 8444
self-signed, 8445 expired, 8446 resumption from Nginx's own session
cache, 8447 client certificates, 8448 a revoked certificate, 8449 one
that is not), and the test retries its connect while Nginx starts. `/session` on 8443 and 8446 answers Nginx's
`$ssl_session_reused` ("r" resumed, "." not), `/client` on 8447
`$ssl_client_verify $ssl_client_s_dn`: what Nginx saw, the peer's
view of a check. On Windows, run-all.ps1 starts the same Nginx from a
prefix in TEMP and, as an administrator, imports the test CA into
LocalMachine\Root for the run (skipped otherwise). `nginx_tls` instead
uses the host's `curl -k` as the client. `winhttp_test` and
`wininet_test` (C, built with their `build.sh`; `wow64_wininet_test` is
wininet_test's source for x86) use the same Nginx, also
on plain HTTP (8080); a failed request names its step in the ledger
(`winhttp.dll!connect`, `wininet.dll!TLS handshake`, ...). Both DLLs run
on `user/http_client`: a change to the engine boots both scenarios.

- Host-test the engine first: `cargo test -p schannel_tls` runs both
  sides in memory (seconds, no QEMU); boot the scenario once those pass.
- Test certificates: `user/schannel_test/certs/make.sh` (host openssl
  3.0+; run it in place and commit the results; test data, not secret):
  a CA, an issued `localhost` certificate (ECDSA), an expired one
  (`openssl ca` sets a past validity, `x509 -req` cannot), a self-signed
  one, an RSA pair for the test's own server (`server-rsa.cer`/`.p8`,
  also used by the host tests), an ECDSA P-384 one for it
  (`server-ec384.cer`/`.p8`), a client certificate (`client.*`,
  ECDSA P-256, clientAuth), and the revocation set: `revoked.*` and
  `notrevoked.*` naming the CA's CRL at http://127.0.0.1:8080/ca.crl,
  and `ca.crl` listing the first (`sh make.sh crl` makes only these
  with the CA that is there). They are valid from the moment they are
  made: host tests use a fixed clock after that (`TODAY` in
  `user/schannel_tls/src/tests.rs`: move it when the certificates are
  made again). Regenerating changes every file and nothing in the tests'
  expectations.
- gala's machine root store is `C:\Windows\System32\gala\roots`: a
  scenario that needs a trusted root ships the CA there (an
  EXTRA_MODULES line). Without one, automatic validation gives
  SEC_E_UNTRUSTED_ROOT and the ledger says `crypt32.dll!ROOT store (no
  gala/roots directory)` (crypt32's ROOT store is what Schannel trusts).
  Full images (`cargo xtask iso`) also ship
  the public CA bundle there (`user/roots/public-roots.pem`); a scenario
  that wants it lists it (`pk_install_https`). To move to a newer
  bundle: `sh user/roots/update.sh <date>` (a release from
  curl.se/docs/caextract.html, its published SHA-256 checked), then the
  date and checksum in THIRD_PARTY.md and `cargo test -p schannel_tls`.
- The Nginx configuration also serves redirects for `winhttp_test` and
  `wininet_test` (`/redirect-rel`, `/to-https`, `/to-http`, `/loop`,
  `/post-307`) and `pk_install_https`'s release URL; Nginx's
  `absolute_redirect off` keeps a relative Location relative. A failed
  redirect names its reason in the ledger (`winhttp.dll!redirect:
  ...`).
- A failing handshake leaves `schannel.dll!ISC: <rustls error>` (ASC for
  a server) in the ledger with the SSPI status: read that first. A
  refused certificate also leaves crypt32's verdict,
  `crypt32.dll!SSL: CERT_E_UNTRUSTEDROOT` (or BASE, BASIC_CONSTRAINTS;
  CERT_E_EXPIRED, CERT_E_CN_NO_MATCH, ...) with the chain's
  CERT_TRUST_* error bits as detail (0x20 an untrusted root, 0x10000 a
  partial chain, 0x1 time, 0x8 a signature): Schannel and programs that
  check chains themselves (curl's `--cacert`, Go) leave the same line.
  A file of `gala\roots` or `gala\ca` crypt32 cannot decode is
  recorded as `crypt32.dll!machine store (a certificate or CRL that
  does not decode)`.
- Revocation is checked offline: crypt32 takes CRLs from gala's CA
  store, `C:\Windows\System32\gala\ca` (a scenario ships its CRLs
  there with an EXTRA_MODULES line, as schannel_test does `ca.crl`), the
  caller's stores and hAdditionalStore, and fetches nothing. A refused
  certificate leaves `crypt32.dll!SSL: CRYPT_E_REVOKED` (or
  CRYPT_E_REVOCATION_OFFLINE: 0x1000040 in the bits; or
  CRYPT_E_NO_REVOCATION_CHECK: 0x40, a certificate naming no CRL
  distribution point, which is never checked by CRL);
  `crypt32.dll!revocation: CRL not fetched (offline only)` says no CRL of
  its issuer was there (OCSP: `OCSP not asked`); `crypt32.dll!revocation:
  a CRL whose signature does not verify` (or of another scope) says a CRL
  there was not used.
  Schannel returns the CRYPT_E_* status itself (curl prints
  `CRYPT_E_REVOKED (0x80092010)`, exit 35).
- The chain engine's own tests are `cert_test`'s chain part
  (`user/cert_test/src/chain.rs`) with certificates made once by
  `user/cert_test/chain/make.sh` (RSA root, ECDSA P-384 intermediate,
  P-256 leaves; fixed validity 2025-2045, so the times in the test are
  fixed); its trust comes from an engine with hExclusiveRoot, so the
  test needs no change to Windows' root store; the same `make.sh` makes
  the CRLs of the revocation checks (`inter.crl` listing `revoked.cer`,
  an out-of-date `inter-old.crl`, `root.crl`), which `cert_test`'s CRL
  part (`user/cert_test/src/crl.rs`) uses too. Its keys are not kept: a
  new CRL means making them all again (the tests' expectations stay).
  Each part prints `cert_test: chain: <part>` (`cert_test: crl:
  <part>`) before its checks.
- SEC_E_INCOMPLETE_MESSAGE means the caller passes the same bytes again
  with more (schannel_tls keeps a count, `retained`); a rustls slice that
  still holds a partial handshake message must be the same buffer on the
  next call (the engine hands it the context's own copy). After a rustls
  error, do not call it with an empty buffer: it may still point into
  the last one (it panics).
- schannel.dll is 1.4 MB (rustls and RustCrypto): only processes that
  use Schannel load it, through secur32.
- **curl and Git** (`curl`, `curl_schannel`, `git_https`): unmodified
  PREBUILT programs run by busybox sh scripts (`user/curl/curl.sh`,
  `user/git_https/git.sh`) against the same Nginx, one labelled line
  with the exit code per step. curl.se's Windows build is LibreSSL
  only (CURL_SSL_BACKEND does nothing there: `curl -V` lists one
  backend), so the Schannel curl is Git for Windows' (PREBUILT
  `gitcurl`: curl.exe, its Schannel-only libcurl-4.dll and that DLL's
  imports, from PortableGit). curl prints Schannel's failures as
  `schannel: SEC_E_... (0x8009....) - <FormatMessage text>`: the SSPI
  status is the place to start (then the `schannel.dll!ISC:` ledger
  line). With `--cacert` Schannel's curl validates itself
  (SCH_CRED_MANUAL_CRED_VALIDATION, then crypt32's chain engine with the
  CA file as hExclusiveRoot and the remote certificate's hCertStore as
  additional store) and prints `schannel: CertGetCertificateChain trust
  error CERT_TRUST_...` (exit 60), then matches the host against
  CertGetNameString(CERT_NAME_DNS_TYPE, all names); curl.sh's `cacert`
  steps cover it (CA files in C:\gala). Git's dumb HTTP protocol needs only static files:
  `user/git_https/repo.git` is a bare repository after `git
  update-server-info`, cut to HEAD, info/refs, objects/info/packs and
  the loose objects (made with fixed author and committer dates, so the
  commit id in the scenario stays). To serve more, make it the same way
  on the host and check the files in.
- Resumption and client certificates are per credential handle: a test
  (or a program) that wants a resumed handshake keeps its handle, and a
  check that wants a full one takes a new handle (or
  SCH_CRED_DISABLE_RECONNECTS). A session made after a client
  certificate came with a new credential belongs to that credential.
  SECPKG_ATTR_SESSION_INFO says SSL_SESSION_RECONNECT; Nginx's
  `/session` must agree.
- SEC_I_INCOMPLETE_CREDENTIALS then a new credential runs the client's
  handshake again from its recording (schannel_tls `replay.rs`). If it
  fails, the ledger has `schannel.dll!ISC: General("the handshake run
  again differs")` (something rustls draws escaped the tape: a new
  source of randomness or time must go through it) or `General("a
  client certificate after a refused session offer: no replay")`. The
  host tests `incomplete_credentials_*` run it in memory, also with the
  server's flight a few bytes at a time.
- Schannel checks revocation when the credential asks
  (SCH_CRED_REVOCATION_CHECK_*: curl without `--ssl-no-revoke`). The
  scenarios' steps against certificates that name no CRL distribution
  point pass curl's `--ssl-no-revoke` and Git's
  `http.schannelCheckRevoke=false`: Windows has nothing to check those
  by. curl.sh's `revocation`/`revoked` steps (ports 8449/8448) check
  without it.

### busybox

`user/busybox/build.sh` rebuilds `user/bin/busybox.exe` (pinned
busybox-w32 commit, reproducible; the build directory
`/tmp/gala-busybox-build` keeps `busybox_unstripped.exe.map`, which maps
a faulting rip to a function). The `busybox` scenario runs a sh command
line; to try others, change its `args=` (percent-encoded). The script
scenarios (`busybox_files`, `busybox_script`) run checked-in scripts
from `user/busybox/` (boot modules in C:\gala); each prints one labelled
line per result. busybox is also the tool for looking at files on gala:
`run=a.exe,busybox.exe args=...,cat%20C:/dir/file` (gala's cmd has no
`type` or `&` lists).

### TinyCC

`user/tcc/build.sh` rebuilds `user/bin/tcc.tar` (pinned TinyCC commit,
reproducible): it compiles `tcc.c` as one unit with mingw-w64 and writes
`config.h` itself instead of running TinyCC's `configure` and `make`
(which would run third-party build scripts on the host);
`CONFIG_TCC_PREDEFS=0` makes tcc read `include/tccdefs.h` at run time
instead of a header generated by a host-built tool.

- `tcc` (`user/tcc/tcc.sh`, run by busybox sh): unpacks the archive with
  busybox's tar, builds the runtime library (`libtcc1.a`, the startup
  code) with that `tcc.exe` as win32/build-tcc.bat does, compiles
  `user/tcc/hello.c` and runs it (about 4 s in the kernel, 16 tcc runs).
  A program tcc builds imports msvcrt's startup functions
  (`__getmainargs`, `__argc`, `_controlfp`, `_XcptFilter`); tcc itself
  uses `_spawnvp`/`_cwait`/`SearchPathA` only for tools the scenario
  does not run.
- `tcc_self` (`user/tcc/self.sh`): tcc.sh, then TinyCC's sources
  (`tcc/src/tcc/` in the archive) compiled by tcc on gala into
  tcc2.exe, which builds hello.c and `user/msvcrt_test/main.c` (about
  7 s). main.c must build with both mingw's and TinyCC's headers
  (TinyCC's float.h lacks `_controlfp`; mingw's headers send `strtod` to
  libmingwex, so the test looks msvcrt's up).
- `tcc_make` (`user/tcc/make.sh`): builds `user/tcc/make/` (boot modules
  in `C:\gala\make`) with busybox's make, passing `CC=../tcc/tcc.exe
  CFLAGS=-B../tcc` so make's echoed commands are the same on gala and
  Windows; each step's lines carry the step's name (`build:`, `after
  touch:`, `again:`, `broken:`).

### Lua

`user/lua/build.sh` repacks the Lua 5.4.7 release into
`user/bin/lua.tar` (checked against lua.org's published SHA-256; nothing
is compiled on the host). The `lua` scenario (`user/lua/lua.sh`) builds
it on gala with Lua's own `src/Makefile`, overriding only its documented
settings (`CC`, `CFLAGS`, `AR=tcc -ar rcs`, `RANLIB=:`, `LIBS=` since
msvcrt has the math functions, `LUA_T`/`LUAC_T` with `.exe`), and runs
`user/lua/test.lua`. Expected lines come from Lua's and msvcrt.dll's
documented behaviour (compiling Lua on the host would be a third-party
build; Windows is the reference through `run-all.ps1`). test.lua avoids
output that depends on the script's path (`error(msg, 0)`) and exact
decimal ties (C runtimes round them differently).

### SQLite

`user/sqlite/build.sh` fetches the SQLite amalgamation zip (checked
against sqlite.org's SHA3-256) into `user/bin`; the `sqlite` scenario
unzips it on gala, compiles it with tcc and `user/sqlite/sqlrun.c`, then
runs `test.sql` and `reopen.sql` (expected lines computed with Python's
sqlite3 module on the host: same SQL, another SQLite version). TinyCC's
Windows headers and `.def` files are older than SQLite needs: `tcc
-impdef` makes kernel32.def from the system's kernel32.dll, and
`tcc_win.h` (force-included) declares GetTickCount64. The last step runs
two writers at once (busybox `&` and `wait`); to see whether they really
contended, run with `XTASK_EVENTS=syscall,process` and count NtLockFile
results per process (STATUS_LOCK_NOT_GRANTED 0xc0000055 is a retry).

### Prebuilt programs

Unmodified releases (Caddy, the JDK, .NET, PowerShell, FileZilla, FlaUI,
...) are not kept in the repository: the `PREBUILT` table in xtask pins
each one (URL, SHA-256, the files used), and a scenario module path
`target/prebuilt/<name>/<file>` makes xtask download, check and unpack
it on first use.

- A release that is a single file (busybox.net's static Linux
  `busybox`) names only that file as its one member, the URL's last
  component: the download is kept as it is.
- Unpackers: unzip for zips (`apt install unzip`; a fresh Ubuntu, WSL's
  included, has none, and the prebuilt fails to spawn it), msitools'
  `msiextract` for an MSI (`apt install msitools`), 7-Zip (`7z` from
  p7zip-full) for a `.7z` archive (mesa-dist-win's; members are file
  paths such as `x64/opengl32.dll`), an NSIS installer or a 7-Zip
  self-extractor, and for a Chocolatey `.nupkg` the installer in
  its `tools/` unpacked with 7-Zip; a nuget.org `.nupkg` (a library
  package: FlaUI) is a plain zip, its members paths such as
  `lib/net8.0-windows7.0/FlaUI.Core.dll`; a Debian package (`.deb`:
  Ubuntu's glibc, weston, libwayland-client, libffi) is unpacked with
  `dpkg-deb -x`, its member files (`usr/lib/x86_64-linux-gnu/libc.so.6`)
  moved up by their names; a member that is a symbolic link (a
  library's soname, `libffi.so.8`) is its target under the link's name.
  Take a .deb's SHA-256 from the release's signed `Packages` index
  (`dists/noble/<component>/binary-amd64/Packages.gz`). One fetch runs at a time
  (ISOs are built in parallel), and a `.unpacked-<sha256>` marker makes
  an interrupted unpack run again. An unpack (no marker for the pinned
  checksum and selection: a new version, changed members, an interrupted
  run) starts by clearing `target/prebuilt/<name>`, so an older
  release's files never stay beside the new ones.
- `members` are unzip patterns (`*` crosses `/`), `excludes` unzip `-x`
  patterns; the marker includes a hash of both, so changing them unpacks
  again. For an MSI, members are directories of the installed layout,
  each moved to `target/prebuilt/<name>/<last component>`, and excludes
  are paths removed after that; for 7-Zip archives members are
  directories.
- A pinned release can vanish upstream (SourceForge dropped FileZilla
  3.69.3's installer): pin a release through a source that keeps old
  versions, such as Chocolatey's package of the official installer
  (FileZilla Client and Server), and change the pinned version when it
  goes; a package carrying a 32-bit and a 64-bit installer is unpacked
  from the 64-bit one (by name: `win64`/`x64`). Copy a fetched
  `target/prebuilt/<name>` aside first to compare against the new
  version (the re-pin clears it).
- **Large programs**: a module path ending in `/`
  (`target/prebuilt/jdk/jdk-21.0.5+11/`) ships the whole tree as
  `C:\<dir>` with its subdirectories, packed into one USTAR archive
  (`boot/<dir>.tree.tar`, module string `tar:<dir>`) that the kernel
  unpacks into its ramfs in place (`kernel/src/fs/tar.rs`): Limine panics
  with "Memory map exhausted" with a few hundred modules. `run=` finds a
  program inside an archive by file name, or by its path when the name
  has a `/`, in the archive or below C: (`run=cmd/git.exe`: MinGit has
  two git.exe; `run=git/usr/bin/bash.exe` when each directory is its own
  archive). Scenarios that ship the same large set share it through
  `MODULE_SETS`.
- **A program's own configuration** lives under `user/<program>/` (e.g.
  `user/filezilla_server/fzs`): a directory module (`dir/` path) becomes
  a tree on C:, read-only unless its directory is in `WRITABLE_TREES`
  (programs that rewrite their settings need that). To see the
  configuration a program writes by default, run it and then `cat` the
  files with busybox.
- **Java**: a crashing JVM writes `C:\hs_err_pid<N>.log` (registers, the
  faulting address, the thread's stack, the memory map); the boot option
  `dump=C:/hs_err_pid4.log` (comma-separated, percent-encoded paths;
  a directory gives every file below it) prints such files to the
  serial log as `dump:` lines once the `run=` programs have ended, and a
  failed scenario's dumped files are written back out under
  `target/failed/<name>.dump/` (OBSERVABILITY "Guest files"). The Java
  scenarios have it, so a crash's report is in `target/failed/<name>.log`;
  any program that writes its own log (ODT's setup.exe: `office_install`)
  should get one. ODT writes `C:\Users\Public\<MACHINE>-<date>.log`
  (UTF-16, tab-separated) until it has read the configuration's Logging
  path, so the scenario dumps `C:/Users/Public` too; its `Unexpected`
  and `Non Task Error` lines say why it stopped (`MRO::LoadFromVCab ...
  InvalidSignature (HR:0x800B0003)` was the cabinet's trust).
- **The last ledger lines are not the cause.** What a program does on
  its way out shows up last: ODT's slc.dll, SetSystemFileCacheSize and
  NtSetSystemInformation came after its log's "Bootstrapper Finished"
  (telemetry at exit), the cabinet's rejection before it. Read the
  program's own log, or the events up to the first error path, before
  filling the last gap.
- **.NET**: the ledger first, then `XTASK_EVENTS=syscall,image,fault`.
  CoreCLR's own source (dotnet/runtime, MIT) says what it expects of the
  OS (read only, as any application's documentation).
- An unhandled exception's code and address are in the ledger;
  `XTASK_EVENTS=image` gives each image's base and size to map them.

### Win32-OpenSSH (sshd)

The sshd scenarios (`sshd_server`, `sshd_shell`, `sshd_sftp`,
`sshd_scp`, `sshd_forward`, `sshd_client`, ...) run Win32-OpenSSH; "SSH
into a running gala" says how to log in.

- As SYSTEM (and with `-d`) sshd and ssh-keygen log to the system log
  (ETW), not the console: pass `-e` to sshd for stderr. The scenarios tee
  `sshd.log` / `sshd-session.log` to the serial log (`logtee:` lines).
- Its source (github.com/PowerShell/openssh-portable, BSD-licensed; not
  Windows source) says what it expects of an API, for example
  `contrib/win32/win32compat/pwd.c` (accounts must be SidTypeUser; home
  from ProfileList). Read the shipped tag (`v9.8.3.0`), not
  `latestw_all`: the branch waits for sshd-session in `sshd -d`, 9.8.3
  exits with 255 right after spawning it.
- Win32-OpenSSH does not create %ProgramData%\ssh\logs (an installer
  does: the scenarios ship a placeholder file there); it runs subsystems
  and commands through DefaultShell with the command quoted, so
  sftp-server.exe must be on the shell's PATH (a copy in System32), and
  so must scp.exe for legacy scp (`scp -O` runs `scp -t`/`scp -f` there;
  it imports shlwapi); an ssh client without a tty asks for a 0 x 0 pty
  (conhost then uses 80 x 25).
- With DefaultShellEscapeArguments 0 sshd runs `busybox.exe sh -c
  "<command>"`: a command xtask sends must not contain double quotes
  (quote a path with spaces in single quotes, as `sshd_client` does).
- Host-side checks of times: the host's own file moves under the test
  (relatime updates the access time of a file read whose access time is
  older than its modification time), so `scp_client` sets the times
  again before each upload.
- Forwarding tests need no host port: `ssh -L` forwards from a Unix
  socket in the client's directory, `ssh -R` to an echo server xtask
  binds on port 0; the far end inside gala is `sshfwd_test`.
- Its sockets (`contrib/win32/win32compat/socketio.c`) keep one
  overlapped WSASend with a completion routine pending, of up to 70 KiB
  (more than a gala socket's send buffer), report it as written, and a
  channel calls shutdown(SD_SEND) as soon as its own buffer is empty:
  the FIN must follow that send (ARCHITECTURE "AFD"). A short
  `sshd_forward` stream: the verdict says which leg lost bytes
  (sshfwd_test checks the pattern as it arrives and says how much it
  served), and the `tcp shutdown`/`tcp close` events (`events=net`, on in
  the scenario) count each loopback connection's bytes at both ends;
  `afd send failed` names a send that never finished.
- Each process's console-control handler thread logs how late it took a
  Ctrl+C (`took Ctrl+C N late`, from 250 ms on).

### FlaUI (UI Automation tests in .NET)

`user/flaui_test/` holds a C# test (`FlaUITest.cs`) and the program it
drives (`target.c`, a Win32 window with a button, an edit and a check
box). `build.sh` builds both into `user/bin` (`flaui_test.dll`,
`flaui_target.exe`; reproducible): mcs compiles against .NET 8's
reference assemblies (Microsoft.NETCore.App.Ref, which the script
fetches with its checksum into `target/prebuilt/dotnet-ref`) and the
FlaUI assemblies xtask fetches (PREBUILT `flaui-core`, `flaui-uia3`,
`uiautomation-interop`; run `cargo xtask boot-test flaui_test` once
first, which fetches them and then fails if the test is not built).

Building and running:

- mcs cannot compile against the runtime's own assemblies
  (`shared/Microsoft.NETCore.App/8.0.33`): System.Private.CoreLib's Span
  has ref fields, which mcs's metadata reader rejects. The reference
  pack declares them with dummy fields. Mono's own mscorlib does not
  work with FlaUI either (FlaUI's types come from System.Runtime, and
  mcs would see two System.Objects).
- mcs (C# 7-ish) rejects nullable reference annotations (`Window?`);
  a type forwarded to an assembly not in build.sh's reference list
  (System.Threading.Thread) needs that assembly added there.
- A new test is a new `.cs` (or a new program in the directory) next to
  FlaUITest.cs; its scenario runs `dotnet.exe C:/gala/<name>.dll` with a
  `<name>.runtimeconfig.json` and the FlaUI assemblies in the `dotnet`
  module set (EXTRA_MODULES), as `flaui_test` does.
- On Windows: `dotnet flaui_test.dll` in a directory holding the test,
  its runtimeconfig, `flaui_target.exe` and the three FlaUI assemblies.
  run-all.ps1 skips `flaui_target.exe`, which would wait for its window
  to be closed.
- When FlaUI throws, the test prints the exception with its stack
  (`flaui_test: exception:`), which names the
  Interop.UIAutomationClient method; `UIAutomationCore.dll!...` ledger
  lines name gala's missing methods (E_NOTIMPL becomes
  NotImplementedException in .NET).

**Screen tests on FlaUI** (`filezilla_flaui`, FileZillaFlaUI.cs, is the
example): a C# program next to FlaUITest.cs, built by the same build.sh
(add a `build` line), shipped through the `dotnet` module set plus the
program's own: list the scenario twice in MODULE_SETS
(`("filezilla_flaui", "dotnet")`, `("filezilla_flaui", "filezilla")`).
The scenario starts servers with `start=` and runs `dotnet.exe
C:/gala/<test>.dll`; the test launches the program itself
(Application.Launch) so it can wait for it to end. No screen is needed
(BIOS boot, `noscreen`): windows exist and are driven without being
shown, several times faster than the pixel version.

Writing one:

- Find controls by what a tester sees: ControlType and Name (a field's
  Name is its label's text, `ByName("Host", MatchSubstring)`), not
  AutomationId when the program numbers its controls itself (wx's ids
  are negative pool ids, many 5999).
- Dialogs are owned windows: children of the window that owns them
  (`main.FindFirstChild(cf => cf.ByControlType(Window))`), not of the
  desktop. Answer them in a loop with a deadline, as they come.
- Invoke, Expand and SetValue return before the program has acted: wait
  for the effect with `Retry.WhileNull`/`WhileTrue` (a dialog gone, a
  list item there, a menu open), never with fixed sleeps.
- An element can go between two reads (a dialog closing after OK, a
  pane the program rebuilds): FlaUI then throws ElementNotAvailable
  (COMException 0x80040201) from the second read, on Windows too. Wait
  for a dialog to go with a check that treats the exception as "gone"
  (the test's `Showing`), and catch it in polling loops.
- On failure print the tree (the test's `Dump`: control type, name,
  automation id, class for a few levels).
- Close a window with `window.Close()`: FlaUI invokes the title bar's
  Close button (AutomationId "Close", WM_SYSCOMMAND SC_CLOSE posted), so
  it returns before the program has acted; wait for `app.HasExited` or
  the window gone. Window/Transform calls need a top-level window with a
  caption: SetWindowVisualState(Maximized) on a window without
  WS_MAXIMIZEBOX is UIA_E_INVALIDOPERATION (InvalidOperationException),
  Move on a maximized one too.
- A window's first UIA child is its TitleBar (no HWND): count a window's
  controls with a NativeWindowHandle condition, or by ControlType, not
  by position among all children.
- Grids: FlaUI's AsGrid works on a report-view list view (cells are the
  items' children, so FindAllDescendants on a big list visits a cell per
  column too; search children of the list where you can).
- Events (FlaUI's Register*Event): the handlers run on
  UIAutomationCore's event thread, not the test's: collect what they see
  in a ConcurrentQueue and wait for it with Retry (flaui_test's
  `Event`); register before the action that raises the event (an Add
  returns once the event thread's hook is in place). They come from
  WinEvents that user32 and comctl32 raise: an event a control does not
  raise never comes (ARCHITECTURE "Windows across processes: WinEvents"
  lists them).
- A native thread that calls into .NET COM objects (a handler) must be
  in an apartment (CoInitializeEx), or the call fails with
  CO_E_NOTINITIALIZED (0x800401F0) and the handler never runs.
- A UIA call on another process's element is a sent message to it
  (SendMessageTimeout with SMTO_ABORTIFHUNG and the TransactionTimeout,
  as on Windows): if the program's thread is not pumping, the call waits
  up to the TransactionTimeout (20 s by default) or 5 s once the program
  counts as hung, then reads empty (ledger line
  `UIAutomationCore.dll!SendMessageTimeout(timed out)`).

Implementing what FlaUI needs (gala's UIAutomationCore, user32):

- FlaUI reads every property with GetCurrentPropertyValueEx(id, TRUE)
  and compares the result with ReservedNotSupportedValue by reference: a
  property UIAutomationCore cannot answer must be the not-supported
  value, not VT_EMPTY, or FlaUI converts null and throws.
- A property whose value is an element or element array
  (SelectionContainer, Selection) is VT_UNKNOWN holding our element /
  IUIAutomationElementArray object: FlaUI casts the RCW to the interface
  (QueryInterface), so those objects must answer it.
- Driving another process's list view: LVM_* messages are not marshaled
  across processes (they are above WM_USER, on Windows too); pointers in
  them must be in the list's process (`items.rs`'s `Buffer`). LB_/CB_
  messages are marshaled by user32 (`foreign::send_remote`): pass local
  pointers.
- user32 calls on another process's window that change it
  (SetWindowPos, menus) go there as a message user32 answers itself
  (GALA_SETWINDOWPOS, GALA_MENU, in the system range below WM_USER); a
  new one needs its buffer marshaled in `foreign::send_remote` and an
  entry in `carries_pointer` (it cannot be posted), and its answer in
  `SendMessageW` before the window procedure.

### Third-party code on the host

Two kinds of downloaded code, two places:
- **Test subjects** (the Windows and Linux programs gala is tested with:
  Chromium, busybox, installers, servers in the guest) run inside gala:
  their behaviour on gala is the point, and most cannot run on the host
  anyway.
- **Host-side tools** that drive or serve a test (WPT's `./wpt serve`,
  the host's curl, ssh, psql) may run on the host when they come from a
  well-known source, are pinned (a commit or a checksum, fetched by
  xtask) and are recorded in THIRD_PARTY.md, as wptserve is.

### Chromium (BR)

`chromium_wpt` (PENDING; web-platform-tests through ChromeDriver,
`xtask/src/wpt.rs`) runs WPT's own server on the host (`./wpt serve`
in `target/prebuilt/wpt`, HTTP/2 off, ports 8000, 8001, 8443, 8444, 8666
and 8889; its first start makes a Python virtualenv with its pip
packages; its log is `target/wpt/wptserve.log`). WPT's host names must
resolve to the loopback first: `cd target/prebuilt/wpt && ./wpt
make-hosts-file >> /etc/hosts` (the run says so when they do not). The
guest reaches it as 10.0.2.100 on those ports (QEMU runs `xtask relay
<port> <log>` per connection; each logs two lines in `target/wpt/relay.log`,
keyed by the relay's pid: `open` with its start, then its start and end
(host epoch seconds with ms, as ChromeDriver stamps its log, and UTC),
bytes each way and how each side ended in order, or why it could not
connect; an `open` without its second line never ended) and Chromium maps web-platform.test and its subdomains
there (`--host-resolver-rules`); our testharnessreport.js replaces the
tree's (the file WPT leaves to vendors). Each test waits for its
results as long as testharness's own timeout allows, times 4, plus
20 s; a test without results is a TIMEOUT whose message says what the
page was doing (its URL, an error page's being chrome-error://,
readyState, title, whether testharness.js ran, each fetched resource
with its status). Before the VM boots, the run checks the path: it
refuses to start when something already listens on a WPT port (a
stale server would answer for this tree), and runs the relay as QEMU
will, failing at once unless our testharnessreport.js comes back. The
server runs in a process group of its own, ended as a whole (its
servers are child processes that outlived the parent). A test whose
load gets no answer first asks the guest for a state dump (`nmi`
through QEMU's monitor; read-only: the `statedump:` lines in the
serial log show each thread's system call and wait at the hang), then
ends its session and the run goes on in a new
one, its Chrome on a profile directory of its own (`C:\tmp\chrw-<n>`,
as wptrunner starts each browser: the old Chrome may still be exiting,
and a new one on its profile would hand over to it and exit); if none can be made, the rest are SKIP ("not run: ...") in
the report, which is written either way. `XTASK_WPT_FILTER=<substring>` and
`XTASK_WPT_LIMIT=<n>` narrow the run (`XTASK_WPT_LIMIT=5 cargo xtask
boot-test 'chromium_wpt$'`, about 3 minutes); xtask fetches WPT_COMMIT
into `target/prebuilt/wpt` with the host's git. Each test's line
(`xtask: wpt: /dom/nodes/X.html: OK 9/9`) and the summary are in the
run's serve log (`target/debug/test-chromium_wpt.serve.log`, appended to
the scenario's log), the run as `target/wpt/wptreport.json` (wptrunner's format, what wpt.fyi
takes: `run_info`, each test's status, duration and subtests).
`cargo xtask wpt-compare` compares that report with wpt.fyi's pinned
reference runs (Edge 159 on Windows first, Chrome 159 on Linux; the
search API through the host's curl): per-test passes, the totals, and
every test that passes fewer subtests on gala than on Edge with gala's
first failing subtest (`target/wpt/compare.txt`).

`chromium_uia` (a UIA client over Chromium's accessibility tree:
user/uia_chromium_test, built by its build.sh) prints each step with
its time (`uia_chromium_test: link found (147659 ms)`); when the link
is not found it prints the window's element tree four levels deep
(`uia_chromium_test: tree <control type> "<name>"`) before failing.

`cargo xtask boot-test chromium_version` (PENDING; PREBUILT `chromium`:
370 MB download, ~430 MB unpacked, a 430 MB ISO per run: delete
`target/debug/test-chromium_*.iso` after a failed run). A run takes
about a minute under TCG. Read, in order:

- Chromium's own log lines in the serial log: `[pid:tid:date:LEVEL:file:line]`
  (`--enable-logging=stderr`; the scenario's `dump=` adds
  `C:\chromium\debug.log` and the profile's `chrome_debug.log` when they
  exist). Exit status 0xffff7001/7002 follows a FATAL; 21 is
  CHROME_RESULT_CODE_PROFILE_IN_USE (the process singleton: its mutex and
  lock file).
- `delay_load_failure_support.cc ... NOTREACHED`: a delay-loaded
  function or DLL is missing; the ledger's `missing export` / `missing
  dll` line names it. chrome.dll delay-loads most of what it uses, and
  `gala_stubs` stubs are invisible to GetProcAddress, so a stubbed delay
  import fails the same way. Rather than one function per run, list them
  all: `python3 -I tools/import_survey.py target/x86_64-pc-windows-msvc/release
  target/prebuilt/chromium/chrome-win/chrome.dll` prints every import
  (static and delay-loaded) gala's built DLLs lack or have only as stubs,
  API set names resolved as the loader resolves them.
- Imports by ordinal (`#N`) need the ordinal pinned in the DLL's
  build.rs (`/EXPORT:name,@N`, the number checked in Wine's .spec, not
  recalled): an unpinned DLL numbers its exports arbitrarily, so `#N`
  binds to an unrelated function and the survey cannot tell.
- A `Check failed` with an unresolved backtrace (`Symbols not available.
  Dumping unresolved backtrace`): symbolize it (below). chrome.dll is
  mapped at its system-wide alternative base, the same every run
  (`XTASK_EVENTS=image` gives it: 0x3ff7eb500000 so far); a frame's
  address in it is `address - base + 0x180000000` for llvm-symbolizer.
- `XTASK_EVENTS=syscall,image,fault,file,process,thread`: the `file`
  category (`"ev":"open"`, path, handle, status) shows each open, which
  `syscall` events cannot (their arguments are pointers).
- A crash report's `module+offset`: symbolize it with the snapshot's own
  PDBs (symbols are not disassembly: CLEAN_ROOM allows reading
  Chromium's source, and the PDB names the source line). The revision is
  pinned partly because its `chrome-win32-syms.zip` exists (922 MB): fetch
  only the member you need, `python3 -I tools/zipmember.py <the zip's URL>
  chrome-win32-syms/<module>.pdb target/prebuilt/chromium-syms/<module>.pdb`
  (HTTP range requests): chrome_elf.dll.pdb is 30 MB, chrome.exe.pdb 40 MB,
  chrome.dll.pdb 2.6 GB (about 900 MB downloaded; mind the 2 GB disk
  floor, and delete it when done). Put the PDB next to a link to the
  module and run `llvm-symbolizer --obj=chrome.dll 0x<ImageBase + offset>`
  (ImageBase 0x180000000 for the DLLs, 0x140000000 for chrome.exe); the
  first query loads the PDB (seconds).
- Crashpad's termination codes: 0xffff7001 the client found no handler
  and ended itself, 0xffff7002 the handler could not snapshot the
  crashed process (its reads of the process failed), 0xffff7003 a dump
  was asked of a client never registered.
- Chromium's source (github.com/chromium/chromium; crashpad on
  chromium.googlesource.com) says what a function expects: read it as
  documentation, as for any program.
- A `CHECK` in Crashpad's handler kills only the handler (the browser
  logs "crash server did not respond" and goes on until it needs it).
  The handler re-runs chrome.exe as a child with an inherited handle
  list, so child-process and handle-inheritance bugs show there first.

### Installing software (`gala install`, ROADMAP PK)

- **The logic** is `user/galapkg`, tested on the host: `cargo test -p
  galapkg` (seconds; `cargo xtask test` runs it with the unit tests).
  Its install tests run against `user/pk_install_test/source` through an
  in-memory machine and make every mutating call fail in turn: a new
  step in the transaction gets that coverage for free, so add steps
  through `install::Tx`, never with a bare `Host` call.
- **The test packages** are made by `python3 -I
  user/pk_install_test/make_source.py` (zips of `user/bin/hello.exe`,
  SHA-256s written into the manifests; `export-tests` reruns it).
  `manifests/b` and `manifests/j` are real winget-pkgs manifests (leave
  them; THIRD_PARTY.md).
- **Installer packages (Inno, NSIS, exe; `install::exe`)**: the NSIS
  test installers are built from `user/pk_install_test/nsis/pknsis.nsi`
  by make_source.py with `makensis` (`apt install nsis`; Debian's package
  has the `amd64-unicode` stubs, so `Target amd64-unicode` gives an x64
  installer, which gala can run: there is no WOW64 for 32-bit ones; it
  has no amd64 plug-ins, so the script uses none). The build is
  reproducible; without makensis the checked-in installers are written
  back from the working tree unchanged, so `export-tests` leaves the same
  bytes either way (make_source.py removes and remakes `source/installers`
  on every run, so keeping them means writing them back, not skipping
  them; it stops with a message if one is missing and it cannot build it).
  Third-party installers are test subjects: they run inside QEMU (see
  "Third-party code on the host"). The
  boot test is `cargo xtask boot-test pk_install_nsis`; a missing import
  of the installer stops its load before anything runs (exit 3/4 with
  the ledger naming it). The Fake host in `install/tests.rs` plays the
  installers (`run`), the NSIS uninstaller's late removal (`sleep`) and
  an Inno Setup installer (`inno_registers`).
- **Machine scope** is `cargo xtask boot-test pk_install_machine`
  (`pk_install_test machine`, booted `as=system`: SYSTEM is an
  administrator, the boot user is not, and the zip run checks the boot
  user's refusal, exit 6). Host tests: `cargo test -p galapkg
  machine_scope`. The install log says which layout and whether the
  caller was an administrator (`layout scope=`, `elevation`, `error
  requires-admin`).
- **A real Inno Setup installer** is `cargo xtask boot-test
  pk_install_inno` (about 30 s): Inno Setup 7.1.1's x64 installer (PREBUILT
  `innosetup`, put in the snapshot's `installers\` mirror by
  EXTRA_MODULES) with winget-pkgs' manifest
  (`manifests/j/JRSoftware/InnoSetup/7/7.1.1`, real: leave it). Inno's
  own log (`/LOG`, under `%LOCALAPPDATA%\gala\logs`) is teed into the
  serial log, and on a failure its last lines are in the install log
  (`setup-log:`). Before booting, list what Setup imports that gala
  lacks: `python3 -I tools/inno_setup_exe.py
  target/prebuilt/innosetup/innosetup-7.1.1-x64.exe <dir>/setup.tmp`
  (the program the loader unpacks to `%TEMP%\is-*.tmp`; never run it),
  then `tools/import_survey.py` on it. A delay import that is missing
  or only a stub ends Setup with 0xc06d007f (the ledger's `missing
  export` names it); Inno's source (github.com/jrsoftware/issrc) says
  what Setup expects of Windows, read as documentation (as CoreCLR's).
  Setup reports most failures in its log and goes on ("Defaulting to OK
  for suppressed message box" after an "Exception message:"), so a
  passing run can still skip steps: read the serial log's teed setup
  log for those, and the ledger. Setup's second stage
  (`<installer>.tmp`, run again by the first) is the one that installs;
  a Delphi exception naming a Windows error code (Setup's "System
  Error. Code: 1410") is the last error of the call that failed, and
  `XTASK_EVENTS=window` shows the windows the VCL made up to it (the
  next one is the control whose creation failed). A program Setup runs
  (`[Run]`: `ISIDE.exe /assoc`) that fails says why in a message box,
  which gala's user32 writes to the program's standard error: nowhere,
  for Setup's child. Setup logs only the exit code; run the program
  again from the test, with its output captured, as `inno_test` does
  when the `.iss` association is missing.
- **A vendor installer exits with an odd code and no ledger line**:
  boot with `XTASK_EVENTS=syscall,image` and list the installer's system
  calls (its pid from the `image` event; skip the memory calls and the
  reads): the failing one is usually near the end. NSIS exit codes: 1
  cancelled, 2 aborted (an error, silently in /S mode). The events file
  is large (18 MB for pk_install_nsis): delete it afterwards.
- **The boot test** is `cargo xtask boot-test pk_install_zip`; a failed
  run shows the install log twice (`logtee: install.log:` lines, and
  pk_install_test prints it after the failing check). Its checks of the
  log (`log_has`) scan the whole file, so a later check can find a line
  of an earlier command: give each check text only its own step writes
  (a URL, an id).
- **Which installer, and why**: the install log's `selected`,
  `runner-up`, `rejected` (with the resolver's reason) and `fallback`
  lines; `plan ...` for dependencies; `installed scope= type=` and
  `matched key= by=` for upgrades. A new test package's expected
  selection is winget's documented rule (ARCHITECTURE "The winget
  resolver"), not what gala happens to do.
- **Downloads** are `pk_install_https` (`pk_install_test https`, with
  schannel_test's Nginx): the served zip is
  `user/pk_install_test/served` (make_source.py writes it with the
  manifests). A failed download's install log has `fetch error
  step=<WinHTTP call> code=<error>` (12029 cannot connect, 12007 name
  not resolved, 12175 certificate, 12156 redirect refused) and the
  ledger the engine's reason (`winhttp.dll!TLS handshake`,
  `schannel.dll!ISC: ...`, `winhttp.dll!redirect: ...`). The test VMs
  have no internet: a download test points at the in-guest Nginx.
  Manifests over HTTPS come from the same Nginx
  (`--source https://localhost:8443/winget-pkgs`, the tree
  make_source.py writes to `user/pk_install_test/served/winget-pkgs`,
  listed as JSON by `schannel_test/nginx.conf`); a source's reads are
  the log's `source list|read url=` lines. GitHub's API (the default
  source) is host-tested only, through the Fake's `fetch`.
- **gala.exe is `no_std` with `alloc`** (gala_heap): code that brings in
  more of `alloc` may need another ntdll C function in main.rs's import
  list (`memmove`, `strlen`; the link error names it). advapi32 is
  loaded at run time (`pkg::Reg`): do not import it statically, or
  `gala`'s window commands break on images without it. Every installed
  command is a copy of gala.exe: a gala.exe change does not reach
  commands installed before it until they are reinstalled.

**The winget resolver** (`galapkg::winget`) is tested on the host with
`cargo test -p galapkg winget` (`-- --nocapture` prints the corpus
summary and every warning) and on the install path by `cargo test -p
galapkg install` and the pk_install scenarios.

- The corpus is `user/galapkg/testdata/winget-corpus/manifests` (real
  manifests: never edit them; `.gitattributes` keeps their CRLFs).
  `selected.txt` there is the golden answer of installer selection for 8
  machine profiles; after a deliberate rule change regenerate it with
  `GALAPKG_BLESS=1 cargo test -p galapkg winget::corpus`, read the diff
  (each changed line is a package whose installer moved) and say why in
  the commit. The test fails with the first differing golden lines; a
  manifest that does not read is listed with `file:line:col`.
- Refreshing the corpus (rarely; keep it under about 3 MB): outside the
  repository, `git clone --depth 1 --filter=blob:none --sparse
  https://github.com/microsoft/winget-pkgs <dir>`, `git -C <dir> ls-tree
  -r HEAD --name-only manifests > paths.txt`, `python3 -I
  user/galapkg/testdata/winget-corpus/pick.py paths.txt 400 > pick.txt`,
  `git -C <dir> sparse-checkout set --no-cone --stdin < pick.txt`, copy
  `<dir>/manifests` over the corpus's, bless the golden, and record the
  new commit in THIRD_PARTY.md. A manifest that fails to read is a
  parser bug to fix (winget-pkgs is validated), not a file to drop.

**MSI and cabinet fixtures** (`user/galapkg/testdata/msi`):

- `sh user/galapkg/testdata/msi/make_fixtures.sh` rebuilds `basic.msi`
  (from `basic.wxs` with wixl), `long.msi.zlib` (a generated package with
  a 70 KB string, zlib-compressed because it is mostly one letter),
  `mszip.cab` and `stored.cab` (gcab), the install-plan packages
  `plan.msi` (`wixl -a x64`), `dual.msi` and `badcond.msi`, and rewrites
  `expected/` from msiinfo, msiextract, gcab and 7z. Commit all of it
  together; the MSI's package code and dates change on every run (the
  tests read them from `expected/`), so restore `basic.msi`,
  `long.msi.zlib` and `expected/basic.suminfo` with `git checkout`
  unless they were what you changed.
- wixl's limits: it lacks some WiX attributes (VBScriptCall, ShortName;
  Product/@Codepage is ignored: its pools have code page 0 with UTF-8
  strings); it miswrites a string longer than 128 KiB (its own reader
  rejects it, "string table load failed"), so keep long-string fixtures
  between 64 and 128 KiB; it writes no `Condition` under Feature or
  Component, no `Directory` on a CustomAction, no `MultiStringValue`
  (write the `[~]` value as a string), no `Package/@Platform` (use
  `wixl -a x64`); it writes HKMU as Root 4 instead of -1, drops the
  `#x`/`#%` prefixes of binary and expandable values and eats `$` in
  formatted text (`[$Comp]` becomes `[Comp]`). make_fixtures.sh fixes
  all of these afterwards with `msibuild <msi> -q "<SQL>"` (UPDATE,
  INSERT, CREATE TABLE work; quote names with backquotes); the test
  `plan_packages_read_as_msiinfo_exports_them` checks the reader against
  msiinfo after that rewrite.
- `msiinfo export` writes a `Binary/` directory into the current
  directory (the script runs it in its temporary directory). gcab writes
  only uncompressed and MSZIP cabinets.
- The install plan (`msi::plan`) has no msiexec to compare with: its
  tests' expected values are the SDK's documented rules, each test
  naming its page. When a Windows run becomes possible, record `msiexec
  /i x.msi /qn /l*v log` of the same fixtures and compare.
- The readers' own tests build edge cases with small writers
  (`cfb::tests::build`, `cab::tests::build_with`); 7-Zip reads what they
  make (`7z t`), keep it so.

**LZX** (`user/galapkg/testdata/lzx`): no tool on Linux writes LZX, so
`sh user/galapkg/testdata/lzx/make_fixtures.sh` makes the cabinets with
the test encoder (`src/lzx/encode.rs`, through the ignored test
`lzx::tests::write_fixtures`) and records cabextract's and 7-Zip's
decoding (`apt-get install -y cabextract`); the test
`fixtures_match_cabextract_and_7z` also checks the encoder still makes
the checked-in files byte for byte (change the encoder: rerun the
script). `sh user/galapkg/testdata/lzx/fetch_real.sh` fetches a cabinet
Microsoft made (inside the DirectX web installer, not committed) into
`target/galapkg-fixtures`; `microsoft_cabinet_matches_cabextract`
decodes it when present (it skips otherwise, so run the fetch before
changing `lzx.rs`). Do not read cabextract's, libmspack's, 7-Zip's or
Wine's source: black-box outputs only.

**MSIX fixtures** (`user/galapkg/testdata/msix/`): regenerate with
`python3 -I user/galapkg/testdata/msix/make_fixtures.py`, then run `sh
user/galapkg/testdata/msix/check_fixtures.sh` (unzip, 7z, xmllint,
sha256sum: the independent cross-check) and update the hashes, line
numbers and sizes the `msix::tests` expect. Another zlib version may
deflate differently (the fixtures are checked in; only a regeneration
changes them); the generator refuses a fixture over 100 KB; the
corrupted fixtures must stay valid zips where the test says so
(bad_hash keeps the changed data's CRC-32 and block sizes, only the hash
disagrees), or the reader reports a different error first; shell
functions in check_fixtures.sh share variables with their callers. The
test publisher's id and the block-run rules come from our reading of
the documentation, not from makeappx (ARCHITECTURE "MSIX/AppX reading"
says which values are independently verified).

## Tools

### Screenshots

`cargo xtask screenshot [--uefi] [--at 1,3,5] [--name N] [--scenario S] [kernel cmdline...]`
boots an ISO (all test programs and DLLs as boot modules, ntdll in
System32; BIOS unless `--uefi`) and writes
`target/screenshots/<name>-<t>s.png` at each time (default 3 s) through
QEMU's monitor (`screendump`; a headless desktop, `presenter=test`, has
nothing on the screen: its frame is what screen checks read). `--scenario S` adds S's files from the
repository (scripts, archives). The kernel's screen console shows the
same log as the serial port (green header bar; warnings yellow, errors
and panics red). The boot splash replaces the text console: add
`bootlog` to see the log.

    cargo xtask screenshot --name demo --at 6 run=hello.exe,smp_test.exe args=,cpus=2
    cargo xtask screenshot --name tcc --scenario tcc --at 16 run=busybox.exe "args=sh%20C:/gala/tcc.sh%20C:/gala/tcc.tar"

#### Watching the screen live

The pinned QEMU is built `--disable-gtk --disable-sdl`, so its only
display backends are `none` and `dbus` and it opens no window of its
own. VNC is compiled in, though, and `XTASK_QEMU_ARGS` is appended
after everything else, so a `-vnc` there wins over the `-display none`
that `run --headless` adds:

    XTASK_QEMU_ARGS="-vnc 127.0.0.1:1" cargo xtask run --headless

keeps the machine up (serial log on stdio, Ctrl-C to stop) with a VNC
server on port 5901. Unlike a screendump this takes mouse and keyboard,
so the desktop can be driven by hand.

From Windows over WSL2, `localhost:5901` reaches it: localhost
forwarding covers a port bound to the loopback address, so there is no
need to listen on every interface. Windows has no VNC client of its own
(`winget install TigerVNC.TigerVNC`, then `"C:\Program
Files\TigerVNC\vncviewer.exe" localhost:5901`). Prefer the loopback
form to `-vnc :1`, which listens on every interface with no password:
under WSL's default NAT only the host can reach that, but mirrored
networking would put it on the network.

#### Screen checks

A scenario in `SCREEN_SCENARIOS` gets a QEMU monitor socket; when its
serial log shows the trigger line (a program's can be the desktop's
`desktop: shown window '<title>'`), xtask runs the check's steps and
logs `xtask: screen ok` or the mismatch; a mismatch (or a step's
`after` line not coming within 120 s) ends the run at once, after the
guest's state dump, and is the scenario's verdict. Such scenarios must boot by
UEFI (BIOS boots have no framebuffer), unless their desktop is headless
(`presenter=test`, below).

- A step first sends QEMU monitor commands, 250 ms apart: `mouse_move dx
  dy`, `mouse_button 1` (bit 0 the left button, bit 1 the right:
  `mouse_button 2` is a right press), `sendkey`. Then it takes a
  screendump (none when the step checks no pixels, e.g. the click that
  ends the program), kept as `target/debug/test-<scenario>.screen<step>.png`,
  and checks pixels (exact colours, or "some pixel near this colour in
  this rectangle" for anti-aliased text).
- A pixel check is retried (a new screendump every 0.5 s, for about
  30 s) before it fails: a redraw under load can outlast the first
  dump, and a step may wait for the program.
- The guest's pointer starts in the middle of the screen and is clamped
  to it: `mouse_move -255 -255` three times puts it at (0, 0) whatever
  the earlier steps did. Keep each `mouse_move` within 255 per axis:
  QEMU queues PS/2 packets of at most 255 and hands the rest of a larger
  move over with later events (a click then lands elsewhere).
- A mouse press is sent 0.75 s after the command before it, so queued
  moves are drained first. Typed input is queued too: check for the
  final state.
- Windows-key chords are one `sendkey` (`sendkey meta_l-right`: Snap).
  The wheel: `mouse_move 0 0 1` is one notch up (away from the user; a
  negative dz is down).
- When a click seems to do nothing, give that step a pixel check that
  always passes to get its screendump; if the click landed, look at how
  the program waits (wx's modal loops wait in MsgWaitForMultipleObjects,
  which must wake for the input): the hang dump shows each thread's
  system call.
- A dump is taken as soon as its pixels match, which may be while the
  program is still repainting. Check the final state with pixels that
  only it has (text in the new place); to tell a wrong layout from a
  late repaint, probe SetWindowPos (the sizes the program set) and
  present a window's whole buffer again a few seconds later: if the dump
  is right then, the layout was. The desktop redraws a damage
  rectangle row by row from the top, slowly under TCG: a dump during a
  window move shows the moved window cut off below some row. Check a
  pixel the redraw reaches last (the window's bottom-right corner).
- A screen check pins a wallpaper with `wallpaper=<name>` (the default
  is a random one).
- **Headless screen checks**: a scenario whose command line has
  `presenter=test` shows the console session's desktop in a frame in
  the guest's memory (1280 x 800, no screen needed: it may boot by BIOS,
  `gui_test_headless`); each step reads the frame with the monitor's
  `memsave` at the address of the serial log's `presenter: test frame
  for session 1:` line instead of a screendump (kept the same way as
  `screen<n>.png`). Input is the same (PS/2 through the monitor). A
  failed read quotes the monitor's reply (`memsave`'s file name must be
  quoted: unquoted, its slashes are read as part of the size).
- **Each desktop keeps its own pointer** (a session's, across console
  switches and injected input): a step after a switch first puts the
  pointer at (0, 0) with four `mouse_move -255 -255` (presenter_test).
  A step whose input makes the program change the screen (a click that
  starts a console switch) checks no pixels: its dump would race the
  change; check them in a step before it, and let a later input (a
  second click) tell the program a check is done.

#### Fonts, icons and wallpapers

The desktop's text and pictures are baked at build time, so the kernel
has no font engine or SVG renderer (both are xtask host dependencies).
Rerun the command and commit its output after changing a source.

- `cargo xtask bake-fonts` rasterizes Selawik (`fonts/selawik/`) with
  fontdue into `kernel/assets/ui-<px>-<weight>.glyphs` (12 regular and
  semibold, 14 and 20 semibold, 28 semilight; chars 0x20-0xFF). The
  format is in `bake_fonts`; `paint.rs` reads it. Add a size there and
  in `paint::Font` together.
- `cargo xtask bake-icons` renders `kernel/assets/fortress.svg` (24 and
  96 px, square) and `wordmark.svg` (112 px wide, cropped) with resvg
  into `.alpha` coverage masks; the compositor tints them.
- Wallpapers are QOI files in `kernel/assets/wallpapers/` (provenance
  and conversion in its README.md); scenarios that use the screen list
  them in `EXTRA_MODULES` (`Windows/Web/Wallpaper`).

### Serving to a browser

`cargo xtask serve <scenario> [port]` boots a network scenario (nginx,
nginx_master, nginx_tls, go_http, ...) as its boot test does, but
forwards a fixed host port (default 8080) to the guest's port 7 and
keeps the VM running, with the serial log on stdout. Then open
`http://127.0.0.1:8080/` (or `https://` for nginx_tls; the certificate
is self-signed). For a picture, Playwright's Chromium is preinstalled in
the cloud container: `chromium.launch({args: ['--no-proxy-server']})`,
because the agent proxy would otherwise intercept 127.0.0.1.

### SSH into a running gala

`cargo xtask serve sshd [port]` boots the `sshd_server` scenario (sshd
as SYSTEM on its port 22, host keys made at boot, logs teed to the
serial log) and forwards host port 2222 (or the one given) to the
guest's 22. Then, from another terminal:

```sh
cp user/sshd/client_ed25519 /tmp/gala_key && chmod 600 /tmp/gala_key
ssh -i /tmp/gala_key -p 2222 -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null galatest@127.0.0.1
```

(or galatest's password `Gala-Test-1`). The shell is busybox sh
(`user/sshd/sshd.regseed`: DefaultShell, DefaultShellArguments) in a
pseudo console; `sftp -P 2222 ...` works the same way. The serial log
shows `logtee: sshd.log:` / `sshd-session.log:` lines for each
connection; add `conpty=trace` to the scenario's command line to see
every byte between the terminal and the console.

### Benchmarks

- The `nt:` line at the end of a scenario's serial log gives the NT
  layer's system call, fault and process creation time without
  disturbing the run; the event stream gives per-call detail but slows
  the run down (docs/PERF.md has the method and the numbers).
- `cargo xtask bench --release` runs `user/bench` (without `--release`
  it benchmarks a debug kernel, about 30x slower per system call) and
  writes `target/report/bench.json`; on Windows, `user/bin/run-all.ps1`
  writes `bench-windows.txt`, which `cargo xtask bench bench-windows.txt`
  puts next to gala's numbers.
- Inside the kernel: `XTASK_EVENTS=syscall cargo xtask boot-test bench`,
  then group the `events.jsonl` lines by `name` and look at `ns`.
- Under QEMU TCG, user code in shared physical frames is translated
  once: copying code pages per process costs far more than the copy.

### OpenTelemetry

`cargo xtask otel` converts a boot's event stream and serial log to
OpenTelemetry (OTLP/JSON files; ROADMAP OT1; the mapping table is in OBSERVABILITY
"OpenTelemetry export"): the boot is one trace, processes and threads are
spans, system calls spans under their thread, faults, image loads and
other events span events, the serial log log records (a crash report or
state dump one record each), `stats:` lines metrics.

```sh
XTASK_EVENTS=all cargo xtask boot-test 'crash$'   # spans need the stream
cargo xtask otel crash                            # newest log of the scenario
cargo xtask otel target/failed/java_http.log      # a kept failure
cargo xtask otel target/debug/batch-x.serial.log  # a shared VM
XTASK_EVENTS=all XTASK_OTEL=1 cargo xtask boot-test 'hello$'  # every boot
```

- It writes `<log stem>.otlp.traces.json`, `.logs.json` and
  `.metrics.json` next to the log (`--out <prefix>` elsewhere; for
  boot-test `target/<profile>/<image>.otlp.*`). Each line is one
  OTLP/JSON request (`ExportTraceServiceRequest` etc., at most 5000
  spans or records), so a line is a valid body for `POST /v1/traces`.
- It prints `otel:` lines: what each event kind became, what was NOT
  mapped and the gaps in the stream; read them before trusting an empty
  view. Without `XTASK_EVENTS` a boot has only the boot span, log records
  and metrics.
- **Sending**: `--endpoint http://host:4318` or
  `OTEL_EXPORTER_OTLP_ENDPOINT=http://host:4318` (also honoured by
  `XTASK_OTEL=1` boot tests) POSTs it over OTLP/HTTP (plain http only):
  binary protobuf, gzipped, at most 512 spans, records or points a
  request (OBSERVABILITY "OpenTelemetry export");
  `OTEL_EXPORTER_OTLP_PROTOCOL=http/json` sends OTLP/JSON instead and
  `OTEL_EXPORTER_OTLP_COMPRESSION=none` leaves gzip out (to read the
  requests in a capture). The `otel: sent to` line says per signal the
  requests, items and bytes, the retries (and why the last one
  happened), drops, and anything FAILED, NOT SENT or REJECTED.
- **Large boots**: a system-call-heavy boot (a JVM) is hundreds of
  thousands of spans; Jaeger's UI gets slow past about 50k spans in one
  trace. Convert a shorter run, or filter the stream first (`grep -v
  '"ev":"syscall"'` keeps lifetimes and events only).

**Viewing in Jaeger** (traces only; Jaeger 1.35+ takes OTLP by default).
This recipe has not yet been run against a real Jaeger (the output was
checked against the OTLP protobuf messages with python's strict
`json_format`, and xtask's unit tests check the shape, decode the
protobuf back against the JSON, and check the batched, gzipped, retried
posts): fix it if a step differs.

```sh
docker run --rm -p 16686:16686 -p 4318:4318 jaegertracing/all-in-one:1.62.0
# or the binary from github.com/jaegertracing/jaeger/releases:
#   ./jaeger-all-in-one          (UI :16686, OTLP/HTTP :4318)
cargo xtask otel crash --endpoint http://localhost:4318
# or post the file by hand, one request per line:
while read -r l; do curl -sS -H 'Content-Type: application/json' \
  --data "$l" http://localhost:4318/v1/traces; done < target/debug/test-crash.otlp.traces.json
```

Open http://localhost:16686, service `gala-os`, operation `boot crash`
(or search by tag `gala.scenario=crash`, `process.pid=4`,
`gala.syscall.status=0xc0000034`). The trace view shows processes,
threads and system calls nested in time; span events (faults, image
loads, the crash's `exception`) are under each span's "Logs". Logs and
metrics need a collector.

**With the OpenTelemetry collector** (otelcol-contrib), reading the files
directly (the `otlpjsonfile` receiver reads this one-request-per-line
format):

```yaml
receivers:
  otlpjsonfile:
    include: [target/debug/*.otlp.*.json]
    start_at: beginning
exporters:
  debug: {verbosity: detailed}
  otlphttp/jaeger: {endpoint: http://localhost:4318}
service:
  pipelines:
    traces: {receivers: [otlpjsonfile], exporters: [debug, otlphttp/jaeger]}
    logs: {receivers: [otlpjsonfile], exporters: [debug]}
    metrics: {receivers: [otlpjsonfile], exporters: [debug]}
```

`otelcol-contrib --config otel.yaml`; or an `otlp`/`otlphttp` receiver
on :4318 and `--endpoint` pointed at it, which fans out to Tempo, Loki,
Prometheus or Grafana.

## Debugging and troubleshooting

### When a test fails (read this first)

- **Start at the end of the failed log**: `target/failed/<scenario>.log`
  (also printed to the console) ends with a `===== triage` block: the
  verdict, how long it ran, the host's CPUs, load and free disk, the
  guest's CPUs and memory (heap peak, fewest free frames), warning and
  error counts and the first error, panics, CPU exceptions, watchdog,
  leak, out-of-memory and missing-function lines, programs that began
  and did not end, processes that failed, the ledger's top five, the
  state dump's CPUs, locks and threads, each thread's last system calls
  and the last lines. `cargo xtask triage <log> [events.jsonl]` prints it
  for any log (a kept serial log, a CI artifact); the event stream next
  to the log is found by name.
- **A timeout comes with a state dump**: before killing a timed-out VM,
  xtask injects an NMI and the kernel prints `statedump:` lines: per CPU
  the rip (user or kernel) and a kernel backtrace (symbolized below the
  log as `statedump: cpu N backtrace (symbolized)`), the thread each CPU
  runs, `statedump: locks: ...` (a lock still HELD after 100 ms of
  trying is what everything waits for), heap and frames, the flight
  recorder (system calls by name with pid and tid); then from the boot
  thread every NT thread's system call and wait object (with its signal
  state and timeout), every scheduler thread, the ledger,
  `statedump: end`. xtask's last line says how far it got: "did not
  answer the NMI" means the guest was wedged or not yet past early boot;
  "NMI part came, its threads part did not" means the boot thread could
  not run (the scheduler or a lock is stuck: look at the locks line and
  the CPUs' backtraces). To get the dump of a hang now rather than at
  the timeout: `XTASK_TIMEOUT=30 cargo xtask boot-test <name>`.
- **A program that crashed** has `crash report:` lines before its exit
  line: the exception (code and name), the thread, the address as
  `module+offset`, what it read, wrote or executed, the registers and the
  top of the stack with module offsets; look the offset up in that
  module's disassembly (`llvm-objdump -d --start-address=...`).
- **A kernel panic** prints `backtrace:` return addresses (complete:
  frame pointers); failed boot tests show them symbolized (function and
  source line) and the failed log keeps the symbolized copy at its end
  (addresses move with the next build; `cargo xtask symbolize <log>`
  only fits the build that ran).
- **Is it flaky?** `cargo xtask test`/`boot-test` keep every scenario's
  outcome in `target/report/history.jsonl` and end with `xtask: health:`
  lines for this run's scenarios that both passed and failed in their
  last 30 runs, or got much slower. A known-flaky scenario's failure is
  still a bug; its history line has the reason of each failure (`jq
  'select(.scenario=="java_http")' target/report/history.jsonl`).
- **The event stream**: `XTASK_EVENTS=all cargo xtask boot-test <name>`,
  then `cargo xtask events target/debug/test-<name>.events.jsonl` (or
  the copy in `target/failed/`) for system calls by time, failing
  statuses, each thread's last calls, faults and images. In a trace
  viewer: `cargo xtask otel <scenario or log>` ("OpenTelemetry").
- **"the disk is (nearly) full"** from xtask: the message says how much
  is free and what to delete ("Disk space").
- **A watchdog line** (`watchdog: CPU N has taken no timer interrupt`)
  is followed by that CPU's own backtrace (`watchdog: CPU N backtrace:`,
  symbolized at the end of the failed log). Watchdogs right after boot
  on a loaded host: check the clock first.
- **Is the guest's clock right?** Look for `xtask: clock:` lines (xtask
  timed the guest's `uptime_ms=` marks against the host, in a thread of
  their own, each within its poll window: `guest 5118 ms over host
  5096..5136 ms (+0.0% of the middle)` is fine, `xtask: clock: WRONG` is
  not; under TCG the right guest clock is the host's, starved or not),
  the kernel's `tsc:` line (which
  reference, how many samples were rejected, the spread) and `clock: RTC
  check: agrees` / `clock: WRONG` (the kernel's own check against the
  CMOS RTC a few seconds into the boot, which recalibrates). A wrong
  clock makes sleeps end early, timeouts and watchdogs fire, and timing
  checks fail; the verdict then ends with "the guest's clock was wrong".
  A program that wants its own timing checked prints `<name>: ...
  uptime_ms=<n>` at two points (clockcal_test does).
  `HOST_LOAD_SCENARIOS` run with a busy thread on every host CPU; the
  kernel flags `calstall` (stalls the boot CPU inside the calibration)
  and `calwrong` (makes the first calibration 20x low; the RTC check
  must catch it) test this.
- **A host loaded far past its CPUs** (load 25-35 on 4 CPUs, several
  agents' suites at once): the clock is right but the handover
  scenarios' sleep checks (handover_test 0x16/0x32, handover_k32_test
  14/58) can still fail when the handover itself takes longer than the
  sleep's slack (`resumed, N ms after the record` in the log): that is
  load, not the clock; rerun alone.
- **A corruption only the kernel shows: eliminate layers, then suspect
  the emulator** (the method that found the JVM heap corruption):
  1. Run the same program in gala-um (same `nt` code and DLLs, real
     processors): hundreds of clean runs there put the fault below the
     NT layer (paging, TLB shootdowns, fault and exception entry,
     context switches, interrupts) or in the emulator.
  2. Make a reproducer that fails often (`java_stress`: one boot in
     three) and repeat it (`XTASK_REPEAT=n XTASK_REPEAT_JOBS=k`).
  3. Turn features off one at a time and record each batch: `-smp 1`
     (not a cross-processor race), `tlbparanoid` (not a missed flush),
     `-cpu Nehalem` (not XSAVE or a CPU extension), `eagercommit` (not
     demand-zero faults), `checkuser` (return-to-user invariants), the
     program's own verification (HotSpot's `-XX:+VerifyBeforeGC
     -XX:+VerifyAfterGC`). A failure that survives all of them, with the
     state consistent at one point and stale just after, points at
     stale translated code.
  4. Write a focused test of the suspected emulator behaviour
     (`xmodify_test`, `straddle_test`) and try other QEMU versions.
  5. Fix the cause (here: pin a fixed QEMU), not with a kernel
     workaround.

### Known intermittent

Scenarios that fail now and then, and what is known.

- **Java heap corruption on QEMU 8.1.0-8.2.5**: java_http/java_hello
  failing about 1 run in 30-40 with a stale String reference
  (`String.equals` reading a bogus klass, a NullPointerException inside
  String.equals from javac's MemberName.<init>, a crash in
  MethodHandleNatives.resolve; the hs_err report in the failed log's
  `dump:` lines) is QEMU's TCG bug: `tb_record` registered a block that
  spans two pages under its first page only, so a write to the second
  page did not invalidate it, and HotSpot's patched `mov r64, imm64`
  constants straddling a page end kept their pre-GC values. Fixed in
  QEMU 8.2.6. If it shows, check which QEMU the run used ("QEMU").
- **Multi-threaded TCG and code rewritten by another processor** (still
  open in QEMU 10.2, QEMU issue 4669): it can keep a translation of
  guest code made while another processor was writing it and run the
  old instructions after the write, on every processor, until something
  else invalidates the page. `xmodify_test` shows it (with `-accel
  tcg,thread=multi` its writer called a function it had just rewritten
  and got a version thousands of rewrites old, about 1 boot in 10). A
  JIT patches code that other threads run (inline caches, a GC
  rewriting embedded object addresses), so the `SINGLE_THREAD_TCG`
  scenarios (java_*, elasticsearch, dotnet_*, pwsh_*, flaui*,
  xmodify_test, straddle_test) boot with `-accel tcg,thread=single`
  when there is no KVM: both processors emulated by one host thread,
  about half as fast, so their time limits are doubled. A new program
  that generates or patches code (V8, LuaJIT, ...) belongs in that
  list.
- **java_http timing out with a lost wake-up's signature**: once, a
  state dump showed three JVM threads in NtYieldExecution and two
  waiting on semaphores with count 0, the others in delays or waits on
  events: the signature of a lost semaphore release at HotSpot's wait
  barrier, seen after the NtReleaseSemaphore fix (the waits on events
  may have been the lost wake-up in `nt::wait` fixed since: a
  notification event reset by one woken waiter before another looked).
  Open; whether it recurs on the pinned QEMU is not recorded.
  `target/failed/java_http.log` is the starting point if it does.
- **Watchdogs firing during boot**: look first at the `apic:` / `tsc:`
  lines (the triage shows their figures near the guest line). A TSC
  calibrated at tens of MHz instead of about 2.8 GHz makes guest time
  run 20-200x fast: sleeps end early and the watchdog reports CPUs
  "stuck" for "5 s" every quarter second. The triage flags a TSC under
  500 MHz and `clock: WRONG`.
- **Scenarios cut short by host load**: limits count run time
  ("Scenarios"), so a starved VM gets its time; a timeout's `xtask: vm
  time:` line says how much it had. On a shared host, `XTASK_VMS=1 nice
  -n 19 cargo xtask test` boots one VM at a time (the default is one per
  host CPU).
- **timer_test** failed once in three full runs under the load of the
  full suite and passes alone; if it recurs, its log says which bound
  was missed.

#### Reproducing the Java corruption: `java_stress`

`java_stress` (in `PENDING`) runs `user/java/Stress.java`: javac
in-process 48 times, three at a time. On an affected QEMU about one boot
in three fails (javac's NullPointerException in `BaseFileManager.getKind`
or `ClassSymbol.setAnnotationTypeMetadata`, a crash in String.equals or
in the GC, the hs_err in the log). `XTASK_REPEAT=6 XTASK_REPEAT_JOBS=2
cargo xtask boot-test java_stress` (each boot 5-15 min under load; its
limit is 900 s, doubled without KVM). Other JVM options replace the
scenario's through the last `args=`:
`XTASK_CMDLINE="args=-Xmx64m%20C:/gala/Stress.java%2048"` (a second
argument, ms, forces a full collection that often). The same program in
gala-um (`um --dir=jdk/bin ... java.exe "C:/gala/Stress.java 48"`, about
25 s) has never failed.

### Flaky tests and repeats

Flaky boot tests usually mean a race; rerun the one scenario a few times
before blaming CI:

- `XTASK_REPEAT=12 cargo xtask boot-test name` boots one scenario's
  image 12 times, one VM per two host CPUs at once (`XTASK_REPEAT_JOBS`
  for more: the load brings races out, but a JVM then runs into its
  time limit), keeps each failed boot's log
  (`target/debug/test-<name>.r<i>.serial.log`, its triage in
  `target/failed/<name>.r<i>.log`) and ends with "n of 12 boots failed".
  With `XTASK_REPEAT` a scenario's exact name selects it alone
  (`handover_upgrade`, not also `handover_upgrade_uefi`), and the
  filter must select one scenario of its own VM.
- `XTASK_QEMU_ARGS="-accel tcg,thread=single"` boots any scenario
  without host-level parallelism (to tell a guest race from an emulator
  one); `-accel tcg,thread=multi` boots a JIT scenario multi-threaded.
- A nondeterministic failure in one component can be another thread
  damaging shared state, not a race in that component: rule out the
  file system before the code. A "missing dll" ledger line for a DLL the
  image has, or opens failing with STATUS_OBJECT_PATH_NOT_FOUND below
  `C:\Windows`, means something deleted it (a program's cleanup given a
  path gala answered wrongly: an empty path once named the current
  directory, and ODT's setup.exe then deleted `\*`). `events=file,syscall`
  shows NtSetInformationFile class 13 (delete) per handle; the event
  stream slows opens enough to hide such a race, so the ledger (which
  costs nothing) is the first look.

### Hunting a rare hang

- Loop `cargo xtask boot-test <name>` (or `XTASK_REPEAT`) until it
  fails and keep that serial log. After a minute without the program
  finishing the kernel dumps every thread (ARCHITECTURE "Diagnostics of
  stuck and crashed programs"):
  which system call each NT thread is in, on which object, with which
  timeout; at the boot test's timeout xtask asks for a state dump too.
  Tracing (`trace`) changes the timing enough to hide races, so prefer
  the dumps.
- After the first minute the kernel also prints what the ledger newly
  recorded every 30 seconds, so gaps that a screen check's input reaches
  late still show.
- In gala-um, catch a hung instance live and `gdb -p` the `exe` process
  (`thread apply all bt`).
- **A program hangs without a ledger** (it never exits, so the ledger is
  not printed): add `events=syscall,thread` to its scenario's command
  line and read `target/debug/test-<name>.events.jsonl` while QEMU still
  runs (`grep -v` the calls it repeats; a loop of time queries means it
  waits for a clock to move). Waiting services log a `"ev":"wait"` event
  when they start, so a wait that never returns shows as the thread's
  last line. Do not delete the events file of a running VM (it is still
  open: `/proc/<qemu pid>/fd/` reaches it).
- **A server hangs mid-conversation**: `XTASK_EVENTS=syscall` shows
  which call pended.
- **A wait that ends only at its timeout** although the object was set:
  look for someone resetting it before the waiter looked (ARCHITECTURE
  "Waits").
- **Where a thread was when it was terminated**: log `regs.rip` in
  `on_user_return` (kernel/src/nt/thread.rs) and look the address up in
  the DLL (`llvm-objdump -d` on our own build in
  `target/x86_64-pc-windows-msvc/release/`).
- **"run: address space still referenced N times after exit"** is
  preceded by the holders: the threads not reaped (state, CPU), the
  references reaps in progress hold, and the process's own references
  (thread objects, handles in other processes). Count them against N:
  what is left over is held by something else (a clone of
  `Process::space`).

### Programs that spin or fail silently (probes)

- **A thread spinning in user mode** (no system calls): the one-minute
  hang dump prints `in user mode at rip ...` (the last timer tick that
  caught it) and the ledger so far; map the rip to a module with
  `events=image`, then `llvm-objdump -d` the DLL around it (gala's DLLs
  name their exports). In gala-um: `gdb -p` the `exe` process; `thread
  apply all x/3i $pc` shows the loop, and the image's export directory
  (read from memory at the 64 KiB-aligned base below the address) names
  the DLL. A GUI program that hangs or spins often waits for something
  gala lacks: the same dump prints the missing-features ledger, and an
  unregistered window class shows there as
  `user32.dll!CreateWindowExW(<class>)`.
- **Probes in gala's DLLs**: a temporary `unsupported(b"PROBE", detail)`
  shows up in `events=syscall` as GalaRecordMissing with its detail.
  Probes are told apart by their name's length (args[1] in the event:
  the event has the name's pointer and length, not its text) and their
  detail (args[3]). When `events=syscall` slows a screen scenario too
  much, write the probe's sequence number and detail into its name
  instead (a stack buffer, e.g. `0012 PROBE_SWP <hex>`): each call is
  then its own ledger line, in order once sorted, without the event
  stream. Put `GetTickCount` differences in the detail to see how long
  things take.
- **A program fails without a ledger entry or a failing system call** (a
  function returned data it did not like): add temporary probes,
  `missing_detail(b"TRACE dll!Function", detail)` at the top of the
  suspect DLL functions (name, class or argument as the detail, or the
  argument text in the name), run, read the `TRACE` ledger lines.
- **Tracing exception dispatch in a third-party program**: a temporary
  `GalaRecordMissing` call in gala's ntdll dispatcher (`seh.rs`: code and
  address in the detail; one per walked frame with its pc and handler)
  shows up in the `events=syscall` stream, in order and per thread; map
  addresses to modules with `events=image` (base and size per process).
- **User faults**: the `trace` flag also logs every user fault (vector,
  rip, cr2), which a system call trace cannot show; an unhandled one ends
  the process with its status through ntdll's dispatcher.
- Remove every probe before committing.

### Fault injection

`cargo xtask fault-test` runs `FAULT_PROGRAMS` in gala-um under
`FAULT_RULES` (`heap:every:2`..`heap:every:499`, `heap:random:20`,
`frame:every:3`, `frame:every:61`, `frame:random:20`, `net_tx:every:3`, a
mixed rule; seeds 1-3; ARCHITECTURE "Fault injection") and sweeps
`FAULT_SWEEPS`: about 3200 runs in about 40 s. Each run must end without a
panic, an abort, a hang (10 s) or a valid image reported as a bad one.
`--sweep` fails each heap allocation of each program in turn
(`heap:nth:k`), `--sweep=frame` each frame allocation, `--sweep=net_tx`
each packet; a filter picks programs (`cargo xtask fault-test pipe_test
--sweep`; a full heap sweep is about 14,800 runs in four minutes, a frame
sweep about 3,500). A failed run's whole log is in
`target/failed/fault-<program>-<rule>-<seed>.log`. In QEMU: the
`failpoint_*` boot scenarios, or `failpoint=heap:every:97 failseed=7` on
any command line (debug builds). Add a program to `FAULT_PROGRAMS` once
its sweep passes.

To find the code behind a failure, rerun the one rule by hand with
`--failtrace` (kernel: `failtrace`): each injected failure prints a
backtrace, and the program's own symptom (a status, a hang) follows:

    target/debug/um --failtrace --trace --failpoint=heap:nth:976 \
        --module=Windows/System32/ntdll.dll=user/bin/testntdll.dll ... \
        user/bin/pipe_test.exe

- **"KERNEL PANIC: memory allocation of N bytes failed"** is an
  infallible allocation: alloc's `handle_alloc_error` panics. Skip the
  panic and liballoc frames of the backtrace (`handle_error`,
  `with_capacity_in`, `to_vec`, `Clone::clone`): the first kernel frame
  is the culprit. Then run `cargo xtask alloc-audit` (also part of
  `cargo xtask test`): it disassembles the debug kernel and lists every
  kernel function that calls library code which can abort on
  out-of-memory, which clippy cannot see by name (`Clone::clone` of a
  Vec/String/Box, `Extend`, generic code); growth after `try_reserve`
  (`extend`, `resize`, an inlined `try_push`) is excused (`--all` lists
  it too). Fix with the fallible APIs (`mm::heap::try_to_vec` for a
  copy) and check the audit is clean. It needs `objdump` (skipped with a
  warning otherwise).
- **failpoint_heap fails every n-th allocation by count**: a change that
  adds allocations moves the failure onto other paths and can expose old
  gaps.
- **A hang under fault injection may be the test's**: a test thread that
  gives up after a failed call without closing its pipe end, or a loop
  waiting for a thread that CreateThread never made, waits for ever on
  Windows too. Check every result before waiting on what it should have
  produced, and release what the other side waits for on the error path.
  Tell it from a kernel bug with the trace: the kernel's bug is a call
  that returned success (or nothing) while dropping work.
- **What must not fail must not allocate on its way**: ending one's own
  process, reporting a finished I/O request, waking a waiter. Allocate
  beforehand (when the request is made) or keep the state in what
  already exists (the pending entry). `let _ = v.try_push(x)` on such a
  path is the bug pattern to look for.
- **gala-um's host code allocates infallibly** (std, the platform's
  bookkeeping): it runs under `hosted::HostCode`, so injected heap
  failures only reach the NT layer. New host code reached from NT code
  that allocates (a std collection, a thread, printing) needs the guard,
  or a sweep aborts with "memory allocation of N bytes failed" in host
  code.
- **Out of frames looks like a bad image**: when NtMapViewOfSection
  cannot get a frame, the loader reports STATUS_INVALID_IMAGE_FORMAT
  (0xC000007B). Check the free frames (`mm::frame_stats`, the `stats:`
  line) before suspecting the image.
- **A JVM crashing with access violations at odd addresses** after a
  long run may be the machine out of memory: a user page fault that
  finds no free frame becomes an access violation (the commit charge
  covers private memory only; mapped and image pages and the kernel heap
  are not charged). Look for "vm: out of memory" in the log and the free
  frames on the `stats:` line; give the scenario more memory (`qemu()`
  in xtask).

### Deterministic simulation (reproducing a race from its seed)

`cargo xtask sim-test` runs the multi-threaded test programs
(`SIM_PROGRAMS`) in gala-um's simulation (`um --sim=<seed>`,
ARCHITECTURE "Deterministic simulation") under seeds 1..4, each twice:
expected output, the same trace for the same seed, different traces
across seeds. `--seeds 40` explores more, `--seed S` runs only S,
`--all` takes every gala-um program, a filter picks programs. A failing
run is reported with its seed, its log
(`target/failed/sim-<program>-<seed>-<run>.log`) and the um command line
that repeats it exactly:

    target/debug/um --cpus=2 --sim=17 --module=... user/bin/pipe_test.exe ''

- Add `--trace` to that line to see every system call with its thread:
  the same seed gives the same sequence each time, so print-debugging
  and `gdb` see the same interleaving (adding log lines does not change
  it). In gdb: `handle SIGSYS nostop noprint`, `handle SIGURG nostop
  noprint`, `handle SIGSEGV nostop noprint` (RDTSC faults on purpose),
  `handle SIGTRAP nostop noprint`.
- In the simulation program code faults on purpose: CPUID (CPUID
  faulting), RDTSC, and any read of KUSER_SHARED_DATA (its page is
  `PROT_NONE`; the read is then single-stepped, SIGTRAP); the spin
  capture single-steps spinning threads. Do not read 0x7ffe0000 from gdb
  expecting data (use the NT layer's copy). A program's own
  single-stepping (the trap flag) still works, except on an instruction
  that reads KUSER_SHARED_DATA.
- Changing the NT layer changes the interleavings a seed makes (a new
  lock or system call is a new decision): keep the seed in the bug
  report, and after a fix check the seed passes and a sweep (`--seeds
  40`) still does.
- `DEADLOCK` (um exits with 4): the dump lists every simulated thread
  (state, deadline) and the NT layer's view (each thread's system call
  and the objects it waits on). Make the smallest test program that
  waits that way and add it to `SIM_PROGRAMS`.
- "not reproducible (watchdog preemptions)": a thread computed in
  program code for about 100 ms without a system call while another
  could run, and the watchdog preempted it where it was (a spin loop that
  repeats exactly is preempted reproducibly instead: "N spin loops
  preempted"). Reported, not failed, for programs outside
  `SIM_PROGRAMS`; a SIM_PROGRAM must repeat. A hang or a wrong result is
  a failure.
- A new multi-threaded test program: add it to `SIM_PROGRAMS` (`true`
  if its system calls can interleave, so traces must differ across
  seeds).

### Clock control, record/replay, slow devices (R7)

ARCHITECTURE "Clock control" and "Record/replay" have the design and
formats.

- **Timeouts, timers, expiry without waiting**: boot (or run in gala-um)
  with `clock=control` and call GalaClockControl from the test
  (`ntrt::clock_control`: `CLOCK_SET_RATE` 0 freezes, `CLOCK_ADVANCE`
  jumps every view of the clock, `CLOCK_SET_SYSTEM` moves only the
  system time). Freeze first, then advance: the views then move by
  exactly the amount, and waits in progress whose deadline passed end
  (`clock_test` is the example). Without the call: `clock=rate:1000`
  runs a whole program 1000 times faster, `clock=system:2147483648`
  starts it in 2038, `clock=freeze` stops the programs' clock (the
  kernel keeps its own).
- **A failure that depends on outside input** (timing, random numbers):
  run it in gala-um with `--record=f.rec` until it fails (with `--sim`,
  vary the seed), then `target/debug/um --replay=f.rec --module=...
  program.exe` repeats it with the same settings and inputs; add
  `--trace` or attach gdb to the replay. In QEMU, `XTASK_RECORD=1 cargo
  xtask boot-test <name>` records every boot (`xtask: recording
  target/debug/test-<name>.rec` in the log; random numbers and the boot
  time); replay one with `cargo xtask iso --scenario <name> --file
  target/debug/test-<name>.rec=gala replay=test-<name>.rec run=...` or
  in gala-um (`--replay`: a kernel recording replays there too).
  `replay: diverged at ...` names the first input the recording did not
  have (the run took another path, or a helper thread ran longer).
- **A slow disk**: `failpoint=disk_io:delay:<ms>` (with `persist`).
- Keep checked-in recordings (`user/replay_test/replay_test.rec`)
  readable: a format change bumps `replay::VERSION` and the reader keeps
  the previous version.

### Lock-order checking (lockdep)

Debug kernels, host tests and gala-um panic with `lockdep: X taken while
holding Y, but Y was taken while holding X before` on the first
lock-order inversion (ARCHITECTURE "Built in early"), and with `lockdep:
releasing X turns interrupts on while Y is still held` when IrqMutex
guards drop out of order.

- A class is the protected type, so give a lock over a plain type
  (`u32`, `Option<id>`, `()`) a name with `IrqMutex::named("module.field",
  value)`, or unrelated locks of that type share a class and report
  false inversions.
- A guard taken inside a format macro's arguments (`info!("{}",
  *a.lock())`) lives to the end of the statement: read such values into
  locals first.
- Diagnostic code that peeks at a lock from an interrupt, an NMI or a
  panic uses `IrqMutex::try_lock_untracked`: a tracked `try_lock` there
  teaches lockdep an order no real path has, and a later correct path
  panics as an "inversion".

### Debugging tools

- `XTASK_VERBOSE=1 cargo xtask boot-test name` prints the serial log
  while it runs.
- Kernel command-line flag `trace` logs every system call with arguments
  and status (`trace: pid 4 tid 8 NtWriteFile(...) -> 0x0`) and every
  user fault; add it to a scenario's `cmdline` temporarily (it changes
  timing).
- `XTASK_QEMU_ARGS="-d int -D qemu.log" cargo xtask boot-test name`: the
  interrupt log shows every exception with the full register state
  (EFER, CR4, TR...), which is how per-CPU setup bugs show up (#UD on
  `syscall` = EFER.SCE missing on that CPU).
- `cargo xtask symbolize <serial log>` resolves the kernel addresses in
  any log.
- The event stream (`XTASK_EVENTS=all`) writes JSON lines (system calls
  with timing, image maps, threads, processes, faults) to
  `target/debug/<iso>.events.jsonl`; filter with `jq` or grep. Handle
  values are reused at once (the lowest free slot), so follow NtClose and
  creation order rather than guessing which handle is which from the
  value.
- Reading MSVC C++ EH data (as data, from .pdata/.xdata):
  check the format before decoding. A FuncInfo starting with the magic
  number 0x19930520-0x19930522 (bytes `22 05 93 19`) is the older format
  (__CxxFrameHandler3: try blocks, unwind map, an ip-to-state map with
  absolute RVAs); FH4's FuncInfo4 starts with a one-byte header. Reading
  FH3 data as FH4 gives plausible-looking nonsense. The handler data of
  a GS-checking handler is the FuncInfo RVA, then the GS data.
  `llvm-readobj --unwind` shows unwind data.
- **One CPU**: `XTASK_QEMU_ARGS="-smp 1" cargo xtask test` runs the whole
  suite on one processor; only the checks that expect two processors
  fail. The `onecpu_*` scenarios do it for a few.
- **TLB or migration suspects**: boot with `tlbparanoid` (a bug that
  goes away with it is a TLB bug), `nomigrate`, `checkuser`, `framecheck`
  (ARCHITECTURE "Kernel command line"); the shootdown counters before
  `boot ok` include threads moved to another CPU mid-shootdown.

### A boot that ends without a word

The serial log just stops and QEMU "exited with Some(0)": the machine was
reset (a triple fault, with `-no-reboot`) before the kernel could say
anything. Read, in this order (OBSERVABILITY "Early boot"):

1. the `xtask:` lines at the end of `target/failed/<scenario>.log`: how
   QEMU ended, the vCPUs it made, the host's load (a load average far
   above the host's CPUs makes vCPUs late);
2. `target/<profile>/<image>.phases.log`: the last `phase` line is how
   far the boot got (`fault`/`panic` lines are exceptions on the way);
   the CPU in it says which CPU wrote it;
3. `<image>.qemu.log`: "Triple fault" and the register dumps of CPU
   resets (an INIT is a reset too); a dump of CPU 1 with RIP in low
   memory or at HHDM + a low address is an AP in a bootloader
   trampoline;
4. `<image>.bios.log`: SeaBIOS (how many CPUs it found, the boot
   device).

For the exception's own state use `XTASK_QEMU_LOG=int,cpu_reset` (every
exception with its registers in `.qemu.log`: large; with
`XTASK_REPEAT=n` each failed boot keeps its own `.rN.*` files).
`smp: ... CPUs online` lower than asked: the bootloader missed an AP and
the kernel could not start it either; xtask boots such a VM once more and
says `VM got 1 of 2 CPUs (host overloaded?)`, keeping the short boot's
logs (serial, phases, qemu, bios) as `*.short-cpus.*` next to the image. A
reset with no word is typically an AP that Limine timed out on (its vCPU
thread did not run in time on a loaded host, seen at load averages of
12-20) and that later ran Limine's trampoline without its identity map and
triple-faulted; the kernel shelters such APs (ARCHITECTURE "APs the
bootloader missed"; `smptest=late`, `smptest=hide` reproduce a late or
hidden AP). To make Limine miss an AP for real, hold the AP's vCPU thread:
QEMU with `-name x,debug-threads=on`, then ptrace-stop the thread named
`CPU 1/TCG` (PTRACE_SEIZE + PTRACE_INTERRUPT) once SeaBIOS has printed
"Booting from" (SeaBIOS waits for every CPU) and release it after the
kernel banner; `smptest=late` is the in-guest, deterministic version.

### Debugging a kernel handover (R9)

The handover's design: ARCHITECTURE "Kernel handover", "Programs across
a handover", "Sockets across a handover", "Versions and negotiation".

**The jump** (`cargo xtask boot-test handover_same`):

- With `XTASK_VERBOSE=1` every kernel prints its banner again;
  `handover: generation A -> B: record ...` (written by the old kernel:
  where the record, the new image and the scaffold are) is the last line
  before the jump, and `handover: generation B (from generation A)` the
  new kernel's first word about it.
- Nothing after the jump line: the new kernel died before its console
  came up (a triple fault resets the VM, `-no-reboot` ends QEMU).
  `XTASK_QEMU_ARGS="-d int,cpu_reset -D qemu.log"` shows the faulting
  RIP and CR3: a CR3 equal to the scaffold's table pool and a RIP in the
  kernel window means the new image or its mapping is wrong (check
  `prepare`); a RIP at the trampoline's identity address means the jump
  code or its GDT. `XTASK_FIRMWARE=uefi` runs the same under OVMF.
- `NotParked(n, m)` in the panic: a CPU did not take the park IPI
  (interrupts off for 2 s: a lock held, or a CPU spinning with
  interrupts off).
- A refused record panics with `bootloader handoff rejected: handover
  record: ...` (CRC, version, a REQUIRED section this kernel does not
  know). The record's format and tests are in
  `kernel/src/handover/record.rs`. A new kind of state goes into a new
  section (or new fields at the end of one, its version raised), never a
  changed meaning of an old field; add a round-trip test and an "older
  writer" test.

**Programs** (`handover_idle`, busy ones `handover_k32`):

- `handover: refused: <reason>` says what stopped it (the programs go
  on). A thread that "runs user code" or "is not blocked yet" for 3 s
  never reached a wait or the user-return hook (a busy loop, a CPU with
  interrupts off, or in `thread_entry` without having entered user
  mode); "is in a system call that cannot be made again (NtXxx)" names a
  call blocked for 3 s that is not in `syscalls::restartable` /
  `restart_as` (make it restartable only if it has done nothing while
  blocked: the reissue does it all again).
- `handover: carrying N process(es), ...` (with `(3 stopped in user
  mode)` for busy ones) is the last line before the record; in the new
  kernel `handover: resumed process ...` and `... resumed, N ms after the
  record`. `cannot resume the programs: Failed { section, index, status
  }` names the record section and entry that could not be rebuilt (the
  programs are then lost: the record is past the point of no return;
  check what the old kernel wrote in `carry::Programs::write`).
- A carried thread that faults or returns garbage right after the
  handover: its registers come from `in_syscall` (the `syscall`'s) and
  its x87/SSE state from the scheduler's saved context; the call is made
  again through `arch::syscall::reissue`. USER threads come back through
  `thread_entry`'s parked path (IRETQ with every register, `fxrstor` of
  the held FXSAVE). A DLL page with wrong contents means the layout
  differs (`INVALID_IMAGE_HASH` in `cannot resume the programs`) or a
  page was taken as shared that was private (only borrowed frames equal
  to `ImageLayout::view_frame` are IMAGE_VIEW).
- A sleep that ends early or late across a handover is the clock: the
  tick count must go on from the record's (`time::resume`), at the first
  boot's timer rate (CLOCK v2).
- Something new a program can hold must be carried or refused in
  `carry::check_object` (the match is exhaustive over `ObjectBody`: a new
  object kind does not compile until it is decided).
- A new system call that blocks: decide whether a thread blocked in it
  can be carried (`syscalls::restartable` and `restart_as`); a call with
  a side effect before its wait (signal-and-wait, alert-by-id) carries
  its blocked half as the plain wait.

**Changing the record's format** (R9 (d)): change the section's writer
and reader (fields at the end only), raise its version in
`handover::version::KINDS` (and its comment), decide `must_read` (raise
it to the new version only if a reader ignoring the new fields would lose
what a program sees), make the reader give the new fields their defaults
for older versions, and add host tests (the previous version read with
defaults, the new one round-tripped). Run `cargo test -p kernel --lib`
and `cargo test -p kernel --lib --features handover_next handover::`
(both builds), then `cargo xtask boot-test handover_upgrade` and
`handover_downgrade_refused` (the default build to the next and back: if
the default build now writes what `handover_next` writes, move
`handover_next` ahead so the scenarios still cross a format change).
`handover: refused: the next kernel (...) reads X up to version N` is
negotiation working; `bootloader handoff rejected: handover record:
section X version V needs a reader of version M` in the new kernel means
a record got past negotiation (a must_read the note did not match):
check `KINDS` against the writers (`readelf -n` on the kernel shows the
note).

**Sockets** (`handover_net`): `handover: carrying N socket(s): C TCP
connection(s), L listener(s), U UDP socket(s); the network is stopped`
in the old kernel, `handover: N socket(s) resumed; the network was
stopped for X ms` in the new one.

- `refused: the network did not stop`: the net thread did not park
  within 1 s (it is stuck in a poll or a notify hook).
- A connection that hangs after the handover: compare the carried
  `ConnState` with what the peer expects (sequence numbers:
  `send_unacked`/`send_last`, `recv_first` plus the unread bytes is
  RCV.NXT); a reset right after it means the new kernel's socket was not
  found for the peer's segment (smoltcp answers unknown segments with
  RST: an entry not rebuilt, a wrong tuple or listen address).
- `Failed { section: 34/35, .. }` in `cannot resume the programs` names
  NET or SOCKETS (`afd::adopt`, `tcp::adopt_entry`: NO_MEMORY also when
  a carried buffer is larger than this kernel's `BUFFER`).
- The host test `net::tcp::tests::connections_go_on_in_a_new_stack`
  carries a loopback stack into a new one without booting.

**Upgrading smoltcp**: it is vendored (`vendor/smoltcp`, a
`[patch.crates-io]`, not a workspace member) with gala's patch between
`gala: begin`/`gala: end` markers in `src/socket/tcp.rs` (and
`#![allow(warnings)]` in `src/lib.rs`: a path dependency's warnings are
not capped). Copy the new release from the registry (drop examples,
benches, dev-dependencies from its `Cargo.toml`), apply the marked
blocks again, run `cargo test -p kernel --lib net::tcp handover::` and
the handover_net scenarios; the record (`ConnState`) changes only if the
new release's behaviour needs more state. Record the version in
THIRD_PARTY.md.

## Things that are easy to get wrong

Pitfalls met so far, by area. Test-writing pitfalls are under "Test
programs", harness ones under "Scenarios", FlaUI's under "FlaUI".

### Kernel code

- **The seams**: core code (`net/`, and anything a second personality
  shares) reaches threads, waits and time through `crate::runtime`, not
  `crate::nt::platform` (the NT layer's seam) and not
  `crate::sched`/`crate::time` directly (those do nothing in gala-um). A
  new process constructor must set `personality` (ARCHITECTURE "The
  seams") and attach a quota block to its handle table (`quota`,
  `HandleTable::attach_quota`: a table without one charges and limits
  nothing).
- **Handles go in through `HandleTable::insert`, and its error is the
  caller's status**: it fails with STATUS_QUOTA_EXCEEDED or
  STATUS_INSUFFICIENT_RESOURCES at a process's quotas, not only
  STATUS_NO_MEMORY; mapping every failure to NO_MEMORY hides which
  limit a program hit (ARCHITECTURE "Pool and handle quotas").
- **Kernel memory a process makes grow is charged when it grows**: a
  new buffer, queue or list that a program can fill (a socket's
  buffers, a port's packets, a token's default DACL) takes its charge
  from the owner's quota block before it allocates (`crate::quota`:
  `Held`, `Block::try_charge`; `nt::quota::grow` for an object's), and
  gives it back when it goes; a refusal is the program's status
  (STATUS_QUOTA_EXCEEDED, WSAENOBUFS, ENOBUFS), not NO_MEMORY. Core code
  (`net/`) charges `crate::quota::Block`, never `nt` types. Something
  retried on every poll (a listener's listening socket) asks
  `Block::fits` first once refused, or each poll logs and counts a
  refusal.
- **A new `ObjectBody` variant over a few hundred bytes is boxed**:
  every object is the size of the largest variant (an unboxed 10 KiB
  thread once made every event cost 10 KiB, all of it charged to pool
  quotas); `nt::quota::tests::objects_stay_small` checks it.
- **No lock in a log line's formatting**: a `Display` that takes a lock
  runs while `print` holds the serial port's, so the port is then
  "taken before" that lock, and the first log line written under it
  makes lockdep panic (debug kernels). The console's lock must stay the
  last one taken: take what the line needs first (`quota::peaks()`),
  and do not log while holding a lock that a line's formatting takes.
- **Host unit tests have no console**: code that logs (`info!`, `warn!`)
  writes to the serial port and crashes a host test (SIGSEGV); keep the
  logging out of the tested path or under `cfg!(not(test))`.
- **Arch code** (declared before `console` in lib.rs) writes
  `crate::info!`/`crate::warn!`, not the bare macros.
- **Multiprocessor**: all CPUs schedule threads, and any thread, the
  boot thread included, can move between CPUs at any reschedule. Never
  rely on "interrupts off" as a lock; use `sync::IrqMutex`. Per-CPU data
  lives behind GS (`arch::percpu`); per-CPU hardware setup belongs in
  each CPU's own init path (`ap_main` / early `kernel_main`), never in
  code run later by a thread. Per-thread state goes in `sched::Thread`
  or `NtThread`, not in a global "current request" slot.
- **Per-CPU state read while interrupts may be on** (system calls run
  with `sti`) must be one GS-relative load (`percpu::cpu_index`,
  `percpu::personality`): reading `gs:[0]` and then a field can land on
  another CPU's block after a migration. A per-CPU index read with
  interrupts on is stale after the next instruction: use it only with
  interrupts off or for diagnostics.
- **IrqMutex guards go in reverse order**: each guard restores the
  interrupt flag it found, so dropping an outer guard before an inner
  one turns interrupts back on while the inner lock is still held (the
  thread can then be preempted holding it). Let guards drop at scope
  end, or drop them innermost first; debug kernels panic otherwise. No
  `schedule()` with a lock held.
- **Read-modify-write of shared kernel state in one lock hold**: reading
  a value under a lock, dropping it and storing a value computed from it
  under a second hold loses whatever happened in between (two
  NtReleaseSemaphore calls at once once added one, and a wait satisfied
  in between was undone). Use one hold, or the dispatcher's
  `update_signal`/`change_mutant`. A heap corruption in a program with
  its own garbage collector is worth checking against the
  synchronization services its stop-the-world uses (for HotSpot on
  Windows: semaphores, events, critical sections), not only the memory
  manager; `smpsem_test` is the pattern for such a test.
- **Waits**: every `prepare_wait`/`wait` sits in a loop that re-checks
  its own condition and deadline (`sched::wake` wakes a waiting thread
  whatever it waits for). Kernel code that shares one event between
  several waiters must not count on each waiter seeing it set: a count
  or generation next to it (GalaConsoleCtrl's) says what happened.
- **Signals**: never read-modify-write a signal with `signal_state()` +
  `set_signal`, and never reset a signal after testing other state
  outside `wait::update_signal`: both lose wake-ups under SMP
  (`smpsem_test`, `smprace_test`; ARCHITECTURE "Waits").
- **A request that ends a stream waits for the pending ones that write
  it**: an asynchronous driver (AFD, pipes) may hold requests whose
  bytes are not in the stream yet (an overlapped send waiting for room);
  a shutdown, disconnect or end-of-file marker taken at once overtakes
  them, and the peer sees a clean end of stream short of the data. Order
  it behind them (AFD's `send_side`, ARCHITECTURE "AFD"); a call that
  must not block, like shutdown, records it and the last pending
  request's completion carries it out.
- **Termination-aware waits** call `thread::wake_if_terminating()` right
  after `platform::prepare_wait`; checking `is_terminating()` before
  marking the thread waiting loses a termination request made in
  between.
- **Memory manager locks**: a VAD change and the matching page-table
  change happen under one hold of the VAD lock (views, VAD, space;
  ARCHITECTURE "Virtual memory"); reading the VAD, dropping its lock and
  then mapping lets a fault or service on the other processor act on the
  state in between. Nothing that may write user memory (with_fs, which
  delivers directory change notifications) may run under those locks:
  lockdep panics.
- **Changing user mappings** goes through `process.space.lock()` (a
  `SpaceGuard`): the guard's drop does the TLB shootdown and frees the
  unmapped frames. Do not free user frames any other way while the space
  may be live.
- **32-bit system call arguments**: a ULONG (lengths, classes, codes,
  flags) passed on the stack, and sometimes in a register, has garbage in
  its upper half. Read it with `arg!(call, n) as u32` (NtCreateFile once
  read EaLength as 64 bits, `0xc0_0000_003a`; `XTASK_EVENTS=syscall`
  showed it).
- **Vector state**: the kernel is soft-float; never add code that uses
  x87/SSE/AVX registers in the kernel (the user's state is live in them
  during system calls and traps). Read a thread's saved state only
  through `Context::saved_xstate` or `xstate::save` (both `normalize`:
  XSAVEOPT leaves components in their initial state unwritten), never
  write a thread's switch area, and load user-supplied state only
  through `xstate::restore` (it sanitizes: a bad XSTATE_BV or MXCSR bit
  makes XRSTOR fault in the kernel). `XState` is 2.7 KiB: keep copies
  off deep kernel stacks.
- **Kernel code that host unit tests reach must not log**: `info!`/
  `warn!` write to the serial port, and in a `cargo test -p kernel
  --lib` build the test binary dies with SIGSEGV and no test name
  ("process didn't exit successfully ... signal: 11"). Gate the log
  lines with `#[cfg(not(test))]` or a wrapper compiled out under
  `cfg(test)` (`nt::session`'s and `display::virtio_gpu`'s `log!`);
  events are fine (`events::emit` is a no-op in tests). Find the test
  with `--test-threads=1` or by filtering module names.
- **Window behaviour belongs in `nt::winstate`**: what a window is
  (rectangles, Z order, focus, capture, client pixels, input for
  programs, the named actions) lives there, which every desktop has, a
  bare one too; `nt::desktop` only draws it (frames, taskbar, start
  menu, pointer). A change to window behaviour made in the compositor
  instead is missing where nothing shows the desktop
  (`noscreen_windows`, `server_profile_windows`).
- **Windows live in a session**: kernel code reaches the window
  registry, WinEvent hooks and shasha's desktop through the process
  (`process.desk.windows` / `.hooks` / `.shell`), never a global;
  `desktop::screen(shell)` is None for a session the console does not
  show. An HWND of another session is no window
  (ERROR_INVALID_WINDOW_HANDLE; `events=window` and the log say where it
  is).
- **32-bit code (WOW64) and the segments**: DS, ES and FS are per CPU,
  not per thread: a WOW64 thread gets them (and this CPU's TEB32 entry)
  at every switch to it (`sched::set_teb32`, `percpu::load_teb32`);
  32-bit code with DS 0 faults at its first memory access. Never load
  GS for a WOW64 thread: its base is the 64-bit TEB, through which
  wow64.dll finds the thread's CPU area. `syscall::enter_user_regs`
  always returns with CS 0x33: anything that redirects a thread
  interrupted in 32-bit code (an exception, an APC) must enter 64-bit
  code (wow64.dll's dispatchers), never resume at a 64-bit address with
  the frame's CS 0x23: a trap frame redirected there gets CS 0x33 and
  SS 0x2b too (`process::deliver_to_user`), on the thread's 64-bit stack,
  not the 32-bit one.

### gala's DLLs

- **A message's digest is not its content parameter**: a PKCS #7
  SignedData whose content is not `data` (Authenticode's
  SpcIndirectDataContent) is digested over the inner TLV's contents
  octets, without its tag and length, while CMSG_CONTENT_PARAM gives the
  whole TLV; a CMS one (an RFC 3161 token's TSTInfo) wraps its content in
  an OCTET STRING, digested and returned as the octets
  (crypt32's `msg::Signed::digested` and `content_param`). Hashing the
  whole TLV makes every Authenticode signer CRYPT_E_HASH_VALUE.
- **A signed binary's signer may have expired**: Authenticode checks
  the chain at the RFC 3161 timestamp's time (python.exe's signer was
  valid for three days in 2024), so a verification that fails with
  CERT_E_EXPIRED on a real binary means the timestamp was not found or
  not used, not that the binary is bad. wintrust_test's real images
  (ODT's setup.exe, CPython's python.exe) come from their PREBUILT
  downloads, shipped to C:\gala by the scenario (`args=require` makes
  their absence a failure; on Windows they are skipped when absent).
- **Small integer LPCSTRs**: a store provider or structure type passed
  as `1 as *const u8` trips clippy's `manual_dangling_ptr`; write
  `core::ptr::without_provenance(1)`.
- **A stub is invisible to GetProcAddress**: gala_stubs' exports live in
  `.gstub`, which ntdll's GetProcAddress skips (a probing program takes
  its fallback), while they are in the export directory. A test of which
  names a DLL exports reads the export directory for presence
  (`kernel32_test`'s `export_split.rs`) and GetProcAddress only for
  absence and addresses.
- **kernel32 builds kernelbase's `address_wait.rs` for itself**: its
  locks wait on their words through a private copy with its own waiter
  list. Keep it unexported in kernel32 and keep its users there to
  kernel32's own words; a WakeByAddress* from one DLL never wakes a
  waiter of the other.
- **Which DLL exports a name is measured, not guessed**: Wine's specs
  are the list's starting point, but Windows' kernel32 and kernelbase
  split differently in places (`tools/kernelbase_exports.py`'s
  MEASURED); a name moved between them goes in `export_split.rs` with
  the measurement.
- **Objects told apart by a leading magic must be `#[repr(C)]`**:
  Rust reorders a plain struct's fields, so "every key object starts
  with its magic" is false without it. bcrypt's RSA and ECDSA key
  structs lacked it: an ECDSA handle read as an RSA key matched, and
  BCryptExportKey of it answered for RSA (seen once ECDSA export was
  added).
- **`cond.then_some(f(x))` evaluates `f(x)` even when `cond` is
  false**: with `f` dereferencing a pointer `cond` checks for null,
  the compiler takes the dereference as proof the pointer is never
  null and drops the checks that lead there (ntdll's function table
  search lost its loop exit and spun on a miss). Use
  `cond.then(|| f(x))`.

- **Table rows keyed by numbers typed from memory drift**: kernel32's
  LCTYPE table once had LOCALE_IFIRSTDAYOFWEEK at 0x1009 (it is
  LOCALE_ICALENDARTYPE) and three more rows one or more places off;
  nothing failed until Delphi asked for the real ones. Check each new
  row's number against the mingw-w64 header
  (`/usr/share/mingw-w64/include/winnls.h`) and test it by number.

- **An HGLOBAL is not a pointer**: GMEM_MOVEABLE memory (what
  CreateStreamOnHGlobal, the clipboard, STGMEDIUM and DDE hand around)
  is a handle to a table entry (kernel32's `global.rs`); read and write
  it between GlobalLock and GlobalUnlock, never through the handle.
- **The standard handles are not consoles without a console session**:
  making GetConsoleMode succeed on the serial-line handles changes
  programs' output (Caddy's logging; `kernel32_sys_test` and
  `winsock_test` expect redirected handles). Keep them non-consoles.
- **Static tables** go in a DLL's data section (copied into every
  process that loads it) unless their initial value is all zero bytes:
  give "empty" slots zero fields (`disabled` rather than `enabled:
  true`, offsets rather than a base like 0xC000) and data-carrying enums
  `#[repr(u8)]` with the empty variant 0. `llvm-objdump -h` shows the
  .data size.
- **A DLL that exports a function under the name of one it calls in
  another DLL** (ncrypt's BCryptGenRandom, which Windows forwards to
  bcrypt; advapi32's CreateProcessAsUserW to kernel32's) cannot import
  that name with `raw-dylib`: the linker binds the import to the DLL's
  own export and the call jumps to itself for ever (`jmp $`, a silent
  hang with no stack growth). Look the other DLL's function up at run
  time (LoadLibraryW + GetProcAddress), as `user/ncrypt` does.
- **Functions that must start from their caller's registers** (anything
  that captures a context or walks frames: RtlCaptureContext,
  RtlRaiseException, RtlUnwindEx, the dispatchers) are `global_asm!`
  with `.seh_*` directives in ntdll's `seh.rs`, not naked functions:
  naked functions get no unwind data, and a walk cannot pass through
  them. Assembly symbols are exported through `/EXPORT` in testntdll's
  build.rs. `llvm-readobj --unwind` shows what the assembler emitted.
- **The exception dispatcher runs on what is left of the stack** (a
  stack overflow is delivered with only the guard region below rsp):
  keep the vectored-handler path's frames small and put CONTEXT copies
  in `#[inline(never)]` helpers (`guard_test` catches this).
- **CONTEXT_XSTATE**: gala's ntdll drops it from its own CONTEXT copies
  (they have no CONTEXT_EX); a copy that keeps the flag makes NtContinue
  read whatever follows it as a CONTEXT_EX (validated, then ignored if
  wrong).
- **Predefined registry keys are sign-extended on x64**: winreg.h
  defines HKEY_LOCAL_MACHINE as `(HKEY)(ULONG_PTR)((LONG)0x80000002)`,
  0xFFFFFFFF80000002; C programs and .NET pass that form, Rust test
  programs usually the zero-extended one. Accept both (advapi32's
  `predefined`).
- **x64 structures have padding**: WNDCLASSW is WNDCLASSEXW without
  cbSize, but its lpfnWndProc is still at offset 8 (4 bytes of padding
  after the style), so converting between them is not one copy from
  offset 4 (GetClassInfoW once gave Delphi's VCL a wrong window
  procedure and every second control of a class failed to register).
  Lay a converted structure out field by field from the header.
- **SDK structure packing**: richedit.h packs its structures to 4 bytes
  (`pshpack4.h`): a pointer or WPARAM after a 4-byte field is not
  8-aligned (ENLINK's wParam at 28, lParam at 36, chrg at 44;
  GETTEXTEX's lpDefaultChar at 12; EDITSTREAM's callback at 12). Write
  such structures byte by byte, and check other SDK headers for
  `pshpack` before laying out a structure.
- **Nothing that may send a message under a control's lock**:
  BeginPaint sends WM_ERASEBKGND, SetWindowLong(GWL_STYLE) may send
  WM_STYLECHANGING, and wx subclasses every control (its procedure may
  send the control anything). Take the DC (or change the style) outside
  the lock, as `user/richedit` does; its `Store` returns None to a thread
  that asks again while holding it rather than spinning.
- **Two DLLs with one implementation** (riched20 and msftedit): an rlib
  crate (`user/richedit`, its own panic handler and imports) and a small
  cdylib crate per DLL whose DllMain registers its classes; list each
  cdylib in `USER_DLLS` and the rlib on the CI clippy line.
- **COM interfaces written in Rust** (gala's UIAutomationCore): take the
  method order from the C `...Vtbl` structs in the mingw-w64 header
  (inherited methods included). On x64 a by-value argument over 8 bytes
  (VARIANT, RECT) arrives as a pointer to the caller's copy, an 8-byte
  POINT in one register. Tell objects apart by a field, not only by
  vtable address: identical read-only tables may be folded by the
  linker. mingw-w64's headers spell some names differently from the SDK
  (`TreeScope_SubTree` for `TreeScope_Subtree`): a C test meant to build
  with both uses the value.
- **Datagram sockets in ws2_32**: whether a socket is a datagram one is
  ws2_32's own per-socket record (SO_TYPE in the options table, set by
  socket/WSASocketW and WSADuplicateSocket's receiver); sendto/recvfrom
  pick the AFD request from it. A socket handle ws2_32 did not make (an
  accepted one, one from another process without WSADuplicateSocket)
  counts as a stream.
- **Non-blocking state is inherited**: Windows gives an accepted socket
  the listener's non-blocking mode (Nginx relies on it; without it recv
  blocked on the TLS handshake's second flight while plain HTTP, arriving
  in one piece, hid the bug).
- **Rust for x86 (i686-pc-windows-msvc)**: `extern "C"` symbols are
  decorated with a leading underscore, so `global_asm!` defines
  `_name` for Rust's `name` (C runtime ones too: `_memcpy`); `extern
  "system"` is stdcall (the callee pops) where x64 has one convention;
  `core` calls MSVC's 64-bit division helpers (`__aulldiv`,
  `__aullrem`), which ntrt's x86 runtime provides; Rust's target marks
  images large-address-aware (a 4 GiB 32-bit address space under
  WOW64). 32-bit code inside a 64-bit image (`.code32` in wow64.dll's
  `global_asm!`) cannot use label differences in memory operands with
  Intel syntax: keep such sequences fixed-length and check the bytes
  with `llvm-objdump -d --triple=i386`.

### Windows behaviour gala must copy

- **A thread's windows go when it ends**: a test or gala DLL that
  creates a window on a worker thread which then returns loses the
  window (IsWindow FALSE, sends fail with ERROR_INVALID_WINDOW_HANDLE).
  Keep such a thread alive and pumping. A thread gets a message queue
  (PostThreadMessage reaches it) only once it creates a window or calls
  a message function; post to a new thread after it has called
  PeekMessage, as PostThreadMessage's documentation says.
- **user32's queue event is one per process** (auto-reset): with several
  threads waiting in GetMessage at once, a wake can go to the wrong one,
  which then waits again. Tests with several pumping threads keep only
  one waiting at a time (windows_across_test waits on thread handles
  without pumping while a worker pumps).
- **Hover logic that waits for "the pointer left"** (tooltips): a window
  only gets WM_MOUSEMOVE while the pointer is over it, so code fed by one
  window's moves (a subclass, TTM_RELAYEVENT) never sees it leave across
  other windows; poll GetCursorPos against the rectangle on a timer while
  the pointer is on it.
- **A window moved without a new size** (snapped from the left half to
  the right) gets WM_MOVE but no WM_SIZE, as on Windows: a test program
  that watches its rectangle checks both.
- **A program's window state reaches it after it handled the request**
  (user32 mirrors it): poll IsZoomed/IsIconic from another process
  rather than reading them once right after `gala windows` (which is
  shasha's own state) changed.
- **WinINet's status callback is silent for a handle whose context is
  0** (InternetConnect's, HttpOpenRequest's dwContext): a test that
  expects notifications gives its handles a nonzero context; the
  address notifications (NAME_RESOLVED, CONNECTING/CONNECTED_TO_SERVER)
  are ANSI text even for InternetSetStatusCallbackW.

## Hygiene sweeps

A sweep reads existing code for one class of fault at a time instead of
waiting for a flaky scenario to point at it. The kernel's primitives
were written for one CPU at a time and SMP came after. Periodic sweeps
are parked until gala runs live (ROADMAP HS); until then use the
checklist when a flaky scenario has no explanation yet.

**What to check** (each item: what the bug looks like, where to look):

1. *Check, then act, in separate lock holds*: a condition read under a
   lock and acted on after letting go (or under another lock). Look for
   `let x = a.lock().thing;` followed by a change elsewhere; for
   `set_signal(.., 0)` after a test of some queue; for "decided under
   lock A, signaled under lock B". Fix: one hold, or the dispatcher's
   `update_signal`/`change_mutant`; or a pin (`NamePin`).
2. *Wake-ups whose condition changes before the woken thread runs*: a
   waiter that wakes and looks again can find the condition gone (a
   notification event reset: `Waiter::released`); a waiter that does
   not look again takes any wake-up for its own (NtDelayExecution and
   NtResumeThread were). Every `prepare_wait`/`wait` must sit in a loop
   that re-checks its own condition and deadline.
3. *Order of teardown*: what a waiter on the dying thing finds when it
   wakes (a process signaled before its handles were closed); queues of
   work for something that is going away (APCs to an exiting thread,
   packets to a closed port); a name or count dropping to zero while
   another CPU is about to take a reference (handle count vs. lookups).
4. *"This CPU"*: a per-CPU index read with interrupts on is stale after
   the next instruction (`cpu_index()`, `current_cpu()`); only use it
   with interrupts off or for diagnostics (shootdown's `changed_on`).
5. *Interrupt state*: IrqMutex guards must drop innermost first (debug
   kernels panic otherwise); no `schedule()` with a lock held.
6. *Duplicate or aliased arguments*: the same object twice in one call
   (WaitAll with one semaphore twice consumed it twice).
7. *Non-race hygiene on the way*: failure paths that leave nothing
   behind (OBSERVABILITY.md), infallible allocation (`cargo xtask
   alloc-audit`), `unsafe` without `SAFETY:`, stale TODOs.

**How**: one SMP stress test per sweep (`smprace_test` for the first),
one round per finding, two threads pinned to processors 0 and 1 doing
the two sides in a loop, each round shown to fail before its fix (keep
the iteration count at a few times what the failure needed: boots under
a loaded host are slow, and the scenario's timeout is 120 s). A finding
that cannot be made to fail is fixed when the fix is small and clearly
right, and the reasoning is written down. Record each finding in ROADMAP
and the invariant it breaks in ARCHITECTURE.

**Sweeps done**: the first covered the dispatcher and waits (events,
semaphores, mutants, timers), APCs, thread and process teardown, handle
tables and names, IOCP, the scheduler's waits, the heap and frame
allocator's locks and the console's input signal. `smprace_test`'s
rounds are its findings: WaitAll with an object twice (round 1), a
timer's stale expiry signaled after a re-arm (2), a name removed while
an open was taking its handle (3), a process signaled before its handles
were closed (4), NtDelayExecution ended by any wake-up (5). What was
fixed without a test, what was left open and what was checked and found
sound are in ROADMAP "HS. Hygiene sweeps"; the invariants in
ARCHITECTURE "Waits".

## Where things are

| Area | Path |
|---|---|
| Boot, Limine protocol | `kernel/src/boot/` |
| CPU, paging, interrupts, SYSCALL, SMP, virtio transport | `kernel/src/arch/x86_64/` |
| Frames, heap | `kernel/src/mm/` |
| Scheduler | `kernel/src/sched/` |
| Core seam to threads, waits, time | `kernel/src/runtime.rs` |
| NT personality (objects, processes, files, sections, syscalls, registry seed) | `kernel/src/nt/` |
| Linux personality (exec, system calls, mmap/brk, ELF loader, initial stack, syscall/errno tables) | `kernel/src/lx/` |
| Linux ABI data (signal frames, sigsets, termios, struct layouts) | `kernel/lx_abi/` |
| Ramfs, boot-module tar trees | `kernel/src/fs/` |
| Disk (virtio-blk) | `kernel/src/disk/` |
| Display (virtio-gpu; the protocol and driver host-tested) | `kernel/src/display/` |
| Kernel handover record | `kernel/src/handover/` |
| gala-um's platform | `kernel/src/hosted/`, `um/` |
| Baked fonts, icons, wallpapers | `kernel/assets/`, `fonts/selawik/` |
| Test runtime for user programs (x64, and x86 for 32-bit ones) | `user/ntrt/` (`x64.rs`, `x86.rs`) |
| WOW64: the kernel's part, gala's wow64.dll, the 32-bit ntdll and kernel32, the 32-bit tests | `kernel/src/nt/wow64.rs`, `user/wow64/`, `user/testntdll/` (`x86.rs`, `seh_x86.rs`), `user/kernel32/` (`x86.rs`), `user/wow64_*_test/` (`wow64_c_test`: C), `user/testdll32/` |
| Test programs, test DLLs, stand-in ntdll | `user/*_test/`, `user/testdll*`, `user/testntdll/` |
| Linux test programs, their binaries and transcripts | `user/lx/`, `user/lx/bin/` |
| `gala install` (winget packages): library, commands, downloads, test source | `user/galapkg/`, `user/gala/src/pkg.rs`, `user/gala/src/fetch.rs`, `user/pk_install_test/` |
| The public CA bundle of the machine root store | `user/roots/` (`public-roots.pem`, `update.sh`) |
| winget manifest resolver and its corpus | `user/galapkg/src/winget/`, `user/galapkg/testdata/winget-corpus/` |
| Third-party program builds, scripts and configuration | `user/<program>/` (`build.sh`) |
| Checked-in binaries, Windows runner (`run-all.ps1`) | `user/bin/` |
| Triage of the test programs on real Windows: run-all's data and probe scripts | `user/windows/` (`docs/windows-triage-brief.md`; requests and findings are GitHub issues labelled `windows` / `from-windows`) |
| gala under KVM on the Windows machine: Chromium's memory, the WPT run, the performance profile | `docs/windows-kvm-brief.md` (GitHub issue #1, branch `windows-kvm`) |
| Build, boot tests, scenarios, reports | `xtask/src/main.rs`; signing `xtask/src/trust.rs`; triage `triage.rs`; OTLP `otel.rs` (encoders `otel/wire.rs`, batching and sending `otel/send.rs`); lx transcripts `lx_expect.rs`; audit `alloc_audit.rs`; clock check `clockcheck.rs` |
| Docker test image | `docker/test.Dockerfile` |
| Vendored dependencies (smoltcp with gala's handover patch) | `vendor/` |

Output under `target/`:

| Path | What |
|---|---|
| `target/<profile>/test-<name>.serial.log`, `.events.jsonl`, `.phases.log`, `.qemu.log`, `.bios.log` | a boot's logs |
| `target/failed/<scenario>.log` | the last failed boot's log with its triage |
| `target/report/` | `history.jsonl`, `timing.txt`, `ledger.md`, `gala.{json,md}`, `bench.json` |
| `target/prebuilt/<name>/` | fetched releases |
| `target/screenshots/` | screenshots |
| `target/trust/` | the test signing key |
| `target/net-client/<port>/` | a net client's files during a boot |
| `target/lx-expect/<binary>/` | lx-expect's runs and `actual.txt` |
| `target/next/` | the `handover_next` kernel |
| `~/.cache/gala-qemu/<version>` | the pinned QEMU |

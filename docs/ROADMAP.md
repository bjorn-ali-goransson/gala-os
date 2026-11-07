# Roadmap

What is built, what is in flight and what comes next, by item ID. It is
status and plan only: how things work is in `ARCHITECTURE.md` (and
`SECURITY.md`, `OBSERVABILITY.md`, `GALA_UM.md`), how to do the work is
in `WORKFLOW.md`, and measurements are in `PERF.md`.

The file is updated in the same commit as the work it describes. Every
piece of work is test first: a test program in `user/<feature>_test`
whose expected results are real Windows' (real Linux's for item LX), its
boot scenario in `xtask/src/main.rs`, seen failing, then the
implementation; binaries exported to `user/bin/` with `cargo xtask
export-tests` (WORKFLOW "One chunk of work").

## Vision

A running gala updates its system image and its kernel without a reboot,
with programs kept running (item R; PROJECT_BRIEF goal 3). Until that is
built, every change keeps R1's invariants: state as data, restartable
system calls, versioned formats, drivers that survive a reset, one clock;
a change that would make the kernel handover harder argues for itself in
its commit. gala ships as two editions: the *server* edition (console
programs and servers, never a graphical interface) and the *desktop*
edition, a strict superset with **shasha** (Arabic شاشة, "screen"), the
graphical shell, where everything shasha does can also be done with the
`gala` command (item SD). See `PROJECT_BRIEF.md` for goals, values and
scope.

## Priorities

The ordering rules in force (set by the owner):

1. **Foundational work first.** Of the work in flight, the most
   foundational goes first and gets the machine's resources; nothing
   running is cancelled. Order: kernel correctness (races, lost
   wake-ups, corruption), then core structures other items build on (LX
   seams, the core/NT process split, RS's sessions), then features on
   top (PK installers, BR), which run niced and yield VMs to the rest.
2. **Tooling that speeds up development and troubleshooting** comes
   before other work of the same rank, since we are the ones who benefit:
   deterministic simulation (R5), record/replay and clock control (R7),
   fixing flaky scenarios, guest-time test limits (T2).
3. **Observability carries great weight.** Every change ships with what
   explains its failures (OBSERVABILITY's rule); a gap a failure exposes
   is filled before or with the fix.
4. **Hygiene sweeps.** Existing code is read for one class of defect at
   a time (item HS; checklist in WORKFLOW "Hygiene sweeps"). Parked
   until gala runs live (the owner); a flaky scenario with no
   explanation still gets the checklist.
5. **Token-heavy implementation as filler.** When an agent slot frees up
   and nothing more foundational waits: item LX's Linux system calls
   (milestone by milestone, test first against real Linux) and item AG's
   API gaps (exports and behaviour the ledgers report missing, test
   first against Windows).
6. **Structural, root-cause fixes.** A failure is fixed where it arises,
   not worked around. A stopgap is allowed only when named as one, with
   its structural fix and owning item listed in "Known issues and
   stopgaps" below, and is replaced.
7. **Server and desktop stay apart; the CLI comes first.** Server
   workloads (servers, console programs, the Docker image) never start
   the graphical interface; a desktop feature is built as an action the
   `gala` command reaches first, then given a mouse and keyboard binding
   (item SD, WORKFLOW "Rules for every change").

## Current focus

In order:

0. **BR stage 2** first (the owner: Chromium working, then the rest of
   this list): its window, input and accessibility tree are done
   (`chromium_window`, `chromium_uia`), ChromeDriver too
   (`chromium_driver`); now the conformance report.
1. **TLS**: step 6's rest (revocation information fetched from the
   distribution point, winhttp keeping its credential handle so it
   resumes, a server asking for client certificates, the MY store's
   automatic client certificate; ECDSA server keys, client session
   resumption, client certificates and offline revocation are done).
2. **LX**: seams 1 and 5 as their areas are touched, seam 4's NT surface;
   milestone 2's rest (busybox runs; threads, futex, fs_test's links
   done) as filler.
3. **RS**: step B3a's rest (`presenter=` in gala-um, xtask's headless
   screen checks through SESSION_INPUT); then B3b's remote presenter.
4. **WOW** follow-ups (done otherwise): the x86 Inno runs' other ledger
   gaps; kernelbase; kernel32_test built for x86.
5. **R**: R7's next part (network frames, disk reads and input in
   record/replay), R6's rest (quota_test on Windows), R8's Secure
   Boot.
6. **PK**: winget's source index (step 2's rest); step 1's rest (an
   upgrade's dependencies, UpgradeBehavior, winget's settings).
7. **BR** stage 1's rest (the profile's hotspots).
8. **AG**: the remaining stubs (filler).
9. **SSH**: the client against the host's sshd, agent forwarding;
   **H11**: busybox-w32's httpd and wget over loopback (the Linux
   busybox's run: `lx_httpd`).

## Status at a glance

States: **done**, **in progress** (some built, work open), **planned**
(scheduled, nothing built), **later** (wanted, not scheduled),
**parked** (stopped or deferred on purpose). Every row that is not done
has a section under "Open items".

### Foundation and the NT layer

| ID | Title | State | Note |
|---|---|---|---|
| M1, M2, M4 | Boot, memory, scheduler, user mode | done | ARCHITECTURE "Boot", "Virtual memory", "Threads and scheduling", "NT personality" |
| — | NT core services | done | the table below lists them with their tests |
| — | XSAVE and AVX state | done | `xstate_test` (also gala-um); ARCHITECTURE "x87/SSE/AVX state" |
| — | Screen console, `cargo xtask screenshot` | done | WORKFLOW "Screenshots" |
| S1 | SMP bring-up | done | `smp_test`; ARCHITECTURE "Processors" |
| S2 | SMP scheduling | done | `smpsched_test` |
| S3 | SMP memory: user TLB shootdown | done | `smpmem_test`, `smpcow_test`; ARCHITECTURE "TLB shootdown" |
| — | SMP robustness and early-boot telemetry | done | APs Limine missed started from the MADT, late APs sheltered, one-CPU boots; `smp_missed`, `smp_late`, `onecpu_*`; OBSERVABILITY "Early boot" |
| — | Clock calibration under host load | done | `clockcal_test`, `clockcal_recal`, `handover_park_late`; ARCHITECTURE "Interrupts and time" |
| S4 | SMP, the rest | later | per-CPU run queues, ranged shootdowns, PCIDs |
| T1 | Test speed | done | boot tests 66 s -> 14 s; WORKFLOW "Test speed and environments" |
| T2 | Boot-test limits on the VM's run time | done | `xtask/src/vmtime.rs`; WORKFLOW "Scenarios" |
| O1 | Ledger, event stream, backtraces, benchmarks | done | OBSERVABILITY "Event stream", "Missing-features ledger", "Backtraces and panic output", "Benchmarks" |
| U1-U3 | gala-um | done | 109 programs in seconds; GALA_UM.md |
| U4 | gala-um: more than one NT process | planned | child processes and `start=`; GALA_UM "Limits and open questions" |
| U5 | gala-um: a window on the host | planned | the desktop in a host window (X11/Wayland), with its input; headless in um-test |
| U6 | Windows applications in CI through gala-um | planned | a Windows app and its FlaUI tests headless in a stock GitHub Actions runner; needs U4 and U5 |
| HS | Hygiene sweeps | parked | first sweep and its four leftovers done; no further sweeps until gala runs live (the owner) |

### NT core services (done, by test)

| Area | Tests |
|---|---|
| Virtual memory: VAD, demand-zero paging, allocate/free/protect/query, guard pages and stack growth, copy-on-write | `vm_test`, `guard_test`, `cow_test`, `vmcycle_test`, `commit_test` |
| Objects, handles, names, symbolic links; pool and handle quotas | `object_test`, `symlink_test`, `quota_test` |
| Waits, events, mutants, semaphores, keyed events, timers | `wait_test`, `sync_test`, `keyed_event_test`, `timer_test`, `smpsem_test`, `smprace_test` |
| Threads, suspension, context, TEB/PEB/KUSER_SHARED_DATA, process startup | `thread_test`, `threadinfo_test`, `context_test`, `kuser_test`, `peb_test`, `startup_test` |
| Sections, image sections, memory-mapped files | `section_test`, `image_test`, `filemap_test` |
| Files, directories, async I/O, APCs, IOCP, file locks, change notification | `file_test`, `dir_test`, `async_test`, `apc_test`, `ioapc_test`, `iocp_test`, `lock_test`, `dirchange_test` |
| User-mode exceptions, SEH and unwinding, C++ exceptions, x87/SSE | `exception_test`, `seh_test`, `cxx_eh_test`, `fpu_test`, `safepoint_test` |
| Loader: imports, dynamic loading, static TLS, API sets | `import_test`, `loader_test`, `tls_test`, `missing_import_test` |
| System and process information | `sysinfo_test` |
| Pipes, child processes with redirected stdio | `pipe_test`, `process_test` |
| Registry | `registry_test`; ARCHITECTURE "Registry" |
| Differential runner against Windows | `cargo xtask report`/`compare`, `user/bin/run-all.ps1` |

### Programs and Win32

| ID | Title | State | Note |
|---|---|---|---|
| G1 | kernel32, first part | done | `kernel32_test` |
| G2 | Go hello world | done | `go_hello` |
| G3 | Workable Go programs | done | `go_goroutines`, `go_time`, `go_gc`, `go_os`, `go_exit` |
| N1 | Networking: virtio-net, TCP/IP, AFD, ws2_32, UDP | done | `net_echo`, `winsock_test` (also many listeners, a full backlog), `winsock_async_test`, `go_http`, `udp_test`, `udp_echo`; UDP limits, listeners and the socket limit in ARCHITECTURE "Networking" |
| N2 | Nginx (x64 build, master/worker, zlib, PCRE2, OpenSSL) | done | `nginx`, `nginx_master`, `nginx_tls`; WORKFLOW "Nginx" |
| N3 | IPv6 sockets: loopback ::1, IPV6_V6ONLY and dual stack, the card's addresses | parked (steps 1 and 2 done: `ipv6_test`: ::1, IPv4-mapped addresses and dual stack, IPV6_V6ONLY, ports per family, AF_UNSPEC localhost; step 3, the card's addresses and AAAA queries, out of scope for now: ARCHITECTURE "Plan: IPv6") | ARCHITECTURE "Plan: IPv6"; Chromium probes AF_INET6 (BR) |
| H1-H10 | Toward self-hosting: pipes, busybox, scripts, TinyCC (and itself), make, Lua, build speed, SQLite, file locks | done | `busybox*`, `tcc`, `tcc_self`, `tcc_make`, `lua`, `sqlite`; PERF.md |
| H11 | busybox httpd and wget over loopback | in progress | Linux busybox done (`lx_httpd`); busybox-w32 and the Winsock client open |
| WOW | 32-bit x86 programs (WOW64) | done (follow-ups listed) | stages 1-2 done: `wow64_test`, `wow64_thread_test`, `wow64_crash`, `win32_misc_test`'s WOW64 answers; stage 3's 32-bit ntdll, exceptions, APCs, redirection and environment: `wow64_import_test`, `wow64_seh_test`, `wow64_seh_crash`, `wow64_fs_test`; the 32-bit kernel32: `wow64_k32_test`, `wow64_c_test`; the 32-bit msvcrt: `wow64_crt_test`; the 32-bit advapi32 and the registry's 32-bit view: `wow64_adv_test`; stage 4's 32-bit user32 and gdi32: `wow64_gui_test`; an x86 Inno Setup installer: `wow64_inno`, under `gala install`: `pk_install_inno32`; ARCHITECTURE "Plan: WOW64" |
| P1 | Unmodified prebuilt programs | in progress | Caddy, JDK, .NET, PowerShell, Elasticsearch, LocalDB, OpenSSH, PostgreSQL, MinGit, FileZilla Server done; Git Bash parked |
| SEC | Security model: tokens, descriptors, access checks, logons, integrity, UIPI | done | SECURITY "Status": phases S1-S6 (leftovers listed there) |
| D1-D3 | Disk: virtio-blk, C: kept (`persist`), the registry kept | done | `disk_test`, `persist_test`, `sqllocaldb_persist`; ARCHITECTURE "Disk" |
| TLS | Schannel and the HTTP stacks on it | in progress | Schannel, winhttp and wininet (synchronous, one engine, redirects), curl and Git's https remotes (curl's `--cacert` too), `gala install`'s downloads with a public CA bundle, crypt32's chain engine (Schannel's trust decision), ECDSA server keys, session resumption, client certificates, revocation from CRLs (offline) done; fetching CRLs next |
| AG | API gaps in the shell and runtime DLLs | in progress | 13 groups done; filler |
| LX | Linux personality | in progress | seams 2, 3, 6 and milestone 1 done, seam 4's Linux side (links, case-sensitive directories, modes kept); static busybox runs (milestone 2), TCP sockets, threads, futex, epoll |
| CT | Linux containers: Docker Desktop and Podman over a WSL facade | planned | spec recordings first; needs LX beyond milestone 5 |

### Desktop, sessions and remote access

| ID | Title | State | Note |
|---|---|---|---|
| W1 | Windows on the screen | done | `gui_test`; ARCHITECTURE "Windows on the screen" |
| W2 | Styled desktop and input | done | `gui_test` |
| W3 | FileZilla | in progress | connects to FileZilla Server, driven by pixels and by FlaUI; leftovers |
| W4 | The Windows look | in progress | steps 1-2 done; step 3 (themes) next |
| SH | The shell in user mode | planned | taskbar, start menu and wallpaper out of the kernel; the compositor later (RS B3a) |
| AI | An AI-enabled start menu | planned | aichat through `gala install`; needs crypt32's ROOT store (TLS) and SH |
| UIA | UI Automation and windows across processes | in progress | `uia_test`, `flaui_test`, `windows_across_test`, `filezilla_flaui`; ARCHITECTURE "UI Automation" |
| SD | Server and desktop workloads kept apart | in progress | `profile=server`, CLI parity, server kernel, bare desktops (the shasha feature only adds) done |
| SSH | SSH as a remote shell | in progress | steps 1-5 in their first form |
| CP | An interactive console prompt | planned | serial line, screen and keyboard, gala-um's terminal; no SSH needed |
| RS | Remote access and sessions | in progress | A, B1, B2, B3a's window stations, presenters, console switch and window state apart from the compositor (server kernel and bare desktops too) done; B3b next |
| GPU | Graphics and GPU | in progress | G0 and G1 done; G2 next; G3 parked |
| BR | A web browser: stock Edge | in progress | stage 1 done: chromium_version, chromium_dump (a page's script runs), chromium_https (a page from the guest's nginx over TLS) and chromium_host (a page from the host network) pass in the gate; stage 2 (a window) under way: chromium_window |
| OF | Microsoft 365 apps and the new Outlook | planned | gap analysis first |

### Robustness, live update, tooling

| ID | Title | State | Note |
|---|---|---|---|
| R1 | Invariants | done | adopted; new code follows them (ARCHITECTURE "Invariants") |
| R2 | Fault injection | done | leftovers in its section; ARCHITECTURE "Fault injection" |
| R3 | Lock-order checking | done | `sync::lockdep`; WORKFLOW "Lock-order checking (lockdep)" |
| R4 | Flight recorder, watchdogs, crash record across reboots | done | `watchdog`, `kernel_watchdog`, `crash_record` |
| R5 | Deterministic simulation in gala-um | done | `cargo xtask sim-test`; one refinement open |
| R6 | Charging resources to an owner | in progress | commit charge, pool and handle quotas, sockets' buffers, completion packets, ProcessQuotaLimits done |
| R7 | Live mocking: clock control, record/replay, device faults | in progress | clock control, record/replay of time and randomness, disk latency done |
| R8 | Chain of trust for updates | in progress | steps 2-6 built; Secure Boot next |
| R9 | Live update, kernel included | in progress | handover milestones (a)-(e) done |
| OT | OpenTelemetry as the telemetry model | in progress | OT1 done; OT2: threads on every event, the process tree |
| PK | `gala install` over winget manifests | in progress | zip/portable installs per user and machine-wide, NSIS installs and a real Inno Setup install (Inno Setup 7.1.1: installed with its Start menu shortcuts and `.iss` association, run, uninstalled), on the winget resolver (selection, dependencies, ARP matching); MSI and MSIX readers |
| DR | Windows drivers, staged and isolated | later | |

## Open items

### U4. gala-um: more than one NT process

**Goal**: the scenarios with child processes or a server next to the
test (`start=`) run in gala-um too: process_test, conpty_test, busybox,
cmd_test, go_exec, the cross-process window tests, schannel_test (its
Nginx), system_test and session_test.

**Done so far**: such scenarios are left out by rule (`NOT_IN_UM`, and
`UM_FLAGS` for the flags gala-um does not apply), never run without
their setup. schannel_test passes in gala-um without its Nginx parts,
the in-process TLS 1.3 and 1.2 server included.

**Next**:
1. A host process per NT process sharing one NT layer (objects,
   handles, the loopback TCP stack), or several address spaces in one
   host process: decide which, in GALA_UM.md.
2. `start=` and child process creation on it; then the scenarios above
   out of `NOT_IN_UM`.

Also needed by CP step 3: programs started from gala-um's interactive
prompt are child processes.

**Done when**: schannel_test and process_test pass in um-test.

### U5. gala-um: a window on the host

**Goal**: `um --window <program.exe>` shows the desktop and the
program's windows in a window on the Linux host, with the host
window's keyboard and mouse as the input, so GUI programs (gui_test,
console_window) run and can be debugged in gala-um.

**Approach**: the NT layer already draws through `nt::platform::Display`
(`screen_size`, `screen_write_row`, `screen_flush`, `screen_changed`);
gala-um's implementation (`kernel/src/hosted`) reports no screen. With
`--window` it backs those calls with a host pixel buffer shown in an
X11 or Wayland window (a small windowing crate, recorded in
`THIRD_PARTY.md`; host code, so `hosted::HostCode` around its
allocations), the window's size as the screen's and its resizes
through `screen_changed`. A host thread turns the window's events into
the desktop's input entry points (`nt::desktop::key_event`,
`wheel_event`, and an absolute pointer call beside the PS/2 driver's
relative `pointer_event`). The window is a presenter in RS B3a's sense
(the desktop's state apart from where it is shown). Without
`--window`, nothing changes: um-test runs in containers with no
display.

**Next** (test first):
1. The `Display` implementation and the window; a host test that a
   frame written through `Display` reaches the buffer, and
   `um --window --screenshot=<png>` (a headless buffer, no host window)
   so the screen scenarios' pixel checks run in gala-um.
2. Input: keys, wheel and an absolute pointer; gui_test and
   console_window out of `NOT_IN_UM` for the headless mode, with input
   scripted as xtask scripts it for QEMU.
3. With U4: programs started from the start menu and windows across
   processes (windows_across_test, uia_test).

**Done when**: gui_test and console_window pass in um-test headless,
and `um --window` shows gui_test's window on a Linux desktop, driven
by the host's mouse and keyboard. Observability: the window's size,
resizes and input events under `--trace`, and a failure to open the
host window reported with the display it tried.

### U6. Windows applications in CI through gala-um

**Goal**: an unmodified Windows application and its UI Automation tests
(FlaUI under `dotnet test`) run headless on a stock GitHub Actions
Ubuntu runner through gala-um, with no KVM, QEMU or root, and leave
screenshots, test results and gala's own diagnostics as artifacts. A
fast first pass on every commit, not a substitute for testing on
Windows: a pass in gala-um is not a pass on Windows, and the ledger
says what the application used that gala lacks.

**Needs**: U4 (a test runner driving the application is two processes;
`dotnet test` starts its test host as a child), U5 (headless drawing,
screenshots, scripted input).

**Next** (each step test first):
1. Host directories as drives: `um --mount=<drive or directory on
   C:>=<host directory>` backing part of the namespace with a host
   directory, read and write, so the application, the test assemblies
   and the outputs pass between the runner and gala (today C: is ramfs
   filled file by file with `--module=`). Scenario: a program reads a
   mounted file and writes one the host checks.
2. Networking to the host: host sockets behind AFD (gala-um's TCP stack
   reaches only its own loopback today), for applications that talk to
   a test server or the network.
3. Memory: the memfd's size (`MEMORY_SIZE`, 1 GiB) as an option, enough
   for .NET plus a test runner and for Electron/Chromium applications.
4. Acceptance in um-test: `flaui_test`, then `filezilla_flaui`,
   headless.
5. Packaging: a release bundle (the `um` binary, gala's DLLs and
   programs, the default registry seed) and a GitHub Action that runs a
   command against a mounted directory and uploads the artifacts: the
   screenshots, the test results, and on failure the ledger, the event
   stream, crash reports and the last screen. A sample repository whose
   workflow runs a small application's FlaUI tests is the done check.
6. Flaky UI tests: a failing run's `--record` file kept as an artifact,
   so it replays deterministically on a developer's machine (R5, R7).

**Done when**: the sample repository's workflow passes on a stock
`ubuntu-latest` runner, and a deliberately broken test leaves a
screenshot, the ledger and the event stream as artifacts that explain
it.

**Licensing**: distributing gala-um for use in other people's pipelines
needs a licence for gala (README "License") and honours the obligations
of what the bundle carries (`THIRD_PARTY.md`). That is the owner's
decision, not yet taken; step 5 waits on it.

**Design**: GALA_UM.md ("How a Windows program runs inside a Linux
process", "Limits and open questions"); ARCHITECTURE "UI Automation".

### HS. Hygiene sweeps

**Goal**: regular reads of existing code for one class of defect at a
time, instead of waiting for flaky scenarios to point at each one.

**Done so far**:
- The first sweep: the early primitives for single-CPU assumptions,
  with `smprace_test` (five rounds, each failing before its fix;
  findings and checklist in WORKFLOW "Hygiene sweeps").
- Fixed without a failing test (invariants in ARCHITECTURE "Waits"):
  NtReleaseMutant and mutant abandonment changed the owner outside the
  dispatcher lock; the console's `settle` reset `input_ready` after
  checking the queue in an earlier lock hold; and the out-of-order
  IrqMutex release detector was added (debug kernels panic: `lockdep:
  releasing X turns interrupts on while Y is still held`).
- Checked and found sound: the scheduler's `prepare_wait`/`wait`/`wake`
  protocol (preemption between them included), `on_cpu`, KArc's counts,
  the heap's and frame allocator's single locks, keyed events,
  semaphores (`update_signal`), APC queueing against the alertable
  flag, termination requests against `wake_if_terminating`,
  `add_thread` against `terminate`, the last-thread decision.

**Next** (the first sweep's leftovers; each needs a test against
Windows first):
1. (Done: `iocp_test`.) NtRemoveIoCompletion waits end when the port's
   last handle closes (STATUS_ABANDONED_WAIT_0; GetQueuedCompletionStatus
   fails with ERROR_ABANDONED_WAIT_0 and a NULL lpOverlapped).
2. (Done, fixed without a failing test: no user-mode test can hold a
   thread between its exit's start and its end.) An APC queued to a
   thread just as it exits was accepted (STATUS_SUCCESS) and never ran:
   the exit is now marked under the dispatcher lock, which the queuing
   checks under; APCs still queued are discarded at exit, as on
   Windows.
3. (Done: `iocp_test`.) NtRemoveIoCompletion writes (probes) the
   caller's outputs before taking a packet: a bad pointer fails with
   STATUS_ACCESS_VIOLATION and the packet stays queued.
4. (Done, fixed without a failing test, as 2.) NtSuspendThread on an
   exiting thread could count a suspension the thread never saw: the
   exit is marked under the suspend count too, which suspending checks
   under (THREAD_IS_TERMINATING).
5. Further sweeps: parked until gala runs live (the owner).

**Done when**: each leftover has a test whose results match Windows and
passes on gala and gala-um.

### S4. SMP, the rest

**Goal**: the remaining multiprocessor pieces, each when a benchmark or
program needs it.

**Done so far**: suspending a thread running on another CPU works (it
parks at its next tick).

**Next**: per-CPU run queues with stealing and per-CPU timer bookkeeping
(when a benchmark shows the shared queue costs); ranged (invlpg)
shootdowns and PCIDs; boot scenarios on 4 CPUs.

**Design**: ARCHITECTURE "Threads and scheduling", "Processors", "Known
limits".

### H11. busybox httpd and wget over loopback

**Goal**: sockets from C programs and busybox's networking: busybox-w32's
httpd serves a directory on gala and its wget (and a tcc-built Winsock
client) fetch files from it over 127.0.0.1, in one boot, output as on
Windows.

**Done when**: a scenario runs httpd, fetches files with wget and the
tcc-built client, and compares them byte for byte.

**Done so far**: the same with Linux's static busybox on the Linux
personality (LX): `lx_httpd` runs `busybox httpd -f -p 127.0.0.1:8080`,
fetches a small and a 106 KiB file with `busybox wget` (byte for byte,
and through a pipe), sees a 404 fail, and stops the server with kill;
Linux sockets on the kernel's TCP stack (`lx::socket`, `lx_socket`).

**Next**: busybox-w32's httpd and wget on the NT side, and the tcc-built
Winsock client.

### P1. Unmodified prebuilt programs

**Goal**: run `.exe` files as their makers ship them (fetched by xtask,
`PREBUILT`, SHA-256 pinned; WORKFLOW "Prebuilt programs").

**Done so far** (scenarios; pending ones run with `cargo xtask
boot-test <name>`, see "Pending scenarios" below):
- Caddy 2.8.4: `caddy`, `caddy_http`.
- Frame-based exception handling in ntdll (`seh_test`), what the JDK and
  .NET need first.
- Step 1, Temurin JDK 21: `java_version`, `java_hello`, `java_http`;
  `java_stress` (pending).
- Step 2, .NET 8: `dotnet_info`, `dotnet_hello` (JIT, GC, exceptions,
  threads; no ICU: the NLS APIs, en-US only).
- Step 3, PowerShell 7.4.22: `pwsh_hello` (one command,
  non-interactive).
- Step 4, Elasticsearch 8.15.3 on the JDK: starts and answers the host's
  GET / (`elasticsearch`, pending: 3 GiB, about 8 minutes under TCG).
- Step 5, SQL Server 2022 LocalDB: first and second start as
  SqlUserInstance.dll starts them, a TDS query from `tds_test`,
  SHUTDOWN, a database kept across a reboot (`sqllocaldb`,
  `sqllocaldb_persist`, pending).
- Step 6, FileZilla Server 1.12.6 (`filezilla_server`); the client is
  item W3.
- Step 7, Win32-OpenSSH 9.8.3.0p2: key and password logins
  (`sshd_login`, `sshd_password`); the remote shell is item SSH.
- PostgreSQL 17.2: initdb, the server, the host's psql (`postgres`).
- MinGit 2.47.1: init, add, commit, log (`mingit`, pending); a clone
  over https on Schannel (`git_https`, ROADMAP TLS).
- Git Bash (PortableGit 2.47.1): **parked**; msys-2.0.dll loads and
  bash gets past its startup deadlock, then dies with an access
  violation at address 0 after looking up kernelbase's VirtualAlloc2
  (a stub) and NtQueryInformationProcess(ProcessVmCounters).

**Next**:
1. PowerShell's interactive prompt: console input (a keyboard on the
   serial line or a ConPTY session, item SSH). Still in its ledger:
   mpr.dll, amsi.dll and wldp.dll (absent means none), LCTYPEs 0x1006
   and 0x100d, probes for newer APIs.
2. Elasticsearch's node name from HOSTNAME (gala's boot cannot set
   environment variables yet); Netty's MAC lookup
   (GetAdaptersAddresses).
3. LocalDB's non-fatal errors: the startup assertion HadrWsfcUtil.cpp:516
   'fSuccess' (a dump at every start, cause unknown), the service master
   key (ESECCrypto error 21, LastOsError 6), FSCTL_FILESYSTEM_GET_STATISTICS
   (17053) and IOCTL_VOLUME_GET_GPT_ATTRIBUTES (refused), the CLR (6511:
   no mscoree.dll); sqlservr's exit code 0x191 to check against Windows.
4. .NET's ledger: FlsGetValue2, InitializeContext2, QueueUserAPC2, the
   diagnostics server's named pipe, job queries, the event log.
5. Git Bash when resumed: VirtualAlloc2 (MEM_RESERVE_PLACEHOLDER and the
   address requirements Cygwin's mmap and stacks use),
   ProcessVmCounters, then fork emulation; the `bin\bash.exe` launcher
   exits 1 without output.

**Done when**: each program's scenario passes; pending ones are moved
into `cargo xtask test` when CI can afford them.

**Design**: ARCHITECTURE "NT personality", "Registry", "Processes started
by programs"; SECURITY.md for OpenSSH and PostgreSQL.

### W3. FileZilla

**Goal**: the FileZilla 3.69.3 client (wxWidgets) runs as on Windows.

**Done so far**:
- The main window with toolbar, Quickconnect bar, local and remote
  panes, queue, status bar, menus, combo box drop-downs, tooltips, rich
  edit message log, snap; it connects to FileZilla Server in the same
  boot over TLS 1.3 and lists the remote directory (`filezilla`, a
  pixel screen check, and `filezilla_flaui`, driven through UI
  Automation; both pending).
- Rich edit controls: msftedit.dll and riched20.dll (`richedit_test`).

**Next**:
1. DefWindowProc's WM_WINDOWPOSCHANGING sends WM_GETMINMAXINFO and keeps
   a thick-framed window within its track sizes (FileZilla's minimum is
   700 x 500, so its snapped half would be 700 wide as on Windows; the
   desktop's docking test and the scenario's checks change with it);
   SetWindowPos's WM_NCCALCSIZE.
2. Rich edit: RTF streams, undo, the clipboard (user32 has none yet),
   OLE callbacks (wx asks for them).
3. A fetchable installer (see "Known issues and stopgaps").

**Done when**: `filezilla` and `filezilla_flaui` pass from a fresh
checkout.

**Design**: ARCHITECTURE "Windows on the screen", "UI Automation".

### W4. The Windows look

**Goal**: imitate Windows 11's visuals as closely as the clean-room rules
allow.

**Done so far**: Windows 11's shell in the compositor (light captions,
a centred translucent taskbar, a centred start menu, the clock's
format), Selawik text baked by xtask, the owner's fortress SVGs (start
button, boot splash, wallpaper mark), four generated wallpapers chosen
per boot, maximize/restore; proportional Selawik in gdi32 with
Windows' metrics (`gdi32_test`).

**Next**:
1. Step 3: uxtheme with themes on and Windows 11-style control drawing.
2. Later: resizing windows by the frame, keyboard focus per window
   (SH's step 1 needs it); the shell itself leaves the kernel in item SH.

**Design**: ARCHITECTURE "Windows on the screen".

### SH. The shell in user mode

**Goal**: the kernel keeps what win32k keeps on Windows (window state,
Z order, focus, message routing, input queues, UIPI: what enforces
isolation between processes) and nothing of the shell. The taskbar,
start menu and wallpaper run as a user-mode program, as explorer.exe
does; the compositor follows later, as DWM is a user-mode process.

**Why**: a fault in the shell restarts a process instead of panicking
the kernel (it matters more once the start menu holds an LLM client,
item AI); the shell launches programs under the logged-on user's token
(the start menu passes no token to `process::launch` today); less
kernel state to carry across a handover (item R); the shell exercises
gala's user32, gdi32 and GalaDesktopAction as any program does and can
be replaced like one; 2.5 MB of the kernel (the `shasha` feature's
shell part) leaves it.

**Next** (each step test first, screen and UIA checks):
1. `shasha.exe` (`user/shasha`): the taskbar, the start menu and the
   wallpaper as ordinary topmost windows of a user-mode process, on
   the existing calls (GalaDesktopAction's Start, Windows, Programs,
   Activate, Minimize/Maximize/Restore, Close, Snap, Wallpaper; the
   window and WinEvent calls for the taskbar's list). Started per
   session under its user's token; the kernel's shell code removed in
   the same change. Needs keyboard focus per window (W4): the start
   menu gets keys as a focused window, not through the input thread.
2. Supervision: the session (RS B4's session manager, the kernel until
   then) restarts a shell that ends, with its exit status and crash
   record in the event stream; windows stay usable meanwhile (Win+arrows
   and the `gala` command do not depend on the shell). A scenario kills
   the shell and checks it comes back with the same windows listed.
3. What must not depend on the shell stays out of it: Ctrl+Alt+Del, a
   later lock screen and consent prompts belong to the kernel or a
   separate trusted process (RS's secure desktop).
4. Later: the compositor in user mode along RS B3a's presenter seam
   (shared-memory surfaces between programs and the compositor, one
   copy to the Display seam per frame, damage tracking kept), after
   B3a and GPU G0. Until then composition stays in the kernel.

**Done when**: a scenario boots to a desktop drawn by `shasha.exe`
under the boot user's token, starts a program from the start menu and
switches to it from the taskbar (UIA), kills the shell and sees it
restarted; the server kernel and `profile=server` start no shell.

**Design**: ARCHITECTURE "Windows on the screen" (shasha), "Server and
desktop workloads", "Plan: sessions and remote shasha" (presenters).

### AI. An AI-enabled start menu

**Goal**: the start menu talks to any standard LLM API (OpenAI-style,
Anthropic, Gemini, Ollama or any OpenAI-compatible endpoint, local
models included) with streamed replies and tool calls that act on the
desktop; the same assistant is a console command first (CLI parity,
the server edition too).

**Approach**: no web view and no browser engine in the image (gala is
under 10 MB, Chromium over 300 MB; BR brings Edge and WebView2 as an
installed package, never part of the image). The LLM client is an
existing CLI, **aichat** (sigoden/aichat, MIT or Apache-2.0, Rust, one
exe): streaming over server-sent events, many providers, function
calling (its llm-functions tools are scripts), a REPL, one-shot use and
`aichat --serve`, a local OpenAI-compatible server. It is installed
with `gala install sigoden.AIChat` (winget-pkgs: a zip with a portable
`aichat.exe`, which PK's stages 1-2 handle), not shipped. The tools are
the `gala` commands (start, windows, activate, snap, close, ...) and
UIA for driving programs, so the model acts only through what CLI
parity already exposes.

**Next** (each step test first, the ledger leading):
1. Install and run: a scenario installs `sigoden.AIChat` from a
   mirrored manifest and zip (`--source` with an `installers\` mirror,
   as `pk_install_zip`; xtask fetches the real zip, never committed)
   and runs `aichat` one-shot and streaming against an in-guest mock
   endpoint over plain HTTP (a test program serving a scripted SSE
   stream, a tool call included), so streaming is tested without TLS.
   Open: the manifest's `Microsoft.VCRedist.2015+.x64` dependency (a
   Burn exe, PK stage 3) and the UCRT (ucrtbase.dll ships with Windows,
   not with gala; scenarios copy the JDK's app-local copy today) need a
   decided home in the image or the installer.
2. HTTPS: aichat verifies servers with rustls against Windows' ROOT
   system store (rustls-native-certs through crypt32), which gala opens
   empty today; TLS step 5's ROOT store makes it work (aichat's TLS is
   its own, so it does not wait for winhttp). A scenario against the
   in-guest Nginx with the test CA in gala's roots.
3. `gala ask`: the console entry point (provider and model from
   configuration, the key stored with DPAPI, never in a plain file),
   streaming to the console, the `gala` commands as tools; every
   request, tool call (arguments and result) and confirmation in the
   event stream; actions that close, kill or delete ask first; text a
   tool returns is treated as untrusted (prompt injection).
4. The start menu's chat panel in `shasha.exe` (after SH step 1): a
   native panel (input, streamed text, a Markdown subset: paragraphs,
   lists, code blocks) over `aichat --serve` on localhost; the panel
   holds no TLS or provider code. If native drawing is not enough, a
   small HTML/CSS renderer without JavaScript (litehtml, BSD) before
   any browser engine.
5. Later: WebView2 rendering when Edge is installed (BR stage 4), as an
   option the start menu never depends on.

**Done when**: from a fresh image, `gala install sigoden.AIChat` then
`gala ask` streams an answer from the mock endpoint and performs a
`gala` tool call with its confirmation; the start menu does the same
through UIA; both leave the request and tool events in the event
stream.

**Design**: to be written in ARCHITECTURE with step 3 (the assistant's
process, tools, confirmation and key storage); "Installing software"
(PK); "Schannel (TLS)" for the root store.

### UIA. UI Automation and windows across processes

**Goal**: screen tests drive programs as testers do on Windows: find
elements by AutomationId, Name and ControlType, invoke them through
patterns and read their state, so one test runs on Windows and on gala
(FlaUI as the driver).

**Done so far**:
- Windows across processes, steps 1-5: the kernel window registry with
  global HWNDs, PostMessage and SendMessage across processes with
  marshaling (`windows_across_test`), UIA driving another process
  (`uia_test`), FlaUI 5.0.0 on gala's .NET 8 (`flaui_test`).
- gala's UIAutomationCore.dll: IUIAutomation through IUIAutomation6
  over the HWND tree, conditions, FindFirst/FindAll, tree walkers, cache
  requests; Invoke, Value, Toggle, Selection/SelectionItem, ScrollItem,
  ExpandCollapse (menus), Window, Transform, Grid/Table and their item
  patterns; title bars; owned windows under their owner; UIA events
  from WinEvents (`winevent_test`); every message sent with
  SendMessageTimeout and SMTO_ABORTIFHUNG.
- `filezilla_flaui`: the first real program driven through UIA.
- Providers inside programs, in the client's process and in others:
  WM_GETOBJECT answered through UiaReturnRawElementProvider, the
  provider's properties first (across processes through gala's
  GALA_UIA message), the window's (UiaHostProviderFromHwnd) where it
  gives none; a fragment root's fragments as the window's descendants,
  with their Invoke and Value patterns (`uia_test`).
- `chromium_uia`: Chromium's page read and its link invoked through
  Chromium's own provider.

**Next**:
1. More of what providers give, as programs need it (Chromium's tree
   is read and its links invoked: `chromium_uia`): more patterns
   (Toggle, ExpandCollapse, Selection, Text), ElementProviderFromPoint,
   GetFocus, a root provider's own patterns, events from providers
   (UiaRaise*); oleacc's CreateStdAccessibleObject.
2. IUIAutomation3..6's event handlers and handler groups.
3. Combo box and tree-view items, GetParent of owned popups, the system
   menu in the title bar.
4. The other screen tests rewritten on FlaUI.
5. The rest of the limits in ARCHITECTURE "UI Automation" ("Limits",
   "Plan: windows across processes"): UiaRaise*, real caching, the
   LegacyIAccessible, RangeValue and Text patterns; HWND_BROADCAST,
   SMTO_ERRORONEXIT, IsHungAppWindow, owned windows destroyed and
   hidden with their owner, minimized windows moved to -32000,-32000.

**Design**: ARCHITECTURE "UI Automation", "Plan: windows across
processes"; WORKFLOW "FlaUI".

### SD. Server and desktop workloads kept apart

**Goal**: the server edition never draws a desktop, and everything the
desktop can do is reachable from the command line.

**Done so far**:
- `profile=server` (scenarios `server_profile`,
  `server_profile_windows`): nothing draws a desktop even with a screen,
  each session's keeps its windows as the server kernel's do, `console=`
  starts no conhost.exe.
- **CLI parity** (scenario `gala_cli`): one kernel entry point,
  GalaDesktopAction (0x7f58: Start, Windows, Programs, Activate,
  Minimize, Maximize, Restore, Close, Snap, Move, Wallpaper), which
  shasha's taskbar, caption buttons, Win+arrows and start menu call and
  the `gala` command (user/gala, in every image) calls too; `gala
  ps`/`gala kill`.
- UIPI on cross-process messages and on GalaDesktopAction's window
  actions; `gala start` runs the program under a copy of the caller's
  token (`uipi_test`, SECURITY S6).
- The server kernel (cargo feature `shasha` off; scenario
  `server_kernel`): no compositor, wallpapers or start menu (2.5 MB
  smaller); it keeps GUI programs' windows and their pixels as on a
  desktop nobody shows (`nt::winstate`, scenario `server_windows`), with
  no shell (window actions STATUS_NOT_SUPPORTED). A server image without user32/gdi32 is not planned: many
  console and server programs import them, and loading them starts
  nothing; only wallpapers, fonts and GUI-only programs (conhost.exe,
  FileZilla) stay out of a server image.

- The shasha feature only adds: where nothing can show a desktop (no
  screen and no `presenter=`: BIOS boots, gala-um; `profile=server`)
  the desktop kernel's desktops are bare, the window state alone shown
  by none, exactly as the server kernel's (scenarios `noscreen_windows`,
  `server_profile_windows`, the same program as `server_windows`;
  user32_test's line on BIOS and in gala-um).

**Next**: the wallpaper kept across boots; a `gala` command for each new
shasha feature (it comes first).

**Design**: ARCHITECTURE "Server and desktop workloads".

### SSH. SSH as a remote shell

**Goal**: Win32-OpenSSH's sshd on gala is something to live in: a
server, an interactive shell, file transfer, forwarding, a client.

**Done so far**:
- Step 1: `sshd_server`: sshd started at boot as SYSTEM without `-d` on
  port 22, three logins in a row and two at once; restarted by the
  kernel's `restart=` supervisor (`service_restart`); logs in
  `%ProgramData%\ssh\logs` copied to the serial log by `logtee=`;
  `events=net,logon` checked. `cargo xtask serve sshd` forwards host
  port 2222 (WORKFLOW "SSH into a running gala").
- Step 2: ConPTY (CreatePseudoConsole, ResizePseudoConsole,
  ClosePseudoConsole; conhost.exe `--headless`; `conpty_test`) and
  `sshd_shell` (busybox sh under ConPTY: typing, line editing, Ctrl+C
  interrupting `sleep 60`, exit; `conpty=trace`).
- Step 3: `sshd_sftp` (put a 200 KiB file, ls, get it back byte for
  byte, rm); `sshd_scp` (the host's `scp -p` both ways over SFTP and the
  legacy protocol, `-O`, which runs gala's System32\scp.exe: bytes,
  modification and access times; gala's files keep an access time of
  their own for it, ARCHITECTURE "File times").
- Step 4: `sshd_forward`: `ssh -L` (direct-tcpip: sshd-session connects
  over loopback to `sshfwd_test serve`) and `ssh -R` (tcpip-forward:
  sshd-session listens on gala's 127.0.0.1:7001, `sshfwd_test connect`
  reaches an echo server on the host), 200 KiB each way with half-closes
  (the FIN follows sshd's still-pending overlapped send: ARCHITECTURE
  "AFD").
- Step 5 begun: `sshd_client`: gala's own ssh.exe logs on to gala's
  sshd over loopback with the key in galatest's profile, accepting the
  host key into `~/.ssh/known_hosts` and then checking it strictly.

**Next**:
1. sshd in the Docker/server image.
2. The shell: an interactive prompt in gala's cmd.exe (busybox sh is
   the shell today); resize over ssh exercised (the scenario's client
   has no tty, sshd sends a 0 x 0 resize); full-screen programs that
   draw with the cell APIs (SetConsoleCursorPosition,
   WriteConsoleOutput) reaching the terminal (only VT text does now);
   `gala` commands in the shell.
3. File transfer: permissions (scp -p's mode, ACLs), large files (more
   than the 300 KiB the scenarios move).
4. Port forwarding: dynamic (`ssh -D`), agent forwarding if cheap.
5. The client: gala's ssh.exe connecting to the host's sshd (a host
   server for xtask to run, like the echo server `sshd_forward` uses),
   ssh-agent, an interactive client session under a pty.
6. Observability: sshd's and sshd-session's logs in failed logs,
   connection and auth events in the event stream, the ledger's
   ssh-related gaps in the report.

**Done when**: each step's scenario passes; host keys persist across
boots (`persist`) and sessions survive a kernel handover (R9 (e)
carries sockets and pipes; consoles not yet).

**Design**: ARCHITECTURE "Pseudo consoles"; SECURITY "Accounts and
logon".

### CP. An interactive console prompt

**Goal**: a shell prompt typed at gala's own console, with no SSH: on
the serial line (QEMU with `-nographic`, QEMU in a container, a VM's or
a PC's serial port), on the screen with a PS/2 keyboard when there is
no desktop (the server edition, `profile=server`), and in gala-um on
the Linux terminal it was started from. Today the serial line is
output only (`arch/x86_64/serial.rs` has no receive path), the
sessionless console's reads return nothing (ARCHITECTURE "Consoles"),
and the only interactive shell is busybox sh over sshd (SSH step 2).

**Approach**: one path for every source. Each source is a terminal (a
byte stream with VT sequences and a size) attached to a pseudo console,
so programs get a real console session exactly as under sshd:
conhost.exe `--headless` between the terminal and the processes
(ARCHITECTURE "Pseudo consoles (ConPTY)"). The terminal is the core
terminal object of LX seam 1, so a Linux shell can use the same
console later.

**Next** (each step test first):
1. Serial input: the UART's receive path (polled, like the other
   devices), its bytes into a terminal; the console prompt as a boot
   option (a kernel flag naming the source and the shell, for example
   `prompt=serial`), busybox sh first. Scenario: xtask writes
   `echo hi` and `exit` to QEMU's serial chardev and reads the echo and
   the output. Decide where kernel log lines go while a prompt owns the
   line (held as the serial discipline holds them today, or a second
   port), so the log never lands inside a typed line.
2. The screen without a desktop: a text terminal drawn on the
   framebuffer (the boot log's screen console) and the PS/2 keyboard
   feeding it, when shasha is off or `profile=server`. Scenario: keys
   sent through the QEMU monitor (`sendkey`), a screen check of the
   result.
3. gala-um: `um --prompt` attaches the host terminal (raw mode, the
   window size from `TIOCGWINSZ` and SIGWINCH) as the terminal.
   Builtins work at once; running programs from the prompt needs child
   processes in gala-um (U4). Scenario: um-test drives it through a
   host pty.
4. Logon: the prompt asks for a user and password (LogonUserW, as
   sshd's password logon) and runs the shell in that user's token and
   profile; a test-only flag skips it for scenarios.
5. gala's cmd.exe as the shell: its interactive prompt (`PROMPT`, line
   editing through the console's cooked mode, `cd`, `dir`, `set`,
   batch files), shared with SSH step 2; busybox sh until then.

**Done when**: a scenario per source (serial, screen, gala-um) logs in,
runs a program from cmd.exe's prompt and exits cleanly.
Observability: the prompt's logons and the shell's start and exit in
the event stream (`events=logon`), the terminal's bytes with
`conpty=trace`, and a stuck prompt visible in the state dump (the
reading thread and its wait).

**Design**: ARCHITECTURE "Consoles" (with the serial line's
discipline), "Pseudo consoles (ConPTY)"; SECURITY "Accounts and logon";
LX seam 1.

### RS. Remote access and sessions

**Goal**: sessions own desktops; a desktop is shown by a presenter
(console, remote shasha client, none, test); remote users get their
own session and shell.

**Done so far**:
- A: SSH steps 1-2 (item SSH).
- B1: session ids and per-session named objects (`nt::session`, each
  session's BaseNamedObjects, BNOLINKS, a process's session from its
  primary token, PROCESSES version 3 carrying it across a handover;
  `session_test`).
- B2: a desktop per session (`nt::session::Desk`: window registry,
  WinEvent hooks, shasha's desktop, reached through `Process::desk`; the
  `REGISTRY`, `STATE` and `DESKTOP` statics gone); only the console
  session's desktop is drawn and gets input; window calls never cross
  sessions (`session_test`); `events=window`; the core/NT process split
  (`nt::process::Core`, which Linux processes use too).
- B3a, window station and desktop objects (`nt::winsta`): WinSta0 and
  its Default desktop in every session, SYSTEM's `Service-0x0-3e7$` in
  session 0, a non-interactive logon's station made on first use; a
  process's station from an inherited handle, STARTUPINFO.lpDesktop,
  its parent or its logon session; user32's GetProcessWindowStation,
  Open/CloseWindowStation, SetProcessWindowStation, EnumWindowStationsW,
  GetThreadDesktop, SetThreadDesktop, OpenDesktop, OpenInputDesktop,
  CloseDesktop, EnumDesktopsW, GetUserObjectInformationW/A over gala
  calls (`winsta_test`, `winsta_service`).
- B3a, presenters (`desktop/present.rs`, GalaSession): every session's
  desktop keeps its windows and their pixels whether or not anything
  shows it (a session-2 window kept while not shown, `presenter_test`);
  presenters Console (the screen), None and Test (a frame in memory,
  read back with GalaSession's SESSION_FRAME); `presenter=test|none` for
  a headless desktop, whose frame xtask's screen checks read with QEMU's
  `memsave` (gui_test's screen check on a BIOS boot,
  `gui_test_headless`).
- B3a, the console switch (`desktop::switch_console`, `gala attach
  <session>`, GalaSession): the screen and the PS/2 keyboard and mouse
  move to another session's desktop, the one shown before goes to none
  and keeps its windows, KUSER_SHARED_DATA's ActiveConsoleId
  (WTSGetActiveConsoleSessionId) follows; a Test frame's desktop takes
  injected input (GalaSession's SESSION_INPUT). `presenter_test`: a
  click injected into session 2's frame, session 1's window on the
  screen, `gala attach 2`, PS/2 clicks reaching session 2's window,
  back to session 1 with its window kept (a screen check).
- B3a, the window state apart from the compositor (`nt::winstate`, in
  both kernels; shasha's `Desktop` draws it): the server kernel keeps
  each session's windows, their last pixels, Z order, focus, capture and
  input as on a desktop nobody shows (GalaSession's query reports them,
  frames and Test/Console presenters refused; `server_windows`,
  presenter_test's `server` mode; host tests in `winstate/tests.rs`);
  so does the desktop kernel wherever nothing can show a desktop (bare
  desktops: no screen and no `presenter=`, `profile=server`, gala-um;
  `noscreen_windows`, `server_profile_windows`).

**Next**:
1. B3a: `presenter=` in gala-um
   (screen checks there; its sessions' desktops are bare today); xtask's
   headless screen checks sending their input through SESSION_INPUT
   instead of PS/2; Test frames recorded over time; the console switch's
   open ends: WM_WTSSESSION_CHANGE (WTSRegisterSessionNotification),
   a switch without a screen (`presenter=test`), held keys released with
   WM_KEYUP. The presenter seam is also where the compositor later
   leaves the kernel (SH step 4).
   Window stations' open ends: DACLs on stations and desktops;
   CreateWindowStationW and CreateDesktopW stay unsupported until BR
   stage 3 (Chromium's sandbox) needs them, with SwitchDesktop and
   desktops with windows of their own; UOI_USER_SID; clipboard and atoms
   per station.
2. B3b: remote shasha: the compositor as a crate that also builds for
   the host, a versioned protocol, the server end (also in the server
   kernel, which has no compositor), a host client (winit).
3. B4: a session manager, `wtsapi32` (only WTSGetActiveConsoleSessionId
   exists), `gala sessions/logoff/disconnect` (`gala attach` exists, for
   SeTcbPrivilege holders: B4 decides who may attach where), a user per
   session (another session's start menu starts nothing yet); and B1's
   open ends:
   DosDevices per session and logon, the SeCreateGlobalPrivilege check for
   `Global\` file mappings outside session 0, DACLs on session directories
   and on processes (a standard user's ProcessIdToSessionId of a SYSTEM
   process succeeds).
4. B5: sessions across a kernel handover (session, desktop and window
   state carried; the window registry is refused today), remote clients
   reconnecting after it.
5. Later: several desktops per session, a secure desktop, clipboard,
   seamless windows, RDP proper. Interim, with no gala work: QEMU's
   `-vnc`.

**Done when**: B3b: an xtask test client connects through a forwarded
port, gets a test program's window (title, pixels), clicks, the program
reports WM_LBUTTONDOWN at the right client coordinates, and a reconnect
gets the same window back. B4: two remote clients each log on to their
own session and shell.

**Design**: ARCHITECTURE "Plan: sessions and remote shasha".

### GPU. Graphics and GPU

**Goal**: software rendering first, so everything works on every VM and
server; paravirtual hardware next; vendor GPU drivers (WDDM) never in
the kernel.

**Done so far**:
- G0: `dxgi.dll`, `d3d11.dll`, `d3d12.dll`, `dcomp.dll` as stubs failing
  with DXGI_ERROR_UNSUPPORTED (outputs NULL, each call in the ledger);
  `gpu_stub_test`; Chromium's GPU process takes it as "no GPU".
- G1, virtio-gpu 2D: the modern virtio-pci transport, the driver over a
  `Transport` trait host-tested against a fake device, `display=virtio`
  (default `fb`), `events=display`, `gputrace`, `gpureset=`; scenarios
  `virtio_gpu`, `virtio_gpu_reset`.

**Next**:
1. G1's rest: shasha's pointer on the hardware cursor,
   WM_DISPLAYCHANGE and refitting maximized windows on a resize (a test
   resizing the display through QEMU's monitor), several scanouts as
   monitors, device interrupts instead of spinning, the boot splash on
   virtio-gpu (the screen is black from the driver's start until the
   desktop draws).
2. G2, software 3D for every program: `vulkan-1.dll` (Khronos loader),
   Mesa lavapipe (fallback SwiftShader), DXVK (d3d9/10/11, dxgi),
   vkd3d-proton (d3d12), Mesa llvmpipe `opengl32.dll`; PREBUILT,
   registered as on Windows; `vk_test`, `d3d11_test`, `gl_test` with
   screen checks, then DXVK's and vkd3d-proton's own tests. WARP is
   ruled out (not redistributable).
3. G4, GPU presentation: DXGI flip-model swap chains as resources shasha
   composes; then BR stage 5.
4. G3, virtio-gpu 3D through Venus: **parked** (deferred by the owner
   until there is spare capacity; hardware partitioning and passthrough
   are out of scope).

**Done when** (each stage): its tests pass; per-context submit/fence
events and GPU hangs (a fence late by N s) in crash reports; `gala gpu`.

**Design**: ARCHITECTURE "Plan: graphics and GPU" (security: contexts
as kernel objects on `\Device\Gpu0`; handover: devices reset, programs
see device-lost).

### BR. A web browser: stock Microsoft Edge

**Goal**: stock Edge runs. Edge is Chromium plus Microsoft's services,
so the open-source Chromium build (a portable zip) goes first and finds
most gaps. Binaries are fetched by xtask, never committed, run only in
QEMU.

**Done so far** (stage 1 under way; ARCHITECTURE "A web browser:
Chromium, then Edge", WORKFLOW "Chromium (BR)"): PREBUILT `chromium`
(snapshot Win_x64 1733063, 159.0.8140.0, with published symbols);
scenarios `chromium_version` (`--dump-dom about:blank`) and
`chromium_dump` (a local page changed by its script: V8 runs), both in
the gate, in
`SINGLE_THREAD_TCG`, 3 GiB, 900 s; `browser_api_test` and `dwrite_test`
cover what was added. chrome.exe starts; chrome_elf.dll's early security
and Crashpad come up (the handler runs as a child with an inherited
handle list); chrome.dll loads; the profile's process singleton is
taken; DirectWrite and Skia's font manager come up on Selawik; the
browser's main start-up runs to its end, every delay import it reaches
on the way a real export; Crashpad's client registers with its handler
(TransactNamedPipe as one operation) and its handler snapshots crashes
(PEB.Ldr, the unload trace, NtOpenThread); the browser launches GPU,
utility (network, storage, audio) and renderer processes, all sharing
one chrome.dll layout; renderers start V8 (growable function tables)
and create script contexts; crashes print symbolizable stacks
(dbghelp's StackWalk64) and leave a crash report even when Crashpad's
filter ends the process. `chromium_version` passes and is in the gate:
`--dump-dom about:blank` prints `<html><head></head><body></body></html>`
and chrome.exe exits with 0, in about 250-280 s of its 900 under TCG
(840 s before two fixes the traces found: overlapped pipe writes
blocked the browser's IO thread, 149 s of a run inside NtWriteFile; and
`thread::current` scanned every thread on each system call, 36% of all
samples).

**Next** (each stage test first, the ledger leading):
1. Headless Chromium. Its local goal is met (`chromium_dump`: the DOM
   of a page its script changed, about 410 s; `chromium_https`: an
   `https://` page from the guest's nginx, its CA in crypt32's ROOT
   store, about 320 s; `chromium_host`: `http://10.0.2.100/` from
   the host by name, `gala-host.test` from the hosts file, `xtask
   serve-page`, about 165 s); AF_INET6 sockets exist (N3 steps 1 and 2:
   ::1, dual stack; IPv6 off the loopback is out of scope for now), and
   the profile's top items on a release kernel (`cargo xtask profile
   --release`: the kernel is 22% of samples, 124 s in the kernel
   against 215 s for the debug kernel the gate boots, whose lockdep
   alone is 13% of kernel samples). Done: the run edits of
   `Vad::commit`, one status read per serial FIFO, demand-paged image
   views (they were the longest interrupts-off stretches). What is left
   is about 1% of a run each: demand-zero faults zeroing frames
   (`on_user_exception`; a pre-zeroed frame list would move it to idle
   CPUs), kernel access to user memory through the space lock and a
   page-table walk per call (`Process::read_u64`, also for each system
   call argument past the fourth; a fault-fixup user copy would read it
   directly), laying out chrome.dll (one whole copy in the kernel heap
   per file version); the kernel is about 20% of samples, the rest
   chrome.dll's own code under TCG; the 2 utilities a run that still end "after 15
   seconds with no connection" (the browser's UI thread answers their
   ping late); the on_device_model utility's delay import of `dxgi.dll`
   (G0). After it:
   what `tools/import_survey.py` still lists (the rest of the delay imports:
   advapi32's services and ImpersonateAnonymousToken, iphlpapi,
   wintrust's catalogs, dbghelp's stack walking, pdh, winhttp's proxy
   resolver, oleacc; the static imports that are still `gala_stubs`:
   crypt32, kernel32's comm and locale formatting); GetAdaptersAddresses
   and NotifyAddrChange (iphlpapi), WSALookupServiceBegin for NLA; the
   renderer and utility processes (Mojo over inherited pipes),
   DirectWrite's custom collections and file loaders (the renderer's
   font proxy), glyph rasterization (CreateGlyphRunAnalysis), V8 under
   TCG. Ledger lines that are harmless so far: `uxtheme.dll!#133/#135`
   (dark mode, probed), `mdmregistration.dll`, `msdia140.dll`,
   `api-ms-win-appmodel-runtime-l1-1-2.dll` (looked up, absent),
   `NtQueryInformationToken` class 39, `CoGetClassObject` of an
   unregistered class. The goal of the stage: `chrome.exe --headless
   --no-sandbox --disable-gpu
   --dump-dom file:///C:/gala/page.html` prints the DOM (done; and an
   `https://` page from the guest's nginx); then a page from the host
   network. Needs: Mojo IPC over named pipes, many threads, large
   virtual memory (V8's reservations, the commit charge), DirectWrite or
   Chromium's font fallback, CNG, Winsock, the JIT (add to
   `SINGLE_THREAD_TCG`).
2. Headful with software rendering (`--disable-gpu`; scenario
   `chromium_window`: a page's colours checked on the screen, then
   Alt+F4). Done: the GPU stubs (GPU G0; the GPU process crashed on
   dxgi.dll's delay import), the renderer's font proxy (DirectWrite
   custom collections over the system's fonts: the renderer found no
   font and ended in `FontCache::CrashWithFontInfo`), the GPU process
   drawing into the browser's window (shared window surfaces, GetDC of
   another process's window; CreateDIBSection over a file mapping).
   Glyph run analysis and outlines (DirectWrite: Skia's text). With
   `--disable-direct-composition` (a stopgap: "Known issues and
   stopgaps"). Chromium's own frame (WM_NCCALCSIZE, custom frames);
   foreign DCs present only what they drew (GetBoundsRect) and keep
   their mapping: whole-window presents per spinner frame starved the
   renderer. Window activation (WM_ACTIVATE gives the focus: Chromium's
   views took no key before it; the desktop's `key` events showed the
   keys arriving). `chromium_window` passes in the gate: the page's
   colours on the screen, then Alt+F4 ends Chromium with 0 (about 2
   minutes of kernel time). Dragging by its own caption (WM_NCHITTEST
   HTCAPTION on a press: GalaWindowDrag; the scenario drags Chromium by
   its tab strip) and resizing by its own border (HTLEFT..HTBOTTOMRIGHT:
   GalaWindowResize; desktop unit test, no outline while resizing yet).
   A click on a link: the scenario clicks window.html's link block
   after the drag, Chromium opens window2.html (its marker in the log,
   its blue on the screen). The page's accessibility tree through
   Chromium's own UIA provider (`chromium_uia`: a UIA client, the API
   FlaUI calls, finds the link by ControlType Hyperlink and Name, reads
   its rectangle and invokes it; Chromium builds its tree once asked,
   about 80 s under TCG). Ledger line left there:
   `oleacc.dll!CreateStdAccessibleObject` (unsupported; Chromium goes
   on). Typing with no IME (imm32 has no input contexts):
   `chromium_window` clicks a text field and types "gala" through the
   emulated keyboard, the page sees it (its `input` event's marker).
   Next: the conformance report below.
   Then a conformance report (the owner). Step 1 done:
   `chromium_driver`: ChromeDriver from the snapshot (PREBUILT
   `chromedriver`) serves WebDriver on port 7, xtask opens a session on
   headless Chromium, loads page.html, reads it through a script and
   ends the session (about 2 minutes of kernel time; it needed
   user32's LoadKeyboardLayoutW/ToUnicode and ole32's thread OLE data
   in the layout Chromium reads). Step 2 under way: `chromium_wpt`
   (PENDING: a report run) serves WPT (pinned commit, `dom/nodes`) from
   the host and runs each test page in headless Chromium through
   ChromeDriver; its first five tests run and pass. Next: the whole
   directory, then the comparison with Chrome on Windows (wpt.fyi). The report itself: a subset of web-platform-tests
   (DOM, HTML parsing, CSS layout basics, fetch/XHR, WebSockets,
   storage, encoding, timers) driven through ChromeDriver from the
   snapshot's revision, the WPT server on the host reached as
   `chromium_host` reaches it; results per directory against
   Chrome-on-Windows on wpt.fyi, the tests that fail only on gala with
   the ledger lines behind them. Run by a sub-agent, widened as the
   speed allows; Acid3 and Speedometer/JetStream alongside (performance
   only with KVM or a release kernel).
3. The sandbox: renderers in restricted, low-integrity and
   AppContainer/LowBox tokens, job objects, process mitigation policies,
   handle inheritance lists, desktop and window station checks (RS);
   `--no-sandbox` dropped.
4. Stock Edge: the offline enterprise MSI (PK stage 5) or the standalone
   setup, installed as on Windows (updater and elevation service may stay
   off), the first-run experience, sign-in off; then WebView2 programs.
5. Performance and graphics: GPU G3 + G4.

**Done when**: each stage's scenario passes. Observability with each:
`--enable-logging=stderr --v=1` into the serial log, crashpad dumps
kept, the ledger's top missing functions per stage in the report.

**Licensing**: Chromium is BSD-style; Edge's licence allows installing
and running it, not redistributing it.

### OF. Microsoft 365 apps and the new Outlook

**Goal**: find and then remove the blockers for the current Office:
Word, Excel and PowerPoint (Click-to-Run) and the new Outlook (an MSIX
package over WebView2). Classic Outlook is out of scope.

**Next**:
1. Gap analysis (black-box metadata only, CLEAN_ROOM): import tables,
   delay loads, COM classes and services of the apps (installed with the
   Office Deployment Tool) against gala's exports and stubs; a ranked
   blocker list in this section, like AG's.
2. The new Outlook after BR's WebView2: MSIX install (PK stage 4),
   sign-in through the Web Account Manager, notifications.
3. The Microsoft 365 apps: Click-to-Run's service and virtualization,
   then the apps' needs (expected: DirectWrite/Direct2D/Direct3D 11 in
   software, GPU G2; Schannel; COM/OLE depth).

How far to go into sign-in and activation (Microsoft's services and
licence terms) is the owner's decision.

### TLS. Schannel and the HTTP stacks on it

**Goal**: Windows' TLS (SSPI's Schannel) and the HTTP clients built on
it, for programs and for gala's own tools.

**Done so far**: Schannel, client and server, TLS 1.2 and 1.3, in
schannel.dll on rustls with the RustCrypto provider (`schannel_test`
against Nginx with OpenSSL and an in-process server; host tests `cargo
test -p schannel_tls`): SSPI's token rules, EncryptMessage/
DecryptMessage, QueryContextAttributes, SNI and the name check, ALPN,
validation against gala's machine root store (`System32\gala\roots`),
decided by crypt32's chain engine and SSL policy (step 5).
Not yet run on real Windows (run-all.ps1 runs it when Nginx can be
started and, as administrator, imports the test CA first). winhttp's
and wininet's synchronous clients over it, on one engine (steps 1 and
2 below); unmodified curl and Git over it (`curl_schannel`,
`git_https`; step 3); `gala install`'s downloads over winhttp, with the
public CA bundle in gala's machine root store (`pk_install_https`; step
4); ECDSA server keys, session resumption and client certificates
(step 6).

**Next**:
1. Done: winhttp over Schannel, synchronous (`winhttp_test` against the
   same Nginx: http and https GETs, status and headers, Content-Length
   and chunked bodies, an untrusted issuer, a closed port,
   WinHttpCrackUrl; automatic redirects by WINHTTP_OPTION_REDIRECT_POLICY
   and MAX_HTTP_AUTOMATIC_REDIRECTS, WINHTTP_OPTION_URL; ARCHITECTURE
   "WinHTTP and WinINet"). Still open there: connection reuse,
   asynchronous sessions and status callbacks, proxies, authentication,
   WINHTTP_DISABLE_REDIRECTS.
2. Done: wininet's HTTP functions on the same engine as winhttp
   (`user/http_client`; `wininet_test` against the same Nginx:
   InternetOpenW/ConnectW, HttpOpenRequestW/AddRequestHeadersW/
   SendRequestW, HttpQueryInfoW with CUSTOM, InternetQueryDataAvailable
   and InternetReadFile, InternetOpenUrlW, InternetCrackUrlW, an unknown
   issuer as ERROR_INTERNET_INVALID_CA, a closed port, a redirect
   followed and one not (INTERNET_FLAG_NO_AUTO_REDIRECT); ARCHITECTURE
   "WinHTTP and WinINet"). Still open there, besides winhttp's list:
   the A functions, InternetQueryOptionW, FTP, the cache,
   InternetTimeFrom/ToSystemTimeW.
3. Done: curl and Git over Schannel. curl.se's own Windows build
   (8.23.0_8, PREBUILT `curl`) has LibreSSL as its only TLS backend (no
   Schannel, no MultiSSL), so it runs as that (`curl`: its CA list is
   the Windows ROOT store, crypt32's CertOpenSystemStoreA); the Schannel
   curl is Git for Windows' (PortableGit 2.47.1's curl 8.11.0, PREBUILT
   `gitcurl`; `curl_schannel`), the libcurl MinGit's https remotes load
   (`git_https`: ls-remote and a clone over Git's dumb HTTP protocol
   from the in-guest Nginx, and an untrusted certificate refused). Both
   curl scenarios run `user/curl/curl.sh` against the same Nginx: plain
   HTTP, HTTPS by name and address, an untrusted root (exit 60), an
   expired certificate (60 on LibreSSL, 35 on Schannel). What they
   needed: secur32's ANSI table (InitSecurityInterfaceA,
   QuerySecurityPackageInfoA, EnumerateSecurityPackagesA), Schannel
   counting only its own side's SP_PROT_* bits and taking a
   SECBUFFER_EMPTY output buffer, ntdll's RtlVerifyVersionInfo,
   msvcrt's rand_s/_sys_errlist/_sys_nerr, iphlpapi's
   if_nametoindex/if_indextoname, kernel32's Module32First/Next (ANSI),
   SSPI texts in FormatMessage, wldap32 stubs. Still open there: the
   pinned curl.se version is fetched from curl.se, which keeps versions
   from 8.17.0 on; a smart-HTTP Git server (`git
   http-backend`) and pushes; what the ledger shows besides
   (msvcrt's _localtime64_s, userenv's GetUserProfileDirectoryA, bcrypt's
   DH/DSA and chaining-mode property for libssh2's WinCNG probe,
   WSAIoctl's SIO_IDEAL_SEND_BACKLOG_QUERY).
4. Done: `gala install`'s HTTPS downloads (PK) on winhttp
   (`pk_install_https`: a package downloaded from the in-guest Nginx
   behind a release URL that redirects, as GitHub's do, its SHA-256
   checked; an untrusted certificate and an unreachable host fail with
   the reason), with gala's root store holding a pinned, documented
   public CA bundle (Mozilla's, as curl.se publishes it:
   `user/roots/public-roots.pem`, THIRD_PARTY.md; shipped in every full
   image beside the test CAs; host test
   `public_roots_are_all_trust_anchors`; ARCHITECTURE "Schannel (TLS)").
   Still open there: a download from the real GitHub in a scenario (the
   test VMs have no internet: a host-network scenario could relay one),
   root updates without a new image (Windows Update's role).
5. Done: crypt32's system stores and chain engine, and Schannel
   deciding through them (`cert_test` chain.rs, `schannel_test`, the
   `--cacert` steps of `curl` and `curl_schannel`; ARCHITECTURE
   "Certificate chains"): ROOT (gala's machine root store), CA and MY
   (empty) system stores; CertCreateCertificateChainEngine
   (hExclusiveRoot, hRestrictedRoot, additional stores),
   CertGetCertificateChain (issuers from hAdditionalStore and the
   engine's stores; time, signatures with RSA and ECDSA P-256/P-384
   through bcrypt, untrusted roots, partial chains, basic constraints,
   key usage, critical extensions, RequestedUsage),
   CertVerifyCertificateChainPolicy (BASE, SSL with the server name,
   BASIC_CONSTRAINTS; the documented CERT_E_* errors and element
   indexes), CertGetNameStringW's CERT_NAME_DNS_TYPE; a context's
   hCertStore keeps its store alive, and Schannel's remote certificate
   carries the server's chain in it (what curl's `--cacert` check
   reads). Schannel's automatic validation is the same engine and SSL
   policy (one trust decision; webpki only in the host tests). Still
   open there: name constraints (a
   stopgap: "Known issues and stopgaps"); P-521 keys (one root of the
   public bundle cannot verify what it signs, recorded); RSA-PSS
   signatures; issuance policies; AIA fetching of missing
   intermediates (a chain the server sends incomplete stays partial);
   CTLs, lower-quality chains; no Windows run of `cert_test`'s chain
   part yet (its root is an exclusive one, so it needs no
   administrator).
6. Done: ECDSA server keys, session resumption, client certificates
   (`schannel_test` against Nginx on ports 8443-8447 and its in-process
   server, host tests `cargo test -p schannel_tls`, `curl_schannel`'s
   resumption steps; ARCHITECTURE "Schannel (TLS)"). ECDSA: bcrypt signs
   with P-256/P-384 private keys (ECCPRIVATEBLOB), ncrypt imports them
   (PKCS#8, ECCPRIVATEBLOB) and signs, Schannel serves with them (and
   validates a P-384 leaf through crypt32). Resumption: each credential
   handle's session cache (client: per target name; TLS 1.2 session IDs
   and tickets, TLS 1.3 tickets; server: session IDs and stateful TLS
   1.3 tickets), SCH_CRED_DISABLE_RECONNECTS, SECPKG_ATTR_SESSION_INFO;
   Nginx's $ssl_session_reused agrees. Client certificates: paCred's
   certificate sent when asked; SEC_I_INCOMPLETE_CREDENTIALS with
   SECPKG_ATTR_ISSUER_LIST_EX, then a credential with a certificate (the
   handshake run again from its recording) or ISC_REQ_USE_SUPPLIED_CREDS
   (none). Revocation, offline (`cert_test` crl.rs and chain.rs,
   `schannel_test` on ports 8448/8449, `curl_schannel`'s `revocation`
   and `revoked` steps; ARCHITECTURE "CRLs and revocation"): crypt32's
   CRL contexts and CRL stores (gala's CA store from
   `System32\gala\ca`), CertFindCertificateInCRL,
   CertVerifyCRLRevocation, CertVerifyRevocation's default provider, the
   chain engine's CERT_CHAIN_REVOCATION_CHECK_* (CERT_TRUST_IS_REVOKED,
   REVOCATION_STATUS_UNKNOWN, IS_OFFLINE_REVOCATION) and the policies'
   CRYPT_E_* answers; Schannel's SCH_CRED_REVOCATION_CHECK_* and
   SCH_CRED_IGNORE_* flags, a revoked server certificate failing with
   CRYPT_E_REVOKED (curl 35). Decided: offline first, nothing fetched
   (crypt32 does not depend on winhttp; Windows fetches through
   cryptnet). Still open there: fetching a CRL from the distribution
   point the certificate names (cryptnet's CryptRetrieveObjectByUrl over
   winhttp, cached; the test certificates name Nginx's
   http://127.0.0.1:8080/ca.crl, which run-all.ps1 serves on Windows),
   OCSP and stapled OCSP responses, delta CRLs and partitioned
   (issuing distribution point) CRLs, CERT_REVOCATION_INFO in the chain
   elements, WinHTTP's WINHTTP_ENABLE_SSL_REVOCATION and WinINet's
   revocation option; a Windows run of `cert_test`'s CRL part and
   schannel_test's revocation checks (the expectations are the
   documented ones and Wine's recorded results);
   winhttp/wininet keeping a
   credential handle per session so they resume (they acquire one per
   connection); a server asking for client certificates
   (ASC_REQ_MUTUAL_AUTH, recorded) and checking them; the MY store
   (persistent, and Schannel's automatic choice from it,
   ISC_RET_USED_COLLECTED_CREDS); intermediates sent with a certificate;
   a client certificate after a refused session offer (the replay cannot
   be made: the context fails, named in the ledger); keys persisted
   across processes; curl.se's LibreSSL curl resuming (it does not here:
   to be compared with Windows).

**Design**: ARCHITECTURE "Schannel (TLS)"; WORKFLOW "TLS tests
(Schannel)".

### AG. API gaps in the shell and runtime DLLs

**Goal**: filler work: the exports real programs import from msvcrt,
shlwapi, oleaut32, crypt32, version, dbghelp, comctl32, shell32, ole32
and small new DLLs, test first (expected values from Microsoft's
documentation and Wine's conformance tests).

**Method**: the gap list comes from the import tables
(`x86_64-w64-mingw32-objdump -p`) of the programs in `user/bin`, the
JDK, PostgreSQL 17.2's `bin` (with StackBuilder's wxWidgets 3.2 VC
DLLs), FileZilla Server 1.12.6 (with its GUI) and SQL Server LocalDB
2022, against gala's built DLLs and their `gala_stubs` stubs
(`python3 -I tools/import_survey.py target/x86_64-pc-windows-msvc/release
<pe>...`, imports and delay imports, by name and ordinal; oleaut32's
ordinals from Wine's oleaut32.spec). Two kinds: missing exports stop a
program from loading (delay-loaded ones fail when called); stubs stop it
when called.

**Done so far** (groups 1-13; no Windows run yet, each test still needs
a run-all.ps1 check there): msvcrt's wide classes, `_wtol`, `difftime`
(`msvcrt_test`); oleaut32's variant time and string conversions,
`VarCmp`, `GetActiveObject`, and the DATE/currency/double conversions
msxmlsql imports by ordinal (#82, 83, 92, 93, 96, 103, 112, 123, 124)
(`oleaut32_test`); all of shlwapi's surveyed path, string and URL
functions, AssocQueryStringW, SHCreateStreamOnFileEx/W, StrToIntW,
StrTrimW, PathIsFileSpecW (`shlwapi_test`; no shlwapi stubs or missing
exports left); shell32's ShellExecuteExW, drag-and-drop,
SHFileOperationW, SHChangeNotify, icon extraction (`shell32_test`);
ole32's PROPVARIANT and STGMEDIUM clean-up,
OleRun/OleLockRunning/OleSetContainedObject, proxy blankets,
CoCreateInstanceEx, CreateBindCtx, CreateStreamOnHGlobal with an HGLOBAL
and GetHGlobalFromStream (clones, CopyTo), CoFileTimeNow,
CoGetCurrentProcess (`ole32_test`), on kernel32's movable global memory
(GlobalAlloc/LocalAlloc GMEM_MOVEABLE handles, Lock/Unlock/Size/ReAlloc/
Flags/Handle as Windows does them, `kernel32_misc_test`); dbghelp's
ImageNtHeader, SymSrvGetFileIndexInfoW and symbols over exports, a new
normaliz.dll (`dbghelp_test`); crypt32's memory functions,
CertGetNameStringA, PFXIsPFXBlob, certificate stores, CryptQueryObject,
CryptDecodeObjectEx for alternative names, PFXImportCertStore (PKCS #12
with OpenSSL's modern and legacy algorithms, keys into ncrypt's KSP) and
CryptAcquireCertificatePrivateKey for CNG keys (`cert_test`); comctl32's
image-list drag functions (`comctl32_test`); what Inno Setup's Setup
(Delphi's VCL) looks up or calls (PK stage 3): oleaut32's variant
arithmetic and logic (VarAdd .. VarNot), VarR4FromStr, VarBstrFromDate
(`oleaut32_test`), ole32's server-process count and class-object
suspension (`ole32_test`), comctl32's flat scroll bars
(`comctl32_test`), user32's AnimateWindow, layered window attributes,
shutdown-block reasons, GetLastActivePopup, ShowOwnedPopups and a
fixed GetClassInfoW, msctf.dll's input scopes (`user32_test`), gdi32's
EnumFontsW and older calls (`gdi32_test`), shell32's Start menu
folders (`shell32_test`), kernel32's INI functions
(`kernel32_misc_test`); what Inno Setup's shortcuts and IDE need:
shell32's shell links and SHAddToRecentDocs (`shell32_test`),
comdlg32's GetFileTitleW and dialog parameter checks
(`comdlg32_test`), user32's GetMenuBarInfo, GetMenuItemRect,
SendDlgItemMessageW, RegisterClassExA and imm32's input contexts
(`user32_test`), gdi32's ANSI text functions (`gdi32_test`),
kernel32's FileTimeToDosDateTime, FindStringOrdinal (`kernel32_misc_test`)
and EnumResourceLanguagesW (`shell32_test`), bcrypt's ECDSA key export
(`bcrypt_test`). FileZilla Server's GUI and
StackBuilder find every msvcrt, oleaut32 and ole32 import; version and
msvcrt have nothing left the surveyed programs import. The survey
(`tools/import_survey.py` over user/bin, the JDK, PostgreSQL, FileZilla
Server and LocalDB) finds no missing static import in these DLLs: what
is left is delay-loaded or stubbed.

**Next** (what is left, by DLL):
1. ole32: CoRegisterMallocSpy (sqlmin; the task allocator would call the
   spy around every operation and track spied blocks), the OLE clipboard
   and DoDragDrop (need user32's clipboard).
2. crypt32: the other decode types the programs use (which ones is only
   seen at run time: the ledger's `CryptDecodeObjectEx` lines),
   CRLs/CTLs, collections' member stores, PFX with ECDSA keys (ncrypt
   imports RSA only) or public-key integrity, CryptoAPI (CSP) private
   keys.
3. Delay imports the survey finds missing (they fail only when called):
   oleaut32's VarParseNumFromStr/VarNumFromParseNum (#46, 47),
   VarFormat (#87), VarDateFromUdate/VarUdateFromDate (#330, 331)
   (msxmlsql); comctl32's window subclassing (#410-413: awt.dll),
   shell32's SHGetSettings (awt), version's VerInstallFileW (sqlmin).
4. shell32: SHBrowseForFolderW (a dialog).
5. comctl32: CreateUpDownControl (a new control class); drawing the drag
   image.
6. dbghelp: MiniDumpWriteDump (a stub), PDB symbols, other processes'
   modules.
7. New DLLs: winspool.drv (StackBuilder's wxmsw326u_core), the JDK's
   optional dsound, winscard, msi, dbgeng (loaded only by jsound,
   j2pcsc, jpackage, saproc).
8. Elsewhere, for the agents working there: FileZilla Server's GUI also
   stops on user32 (54 stubs: carets, the clipboard, DDE,
   DrawEdge/DrawFrameControl, window placement), gdi32 (16: metafiles,
   font enumeration), kernel32 (13), comdlg32 (4), oleacc, winmm's
   PlaySoundW, ws2_32's inet_pton/inet_ntop (GnuTLS).

**Known problem**: the FileZilla client's PREBUILT URL
(`downloads.sourceforge.net/project/filezilla/FileZilla_3.69.3_win64-setup.exe`,
also the `FileZilla_Client/3.69.3/` path) answers 404, so the
`filezilla`, `filezilla_flaui` and `filezilla_server` scenarios (one
module set) cannot be fetched on a machine without
`target/prebuilt/filezilla`. No official mirror with the pinned SHA-256
(e87b8787...) has been found; the URL stays until one is.

### LX. Linux personality

**Goal**: Linux programs on the same kernel core through their own
system-call layer (PROJECT_BRIEF goal 5). The seams are built as their
areas are touched; the Linux system calls are filler work, milestone by
milestone, test first against real Linux (CLEAN_ROOM "Linux").

**Done so far**:
- Seam 2: a `Personality` on every process (fixed at creation) and a
  dispatcher table keyed by it at system-call entry and in gala-um's
  SIGSYS handler (`kernel/src/personality.rs`); the personality in
  process events, crash reports and thread dumps.
- Seam 3: the core/NT split of `Process` (`nt::process::Core`, with RS
  B2); Linux processes are built through it.
- Seam 6: `net` uses `crate::runtime`, not `nt::platform`
  (`net_does_not_depend_on_nt` in xtask).
- Milestone 1: `lxrun=/lx/<binary>` (with `lxargs=`, `lxenv=`) execs a
  static or static-pie ELF as a `Personality::Linux` process
  (`lx::exec`); `lx::dispatch` serves read, write, writev, close, pipe2,
  fcntl, mmap, munmap, mprotect, brk, arch_prctl, exit, exit_group,
  set_tid_address, the get*id calls, and ioctl as a stub; ENOSYS for
  the rest, logged by name and in the ledger
  (`linux syscall missing`). Scenarios `lx_hello`, `lx_write_exit`,
  `lx_brk`, `lx_mmap`, `lx_arch_prctl`, `lx_auxv`, each also `_pie`,
  match real Linux's transcripts exactly.
- Milestones 2-3 test corpus: fs_test, pipe_test, fork_test,
  thread_test, signal_test, clock_test, poll_test, pty_test, proc_test
  recorded on Linux 6.18 (`cargo xtask lx-expect`, `--runs N`), and the
  `kernel/lx_abi` crate (signal, termios, stat/statx/dirent64/epoll/poll
  layouts) host-tested against them.

- Milestone 2 under way: pipes as Linux keeps them (whole pages:
  F_GETPIPE_SZ/F_SETPIPE_SZ, a full pipe takes a small write only when
  a page is free), the old `pipe` call, lseek (ESPIPE), dup, dup2, dup3,
  F_DUPFD(_CLOEXEC), close-on-exec per fd, O_NONBLOCK shared by dups
  (F_SETFL). Signals (`lx::signal`, `lx::sigcalls`, on `lx_abi`, now a
  kernel dependency): rt_sigaction, rt_sigprocmask, rt_sigpending,
  rt_sigtimedwait, rt_sigreturn; SIGPIPE from a pipe with no reader;
  delivery on the way back from a system call (default actions, SIG_IGN,
  handlers on an rt_sigframe with SA_RESTORER, SA_RESTART for
  read/write/wait4). fork (and clone with only SIGCHLD) with an eagerly
  copied address space, wait4, SIGCHLD at the child's end. Scenario
  `lx_pipe` matches pipe_test's transcript exactly.
- Milestone 2's static busybox runs: busybox.net's static musl build
  (a `PREBUILT`, run only in gala) runs a shell script of pipelines,
  redirections, here-documents, functions, background jobs and the
  coreutils-style applets through fork and execve (`lx_busybox`).
  Built for it, each with a corpus program recorded on Linux and a
  scenario matching it exactly: files and directories on the shared
  ramfs (`lx::file`: open/openat flags, read/write/pread/pwrite, lseek,
  the stat family, getdents64, mkdir/unlink/rmdir/rename, chdir/getcwd,
  access, umask, chmod, utimensat; POSIX modes on ramfs nodes; /dev/null and
  friends: fileio_test, `lx_fileio`); execve with `#!` scripts, getppid,
  uname, setuid/setgid (exec_test, `lx_exec`); TCP sockets and poll/
  select (socket_test, `lx_socket`; busybox httpd and wget: `lx_httpd`,
  H11). Also vfork (a copy, the parent waits), kill, rt_sigsuspend,
  sendfile, readv, close_range, the clocks and sleeps, ITIMER_REAL and
  alarm; blocking pipe writes larger than the room write piece by
  piece.
- Threads and their primitives (`lx_thread`, thread_test, with musl's
  pthreads on top): clone with CLONE_THREAD as pthread_create makes it
  (`lx::threads`), signal masks and pending sets per thread, tgkill and
  tkill, set_tid_address, CLONE_CHILD_SETTID/CLEARTID with the exit's
  futex wake, exit against exit_group, Linux tids (the first thread's is
  the pid); futex WAIT, WAKE, REQUEUE, CMP_REQUEUE, WAIT_BITSET,
  WAKE_BITSET with timeouts on both clocks (`lx::futex`). epoll
  (`lx::epoll`; `lx_poll` with poll_test, `lx_epoll` with epoll_test:
  pipes, sockets, nested epolls, EPOLLET, EPOLLONESHOT, EPOLLRDHUP,
  epoll_pwait/epoll_pwait2). Wait queues (`lx::wait`) replace the 1 ms
  polling for pipes, poll/select/epoll over pipes, sleeps,
  rt_sigsuspend, pause, rt_sigtimedwait and wait4, and a signal for a
  blocked thread wakes it.
- Seam 4's Linux side, one file system for both (`lx_fs` with fs_test,
  each also `_pie`; ARCHITECTURE "Files", "The lx module"): symbolic
  links (symlink, readlink, lstat, followed in path walks, ELOOP past
  40), hard links (link, linkat, nlink), unlink of an open file (the
  name goes at once, the file lives on), statx, RENAME_EXCHANGE;
  directories a Linux program makes are case-sensitive (Windows'
  per-directory flag, honoured for NT callers too), so two names
  differing only in case live in one; POSIX modes, case sensitivity,
  symbolic and hard links kept across reboots (`fs::log` record
  version 3, older records still read; host tests). busybox's applets
  run by their own names through symbolic links (`lx_busybox`).

**Next**:
1. Milestone 2's rest: the rest of signal_test and fork_test
   (rt_sigqueueinfo, sigaltstack, faults as SIGSEGV/SIGBUS/SIGFPE
   through the exception path, delivery to a thread running user code,
   stop/continue, process groups and sessions, waitid, rusage), vfork
   sharing memory and copy-on-write for fork (shared frames,
   `vm::copy_on_write` for private pages; MAP_SHARED anonymous memory
   shared with the child, then futexes keyed by the shared page);
   execve ending the other threads first; per-thread CPU time
   for clock_test's CPU clocks; wait queues on sockets instead of their
   1 ms look (seam 5: the stack's notify hook is AFD's alone);
   `lx::state` onto the core process; an `lx_pipe_pie` scenario.
   Each corpus program becomes a boot scenario as its calls land; the
   busybox scripts grow with them.
2. Milestone 3: clocks, ptys over the core terminal object, /proc.
3. Seam 1: the pty as a core terminal object under ConPTY.
4. Seam 4's NT surface, each test first against real Windows:
   FileCaseSensitiveInformation (query, and set on an empty directory),
   FILE_DISPOSITION_POSIX_SEMANTICS and FILE_RENAME_POSIX_SEMANTICS,
   CreateHardLinkW (FileLinkInformation), CreateSymbolicLinkW; an NT
   delete through a hard link's name removing that name; uid/gid per
   file with more than one Linux user; a reboot scenario for Linux
   files (modes and links are host-tested only).
5. Seam 5: shared core objects when pipes, sockets or files are
   reworked.
6. Also open (ARCHITECTURE "The lx module"): address randomization, a
   vDSO, Linux in gala-um, the system-call numbers 462..=469.

**Done when** (each milestone): every corpus program of the milestone
runs as a boot scenario whose transcript equals its `expected.txt`.

**Design**: ARCHITECTURE "Plan: a second personality (Linux)", "The
seams", "The Linux layer's milestones", "The lx module", "The lx_abi
crate"; WORKFLOW "Linux test programs".

### CT. Linux containers: Docker Desktop and Podman over a WSL facade

**Goal**: stock Docker Desktop for Windows and stock Podman for Windows
(`podman machine`, later Podman Desktop) install and run Linux
containers on gala, unmodified. On Windows both run their Linux side in
a WSL2 distribution; gala answers WSL2's interface itself and runs that
distribution on the Linux personality (LX), with no virtual machine.

**Out of scope**: running WSL2 itself (a VM with Microsoft's Linux
kernel), the Hyper-V backends of both products, and Windows containers
(Docker Desktop's "Windows containers" mode: HCS, server silos).

**Next** (each step test first; the recordings of step 1 are the spec):
1. Spec: on real Windows 11 24H2, record what each product does to set
   up and to run `hello-world`: every `wsl.exe` command line and its
   output, the WSL service calls, named pipes and sockets (hvsocket and
   localhost TCP) between the Windows and Linux sides, registry keys,
   WMI queries and optional-feature checks, files under `%LOCALAPPDATA%`
   and `%ProgramData%`; inside the distribution, `strace -f` of its
   init, dockerd/containerd/runc and podman/conmon/crun. The ranked
   result (Windows side, facade, Linux system calls) goes in this
   section, like AG's blocker list.
2. The facade's core, test first against recorded `wsl.exe`
   transcripts: gala's `wsl.exe` and WSL service with `--import` of a
   rootfs tarball, `-d <distro> [--exec] <cmd>`, `-l -v`,
   `--terminate`, `--shutdown`, `--unregister`, `--version`, the
   per-user distribution registry, and a distribution's init as an LX
   process. Scenario: `wsl --import` of a busybox rootfs, then
   `wsl -d bb -- echo hi`.
3. The facade's plumbing: `/mnt/<drive>` inside the distribution and
   `\\wsl.localhost\<distro>` outside (one file system, no 9P), interop
   (Windows programs started from Linux and back), `wsl.conf`, hvsocket
   (`AF_HYPERV`) on the Windows side meeting `AF_VSOCK` on the Linux
   side as one core socket pair, localhost forwarding over one shared
   network stack.
4. LX up to the distributions' needs (beyond milestone 5): glibc,
   systemd as PID 1 (Podman's machine), namespaces (mount, PID, UTS,
   IPC, network, user), cgroup v2, overlayfs, bind mounts and
   `pivot_root`, netlink, veth and bridges, nftables/iptables, seccomp,
   `/proc` and `/sys` as these programs read them. Namespaces and
   cgroups are core objects (ARCHITECTURE "Plan: Linux containers
   through a WSL facade"), not LX-only state.
5. Podman: `podman machine init`, `start`, then `podman run` from the
   Windows side; Podman Desktop after BR stage 2 (Electron).
6. Docker Desktop: install, the backend service, its distribution, the
   `docker` CLI over `\\.\pipe\docker_engine` running containers; then
   the dashboard (Electron, after BR stage 2).

**Done when**: from a gala command prompt, `podman run --rm hello-world`
and `docker run --rm hello-world` print their greeting with both
products installed by their own installers; then a container serving
HTTP reached from the Windows side (`-p 8080:80`). Observability with
each step: the facade's calls as events (`events=wsl`), each
distribution's console and the products' logs copied into the serial
log, missing Linux calls in the ledger by distribution.

**Licensing**: Podman is Apache-2.0; Docker Desktop's licence terms
(a paid subscription for larger organisations) govern running it.
Neither is committed; xtask fetches them and runs them only in QEMU.
WSL's source (MIT) is an allowed source (CLEAN_ROOM "WSL").

**Design**: ARCHITECTURE "Plan: Linux containers through a WSL facade".

### WOW. 32-bit x86 programs (WOW64)

**Goal** (approved by the owner): unmodified 32-bit x86 Windows programs
run on gala as on 64-bit Windows, with the answers 64-bit Windows gives
about them (IsWow64Process2, GetNativeSystemInfo, SysWOW64, the
Wow6432Node registry view). Every Inno Setup installer checks for it
(its Setup, x64 builds included, refuses a Windows without x86
support: PK stage 3), x86 Inno installers run on it, and so do the
many installers and programs only shipped for x86. Design: ARCHITECTURE "Plan: WOW64" (the kernel stays 64-bit; gala's
wow64.dll converts 32-bit system calls in user mode).

**Done so far** (stages 1 and 2):
- Compatibility mode: the TEB32 segment (0x53) and DS/ES/FS loaded at
  every switch to a WOW64 thread; PE32 executables (without imports) as
  WOW64 processes: the 2/4 GiB address limit (large-address-aware), the
  PEB32 and 32-bit process parameters, a TEB32 and CPU area per thread,
  gala's wow64.dll (thread start, the 32-to-64-bit transition, thunks
  for NtClose, NtTerminateProcess, NtQueryPerformanceCounter,
  NtRead/WriteFile, NtAllocate/Free/Protect/QueryVirtualMemory,
  NtQueryInformationProcess; the rest in the ledger as
  `wow64.dll!service 0x...`). `wow64_test` (32-bit, ntrt's x86 runtime,
  built for i686-pc-windows-msvc): CS 0x23, TEB32, PEB32, the 32-bit
  parameters, the services above in their x86 layouts.
- Truthful answers: IsWow64GuestMachineSupported (x86 only),
  GetSystemWow64DirectoryW/A (`C:\Windows\SysWOW64`, which exists),
  IsWow64Process and IsWow64Process2 of a native process and of a
  32-bit child (`win32_misc_test`), from ProcessWow64Information.
- Observability: the process start line and event (`32-bit x86
  (WOW64)`, `"machine":"x86"`), crash reports with the 32-bit registers
  (`wow64_crash`), the thread dump's 32-bit caller, the state dump's
  `user 32-bit`.
- Stage 2, the conversions a 32-bit ntdll and kernel32 need, against
  Windows in `wow64_test`: wow64.dll converts the services gala's
  kernel serves (a table of argument kinds: handles, OBJECT_ATTRIBUTES,
  UNICODE_STRING, CLIENT_ID, PVOID*/SIZE_T*, security descriptors) and
  the information classes with pointer-sized fields (process, thread,
  system incl. the process list, token, object, section, file rename and
  completion, directory objects, symbolic links, the handle arrays of
  waits); files and directories, sections, events, mutants, semaphores,
  keyed events, timers, the registry, tokens; GalaCreateProcess from a
  32-bit parent. Every access to a caller's memory is fault-safe: an
  invalid 32-bit pointer fails the call with STATUS_ACCESS_VIOLATION
  (faults in wow64.dll's 64-bit code reach its GalaWow64Exception). The
  kernel writes the x86 IO_STATUS_BLOCK for a WOW64 process, so I/O that
  completes later reports in the caller's 8-byte block. 32-bit threads
  (`wow64_thread_test`): NtCreateThread with the x86 CONTEXT and
  INITIAL_TEB (a 64-bit stack and TEB32 per thread, freed at its exit),
  NtGet/SetContextThread with the x86 CONTEXT (CS 0x23 for a thread the
  kernel stopped in 32-bit code: `Parked::compat`; the CPU area for one
  in wow64.dll; a thread created suspended reports its start),
  NtContinue; the 32-bit stack grows through its guard page.
- Stage 3 so far: **the 32-bit ntdll** (`wow64_import_test`): gala's
  ntdll built for i686 (`user/testntdll`, its x64 and x86 parts in
  `x64.rs`/`x86.rs`), shipped as `C:\Windows\SysWOW64\ntdll.dll` and
  exported as `testntdll32.dll`; the kernel starts a 32-bit process at
  its LdrInitializeThunk, whose loader binds PE32 imports (from the
  executable's directory, then SysWOW64), with relocation (by the
  kernel's image sections), TLS and TLS callbacks, DllMain, PEB32.Ldr's
  lists, the process heap, and the 32-bit system call stubs through
  wow64.dll's transition. **Exceptions and APCs in 32-bit code**
  (`wow64_seh_test`, `wow64_seh_crash`): the kernel hands an exception
  in 32-bit code to wow64.dll's dispatcher on the 64-bit stack, which
  passes it to the 32-bit ntdll's KiUserExceptionDispatcher (the x86
  SEH chain at fs:[0], RtlUnwind, RtlRaiseException, vectored
  handlers); user APCs with 32-bit routines (NtQueueApcThread, the APC
  routines of I/O and timers, NtContinue's TestAlert) run through its
  KiUserApcDispatcher. **File system redirection and the 32-bit
  environment** (`wow64_fs_test`): System32 to SysWOW64 in wow64.dll's
  file thunks, Sysnative, the documented exemptions, per-thread
  RtlWow64EnableFsRedirection(Ex); PROCESSOR_ARCHITECTURE=x86,
  PROCESSOR_ARCHITEW6432, ProgramFiles and CommonProgramFiles the (x86)
  directories, ProgramW6432, `C:\Program Files (x86)`. **The 32-bit
  kernel32** (`wow64_k32_test`, a Rust program; `wow64_c_test`, a C
  program built by clang with stdcall imports through an import library,
  not large-address-aware): gala's kernel32 built for i686, shipped as
  `C:\Windows\SysWOW64\kernel32.dll` (`testkernel32_32.dll`), what
  differs by architecture in its `x64.rs`/`x86.rs` (TEB access, the TEB,
  PEB, process parameter, CONTEXT and optional header offsets,
  SLIST_HEADER, the new thread's CONTEXT) and the structures with
  pointer-sized fields typed (`repr(C)` with `usize`: the basic
  information classes, MEMORY_BASIC_INFORMATION, OVERLAPPED,
  STARTUPINFO, SECURITY_ATTRIBUTES, EXCEPTION_RECORD, toolhelp's
  entries, psapi's counters, the processor information); GetSystemInfo's
  32-bit answers (PROCESSOR_ARCHITECTURE_INTEL, the process's 2/4 GiB
  range), GetNativeSystemInfo's AMD64, GetSystemDirectory's SysWOW64,
  Wow64Disable/Revert/EnableWow64FsRedirection; wow64.dll converts
  GalaConsoleMode and GalaConsoleCtrl (the console control thread).
  **The 32-bit msvcrt** (`wow64_crt_test`, a C program built by
  mingw-w64's i686 GCC with its own startup code): gala's msvcrt built
  for i686, shipped as `C:\Windows\SysWOW64\msvcrt.dll`
  (`testmsvcrt32.dll`): the `va_list` walker by the architecture's slots
  (`va.rs`: 4 bytes, doubles and __int64 8), its `x64.rs`/`x86.rs`
  (the variadic entry points: cdecl naked functions on x86;
  `_setjmp3`/`_setjmp`/`longjmp` with the x86 `_JUMP_BUFFER`, unwinding
  the SEH frames registered since setjmp through RtlUnwind; `_control87`
  and `_controlfp` on the x87 control word and MXCSR together), `%I`
  and `%p` pointer-sized, FILE, `_iob` (a data export), lconv,
  STARTUPINFO and the descriptor-inheritance block with 4-byte
  pointers, the 32-bit time_t of `time`/`gmtime`/`localtime`/`mktime`/
  `ctime`/`difftime`, and what mingw's startup imports (`__p__fmode`,
  `__p__commode`).
  **The 32-bit advapi32** (`wow64_adv_test`, a Rust program): gala's
  advapi32 and bcryptprimitives built for i686, shipped as
  `C:\Windows\SysWOW64\advapi32.dll` and `bcryptprimitives.dll`
  (`testadvapi32_32.dll`, `testbcryptprimitives32.dll`): the registry
  (32-bit predefined HKEYs), tokens (TokenUser, TokenGroups, DuplicateTokenEx
  with an x86 SECURITY_ATTRIBUTES, CheckTokenMembership), SIDs and
  LookupAccountSidW, absolute (20-byte) and self-relative descriptors,
  SetEntriesInAclW and BuildExplicitAccessWithNameW with x86
  EXPLICIT_ACCESS_W, CryptGenRandom; its structures with pointers typed
  in `layout.rs`; the 32-bit ntdll's absolute descriptors at its pointer
  width; wow64.dll converts GalaLogonUser's UNICODE_STRINGs. **The
  registry's 32-bit view** (`wow64_adv_test`; stage 4's first part, in the
  kernel: `nt/regview.rs`): HKLM\SOFTWARE to WOW6432Node for a WOW64
  process or KEY_WOW64_32KEY, KEY_WOW64_64KEY for the 64-bit view, the
  shared keys of "Registry Keys Affected by WOW64" (Windows 7 and later),
  Classes' redirected subkeys in `Classes\WOW6432Node`, names relative to
  a handle by their full name, RegDeleteKeyEx in the view it names;
  WOW6432Node seeded with the version keys and the 32-bit
  ProgramFilesDir/CommonFilesDir.
  **The 32-bit user32 and gdi32** (`wow64_gui_test`, C built with
  mingw's i686 GCC; stage 4's second part): gala's user32, gdi32 and
  gala_heap built for i686 (`testuser32_32.dll`, `testgdi32_32.dll`), a
  32-bit program's window class and window (CREATESTRUCTW, window
  longs, extra bytes), SendMessageW/PostMessageW with 32-bit
  WPARAM/LPARAM through GetMessage/DispatchMessage, GetClientRect,
  WM_PAINT with FillRect read back by GetPixel, DestroyWindow and
  WM_QUIT; wow64.dll's rows for gala's window, console, desktop and
  window-station services; GALA_WM_PLACE packed for a 32-bit WPARAM.
  Window extra bytes (cbWndExtra) for both architectures (`user32_test`
  too). `cargo xtask stub-audit` checks that every DLL declares a stub's
  function with the stub's argument sizes and that wow64.dll's rows
  have the stub's word count. **What an x86 Inno installer loads**, built
  for i686 (`test<name>32.dll`): comctl32 (its message structures typed:
  `layout.rs`), oleaut32 (the x86 VARIANT), ole32, shell32, shlwapi,
  version, mpr, comdlg32, uxtheme, imm32, dwmapi, msimg32, msctf, winspool,
  wtsapi32, shfolder, bcrypt, winhttp, secur32, ws2_32; the 32-bit ntdll's
  MSVC 64-bit arithmetic helpers (`wow64_import_test`); user32 and gdi32's
  pointer-bearing structures typed for x86 (GetClassInfoW, GetObjectW's
  DIBSECTION, CURSORINFO, WM_COPYDATA's wire form), GetClassLongW and
  SetClassLongW; the 32-bit ShellLink class in `Classes\WOW6432Node`.
  **An x86 Inno Setup installer installs** (`wow64_inno`: Inno Setup
  6.5.4's own, winget's x86 choice, PREBUILT innosetup6, silently for the
  current user: its files, Start menu shortcuts, uninstall entry, then the
  installed 32-bit Compil32.exe /ASSOC). **`gala install` of winget's x86
  choices** (`pk_install_inno32`: JRSoftware.InnoSetup 6.5.4, whose
  installers are all x86, installed per user, found by the ProductCode of
  its AppsAndFeaturesEntries, its 32-bit compiler run, the .iss
  association its IDE writes, listed, uninstalled through its 32-bit
  uninstaller); an x86 portable program is gala's to install too (no
  fallback to another installer). **32-bit sockets**
  (`wow64_sock_test`: the x86 WSADATA, ADDRINFOA, WSABUFs and fd_set; TCP
  and UDP on the loopback address; select): the kernel's AFD reads a
  WOW64 caller's requests in their x86 layout, from `kernel/afd_abi`,
  which gala's ws2_32 builds them with too; wow64.dll passes AFD's
  IOCTLs through. The "Done when" below is met; what follows are
  follow-ups.

**Next**:
1. Stage 3's rest, the 32-bit system DLLs: kernelbase; kernel32_test built for x86 as a second scenario (xtask builds a package for one target per
   name: an x86 twin needs its own export name, and testdll/testdll2 for
   x86). The 32-bit kernel32's own gaps: GetProcessInformation's
   ProcessMachineTypeInfo answers AMD64; the Wow64*FsRedirection
   functions are x86-only (64-bit Windows' kernel32 exports them too:
   what they answer in a 64-bit process is to be observed); the pseudo
   console's services (GalaConsoleCreate/Size/Close/Trace) and
   GalaClockControl, GalaSession are not converted by wow64.dll (the
   ledger names them); its exports include ntrt's runtime symbols
   (memcmp, strlen, bcmp, __CxxFrameHandler3; the 32-bit msvcrt's too).
   The 32-bit msvcrt's own gaps: the 32-bit `_stat`/`_fstat`/`_findfirst`
   forms and the other `_*32` time functions (mingw's i686 headers name
   them under `_USE_32BIT_TIME_T`; missing exports until a program needs
   them), `_except_handler3`/`_except_handler4` and `_local_unwind2`
   (MSVC's `__try` in C; a longjmp over a try level is in the ledger as
   `msvcrt.dll!longjmp(try level)`), msvcrt_test's C source built for
   x86 too (it needs a 32-bit shell32 for CommandLineToArgvW). Also: the redirector for GalaCreateProcess's image
   path and names relative to a RootDirectory handle (it rewrites full
   NT paths only); nested exceptions and collided unwinds in the x86
   dispatcher; ThreadWow64Context for 64-bit debuggers. Stage 2's
   leftovers, each in the ledger: the job information classes,
   CONTEXT_FLOATING_POINT on NtSetContextThread.
   The 32-bit advapi32's own gaps: its service SIDs load bcrypt, which
   has no 32-bit build yet (LoadLibrary fails: NT SERVICE names do not
   resolve in a 32-bit process); its exports include ntrt's runtime
   symbols. The registry view's: WOW6432Node's `Classes` link and
   `HKEY_USERS\<SID>_Classes` are not modelled (ARCHITECTURE "Registry").
2. Gaps the x86 Inno runs leave in the ledger: FormatMessage's HRESULT texts (REGDB_E_CLASSNOTREG:
   on that error path, when a COM class is missing, Inno's Setup raised an
   access violation, not yet understood), comctl32's TaskDialogIndirect,
   shell32's SHPathPrepareForWriteW, rstrtmgr, riched20/msftedit and sfc
   for i686.
3. Later: the handover carrying WOW64 processes; gala-um running 32-bit
   code (a 32-bit code segment and a TEB32 through the host's
   modify_ldt); ARM64 guests are out of scope.

**Done when**: an x86 Inno Setup installer installs, its 32-bit program
runs and uninstalls under `gala install` (a scenario like
`pk_install_inno` for the x86 build, in the suite),
the wow64 scenarios pass in the suite, and the ledger lists no
`wow64.dll!` gaps for them.

### PK. Installing software: `gala install` over winget manifests

**Goal**: `gala install` reads winget's community manifests
(winget-pkgs: identifiers, versions, installer types and URLs, SHA-256,
silent switches, nested installer files) and installs as winget would,
from the command line first (CLI parity; the server profile too).

**Done so far**:
- Stages 1-2 (`pk_install_test`, scenarios `pk_install_zip` and
  `pk_install_https`; host tests `cargo test -p galapkg`, with a failure
  injected at every step): `gala install|search|list|upgrade|uninstall`
  over a winget-pkgs tree (`--source <dir>` with an `installers\`
  mirror; installers not mirrored downloaded over http(s) through
  winhttp and Schannel, redirects followed: TLS step 4; the tree itself
  over HTTPS: GitHub's contents API and raw files for
  `https://github.com/<owner>/<repo>`, the default source, or a tree
  served with JSON listings, scenario `pk_install_https`), manifests 1.x
  singleton and multi-file, SHA-256 checked, zip
  (deflate) and portable installs to `WinGet\Packages`, commands as
  shims (gala.exe + `.shim`) in `WinGet\Links` on the user's PATH,
  winget's ARP entry, transactions with rollback (upgrade included), an
  install log and ledger records; CreateProcess searches PATH.
- Machine-scope portable installs (scenario `pk_install_machine`,
  `pk_install_test machine` run as SYSTEM; host tests
  `machine_scope_*`): `--scope machine` (or a portable installer whose
  Scope is machine) installs to `%ProgramFiles%\WinGet\Packages` with
  its commands in `%ProgramFiles%\WinGet\Links` on the machine's PATH
  (`HKLM\SYSTEM\...\Session Manager\Environment`) and the ARP entry in
  HKLM; list, upgrade (keeping machine scope) and uninstall; without
  administrator rights (the token not in BUILTIN\Administrators,
  CheckTokenMembership) install and uninstall are refused before any
  change, exit 6, as winget's APPINSTALLER_CLI_ERROR_COMMAND_REQUIRES_ADMIN
  (the zip scenario checks the boot user is refused).
- The winget resolver (`galapkg::winget`, host-tested on 400 real
  winget-pkgs version directories): schemas 1.0.0-1.28.0, installer
  selection as winget does it with every rejection's reason, switches,
  dependencies into an ordered plan, ARP matching, version ordering.
  Every package command uses it (scenarios `pk_install_zip`,
  `pk_install_nsis`): `--scope`, `--architecture`, `--locale`,
  `--installer-type` as requirements; `--silent`/`--interactive` (else
  winget's silent-with-progress), `--custom`, `--override`, `--log`;
  PackageDependencies installed first (`--skip-dependencies`); upgrades
  keep the installed type and scope; `list` and `upgrade` match programs
  installed without gala to the source's packages (name and publisher,
  product codes) and show what is available. Where winget's choice is a
  type gala cannot install yet it falls back to one it can (a stopgap:
  "Known issues and stopgaps").
- Stage 3, installers that are programs (scenario `pk_install_nsis`,
  `pk_install_test nsis`; host tests): InstallerType inno, nullsoft and
  exe run as winget runs them (staged in %TEMP%, winget's silent
  switches or the manifest's, Inno's /LOG, `--location` as /DIR= or
  /D=); the install is found through its ARP entry (ProductCode, the new
  key, the package's name, the previous version's key) and tagged;
  upgrade runs the new installer over the old; uninstall runs
  Quiet/UninstallString with the silent switch and waits for the entry
  to go. A real x64 NSIS installer (made with makensis from
  `user/pk_install_test/nsis`) installs, runs, upgrades, uninstalls and
  installs with /D= under gala. A real Inno Setup installer (scenario
  `pk_install_inno`: Inno Setup 7.1.1's own x64 installer with
  winget-pkgs' manifest, PREBUILT `innosetup`) installs per user under
  `gala install` with winget's switches (its loader unpacks Setup to
  %TEMP%, Setup passes its x86 check since WOW stage 1 and runs its
  second stage, which installs the files, the uninstaller, the `.url`
  shortcut file, the six `[Icons]` shell links in the Start menu group
  (CLSID_ShellLink, `.lnk` files as [MS-SHLLINK] lays them out:
  shell32_test) and the `Inno Setup 7_is1` entry, and runs its `[Run]`
  entry `ISIDE.exe /assoc`, the IDE writing the `.iss` association under
  `HKCU\Software\Classes`), its ISCC.exe runs, it is listed and
  uninstalled through its QuietUninstallString (the shortcuts and, by
  `ISIDE.exe /unassoc`, the association removed). Its x86 twin
  (`pk_install_inno32`: Inno Setup 6.5.4, winget's JRSoftware.InnoSetup,
  whose ProductCode is only in AppsAndFeaturesEntries) does the same under
  WOW64. What
  it needed of gala is built and tested (win32_misc_test,
  kernel32_locale_test, gui_test, comctl32_test, user32_test,
  gdi32_test, shell32_test, kernel32_misc_test, oleaut32_test,
  ole32_test): EnumCalendarInfoW, SetSearchPathMode,
  SetProcessDEPPolicy, GetThreadUILanguage,
  SetThreadPreferredUILanguages, global atoms, the en-US LCTYPEs Delphi
  reads (and the hand-typed ones corrected), winspool.drv (no
  printers), SetClassLongPtrW, the *ForDpi and gesture functions,
  GetCurrentPositionEx, uxtheme's buffered painting (no buffers);
  comctl32's flat scroll bars; user32's GetClassInfoW (the window
  procedure was copied to the wrong offset of WNDCLASSW, so the VCL
  re-registered its control classes and failed with
  ERROR_CLASS_ALREADY_EXISTS), GetLastActivePopup, ShowOwnedPopups,
  AnimateWindow, layered window attributes, shutdown-block reasons;
  gdi32's EnumFontsW, GetTextExtentPointW, LineDDA, CreatePenIndirect,
  UnrealizeObject, RectVisible; shell32's Start menu folders and
  FOLDERID_UserProgramFiles (created on request); kernel32's private
  profile (INI) functions; oleaut32's variant arithmetic and logic,
  VarR4FromStr, VarBstrFromDate; ole32's server-process count; a new
  msctf.dll (input scopes); for the shortcuts, shell32's shell links
  (IShellLinkW, IPersistFile, IShellLinkDataList, IPropertyStore) and
  SHAddToRecentDocs; for the IDE to load (comdlg32_test, user32_test,
  gdi32_test, kernel32_misc_test, bcrypt_test): comdlg32's
  GetFileTitleW and the dialogs' parameter checks, user32's
  SendDlgItemMessageW, GetMenuBarInfo, GetMenuItemRect,
  RegisterClassExA, kernel32's FileTimeToDosDateTime, FindStringOrdinal,
  EnumResourceLanguagesW, gdi32's ANSI text functions, imm32's
  input-context calls, bcrypt's ECDSA key export (the IDE's signature
  check of ISCmplr.dll).
- Stage 4 groundwork: the MSIX/AppX reader (packages and bundles,
  AppxManifest.xml, block-map hashes per 64 KiB block, package full and
  family names); AppxSignature.p7x presence only.
- Stage 5 groundwork: compound files, the MSI database, cabinets (none,
  MSZIP, LZX), conditions, Formatted strings and the install plan
  (`msi::plan`).

**Next**:
1. The resolver's rest on the install path: dependencies of an
   upgrade's new version, UpgradeBehavior (`uninstallPrevious`, `deny`),
   RequireExplicitUpgrade, the user's winget settings
   (`installBehavior.preferences`/`requirements`) and locale.
2. winget's source index for search and listing
   (`cdn.winget.microsoft.com/cache/source2.msix`: the MSIX reader and
   sqlite), instead of GitHub's API and its rate limit; a first
   real-world install against GitHub when a scenario can reach it
   (`sigoden.AIChat`, item AI); real symbolic links for commands; the
   logon environment from `HKCU\Environment`.
3. Stage 3's rest: what Inno Setup's own install still reports and
   skips (the ledger of `pk_install_inno`): unpinning its shortcuts at
   uninstall (SHCreateItemFromParsingName), SHPathPrepareForWriteW, the
   taskbar list (CLSID_TaskbarList), the Restart Manager (rstrtmgr.dll),
   HTML Help (hhctrl.ocx); its compiler compiling a script (ISCC.exe and
   the IDE's ISCmplr.dll write the Setup's icons and version with
   BeginUpdateResourceW/UpdateResourceW/EndUpdateResourceW, stubs today:
   rewriting a PE's resource section); an x86 Inno installer (WOW stage 3); its
   machine install as an administrator (`/ALLUSERS`); Scope switches and
   elevation for machine-wide installers (a non-portable machine
   installer run by a standard user: winget leaves elevation to the
   installer, gala's UAC has no prompt yet); the manifest's success codes; a tracking catalog of
   gala's own instead of tagging vendors' ARP keys; the needs of other
   vendors' installers (IShellLinkA shortcuts, file associations, UAC
   manifests as the admin token) as the ledger names them.
4. Stage 4, MSIX sideloading for packaged Win32 programs: signature
   validation (PKCS#7 over the block map against trusted certificates),
   staging to `C:\Program Files\WindowsApps\<full name>`, registration,
   package identity for full-trust apps (GetCurrentPackageFullName, the
   package's VFS and registry redirection, its app execution alias),
   removal; a makeappx-built fixture (needs Windows) to confirm the
   spec-derived rules (per-block deflate runs, bundle Offset).
   AppContainer apps later (BR stage 3 helps).
5. Stage 5, MSI: the engine running the plan (files from the cabinets,
   registry, shortcuts, an ARP entry, rollback through `install::Tx`),
   AppSearch and type 51/35 custom actions, then msiexec/msi.dll with
   custom actions in DLLs and EXEs, transforms, `/l*v` logging; enough
   for `msiexec /i x.msi /qn` of common packages and Edge's MSI. Plans
   are checked against the documented rules only (TODO: compare
   `msiexec /l*v` on Windows).
6. Later: stock winget itself (an MSIX) against gala, the Microsoft
   Store source, `winget configure`.

**Done when** (each stage): a scenario installs a real package of that
type, runs it and uninstalls it, and is kept as a regression test with
the ledger's per-installer gaps in the report.

**Design**: ARCHITECTURE "Installing software", "The winget resolver",
"LZX"; WORKFLOW "Installing software".

### R. Robustness and live update, kernel included

**Goal**: the path to the vision: invariants, safety nets, then live
update of user mode and the kernel. Order: R1 -> R2 -> R3 -> R4
(alongside other work) -> R5 -> R6 -> R7 -> R8 -> R9.

**Design**: ARCHITECTURE "Built in early: invariants, safety nets, live
update" and its subsections.

#### R2. Fault injection (done; leftovers)

Built: `failpoint=` / `failseed=` / `failtrace` on the kernel command
line and in gala-um, `cargo xtask fault-test [filter] [--sweep[=point]]`,
`cargo xtask alloc-audit`, scenarios `failpoint_heap`, `failpoint_frame`,
`failpoint_random`, `failpoint_disk` (ARCHITECTURE "Fault injection",
WORKFLOW "Fault injection"). Left:
- Out of memory in gala's ntdll loader shows as STATUS_DLL_NOT_FOUND and
  in child process creation as STATUS_INVALID_IMAGE_FORMAT.
- A QEMU sweep of the kernel-only paths (child processes, the
  scheduler, vmap) like `--sweep`; a `disk_io` sweep (disk scenarios do
  not run in gala-um); more programs in FAULT_PROGRAMS (Go's runtime
  beyond go_hello, the AFD tests).

#### R5. Deterministic simulation (done; one refinement)

Built: `um --sim=<seed>`, `cargo xtask sim-test` (`--all`: every gala-um
program); all programs, Go's included, repeat from their seed
(ARCHITECTURE "Deterministic simulation", WORKFLOW "Deterministic
simulation"). Next: preemption by instruction count (performance
counters, as rr does) for long computations, on hosts that have them.

#### R6. Charging resources to an owner

Done: the commit charge (`nt::charge`, `commit_test`; ARCHITECTURE
"Commit charge"); kernel pool and handle quotas (`crate::quota`,
`nt::quota`, `quota_test`, `quota_sockets`, `quota=`; ARCHITECTURE "Pool
and handle quotas"): objects, handle entries, pipe buffers, section page
tables, tokens (their dynamic part as a default DACL grows), completion
ports' queued packets and sockets' buffers (AFD and Linux, sized by
SO_RCVBUF/SO_SNDBUF) are charged to the process, STATUS_QUOTA_EXCEEDED
(WSAENOBUFS, ENOBUFS) and the handle limit's STATUS_INSUFFICIENT_RESOURCES
fail the program using too much; NtSetInformationProcess
(ProcessQuotaLimits) changes the limits (SeIncreaseQuotaPrivilege).
Registry data, image layouts and job-wide pool limits are left out by
design (ARCHITECTURE says why). Next:
1. quota_test on real Windows (run-all.ps1): the handle limit's status
   (ERROR_NO_SYSTEM_RESOURCES is assumed) and its exact count; the
   socket part (SO_RCVBUF read back, connected sockets' charge).
2. kernel32's SetProcessWorkingSetSize through ProcessQuotaLimits; the
   sockets of an AFD endpoint a kernel handover carried charged again
   (its process's quota is restored after it).
GPU memory is charged here too (item GPU).

#### R7. Live mocking

Done: clock control (`clock=`, GalaClockControl), record/replay of the
boot time, clock readings and random numbers (`record`, `replay=`,
`um --record/--replay`, GalaRandom), disk latency
(`failpoint=disk_io:delay:<ms>`), absolute timers and timeouts that
follow a moved system time (ARCHITECTURE "Clock control",
"Record/replay"; WORKFLOW "Clock control, record/replay, slow devices").
Next:
1. Record/replay of network frames, disk reads and input events (needs
   a deterministic guest: gala-um's simulation first).
2. The display and input as swappable devices; device events (NIC
   removed, resolution change).
3. API-level faults in gala's DLLs (Application Verifier style).
4. The read-write control channel (OBSERVABILITY "Planned facilities",
   the live introspection channel).

#### R8. Chain of trust for updates

Done (steps 2-6; scenarios `boot_tampered`, `boot_config_edited`,
`handover_unsigned_refused`, `rollback_refused`): Limine's per-file
BLAKE2B hashes and enrolled config hash, an Ed25519-signed manifest
(GALAMANI version 1) checked by the kernel and by a handover, a rollback
floor in the superblock's sector. Next:
1. Step 1: Secure Boot with OVMF test keys (ARCHITECTURE "Chain of
   trust" lists what it needs).
2. Images from the system tree checked against the manifest.
3. A release key separate from the test key; the rollback floor kept in
   a TPM NV counter, and raised only after the new release's self-check.

WORKFLOW "Signed images".

#### R9. Live update, kernel included

**Goal**: user mode first (versioned system images behind an atomic
pointer, old processes restarted one at a time), then the kernel
handover: the running kernel stops every thread at a quiescent point,
writes its state as versioned data, and a new kernel rebuilds it and
reissues blocked calls, so programs carry on. Live patching of single
functions only if a need appears.

**Done so far** (milestones, each with its scenarios): (a) handover to
the same build with no programs (`handover_same`, `handover_uefi`,
`handover_disk`); (b) with idle programs (`handover_idle`,
`handover_idle_uefi`, `handover_idle_twice`, `handover_refused`); (c)
with busy programs on gala's DLLs, blocked in waits and I/O
(`handover_k32`, `handover_k32_uefi`, `handover_k32_twice`); (d) to a
newer build with a changed state format and back
(`handover_upgrade`, `handover_upgrade_uefi`,
`handover_downgrade_refused`); (e) with open TCP and UDP connections
(`handover_net`, `handover_net_uefi`). What cannot be carried makes the
handover refuse cleanly with the reason.

**Next** (what is left for live update):
1. Carry what is refused today: threads blocked over 3 s in calls that
   are not restartable (pipe writes on a full pipe, console reads, pipe
   listens); files and pipes bound to completion ports and asynchronous
   pipe requests in flight (carry the request: status block, event,
   port, APC); console windows and conhost sessions; jobs; registry keys;
   program-made object directories and links; writable file views; a
   handle to the AFD device.
2. Carry what is neither carried nor refused today: byte-range locks, a
   pipe client's security context, the window registry, the desktop
   (shasha), volatile registry keys (RS B5 for the window state).
3. Image pages as frames across a loader change (refused today; the
   layout digest catches a difference only after the point of no
   return, so a newer loader must keep the layout until then).
4. Networking: IPv6 and raw sockets, the neighbor cache (asked again by
   ARP today), HTTP keep-alive with a real server (go_http or Nginx;
   Nginx's logs need `persist`), a shorter window without network (stop it
   after the image is prepared).
5. Free the first kernel's image and Limine's reclaimable memory.
6. R8's rest (Secure Boot, a release key).
7. The user-mode half: versioned system images behind an atomic
   pointer, processes restarted one at a time.

**Done when**: a running gala serving sshd and Nginx updates its
system image and its kernel and every connection and session goes on.

**Design**: ARCHITECTURE "Kernel handover", "Programs across a
handover", "Versions and negotiation", "Sockets across a handover";
WORKFLOW "Debugging a kernel handover (R9)".

### OT. OpenTelemetry as the telemetry model

**Goal**: one data model (traces, spans, logs, metrics sharing trace
and span ids, W3C Trace Context) for what are separate streams today
(serial log, debugcon events, `stats:` lines, crash reports, state
dumps), readable by standard tools (an OTel collector, Jaeger, Tempo,
Grafana), so work can be followed from a program into the kernel. OTel
is the model, not the in-kernel wire format: the kernel keeps compact
per-CPU binary records, and an exporter produces OTLP outside the hot
path.

**Done so far**: OT1, `cargo xtask otel <log | events.jsonl | scenario>`
and `XTASK_OTEL=1` (`xtask/src/otel.rs`): a boot as OTLP/JSON traces,
logs and metrics, to files or OTLP/HTTP; it counts what it could not
map and the stream's gaps. Not yet shown in a live Jaeger (the WORKFLOW
recipe is untested). From OT2: every event names the thread whose work
emitted it (`"by"`, kept per CPU by the scheduler), so faults, image
loads and console or network events land on a thread; `process start`
names the parent process and creating thread, and the export nests a
child under its parent with a span link to its creator (scenario
`events_process`). From OT3: the `ipc` events (pipes with byte offsets,
completion packets with who queued them and when; scenario
`events_ipc`) become spans linked write to read and post to dequeue,
and the converter names the slowest hand-offs. The sampling profiler's
samples go to OTLP's profiles signal (v1development), each linked to
its thread's span; kernel samples carry their frame-pointer stack, so
the profile names a hot kernel function's callers (user-mode stacks
need unwinding: next).

**Next**:
1. OT2: trace context in the kernel; system calls, waits, faults, I/O
   and network as spans; per-CPU rings; user-mode stacks on samples
   (the unwind tables of x64 images). Still open of OT1's gaps: an
   entry event for every system call; times on serial lines; the boot's
   wall-clock start.
2. OT3: trace context carried through ntdll/kernel32 (a TEB slot) and
   ALPC/RPC; sockets and events signaled across processes as links like
   pipes and completion ports.
3. OT4: the ETW bridge (EventRegister/EventWrite, TraceLogging) turning
   what Windows programs emit into OTel logs and spans; later the same
   for LX.
4. OT5: the in-guest OTLP exporter for servers, controlled from the
   `gala` CLI (on/off, sampling rules, endpoint), metrics pushed to
   Prometheus' OTLP receiver on the same host (diagnosis, not
   availability; `/metrics` for scraping as an option); later, anomalies against a key's own baseline
   suggesting (then, within a budget, starting) an investigation;
   defaults: metrics, failures, ledger, crash reports and the flight
   recorder always on, traces and the profiler on demand
   (OBSERVABILITY "Volume").
5. OT6: visualization (OBSERVABILITY "Visualization"): the Perfetto
   export is done (`cargo xtask perfetto`: tracks per thread, IPC
   flows, samples); next a live view (`cargo xtask
   observe`: streaming OTLP into a collector and Grafana's stack with
   provisioned dashboards: overview, system (Task Manager's Performance
   tab: per-CPU user, kernel and interrupt time, per-process CPU and
   memory, from kernel time accounting that ThreadTimes and
   NtQuerySystemInformation also return), hot paths, impact table,
   service map; volume bounded per signal: metrics aggregated, traces
   sampled per transaction at the root and at its end by sampling rules
   (matching name, type, process, status, duration; OBSERVABILITY
   "Volume"), profiles aggregated per stack, a byte budget, dropped
   counts reported), then transactions (causal chains with owners, callers and a
   critical-path breakdown) as their own view.

**Design**: OBSERVABILITY "OpenTelemetry export"; WORKFLOW
"OpenTelemetry".

## Known issues and stopgaps

Each entry: what, why it matters, the structural fix, the owning item.

### Stopgaps (to replace)

- **Hand-written busybox transcripts** (`user/lx/busybox/lx_busybox.txt`,
  `lx_httpd.txt`): every other Linux transcript is recorded on real
  Linux (`cargo xtask lx-expect`); these were written from POSIX and
  busybox's documented output and cross-checked against the host's dash
  and coreutils, because the pinned busybox (busybox.net's static 1.35.0)
  is not run on the host. Only the busybox-specific lines (xargs, wget's
  status codes) are unchecked. Structural fix: record both transcripts
  by running the same pinned binary on a Linux machine (or a Linux VM
  xtask boots), and let `lx-expect` regenerate them. Item LX.

- **`--no-sandbox` in the Chromium scenarios**: Chromium's renderers
  normally run in a sandbox (restricted, low-integrity and
  AppContainer/LowBox tokens, job objects, process mitigation
  policies, handle inheritance lists, an alternate desktop); gala lacks
  parts of it (CreateDesktopW, LowBox tokens, mitigation policies), so
  sandbox set-up would fail and no renderer would start. The switch,
  Chromium's own, runs every Chromium process on the user's ordinary
  token (the browser shows its "unsupported command-line flag" bar).
  Structural fix: BR stage 3, the sandbox's pieces; then the switch
  goes.

- **`--disable-direct-composition` in `chromium_window`**: on Windows 11
  22H2 and later Chromium draws even software frames through a D3D11
  DXGI swap chain under DirectComposition (`kRemoveRedirectionBitmap`),
  which gala's GPU stubs refuse (its GPU process then CHECKs for
  E_ACCESSDENIED, session 0's answer). The switch, Chromium's own, keeps
  its GDI path (GetDC and BitBlt into the window), which every Windows
  also has. Structural fix: DXGI swap chains and DirectComposition
  surfaces that shasha composes (GPU G2 for the device, G4 for
  presentation); then the switch goes. Item BR stage 2.

- **`--ipc-connection-timeout=120` in `chromium_window`**: a Chromium
  child process ends itself when its browser has not answered its first
  ping within 15 s ("Terminating current process after 15 seconds with
  no connection"); the browser answers on its UI thread, which under TCG
  is busy with start-up for longer than that (the guest's clock follows
  the host's, its CPU runs tens of times slower). The network service
  died mid-navigation and the page never loaded (the switch makes it
  load: measured). The headless scenarios recover by restarting the
  service. Structural fix: guest time that follows the emulated CPU's
  work, so that programs' own timeouts see what real hardware would
  (QEMU `-icount`: `shift=0,sleep=off` stops at "Bad icount read" during
  the UEFI firmware, to investigate), or KVM. Item BR stage 2 (owner),
  T2 (test environment).

- **Name constraints fail closed**: crypt32's chain engine does not
  evaluate a CA's name constraints extension; a chain through such a CA
  gets CERT_TRUST_INVALID_NAME_CONSTRAINTS and the policies refuse it
  (CERT_E_INVALID_NAME), recorded as `crypt32.dll!CertGetCertificateChain(name
  constraints)`. Accepting it unchecked would let a constrained CA
  vouch for any name. Structural fix: evaluate permitted and excluded
  subtrees (DNS, IP, directory names) as RFC 5280 6.1 does. Item TLS.

- **`gala install` falls back across installer types**: winget installs
  the installer its selection ranks first; where that is one gala cannot
  install yet (msi, wix, burn, msix, appx, a zip of those), gala
  installs the
  best-ranked one it can instead (`install::choose`), logged as
  `fallback installer=N instead of installer=M` and recorded in the
  ledger. Without it a package that offers both an MSI and a zip would
  not install at all; with it the program lands where the zip's layout
  puts it, not where winget would. Scope never falls back. Structural
  fix: PK stages 4 and 5 (MSIX, MSI), then gala installs winget's
  choice and the fallback goes. Item PK.

### Defects known, not yet fixed

- **chromium_window: input that reaches Chromium's window but not the
  page**, about 1 run in 4 since the typing step: once the link click
  (delivered at the link's place to Chromium's window, the button ring
  shows), once the typed keys (the page saw the field's click, then no
  key). Whether Chromium's UI thread took that input from gala's queue is
  what the state dump now says (each recent button and key: taken after
  N ms, not taken, or dropped), and window.html logs every mousedown,
  click and keydown it gets; the next failure tells gala's side from
  Chromium's. Item BR.

- **An NT delete through a hard link's second name removes the first
  name**: hard links exist (Linux link/linkat, seam 4), but NT's delete
  (FILE_DISPOSITION / DeleteFileW) acts on the node's first name, not the
  name it was opened by. Only a Linux-made link deleted from Windows
  shows it today (NT has no CreateHardLinkW yet). Fix: NT file objects
  remember the name they were opened through and delete that one. Item
  LX (seam 4's NT side).

- **console_window step 5 under load**: about 1 run in 20 of the full
  suite, the wheel scrolls conhost back 15 lines instead of 18 (the
  screenshot shows `history[1]` on top): one of six notches is missing.
  Which layer drops it (QEMU's PS/2 queue while the guest is slow, the
  kernel's input path, the window's message delivery) is open; the
  `input:` line at exit now counts the notches the kernel received, so
  the next failure says. Item W3.
- **Fault-injection leftovers**: out-of-memory reported as
  STATUS_DLL_NOT_FOUND / STATUS_INVALID_IMAGE_FORMAT. Item R2.
- **Seen once, unexplained**: sshd_password's sshd.exe ended with an
  unhandled access violation at 0xffffd0000013f948 (a kernel-half
  address read from user mode) after `ReadFileEx ERROR 38` lines, under
  the full suite; it passes alone. Owner: SSH; to hunt with
  `XTASK_REPEAT`.
- **Observability gaps** still open from the audit: program output on
  its own channel (split lines), reference holders of a leaked address
  space, a real user stack walk in crash reports, repeat boots and
  um-test results in `target/report/history.jsonl` (OBSERVABILITY "Open
  gaps").
- **java_http timed out once with a lost wake-up's signature** (JVM
  threads spinning in NtYieldExecution, two waiting on semaphores with
  count 0) after the NtReleaseSemaphore fix; whether it recurs on the
  pinned QEMU is not recorded. Owner: P1; WORKFLOW "Known intermittent".

### Pending scenarios

`PENDING` in xtask: run by `cargo xtask boot-test <name>`, not by `cargo
xtask test` or CI.

| Scenario | Why pending | Owner |
|---|---|---|
| `java_stress` | a stress reproducer: 5-15 min a boot under load (30 min limit); passes 15 of 15 on the pinned QEMU; to decide: a regular scenario or dropped | P1 |
| `elasticsearch` | 3 GiB and about 8 minutes under TCG | P1 |
| `sqllocaldb`, `sqllocaldb_persist` | 3 GiB and a 50 MB MSI download (1 GiB disk for the second) | P1 |
| `mingit` | not recorded (it passed when written; a PREBUILT download); a candidate for the suite | P1 |
| `gitbash` | blocked (P1 Git Bash is parked) | P1 |
| `filezilla`, `filezilla_flaui` | slow (about 100 s and 25 s in the kernel) and the installer's URL answers 404 (AG "Known problem") | W3 |
| `sshd` | `ssh-keygen -A` and `sshd -t` only; superseded by `sshd_login`, `sshd_server` | SSH (move into the suite or drop) |

### Environment

- **QEMU must be the pinned one**: QEMU 8.1.0-8.2.5's TCG (Ubuntu
  24.04's 8.2.2) runs stale code rewritten on a translated block's
  second page; it corrupted the JVM's heap. xtask runs QEMU 11.1.3
  (`cargo xtask qemu`); `straddle_test` checks for the bug (WORKFLOW
  "QEMU", "Known intermittent").
- **QEMU's multi-threaded TCG** can run stale translations of code
  another processor rewrote (`xmodify_test`): programs that generate or
  patch code boot with `-accel tcg,thread=single` (`SINGLE_THREAD_TCG`
  in xtask), at about half the speed. A new JIT program belongs in that
  list (WORKFLOW "Known intermittent").

## Later

Wanted, not scheduled. Each becomes an item with a section when it is.

- **Self-hosting**: building gala's own test programs (then Nginx) on
  gala. H1-H10 built the base (pipes, child processes, busybox, TinyCC
  building itself, make, Lua and SQLite built on gala); what is left is
  a Windows-hosted toolchain (llvm-mingw, or the MSVC Rust target)
  running on gala, cargo spawning compilers, and a larger writable file
  system on the disk.
- **DR, Windows drivers, staged and isolated**: Windows keeps a stable
  binary driver ABI, so vendor drivers are a realistic catalogue (Linux
  drivers are not: no stable in-kernel API, GPL-2.0; LX is a user-space
  ABI only). Sources: the WDK documentation (CLEAN_ROOM). Invariant: a
  third-party driver never runs unsupervised in gala's kernel address
  space; a driver crash restarts its host, and kernel handover (R9)
  keeps working. Stages: (1) UMDF 2 drivers in a user-mode host
  (sensors, many USB devices, some printers and readers); (2) KMDF
  drivers in an isolated driver host with a restarting supervisor, plus
  the class frameworks hardware needs: NDIS miniports, Storport
  miniports, HID minidrivers; (3) raw WDM drivers only for a needed
  device with no other route; (4) WDDM: not planned (item GPU covers
  virtual machines). Each stage test first against a real driver binary
  in QEMU, with the driver host's state in crash reports and the state
  dump. User-space Linux drivers (libusb, FUSE, CUPS, SANE) come with LX
  when it gets device interfaces.
- **gala's own drivers** against published standards: after virtio,
  AHCI/NVMe, xHCI, e1000; device interrupts (all devices are polled
  today).
- **ARM64**: a second arch port (boot, MMU, exceptions, timer, context
  switch), ARM64 PE images and ARM64 builds of gala's DLLs; the NT layer
  above is portable. x86_64 (Intel and AMD) is the only port today.
- **Raspberry Pi** (on the ARM64 port): Pi 4/5, QEMU's `raspi4b`/`virt`
  first; Limine or the Pi's firmware loading the kernel, the GIC and the
  generic timer, the PL011 UART, the mailbox framebuffer, then EMMC2/SD,
  USB through xHCI (the Pi 5's RP1, the Pi 4's VL805) and GENET
  Ethernet, all against published datasheets.
- **x64 programs on ARM64**: a translator of gala's own from documented
  behaviour (the x86-64 manuals, Arm's ARM): an interpreter of user-mode
  x86-64 for correctness first, then a block-at-a-time JIT to ARM64 with
  a translation cache, lazy flags, x87/SSE on NEON, self-modifying code
  caught through write protection, x64 DLLs loaded as on x86, and the
  syscall boundary translated (the NT layer stays native). Windows'
  documented ARM64EC/CHPE design can guide the interface between
  emulated and native code. After the ARM64 port runs native programs.
- **Smaller gaps with no item yet** (each stated as a limit in its
  ARCHITECTURE section; cross-cutting ones in its "Known limits"):
  - Scheduler: processor time accounting (thread times are 0,
    GetSystemTimes reports all idle); thread priorities are not
    scheduled on.
  - Loader and threads: relocating an executable whose preferred base
    is taken (`process::map_image`); LOAD_WITH_ALTERED_SEARCH_PATH's
    dependency search; NtCreateThread applying FltSave beyond MxCsr and
    the XSTATE area; SEH's nested exceptions and collided unwinds.
  - Waits: wait-all satisfied only when all objects are signaled at its
    re-check; a kernel address wait (WaitOnAddress) for kernel32's
    lightweight locks; an atomic SignalObjectAndWait; the thread pool's
    callback environments and freeing of closed objects.
  - Pipes: WaitNamedPipe, TransactNamedPipe, PIPE_NOWAIT, pipe
    security, asynchronous writes that pend.
  - Access checks for processes, threads, pipes, devices and traversal;
    labels on objects a low-integrity token creates; SetSecurityInfo's
    inheritance propagation; auditing.
  - Consoles: Ctrl+Break, CTRL_CLOSE_EVENT, GenerateConsoleCtrlEvent,
    process groups, READCONSOLE_CONTROL wake-up keys, the scroll bar,
    rewrap on resize; ConPTY rendering of cell-based console APIs into
    VT, OSC 0 titles, mouse, bracketed paste, VT input.
  - Resources: job limit messages,
    charging copy-on-write section views, limits on kernel commits;
    nested jobs, job time accounting and the other job limits.
  - Files: directory notification filters for attributes and times.
  - DLLs: msvcrt printing inf/nan as `inf`/`nan` instead of `1.#INF00`;
    ncrypt's persistent named keys, ECDSA, key generation and export;
    crypt32's persistent certificate stores (system stores open empty);
    secur32's SPNEGO-wrapped Negotiate tokens, NTLM proved against
    Windows.
  - Accounts: the SAM key readable by programs, NetUserAdd,
    KERB_S4U_LOGON, identification-level S4U tokens for untrusted
    callers, the default profile's contents and NTUSER.DAT (D3).
- **Tooling**: bindgen-generated layouts from mingw-w64 headers;
  verifying syscall numbers against traces of the pinned build (Windows
  11 24H2, 26100); trace diffing against Windows, the sampling profiler
  and live introspection, fuzzing (OBSERVABILITY "Planned
  facilities").

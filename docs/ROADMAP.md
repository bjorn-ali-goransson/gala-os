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
   a time (item HS; checklist in WORKFLOW "Hygiene sweeps"), after a
   change of assumptions and otherwise about monthly.
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

1. **HS**: the hygiene sweep leftovers (IOCP waits on a closed port, APCs
   to an exiting thread, NtRemoveIoCompletion losing a packet,
   NtSuspendThread on an exiting thread), each test first.
2. **TLS**: winhttp over Schannel (TLS step 1).
3. **LX**: seams 1, 4 and 5 as their areas are touched; milestone 2
   (static busybox) as filler.
4. **RS**: step B3a (window state apart from composition, presenters).
5. **R**: R7's next part (network frames, disk reads and input in
   record/replay), R6's kernel pool and handle quotas, R8's Secure Boot.
6. **PK**: `gala install`/`list`/`upgrade` on the winget resolver; HTTPS
   downloads (after TLS step 1); a real Inno Setup installer.
7. **BR** stage 1 (headless Chromium; the CHECK at the end of the
   browser's start-up next) with **GPU** G0.
8. **AG**: the remaining stubs (filler).
9. **SSH**: scp, port forwarding, the client; **H11**: busybox httpd
    and wget over loopback.

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
| HS | Hygiene sweeps | in progress | recurring; first sweep done, four leftovers |

### NT core services (done, by test)

| Area | Tests |
|---|---|
| Virtual memory: VAD, demand-zero paging, allocate/free/protect/query, guard pages and stack growth, copy-on-write | `vm_test`, `guard_test`, `cow_test`, `vmcycle_test`, `commit_test` |
| Objects, handles, names, symbolic links | `object_test`, `symlink_test` |
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
| N1 | Networking: virtio-net, TCP/IP, AFD, ws2_32, UDP | done | `net_echo`, `winsock_test`, `winsock_async_test`, `go_http`, `udp_test`, `udp_echo`; UDP limits in ARCHITECTURE "Networking" |
| N2 | Nginx (x64 build, master/worker, zlib, PCRE2, OpenSSL) | done | `nginx`, `nginx_master`, `nginx_tls`; WORKFLOW "Nginx" |
| H1-H10 | Toward self-hosting: pipes, busybox, scripts, TinyCC (and itself), make, Lua, build speed, SQLite, file locks | done | `busybox*`, `tcc`, `tcc_self`, `tcc_make`, `lua`, `sqlite`; PERF.md |
| H11 | busybox httpd and wget over loopback | planned | |
| P1 | Unmodified prebuilt programs | in progress | Caddy, JDK, .NET, PowerShell, Elasticsearch, LocalDB, OpenSSH, PostgreSQL, MinGit, FileZilla Server done; Git Bash parked |
| SEC | Security model: tokens, descriptors, access checks, logons, integrity, UIPI | done | SECURITY "Status": phases S1-S6 (leftovers listed there) |
| D1-D3 | Disk: virtio-blk, C: kept (`persist`), the registry kept | done | `disk_test`, `persist_test`, `sqllocaldb_persist`; ARCHITECTURE "Disk" |
| TLS | Schannel and the HTTP stacks on it | in progress | Schannel done; winhttp, wininet, curl next |
| AG | API gaps in the shell and runtime DLLs | in progress | 11 groups done; filler |
| LX | Linux personality | in progress | seams 2, 3, 6 and milestone 1 done |

### Desktop, sessions and remote access

| ID | Title | State | Note |
|---|---|---|---|
| W1 | Windows on the screen | done | `gui_test`; ARCHITECTURE "Windows on the screen" |
| W2 | Styled desktop and input | done | `gui_test` |
| W3 | FileZilla | in progress | connects to FileZilla Server, driven by pixels and by FlaUI; leftovers |
| W4 | The Windows look | in progress | steps 1-2 done; step 3 (themes) next |
| UIA | UI Automation and windows across processes | in progress | `uia_test`, `flaui_test`, `windows_across_test`, `filezilla_flaui`; ARCHITECTURE "UI Automation" |
| SD | Server and desktop workloads kept apart | in progress | `profile=server`, CLI parity, server kernel done |
| SSH | SSH as a remote shell | in progress | steps 1-3 in their first form |
| RS | Remote access and sessions | in progress | A, B1, B2 done; B3a next |
| GPU | Graphics and GPU | in progress | G1 done; G0 next; G3 parked |
| BR | A web browser: stock Edge | in progress | stage 1: Chromium's browser start-up runs |
| OF | Microsoft 365 apps and the new Outlook | planned | gap analysis first |

### Robustness, live update, tooling

| ID | Title | State | Note |
|---|---|---|---|
| R1 | Invariants | done | adopted; new code follows them (ARCHITECTURE "Invariants") |
| R2 | Fault injection | done | leftovers in its section; ARCHITECTURE "Fault injection" |
| R3 | Lock-order checking | done | `sync::lockdep`; WORKFLOW "Lock-order checking (lockdep)" |
| R4 | Flight recorder, watchdogs, crash record across reboots | done | `watchdog`, `kernel_watchdog`, `crash_record` |
| R5 | Deterministic simulation in gala-um | done | `cargo xtask sim-test`; one refinement open |
| R6 | Charging resources to an owner | in progress | commit charge done |
| R7 | Live mocking: clock control, record/replay, device faults | in progress | clock control, record/replay of time and randomness, disk latency done |
| R8 | Chain of trust for updates | in progress | steps 2-6 built; Secure Boot next |
| R9 | Live update, kernel included | in progress | handover milestones (a)-(e) done |
| OT | OpenTelemetry as the telemetry model | in progress | OT1 done |
| PK | `gala install` over winget manifests | in progress | zip/portable and NSIS installs; MSI, MSIX and winget readers |
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

**Done when**: schannel_test and process_test pass in um-test.

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
1. NtRemoveIoCompletion waits are not ended when the port's last handle
   closes (Windows ends them: GetQueuedCompletionStatus fails with
   ERROR_ABANDONED_WAIT_0).
2. An APC queued to a thread just as it exits is accepted
   (STATUS_SUCCESS) and never runs.
3. NtRemoveIoCompletion takes the packet before writing the caller's
   outputs, so a bad output pointer loses it.
4. NtSuspendThread on an exiting thread can count a suspension the
   thread never sees.
5. The next sweep: after the next change of assumptions, otherwise a
   month after the first.

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
- MinGit 2.47.1: init, add, commit, log (`mingit`, pending).
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
2. Later: a user-mode shell (taskbar and start menu out of the kernel),
   resizing windows by the frame, keyboard focus per window.

**Design**: ARCHITECTURE "Windows on the screen".

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

**Next**:
1. WM_GETOBJECT providers (UiaReturnRawElementProvider, same process
   first), then providers inside programs (wx and Qt implement
   IRawElementProviderSimple).
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
- `profile=server` (scenario `server_profile`): the desktop is never
  drawn even with a screen, windows exist as without one, `console=`
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
  smaller). A server image without user32/gdi32 is not planned: many
  console and server programs import them, and loading them starts
  nothing; only wallpapers, fonts and GUI-only programs (conhost.exe,
  FileZilla) stay out of a server image.

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
- Step 3 begun: `sshd_sftp` (put a 200 KiB file, ls, get it back byte
  for byte, rm).

**Next**:
1. sshd in the Docker/server image.
2. The shell: an interactive prompt in gala's cmd.exe (busybox sh is
   the shell today); resize over ssh exercised (the scenario's client
   has no tty, sshd sends a 0 x 0 resize); full-screen programs that
   draw with the cell APIs (SetConsoleCursorPosition,
   WriteConsoleOutput) reaching the terminal (only VT text does now);
   `gala` commands in the shell.
3. File transfer: scp, permissions, large files.
4. Port forwarding: `ssh -L` and `-R`; agent forwarding if cheap.
5. The client: gala's ssh.exe connecting to the host's sshd, keys in the
   user's profile, known_hosts.
6. Observability: sshd's and sshd-session's logs in failed logs,
   connection and auth events in the event stream, the ledger's
   ssh-related gaps in the report.

**Done when**: each step's scenario passes; host keys persist across
boots (`persist`) and sessions survive a kernel handover (R9 (e)
carries sockets and pipes; consoles not yet).

**Design**: ARCHITECTURE "Pseudo consoles"; SECURITY "Accounts and
logon".

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

**Next**:
1. B3a: window state apart from composition, and presenters: console
   attach and switch; a test presenter (headless rendering for screen
   checks without QEMU screendumps); a session with no presenter keeps
   its windows. The Display seam's `screen_flush` and `screen_changed`
   (from GPU G1) are what a presenter needs. Window station and desktop
   objects (GetProcessWindowStation is NULL today).
2. B3b: remote shasha: the compositor as a crate that also builds for
   the host, a versioned protocol, the server end (also in the server
   kernel, which has no compositor), a host client (winit).
3. B4: a session manager, `wtsapi32` (only WTSGetActiveConsoleSessionId
   exists), `gala sessions/logoff/disconnect/attach`; and B1's open ends:
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
- G1, virtio-gpu 2D: the modern virtio-pci transport, the driver over a
  `Transport` trait host-tested against a fake device, `display=virtio`
  (default `fb`), `events=display`, `gputrace`, `gpureset=`; scenarios
  `virtio_gpu`, `virtio_gpu_reset`.

**Next**:
1. G0, for BR stage 2: `dxgi.dll`, `d3d11.dll`, `d3d12.dll`, `dcomp.dll`
   as stubs that fail as Windows does without a usable adapter
   (`gpu_stub_test`: D3D11CreateDevice, CreateDXGIFactory1,
   D3D12CreateDevice and DCompositionCreateDevice with the codes
   Chromium's fallbacks expect).
2. G1's rest: shasha's pointer on the hardware cursor,
   WM_DISPLAYCHANGE and refitting maximized windows on a resize (a test
   resizing the display through QEMU's monitor), several scanouts as
   monitors, device interrupts instead of spinning, the boot splash on
   virtio-gpu (the screen is black from the driver's start until the
   desktop draws).
3. G2, software 3D for every program: `vulkan-1.dll` (Khronos loader),
   Mesa lavapipe (fallback SwiftShader), DXVK (d3d9/10/11, dxgi),
   vkd3d-proton (d3d12), Mesa llvmpipe `opengl32.dll`; PREBUILT,
   registered as on Windows; `vk_test`, `d3d11_test`, `gl_test` with
   screen checks, then DXVK's and vkd3d-proton's own tests. WARP is
   ruled out (not redistributable).
4. G4, GPU presentation: DXGI flip-model swap chains as resources shasha
   composes; then BR stage 5.
5. G3, virtio-gpu 3D through Venus: **parked** (deferred by the owner
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
`chromium_dump` (a local page changed by its script), PENDING, in
`SINGLE_THREAD_TCG`, 3 GiB, 900 s; `browser_api_test` and `dwrite_test`
cover what was added. chrome.exe starts; chrome_elf.dll's early security
and Crashpad come up (the handler runs as a child with an inherited
handle list); chrome.dll loads; the profile's process singleton is
taken; DirectWrite and Skia's font manager come up on Selawik; the
browser's main start-up runs to its end, every delay import it reaches
on the way a real export; Crashpad's client registers with its handler
(TransactNamedPipe as one operation), and the browser gets as far as
creating its first renderer host. A run takes about a minute.

**Next** (each stage test first, the ledger leading):
1. Headless Chromium. First the crash Crashpad's snapshot reported
   nothing of until ntdll kept PEB.Ldr (the browser ended with 0xffff7002,
   snapshot failed). After it: what
   `tools/import_survey.py` still lists (the rest of the delay imports:
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
   --dump-dom file:///C:/gala/page.html` prints the DOM; then an
   `https://` page from nginx_tls in the guest and a page from the host
   network. Needs: Mojo IPC over named pipes, many threads, large
   virtual memory (V8's reservations, the commit charge), DirectWrite or
   Chromium's font fallback, CNG, Winsock, the JIT (add to
   `SINGLE_THREAD_TCG`).
2. Headful with software rendering (`--disable-gpu`,
   `--use-angle=swiftshader`; GPU G0): a window on shasha, DirectWrite,
   DirectComposition fallbacks, input, IME off; a screen check of a page
   and a click on a link; FlaUI reading the page's accessibility tree.
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
validation against gala's machine root store (`System32\gala\roots`).
Not yet run on real Windows (run-all.ps1 runs it when Nginx can be
started and, as administrator, imports the test CA first).

**Next**:
1. winhttp over Schannel: WinHttpOpen/Connect/OpenRequest/SendRequest/
   ReceiveResponse/ReadData for http and https (the JDK's and .NET's
   HttpClient use it), tested against the same Nginx.
2. wininet's HTTP functions on the same code (InternetOpen/Connect,
   HttpOpenRequest/SendRequest, InternetReadFile).
3. curl for Windows (its Schannel backend), unmodified, fetching from
   the in-guest Nginx; then Git's https remotes (MinGit uses curl with
   Schannel).
4. `gala install`'s HTTPS downloads (PK) on winhttp, with gala's root
   store holding a pinned, documented public CA bundle.
5. For BR: crypt32's stores and chain engine
   (CertVerifyCertificateChainPolicy; the ROOT store from the same
   directory), Schannel then validating through them.
6. Also open: session resumption, client certificates, ECDSA server
   keys, revocation, keys persisted across processes.

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
2022, against the exports of `user/bin/test*.dll` (oleaut32's ordinals
from Wine's oleaut32.spec) and the `gala_stubs` stubs. Two kinds:
missing exports stop a program from loading; stubs stop it when called.

**Done so far** (groups 1-11; no Windows run yet, each test still needs
a run-all.ps1 check there): msvcrt's wide classes, `_wtol`, `difftime`
(`msvcrt_test`); oleaut32's variant time and string conversions,
`VarCmp`, `GetActiveObject` (`oleaut32_test`); all of shlwapi's surveyed
path, string and URL functions, AssocQueryStringW,
SHCreateStreamOnFileEx (`shlwapi_test`; no shlwapi stubs left);
shell32's ShellExecuteExW, drag-and-drop, SHFileOperationW,
SHChangeNotify, icon extraction (`shell32_test`); ole32's PROPVARIANT
and STGMEDIUM clean-up, OleRun/OleLockRunning/OleSetContainedObject,
proxy blankets, CoCreateInstanceEx, CreateBindCtx (`ole32_test`);
dbghelp's ImageNtHeader, SymSrvGetFileIndexInfoW and symbols over
exports, a new normaliz.dll (`dbghelp_test`); crypt32's memory
functions, CertGetNameStringA, PFXIsPFXBlob, certificate stores,
CryptQueryObject, CryptDecodeObjectEx for alternative names
(`cert_test`); comctl32's image-list drag functions (`comctl32_test`).
FileZilla Server's GUI and StackBuilder now find every msvcrt, oleaut32
and ole32 import; version and msvcrt have nothing left the surveyed
programs import.

**Next** (what is left, by DLL):
1. ole32: GetHGlobalFromStream and CreateStreamOnHGlobal with an
   HGLOBAL (they need movable global memory: GlobalSize is a stub and
   GlobalAlloc hands out fixed blocks), CoRegisterMallocSpy, the OLE
   clipboard and DoDragDrop (need user32's clipboard).
2. crypt32: PFXImportCertStore (PKCS #12), the other decode types,
   CRLs/CTLs, collections' member stores (CertAddStoreToCollection),
   CryptAcquireCertificatePrivateKey; the chain engine is TLS step 5.
3. shell32: SHBrowseForFolderW (a dialog).
4. comctl32: CreateUpDownControl (a new control class); drawing the drag
   image.
5. dbghelp: MiniDumpWriteDump (a stub), PDB symbols, other processes'
   modules.
6. New DLLs: winspool.drv (StackBuilder's wxmsw326u_core), the JDK's
   optional dsound, winscard, msi, dbgeng (loaded only by jsound,
   j2pcsc, jpackage, saproc).
7. Elsewhere, for the agents working there: FileZilla Server's GUI also
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
  set_tid_address, the get*id calls, and rt_sigprocmask and ioctl as
  stubs; ENOSYS for the rest, logged by name and in the ledger
  (`linux syscall missing`). Scenarios `lx_hello`, `lx_write_exit`,
  `lx_brk`, `lx_mmap`, `lx_arch_prctl`, `lx_auxv`, each also `_pie`,
  match real Linux's transcripts exactly.
- Milestones 2-3 test corpus: fs_test, pipe_test, fork_test,
  thread_test, signal_test, clock_test, poll_test, pty_test, proc_test
  recorded on Linux 6.18 (`cargo xtask lx-expect`, `--runs N`), and the
  `kernel/lx_abi` crate (signal, termios, stat/statx/dirent64/epoll/poll
  layouts) host-tested against them.

**Next**:
1. Milestone 2, static busybox: files and directories, wait4, execve,
   clone, futex (priorities from `strace` on real Linux and the
   ledger); signals (SIGSEGV, SIGPIPE; frames from `lx_abi::signal`);
   wait queues for pipes; `lx::state` onto the core process. Each corpus
   program becomes a boot scenario as its calls land.
2. Milestone 3: threads and futex, clocks and polling, ptys over the
   core terminal object, /proc.
3. Seam 1: the pty as a core terminal object under ConPTY.
4. Seam 4: file-system metadata: per-directory case sensitivity
   (FileCaseSensitiveInformation), POSIX metadata per file, a plan for
   symlinks, hard links, POSIX delete and rename.
5. Seam 5: shared core objects when pipes, sockets or files are
   reworked.
6. Also open (ARCHITECTURE "The lx module"): address randomization, a
   vDSO, Linux in gala-um, the system-call numbers 462..=469; `lx_abi`
   becomes a kernel dependency with the signal, pty and file calls.

**Done when** (each milestone): every corpus program of the milestone
runs as a boot scenario whose transcript equals its `expected.txt`.

**Design**: ARCHITECTURE "Plan: a second personality (Linux)", "The
seams", "The Linux layer's milestones", "The lx module", "The lx_abi
crate"; WORKFLOW "Linux test programs".

### PK. Installing software: `gala install` over winget manifests

**Goal**: `gala install` reads winget's community manifests
(winget-pkgs: identifiers, versions, installer types and URLs, SHA-256,
silent switches, nested installer files) and installs as winget would,
from the command line first (CLI parity; the server profile too).

**Done so far**:
- Stages 1-2 except downloads (`pk_install_test`, scenario
  `pk_install_zip`; host tests `cargo test -p galapkg`, with a failure
  injected at every step): `gala install|search|list|upgrade|uninstall`
  over a winget-pkgs tree (`--source <dir>` with an `installers\`
  mirror), manifests 1.x singleton and multi-file, SHA-256 checked, zip
  (deflate) and portable installs to `WinGet\Packages`, commands as
  shims (gala.exe + `.shim`) in `WinGet\Links` on the user's PATH,
  winget's ARP entry, transactions with rollback (upgrade included), an
  install log and ledger records; CreateProcess searches PATH.
- The winget resolver (`galapkg::winget`, host-tested on 400 real
  winget-pkgs version directories): schemas 1.0.0-1.28.0, installer
  selection as winget does it with every rejection's reason, switches,
  dependencies into an ordered plan, ARP matching, version ordering.
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
  installs with /D= under gala. Inno Setup is host-tested only.
- Stage 4 groundwork: the MSIX/AppX reader (packages and bundles,
  AppxManifest.xml, block-map hashes per 64 KiB block, package full and
  family names); AppxSignature.p7x presence only.
- Stage 5 groundwork: compound files, the MSI database, cabinets (none,
  MSZIP, LZX), conditions, Formatted strings and the install plan
  (`msi::plan`).

**Next**:
1. Switch `gala install`/`list`/`upgrade` to `galapkg::winget`, then
   delete `manifest.rs`/`yaml.rs`; a boot scenario with a dependency and
   an upgrade that keeps scope.
2. The HTTPS fetcher (on winhttp and Schannel, TLS step 4): the GitHub
   raw and contents URLs, winget's source index for search; real
   symbolic links for commands; the logon environment from
   `HKCU\Environment`.
3. Stage 3's rest: a real Inno Setup installer in a scenario (they are
   32-bit, so WOW64 first; the compiler runs only on Windows, so a
   stand-in checks the switches and the "no uninstall entry" failure
   meanwhile); Scope switches and elevation for machine-wide
   installers; the manifest's success codes; a tracking catalog of
   gala's own instead of tagging vendors' ARP keys; the needs of other
   vendors' installers (IShellLink shortcuts, file associations, UAC
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
- The completion port and APC queues still grow when a completion is
  reported (`notify::complete`: a failed push loses the packet).
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
"Commit charge"). Next: kernel pool and handle quotas
(STATUS_QUOTA_EXCEEDED), so the program using too much fails instead of
everyone starving. GPU memory is charged here too (item GPU).

#### R7. Live mocking

Done: clock control (`clock=`, GalaClockControl), record/replay of the
boot time, clock readings and random numbers (`record`, `replay=`,
`um --record/--replay`, GalaRandom), disk latency
(`failpoint=disk_io:delay:<ms>`) (ARCHITECTURE "Clock control",
"Record/replay"; WORKFLOW "Clock control, record/replay, slow devices").
Next:
1. Record/replay of network frames, disk reads and input events (needs
   a deterministic guest: gala-um's simulation first).
2. The display and input as swappable devices; device events (NIC
   removed, resolution change).
3. API-level faults in gala's DLLs (Application Verifier style).
4. The read-write control channel (OBSERVABILITY "Planned facilities",
   the live introspection channel).
5. A frozen clock's effect on absolute timers (Windows re-evaluates them
   on a system time change).

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
recipe is untested).

**Next**:
1. OT2: trace context in the kernel; system calls, waits, faults, I/O
   and network as spans; per-CPU rings. It must fill OT1's gaps: a
   thread on `fault`, `image`, `console` and `net` events; an entry
   event for every system call; a parent pid and trace context on
   `process start`; times on serial lines; the boot's wall-clock start.
2. OT3: propagation through ntdll/kernel32, process creation, pipes,
   ALPC/RPC, IOCP.
3. OT4: the ETW bridge (EventRegister/EventWrite, TraceLogging) turning
   what Windows programs emit into OTel logs and spans; later the same
   for LX.
4. OT5: the in-guest OTLP exporter for servers, controlled from the
   `gala` CLI (on/off, sampling, endpoint).

**Design**: OBSERVABILITY "OpenTelemetry export"; WORKFLOW
"OpenTelemetry".

## Known issues and stopgaps

Each entry: what, why it matters, the structural fix, the owning item.

### Stopgaps (to replace)

None open.

### Defects known, not yet fixed

- **Hygiene sweep leftovers**: four lifecycle defects in IOCP, APCs and
  suspension. Item HS.
- **Fault-injection leftovers**: a failed push in `notify::complete`
  loses a completion packet; out-of-memory reported as
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
- **WOW64**: 32-bit programs (a 32-bit ntdll and thunks); the official
  Nginx build and many installers are PE32.
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
  - Resources: pool and handle quotas (R6), job limit messages,
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

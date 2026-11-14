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
its commit. gala is one kernel for two workloads: servers and console
programs (`profile=server`, or no screen: never a graphical interface)
and the desktop with **shasha** (Arabic شاشة, "screen"), the graphical
shell, where everything shasha does can also be done with the `gala`
command (item SD). See `PROJECT_BRIEF.md` for goals, values and
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

**The next big thing (the owner): Microsoft Office, item OF, with
everything it depends on.** It goes before the list below (work in
flight finishes: LD's first GTK program). Its chain, in order:
OF step 1's gap analysis (what the apps import and register, ranked
against gala); GPU G2's rest (DXVK: Direct3D 11 in software over
lavapipe, which the apps' rendering expects); COM/OLE depth and the
other blockers the analysis ranks; Click-to-Run's install and service
(PK); then BR's WebView2 and PK stage 4's MSIX for the new Outlook.

In order, after it:

0. **BR stage 2** first (the owner: Chromium working, then the rest of
   this list; heavier, meatier work before polish): its window, input
   and accessibility tree are done (`chromium_window`, `chromium_uia`),
   ChromeDriver too (`chromium_driver`); now the conformance report.
   Its blockers (Chrome's out-of-memory exceptions 0xE0000008,
   ChromeDriver not exiting, the full WPT run) go with item 12 to the
   Windows machine, where gala runs under KVM (the owner;
   `docs/windows-kvm-brief.md`).
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
   gaps; kernel32_test built for x86.
5. **R**: R7's next part (network frames, disk reads and input in
   record/replay), R6's rest (quota_test on Windows), R8's Secure
   Boot.
6. **PK**: winget's source index (step 2's rest); step 1's rest (an
   upgrade's dependencies, UpgradeBehavior, winget's settings).
7. **BR** stage 1's rest beyond the profile (its hotspots wait with
   item 12).
8. **AG**: the remaining stubs (filler).
9. **SSH**: the client against the host's sshd, agent forwarding;
   **H11**: busybox-w32's httpd and wget over loopback (the Linux
   busybox's run: `lx_httpd`).
10. **LN**: lints before every commit (the owner: quality built in
    before committing, not found later).
11. **PT**: the portability seams for a second architecture (the owner:
    the ARM64 port itself later): the kernel's done, user mode's next.
12. **On the Windows machine under KVM** (the owner): the performance
    profile and the optimization after it, in one chunk with BR's
    conformance blockers (item 0) and BR stage 1's hotspots: gala boots
    there under KVM in WSL2, real Windows next door
    (`docs/windows-kvm-brief.md`; its findings in
    GitHub issue #1 and its parts #2-#5, fixes on branch `windows-kvm`, merged
    and gated here). The same workloads (`bench`, the scenarios'
    programs with run-all's `ms` and `cargo xtask compare`, Chromium's
    page loads, Python) profiled on both, compared operation by
    operation, gala's hot spots fixed in order of the gap, each with a
    benchmark in `bench` and its numbers in PERF.md.

Then the next breakthroughs (the owner), in this order: **LD**, desktop
Linux applications (dynamic glibc programs first, then the Wayland
server); **GPU** G2, software rendering, which serves both personalities
(Mesa's llvmpipe and lavapipe are the same code behind Windows' DLLs and
Linux's libraries); **AR**, the ARM64 port; **XT**, x64 Windows programs
on ARM64; **AN**, Android applications.

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
| LN | Lints before every commit | in progress | `cargo xtask lint` from a committed pre-commit hook: architecture code, check-then-sleep, split atomic operations, hard-coded answers |
| PT | Portability seams for a second architecture | in progress | the kernel's seam done (`arch`'s interface, the per-architecture ABIs `nt::abi`/`lx::machine`, an aarch64 stub CI checks); user mode's next; the ARM64 port itself is in "Later" |
| HS | Hygiene sweeps | parked | first sweep and its four leftovers done; no further sweeps until gala runs live (the owner) |

### NT core services (done, by test)

| Area | Tests |
|---|---|
| Virtual memory: VAD, demand-zero paging, allocate/free/protect/query, guard pages and stack growth, copy-on-write | `vm_test`, `guard_test`, `cow_test`, `vmcycle_test`, `commit_test` |
| Objects, handles, names, symbolic links; pool and handle quotas | `object_test`, `symlink_test`, `quota_test` |
| Waits, events, mutants, semaphores, keyed events, timers | `wait_test`, `sync_test`, `keyed_event_test`, `timer_test`, `smpsem_test`, `smprace_test` |
| Threads, suspension, context, TEB/PEB/KUSER_SHARED_DATA, process startup | `thread_test`, `threadinfo_test`, `context_test`, `kuser_test`, `peb_test`, `startup_test` |
| Sections, image sections, memory-mapped files | `section_test`, `image_test`, `filemap_test` |
| Files, directories, async I/O, APCs, IOCP, file locks, change notification, reparse points and NT symbolic links | `file_test`, `dir_test`, `async_test`, `apc_test`, `ioapc_test`, `iocp_test`, `lock_test`, `dirchange_test`, `reparse_test` |
| User-mode exceptions, SEH and unwinding, C++ exceptions, x87/SSE | `exception_test`, `seh_test`, `cxx_eh_test`, `cxx_throw`, `fpu_test`, `safepoint_test` |
| Loader: imports, dynamic loading, static TLS, API sets | `import_test`, `loader_test`, `tls_test`, `missing_import_test` |
| System and process information | `sysinfo_test` |
| Pipes, child processes with redirected stdio | `pipe_test`, `process_test` |
| Registry, change notification, rename | `registry_test`, `regnotify_test`, `wow64_regnotify_test`; ARCHITECTURE "Registry" |
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
| WOW | 32-bit x86 programs (WOW64) | done (follow-ups listed) | stages 1-2 done: `wow64_test`, `wow64_thread_test`, `wow64_crash`, `win32_misc_test`'s WOW64 answers; stage 3's 32-bit ntdll, exceptions, APCs, redirection and environment: `wow64_import_test`, `wow64_seh_test`, `wow64_seh_crash`, `wow64_fs_test`; the 32-bit kernel32: `wow64_k32_test`, `wow64_c_test`; the 32-bit msvcrt: `wow64_crt_test`; the 32-bit advapi32 and the registry's 32-bit view: `wow64_adv_test`; the 32-bit crypt32 and ncrypt: `wow64_cert_test`; msi: `wow64_msi_test`; stage 4's 32-bit user32 and gdi32: `wow64_gui_test`; an x86 Inno Setup installer: `wow64_inno`, under `gala install`: `pk_install_inno32`; ARCHITECTURE "Plan: WOW64" |
| P1 | Unmodified prebuilt programs | in progress | Caddy, JDK, .NET, PowerShell, Elasticsearch, LocalDB, OpenSSH, PostgreSQL, MinGit, FileZilla Server, Python (pip, venv, asyncio, multiprocessing, numpy) done; Git Bash parked |
| SEC | Security model: tokens, descriptors, access checks, logons, integrity, UIPI | done | SECURITY "Status": phases S1-S6 (leftovers listed there) |
| UAC | User Account Control: the consent prompt | planned | an administrator account (the boot user or a second one) logs on with a filtered token linked to its full one (TokenLinkedToken, elevation types 2/3); `runas`, `requireAdministrator` manifests and COM elevation (`Elevation:Administrator!new:`) go to an AppInfo-like service that shows a consent prompt (or a credential prompt for a standard user) on a secure desktop the user's programs cannot reach (with SH step 3's trusted process) and creates the process with the full token; CreateProcess's ERROR_ELEVATION_REQUIRED; UAC virtualization of Program Files and HKLM for legacy programs; a scenario that drives the prompt through input and screen checks. Today an elevated caller's `runas` runs without a prompt and everyone else's is declined as a cancelled prompt (SECURITY "Elevation") |
| D1-D4 | Disk: virtio-blk, C: kept (`persist`), the registry kept, files' contents in the disk's data area (a disk-backed C:: free space is the disk's, files larger than memory), written back lazily as Windows' cache manager does (FlushFileBuffers, write-through and unbuffered handles at once; a dirty limit throttling writers) | done | `disk_test`, `persist_test`, `volume_test`, `sqllocaldb_persist`, `writeback_test`, `writeback_crash`, `writeback_lazy`; ARCHITECTURE "Disk", "A disk-backed C:" |
| TLS | Schannel and the HTTP stacks on it | in progress | Schannel, winhttp and wininet (synchronous, one engine, redirects), curl and Git's https remotes (curl's `--cacert` too), `gala install`'s downloads with a public CA bundle, crypt32's chain engine (Schannel's trust decision), ECDSA server keys, session resumption, client certificates, revocation from CRLs (offline) done; fetching CRLs next |
| AG | API gaps in the shell and runtime DLLs | in progress | 13 groups done; filler |
| WD | Windows divergences: the test programs on real Windows | in progress | triage in `user/windows/` (build 26200); the first fixes done; probes to come back |
| LX | Linux personality | in progress | seams 2, 3, 6 and milestone 1 done, seam 4's Linux side (links, case-sensitive directories, modes kept); static busybox runs (milestone 2), TCP sockets, threads, futex, epoll; file mappings, glibc's dynamic programs (LD step 1); seam 5's first core object, AF_UNIX sockets, and memfd (LD step 2) |
| CT | Linux containers: Docker Desktop and Podman over a WSL facade | planned | spec recordings first; needs LX beyond milestone 5 |
| LD | Desktop Linux applications | in progress | step 1: dynamically linked glibc programs run through ld.so (`lx_dyn`); step 2: memfd and AF_UNIX sockets with SCM_RIGHTS (`lx_memfd`, `lx_unix`), the Wayland wire format (`wl_abi`) and the first in-kernel Wayland server (wl_compositor, wl_shm, xdg_wm_base, wl_output; toplevels as desktop windows: `lx_wl`), wl_seat input and window control (`lx_wl_input`), weston-simple-shm (`lx_simple_shm`), popups (`lx_wl_popup`); step 3: the first GTK program, its window drawn and clicked (`lx_gtk`, GTK 3 on a Linux root of Ubuntu's packages); step 5: X11 programs through rootless Xwayland, gala its window manager, xterm on a pty (`lx_xterm`), GTK under X11 (`lx_gtk_x11`); step 6's first part: a distribution's desktop in one window (`lx_xdesktop`); step 7: Linux distributions in the start menu (`linux_menu`) |
| AR | ARM64 port | planned | the kernel's aarch64 `arch` on QEMU `virt`, then ARM64 Windows and Linux user mode |
| XT | 64-bit Windows programs on ARM64 | planned | dynamic binary translation of x64 code; after AR |
| AN | Android applications | planned | binder, ART, SwiftShader graphics; x86_64 images first |

### Desktop, sessions and remote access

| ID | Title | State | Note |
|---|---|---|---|
| W1 | Windows on the screen | done | `gui_test`; ARCHITECTURE "Windows on the screen" |
| W2 | Styled desktop and input | done | `gui_test` |
| W3 | FileZilla | in progress | 3.71.0: connects to FileZilla Server (FTP over TLS) and to sshd (SFTP), driven by pixels and by FlaUI; leftovers |
| W4 | The Windows look | in progress | steps 1-2 done; step 3 (themes) next |
| SH | The shell in user mode | planned | taskbar, start menu and wallpaper out of the kernel; the compositor later (RS B3a) |
| SM | Start menu: default apps and install-on-click | steps 1-2 done; step 3 next (the owner) | see "SM. The start menu's apps" below |
| AI | An AI-enabled start menu | planned | aichat through `gala install`; needs crypt32's ROOT store (TLS) and SH |
| UIA | UI Automation and windows across processes | in progress | `uia_test`, `flaui_test`, `windows_across_test`, `filezilla_flaui`; ARCHITECTURE "UI Automation" |
| SD | Server and desktop workloads kept apart | in progress | one kernel: `profile=server`, CLI parity, bare desktops done |
| SSH | SSH as a remote shell | in progress | steps 1-5 in their first form |
| CP | An interactive console prompt | planned | serial line, screen and keyboard, gala-um's terminal; no SSH needed |
| RS | Remote access and sessions | in progress | A, B1, B2, B3a's window stations, presenters, console switch and window state apart from the compositor (bare desktops too) done; B3b next |
| GPU | Graphics and GPU | in progress | G0 and G1 done; G2 in progress (OpenGL and Vulkan done); G3 parked |
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
| CS | Crash-safe: no hard resets, a kind crash screen | planned | the kernel restarts itself through R9's handover; the last-resort path saves and explains |
| TM | A task manager for Windows and Linux programs alike | in progress | steps 1-3 done (`taskmgr`: one list of both personalities, gala's taskmgr.exe ending one of each, "Not responding"); inspection next, then statistics (p50/p95/p99, the busiest threads, where they wait) and a Copy button for pasting a view into an agent, and sparklines beside every metric |
| EX | A file explorer over every file system | planned | Windows drives, WSL distributions and LX's root in one tree |
| PK | `gala install` over winget manifests | in progress | zip/portable installs per user and machine-wide, NSIS installs and a real Inno Setup install (Inno Setup 7.1.1: installed with its Start menu shortcuts and `.iss` association, run, uninstalled), on the winget resolver (selection, dependencies, ARP matching); MSI and MSIX readers |
| SN | Snapshots: whole machines, then single programs, from the command line | in progress | machine snapshots in xtask done (step 1); hot swapping gala's DLLs into a resumed snapshot done (step 1d: at your own risk, never a real run); the kernel too, alone or with DLLs in one plan (step 1e, the same rule); modules no process maps replaced whole and new ones added, every changed module taken by the flag alone (step 1f); next a failing CI run's snapshot as an artifact; program snapshots on R9's state format; `gala snapshot`, `restore`, `export`, `import` |
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

### LN. Lints before every commit

**Goal** (the owner): the classes of defect we have had are caught
before a commit exists, by custom analyzers that prove a property over
the source (parsed with `syn` in xtask), not by tests that hope to hit
its symptom hours later; the scenarios then verify, they are not the
only defence. Each analyzer has fixtures of its own (code it must flag
and code it must not), which verify the analyzer. One entry point, `cargo xtask lint`, run by
a committed git pre-commit hook (`core.hooksPath` to a tracked `hooks/`
directory, set by xtask on first use), so every commit (a session's or
a sub-agent's) passes through it; also what `cargo xtask test` runs
first.

**Next** (each lint with a test of its own: a fixture that must be
flagged and one that must not):
1. Done: `cargo xtask lint` (also first in `cargo xtask test`) and the
   committed pre-commit hook, which any xtask run enables; the
   analyzers and `cargo fmt --check`; `cargo xtask lint --full` adds
   CI's Clippy commands, read from ci.yml's Clippy step (one list), and
   the committed pre-push hook runs it.
2. Architecture code only in `kernel/src/arch`: done, the kernel's
   clippy for `aarch64-unknown-none` (item PT; in the pre-push run).
3. No check-then-sleep: `wait-site` (done) allows the kernel's raw
   `prepare_wait` only at the reviewed sites (each registers and tests
   its condition under the lock its waker takes); a new one needs a
   review. Next: those sites through one primitive. A debug-build watchdog alongside: a
   pending request (an AFD poll or select, a pipe read or listen, a
   keyed-event wait) whose condition already holds is reported as a
   lost wake-up.
4. No documented-atomic operation split across calls: `event-reset`
   (done) flags an event reset in gala's DLLs outside the reset APIs
   themselves (WSAEnumNetworkEvents reset its event after the kernel's
   copy and lost connects); a site that needs one says why.
5. No hard-coded answers: `stub_audit` flags functions and match arms
   that return a constant without reading anything (`SO_ERROR => 0`
   hid refused connects), unless a comment names the documented
   constant.

### PT. Portability seams for a second architecture

**Goal**: the kernel and gala's DLLs keep every architecture-specific
part behind the `arch` layer, so the ARM64 port (item AR) starts from a
clean seam and new code cannot add x86 coupling unnoticed. Not the port
itself (the owner: later).

**Done so far** (design: ARCHITECTURE "The architecture seam"; the rule
in WORKFLOW "Rules for every change"):
- The audit: the x86 assumptions outside `kernel/src/arch`, by seam (the
  register model, the clock and timer, boot, x86 only by nature, user
  mode).
- The kernel (steps 2 and 3): everything architecture-specific is behind
  `arch`'s interface, x86_64 its one implementation and aarch64 a stub
  that compiles; CI's Clippy step checks the kernel for
  `aarch64-unknown-none` (with and without `shasha`), so an x86
  assumption outside `arch` fails the build. The register frames
  (`UserRegs`, `SyscallFrame`, `TrapFrame`) keep their fields private to
  `arch` behind neutral accessors; the clock reads `cpu::cycles` and
  drives `arch::timer`; the boot steps, phase names, per-CPU snapshot and
  NMIs are `arch`'s; PCI and virtio are shared over `pci_config` and
  `port`. Windows' x64 ABI is `nt::abi::x64` (CONTEXT, argument
  registers, thread starts, exception and APC frames, the CONTEXT
  services, KUSER_SHARED_DATA's processor fields, CPUID's registry
  values, the crash report's registers); Linux's x86-64 ABI is
  `lx::machine::x86_64` (arguments, restart, start registers, auxv, the
  signal frame) with the system call table `lx/syscalls.rs`; their
  aarch64 counterparts are stubs. gala-um is x86_64-only
  (`compile_error!` elsewhere).

**Next**:
1. User mode: gala's DLLs compiled for `aarch64-pc-windows-msvc` where
   they have no architecture-specific code, as the same guard. Their x86
   parts: ntdll's (`user/testntdll`) and ntrt's x64/x86 modules (system
   call stubs, context, unwinding), kernel32's `x64.rs` and `x86.rs` (a
   new thread's context).
2. The kernel's remaining x86-shaped parts, each named here with why it
   stays for now:
   - `mm::paging` is x86-64's page-table format (entry bits, PAT), kept
     outside `arch` so the mapper is host-tested; ARM64's (VMSAv8
     descriptors, MAIR) moves it behind `arch` with the port.
   - The user thread-pointer bases keep x86-64's names in `sched`, the
     platform traits and `percpu` (`gs_base`, `fs_base`, `teb32`), as
     does the floating-point control register (`mxcsr`: NtCreateThread's
     start state, `set_mxcsr`); renamed when a second implementation
     gives them a meaning (TPIDR_EL0/x18, FPCR).
   - `boot` reads Limine's x86-64 protocol (the MP response's local APIC
     ids, `boot::Processors::lapic_id`) and the ACPI tables the x86-64
     calibration uses; aarch64's Limine responses come with the port.
   - WOW64's NT part (`nt/wow64.rs`: 32-bit Windows' layouts) compiles
     everywhere and runs only where `set_teb32` can enter 32-bit code
     (x86-64): no `cfg` needed.
   - Field and variable names in the handover's state format and logs
     (`tsc_hz`, `rip` in events) stay: they are formats readers depend
     on.

**Done when**: the aarch64 builds pass in CI and the audit's list is
empty or each remaining item is named in it with why it stays.

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
- Python (the owner's interest; done), CPython 3.13.1 for x64, each
  scenario's checks as on Windows: the embeddable package (PREBUILT
  `python`): `python_hello`, `python_script` (threads, sockets, select,
  subprocess, hashlib, zlib, json, sqlite3, ctypes), `python_async`
  (asyncio's proactor loop: TCP, UDP, subprocess pipes, the executor),
  `python_mp` (multiprocessing: Process, Queue, Pool, shared Value and
  Lock, Pipe, shared_memory); the full layout (NuGet's `python`
  package, PREBUILT `python-full`): `python_pip` (pip installs a wheel,
  its console-script launcher, `pip list`, venvs with and without pip),
  `python_pip_https` (pip from a package index over https through the
  Windows root store and chain engine; an untrusted index refused),
  `python_numpy` (numpy 2.2.1's wheel: C extensions, its bundled
  OpenBLAS, LAPACK, FFT). The Windows behaviour they needed is in the
  tests named in the commits (`kernel32_py_test`, `winsock_test`,
  `pipe_test`, `loader_test`, `kernel32_file_test`, `cert_test`, ...)
  and ARCHITECTURE.
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
6. Python's leftovers: the interactive prompt (console input, as
   PowerShell's in item 1); WSAID_TRANSMITPACKETS (asked for by asyncio's
   _overlapped, unused so far). Its ledger (optional probes):
   propsys.dll (from shell32) and
   api-ms-win-appmodel-runtime-l1-1-2 (absent), NtAllocateVirtualMemoryEx,
   GetNumaNodeProcessorMaskEx, GetFileInformationByName,
   GetFileInformationByHandleEx class 18. Not yet seen loading: ws2_32's
   ordinal 53; advapi32's RegSaveKeyW, RegLoadKeyW and
   RegConnectRegistryW.

**Done when**: each program's scenario passes; pending ones are moved
into `cargo xtask test` when CI can afford them.

**Design**: ARCHITECTURE "NT personality", "Registry", "Processes started
by programs"; SECURITY.md for OpenSSH and PostgreSQL.

### W3. FileZilla

**Goal**: the FileZilla 3.71.0 client (wxWidgets) runs as on Windows.

**Done so far**:
- The main window with toolbar, Quickconnect bar, local and remote
  panes, queue, status bar, menus, combo box drop-downs, tooltips, rich
  edit message log, snap; it connects to FileZilla Server in the same
  boot over TLS 1.3 and lists the remote directory (`filezilla`, a
  pixel screen check, and `filezilla_flaui`, driven through UI
  Automation; both pending: slow).
- SFTP through FileZilla's own SSH libraries (libfzssh,
  libfzssh-client, libfzssh-crypt, inside filezilla.exe; there is no
  separate `fzsftp.exe` engine to run headless any more):
  `filezilla_sftp` (pending: slow) is a real session: Win32-OpenSSH's
  sshd runs as a service (`start_as=system`, host keys from
  `ssh-keygen -A` first with `start_wait=`), and the same FlaUI test
  with `sftp` Quickconnects as galatest with its password, accepts the
  unknown host key and lists C:\ftp through sftp-server.exe.
- Rich edit controls: msftedit.dll and riched20.dll (`richedit_test`).
- msvcrt's `_ismbblead` (3.71's mingw WinMain entry; `msvcrt_test`).

**Next**:
1. DefWindowProc's WM_WINDOWPOSCHANGING sends WM_GETMINMAXINFO and keeps
   a thick-framed window within its track sizes (FileZilla's minimum is
   700 x 500, so its snapped half would be 700 wide as on Windows; the
   desktop's docking test and the scenario's checks change with it);
   SetWindowPos's WM_NCCALCSIZE.
2. Rich edit: RTF streams, undo, cut, copy and paste over user32's
   clipboard, OLE callbacks (wx asks for them).
3. A transfer over SFTP (`filezilla_sftp` lists only): a download and
   an upload through the queue.

**Done when**: `filezilla`, `filezilla_flaui` and `filezilla_sftp` pass
from a fresh checkout.

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

### SM. The start menu's apps

**Goal** (the owner): the start menu offers what a fresh Windows does
(the Command Prompt and gala's other default programs) and, beside them,
apps that are not installed yet: a click installs them through
`gala install` (winget manifests, item PK) and the tile then starts the
installed program. The first: Microsoft Office (`Microsoft.Office`, the
Office Deployment Tool: item OF), FileZilla (`TimKosse.FileZilla.Client`),
.NET (`Microsoft.DotNet.SDK.9`), Python (`Python.Python.3.13`), Chromium
(`Hibbiki.Chromium`), Git (`Git.Git`), 7-Zip (`7zip.7zip`).

**Done**: steps 1 and 2. The start menu lists gala's Command Prompt
(System32's cmd.exe, else C:\gala's) and the catalog: each entry a name,
a winget id and where its installer puts the program (checked against
the winget-pkgs manifests: user scope under %LOCALAPPDATA% first, then
the machine's). Not installed, a tile is dimmed with a download badge
and a click runs `gala install <id>` in a console window; installed, it
starts the program. A console program started from the menu gets its own
console window. Tests: the desktop's and `nt::action`'s host tests,
scenario `start_menu` (start_menu_test and a screen check: the tiles,
the Command Prompt clicked, 7-Zip's install started and its window
closed). TimKosse.FileZilla.Client is not in winget-pkgs today: its tile
shows `gala install`'s "no package" in the window (the portable
C:\filezilla placement of the FileZilla scenarios still counts as
installed).

**Next** (each test first: the desktop's host tests, a screen scenario,
a screenshot):
1. Step 3: the Windows way of listing installed apps: the start menu
   reads the shortcuts under the Start Menu\Programs folders (all users
   and the user's, `.lnk` per [MS-SHLLINK]) that installers create, so
   anything installed appears without a catalog entry; the catalog then
   only names what can be installed. With SH, all of this moves to
   `shasha.exe`.
2. gala's cmd.exe has no interactive prompt yet (only `/c`): the
   Command Prompt's window says so and cmd ends (ledger
   `cmd.exe!interactive prompt`). The prompt (reading lines from the
   console, the internal commands it has) makes the tile useful.
3. A scenario that installs a catalog package on a click from a source
   the test serves (needs a way to give the boot user's environment
   `%GALA_WINGET_SOURCE%`; the kernel's initial environment is fixed).
   Installs that fail on gala (Office until OF is done, an installer
   needing elevation without UAC's prompt) fail in their window with
   `gala install`'s own message.

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
restarted; `profile=server` starts no shell.

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
   Burn exe, PK stage 3); the UCRT is gala's own ucrtbase.dll
   (ARCHITECTURE "The UCRT").
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
  each session's keeps its windows shown by none, `console=` starts no
  conhost.exe, `gala`'s window commands are not supported.
- **CLI parity** (scenario `gala_cli`): one kernel entry point,
  GalaDesktopAction (0x7f58: Start, Windows, Programs, Activate,
  Minimize, Maximize, Restore, Close, Snap, Move, Wallpaper), which
  shasha's taskbar, caption buttons, Win+arrows and start menu call and
  the `gala` command (user/gala, in every image) calls too; `gala
  ps`/`gala kill`.
- UIPI on cross-process messages and on GalaDesktopAction's window
  actions; `gala start` runs the program under a copy of the caller's
  token (`uipi_test`, SECURITY S6).
- One kernel for both: shasha is always built in (the owner: a
  separate server build was not worth its 2.5 MB). Where nothing can
  show a desktop (no screen and no `presenter=`: BIOS boots, gala-um;
  `profile=server`) the desktops are bare, the window state alone shown
  by none, with no shell (window actions STATUS_NOT_SUPPORTED) and no
  composition work (scenarios `noscreen_windows`,
  `server_profile_windows`; user32_test's line on BIOS and in gala-um).
  A server image without user32/gdi32 is not planned: many console and
  server programs import them, and loading them starts nothing; only
  wallpapers, fonts and GUI-only programs (conhost.exe, FileZilla) stay
  out of a server image.

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
7. "Ignore Ctrl+C" (SetConsoleCtrlHandler(NULL, TRUE)) is inherited by
   child processes on Windows (#33); gala's kernel32 keeps it per
   process.

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
- B3a, the window state apart from the compositor (`nt::winstate`;
  shasha's `Desktop` draws it): wherever nothing can show a desktop
  (bare desktops: no screen and no `presenter=`, `profile=server`,
  gala-um) each session's windows, their last pixels, Z order, focus,
  capture and input are kept as on a desktop nobody shows (GalaSession's
  query reports them, frames and Test/Console presenters refused;
  presenter_test's `server` mode in `noscreen_windows` and
  `server_profile_windows`; host tests in `winstate/tests.rs`).

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
   desktops with windows of their own; UOI_USER_SID; atoms per station
   (the clipboard is the station's: LD step 3).
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
5. Later: several desktops per session, a secure desktop, the
   clipboard over the remote protocol, seamless windows, RDP proper. Interim, with no gala work: QEMU's
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
- G2, OpenGL: Mesa 26.2.5's llvmpipe (PREBUILT `mesa`, mesa-dist-win's
  MSVC build) as System32's `opengl32.dll`; gdi32's pixel formats and
  SwapBuffers go to it; user32 shows what DCs a program keeps drew and
  answers WindowFromDC; `gl_test` (a WGL context, a triangle read back
  and checked on the screen).
- G2, Vulkan: the Khronos loader (PREBUILT `vulkan-runtime`) and Mesa's
  lavapipe in System32, lavapipe registered under
  `HKLM\SOFTWARE\Khronos\Vulkan\Drivers`; gala's cfgmgr32 (no device
  tree: `cfgmgr32_test`); dxgi's factory exists with no adapters (the
  loader needs it; `gpu_stub_test`); ntdll resolves `.` and `..` in DLL
  paths (`loader_test`); `vk_test` (a triangle read back, then presented
  through VK_KHR_win32_surface and checked on the screen).

**Next**:
1. G1's rest: shasha's pointer on the hardware cursor,
   WM_DISPLAYCHANGE and refitting maximized windows on a resize (a test
   resizing the display through QEMU's monitor), several scanouts as
   monitors, device interrupts instead of spinning, the boot splash on
   virtio-gpu (the screen is black from the driver's start until the
   desktop draws).
2. G2's rest: DXVK (d3d9/10/11, dxgi) and vkd3d-proton (d3d12) over
   lavapipe, PREBUILT, beside the programs that use them (System32's
   dxgi stays gala's: the Vulkan loader calls it); `d3d11_test` with a
   screen check, then DXVK's and vkd3d-proton's own tests. SwiftShader
   stays the fallback should lavapipe fall short. WARP is ruled out (not
   redistributable). gdi32's GetGlyphOutline (Mesa's
   wglUseFontOutlines) is still GDI_ERROR. What DXVK 3.1.1
   (github.com/doitsujin/dxvk, `dxvk-3.1.1.tar.gz`, x64) needs, from its
   import tables: the UCRT (`api-ms-win-crt-*`: gala's ucrtbase) and gdi32's D3DKMT* functions (19 of them: failing with
   STATUS_NOT_SUPPORTED, in the ledger: DXVK uses them only for shared
   resources); everything else it imports gala has. d3d11_test needs
   DXBC shaders, and the host has no HLSL compiler (Ubuntu's
   vkd3d-compiler 1.2 predates vkd3d's HLSL): build a current
   vkd3d-shader compiler for `build.sh`, or assemble the two shaders
   with their checksum. Then vkd3d-proton 3.1 (d3d12, d3d12core)
   beside the program the same way.
   `d3dcompiler_47.dll` (the decision, for Office's Click-to-Run UI,
   which imports D3DCompile): gala's own DLL with a working
   D3DCreateBlob and no HLSL compiler: D3DCompile, D3DCompile2 and
   D3DCompileFromFile fail with E_NOTIMPL, no code and an error-message
   blob, recorded with the target profile (a stopgap, named so: compiled
   shaders need a Direct3D 11 device, which d3d11's G0 stub does not
   give, so a program cannot use them yet anyway; `office_api_test`
   expects Windows' S_OK on Windows). It is replaced by a real compiler
   with DXVK: vkd3d-shader's HLSL compiler (Wine's d3dcompiler is built
   on it) behind this DLL's exports, PREBUILT.
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
   advapi32's service functions beyond the SCM's queries (svc_test) and
   ImpersonateAnonymousToken, iphlpapi,
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
   (PENDING: a report run) runs WPT's own server on the host (pinned
   commit, `dom/nodes`) and each test page in headless Chromium through
   ChromeDriver, the report as wptreport.json, `cargo xtask
   wpt-compare` against Edge 159 on Windows on wpt.fyi. The first five
   tests through wptserve: all harness OK, 47 of 47 subtests, as Edge.
   The whole directory (406 tests) once: 52 ran (49 harness OK, 1673 of
   1698 subtests; the failures Blink's own, as Edge-159-win and
   Chrome-159-linux score, and 8 of createDocument's qualified-name
   checks where Chrome 8143 passes and our 8140 snapshot may predate the
   change); then three pages hung and the rest went unrun. Next, in
   order: (a) the page loads that stall again (connections to wptserve
   with 0 bytes, closed by Chromium after ~240 s, its connect timeout: a
   second lost readiness report besides the FD_CONNECT one fixed;
   a run with `events=net` to see the stalled socket's AFD history);
   ChromeDriver's session thread waits on its condition variable for
   the navigation's answer meanwhile (`kernel32_sync_test`'s
   lost-wake-up stress and the state dump's keyed-event lines found no
   lost wake in the locks); (b) Chrome's out-of-memory exceptions
   (0xE0000008) when a second browser starts with the first still alive,
   with free frames left: which limit (commit, reservations, a quota);
   (c) ChromeDriver not exiting after the client's /shutdown (the
   scenario then runs to its limit); (d) the whole directory again, then
   the comparison with Chrome on Windows (wpt.fyi). The report itself: a subset of web-platform-tests
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

**Priority**: the current focus (the owner), ahead of the rest of the
list; its dependencies (GPU G2's DXVK, COM/OLE, Click-to-Run's
install, WebView2, MSIX) are worked as the analysis ranks them.

**Gap analysis** (step 1; the `office-survey` workflow, WORKFLOW "The
Office survey"; Microsoft 365 Apps 16.0.20527.20150, Current channel,
x64: 1486 PE files, 867 of them x64, 529 DLLs of Office's own). What
Word, Excel and PowerPoint load (`import_survey.py --dumps ... --roots
WINWORD.EXE,EXCEL.EXE,POWERPNT.EXE`): 114 files at start, 85 more
through delay imports. Office carries its own UCRT and VC runtime
(app-local `ucrtbase.dll`, `vcruntime140.dll`), so the apps do not
need gala's.
- **To load** (static imports of the 114; no DLL is missing, only
  functions): kernel32's CreateFile2 (15 files), the activation-context
  queries FindActCtxSectionStringW and QueryActCtxW (11), CreateMutexExW,
  FindFirstFileNameW/FindNextFileNameW, GetShortPathNameA,
  GetThreadIOPendingFlag, GetVolumePathNamesForVolumeNameW,
  IsSystemResumeAutomatic, OpenSemaphoreA, QueueUserWorkItem,
  WerRegisterMemoryBlock/WerUnregisterMemoryBlock (5 each) and ten more
  used once; ntdll's AVL generic tables (RtlInitializeGenericTableAvl
  and its seven companions), NtNotifyChangeMultipleKeys, NtRenameKey,
  RtlIntegerToUnicodeString, RtlIsNameInExpression (5 each). Each must
  work, not only exist.
- **Then, as the apps draw and work** (delay imports, files needing
  each): gdiplus (13), Direct2D `d2d1` (7), `mscms` colour management
  (5), Media Foundation `mfplat`/`mf` (4/2), `propsys` (4), `cryptui`
  (4), `msi` (4), WIC `windowscodecs` (3), `rstrtmgr` (3), the
  WinAppSDK/XAML pieces' app-model and ntuser API sets, `d3d10_1` and
  `d3dcompiler_47` (2), `httpapi`, `srpapi`, `ninput`, `dnsapi`,
  `activeds`, `credui`, `davclnt`; the full list with every function is
  the survey's output. Direct2D and WIC lead the drawing path (Office
  draws through Direct2D/DirectWrite, falling back to GDI+), with GPU
  G2's Direct3D 11 under Direct2D.
  - WIC (`windowscodecs`, ARCHITECTURE "windowscodecs"; `windowscodecs_test`,
    x64): done for the factory (both Office imports,
    WICCreateImagingFactory_Proxy and WICConvertBitmapSource, and COM),
    bitmaps and locks, palettes, streams, the PNG and BMP decoders in
    their native formats, the format converter, the scaler and the
    clipper. Next: a JPEG decoder (Office's documents carry JPEG
    pictures), CreateBitmapFromHBITMAP/HICON, the ICO and GIF decoders,
    conversion to grey and indexed formats, the PNG metadata reader and
    the PNG encoder, each as Office's use shows it (the ledger names what
    it asks for). A stopgap to replace: the scaler's Cubic, Fant and
    HighQualityCubic modes interpolate as Linear (recorded each time);
    their filters, measured against Windows, come with the JPEG work.
    The `info` lines of `windowscodecs_test` (a PNG's resolution
    without pHYs, PBGRA -> BGRA rounding, nearest-neighbour sampling when
    halving, Linear's weights, a PNG's GetFrame(1)) wait for a Windows
    run; its first run (issue #91) settled WICCreateImagingFactory_Proxy(0)
    (WINCODEC_ERR_UNSUPPORTEDVERSION) and a BMP's GetFrame(1)
    (WINCODEC_ERR_FRAMEMISSING).
  - Direct2D (`d2d1`, ARCHITECTURE "Direct2D"; `d2d1_test`, x64, and
    the `d2d1_window` screen scenario): done for the factory (both
    kinds, ID2D1Multithread; the ordinals Office imports), the matrix
    helpers, software render targets over WIC bitmaps, device contexts
    (BindDC) and windows (hardware requests drawn in software), solid,
    linear, radial and bitmap brushes, bitmaps (from memory and WIC),
    rectangles, rounded rectangles, ellipses, lines and path geometries
    (lines, Béziers, arcs, fill modes) filled and stroked with widths,
    caps and joins, antialiased or aliased, transforms and axis-aligned
    clips, DrawBitmap. Next, as Office's use shows it (the ledger names
    each): text (DrawText/DrawTextLayout, which needs DirectWrite's text
    layout first), layers, compatible render targets, drawing state
    blocks, the GDI interop interface, dashes, ID2D1Factory1 and device
    contexts, then D3D11-backed targets with GPU G2. The `info` lines of
    `d2d1_test` (drawing outside BeginDraw, CreateBitmapFromWicBitmap
    of 32bppBGRA, antialiased edge values) and its expectations wait for
    a Windows run: the first (issue #94) stopped at D2D1CreateFactory,
    E_NOINTERFACE for an IID_ID2D1Factory with two bytes wrong in both
    the test and the DLL (now d2d1.h's value).
- **Install** (Click-to-Run): the `ClickToRunSvc` service (automatic,
  LocalSystem, `OfficeClickToRun.exe /service`), its integrator, and
  ten scheduled tasks; ODT's own `setup.exe` is 32-bit (below).

**Next**:
1. The load list above, test first (a kernel32/ntdll test program for
   the functions, expected results from Windows), until the three
   apps' processes start in gala with Office's files in place; a
   scenario that boots them, with the files gala's own install put
   there (the owner: no files from elsewhere; the native install below
   comes first).
   - The UCRT (the owner's decision: gala's own; also the answer to
     PK's AIChat UCRT question): gala's own `ucrtbase.dll` (the `api-ms-win-crt-*` sets
     resolve to it), x64 and x86, msvcrt's source with the UCRT's own
     functions (ARCHITECTURE "The UCRT"; tests `ucrt_test`,
     `wow64_ucrt_test`, expected results from Windows' ucrtbase). It
     covers every UCRT import of ODT's setup.exe and of curl, CPython,
     `cxx_eh_test`, `pk_install_inno` and NumPy's DLLs, whose scenarios
     use it and no longer copy the JDK's. Next: what the JDK,
     PowerShell, .NET, PostgreSQL and SQL Server LocalDB still import
     that gala's lacks (65 functions across their files: the C++
     runtime functions Windows' ucrtbase also exports,
     `_CxxThrowException`, `__CxxFrameHandler3`, `__std_exception_copy`,
     `__std_type_info_*`, `__RTDynamicCast`; `__stdio_common_vfwscanf`;
     `_Gettnames` (recorded, NULL); the `_s` path, date and case
     functions (`_splitpath_s`, `_strdate_s`, ...; the wide `_wcsupr_s`,
     `_wcslwr_s`, `_wsplitpath_s` and `_wmakepath_s` are done); the
     multibyte `_mbs*`; `qsort_s`, `div`, `ilogb`, `nan` and a few more;
     `tools/import_survey.py` on their directories lists them), then those
     scenarios drop the copied `ucrtbase.dll` and `api-ms-win-crt-*`
     files. `ucrt_test`/`wow64_ucrt_test` pass on CI's Windows
     (`user/windows/ci-passing.txt`).
   - The load list's kernel32 and ntdll functions, test first in
     `office_api_test`/`wow64_office_api_test` (x64 and x86): done:
     CreateFile2, FindFirstFileNameW/FindNextFileNameW, CreateMutexExW,
     OpenSemaphoreA/W, K32EnumProcesses, K32GetProcessImageFileNameW,
     RtlCaptureStackBackTrace (x86 too), WerRegisterMemoryBlock/
     WerUnregisterMemoryBlock, IsSystemResumeAutomatic,
     GetPhysicallyInstalledSystemMemory, QueryActCtxW (basic
     information), FindActCtxSectionStringW, QueueUserWorkItem,
     GetShortPathNameA, GetVolumePathNamesForVolumeNameW,
     AttachConsole/AllocConsole, GetDiskFreeSpaceA, the AVL generic
     tables, RtlIntegerToUnicodeString, RtlIsNameInExpression;
     registry change notification (NtNotifyChangeKey,
     NtNotifyChangeMultipleKeys, RegNotifyChangeKeyValue with
     REG_NOTIFY_THREAD_AGNOSTIC) and NtRenameKey/RegRenameKey with its
     change-log record (`regnotify_test`, `wow64_regnotify_test`,
     `persist_test`; ARCHITECTURE "Registry"); the completion statuses,
     a kept change, a deleted watched key and a subordinate in the
     master's hive are Windows' (issue #83); to measure before
     `ci-passing.txt` lists them (the tests' `info` lines): a kept
     change's IOSB, a subordinate in another hive and two, what a rename
     fires and the renaming handle's own name.
     Activation contexts from real manifests (`actctx_test`,
     `wow64_actctx_test`; ARCHITECTURE "Activation contexts"): the
     manifest read (embedded RT_MANIFEST and .manifest files, UTF-8 and
     UTF-16, refused as Windows refuses), dependencies found beside the
     application or the manifest (Microsoft's as system assemblies), the
     DLL redirection, window class, COM server and ProgID sections in
     Windows' layouts, FindActCtxSectionStringW/A and
     FindActCtxSectionGuid through the active then the process default
     context (the executable's own manifest), QueryActCtxW's classes 2-6
     and a DLL's own context (RT_MANIFEST 2); GetThreadIOPendingFlag is
     ThreadIsIoPending, the kernel's count of a thread's outstanding
     socket, pipe and directory-watch requests (`office_api_test`;
     ARCHITECTURE "I/O requests and their thread"). The process
     default's root manifest path (the executable's) and a waiting
     registry watch's GetThreadIOPendingFlag (0) are Windows' (issues
     #85, #83). To measure before `ci-passing.txt` lists `actctx_test`
     (the `info` lines): the handle RETURN_HACTCTX gives for the process
     default (25H2 printed "not NULL: 0", i.e. NULL, while its report
     read it as not NULL), the executable's
     QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE context, a dependency's
     directory name, ulFlags and ulFileCount; and for
     GetThreadIOPendingFlag a request on a file with a completion port
     (gala: not counted; `office_api_test`).
     The kernel32 functions the apps and OfficeC2RClient.exe use once
     (`import_survey.py ... --roots WINWORD.EXE,EXCEL.EXE,POWERPNT.EXE
     --static`), in `office_k32_test`/`wow64_office_k32_test`
     (ARCHITECTURE "kernel32"): CompareStringA (and punctuation before
     digits before letters, NULs without weight, in every linguistic
     comparison), FoldStringW (Unicode normalization tables from
     `tools/nls_tables.py`), GetStringTypeA/ExA, IsBadReadPtr,
     IsBadWritePtr, IsBadCodePtr, IsBadStringPtrA/W, Offer/Reclaim/
     DiscardVirtualMemory, ResolveLocaleName, SetThreadIdealProcessor(Ex)
     (the kernel's ThreadIdealProcessor and ThreadIdealProcessorEx),
     WinExec, GetNumberFormatW/A/Ex and GetCurrencyFormatW/A/Ex. To
     IsBadReadPtr using up a guard page, OfferVirtualMemory of an
     unaligned address (ERROR_INVALID_PARAMETER) and CompareStringA's
     ERROR_INVALID_FLAGS for SORT_DIGITSASNUMBERS are Windows' (issue
     #88). To measure before `ci-passing.txt` lists them (the tests'
     `info` lines): which default ResolveLocaleName(NULL) names (25H2's
     was not the system's), GetNumberFormatA's rounding of 9.995; and
     whether Windows' kernelbase exports OfferVirtualMemory and
     ReclaimVirtualMemory (`api-ms-win-core-memory-l1-1-2`; Wine's
     kernelbase.spec, which `exports.txt` follows, lacks them).
     Left: the sections gala reads but does not build (typelibs,
     interface proxy/stubs, CLR classes and surrogates, WinRT
     activatable classes, loadFrom; recorded), QueryActCtxSettingsW.
     `office_api_test` and `wow64_office_api_test` pass on CI's Windows
     (`ci-passing.txt`).
   - Alongside, natively: the pending scenario `office_install` runs
     the Office Deployment Tool's `setup.exe /configure` in gala
     (PREBUILT `odt`: 16.0.20430.20146, SHA-256 c883ab32...bb4e322,
     re-pinned when Microsoft replaces the file at its URL;
     `user/office_install/config.xml`: Word, Excel and PowerPoint, 64-bit,
     Current channel, Display None, AcceptEULA; a net scenario, the
     guest reaching the CDN through QEMU's NAT, so on CI's boot runners).
     `setup.exe` is a 32-bit x86 program (WOW64); every static import is
     there (the UCRT and kernel32 functions above). Done: the i686
     crypt32 and ncrypt (`wow64_cert_test`: cert_test's source for x86,
     the caller's structures typed and checked against wincrypt.h for
     both widths, ARCHITECTURE "The 32-bit system DLLs"). How far it
     gets: it starts, starts its copy (a second setup.exe), whose crypt32
     now loads and answers (its own embedded signature asked of
     CryptQueryObject, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED: the
     message and its store, below); it reads its Office and Click-to-Run registry keys (absent)
     and ends with 0xc06d007e at its next delay-loaded DLL, msi.dll,
     imported by ordinal (MsiCloseHandle #8, MsiEnumComponentQualifiersW
     #39, MsiEnumProductsW #45, MsiLocateComponentW #90,
     MsiQueryProductStateW #113, MsiSetInternalUI #141,
     MsiGetComponentPathW #173, MsiConfigureProductExW #190,
     MsiProvideQualifiedComponentExW #203). Done: gala's msi.dll, x64
     and x86, these nine with their ordinals, answered as Windows does
     for products and components it has not installed (`msi_test`,
     `wow64_msi_test`; ARCHITECTURE "Other DLLs"). Past msi it disables
     its input methods (imm32's ImmDisableIME: done, TRUE as Windows,
     `office_api_test`), queries its activation context (QueryActCtxW's
     module context and the manifest sections: done, `actctx_test`),
     asks COM for a class not registered, and loads normaliz and powrprof
     (done: both built for i686 too; `office_api_test` and
     `wow64_office_api_test` check IdnToAscii/IdnToUnicode and
     CallNtPowerInformation's processor information). It then sets up
     its download of its configuration from officeclient.microsoft.com
     over a synchronous winhttp session with a status callback (done:
     WinHttpSetStatusCallback and the synchronous notifications, per
     handle and inherited, in Windows' order; the real connect time-out,
     30 s here; WINHTTP_OPTION_CONFIGURE_PASSPORT_AUTH accepted as
     Windows does; `winhttp_cb_test`, `wow64_winhttp_cb_test`;
     ARCHITECTURE "WinHTTP and WinINet"). Its relaunched copy asks
     ShellExecuteEx for "runas" (done: elevation as SECURITY.md
     "Elevation" designs it; `runas_test`, `wow64_runas_test`,
     `runas_declined`): as the boot user, a standard user, gala declines
     as a cancelled UAC prompt (recorded) and setup.exe goes to report the
     error, so the scenario runs it as SYSTEM (`as=system`, an
     administrator, as Configuration Manager and Intune run ODT), elevated
     (the `, elevated token` start lines), which asks for no elevation.
     Its configuration it reads with MSXML 6 (done: gala's msxml6.dll,
     x64 and x86, DOMDocument60 registered in both registry views;
     `msxml_test`, `wow64_msxml_test`; ARCHITECTURE "msxml6"): it loads
     `C:\gala\config.xml` (load, async left on: loaded before load
     returns, recorded), then documentElement, selectSingleNode("Add"),
     selectNodes("Product", "Language", "ExcludeApp", "Remove",
     "RemoveMSI", "Display", "Logging", "Updates", "Property", "@ID"),
     the node lists' length and item, getAttribute (SourcePath, Version,
     Channel, OfficeClientEdition, Level, AcceptEULA, Path, Enabled, ...),
     attributes, nodeType and nodeValue: no DOM method it calls is
     missing. Before that, urlmon is asked for and missing
     (`urlmon.dll` missing dll, survived), two WinHttpSetOption options
     are recorded unsupported (77 WINHTTP_OPTION_AUTOLOGON_POLICY, 63
     WINHTTP_OPTION_DISABLE_FEATURE), no TCP connection is made yet
     (`events=net` shows none). After the configuration it asks kernel32
     for GetUserDefaultGeoName (done: "US", GetUserGeoID's GEO_ISO2;
     `office_api_test`, `wow64_office_api_test`) and loads wldp.dll to
     check for S mode (done: gala's wldp.dll, x64 and x86, unlocked, no
     policy in force; the same tests), looks for an Office 2013
     Click-to-Run (`C:\Program Files\Microsoft Office 15\ClientX64\...`,
     `HKLM\SOFTWARE\Microsoft\Office\15.0\ClickToRun\Configuration`:
     absent, as on a clean Windows) and throws a C++ exception over the
     absent key (the crash report decodes it: `shared::Exception`, a
     std::exception, its message "Cannot open Registry Key"), which its
     own catch takes. Done: that catch works (the 32-bit ntdll links a
     registration of its own around each handler call, as Windows; before,
     an earlier catch had linked an unwound frame back and the
     registration list looped on itself, so this throw found no handler:
     the crash report's SEH list showed it; ARCHITECTURE "WOW64",
     `wow64_seh_test`). The elevated copy (pid 12) then delay-loads
     advapi32's OpenSCManagerW (done: gala's service control manager,
     in the kernel over the registry's Services key, the client
     functions in advapi32 for both widths; `svc_test`,
     `wow64_svc_test`; ARCHITECTURE "The service control manager"):
     it connects (granted 0x2000b: connect, create service and lock, with
     READ_CONTROL) and opens `OfficeSvc` twice, ERROR_SERVICE_DOES_NOT_EXIST
     as on a machine without Office (`scm:` lines). It then asks WinRT
     for Windows.Management.Deployment.PackageManager through C++/WinRT,
     which loads `combase.dll` by name (done: gala's combase.dll, x64 and
     x86, Windows' export list as forwarders to ole32, which has the
     HSTRINGs in Windows' layout, RoGetActivationFactory from the
     `ActivatableClassId` registration, CoIncrementMTAUsage and error
     origination; PackageManager in gala's AppXDeploymentClient.dll,
     answering as a machine without MSIX packages; `combase_test`,
     `wow64_combase_test`; ARCHITECTURE "COM: ole32 and oleaut32"), and
     looks for a Store Office (`appxdeploymentclient.dll!FindPackagesByNamePublisher(Microsoft.O...`
     in the ledger: none). The elevated copy (pid 12) then starts its
     downloads over winhttp, on TLS from 32-bit (done: gala's
     schannel.dll for i686, its caller structures typed for both widths;
     `wow64_schannel_test`, schannel_test's source built for x86 against
     the same Nginx; ARCHITECTURE "Schannel (TLS)"). The scenario ships
     the public roots a full image has (`user/roots/public-roots.pem`).
     DNS and TCP connections to Microsoft's servers work over QEMU's
     NAT; in the agent sandbox the handshake then fails as it
     should: its egress gateway intercepts TLS with its own CA ("Egress
     Gateway SDS Issuing CA", `openssl s_client` on the host shows the
     same), which no root store trusts (`crypt32.dll!SSL:
     CERT_E_UNTRUSTEDROOT`, chain error 0x20, `schannel.dll!ISC:
     UnknownIssuer`, SEC_E_UNTRUSTED_ROOT), every run (an empty path
     is ERROR_PATH_NOT_FOUND, not the current directory, so setup.exe's
     cleanup of its unset folder "" no longer deletes C:\, SysWOW64's
     bcrypt.dll with it; `kernel32_file_test`, ARCHITECTURE "Files").
     With direct egress the chain is Microsoft's,
     to a public root (not yet seen in a run). Its error path asks urlmon's
     security manager (done: urlmon.dll for x64 and x86,
     CoInternetCreateSecurityManager with MapUrlToZone and GetSecurityId
     as Windows' default settings answer them; `office_api_test`,
     `wow64_office_api_test`, expectations from Wine's sec_mgr.c run on
     Windows; ARCHITECTURE "WOW64"): ProcessUrlAction(
     https://officeclient.microsoft.com..., URLACTION_CREDENTIALS_USE
     0x1a00), answered E_NOTIMPL and recorded (Windows answers from the
     Internet zone's policy under Internet Settings\Zones), and MapUrlToZone(
     https://ecs.office.com/config/v2/Office/...) = 3, Internet. It then
     asks wininet for its cookies (done: gala's wininet.dll for i686 too,
     with every wininet function setup.exe imports: InternetGetCookieExW,
     no cookies as on a fresh profile, recorded `wininet.dll!InternetGet
     CookieExW(no cookie store)`, asked twice, with
     INTERNET_COOKIE_HTTPONLY and without; InternetQueryOptionW,
     InternetSetStatusCallbackW with WinINet's notifications, and the A
     functions InternetConnectA, HttpOpenRequestA, HttpAddRequestHeadersA,
     HttpQueryInfoA, InternetCrackUrlA; `wininet_test` and its x86 twin
     `wow64_wininet_test`, expectations from Wine's wininet tests run on
     Windows, both of which pass on Windows with the test CA trusted
     (measured on 26200, issue #61; `ci-passing.txt`); ARCHITECTURE
     "WinHTTP and WinINet"), asks the 32-bit ntdll
     for RtlConvertDeviceFamilyInfoToString through GetProcAddress
     (missing export, survived), lists the network adapters (done:
     gala's iphlpapi.dll for i686 too, with GetAdaptersInfo for both
     widths from the kernel's interface records, which carry the
     gateway; `office_api_test`, `wow64_office_api_test`, on a card;
     ARCHITECTURE "Network interfaces"), asks WinRT for
     Windows.Networking.Connectivity.NetworkInformation and
     Windows.Security.Authentication.OnlineId.OnlineIdSystemAuthenticator
     (not registered, recorded; REGDB_E_CLASSNOTREG, survived) and opens
     an asynchronous wininet session (`wininet.dll!InternetOpenW(INTERNET_
     FLAG_ASYNC)`, ERROR_NOT_SUPPORTED, recorded, survived). It then uses
     shlwapi's path helpers (done: PathIsUNCW, PathGetArgsW,
     PathGetCharTypeW, PathGetDriveNumberW, StrRetToBufW, every shlwapi
     function it imports; `shlwapi_test` and its x86 twin
     `wow64_shlwapi_test`) and ole32's call cancellation (done:
     CoEnableCallCancellation, CoDisableCallCancellation, CoCancelCall;
     `combase_test`, `wow64_combase_test`), asks COM for BITS's
     BackgroundCopyManager ({4991D34B-80A1-4291-83B6-3328366B9097}, an
     out-of-process server: `ole32.dll!CoGetClassObject(out-of-process
     server)`, recorded, survived) and the zone policy of the CDN's URL
     (`urlmon.dll!ProcessUrlAction(http://officecdn.microsoft.com/...)`).
     It then checks signatures with WinVerifyTrust (done: gala's
     wintrust.dll, x64 and x86, Authenticode over PE images: the image
     hash, the PKCS #7 SignedData through crypt32's CryptMsg* decoding,
     the signer's signature, the RFC 3161 timestamp, the chain with the
     Authenticode policy, Microsoft's code signing roots in the root
     store, and the three WTHelper functions; CryptQueryObject's
     embedded PKCS #7, which setup.exe asks for its own signature, is
     answered too; `wintrust_test`, `wow64_wintrust_test`, expected
     results from Wine's softpub.c tests run on Windows; ARCHITECTURE
     "wintrust: Authenticode"; issue #57's measurements: a signer whose
     signature fails is TRUST_E_NOSIGNATURE, a dual-signed image falls
     back to its nested signature): its own setup.exe verifies (its
     WINTRUST_SIGNATURE_SETTINGS recorded), an earlier
     CryptMsgOpenToDecode asks for CMSG_DETACHED_FLAG (recorded, decoded
     as attached), and it downloads the version descriptor cab from
     officecdn.microsoft.com over http and verifies it (done: the
     cabinet subject in gala's wintrust, its digest the range every
     signed CDN cabinet's signature matches; `wintrust_test`'s cabinet
     cases, which are `cab_probe`'s answers on Windows (issue #59);
     ARCHITECTURE "wintrust:
     Authenticode"). Rejected, that cabinet ended the run: ODT's log
     (`C:\Users\Public\GALA-<date>.log`, dumped) said
     `MRO::LoadFromVCab ... InvalidSignature (HR:0x800B0003)`, "Cannot
     download and extract version descriptor", and only then, on its
     way out, came slc.dll and the SetSystemFileCacheSize and
     NtSetSystemInformation lookups (telemetry at exit, not causes).
     Accepted, setup.exe closes three all-zero WINTRUST_DATAs
     (dwStateAction CLOSE, cbStruct 0: 1 with the last error 1, as
     Windows answers, `cab_probe` on 26200, issue #59) and extracts
     the cabinet with setupapi's SetupIterateCabinetW (done: x64 and
     x86, over galapkg's cabinet reader, v64.cab being LZX:21;
     `setupcab_test`, `wow64_setupcab_test`, expected results from
     Wine's setupcab.c tests, both passing on Windows (issue #59; the
     32-bit one has an `asInvoker` manifest so that UAC's installer
     detection lets it start unelevated); ARCHITECTURE "Other DLLs").
     It loads the descriptor's XML with MSXML 6 and clones from it
     (done: IXMLDOMNode::cloneNode; `msxml_test`, `wow64_msxml_test`,
     Wine's test_cloneNode), downloads the product's cabinets and
     streams for about 70 s (DeliveryOptimization and BITS are asked
     first: REGDB_E_CLASSNOTREG, it falls back to HTTP), installs their
     catalog files (wintrust's CryptCATAdminAcquireContext is a stub:
     `wintrust.dll!CryptCATAdminAcquireContext` missing export; ODT logs
     `C2R::CatalogFiles::Install` UnexpectedError and goes on) and
     starts the 64-bit Click-to-Run client, `C:\Program Files\Common
     Files\Microsoft Shared\ClickToRun\OfficeClickToRun.exe platform=x64
     culture=en-us productstoadd=O365ProPlusRetail.16_en-us_x-none ...`
     (done: the scenario ships the whole 64-bit system,
     `FULL_SYSTEM_SCENARIOS`; the client's missing exports, kernel32's
     WerRegisterFile and ntdll's NtSetInformationKey, and its delay-loaded
     user32 SetCoalescableTimer, x64 and x86: `office_api_test`,
     `registry_test`, `user32_test`). The client registers its log for
     error reports, sets its registry keys' control flags
     (REG_KEY_DONT_VIRTUALIZE | REG_KEY_RECURSE_FLAG, recorded: gala has
     no registry virtualization) and installs a global hook
     (`user32.dll!SetWindowsHookExW(global)`, recorded).
     It then becomes an RPC server (done: gala's RPC runtime in rpcrt4,
     local RPC over ncalrpc and ncacn_np with the NDR engine's -Oicf
     interpreter, every rpcrt4 function the Click-to-Run client, C2R64.dll,
     ApiClient.dll and the App-V DLLs import; `rpc_test`, expectations
     from Wine's rpc.c and server.c run on Windows; ARCHITECTURE "RPC:
     rpcrt4"): RpcServerUseProtseqEpW and RpcServerRegisterIf3 with
     security descriptors (recorded: no access check), and it creates
     the ClickToRunSvc service (`scm:` lines: automatic start, its
     `OfficeClickToRun.exe /service` path).
     It sets the service's description and failure actions (done:
     ChangeServiceConfig2W/A and QueryServiceConfig2W/A, levels 1-7 as
     the service key's values; `svc_test`, `wow64_svc_test`, Wine's
     service.c test_queryconfig2 on Windows; ARCHITECTURE "The service
     control manager").
     It gives the service its descriptor (done: SetServiceObjectSecurity
     and QueryServiceObjectSecurity, the descriptor kept in the service
     key's `Security\Security`; `svc_test`, `wow64_svc_test`).
     It starts ClickToRunSvc (done: the SCM runs services, own-process
     ones as LocalSystem in session 0, with SYSTEM's profile environment
     and no standard handles, the dispatcher's handshake,
     SetServiceStatus, the controls and a killed process's
     ERROR_PROCESS_ABORTED; `svcrun_test`, `wow64_svcrun_test`, Wine's
     advapi32 and services tests run on Windows, and issue #65's
     measurements on 26200: a failed start's status, a missing
     executable's ERROR_FILE_NOT_FOUND, a STOP to a START_PENDING
     service, the second dispatcher, the environment; ARCHITECTURE "The
     service control manager"; what Windows reports for a live process
     whose service reported STOPPED with controls accepted is not
     measured): `OfficeClickToRun.exe /service` connects
     at once and reports RUNNING (accepting stop, shutdown and
     pre-shutdown, 0x85). Its process pool creates its COM objects from
     a thread that never initialized COM while the process has an MTA
     (done: the implicit MTA in CoGetClassObject; `msxml_test`; before,
     CO_E_NOTINITIALIZED stopped the service with 17000), and it asks
     shell32 for CSIDL_SYSTEMX86 (done: SysWOW64 and the x86 program
     files; `win32_misc_test`).
     It runs the INSTALL scenario
     (DeliveryOptimization is REGDB_E_CLASSNOTREG, it falls back to HTTP;
     catalogs as before), downloads and indexes the stream manifest, and
     checks for room (done: a disk-backed C:, ARCHITECTURE "A disk-backed
     C:", `volume_test`; the scenario is MANUAL, `persist` on a 16 GiB
     sparse disk, 3600 s of run time; its log: `AvailableSpace 15741`,
     `free:=16506359808 required:=4437233664`).
     It stages its download through the Restart Manager (done: gala's
     rstrtmgr.dll, below) and downloads the streams over HTTP: 256 KiB
     ranges from officecdn.microsoft.com, now over kept-alive
     connections (done: connection reuse in gala's winhttp and wininet,
     ARCHITECTURE "WinHTTP and WinINet", `winhttp_test` 14; a connection
     carries 95-300 MB), at 0.5-0.6 MB/s of guest time under TCG per
     connection (`XTASK_EVENTS=net`'s `tcp close` lines carry each
     connection's bytes; keep-alive did not raise it, so the time is the
     network path's under TCG and the client's own work, not the
     connections). It loads wofutil.dll (done: gala's, no overlay
     filter, below).
     Done: the memory. A 512 MiB VM ran out of memory about 30 min in;
     frames counted by use (ARCHITECTURE "Physical memory") showed 224
     MiB of user pages: the client service's commit grew with every
     short thread it made and ended (5252 reservations), as gala freed
     neither an ended thread's TEB nor its stack. Now the kernel frees
     them (ARCHITECTURE "What an ended thread leaves"; `memload_test`),
     the writeback needs no memory (buffers reserved at mount, records put
     back instead of lost: before, one failed sync left only snapshots,
     which failed for want of memory), eviction is a clock with no queue
     to grow, and pages of a mapped file leave unless a live view shows
     them (ARCHITECTURE "A disk-backed C:").
     Done: the slowdown after about 1400 s (connections carrying 3-10 MB
     over 100-300 s, TLS handshakes growing from 0.1 s to 1 s, the
     download at 0.03 MB/s by 2000 s). The client makes a thread per
     range and per pipe client, about 9000 in an hour, and the scheduler
     kept every dead thread's record: the boot CPU's tick walked all of
     them under the scheduler's lock for sleepers and timed-out waiters
     (the NMI dump caught CPU 0 in `sched::on_tick`'s walk and CPU 1
     spinning on the lock, with 8961 dead records). Now reaping removes a
     record and its slot is reused under a new id (a slot's generation in
     the id; ARCHITECTURE "Threads and scheduling"; `sched::table`'s unit
     tests; `memload_test`: 2777 threads made, 10 slots).
     The download completes in about 1650-2100 s of run time (it varies
     with the host): 10701 of 10701 files (`CDownloadManager.RetryTelemetry`
     `completeSuccess`), 1.82 GB over HTTP, 4.41 GB under
     `Program Files\Microsoft Office\Updates\Download\PackageFiles`,
     memory flat (the thread table at 33 threads in 40 slots at the
     end). Done: staging (`CRepoOperation.Stage` `CompleteStaging`,
     4.41 GB written, `DownloadSucceeded` true, about 4 min after the
     download). It links Click-to-Run's virtualization DLLs
     (AppvIsvSubsystems32/64, C2R32/64) from `Common Files\Microsoft
     Shared\ClickToRun` into the package's `root\client` and
     `root\office16` with CreateSymbolicLinkW (16 links); kernel32's
     stub answered ERROR_PRIVILEGE_NOT_HELD
     (`VirtualizationHelper::EnsureVirtualizationDlls: Failed to create
     SymLink`), which repoman reported as `repoleaves.cpp:1028` hr
     0xa0000058 and `SetCurrentState failed ... currentState=1 state=2`,
     retried every minute (the service's late system calls,
     `events_from=`, showed the root\client open and the FormatMessage
     of 1314 right before the error). Now NT symbolic links and reparse
     points are there (ARCHITECTURE "Files", `reparse_test`), SYSTEM's
     SeCreateSymbolicLinkPrivilege enabled for the call.
     The client's next task, STAGEREGISTRY
     (`C2R::VirtualRegistry::MergeAndStageRegistry`), loads OffReg.dll
     (the offline registry library: it reads and writes hive files
     itself, with CreateFileW/ReadFile/WriteFile), which failed to load
     (`RegistrationFailure (Failed to load OffReg.dll , Error:0x7f)`,
     setup.exe exit 17004) for want of its imports. Done: ntdll's
     RtlRunOnceBeginInitialize/Complete (and the rest of RtlRunOnce*,
     with kernel32's InitOnce* over them), the SRW locks in ntdll
     (kernel32's exports forward, as on Windows) and every word wait in
     ntdll's RtlWaitOnAddress (kernelbase's WaitOnAddress and kernel32's
     condition variables too: one waiter list, so a lock taken through
     one DLL is woken through the other), the bitmap routines
     (RtlNumberOfSetBits, RtlFindNextForwardRunClear and their
     companions); msvcrt's bounded string functions (wcsnlen,
     wcsncpy_s, wcscat_s and the rest of the strcpy_s family, shared
     with ucrtbase); advapi32's private object security
     (CreatePrivateObjectSecurity*, SetPrivateObjectSecurity(Ex),
     DestroyPrivateObjectSecurity, MakeSelfRelativeSD) over the kernel's
     inheritance rules (gala's GalaNewSecurityObject and
     GalaSetSecurityObject; ARCHITECTURE "Security descriptors"), and
     GetSidLengthRequired, which it calls once loaded;
     `office_api_test` and `wow64_office_api_test` (`offreg.rs`, Wine's
     conformance tests run on Windows and the documentation).
     Then MergeAndStageRegistry detokenizes the package's registry
     through known folders and stopped at those shell32 lacked
     (`Failed to get known folder mapping.`, 0x80070002 for
     [{Fonts}], [{ProgramFilesX86}], [{ProgramFilesX64}], the common
     files ones and [{SystemX86}]); done: those known folders
     (`win32_misc_test`). [{AppVPackageRoot}] is App-V's own token, not
     a Windows known folder.
     STAGEREGISTRY then succeeds. The next task, UNINSTALLCENTENNIAL
     (removing a Store-installed Office), runs
     `InspectorOfficeGadget.exe deprovisionuninstall` (32-bit, .NET
     Framework: it imports mscoree.dll alone, which gala lacks, so each
     start ends 0xc0000135; the task goes on), and the service ended on
     a delay-load failure of wtsapi32's WTSEnumerateSessionsW (crash
     report `delay load of WTSAPI32.dll!WTSEnumerateSessionsW`, about
     2520 s). Done: WTSEnumerateSessionsW/A over the kernel's session
     list (Wine's wtsapi.c test on Windows); ntdll's
     RtlValidRelativeSecurityDescriptor, whose import the service's
     pieces resolve (`office_api_test`'s `service.rs`).
     The service then writes Office's registry (NtCreateKey and
     NtSetValueKey, one after another), and each took 0.65 s: with
     `persist`, every system call's sync rewrote the registry's log as a
     whole snapshot once the changed keys alone passed the log's 1 MiB
     bound (the NMI dump caught `persist::sync_registry` encoding a
     snapshot; 1016 creates in 1290 s). Done: the bound is also twice
     the last snapshot, so syncs cost their own changes, amortized
     (ARCHITECTURE "The registry kept"; `persist`'s unit test). With
     that the service wrote 17000 keys and 25000 values in 1200 s, until
     the 16 MiB snapshot, encoded whole, failed for want of memory
     (`persist: registry changes not written (NoMemory)`, frames free
     reaching 0) and every sync after it tried again; done: snapshots
     are written as the registry is walked, a chunk at a time.
     On the fast path (below) the registry staging then ran for the
     rest of the hour at about 40 ms per NtCreateKey and NtSetValueKey
     (19000 creates and 25000 values from 1300 s to 3600 s, every
     system call's sync writing the change and flushing the disk; NtClose
     20 ms); done: the registry is flushed lazily, as Windows' is (at the
     first sync 5 s after the last, or at NtFlushKey, a program's end, a
     handover; ARCHITECTURE "The registry kept").
     With that STAGEREGISTRY ends in minutes and UNINSTALLCENTENNIAL
     runs at about 1700 s on the fast path; the service then ended on
     the delay load of WTSAPI32.dll!WTSQueryUserToken (crash report);
     done: WTSQueryUserToken (the console user's token for a
     SeTcbPrivilege holder, as documented; GalaSession's
     SESSION_USER_TOKEN, wow64.dll converts GalaSession now).
     The next delay load the task needed was USERENV.dll!
     CreateEnvironmentBlock (crash report, about 1360 s in); done:
     CreateEnvironmentBlock and DestroyEnvironmentBlock (Wine's userenv
     test on Windows), the boot writing the processor variables into
     `Session Manager\Environment` as Windows keeps them.
     UNINSTALLCENTENNIAL then completes (InspectorOfficeGadget's starts
     still fail without .NET Framework) and APPLYCONFIGURATION fails
     publishing the staged install (`Failed to publish working op ,
     Error:0x80070057`; repoman's DownloadManagers.hpp:575 E_INVALIDARG,
     `SetCurrentState ... currentState=2 state=4`; setup.exe's client
     ends 17004 at about 1850 s), the one ERROR_INVALID_PARAMETER of the
     run's ledger being kernel32's GetFileInformationByHandleEx(FileIdInfo,
     class 18); done: FileIdInformation (ARCHITECTURE "Files").
     With it the publish goes on and the service ended on the delay load
     of SHLWAPI.dll!PathBuildRootW (crash report, about 1835 s); done:
     PathBuildRootW and A (Wine's shlwapi path.c test on Windows).
     APPLYCONFIGURATION then published the staged install into
     `Program Files\Microsoft Office\root` at about 0.13 MB/s (4 KiB
     NtReadFile/NtWriteFile pairs of about 30 ms each, every call's sync
     writing its page and the volume's record and flushing the disk) and
     was cut off by the hour; done: file data is written back lazily, as
     Windows' cache manager does (ARCHITECTURE "A disk-backed C:": the
     lazy writer every second; FlushFileBuffers, write-through and
     unbuffered handles and a program's end at once; a dirty limit;
     `writeback_test`, `writeback_crash`, `writeback_lazy`).
     With it STAGEREGISTRY and UNINSTALLCENTENNIAL end and
     APPLYCONFIGURATION starts at about 1350 s (550 s earlier); its first
     step launched OfficeC2RClient.exe, whose 30 MiB image took 400 s to
     lay out from the disk because every page read walked the whole tree
     to let pages go while other uses held the memory; done: a futile
     eviction is not repeated until pages could leave (ARCHITECTURE "A
     disk-backed C:"). Where the fast path ends now (about 1600 s):
     APPLYCONFIGURATION publishes the staged files with hard links
     (`fileops::HardlinkOperation`), which fail (kernel32's CreateHardLinkW
     is unsupported, ERROR_NOT_SUPPORTED; NtSetInformationFile has no
     FileLinkInformation), falls back to copying, and the copy fails with
     ERROR_NOT_ENOUGH_MEMORY: the 512 MiB VM has no free frame (the
     `persist: free memory stays low` lines: the kernel heap 157 MiB,
     most of it grown during the registry staging, Office's registry kept
     in memory; the service's 190 MiB of private memory; 63 MiB of
     section pages, the client's 30 MiB image laid out whole; the files'
     pages only 2 MiB); setup.exe ends with 17004. Done: hard links
     (CreateHardLinkW/A, NtSetInformationFile FileLinkInformation(Ex),
     a handle opened by a link acting on its own name; the change log's
     Link record keeps them) and FileDispositionInformationEx
     (SetFileInformationByHandle's FileDispositionInfoEx, class 21, 10
     hits), test first (`hardlink_test`, `wow64_hardlink_test`, from Wine's
     test_hard_link, test_file_link_information and
     test_file_disposition_information; ARCHITECTURE "Files"). With
     these APPLYCONFIGURATION publishes the staged
     install with hard links (`CRepoOperation.Publish` in 5.5 min),
     publishes its registry, runs `integrator.exe /U ...` (it returns
     0), and then the service (OfficeClickToRun.exe) ends with an access
     violation at about 1595 s (deterministic; crash report:
     OfficeClickToRun.exe+0x2da027 reading 0, rax E_NOINTERFACE):
     it loads gala's appxdeploymentclient.dll (PackageManager) and asks
     one of its objects for interface
     {1aa79035-cc71-4b2e-80a6-c7041d8579a7}, which it lacks, and goes on
     without checking (the ledger's `appxdeploymentclient.dll!QI ...`,
     recorded since). Done: that interface, IPackageManager9 (probe issue
     #108 measured its IID and method order on Windows; ARCHITECTURE
     "Windows Runtime classes"; `combase_test`): the service calls its
     FindProvisionedPackages (an empty IVector<Package>) and goes on
     (`PackageManager::GetSortedProvisionedMsixPackages`, MSIX cleanup)
     to the virtualization's registration (`OfficeVirtRegistration`),
     where loading `ClickToRun\AppVIsvApi.dll` fails with 0x7f nine times
     (`OISVAPI::LoadDll`): gala's kernel32 lacks the thread pool
     functions it imports (CreateThreadpool, CloseThreadpool,
     SetThreadpoolThreadMaximum/Minimum, CreateThreadpoolCleanupGroup,
     CloseThreadpoolCleanupGroup(Members); the ledger's missing exports,
     9 hits each). APPLYCONFIGURATION then fails ("Encountered fatal
     exception", ErrorCodeOnly 127), the service ends with 17002 at
     1872 s and setup.exe with 17002 at 1913 s. Done: the thread pool's
     whole family Click-to-Run's DLLs import (`tools/import_survey.py`
     over the ClickToRun files of `i640.cab`, x64 and x86: private pools
     and their limits, cleanup groups, and callback environments for
     the work, timer and wait objects that 18, 13 and 14 of the files
     use), with
     I/O objects, the instance functions and the stack information
     (`threadpool_test`, `wow64_threadpool_test`, from Wine's ntdll
     threadpool and kernel32 thread tests; ARCHITECTURE "Thread pool").
     Next: the values the tests print as `info` until Windows' answer is
     read from the `windows` job's report: 32-bit
     SetThreadpoolStackInformation's refusal of sizes of 1 (its error and
     rule), whether a version 9999 environment's priority is read, a
     wait's handles once it fired, and `_wmakepath_s`'s buffer after
     ERANGE (gala writes "\0:d\\" and leaves the rest).
     With it AppVIsvApi.dll loads and OISVAPI::Initialize fails
     instead in the orchestration's component loader (`componentloader.h
     : 216`, 0x7f; ErrorCode 30175, APPLYCONFIGURATION 127, the service
     17002 at about 1845 s): the App-V DLLs it loads import, statically,
     the 64-bit kernel32's Wow64Disable/RevertWow64FsRedirection (the
     ledger's missing exports), DosDateTimeToFileTime,
     PathCchCanonicalize and the UCRT's `_wcsupr_s`, `_wcslwr_s`,
     `_wsplitpath_s`, `_wmakepath_s`, `_resetstkoflw` and
     `_get_heap_handle` (`tools/import_survey.py` on them). Done, test
     first (`office_api_test`'s `appv.rs`, `ucrt_test`, x64 and x86):
     every static import of the App-V DLLs now resolves (NtOpenJobObject
     only as a stub). With them the orchestration initializes and
     `Orchestration::UpdateAppV` merges Office's AppX manifests
     (`C:\Program Files\Microsoft Office\PackageManifests`): loading
     `AppXManifest.common.16.xml` (2.1 MB, UTF-16 LE with a BOM) fails
     (`TryRefreshMergedManifest`: `XmlDocument loadXML failed , Error:0x1`,
     msxml's S_FALSE), and APPLYCONFIGURATION ends with 30088 (the
     service 17002 at about 1816 s). gala_xml parses that file as read
     from the disk (a host check), so the string the service hands
     loadXML differs; msxml6 now records each parse error's reason and
     place in the ledger (ARCHITECTURE "msxml6"): the string fails at
     line 1, column 18 with `undeclared namespace prefix 'appv'` (the
     service 17002 at about 1874 s). A one-line string using `appv:`
     without declaring it is what reloading a node's `xml` gave: gala's
     msxml6 serialized an element subtree with its own attributes only,
     not the namespace declarations it inherits. Done: `xml` declares
     a prefix or default namespace an ancestor left out bound on the
     element that uses it (Wine's msxml3 tests; `msxml_test`'s
     `inherited_namespaces`, x64 and x86: each subtree's xml loads
     again with its names in their namespaces), and the ledger has the
     text at a parse error (`parse error at: ...`). With it the
     manifest loads, and the merge then ended the service at
     `msxml6.dll!IXMLDOMNode::replaceChild` (not implemented, 17002 at
     about 1650 s; the same end resumed from a machine snapshot taken
     at `process integrator.exe` in 21 s of run time instead of 1706 s,
     ROADMAP SN). Done: the DOM's mutation family as a whole (the
     create methods and createNode, document fragments, insertBefore,
     appendChild, removeChild, replaceChild, putref_documentElement,
     setAttribute, removeAttribute, setAttributeNode,
     removeAttributeNode, the attribute map's setNamedItem,
     removeNamedItem and removeQualifiedItem, put_text, put_nodeValue,
     normalize, getElementsByTagName, importNode, save to a file),
     test first (`msxml_test`'s `mutation`, x64 and x86, from Wine's
     msxml3 tests run on Windows; ARCHITECTURE "msxml6"). To measure on
     Windows (its `info` lines): removeAttributeNode and removeAttribute
     of what the element lacks, normalize's empty text, importNode(NULL),
     the bytes save writes. With them the merge completes (C2R's log:
     `AppX::ManifestMerge::Merge` "Finished AppX manifest merge", each
     product's manifest merged into `AppXManifest.common.16.xml`,
     `AppXManifest::Save` to `Microsoft Office\AppXManifest.xml` and
     `ThinAppXManifest.xml`), and `OfficeVirtRegistration` then failed
     in App-V's `OISVAPI::ConfigurePackage` (`xmlutils.cpp : 56`,
     0x80040154): it asks COM for FreeThreadedDOMDocument60
     (`ole32.dll!CoGetClassObject(class not registered)`, detail
     0x88d96a06), so APPLYCONFIGURATION ended with 30175 and the
     service with 17002 at about 1700 s. Done: FreeThreadedDOMDocument60
     registered and served (`msxml_test`'s `free_threaded`, x64 and
     x86). A stopgap to replace: it is the rental document, which locks
     nothing (recorded at each creation); a free-threaded document needs
     its calls serialized (a lock per document around every method)
     before a program uses one from two threads at once. With it
     ConfigurePackage gets further and fails at `xmlutils.cpp : 592`,
     0x80040154 again: it asks COM for XMLSchemaCache60
     (`ole32.dll!CoGetClassObject(class not registered)`, detail
     0x88d96a07), MSXML 6's XSD schema cache; APPLYCONFIGURATION ends
     with 30175, the service and setup.exe with 17002 at about 1570 s.
     Done: XMLSchemaCache60 registered in both views and served
     (IXMLDOMSchemaCollection2, the document's `schemas`, validate,
     validateNode and validateOnParse against XSD, schemas from files and
     `res:` URLs), test first (`msxml_test`'s `schema`, x64 and x86;
     ARCHITECTURE "msxml6"). The XSD subset is sized from the schemas
     App-V's own DLLs carry as RT_HTML resources (AppVManifest.dll: the
     AppX root, the appv/2014 and appv/2019 manifest schemas;
     AppVPolicy.dll: deployment and user configuration, connection
     groups; AppVFileSystemMetadata.dll): all of them compile and resolve
     with gala's subset (checked on the host against the files taken
     from Office's i640.cab), and what the subset lacks fails the add
     loudly. Those DLLs also carry the XSLT transforms App-V runs before
     validating (below).
     With the cache registered, ConfigurePackage failed at the same
     place with E_NOINTERFACE (17002 at about 1768 s): it creates the
     cache asking for an IID whose first dword is CLSID_XMLSchemaCache60's
     (`XMLSchemaCache60::QueryInterface` miss in the ledger). Done: the
     cache answers QueryInterface(CLSID_XMLSchemaCache60) with itself
     (App-V's code works on Windows, so MSXML answers it; probe issue
     #125 asks what Windows gives), and every QueryInterface miss of
     msxml6 now records the whole IID. With it ConfigurePackage creates
     the cache, adds one schema (a document, not a URL; namespace
     `http://schemas.microsoft.com/ap...`, cut by the ledger: the
     ledger now keeps a long value's end) and sets it as a document's
     `schemas`, then fails at `xmlutils.cpp : 672` with 0x80040154: it
     asks COM for XSLTemplate60 ({88D96A08-...},
     `ole32.dll!CoGetClassObject(class not registered)`, detail
     0x88d96a08); APPLYCONFIGURATION ends with 30175, the service and
     setup.exe with 17002 at about 1678 s. No validation ran before it
     (no validate, validateNode or load-time validation in the ledger):
     the transform comes first, as the comment of App-V's clean-up XSLT
     says ("prior to using XSD for validation").
     Done: XSLTemplate60 registered in both views and served
     (IXSLTemplate, IXSLProcessor: input, output to a string, a
     document or a stream, addParameter, setStartMode, transform) with
     IXMLDOMNode's transformNode and transformNodeToObject, test first
     (`msxml_test`'s `xslt`, x64 and x86: Wine's msxml3 XSLT tests and
     transforms of this test's own shaped like App-V's; ARCHITECTURE
     "msxml6"). The XSLT 1.0 subset is sized from App-V's own stylesheets
     (RT_HTML resources: AppVManifest.dll's clean-up that keeps the
     namespaces a parameter lists, the normalization of older appv
     namespaces into the newest, shared with AppVPolicy.dll and
     AppVFileSystemMetadata.dll, which reads IgnorableNamespaces'
     prefixes through the namespace axis, and the alias step that
     declares `appv` through `msxsl:node-set()`); what it lacks fails the
     stylesheet with `XSLT not supported: <what>` in the ledger. With it
     ConfigurePackage transforms the manifest three times (each into a
     document, with 2 and 4 parameters), validates it against the
     cache (valid), and then asks COM for CLSID_KnownFolderManager
     ({4DF0C730-DF9D-4AE3-9153-AA6B82E9795A}, `knownfolder.cpp : 64`;
     without it 0x80040154, `ole32.dll!CoGetClassObject(class not
     registered)`, detail 0x4df0c730, APPLYCONFIGURATION ends with 30175).
     The loop: snapshot at `process integrator.exe (pid`, a resume to
     the failure in 37 s against about 1820 s from boot, the service's
     log read from the resume's kept layer (`disk-files` reads it now).
     Done: the Known Folder Manager, registered for shell32 in both
     views, over one table of 72 known folder definitions that
     SHGetFolderPathW and SHGetKnownFolderPath read too, test first
     (`knownfolder_test`, x64 and x86: Wine's shell32 known folder
     tests, every definition checked against Windows' as that test
     lists them; ARCHITECTURE "shell32, version, powrprof, comctl32,
     user32 strings"). To measure on Windows (its `info` lines):
     FindFolderFromPath's code for no match, the codes for an unknown
     id, and the folders whose path differs from SHGetFolderPathW's.
     With it ConfigurePackage goes on to read typed values. Done:
     nodeTypedValue and dataType, get and put, over the `dt:dt` data
     types (test first, `msxml_test`'s typed section on x64 and x86:
     Wine's msxml3 typed value tests, aligned to MSXML 6's answers on
     Windows where they differ; ARCHITECTURE "msxml6", Typed values).
     With them (as first written, before that alignment: a run with
     the aligned msxml6 is to confirm) ConfigurePackage succeeds, the service updates
     the App-V configuration and merges and saves the AppX manifests
     (`ThinAppXManifest.xml`), then OfficeClickToRun.exe died of an
     access violation executing shell32.dll+0 from AppVCatalog.dll
     (about 1735 s after boot): AppVCatalog.dll imports
     `SHELL32.dll!#165`, which gala's shell32 did not export, and the
     loader bound the empty ordinal slot (RVA 0) to the module's base.
     Done: the loader treats an RVA of 0 as no export (an import of it
     is a missing export, `dll!#N` in the ledger, the load failing with
     STATUS_ORDINAL_NOT_FOUND; LdrGetProcedureAddress fails the same
     way; the kernel's `find_export` too), test first in `loader_test`
     (`testdll`'s ordinal gap, `testdll_gap` importing it); shell32's
     ordinal 165, SHCreateDirectory, with SHCreateDirectoryExW, test
     first in `knownfolder_test` (x64 and x86, by ordinal, Wine's
     shlfileop tests). With the loader fix OfficeClickToRun.exe no
     longer started (0xc0000138): it imports oleaut32's RegisterTypeLib
     (#163) and UnRegisterTypeLib (#186), which had bound to the base
     unnoticed; both are exported at their ordinals now and fail with
     TYPE_E_REGISTRYACCESS, recorded (`oleaut32_test`; gala loads no
     type library). Then OfficeClickToRun.exe got past the App-V
     catalog and died of 0xc06d007f (delay-loaded function not found)
     in AppVIntegration.dll: `USERENV.dll!ExpandEnvironmentStringsForUserW`,
     a named stub (GetProcAddress treats stubs as absent). Done:
     ExpandEnvironmentStringsForUserW against the user's environment
     block, test first in `office_api_test` (Microsoft's documentation).
     With that, the App-V integration runs and the install reaches its
     fonts task, but its second integrator.exe
     (`root\Integration\Integrator.exe /I /License ...`, about 2450 s
     after boot) died of 0xc06d007e (delay-loaded DLL not found):
     `osppc.dll!SLOpen`. No osppc.dll is laid down: on Windows 8 and
     later Office's licensing goes to the system's Software Protection
     client, sppc.dll, which gala lacked. Done: gala's sppc.dll
     (ARCHITECTURE "Other DLLs", sppc), a file-backed license store
     with the SL* functions integrator.exe imports, test first in
     `sppc_test` (failure answers measured against Windows' documented
     codes; the round trip of an unsigned license is gala's own, which
     Windows refuses). Integrator.exe now installs Office's licenses
     through it (SLInstallLicense, SLGetSLIDList, SLGetLicenseInformation
     in the ledger) and exits 0. The next integrator.exe died of
     0xc06d007f (delay-loaded function not found):
     `CRYPT32.dll!CryptSIPAddProvider`, a named stub. Done: crypt32's
     CryptSIPAddProvider and CryptSIPRemoveProvider (ARCHITECTURE
     "crypt32", SIP registration), the documented registry entries
     under `HKLM\Software\Microsoft\Cryptography\OID\EncodingType 0`,
     test first in `sip_test` (Wine's crypt32 sip tests, which pass on
     Windows); integrator.exe registers Office's VBA SIP through it.
     The same integrator.exe then died of 0xc06d007f on
     `WINTRUST.dll!WintrustAddActionID`. Done: wintrust's
     WintrustAddActionID and WintrustRemoveActionID (ARCHITECTURE
     "wintrust: Authenticode", trust provider registration), the
     documented entries under
     `HKLM\Software\Microsoft\Cryptography\Providers\Trust`, test
     first in `actionid_test`. Next: the install's next blocker, found
     from a snapshot taken at integrator.exe's start. A failure there fails the install as before: the service ends with 17002 and setup.exe with
     0x426a (17002, "C2RClient returned a failing exit code"). Seen
     too, not fatal so far: `slc.dll` a missing DLL (loaded by name at
     run time), Office's product key not decoded by gala's sppc
     (SLInstallProofOfPurchase answers SL_E_INVALID_PRODUCT_KEY; no
     run has called it yet), and osppcext.dll!SLActivateProduct
     (integrator.exe's other delay import, not reached).
     To measure on Windows (`loader_test`'s `info`
     lines): GetProcAddress for an ordinal in a gap (documented as
     possibly an invalid non-NULL address) and the status of loading a
     DLL that imports one. Seen too,
     not fatal so far: `kernelbase.dll!SetSystemFileCacheSize`
     and `ntdll.dll!NtSetSystemInformation` missing exports. To measure
     on Windows (`msxml_test`'s `info` lines): a string output's
     declaration, the prefixes and declarations of the App-V-shaped
     results, the prefix the alias step's elements get. To measure on Windows
     (the test's `info` lines, probe issue #116): the exact text of each subtree, in
     particular whether declarations it does not use come along (an
     attribute value `u:n` names a prefix only its ancestor declares).
     And (probe issue #125) the schema test's `info` lines: the
     validation error codes per case, add's answers for a namespace
     other than the targetNamespace and an unresolved reference, the
     QueryInterface above.
     Seen too: CryptCATAdminAcquireContext is missing (`CatalogFiles::Install`,
     30088, logged as non-fatal at the start).
     The memory (`heap by owner` and `image pages`, the state dump of a
     run bounded at 1800 s with 4 GiB; the files' pages, 2.5-2.7 GiB
     there, are the cache, given back under pressure): before the layout
     fix, image layouts 80 MiB (89 images) plus 69 MiB of section pages
     copied from them; with layouts held once, 83 MiB and 7 MiB of
     section pages; with image pages built at their first touch
     (ARCHITECTURE "Sections"), 34 MiB (7 900 of 20 400 pages in memory);
     the file tree 47 MiB for 12 000 nodes (16 bytes per 4 KiB page);
     the registry 16 MiB (22 000 keys, 31 000 values), other 20 MiB.
     Done: image pages built at their first touch and dropped under
     memory pressure (built again from their files; `imagetrim_test`),
     and a file's page tables as extents (pages in memory in chunks,
     clusters as runs: `fs::pagetable`). At 512 MiB the run then reached
     the same point (the publish, `integrator.exe` returning 0, then the
     service's IPackageManager9 crash, fixed since; 1630 s of the service against
     1550 s with 4 GiB), the kernel heap 84 MiB at 2400 s: registry 16
     MiB, file tree 20 MiB, image layouts 23 MiB (5 200 of 20 400 pages
     in memory; 3 000 dropped, 320 built again), files' pages 7 MiB,
     other 19 MiB; the 4 GiB stopgap is gone (the scenario has the
     default 512 MiB). Done: a page file for private memory
     (ARCHITECTURE "The page file"; `pagefile_test`, `pagefile_fail`):
     `C:\pagefile.sys` on the data area, the commit limit memory plus the
     page file (443 MiB at 512 MiB, so 886 MiB, growing up to 4 GiB),
     NT processes' private pages not touched since the clock last passed
     written there when the file cache's pages cannot bring free memory
     back to the mark (before image pages go), read back at their next
     touch. At 512 MiB the run reached the same point (the publish,
     `integrator.exe` returning 0, then, before IPackageManager9, the
     service's crash at 1810 s); at 2400 s the kernel heap is 82 MiB (registry 16 MiB, file
     tree 19 MiB, image layouts 23 MiB with 5 250 of 20 400 pages in
     memory, files' pages 4 MiB, other 18 MiB), user pages 205 MiB, free
     memory 118 MiB, never below 93 MiB: nothing needed paging out. At
     384 MiB (`XTASK_QEMU_ARGS="-m 384M"`, a 315 MiB page file) it reaches
     the same point (1826 s): 117 MiB of private memory paged out in 1 878
     writes, 24 MiB read back, user pages 112 MiB at 2400 s; image pages
     dropped 4 400 and built again 1 570 (with image pages let go before
     private ones: 15 300 and 9 750, while 131 MiB of private memory stayed
     resident). Next, as the measurement shows them: the registry (16
     MiB), the file tree (19 MiB).
     Also: the page file's limits (ARCHITECTURE "The page file": a page
     read back gives up its slot, no read-around, one second chance, no
     per-process working sets); `pagefile_test`'s expectations are
     Windows' documented behaviour and wait for a Windows run before
     `ci-passing.txt` lists it (CI's page file is on D:, which the test
     reads from SystemPageFileInformation).
     Also: FindFirstFileNameW listing every name of a file with hard
     links (FileHardLinkInformation).
     Also seen: the client's `OfficeC2RClient.exe
     /installupdatesdx` (session 1) ends with 3 after asking d2d1 for
     ID2D1Factory1/2/3 (Direct2D 1.1+, recorded); the gadget needs .NET
     Framework.
     Its log also warns per file that kernel32's GetCachedSigningLevel and
     SetCachedSigningLevel are missing (`GetTrustLevelOnFile failed
     getting proc addr for get/set cached signing level`, a warning; it
     goes on); their answers for a file with no cached level need a
     Windows measurement.
     HKCU still names the boot user's hive while running as SYSTEM
     (advapi32's `current_user_path` is fixed to the boot user's SID;
     Windows maps it from the caller's token: S-1-5-18's for SYSTEM).
     Done: gala's rstrtmgr.dll (x64 and x86, for 32-bit installers such
     as Inno Setup's too: sessions, RmRegisterResources,
     RmGetList over the kernel's FileProcessIdsUsingFileInformation,
     RmShutdown, RmRestart, the filters, RmJoinSession within a process;
     QueryFullProcessImageNameW of other processes; `rstrtmgr_test`, `wow64_rstrtmgr_test`, from
     Microsoft's documentation, its open answers printed for a run on
     Windows; ARCHITECTURE "Other DLLs"). Left of it: sessions shared
     between processes (RmJoinSession of another process's key,
     recorded), a window lookup for RmMainWindow (recorded), asking
     applications to close before forcing (recorded).
     Done: gala's wofutil.dll (x64: WofShouldCompressBinaries FALSE,
     WofIsExternalFile a physical file, the driver functions
     ERROR_INVALID_FUNCTION recorded; `wofutil_test`).
     Done: a development fast path, `office_install_local` (MANUAL):
     the pinned build's files from the CDN cached on the host once
     (`cargo xtask office-source`, 3.2 GB) and put on the fresh disk as
     C:\OfficeSource in seconds (`diskimage::inject`), with ODT's
     SourcePath pointing there; the Click-to-Run client starts about as
     before but nothing is downloaded (WORKFLOW "Office's fast path").
     `office_install` stays the whole path's check.
     Next, in the order the run reaches them: what STAGEREGISTRY and
     the apply after it need; the stream
     download's speed under TCG (the
     network path: `memload_test`'s loopback HTTP moves about 1.4 MB/s,
     its AFD calls 4 ms each); GetCachedSigningLevel and
     SetCachedSigningLevel (a Windows measurement first); HKCU from the caller's token; the RPC calls the client's
     pieces then make of each other (the ledger's `rpcrt4.dll!NDR(...)`
     names a format the interpreter lacks; rpcrt4 for x86 if a 32-bit
     piece needs it); CryptCATAdmin*
     (catalogs: a catalog SIP, member lookup
     by hash); slc.dll and the two exports once a run needs them, with
     their Windows answers (measured on 26200 for a standard user and an
     administrator, issue #60): SLOpen S_OK with a non-NULL handle and
     SLClose S_OK for either; SetSystemFileCacheSize(-1, -1, 0) FALSE
     with ERROR_ACCESS_DENIED and
     NtSetSystemInformation(SystemFileCacheInformationEx, 0x51)
     STATUS_ACCESS_DENIED until SeIncreaseQuotaPrivilege is enabled
     (an administrator holds it disabled; a standard user does not hold
     it, so AdjustTokenPrivileges gives ERROR_NOT_ALL_ASSIGNED), then
     TRUE and STATUS_SUCCESS;
     then the other DLLs
     setup.exe imports that gala has no i686 build of (xmllite,
     webservices, cabinet, credui, and osppc.dll, which is the system's
     sppc.dll, gala's being x64 only so far) and the functions its imports lack (shell32's
     SHGetSettings, user32's CheckRadioButton,
     iphlpapi's CreateSortedAddressPairs); BITS (the
     BackgroundCopyManager it asks for: absent, survived); WinINet's
     asynchronous sessions (INTERNET_FLAG_ASYNC: setup.exe opens one),
     RtlConvertDeviceFamilyInfoToString in ntdll, urlmon's
     ProcessUrlAction from the zones' default policies (the Internet Settings\Zones keys), a run with
     direct egress (the TLS chain of Microsoft's servers to a public
     root), then what the run reaches after them (its log
     in `target/failed/office_install.dump/` once it writes one); more of
     gdiplus as setup.exe's error UI asks it, HKCU from the caller's token,
     winhttp's AUTOLOGON_POLICY and DISABLE_FEATURE, MSXML for
     the XML it downloads (the CDN's cab manifests: more of the DOM as
     the run reaches it; load of URLs is recorded unsupported);
     wintrust's WINTRUST_SIGNATURE_SETTINGS (choosing among the
     signatures) and legacy countersignatures,
     crypt32's detached messages; FormatMessage's system text for
     ERROR_CANCELLED (1223, recorded on the declined path); the 32-bit
     dispatcher's nested exceptions and collided unwinds.
   - The install to watch: the MANUAL scenario `office_install_gui` is
     `office_install` with its windows shown
     (`user/office_install/config_full.xml`: Display Level Full,
     AcceptEULA), on the same 16 GiB disk and limit, booted by UEFI.
     setup.exe still runs as SYSTEM, elevated, but on the console's
     desktop: `consolerun.exe` starts it in the console's session on
     `winsta0\default` (as `psexec -s -i`, MDT's ServiceUI; SECURITY.md
     "Elevation"); `wait=all` keeps the boot while Click-to-Run works
     after setup.exe ends. `XTASK_SHOTS=60,120,300,... cargo xtask
     boot-test office_install_gui` writes screenshots (WORKFLOW
     "Screenshots"). Done, test first: setup.exe's splash draws with
     gala's gdiplus.dll (`gdiplus_test`, `wow64_gdiplus_test`: the
     seventeen flat-API functions it delay-loads, PNG decoding;
     ARCHITECTURE "Other DLLs"), asks Windows.UI's UISettings
     (`uisettings_test`, `wow64_uisettings_test`; gala's windows.ui.dll),
     LOCALE_IREADINGLAYOUT (`kernel32_locale_test`), answers its
     static text's WM_CTLCOLORSTATIC with a null brush (which now paints
     nothing: `gdi32_test`) and wraps its texts in U+202A/U+202C (no
     width, not drawn: `gdi32_test`). The splash ("We're getting things
     ready...", the Microsoft logo) shows as on Windows from about 25 s
     to 70 s of run time.
     Where the watched install ends now: when setup.exe has started the
     Click-to-Run client it closes its splash (its main thread sends
     WM_USER+1; the splash thread destroys its window, posts WM_QUIT)
     and the splash thread then faults right after its message loop,
     at setup.exe+0x817f reading 0xffffffec (a NULL object less 0x28,
     read at +0x14; its thread argument still holds the two HWNDs), with
     no system call between WM_QUIT and the fault. The crash report's
     register memory and deeper stack place it inside the thread procedure, not in thread
     exit: the C++ runtime's thread_start (+0x37fdcc) still has its call
     to the procedure on the stack (return +0x37fdf7), then frames
     returning to +0x3f6e75, +0x38094d, +0x3805e4 and +0x8100; ebx can be
     the splash's own object (the pointer its window keeps in extra
     bytes index 0, which GetWindowLongW returns on every message up to
     WM_NCDESTROY, as on Windows): its first word is in setup.exe (a
     vtable), its second its own address, its fifth to seventh 0, and
     none of its first twelve words changes from WM_CREATE to the fault
     (traced). Not the cause (each tried and ruled out): UISettings'
     activation, the hide and deactivation messages DestroyWindow sends
     before WM_DESTROY on Windows, WM_SIZE/WM_MOVE at creation,
     WM_SHOWWINDOW (now sent, and the window hidden before WM_DESTROY:
     `wow64_gui_test`), any GDI+ status (all Ok), the user32, gdi32 and
     kernel32 names the process resolves with GetProcAddress (all
     exported; setup.exe imports no user32 function and resolves every
     one so), SetForegroundWindow failing (tried: no timer follows), one
     more loop iteration before WM_QUIT (a WM_NULL first: the same
     fault), and the timer lead: setup.exe never calls SetTimer; its
     timers are SetCoalescableTimer's, ids 5 (30 s) and 7 (5 s), on the
     window of another thread (tid 32 here), which also registers for
     raw input and six power-setting notifications and kills them on
     WM_TIMER; the WM_TIMER #77 saw on the splash thread may be its IME
     windows' (the hooks were per thread). Done on the way: that window
     died at once before (RegisterRawInputDevices refused the 32-bit
     RAWINPUTDEVICE, 12 bytes; ARCHITECTURE "Raw input",
     `wow64_gui_test`), and a window's timer set from another thread
     now fires on the window's thread, with each waiting thread woken
     by an event of its own (ARCHITECTURE "Timers", "The message
     queue"). On gala the splash thread enters GetMessage once and gets
     no message until WM_QUIT: the main thread's SendMessage(WM_USER+1)
     runs its close (ShowWindow(SW_HIDE), PostQuitMessage from
     WM_DESTROY, DestroyWindow); on Windows (#77) the loop dispatches a
     WM_PAINT between DestroyWindow and WM_QUIT. Next: a Windows
     measurement of the splash object's words over its life and of the
     calls the splash thread makes between GetMessage's 0 and its exit
     (the issue drafted with this change), and a run as SYSTEM on the
     console (psexec -s -i), the account the scenario uses, to learn
     whether Windows' setup.exe faults there too.
     setup.exe ending so takes consolerun with it; the Click-to-Run
     client and service go on downloading (1.4 GB on C: in 900 s). The
     client's NotificationFlyout ("Microsoft 365 and Office are
     installing in the background (0%)") is shown at (-9999, -9999),
     where it waits to be placed by its notification-area icon (NIM_ADD,
     NIM_SETVERSION). Done: shasha's taskbar has a notification area
     (Shell_NotifyIconW/A with every NIM_ message, the NOTIFYICONDATA
     sizes and versions, version 4's callback layout, balloons,
     Shell_NotifyIconGetRect; `gala tray` and its clicks; comctl32's
     LoadIconMetric and LoadIconWithScaleDown, PNG icons and sizes in
     CreateIconFromResourceEx, .ico files in LoadImageW; `tray_test`,
     `wow64_tray_test`; ARCHITECTURE "The notification area"); whether
     the flyout then shows is the next run's to see.
     OfficeC2RClient.exe, the client's React Native UI
     (`react-native-win32.dll`, `c2rui.dll`, delay-loaded), started with
     0xc0000135. Done, test first (`office_api_test`,
     `wow64_office_api_test`; `tools/import_survey.py --roots
     OfficeC2RClient.exe --static` lists only two stubs left, kernel32's
     GetCurrencyFormatW and GetNumberFormatW, which end the process if
     called): kernel32's local atom table (AddAtomW/A,
     FindAtomW/A, GetAtomNameW/A, DeleteAtom, InitAtomTable, and the
     global table's A forms); the app-model API sets
     (`api-ms-win-appmodel-runtime-*`, which its C++ runtime loads for
     the app policies) resolve to kernelbase, and kernel32 answers the
     AppPolicy queries and the package queries
     (APPMODEL_ERROR_NO_PACKAGE); oleaut32's imports by ordinal:
     OleCreateFontIndirect (#420, an IFont), SafeArrayRedim (#40) and the
     DECIMAL functions (VarDecAdd/Sub/Mul/Div/Abs/Fix/Int/Neg/Round/Cmp,
     VarDecFromI4/R8, VarR8FromDec); `d3dcompiler_47.dll` (below and
     GPU G2's decision). Not yet run: whether the UI then starts (a run
     of `office_install_gui`). Next, in that order: the splash thread's fault
     (the Windows measurement above); OfficeC2RClient.exe's React Native
     UI (Direct3D
     11 through GPU G2, DirectComposition). Found on the way, not yet
     done: CreateWindowEx sends no WM_SIZE/WM_MOVE after WM_CREATE to a
     child or popup window (Wine's msg.c WmCreateChildSeq,
     WmCreatePopupSeq) and DestroyWindow sends no WM_WINDOWPOSCHANGING/
     CHANGED (hide), WM_NCACTIVATE, WM_ACTIVATE and WM_KILLFOCUS before
     WM_DESTROY (WmDestroyOverlappedSeq, WmDestroyChildSeq);
     GetSystemMetrics answers 0 for the scroll bar metrics (SM_CXVSCROLL
     and the others; SystemParametersInfo says 17) and for any index it
     does not know, without a ledger line; PatBlt is fixed for a null
     brush but BitBlt and MaskBlt with a pattern raster operation still
     paint a null brush as black; PostThreadMessage to a thread of
     the same process goes through the kernel and arrives after what
     the thread posted locally since (PostMessage, PostQuitMessage),
     not in posting order as in Windows' one posted queue; there is
     no trace of a program's user32 calls (this hunt used a
     throw-away one over GalaConsoleTrace, which WOW64 does not thunk).
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
against Nginx with OpenSSL and an in-process server, and its x86 build
`wow64_schannel_test` through SysWOW64's schannel.dll; host tests `cargo
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
   "WinHTTP and WinINet"); the status callbacks of a synchronous
   session and the connect time-out (`winhttp_cb_test`,
   `wow64_winhttp_cb_test`). Still open there: connection reuse,
   asynchronous sessions (and their completion notifications), proxies,
   authentication,
   WINHTTP_DISABLE_REDIRECTS.
2. Done: wininet's HTTP functions on the same engine as winhttp
   (`user/http_client`; `wininet_test` against the same Nginx:
   InternetOpenW/ConnectW, HttpOpenRequestW/AddRequestHeadersW/
   SendRequestW, HttpQueryInfoW with CUSTOM, InternetQueryDataAvailable
   and InternetReadFile, InternetOpenUrlW, InternetCrackUrlW, an unknown
   issuer as ERROR_INTERNET_INVALID_CA, a closed port, a redirect
   followed and one not (INTERNET_FLAG_NO_AUTO_REDIRECT); ARCHITECTURE
   "WinHTTP and WinINet"); x86 too (`wow64_wininet_test`), with
   InternetQueryOptionW, InternetSetStatusCallbackW, InternetGetCookieExW
   (no cookie store: none) and the A functions ODT's setup.exe imports.
   Still open there, besides winhttp's list: the other A functions, the
   cookie store (Set-Cookie kept, sent and answered), FTP, the cache,
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
   (OleSetClipboard and friends, over user32's clipboard) and
   DoDragDrop.
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


### WD. Windows divergences: the test programs on real Windows

**Goal**: every test program passes on real Windows too, so that what
the tests assert is what Windows does (and gala, which passes them,
does it). The agent on a Windows machine runs `user/bin/run-all.ps1`,
finds each failing check and probes it (`docs/windows-triage-brief.md`);
its findings are `user/windows/triage.json` and `triage.md` (Windows 11
25H2, build 26200). Fixes are made here, test first; a fix only follows
what was observed there or is documented.

Done (the first triage): InitOnceExecuteOnce passes the caller's Context
to the callback as it is, NULL included (kernel32_sync_test faulted
writing through it); DirectWrite asks a custom loader for its extended
interfaces, so dwrite_test's objects answer QueryInterface only for
IUnknown and their own; the 32-bit RtlUnwind does not preserve ebx, esi,
edi and ebp (wow64_seh_test saves them around it); WriteConsoleW on a
pipe is ERROR_INVALID_FUNCTION (ERROR_INVALID_HANDLE on a file), and
WriteConsoleA the same; SetPriorityClass ignores the bits that are no
class and IDLE's wins, a value with no class bit is
ERROR_INVALID_PARAMETER, REALTIME without SeIncreaseBasePriorityPrivilege
is HIGH; NtCreateTimer's unknown type is STATUS_INVALID_PARAMETER_4; NtCreateEvent with an unknown type is
STATUS_INVALID_PARAMETER (not _4); the process handle limit is 0xff0000
(quota_test, which also ends its children on every path now); the
build-number checks (kuser_test, peb_test, manifest_test, registry_test,
wow64_test) follow KUSER_SHARED_DATA's NtBuildNumber, at least 26100;
kernel32_file_test removes an earlier run's directory first; sspi_test
compares the client's name with the process's own account;
browser_api_test imports CompareObjectHandles through its API set.

Done (Round 3, the tests' own bugs): the tests that wrote to C:\ or
C:\gala use `%TEMP%` and remove an earlier run's leftovers
(kernel32_pool_test, kernel32_system_test, security_test, cmd_test;
kernel32_test copies testdll2.dll there under a name found nowhere else
for SetDllDirectoryW); kernel32_clr_test follows
GetActiveProcessorCount and setupapi_test the batteries the machine has;
kuser_test compares NtSystemRoot without case; kernel32_file_test reads
the IO_STATUS_BLOCK's 32-bit Status; kernel32_py_test clears
NoDefaultCurrentDirectoryInExePath first; seh_test registers its dynamic
function table for memory outside any image; threadinfo_test's buffers
are aligned; object_test asks TRAVERSE | CREATE_OBJECT of the global
BaseNamedObjects, section_test and symlink_test name their objects in
the session's; smpsem_test, safepoint_test and smprace_test (ntrt's
timers) reach NtCreateSemaphore, NtQuerySemaphore, NtCreateTimer and
NtSetTimer through ntdll's exports, wow64_test the 32-bit ntdll's for
gala's own numbers; oleaut32_test no longer passes VarBstrFromCy a NULL
output; disk_test writes nothing unless the disk is 16 MiB long with
xtask's marker in sector 1 (exit 0xd15c otherwise).

Done (Round 3, Windows' behaviour, each from a measurement on 25H2):
TokenIntegrityLevel's attributes are 0x60 (advapi32_test);
CERT_CONTEXT keeps only the certificate encoding (cert_test);
RaiseException sets EXCEPTION_SOFTWARE_ORIGINATE (kernel32_misc_test);
SetThreadDescription returns 0x10000000 (kernel32_thread_test, rtl_test,
kernel32_clr_test); LdrGetProcedureAddress of a missing name is
STATUS_ENTRYPOINT_NOT_FOUND (loader_test); a PE32 view in a 64-bit
process is STATUS_IMAGE_MACHINE_TYPE_MISMATCH (image_test) and a
relocated PE32 header names its load address (wow64_import_test);
PeekNamedPipe's bytes left go below 0 on a byte pipe (pipe_test); an
ended thread's TebBaseAddress is NULL (thread_test, wow64_thread_test);
PrivilegeCheck takes a primary token (token_test); GetGestureInfo(NULL)
is ERROR_INVALID_PARAMETER (win32_misc_test); MEM_DECOMMIT of size 0
away from the base is STATUS_FREE_VM_NOT_AT_BASE (vmcycle_test);
Kerberos's ISC is SEC_E_NO_CREDENTIALS (sspi_test);
LsaRegisterLogonProcess without SeTcbPrivilege is
STATUS_PORT_CONNECTION_REFUSED, and lsa_test stops there, "skipped"
(lsa_test); GetBoundsRect gives DCB_SET or DCB_RESET only (gdi32_test);
SystemQueryPerformanceCounterInformation needs Version 1 (sysinfo_test);
SO_RANDOMIZE_PORT is for unbound sockets (udp_test);
AssocQueryStringW(ASSOCSTR_EXECUTABLE) of a missing program is
CO_E_APPNOTFOUND (shlwapi_test); SHGetIconOverlayIndexW hands out an
index per path (shell32_test); SafeArrayAllocDescriptorEx leaves
cbElements 0 (com_test); CoCreateInstanceEx wants every MULTI_QI's
pItf NULL (ole32_test); SymSrvGetFileIndexInfoW refuses an image
without a debug directory (dbghelp_test, which now indexes
kernel32.dll); CB_SETCURSEL past the end keeps the selection
(controls_test); WSB_PROP_WINSTYLE has a low bit per scroll bar
(comctl32_test); user32_test counts only its own windows in its CBT hook
(Windows adds the thread's "Default IME" window). From Round 4's probes:
the 64-bit RtlRaiseException itself marks the record 0x80 (the 32-bit
one does not; seh_test, wow64_seh_test); a relocated image's mapped
OptionalHeader.ImageBase names its load address, PE32+ as PE32; a PE32
view in a 64-bit process is IMAGE_MACHINE_TYPE_MISMATCH whether or not
its base was free; PeekNamedPipe's bytes left in byte mode is 0 minus
what was read (0 with no buffer), as gala had it.

Done (Round 4, the tests' own bugs): browser_api_test's report names the
failing line (its check numbers are shared by several calls);
quota_test judges the handle limit against the count just before its
duplicate loop and prints base, that count and n with a failure (the
children and the event as all that came in between is gala's claim
only); kernel32_clr_test opens its DLLs beside itself (x86_native.dll
exported to user/bin); advapi32_test's service SIDs say "skipped" when a
standard user may not create the service key; win32_misc_test counts the
clipboard's formats against what enumerating it yields; wow64_test grows
its SystemProcessInformation buffer to what is needed; conpty_test
passes STARTF_USESTDHANDLES with NULL handles, as Microsoft's ConPTY
samples do (without it a console child takes the parent's non-console
handles), and gala makes each NULL one a handle to the pseudo console;
token_test's SIDs are ULONG-aligned; user32_test's activation checks are
skipped, with a line, when SetForegroundWindow is refused (the
foreground lock); oleaut32_test's GetActiveObject runs in an apartment;
kernel32_locale_test compares the system's preferred UI languages with
its default UI language.

Done (Round 4, Windows' behaviour, each from a measurement on 25H2):
cmd passes a program its command line as written, the space before a
removed redirection included (cmd_test's `tail` mode); NtFlushBuffersFileEx
does not refuse flag 0x100 (kernel32_file_test; other unknown flags are
recorded and ignored); GetThreadDescription returns 0x10000000
(kernel32_thread_test, kernel32_clr_test); THREAD_BASIC_INFORMATION's
BasePriority is the increment over the process's base (threadinfo_test;
GetThreadPriority returns it); InitSecurityInterfaceA's table is version
1 (sspi_test); SO_RANDOMIZE_PORT reads back as one byte (udp_test);
CertGetNameStringA with a buffer too small returns 1 (cert_test);
inherited ACEs carry INHERITED_ACE, and the child's DACL
SE_DACL_AUTO_INHERITED, only under an auto-inherited parent DACL
(security_test, which also checks a "D:PAI" parent); ntdll's
BaseDllName is not inside its FullDllName, so loader_test compares the
text. From the Windows agent's probes: VerSetConditionMask sets bit 63
with a condition (kernel32_system_test, whose report prints a 64-bit
value whole); AssocQueryStringW with a short buffer is
HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER), the size and the string
truncated (shlwapi_test); CoCreateInstanceEx resets the MULTI_QI
entries in order and stops at one whose pItf is set (ole32_test);
CB_RESETCONTENT returns 1 (controls_test); Segoe UI's cell and ascent
from an em are truncated, 15 = 12 + 3 at lfHeight -12 and 96 DPI
(gdi32_test); ExtractIconW of a file that is no image is NULL with no
error (shell32_test); SymFromAddr's export-table symbol has ModBase 0
and SizeOfStruct must be 88 (dbghelp_test; Windows' Size, 112, is not
asserted: gala cannot know it from the exports).

Done (Round 5, the tests' own bugs): what needs a precondition an
ordinary user lacks checks for it and says "skipped" without it (exit
0), while gala's scenarios give it and run the test whole: the galatest
account (access_test, logon_test, profile_test: LookupAccountNameW's
ERROR_NONE_MAPPED), SYSTEM (system_test, session_test: TokenUser), an
administrator (restricted_test: CheckTokenMembership; disk_test and
wow64_adv_test's registry view: ERROR_ACCESS_DENIED with TokenElevation
0); what is gala's own runs only on gala (`ntrt::on_gala`, now on x64 too:
clock_test, handover_test, handover_k32_test and gala_cli_test,
pk_install_test whole, handover_net_test's `handover` line), since gala's
service numbers reach win32k's table on Windows and fault
(STATUS_FATAL_USER_CALLBACK_EXCEPTION, Round 5's three crashes). Also:
advapi32_test's files go in %TEMP% (an earlier run's removed first);
kernel32_file_test's QueryDosDeviceW("C:") checks the form
`\Device\HarddiskVolume<digits>` (gala: 1); kernel32_locale_test's
system locale is the machine's (installed, the LCID of
GetSystemDefaultLocaleName; 25H2 measured ar-SA under an en-US user;
gala: 0x409); kernel32_system_test's RtlVerifyVersionInfo compares with
the running build (NtBuildNumber) and the next; loader_test copies
testdll and testdll2 to %TEMP%\gala_loader\sub itself (and removes
them), expecting the plain load's dependency beside the exe;
shell32_test initialises COM (single-threaded) before ShellExecuteEx, as
its documentation asks; persist_test's last-access time may move forward
off gala (a reader such as Defender moves it); win32_misc_test skips its
clipboard part off gala when another process holds the clipboard,
naming that process.

Done (Round 5, Windows' behaviour, each from a measurement on 25H2):
the unwind's handlers do not see the record's EXCEPTION_SOFTWARE_ORIGINATE
(seh_test: 0x02 and 0x22; Round 4's probe); PFXIsPFXBlob does not look
at the version (cert_test); cmd /c of an unknown command exits 1, with
`2> f`, `> f`, `< f` or no redirection (cmd_test); StackWalk64 is not asked to fail
for IMAGE_FILE_MACHINE_I386, which the x64 dbghelp walks (dbghelp_test
uses 0x1234; gala records I386 as unsupported); GetTextExtentExPointW
fills lpnDx only for the characters that fit when lpnFit is given, as
documented (gdi32_test; the A form too), and leaves the rest as they
were (gdi32_test's -1s stay). KB: kernelbase hosts the core API sets as
on Windows (the schema's hosts from Wine's apisetschema.spec; kernelbase
exports Windows' kernelbase list as forwarders to gala's DLLs, ships
beside every kernel32, x64 and x86), and CompareObjectHandles is
kernelbase's only (browser_api_test: its API-set import is
GetProcAddress(kernelbase)'s address, kernel32's GetProcAddress NULL). WOW64's file system redirector as
documented, under a 32-bit kernel32 that names System32 as Windows'
does (wow64_k32_test: GetSystemDirectoryW/A `C:\Windows\system32`,
25H2's "C:\WINDOWS\system32"; GetSystemWow64DirectoryW SysWOW64;
CreateFileW of System32's ntdll the 32-bit one, Sysnative's the 64-bit
one, the drivers\etc exemption; Wow64Disable/Revert/EnableWow64FsRedirection
per thread; CreateProcessW's image redirected, Sysnative's cmd 64-bit;
wow64_fs_test: every documented exemption).

Done (issues #12 and #15, Windows' behaviour from measurements on 25H2):
kernel32 does not export the PathCch functions, WaitOnAddress,
WakeByAddressSingle/All or the interrupt-time queries, which kernelbase
implements itself (the address waits from a source kernel32 also builds
privately for its locks: ARCHITECTURE "API sets");
MapUserPhysicalPagesScatter is kernel32's alone; FlsGetValue2 and
TlsGetValue2 are one function behind both, CheckTokenMembershipEx,
SetFileIoOverlappedRange and the XState functions in both
(kernel32_test, wow64_k32_test: `export_split.rs`). SetStretchBltMode
keeps any mode, 0, 5 and -1 included, and returns the previous one
(gdi32_test, a DISPLAY DC and a memory DC). The first SetWindowPos of a
never-shown window activates it (a second WM_WINDOWPOSCHANGING, then
WM_ACTIVATE, before WM_WINDOWPOSCHANGED), the next one does not
(user32_test, when the process may set the foreground; it prints the
sequence when that check fails).

Done (issues #67-#94, Windows' answers on 25H2 build 26200, a standard
user): d2d1's and d2d1_test's IID_ID2D1Factory had two bytes wrong (the
factory was E_NOINTERFACE on Windows), and actctx_test's and kernel32's
ERROR_SXS_SECTION_NOT_FOUND was 14004 for 14000: both now the headers'
values (a scan of every `ERROR_*` constant against winerror.h finds no
other). WICCreateImagingFactory_Proxy(0) is
WINCODEC_ERR_UNSUPPORTEDVERSION and a BMP's GetFrame(1)
WINCODEC_ERR_FRAMEMISSING (windowscodecs_test). IsBadReadPtr uses up a
guard page (PAGE_READWRITE afterwards), OfferVirtualMemory of an
unaligned address is ERROR_INVALID_PARAMETER, CompareStringA refuses
SORT_DIGITSASNUMBERS with ERROR_INVALID_FLAGS (office_k32_test). The
process default's root manifest path is the executable's, type 2, and
its assembly's manifest type 2 (actctx_test). Registry notification
(regnotify_test): STATUS_NOTIFY_ENUM_DIR and Information 0 after a
change, STATUS_NOTIFY_CLEANUP after a close and for the watched key's
own deletion, a kept change returns STATUS_SUCCESS at once, a
synchronous request STATUS_NOTIFY_ENUM_DIR, the same value set again is
no change, a security change fires ATTRIBUTES, asynchronous
RegNotifyChangeKeyValue without an event is ERROR_INVALID_PARAMETER, a
subordinate in the master's hive STATUS_INVALID_PARAMETER, a waiting
watch is no pending I/O. ntdll has no RtlFindNextForwardRunSet (Wine's
only), so gala's does not export it and office_api_test checks it is
absent. tray_test passes as it is (its unknown icon's HRESULT is now an
`info` line); volume_test passes, its disk-full part skipped on a C: with
2 GiB free or more.

Next:
1. The next run on Windows (each fix may reveal a program's next
   failing check), and the probes still open, asked of the agent there:
   - from issues #67-#94: reparse_test as a standard user made a link
     with SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE (developer mode
     unknown; the result and AllowDevelopmentWithoutDevLicense are now
     `info` lines) and its elevated half; regnotify_test's renaming
     handle, whose KeyNameInformation is not the new name on Windows
     (its query status and name are `info` lines: the handle lacks
     KEY_QUERY_VALUE); ResolveLocaleName(NULL), not the system default
     on 25H2 (system or user default is an `info` line); actctx_test's
     RETURN_HACTCTX handle for the process default (the report's value
     0 means NULL, its text says not NULL; the handle itself is printed
     now); d2d1_test past the factory and windowscodecs_test past the
     factory (their `info` lines); memload_test's thread-stack totals
     (unchanged when sampled: they need a wait before the sample to say
     whether Windows frees the stack); winhttp_test 14's connection reuse
     (needs the elevated Nginx setup); volume_test's disk-full part on a
     volume under 2 GiB free (TEMP on a small VHD: the program writes in
     %TEMP%);
   - SetPriorityClass's REALTIME from a token that holds
     SeIncreaseBasePriorityPrivilege disabled (an elevated prompt):
     REALTIME or HIGH (gala: REALTIME);
   - the not-yet-probed checks in `triage.md` (75 programs classed
     "expectation" without a probe), most numerous first: the kernel32
     tests, then user32/gdi32/comctl32/shell32/shlwapi;
   - clusapi_test (resutils.dll faults, no cluster installed): the call
     it makes last (it prints nothing first: a line per step would
     tell);
   - what Round 3's changes left unmeasured: whether the unwind keeps
     the record's EXCEPTION_SOFTWARE_ORIGINATE (gala: 0x82, seh_test
     asserts it); PrivilegeCheck's
     attributes on a primary token (SE_PRIVILEGE_USED_FOR_ACCESS);
     GetBoundsRect after SetBoundsRect(DCB_DISABLE) (gala: DCB_RESET);
     SO_RANDOMIZE_PORT on a bound TCP socket; seh_test's lookup of a
     dynamic table's indirect entry (the test now registers it for
     VirtualAlloc'd memory);
   - token_test's CreateRestrictedToken with its SIDs aligned (Round 4
     gave ERROR_NOACCESS with an unaligned one): if it passes, gala's
     NtFilterToken refuses a misaligned SID as Windows does;
     GetActiveObject without CoInitialize (gala: MK_E_UNAVAILABLE;
     Windows likely CO_E_NOTINITIALIZED); a console child started on a
     pseudo console without STARTF_USESTDHANDLES from a parent whose
     standard handles are a mix of console and pipe handles (which ones
     it takes; gala gives it the console's three);
     from Round 4's fixes: NtFlushBuffersFileEx with flags 0x8, 0x10,
     0x8000_0000, 0xffff_ffff and with Parameters of size 0 and 4 (which
     are refused); ThreadBasePriority 15 and -15 read back from
     THREAD_BASIC_INFORMATION (gala: 15 and -15); InitSecurityInterfaceW's
     version (gala: 4); getsockopt(SO_RANDOMIZE_PORT)'s returned length
     (gala: 1); what CertGetNameStringA leaves in a buffer too small
     (gala: a NUL) and CertGetNameStringW with cchString 5; a subkey's ACE
     under a key made from "D:P(A;CI;KA;;;WD)" (gala: 0x02, the files'
     rule) and security_test's "D:PAI" case; Segoe UI's metrics at
     lfHeight -9, -16 and 20 (gala truncates from an em, rounds from a
     cell height); VerSetConditionMask with type mask 0 (gala: the mask
     unchanged); kernel32_misc_test's raise
     address: GetModuleFileNameW of RtlPcToFileHeader(ExceptionAddress)
     after RaiseException (very likely KernelBase; the test is unchanged
     until it is named, since kernel32's RaiseException may be a stub
     of its own);
     persist_test needs run-all.ps1 to remove C:\persist and
     HKCU\Software\GalaPersist and run it twice, judging the second run.
     From Round 5's fixes: RaiseException(code, EXCEPTION_NONCONTINUABLE)
     unwound (seh_test's shape): does the unwind see 0x03 or 0x02 (gala:
     0x03, only 0x80 dropped); after StackWalk64(IMAGE_FILE_MACHINE_I386) with an x64
     context, frame.AddrPC/AddrFrame/AddrStack and a second call's result,
     and StackWalk64(0x1234) (dbghelp_test asserts FALSE);
     BuildSecurityDescriptorW(NULL, NULL, 1 entry, 0, NULL, NULL):
     GetSecurityDescriptorControl, the SACL pointer (NULL or an ACL: its
     AclRevision, AclSize, AceCount), before browser_api_test and gala
     follow (25H2 said SE_SACL_PRESENT); shell32_test's CoInitialize
     result now (did a shell call change the apartment?).
     From issue #12: user32_test's first SetWindowPos when
     run-all's process has (and has not) the foreground (the measurement
     ran from a console that held it; the test prints foreground rights,
     activation and the message sequence if they disagree), and what the
     second WM_WINDOWPOSCHANGING's WINDOWPOS holds (gala: SWP_NOMOVE |
     SWP_NOSIZE, HWND_TOP); a WS_OVERLAPPEDWINDOW's client width at
     other DPIs (25H2 at 96: 304 of 320; gala's frame: 318).
     The open probes are GitHub issues labelled `windows` and `probe`
     (#6 Round 5's); the runner's skips and expected exits are #7. #8's
     fifteen measurements are acted on: the tests now expect what
     Windows does, except the two WOW64 ones (WOW item 3).
2. The tests that need a setting run-all.ps1 cannot give say "skipped"
   without it (system_test, session_test: SYSTEM; restricted_test,
   disk_test, wow64_adv_test's view: an administrator; access, logon,
   profile, lsa: the galatest account or SeTcbPrivilege): run-all gets a
   way to run them for real (psexec-like as SYSTEM, elevated, the account
   created).
3. run-all.ps1's `fault` field: it reads Windows Error Reporting's
   event 1000, which SEM_NOGPFAULTERRORBOX (set by the runner) prevents;
   `user/windows/fault.py`, which debugs the program, gives the same
   facts. run-all uses that way instead.
4. **KB**'s rest: kernel32's exports of ntrt's runtime symbols
   (`__C_specific_handler`, `_fltused`, `mainCRTStartup`), which
   Windows' has not; the 32-bit kernel32/kernelbase split, asserted as
   the 64-bit one measured (wow64_k32_test), to confirm on a 32-bit run;
   the four PathCch stubs (PathCchAppend, PathCchCombine,
   PathCchRemoveBackslash, PathCchRemoveExtension) to implement from
   pathcch.h's contracts, test first. gala's kernel32 does not import
   kernelbase (Windows' does, so there kernelbase is in every process).
5. **The default IME window**: on Windows a thread's first window brings
   the thread's IME window (class "IME", title "Default IME"; a CBT hook
   sees its HCBT_CREATEWND too). gala makes none (user32_test counts
   past it); programs that enumerate a thread's windows or look for it
   (ImmGetDefaultIMEWnd) see the difference.

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
  read/write/wait4). fork (and clone with only SIGCHLD) with a
  copy-on-write address space, wait4, SIGCHLD at the child's end. Scenario
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
- Linux identity from gala's accounts (`lx_id` with id_test, recorded
  on Linux as an ordinary user; the owner: user accounts from the
  start): uid, gid and groups from the process's token (the boot user
  `gala` is 1000, SYSTEM root, other SIDs nobody), everywhere Linux
  reports them; `/etc/passwd` and `/etc/group` made at boot from the
  account database, bound over the shared tree's `/etc`; homes on the
  users' profiles (`/Users/gala`); getgroups (ARCHITECTURE "The lx
  module", Linux identity).
- Milestone 4's dynamic linking (LD step 1): glibc programs start
  through their interpreter (PT_INTERP) and ld.so loads their libraries
  (`lx_dyn`, `lx_dyn_pie`); file mappings (views of sections on the
  file's pages: MAP_PRIVATE copy-on-write, MAP_SHARED, cut by munmap,
  MAP_FIXED and mprotect, shared by a fork), mremap, madvise (mmap_test,
  `lx_mmap`); clone3, set_robust_list, get_robust_list (thread_test,
  `lx_thread`); prlimit64, getrlimit, setrlimit, getrandom,
  `/proc/self/exe` (exec_test, `lx_exec`).
- Copy-on-write fork (LD step 1's rest; forkmem_test, `lx_forkmem`,
  `lx_forkmem_pie`): a fork shares the parent's private pages with the
  child (frames with owner counts in the frame allocator, read-only in
  both until a write copies them: `UserSpace::share_into`,
  `make_writable`), MAP_SHARED anonymous memory is a section both map,
  and a shared futex on it is keyed by its physical address; the fork
  line counts the pages shared and copied (`0 copied` required).

**Next**:
1. Milestone 2's rest: the rest of signal_test and fork_test
   (rt_sigqueueinfo, sigaltstack, faults as SIGSEGV/SIGBUS/SIGFPE
   through the exception path, delivery to a thread running user code,
   stop/continue, process groups and sessions, waitid, rusage), vfork
   sharing memory;
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
   delete through a hard link's name removing that name; a file's
   owner from its security descriptor's owner (the Linux ids come from
   the token, `lx::identity`; a file shows the caller as its owner
   today); a reboot scenario for Linux
   files (modes and links are host-tested only).
5. Seam 5: shared core objects when pipes, sockets or files are
   reworked. Built: AF_UNIX sockets (`unix`, under `lx::unix`); next for
   them: Windows' AF_UNIX through afd on the same core sockets (a test
   first on real Windows: bind, connect, accept, the socket file as a
   reparse point), then pipes and TCP sockets' state (with wait queues
   instead of TCP's 1 ms look).
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

**Done so far**:
- Step 2's core (`wsl` with user/wsl_test, the screen scenario
  `wsl_terminal`; ARCHITECTURE "The WSL facade"): gala's `wsl.exe` in
  System32 with `--import` of a rootfs tarball (tar or tar.gz),
  `-l`/`-l -v`, `-d <distro> [--exec] <cmd>` and `-- <command line>`,
  the default distribution's login shell, `--terminate`, `--shutdown`,
  `--unregister`, `--set-default`, `--version`, with WSL's messages,
  UTF-16 output and exit codes; the per-user registry under
  HKCU\...\Lxss as Windows keeps it; each distribution's own tree
  (`<BasePath>\rootfs`, made by the kernel: GalaLxNode) in which its LX
  processes are rooted (a file written in one is absent from another);
  the starter's console and pipes as their fds (`lx::ntio`), so `wsl -d
  bb` in a console window (and the start menu's Linux tile) is the
  distribution's terminal: busybox's shell prompts, edits and runs
  commands there; `events=wsl`. wsl_test's uncertain answers (version
  lines, error-code lines, BasePath's form, the state right after a
  command, a terminated wsl.exe's exit code) are `info` lines to record
  on Windows; real WSL's transcripts (step 1) are still to be recorded.

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
2. The facade's core's rest: wsl_test checked on Windows (its `info`
   lines become checks); the WSL service (the COM interface Docker
   Desktop and Podman call, the registry's one writer), a
   distribution's init as an LX process (an instance that outlives its
   last command, as WSL's), DefaultUid applied (root inside the
   distribution), `-u`, `--cd`, `--export`, ptys and process groups
   for the terminal (LX "Next" 2-3: job control, Ctrl+C as SIGINT).
3. The facade's plumbing: `/mnt/<drive>` inside the distribution
   (`\\wsl.localhost\<distro>` outside is done: EX, `nt::wslfs`), interop
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
   Nesting is a requirement, not an afterthought: namespaces and
   cgroups are trees whose every level carries its own limits (a
   cgroup subtree delegated to a container is divided again inside
   it), and gala's overlay takes an overlay as its upper layer (Linux's
   refuses, which is why Docker-in-Docker falls back to `vfs` or a
   volume there). Goal beyond Linux: a nested container runtime
   without `--privileged`, an inner root fully contained (as Sysbox
   does on Linux).
5. Podman: `podman machine init`, `start`, then `podman run` from the
   Windows side; Podman Desktop after BR stage 2 (Electron).
6. Docker Desktop: install, the backend service, its distribution, the
   `docker` CLI over `\\.\pipe\docker_engine` running containers; then
   the dashboard (Electron, after BR stage 2).
7. Container engines on Linux itself, with no Windows side: in a
   distribution on gala (a WSL one or LD's desktop), the distribution's
   own `podman` and `docker` (dockerd, containerd, runc or crun) run
   containers, rootless Podman first (user namespaces with
   newuidmap/newgidmap and /etc/subuid, rootless networking through
   pasta or slirp4netns, overlay in a user namespace, cgroup v2
   delegated by systemd's user instance), then rootful Podman and
   Docker.
8. Nesting: the official `docker:dind` image runs `docker run --rm
   hello-world` inside it; Podman in Podman (`quay.io/podman/stable`)
   likewise; then both without `--privileged`. The sibling pattern
   (the host's `/var/run/docker.sock` mounted into a container) comes
   with step 6 or 7.

**Done when**: from a gala command prompt, `podman run --rm hello-world`
and `docker run --rm hello-world` print their greeting with both
products installed by their own installers; the same from a shell in a Linux
distribution on gala with its own Podman and Docker, and from inside a
`docker:dind` container; then a container serving
HTTP reached from the Windows side (`-p 8080:80`). Observability with
each step: the facade's calls as events (`events=wsl`), each
distribution's console and the products' logs copied into the serial
log, missing Linux calls in the ledger by distribution.

**Licensing**: Podman is Apache-2.0; Docker Desktop's licence terms
(a paid subscription for larger organisations) govern running it.
Neither is committed; xtask fetches them and runs them only in QEMU.
WSL's source (MIT) is an allowed source (CLEAN_ROOM "WSL").

**Design**: ARCHITECTURE "Plan: Linux containers through a WSL facade".

### LD. Desktop Linux applications

**Goal**: graphical Linux programs (GTK and Qt applications as
distributions ship them) run on gala, each a window on shasha's desktop
beside Windows programs, on the LX personality (the owner: the first of
the next breakthroughs).

**Done so far**:
- Step 1, dynamically linked programs: user/lx/dyn_test, linked
  against glibc with gcc (build.sh's `GLIBC_TESTS`; its libraries
  libdyn.so and libdyn2.so beside it, found through DT_RUNPATH
  `$ORIGIN`), its transcript recorded on Linux: start-up through ld.so
  (AT_BASE, AT_PHDR, AT_PHNUM, AT_ENTRY, AT_EXECFN as glibc reads them),
  stdio, malloc (brk and mmap'd blocks, realloc through mremap), the
  environment, a shared library's function, variable (a copy
  relocation), constructor, destructor and TLS (also in a second
  thread), dlopen/dlsym/dlclose of a library of ours (its TLS) and of
  libm. Scenarios `lx_dyn` and `lx_dyn_pie` (ET_EXEC and PIE with an
  interpreter) match it exactly, with Ubuntu 24.04's glibc 2.39 (the
  `libc6` .deb, a `PREBUILT`) at /lib/x86_64-linux-gnu and /lib64.
  Built for it, each in a corpus transcript recorded on Linux
  (ARCHITECTURE "The lx module"): PT_INTERP (the interpreter mapped
  below the mmap base, AT_BASE); file mappings as views of sections on
  the file's own pages (MAP_PRIVATE copy-on-write, MAP_SHARED writing
  the file, MAP_FIXED and munmap and mprotect over parts of them, a
  fork sharing them), mremap moving pages without copying, madvise
  (mmap_test); clone3 and set/get_robust_list (thread_test);
  prlimit64/getrlimit/setrlimit, getrandom and `/proc/self/exe`
  (exec_test); copy-on-write fork (forkmem_test, `lx_forkmem`: private
  pages shared read-only until written, `0 copied` in the fork line;
  MAP_SHARED anonymous memory and its shared futexes one for parent
  and child); robust futex lists released when a thread ends, at exit
  or its process's end (thread_test); SIGBUS for a file mapping's page
  past the file's end (mmap_test).
- Step 2's first prerequisite, memfd (`lx_memfd`, memfd_test recorded
  on Linux): memfd_create as an anonymous file of the shared file
  system, its mappings shared by every process, the seals (SHRINK,
  GROW, WRITE, FUTURE_WRITE, EXEC, SEAL) checked by the file system;
  a file view follows the file's size (mremap after ftruncate).
- Step 2's second prerequisite, AF_UNIX sockets (`lx_unix`, unix_test
  recorded on Linux), the first shared core object of seam 5: the core
  `unix` module (sockets, names keyed by a file-system socket node or
  abstract bytes, connections, queues, rights in flight, credentials;
  no personality's types) under the Linux view `lx::unix`: socket and
  socketpair (stream, seqpacket, datagram), bind to a path, abstract
  names and autobind, listen, accept4, connect with a backlog,
  read/write, sendmsg/recvmsg (also for TCP), shutdown, the names,
  SO_PEERCRED, poll and epoll on wait queues, SCM_RIGHTS passing open
  file descriptions (a memfd both processes map, a pipe end) with
  MSG_CTRUNC and MSG_CMSG_CLOEXEC.
- Step 2's server, first part (`lx_wl`, `lx_wl_pie`: user/lx/wl_test,
  recorded against weston's headless backend, matches exactly): the
  wire format (`kernel/wl_abi`, tables generated from the protocol
  XML) and a Wayland server in the kernel (`kernel/src/wayland`,
  `wayland=on|trace`): one kernel thread serving every client on
  `/run/user/<uid>/wayland-0` through the core's AF_UNIX sockets; per
  client an object table with libwayland's limits (4096-byte messages,
  28 descriptors, 4096 objects) and every malformed or unknown request
  answered with `wl_display.error` and a disconnect; wl_compositor 6,
  wl_shm 2 (ARGB8888, XRGB8888, pools on the client's memfd),
  xdg_wm_base 6 and wl_output 4; an xdg_toplevel's configure and ack,
  then each commit's damaged rows copied from the memfd into a shasha
  window titled by `set_title`, the buffer released and frame
  callbacks done at once; `delete_id` for every destroyed object;
  `events=wayland`, the trace, the ledger and the state dump. Host
  tests for the protocol state (`cargo test -p kernel --lib wayland`).
- Step 2's server, second part: input and window control (`lx_wl_input`,
  `lx_wl_input_pie`: user/lx/wl_input_test, its transcript written from
  the protocol's specification since weston's headless backend has no
  input devices; driven by QEMU's PS/2 mouse and keyboard from a screen
  check on a headless desktop, which also finds the windows' pixels
  inside shasha's frame and frameless): wl_seat 7 with wl_pointer
  (enter, leave, motion, BTN_LEFT/BTN_RIGHT, the wheel as axis_source,
  axis_discrete and axis, frames) and wl_keyboard (gala's US keymap in
  XKB text as a sealed read-only file passed with SCM_RIGHTS,
  repeat_info, enter/leave following the desktop's active window, evdev
  key codes from the scan codes, modifiers), the desktop's input taken
  by the server as a kernel listener of the client's process
  (`winstate::Listener::Server`, with gala's GALA_WM_ACTIVE and
  GALA_WM_POINTER_LEFT notices); `xdg_toplevel.close` from the frame's
  close button; zxdg_decoration_manager_v1 (server-side and framed
  unless the client asks for client-side: frameless). Pings are not sent
  (no window is marked not responding yet). `events=wayland`'s
  `wayland input`, the trace's `input:` lines, the ledger's `wayland
  window action missing`, the seat in the state dump.
- Step 2's server, third part: configure from the desktop (wl_input_test's
  Snap steps in `lx_wl_input`): the desktop's maximize (SC_MAXIMIZE: the
  caption's button, Win+Up, `gala maximize`), restore (SC_RESTORE) and
  place (GALA_WM_PLACE: Snap, back from Snap, `gala place`) become an
  `xdg_toplevel.configure` of the surface size (the work area or the
  rectangle less shasha's frame) with the states maximized, activated
  (following the desktop's active window, a configure at each change) and
  tiled_left/right/top/bottom (xdg_toplevel 2), and the window moves to
  the desktop's place and takes the buffer's size at the commit after
  the client acks it; an ack may name any configure not yet acked (at
  most 16 kept); `set_maximized`, `unset_maximized` and `set_minimized`
  are the desktop's actions on the window; wm_capabilities maximize and
  minimize. `events=wayland`'s `wayland configure`, `placed` in `wayland
  commit`.
- Step 2's server, fourth part: popups (`lx_wl_popup`, `lx_wl_popup_pie`:
  user/lx/wl_popup_test, recorded against weston's headless backend;
  and wl_input_test's popup steps in `lx_wl_input`, written from the
  specification): xdg_positioner (every request at version 6, each
  checked: `invalid_input`; an incomplete one is `invalid_positioner`)
  and xdg_popup as a frameless popup window above its parent (user32's
  menus' `STYLE_POPUP`), placed from the anchor rectangle, anchor,
  gravity and offset against the parent's window geometry
  (set_window_geometry now applied at commit) and adjusted against the
  work area: flip, slide and resize on each axis in the protocol's
  order; configure and ack as toplevels, reposition (version 3);
  xdg_popup.grab with the serial of the last button or key press (else
  denied: popup_done at once): the topmost grabbing popup has the
  keyboard focus while the toplevel stays activated, the desktop's popup
  grab (`State::popup_grab`) ends at a press outside the client's
  windows, which goes nowhere else (GALA_WM_POPUP_DONE), or when the
  window loses the activation, dismissing the grabbing popups topmost
  first; only the topmost grabbing popup may be destroyed
  (`not_the_topmost_popup`); popups without a grab go in any order, a
  child outliving its parent popup (as weston). `events=wayland`'s
  `wayland popup`, `events=window`'s `popup grab ended`, the ledger's
  ignored set_reactive, set_parent_size and set_parent_configure, the
  grabs and popups in the state dump.
- Step 2's first real client: `weston-simple-shm` (`lx_simple_shm`:
  weston 13's demo, Ubuntu 24.04's build with its libwayland-client and
  libffi, `PREBUILT`s, on glibc) draws its animated frames on a headless
  desktop (the screen check: many colours in its window) and ends at Esc, its transcript real Linux's (against
  weston's headless backend). Built for it: fallocate (`posix_fallocate`
  of its buffer file; memfd_test, recorded on Linux: allocate, KEEP_SIZE,
  PUNCH_HOLE, the seals, the errors); frame callbacks done at the 60 Hz
  refresh after their commit instead of at once (a client drawing on its
  callbacks no longer redraws as fast as the CPU allows); the
  connection's summary line (requests, commits shown, frame callbacks
  done).
- Step 2's server, what GTK 3 needs of it (host tests in
  `kernel/src/wayland/tests.rs`): `wl_data_device_manager` 3 (GTK 3
  makes its seat only once the global is there): data sources and
  devices, `selection` (nil) before each keyboard focus, the client's
  own selection kept and the replaced one cancelled, `start_drag`'s
  source cancelled at once (ledgered), the protocol's errors; a toplevel
  without a decoration object is client-side, frameless (xdg-decoration:
  a client that does not negotiate decorates itself; GTK 3 never binds
  the manager); a frameless window shows its window geometry, so a
  client's translucent shadow outside it is not shown as black while
  alpha is not composed (ARCHITECTURE "Frameless windows").
- Step 3, the first GTK program (`lx_gtk`, `lx_gtk_pie`:
  user/lx/gtk_test, a GTK 3 window with a label and a button, linked
  against Ubuntu 24.04's libgtk-3.so.0 by build.sh's GLIBC_TESTS, its
  transcript recorded on Linux by lx-expect against weston on Xvfb, a
  compositor with a seat): GTK 3 and its whole library closure (58 of
  Ubuntu's packages, from GLib, Pango, cairo and HarfBuzz to libX11) as
  a Linux root assembled from the release's packages (xtask's
  `LINUX_ROOTS` "gtk3", each .deb's SHA-256 from the signed index, the
  downloads in a shared package cache), with what an installation adds:
  the MIME database's mime.cache (gdk-pixbuf picks its PNG loader by
  MIME type through GIO), the xkb-data GDK's default keymap reads, DejaVu Sans as
  the one font. On a headless desktop its window is drawn frameless with
  GTK's own title bar (`GTK_SCREEN` checks GTK's pixels), a PS/2 click on
  its button prints its line, a click on GTK's close button closes it,
  and its transcript is Linux's; the `_pie` scenario clicks itself, as
  on Linux. Built for it: the server's part above; and what GTK's start
  reached, each in a corpus test recorded on Linux: eventfd and eventfd2
  (`lx_eventfd`: GLib's main loop wakes its threads with one), prctl's
  thread name and capability bounding set with getresuid and getresgid
  (`lx_prctl`), statfs and fstatfs (`lx_statfs`). Its ledger after:
  rseq and SA_ONSTACK (below), and the Wayland requests accepted and
  not used (regions, set_cursor, min and max sizes, set_parent).
- Step 3's first part, Windows' clipboard as Windows has it
  (`clipboard_test`: two processes, its expectations Windows'
  documented behaviour; ARCHITECTURE "The clipboard"): one per window
  station in the kernel (`nt::clipboard`, the GalaClipboard* services
  0x7fc0-0x7fc7), OpenClipboard's lock across processes
  (ERROR_ACCESS_DENIED, GetOpenClipboardWindow), the owner
  (GetClipboardOwner after EmptyClipboard), HGLOBAL formats copied
  through the kernel and charged to the setter's paged pool quota
  (GetClipboardData's handle made per process, freed by user32 once the
  data changes), the synthesized text formats and CF_LOCALE in
  Windows' enumeration order, the sequence number, the format listeners
  (WM_CLIPBOARDUPDATE posted across processes), `events=ipc` and the
  state dump's clipboard line.
- Step 5, X11 programs through rootless Xwayland (`lx_xterm`;
  ARCHITECTURE "X11 programs through Xwayland"): `xwayland=on` gives
  display `:0`, held by the Wayland server, whose first client starts
  Ubuntu 24.04's Xwayland (the Linux root "x11": Xwayland, xkbcomp,
  xterm, dash as /bin/sh and their libraries; `-rootless -shm`, the
  listeners and the window manager's connection handed over), and the
  server's own X window manager (`wayland::xwm`, host tests: Composite
  redirection, MapRequest, titles, `WL_SURFACE_ID`, focus, close) gives
  each top-level X window's surface the `x11` role: xterm is a framed,
  titled window of the desktop running dash on a pty; the screen check
  types `echo hi` and sees its output, drags over it with the mouse
  (xterm's selection) and types `exit`, which ends it. A GTK program
  under X11 (`lx_gtk_x11`: gtk_test with GDK_BACKEND=x11, its
  transcript Xvfb's on Linux) shows its window; the screen check clicks
  its button and the frame's close button (WM_DELETE_WINDOW through the
  window manager). Built for it:
  pseudo-terminals, sessions and process groups (`lx::pty`,
  `lx::session`; user/lx/pty_test's transcript, recorded on Linux, now
  matches: `lx_pty`, `lx_pty_pie`), glibc's fork (clone with
  CLONE_CHILD_SETTID, CLONE_CHILD_CLEARTID, CLONE_PARENT_SETTID),
  setresuid and setresgid, Xwayland's output kept out of the program's
  transcript (`lx: pid N service:`), and two Linux roots shipped to one
  directory.

**Next**:
1. Step 1's rest, before real applications need it:
   - the program and its interpreter mapped as file views (shared text
     across processes, as the libraries already are) instead of copied
     at exec;
   - rseq (glibc registers one; ENOSYS, ledgered, until the scheduler
     keeps its cpu_id and aborts its critical sections), shrinking a
     mapped file (ramfs refuses it while mapped: EBUSY, ledgered);
     sigaltstack (glibc 2.39 installs its SIGRT_1 handler with
     SA_ONSTACK, ledgered: without an alternate stack the current one is
     used, as on Linux, so it changes nothing until a program sets one);
   - Linux roots for more programs (`LINUX_ROOTS`: today GTK 3's), and
     what their programs reach, from the ledger (`linux syscall
     missing`).
2. The Wayland server's rest (its first part is done, above; each
   step test first, in wl_test or a client like it recorded against
   weston):
   - the `resizing` state and `xdg_toplevel.resize`/`move` (an
     interactive resize from a client-side frame through the desktop's
     `begin_resize`/`begin_drag`; ledgered as ignored today), fullscreen,
     a decoration mode change on a mapped window (the frame
     put on or taken off in place; today the mode is fixed at mapping),
     the middle button and horizontal wheel once the desktop has them;
   - popups' rest: reconstraining a reactive popup when its parent
     moves or resizes (set_reactive, set_parent_size and
     set_parent_configure are accepted and ledgered today), a grab per
     connection rather than per process on the desktop (two connections
     of one process count as one owner), touch;
   - `xdg_wm_base.ping` on a timer once the desktop marks windows that
     do not answer (none, Windows' or Linux's, are marked today);
   - frame callbacks paced by the presenter's own refresh (a 60 Hz
     tick of the server's today, not the display's vblank), the
     server's allocations charged to the client's quota;
   - a socket per session's desktop under its user's `/run/user/<uid>`,
     with `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` given to the Linux
     programs started there (today one socket for the boot user, the
     environment passed by whoever starts them; the server is on by
     default where the desktop has a shell, its runtime directory in
     `/run`, the Linux view's tmpfs, `lx_run`);
   - more of weston's demos as clients (simple-damage: buffer scale
     and transform, viewporter; eventdemo: input through libwayland),
     each a `PREBUILT` member of the same package;
   - drag-and-drop (the clipboard is step 3 below); alpha composed by shasha (translucent
     windows, client-side shadows and rounded corners, which a
     frameless window's view leaves out today).
   Still open in the prerequisites, ledgered: a datagram socket's
   connect and sendto by name, SCM_CREDENTIALS with SO_PASSCRED, rights
   on a MSG_PEEK, collecting rights cycles in flight.
3. **One clipboard for both personalities** (the owner: "would be
   amazing"), test first at each layer:
   - Windows' clipboard's open ends (its first part is done, above),
     ledgered where a program reaches them: delayed rendering (SetClipboardData
     with NULL is kept, WM_RENDERFORMAT and WM_RENDERALLFORMATS are not
     sent), GDI handle formats across processes (CF_BITMAP,
     CF_ENHMETAFILE, CF_PALETTE stay the setter's; the bitmap and
     metafile synthesized formats CF_DIB, CF_DIBV5, CF_METAFILEPICT are
     not made), registered formats are per process
     (RegisterClipboardFormat shares user32's table of registered
     messages: a station-wide atom table is to come), the viewer chain
     (SetClipboardViewer, WM_DRAWCLIPBOARD), WM_DESTROYCLIPBOARD posted
     rather than sent, CF_LOCALE's code pages (user32 converts text
     through the system's ANSI and OEM code pages); `clipboard_test`
     passes on CI's Windows (`user/windows/ci-passing.txt`);
   - the Wayland selection across clients: data offers to the focused
     client, `receive` passing its descriptor to the source's `send`;
     a two-client test against weston;
   - the bridge: the session's clipboard is one; a Wayland selection
     offers Windows the formats it maps (`text/plain;charset=utf-8` as
     CF_UNICODETEXT, `image/png` as a PNG format and CF_DIB later) and a
     Windows copy offers Wayland clients the same MIME types, the data
     converted when pasted (delayed rendering on both sides); a
     scenario copying from a GTK window and pasting into a Windows edit
     control, and back. What it needs from the kernel clipboard, beyond
     the seams there (`Clipboard::offer`: the Wayland server puts text
     on the clipboard as an owner without a window, refused while a
     program has it open; `Clipboard::text`; the sequence number to
     notice a Windows copy): delayed rendering first (a Wayland source
     offered as formats with no data, rendered by the server's `send`
     when a Windows program asks: the kernel waiting for the bytes
     instead of WM_RENDERFORMAT), a kernel-side listener (the server
     told of a Windows change as WM_CLIPBOARDUPDATE tells windows), the
     session's interactive station (WinSta0) as the one bridged, and the
     text conversion user32 does today (UTF-8 to CF_UNICODETEXT) on the
     kernel side.
4. Step 3's rest: an application as a distribution ships it (GTK 3, the
   owner's choice for now: gnome-calculator or gedit's GTK 3 builds),
   with its GSettings schemas, icon theme and keyboard input checked on
   the screen; then a Qt one; GTK 4 later. A session gives Linux programs
   their environment (`HOME`, `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`: the
   scenarios pass them today; who the program is comes from its token,
   `lx::identity`) and has no D-Bus session bus yet (GTK goes
   without it: accessibility and portals are skipped).
5. X11 programs through Xwayland: rootless Xwayland, xterm and a GTK
   program under X11 are done (above); next, a distribution's GTK
   application under X11. Open, each test first: SysV and
   POSIX shared memory (MIT-SHM; nothing has reached it yet),
   `/proc/<pid>/cmdline` (Xwayland names its clients by it), the X
   selections and the clipboard bridged to the desktop's,
   `_NET_WM_STATE` (maximize, full screen) for X windows,
   `xwayland_shell_v1` instead of `WL_SURFACE_ID`, a stop for SIGTSTP
   (job control), a recorded glibc fork test of the clone flags.
6. A distribution's desktop (the owner; its first part done: `lx_xdesktop`,
   its screen check passing: user/lx/xdesktop's
   session script runs rootful Xwayland on `:1` as one window "Xwayland
   on :1" framed by shasha (`-decorate`: libdecor takes the server's
   decorations), twm managing an xterm in it, typed into from the
   desktop; `exit` ends the session; seen on a manual boot. Maximised
   from the desktop (Win+Up in the screen check, as the caption's button
   or `gala maximize`): the server's configure of 1278 x 718, maximized,
   and Xwayland's X screen follows the window's size (its root fills the
   window, twm and the xterm redrawn in place). twm is the
   lightest X desktop that shows the path; Xfce needs a D-Bus session
   bus, xfconf and over a hundred packages, a later step. Next:
   full screen (xdg_toplevel.set_fullscreen, ignored today), then
   Xfce): rootful Xwayland as one window
   running a light desktop session (Xfce), which can be maximised or
   put full screen. Full screen is a choice of presentation, not of
   speed: both paths copy the session's damaged pixels in software.
   Screenshots at each step.
7. Linux distributions in shasha's start menu (the owner; built:
   `linux_menu`, ARCHITECTURE "Start menu"): a "Linux distributions"
   section of suggested distributions (Ubuntu, Alpine), collapsible,
   each suggestion dismissable and the whole section hideable, the
   choices kept in the user's hive; a suggestion installs on click
   (`gala linux-install`: its rootfs fetched over winhttp and imported
   through `wsl --import`, the progress on its tile); an installed one
   starts step 6's desktop; its context menu opens a terminal (`wsl -d
   <name>` in a console window) or one of its applications (its
   `.desktop` files), X and Wayland programs reaching the desktop
   through WSLg's environment and a socket node in the distribution's
   tree. The scenario's distributions are stand-ins served by the host.
   Open: the desktop session is gala's script, which a real
   distribution lacks (it should come from gala, through `/mnt/c` or
   WSLg's `/mnt/wslg`, once CT step 3 mounts them, with Xwayland
   installed in it or provided), the persistence across a reboot checked
   by a two-boot scenario (today by `persist_test`'s registry part and
   linux_menu_test's reading of the keys), import progress (`wsl
   --import` reports none), cancelling an install, icons from the
   `.desktop` files' `Icon=`.
8. Linux Chromium beside Windows Chromium (BR's): a distribution's
   Chromium (or Google's Linux build) with `--ozone-platform=wayland
   --no-sandbox` in a boot scenario, its window on the same desktop as
   `chromium_window`'s, both browsers running at once and loading a page
   from the guest's nginx. Expected blockers, each a Linux test program
   recorded on real Linux first: `/proc` files Chromium reads
   (`/proc/self/maps`, `status`, `stat`, `/proc/meminfo`), eventfd,
   timerfd, inotify, `/dev/shm`, fontconfig and fonts, D-Bus optional
   (absent: Chromium carries on without it). Then its sandbox
   (user namespaces, seccomp-bpf; shares CT's namespace work) so
   `--no-sandbox` goes, and the clipboard between the two browsers
   (`wl_data_device` bridged to the Windows clipboard).

**Done when** (each step): its test and scenario pass, and the Linux
programs' transcripts match real Linux's.

**Design**: ARCHITECTURE "Linux applications on the desktop
(Wayland)": the server in the kernel beside the compositor for now
(moving to user mode with SH), `wl_shm` pools as memfd sections,
surfaces as `winstate` windows wearing shasha's frame through
xdg-decoration, one socket per session, the limits on client input,
`events=wayland` and `wayland=trace`.

### AR. ARM64 port

**Goal**: gala boots and runs on ARM64 (QEMU's `virt` machine first),
on the seams item PT prepared: native ARM64 Windows programs
(`aarch64-pc-windows-msvc` PE images) and ARM64 Linux programs.

**Next**:
1. The kernel's `arch` for aarch64: boot through Limine's aarch64
   protocol on UEFI, the exception vectors, the GIC, the generic timer,
   VMSAv8 page tables behind `arch` (`mm::paging`'s format moves there),
   the context switch, PSCI for the other CPUs, the PL011 UART. Test
   first: the boot scenario `hello` reaching its kernel line under
   `qemu-system-aarch64`.
2. User mode: Windows' ARM64 ABI (`nt::abi::arm64`: CONTEXT, the system
   call convention, unwinding) and gala's DLLs built for ARM64; Linux's
   aarch64 ABI (`lx::machine::aarch64`, its system call table).
3. CI: the aarch64 boot scenarios on GitHub's ARM64 runners (free for
   public repositories) or under QEMU's emulation.

**Done when**: the boot scenarios of the native ARM64 test programs pass
on aarch64 in CI.

### XT. 64-bit Windows programs on ARM64

**Goal**: x64 Windows programs run on ARM64 gala (the owner: needed,
since most Windows programs ship for x64 only), through dynamic binary
translation as Windows' own Prism does: x64 code translated to ARM64
when it first runs, and cached. After AR runs native programs.

**Next**:
1. An interpreter of user-mode x86-64 for correctness first (the x86-64
   manuals and Arm's Architecture Reference Manual as the sources); its
   tests are the existing x64 test programs.
2. A block-at-a-time JIT to ARM64 with a translation cache, lazy flags,
   x87/SSE on NEON, self-modifying code caught through write protection,
   and x64's strong memory ordering kept on ARM64's weaker one (the
   classic source of multithreaded bugs in translators).
3. The boundary, measured before choosing: x64 DLLs loaded as on x86-64
   and the system call boundary translated (the NT layer stays native),
   or gala's system DLLs native with translated code calling into them
   (Windows' documented ARM64EC/ARM64X design).

Open-source translators (FEX-Emu, Box64; MIT) may be read, and anything
borrowed is recorded in THIRD_PARTY; the clean-room rule is about
Windows' code.

### AN. Android applications

**Goal**: Android applications run on gala, on the LX personality:
x86_64 Android images (AOSP's emulator builds) first, ARM64 after AR.

**Next** (each test first against real Android where it applies):
1. LX far enough for Android's userspace: binder (the IPC driver),
   ashmem/memfd, bionic's needs, Android's init and property service.
2. ART running a command-line Java program (dalvikvm), then the
   framework's system server.
3. Graphics: gralloc and EGL over SwiftShader (Android's own software
   renderer), SurfaceFlinger's output as a shasha window.
4. A first application: an APK from AOSP's samples, its window and input
   checked on the screen.

AOSP is open source (Apache-2.0; the kernel GPL-2.0; CLEAN_ROOM
"Linux"). Needs LD's dynamic linking and most of LX.

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
  file thunks and GalaCreateProcess's image, Sysnative, the documented
  exemptions, per-thread
  RtlWow64EnableFsRedirection(Ex); PROCESSOR_ARCHITECTURE=x86,
  PROCESSOR_ARCHITEW6432, ProgramFiles and CommonProgramFiles the (x86)
  directories, ProgramW6432, `C:\Program Files (x86)`. **The 32-bit
  kernel32** (`wow64_k32_test`, a Rust program; `wow64_c_test`, a C
  program built by clang with stdcall imports through an import library,
  not large-address-aware): gala's kernel32 built for i686, shipped as
  `C:\Windows\SysWOW64\kernel32.dll` (`testkernel32_32.dll`; kernelbase
  beside it, `testkernelbase32.dll`, the core API sets' host), what
  differs by architecture in its `x64.rs`/`x86.rs` (TEB access, the TEB,
  PEB, process parameter, CONTEXT and optional header offsets,
  SLIST_HEADER, the new thread's CONTEXT) and the structures with
  pointer-sized fields typed (`repr(C)` with `usize`: the basic
  information classes, MEMORY_BASIC_INFORMATION, OVERLAPPED,
  STARTUPINFO, SECURITY_ATTRIBUTES, EXCEPTION_RECORD, toolhelp's
  entries, psapi's counters, the processor information); GetSystemInfo's
  32-bit answers (PROCESSOR_ARCHITECTURE_INTEL, the process's 2/4 GiB
  range), GetNativeSystemInfo's AMD64, GetSystemDirectory's System32
  (redirected to SysWOW64 below kernel32, as on Windows),
  GetSystemWow64Directory's SysWOW64,
  Wow64Disable/Revert/EnableWow64FsRedirection (their effect on files and
  on CreateProcessW's image); wow64.dll converts
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
1. Stage 3's rest, the 32-bit system DLLs: kernel32_test built for x86 as a second scenario (xtask builds a package for one target per
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
   x86 too (it needs a 32-bit shell32 for CommandLineToArgvW). Also: the
   redirector for names relative to a RootDirectory handle (it rewrites
   full NT paths and GalaCreateProcess's image only); 32-bit builds of
   the System32 programs a 32-bit program may start by their System32
   path (cmd.exe and the rest: redirected to SysWOW64, where gala has
   none yet); nested exceptions and collided unwinds in the x86
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
   shell32's SHPathPrepareForWriteW, riched20/msftedit and sfc
   for i686.
3. Two Windows 11 measurements (issue #8) that change how threads and
   static TLS start in a 32-bit process. **NtCreateThread** (the old call
   with a CONTEXT and an INITIAL_TEB) is refused with
   STATUS_ACCESS_DENIED in a WOW64 process, by syscall and by export;
   NtCreateThreadEx and kernel32's CreateThread work. gala has no
   NtCreateThreadEx yet and its 32-bit kernel32 creates threads with
   NtCreateThread. So: NtCreateThreadEx in the kernel (the kernel
   allocates the stack and TEB, and the thread starts at ntdll's
   RtlUserThreadStart), kernel32 moved onto it (64-bit too, as Windows
   does), then NtCreateThread refused for WOW64 and `wow64_thread_test`
   creating its thread with NtCreateThreadEx (checking CS, TEB, TID and
   stack bounds from inside the thread). **testdll32's static TLS** is
   never set up on Windows: no TLS callback, `_tls_index` never written,
   by static import and by LoadLibrary alike, although its TLS
   directory and relocations are well formed, so `wow64_import_test`
   check 12 fails there. The 64-bit tlsdll.dll is the same on a Windows
   Server 2025 runner: its TLS callback runs, yet its first TLS value
   reads 0 where the template says 0x5050 (tls_test check 13). Still to
   explain before gala changes:
   testdll.dll (x64) has no TLS directory at all, so the 64-bit build is
   not even linking `_tls_used`; the question is whether the DLLs'
   build (lld, `_tls_used`) or the loader is what differs.
4. Later: the handover carrying WOW64 processes; gala-um running 32-bit
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
   taskbar list (CLSID_TaskbarList), the Restart Manager's sessions
   shared between processes (rstrtmgr.dll, ARCHITECTURE "Other DLLs"),
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
1. quota_test on real Windows: the socket part (SO_RCVBUF read back,
   connected sockets' charge); the handle limit's status and count are
   measured (ERROR_NO_SYSTEM_RESOURCES at 0xff0000).
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
4. The update feed (the owner): releases published to object storage as
   static files in TUF's format, so any bucket, CDN or mirror serves
   them untrusted; one rolling channel, no LTS (the owner: none unless
   paid for). Done: the metadata, writer and client checks (`user/tuf`,
   host tests for every refusal: expired timestamp, rolled-back
   versions, tampered metadata and blobs, keys not in root, root
   rotation), the client's whole flow over a feed and a store the caller
   provides (`tuf::update`, host tests: a first update, a quiet feed, a
   release fetching only what changed, a new root; a tampered blob, a
   frozen feed, a rollback and a missing file each refused with nothing
   kept), `cargo xtask feed publish|refresh|check` (check runs that
   flow), checked against python-tuf 6.0.0; CI publishing
   (`.github/workflows/release.yml`): every commit on main whose CI
   passes is published to the R2 bucket as the next release (the
   release tree, `cargo xtask release-tree`: kernel, system DLLs,
   Limine, sealed config, manifest), refreshed every 6 hours (targets
   too when near its expiry), root renewed monthly (`feed renew-root`);
   every key, root included, from the `GALA_FEED_KEYS` secret (`feed
   keygen`); `feed pull` / `feed push` move only what changed, timestamp
   last, never overwriting a published version; each run checks the
   result as the client does before and after the upload. Host tests
   for each piece and for the workflows' steps against a directory
   standing in for the bucket (ARCHITECTURE "The update feed"; WORKFLOW
   "The update feed"). Every key lives in CI's secrets (the owner's
   decision, for now): CI can rotate keys and re-sign root, at the
   price that whoever controls CI controls the trust chain; root's
   validity is short (90 days, re-signed monthly; targets 30 days,
   snapshot 7, timestamp 1), so a later move of root offline is one
   rotation that every client takes within weeks. Next:
   - The first real run: the owner's one-time setup (WORKFLOW "The
     update feed"), then a publish that proves what the local tests
     cannot (R2's 404 for an absent file at the public URL, `aws`
     against R2's endpoint, the head-object guard, the job summary).
   - The updater in gala (test first): a scenario where xtask serves a
     feed from a local directory (a fake bucket) and gala's updater
     (`tuf::update` over winhttp and C:, the trusted root in the image)
     takes a release, and hands the staged release to R9's live update;
     raises the rollback floor after the self-check. Refusal scenarios
     for an expired timestamp, a rolled-back targets version, a
     tampered blob and a key not in root. Every step and refusal as
     events.
   - The release tree's GALAMANI manifest signed with a release key
     and numbered as the feed's release (today the runner's throwaway
     test key and release 1, so the kernels of two releases do not
     trust each other's manifests; item 3), or derived from targets /
     replaced by it: one signed list of a release's files, not two.
   - Root keys offline and a key rotation (`feed` can only renew root
     with the same keys today).
   - A staged rollout field in targets (a share of machines by
     machine-id hash).
   - Out of it: distributions inside LX update themselves (their own
     package managers); programs from `gala install` keep winget's
     vendor-pinned installers (PK).
   - Open: a custom domain or CDN in front of the R2 bucket (the public
     URL is R2's development URL, rate-limited; `FEED_URL` in
     release.yml); where the release key lives.

WORKFLOW "Signed images".

#### R9. Live update, kernel included

**Goal**: user mode first (versioned system images behind an atomic
pointer, old processes restarted one at a time), then the kernel
handover: the running kernel stops every thread at a quiescent point,
writes its state as versioned data, and a new kernel rebuilds it and
reissues blocked calls, so programs carry on. Live patching of single
functions is not part of live update; its one form is SN steps 1d and
1e's hot swap of gala's DLLs and kernel into a resumed test snapshot, a
development tool whose runs are never real.

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

### SN. Snapshots: whole machines, then single programs

**Goal**: save a running state and start from it again, as often as
wanted. For testing, a long scenario is reached once (the Office
install's 30 minutes to App-V's manifest merge), and each try at a fix
starts from there in seconds; one snapshot can be restored several
times to branch (try two fixes from the same state). For users,
`gala snapshot` saves a program and `gala restore` brings it back,
on the same machine or, exported, on another.

**Step 1: whole-machine snapshots in xtask.** Done
(`xtask/src/snapshot.rs`, WORKFLOW "Machine snapshots"; scenario
`snapshot_machine`, host tests `snapshot::tests`). `boot-test <scenario>
--snapshot-at <line> --snapshot-name <name> [--snapshot-stop]` saves the
machine when the line comes in its serial log (the ledger's lines are
there too): over QMP, `stop`, `blockdev-snapshot-sync` (the disk image,
frozen, moves into the snapshot; the run goes on in a new qcow2 layer),
then a migration of memory and devices through gzip. `boot-test
--from-snapshot <name>` starts QEMU with `-incoming` from it, on a qcow2
layer of its own over the frozen disk, and judges what comes after the
resume. `cargo xtask snapshots [rm <name>]` lists and deletes them
(`target/snapshots/<name>/`: manifest, state, disk, the log up to the
save). A snapshot resumes only on the build it was taken from: the
manifest names the SHA-256 of the kernel, every boot module and the
command line, QEMU, the accelerator, memory and processors, and a
difference is refused with each one named. That is exactly what the
saved memory holds: gala's DLLs and test programs are boot modules
loaded into memory by Limine, not files on the disk, so a changed DLL is
refused as a changed kernel is. No kernel change was needed: the
guest's clocks stop with the machine and go on from the saved values
(no jump; only the RTC follows the host, and the kernel reads it only
after boot), so its wall clock is behind the host's by the time saved
(ARCHITECTURE "What a guest's clock follows"); the host-side clock check
leaves the save's pause out. `snapshot_machine` runs in `cargo xtask
test` (about 20 s under TCG). Connections to the host and the internet
do not survive a resume (local sources are fine); a resumed run is not
a replay (R5 is the deterministic tool); snapshots never replace a full
run in CI. Limits: one scenario booted alone, BIOS boots, not a
snapshot of a resumed run.
Measured on `office_install_local` (TCG, with #117's msxml6): saved at
`process integrator.exe (pid` (1675 s into the run, just before App-V's
manifest merge) in 5.2 s, 117 MiB of state and the 7.6 GB disk (in
`/dev/shm`); resumed in 3.1 s, the run reached the install's end (the
service's IXMLDOMNode::replaceChild, ROADMAP OF) in 21 s of run time,
where the run from boot took 1706 s. The service had loaded
`System32\msxml6.dll` at about 42 s (`events=image`), so no snapshot
past the first minute can take a changed msxml6.dll as a module, even
once DLLs are files on the disk: a resumed run can only take a module no
process has mapped yet, or patch one into the running programs (step
1d, at the user's risk).
Next (step 1c): resuming with other DLLs. Since they are boot
modules, a snapshot cannot take a fixed DLL today; for fixes in DLLs
the Office install has not loaded yet at the save, the boot modules
would move to the disk (the open question in ARCHITECTURE "Chain of
trust"), and a resumed run's own layer would take the new files (same
place on the disk; the kernel's file tree in memory names their size and
clusters, so a file that grows needs the kernel's help, a boot module
the kernel reads at a resume). Until then a snapshot is taken with the
build the fix is tried on (the fix in a stage after the save) and
resumed as often as the stage needs.

**Step 1b: a failing CI run leaves its snapshot.** When a scenario
fails or times out in CI, xtask asks QEMU to save the VM before ending
it (the stuck state itself: every thread, what it waits on, who holds
what; a hang or a leak shows its cause instead of only where it
stopped). The artifact holds the snapshot, compressed, with the build
it needs: the kernel, the boot image and the test binaries.
`cargo xtask restore-ci <run> <scenario>` downloads it and resumes it
locally, paused, with the monitor and a debugger attached; CI under KVM
and a local TCG run differ in timing, so a failure that does not
reproduce locally is still inspected as it happened. Bounds: only on
failure, a size cap per artifact, a few days' retention (GitHub's
artifact storage quota is the limit), at most one snapshot per job. The
guest holds test data only, and must stay so: artifacts are as visible
as the repository. From step 2 on, a faulting program is snapshotted at
the fault, before it ends (a minidump that can be resumed), which is
much smaller than the machine.

**Step 1d: hot swapping gala's DLLs into a resumed snapshot (at your
own risk).** Done (`xtask/src/hotswap.rs`, `kernel/src/hotswap.rs`,
`kernel/src/arch/x86_64/fwcfg.rs`; WORKFLOW "Hot swapping DLLs into a
snapshot (at your own risk)", ARCHITECTURE "Hot swapping DLLs into a
resumed machine"; scenarios `hotswap_machine`, `hotswap_stack` and
`hotswap_wow64` on `hotswap_test` (and its 32-bit build) and
`hotswap_dll`; host tests `hotswap::tests` in xtask and the kernel). A fix to a DLL is tried in seconds from a snapshot
taken just before the failure: `boot-test --from-snapshot <name>
--hot-swap-unsafe <dll>[,<dll>...]`. xtask compares the snapshot's build
of each DLL (the snapshot keeps its DLLs and lld's maps) with the new
one function by function, references normalized (decoded code,
relocations, jump tables), and makes a plan; the resumed machine gets it
through fw_cfg with an NMI before its first instruction, stops every
program thread, checks that no changed function is on any thread's
stack, maps the new code beside the image in every process that has
it, bound to the old image's statics, functions and import table, and
makes each changed function's old entry jump to its new version (old
vtables and stored pointers land in new code), then replaces the file
for later loads. Refused with the reason: a changed function on a
stack, a static that changed size, anonymous writable data, a new
import, changed thread-local data, code that does not decode or
relocate, a function too short for the jump, changed identical-code
folding, any other change to the build. Never a real run: a banner,
`hotswap:` lines, a ledger entry and an event, marked crash reports,
state dumps and triage, the verdict `hot-swapped (not a real run)` with
exit status 3, refused in CI; every fix still gets a full run from boot
before its pull request. Not checked (the user's risk): state the old
code built that the new code reads differently, invariants of data
whose layout did not change. Limits: no unwind data for the new code,
new exports invisible to GetProcAddress, `run=` scenarios only.
Shown on the Office install (`office_install_local`, TCG): saved at
`process integrator.exe (pid` (1827 s in; 118 MiB of state, the 7.6 GB
disk), resumed with msxml6.dll's `replaceChild` changed (a new ledger
name): xtask patched that one function in the 64- and the 32-bit build;
the guest stopped 29 threads, patched both OfficeClickToRun.exe
processes (64-bit) and setup.exe (WOW64, the blob below 2 GiB), and the
service reached the new code (`msxml6.dll!IXMLDOMNode::replaceChild
(hot-swapped build)` in the ledger) in a resumed run of 22 s, where the
run from boot needs 30 minutes to get there. A first try found two
faults, fixed with `hotswap_wow64` and the all-or-nothing rule: the
32-bit blob's reservation was not at a 64 KiB boundary, and a later
DLL's refusal left an earlier one swapped.

**Step 1e: hot swapping the kernel into a resumed snapshot (at your own
risk).** Done (the DLL swap's code with `Image::from_elf` in
`xtask/src/hotswap.rs`, `KernelPatch` in `kernel/src/hotswap.rs`,
`kernel/src/arch/x86_64/hotpatch.rs`, `smp::stop_others`,
`sched::kernel_stacks`; WORKFLOW "Hot swapping DLLs and the kernel into a
snapshot (at your own risk)", ARCHITECTURE "Hot swapping the kernel into a
resumed machine"; scenarios `khotswap_machine`, `khotswap_stack`,
`khotswap_both` on hotswap_test's kernel modes and the kernel's probe
(`nt::hotswap_probe`); host tests in xtask and the kernel). A kernel fix
late in a long run is tried from a snapshot: `--hot-swap-unsafe kernel`,
or `kernel,msxml6.dll` (one plan, all or nothing). The kernel is linked
with `--emit-relocs` and lld's map; xtask diffs the snapshot's kernel
with the new build function by function as it does a DLL (its merged
constants and GOT split, relocations' symbols, sign-extended addresses,
absolute jump tables, assembly refused). The guest applies it last,
after the DLLs: the new code mapped in a window beside the image
(0xffffffffe0000000, W^X), every other CPU stopped, every kernel stack in
use and every interrupt stack scanned, then each changed function's
entry made a jump through the direct map's alias. Refused, naming the
symbol: a changed function on a kernel stack (or the patch's own), a
static that changed size, assembly, a function too short for the jump,
any other change to the build. Never a real run, marked as step 1d's
runs are. For a diff to see only the edit, the kernel's dev profile has
a codegen unit per module (`codegen-units = 1024`; rustc's 256 merged
modules by size, and any edit moved other code between units) and the
scenarios' kernel variants are chosen by a cfg, not a feature (a feature
changes every TypeId). Not checked (the user's risk): a structure on the
heap whose layout changed (only statics' sizes are compared), state the
old code built that the new code reads differently. Limits: no symbols
for new kernel code, one kernel swap per boot, the dev build (a release
kernel's 16 codegen units see unrelated changes), `run=` scenarios.
Shown on the Office install (`office_install_local`, TCG): saved at
`process integrator.exe (pid` (1755 s in; 116 MiB of state, the 7.7 GB
disk), resumed with `nt::process::finish`'s exit line changed: xtask
patched that one function (and its new string); the guest scanned 35
kernel stacks and 6 interrupt stacks with the other CPU stopped for 14
ms, and every later exit line came from the new code, the install
reaching its end (msxml6's replaceChild, ROADMAP OF) in 19 s of run
time, where the run from boot needs 29 minutes to get there.

**Step 1f: whole files and added modules in a hot swap.** Done
(`xtask/src/hotswap.rs`: `prepare`, plan version 2 with a mode per
record; `snapshot::check_build_swap`; `kernel/src/hotswap.rs`:
`file_check`, `apply_file`, `FILE_RECORDS`; `file::processes_mapping`,
`file::create_with_directories`; scenario `hotswap_files`, the
`hotswap_machine` layout case refused by the guest; host tests in xtask
and the kernel). So that one snapshot serves a whole fix loop: a module
no process maps at the resume (a DLL loaded later, a program started
later) is replaced as a whole file, with no layout check, and a module
new in the build is added; Limine's config and the manifest may differ
by those modules alone. `--hot-swap-unsafe` without a list takes every
changed module (patched where a process maps it and the diff takes it,
else whole); with a list, the named DLLs are patched and other changed
modules replaced whole. A whole file a process maps is refused by the
guest, naming the process. Each file is logged (`hotswap: <path>:
replaced whole|added ...: <reason>`, `hotswap: applied: <name>`), with
the ledger entry and event; the run stays not a real one.

**Step 2: single programs, inside gala.** A program snapshot is R9's
handover state cut down to one process tree: its address space (private
pages, page-file slots read back, image and file views by name and
digest), threads with their contexts and the calls they are blocked in
(reissued on restore, as after a handover), handles and the objects
behind them, its token, windows, console, registry views and sockets.
What cannot be carried makes the snapshot refuse with the reason, as a
handover does; R9's list of what is not carried yet is the list to
work through here too. A program's files are captured as references
plus digests: restore refuses, naming the file, if an image or mapped
file differs, and a snapshot can include the files it has open for
writing.

**Step 3: the command line.**
- `gala snapshot <pid|name> [--name <n>] [--stop]`: freezes the
  process tree at a quiescent point, writes the snapshot, then lets it
  run on (or ends it with `--stop`).
- `gala snapshots`: lists them (program, time, size, build).
- `gala restore <name>`: starts the program from the snapshot as a new
  process tree (new process and thread ids; handles and window handles
  are remapped as a handover remaps them).
- `gala snapshot delete <name>`.
- Restoring the same snapshot twice runs two independent copies (the
  branch).

**Step 4: export to another machine.**
- `gala snapshot export <name> <file>`: one file, versioned like R9's
  state format, holding the snapshot, the images and files it needs (or
  only their names and digests with `--refs`), and a manifest naming
  the gala build, the program's identity and what it held (sockets,
  windows) that the target must reconnect.
- `gala snapshot import <file>`: checks the format version, the
  digests and a signature (R8's keys).
- On the new machine, connections to the outside are closed with the
  error a lost network gives, and the program sees its windows on the
  new desktop.
- Snapshots hold secrets (a program's memory, its token's identity), so
  an export is encrypted to its target and restoring as another user
  is refused (SECURITY).

**Tests**: scenarios `snapshot_machine` (save at a line, restore twice,
both runs reach the end), `snapshot_program` (a test program counts,
is snapshotted, killed, restored, goes on counting; a second restore
runs beside it), `snapshot_refused` (a program holding what cannot be
carried) and `snapshot_export` (export, import on a fresh boot,
restore).

**Design**: builds on ARCHITECTURE "Kernel handover", "Programs across
a handover" and "Versions and negotiation"; the design goes in its own
ARCHITECTURE section when step 2 starts.

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
need unwinding: next). The wire format (the owner): what is sent is
OTLP/HTTP binary protobuf, gzipped, in batches of at most 512 items
from bounded queues (drops counted), retried with backoff and
`Retry-After` (`xtask/src/otel/send.rs`, its `Batcher` meant for OT5's
in-guest exporter too); a hand-written encoder checked by decoding it
back against the JSON form; `OTEL_EXPORTER_OTLP_PROTOCOL=http/json`
selectable, files still OTLP/JSON, profiles still JSON on the wire
(OBSERVABILITY "OpenTelemetry export"). The input path as pipes and
completion ports have it: the kernel's `input taken` event, each
taking linked to its `button` or `key` delivery, `otel: slow input`
among the slowest hand-offs.

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

### CS. Crash-safe: no hard resets, a kind crash screen

**Goal** (the owner): the user never has to reset the machine, and when
the worst happens the screen they see is calm, clear and useful: no
blue screen. A kernel failure costs at most the thread or program that
was in the kernel when it happened.

**Today**: a program's fault ends that program (crash report, the rest
runs on); a kernel panic prints the message, a backtrace and the flight
recorder to the console and serial log, then halts. gala loads no
third-party kernel drivers (the usual cause of Windows' bugchecks); its
own drivers are Rust, `unsafe` confined, allocation fallible.

**How Windows does it** (public documentation, for comparison): a
bugcheck stops every processor, paints the stop screen (stop code, a QR
code to its documentation, "% complete"; Windows 11 24H2 made it
black and shorter), writes a crash dump through a separate, preloaded
copy of the boot disk's driver run in polled mode (the normal stack is
not trusted after a crash) into the page file or a dedicated dump
file, and restarts; at the next boot the dump becomes MEMORY.DMP or a
minidump and "Windows recovered from an unexpected shutdown" is
offered. Repeated failed boots go to the recovery environment (24H2:
Quick Machine Recovery fetches fixes from there). Watchdog bugchecks
catch hangs; live kernel dumps record some failures without stopping.

**Next** (each test first, with fault injection: R2's `failpoint=`
gains kernel panic, hang and corruption points):
1. Restart on panic: instead of halting, the panic path writes its
   crash record and hands the machine to a fresh copy of the running
   kernel through R9's handover, carrying programs, windows and
   sockets; the thread that was in the kernel gets an error from its
   system call, or its program ends if its state cannot be trusted.
   The record is validated section by section (CRC, versions) and a
   section that fails is dropped, never trusted; if carrying fails,
   the fresh kernel starts clean without the firmware (R9's
   "same build" path), the session restored from the disk (registry
   and file log are already crash-consistent). Scenario: a panic
   injected under a desktop workload; the programs still answer after.
2. Hangs: a watchdog (NMI per CPU, the scheduler's heartbeat) takes the
   same path when the kernel stops making progress.
3. Telling the user after a recovery: a notification on shasha's
   desktop ("gala recovered from a problem; your programs kept
   running"), what was affected, a link to the report.
4. The last resort, when no handover is possible: a self-contained
   panic path (its own stack, no heap, no locks, the framebuffer
   already mapped, the other CPUs stopped) that
   - paints the crash screen (step 5);
   - saves the crash record where it survives: a reserved RAM region
     kept across a warm reset (as Linux's pstore/ramoops), a reserved
     area of the boot disk written by a polled driver of its own (as
     Windows' dump stack), and a small UEFI variable as the fallback;
   - restarts, by handover when the image is intact, else by reset;
     at the next start the record becomes a report and the session is
     restored: programs that registered for restart
     (RegisterApplicationRestart) come back with their own recovery
     data, the rest are offered to reopen.
   Repeated failures within minutes start the previous kernel image
   kept beside the current one (A/B), then a minimal safe mode.
5. The crash screen (the owner: as kind as possible, thinking outside
   the box), drawn by the last-resort path itself. The layout is the
   owner's mockup, `docs/images/crash-screen-layout.png`, with its
   illustration `docs/images/crash-screen-art.png` (the camel being
   revived) built into the kernel image so nothing has to load: the
   gala logo top left; the illustration on the left; on the right the
   headline ("Gala has crashed."), the reassurance ("This was a
   problem in Gala, not in your programs or your files.") and the
   saving checklist (emergency path, resilient memory, disk with a
   percentage, firmware backup, each Done, in progress or Pending);
   below, "What happens next" on the left and the QR code with "Crash
   details" on the right; at the bottom the countdown and three keys,
   R (restart now), S (stay on this screen), D (technical details).
   Text and figures scale with the screen's resolution. Its contents:
   - plain words: what happened, that it was gala's fault and not
     theirs, what is safe (files saved up to when, which programs were
     open and will come back) and what happens next, with a countdown
     to the automatic restart and a key to restart now or to stay;
   - progress of the saving, step by step;
   - the program that was running when it happened, if one was;
   - a QR code that carries the crash itself (the owner), not only a
     link: a URL to galaos.com whose fragment (after `#`, so nothing
     reaches a server until the user opens it) holds the build
     (version and git commit), the stop reason, and the backtrace as
     code offsets in the kernel image, compressed to fit a QR code
     readable from a phone at arm's length. No user data goes in it:
     no file names, no memory contents. The page resolves the offsets
     against the debug information published for that exact build
     (CI uploads each release's symbols) into function names and lines
     linked to the source on GitHub at that commit; it hashes the top
     frames into a crash signature, shows the GitHub issue already
     open for that signature or a prefilled new one, and what changed
     since in those files (fixed in a later build, with the commit).
     The same link sits in the recovery notification (step 3) and the
     saved report; at the next start gala offers to send the full
     record (nothing leaves the machine without consent);
   - large text, high contrast, keyboard alone (no mouse driver is
     trusted), localised strings built in;
   - a details view for developers: message, backtrace, flight
     recorder, the record's identifier.
   Screenshots of each state; a design review against Windows' and
   macOS' screens first.
6. The crash page on galaos.com (with step 5's QR code): the symbol
   store per build, the decoder of the fragment, signatures matched to
   GitHub issues, and a test that a crash injected in a scenario
   decodes to the right function and line at the right commit.

**Done when**: injected panics, hangs and corruptions in each
subsystem (scheduler, memory, network, disk, display, input) end in a
recovered desktop with its programs (steps 1-3) or, for corruption the
handover refuses, in the crash screen, a saved record and a restored
session after the restart (steps 4-5); no scenario needs a reset.

**Design**: ARCHITECTURE "Kernel handover", "Diagnostics of stuck and
crashed programs"; the crash screen and the last-resort path get their
own sections with step 4.

### TM. A task manager for Windows and Linux programs alike

**Goal** (the owner): one task manager on shasha's desktop that shows
every process gala runs, Windows and Linux (LX, WSL distributions)
together, and lets the user end or inspect any of them.

**Done so far** (scenario `taskmgr`, test `user/taskmgr_test`, host
tests in `nt::tasks`, `nt::desktop` and `cargo test -p taskmgr`):
- Step 1, the data: GalaTasks lists every process of both
  personalities in one layout (pid, parent, platform, image or `comm`,
  command line, user, CPU time from the scheduler's new per-thread
  accounting, working set, commit, threads, handles or fds, start time,
  taskbar window, WSL distribution) and the machine's load (busy and
  idle time per processor, memory, disk and network bytes);
  NtQuerySystemInformation keeps Windows' contents (NT processes only);
  `gala ps` lists both, `gala kill` ends a Linux process as End task
  does.
- Step 2, the program: gala's `taskmgr.exe` (Ctrl+Shift+Esc, the
  taskbar's context menu, the start menu's tile when System32 has it):
  Processes grouped as Windows does with Linux by distribution,
  sortable columns, search, End task (TerminateProcess; SIGTERM then
  SIGKILL; a distribution's whole tree from its heading), Details with
  the full columns, Performance graphs per processor, memory, disk and
  network. The scenario ends one of each from it and its graphs move
  under a busy thread; its screendumps are the views' pictures.
- Step 3, hung programs: a window whose input waits untaken for 5 s is
  "Not responding" (Windows' rule; the desktop's input queue), in the
  query and red in the Status column; the scenario's frozen window is,
  its busy one never. Not covered: messages between programs, Wayland
  clients, Linux D state (gala's Linux waits are all interruptible:
  ledgered); no "(Not Responding)" caption or ghost window yet.

**Next** (each test first, checked on the screen, screenshots):
1. Inspection: open file location, a process's threads with their
   stacks (the state dump's walker), its open handles or fds, and
   "Create dump file" through the crash report's writer.
2. Images: `taskmgr.exe` in System32 of every desktop image (it ships
   per scenario now, as `wsl.exe`), and ProcessTimes/GetSystemTimes
   from the same CPU accounting.
3. Statistics: distributions over time, not only totals, in a
   Statistics view beside Performance (top lists, sortable, p50/p95/p99
   columns, a chosen window of 1, 10 or 60 s) and `gala stats`:
   - per thread and process: CPU over the window (the busiest threads
     now, by name), scheduling latency (runnable to running), run-slice
     length, context switches per second, and blocked time split by
     what is waited on (event, pipe, socket, disk, lock) with the top
     objects named;
   - per system call and per file, disk or socket: latency
     percentiles; page faults per second, served from memory or read
     from disk and the page file;
   - trends: working set, commit, handles or GDI objects that only grow
     (leak suspects, from the quota accounting); the kernel locks with
     the most contention and wait (lockdep knows each); how long a hung
     window's input has waited.
   Built as fixed-size log-bucket histograms per CPU (no allocation on
   the hot path), percentiles read from them on demand, exported as
   OpenTelemetry exponential histograms (item OT); the cost measured
   with `bench`, and each counter switchable at run time.
4. Copy: a Copy button (and Ctrl+C) in every view that puts what the
   view shows on the clipboard as text, ready to paste to a person or
   an agent: the view's name, the time and window, the gala build, the
   sort and filter, then the visible rows as a Markdown table with
   their units (selected rows only when there is a selection), and in
   Statistics the percentiles' sample counts. The same text from
   `gala ps --copy` and `gala stats --copy`; the scenario checks the
   clipboard's text against the view.
5. Sparklines: a small line of the last minute beside every metric, as
   Windows' Performance sidebar: in Performance's list on the left
   (each processor, memory, disk, network, the process, thread and
   handle counts, from `taskmgr::History`), and in the CPU, Memory,
   Disk and Network columns of Processes and Details per process
   (a fixed ring of samples per process, kept with its row and dropped
   when it ends; no allocation per sample), and beside each
   Statistics row (its p50 and p99 over time). Scaled per metric
   (CPU and memory to 100 %, throughput to the visible peak, the peak
   written beside it); the scenario checks the lines rise under its
   busy thread and stay flat for an idle process, and the Copy text
   carries the sampled values.

**Done when**: a scenario with Windows programs and a WSL distribution
running shows them all in one list, ends one of each from it, and its
Performance graphs move under a load (met by `taskmgr`; the item stays
open for the steps above).

**Design**: ARCHITECTURE "The task manager".

### EX. A file explorer over every file system

**Goal** (the owner): an Explorer for opening folders and files, as
Windows' File Explorer does, that lists every file system gala holds in
one tree: the Windows drives, each WSL distribution's root (as Windows
shows `\\wsl.localhost\<distro>`), the Linux personality's own root,
and removable or network volumes as they come.

**Done so far** (`explorer_test`, scenario `explorer` with its screen
check; ARCHITECTURE "The file explorer"):
- Step 1's core: `\\wsl.localhost\<distro>\...` and `\\wsl$\...` for
  every Windows program, the distribution's own tree in the one file
  system (`nt::wslfs`: UNC paths through `\Device\Mup`, Linux links
  followed inside the distribution, names case-sensitive, Windows'
  new files 0644), FileStatLxInformation with the Linux mode, uid and
  gid; shell32's namespace (`user/shellns`, `namespace.rs`): the
  desktop, This PC with the drives, Linux with each distribution,
  IShellFolder, IShellItem, their enumerators, the item ID list
  functions, known folders' lists and items, parsing and display names
  as Windows gives them (Wine's shlfolder.c and shellpath.c and
  Microsoft's documentation); ShellExecute opens documents with their
  association (a Linux file's Linux path for `wsl.exe`).
- Step 2's core: gala's `explorer.exe` (navigation pane, address bar
  with paths and `\\wsl.localhost` names, Details and Icons views,
  sorting, search by name, opening with the association, Back and Up),
  Win+E and the start menu's tile.
- Of steps 3-4: copy (and cut) and paste through the clipboard
  (CF_HDROP, SHFileOperationW), rename (F2), delete, new folder, Open in Terminal (cmd, or `wsl
  -d` in the Linux folder), Properties with the Linux permissions, and
  live updates (ReadDirectoryChangesW: a Linux process's new file shows
  without a refresh). The Done-when scenario runs: Explorer browses
  from C: into `bb`'s root through the Linux node, pastes a file there,
  opens it with its program and sees a Linux-made file appear.

**Next** (each test first, checked on the screen, screenshots):
1. The namespace's rest: the files on the user's Desktop, the Recycle
   Bin, IShellItem2 and property stores, GetUIObjectOf's context menus
   and data objects (other programs' "Open folder" and drag sources);
   Linux names Windows cannot hold mapped as WSL maps them; removable
   and network volumes as they come.
2. The program's rest: SHBrowseForFolderW and IFileDialog as dialogs
   of this namespace (and "Open folder" in other programs), several
   selected items, rename in place, a tree that expands any folder.
3. Operations' rest: cut checked on the screen, delete to the Recycle Bin,
   drag and drop between panes and to the desktop, with progress and
   conflicts handled as Windows does; across file systems (a Windows
   drive to a distribution and back) with the source's Linux modes kept
   where they can be (a copy now takes a new file's 0644).
4. Thumbnails through WIC; inotify for Linux programs watching what
   Windows programs change (ReadDirectoryChangesW already sees Linux
   changes); the rest of Properties (sizes of folders, attributes).

**Done when**: a scenario opens Explorer, browses from C: into a WSL
distribution's root through the Linux node, copies a file from one to
the other, opens it with its program, and sees the copy appear without
a refresh.

**Design**: ARCHITECTURE "The file explorer" (and "The WSL facade" for
`\\wsl.localhost`).

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

- **ChromeDriver's `browserStartupTimeout` at 180 s** (`chromium_driver`,
  `chromium_wpt`; ChromeDriver's default is 60 s): ChromeDriver gives up
  when the browser's DevTools server has not answered `/json/version`
  by then. The browser's UI thread answers only after its start-up work,
  40-70 s after launch under TCG with the gate's load; profiled, that
  thread is CPU-bound (no long waits): image page faults with their
  relocation, lockdep in the test kernel, Chromium's own code.
  Structural fix: Chrome's start-up under 60 s in the gate (the BR
  profile-hotspots item: the start-up's kernel share, fewer page
  faults); then the capability goes. Item BR.

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
| `filezilla`, `filezilla_flaui`, `filezilla_sftp` | slow (about 100 s, 45 s and 45 s in the kernel under TCG) | W3 |
| `sshd` | `ssh-keygen -A` and `sshd -t` only; superseded by `sshd_login`, `sshd_server` | SSH (move into the suite or drop) |

### Manual scenarios

`MANUAL` in xtask: never in `cargo xtask test` or CI, by design; run by
exact name (`cargo xtask boot-test office_install`).

| Scenario | Why manual | Owner |
|---|---|---|
| `office_install` | the Office Deployment Tool installs Word, Excel and PowerPoint from Microsoft's CDN: a ~5.4 GB download onto a disk-backed C: (a 16 GiB sparse image, up to ~6 GiB on the host) | OF |

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

- **Hygiene sweep** (the owner; in a few months, not now): first
  enumerate the practices gala's code should not follow, by sweeping
  the code (e.g. large arrays on the stack in kernel and DLL code,
  which a signal or exception path runs on a limited stack); then, for
  each enumerated item, a subagent sweeps the code for instances and
  fixes them. Prompted by the richer crash report (register memory and
  the deeper stack, 1 KiB arrays on the exception path), suspected when
  `fpu_test` failed in gala-um; the cause was elsewhere (a new thread's
  first entry took the host's SSE registers: docs/GALA_UM.md).

- **Self-hosting**: building gala's own test programs (then Nginx) on
  gala. H1-H10 built the base (pipes, child processes, busybox, TinyCC
  building itself, make, Lua and SQLite built on gala); what is left is
  a Windows-hosted toolchain (llvm-mingw, or the MSVC Rust target)
  running on gala, cargo spawning compilers, and a larger writable file
  system on the disk. Then Rust and its build tools (rustup's
  toolchain: cargo, rustc, lld, clippy, rustfmt; llvm-objcopy; git)
  on gala, by the Windows route or the Linux one (LX runs glibc's
  dynamic programs; rustc needs many threads, gigabytes mapped and a
  jobserver over pipes), so gala builds its own kernel and DLLs; the
  new kernel is then taken over by R9's handover (signed with a key the
  running kernel trusts, R8), or a small change patched in place as SN
  step 1e's hot swap does. Tests that boot QEMU stay on a host.
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
- **Raspberry Pi** (on item AR's port): Pi 4/5, QEMU's `raspi4b`/`virt`
  first; Limine or the Pi's firmware loading the kernel, the GIC and the
  generic timer, the PL011 UART, the mailbox framebuffer, then EMMC2/SD,
  USB through xHCI (the Pi 5's RP1, the Pi 4's VL805) and GENET
  Ethernet, all against published datasheets.
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
    re-check; an atomic SignalObjectAndWait; the thread pool's
    persistent callbacks (TP_CALLBACK_ENVIRON's Persistent, recorded)
    and ntdll's Tp* exports (gala's pool is kernel32's).
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
- **MCP: agents working as a real gala user**: `gala mcp` speaks the
  Model Context Protocol over stdin and stdout, reached as
  `ssh user@gala gala mcp`, so an agent (Claude Code or another MCP
  client) works in gala with that user's token and nothing more. Login,
  keys and encryption are item SSH's; no new port, no new
  authentication. Tools: gala's commands (`ps`, `start`, `windows`,
  `kill`, `stats`, the task manager's copy text), the ledger and the
  event stream, window screenshots, UI Automation (find, invoke, type;
  item UIA), and later SN's snapshots. The SSH logon is a network
  logon; seeing and driving windows needs the user's interactive
  session, which "attach to my desktop" asks for under RS's rule (one's
  own session, or SeTcbPrivilege), with the desktop showing that an
  agent is attached and the user able to cut it off. Read-only tools by
  default, input and process control only when the user's settings
  allow, rate limits, every call in the audit trail as any process's
  actions are. Uses: agents testing and debugging real programs (start
  Office, read the ledger, click through a dialog), xtask's scenarios
  on the same tools instead of their own serial scripts, item AI's
  start menu.
- **Applications put to sleep**: a program that is not in use stops
  costing anything. Freeze: every thread of the program's process tree
  stopped (as R9's quiescence stops them), no CPU, its memory moving to
  the page file under pressure. Deep sleep: a program snapshot (SN
  step 2) written to disk and the process ended, all its memory freed,
  restored on wake; Windows cannot do this for Win32 programs (it
  suspends only packaged apps, and throttles others with EcoQoS). Policy:
  asleep after a while minimized, awake on a click or `gala wake`, the
  taskbar showing it; programs that ask get Windows' suspend and resume
  notifications. Never asleep: a program others depend on (a COM server,
  a pipe or shared-lock holder, a service), which the kernel knows from
  the handles between processes; network connections time out and
  timers fire at once on wake, so both are named in the policy's
  decision.
- **Hibernation**: gala saves the whole running system to its own disk,
  powers off, and resumes everything on the next boot, on real hardware
  as well as in a VM. It is R9's handover sent to the next boot through
  the disk instead of to a new kernel in memory: threads stopped at a
  quiescent point, the kernel's state in the versioned handover format,
  memory paged out to the page file where it can be and the rest
  written beside it, devices reset and rebuilt as after a handover. New
  pieces: a resume path at boot (Limine and the kernel find the image,
  check its build and that the disk has not changed since, and resume
  instead of booting fresh); the clock's jump and renewed network leases
  on resume; programs told as on Windows (power broadcasts). What a
  handover refuses, hibernation refuses with the same reason (R9's and
  SN's list). It also gives CS a fallback: on a fatal error, try to
  hibernate before restarting. Sleep (suspend to RAM) is apart and
  harder: ACPI sleep states and drivers that save and restore device
  state.
- **A debugger on the kernel**: `boot-test --gdb` starts QEMU with its
  gdb stub (`-s`, `-S` to wait) and prints the gdb command line (the
  kernel ELF's symbols, the target port), on a boot or on a snapshot's
  resume (`--from-snapshot`: attach just before a failure instead of
  after a 30-minute run). Breakpoints, stepping, reading and writing
  variables, memory and registers; under KVM a few hardware breakpoints,
  under TCG any number. Reverse debugging (gdb's `reverse-step` and
  `reverse-continue`) through QEMU's record/replay: TCG only, one CPU
  practical, much slower, so for short repros rather than the Office
  install. gala-um already takes `gdb -p`, with `--sim` and
  `--record`/`--replay` to return to the same point (WORKFLOW).

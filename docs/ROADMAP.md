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

**Next (the owner): a laptop that runs gala and takes every release by
itself.** In order:
1. **The desktop on ARM** (item AR, "Next" 0): the downloadable
   aarch64 image boots to a desktop that keyboard and mouse can use,
   GTK programs on aarch64, `arm_lx_wl_popup`'s hang.
2. **The installer** (item IN): gala installed from its USB image onto
   a machine's disk, which then boots by itself.
3. **Live update working end to end** (items R8 and R9, on item HW's
   machine): R8's updater in gala takes each release from
   `updates.gala-os.com` and hands it to R9; the user-mode half of R9.
   The machine also needs item HW's drivers (its disk, USB input, a
   network: a wired USB adapter first, Wi-Fi, item WF, after).

**Then (the owner): Microsoft Office, item OF, with everything it
depends on.** It goes before the list below (work in flight finishes:
LD's first GTK program). Its chain, in order:
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
   Its blockers went with item 12 to the Windows machine, where gala
   runs under KVM (the owner; `docs/windows-kvm-brief.md`): the whole
   `dom/nodes` directory runs there (13326 of 13397 subtests), and
   neither Chrome's out-of-memory exceptions (0xE0000008) nor
   ChromeDriver's hang reproduces under KVM, so both wait on the
   conditions they were seen under (stage 2's "Next" (b) and (c)).
1. **TLS**: step 6's rest (revocation information fetched from the
   distribution point, winhttp keeping its credential handle so it
   resumes, a server asking for client certificates, the MY store's
   automatic client certificate; ECDSA server keys, client session
   resumption, client certificates and offline revocation are done).
2. **LX**: seams 1 and 5 as their areas are touched, seam 4's NT surface;
   milestone 2's rest (busybox runs; threads, futex, fs_test's links
   done) as filler.
3. **RS**: step B3a's rest (`presenter=` in gala-um, xtask's headless
   screen checks through SESSION_INPUT); then B3b, the RDP server.
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
    the ARM64 port itself later): the kernel's and the DLLs' done; ntrt
    and the test programs come with AR's user mode.
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
    benchmark in `bench` and its numbers in PERF.md. `bench` is compared
    (PERF.md 12): the system call path is 7x Windows' and the widest
    gap, a reservation split into 4096 runs 6x where one run is faster,
    `demand_zero_fault` 3-8x with 3x variance of its own, and image
    loads 8-31x faster here. Next: the scenarios' programs
    (run-all's `ms` against `cargo xtask report`, then `cargo xtask
    compare`), Chromium's page loads and Python, then `cargo xtask
    profile` on the system call path -- after rechecking #43 (the
    profiler's samples hung the boot thread under KVM) there, since a
    tick's writing is bounded now (ARCHITECTURE, the profiler).

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
| PT | Portability seams for a second architecture | in progress | the kernel's seam done (`arch`'s interface, the per-architecture ABIs `nt::abi`/`lx::machine`, an aarch64 stub CI checks); gala's DLLs built for ARM64 in CI (stubs where the port is needed); ntrt and the test programs with AR; the ARM64 port itself is in "Later" |
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
| CT | Linux containers: Podman and Docker on Linux, reached from Windows | in progress | the WSL facade's core (`wsl`, `wsl_terminal`); next the Linux side first: rootless Podman, then cgroups, networking, rootful Podman and Docker Engine, nesting, then Kubernetes (k3s, kind); then Docker's Windows CLI over a relayed `\\.\pipe\docker_engine` and Podman for Windows; Docker Desktop in "Later" |
| LD | Desktop Linux applications | in progress | step 1: dynamically linked glibc programs run through ld.so (`lx_dyn`); step 2: memfd and AF_UNIX sockets with SCM_RIGHTS (`lx_memfd`, `lx_unix`), the Wayland wire format (`wl_abi`) and the first in-kernel Wayland server (wl_compositor, wl_shm, xdg_wm_base, wl_output; toplevels as desktop windows: `lx_wl`), wl_seat input and window control (`lx_wl_input`), weston-simple-shm (`lx_simple_shm`), popups (`lx_wl_popup`); step 3: the first GTK program, its window drawn and clicked (`lx_gtk`, GTK 3 on a Linux root of Ubuntu's packages); step 5: X11 programs through rootless Xwayland, gala its window manager, xterm on a pty (`lx_xterm`), GTK under X11 (`lx_gtk_x11`); step 6's first part: a distribution's desktop in one window (`lx_xdesktop`); step 7: Linux distributions in the start menu (`linux_menu`); step 9: GIMP 3.2 in batch mode, its saved file Ubuntu 26.04's byte for byte (`lx_gimp_batch`; System V shared memory: `lx_shm`; sysinfo: `lx_sysinfo`), and on the desktop: its window, in.png opened, Colors > Invert and File > Export, the exported file Ubuntu 26.04's byte for byte (`lx_gimp`); next: all of GIMP's plug-ins |
| AR | ARM64 port | in progress | the aarch64 kernel boots on QEMU `virt` with its own page tables, GIC, scheduler, CPUs, virtio disk and network (`arm_*` scenarios); ARM64 Windows programs run through gala's ntdll, bound to ARM64 DLLs, with threads, SEH and APCs; 156 x64 scenarios run on it as ARM64 twins; state dumps at a timeout; Linux programs run on aarch64 too (40 `arm_lx_*` twins); virtio-input keyboard and mouse; kernel handover; next (the owner) the desktop on ARM: the boot ends on a usable desktop (`desktop_at_boot`) and GTK runs (`arm_lx_gtk`); the popup hang left |
| VM | Virtual machines on gala | later | wanted, not scheduled (the owner); step 1: QEMU's emulation (TCG) as an ordinary program, a small Linux guest booted inside gala; step 2: gala as the hypervisor (VT-x/AMD-V, ARM's EL2) behind Linux's `/dev/kvm`, then the Windows Hypervisor Platform (`WinHvPlatform.dll`) |
| XT | 64-bit Windows programs on ARM64 | planned | dynamic binary translation of x64 code, then 32-bit x86 (WOW64 on ARM; Office's x86 installer); after AR |
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
| MCP | The system's actions as tools for AI clients | planned | `gala mcp` and aichat's custom tools from one description per command; tools loaded by category through a tool call |
| UIA | UI Automation and windows across processes | in progress | `uia_test`, `flaui_test`, `windows_across_test`, `filezilla_flaui`; ARCHITECTURE "UI Automation" |
| SD | Server and desktop workloads kept apart | in progress | one kernel: `profile=server`, CLI parity, bare desktops done |
| SSH | SSH as a remote shell | in progress | steps 1-5 in their first form |
| CP | An interactive console prompt | planned | serial line, screen and keyboard, gala-um's terminal; no SSH needed |
| RS | Remote access and sessions | in progress | A, B1, B2, B3a's window stations, presenters, console switch and window state apart from the compositor (bare desktops too) done; B3b, the RDP server, next |
| GPU | Graphics and GPU | in progress | G0 and G1 done; G2 in progress (OpenGL and Vulkan done); G3 parked |
| SND | Sound | later | nothing yet; virtio-sound with a WAV-file check first, then WASAPI; not urgent |
| USB | USB: xHCI, keyboards, mice and storage | later | nothing yet; device interrupts first, then xHCI on `qemu-xhci`, HID, storage; HID is what a laptop with no PS/2 devices needs to be touched at all |
| IN | The installer: gala onto a machine's disk | in progress (steps 1-4's command: `gala setup` installs, the disk boots: `install_to_disk`) | a native gala program on the USB image's desktop: GPT partitions, an EFI system partition with Limine and the kernel, C: in a partition; erase-the-disk and free-space installs first, beside Windows later; a two-boot scenario (install unattended, then boot from the disk alone) |
| HW | A real laptop: gala on hardware, waiting for updates | planned | the target of IN and R8/R9 (the owner): AHCI and NVMe disks, USB input (item USB), a network (a wired USB adapter, then item WF), the firmware's framebuffer, power; the owner's laptop is the first machine |
| WF | Wi-Fi: a dynamic interface, 802.11 and the Windows APIs | later | nothing yet; four layers (DHCP and address changes, the Windows wireless APIs, 802.11 and a WPA2/WPA3 supplicant, the card's driver through DR); a wired USB adapter is the cheap way to a network on real hardware |
| BR | A web browser: stock Edge | in progress | stage 1 done: chromium_version, chromium_dump (a page's script runs), chromium_https (a page from the guest's nginx over TLS) and chromium_host (a page from the host network) pass in the gate; stage 2 (a window) under way: chromium_window |
| OF | Microsoft 365 apps and the new Outlook | in progress | the gap analysis done; the Click-to-Run install completes (`office_install_local`); the AppV shim's virtual environment comes up and WINWORD loads Word, Office's shared code and its resources and puts an untitled window on the screen; with DXVK beside WINWORD.EXE it creates a feature level 11_0 Direct3D 11 device on lavapipe and no longer raises; Direct2D 1.1 and 1.2 are there (ID2D1Factory1 and ID2D1Factory2, their devices, device contexts, geometry realizations and target bitmaps over DXGI surfaces: `d2d1_device_test`), and so is the 1.0 path Word actually takes: a feature level 10.1 device of gala's own `d3d10_1.dll` over that Direct3D 11, its textures and Direct2D render targets over them, each of those targets a device context whose own device Word takes to make a context of its own (`d3d10_1_test`), every call of which Word gets S_OK from, and so are the steps it stopped on next: Direct2D's drawing state blocks, DirectWrite's text-rendering parameters, a target bitmap over Word's own surface and the meshes it draws tessellated shapes with, so the whole Direct2D and DirectWrite path Word walks is answered and its ledger ends on a solid-colour brush over that target, every call of the path S_OK (`d2d1_test`, `dwrite_test`, `d2d1_device_test`); the display is answered as far as Word and DXVK ask for it (EnumDisplayDevicesW's monitor level, a Plug and Play device tree setupapi reads out of the registry, and the panel's EDID the kernel writes into the monitor's device key at boot, which is what DXVK's `getMonitorEdid` wanted: `display_test`); the system metrics a program lays its windows out from are measured on Windows and answered, 73 indices having answered a silent 0 before, sixteen of them among the 34 Word reads (SM_MOUSEPRESENT, both scroll bars, all four frames, the double-click slop, the menu buttons and the window edge), and every index a program asks for leaving a reached row now (`metrics_test`); the device caps a program lays text out from are measured on Windows and answered, sixteen indices having answered a silent 0 before (TEXTCAPS, the curve, line and polygon caps and the pixel's aspect among them), one of them among the ten Word reads (RASTERCAPS, 10889 where Windows answers 32409), and every index a program asks for leaving a reached row now (`caps_test`); the settings a program asks the desktop for are measured on Windows and answered, nine of the answers having been values no Windows gives (every animation and fade off, UI effects off and client-area animation off, where a Windows desktop answers TRUE for all nine), two of them the first two settings Word reads, the non-client metrics now read from the system metrics so two copies of a desktop's sizes cannot drift (a caption button was 32 wide against the metric's 46), and every action a program is answered leaving a reached row now (`spi_test`); Word's window classes are answered: its startup registers more than the 32 a fixed table held, which is measured now because each doubling leaves a row (the table grows to 64), and with a table that grows to the atom space instead `IfrRegisterWndClasses` is passed -- no `ErrorEid`, no `SHIP ASSERT`, no `BootRun {"SH_ErrorId": 13, "BootTask": "IfrRegisterWndClasses"}` and no "Word was unable to start. (13)" box, where Word's startup runs 36.6 s against 32.3 s before and writes 393 events from 10 providers against 353 from 8, measured on a fresh disk (`wndclass_test`, `office_word`); what ends Word now is an exception of Office's own: `Mso30win32client.dll` raises 0xe0000002 through kernel32's RaiseException (exception record flags 0x80, software-originated) and nothing takes it, because the unwind stops inside gala's own ntdll -- `3 frames unwound (1 of them with no unwind data found)`, the third `ntdll.dll+0xca85`, `0 handlers asked`, the walk ending at "no return address: the stack's end" -- so an exception Office raises for its own handler ends the process instead, and that unwind is where this goes next; a reused disk also makes Word ask "start in safe mode?" after a run that crashed, which the ledger records and Word is given no answer to (WORKFLOW "Word's own crash state"); the hardware graphics path is ruled out from Office's own side, with `DisableHardwareAcceleration` seeded Word raising the same box with the same (13), and the silent seconds before it are Word walking its own address space and not a wait; the multisample answer Word reads 197 ms before the box is measured against Windows and is wrong without being the cause -- gala's renderer offers no quality level for 2 samples on B8G8R8A8_UNORM where Windows offers one, which the ledger records now, and answering it as offered leaves the box unchanged (`d3d10_1_test`); every file Word opens is read and none it needs is missing (`events=file`: 482 opens, 225 refusals Windows refuses too, the eight before the box the message box's own side-by-side probe), and so is Word's own 401-byte exit record (`events_data=512`), which is a session record with no reason in it, filed under GracefulExit rather than CrashPersistence; gdi32 answers the two adapter calls DXVK and the Vulkan loader ask for as a machine with no adapter does, measured on Windows with a GPU and without (`D3DKMTEnumAdapters2` succeeds and writes none, `D3DKMTQueryAdapterInfo` refuses every handle offered it: `d3dkmt_test`), DXVK associating the monitor with the adapter by its own fallback either way; cfgmgr32 answers the configuration manager's device lists out of the registry's device tree instead of claiming there is none -- the class, enumerator and service filters, the devnodes their ids name, their properties and the errors for each edge, all measured on Windows (`cfgmgr32_test`) -- and every list a program asks for leaves a row naming the class it wanted with the number of devices it was given, which on its first run found a divergence and a stack capture then named its owner: the display class is asked for sixteen times and each answer is followed by CM_Get_Child on the adapter's devnode, sixteen refusals, and with `stack_at=NtOpenKey stack_from=cfgmgr32` every capture is DXVK's `dxgi.dll` (gala's own dxgi never calls cfgmgr32), so those refusals are what DXVK's three "monitors not associated with any adapter, using fallback" warnings are -- it wants the monitor as the adapter's child, the one thing `Enum` cannot hold (Windows keeps it in its PnP manager, not the registry), which makes a kernel-kept device tree an item-USB-step-5 piece and not a cause of the box, DXVK coping by its fallback; Word's own telemetry names the failure in Word's own words: with gala as the session (`etw=*:255:0:4096 etw_from=WINWORD`, OT4) the startup writes 353 events from 8 providers, every one of them a manifest provider's, whose field names are in no part of the event -- so each line carries the text its bytes held (`kernel/src/nt/etw/strings.rs`), and that text is `ErrorEid {"SH_ErrorId": 66, "SH_ErrorCode": 13}`, then `SHIP ASSERT FAILED! {"wzTag": "qsqk"}`, then the box, then `BootRun {"SH_ErrorId": 13, "BootTask": "IfrRegisterWndClasses"}`: the (13) is Word's own SH_ErrorId and the boot task it died in is Word registering its window classes, which is answered now (above) and was no part of the graphics path the measurements above ruled out one at a time, and the provider side the events go through answers as Windows does, measured function by function including the orders it validates in and the traits a provider may set once (`advapi32_test`); Excel takes the same path, not rerun since (`office_excel`) |

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
| OT | OpenTelemetry as the telemetry model | in progress | OT1 done; OT2: threads on every event, the process tree; next OT6's one query path for every session (`xtask observe`, `xtask query`) |
| GR | Gap reports from real runs, straight to GitHub | planned | the ledger of a run that hit a gap or ended badly kept as a report, redacted, sent with the owner's consent, and turned into (or counted on) a `gap` issue; `gala gaps`, an intake beside the update feed |
| SU | Security updates: where fixes come from and how they reach machines | planned | gala's dependencies, its own code, Windows' advisories, vendors' security releases for installed applications; delivery through R8 |
| CS | Crash-safe: no hard resets, a kind crash screen | planned | the kernel restarts itself through R9's handover; the last-resort path saves and explains |
| TM | A task manager for Windows and Linux programs alike | in progress | steps 1-4 done (`taskmgr`: one list of both personalities, gala's taskmgr.exe ending one of each, "Not responding", every metric's last minute as a sparkline); inspection next, then statistics (p50/p95/p99, the busiest threads, where they wait) and a Copy button for pasting a view into an agent |
| EX | A file explorer over every file system | in progress | steps 1-2 done (the drives and the WSL distributions in one tree) and part of 3-4 (`explorer`); next the namespace's rest (Desktop, Recycle Bin, context menus), file dialogs, drag and drop |
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
   clippy for `aarch64-unknown-none-softfloat` and gala's DLLs' clippy and build
   for `aarch64-pc-windows-msvc` (item PT; in the pre-push run).
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
  `aarch64-unknown-none-softfloat` (with and without `shasha`), so an x86
  assumption outside `arch` fails the build. The register frames
  (`UserRegs`, `SyscallFrame`, `TrapFrame`) keep their fields private to
  `arch` behind neutral accessors; the clock reads `cpu::cycles` and
  drives `arch::timer`; the boot steps, phase names, per-CPU snapshot and
  NMIs are `arch`'s; PCI and virtio are shared over `pci_config`. Windows' x64 ABI is `nt::abi::x64` (CONTEXT, argument
  registers, thread starts, exception and APC frames, the CONTEXT
  services, KUSER_SHARED_DATA's processor fields, CPUID's registry
  values, the crash report's registers); Linux's x86-64 ABI is
  `lx::machine::x86_64` (arguments, restart, start registers, auxv, the
  signal frame) with the system call table `lx/syscalls.rs`; their
  aarch64 counterparts are stubs. gala-um is x86_64-only
  (`compile_error!` elsewhere).
- User mode (step 1): gala's DLLs build for `aarch64-pc-windows-msvc`;
  CI's Clippy step runs Clippy and a build for them (a build, because
  Clippy does not assemble inline assembly: x86 `asm!` outside the
  per-architecture modules then fails). kernel32, msvcrt (and ucrtbase)
  and ntdll have an `aarch64.rs` beside `x64.rs`/`x86.rs` (kernel32's
  64-bit layouts shared in `win64.rs`); the other DLLs reach the TEB
  through x18 and take 64-bit layouts by pointer width. Filled in: the
  TEB, ARM64's CONTEXT layout and a new thread's context, SLIST_HEADER
  with an exclusive pair, the memory routines and `__chkstk`, ntdll's
  system call stubs on the kernel stub's convention (number in x8,
  `svc #0`). Recorded and ending the process (`unported`): the processor
  number, FPCR and the FP status, rpcrt4's guarded calls and server
  routine calls, the APC dispatcher (traps); not built for ARM64:
  ntdll's exception dispatch and unwinding (`seh.rs`, x64's unwind
  format), kernel32's CONTEXT buffers and XState functions
  (`context.rs`), msvcrt's variadic functions, setjmp/longjmp and
  rpcrt4's NdrClientCall2, each ARM64's own (item AR). Unverified until
  the port records them on ARM64 Windows: GetSystemInfo's processor
  type (0 for now) and SLIST_HEADER's header bits (x64's for now).

**Next**:
1. User mode's rest: ntrt (the test programs' runtime: system call
   stubs, context) and the test programs for ARM64, with item AR's
   user mode. wow64 is x86 on x64 by nature and is not built for ARM64.
2. The kernel's remaining x86-shaped parts, each named here with why it
   stays for now:
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
  releasing X turns interrupts on while Y is still held`, each lock with
  the thread that took it), beside the check that catches the cause
  itself: `sched::schedule` panics when the thread giving up its CPU
  still holds a lock.
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
1. The minimum a window has reaches the resizes the desktop drives: a
   drag of a border and a snap keep to it instead of the desktop's own
   120 x 60 (FileZilla's minimum is 700 x 500, so its snapped half would
   be 700 wide as on Windows; the desktop's docking test and the
   scenario's checks change with it). DefWindowProc's
   WM_WINDOWPOSCHANGING asks the window and clamps already
   (ARCHITECTURE, "The size a window may be given"), so what is left is
   the desktop asking the window before it places it. Then
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
and a click installs it on the tile: `gala install <id>` with no window,
the menu open and a progress bar on the tile (the states gala reports
through GalaDesktopAction's InstallProgress), red if it fails and
installed when it is done; installed, a click starts the program. An
install that ends, either way, is also told above the notification area
(the program's name, and what the exit code says when it says more than
that it failed). An install whose `gala install` is gone without a word
(killed, or faulting before it can report) or never started goes quiet
instead of waiting for ever: the desktop watches the installer's pid,
which `start_from_menu` reports, and the process ending leaves the tile
grey under a question mark with its bar where it got to, a click trying
again and the same notification. A console program started from the menu
gets its own console window. Tests: the desktop's, `nt::action`'s and galapkg's host
tests, scenario `start_menu` (start_menu_test and a screen check: the
tiles, the Command Prompt clicked, 7-Zip's install started with no
console window of its own, its tile's bar red when it failed and its
notification above the notification area).
TimKosse.FileZilla.Client is not in winget-pkgs today: its tile's bar
turns red and `gala install`'s "no package" is in the log (the portable
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
   `%GALA_WINGET_SOURCE%`; the kernel's initial environment is fixed),
   so that a bar filling over a download is tested and not only its
   states. Installs that fail on gala (Office until OF is done, an
   installer needing elevation without UAC's prompt) leave the tile's
   bar red, with `gala install`'s own message in the log.

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
desktop workloads", "Plan: sessions and remote desktops" (presenters).

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
calling, a REPL, one-shot use and `aichat --serve`, a local
OpenAI-compatible server. It is installed with `gala install
sigoden.AIChat` (winget-pkgs: a zip with a portable `aichat.exe`, which
PK's stages 1-2 handle), not shipped. The tools are the `gala` commands
(start, windows, activate, snap, close, ...) and UIA for driving
programs, so the model acts only through what CLI parity already
exposes; item MCP describes them once and generates what aichat reads.

aichat's custom tools need nothing of llm-functions (its bash/Node.js
tool kit, not used): aichat reads the declarations from `functions.json`
(a JSON array of name, description and a JSON-schema `parameters`),
runs an executable of the tool's name from its functions `bin`
directory (found through PATHEXT on Windows) with the arguments as one
JSON command-line argument, and reads the result (JSON or text) from
the file `LLM_OUTPUT` names; a nonzero exit code is a failed call
(aichat `src/function.rs`). `mapping_tools` aliases a set of tools and
`use_tools` picks the default sets, which item MCP's categories map
onto. aichat has no MCP client of its own (its MCP route is
llm-functions' Node.js bridge; native support is an open request,
issue #1346 and draft PR #1427); if it gains one, it uses `gala mcp`
instead and nothing else changes. aichat asks no confirmation before a
tool runs: confirmations are gala's (step 3).

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
   streaming to the console, the `gala` commands as tools (item MCP's
   generated `functions.json`, `gala.exe` under each tool's name); a
   test passes awkward JSON (quotes, backslashes, spaces) through the
   tool's command line and checks it arrives intact; every
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

### MCP. The system's actions as tools for AI clients

**Goal**: everything the `gala` command can do is a tool an AI client
can call: `gala mcp` is a Model Context Protocol server, and the same
tools are aichat's custom tools for item AI, all generated from one
description of each command.

**Approach**:
- **One description per command**: each `gala` command's name,
  summary, category and arguments (a JSON schema), and whether it only
  reads or changes something (destructive: closes, kills, deletes,
  installs), are written once; the CLI parser, `gala help`, the MCP tool
  list and aichat's `functions.json` are generated from them, so a new
  shasha feature's `gala` command (item SD) is a tool with nothing more
  to write, and the four never disagree.
- **Two front ends, one implementation**: `gala mcp` speaks MCP
  (JSON-RPC) over standard input and output, so any MCP client starts
  it locally and a remote one reaches it through SSH (`ssh host gala
  mcp`): no new network service. For aichat, `gala.exe` is installed
  in aichat's functions `bin` directory under each tool's name (hard
  links, or copies) and acts as the name it was started as, as busybox
  does; the arguments come as JSON on its command line and the result
  goes to `LLM_OUTPUT` (item AI).
- **Tools loaded by a tool call, by category**: a model chooses worse
  among hundreds of tools, so a client starts with a small core and
  loads the rest on demand. Categories: desktop (windows, start, snap,
  wallpaper), processes, programs (`gala install`, item PK), files,
  services, registry, accounts and sessions, diagnostics (events, the
  ledger, crash reports, state dumps, `gala gpu`), UIA (driving
  programs). The core is `gala_tools` (with no argument the categories
  and their summaries; with a category its tools' names, descriptions
  and schemas) and the read-only tools of the session in use. Loading
  works in each front end as far as its client allows: in `gala mcp`,
  `gala_tools` with `load` adds the category's tools to the server's
  list and sends `notifications/tools/list_changed`, and the client
  lists them again; for clients that do not follow that notification,
  and for aichat (whose `functions.json` is read once at start),
  `gala_call` runs any tool by name with JSON arguments that gala
  checks against the tool's schema, so a tool `gala_tools` described
  is callable without being declared. aichat's `mapping_tools` gets one
  alias per category, so a user or a role can also start with a
  category declared.
- **Diagnostics as resources**: what is read rather than done (the
  event stream, the ledger, crash reports, the state dump) is also an
  MCP resource, so an agent reads gala's own observability directly.
- **Security**: the server is an ordinary process with the caller's
  token, so the access checks and UIPI of the actions themselves apply;
  no tool reaches what the `gala` command cannot. Every tool carries
  MCP's read-only and destructive hints, and a destructive call asks
  for confirmation on gala's side (MCP elicitation where the client
  supports it, otherwise the console or a shasha dialog), never left to
  the client. A low-integrity server offers the read-only tools only.
  Text a tool returns (window text, file contents, pages) is marked as
  untrusted for the client (prompt injection). Nothing leaves the
  machine unless the user connects a remote model; then what tools
  return is sent to it, which the AI item's configuration states.
- **Observability**: every call (client, tool, arguments, result,
  confirmation asked and answered, time taken) is an event in the event
  stream; a failed call returns the NTSTATUS and the ledger entry behind
  it, not a bare error.

**Next** (each step test first):
1. The command descriptions and the generated CLI parser and `gala
   help` for the existing commands (`gala_cli` unchanged).
2. `gala mcp` over stdio with read-only tools and `gala_tools`: a
   test program acting as an MCP client (initialize, tools/list,
   tools/call, a malformed request, an unknown tool) in a boot scenario
   and in gala-um.
3. Loading by category (`load`, `notifications/tools/list_changed`)
   and `gala_call` with schema checks.
4. The actions, with the destructive hints and confirmation (an
   elicitation answered yes and no by the test client), each call in the
   event stream.
5. aichat's front end: the generated `functions.json`, `mapping_tools`
   aliases per category, `gala.exe` under each tool's name (with item AI
   step 3).
6. Diagnostics as resources; UIA's tools (find, invoke, read a value).
7. Later, if wanted: MCP's HTTP transport with its authorization, for
   clients that cannot use SSH.

**Done when**: an MCP client test lists the core tools, loads a
category through `gala_tools`, calls a read-only tool and a destructive
one (refused when the confirmation says no), and aichat calls the same
tools through `functions.json`; every call is in the event stream.

**Design**: to be written in ARCHITECTURE with step 1 (the command
descriptions, the two front ends, categories and loading, confirmation);
item SD for CLI parity, item AI for the client.

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
(console, an RDP client, none, test); remote users get their own
session and shell, reached with the Remote Desktop clients they
already have (mstsc, the Windows App, FreeRDP).

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
2. B3b: the RDP server (ARCHITECTURE "Plan: sessions and remote
   desktops"): a session-0 service on TCP 3389 presenting a session's
   desktop through GalaSession, from Microsoft's Open Specifications
   ([MS-RDPBCGR] first) with IronRDP's crates where they fit; TLS
   through Schannel's server side and network level authentication
   (CredSSP) through secur32's NTLM server side, whose interoperability
   with Windows is proven here. First a whole desktop (planar and
   bitmap updates, fast-path input, disconnect and reconnect,
   resizing); then the graphics pipeline, clipboard, RemoteApp (the
   server profile's GUI programs as windows on the client), audio,
   drives. Test first: an xtask client from the host through a
   forwarded port; mstsc in the Windows test lab as the reference.
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
5. Later: several desktops per session, a secure desktop; H.264 in
   RDP's graphics pipeline (an encoder and its licence); remote shasha,
   gala's own protocol (the server owning window state, the client the
   chrome), only if a need RDP cannot carry is shown (a GUI kept across
   a handover with no visible reconnect). Interim, with no gala work:
   QEMU's `-vnc`.

**Done when**: B3b: an RDP client (xtask's, then mstsc on real
Windows) connects through a forwarded port with network level
authentication, sees a test program's window, clicks, the program
reports WM_LBUTTONDOWN at the right client coordinates, and a reconnect
after a disconnect gets the same session back. B4: two remote clients
each log on to their own session and shell.

**Design**: ARCHITECTURE "Plan: sessions and remote desktops".

### GPU. Graphics and GPU

**Goal**: software rendering first, so everything works on every VM and
server; paravirtual hardware next; vendor GPU drivers (WDDM) never in
the kernel.

**Done so far**:
- G0: `dxgi.dll`, `d3d10.dll`, `d3d10_1.dll`, `d3d11.dll`, `d3d12.dll`,
  `dcomp.dll` as stubs failing with DXGI_ERROR_UNSUPPORTED (outputs
  NULL, each call in the ledger); `gpu_stub_test`; Chromium's GPU
  process takes it as "no GPU", and Office's shared code can bind the
  Direct3D 10 entry points it delay-loads (ROADMAP OF).
  `d3d10_1.dll`'s D3D10CreateDevice1 is no longer one of them: it gives
  a real device now, over whichever Direct3D 11 the program brought
  (G2 below).
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
  `HKLM\SOFTWARE\Khronos\Vulkan\Drivers`; gala's cfgmgr32 answering
  the registry's device tree, which carries no Vulkan driver value
  (`cfgmgr32_test`); dxgi's factory exists with no adapters (the
  loader needs it; `gpu_stub_test`); ntdll resolves `.` and `..` in DLL
  paths (`loader_test`); `vk_test` (a triangle read back, then presented
  through VK_KHR_win32_surface and checked on the screen).
- G2, Direct3D 11: DXVK 3.1.1 (PREBUILT `dxvk`) over that Vulkan stack,
  its `d3d11.dll` and `dxgi.dll` beside the program (System32's dxgi
  stays gala's, the one the Vulkan loader calls); gdi32's 19 `D3DKMT*`
  thunks (STATUS_NOT_SUPPORTED, in the ledger) and ucrtbase's
  `mbsrtowcs` and `wctype`, which its import tables need; `d3d11_test`
  (a device and a swap chain, a cleared back buffer with a copied
  block, read back, presented and checked on the screen). No shader,
  so no HLSL compiler yet. `office_word` ships the same pair beside
  WINWORD.EXE and Word creates a D3D_FEATURE_LEVEL_11_0 device there,
  which is how a program other than a test gets one (ROADMAP OF).
- G2, Direct3D 10.1: gala's own `d3d10_1.dll` (`user/d3d10_1`,
  ARCHITECTURE "Direct3D 10.1") over whichever `d3d11.dll` the program
  brought, since DXVK dropped Direct3D 10 in 2.0: a merged device and
  context answering ID3D10Device and ID3D10Device1 (feature levels,
  textures, Map/Unmap, CopyResource, format support) whose objects
  answer QueryInterface with the real Direct3D 11 ones underneath, so
  Direct2D draws into them as into any DXGI surface; everything else of
  the interface records its own name. `d3d10_1_test` (a device over
  DXVK on lavapipe, a texture, a Direct2D 1.0 surface render target
  over it, the frame read back through a staging texture). It is what
  Word draws with (ROADMAP OF).

**Next**:
1. `virtio_gpu_reset`'s QEMU wedge (open): once on CI, QEMU stopped
   answering its monitor (no `x`, no `info registers`, no NMI) after the
   guest started its program; not reproduced locally. A failed log now
   carries every QEMU thread's stack when the monitor gives nothing:
   the next occurrence says which thread holds QEMU, and whether the
   device reset (`gpureset=`) is in it.
2. G1's rest: shasha's pointer on the hardware cursor,
   WM_DISPLAYCHANGE and refitting maximized windows on a resize (a test
   resizing the display through QEMU's monitor), several scanouts as
   monitors, device interrupts instead of spinning, the boot splash on
   virtio-gpu (the screen is black from the driver's start until the
   desktop draws).
3. G2's rest: a drawn triangle in `d3d11_test` (DXVK is in, but
   nothing is drawn through the pipeline yet), then DXVK's d3d9 and
   d3d10core and its own tests, then vkd3d-proton 3.1 (d3d12,
   d3d12core) beside the program the same way. Drawing needs DXBC
   shaders, and the host has no HLSL compiler (Ubuntu's vkd3d-compiler
   1.2 predates vkd3d's HLSL): build a current vkd3d-shader compiler
   for `build.sh`, or assemble the two shaders with their checksum.
   SwiftShader stays the fallback should lavapipe fall short. WARP is
   ruled out (not redistributable). gdi32's GetGlyphOutline (Mesa's
   wglUseFontOutlines) is still GDI_ERROR.
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

### SND. Sound

**Goal**: programs play sound (and later record it) through the
Windows audio APIs they use, on a virtio sound device first, with what
was played checkable in a test. Not urgent (the owner): after the
current focus.

**Today**: nothing. No audio driver; winmm has its timer functions
only and reports no audio or MIDI devices (ARCHITECTURE "Other
DLLs"); no MMDevice API, WASAPI, DirectSound or XAudio2.

**Next** (each step test first):
1. The virtio-sound driver (virtio 1.2's sound device; QEMU's
   `virtio-sound-pci`) over the modern virtio-pci transport GPU G1
   uses: its streams, PCM formats and the period/buffer model, the
   driver host-tested against a fake device as virtio-gpu's is.
   Scenarios run QEMU with `-audiodev wav`, which writes everything
   played to a file: a test plays a known tone and xtask compares the
   file (frequency, length, no gaps), deterministic and without a sound
   card on the host.
2. The audio device list and WASAPI (`mmdevapi.dll`,
   `audioses.dll`: IMMDeviceEnumerator, the default endpoint,
   IAudioClient in shared mode with event-driven buffers,
   IAudioRenderClient, the mix format), which Chromium's audio service
   and most current programs use; `audio_test` against real Windows
   (device listed, a stream started and fed, its position advancing).
   Wine's mmdevapi and winmm (LGPL) are allowed sources (THIRD_PARTY.md
   for anything borrowed).
3. On top: winmm's waveOut and PlaySound, then DirectSound.
4. Later: capture (a microphone; QEMU's wav backend has no input, a
   scripted source instead), Intel HD Audio for real machines (Intel's
   published HDA specification), exclusive mode, XAudio2, sound sent to
   RS's remote client (ARCHITECTURE "Remote access", the client's
   "sound").

**Done when** (each step): its tests pass; stream starts, stops,
underruns (the device starved) and the device's errors in an `events=`
category, and the streams per process in the state dump.

**Design**: to be written in ARCHITECTURE with step 1 (the driver, the
kernel interface the audio DLLs use: shared buffers and a position
event, as GPU's `GalaGpu*` calls are planned; handover: the device is
reset, streams restart).

### USB. USB: xHCI, keyboards, mice and storage

**Goal**: gala drives USB through its own xHCI driver, so USB
keyboards, mice and sticks work, first on QEMU's `qemu-xhci`, then on
real machines; Windows programs see USB devices through the APIs they
use. Wanted next (the owner), its HID part first: it is what stands
between gala and a machine of the owner's own, where the desktop comes
up and cannot be touched.

**Today**: nothing. The keyboard and mouse are PS/2, every device is
polled, and xHCI is only listed among gala's own drivers under
"Later". (The image boots from a USB stick through the firmware, not
through gala.) What a machine without PS/2 devices gets is the
diagnosis and nothing more: the input thread says `input: the PS/2
controller (ports 0x60 and 0x64) never answered` (or which of the two
devices answered, when one did) and stops, while the desktop keeps its
clock, its windows and whatever input a presenter brings (scenario
`no_i8042`, QEMU's `-machine q35,i8042=off`; ARCHITECTURE "Screen,
devices and boot splash"). A laptop is the usual case: many have a
PS/2 keyboard and touchpad behind the firmware's emulation, and the
newest have neither.

**Next** (each step test first):
1. Device interrupts: MSI/MSI-X for PCI devices, the virtio devices
   moved off polling first (their scenarios unchanged), as xHCI's
   event rings need them.
2. The xHCI driver and hubs, from Intel's published xHCI
   specification and the USB 2.0/3.x specifications: rings, slots,
   enumeration, control and interrupt transfers, hot plug, a device
   unplugged mid-transfer. Host tests against a fake controller; a
   scenario with QEMU's `qemu-xhci`, devices plugged and unplugged
   through the monitor.
3. HID: the boot protocol for `usb-kbd` and `usb-mouse`, then report
   descriptors; `usb-tablet` gives an absolute pointer, into the
   desktop's input as the PS/2 driver's events are (scripted screen
   tests can then click by position). With steps 1 and 2 this is all a
   real machine needs to be used: it comes before storage, and the
   first test on hardware is the owner's own laptop (its internal
   keyboard and touchpad are USB on the newest machines, and an
   external keyboard and mouse are USB on every machine).
4. Mass storage (bulk-only transport, SCSI commands) as a disk under
   gala's file systems; `usb-storage` in a scenario.
5. What Windows programs see: `hid.dll` and HID collections, device
   enumeration through SetupAPI and cfgmgr32 (the display's two
   devices today, out of the registry, with no tree shape to walk:
   `cfgmgr32_test`), raw input for HID devices, WinUSB for programs
   talking to their own device; tests against real Windows with the
   same QEMU devices.
6. Third-party USB device drivers through DR's user-mode driver host
   (UMDF); user-space Linux USB (libusb) with LX's device interfaces.

**Done when** (each step): its tests pass; enumeration, plug and
unplug, transfer errors and stalls in an `events=usb` category; the
device tree in the state dump and in `gala devices`.

**Design**: to be written in ARCHITECTURE with step 2 (the driver,
the device tree, handover: the controller reset and devices enumerated
again by the next kernel).

### WF. Wi-Fi: a dynamic interface, 802.11 and the Windows APIs

**Goal**: on a real machine gala joins a Wi-Fi network - the networks
in range listed from the taskbar, a password typed once and kept, WPA2
and WPA3, the address and DNS from the network - and Windows programs
see a connected wireless adapter through the APIs they use (wlanapi,
the network list manager, NCSI). It is what makes a machine of the
owner's own useful once USB's HID part lets it be touched at all.

**Today**: nothing of it. The network is one virtio-net card at a fixed
10.0.2.15/24 with QEMU's 10.0.2.2 as its gateway, polled with no
interrupts, IPv4 only (N3 step 3 is parked); the interface list is
static (ARCHITECTURE "Network interfaces"), so there is no DHCP client,
no address that changes, no GetAdaptersAddresses and no
NotifyAddrChange (both ERROR_NOT_SUPPORTED, recorded), nothing of
802.11 and no wlanapi.dll. What the radio's own cryptography needs is
largely there already, in the primitives TLS brought (SHA-1 and SHA-2,
HMAC, AES, elliptic curves).

**Next** (four layers, each test first; the first two are weeks of
work, the third is an item's worth and the fourth is the biggest and
waits on DR):
1. **A network that is not fixed**: a DHCP client (QEMU's user
   networking answers one, so this is testable today over the
   virtio-net card), the interface's address, prefix, gateway, DNS
   servers and lease taken from it, the lease renewed, and an
   interface that comes and goes - which means the dynamic interface
   list the whole stack and `GALA_NETWORK_INTERFACES` assume away
   today, GetAdaptersAddresses and NotifyAddrChange over it, and the
   clock's jump and the renewed lease on a snapshot's resume or a
   handover (R9, SN). Static addressing stays what the scenarios use.
2. **What a Windows program sees, before there is a radio**: wlanapi's
   surface proved against real Windows first (`wlan_test`:
   WlanOpenHandle, WlanEnumInterfaces, WlanScan,
   WlanGetAvailableNetworkList, WlanConnect and its notifications,
   profiles), the network list manager (INetworkListManager and its
   events), NCSI's connectivity probe and the taskbar's network icon
   and flyout (item SH's shell, item SD's rule: the `gala` command
   reaches it first). In gala it runs over a simulated adapter with
   scripted networks, as USB's host tests run against a fake
   controller, so every layer above the radio is finished and tested
   before a card exists.
3. **802.11 and a supplicant**, from the published IEEE 802.11-2020
   standard and the RFCs (CLEAN_ROOM): scanning, authentication and
   association, the WPA2-PSK four-way handshake (PBKDF2 for the PMK,
   EAPOL-Key, CCMP), WPA3's SAE, protected management frames, roaming
   between access points. Host tests of the state machines against the
   test vectors the standard and the RFCs publish; the only real test
   is hardware with a real access point, since QEMU emulates no
   wireless card.
4. **The card**: a laptop's Wi-Fi is an Intel or Qualcomm part with a
   vendor NDIS miniport and signed firmware, so the route is DR stage
   2's isolated driver host with the NDIS miniport framework ("Later":
   a third-party driver never runs unsupervised in gala's kernel
   address space). gala's own driver for one published card is the
   alternative where a datasheet exists and no firmware is needed, for
   one machine rather than for machines in general.

**The cheap way to a network on real hardware** meanwhile, which this
item does not need: a USB Ethernet adapter over CDC-ECM or NCM on item
USB's stack, both documented class specifications and no vendor driver
at all. It gives a real machine a wired network as soon as USB's steps
1 and 2 are in, and it exercises layer 1 above (a second interface
that comes and goes).

**Done when** (each layer): its tests pass; the lease, the scan
results, the supplicant's state machine and every association failure
in an `events=net` category with the reason, the interfaces and their
state in the state dump and in `gala devices`; a real machine joins the
owner's network from the taskbar.

**Design**: to be written in ARCHITECTURE with layer 1 (the dynamic
interface list, the DHCP client, what a handover carries).

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
   with free frames left: which limit (commit, reservations, a quota).
   Not reproduced under KVM on the Windows machine, over the whole
   `dom/nodes` directory (406 tests) nor in the `XTASK_WPT_LIMIT=20`
   run, with no memory pressure (fewest frames free 469219 of 673061);
   the run keeps one session and starts a new browser only when one
   dies, so the condition needs a browser that died, and what it was
   seen under (TCG, the VM's memory) decides where to look next;
   (c) ChromeDriver not exiting after the client's /shutdown (the
   scenario then runs to its limit): under KVM `chromedriver.exe`
   finished with status 0x00000000 after 406 tests, twice, so it wants
   a TCG run or the conditions it was seen under; (d) done under KVM:
   406 tests, 404 harness OK, 13326 of 13397 subtests pass against
   edge-159-win's 13343 (`user/windows/wpt/` records it); next, the
   other directories and the comparison with Chrome on Windows
   (wpt.fyi). The report itself: a subset of web-platform-tests
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
  - Direct2D (`d2d1`, ARCHITECTURE "Direct2D"; `d2d1_test`, x64, the
    `d2d1_window` screen scenario and `d2d1_device_test`): done for the
    factory (both kinds, ID2D1Multithread, ID2D1Factory1 and
    ID2D1Factory2; the ordinals
    Office imports), the matrix
    helpers, software render targets over WIC bitmaps, device contexts
    (BindDC) and windows (hardware requests drawn in software), solid,
    linear, radial and bitmap brushes, bitmaps (from memory and WIC),
    rectangles, rounded rectangles, ellipses, lines and path geometries
    (lines, Béziers, arcs, fill modes) filled and stroked with widths,
    caps and joins, antialiased or aliased, transforms and axis-aligned
    clips, DrawBitmap; and Direct2D 1.1 and 1.2 over the program's own
    Direct3D 11 device (GPU G2's DXVK): CreateDevice on either factory,
    CreateDeviceContext on either device (rendering priority),
    a target bitmap over a DXGI
    surface (CreateBitmapFromDxgiSurface), geometry realizations drawn
    at the rasterizer's own tolerance, and a frame drawn in software
    written back into that surface with UpdateSubresource. Direct2D
    1.0's own way onto such a surface is there too, which is the one
    Word takes: CreateDxgiSurfaceRenderTarget over a texture of a
    Direct3D 10.1 device, and the target it gives is a device context
    over a bitmap of that surface, which is what every Direct2D render
    target has been since Windows 8 and what a program on the 1.0 path
    asks it for (`d3d10_1_test`). Drawing state blocks are there as well, on both
    factories and in one object answering ID2D1DrawingStateBlock and
    ID2D1DrawingStateBlock1, with the text-rendering parameters
    DirectWrite now makes (the `dwrite` item); so are meshes
    (ID2D1Mesh, its tessellation sink, FillMesh triangle by triangle,
    aliased only as Direct2D has it) and the options, alpha mode and
    DPI Direct2D gives a bitmap over a DXGI surface when the program
    asks for none, with D2DERR_INVALID_TARGET for a frame whose target
    bitmap is no target. Next, as
    Office's use shows it (the ledger names
    each): text (DrawText/DrawTextLayout, which needs
    DirectWrite's text
    layout first), layers, compatible render targets, the GDI interop
    interface, dashes, effects and command lists.
    `d2d1_test` runs on Windows too, which is where its expectations
    come from (its first run, issue #94, stopped at D2D1CreateFactory:
    IID_ID2D1Factory had two bytes wrong in both the test and the DLL,
    now d2d1.h's value). The bounds are Direct2D's own now, each
    measured on Windows and checked in the test: GetBounds is the
    fill's, with hollow figures out of it and +inf corners and +FLT_MAX
    extents for a geometry with nothing to measure, and
    GetWidenedBounds is the stroke outline's, widened in the geometry's
    own coordinates (the transform applies to the outline afterwards)
    with the stroke style's caps, joins and miter limit in it
    (ARCHITECTURE "Direct2D"). One difference between gala and Direct2D
    is left, with an `info` line of its own and the checks around it
    banded rather than exact: a half-covered or half-transparent
    channel can be one off, as gala composes in float and rounds half
    up where Direct2D quantises the source's alpha to 8 bits first and
    blends in integers (so 50% red over white is ff8080 here and
    ff7f7f there) -- to be settled by blending the way Direct2D does,
    not by widening the bands further.
- **Install** (Click-to-Run): the `ClickToRunSvc` service (automatic,
  LocalSystem, `OfficeClickToRun.exe /service`), its integrator, and
  ten scheduled tasks; ODT's own `setup.exe` is 32-bit (below).

**Next**:
1. The load list above, test first (a kernel32/ntdll test program for
   the functions, expected results from Windows), until the three
   apps' processes start in gala with Office's files in place. The
   scenarios that boot them are there: `office_word` and `office_excel`
   (MANUAL, WORKFLOW "Word and Excel on gala's own install"), each on a
   copy of the disk `office_install_local` installed on -- Word makes a
   document from a template of that install, Excel opens the blank
   workbook it put in its shell-new folder, so no file comes from
   anywhere else (the owner). The screen checks they end in are
   written (`OFFICE_WORD_SCREEN`, `OFFICE_EXCEL_SCREEN`): the window
   Windows draws, checked where the program is free to put it -- at
   least a tenth of the 1280 x 800 screen flat white (Word's page,
   Excel's cells; the bare desktop has 0.008% of it, the wallpaper's
   brightest) and the active window's pill on the taskbar, which the
   bare desktop has nothing near -- then Alt+F4, and consolerun ends
   with the app's own status, 0.
   Neither run draws the window its check wants yet. Word's puts its
   untitled splash window (MsoSplash) on the screen and walks the whole
   Direct2D and DirectWrite path the ladder below answers: every method
   of both DLLs records the first call that reaches it (OBSERVABILITY
   "Missing-features ledger"), and the last one Word makes is
   ID2D1RenderTarget::CreateSolidColorBrush. Nothing follows it -- no
   method on the brush it asked for, no BeginDraw, no geometry, and no
   exception the program handles itself -- and with nothing of the frame
   drawn Word prints "Word was unable to start. (13)" and exits with
   status 0 after 31.2 s. What it does between that call and the
   message is read, and holds no gap: it opens the absent
   `16.0\Word\Options\Debug` key eight times, reads the four display
   answers gala gives it (SystemParametersInfo 0x4a, 0x200a and 0x2006,
   GetSystemMetrics 45 and 46), loads `uxtheme.dll` and probes the
   Common-Controls side-by-side assembly -- the message box being built
   -- and makes no further Direct2D, DirectWrite or DXGI call, and no
   call of any kind that fails. The twelve registry values it looks for
   and does not find in that last second are options a fresh Windows
   profile lacks as well (`ForceConcurrencyOverride`,
   `ScreenReaderPresent`, `NoClearTypeNW`, `NoTranslucentSelection` and
   the three font-smoothing thresholds, read twice each; named by the
   `value` events of `events=file`), so no absent value decides it.
   Word decides on answers gala gave, which moves the question from
   which call is missing to what those answers say -- and it writes its
   own account of the exit to the registry right after the message: 401
   bytes of REG_BINARY to `Office\Common\GracefulExit\WINWORD\12:2` and
   2 bytes to `:3`, the previous run's 360-byte
   `CrashPersistence\WINWORD\12:4` read and deleted first. Gala
   persists the registry, so dumping
   `C:/Windows/System32/config/gala.reglog` (OBSERVABILITY "Guest
   files") and reading its `GREG` records
   (`kernel/src/nt/reglog.rs`) is where that blob can be read. Word
   writes no log of its own to read instead: of the nine ULS logs the
   run leaves, every one is the install's (SETUP, OFFICECL, INTEGRAT)
   and none is WINWORD's. The probes of the absent `Word\Options\Debug`
   and `16.0\Common\Debug` keys are not where its logging is switched
   on, which is measured rather than assumed: seeded empty, the two keys
   make Word name every value it looks for in them -- `TCOTrace` in
   `Common\Debug`, and `ShowScreenUpdates`, `DisablePTSLineCache`,
   `SlowTyping`, `SlowTypingChunkMax`, `NoUpgradeLegacyObjects`,
   `ForcedXDpi` and `ForcedYDpi` in `Word\Options\Debug` -- every one a
   drawing or typing knob a fresh profile lacks as well, with no log
   among them, and the run ends at the same message at the same second.
   Word does trace, and its own words about the exit are in what it
   traces. A startup registers 20 ETW providers and writes nothing of
   its own accord, because TraceLogging builds an event's payload only
   once a session has enabled the provider -- so gala is the session
   (`etw=`, OT4, OBSERVABILITY "A session gala starts"), and under
   `etw=*:255:0:4096 etw_from=WINWORD` the startup writes **353 events
   from 8 providers**, every one of them WINWORD.EXE's. Every event is
   a manifest provider's, whose field names live in a resource outside
   the program, so what a line reads of one is the text its bytes
   carried (`kernel/src/nt/etw/strings.rs`) -- and that text is Word's
   account of its own failure, in this order:
   `ErrorEid {"SH_ErrorId": 66, "SH_ErrorCode": 13}`, then
   `SHIP ASSERT FAILED! {"wzTag": "qsqk"}`, then the message box, then
   `BootRun {"SH_ErrorId": 13, "BootTask": "IfrRegisterWndClasses"}`.
   The `(13)` the box shows is Word's own SH_ErrorId, and the boot task
   it died in is `IfrRegisterWndClasses`: **Word registering its window
   classes**. That is why nothing in the Direct2D ladder above explains
   the box -- the drawing path Word walked is answered to its last
   call, and the step it stopped on is a window class rather than a
   frame -- and it is why the graphics measurements ruled themselves
   out one at a time. The rest of the 353 is Office narrating the same
   stretch: the virtual registry naming WINWORD.EXE's own path,
   `Office.Performance.Boot` and six further activities started and
   detached, 7 `Failed to register WER metadata` errors, 27
   `RegistryWrapper::ReadFromOrapi > Registry read failed` lines, a
   `Use of CForegroundDispatcher outside of liblet initialization`, and
   after the box `WordShutdown` and AirSpace's compositor finishing its
   device. That boot task is answered. The table those
   classes went in held a fixed 32 of them -- eight already spent on
   the built-in control classes `controls.rs` registers, though those
   are registered lazily and a program gets all 32 until it asks for
   one -- and a full table answered 0 with ERROR_NOT_ENOUGH_MEMORY and
   said nothing at all, which is why the ledger carried no row while
   Word died of it. The table grows now: 32 slots on the process heap,
   doubled as they fill, bounded by the application atom space
   (16384) because every class has an atom, which is Windows' own
   bound and not a number gala chose; a slot's index is its class's
   atom, so register and unregister no longer leak atoms and no two
   live classes can share one; a refusal records the class name with
   the slots held, and each doubling records the size it reached
   (ARCHITECTURE, OBSERVABILITY, `wndclass_test`: 200 classes, their
   atoms distinct, the table growing 32-64-128-256). What Word needed
   is measured by that row: **the table grows to 64**, so Word's
   startup registers more than 32 classes and the 33rd was the one
   refused. With it, `IfrRegisterWndClasses` is passed -- the
   `ErrorEid`, the `SHIP ASSERT`, the `BootRun` and the (13) box are
   all gone from the run, which lasts 36.6 s where it lasted 32.3 s and
   writes 393 events from 10 providers where it wrote 353 from 8. That
   is measured on a fresh disk, not a reused one: Word's own crash
   state makes the run after a crashed one open with its safe-mode
   question instead (WORKFLOW "Word's own crash state").
   Word stops on an exception instead, and the next piece of work is
   that exception rather than anything of Word's: `Mso30win32client.dll`
   raises 0xe0000002 through kernel32's RaiseException (record flags
   0x80, EXCEPTION_SOFTWARE_ORIGINATE) and gala's dispatch asks **0
   handlers** before ending the process, because the unwind stops
   inside gala's own ntdll -- 3 frames unwound, the third
   `ntdll.dll+0xca85` with **no unwind data found**, and the walk
   ending at "no return address: the stack's end". That offset is
   inside **`KiUserApcDispatcher`** (it starts at 0xca70, and 0xca85 is
   the instruction after its `call` of the APC routine), and ntdll's
   own `.pdata` confirms the gap rather than only the crash report
   saying so: its entries run ... 0xca34, 0xca4e, then 0xceb0, with
   nothing covering the dispatcher at all. So Word's exception is
   raised inside an APC -- kernel32's RaiseException, called by
   `Mso30win32client.dll`, called by the APC routine the dispatcher
   invoked -- and the handler that would take it is below the
   dispatcher, in the frames the thread was running before the APC
   came. Windows unwinds through its own dispatcher; gala's cannot,
   because a frame with no unwind data ends the walk. It is the only
   entry point of ntdll's that lacks one: `KiUserExceptionDispatcher`
   (0xca34) and `LdrInitializeThunk` (0x2610) both have their `.pdata`
   entry, which is why an exception raised in ordinary code unwinds
   and one raised under an APC does not. The work is to give
   `KiUserApcDispatcher` unwind data too, with a test that raises an
   exception inside an APC and catches it in the frame the APC
   interrupted -- `user/apc_test` is where that goes. A program whose own
   handler never gets asked is a gap of the same kind the class table
   was, one layer down.
   The same silent-refusal shape is still in user32's other fixed
   tables, each to be measured against a real program rather than
   widened: `accel.rs` 32 accelerator tables (`MAX_TABLES`), `menu.rs`
   64 items per menu (`MAX_ITEMS`, which a Word menu passes on its
   own) and 512 menus, `timer.rs` 128 timers, `surface.rs` 16 foreign
   surfaces. All four answer 0 or false when full and record nothing,
   so a program they stop leaves a run with no trace of why -- the row
   is the cheap half of that work and comes first.
   An absent key names nothing it was asked for, so a key seeded empty
   is how a program is made to say what it wanted: the `value` events of
   `events=file` then carry each name. Office's own logging switch,
   seeded in `user/office_install/uls_logging.regseed`, is read only by
   OfficeClickToRun.exe; Word never opens `16.0\Common\Logging` at
   all. DXVK's two logs, dumped with the run, name nothing gala
   refuses -- the three `DXGI: Found monitors not associated with any
   adapter, using fallback` warnings in `winword_dxgi.log` are the
   absent Vulkan device LUID below, where that warning is read line by
   line, and not a gap. `winword_d3d11.log` holds **two** devices on
   llvmpipe, both whole: each creation and each finish is there, the
   10_1 device's and the 11_0 one's, and the file ends on the second's
   resource descriptor heap because nothing is asked of DXVK after it
   (how to read its blocks, and the mis-split that makes the second
   look like a device that stopped, is below). The Direct3D 10.1 device
   is one of the two and Word is given it: the ledger has
   `d3d10_1.dll!D3D10CreateDevice1` answered 0 at 46260 ms, and its one
   unsupported flag (`ALLOW_NULL_FROM_MAP`, detail 16) recorded at
   45657 ms where the call began, so the 603 ms between them is
   lavapipe building a device Word then holds, and the box comes after
   it. Holding two devices at once is not what stops Word either:
   `d3d11_test` asks for a second one the way Word does -- a second
   `D3D11CreateDevice` while the first device and its swap chain are
   alive, at its own feature level (10_1), with a texture made on it --
   and it passes. What Word asks the device it was given is the next
   thing to read, and the ledger cannot say: it holds no
   `d3d10_1.dll!ID3D10Device1::` row at all, so every method Word
   called is one gala answers, and which they were and in what order is
   what `trace_from=` records: d3d10_1's slots are wrapped now, the
   device's implemented methods and its eighty-seven E_NOTIMPL stubs
   alike (`cargo xtask gen-trace`, scenario `trace_from_d3d10_test`,
   OBSERVABILITY "Call tracing"), and the reading is made. Armed at
   `d3d10_1.dll!ID3D10Device::CheckMultisampleQualityLevels`, the last
   of its methods the ledger shows Word reaching, a run records three
   calls and all three answers:
   `CheckMultisampleQualityLevels(B8G8R8A8_UNORM, sample count 2)`
   answered S_OK, then `GetDeviceRemovedReason` twice, S_OK each time;
   a run that gets further adds `CreateTexture2D` answered S_OK and
   the whole Direct2D render-target, bitmap and brush sequence after
   it. So the device refuses Word nothing, and the two
   `GetDeviceRemovedReason` answers say Word is not being told its
   device went away either. The same runs found the thing to hold on
   to instead: Word's path varies between them. Of five `office_word`
   runs three stop within 350 ms of that second
   `GetDeviceRemovedReason` having made no Direct2D call at all, and
   one goes on through the render target to `CreateSolidColorBrush`
   and the box nine milliseconds after it; an untraced control run
   took the short path too, so the variance is Word's own and not what
   the stream costs. The short path's silence after the device call is
   measured now rather than inferred: those runs were armed per DLL,
   which could not have recorded a Direct2D call if Word had made one,
   and four runs armed at the same method since the arming became the
   process's record the same three calls and nothing else -- three
   `call` events and three `ret` events in the whole stream, all three
   answered S_OK, no `d2d1.dll!` line among them. So Word asks the
   device the three things, is refused nothing, and gives up without
   reaching Direct2D at all. What it does between that second
   `GetDeviceRemovedReason` and the box is therefore no call into a
   DLL gala wrote at all. Nor is the run's one dxgi record Word's:
   `CreateDXGIFactory1(interface)` unsupported three times at 37.4 s
   is the Vulkan loader asking for IDXGIFactory6 as it sorts adapters
   (below), and the refusal leaves it holding no factory, so a wrapped
   dxgi slot would have no object to be called on either. The decision
   is in Word's own code or in a Microsoft DLL, where the stack at the
   box's write and the import trace below are what can reach.
   Both paths end the same way, which `events=file`
   on a short-path run shows (`open`, `key` and `value` events,
   OBSERVABILITY "Event stream"): the last thing before the box is
   `uxtheme.dll` loaded from System32 after the Office16 miss, then
   the eight private `Microsoft.Windows.Common-Controls` probes, and
   63 ms later the box -- the box being built, as on the long path
   below, not what decided it. So the long path's 9.2 ms of user-mode
   work is not where the decision has to be: the short path gives up
   before Direct2D, and neither run records anything failing.
   Nor is it an add-in, a template or a user setting, which is the
   class Word's own remedies for this message on Windows point at
   (safe mode, disabling add-ins, renaming `Normal.dotm`). Started
   with `/safe` in place of the scenario's template argument --
   `XTASK_CMDLINE='args=%22C:\Program%20Files\Microsoft%20Office\root\Office16\WINWORD.EXE%22%20/safe'`,
   which the kernel takes over the scenario's own `args=` (WORKFLOW's
   boot-option table) -- Word runs with add-ins, the global template
   and its customizations all off, and fails the same way to the
   second: the same `(13)` box at 39.7 s, its own exit after 32.7 s.
   The message itself is Word's own `user32!MessageBoxW`, and the stack
   at its write names who asked for it: `Mso20win32client.dll`'s frame
   directly above user32's, then `MSO.DLL` and `Mso30win32client.dll`
   among WWLIB's own frames down to `WINWORD.EXE` itself, with
   `WWINTL.DLL` beside the write. Aim the capture at the module that is
   *in* the call: `stack_at=NtWriteFile:6 stack_from=user32
   stack_words=400` catches the box's two writes and nothing else
   (OBSERVABILITY "Stack captures"), where `stack_from=wwlib` spends
   captures on C2R64's own log writes early in the run, WWLIB being
   only a dead word on those stacks. Excel's ends with EXCEL.EXE raising 0xc06d007f after 30 s: the
   delay-load helper's "not found" for user32's WaitForInputIdle, the
   one gap its run names -- nothing of Office's shared code fails in
   it, so Excel's next step is that function and a rerun. The ledger's
   25 missing `c2r64.dll!` exports are not a gap: they are the AppV ISV
   shim's optional hooks (`EnsureFile`, `LoadRange`, `StreamFault`,
   `HandleLoadIconW`, ...), and the real `C2R64.dll` the ClickToRun
   directory holds exports none of the 25 either (101 exports,
   measured), so the same probes miss on Windows.
   Two things, both next:
   - Under the Office root Windows has 16 reparse points (`C2R32.dll`,
     `C2R64.dll`, `AppvIsvSubsystems32.dll`, `AppvIsvSubsystems64.dll`,
     each in `\Client`, `\Office16`,
     `\vfs\ProgramFilesCommonX64\Microsoft Shared\Office16` and
     `\vfs\ProgramFilesX86\Microsoft Office\Office16`) whose target is
     the file of the same name in
     `C:\Program Files\Common Files\Microsoft Shared\ClickToRun`.
     Gala's install holds that directory's real DLLs, Windows' sizes to
     the byte, but has the 16 as plain empty files: what makes them is
     the integration step that fails (`ConfigureMSIs`, "Final
     Extensions Publishing", 1603 and Integrator 0x643 below), which
     waits on PK's MSI engine.
   - What Word loads, and where it stops. The 24 user32 functions `mfc140u.dll` imports statically
     that gala lacked are exported now, Windows' results measured first
     (`win32_misc_test`'s `mfc` section, 158 checks): the dialog-item
     helpers (CheckRadioButton, IsDlgButtonChecked, Get/SetDlgItemInt,
     SendDlgItemMessageA, GetNextDlgGroupItem), the menu functions
     (GetMenuCheckMarkDimensions, GetMenuDefaultItem,
     SetMenuItemBitmaps and LoadMenuW, which reads a template of
     either version), LockWindowUpdate, RealChildWindowFromPoint,
     InvalidateRgn, IsCharLowerW, GetTabbedTextExtentW, TabbedTextOutW,
     GrayStringW, Set/GetWindowContextHelpId, the four DDE lParam
     helpers and WinHelpW; what they approximate is in ARCHITECTURE
     ("Menus", "Dialogs", "Controls" and the painting bullets) -- a
     locked window's drawing is not held back, an item's check-mark
     bitmaps are kept but not drawn, GrayStringW grays by colour, and
     WinHelpW answers only HELP_QUIT. Of the 247, nine are `gala_stubs`
     stubs, which bind (so MFC loads) and end the process when called:
     ChildWindowFromPoint, CopyImage, DefFrameProcW, DefMDIChildProcW,
     DrawStateW, GetDCEx, GetKeyNameTextW, HideCaret and
     TranslateMDISysAccel. None is reached yet, and neither is
     ChildWindowFromPointEx, which user32 does not export at all.
     What the shim's DllMain waits for is the AppV ISV RPC server in
     the service, and that comes up: it registers
     `APPV-VIRTMAN-NOTIFICATIONS`, `APPV-jitv_server`,
     `APPV-VREG_SERVER` and `LRPC-C2RClientAPI_Server_System16` at
     34.3-36.4 s of the run, and the shim connects to two of them (the
     `no server at pipe` records before that -- 87 for the C2R client
     API, one every 0.25 s -- are the shim retrying while the service
     starts, and they end in a connect at 34.44 s). The lesson of the
     0x7e that stood here before: the server's setup takes
     `GetModuleHandle("Rpcrt4.dll")` and gave up with
     ERROR_MOD_NOT_FOUND (126) because nothing in that process had
     loaded rpcrt4, where Windows' advapi32, oleaut32, msi and combase
     all import it statically. gala's system DLLs carry Windows' own
     dependencies where a program can see them (ARCHITECTURE "The
     module graph", `kernel32_test`), and the two records that found it
     stay: a handle asked for a module that is not loaded is recorded
     with the name asked for (`kernel32.dll!GetModuleHandle(<name>)`),
     and a connect that finds no pipe with the endpoint's
     (`rpcrt4.dll!no server at pipe(<name>)`, the Win32 error as the
     detail, `rpc_test`).
     The virtual process environment the shim waits for now comes
     up: the service's `AppVEvents::VirtualProcessCreateFailed` and the
     `xmlutils.cpp` failures under it are gone, the ISV calls are
     answered, and `appvisvsubsystems64.dll` loads. Two engines were
     short of what the AppV package catalog under
     `C:\ProgramData\Microsoft\ClickToRun\MachineData\Catalog\Packages`
     and the deployment configuration need. msxml6 had none of the
     MSXML extension functions a selection may call, where the
     configuration compares strings with `ms:string-compare`
     (ARCHITECTURE "msxml6"). rpcrt4's NDR engine had two defects on
     the ISV interface's own parameters, each reproduced first in
     `rpc_test` against Windows' engine before it was fixed: a complex
     array's element is not a format character in place but a reference
     to the element's own description (FC_EMBEDDED_COMPLEX), and the
     unmarshal side dropped the referents that a deferred referent
     defers itself, where the marshal side wrote them.
     WINWORD.EXE runs its own code and loads Word: it maps WWLIB.DLL
     (59 MiB) and OART.DLL, where it ended at 0xC0000142 before
     reaching them and at status 1 when wwlib's own imports would not
     bind. The nine imports wwlib was
     short of are implemented, each measured on Windows first:
     kernel32's NormalizeString and IsNormalizedString (which normaliz
     forwards here too), EnumSystemLanguageGroupsW and EnumUILanguagesW
     (`office_k32_test`, ARCHITECTURE "Locales and text"), and six of
     oleaut32's by ordinal, RegisterActiveObject (33),
     RevokeActiveObject (34), VarR4FromR8 (71), VarR8FromR4 (81),
     VarBstrFromI4 (110) and VarBstrFromR4 (111) (`com_test`; the
     ordinals are Windows' own, `user/oleaut32/build.rs`) -- the two
     active-object calls over a running object table of the process and
     the two number-to-string ones over the exact decimal expansion the
     C runtime prints with, moved into `user/decfloat` so that both
     round alike (ARCHITECTURE "COM: ole32 and oleaut32"). The run's
     ledger has no `missing import` and no `load failed` left at all.
     Word finds Office's shared code: a process of a Click-to-Run
     package sees the package's `VFS\<token>` tree at the token's real
     path (ARCHITECTURE "A package's VFS view", `nt/pkgview.rs`,
     `pkgview_test`), so WINWORD.EXE's opens of
     mso20win32client.dll, mso30win32client.dll,
     mso40uiwin32client.dll, mso50win32client.dll,
     mso98win32client.dll and mso.dll at
     `C:\Program Files\Common Files\Microsoft Shared\Office16` are all
     served from the Office root's
     `VFS\ProgramFilesCommonX64\Microsoft Shared\Office16`, where the
     install keeps them (`cargo xtask disk-files`; the `package` file
     events name each one, `XTASK_EVENTS=file`). What Windows does
     there, measured on a machine with the same install: Word reports
     those six, MSPTLS.DLL and `1033\OSFINTL.DLL` loaded from that
     directory while no process on the machine finds it, those modules
     sit outside the loader's own address range, and none of the App-V
     services or drivers runs -- the view is Office's own loader naming
     what it mapped itself, not a file system anything keeps. gala
     makes the same files answer at the same paths from the kernel
     instead, for a process whose image is under a package root
     registered in `HKLM\SOFTWARE\Microsoft\AppVISV`, which is coarser
     than Windows' (every program of the package has the view) and says
     so where it lands. (OfficeClickToRun opens its own files by the
     package path directly, so the service needs nothing of this.)
     What stops Word now is what those six import: all six are laid
     out and mapped, and their imports do not bind.
     mso20win32client.dll fails first, STATUS_ENTRYPOINT_NOT_FOUND
     over kernel32's GetSystemFirmwareTable, and the five behind it
     fail STATUS_DLL_NOT_FOUND because each imports the one before.
     What they are short of, from the run's ledger, is 15 kernel32
     functions: GetSystemFirmwareTable (mso20win32client),
     RegisterApplicationRecoveryCallback (mso30win32client),
     ApplicationRecoveryFinished, ApplicationRecoveryInProgress,
     PulseEvent, GetDateFormatA, GetTimeFormatA and EnumCalendarInfoA
     (mso98win32client), GetPrivateProfileStringA, GetProfileStringA,
     GetProfileIntA, GetPrivateProfileIntW, WritePrivateProfileStringA,
     OpenFile, SetFileShortNameW and FindPackagesByPackageFamily
     (mso.dll). All fifteen are implemented, each measured on Windows
     first (`office_k32_test`, ARCHITECTURE "kernel32"): the recovery
     calls, GetDateFormatA, GetTimeFormatA, EnumCalendarInfoA, the
     profile A and Int forms, OpenFile and FindPackagesByPackageFamily,
     and the three that wanted the kernel -- PulseEvent (NtPulseEvent,
     "Synchronization objects"), GetSystemFirmwareTable (the machine's
     SMBIOS table from the boot protocol, "Firmware tables") and
     SetFileShortNameW (8.3 names in the ramfs, "Files");
     mso40uiwin32client and mso50win32client name nothing of their own,
     so they bind as soon as the chain does.
     With the chain bound, Word loads Office's shared code and its
     resources -- MSO40UIRES, MSO99LRES and MSORES (81 MiB) -- and puts
     a window on the screen, untitled, 29 s in. Two delay loads ended
     it before that, each over a DLL whose exports have to exist
     whatever they answer (one that resolves to nothing ends the
     process before any handler runs): msi.dll, short eleven functions the shared
     code binds by ordinal (0xc06d007f over ordinal 137), and
     d3d10_1.dll (0xc06d007e over D3D10CreateDevice1). Both are
     implemented, each answer measured on Windows first (`msi_test`,
     `d3d10_1_test`; ARCHITECTURE "Other DLLs", "Direct3D 10.1").
     What stops Word now is that its render target is not a device
     context. Everything up to that works. The rendering device it asks
     for is there: the scenario ships DXVK's `d3d11.dll` and `dxgi.dll`
     beside WINWORD.EXE, the directory the loader searches before
     System32, over the Vulkan stack in System32 (GPU G2), and DXVK
     creates a D3D_FEATURE_LEVEL_11_0 device on Mesa's lavapipe -- its
     own logs say so, and the scenario's `dump=` keeps them
     (C:/gala/winword_d3d11.log and winword_dxgi.log, OBSERVABILITY
     "Guest files"). With a device in hand Word no longer raises
     0xe0000002, the fail-fast it took when nothing answered. Direct2D
     1.1 and 1.2 over that device are there too (the Direct2D item
     above), but Word does not take them: it asks the factory for
     ID2D1Factory2 ({94f81a73-...}) and is answered, and that is a
     version probe -- it calls CreateDevice on neither interface, and
     the whole device path leaves nothing in its ledger, where the
     calls that work are recorded (OBSERVABILITY "Missing-features
     ledger").
     What it does instead, 39.0 s in, is Direct2D 1.0's own path,
     which is there now: one D3D10CreateDevice1 at feature level 10.1
     -- gala's own `d3d10_1.dll` over the Direct3D 11 beside the exe,
     ARCHITECTURE "Direct3D 10.1" -- then one
     ID3D10Device1::CreateTexture2D and one
     ID2D1Factory::CreateDxgiSurfaceRenderTarget over that texture,
     both S_OK and in the ledger. The target it is given is a device
     context as well (ARCHITECTURE "A DXGI surface render target"), and
     Word uses it as one: it takes the ID2D1Device behind it and makes
     a context of its own, ID2D1Device::CreateDeviceContext, S_OK, 2 ms
     later. That is where it used to ask for ID2D1DeviceContext1, be
     refused, and stop.
     Everything it asks that context for is answered now, and the
     ledger says how far that reaches: two
     ID2D1Factory::CreateDrawingStateBlock (the state a frame saves and
     restores, ARCHITECTURE "Drawing state blocks") with
     IDWriteFactory::CreateRenderingParams between them, then
     ID2D1Device::CreateDeviceContext, then
     ID2D1DeviceContext::CreateBitmapFromDxgiSurface over the texture
     and ID2D1DeviceContext::SetTarget with that bitmap -- every one
     S_OK, the last of them 37.8 s in, and nothing after it. The meshes
     Word asked for a run earlier (ID2D1RenderTarget::CreateMesh,
     unsupported) are there too and that record is gone. So Word now
     has a render target of its own with Word's own texture behind it
     and a frame it may draw into, and what it does with it is the next
     thing to find out: it draws nothing that reaches the window.
     Instead comes the "(13)" message box ("Sorry, something went
     wrong and Word was unable to start. (13)") on the untitled window,
     exit status 0 after 30.5 s, which is the status the scenario asks
     for, so only its screen check fails. After them comes text itself,
     which needs DirectWrite's text layout -- a document is text.
     The other gaps the ledger names around that exit, each to be
     measured on Windows before it is answered: kernelbase's
     SetSystemFileCacheSize over ntdll's NtSetSystemInformation (six
     times), HeapQueryInformation's class, and the interface gala's
     dxgi refuses CreateDXGIFactory1 (three times). gdi32's
     D3DKMTEnumAdapters2 and D3DKMTQueryAdapterInfo, which the Vulkan
     loader tries before the registry's list of drivers, are measured
     and answered: no adapters, and no handle ever issued to ask about
     (`user/d3dkmt_test`, `user/gdi32/src/d3dkmt.rs`). The sixteen
     lookups of each that the ledger used to name are a found export
     now, and the run makes one call: Word asks how many adapters
     there are, hears none, and never asks an adapter anything
     (`D3DKMTEnumAdapters2(no adapters)`, one hit, the room it offered
     0). Word stops where it did -- "(13)", status 0 after 32.8 s --
     so this was a gap and not the cause.
     A run with every system call traced from boot takes a different
     path, not a slower one: it stops in `MsoSplash`, asking
     RoGetActivationFactory for
     `Windows.Security.Authentication.OnlineId.OnlineIdSystemAuthenticator`
     over and over, a runtime class gala has none of. `events_from=`
     (OBSERVABILITY "Event stream") is the way past that:
     started 35 s in, after the splash, the stream reaches the same
     "(13)" message box an untraced run does, so the failure window
     can be read as a syscall stream after all. The box is two
     NtWriteFile calls on one thread, 21 bytes ("Microsoft Word 16.0")
     then 64 (the message), 65.3 s in. Everything that fails in the
     twelve seconds before them is routine: absent registry keys,
     empty window queues, and the private-assembly probes
     `Model::resolve` (`user/kernel32/src/actctx/manifest.rs`) makes
     for `Microsoft.Windows.Common-Controls` before it falls back to
     the Microsoft-key system assembly, which are expected and not a
     failure -- `CreateActCtxW` returns Ok. Word also writes no log of
     its own: Office's ULS logging under HKCU `Office\16.0\Common\
     Logging` is read but produces nothing, so the %TEMP% dumps hold
     only OFFICECL's.
     What decides the box is pure user-mode code, and the stream says
     where it sits. Word never begins a frame: BeginDraw, Clear,
     FillRectangle, DrawRectangle and EndDraw all carry `reached!` and
     sit in the traced vtable, and none of them is in the ledger
     anywhere in the run. The last graphics call Word ever makes is
     ID2D1RenderTarget::CreateSolidColorBrush, which succeeds
     (`brush::create_brush` fails only on OOM). Nine milliseconds
     later, with no system call in between, the main thread reads
     `16.0\Word\Options\Debug` (absent), and everything after that is
     the box being built: uxtheme, the Common-Controls probes,
     GalaScreenInfo three times to place it, `Common\CrashPersistence\
     WINWORD\12`, the absent `Word\Security` policy keys, and the two
     writes. So Word sets its whole drawing apparatus up, gets a brush
     back, and gives up without drawing with it.
     The code that decides sits in Office's shared layer, not in Word
     alone. `stack_at=NtWriteFile stack_from=user32` captures the box,
     and `stack_words=400` (OBSERVABILITY "Stack captures") reads past
     MessageBoxW's own kilobyte of locals, which the default 128 words
     stop inside. Both writes capture the same chain: above user32 at
     [rsp+0x090] the words pointing into an image are WWLIB.DLL, then
     MSO.DLL+0xbb0450, Mso30win32client.dll+0x8646f3,
     Mso40UIwin32client.dll+0x1b8170 and Mso20win32client.dll+0x8fb560,
     with WINWORD.EXE's own base only at [rsp+0xa08]. The scan is not
     an unwind, so some of those words are data rather than return
     addresses (the bare `Mso40UIwin32client.dll+0x0` is a module
     base), but the layer is unambiguous: Word hands the failure to
     MSO's UI client DLLs and they raise the box, whose text is
     Office's generic "Sorry, something went wrong and Word was unable
     to start. (13)".
     The decision is made in 9.2 ms of pure user-mode code, between
     CreateSolidColorBrush at 63926.5 ms and the `Word\Options\Debug`
     read at 63935.7 ms that is Word checking whether to break before
     it gives up; everything after is the box (GalaScreenInfo, uxtheme,
     the Common-Controls probes, the two writes). Nothing in that
     window failed: of the 660 calls the thread made in its last three
     seconds the only non-zero statuses are absent keys and paths, and
     every Direct2D call that records its answer at all recorded S_OK
     (`answered`, `user/d2d1/src/lib.rs`). CreateMesh and
     CreateSolidColorBrush have no `answered` site, so their silence
     says nothing either way.
     What hid the deciding call is that `reached!` is one-shot per
     site: a repeat of a method Word already called leaves no record,
     so the 9.2 ms window was invisible by construction. OT3b piece 2
     lifts it, and the reading is a negative result. Armed at
     `trace_from=d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush`
     with `events=call` (OBSERVABILITY "Call tracing"), the run
     records six calls, all of them that same method with identical
     arguments, over 18.5 ms -- and Word makes no other Direct2D call
     at all afterwards. Direct2D is not where it gives up. What the
     ledger shows in the ~900 ms that follow, in order: GetModuleHandle
     for tbarcode10.dll and tbarcode10_x64.dll, the Word\Security
     keys, then the gap of the day and, behind it, the next one, and
     GetModuleHandle(mscoree.dll) -- which the ledger's own times place
     after the box and not before it (below). One blind spot
     stands beside the trace, named in OBSERVABILITY: QueryInterface,
     AddRef and Release are hand-written slots every object shares,
     which the generator does not wrap -- a refused interface is in
     the ledger, but where it falls in the sequence is not.
     The gaps in that window come one behind another, each hiding the
     next, because the ledger names a missing export only until it
     exists. Closed so far: kernelbase's SetSystemFileCacheSize and
     ntdll's NtSetSystemInformation (six hits each, the same retry
     shape), measured on Windows first -- reading the file cache's
     limits needs no privilege and gives 1 MiB and 16 TiB on a system
     whose size was never set, setting them needs
     SeIncreaseQuotaPrivilege, and a process without it is refused
     ACCESS_DENIED rather than the PRIVILEGE_NOT_HELD the name
     suggests (`kernel32_system_test`; `GetSystemFileCacheSize` and
     `SetSystemFileCacheSize` in `user/kernel32/src/sysmem.rs`,
     `NtSetSystemInformation` in `kernel/src/nt/syscalls.rs`). Both
     rows are gone from the run that follows, and Word fails the same
     way at the same place: the file cache is not what decided it.
     What stood behind them,
     `kernelbase.dll!PrivIsDllSynchronizationHeld` with six hits
     across 37835-38293 ms, is not a divergence at all: Windows does
     not export it either. Measured on the pinned build's kernelbase
     (file version 10.0.26100.9974), GetProcAddress answers 0 for that
     name on kernelbase, kernel32 and ntdll, where
     `PrivCopyFileExW` and `GetSystemFileCacheSize` give real
     addresses -- so Word's six lookups fail on Windows as they do
     here, and implementing it would have been wrong. A missing-export
     row is a divergence only once the host is asked, which is why the
     ledger's row is a question and not a verdict.
     That empties the window of named gaps and moves the question to
     what Word was told, not what it could not find: the last thing it
     reached is
     `d2d1.dll!ID2D1RenderTarget::CreateSolidColorBrush` at 37477 ms,
     six calls, and everything after 37627 ms -- add-in probes, the
     policy keys, the loader-lock lookup, GracefulExit -- reads as
     MSO's failure path. All six answered S_OK, and that is only half
     of what the reading gave: arming one method records every traced
     d2d1 call that process makes afterwards, and there were none. Word
     asks Direct2D for six brushes, is given all six, and never calls
     the DLL again -- no layer, no BeginDraw, no text. So Direct2D is
     not what decides it either, and the question is no longer what
     gala refused but what Word concluded in the 150 ms of user-mode
     work after the last brush, where it makes no syscalls at all.
     That is what OT3b piece 3 is for, and it exists now:
     `imports=<program>[!<dll>][:<n>]` records the first call through
     each of a program's import slots, which is the only instrument
     that sees a call into a DLL gala did not write (OBSERVABILITY
     "Import tracing"). What it gives for that window is the set of
     Microsoft entry points Word reaches in it, and with
     `imports_repeat=<n>` beside it the sequence of calls through those
     slots rather than the set: the slot stays armed, so every call
     through it faults and is recorded, at a fault a call. What each of
     those calls answered is `imports_answers=<n>`, which traps the
     return as well.
     The reading needs every slot armed to be a reading at all:
     `imports=WINWORD.EXE:65536` arms 23375 of them and records 1876
     first calls without spending the budget, where 8192 is spent on
     the modules that load first and leaves wwlib's and MSO's imports
     unarmed -- a set of load order rather than of the program. With
     the Direct3D 11 device beside Word the last brush is at 41754 ms,
     and the entry points it reaches for the first time after it are:
     between 41765 and 41805 ms seven ordinals of
     `mso40uiWin32Client.dll` (#51400, #55304, #43924, #36626, #9850,
     #3393, #58199) with three of Mso20win32client's and
     Mso30win32client's among them; the four activation-context
     functions the box's Common-Controls manifest needs (QueryActCtxW,
     ActivateActCtx, FindActCtxSectionStringW, DeactivateActCtx); and
     at 41861 ms GetStdHandle(STD_ERROR_HANDLE) and a 21-byte
     WriteFile, which is gala's own user32 echoing the box's caption to
     stderr (`user/user32/src/lib.rs`). So the box is raised 107 ms
     after the brush, and the decision is made in the 51 ms of MSO
     ordinals before it. What follows is teardown and not a decision: a
     worker thread that sleeps, four `oart.dll` ordinals, one more
     DLL's CRT DllMain, GalaWindowUnregister at 42077 ms, the
     GracefulExit value deleted, exit(0) at 43151 ms.
     What the window does not hold is a call gala answered badly. The
     only ledger activity in it is the `Word\Options\Debug` read
     already named above, and every graphics call in the 100 ms before
     it answered S_OK: D3D10CreateDevice1 (631 ms of it, on DXVK over
     lavapipe), CreateTexture2D, CreateDxgiSurfaceRenderTarget,
     CreateDeviceContext twice, CreateRenderingParams,
     SetTextRenderingParams, CreateMesh, CreateDrawingStateBlock,
     SaveDrawingState, CreateBitmapFromDxgiSurface, SetTarget,
     CreateSolidColorBrush. Word is given a device, a render target
     bound to a DXGI surface and a brush, and asks for nothing further.
     It concluded what it concluded from something it already held,
     and it was never refused an interface on the way: every object of
     gala's d2d1 records the IID of a QueryInterface it cannot answer
     (`query`, `user/d2d1/src/lib.rs`), and the run has no such row.
     Nor does Word ask DirectWrite for a font: of the 59 entry points
     dwrite records reaching, not one is reached in the whole run, and
     IDWriteFactory::CreateRenderingParams is the only call it makes
     into that DLL at all. The decision is therefore made within 19 ms
     of the brush -- the `Word\Options\Debug` read at 41773 ms is
     already Word asking whether to break before it gives up -- and on
     a value some call answered S_OK with rather than on a status.
     Which value came within reach by narrowing the arming to one DLL.
     `imports=WINWORD.EXE!Mso20Win32Client.dll:8192` with
     `imports_repeat=`, `imports_answers=` and `imports_strings=`
     records 76 first calls, 2135 later ones and 2210 answers without
     spending the budget on the rest of the program, and what it shows
     is that the stretch before the box is a loop of three ordinals on
     the startup thread -- `#16040` 134 times, `#15239` 64 and
     `#59438` 62 -- every `#16040` answering the buffer it was handed.
     It ends 86 ms before the box in one run and 261 ms in another,
     and where a run raises the box twice the same loop sits between
     the two, so it is what Word does each time it gives up. Joined
     with its syscalls (`events=syscall,call,file`) the loop makes
     almost none: twelve NtProtectVirtualMemory, four GalaScreenInfo,
     the section map of uxtheme.dll and one NtQueryValueKey across the
     whole stretch. So it is Word computing and not Word asking,
     nothing is refused it there, and what it acted on is in those
     buffers. They are quoted now (OT3b, text at a return, scenario
     `import_answer_text`) and hold no text, because the ordinal
     answers a structure by value: the ABI hands it the caller's
     buffer in an argument register and answers that pointer, which
     is the answer of all 134 of those calls (argument 0 in every
     one). So what Word acted on is bytes and not a string. Those
     bytes are read now as well (OT3b, `imports_bytes=`, scenario
     `import_answer_bytes`), and read they make the loop innocuous.
     Each of the 134 answers is a structure beginning with the float
     1.0, and the two ordinals around it are a parser and a log.
     `#59438`, 62 calls on that thread, is handed the UTF-16 string
     `"400"` with 1.0 beside it, answers 3 -- the characters it
     consumed -- and leaves 0x190 (400) in the companion word: a
     string-to-number conversion. `#15239`, 148 calls, is handed names
     and paths (`OFFICE.ODF`, `CoreMessagingXP.dll`, `MSO40UIRES.DLL`,
     and the `/t Default.dotx` command line the scenario itself gave
     Word) and answers 1 every time: a logging or formatting call.
     With the digits the loop reads -- "320", "300", "350", "400",
     "250", "380", each with that same scale of 1.0 -- the stretch
     reads as Office parsing a table of numbers (durations, or theme
     metrics) and not as a verdict on anything gala answered. So the
     decision is not made in the loop, and the question moves off the
     mso ordinals: the seven seconds before the box hold no refusal at
     all.
     The window records interleaved among those calls are another
     thread's. The pump -- `GalaWindowPosted`, `GalaWindowReceive`,
     `GalaWindowInput`, `NtWaitForMultipleObjects` timeouts -- runs on
     a thread of its own throughout, so the stream is read per thread
     (`by`) and not as one sequence: the box's own thread creates,
     registers and updates windows and never once asks for a message.
     What the arming also shows is the work Word names itself, some
     of those ordinals taking a name as an argument (an ETW event's,
     or a scope's). The seven seconds before the box are, on the
     deciding thread, `DeviceModeEtw`, `AirSpace::SetDeviceMode`,
     `AnimationDisabledReason` and
     `AnimationsEnvironmentChangeEtw` and then the loop, and on the
     render thread
     `AirSpace::RenderThread::Painter::SetDeviceContext`,
     `Compositor::BeginPainterBatch`,
     `UpdateDeviceModeCommand::Execute` and
     `PainterBatchCommand::Execute` -- a graphics stack coming up --
     and the box itself now carries its own words, `#7228(0x7173716b,
     0, "Sorry, something went wrong and Word was unable to start.
     (13)", "Microsoft Word 16.0")`.
     Nor is anything refused in what little that thread does ask for
     in those seven seconds -- 44 NtProtectVirtualMemory, 27
     NtQueryPerformanceCounter and nine GalaScreenInfo among them.
     Its registry reads are measured now: a `value` event says the
     key, the value's name, its type and the size of its data, or the
     status alone where there is no value (`events=file`,
     OBSERVABILITY's `file` category, scenario `events_registry`),
     where before this the stream named the key a program opened and
     never the value it went on to read -- so the ten value reads
     counted then were counted without one, and the number does not
     survive the reading. The ledger cannot stand in
     for it -- it keeps one row an absent key and nothing at all about
     values -- so a read that is answered left no trace of itself
     anywhere. The thread that writes the box is pid 12, tid 16 (the
     capture over `NtWriteFile` names it, and it is the same thread
     that sweeps `Word\Options`), and what it reads in the seven
     seconds before the box is 175 values, not one of them a refusal:
     94 absent, 56 answered, 15 ends of an enumeration, 10 size
     probes. Every absence is a fresh profile's -- the `Word\Options`
     compatibility and view flags (`EnableCopilot`,
     `DisableModernComments`, the `RevMode*` group,
     `MarginClipPrinting`), `16.0\Common\ScreenReaderPresent`,
     HKLM's `Office\Common\ForceConcurrencyOverride` -- and a fresh
     Windows lacks each of them too. It gets as far as choosing how to
     draw text: its last reads are `NoClearTypeNW`,
     `FontSmoothingThreshold`, `BoldFontSmoothingThreshold` and
     `ClearTypeThreshold` twice over, ending 550 ms before the box,
     then `NoTranslucentSelection` 460 ms before it, by which time it
     has set a page size (`Word\Wizards\PageSize`, 14 bytes) and
     registered its startup item (`Word\Resiliency\StartupItems`, 28
     bytes). That last stretch before the box -- 460 ms in one run, 293
     in another -- holds no value read, no call into a DLL gala wrote
     and no exception, so Word decides on what it already has. The one
     thing it has that gala saw is what it wrote itself: five seconds
     before the box it reads
     `Software\Microsoft\Office\Common\Graphics\AdapterInformation`
     -- the version-less key, not the absent `Office\16.0\Common\Graphics`
     below, which reads alike -- under the adapter's own name
     (`00010005-00000000-00000000-00000000_00000023-00000000-0000FFFF-0000FFFF`),
     finds it absent and writes a 4-byte DWORD there: a `1`, 6 ms after
     the read that found nothing, which the stream says because a value
     event carries its data (OBSERVABILITY's `file` category). That
     write is a cache, not a gate. Seeded with a `0` before the run,
     Word reads the `0`, is answered, writes nothing back -- and prints
     the same box 4.4 s later. So neither the write nor what it holds
     decides anything, and the stretch before the box holds no registry
     read, no call into a DLL gala wrote, no exception and no cached
     verdict either. What Word writes about itself reads whole now
     (`events_data=<n>`, OBSERVABILITY's `file` category), and it holds
     no verdict: the `WINWORD\12` values are a session record in JSON
     -- `AppName`, `AppVersion` (16.20527.20196), `ReleaseFork`
     `2609-Sep`, `ReleaseChannel`, `ProcessId`, a `SessionId` GUID,
     `SessionInitTime`, `ProcessorArchitecture` `x64`,
     `InstallMethod`, `PreviousBuild` `Unknown` and an empty `EcsETag`
     -- 360 bytes of it written under `Common\CrashPersistence` 6.3 s
     before the box and 401 under `Common\GracefulExit` 2 s after,
     which is the same record with a `SessionUninitTime` added. Not a
     field of either names an error, a module or a code. What they do
     say is that Word calls this exit a graceful one: after the
     GracefulExit record it re-reads the CrashPersistence one, deletes
     it with its two companions and enumerates to NO_MORE_ENTRIES --
     the teardown of a clean shutdown, which agrees with the run taking
     no exception. The box is a decision Word reaches and then tidies
     up after. The only other thing under that key is a 4-byte value
     (`6`) that appears 57 ms after the box and whose last byte counts
     down by one over seven writes (`1e54e091`..`1e54e08b`) while the
     box stands. So no record Word keeps says why, which closes the
     last place a verdict could have been written down: the box is
     reached, the exit is clean, and Word's own account of the session
     is identity and clocks. Nor is the loop named above what is left:
     the buffers its `#16040` answers are quoted and dumped now
     (above, scenarios `import_answer_text` and
     `import_answer_bytes`) and hold a table of numbers Office parses,
     so the mso ordinals decide nothing either.
     What fails of the reading is keys a fresh Windows does not have
     either (`...\Office\16.0\Common\Graphics`,
     `16.0\Word\Options\Debug`) and, 70 ms before the box, gala's
     own probe for a private `Microsoft.Windows.Common-Controls`
     assembly -- four paths, twice, `kernel32`'s `actctx/manifest.rs`
     on its way to the WinSxS assembly it then resolves, which leaves
     no `CreateActCtxW` row in the ledger.
     Nor is it an exception Word swallows: a run armed
     `events=syscall,fault events_match=WINWORD events_from=38`
     (47800 events over the last four seconds) dispatches not one
     `exception` event to the program's own handler anywhere before the
     box. Nothing faults on the way there, so what Word decides, it
     decides on something it was told.
     The box itself is in the ledger now, so a run says it gave up
     without a capture having been armed first:
     `user32.dll!MessageBoxW(Sorry, something went wrong and Word was
     unable to start. (13))`, "unsupported" (user32, OBSERVABILITY
     "Missing-features ledger"), where before it was two lines on
     Word's own standard error with no time, no process and no caller
     to them.
     Word goes on running after the box as well: pagelayout.dll
     0.6 s later, `INetCache\Content.MSO` at 4 s, OProcSessId.dat at
     7.7 s.
     What those calls were handed and what they gave back is what
     `trace_from=<dll>!<Interface>::<Method>` with `events=call`
     measures, the ledger keeping one row a site and so hiding every
     repeat and every answer: the ordered stream is
     SetUnitMode(ctx, 1), GetDevice three times, CreateDeviceContext
     three times, SetTextRenderingParams, CreateMesh,
     CreateDrawingStateBlock(NULL, NULL), SaveDrawingState,
     CreateBitmapFromDxgiSurface(surface, NULL), SetTarget, then six
     CreateSolidColorBrush -- every one S_OK, and nothing after it. No
     BeginDraw, no RestoreDrawingState, not even a Release.
     Two gaps stand beside the window and neither is in it: the Vulkan
     loader asks gala's System32 dxgi for IDXGIFactory6
     (c1b6694f-ff09-44a9-...) three times as it sorts
     adapters and is refused, and `d3d10_1.dll!D3D10CreateDevice1`
     drops D3D10_CREATE_DEVICE_ALLOW_NULL_FROM_MAP (0x10), which has no
     Direct3D 11 flag of the same meaning -- nothing in the run maps a
     resource.
     What is on the screen while this happens is Word's splash. The
     window `desktop: shown window ''` names is a real top-level window
     of Word's, created at 38206 ms and shown at 40315 ms, and a
     screenshot of the second it lives (`XTASK_SHOTS`, WORKFLOW
     "Screenshots") has the Office splash drawn whole: the Word
     wordmark, "Starting...", the custom title bar with its minimise
     and close buttons, the icon in the taskbar. Its title is
     empty because a splash has none -- Word draws "Office" into the
     client area and never calls SetWindowTextW, which is absent from
     the import set. So the scenario's screen check for
     `Document1 - Word` cannot fire on this window, and it is still the
     right check: the document frame is a second window Word never
     reaches. The splash's frame is presented (GalaWindowPresent,
     40473 ms) a second before the Direct2D work above, so it is not
     what the six brushes were for.
     Nor is the graphics path what stops it. Office's own switch for
     that path is `DisableHardwareAcceleration` under
     `HKCU\Software\Microsoft\Office\16.0\Common\Graphics`,
     and seeding it as 1 moves Word onto the software target:
     `ID2D1Factory::CreateWicBitmapRenderTarget` in place of
     CreateDxgiSurfaceRenderTarget, with no device context, no bitmap
     from a DXGI surface and no CreateTexture2D (it still probes the
     Direct3D 10.1 device). Word then fails in the same place with the
     same message, after 31016 ms against 31216 ms. The two paths share
     only their tail -- CreateMesh, SetTextRenderingParams, six
     CreateSolidColorBrush, nothing after -- so what Word concludes is
     reached from both, and neither the DXGI surface nor the Direct3D
     device is what it concludes it from.
     Safe mode is the same failure, which rules out everything safe
     mode skips. Launched `/safe` (the scenario's own `args=` replaced
     through `XTASK_CMDLINE`, `cmdline::value` reading the last word of
     a key) Word loads no add-ins, no global templates, no
     Normal.dotm, no toolbar or customization file and no resiliency
     list, and it fails with the same message after 30817 ms against
     30774 ms, with the same splash on the screen. None of those is
     what it reads and gives up on, which leaves the decision where
     the stack capture put it: Word's core boot, in wwlib.
     Word never reaches a template at all. It creates
     `C:\Users\gala\AppData\Roaming\Microsoft\Templates` 285.7 s in
     -- path not found, created, then opened, and the eight opens of the
     whole run that name a template are all of that directory itself -- and that is as far as it gets: `Normal.dotm`
     is neither opened nor created anywhere in the run, and neither is
     the `Default.dotx` the scenario passes it with `/t`. So Word
     concludes what it concludes before a document or a template of
     its own exists, and nothing it was refused on the way there is
     the cause, because it never asked.
     Nor is it any other file. The whole file picture of a run costs
     1608 events (308 KB) with `events=file,image
     events_match=WINWORD`: Word makes 266 opens of its own, 165 of
     them refused, and all but fifteen of the refusals are DLL probe
     paths its loader walks. What is left is the Click-to-Run pipe (87,
     above), `C:\gala\dxvk.conf` (DXVK's own optional config), the
     private `Microsoft.Windows.Common-Controls` manifests gala probes
     for (above), the Templates directory before Word creates it, and
     `INetCache\Content.MSO`, which it asks for after the box. Every
     data file it opens is answered -- its own Click-to-Run log,
     `office.odf`, OProcSessId.dat, MSO's `MEMA5D8.tmp`, the Vulkan
     driver manifest, the walk that creates Templates -- so nothing on
     disk is refused before the decision, and the question stays a
     value Word was handed.
     Where the box falls among the gaps needs no capture: the ledger's
     `last` times put it there. `d3d10_1.dll!D3D10CreateDevice1` at
     42691 ms, the box at 42874 ms, and after it everything that reads
     like the window's tail -- GetModuleHandle for dxgitrace.dll and
     the two tbarcode10 names, the `Word\Security` policy keys,
     `Office\Common\Graphics\Blocklist`,
     `kernelbase.dll!PrivIsDllSynchronizationHeld` (43307-43755 ms),
     `GracefulExit\WINWORD`, `HeapQueryInformation(class)` and
     GetModuleHandle(mscoree.dll) at 43836 ms. Those are MSO's failure
     path, which is why closing one of them moves nothing.
     Pitfall, so the next run does not chase it: the 26 `missing
     export <dll>!__register_frame` and `__deregister_frame` rows that
     land 300 ms before the box, one per loaded module, are not Word's.
     They are MinGW's unwind registration in DXVK and lavapipe as
     `vulkan_lvp.dll` loads, with
     `kernel32.dll!GetModuleHandle(libgcc_s_dw2-1.dll)`,
     `GetModuleHandle(winevulkan.dll)` and
     `ntdll.dll!__wine_dbg_output` beside them -- gala's own graphics
     stack asking Wine's questions, and nothing waits on the answers.
     The time the traced run spends inside D3D10CreateDevice1 is
     lavapipe's, and the untraced run's 631 ms is the honest figure for
     it. With every event recorded the one call runs from 290749 ms to
     336451 ms, and of the 1244 user-mode sampler ticks in that window
     941 sit in `vulkan_lvp.dll` and 230 in ntdll's allocator under it
     (`cargo xtask profile <events.jsonl> --pid 12 --from 290.749 --to
     336.451`), with 7574 of the 9822 user faults Word raises there in
     lavapipe too. Nothing in the call waits on a lock
     or a reply: it is Mesa's software rasterizer building a pipeline,
     so the stretch before the box is not Word deliberating. The
     window's 3123 kernel samples are the instrument and not the
     system (OBSERVABILITY "Event stream"), which is why only the
     user-mode share of a traced run is a measurement at all.
     DXVK's own logs end clean beside it, which says the device is not
     merely created but unobjected-to: `winword_d3d11.log` in the dump
     ends on a feature level 10.1 device, then an 11_0 one and its
     8 MiB resource descriptor heap, with nothing after it and one
     warning in the file (external memory features unsupported), and
     `winword_dxgi.log` holds only the monitor-metadata errors named
     above. Read its 580 lines by their blocks, or the second device
     reads as one that stopped: the two creations are byte-identical
     273-line blocks (lines 1-273 and 274-546, `Creating device:`
     through the whole feature dump), and both finishes follow them --
     descriptor sizes, the buffer memory-type mask, the 64 kB sampler
     heap, the binding model, the maximum and the chosen feature level,
     and `Creating resource descriptor heap (8192 kB)` as the last line
     of each. Split the file at `Using feature level` instead and the
     first device appears to print twelve lines the second lacks, which
     are the second device's own finish.
     Word can be asked to log for itself, and what it logs says the last
     thing it does over the Click-to-Run channel is licensing, and that
     licensing is not what stops it. Microsoft's own public switches
     seed it (`user/office_install/uls_logging.regseed`: `EnableLogging`
     under the user's `Office\16.0\Common\Logging`, `LogLevel` and
     `PipelineLogging` under `ClickToRun\OverRide`), and the scenario
     dumps the Temp directories they write into, which takes the dump
     from 13 guest files to 16. Two of the new files are ULS logs of
     OfficeClickToRun's own and not Word's -- with EnableLogging set
     Word still writes none, so this instrument gives the service's view
     and Word's Click-to-Run client's, not Word's own -- and the third
     is that client's log, which ends on `ProductReleaseId =
     O365ProPlusRetail`, `HrGetDeviceBasedLicensing:
     deviceBasedLicensing for o365proplusretail=0` and a
     `SharedComputerLicensing` with an empty value. That is what a
     retail product with no device-based licensing answers on Windows
     too, and the service's matching
     `ClickToRunData::GetConfigurationSetting - Failed to get value for
     o365proplusretail.devicebasedlicensing` is the same non-answer from
     the other end. Word asks, is answered as it would be, and goes on;
     the identity and telemetry activities that fail beside it
     (`Office.Identity.ConfigService` five times,
     `RequestMsaDeviceToken` with REGDB_E_CLASSNOTREG) are the service's
     own and want a network.
     The ledger's largest row is Word's own impatience and not the
     cause. The 86 refusals of `rpcrt4.dll!no server at
     pipe(LRPC-C2RClientAPI_Server_System16)` are Word reaching for the
     Click-to-Run service before it has registered its endpoint: in the
     traced run Word tries 117 times, is refused 116 of them
     (0xC0000034) from 11940.7 ms, connects at 56709.3 ms on its main
     thread -- and raises the same box with the connection in hand. In
     the untraced run the service registers its interface at 32782 ms,
     after Word has given up at 30774 ms. gala's SCM does start
     `ClickToRunSvc` on demand and it reaches RUNNING; that the service
     needs 24 s to get from started to registered is a divergence of its
     own to measure, and not this failure.
     Nothing in the run throws: the process dispatches no exception at
     all (not one `exception` event for pid 12), so what Word gives up
     on is a value it was handed, and not a status and not a throw. Nor
     is anything in that window gala's answer. The four activation-
     context functions MSO reaches there are the box's own -- the side-
     by-side context its Common-Controls manifest needs -- and the run's
     only activation-context row is `CreateActCtxW(manifest sections not
     built)` at 32602 ms, one manifest's typelibs, proxy/stubs and CLR
     classes (the skipped mask, 26); no lookup in the whole run asks for
     one of them, which `FindActCtxSection(section not built)` would
     record and does not. What Word holds, it was given earlier. The
     nearest thing already measured was the display configuration, and
     it is answered now and is not the cause. The three CCD calls are
     implemented, measured on Windows first (`display_test`,
     ARCHITECTURE "The display configuration"), and the import trace
     taken after them shows Word walking straight through:
     `GetDisplayConfigBufferSizes` at 32874.7 ms, `QueryDisplayConfig`
     at 32878.2 ms, `DisplayConfigGetDeviceInfo` at 32881.6 ms, a
     `wcscmp` of two names, then `SETUPAPI.dll!SetupDiGetClassDevsW`
     with DIGCF_DEVICEINTERFACE at 32894.0 ms and
     `SetupDiEnumDeviceInterfaces` at 32906.1 ms, both of which gala
     refuses. So Word takes the active path, asks for the target's name
     and goes looking for that monitor's EDID -- and raises the same box
     931 ms later, at 33813.0 ms, with nothing gala refused in between
     (the ledger rows in that window are "reached" and the DLLs' own,
     and the box's 21-byte `WriteFile` closes it). The display
     configuration was a gap and not the cause; what stands behind it is
     the monitor's own description (next, below).
     Who raises the box is measured rather than inferred, by the
     facility built for it: `events_match=WINWORD stack_at=NtWriteFile:4
     stack_from=user32 stack_words=400` catches gala's MessageBoxW
     writing the caption and the text, and passes over 634 writes of
     DXVK's that would have spent the count on their own. Both captures
     read alike -- kernel32 and WWINTL.DLL (where the localized string
     comes from) under the system call, user32 under them, then
     MSO.DLL and Mso30win32client, and below those a run of WWLIB.DLL
     frames. So the decision is Word's own, in wwlib, and MSO only
     puts the box on the screen for it. A scan is evidence and not
     proof (OBSERVABILITY "Stack captures"), but it names the DLL to
     ask about, which no other facility here does.
     Measured beside it: the display is answered further now, and one
     call is all that is left of it. EnumDisplayDevicesW has the second
     level Windows has -- the monitor under the adapter, name
     `\\.\DISPLAY1\Monitor0`, ACTIVE | ATTACHED, the CCD target's
     friendly name, a device instance id under GUID_DEVCLASS_MONITOR
     with that class's key beside it, and with
     EDD_GET_DEVICE_INTERFACE_NAME the same device-interface path
     DisplayConfigGetDeviceInfo gives for that target -- and the adapter
     has an instance id and the Video key now, and any `cb` is filled as
     far as it reaches instead of refused (`cb` 0 and 4 answer TRUE and
     write nothing, 328 fills the names and not the ids): measured on
     Windows first, checked by `display_test` on both. It does not move
     Word, and DXVK's source says why not, line by line.
     `getMonitorEdid` does not go through the CCD at all: it wants a
     Plug and Play device tree, enumerating GUID_DEVINTERFACE_MONITOR,
     matching each DevicePath against the CCD target's without regard to
     case, opening that device's registry key and reading an `EDID`
     value. gala answers that walk now -- setupapi reads the device
     tree out of the registry, where Windows keeps one, and the kernel
     writes the panel's EDID into the monitor's `Device Parameters` at
     boot from the screen's own size -- and DXVK's `err:
     getMonitorEdid: Failed to find device interface for monitor using
     setupapi.` is gone from the run. Windows settled two things about
     that walk which `display_test` now checks on both: the DevicePath
     setupapi hands back is the CCD's in lower case (which is why DXVK
     compares the two with `_wcsicmp`), and the size a detail call asks
     for is the path's offset plus the path and its terminator, 168
     bytes for this laptop's 81-character path, and not the structure's
     own `cbSize`. The blank display metadata beside it was that
     missing EDID and nothing else, and is what a monitor with no HDR
     metadata gives anyway. The warning that reads like a gap, "Found
     monitors not associated with any adapter", is none: DXVK takes a
     monitor off its list only for an adapter whose Vulkan
     `luidIsValid` is set, and
     gala's Vulkan driver reports no device LUID, which is what DXVK's
     own "Adapter LUID 0: 0:10006e" says -- that number is a LUID its
     `GetAdapterLUID` asked gala's `AllocateLocallyUniqueId` for, in the
     branch it takes only when Vulkan has none. So the loop never runs,
     every monitor stays on the list, and the fallback is to enumerate
     them all, which for one monitor is the same answer; gala's CCD
     adapter LUID is never compared with anything. With that, the
     display has nothing left that Word's walk or DXVK's asks for and
     gala refuses, and the run ends where it did: Word's untitled
     window up, "(13)", status 0 after 30.7 s, the decision in wwlib.
     (gdi32's `D3DKMTEnumAdapters2` and `D3DKMTQueryAdapterInfo` are
     answered as a Windows with no display adapter answers them --
     `user/gdi32/src/d3dkmt.rs`, measured in `user/d3dkmt_test`. The
     Vulkan loader asks them before the registry's list of drivers,
     hears that there are no adapters, and finds lavapipe in the
     registry after it, which `vk_test` expects of every run.)
     The machine's size is not the cause either, and that question is
     closed: on 4 GiB instead of 596 MiB, with 3.5 GiB still free,
     Word fails identically, and gala itself never came near the edge
     (fewest frames free 38129, about 149 MiB, pagefile untouched).
     The office scenarios stay at their old size.
     Measured on Windows and no divergence: the ten
     NtAllocateVirtualMemory calls refused with CONFLICTING_ADDRESSES
     (MEM_COMMIT|MEM_RESERVE, PAGE_READWRITE) against 685 that
     succeed. MEM_COMMIT alongside MEM_RESERVE does not excuse the
     reservation -- a fixed base needs the whole range free -- and
     build 26200 refuses part of a reservation, all of it, a range
     across its end and an already committed one alike, with only
     MEM_COMMIT on its own committing inside a reservation
     (`vm_test`). Office's allocators probe a preferred address this
     way, as gala's own loader does (`thread.rs`), so the refusals
     are answers.
     What a dying thread called last is in the crash report
     (OBSERVABILITY "Crash reports"), and the two gaps the ledger named
     where Word used to raise are closed, each measured on Windows
     first: user32's SystemParametersInfoW(SPI_GETFONTSMOOTHINGTYPE,
     SPI_GETFONTSMOOTHINGCONTRAST), which Word reads before it lays
     text out (`win32_misc_test`), and kernelbase's QueueUserAPC2,
     which needed special user APCs in the kernel (`apc_test`,
     `kernel32_thread_test`, ARCHITECTURE "Special user APCs").
     `office_word` and `office_excel` now ask for the window as well,
     not only the program's exit status: a screen check whose marker
     never appeared was skipped in silence, so a run where nothing drew
     passed on the status alone. The run says what nothing printed
     (`xtask: screen: nothing printed "<marker>"`) and the scenarios
     expect `xtask: screen ok`, which is what the marker and the pixel
     checks of the window Word draws leave.
     The three gaps that stood here before are closed, each measured on
     Windows first: ntdll's RtlUTF8ToUnicodeN and RtlUnicodeToUTF8N
     (`rtl_test`), kernel32's FindFirstChangeNotification family
     (`dirchange_test`, ARCHITECTURE "Directory change notification")
     and OpenProcessToken and OpenThreadToken through
     `api-ms-win-core-processthreads-l1-1-0`, which gala hosts in
     kernel32 and which now forwards both to kernelbase as Windows does
     (`token_test`). What the run's 38 `missing export` records name is
     asked for with GetProcAddress and nothing waits on: ntdll's
     NtDeleteFile, NtQueryDirectoryFileEx and NtQueryFullAttributesFile,
     kernel32's GetCurrentDirectoryA and OOBEComplete, ole32's
     CoRegisterClassObject, CoRevokeClassObject, CoGetInstanceFromFile
     and OleRegEnumFormatEtc, oleaut32's RegisterActiveObject and
     RevokeActiveObject by name, msi's MsiGetProductCodeW and the AppV
     shim's 25 `c2r64.dll` hooks. They are not what to implement next.
     One gap in the NDR engine stands, recorded rather than guessed at:
     freeing a conformant array of pointers leaks its referents,
     because the count that sized it is not kept with the object
     (`rpcrt4.dll!NDR(free: conformant array of pointers)`, 4 hits in
     the run). The lesson of the attempt that is not the fix:
     recomputing the count at free time from the stack or a constant is
     unsound -- the stack frame the conformance was read from is gone
     by then, and freeing by the count it yields corrupted
     OfficeClickToRun's heap -- so the count has to be kept when the
     array is unmarshalled.
     Pitfalls, measured, not worth chasing again: the module names the
     run probes for and does not find (`sftldr.dll`,
     `sftldr_wow64.dll`, `mso20win32client`, `mscoree.dll`,
     `c2r64.dll`, `AppVEntSubsystems32/64.dll`) and the five files it
     looks for by full path in `ClickToRun` (`AppVISVStream32.dll`,
     `AppVISVStream64.dll`, `Mso20win32client.dll`,
     `Mso30win32client.dll`, `Mso40UIwin32client.dll`) are absent on
     Windows too (measured on the Windows machine; the three Mso ones
     live under the Office root's
     `vfs\ProgramFilesCommonX64\Microsoft Shared\Office16`, and no
     Click-to-Run binary imports slc.dll or any of the five: 165 files
     read). Of those misses one was gala's own gap and is closed:
     slc.dll (ARCHITECTURE "slc", `slc_test`, Windows' answers
     measured), which the service loads from System32 and asks one
     thing,
     `SLGetWindowsInformationDWORD(Microsoft-Windows-Container-License-Mode)`
     (SL_E_VALUE_NOT_FOUND, in the ledger). No DLL load failed in the
     service in the whole run: the ledger has neither a `missing dll`
     nor a `load failed` for it, and gala's loader records both
     ("Gaps are never silent", `load_module` in `user/testntdll`).
     The graphics tail is the same run to run, which is what makes the
     readings above measurements and not one run's weather. Two further
     runs from boot are twins of each other and of the run recorded
     here: eleven d2d1 calls reached, the same eleven, ending in
     ID2D1RenderTarget::CreateSolidColorBrush, and the box 64 ms and
     57 ms after that brush; Word's lifetime 31992 ms and 31276 ms, its
     first reach for the Click-to-Run pipe at 9788 ms, 86 refusals of
     it. Seen once and not since, so it is recorded and not built on: a
     run that gave up with a Direct3D device in hand and no
     CreateTexture2D after it, 328 ledger rows and not one "reached"
     row from d2d1. Its artifacts were overwritten by the next failing
     run of the same scenario before it could be read twice, which is
     the pitfall as much as the finding -- a reading worth keeping gets
     copied out of `target/failed/` before anything else runs.
     Word's 30 s is not a deadline, and nothing is to be read into how
     close the box is to it. The lifetimes cluster (30774, 30817,
     31016, 31276, 31992 ms) because untraced runs do the same work at
     the same speed, not because Word gives up on a clock: in the
     fully-traced run D3D10CreateDevice1 alone runs from 290749 ms to
     336451 ms and Word is alive after it, having waited out a 45.7 s
     call. A deadline hypothesis would cost a run to test; this one is
     answered by runs already taken.
     A gap of the shape this investigation keeps meeting is closed, and
     it is the first one found by asking what gala answers rather than
     what it refuses. GetSystemMetrics answered every index it had no
     answer for with 0 and recorded nothing -- 73 of the indices a
     program asks for, about thirty of them diverging from Windows,
     SM_CXVSCROLL and SM_CYMAXIMIZED among them. That is exactly the
     kind of value the standing reading of the box ("what Word holds,
     it was given earlier") is about: handed over, believed, and a
     guess no caller can tell from a measurement. `metrics_test`
     measures every index Windows documents on the Windows machine
     first and checks gala against it -- the theme's sizes as values,
     the frame's as the relations Windows holds them in, because gala's
     caption is 32 pixels where that Windows' is 23 -- and gala answers
     all but two now (ARCHITECTURE "Window, Transform and title
     bars"), each of those two a ledger row every run instead of a 0.
     SM_CLEANBOOT (67), which Microsoft documents a Click-to-Run Office
     as refusing to start for, was already right (0) and is not the
     cause.
     What Word reads of them is measured, and it is not the cause
     either. Every index a program asks for leaves a "reached" row the
     first time it asks, bounded to one row and one kernel call an
     index, and Word asks for 34: SM_REMOTESESSION alone at 36760 ms,
     then 31 in one burst at 37299 ms -- the screen and the work area,
     both scroll bars, all four frames, the caption and its buttons,
     the menus, the icon and the cursor, the double-click slop -- then
     SM_MOUSEPRESENT at 37755 ms and the window edge at 38461 ms, with
     the box at 38509 ms. Sixteen of the 34 were answered 0 before
     this: the scroll bars (2, 3, 20, 21), the frames (7, 8, 32, 33),
     the double-click slop (36, 37), the menu buttons (54, 55), the
     edge (45, 46) and SM_MOUSEPRESENT (19), which told Word there is
     no mouse. So the divergence was real and Word did read it -- and
     it reads all of it in the last 1.8 s of a 30.8 s life, after the
     deciding thread's seven silent seconds and inside the path that
     ends in the box, so with all sixteen right the run is unchanged:
     the same box, the same status 0 after 30.8 s, no document drawn.
     The ledger carries no pid, so that attribution is the scenario's
     and not the row's: Word is the only process with a window in it.
     The second gap of that shape is closed, and this one Word read.
     GetDeviceCaps answered fourteen index groups and 0 for every
     other cap, sixteen of them diverging from Windows: TEXTCAPS 0
     said this display draws no text at all, ASPECTX and ASPECTY 0
     that a pixel is nothing wide, and CURVECAPS, LINECAPS,
     POLYGONALCAPS and CLIPCAPS 0 that it draws no curve, line or
     polygon and clips to nothing. `caps_test` measures every index
     Windows documents on the Windows machine first and pins two
     answers a caller depends on besides: a memory DC created from the
     screen answers what the screen answers, the caps being the
     device's and not the selected bitmap's, and a handle that is not
     a DC answers 0 for every index with no error. Word asks for ten
     caps: LOGPIXELSY at 28440 ms (twice), VERTRES at 36034 ms, then
     LOGPIXELSX, HORZRES, HORZSIZE, VERTSIZE, NUMCOLORS, BITSPIXEL,
     PLANES and RASTERCAPS together at 36137 ms, 1.2 s before the box.
     One of the ten was wrong: RASTERCAPS answered 10889 where Windows
     answers 32409, without RC_GDI20_OUTPUT, RC_BIGFONT, RC_FLOODFILL
     or RC_OP_DX_OUTPUT. It is right now and the run is unchanged (the
     same box, status 0 after 30.3 s, no document drawn), so the caps
     are not the cause either; Word never asks for TEXTCAPS.
     The third gap of that shape is the settings themselves.
     SystemParametersInfoW answered 31 actions and refused the rest,
     and nine of its answers were values no Windows gives: every
     animation and fade off, UI effects off and client-area animation
     off, where a Windows desktop answers TRUE for all nine. That is
     the question Word's own `AnimationDisabledReason` asks on the
     deciding thread. `spi_test` measures every action on the Windows
     machine first: the values every Windows answers the same as
     values, the two that are a machine's own (the foreground lock
     timeout, the taskbar's flash count) as answered at all, and the
     desktop's sizes as the relations Windows holds them in. Those
     relations found a drift inside gala: the non-client metrics said a
     caption button was 32 wide while GetSystemMetrics said 46, a menu
     bar 20 tall against the metric's 19, and a small caption 20 wide
     against 46; the structure now reads every size from
     GetSystemMetrics, which is Windows' own invariant, so the two
     cannot drift again (the metrics once answered 0 for a scroll bar
     where this structure said 17). 24 more actions Windows answers are
     answered, and every action a program is answered leaves a reached
     row. Word asks for eight of them, all in the last three seconds of
     its life: client-area animation at 36255 ms and UI effects at 36633
     ms, the two that were wrong, then high contrast and the non-client
     metrics beside the 34 system metrics at 36637 ms, the screen reader
     at 37629 ms, and font smoothing and the caret's width at 39033 ms,
     150 ms before the box. With the nine answers right Word lives 1.5 s
     longer (32.3 s against 30.8 s) and gets 0.7 s further before the
     box (39187 ms against 38509 ms), reading the two text settings it
     did not reach before -- but the box is the same one, with the same
     (13), and no document is drawn. So the settings were a real
     divergence on the deciding path and not its cause, and the rows it
     reached show what the end of that path is made of. In the 300 ms
     before the box Word builds the whole Direct2D path and gets S_OK
     for every call of it: a Direct3D 10.1 device at 38889 ms, a
     texture at 38955 ms, a surface render target at 38978 ms, a device
     context, its unit mode, rendering parameters, a mesh, a drawing
     state block, a bitmap over the surface, SetTarget, and at 38984 ms
     a solid-colour brush -- which is one call further than the path
     ended before. Then it reads font smoothing and its type and the
     window edge (39033 ms) and the caret's width (39082 ms), and
     105 ms later puts up the box, having drawn nothing and left no
     further row of any kind. So the answer is not in what gala
     refuses: the next instrument is the call trace over those 105 ms
     (item OT and `events_from=`), which is what would say which call
     Word makes after the brush.
     Graphics is ruled out, and from Office's own side rather than
     gala's. Word carries the switch a Windows user with a bad driver
     sets: `DisableHardwareAcceleration` and `DisableAnimations` under
     the user's `Office\16.0\Common\Graphics`. Seeded for one probe run,
     the seed demonstrably reaching Word -- a new absent-key row for
     `Common\Graphics\EnableWinCompBackEnd` at 37109 ms, which only a
     Word that read that key asks for -- Word raises the same box with
     the same (13), at 42347 ms. So what Word gives up on does not
     depend on the hardware graphics path: with Office's own switch
     turning that path off, Word stops in the same place for the same
     reason. The d2d1, dwrite and d3d10_1 work above is answered and
     worth having, and it is not what the box waits on. The probe is not
     committed: it diverges from the install's own state and would hide
     the graphics work the scenario exists to exercise.
     The silent seconds before the box are a measurement Word takes of
     itself and not a wait. Under `events=syscall` the deciding thread
     spends them walking its own address space: NtQueryVirtualMemory
     over 778 regions, four times over, 3104 calls, the address
     advancing monotonically and the last walk finishing at
     0x7ff000172000 at 153785 ms. Nothing in it blocks and nothing in it
     is refused. Two readings of that window that look like a hang and
     are not: its one unsignalled wait is a poll at the same rate before
     the box and after it (thread 196, about six waits a second either
     side), and the walk runs to its end instead of stalling. The 7.5 s
     gap an untraced run shows there is that pass, slowed by the trace,
     so the instrument to spend on the window is the call trace and not
     another wait or memory reading.
     What Word reads in the last 150 ms is right, answer by answer:
     font smoothing, its type (ClearType), its contrast (1200), its
     orientation (RGB), the caret's width and the window edge all carry
     the values Windows gives, and the two Direct3D calls it makes there
     -- GetDeviceRemovedReason and CheckMultisampleQualityLevels -- are
     forwards into DXVK's own ID3D11Device and not stubs of gala's, so
     neither is gala answering. The number in the box says nothing from
     outside either: Microsoft documents no (13), so it is an index into
     wwlib's own startup steps and not a status to look up.
     The service's 24 s from started to registered has a measured cause
     now, and it is not a gap of gala's. Its own ULS log spends 16.2 s
     of 17.6 s in pauses, the largest of them 9.81 s, which ends on
     `[CloudDomainJoin] GetMachineJoinInfo {"Message": "Machine is not
     domain-joined.", "JoinStatus": 2}` -- nearly ten seconds inside a
     domain-join check that answers no. No ledger row stands against
     it: nothing of AAD, dsreg or netapi32 is refused in that window, so
     what the service waits out is its own retry and not a missing call.
     It delays Word's licensing answers and nothing more, Word connects
     and is answered, so it stays a divergence of its own to measure and
     is not the box. Word's own logging gives nothing further to read
     here: with `EnableLogging` set WINWORD writes no ULS log at all,
     only OfficeClickToRun does.
     So the window to instrument has not moved, and it has narrowed:
     the call trace over the last 105 ms, with the graphics path, the
     settings, the metrics, the caps, the display, the machine's size
     and the service's delay all measured and none of them the cause.
     The last graphics answer Word reads is measured against real
     Windows now, and it is wrong without being the cause. 197 ms
     before the box Word asks CheckMultisampleQualityLevels for 2
     samples on B8G8R8A8_UNORM, the format Direct2D draws onto, and is
     answered S_OK with no quality levels; it then asks
     GetDeviceRemovedReason twice, 4.5 ms apart, and both say the
     device is there. `d3d10_1_test` measures every count on that
     format: real Windows (an Intel UHD Graphics 630) offers a quality
     level for 1, 2, 4, 8 and 16 and none for 32, where the renderer
     under gala offers one for 1, 4 and 8 and none for 2, 16 or 32 --
     DXVK reports the sample counts lavapipe advertises, and 2 is not
     among them. gala records that empty answer in the ledger now
     (`ID3D10Device::CheckMultisampleQualityLevels (none offered)`,
     whose detail is the format and the count), because the call
     succeeds while giving it and nothing else would show it.
     It is still not what stops Word: probed by answering 2 samples
     with what the renderer offers for 4, so that Word reads one
     quality level where it read none, Word raises the same box with
     the same (13) at 39991 ms, 650 ms after the same two
     GetDeviceRemovedReason calls. That probe is not committed either.
     So the 650 ms between the last graphics answer and the box hold
     the decision. What of gala they reach is the five display answers
     above and nothing more, and the call trace is not the instrument
     for the rest of them: armed on
     `d3d10_1.dll!ID3D10Device::CheckMultisampleQualityLevels` it
     records the calls into that one DLL ("3 calls recorded ... into
     d3d10_1.dll") and not a user32 call beside them. wwlib's own
     import slots are, and that reading is made now. `imports=WINWORD.EXE imports_in=WWLIB.DLL`
     arms 3266 slots in that one image, of the 21136 the process has
     in 46 images, which is what makes the run affordable at all
     (100.9 s against 443.2 s armed whole), and records 54 first
     calls, 1749 later calls, 1803 answers and 138 quoted strings with
     every budget untouched.
     Every answer Word reads through them is right. It asks for the
     three directories (`GetWindowsDirectoryW` "C:\Windows",
     `GetSystemDirectoryW` "C:\Windows\system32" and
     `GetModuleFileNameW` its own path), `PATH`, `GetDriveTypeW` 3
     (DRIVE_FIXED), and then its user template directory:
     `GetFullPathNameW` canonicalises
     `C:\Users\gala\AppData\Roaming\Microsoft\Templates\X` and the
     directory itself, and all four `GetFileAttributesExW` on it
     answer TRUE -- so Normal.dotm's directory is there and (13) is
     not about it. The last thing the thread that raises the box does
     through those slots is window-class work: it resolves the
     memory-DC and text functions (`CreateCompatibleDC`,
     `CreateCompatibleBitmap`, `SelectObject`, `GetObjectW`,
     `SetTextAlign`, `CreatePen`) and `SystemParametersInfoW`, then
     makes 22 `ActivateActCtx`/`DeactivateActCtx` pairs around one
     `FindActCtxSectionStringW(Comctl32.dll)`, then resolves
     `RegisterClassExW`; 46 ms after the box it resolves
     `EvtWordCoreBootStop`, its own telemetry event for a startup that
     stopped. The trace is that one thread's almost entirely: 1790 of
     the 1858 records before the box are its, and each of the
     seventeen other threads that reach wwlib's slots at all makes
     exactly four calls.
     Which of Word's frames asked for the box is measured too, and
     with the facility built for it: `stack_at=NtWriteFile
     stack_from=user32 stack_words=400` captures the caption and the
     text and passes over 645 other writes of the same run, which is
     the filter earning its place rather than a count that never
     reaches the call. The kilobyte of locals user32's MessageBoxW
     holds puts Word's own frames at the 177th stack word, past the
     128 a default capture reads, as OBSERVABILITY says; above them
     the stack carries WWINTL.DLL (where the sentence comes from),
     Mso30win32client.dll, MSO.DLL, Mso20win32client.dll,
     Mso40UIwin32client.dll, WWLIB.DLL and WINWORD.EXE, and no frame
     of gala's but the write itself. So nothing gala wrote is on the
     path that decides, which is the third measurement against a
     refused call being the cause.
     That thread is measured now, and it held the first wrong answer
     of gala's in this window. With a
     `Microsoft.Windows.Common-Controls` 6.0 dependency active -- the
     one dependency WINWORD.EXE's manifest declares -- a Windows 11
     26100 machine answers `FindActCtxSectionStringW(2,
     "comctl32.dll")` TRUE: a 20-byte PATH_OMITS_ASSEMBLY_ROOT record
     with no path, roster index 2, because the assembly's WinSxS
     manifest declares that file. gala answered FALSE
     (ERROR_SXS_KEY_NOT_FOUND), having built a system assembly with no
     files at all. It has them now, from a table measured against the
     host's WinSxS manifests (ARCHITECTURE "Activation contexts";
     `actctx_test`'s "system assemblies" section, which passes on real
     Windows), so the lookup Word makes 130 ms before the box -- inside
     its 22 `ActivateActCtx`/`DeactivateActCtx` pairs and immediately
     before it resolves `RegisterClassExW` -- is answered as Windows
     answers it. Word still raises the box after it and asks for no
     WinSxS path, so that wrong answer was not the cause and the record's
     PATH_OMITS_ASSEMBLY_ROOT costs nothing here; the assembly has no
     directory on gala to omit the root of, which is what giving it one
     would be for.
     What is still unanswered on that thread is the class itself.
     Windows' Common-Controls assembly declares 29 window classes
     (ToolbarWindow32 through Button, Edit and Combobox) and
     `FindActCtxSectionStringW(3, "Button")` answers
     `6.0.26100.9818!Button` with module `comctl32.dll` out of a
     29-entry section; gala answers FALSE, and must keep doing so until
     two things change: its comctl32 registers no versioned class name
     and its user32 never consults the activation context, so handing a
     program `6.0.x!Button` today would break the plain-name path that
     works. Those two, and then the classes in the table, are the next
     piece here.
     The window gala gave it is measured and answered now: the
     three windows Word made smaller than their own frame (the last
     1.6 s before the box) were given a client area of one pixel where
     Windows gives none, and user32 now holds a window to the minimum
     it has and answers 0 x 0 for a client area that is nothing at all
     (ARCHITECTURE, "The size a window may be given"; `tiny_windows`,
     56 checks, which pass on real Windows too).
     Every file Word opens is read now, and no file it needs is
     missing. `events=file` names each NtCreateFile and NtOpenFile with
     the path as given and the status (the opens no ledger row names),
     and costs 8 s of a 92 s run, so of the three instruments on this
     window it is the cheap one. Word makes 482 opens, 225 of them
     refused over 102 distinct paths, and every refusal is one Windows
     refuses as well: the app-directory search for DLLs it then finds
     in System32 (`imm32.dll`, `dwmapi.dll`, `d3d10_1.dll`,
     `uxtheme.dll`), the Click-to-Run pipe no server answers (89), and
     the eight that come 130 ms before the box, which are the message
     box being built -- the Common-Controls side-by-side assembly
     looked for in Office16, in Common Files\Microsoft Shared\Office16
     and in a private-assembly directory beside each, the probe that
     follows the `FindActCtxSectionStringW(Comctl32.dll)` above. The
     user template directory answers every open of it, with and
     without its trailing separator, which is the second measurement
     against Normal.dotm after the attributes above.
     Word's own account of the exit is read now too, and it holds no
     reason. The 401 bytes it writes to
     `Common\GracefulExit\WINWORD\12:2` are JSON, and `events_data=512`
     carries a value that long whole where the default 64 bytes stop
     inside the first field: `AppName`, `AppVersion` (16.20527.20196),
     `ReleaseFork`, `ReleaseChannel`, `ReleaseAudienceGroup`,
     `ProcessId`, `SessionId`, `SessionInitTime`, `SessionUninitTime`,
     `ProcessorArchitecture`, `InstallMethod`, `PreviousBuild` and an
     empty `EcsETag` -- a session record, with no status, no step and
     no error anywhere in it. What it does say is where Word files it:
     under GracefulExit, the previous run's 360-byte
     `CrashPersistence\WINWORD\12:4` record read and deleted first, so
     Word's own bookkeeping calls the (13) exit orderly rather than a
     crash, which agrees with its status 0 and with the
     `EvtWordCoreBootStop` it resolves after.
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
     and a DLL's own context (RT_MANIFEST 2), a system assembly's files
     and what Windows answers for them (the record, the roster index,
     the case-insensitive key and a DLL the assembly does not declare;
     the identity Windows bound and the WinSxS manifest it has are
     printed, since Windows takes the version it has installed where
     gala keeps the one asked for). Two of the checks this test
     inherited from Wine's hold only where the manifest sits beside the
     executable, which is how Wine writes its manifests and not how
     this test does, so they have never held on a Windows host: the
     application directory Windows reports for a context made from a
     manifest file is that file's directory and not the executable's
     (gala answers the executable's), and the file information of a
     private dependency's file answers nothing at all
     (ERROR_INSUFFICIENT_BUFFER with no size written) where gala
     answers the file -- while a system assembly's answers the file, as
     gala does. Both are measured, neither is on Word's path, and
     correcting them (the test and gala) is the next piece of this
     item. GetThreadIOPendingFlag is
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
     activatable classes; recorded), QueryActCtxSettingsW.
     `office_api_test` and `wow64_office_api_test` pass on CI's Windows
     (`ci-passing.txt`).
   - Alongside, natively: the pending scenario `office_install` runs
     the Office Deployment Tool's `setup.exe /configure` in gala
     (PREBUILT `odt`: 16.0.20527.20150, SHA-256 daea2a1c...e16be1,
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
     disk-backed C:"). A residue of it is measured: laying out one
     36 MiB image still spent 22.3 s inside a single NtCreateSection
     (the flight recorder's unmatched `syscall` on CPU 1, which is what
     the watchdog then reported -- not a livelock: the call finished and
     the process exited 0), with 142 MiB of frames free and no cluster
     read failing. To do: page-in drops the answer `RamFs::fill` gives
     it, so it cannot tell a page it failed to keep from one it kept,
     and it says nothing when it reads the same cluster again -- an
     unbounded retry with no log of its own.
     Where the fast path ends now (about 1600 s):
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
     first in `actionid_test`. The run's third integrator.exe then died
     of 0xc06d007f on `msi.dll` ordinal 92, which is MsiOpenDatabaseW
     read from Windows' own export directory on this machine (build
     26200; the method checked by reading back the nine ordinals msi
     already carries, all of which matched): it opens an installer
     database. Done: msi's MsiOpenDatabaseW, x64 and x86 -- the modes
     that only open (READONLY, TRANSACT, DIRECT) open the file for
     reading; ERROR_OPEN_FAILED for a path that is not there and for a
     file that is not a database alike; ERROR_CREATE_FAILED for a create
     mode and for a persist path, recorded in the ledger as the gap it
     is; ERROR_INVALID_PARAMETER without a path, with an empty one or
     without a handle; the handle set to 0 except where the NULL path
     pointer is refused before it is touched -- test first in `msi_test`
     and `wow64_msi_test` (every answer measured on this machine's
     Windows 11 build 26200, both widths). Two answers are deliberately
     not Windows'. Asked to create a database where it can, Windows
     creates one (16 KiB, measured) and gala does not, a database engine
     being galapkg's business (ROADMAP PK), not msi.dll's. And Windows
     keeps bookkeeping per database file which gala does not: while one
     is open, a second open of the same file is refused
     (ERROR_CREATE_FAILED for another read-only one, ERROR_OPEN_FAILED
     for a writing one, both measured) where gala opens it, so the test
     asks the open modes with no other handle on the file.
     A failure there fails the install as before: the
     service ends with 17002 and setup.exe with 0x426a (17002,
     "C2RClient returned a failing exit code") -- at 90%
     of the scenario's progress, the uninstall-centennial, migrate and
     fonts tasks done (the fonts task lays down Office's TTFs and TTCs
     and succeeds), the service going quiet for the 30 s in which the
     integrator dies and then setting its failure event. Everything
     from APPLYCONFIGURATION on is measured with the guest's egress
     blocked (`restrict=y`, by hand and not checked in), because
     setup.exe with egress reaches the configuration service 3 s in and
     reads its answer with webservices.dll. That is why webservices came
     first of the two blockers: without it no run that behaves like a
     real install gets as far as the integrator, and a snapshot of that
     point cannot be taken at all. Done: gala's webservices.dll, x64 and
     x86 (ARCHITECTURE "Other DLLs", webservices) -- the 15 functions
     setup.exe delay-loads are the XML reader subset alone (heaps, error
     objects, a reader over a buffer of text with its settings and its
     position, WsReadToStartElement, WsReadStartElement,
     WsReadEndElement and WsReadType under both element mappings; no
     channel, no service proxy, no security binding), reading the
     document node by node with `gala_xml`'s `Scan` so that a malformed
     one fails at the read that reaches it, as Windows' reader does, and
     reading one node ahead as Windows' reader does, which is what its
     ROW and COLUMN show; test first in `webservices_test` and
     `wow64_webservices_test`, every answer measured on this machine's
     Windows 11 build 26200 in both widths.
     With webservices in place, the first run from boot with egress on
     the owner's Windows machine under KVM (Intel i9-8950HK, 2 vCPUs;
     2558 s of VM time of the 3600 s limit, nothing waiting for a host
     CPU) got through the download, STAGEREGISTRY,
     UNINSTALLCENTENNIAL and APPLYCONFIGURATION's publish, Migrate and
     Fonts, to 92% of the scenario's progress, with Office's files
     published on disk: WINWORD.EXE and EXCEL.EXE are among the
     10 985 files, under `%ProgramFiles%\Microsoft Office\root\Office16`
     -- and
     then failed in the integrator's last task, which reads the MSIs it
     is handed: `C2RManifest::ConfigureMSIs`, "MsiAPICallFailure
     (MsiOpenDatabase call failed. , Error:0x6e)", ErrorCode 30050, for
     `Integrator Mode[Install] Scope[Global] Phase[Complete]
     Scenario[C2R] Actions[Extension Msi ]
     Msi[C2RInt.16.msi,C2RIntLoc.en-us.16.msi,*]`. 0x6e is
     ERROR_OPEN_FAILED, which is what gala's MsiOpenDatabaseW answered
     every database with until it could read one. integrator.exe exits
     30050 twice, the C2R task fails, and the service and setup.exe end
     with 17002.
     Done: reading an installer database. What Office asks of one is
     bounded, from the names Integrator.exe looks up at run time (it
     imports msi.dll from no import directory): MsiOpenDatabase,
     MsiDatabaseIsTablePersistentW, MsiDatabaseOpenViewW,
     MsiViewExecute, MsiViewFetch and MsiRecordGetStringW, and the
     queries in it are ``SELECT * FROM `Feature` `` and ``SELECT * FROM
     `Property` `` -- a reader for the container and the two tables, and
     a query that selects a whole table, not an engine. That is what
     msi.dll's `db` is, over galapkg's MSI reader: handles, views with a
     cursor, records, and SELECTs of a whole table or of named columns
     (ARCHITECTURE "Other DLLs", msi). Test first in `msi_test` and
     `wow64_msi_test` over the checked-in fixture
     (`user/galapkg/testdata/msi/basic.msi`, so the one program runs
     both on Windows and in gala), every answer measured on this
     machine's Windows 11 build 26200 in both widths -- three of them
     answers the documentation does not suggest: a field past the
     record's last reads as empty rather than failing,
     MsiRecordGetStringW reports the length it needs when it is given no
     buffer, and a fetch past the last row returns ERROR_NO_MORE_ITEMS
     and rewinds the cursor.
     Not there yet, each recorded in the ledger where a program asks for
     it: a query with a WHERE or an ORDER BY (Windows runs it, gala
     refuses it as ERROR_BAD_QUERY_SYNTAX and records that the query was
     not understood rather than wrong), the query parameters a record
     stands in for, and any query that changes a database
     (ERROR_FUNCTION_FAILED, which is Windows' own answer for a database
     opened read-only).
     With the reader in place the run's third integrator.exe got past
     ConfigureMSIs and died of 0xc06d007f on `msi.dll` ordinal 88,
     which is MsiInstallProductW, read from Windows' own export
     directory on this machine as ordinal 92 was (build 26200; the
     method checked again by reading back all nineteen ordinals msi
     already carries, all of which matched): it installs a package.
     Done: msi's MsiInstallProductW, x64 and x86 --
     ERROR_INVALID_PARAMETER without a path, ERROR_PATH_NOT_FOUND for
     an empty one, and for a path that is no package Windows' own
     answer for why not (ERROR_FILE_NOT_FOUND for a missing file,
     ERROR_PATH_NOT_FOUND for a missing directory,
     ERROR_INSTALL_PACKAGE_OPEN_FAILED for a directory and for every
     other reason an open can fail, ERROR_INSTALL_PACKAGE_INVALID for a
     file that is not a package) -- test first in `msi_test` and
     `wow64_msi_test` (every answer measured on this machine's Windows
     11 build 26200, both widths). One answer is deliberately not
     Windows': a package gala does read it cannot install, an installer
     engine being galapkg's business (ROADMAP PK), so it answers
     ERROR_INSTALL_FAILURE, Windows' own code for an install that
     failed, and the ledger carries which package was asked for. The
     export is there all the same because the import is delay-loaded:
     an unresolved one ends the process before it can handle anything,
     where a refusal it can see leaves it to report the failure.
     With the export in place the run from boot (the owner's Windows
     machine under KVM, 2 vCPUs, 1229 s of VM time) gets past the delay
     load and the blocker moves to the install itself: the third
     integrator.exe runs its ConfigureMSIs task for 314 s instead of
     dying, asks MsiInstallProductW for `C:\Program Files\Microsoft
     Office\root\Integration\C2RInt.16.msi` and
     C2RIntLoc.en-us.16.msi, and takes ERROR_INSTALL_FAILURE for a
     retryable error (EnsureMsiNotBusy and InstallMsi eleven times over
     five minutes, then "Install of MSI encountered a retryable error
     but no time remains for retry."); ConfigureMSIs then fails 1603 and
     Integrator::Main ends 0x643, a fourth pass does the same for 302 s,
     and the service and setup.exe end with 0x426a as before. The
     registry holds 42 907 keys by then, against 22 291 at the
     ordinal-88 death. Next: those two integration MSIs have to be
     installed, which means galapkg's engine behind MsiInstallProductW
     (ROADMAP PK) and not another answer -- Office retries 1603, so no
     refusal gets the install further.
     Seen too, not fatal so far: `wevtutil.exe` is absent, so the
     integrator's InstallEtwManifest aborts (ErrorCode 2) and carries
     on; `schtasks.exe` is absent, so PublishCustomExtension cannot
     delete the "Office Feature Updates" task and carries on;
     `InspectorOfficeGadget.exe` cannot start nine times (0xc0000135,
     no .NET Framework); `OfficeC2RClient.exe` exits 1. And: `slc.dll`
     a missing DLL (loaded by name at
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
     ID2D1Factory1/2/3 (1 and 2 answered, 3 recorded); the gadget needs .NET
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
     cabinet, credui, and osppc.dll, which is the system's sppc.dll,
     gala's being x64 only so far) and the
     functions its imports lack (shell32's
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
1. `thread_test`'s futex requeue (open): once on CI its transcript
   differed from `expected.txt` -- FUTEX_CMP_REQUEUE said it had moved
   the waiter to the second word, and the FUTEX_WAKE after it found
   the waiter on the first. lx-expect runs these programs on the host
   kernel, so what raced is the program's own expectation: `rq_ready`
   is set before the waiter blocks, and nothing proves it is queued
   when the requeue runs. Not reproduced locally (200 runs). The
   report now names the host kernel, so the next occurrence says
   which Linux answered that way; the fix is a handshake that proves
   the waiter is queued, not a wider retry.
2. Milestone 2's rest: the rest of signal_test and fork_test
   (rt_sigqueueinfo, faults as SIGSEGV/SIGBUS/SIGFPE through the
   exception path, delivery to a thread running user code,
   stop/continue, process groups and sessions, waitid's stopped and
   continued children, rusage), vfork
   sharing memory;
   execve ending the other threads first; per-thread CPU time
   for clock_test's CPU clocks; wait queues on sockets instead of their
   1 ms look (seam 5: the stack's notify hook is AFD's alone);
   `lx::state` onto the core process; an `lx_pipe_pie` scenario.
   Each corpus program becomes a boot scenario as its calls land; the
   busybox scripts grow with them.
3. Milestone 3: clocks, ptys over the core terminal object, /proc.
4. Seam 1: the pty as a core terminal object under ConPTY.
5. Seam 4's NT surface, each test first against real Windows:
   FileCaseSensitiveInformation (query, and set on an empty directory),
   FILE_DISPOSITION_POSIX_SEMANTICS and FILE_RENAME_POSIX_SEMANTICS,
   CreateHardLinkW (FileLinkInformation), CreateSymbolicLinkW; an NT
   delete through a hard link's name removing that name; a file's
   owner from its security descriptor's owner (the Linux ids come from
   the token, `lx::identity`; a file shows the caller as its owner
   today); a reboot scenario for Linux
   files (modes and links are host-tested only).
6. Seam 5: shared core objects when pipes, sockets or files are
   reworked. Built: AF_UNIX sockets (`unix`, under `lx::unix`); next for
   them: Windows' AF_UNIX through afd on the same core sockets (a test
   first on real Windows: bind, connect, accept, the socket file as a
   reparse point), then pipes and TCP sockets' state (with wait queues
   instead of TCP's 1 ms look).
7. Also open (ARCHITECTURE "The lx module"): address randomization, a
   vDSO, Linux in gala-um, the system-call numbers 462..=469.

**Done when** (each milestone): every corpus program of the milestone
runs as a boot scenario whose transcript equals its `expected.txt`.

**Design**: ARCHITECTURE "Plan: a second personality (Linux)", "The
seams", "The Linux layer's milestones", "The lx module", "The lx_abi
crate"; WORKFLOW "Linux test programs".

### CT. Linux containers: Podman and Docker on Linux, reached from Windows

**Goal**: stock Podman and Docker Engine (dockerd, containerd, runc or
crun), installed from a Linux distribution's own packages, run Linux
containers in that distribution on gala's Linux personality (LX),
unmodified. Windows programs reach the same engines without a virtual
machine: Docker's own Windows CLI and the tools that speak Docker's API
(VS Code Dev Containers, Visual Studio, JetBrains IDEs, Testcontainers)
through `\\.\pipe\docker_engine`, and stock Podman for Windows
(`podman machine`) through gala's answer to WSL2's interface, the
distribution running on LX.

**Not in CT**: Docker Desktop for Windows is wanted but not scheduled
("Later": the engine reached from Windows serves the tools that use
Docker first). **Out of scope**: running WSL2 itself (a VM with
Microsoft's Linux kernel), the Hyper-V backends, Docker Desktop for
Linux (a VM of its own), and Windows containers (HCS, server silos).

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

**Next** (each step test first; step 1's recordings are the spec). Order:
the Linux side first (steps 4, 7 and 8, with step 1's Linux
recordings), then the Windows side (steps 2, 3, 6, then 5); Kubernetes
(step 9) after step 8, alongside the Windows side.
1. Spec. On real Linux (Ubuntu 26.04): `strace -f` of `podman run --rm
   hello-world` rootless and rootful (podman, conmon, crun, pasta) and
   of `docker run --rm hello-world` (dockerd, containerd, runc, started
   by systemd), what each reads in `/proc`, `/sys` and cgroupfs, and
   the netlink and nftables traffic. On real Windows 11 24H2: what
   Podman for Windows does to set up and run `hello-world` (every
   `wsl.exe` command line and its output, the WSL service calls, named
   pipes and sockets (hvsocket and localhost TCP) between the two
   sides, registry keys, WMI queries and optional-feature checks, files
   under `%LOCALAPPDATA%` and `%ProgramData%`), and what Docker's
   Windows CLI and the tools above send over `\\.\pipe\docker_engine`
   (Docker Engine's API is documented; the recordings show which calls
   and which path forms the tools use). The ranked result (Linux system
   calls, facade, Windows side) goes in this section, like AG's blocker
   list.
2. The facade's core's rest: wsl_test checked on Windows (its `info`
   lines become checks); the WSL service (the COM interface Podman
   calls, the registry's one writer), a distribution's init as an LX
   process (an instance that outlives its last command, as WSL's),
   DefaultUid applied (root inside the distribution), `-u`, `--cd`,
   `--export`, ptys and process groups for the terminal (LX "Next" 3-4:
   job control, Ctrl+C as SIGINT).
3. The facade's plumbing: `/mnt/<drive>` inside the distribution
   (`\\wsl.localhost\<distro>` outside is done: EX, `nt::wslfs`), interop
   (Windows programs started from Linux and back), `wsl.conf`, hvsocket
   (`AF_HYPERV`) on the Windows side meeting `AF_VSOCK` on the Linux
   side as one core socket pair, localhost forwarding over one shared
   network stack.
4. LX up to the engines' needs (beyond milestone 5): systemd as PID 1
   (a distribution's dockerd and Podman's machine), namespaces (mount,
   PID, UTS, IPC, network, user), cgroup v2, overlayfs, bind mounts and
   `pivot_root`, TUN/TAP (pasta, slirp4netns), netlink, veth and
   bridges, nftables/iptables, seccomp, `/proc` and `/sys` as these
   programs read them. Namespaces and cgroups are core objects
   (ARCHITECTURE "Plan: Linux containers"), not LX-only state.
   Nesting is a requirement, not an afterthought: namespaces and
   cgroups are trees whose every level carries its own limits (a
   cgroup subtree delegated to a container is divided again inside
   it), and gala's overlay takes an overlay as its upper layer (Linux's
   refuses, which is why Docker-in-Docker falls back to `vfs` or a
   volume there). Goal beyond Linux: a nested container runtime
   without `--privileged`, an inner root fully contained (as Sysbox
   does on Linux).
5. Podman for Windows: `podman machine init`, `start`, then `podman run`
   from the Windows side; Podman Desktop after BR stage 2 (Electron).
6. Docker from Windows, without Docker Desktop: gala serves
   `\\.\pipe\docker_engine` and relays it to the Docker Engine of a
   chosen distribution (its `/var/run/docker.sock`), so Docker's
   standalone Windows CLI (`docker.exe`, with the compose plugin;
   fetched by xtask, not committed) and the tools above work
   unmodified; `docker context` and `DOCKER_HOST` as on Windows. Windows
   paths in bind mounts (`docker run -v C:\src:/app`, the API's `Binds`
   and `Mounts`, compose files) are translated to the distribution's
   `/mnt/<drive>` paths (step 3) by the relay, which reads the API's
   requests rather than passing bytes blindly; what it rewrites is in
   `events=wsl`. Then VS Code Dev Containers and Testcontainers for
   .NET and Java running a container from the Windows side.
7. Container engines on Linux, with no Windows side: in a distribution
   on gala (a WSL one or LD's desktop), the distribution's own `podman`
   and `docker` run containers, in tiers: rootless Podman first with no
   network and no limits (`--network=none --cgroups=disabled`: user
   namespaces with newuidmap/newgidmap and /etc/subuid, overlay in a
   user namespace, `pivot_root`); then cgroup v2 delegated by systemd's
   user instance, its limits enforced; then rootless networking through
   pasta or slirp4netns; then rootful Podman, and Docker Engine from
   the distribution's packages (`apt install docker.io`, or Docker's
   `docker-ce`) as the distribution starts it (`docker.service` and
   `docker.socket` under systemd, the `docker` group, its iptables
   variant), with bridge networking and published ports; then rootless
   Docker.
8. Nesting: the official `docker:dind` image runs `docker run --rm
   hello-world` inside it; Podman in Podman (`quay.io/podman/stable`)
   likewise; then both without `--privileged`. The sibling pattern
   (the host's `/var/run/docker.sock` mounted into a container) comes
   with step 7.
9. Kubernetes, on step 7's engines: single-node k3s first (containerd
   built in), a Deployment's pods running and a Service reached by its
   cluster IP and by DNS (CoreDNS); then kind (Kubernetes nodes as
   Docker containers: step 8's nesting with systemd inside). Beyond
   Docker it needs cgroup v2's statistics as exact as Linux's, since
   the kubelet evicts and schedules by them (`memory.current`,
   `memory.events`, `memory.stat`, `cpu.stat`, the pressure files,
   pids), kube-proxy's Service NAT (iptables or nftables with
   conntrack and its netlink interface, hairpin NAT, bridged traffic
   through the firewall: `br_netfilter`'s sysctls), mount propagation
   (shared and slave mounts), `/proc/<pid>/mountinfo`, `/proc/net`,
   `/dev/kmsg` and inotify at scale. Multi-node clusters across gala
   machines (flannel's VXLAN) after that. Observability: the cgroup
   figures the kubelet read and the NAT rules a Service installed,
   both in the state dump, so a wrong eviction or an unreachable
   Service can be explained after the fact.

**Done when**: from a shell in a Linux distribution on gala, its own
`podman run --rm hello-world` and `docker run --rm hello-world` print
their greeting, rootless and rootful, and the same from inside a
`docker:dind` container, and a single-node k3s serves a Deployment
through a Service; from a gala command prompt, Docker's Windows
CLI runs `docker run --rm hello-world` against that engine and `podman
run --rm hello-world` works with Podman for Windows installed by its
own installer; then a container serving HTTP reached from the Windows
side (`-p 8080:80`) and a bind mount of a Windows directory. Observability with
each step: the facade's and the relay's calls as events (`events=wsl`),
each distribution's console and the engines' logs copied into the
serial log, missing Linux calls in the ledger by distribution.

**Licensing**: Podman, Docker Engine (Moby, containerd, runc),
Docker's CLI, Kubernetes, k3s and kind are Apache-2.0. None is committed; xtask fetches them
and runs them only in QEMU. WSL's source (MIT) is an allowed source
(CLEAN_ROOM "WSL").

**Design**: ARCHITECTURE "Plan: Linux containers".

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
   POSIX shared memory (System V's done for GIMP: `lx_shm`; MIT-SHM's
   use of it and `/dev/shm` not reached yet),
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
9. GIMP on Linux (the owner): a distribution's GIMP (Ubuntu 26.04 LTS's
   3.2.2, a GTK 3 program: its window over gala's Wayland server)
   started on the desktop, opening an image, applying a filter and
   saving it, in a boot scenario with a screen check, the saved file
   compared with the one GIMP writes on real Linux. Done: GIMP in batch mode (`lx_gimp_batch`, the Linux root "gimp"
   beside "gtk3" and "x11"; WORKFLOW "GIMP"): the console GIMP 3.2 loads
   its data and a PNG through its file-png plug-in, blurs and inverts it
   with GEGL from script-fu (each plug-in a process, tiles handed over in
   a System V segment) and saves a PNG whose SHA-256 is the one GIMP
   writes on Ubuntu 26.04, its whole transcript Ubuntu's. Its window
   (`lx_gimp`, `user/lx/gimp/gui.sh`): GIMP 3.2 over gala's Wayland
   server, its window and its welcome dialog shown, the dialog closed with
   Escape, in.png opened (the title names it: `desktop: window titled`)
   and painted on the canvas (its orange rectangle checked), Colors >
   Invert (GIMP's action search; the rectangle blue), File > Export As
   out.ppm, Ctrl+Q and Discard Changes: the exported file's SHA-256 is the
   one the same keys give on Ubuntu 26.04 (the transcript); what it
   needed:
   `/proc/<pid>/fd` links (GLib names an icon file by its link for
   glycin), the GSettings schemas compiled at assembly, GIMP's whole icon
   theme (GTK aborts on a missing icon), the file-pnm plug-in and the
   file chooser. Next: all of its plug-ins (about 130 processes queried
   at a first start); its welcome dialog stays white (not drawn yet).
   The Linux roots moved to Ubuntu
   26.04 LTS with it (WORKFLOW "Linux roots from Ubuntu 26.04"): glibc
   2.43, libffi, libwayland-client, weston and the "gtk3", "x11" and
   "gimp" roots are resolute's (traced on 26.04), and the scenarios on
   them pass, `lx_gtk` among them (GTK 3's images through glycin:
   bubblewrap fails as on an unprivileged Linux without user namespaces
   and glycin runs its loaders unsandboxed, its warning in the
   scenarios' own transcripts, recorded on 26.04), `lx_xdesktop` and
   `linux_menu` (termios2; and on 26.04 twm gives the keys to the xterm
   only with the pointer on it, so their screen checks point first). What
   26.04 needed that gala lacked, each test first: the ioctls every descriptor
   answers (`lx_fdioctl`), capget and capset (`lx_caps`), timerfd
   (`lx_timerfd`), descriptor arguments' upper halves ignored
   (`lx_fdbits`: glycin's int fds), PR_SET_NO_NEW_PRIVS, PR_GET_AUXV and
   PR_SET_PDEATHSIG (`lx_prctl`: bubblewrap, uutils' `sleep`, glycin's
   loaders), /proc's first part and unshare, a namespace refused as Linux
   refuses it at max_user_namespaces 0 (`lx_procfs`), sigaltstack
   (`lx_altstack`: Rust's runtime), and an epoll_wait seeing what another
   thread ADDs or MODs meanwhile (`lx_epoll`: glycin's loader connection,
   armed from one thread while another waits), termios2 (`lx_pty`:
   glibc 2.43's isatty asks with TCGETS2), pidfds and waitid (`lx_pidfd`: GLib 2.88 watches each child with a
   pidfd and reaps it with waitid(P_PIDFD)), /proc/mounts (`lx_procfs`:
   GIO's mount monitor), inotify (`lx_inotify`: GIO's file monitors),
   setfsuid and setfsgid (`lx_fsid`), a process's name after execve in
   the log (`lx_gimp_batch`: `process gimp-console-3.2`), seccomp: strict
   mode, filters enforced, the probes (`lx_seccomp`). Open: once, in a
   full parallel run of the `lx_` scenarios, `lx_gimp_batch`'s file-png
   plug-in (the load of in.png, its result right) printed "gimp_flush():
   error: Broken pipe" at its end: it wrote after GIMP had closed its
   pipe. Seen once more, in CI on a host at load 5.9 for 4 CPUs: that
   time two plug-ins at once, file-png and script-fu, each of which had
   taken far longer than on an idle host (521 ms and 1670 ms), while
   gimp-console was still running (it exited 5.7 s later). A third
   sighting, again in CI (host load 5.6 for 4 CPUs), was file-png alone,
   so two plug-ins at once is not what it takes. Every sighting is a
   slow plug-in on a loaded host, which puts the question at the
   plug-in's last write and not at a reader that had gone; the cause is
   not found yet.
10. Inkscape (the owner): Ubuntu 26.04's Inkscape 1.4.3 (LINUX_ROOTS
   "inkscape"), first in batch mode (`lx_inkscape_batch`: in.svg rendered
   to a PNG Ubuntu's byte for byte; WORKFLOW "Inkscape") and its window
   (`lx_inkscape`: in.svg drawn on the canvas, Ctrl+Q, exit status 0).
   It asks to be maximized before its first commit (as weston grants:
   `lx_wl_maximize`). Next: an edit and an export compared with
   Ubuntu's. Linux system calls
   are traced with `events=syscall` (`lx syscall`). Open: rseq is ENOSYS
   (glibc carries on without it).

**Done when** (each step): its test and scenario pass, and the Linux
programs' transcripts match real Linux's.

**Design**: ARCHITECTURE "Linux applications on the desktop
(Wayland)": the server in the kernel beside the compositor for now
(moving to user mode with SH), `wl_shm` pools as memfd sections,
surfaces as `winstate` windows wearing shasha's frame through
xdg-decoration, one socket per session, the limits on client input,
`events=wayland` and `wayland=trace`.

### WS. The website and the domain

**Goal**: gala-os.com explains what gala is and what runs on it, from the
same facts the repository holds, and the domain also carries the update
feed.

**Done**: the site itself (`web/`, WORKFLOW "The website"): a React page
prerendered to static HTML, with the map of ARCHITECTURE's sections and
their status, the programs and their boot scenarios, and the design
decisions; its own pipeline (`website.yml`) publishing to GitHub Pages,
kept apart from the system's CI in both directions (WORKFLOW "The two
pipelines stay apart"). GitHub Pages is enabled on the repository with
"GitHub Actions" as its source, and its custom domain is `gala-os.com`.
DNS at Cloudflare now serves the apex as four `A` and four `AAAA`
DNS-only records to GitHub Pages, and `www` as a DNS-only `CNAME` to
`bjorn-ali-goransson.github.io` (WORKFLOW "The domain"); the proxied
records to GoDaddy's parking addresses that answered with error 525 are
gone. What crawlers read is in place too (WORKFLOW "Being found"):
`robots.txt` naming the search engines and AI crawlers one by one, a
generated `sitemap.xml` and `llms.txt`, canonical and Open Graph and
JSON-LD in the page, and an IndexNow submission after each deploy. The
certificate for the domain has been issued and HTTPS is enforced, so the
site answers on `https://gala-os.com` and nothing else. The feed has its
own name as well: `updates.gala-os.com` is a proxied R2 custom domain in
front of the release bucket, and `FEED_URL` in release.yml points at it
rather than at R2's rate-limited development URL.

**Next**:
1. Verify ownership in Google Search Console and Bing Webmaster Tools,
   which is a sign-in and so is done by hand. Search Console's DNS method
   wants a TXT record on the apex, in the same Cloudflare zone.
2. The owner's own screenshots of gala running, replacing the two crash
   screens the site shows now (`web/public/shots/`, listed in `App.tsx`).
3. Open: whether the ARCHITECTURE map is generated from the document's
   headings instead of restated in `web/src/content.ts`, which would stop
   the two drifting apart.

### AR. ARM64 port

**Goal**: gala boots and runs on ARM64 (QEMU's `virt` machine first),
on the seams item PT prepared: native ARM64 Windows programs
(`aarch64-pc-windows-msvc` PE images) and ARM64 Linux programs.

**Done so far** (design: ARCHITECTURE "The aarch64 port"):
- The tooling: `cargo xtask qemu` builds the aarch64 emulator beside
  x86-64's (QEMU's own edk2 firmware with it); `AARCH64_SCENARIOS` boot
  the aarch64 kernel from the same kind of ISO on `virt`
  (`Boot::UefiAarch64`), their verdicts as x86-64's (exit through
  semihosting with the same statuses).
- The kernel boots to its frame allocator (`arm_boot`, with
  `stop_after=frames`): the entry (FP/SIMD on, the boot stack on SP_EL1),
  the exception vectors (`arm_fault_ud`, `arm_fault_pagefault` with
  `early_fault=`), the PL011 console through a device page added to the
  bootloader's tables, the processors by MPIDR from Limine's MP response.
- Its own page tables (`arm_boot` now to `stop_after=paging`):
  `mm::paging`'s entry encoding is a `Format` (x86-64's `X86`, VMSAv8's
  `Vmsa`, host-tested bit for bit), one mapper for both; TTBR1 and MAIR
  switched to the kernel's tables, the device pages carried over.
- Interrupts and time (`arm_boot` to `stop_after=timer`, the tick seen
  running): the kernel is soft-float (no FP/SIMD state to save at an
  interrupt, as x86-64's), the IRQ vectors save and restore a
  `TrapFrame`, the GICv3 (distributor, this CPU's redistributor, the
  system-register CPU interface), the generic timer's virtual channel
  at CNTFRQ_EL0's architected rate, periodic by compare value.
- Threads (`arm_boot` to `stop_after=sched`; `arm_sched`, and x86-64's
  `sched_selftest`, with `schedtest`: two threads switched on yields,
  woken by the tick, joined): kernel stacks, the context switch (with
  the FP/SIMD state user mode will have), the per-CPU block (TPIDR_EL1).
- The other CPUs (`arm_boot` to `stop_after=smp_done`: both online, the
  MADT's GICC entries agreeing): the APs Limine started sent to the
  kernel's AP entry (stack, tables, per-CPU block, vectors, GIC part,
  tick), IPIs as SGIs, TLB flushes by broadcast; hardware ids are
  MPIDR's affinity everywhere.
- The NT layer's start (`arm_boot` to `stop_after=drivers`): the system
  call dispatchers and the user-mode hooks kept, KUSER_SHARED_DATA.
- PCI (`arm_boot`, with a disk and a card attached): configuration space
  through segment 0's ECAM window from the MCFG (host-tested parser), the
  virtio-blk and virtio-net drivers set up on their devices through
  virtio's modern interface (on both architectures).
- The rest of the boot, to `gala: boot ok` with no program (`arm_boot`
  to `stop_after=manifest`: the manifest verified): the card's frames
  both ways (`arm_net_echo`: the kernel's echo server and xtask's
  client), the disk's requests both ways (`arm_persist`: C: formatted on
  the disk by a first boot, its area and rollback floor read back by the
  second).
- The clock checked against the wall clock (`arm_clock`, with
  `calwrong`): the PL031 RTC, the check's verdict and the recalibration
  (CNTFRQ_EL0's rate read again; the RTC's measurement wins over it when
  they disagree), the log naming the architecture's counter and timer.
- The GIC's pseudo-NMI: interrupts masked by priority (ICC_PMR_EL1),
  the NMI an SGI above it, an IRQ handler running with NMIs let through;
  frame-pointer backtraces (`arm_watchdog`: `hangtest`, the watchdog's
  backtrace of a CPU stuck in its tick handler; `hangtest` now holds the
  boot's end until the hang is over).
- CPUs the bootloader did not start, started with PSCI CPU_ON (the
  conduit from the FADT; a trampoline that turns the MMU on from a
  TTBR0 identity page): `arm_smp_missed` (`smptest=hide`: the
  bootloader's AP powers itself off first).
- User mode, its first program (`arm_hello`: hello.exe built for ARM64,
  ntrt's ARM64 runtime): user spaces on TTBR0 (`arch::user_space`, one
  for both architectures), EL0 entry and return, SVC system calls (NT's
  number in the immediate, as Windows ARM64's ntdll makes them), user
  faults (demand paging), ARM64 PE images (`arch::runs_machine`);
  xtask's aarch64 images carry the ARM64 builds of the DLLs and of
  USER_PROGRAMS_ARM64 (exported to `user/bin/arm64`).
- Programs through gala's ntdll (`arm_startup_test`, `arm_import_test`): its ARM64
  build's stubs make `svc #N`; a process starts at LdrInitializeThunk
  with ARM64's CONTEXT (`nt::abi::arm64::context`, host-tested), which
  NtContinue resumes (floating-point part included); NtCreateThread
  reads it; imports bind from ARM64 DLLs. Threads, the synchronization
  objects and waits run as on x64 (`arm_thread_test`, `arm_sync_test`,
  `arm_wait_test`).
- Exceptions and APCs in ARM64 user mode: faults, `brk` (its immediate
  the exception, as Windows ARM64 assigns them) and `udf` reach gala's
  ntdll's dispatcher and its vectored handlers, which may continue
  (`arm_exception_test`); RtlRaiseException and RtlCaptureContext; the
  top-level filter and the crash report (`arm_crash`,
  `arm_crash_dispatched`, `arm_crash_filtered`); user APCs from alertable
  waits and NtContinue's TestAlert (`arm_apc_test`); kernel32 loads (`arm_tls_test`).
- Frame-based exception handling on ARM64 (`arm_seh_test`, seh_test's ARM64
  part): .pdata and .xdata, packed unwind data, partial prologues and
  epilogues, RtlLookupFunctionEntry, RtlVirtualUnwind, frame handlers in
  dispatch and RtlUnwindEx, unwind consolidation, the dispatcher's
  MSFT_OP_CONTEXT frame.
- NtGetContextThread and NtSetContextThread on ARM64 (`arm_context_test`:
  suspended threads, the caller; `arm_threadinfo_test`).
- Function tables registered at run time on ARM64 (RtlAddFunctionTable,
  the growable tables, callbacks): gala's ntdll keeps one registry for
  x64 and ARM64 (`ftable.rs`); `arm_seh_test` checks a table and a growable
  one. ntrt's 64-bit runtime is one
  for x64 and ARM64 (`nt64.rs`), so the x64 test programs build for
  ARM64 as they are.
- Devices where the firmware says: the console's PL011 from the SPCR,
  the GIC's distributor and redistributors from the MADT's GICD and GICR
  entries (host-tested parsers; `arm_boot` checks the log says so).
- xtask's state dump at an aarch64 timeout: a break on the console
  (`chardev-send-break`), the PL011's interrupt taken at the
  pseudo-NMI's priority and turned into the NMI on every CPU
  (`arm_statedump_test`); the thread dump reads a waiting thread's
  arguments (the registers it made the call with: ARM64's x0 is both
  the first argument and the result).
- The x64 test programs run on ARM64 as twins (`ARM64_TWINS`:
  `arm_<name>`, the x64 scenario's expectations; 156 of them, with the
  network, single-CPU and shipped-program cases): ProcessPrng from the
  kernel (RNDR), RtlCaptureStackBackTrace, NdrClientCall2 and rpcrt4's
  guarded calls, the native machine (KUSER_SHARED_DATA, IsWow64Process2)
  on ARM64; the kernel refuses an executable of a machine it does not
  run. The programs' x64-only parts have ARM64 ones (the TEB through
  x18, the processor number through NtGetCurrentProcessorNumber, `svc
  #N` stubs, `brk #0xf000`, ARM64's RUNTIME_FUNCTION and CONTEXT;
  fpu_test's d0-d31 and FPCR, xmodify_test's cache maintenance).
  What they found missing on ARM64, done: user mode reads the virtual
  counter and runs cache maintenance by address (CNTKCTL_EL1,
  SCTLR_EL1.UCT/UCI); FlushInstructionCache cleans and invalidates;
  KUSER_SHARED_DATA's ARM64 processor features; kernel32's context and
  XState functions (ARM64's CONTEXT, no XState feature), its processor
  number; msvcrt's `_controlfp` and floating-point status on
  FPCR/FPSR; the Win32 start address from x0; an EL0 system-register
  trap is an illegal instruction.
- `.pdata` link records (Flag 3: jump islands) in the ARM64 unwinder
  (`arm_seh_test`: the lookup returns the parent, the island unwinds as
  its body).
- Linux's aarch64 ABI: the generic system-call table, restarts, clone's
  argument order, the open flags, `struct stat` and `struct
  epoll_event`, AT_HWCAP, seccomp's architecture and strict mode, the
  signal frame (fpsimd_context), the thread pointer kept across
  switches; the Linux test programs built for aarch64 (40 `arm_lx_*`
  twins, their transcripts real Linux's). Copy-on-write fork now works
  wherever frames are numbered above the managed count (QEMU `virt`'s
  memory starts at 1 GiB).
- Code the kernel writes for user mode (image pages, a loader's,
  WriteProcessMemory's) is cleaned and invalidated for instruction fetch
  before it runs (a real ARM64 machine's caches need it; QEMU's do not).
- Input on aarch64: virtio-input keyboards and relative mice
  (`input/virtio.rs`), xtask's input pacing on the events the guest
  reads (`virtio_events`, measured against QEMU); `arm_lx_wl_input` and
  `arm_gui_test_headless` are twins. Screens on aarch64: the
  screen-checked twins get a virtio-gpu the kernel drives
  (`display=virtio`; the firmware gives no framebuffer and Limine
  refuses ramfb's): gui_test, start_menu, tray_test and d2d1_window run
  there; every ARM64 image ships gala.exe's ARM64 build. The WSL
  distribution of an aarch64 image is Alpine's static aarch64 busybox
  (busybox.net has none; `target/wsl/aarch64`): wsl, wsl_terminal,
  taskmgr and explorer run as twins. glibc on aarch64: dyn_test and
  its PIE run against Ubuntu's arm64 glibc 2.43 (`arm_lx_dyn`,
  `arm_lx_dyn_pie`; GLIBC_ARM64, build-aarch64.sh's sysroot).
  safepoint_test on ARM64 (`arm_safepoint_test`): HotSpot's Windows
  ARM64 safepoint, a `ldr wzr, [x16]` poll, the ARM64 CONTEXT and a
  register-saving stub entered through NtContinue; the compiled
  constants in `movz`/`movk` immediates, patched while threads are
  stopped in that code and made visible to instruction fetch (dc
  cvau/ic ivau). TCG keeps instruction fetch coherent by itself, so a
  missing flush shows only on hardware (KVM on an ARM64 host).
  Kernel handover on aarch64 (`arch::aarch64::handover`): the next
  kernel's TTBR1 tables and a TTBR0 trampoline from the scaffold, the
  other CPUs powered off (PSCI CPU_OFF) and started again by the next
  kernel (CPU_ON), the record's FIRMWARE section (the RSDP) for its
  console, GIC, PCI and PSCI; every milestone's scenarios run as twins
  (14: the handover, idle, busy and blocked programs, versions with
  the `handover_next` build for aarch64, sockets).
- The event stream on aarch64: a host file written through semihosting
  (`events_file=`); session_test runs there.
- CI runs the aarch64 scenarios in its boot shards, under QEMU's
  emulation (TCG on GitHub's x86-64 runners).

**Next**:
0. **The desktop on ARM** (the owner: the next work). Done: a boot
   that keeps running ends on the console session's desktop (the
   downloadable images showed a black screen on aarch64 and the splash
   on x64: their console programs never made it live);
   `desktop_at_boot` and `arm_desktop_at_boot` boot as the images do and
   check the desktop, the start menu from the Windows key and from a
   click. Left: item 1 below (the popup hang),
   and the hardware a real machine has (item HW). The aarch64 image's
   command line is the twin's; x64's adds the Go programs.
1. `arm_lx_wl_popup`'s hang (open): once on CI, wl_popup_test blocked
   reading its socket in a roundtrip after the "anchor bottom" popup's
   commit while the server's thread waited too (a wakeup lost, or a
   request answered with nothing; the server had served 79 requests
   and had no events waiting); not reproduced locally (0 of 224 boots
   with 4 at once beside 2-3 busy host loops), nor on CI since. The
   state dump now says which (`a round would find work`, each
   client's socket) and names Linux calls in the flight records.
2. User mode, the rest: the desktop Linux applications run on aarch64
   from their roots' arm64 builds (`DEBS_ARM64`): GTK (`arm_lx_gtk`,
   `arm_lx_gtk_x11`), xterm, a distribution's desktop, GIMP and
   Inkscape (`arm_lx_xterm`, `arm_lx_xdesktop`, `arm_lx_gimp(_batch)`,
   `arm_lx_inkscape(_batch)`). arch_prctl_test and signal_test stay x86_64's (the
   call and the sigcontext fields they check are x86's; the aarch64
   signal frame is pipe_test's, id_test's, altstack_test's and
   seccomp_test's).
   xstate_test stays x64's (AVX; ARM64's SVE state is not in
   gala's ARM64 context). rpcrt4's server routine call puts every
   argument in integer registers: ARM64 passes floating-point ones in
   v0-v7 apart, which needs the format string's types.
3. The firmware's boot-menu countdown (EDK2's BdsWait, 5 s) stalled
   about one aarch64 boot in 150 under multi-threaded TCG on a loaded
   host, before the boot loader ran: CPU 0 kept taking its timer tick
   with nothing held for it (gdb's record: `interrupt_request` 0, the
   virtual timer re-armed), the countdown did not end, and xtask's
   timeout handling later woke it. Stopgap: xtask skips the countdown
   (`-boot menu=on,splash-time=0`; 0 stalls in 1000 boots, and each
   boot is 5 s shorter). Not found: why the countdown's timer event
   stops firing (EDK2's TimerDxe and DXE core timer list against QEMU's
   virtual timer); a debug build of the firmware with its DEBUG_INFO
   log is the way in.

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
4. 32-bit x86 programs on ARM64 (WOW64 on ARM): the same translator
   with a 32-bit x86 front end (its encoding, a 32-bit address space,
   x87 as the main FPU). On x86-64 gala WOW64 enters 32-bit code
   natively (compatibility mode); ARM64 has no such mode, so only the
   CPU layer changes: WOW64's NT part (`nt/wow64.rs`, already built for
   every architecture) and the 32-bit system DLLs (x86 code, translated
   like the program) stay as they are. The end-to-end target: Office's
   installer, whose `setup.exe` is 32-bit x86 (Microsoft ships that
   bootstrapper as x86 for ARM64 Windows too).

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

**Out of scope**: the Google Play Store and Google Play Services
(proprietary, licensed by Google only to certified devices; a licence
would be a business decision, not a roadmap step) and Play Integrity's
device verdicts, which gala cannot pass honestly: applications that
require them (many banking, payment, DRM and game titles) do not run,
and the README's "Not there yet" will say so once AN runs
applications. They come from AOSP, F-Droid and APKs the user
installs; microG (Apache-2.0), an open
reimplementation of Play Services' common APIs (push messaging,
location, sign-in), is the route for applications that need those.
AOSP's CTS (open source) is AN's conformance suite.

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
   every key, root included, made by the first publish (`feed keygen`)
   and kept in a private R2 bucket the jobs read (`.github/feed-keys.sh`); `feed pull` / `feed push` move only what changed, timestamp
   last, never overwriting a published version; each run checks the
   result as the client does before and after the upload. Host tests
   for each piece and for the workflows' steps against a directory
   standing in for the bucket (ARCHITECTURE "The update feed"; WORKFLOW
   "The update feed"). Every key lives with CI (the owner's
   decision, for now): CI can rotate keys and re-sign root, at the
   price that whoever controls CI controls the trust chain; root's
   validity is short (90 days, re-signed monthly; targets 30 days,
   snapshot 7, timestamp 1), so a later move of root offline is one
   rotation that every client takes within weeks. Next:
   - The first real run: the owner's one-time setup (WORKFLOW "The
     update feed"), then a publish that proves what the local tests
     cannot (R2's 404 for an absent file at the public URL, `aws`
     against R2's endpoint, the head-object guard, the job summary).
   - The updater in gala (test first; the owner: next after item IN,
     so that item HW's laptop takes every release by itself): a
     scenario where xtask serves a
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
   - The keys live in a private R2 bucket the release jobs make and read
     (`gala-os-signing-keys`); the feed bucket is public at its custom domain,
     `https://updates.gala-os.com` (`FEED_URL` in release.yml). Root keys
     offline and the targets key in a KMS remain the later move above.

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
   The rings are now also what the measurement asks for. An event
   costs its bytes' crossing out of the guest and nothing else, about
   12 us a byte: 2.1 ms for a 162-byte `import` event, the figure
   `user/import_cost_test` and the `import_cost` scenario measure
   (OBSERVABILITY "What the stream costs"). It is the crossing and not
   the device -- the same stream to a port with no device behind it
   costs the same -- and not the instruction either: a line written in
   one `rep outsb` costs what its bytes cost, KVM leaving the guest
   once a byte for string I/O. So a traced Word run is ten to thirty
   times its own length, its events 2 ms apart, and every question
   asked of the stream is first a question about what one can afford to
   write. The way out is to stop crossing a byte at a time: the rings
   handed to the host by DMA (virtio-serial, which QEMU writes to the
   same `<image>.events.jsonl`), one exit a line or a batch of lines
   rather than one an event's every byte. The debug console already
   holds whole lines (`arch::x86_64::debugcon::Line`, with the host
   tests that count the writes a line costs), which is what a batched
   transport needs.
2. OT3: trace context carried through ntdll/kernel32 (a TEB slot) and
   ALPC/RPC; sockets and events signaled across processes as links like
   pipes and completion ports.
3. OT3b: **what a program does inside a DLL gala wrote**, without a
   record typed per method. The ledger says what was missing and the
   event stream what crossed the kernel, but a COM call from a program
   into one of gala's DLLs is an indirect call through a vtable in the
   program's own address space: the kernel cannot see it, and the
   `reached` records that found Word's last call are typed by hand, so
   the gap is always the method nobody instrumented. Three pieces:
   - **A wrapper per vtable slot, generated.** Done for d2d1 and
     d3d10_1:
     `cargo xtask gen-trace` reads each slot list and the interface each
     slot belongs to (`// gen-trace` directives beside the list), and
     writes a wrapper per slot into the DLL's own source -- a generated
     block, checked in and verified in CI as `user/bin`'s binaries are
     (WORKFLOW "Generated code") -- which records the first call that
     reaches the method and then calls it. The slot list is the only
     place a method is named, and a slot no directive explains fails the
     generator, so a new method cannot be left unrecorded. Generated
     typed wrappers rather than assembly thunks: no `unsafe`, no
     per-architecture code, the compiler checks each signature against
     the function it wraps, and a wrapper that knows its arguments can
     record them when asked to. 225 slots of d2d1 are wrapped, against
     the 204 that were written by hand; the hand-written records are
     gone. d3d10_1 followed, 21 slots of its device and texture
     vtables, and its other eighty-seven -- the stubs one `recorders!`
     macro writes, each already recording a row under its own name --
     report to an armed trace without a second record, which a
     `records <module>` directive is how the generator is told
     (OBSERVABILITY "Call tracing"). The runtime both DLLs report
     through is one file, `user/trace_common.rs`, declared as each DLL's
     own `trace` module; a DLL brings its ledger name and its own `Arg`
     impls to it. Next: dwrite, whose functions are named in snake_case
     (`collection_family_count`), so they are renamed to d2d1's
     `<prefix>_<Method>` convention first and the generator then derives
     their names like the rest.
   - **A trigger, so a long run records only what is wanted.** Done.
     The one-shot record a slot leaves is free (a relaxed atomic a
     call), but a record of every call needs a system call per call,
     which a drawing loop cannot pay.
     `trace_from=<dll>!<Interface>::<Method>[:<n>]` arms the DLL at the
     slot named: before it, nothing but the one-shot records; after it,
     every call with its arguments as `call` events, `n` at most
     (default 4096). The decision is made where the call is, before
     anything is formatted (OBSERVABILITY "Volume": decide before the
     cost), and the arming lives in the boot options rather than in a
     DLL's source, so an investigation needs no edit. The name compared
     is the one the ledger's rows spell, so a value is read straight off
     a ledger row, and it is compared once per slot, inside that slot's
     existing one-shot: a call nobody is tracing costs two relaxed loads
     and no string compare at all. A spent budget is answered to the
     DLL, which stops making the call, and the closing line says the
     trace ended rather than the program -- a stream that simply stopped
     would read as a program that stopped calling
     (`kernel/src/nt/tracecall.rs`, `user/trace_common.rs`, scenarios
     `trace_from_test`, `trace_from_budget_test`,
     `trace_from_answer_test`, `trace_from_d3d10_test` and
     `trace_from_across_test`, OBSERVABILITY "Call tracing"). What a
     call answered is recorded with it: the generator knows which
     methods return something and puts a second record in those
     wrappers, so the answer is a `ret` event naming the same method,
     one word, out of the same budget as the calls. That it comes out
     of the same budget is what keeps the pairing honest -- a trace can
     end between a call and its answer, never the other way about. It
     closes the blind spot the first reading of Word's window ran into,
     where six identical calls read the same whether each one succeeded
     or each one failed. The arming is the process's, not one DLL's:
     once armed, every traced call that program makes into a DLL gala
     wrote is recorded, not only the method named and not only its
     DLL. The flag stays each DLL's own static, so a traced call still
     costs one relaxed load; what lets two DLLs' statics reach each
     other is ntdll, where each offers its flag on its first wrapped
     call and the DLL that arms sets them all (`GalaTraceJoin`,
     `user/testntdll/src/trace.rs`), and the run's summary names the
     DLLs the calls were made into. Armed per DLL it could not: the
     Word reading lost every Direct2D call that followed the three
     device calls it recorded, and a reading of one program's two DLLs
     took two runs whose streams could not be interleaved. Its gaps:
     one armed method a run; the OpenTelemetry export does not map
     `call` or `ret` events yet (the same gap `ledger:` rows have,
     OT5).
   - **The import slots, for the DLLs gala did not write.** Done.
     `imports=<program>[!<dll>][:<n>]` records the first call a program
     makes through each of its import slots, `n` at most (default
     4096), as `import` events. That reaches the exported entry points
     of Microsoft's own DLLs, which the vtable wrappers cannot; what it
     does not reach is the COM vtable slots behind such an export,
     which is why both pieces exist.
     A trap page rather than thunks: gala's loader fills the IAT
     (`user/testntdll/src/lib.rs`), so it reserves a region it never
     commits and reports each bound import, and the kernel writes
     `base + index * 16` into the slot. The first call through it
     fetches an instruction from an absent page and arrives in the
     kernel's own page-fault path, with the call's arguments in
     registers and its return address on the stack; the kernel records
     it, puts the export's address back in the slot and sets the
     instruction pointer there, and the call runs on. Thunks would have
     meant generating code at run time for x64, ARM64 and WOW64 alike,
     new `unsafe` on both sides, and a system call on every call for
     ever; this way the only cost after the first call through a slot is
     the slot itself (`kernel/src/nt/traceimports.rs`, `arm_imports` in
     the loader, scenario `iat_trace_test` with an ARM64 twin,
     OBSERVABILITY "Import tracing").
     Its gaps: the first
     four integer argument registers only, read whether the function
     takes that many or not; static imports only, so no delay-load
     import and nothing a program resolved with GetProcAddress; an
     imported variable is left alone, because the value a read of its
     slot returns *is* what the program uses and no fault announces that
     read -- a *function's* slot read as data is answered instead, the
     dereference of the value faulting at the trap address, which
     disarms the slot, repairs the register and takes the instruction
     again, and is recorded as a read rather than a call (without which
     the option cannot trace Word, whose AppvIsvSubsystems64.dll reads
     an import that way); 32-bit programs go
     through the same code untested; and entries are not reclaimed when
     a process ends, which the budget is what bounds. The budget is
     also what bounds the reading: spent part way through, it arms the
     modules that loaded first and leaves the rest, so the ceiling is
     65536 slots (9 MiB of kernel heap) -- what a program of Word's
     size takes whole -- and the summary line says when it was spent.
     One record a slot is no longer among them: `imports_repeat=<n>`
     keeps each armed slot armed instead of repairing it, so that every
     call through it faults and is recorded and not only the first, `n`
     later calls at most (scenario `iat_repeat` with an ARM64 twin).
     Repairing the slot is only what makes the next call free -- the
     instruction pointer is what sends the call on -- so the repeat is
     not writing it, and what a recorded call costs is its record,
     which is what the budget bounds: a fault whose record is not
     written takes none of it and leaves the slot armed, so
     `events_from=` beside the repeat spends the budget on one window
     of a run and not on its first moments (scenario `import_window`
     with an ARM64 twin), and a window the run never reaches leaves the
     stream shut, which the stats say in those words rather than
     blaming the mask (scenario `import_window_late`). Nor is the
     answer:
     `imports_answers=<n>` keeps the return address the call left and
     puts the slot's second trap address in its place, so the return
     faults too, with the answer still in the register the ABI returns
     it in (scenarios `iat_ret_test` and `iat_answers`, both with ARM64
     twins -- the return address is on the stack on x86-64 and in the
     link register there). Pending returns are a stack per thread,
     matched to their calls by the stack pointer a return arrives with,
     which is also what notices a call that never returned. What is
     left of the gap: the first four integer argument registers and the
     one return register, so a wide or indirect argument is not in
     reach, and a callee that reads its own return address while that
     return is trapped (`_ReturnAddress`, a stack walk of its own) sees
     the trap and not its caller -- which is why both budgets are read
     narrow (OBSERVABILITY "Import tracing"). An exception unwound
     through such a frame is the one case repaired rather than kept
     narrow, because it changed what the program did and not only what
     was recorded of it: the walk ends at the trap address, so no
     handler above that frame is asked, and Word's run with the whole
     answers budget ended in a stack overflow inside the exception
     dispatcher where the same run without it ended in none. So gala's
     own ntdll says when a walk of a thread's frames begins and the
     kernel puts that thread's pending returns back where its calls
     left them, recording those calls without an answer; the ask has to
     come from the program, an exception raised in user mode reaching
     no kernel path at all. The call the walk itself begins in is
     answered its return address with the same ask, a walk beginning at
     the frame of the call that raised and so at a trap address where
     that call went through an armed slot -- the shape of Word's C++
     throw (scenarios `iat_unwind_test` and `iat_unwind`, both with
     ARM64 twins).
     Nor is an argument that is a pointer at text:
     `imports_strings=<n>` reads up to 256 bytes at each of the four
     argument registers of a recorded call and quotes what reads as a
     run of printable characters ending in a NUL inside the window --
     two bytes a character or one -- beside the registers in the same
     event, `n` quotes at most (scenarios `import_strings_test` and
     `import_strings`, both with ARM64 twins). A trace of calls to
     `WideCharToMultiByte` or `LoadLibraryW` that cannot say which
     string is a trace of the shape of a program's work and not of the
     work: Word's own give-up builds its message through exactly those
     calls. What is left of the gap there: nothing knows a call's
     signature, so a number that happens to point at such a run is
     quoted like a string the call was really handed, which is why the
     quotes sit beside the registers and not in place of them; the
     narrow range is ASCII only, a pointer's own low bytes being a run
     of high bytes and a zero. What a buffer a call filled holds is
     read where the call's return is trapped: `imports_answers=` and
     `imports_strings=` together quote the answer register and the
     arguments that were no text at the call, each quote keyed by the
     place it was read at (`answer`, or the argument's index), which is
     what most of what a program asks of a DLL needs read, Word's
     own startup among it -- one mso ordinal called over a hundred
     times, every one answering the buffer it was handed (scenario
     `import_answer_text` with an ARM64 twin). What is left of the
     Nor is what a call that says nothing in text holds: a structure
     answered by value is a pointer at bytes and not at text, the ABI
     handing the callee the caller's buffer in an argument register
     and answering it (WORKFLOW, "COM methods that return a structure
     take a hidden pointer"), so a quote of either end reads nothing
     -- which is what all 134 answers of Word's last stretch are.
     `imports_bytes=<n>` writes the first 32 bytes at every place that
     reads as no text as hex instead, in an object of its own beside
     the quotes, keyed by the same places and read from the same
     window: text wins where the window is text and a quote is left,
     so neither object says the same window twice, and a place only
     dumped at the call is read again at the return because the bytes
     of a buffer are what the call changed (scenario
     `import_answer_bytes` with an ARM64 twin: the armed import
     answers the buffer it was handed, 32 zeroes at the call and a
     structure's bytes at the return). What is left of the gap there
     is the window: 32 bytes is a structure and not a buffer, and
     reading more than that of one is a dump of memory and not a line
     per call.
     Nor is who made the call: each record carries `from`, the image
     whose import slot the call went through, where `call` names the
     DLL that supplies the export and not the one that asked for it.
     One import is a slot in every image that imports it, so a trace
     armed over a whole program records the same name called from the
     program and from its DLLs, and without this those records are one
     line twice. An import address table is part of its own image, so
     the slot's own address is the answer: the kernel resolves it as it
     arms the slot, which costs a `u16` that fits the padding the entry
     has anyway -- the 65536-slot ceiling stays the same 9 MiB -- and a
     recorded call then costs nothing to attribute, with no stack to
     walk, nothing asked of the program and the same answer for a DLL
     Microsoft wrote. The summary says the other half, where a run's
     slots were before any call was made: the five biggest images named
     with their counts and the rest counted, which is what a budget
     spent on one DLL's heap imports reads from. Scenario `iat_from`
     with an ARM64 twin, where one name is armed in two images at once
     (testdll2 imports from testdll, and `imports=` arms every image of
     the process), so the two records differ in this field alone. Its
     gap: an image past 65535, or a slot in no image at all, is armed
     and recorded with `?` for the module rather than refused, and the
     summary counts those slots apart.
     Nor is a budget spent on the names that are noise:
     `imports_except=<text>[,<text>...]` leaves an import whose
     `<dll>!<name>` holds one of the texts unarmed, so that it costs no
     slot of the budget, no fault and no record (scenario `iat_except`
     with an ARM64 twin). What that is for, measured: Word armed whole
     at `events_from=38` recorded 1264 first calls of 23363 slots and
     spent the whole 65536-call repeat budget on its own heap and
     string imports (2339 `RtlAllocateHeap`, 1933 `RtlFreeHeap`, 1701
     `memcpy`, 1353 `tolower`), leaving 437 calls into the MSO shims
     among 66800 events in a 259.7 s run, where the same window with
     one shim armed by name recorded 76 first calls, 2135 later ones
     and 2210 answers with the budgets untouched in 95.6 s against
     86.6 s untraced. The texts are substrings and not patterns, so one
     of them names a family (`api-ms-win-crt-`, `ntdll.dll!Rtl`), and
     it is the same narrowing `imports=<program>!<dll>` is from the
     other side: keep the program whole and name what to leave out,
     for a question that spans several DLLs.
     The other half of that narrowing is the image, since one import is
     a slot in every image that imports it: `imports_in=<image>` arms
     the slots in that image alone and leaves every other image's as
     they were (scenario `iat_from_in` with an ARM64 twin, where this
     program's own slot for `testdll.dll!gala_add` is left alone while
     testdll2's slot for that same name is armed). No list of names
     reaches this, the noise being the same call under the same name
     from another image: a whole-program window of Word armed 21136
     slots in 46 images, 8118 of them MSO.DLL's and 3280 WWLIB.DLL's,
     so "what does wwlib call" could not be asked at all. The image is
     the field a record already carries -- its `from` -- asked of the
     arming instead of of the records afterwards, so it costs no more
     than the comparison the loader already makes once per module.

   The kernel's half of the same question is done:
   `stack_at=<service>[:<n>]` logs the calling thread's user stack the
   first `n` times a process reaches a system call, each word of it
   that points into a loaded image named `<module>+<offset>`, and
   `stack_from=<module>` keeps the captures to the calls that module is
   on the stack of -- without it a service another thread reaches in a
   loop spends any count before the call in question
   (`kernel/src/nt/stack_at.rs`, scenarios `stack_at_test` and
   `stack_at_from_test`, OBSERVABILITY "Stack captures"). The wrappers
   say what happened inside a DLL gala wrote; this says who called, in
   any DLL, including Microsoft's own, at any system call and for the
   price of a stack scan -- where an import record's `from` says it for
   nothing, out of the slot's own address, but only of the imports a
   trace armed. Its gaps: a scan, not an unwind, so dead words of
   earlier frames come with the call path, and a 32-bit program's
   frames are above the WOW64 thunk the capture is taken at.

   What this is for: every program whose compatibility is worked on
   next runs into the same wall Word did, and the answer should not be
   another round of records typed by hand. Its gaps when done: the
   32-bit DLLs (WOW64) are not generated for yet, and a slot's
   arguments are recorded by value, not followed.
4. OT4: the ETW bridge (EventRegister/EventWrite, TraceLogging) turning
   what Windows programs emit into OTel logs and spans; later the same
   for LX.

   The session side is done (advapi32's `trace.rs`, ARCHITECTURE
   "advapi32"): StartTraceW, EnableTraceEx2 and ControlTraceW give a
   program the private in-process session it can start without rights,
   the provider side consults it, and an event in reach is kept in the
   session's buffers with its provider, descriptor and payload. That is
   what makes a program build its events at all -- TraceLogging builds
   no payload for a provider no session enabled -- so until it existed
   there was nothing for a bridge to read. The whole enablement
   contract was measured on the pinned build as a standard user and is
   checked on both systems by `user/etw_test` (scenario `etw_test`,
   `ci-passing.txt`).

   A run can also ask for a session itself, without the program asking
   for one: `etw=<provider or name>[:<level>[:<keyword>[:<n>]]]`
   (nt::etw, OBSERVABILITY "A session gala starts"). advapi32 asks the
   kernel once, at a program's first EventRegister, and makes that
   session's enable there, so a provider learns of it in the callback it
   would learn of any session in; each event in reach goes to the kernel
   as it is written and leaves an `etw:` line. Scenarios `etwarm_test`
   (nothing armed, so the program's own answers, which are real
   Windows'), `etw_armed` and `etw_armed_all`.

   The session is spent on the program the run is about:
   `etw_from=<text>` gives it only to a process whose name has the text
   in it, in either case, and the filter is in the ask, so a process the
   run is not about is told nobody is listening and builds no payload.
   Measured, not supposed -- an `office_word` run armed for every
   provider gave 1834 of its 2048 lines to OfficeClickToRun.exe, the
   service Word starts and nobody was reading, and Word's own providers
   registered ten seconds after the budget was gone. Scenarios
   `etw_from` and `etw_from_elsewhere`.

   The decoder is done (`kernel/src/nt/etw/tracelogging.rs`, host unit
   tests): a line carries the event's own name and every field with the
   name the program gave it, read out of the provider metadata (type 2)
   and the event metadata (type 1) that `TraceLoggingProvider.h` builds
   beside each event. Every in-type and out-type that header declares
   is read, nested structs and both kinds of array among them; a line
   that cannot go on says where it stopped and why, and tells an event
   cut to fit gala's record from one whose description does not read.
   An event whose metadata is not TraceLogging's -- a manifest
   provider's, whose field names are in no part of it -- leaves the
   line it can: its size, which of the two it was, and the strings its
   bytes carried (`kernel/src/nt/etw/strings.rs`, host unit tests),
   which is what a payload's message, path or reason is and all there
   is to read of one without its manifest. Scenarios `etwfields_test`
   (the program's own account of every field shape it wrote, which is
   real Windows') and `etwfields_armed` (those same events decoded);
   `etwraw_test` and `etwraw_armed` are the manifest provider's four
   events and the strings read out of them, the longest one cut, the
   seventh counted, and a payload of numbers said to hold none.

   Next: OTel, the decoded events as logs and spans, by the mapping
   OT5 uses for everything else.

   Why this order: a Word startup registers 20 providers and writes
   not one event, so the first thing the bridge needed was the session
   that makes Word write them -- and the first use of the decoded
   events is Word's own account of the startup failure the
   `office_word` scenario reaches.
5. OT5: the in-guest OTLP exporter for servers, controlled from the
   `gala` CLI (on/off, sampling rules, endpoint), metrics pushed to
   Prometheus' OTLP receiver on the same host (diagnosis, not
   availability; `/metrics` for scraping as an option); later, anomalies against a key's own baseline
   suggesting (then, within a budget, starting) an investigation;
   defaults: metrics, failures, ledger, crash reports and the flight
   recorder always on, traces and the profiler on demand
   (OBSERVABILITY "Volume").
6. OT6: visualization (OBSERVABILITY "Visualization"): the Perfetto
   export is done (`cargo xtask perfetto`: tracks per thread, IPC
   flows, samples). Next, one way to query a run for every session,
   people's and agents' alike (OBSERVABILITY "Querying a run"):
   otel-desktop-viewer pinned (recorded in THIRD_PARTY); every
   boot-test, locally and in CI, leaving its run's DuckDB file beside
   its log; `cargo xtask observe <scenario | file>` (the UI) and
   `cargo xtask query <scenario | file> '<sql>' | --std <name>` (no UI);
   named standard queries (errors by NTSTATUS, system calls by time
   with p95, a span's subtree, slowest hand-offs, a thread's
   timeline); then WORKFLOW's recipe moves to the commands and
   CLAUDE.md points every session at them, with the rule that a
   conclusion from a run's data quotes its query. Its first tests: the
   commands on `events_ipc`, each standard query against known
   answers. Its follow-ups, which make the data worth querying: a
   metrics registry in the kernel (counters and histograms kept at the
   source, OBSERVABILITY "Volume", instead of derived from events) and
   kernel time accounting (TM's statistics; ThreadTimes returns zeros
   today); a failure becoming span status wherever it happens, as
   system calls' NTSTATUS does. After that a live view (`cargo xtask
   observe --stack`: streaming OTLP into a collector and Grafana's stack with
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

### GR. Gap reports from real runs, straight to GitHub

**Goal** (the owner): a gap a user finds by running an application is
caught where it happens and reaches the repository by itself, as a
GitHub issue, so the work queue is fed by real runs and not only by
ours. Nothing leaves a machine without its owner's consent.

**Today**: the ledger is already the work queue and already names what
was asked for: every gap a run hits is counted by (service, what,
detail), with the names gala's own DLLs record through
`GalaRecordMissing`, and dumped after each program, before `boot ok`
and in a state dump (OBSERVABILITY "Missing-features ledger"). On a
developer's machine `cargo xtask report` merges those lines over every
scenario and real program into `target/report/ledger.md`, ranked by
programs and hits. Everything after that is missing: on a machine that
is not ours the dump is in a serial log nobody reads, nothing is kept
across boots, nothing notices that a program died of a gap, and
nothing can send one anywhere.

**Next** (each step test first; the CLI before the desktop,
priority 7):
1. **A gap report on the machine.** The system writes the ledger's
   lines as one report when a program had gaps or did not end well (a
   nonzero exit, a crash report, a DLL that failed to load), into
   `C:\gala\reports` as a bounded ring (a count and a byte budget,
   the oldest dropped and the drop counted). A report holds the
   program (name, its PE's version, its SHA-256), the gala build
   (version, commit, channel), the outcome (exit status or crash code,
   and the module the loader named if it named one), the ledger's lines
   since that program started, and the machine's shape (CPU count,
   memory, firmware, real or virtual; no name). Scenario: a program
   that hits a gap and exits nonzero leaves exactly one report, whose
   lines are the dump's.
2. **Redaction, then consent.** A ledger name can carry the user's own
   name (`C:\Users\<name>\...` in a path a program asked for), so a
   report is redacted when it is written, not when it is sent:
   `C:\Users\<name>` becomes `C:\Users\<user>`, and machine and
   domain names, volume GUIDs and IP addresses go the same way, each
   case fed through a test. Sending stays off until the machine's owner
   turns it on (`gala gaps consent on`, or the question asked once by
   the desktop), with the whole bundle readable first (`gala gaps show
   <id>`, `--json` for exactly what would be posted).
3. **The CLI.** `gala gaps` lists this machine's reports ranked as
   `ledger.md` ranks them (what, how often, which programs), with
   `show <id>`, `send [<id>]`, `consent on|off` and `prune`. It sends
   through the intake (step 4) or, with `--github`, straight to the
   GitHub API with a token from the environment or `gh auth token`:
   that path is how we use it before the intake exists, and the
   fallback when the intake is down.
4. **The intake.** No credential belongs on a user's machine, so the
   device posts to a small service of ours beside the update feed
   (`reports.gala-os.com`, a worker in front of the same account, WS
   and R8), which holds the GitHub App's key and nothing else. It
   fingerprints a gap (its kind, its name, the program it came from),
   searches the open `gap` issues for that fingerprint in a
   machine-readable block, and either opens one -- title `gap:
   <dll>!<what>`, body with the counts, the programs and versions that
   hit it and the gala builds -- or adds the new counts to the issue
   that exists, at most once a day per fingerprint, labelled by kind
   (`missing export`, `missing dll`, `unsupported`, `not
   implemented`, ...) and by program. It answers with the issue's
   number, which `gala gaps` shows beside the gap, so the user can
   follow what they reported. Bounded like every other sender here:
   gzipped, a size cap, a few reports a day per machine, a bounded
   queue when offline, retried with backoff and `Retry-After` (OT's
   `Batcher`).
5. **In the desktop.** The gray question-mark tile of a program that
   stopped answering and the notification a tile sends (SM) get
   "Report this", which is step 3's send with the report already
   chosen; so do the recovery notification after a kernel restart and
   the crash screen's link (CS step 3). One place lists what this
   machine has reported and what became of it.
6. **Back into the work queue.** `cargo xtask gaps` reads the open
   `gap` issues, so the ranked list AG and OF work from is the real
   one, and a fix closes its issue by naming it in the commit. A gap
   whose name says nothing cannot be an issue, which is what the
   ledger's own rule is for (ARCHITECTURE "Gaps are never silent"):
   a module miss has to record the name asked for and not only its
   error code (OF's next step).

**Tests**: a scenario that runs a program into a gap and checks the
report on disk; one that sends it to a fake intake on the host (as
`hostnet`'s and H11's scenarios reach the host), with the redaction
cases and the offline queue; the GitHub side against recorded API
answers, replayed as PK's GitHub tests are, and one real run by hand
before the first release.

**Open** (the owner decides): whether the intake is ours or the device
talks to GitHub with a token the user pastes once (no service of ours
to run, but a user without a GitHub account cannot report); whether
consent is asked on first boot or only when something fails; and
whether a report may carry a crash dump itself, which is large and can
hold the user's data, or only its summary.

### SU. Security updates

**Goal**: every vulnerability that can reach a gala machine has a named
source that finds it, an owner who fixes it and a path that delivers the
fix. R8 is the delivery path for gala itself (every commit on main whose
CI passes is a release on the feed); this item covers where the fixes
come from, and how applications installed on gala get theirs.

The boundary follows Windows: Microsoft Update services Windows and
Microsoft's runtimes, not other vendors' applications, which update
themselves or through winget. What gala replaces of Windows (kernel,
system DLLs, Schannel, crypt32, the CA bundle) is gala's to patch;
what is installed as a package (the .NET runtime, the VC++
redistributables, WebView2, any application) is updated as a package.
Not gala's: the test corpus (Chromium, Nginx, the JDK and the rest run
only inside QEMU and never ship), and distributions inside LX, which
update themselves (R8).

**Sources of fixes**:
1. Dependencies shipped in gala: the Rust crates in the system DLLs and
   `gala.exe` (rustls, the RustCrypto and dalek crates, miniz_oxide,
   fontdue and ttf-parser, the vendored getrandom), Limine, Mozilla's
   CA bundle (additions and distrusts), the mingw runtime pieces
   (THIRD_PARTY.md lists them all).
2. gala's own code, the largest source for an OS: every system call
   from user mode (NT and LX), the network stack, the security model
   (SECURITY "Status" lists its known gaps, e.g. NtOpenProcess granting
   what is asked), and every parser of untrusted bytes (PE images, the
   registry log, zip and cabinets, TUF metadata, X.509, fonts, TLS).
3. Microsoft's advisories (MSRC, Patch Tuesday): gala reimplements
   Windows' behaviour, so a flaw Microsoft fixes in an API's semantics
   may be gala's too. Advisories are public documentation (CLEAN_ROOM
   allows them); Wine's and ReactOS's security fixes are the same kind
   of signal.
4. Installed applications: their vendors' security releases, watched
   by gala (next step 8) and applied through the applications' own
   updaters or through `gala upgrade`.

**Next**:
1. Advisories for dependencies in CI: `cargo deny check advisories`
   (RustSec) on every pull request and on a daily schedule (advisories
   appear without commits), Dependabot or Renovate for `Cargo.lock`, a
   watch on what Cargo does not see (the vendored getrandom, Limine
   releases, the CA bundle). Its first finding is known: `rsa` 0.9 has
   RUSTSEC-2023-0071 (Marvin, a timing side channel in RSA decryption,
   no fixed version), used by `user/bcrypt/src/rsa.rs` for private-key
   operations any program can call; decide between another
   implementation and a documented exception.
2. A private path for fixes: CI publishes every commit on main, so a
   fix committed there discloses its hole. A disclosure policy in
   `.github/SECURITY.md` (docs/SECURITY.md is the security model),
   GitHub's private vulnerability reporting, and a private fork where an
   embargoed fix is built and tested before it lands and ships.
3. Fuzzing of gala's own code, run continuously and not only per pull
   request (OBSERVABILITY "Fuzzing"): a system-call fuzzer in QEMU (the
   kernel never panics on user input), `cargo fuzz` on the host for the
   parsers above (OSS-Fuzz takes open-source projects). Every finding
   becomes a test program, as any bug does.
4. Adversarial review of changes that cross a trust boundary (system
   calls, access checks, parsers, the updater), and audits area by area
   on a schedule; the `unsafe` files audited and the host tests run
   under Miri.
5. Microsoft's monthly advisories triaged: each one about an API's
   behaviour that gala implements becomes a test program checked on
   real Windows (as WD's are).
6. Applications' own updaters work: the services, scheduled tasks and
   logon entries they run from (Task Scheduler: `ITaskService`,
   `schtasks`, the task store, has no item yet), elevation services,
   files replaced while in use (MoveFileEx's delay-until-reboot, the
   Restart Manager), MSI patches (`.msp`) and minor upgrades (PK stage
   5). BR stage 4's "updater and elevation service may stay off" is a
   security gap until Edge's updater runs. Test first: an application
   whose updater replaces it with a newer version.
7. `gala upgrade` without being asked: a scheduled check that upgrades
   (or reports, by the user's choice) every package with a newer
   version, honouring UpgradeBehavior and RequireExplicitUpgrade (PK
   next step 1), with applications that are running; it uses winget's
   source index (PK next step 2). Programs installed outside `gala
   install` are matched through their ARP entries, as winget does.
   Every check, upgrade and refusal as events.
8. Vendors' security releases watched, so gala knows which upgrades
   are security upgrades (winget's manifests do not say: a version is a
   version). A CI job, daily, gathers advisories from CISA's Known
   Exploited Vulnerabilities catalog, OSV, GitHub's advisory database,
   NVD and the vendors' own bulletins (MSRC for Microsoft's
   applications and runtimes, Mozilla's MFSAs, Chrome's release notes,
   Oracle's quarterly patch updates, Adobe's bulletins), and maps each
   to winget package identifiers and version ranges. The mapping
   (vendor and product names, CPEs, to `PackageIdentifier`) is a
   curated table checked into the repository, starting with the
   packages gala's scenarios install and the most installed ones; an
   advisory it cannot map is listed in the job's report, not dropped.
   The result is published as signed metadata in R8's feed (a target
   like any other, so clients check it as they check a release), and
   on a machine `gala upgrade` applies security upgrades first and
   sooner than its schedule (at once for a known-exploited one), and
   `gala list` shows which installed versions have advisories against
   them. Test first: a fixture feed with an advisory against an
   installed test package; `gala upgrade` takes that package first and
   the event names the advisory. Every advisory gathered, mapped,
   unmapped and applied as events.
9. A vendor's security release that does not install or run on gala
   (its installer or updater hits a gap) is a regression of the
   highest rank: the advisory job also installs each mapped security
   release in a scenario and reports the ones that fail, so the gap is
   fixed before users are left on the vulnerable version.

The sandbox limits what an exploited application can do before its fix
arrives: integrity levels and restricted tokens (SEC), job objects and
the browser's sandbox (BR stage 3).

**Done when**: advisories for gala's dependencies fail CI; a fuzzer runs
against the system calls and the parsers without a finding left open;
an application updates itself on gala; `gala upgrade` runs on a
schedule and applies vendors' security releases first, from advisories
published in the feed; a security fix can be prepared in private and shipped through
the feed.

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
- Step 4, sparklines: every metric carries its last minute as a line.
  Performance has a list on the left, as Windows' Performance sidebar,
  giving the CPU total, each processor, memory, disk, network and the
  process, thread and handle counts their value, their peak and their
  line; a row's CPU and Memory cells in Processes and Details carry the
  process's own minute behind the number, from a ring per process kept
  with its row (`taskmgr::Trails`, keyed by pid and start time, dropped
  when the process ends; no allocation per sample). The scenario checks
  the sidebar's lines where only a view that has run some seconds has
  drawn, the busy window's CPU cell risen, and an idle `cat`'s flat on
  its floor. Not covered: no Disk or Network column per process to draw
  in (no per-process counters: ledgered), and no Statistics view yet.

**Next** (each test first, checked on the screen, screenshots):
1. Inspection: open file location, a process's threads with their
   stacks (the state dump's walker), its open handles or fds, and
   "Create dump file" through the crash report's writer.
2. Images: `taskmgr.exe` in System32 of every desktop image (it ships
   per scenario now, as `wsl.exe`), and ProcessTimes/GetSystemTimes
   from the same CPU accounting.
3. Statistics: distributions over time, not only totals, in a
   Statistics view beside Performance (top lists, sortable, p50/p95/p99
   columns, each row's p50 and p99 over time as a sparkline beside it,
   a chosen window of 1, 10 or 60 s) and `gala stats`:
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
   their units (selected rows only when there is a selection), the
   values behind each sparkline, and in Statistics the percentiles'
   sample counts. The same text from `gala ps --copy` and `gala stats
   --copy`; the scenario checks the clipboard's text against the view.
5. Per-process I/O: Disk and Network columns in Processes and Details,
   each with its sparkline as CPU and Memory have (gala counts disk and
   network bytes for the machine, not per process: ledgered
   `taskmgr.exe!Processes(Disk)`, `(Network)`).

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

### IN. The installer: gala onto a machine's disk

**Goal** (the owner): a machine booted from gala's USB image comes up
to the desktop, and a graphical installer there puts gala on its disk;
the machine then boots gala from that disk alone.

**Today**: gala boots from its ISO (Limine on the image's EFI system
partition, ARCHITECTURE "Boot"); with `persist` it uses the whole
virtio disk as C: with no partition table (ARCHITECTURE "C: kept on the
disk"). `\\.\PhysicalDrive0` opens the raw disk for Administrators and
SYSTEM, with the length and geometry IOCTLs and no partition ones.

**Shape**: a native gala program (a Rust Windows executable on
gala's user32, as taskmgr and explorer are), not a Linux one. The raw
disk is an NT device behind the security model, which a Linux
distribution's root does not reach; Windows' own disk IOCTLs
(IOCTL_DISK_GET_DRIVE_LAYOUT_EX, IOCTL_DISK_SET_DRIVE_LAYOUT_EX,
IOCTL_DISK_UPDATE_PROPERTIES) are what it calls, so its test program
runs on real Windows against a scratch disk and gives the expected
results, as every other test program's do. Windows Setup has the same
shape. A small Windows-side tool (shrink C: with Windows' own volume
APIs, write the USB stick) is later, for installs beside Windows.

**Libraries** (permissive; recorded in THIRD_PARTY.md when taken):
`gpt` or `gptman` (GPT tables), `mbrman` (MBR disks), `fatfs` (the EFI
system partition), Limine's own `bios-install` for BIOS machines. Not
taken: GNU parted/libparted (GPL-3), Calamares (GPL, Qt, assumes Linux
underneath), ntfs-3g's ntfsresize (GPL; Windows shrinks its own
volume instead).

**Next** (each step test first):
1. Partitions in the kernel: done for GPT disks (`partition_test`,
   ARCHITECTURE "Disk"): the layout IOCTLs (GET/SET_DRIVE_LAYOUT_EX,
   CREATE_DISK, UPDATE_PROPERTIES, GET_PARTITION_INFO_EX) and each
   partition a device (`\Device\Harddisk0\Partition1`...). Open: its
   run on real Windows (GitHub issue #206), MBR tables read.
2. C: in a partition: done (`persist_partition`; ARCHITECTURE "C: kept
   on the disk"): on a GPT disk C: is the partition of gala's C: type;
   a partitioned disk without one, or an MBR disk, is never formatted.
3. Booting from the installed disk: the layout done (`boot_from_disk`,
   ARCHITECTURE "Boot"): a GPT whose EFI system partition (FAT32) holds
   the image's boot tree as it is, found by the firmware's removable-media
   path `EFI/BOOT/BOOTX64.EFI` with no boot entry, and gala's C:
   partition after it. Open: a firmware boot entry (UEFI variables),
   BIOS machines (Limine's `bios-install` on the disk), Secure Boot's
   enrollment with R8 step 1.
4. The installer program: its command done (`gala setup <disk> --erase`,
   `install_to_disk`: a first boot installs onto an empty disk, a second
   boots from that disk alone to the desktop, C: kept in partition 2;
   ARCHITECTURE "Installing gala onto a disk"). Open: the graphical
   installer over it (pick a disk, confirm, progress; GUI after CLI,
   item SD), installing into free space, every step as events, an answer
   file for unattended runs (Windows' autounattend is the model).
   Waiting (the owner): what installing from a USB stick needs, in order:
   a. The downloadable images carry the release tar (only
      `install_to_disk`'s does), so `gala setup` has a release to copy.
   b. A way to start it from the desktop: an "Install gala" entry in the
      start menu, then the graphical installer (gala has no interactive
      prompt, item CP, and the boot menu's command line is sealed, R8).
      With a and b, a downloaded image installs onto a virtual disk in
      QEMU.
   c. On a laptop: its disk (NVMe, or AHCI: item HW step 1) and its
      keyboard and touchpad (USB HID: item USB); gala sees only virtio
      disks today.
5. Beside Windows: the Windows-side tool shrinks C:, the installer uses
   the space; the boot menu offers both.

**Needs**: item AR's desktop on the image (and x64's), item HW's
drivers for a real machine's disk.

**Done when**: step 4's two-boot scenario passes on both architectures,
and the owner's laptop is installed by it.

### VM. Virtual machines on gala

**Not scheduled** (the owner): in the backlog until it is picked.

**Goal**: virtual machines run on gala, first by emulation, then with
the processor's virtualization, through the interfaces existing
virtual machine programs already use.

**Today**: nothing. CT's containers run on LX with no virtual machine;
WSL2, the Hyper-V backends and Docker Desktop's own VM are out of CT's
scope.

**Next** (each step test first):
1. QEMU's emulation (TCG) as an ordinary program: its Windows build,
   and its Linux build in a distribution, boot a small Linux guest
   inside gala (a scenario checks the guest's serial output). It needs
   what JITs already use (executable memory written at run time,
   threads, timers); 10-50x slower than hardware, enough for small
   guests and tests.
2. gala as the hypervisor: one in-kernel core for both architectures
   (a virtual CPU's run loop and exits, the guest's second-level page
   tables, interrupt injection, the scheduler preempting a CPU running
   guest code); VT-x and AMD-V on x86-64, EL2 on aarch64 (the kernel
   already runs at EL2 with VHE, as KVM does there). Guests are
   isolated as processes are (SECURITY).
3. Linux's `/dev/kvm` on that core (QEMU, Firecracker,
   cloud-hypervisor, crosvm), test programs against real Linux's KVM.
4. The Windows Hypervisor Platform (`WinHvPlatform.dll`, the `WHv*`
   functions, Microsoft's public documentation) on the same core:
   QEMU's `whpx`, the Android emulator, VirtualBox and VMware in their
   Hyper-V modes; test programs against real Windows.

Tests run in CI under emulation: QEMU's TCG emulates AMD's SVM on
x86-64 and EL2 on aarch64 (`virtualization=on`).

**Out of scope**: VirtualBox's and VMware's own kernel drivers (gala
loads no Windows kernel drivers), Hyper-V itself (its manager and
services), device passthrough (GPU's G3 note).

**Done when**: each step's scenario passes; a Linux guest boots under
QEMU with `/dev/kvm` and with `whpx`.

### HW. A real laptop: gala on hardware, waiting for updates

**Goal** (the owner): a laptop installed by item IN runs gala, stays on,
and takes every release from the update feed by itself (R8's updater,
R9's live update).

**Today**: gala runs in QEMU only (virtio disk, network, GPU and input;
PS/2 on x86-64). On a machine without PS/2 the input thread says so and
stops (item USB).

**Next** (each test first, in QEMU with the same device, then on the
owner's laptop):
1. Disks: AHCI (`-device ahci`) and NVMe (`-device nvme`) drivers from
   their published specifications, as disks under gala's file systems
   and `\\.\PhysicalDrive<n>`.
2. Input: item USB's device interrupts, xHCI and HID.
3. A network: a wired USB Ethernet adapter (CDC-ECM/NCM) first, with
   WF layer 1's DHCP; then Wi-Fi (item WF).
4. The screen: the firmware's framebuffer (GOP) on x86-64; on ARM
   laptops what their firmware gives.
5. Power: the lid, the battery's state, the backlight, sleep (ACPI).
6. A machine-sized test: R8's updater and R9's handover on the laptop,
   every release taken for a week with nothing restarted by hand.

**Done when**: step 6 holds on the owner's laptop.

## Known issues and stopgaps

Each entry: what, why it matters, the structural fix, the owning item.

### Stopgaps (to replace)

- **`d3d11_test`'s screen check expects red and blue swapped**
  (`0x336699` reaches the screen as `0x996633`): DXVK asks for a
  `VK_FORMAT_R8G8B8A8_UNORM` swapchain for any 8-bit UNORM back buffer,
  Mesa's win32 WSI advertises that format, and the GDI present path --
  the only one Mesa gives a software driver -- copies the image into a
  BI_RGB (so BGRA) DIB section without swizzling it. The swap is in the
  prebuilt stack, not in gala: it is the same on Windows with lavapipe,
  and `vk_test`, which asks for `VK_FORMAT_B8G8R8A8_UNORM` itself, gets
  its colours right through the same window path. Expecting what the
  stack does keeps presentation covered and fails loudly when the stack
  changes; what the frame should be is in the comment beside it.
  Structural fix: a Vulkan driver whose present path matches what it
  advertises -- Mesa's WSI fixed upstream (gala builds no Mesa today,
  only fetches the release), or the GPU-backed driver of G3, which
  presents through DXGI and not GDI. Item G2.

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

- **AFD serves a socket request with the endpoint's lock held**
  (`nt::afd::try_complete`, `submit`, `cancel`): serving a request copies
  the program's memory -- `attempt` its buffers, `finish` its
  IO_STATUS_BLOCK -- while the endpoint's `pending` lock is held, and a
  copy makes a page that is not there, which reads the file or the disk
  and waits. That is a lock held across a wait: a kernel panic in a run
  where the page has been dropped under memory pressure (the state dump
  held `thread.name` the same way, which is how this came up). The lock
  is also what keeps a socket's receives, sends, accepts and connects in
  order, so the copies cannot simply move out of it; the three places
  carry a `sync::lockdep::UserMemoryUnchecked` guard, which turns off
  lockdep's check at every copy for them (a copy that does wait still
  panics, in `none_held_at_switch`). Structural fix: serve one request
  with the lock let go -- its entry stays in the list with its operation
  taken out, so the requests of its kind still wait behind it, a cancel
  that comes meanwhile is answered when the serve ends, and the handover
  carries no half-served request; then the guards go and the check covers
  the whole kernel. Item R1 (invariants).

### Defects known, not yet fixed

- **CI caches keyed on what every branch changes**: every cache key in
  `ci.yml` ends in `hashFiles('xtask/src/main.rs')`, and test-first makes
  a new boot scenario change that file, so each branch writes its own
  set -- six prebuilt caches of about 900 MiB and the Linux roots'
  packages at 320 MiB, over 5 GiB of a repository limit of 10 GiB. The
  limit is therefore always met and eviction always running: one
  branch's caches push out the last branch's, and a rerun of a run's
  failed jobs can find that the packages the `debs` job wrote are gone
  (which is why the boot shards fetch for themselves what their restore
  did not hold, instead of demanding the key). What the keys should name
  is what decides the contents: the pins. They sit among the scenarios
  in a file of 29 777 lines, so the fix is to move them to their own
  module and hash that -- the packages' pins for `gala-debs-`, the
  prebuilt programs' for `gala-prebuilt-` -- after which a branch that
  adds a scenario shares both caches instead of replacing them.
- **Three more tables scanned for a free slot**, the pattern the
  registry's arena no longer has (ARCHITECTURE "Registry"):
  `fs::ramfs`'s `slot`, `nt::object`'s handle tables and `net::tcp`'s
  connections all run `nodes.iter().position(Option::is_none)` for every
  entry they place. None is measured as costly yet and the arithmetic
  says why: the file tree holds 12 213 nodes after an Office install, so
  its scans are about 7.5e7 checks against the registry's 9.2e8 -- some
  75 ms of the 1.1 s that replay takes, inside its own noise -- and the
  busiest handle table in that run held 153 handles. They are quadratic
  all the same, in a count no invariant bounds (a program may hold tens
  of thousands of handles, a file tree may hold a disk's files), and
  each wants the same free list. The registry's measurement is the
  argument for doing them before something else grows: 2.1 s to 0.84 s
  of replay, and a hang watchdog line gone.
- **`arm_lx_inkscape` stalled once on CI** (1 of 5 runs; 0 of 9
  locally, 4 of them at once beside busy host loops): after glycin
  SIGKILLed its image loader (as it does on every run), Inkscape's main
  window never came; both CPUs idle, the Wayland server with no work,
  two Inkscape threads in wait4 (`lx/fork.rs`) and the main thread on a
  futex, until the 300 s limit. Reading wait4 and the child's end
  (`state::become_zombie`, `child_ended`) found no window for a lost
  wakeup. The state dump now lists each Linux process's children
  (running, zombies) and its wait4/waitid waiters with the pid each
  wants, so the next stall names what the threads waited for.
- **`arm_lx_xdesktop` timed out once on CI** (its siblings
  `arm_lx_gtk`, 27 s, and `arm_lx_xterm`, 29.8 s, passed in the same
  job, as did the longer `arm_lx_inkscape` and `arm_lx_gimp_batch`):
  after the desktop showed its window (`desktop: shown window
  'Xwayland on :1'`), a shell forked a child that ran `/usr/bin/xkbcomp`
  to exit 0 and did it again, three times over, and then the watchdog
  said `thread 9 (sh) has been in the kernel for 20 s without blocking
  or reaching user mode (CPU 1, rip 0xffffffff8030b608)`. The flight
  recorder's last records are that process's `read` calls, one every
  3 ms or so, to the end of the run. **That watchdog line was the
  instrument's own fault**: the Linux dispatcher credited no progress
  when a call ended, so a thread whose work is nearly all kernel time
  -- and a `read` every 3 ms is a call ending every 3 ms -- was
  reported as stuck, because the only other thing that counted was a
  timer tick landing in user mode. A finished Linux call is progress
  now (OBSERVABILITY "Hang watchdog"), and a watchdog report fails its
  run, so a report here would be real. What is still unexplained is the
  timeout itself, 300 s with both CPUs idle: the host was loaded (load
  8.11 on 4 CPUs, 40.4 s of the run waiting for a host CPU), and what
  the `read` answers in that loop -- a pipe whose writer is gone,
  answering 0 forever -- is where to start.
- **`arm_lx_gimp` failed its screen check once on CI** (shard 5; not a
  timeout, and the same false watchdog report as `arm_lx_xdesktop`
  above): the line named `thread 4294967304 (sh)`, which is
  `0x1_0000_0008`, scheduler slot 8 generation 1, which the thread dump
  gives as `pid 20 tid 24 (gimp-3.2)` -- the scheduler's name is the
  shell's because an `execve` does not rename the thread. The flight
  ring held that `execve` at 3.357 s and nothing until the watchdog's
  mark at 23.368 s, an `execve` jumping to the new image and recording
  no `syscall-done`, so gimp's loading of its libraries looked like a
  thread making no progress. At 62 s the thread dump had gimp in user
  mode (rip `0x7ffffdb211f0`), alive and running when the check gave
  up: what is left is slowness on a loaded host, not a hang. Reading it
  cost two wrong turns, both of them the instruments': the log's
  addresses were symbolized against the x86-64 kernel (fixed,
  OBSERVABILITY "Symbolized backtraces") and a Linux thread's dump
  names no system call (below).
- **A starved vCPU cannot be told from one with interrupts off**
  (open): the hang watchdog's CPU report (`watchdog: CPU N has taken no
  timer interrupt for 5 s`) is the one of its two that cannot fail a
  run. Both now count the guest's own timer ticks rather than the TSC,
  which settles a pause of the whole machine (`virtio_gpu_reset`
  reported a CPU stuck over a pause of QEMU's own) and every thread
  report; what is left is one vCPU the host does not run while its
  sibling does, which takes no timer interrupt for exactly the reason a
  CPU spinning with interrupts off does not. The guest would have to be
  told how much host time it was given -- KVM's steal-time page, which
  gala does not read -- and then the report could fail a run like the
  thread's (OBSERVABILITY "Hang watchdog", `xtask::WATCHDOG_REPORT`).
- **A Linux thread's state dump names no system call** (open): the NT
  thread lines say `in a system call false`, with no call name, no
  arguments, no wait object and no handle detail, for every Linux
  thread, because `lx/dispatch.rs` never calls
  `nt::thread::set_in_syscall` and the detail those lines print is the
  NT path's (`syscalls::service_name`, the handle tables, the keyed
  events). A thread in the middle of a `read` therefore reads as a
  thread doing nothing, which is how `arm_lx_gimp` above was misread
  twice. The fix is to record the Linux call number and name it from
  `lx`'s own table while skipping the NT-only detail; the scheduler's
  `prepare_wait` caller is the only clue until then (OBSERVABILITY
  "State dump").
- **A lock can be marked held by a thread that is not running**
  (`sync::lockdep`): a held-lock slot belongs to the CPU and carries the
  thread that took it, and `none_held_at_switch` panics if a thread
  gives up its CPU with one held, so a slot's holder should be the thread
  the CPU runs. Twice it was not, both found by the user-memory check
  while it still looked at every slot: `memload_test` panicked with the
  disk's lock (`Option<VirtioBlk>`) marked held by a thread that was
  running on neither CPU, and `postgres` with a handle table's lock
  marked held by a thread that was running on the other CPU. Either a
  take lands in one CPU's slots and its release in another's, or a
  thread leaves its CPU without the switch check seeing it -- and the
  second is a real lock held across a wait. Until it is understood the
  user-memory check only looks at the copying thread's own locks (a copy
  is not where this is diagnosed), so neither sighting is reported any
  more; `none_held`'s message already allows for a foreign holder. Where
  to start: every slot checked at every lock taken, and the flight
  recorder on the virtio-blk and handle-table paths. Item R1
  (invariants).

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
- **`clock_test`'s simulated rate measures the host's scheduling**: the
  last check of the `control` step divides the program time a 30 s
  simulated delay produced by the raw time it cost and asks for at
  least 500 of the 1000 it set, so a host that leaves the VM off a CPU
  for a few of those 30 ms fails it (1 of the scenario's last 24 runs;
  its triage line read `5.4s run of 11.2s wall (5.4s waiting for a host
  CPU)`, and it passes 3 of 3 alone). Nothing inside the VM can tell
  that time apart: the raw clock is the TSC, which runs while the vCPU
  does not. Fix: either the kernel accounts for the time it was not
  running (a gap between timer ticks, steal time) and the check measures
  against that, or the check keeps only what is gala's own -- the
  program clock ran no faster than it was asked to, and 30 s of it cost
  less than 30 s of raw time. Item R7.
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
| `office_install_local` | the same install from the host's cache of that download (`cargo xtask office-source`, put on the disk, installed from `C:\OfficeSource`): development's fast path, and the disk `office_word` and `office_excel` run on | OF |
| `office_install_gui` | the same install again with ODT's and Click-to-Run's own windows, to watch (`XTASK_SHOTS`) | OF |
| `office_word`, `office_excel` | an app on a copy of that install's disk (16 GiB): neither draws its window yet, so both fail on the screen check | OF |

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

- **No binaries in the repository** (the owner): `user/bin` (about
  400 files, 71 MB, rewritten by most changes to a DLL) goes; what is
  built from source is built, not checked in. Building the test
  programs on Windows, to run them there, is that machine's job (the
  Rust ones are freestanding and link with rust-lld, so cargo builds
  them on Windows; the C ones need mingw-w64 there), not a reason to
  keep copies. Steps: the scenarios, um-test and the module graph read
  what the build just made in `target/` (which also ends the pitfall
  of a scenario running a stale copy until `export-tests`); the
  third-party programs (Go, mingw-w64) are built by xtask from their
  pinned sources and cached in CI, keyed by the build script and the
  source's checksum; `run-all.ps1` takes the folder a build made;
  then `user/bin`, `export-tests`' copying and CI's `git diff
  --exit-code user/bin` check go, with README, WORKFLOW and
  ARCHITECTURE's mentions. Last, the history (the owner): every past
  binary is removed from it (`git filter-repo` over a full clone,
  dropping `user/bin` and the other built files), which rewrites every
  commit from the first binary on: main is force-pushed (its ruleset
  allows that for the step), open pull requests and every clone and
  session branch are rebased or remade, and GitHub support is asked to
  drop the old commits that pull request refs keep reachable. Done in
  a quiet moment, with no work in flight; the old hashes in docs and
  commit messages stop resolving.

- **Hygiene sweep** (the owner; in a few months, not now): first
  enumerate the practices gala's code should not follow, by sweeping
  the code (e.g. large arrays on the stack in kernel and DLL code,
  which a signal or exception path runs on a limited stack); then, for
  each enumerated item, a subagent sweeps the code for instances and
  fixes them. Prompted by the richer crash report (register memory and
  the deeper stack, 1 KiB arrays on the exception path), suspected when
  `fpu_test` failed in gala-um; the cause was elsewhere (a new thread's
  first entry took the host's SSE registers: docs/GALA_UM.md).

- **gala on Windows, without virtualization** (the owner): gala-um's
  shape on a Windows host: the kernel as an ordinary Windows process
  (a hosted platform beside `kernel/src/hosted`'s Linux one), Windows
  programs run inside it, their memory in its address space. The hard
  part is the system calls: Linux gives gala-um the program's system
  calls (seccomp's SIGSYS), Windows has no such interception, so a raw
  `syscall` instruction would reach the host's own kernel. Programs
  that enter the kernel through ntdll only (most do) reach gala with
  gala's ntdll in the host's place (calls, not traps); the rest need
  their `syscall` instructions found and redirected, by patching
  decoded code or by translation (XT's machinery). Other questions:
  address-space layout (the host's DLLs and gala's programs in one
  process; fixed-base images), exceptions (vectored handlers for the
  program's faults), threads (host threads as gala's), and the host's
  window for the desktop (as U5's on Linux). Today gala-um already
  runs on Windows through WSL2 (a lightweight VM).
- **gala in a web browser (WebAssembly)** (the owner): the kernel built
  for `wasm32` on a hosted platform of its own (no MMU: user memory
  through the CPU emulator's own translation; threads as Web Workers
  over shared memory; the screen a canvas; storage in the browser), and
  the programs' x86-64 code run by XT's step 1 interpreter built for
  wasm, then a JIT that emits WebAssembly modules per hot block (as
  browser-based x86 emulators do). The interpreter is the shared piece:
  written once for XT (x64 on ARM64), it serves here unchanged.
  Interpreted speed first; correctness against the existing x64 test
  programs, as XT's.
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
- **Docker Desktop for Windows** (the owner: wanted, not scheduled;
  CT's engine relay serves Docker's tools first). Stock Docker Desktop
  installed by its own installer and run unmodified: its installer and
  the checks it makes (Windows version, virtualization and optional
  features through WMI and DISM), its backend service and the
  privileged helper service, the `docker-desktop` distribution it
  imports and drives through the WSL facade (CT steps 2-3: the WSL
  service's COM interface, hvsocket and vsock), its own
  `\\.\pipe\docker_engine` in place of gala's relay, credential
  helpers on the Windows credential store, then the dashboard
  (Electron, after BR stage 2) and its bundled Kubernetes. Worth doing
  for its own sake: everything it needs is standard Windows (services,
  WMI, installers, COM, named pipes, Electron) that other programs use
  too. Its licence terms (a paid subscription at larger organisations)
  govern running it; never committed, fetched by xtask and run only in
  QEMU.
- **Chrome as well as Edge** (the owner): item BR's goal is stock
  Edge, and both browsers are the same Chromium underneath, so Chrome
  is mostly BR's work over again with another vendor's services. Two
  pieces of its own: **Chrome for Testing** (the published zip with
  symbols, fetched as BR already fetches the Chromium snapshot) is
  branded Chrome without an installer, so it finds the
  Google-service gaps the plain Chromium build has none of
  (sign-in, Omaha's update checks, Widevine) - a half-step inside BR
  stage 2 once the window is done; **Chrome as a user installs it**
  (`mini_installer.exe` or the enterprise MSI) then needs item PK's
  installer handling, Omaha as a service and a scheduled task, and the
  COM elevation service for a per-machine install. None of BR's open
  blockers is vendor-specific (the out-of-memory exceptions
  0xE0000008, ChromeDriver not exiting, the WPT run), and Widevine and
  the proprietary codecs are a piece apart for either browser.
- **gala's own drivers** against published standards: after virtio,
  e1000 (AHCI and NVMe: item HW; xHCI and device interrupts: item USB;
  sound: item SND).
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
- **Updates that change data layouts, in place**: migrations, as a
  database has for its schema, for the two paths that cannot change a
  layout today. R9's handover needs none (the new kernel rebuilds its
  objects from the versioned record, whose readers are already the
  migrations). (1) SN's hot swap refuses a static that changed size and
  any other change to data its old code built. With migrations: every
  changed type gets a transformer from the old layout to the new, run
  while every thread is stopped (as the swap already stops them),
  finding each instance and each pointer to it from the build's type
  information, as a moving collector finds them; types that only grow
  can instead keep the new fields beside the old object (Linux
  livepatch's shadow variables) and need no pointer moved. A changed
  type live in a register or on a stack is refused, as a changed
  function on a stack is now; pointers hidden in integers or unions
  are what the type information cannot see, so such types stay
  refused. (2) A handover keeps programs' memory in place, so it only
  carries gala's DLLs with unchanged layouts (the layout digest). R9's
  user-mode half restarts processes onto a new system image; the
  alternative for gala's own DLLs, whose types gala knows, is a
  per-DLL migration of its state inside each process, the old code
  kept mapped for return addresses, vtables and callbacks still
  pointing at it (as the hot swap keeps it). Restarting stays the
  default; migration is for programs that cannot be restarted.
- **Change this program: a coding agent from the window menu** (the
  owner): the user changes one of gala's own user-mode programs
  (Explorer, the task manager, the start menu once SH moves it out of
  the kernel) by saying what they want; a coding agent edits the
  program's source, builds it, and the running program is replaced by
  the new build, its window and state kept. The owner's mockups,
  `docs/images/change-program-<n>-<step>.png`, show the flow, one per
  step; only this feature's elements in them are its scope (the menu
  entry, the dialog, the progress, the toasts), the rest of what they
  draw (Explorer's tabs and toolbar, the taskbar, file names) is
  illustration, not a target:
  1. The window menu (`-1-menu`): "Change this program..." between
     Maximize and Close, with a wand icon. It appears only on programs
     whose source gala has: a window maps to its source by its module
     (the image's crate), never on third-party programs. The same
     action is a `gala` command first (item SD): `gala change <window
     or program> "<request>"`.
  2. Inspect: before or while writing the request, the user points at
     the part of the window to change. The cursor picks the UI
     Automation element under it (item UIA's ElementFromPoint: Name,
     ControlType, AutomationId and bounding rectangle, a fragment's
     elements included) and outlines it on the screen, as browsers'
     developer tools do; the click attaches the element, its path up
     the tree and a screenshot of it to the request ("this column
     header"). gala's own programs make that exact: every control gets
     a stable AutomationId, and the build writes a map from each
     AutomationId to the source that creates it (file and line, as
     lld's map gives the hot swap its functions), so the agent starts
     at the right line instead of searching.
  3. The request (`-2-request`): a small dialog beside the window
     titled "Change <program>", a text box, a line saying the agent
     will edit, build and restart the program with the open folder and
     selection kept, Cancel and "Make the change".
  4. Progress (`-3-progress`): a checklist with live state (reading
     the source, editing <n> files, building with its time,
     restarting), a progress bar, "Show the change" opening the diff;
     the program stays usable meanwhile. The agent and the build run
     on the machine itself, never on another host (no second machine
     the feature depends on): the agent is a gala program calling the
     model's API over gala's own network stack, and the build uses a
     toolchain installed on the machine (rustc, cargo, lld, the
     program's source at the image's commit and its dependencies),
     installed on first use as a package and removable again by a disk
     cleanup tool, which lists it with its size. So the real flow needs
     self-hosting (above) to run rustc on gala first. The agent is a
     small loop, not a general coding assistant (running one such as
     Claude Code on gala needs Node.js on gala, a separate goal): the
     request, the picked part's automation id and that part's source
     files go to the model with three tools - read a file, replace an
     exact piece of text, build - and compiler errors go back to it
     for a bounded number of rounds; every round, edit and build is
     logged, and the card's steps are those events. Stopgap until gala
     runs rustc and calls HTTPS itself: the loop runs on the
     development host against the real source and API, to prove it;
     the same loop moves onto gala unchanged. The agent's
     tools are item MCP's (the ledger, screenshots, UIA), so it checks
     its change on the running desktop; the change is a branch or diff
     the user can read.
  5. Updated (`-4-updated`): the new build in the same window, folder
     and selection; a toast "<program> was updated" with Undo and
     Details. The swap: the program writes its state as a versioned
     record (folder, selection, window place, tabs), the new build
     reads it, the old process exits; a record whose format changed
     goes through a migration the agent writes with the change, as
     R9's handover readers are its migrations. In-place migration
     (above) is for programs that cannot restart.
  6. Rollback (`-5-rollback`): the old build is kept; Undo returns to
     it, and a new build that crashes before it is up is replaced by
     the old one automatically, with a toast saying so ("See what
     happened" opens the crash report and the agent's log, "Try again"
     sends both back to the agent).
  Observability: every request (with its inspected element), the
  agent's change, the build, the swap and a rollback in the event
  stream; the running build's commit and change in the program's
  About and crash reports. Security: the agent works with the user's
  token only, and a replaced system program is signed under R8's rule
  or runs as the user's own build, never silently as gala's.
  Status: the window menu is on the screen (Alt+Space, a caption right
  click) and File Explorer has the flow as a preview with no agent
  behind it (ARCHITECTURE "The file explorer", "Change this program"):
  the menu item, the request card, Inspect over the window's parts by
  automation id, the steps on a timer, a last card saying nothing was
  changed; scenario `explorer` drives it. Next: the agent loop
  (the stopgap above first), the AutomationId-to-source map, the state record and
  the swap, Undo; the item on Task Manager.
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

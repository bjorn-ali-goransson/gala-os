// The site's content, as data. The page (App.tsx) and llms.txt (llms.ts)
// are both built from it, so they cannot disagree. Where the facts come
// from: components are docs/ARCHITECTURE.md's subsystems, statuses are
// docs/ROADMAP.md's "Status at a glance", programs are the README's
// "What runs on gala" (the scenario names are the boot scenarios that
// check them), tooling is docs/WORKFLOW.md's "Commands".

export type Status = "works" | "building" | "planned";

export const statusLabel: Record<Status, string> = {
  works: "Works",
  building: "In progress",
  planned: "Planned",
};

// ---------------------------------------------------------------------------
// Components

export type Component = {
  name: string;
  /** Where it lives in the repository. */
  path?: string;
  status: Status;
  what: string;
};

export type ComponentGroup = {
  id: string;
  title: string;
  intro: string;
  items: Component[];
};

export const componentGroups: ComponentGroup[] = [
  {
    id: "core",
    title: "The kernel core",
    intro:
      "gala's own design, owing nothing to Windows. Everything above it is built on these pieces, and they are free to change.",
    items: [
      {
        name: "Boot",
        path: "kernel/src/boot",
        status: "works",
        what: "Limine loads the kernel from a hybrid image that starts on UEFI and BIOS machines, from a CD, a disk or a USB stick.",
      },
      {
        name: "The architecture seam",
        path: "kernel/src/arch",
        status: "works",
        what: "Everything processor-specific sits behind one interface: x86_64 today, aarch64 being built behind the same seam.",
      },
      {
        name: "Memory",
        path: "kernel/src/mm",
        status: "works",
        what: "Physical frames, page tables, address spaces with demand-zero paging, guard pages and copy-on-write. Every allocation can fail without the kernel panicking.",
      },
      {
        name: "Threads and processors",
        path: "kernel/src/sched",
        status: "works",
        what: "Kernel stacks, context switches and the scheduler, on every CPU, with priorities and affinity as Windows programs set them.",
      },
      {
        name: "One clock",
        status: "works",
        what: "The CPU's counter is calibrated against a reference timer and checked against the real-time clock, so the whole system agrees on the time.",
      },
      {
        name: "Files and disk",
        path: "kernel/src/fs, kernel/src/disk",
        status: "works",
        what: "C: as a case-insensitive file system with Windows' share modes, kept on a virtio disk as a log that is replayed at the next boot, written back lazily.",
      },
      {
        name: "Networking",
        path: "kernel/src/net",
        status: "works",
        what: "A virtio network card and a TCP/IP stack (smoltcp), with loopback. TCP and UDP, IPv4 and the IPv6 loopback.",
      },
      {
        name: "Screen and input",
        path: "kernel/src/display",
        status: "works",
        what: "The bootloader's framebuffer or a virtio-gpu, a PS/2 keyboard and mouse.",
      },
      {
        name: "Kernel handover",
        path: "kernel/src/handover",
        status: "works",
        what: "The running kernel hands the machine to a new kernel image without a reboot, a newer build included, and the programs keep running: blocked threads are reissued and open TCP and UDP connections survive.",
      },
      {
        name: "Handover of everything else",
        status: "planned",
        what: "Consoles, jobs, pipe and completion-port requests in flight, the desktop and its windows, IPv6 and raw sockets. While a program uses one of these, a handover refuses and says why.",
      },
      {
        name: "Signed system images",
        path: "kernel/src/trust",
        status: "works",
        what: "Every boot file is checked against its hash, an Ed25519-signed manifest is checked by the kernel and before every handover, and a rollback floor refuses older images.",
      },
      {
        name: "Secure Boot",
        status: "planned",
        what: "The chain of trust started from the firmware's own keys, and a release signing key.",
      },
    ],
  },
  {
    id: "nt",
    title: "The Windows personality",
    intro:
      "This layer makes programs believe they are on Windows 11. It reproduces what programs can observe at the system call boundary, and nothing of Windows' internals.",
    items: [
      {
        name: "The NT layer",
        path: "kernel/src/nt",
        status: "works",
        what: "Windows' system calls on top of the core: objects and handles, processes, threads and jobs, virtual memory and sections, files and pipes, waits and timers, consoles.",
      },
      {
        name: "Clean-room system DLLs",
        path: "user/<dll>",
        status: "works",
        what: "ntdll, kernel32, user32, gdi32, ws2_32, ole32, shell32, crypt32 and more than eighty others, written from documentation. Programs load the libraries they expect.",
      },
      {
        name: "Registry",
        status: "works",
        what: "Keys, values, notifications and access checks, seeded at boot and kept on disk.",
      },
      {
        name: "Security",
        status: "works",
        what: "Access tokens, security descriptors, access checks on every open, LSA logons, user profiles, sessions, integrity levels.",
      },
      {
        name: "Services",
        status: "works",
        what: "A service control manager: services start at boot, are restarted when they die, and answer their installers' calls.",
      },
      {
        name: "TLS and HTTP",
        path: "user/schannel, user/winhttp",
        status: "works",
        what: "Schannel with TLS 1.2 and 1.3, client and server, plus winhttp, wininet and the certificate chain engine.",
      },
      {
        name: "32-bit programs (WOW64)",
        path: "user/wow64",
        status: "works",
        what: "x86 programs run as on 64-bit Windows, through a 32-bit ntdll, kernel32, C runtime, user32 and gdi32.",
      },
      {
        name: "Graphics in software",
        path: "user/d2d1, user/mesa",
        status: "works",
        what: "GDI, Direct2D's shapes and WIC's images drawn in software, OpenGL and Vulkan on Mesa's llvmpipe and lavapipe, shown through a virtio-gpu.",
      },
      {
        name: "Direct3D 9 to 12",
        status: "building",
        what: "Direct3D over Vulkan through DXVK and vkd3d-proton, in software first. Direct3D 11 works through DXVK on lavapipe: a device and a swap chain clear, copy and present into a window on the desktop. Drawing with shaders comes next, then Direct3D 9, 10 and 12.",
      },
      {
        name: "GPU acceleration",
        status: "planned",
        what: "Drawing and presenting on a real or virtual graphics card instead of the processor.",
      },
    ],
  },
  {
    id: "linux",
    title: "The Linux personality",
    intro:
      "A second system call layer on the same core. Linux programs run beside Windows ones and share the same files, scheduler and screen.",
    items: [
      {
        name: "Linux system calls",
        path: "kernel/src/lx",
        status: "works",
        what: "Static and dynamically linked (glibc) programs: fork and exec, signals, threads and futexes, epoll, TCP and Unix sockets, shared memory and memfd. Each test's output must match real Linux.",
      },
      {
        name: "The rest of Linux",
        status: "planned",
        what: "Job control (stopping and continuing, process groups), vfork, ptys, /proc, and Linux's hard and symbolic links seen from Windows programs.",
      },
      {
        name: "A Wayland server",
        status: "works",
        what: "Built into the kernel. GTK programs such as GIMP and Inkscape open their windows on the same desktop as Windows programs.",
      },
      {
        name: "X11 through Xwayland",
        status: "works",
        what: "Older X11 programs run through a rootless Xwayland, with gala as the window manager.",
      },
      {
        name: "Distributions, WSL-style",
        path: "user/wsl",
        status: "works",
        what: "Ubuntu roots in the start menu, wsl.exe, and \\\\wsl.localhost paths visible to every Windows program.",
      },
    ],
  },
  {
    id: "desktop",
    title: "shasha, the desktop",
    intro:
      "shasha (Arabic for “screen”) is gala's graphical shell. Everything it does can also be done with the gala command, and a server never starts it.",
    items: [
      {
        name: "Compositor and shell",
        path: "user/shasha",
        status: "works",
        what: "Windows, a taskbar, a start menu with search, a notification area, snapping, wallpapers, UI Automation for testers and screen readers. The taskbar, its notification area, the start menu and the wallpaper are an ordinary program, shasha.exe, not part of the kernel.",
      },
      {
        name: "Task manager",
        path: "user/taskmgr",
        status: "works",
        what: "One list for Windows and Linux processes alike, Linux ones grouped by distribution, with CPU, memory, performance graphs and “Not responding”, and an End task button that works on either kind.",
      },
      {
        name: "Inspecting a process",
        status: "planned",
        what: "A process's threads with their stacks, its open handles, and a dump file, from the task manager.",
      },
      {
        name: "File explorer",
        path: "user/explorer",
        status: "works",
        what: "One tree for the Windows drives and each Linux distribution, with search, copy, paste, rename, delete, properties with Linux permissions, and folders that update live.",
      },
      {
        name: "Recycle Bin and file dialogs",
        status: "planned",
        what: "The Desktop folder and the Recycle Bin, context menus, the Open and Save dialogs programs use, drag and drop with progress, and thumbnails.",
      },
      {
        name: "Consoles and remote shells",
        path: "user/conhost, user/cmd",
        status: "works",
        what: "gala's conhost.exe and cmd.exe, console windows on the desktop, and pseudo consoles for SSH logins.",
      },
      {
        name: "Sessions",
        status: "works",
        what: "Services in session 0 and users in their own, as on Windows: each session with its own desktop, window stations and named objects, SSH logins, and the screen switched between sessions.",
      },
      {
        name: "Remote desktop",
        status: "planned",
        what: "A session's desktop over RDP, so Windows' own Remote Desktop clients connect to it, a session manager, and sessions carried across a handover.",
      },
    ],
  },
];

// ---------------------------------------------------------------------------
// Current focus

export type Step = { label: string; status: Status; detail?: string };

export type Focus = {
  id: string;
  title: string;
  lede: string;
  steps: Step[];
};

export const focus: Focus[] = [
  {
    id: "office",
    title: "Microsoft Office",
    lede: "Word, Excel and PowerPoint as Microsoft ships them through Click-to-Run, then the new Outlook. Before any work started, every DLL and function the three apps import was surveyed and ranked against what gala has. That survey sets the order of the work.",
    steps: [
      {
        label: "Survey what Office imports",
        status: "works",
        detail: "1486 files, ranked against gala",
      },
      {
        label: "Install with Microsoft's own installer",
        status: "works",
        detail: "Click-to-Run onto gala's disk",
      },
      {
        label: "Word and Excel start",
        status: "works",
        detail: "their processes load and run",
      },
      {
        label: "Word and Excel windows",
        status: "building",
        detail: "Word puts its window up, then stops for want of a rendering device",
      },
      {
        label: "Shapes and images",
        status: "works",
        detail: "Direct2D and WIC in software",
      },
      {
        label: "Text in Direct2D",
        status: "building",
        detail: "through DirectWrite",
      },
      {
        label: "Direct3D 11 in software",
        status: "building",
        detail: "DXVK over Mesa's lavapipe presents to a window; shaders next",
      },
      {
        label: "The new Outlook",
        status: "planned",
        detail: "WebView2 and MSIX packages",
      },
    ],
  },
  {
    id: "arm",
    title: "ARM64",
    lede: "Moving gala to a second processor architecture. Because everything processor-specific already sat behind one seam, this is a port and not a rewrite. The aarch64 kernel now boots to the end on QEMU's virt machine.",
    steps: [
      {
        label: "Boot, page tables, exceptions",
        status: "works",
        detail: "its own tables, faults reported",
      },
      {
        label: "Interrupts, timer, threads",
        status: "works",
        detail: "GICv3, the generic timer, scheduler",
      },
      {
        label: "The second CPU",
        status: "works",
        detail: "IPIs and TLB flushes",
      },
      {
        label: "PCI, disk and network",
        status: "works",
        detail: "virtio over ECAM, both ways",
      },
      {
        label: "The clock",
        status: "works",
        detail: "checked against the PL031 RTC",
      },
      {
        label: "User mode",
        status: "building",
        detail: "156 x64 test programs and 40 Linux ones pass as ARM64 builds",
      },
      {
        label: "x64 programs on ARM",
        status: "planned",
        detail: "binary translation",
      },
    ],
  },
];

// ---------------------------------------------------------------------------
// What runs

export type Program = {
  name: string;
  version?: string;
  what: string;
  scenarios: string[];
};

export const windowsPrograms: Program[] = [
  {
    name: "Chromium",
    what: "an unmodified build opens its window, takes input, exposes its accessibility tree and answers ChromeDriver",
    scenarios: ["chromium_window", "chromium_uia", "chromium_driver"],
  },
  {
    name: "Java",
    version: "JDK 21",
    what: "the HotSpot VM compiles and runs a program, and serves HTTP",
    scenarios: ["java_hello", "java_http"],
  },
  {
    name: ".NET",
    version: "8",
    what: "C# on the JIT and the garbage collector",
    scenarios: ["dotnet_hello"],
  },
  {
    name: "Python",
    version: "3.13",
    what: "pip installs over HTTPS; asyncio, multiprocessing and numpy",
    scenarios: ["python_pip", "python_async", "python_mp", "python_numpy"],
  },
  {
    name: "PostgreSQL",
    version: "17",
    what: "initdb, then the server answering the host's psql",
    scenarios: ["postgres"],
  },
  {
    name: "OpenSSH",
    version: "9.8",
    what: "key and password logins, an interactive shell, sftp",
    scenarios: ["sshd_login", "sshd_shell", "sshd_sftp"],
  },
  {
    name: "Nginx",
    version: "1.30",
    what: "master and worker processes, HTTPS with OpenSSL",
    scenarios: ["nginx_master", "nginx_tls"],
  },
  {
    name: "Go",
    version: "1.24",
    what: "goroutines, garbage collection, child processes, net/http",
    scenarios: ["go_goroutines", "go_gc", "go_http"],
  },
  {
    name: "PowerShell",
    version: "7.4",
    what: "runs a command and prints its version table",
    scenarios: ["pwsh_hello"],
  },
  {
    name: "TinyCC",
    what: "compiles C on gala, then compiles itself",
    scenarios: ["tcc", "tcc_self"],
  },
  {
    name: "SQLite",
    what: "compiled on gala; two processes writing one database",
    scenarios: ["sqlite"],
  },
  {
    name: "Lua",
    version: "5.4",
    what: "built on gala by its own makefile",
    scenarios: ["lua"],
  },
  {
    name: "Caddy",
    version: "2.8",
    what: "serves a site to the host",
    scenarios: ["caddy_http"],
  },
  {
    name: "FileZilla Server",
    version: "1.12",
    what: "makes its certificates and serves FTP over TLS",
    scenarios: ["filezilla_server"],
  },
  {
    name: "FlaUI",
    version: "5.0",
    what: "drives another program's window through UI Automation",
    scenarios: ["flaui_test"],
  },
  {
    name: "busybox-w32",
    what: "shell scripts, file tools, make",
    scenarios: ["busybox_script", "tcc_make"],
  },
];

export const linuxPrograms: Program[] = [
  {
    name: "GIMP",
    version: "3.2",
    what: "opens an image, inverts it, exports a file identical to the one real Linux writes",
    scenarios: ["lx_gimp"],
  },
  {
    name: "Inkscape",
    version: "1.4",
    what: "draws an SVG on its canvas and renders it to the same PNG as real Linux",
    scenarios: ["lx_inkscape"],
  },
  {
    name: "GTK 3 programs",
    what: "windows drawn and clicked, under Wayland and under X11",
    scenarios: ["lx_gtk", "lx_gtk_x11"],
  },
  {
    name: "xterm",
    what: "an X11 terminal on a pseudo terminal, through Xwayland",
    scenarios: ["lx_xterm"],
  },
  {
    name: "busybox",
    what: "the shell and its tools; httpd serving wget",
    scenarios: ["lx_busybox", "lx_httpd"],
  },
];

// ---------------------------------------------------------------------------
// What gala can do that an ordinary system cannot. From ROADMAP R2-R9 and
// SN, ARCHITECTURE "Kernel handover", "Deterministic simulation", "Live
// mocking", "Fault injection", and WORKFLOW "Machine snapshots".

export type Capability = {
  id: string;
  name: string;
  status: Status;
  what: string;
  how: string;
  /** The boot scenarios (or commands) that hold it to account. */
  proof?: string[];
};

/** The large ones: running state that outlives its kernel or its machine. */
export const liveCapabilities: Capability[] = [
  {
    id: "handover",
    name: "Replace the kernel without a reboot",
    status: "works",
    what: "The running kernel hands the machine to a new kernel image, without the firmware or the bootloader, and the programs carry on: their memory stays where it is, threads blocked in waits and I/O are reissued, and open TCP and UDP connections survive.",
    how: "Every thread is stopped at a quiescent point and the kernel's state is written as a versioned record. The new kernel can be a newer build with a different state format; what it cannot carry yet makes the handover refuse with the reason, never half-happen.",
    proof: ["handover_k32", "handover_upgrade", "handover_net"],
  },
  {
    id: "snapshot",
    name: "Save a whole machine, resume it in seconds",
    status: "works",
    what: "A running machine is saved when a chosen line appears in its log and resumed from there, as often as wanted. One snapshot can be resumed twice to try two fixes from the same state.",
    how: "The Office install takes 28 minutes to reach the step that fails. Saved there, it resumes in 3 seconds and reaches the end 21 seconds later. A snapshot only resumes on the build it was taken with, and every difference is named.",
    proof: ["snapshot_machine"],
  },
  {
    id: "hotswap",
    name: "Hot-swap DLLs and the kernel into a running machine",
    status: "works",
    what: "A fix to a DLL or to the kernel is patched into a resumed snapshot, into every process that has it loaded, without starting over.",
    how: "The old and new builds are compared function by function. Every program thread is stopped, no changed function may be on any thread's stack, the new code is mapped beside the old and each old entry jumps to its new version. Anything unsafe is refused with the reason. It is a development tool: a hot-swapped run is never counted as a pass.",
    proof: ["hotswap_machine", "hotswap_stack", "hotswap_wow64"],
  },
  {
    id: "live-update",
    name: "Live update of the whole system",
    status: "planned",
    what: "A running gala serving SSH and Nginx updates its system image as well as its kernel, and every connection and session goes on.",
    how: "The kernel half is the handover above. What it adds: carrying consoles, jobs, pipes in mid-request and the desktop across a handover, and restarting user-mode processes one at a time onto a new system image.",
  },
  {
    id: "freeze",
    name: "Freeze a program and move it to another machine",
    status: "planned",
    what: "gala snapshot freezes one program and its children; gala restore starts it again, here or, exported as one file, on another machine, where its windows open on the new desktop.",
    how: "A program snapshot is the handover's record cut down to one process tree: its memory, threads, handles, windows and sockets. Exports are signed and encrypted to their target, since a program's memory holds its secrets.",
  },
];

/** The ones that make bugs reproducible instead of rare. */
export const debugCapabilities: Capability[] = [
  {
    id: "sim",
    name: "Deterministic simulation",
    status: "works",
    what: "One thread runs at a time, a seeded random choice picks which, and time is virtual. A race that shows up once repeats every time from its seed.",
    how: "The approach FoundationDB and TigerBeetle use, applied to Windows programs in gala-um.",
    proof: ["cargo xtask sim-test"],
  },
  {
    id: "replay",
    name: "Record, replay and control the clock",
    status: "works",
    what: "A run's boot time, clock readings and random numbers are recorded and played back. The clock can be set and moved while programs run, their timers following it, and the disk can be made slow on purpose.",
    how: "Wrappers at the device and service boundaries (the clock, the random source, the disk), so a test can put the system in a state that is hard to reach for real.",
    proof: ["cargo xtask replay-test"],
  },
  {
    id: "faults",
    name: "Fault injection",
    status: "works",
    what: "Memory allocations, physical pages, disk requests, network sends and page-file writes fail on purpose, every nth time or at random from a seed.",
    how: "Every allocation in the kernel can fail, so every failure path is exercised rather than trusted.",
    proof: ["cargo xtask fault-test"],
  },
  {
    id: "ledger",
    name: "A ledger of what is missing",
    status: "works",
    what: "Every system call, information class, flag or DLL function a program reached that gala does not implement yet is counted and listed at the end of the run. No gap stays hidden behind a crash elsewhere.",
    how: "Ranked by how often each was hit, the list is the work queue for the next program.",
    proof: ["ledger_test"],
  },
  {
    id: "crash",
    name: "Watchdogs and a crash record that survives reboots",
    status: "works",
    what: "A stuck CPU is caught by a watchdog, and a crash writes its message, backtrace and last events to the disk, where the next boot finds and prints them.",
    how: "An always-on flight recorder keeps each CPU's last 256 system calls and exits, at the cost of a few stores each.",
    proof: ["watchdog", "kernel_watchdog", "crash_record"],
  },
  {
    id: "lockdep",
    name: "Lock-order checking",
    status: "works",
    what: "Two locks taken in opposite orders on two paths are reported the first time the inversion is seen, before they ever deadlock.",
    how: "On in debug kernels, host tests and gala-um, so CI catches an inversion even when the timing never lines up.",
  },
];

// ---------------------------------------------------------------------------
// Planned

export type Planned = { name: string; what: string };

export const planned: Planned[] = [
  {
    name: "Microsoft Edge and WebView2",
    what: "after Chromium, the stock Edge, and the WebView2 runtime that the new Outlook and many other apps embed",
  },
  {
    name: "A remote desktop",
    what: "gala's desktop over RDP, reached with the Remote Desktop clients people already have (mstsc, the Windows App, FreeRDP), on the sessions that already separate users",
  },
  {
    name: "Docker and Podman",
    what: "Podman, Docker and Kubernetes in a Linux distribution on gala, used from Windows through Docker's own CLI and Podman for Windows, with no virtual machine",
  },
  {
    name: "x64 Windows programs on ARM64",
    what: "x64 code translated to ARM64 on the fly, then 32-bit x86 the same way",
  },
  {
    name: "Android applications",
    what: "binder, the Android runtime and software graphics, x86_64 images first",
  },
  {
    name: "An AI-enabled start menu",
    what: "every system action described once, so AI clients can use it as a tool and the start menu can take requests in plain language",
  },
  {
    name: "Sound and USB",
    what: "virtio-sound with WASAPI on top; xHCI with keyboards, mice and storage",
  },
  {
    name: "An installer",
    what: "boot the USB image to the desktop and install gala onto the machine's disk, which then boots it by itself",
  },
  {
    name: "Real laptops",
    what: "disk, USB, network and power drivers, so a laptop runs gala and takes every update by itself",
  },
  {
    name: "Virtual machines",
    what: "QEMU running inside gala, then gala as the hypervisor behind Linux's /dev/kvm and the Windows Hypervisor Platform",
  },
];

// ---------------------------------------------------------------------------
// Tooling

export type Tool = { cmd: string; what: string };
export type ToolGroup = { title: string; intro: string; tools: Tool[] };

export const toolGroups: ToolGroup[] = [
  {
    title: "Build and run",
    intro:
      "One driver, cargo xtask, builds the whole system and boots it in a pinned QEMU.",
    tools: [
      {
        cmd: "cargo xtask build",
        what: "the kernel, the DLLs, the test programs and a bootable ISO",
      },
      { cmd: "cargo xtask run", what: "boot it in QEMU with the serial log in the terminal" },
      {
        cmd: "cargo xtask iso --file app.exe run=app.exe",
        what: "an image that runs your own program; write it to a USB stick to boot a PC",
      },
      {
        cmd: "cargo xtask serve nginx 8080",
        what: "keep a server scenario running and open it in your browser, or ssh in",
      },
      { cmd: "cargo xtask qemu", what: "build the pinned QEMU once" },
    ],
  },
  {
    title: "Test",
    intro:
      "Every feature starts as a test program whose expected output is real Windows'. A scenario boots gala, runs it and checks the output.",
    tools: [
      {
        cmd: "cargo xtask test",
        what: "the whole suite: unit tests, gala-um, record and replay, then every boot scenario",
      },
      { cmd: "cargo xtask boot-test sqlite", what: "only the scenarios whose name matches" },
      {
        cmd: "cargo xtask um-test",
        what: "the test programs in gala-um, in seconds rather than minutes",
      },
      {
        cmd: "cargo xtask report / compare",
        what: "every test program on gala, checked against the same run on Windows 11",
      },
      {
        cmd: "cargo xtask lx-expect",
        what: "the Linux test programs against transcripts from real Linux",
      },
    ],
  },
  {
    title: "Find out why",
    intro:
      "A failure has to explain itself. Every run ends with a ledger of missing functions, and a failed run leaves its log, a triage summary and the state behind.",
    tools: [
      { cmd: "cargo xtask triage <log>", what: "the summary of a failed run: where it stopped and why" },
      { cmd: "cargo xtask symbolize <log>", what: "kernel backtraces resolved to functions" },
      {
        cmd: "cargo xtask screenshot",
        what: "PNGs of the screen at chosen moments of a run",
      },
      {
        cmd: "cargo xtask events / profile / perfetto",
        what: "the JSON event stream, the sampling profiler, and a timeline for ui.perfetto.dev",
      },
      { cmd: "cargo xtask otel", what: "the same events as OpenTelemetry" },
      {
        cmd: "cargo xtask disk-files <image>",
        what: "the files a run wrote to C:, read with the kernel's own code",
      },
    ],
  },
  {
    title: "Shake out the rare bugs",
    intro:
      "Races and lost wake-ups are hunted on purpose rather than waited for.",
    tools: [
      {
        cmd: "cargo xtask fault-test",
        what: "programs run while allocations and network sends fail on purpose",
      },
      {
        cmd: "cargo xtask sim-test --seed S",
        what: "deterministic simulation: a race found once is replayed from its seed",
      },
      { cmd: "cargo xtask replay-test", what: "record a run's inputs and play them back" },
      { cmd: "cargo xtask alloc-audit", what: "finds allocations that could panic in the kernel's machine code" },
      { cmd: "cargo xtask bench", what: "micro-benchmarks, compared with Windows operation by operation" },
    ],
  },
  {
    title: "Long runs",
    intro:
      "Some failures come half an hour into a run, as with the Office install. Work on them starts from a saved machine instead of a fresh boot.",
    tools: [
      {
        cmd: "boot-test … --snapshot-at <line>",
        what: "save the whole machine when a log line appears",
      },
      { cmd: "boot-test --from-snapshot <name>", what: "resume it, as often as you like" },
      {
        cmd: "--hot-swap-unsafe",
        what: "patch changed DLLs or kernel functions into the resumed machine. Never counted as a pass",
      },
    ],
  },
];

export type InsideTool = { name: string; what: string };

/** Programs that come with gala and run on it. */
export const insideTools: InsideTool[] = [
  {
    name: "gala",
    what: "the system's command line: start, list, snap and close windows, set the wallpaper, list and end processes. It reaches everything the desktop does.",
  },
  {
    name: "gala install",
    what: "installs software from winget's manifests with the vendor's own installer (zip, NSIS, Inno Setup), and upgrades and uninstalls it.",
  },
  {
    name: "taskmgr.exe, explorer.exe",
    what: "the task manager and the file explorer, for Windows and Linux alike.",
  },
  {
    name: "cmd.exe, conhost.exe, wsl.exe",
    what: "the command prompt, the console host behind every console window and SSH session, and the door into the Linux distributions.",
  },
];

/** Tools that live beside the code rather than in xtask. */
export const repoTools: InsideTool[] = [
  {
    name: "gala-um",
    what: "the NT layer built as an ordinary Linux process. Windows test programs run in milliseconds, under gdb, perf and sanitizers, with no virtual machine.",
  },
  {
    name: "Git hooks",
    what: "cargo xtask lint before every commit, and CI's Clippy checks before every push, so a problem is caught before it reaches anyone else.",
  },
  {
    name: "tools/",
    what: "Python helpers: the import survey (what a program needs that gala lacks), the Office survey, the Linux roots built from Ubuntu's packages.",
  },
  {
    name: "run-all.ps1",
    what: "runs every test program on a real Windows 11 machine and writes the report that cargo xtask compare reads.",
  },
  {
    name: "Docker image",
    what: "the whole suite in a container, for machines that are not Linux.",
  },
  {
    name: "CI",
    what: "every pull request boots every scenario in QEMU and runs the test programs on a Windows runner.",
  },
];

// ---------------------------------------------------------------------------
// Principles

export type Decision = { title: string; body: string };

export const decisions: Decision[] = [
  {
    title: "Clean room, without exceptions",
    body: "Nothing comes from disassembly, decompiled code or leaked source. The sources are public documentation, the Windows Internals books, mingw-w64's headers, Wine and ReactOS, and how real Windows behaves when tested from the outside.",
  },
  {
    title: "Compatible at the ABI and nowhere else",
    body: "Programs see Windows. Under that boundary, memory, scheduling, I/O and drivers are gala's own, so they can be redesigned and tuned.",
  },
  {
    title: "Tests first, against real Windows",
    body: "A piece of work starts as a Windows program whose expected output is Windows 11's. It is seen failing on gala first, and the implementation follows.",
  },
  {
    title: "The kernel survives running out of memory",
    body: "Every allocation in the kernel can fail. The APIs that would panic instead are banned by lint.",
  },
  {
    title: "unsafe confined to a short list",
    body: "Only the architecture layer, boot, the heap and the context switch may use unsafe Rust, and every block states why it is sound.",
  },
  {
    title: "Live update designed in",
    body: "State held as versioned data, system calls that can restart, drivers that survive a reset, one clock. These keep a future update without a reboot possible.",
  },
  {
    title: "A failure explains itself",
    body: "Every change ships with the logs, events and saved state that would explain its own failure.",
  },
  {
    title: "Root causes, not wider timeouts",
    body: "A failure is fixed where it arises. A stopgap is named as one, tracked and replaced.",
  },
];

// ---------------------------------------------------------------------------
// Screenshots

export type Shot = { src: string; title: string; caption: string };

export const shots: Shot[] = [
  {
    src: "/shots/start-menu.webp",
    title: "The start menu",
    caption: "Pinned apps install on first click; Linux distributions sit beside them.",
  },
  {
    src: "/shots/gimp.webp",
    title: "GIMP",
    caption: "A Linux program, through gala's Wayland server, in a desktop window.",
  },
  {
    src: "/shots/inkscape.webp",
    title: "Inkscape",
    caption: "Ubuntu's Inkscape, drawing an SVG on its canvas.",
  },
  {
    src: "/shots/taskmgr-processes.webp",
    title: "Task manager",
    caption: "Windows apps, background processes and a Linux distribution's processes in one list.",
  },
  {
    src: "/shots/taskmgr-performance.webp",
    title: "Performance",
    caption: "Both processors, memory, disk and network.",
  },
  {
    src: "/shots/explorer.webp",
    title: "File explorer",
    caption: "Browsing a Linux distribution, with the file's Linux permissions.",
  },
  {
    src: "/shots/xterm.webp",
    title: "Xwayland",
    caption: "An X11 xterm, with gala as its window manager.",
  },
  {
    src: "/shots/boot.webp",
    title: "Booting",
    caption: "قلعة: gala's boot screen.",
  },
];

// ---------------------------------------------------------------------------
// Downloads: the nightly images (.github/workflows/images.yml, WORKFLOW
// "Downloadable images"). Each file has `.sha256` and `.commit` beside it.

export const imagesUrl = "https://updates.gala-os.com/images/latest";

export type Image = {
  arch: string;
  file: string;
  size: string;
  what: string;
  /** The command that boots it, checked against the published image. */
  qemu: string;
  /** What the command needs besides QEMU. */
  needs: string;
};

export const images: Image[] = [
  {
    arch: "x86_64",
    file: "gala-x86_64.iso",
    size: "about 55 MB",
    what: "For PCs and x86_64 virtual machines. A hybrid image: it boots by BIOS or UEFI, from a CD drive or written to a USB stick.",
    qemu:
      "qemu-system-x86_64 -cpu max -m 1G -smp 2 -serial stdio \\\n" +
      "  -cdrom gala-x86_64.iso",
    needs: "QEMU (the qemu-system-x86 package, or qemu from Homebrew).",
  },
  {
    arch: "aarch64",
    file: "gala-aarch64.iso",
    size: "about 22 MB",
    what: "For QEMU's virt machine on ARM64, booted by UEFI. Real ARM64 hardware is not supported yet.",
    qemu:
      "qemu-system-aarch64 -machine virt,gic-version=3 -cpu max -m 1G -smp 2 -serial stdio \\\n" +
      "  -bios QEMU_EFI.fd \\\n" +
      "  -device virtio-gpu-pci -device virtio-keyboard-pci -device virtio-mouse-pci \\\n" +
      "  -device virtio-scsi-pci -device scsi-cd,drive=cd \\\n" +
      "  -drive if=none,id=cd,media=cdrom,file=gala-aarch64.iso",
    needs:
      "QEMU and its ARM64 UEFI firmware: QEMU_EFI.fd from the qemu-efi-aarch64 package (in /usr/share/qemu-efi-aarch64/), or edk2-aarch64-code.fd, which comes with QEMU (Homebrew: $(brew --prefix)/share/qemu/).",
  },
];

/** What a downloaded image does when it boots (xtask DEMO_CMDLINE*). */
export const imageDemo =
  "Each image is a developer build of main's latest commit that passed every test, rebuilt nightly. It boots the kernel, assembles C:, and runs a few demonstration programs: a minimal Windows program on both machines, and on x86_64 also a Go program and one that starts a thousand goroutines across both processors. Their output and the kernel's log go to the serial console, which the command's serial option shows in your terminal: expect lines such as “Hello from user mode!” and “gala: boot ok”. Then QEMU's window shows gala's desktop: the taskbar, the clock, and the start menu, which the Windows key or a click on the start button opens. Close QEMU's window, or press Ctrl+C in the terminal, to stop it.";

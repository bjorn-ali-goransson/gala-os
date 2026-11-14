// The site's content. Every architecture entry below is a section of
// docs/ARCHITECTURE.md; every program is a row of the README's "What runs
// on gala", where the scenario names are the boot scenarios that check it.

export type Layer = "core" | "nt" | "shell" | "linux" | "project";
export type Status = "works" | "building" | "planned";

export const layers: Record<Layer, { name: string; blurb: string }> = {
  core: {
    name: "Core",
    blurb:
      "gala's own design, owing nothing to NT: memory, time, threads, drivers, the network stack.",
  },
  nt: {
    name: "NT personality",
    blurb:
      "The Windows user-mode ABI, as programs observe it, translated onto the core.",
  },
  shell: {
    name: "shasha and tools",
    blurb:
      "The desktop, its programs, and everything the gala command can do instead.",
  },
  linux: {
    name: "Linux personality",
    blurb: "A second system call layer on the same core, for Linux binaries.",
  },
  project: {
    name: "Across the project",
    blurb: "The rules and seams that every subsystem is held to.",
  },
};

export type Entry = {
  title: string;
  layer: Layer;
  status: Status;
  summary: string;
};

export const architecture: Entry[] = [
  {
    title: "Physical memory",
    layer: "core",
    status: "works",
    summary:
      "A frame allocator keeping one bit per 4 KiB frame, built from the bootloader's memory map.",
  },
  {
    title: "Virtual memory",
    layer: "core",
    status: "works",
    summary:
      "Address spaces with VADs, demand-zero paging, guard pages and stack growth, copy-on-write.",
  },
  {
    title: "Heap",
    layer: "core",
    status: "works",
    summary:
      "The global allocator. Every allocation is fallible: the kernel never panics because memory ran out.",
  },
  {
    title: "Interrupts and time",
    layer: "core",
    status: "works",
    summary:
      "One clock for the whole system. The TSC is calibrated against the HPET or the ACPI timer and checked against the RTC.",
  },
  {
    title: "Threads and scheduling",
    layer: "core",
    status: "works",
    summary:
      "Kernel stacks, context switching and the scheduler, with thread priorities and affinity as Windows programs set them.",
  },
  {
    title: "Processors",
    layer: "core",
    status: "works",
    summary:
      "All CPUs schedule threads. Application processors are brought up by the kernel, including ones the bootloader missed, with TLB shootdowns between them.",
  },
  {
    title: "Networking",
    layer: "core",
    status: "works",
    summary:
      "Bottom up: PCI, a virtio-net driver, a TCP/IP stack, the AFD driver, then Winsock on top. TCP and UDP, overlapped sockets, completion ports.",
  },
  {
    title: "Disk",
    layer: "core",
    status: "works",
    summary:
      "A virtio-blk driver. With persist, C: and the registry live on the disk as a log replayed at the next boot.",
  },
  {
    title: "Boot",
    layer: "core",
    status: "works",
    summary:
      "Limine on hybrid images that start under both UEFI and BIOS, through to the first thread.",
  },
  {
    title: "CPU tables",
    layer: "core",
    status: "works",
    summary:
      "The GDT uses the x64 Windows selector values, because user mode is allowed to read them and programs do.",
  },
  {
    title: "The architecture seam",
    layer: "core",
    status: "works",
    summary:
      "Everything processor-specific sits behind one boundary, so a second architecture is a port and not a rewrite.",
  },
  {
    title: "NT personality",
    layer: "nt",
    status: "works",
    summary:
      "System call entry, the PE loader, TEB, PEB and KUSER_SHARED_DATA, and handle and object semantics as programs observe them.",
  },
  {
    title: "gala's system DLLs",
    layer: "nt",
    status: "works",
    summary:
      "Clean-room ntdll, kernel32, msvcrt, ws2_32 and about fifty more, so programs load the libraries they shipped against.",
  },
  {
    title: "Registry",
    layer: "nt",
    status: "works",
    summary:
      "The configuration manager: keys, values, information classes and access checks, seeded at boot and kept on disk with persist.",
  },
  {
    title: "The service control manager",
    layer: "nt",
    status: "works",
    summary:
      "Services start at boot, are restarted when they die, and answer the control calls their installers make.",
  },
  {
    title: "Schannel (TLS)",
    layer: "nt",
    status: "works",
    summary:
      "SSPI's TLS package, client and server, TLS 1.2 and 1.3 — what nginx, FileZilla and winhttp reach for.",
  },
  {
    title: "Security",
    layer: "nt",
    status: "works",
    summary:
      "Tokens, security descriptors and access checks on files, keys and objects; LSA logons, user profiles, restricted tokens, integrity levels.",
  },
  {
    title: "Diagnostics of stuck and crashed programs",
    layer: "nt",
    status: "works",
    summary:
      "What a failure leaves behind: crash reports, stack walks and state dumps, so a hang explains itself.",
  },
  {
    title: "Windows on the screen",
    layer: "shell",
    status: "works",
    summary:
      "shasha, gala's graphical shell: user32 and gdi32 windows, controls, common dialogs, a taskbar and start menu, on a framebuffer or a virtio-gpu.",
  },
  {
    title: "UI Automation",
    layer: "shell",
    status: "works",
    summary:
      "Programs are driven the way testers drive them on Windows — find an element, invoke it — which is how the screen tests work.",
  },
  {
    title: "Installing software",
    layer: "shell",
    status: "building",
    summary:
      "gala install reads winget's manifests and runs the vendor's own installer, the way winget does.",
  },
  {
    title: "The task manager",
    layer: "shell",
    status: "building",
    summary:
      "One task manager on the desktop for every process gala runs, Windows programs and Linux ones alike.",
  },
  {
    title: "The file explorer",
    layer: "shell",
    status: "building",
    summary:
      "An Explorer over every file system gala holds, with the shell interfaces that programs call into it for.",
  },
  {
    title: "A web browser: Chromium, then Edge",
    layer: "shell",
    status: "building",
    summary:
      "An unmodified Chromium: its window, input and accessibility tree run, and ChromeDriver drives it.",
  },
  {
    title: "Sessions and remote shasha",
    layer: "shell",
    status: "planned",
    summary:
      "One design covering remote shells and a remote graphical desktop, over the sessions that already separate services from the console.",
  },
  {
    title: "Graphics and GPU",
    layer: "shell",
    status: "planned",
    summary:
      "Direct3D over Vulkan, for the browser first and then the Office applications' rendering.",
  },
  {
    title: "A second personality (Linux)",
    layer: "linux",
    status: "building",
    summary:
      "Linux x86_64 binaries on the same core through their own system call layer. busybox's shell, its tools, its httpd and wget run today.",
  },
  {
    title: "Linux applications on the desktop (Wayland)",
    layer: "linux",
    status: "planned",
    summary:
      "Graphical Linux programs on shasha, in windows beside the Windows ones, with shasha as the Wayland compositor.",
  },
  {
    title: "Linux containers through a WSL facade",
    layer: "linux",
    status: "planned",
    summary:
      "Docker Desktop and Podman for Windows run their Linux side against gala's own WSL interface.",
  },
  {
    title: "WOW64 (32-bit x86 programs)",
    layer: "nt",
    status: "planned",
    summary:
      "32-bit programs on gala as on 64-bit Windows, through the same thunking layer Windows gives them.",
  },
  {
    title: "Safety boundary",
    layer: "project",
    status: "works",
    summary:
      "The kernel denies unsafe code outright; the few modules allowed it are listed, and every block carries a proof of why it is sound.",
  },
  {
    title: "Built in early: invariants, safety nets, live update",
    layer: "project",
    status: "works",
    summary:
      "The properties that are cheap now and ruinous later: state as versioned data, restartable system calls, drivers that survive a reset, one clock.",
  },
  {
    title: "Testing",
    layer: "project",
    status: "works",
    summary:
      "Host unit tests for what needs no hardware, and boot scenarios that start gala in QEMU and compare its output against real Windows'.",
  },
  {
    title: "Kernel command line",
    layer: "project",
    status: "works",
    summary:
      "The flags that shape a boot: persist, profile=server, the test harness's own switches.",
  },
  {
    title: "Known limits",
    layer: "project",
    status: "works",
    summary:
      "The gaps that have no subsystem of their own, written down where they can be read rather than discovered.",
  },
];

export type Program = {
  name: string;
  version?: string;
  what: string;
  scenarios: string[];
};

export const programs: Program[] = [
  {
    name: "Chromium",
    what: "an unmodified snapshot: its window, input and accessibility tree, driven by ChromeDriver",
    scenarios: ["chromium_window", "chromium_uia", "chromium_driver"],
  },
  {
    name: "Java",
    version: "Temurin JDK 21",
    what: "the HotSpot server VM starts; java Hello.java compiles in memory with the JDK's own compiler and runs it; com.sun.net.httpserver serves the host",
    scenarios: ["java_version", "java_hello", "java_http"],
  },
  {
    name: ".NET 8",
    what: "a C# program on the JIT and the garbage collector: exceptions, threads, formatting",
    scenarios: ["dotnet_info", "dotnet_hello"],
  },
  {
    name: "PostgreSQL",
    version: "17.2",
    what: "initdb makes a cluster, postgres serves it with a backend per connection, and the host's psql queries it",
    scenarios: ["postgres"],
  },
  {
    name: "Win32-OpenSSH",
    version: "9.8",
    what: "the host logs in with a key or a password; an interactive shell over a pseudo console with line editing and Ctrl+C; sftp upload and download",
    scenarios: ["sshd_login", "sshd_shell", "sshd_sftp", "service_restart"],
  },
  {
    name: "Nginx",
    version: "1.30.6",
    what: "serves its start page from a master and worker processes, and over HTTPS with OpenSSL 3.5",
    scenarios: ["nginx", "nginx_master", "nginx_tls"],
  },
  {
    name: "TinyCC",
    what: "compiles C on gala, builds its own runtime library, then compiles itself into a second compiler that works the same",
    scenarios: ["tcc", "tcc_self"],
  },
  {
    name: "SQLite",
    version: "3.54.1",
    what: "the 9.5 MB amalgamation compiled on gala by tcc, then two processes writing one database at once through file locks",
    scenarios: ["sqlite"],
  },
  {
    name: "Go",
    version: "1.24",
    what: "1000 goroutines over channels and mutexes, a 200 MB garbage collection, child processes through pipes, and net/http serving the host",
    scenarios: ["go_goroutines", "go_gc", "go_exec", "go_http"],
  },
  {
    name: "PowerShell",
    version: "7.4",
    what: "pwsh runs a command and prints its version table",
    scenarios: ["pwsh_hello"],
  },
  {
    name: "FileZilla Server",
    version: "1.12.6",
    what: "generates its TLS certificates and serves FTP; the host logs on",
    scenarios: ["filezilla_server"],
  },
  {
    name: "busybox-w32",
    what: "shell pipelines and scripts, file tools, background jobs, unzip, and make building a multi-file C project with tcc",
    scenarios: ["busybox", "busybox_script", "busybox_files", "tcc_make"],
  },
  {
    name: "Lua",
    version: "5.4.7",
    what: "the unmodified source release built on gala by its own makefile, 37 compiler runs, then a test script",
    scenarios: ["lua"],
  },
  {
    name: "Caddy",
    version: "2.8.4",
    what: "lists its 121 modules and serves a site to the host",
    scenarios: ["caddy", "caddy_http"],
  },
  {
    name: "FlaUI",
    version: "5.0",
    what: "drives another program's window through gala's UI Automation: the element tree, Invoke, Value, Toggle and Window patterns",
    scenarios: ["flaui_test"],
  },
];

export type Decision = { title: string; body: string };

export const decisions: Decision[] = [
  {
    title: "Clean room, with no exceptions",
    body: "Nothing is written from disassembly, decompiled code or leaked source. The implementation comes from public documentation, the Windows Internals books, mingw-w64 headers, Wine and ReactOS, Microsoft's published WSL source, and the behaviour of real Windows observed from the outside. The rules are written down and every contribution is held to them.",
  },
  {
    title: "Compatibility at the ABI, and nowhere else",
    body: "Programs see Windows. Underneath, the memory manager, scheduler, I/O paths and drivers are gala's own design, free to be re-architected and tuned. NT's internal design is never copied, and that is what keeps the core open to being changed.",
  },
  {
    title: "Every feature starts as a test against real Windows",
    body: "A piece of work begins with a freestanding Windows .exe whose expected output is what Windows 11 24H2 produces. The same binary runs on Windows and on gala and the results are compared. The test is written first and seen failing, and the implementation follows it.",
  },
  {
    title: "Allocation can fail, and the kernel survives it",
    body: "There is no allocation in the kernel that panics when memory runs out. The infallible APIs are banned by lint rather than by convention, so the fallible ones are the only ones in reach.",
  },
  {
    title: "unsafe is confined to a list you can read",
    body: "The kernel denies unsafe code by default. A handful of modules are allowed it — the architecture layer, boot, the heap, the context switch — and every block in them states why it is sound.",
  },
  {
    title: "Live update designed in from the first commit",
    body: "A running gala is meant to replace its system image and its kernel without a reboot, with programs still running. Rather than retrofit that, every change keeps the properties it needs: state held as versioned data, system calls that can be restarted, drivers that survive a device reset, one clock. A change that would make the handover harder has to argue for itself.",
  },
  {
    title: "One pinned Windows build",
    body: "System call numbers and structure layouts come from a single Windows version, 11 24H2 build 26100, written in one place in the kernel and one place in the test runtime. Compatibility is a moving target only if you let it move.",
  },
  {
    title: "The command line reaches everything the desktop does",
    body: "Each desktop feature is built first as something the gala command can do, and then given a mouse and keyboard binding. A server never starts a graphical interface at all, and the same kernel serves both.",
  },
  {
    title: "A failure has to explain itself",
    body: "Every change ships with whatever would explain its own failures: the logs, the events, and the state a crash leaves behind. A gap that a failure exposes is filled with the fix, not after it.",
  },
  {
    title: "Root causes, not wider timeouts",
    body: "A failure is fixed where it arises. No widened timeout, retry or exclusion stands in for understanding the cause; when a stopgap is genuinely unavoidable it is named as one, tracked, and replaced.",
  },
];

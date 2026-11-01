# Working on gala-os (handbook for agents and humans)

The fastest way to get productive in a new session. Read `CLAUDE.md`, then
this, then the parts of `ARCHITECTURE.md` you touch. `ROADMAP.md` says what
is done and what comes next (the "Next" list is the agreed order).

## One chunk of work

1. **Test first.** Create `user/<feature>_test/` (copy `Cargo.toml` from
   another test with `sed 's/file_test/<feature>_test/g'`). Its `main`
   returns 0 or the number of the failing check and prints
   `<feature>_test: ok`. Expected values are what real Windows (pinned
   build) returns; say in the doc comment what is assumed.
2. Wire it up:
   - `xtask/src/main.rs`: add to `USER_PROGRAMS`, add a `Scenario`
     (`cmdline: "qemu_exit run=<feature>_test.exe [args=...]"`), and, if it
     needs the test ntdll or DLLs, `EXTRA_MODULES` entries
     (`("x_test", "ntdll.dll", "Windows/System32")`).
   - Root `Cargo.toml` workspace members.
   - `.github/workflows/ci.yml`: append `-p <feature>_test` to the
     Windows-target clippy line; test DLLs go on the DLL build/clippy lines.
   - `user/bin/run-all.ps1` only if the program needs arguments on Windows.
3. `cargo xtask boot-test <feature>_test`: see it fail for the right reason.
4. Implement. Host unit tests for pure logic (`#[cfg(test)]` in the module).
5. Checks (all must pass):
   ```sh
   cargo fmt --all
   # every clippy/build line CI runs (selected by content, not line number)
   grep -E '^ +cargo (clippy|build --release)' .github/workflows/ci.yml | sed 's/^ *//' | bash -e
   cargo xtask test            # host tests + every boot scenario
   cargo xtask export-tests    # refresh user/bin (CI compares)
   ```
6. Update `docs/ROADMAP.md` (move the item to done, with the test name) and
   `docs/ARCHITECTURE.md` (how it works, invariants, TODOs). Commit with a
   message saying what the test checks and what changed; push.

## Go programs

`user/<name>/` with a `main.go` and `go.mod`, listed in `GO_PROGRAMS` in
xtask: built for windows/amd64 with the pinned toolchain (`GO_TOOLCHAIN`,
`-trimpath`, empty build id, so the binary is reproducible and checked in
under `user/bin`). Without `go` installed, xtask uses the checked-in
binary. Bring-up loop: run it (`XTASK_EVENTS=all cargo xtask boot-test
<name>` once it has a scenario, or `cargo xtask screenshot run=<name>.exe`)
and read the `ledger:` lines: every missing import is listed by name.

## Nginx

`user/nginx/build.sh` cross-compiles Nginx (pinned version and SHA-256)
with mingw-w64 (`apt install gcc-mingw-w64-x86-64`) into
`user/bin/nginx.exe`; the build is reproducible (fixed build directory,
no debug info, no link timestamp: two builds give the same hash). CI
uses the checked-in binary. Its configuration, page and placeholder
files (boot modules only create the directories they are in) are in
`user/nginx/` and shipped by EXTRA_MODULES entries with repository
paths. It runs with `-p C:/nginx/ -c conf/nginx.conf`. A server never
exits: scenarios with exit status `XTASK_STOPS` are stopped by xtask
once their net client has finished, and pass if it succeeded.
`cargo xtask screenshot run=nginx.exe "args=-p%20C:/nginx/%20-c%20conf/nginx.conf"`
shows it.

## Adding a function to gala's ntdll or kernel32

1. A test program that imports it (`raw-dylib` against `kernel32` or
   `ntdll`, as `kernel32_test` does), with the results real Windows gives,
   last-error codes included. Its scenario lists the DLLs in
   `EXTRA_MODULES` (`ntdll.dll` and `kernel32.dll` to
   `Windows/System32`).
2. In `user/kernel32/src/lib.rs`: `#[unsafe(no_mangle)] pub extern
   "system" fn Name(...)` with the documented signature. Call ntdll's
   exports (add them to the `raw-dylib` block); set the last error through
   `fail(status, value)` / `fail_with(error, value)`; report unsupported
   cases with `unsupported(b"kernel32.dll!Name(case)", detail)`.
3. A new system call for it goes into testntdll's `syscall_stubs!` (and
   the kernel's table), with a stable x64 number or gala's own (see
   "System call numbers" below).
4. A new NTSTATUS the kernel returns needs a row in testntdll's
   `STATUS_TO_ERROR` (unmapped ones give 317 and are reported).
5. Run the program you are bringing up and read the `ledger:` lines: they
   list every missing import by name.

## Things that are easy to get wrong

- **A JVM crashing with access violations at odd addresses** after a
  long run may be the machine out of memory: a user page fault that finds
  no free frame becomes an access violation (no commit charge yet). Look
  for "vm: out of memory" in the log and the free frames on the `stats:`
  line; give the scenario more memory (xtask's `qemu`).
- **Disk space**: ISOs of the runtime scenarios are 120-470 MB each, and
  their `*-iso-root` copies as much again; `target/debug/*.iso`,
  `*-iso-root` and `*.events.jsonl` are rebuilt on every run and can be
  deleted when xorriso reports "exceeds free space".
- **PREBUILT members** are unzip patterns (`*` crosses `/`), `excludes`
  are unzip `-x` patterns; the unpack marker includes a hash of both, so
  changing them unpacks again. For an MSI (a `.msi` URL) members are
  directories of the installed layout, each moved to
  `target/prebuilt/<name>/<last component>`, and excludes are paths
  removed after that; unpacking needs msitools (`apt install msitools`).
- **A program hangs without a ledger** (it never exits, so the ledger is
  not printed): add `events=syscall,thread` to its scenario's command
  line and read `target/debug/test-<name>.events.jsonl` while QEMU still
  runs (`grep -v` the calls it repeats; a loop of time queries means it
  waits for a clock to move). Waiting services also log a `"ev":"wait"`
  event when they start, so a wait that never returns shows as the
  thread's last line. Do not delete the events file of a running
  VM (it is still open: `/proc/<qemu pid>/fd/` reaches it).
- **Tracing exception dispatch in a third-party program**: a temporary
  `GalaRecordMissing` call in gala's ntdll dispatcher (`seh.rs`: code and
  address in the detail; one per walked frame with its pc and handler)
  shows up in the `events=syscall` stream, in order and per thread; map
  addresses to modules with `events=image` (base and size per process).
  Use probe names of distinct lengths (the event has the name's pointer
  and length, not its text). Revert before committing.
- **Tests and access checks** (since S3): a test that opens objects must
  ask for exactly what it may have on Windows. kernel32's CreateFileW adds
  SYNCHRONIZE for synchronous handles (open overlapped to avoid it), the
  owner of an object only gets READ_CONTROL and WRITE_DAC implicitly,
  deleting a key needs DELETE on it, and FILE_GENERIC_WRITE includes
  SYNCHRONIZE and READ_CONTROL (a deny ACE with FW denies reading too).
  To find a denial: gala-um's `--trace` (or `events=syscall`) shows the
  call that returned 0xc0000022.
- **A DLL forwarding to another DLL's function of the same name**
  (advapi32's CreateProcessAsUserW to kernel32's): a raw-dylib import of
  that name binds to the DLL's own export, so the call jumps to itself
  for ever (no stack growth: a tail call). Look the target up with
  GetProcAddress instead.
- **Win32-OpenSSH's diagnostics**: as SYSTEM (and with `-d`) sshd and
  ssh-keygen log to the system log (ETW), not the console: pass `-e` to
  sshd for stderr. Its source (github.com/PowerShell/openssh-portable,
  BSD-licensed; not Windows source) says what it expects of an API, for
  example `contrib/win32/win32compat/pwd.c` (accounts must be
  SidTypeUser; home from ProfileList).
- **Reading MSVC C++ EH data** (as data, from .pdata/.xdata): check the
  format before decoding. A FuncInfo starting with the magic number
  0x19930520-0x19930522 (bytes `22 05 93 19`) is the older format
  (__CxxFrameHandler3: try blocks, unwind map, an ip-to-state map with
  absolute RVAs); FH4's FuncInfo4 starts with a one-byte header. Reading
  FH3 data as FH4 gives plausible-looking nonsense (it misled the
  SHUTDOWN investigation for a while). The handler data of a
  GS-checking handler is the FuncInfo RVA, then the GS data.
- **C++ exception tests**: `user/cxx_eh_test` is C++ built with clang
  for x86_64-pc-windows-msvc and lld-link (`build.sh`, import libraries
  from .def files with llvm-dlltool), no CRT, against Microsoft's
  vcruntime140 (the JDK's, app-local); the binary is checked in (rebuild
  with the script; the build is reproducible).
- **A scenario needs a server and a client at once**: `run=` starts
  programs one after another, so the client starts the server itself
  (CreateProcessW) and waits for it, as `tds_test` does with sqlservr.
- **A program fails without a ledger entry or a failing system call**
  (a function returned data it did not like): add temporary probes,
  `missing_detail(b"TRACE dll!Function", detail)` at the top of the
  suspect DLL functions (name, class or argument as the detail, or the
  argument text in the name), run, read the `TRACE` ledger lines, and
  revert them before committing. That is how sqlservr's unresolved
  `NT SERVICE\SQLWriter` SID was found.
- **A program imports functions gala lacks**: a missing export stops the
  load (as on Windows), so add it to the DLL's `src/unimplemented.rs`
  (`gala_stubs::unimplemented_exports!`) to get the program running; the
  stub reports `dll!function` ("not implemented") and ends the process
  with STATUS_NOT_IMPLEMENTED when called, so the ledger names the next
  function to implement. Implementing one means removing it from that
  list. List a program's missing imports by comparing its import tables
  (`llvm-readobj --coff-imports`, through its own DLLs) with the
  `--coff-exports` of `user/bin/test*.dll`, ordinals included (an export
  with RVA 0 is an empty ordinal slot, not an export).

- **Predefined registry keys are sign-extended on x64**: winreg.h
  defines HKEY_LOCAL_MACHINE as `(HKEY)(ULONG_PTR)((LONG)0x80000002)`,
  0xFFFFFFFF80000002; C programs and .NET pass that form, Rust test
  programs usually the zero-extended one. Accept both (advapi32's
  `predefined`). PowerShell's 23 registry reads all failed with
  ERROR_INVALID_HANDLE until this was handled.
- **Registry keys a program needs**: boot it and read the ledger's
  `"what":"absent key"` lines (`registry:MACHINE/...`, the end of long
  names). Keys a clean Windows has go in `kernel/src/nt/registry.seed`
  with Windows' values; keys a clean Windows lacks too (policies, other
  products) stay absent.

- **Functions that must start from their caller's registers** (anything
  that captures a context or walks frames: RtlCaptureContext,
  RtlRaiseException, RtlUnwindEx, the dispatchers) are `global_asm!` with
  `.seh_*` directives in ntdll's `seh.rs`, not naked functions: naked
  functions get no unwind data, and a walk cannot pass through them.
  Assembly symbols are exported through `/EXPORT` in testntdll's
  build.rs. `llvm-readobj --unwind` shows what the assembler emitted.
- **The exception dispatcher runs on what is left of the stack** (a stack
  overflow is delivered with only the guard region below rsp): keep the
  vectored-handler path's frames small and put CONTEXT copies in
  `#[inline(never)]` helpers (`guard_test` catches this).

- **System call numbers.** Long-stable x64 numbers are used directly (list
  in `kernel/src/nt/syscalls.rs`). Numbers that change between builds are
  *gala's own choices*, unverified: tests must call those services through
  ntdll exports (`find_ntdll()` + `find_export()` in ntrt), and
  `user/testntdll` provides stubs with gala's numbers (`syscall_stubs!`).
  On Windows the same test then uses the real ntdll.
- **testntdll is gala's stand-in ntdll**, shipped only in scenarios that
  list it in `EXTRA_MODULES`. It contains a small loader (LdrLoadDll etc.),
  exception/APC dispatchers and the stubs. Adding an export there changes
  `user/bin/testntdll.dll`.
- **Fallible allocation only** (`try_box`, `try_push`, `try_reserve`);
  clippy bans the rest. `unsafe` only where `CLAUDE.md` allows it.
- **Multiprocessor.** All CPUs schedule threads (S2), and any thread,
  the boot thread included, can move between CPUs at any reschedule.
  Never rely on "interrupts off" as a lock; use `sync::IrqMutex`. Per-CPU
  data lives behind GS (`arch::percpu`); per-CPU hardware setup belongs in
  each CPU's own init path (`ap_main` / early `kernel_main`), never in code
  run later by a thread. Per-thread state goes in `sched::Thread` or
  `NtThread`, not in a global "current request" slot. Single-CPU tests
  that measure ticks must pin themselves (`sched::set_affinity`).
- **Changing user mappings** goes through `process.space.lock()` (a
  `SpaceGuard`): the guard's drop does the TLB shootdown and frees the
  unmapped frames. Do not free user frames any other way while the space
  may be live. Cross-CPU memory tests need the victim thread pinned to
  another CPU and nothing else flushing meanwhile (let reaping settle
  first, as `smpmem_test` does), or the bug they look for is masked.
- **Commit messages** are passed through the shell: do not put backticks in
  a double-quoted `-m` (use a heredoc `-F -`).
- **Never run two xtask boot runs at once** (e.g. a full `cargo xtask test`
  and a single `boot-test`): they share ISO and serial-log files under
  `target/` and fail each other.
- **Unsupported paths**: when you leave a flag, class or case
  unimplemented, call `nt::ledger::record(service, what, detail)` there, so
  real programs that need it show up in `target/report/ledger.md`.
- **Fresh checkouts**: CI starts without `target/`; things fetched or
  built on first use (Limine) must also work when several threads need
  them at once.
- **Network tests**: VMs have no network card unless the scenario is in
  xtask's `NET_SCENARIOS` (`-nic none` otherwise). Those get a virtio-net
  card on QEMU's user network with a host port forwarded to guest port 7,
  and xtask plays the remote side (`net_client`) once the guest logs that
  it listens; the client's verdict is appended to the serial log as an
  `xtask: net client: ...` line for `expect` to check. QEMU's user network
  needs no privileges, so this works in containers and CI.
- **Disk tests**: VMs have no disk unless the scenario is in xtask's
  `DISK_SCENARIOS` (name, size, what xtask writes into the fresh raw
  image, what the image must hold afterwards). The image is
  `target/.../<scenario>.disk.img` next to the ISO, attached as a legacy
  virtio-blk device; such scenarios boot alone and not in gala-um.
  Raw disk access needs an administrator, so their programs run
  `as=system`. `earlier_boots` boots the same image first (each with the
  lines its log must have; logs kept as `<iso>.boot<n>.serial.log`):
  that is how a test checks what survives a reboot (`persist_test`, which
  tells the boots apart by what it finds on C:).
- **`persist`**: with this boot option C: is kept on the disk; without
  it a disk is left alone (D1's raw tests).
- **Constants from headers**: values typed in by hand go wrong
  silently (STATUS_OBJECT_NAME_EXISTS was 0x40000035, and a test agreed
  with it). With mingw-w64 installed (`apt install mingw-w64-common`, or
  the cross compiler), `cargo test -p xtask` compares every NTSTATUS in
  `kernel/src/nt/status.rs` with `ntstatus.h`; check other constants the
  same way when you add them.
- **Non-blocking state is inherited**: Windows gives an accepted socket
  the listener's non-blocking mode, and Nginx relies on it. Missing that
  made gala's recv block on the TLS handshake's second flight; a request
  that arrives in one piece hid it for plain HTTP. When a server hangs
  mid-conversation, `XTASK_EVENTS=syscall` shows which call pended.
- **The TLS client**: `nginx_tls` uses the host's `curl -k`; on failure
  the end of curl's `-v` trace is in the log (where the handshake or the
  request stopped).
- **Servers start late**: a program may log that it started before it
  listens (Nginx's master logs, then its worker opens the socket). The
  HTTP client retries for 10 s while it gets no response at all.
- **Boot module directories are real paths**: a program run from a boot
  module in `nginx/` has the image path `C:\nginx\nginx.exe`; the
  loader looks for DLLs next to it, then in System32.
- **32-bit system call arguments**: a ULONG (lengths, classes, codes,
  flags) passed on the stack, and sometimes in a register, has garbage in
  its upper half. Read it with `arg!(call, n) as u32`. go_http failed
  6 times out of 10 because NtCreateFile read EaLength as 64 bits
  (`0xc0_0000_003a`); the event stream (`XTASK_EVENTS=syscall`) showed it.
- **Handle reuse**: a handle value freed by NtClose is handed out again
  at once, so "use after close" tests may hit a new object of another
  type (ws2_32 maps "not a socket" device requests to WSAENOTSOCK).
- **Asynchronous Windows APIs in tests**: SuspendThread returns before a
  thread running on another CPU has stopped, on Windows as on gala. A test
  that needs it stopped calls GetThreadContext first (kernel32_thread_test
  was flaky until it did).
- **Known intermittent**: `java_http` once (2026-11-01) died with
  EXCEPTION_ILLEGAL_INSTRUCTION in C1-compiled code (HotSpot patching
  JIT code while another CPU runs it; cross-modifying code on 2 vCPUs,
  possibly TCG); it passed on rerun. If it recurs, record it with
  QEMU record/replay (OBSERVABILITY.md).
- **Flaky boot tests** usually mean a race; rerun the one scenario a few
  times (`cargo xtask boot-test name`) before blaming CI.
- **Read the source of the exact version** of an open-source program
  before deciding what it does: Win32-OpenSSH's `latestw_all` branch
  waits for sshd-session in `sshd -d`, the shipped 9.8.3 (tag
  `v9.8.3.0`) exits with 255 right after spawning it. Tags are listed by
  `git ls-remote --tags`; a sparse, blob-less clone of one directory is
  quick. A program that ends while its children work on needs the boot
  option `wait=all`.
- **Out of frames looks like a bad image**: when NtMapViewOfSection
  cannot get a frame, the loader reports STATUS_INVALID_IMAGE_FORMAT
  (0xC000007B) for the process. Check the free frames (`mm::frame_stats`)
  before suspecting the image; a scenario with many processes or big data
  may need the larger VM (`qemu()` in xtask).
- **A failed boot test keeps its log in `target/failed/`** until the next
  failure overwrites it: after a pass, that file is stale (the pass's own
  log is `target/debug/test-<name>.serial.log`).
- **A stub that ends its process** (gala_stubs' "not implemented") is
  logged at once (`[warn ] <program>: <dll>!<function> not implemented,
  ending the process`): the ledger is only dumped when the `run=`
  program ends, which for a service's children may be never.
- **Which handle is which** in an event stream (`XTASK_EVENTS=all`):
  handle values are reused at once (the lowest free slot), so follow
  NtClose and creation order rather than guessing from the value.

## Test speed and environments

`cargo xtask test` (host unit tests + every boot scenario) took 66 s for
the boot scenarios before T1 and 14 s after, under software emulation on a
4-CPU container (40 scenarios). Where the time went, and what changed:

| | before | after |
|---|---|---|
| VMs | 40 | 13 |
| firmware + bootloader (summed over VMs) | 193 s (OVMF) | 24 s (SeaBIOS) |
| kernel + tests (summed) | 38 s | 18 s |
| ISO creation | 5.7 s (sequential) | 0.5 s (parallel) |
| boot-test wall time | 66 s | 14 s |

- **BIOS boot by default.** ISOs are hybrid (BIOS and UEFI). SeaBIOS
  reaches the kernel in ~0.9 s against ~4.5 s for OVMF. `hello`,
  `selftest` and `fault-pagefault` still boot by UEFI so that path stays
  tested; `XTASK_FIRMWARE=uefi|bios` forces one for everything.
- **Shared VMs.** Scenarios that just run one program with no other
  flags, expect success, boot by BIOS and expect no process ids are
  grouped by identical boot modules and run in shared VMs
  (`run=a.exe,b.exe`). Each program is judged on its own section of the
  log (between `run: begin` and `run: end`, plus the boot preamble).
  `XTASK_NO_BATCH=1` gives every scenario its own VM; `cargo xtask
  boot-test <name>` with one match always boots it alone.
- xtask prints per-scenario (or per-VM) timing and a summary, also written
  to `target/report/timing.txt`.
- **Docker**: `cargo xtask docker-test [xtask args]` builds
  `docker/test.Dockerfile` (Ubuntu 24.04, QEMU, OVMF, xorriso, the pinned
  toolchain) and runs `cargo xtask test` (or the given arguments) inside
  it with the repository at /work and build output in `target/docker`.
  It passes `/dev/kvm` through when the host has it (then boots run at
  near-native speed) and forwards the `XTASK_*` variables. In a cloud
  container without a running daemon, start one with `dockerd &`.

## Benchmarks

Where time goes in a scenario: the `nt:` line at the end of its serial
log (`target/<profile>/test-<scenario>.serial.log`) gives the NT layer's
system call, fault and process creation time without disturbing the
run; the event stream gives per-call detail but slows the run down
(docs/PERF.md has the method and the H8 numbers). Under QEMU TCG,
user code that sits in shared physical frames is translated once:
copying code pages per process costs far more than the copy.

`cargo xtask bench` (always a release kernel) runs `user/bench` and writes
`target/report/bench.json`; on Windows, `user/bin/run-all.ps1` writes
`bench-windows.txt`, which `cargo xtask bench bench-windows.txt` puts next
to gala's numbers. For where time goes inside the kernel:
`XTASK_EVENTS=syscall cargo xtask boot-test bench`, then group the
`events.jsonl` lines by `name` and look at `ns`.

## Prebuilt programs

Unmodified releases (Caddy, the JDK) are not kept in the repository: the
`PREBUILT` table in xtask pins each one (URL, SHA-256, the files used
from its zip), and a scenario module path `target/prebuilt/<name>/<file>`
makes xtask download, check and unzip it on first use (curl, sha256sum,
unzip, or msiextract for an MSI; one fetch at a time, since ISOs are built in parallel, and a
`.unpacked-<sha256>` marker so an interrupted unpack is redone). To try a new prebuilt program before writing its scenario:
`cargo xtask iso --file path/to/app.exe run=app.exe "args=..."` and boot
it with QEMU (`-cpu max`), then follow the ledger and the program's own
error messages (Go programs print which DLL or procedure they could not
find). Survey an import table first with
`x86_64-w64-mingw32-objdump -p app.exe | grep "DLL Name"`; keep
downloaded files in their own directory and run them only inside QEMU.

Large programs (the JDK): a module path ending in `/`
(`target/prebuilt/jdk/jdk-21.0.5+11/`) ships the whole tree as
`C:\<dir>` with its subdirectories, packed into one USTAR archive
(`boot/<dir>.tree.tar`, module string `tar:<dir>`) that the kernel unpacks
into its ramfs in place (`kernel/src/fs/tar.rs`): Limine panics with
"Memory map exhausted" with a few hundred modules (the .NET runtime has
~190 files). `run=` finds a program inside an archive too. Scenarios
that ship the same large set share it through `MODULE_SETS` (and so a
VM). Managed (.NET) test programs are C# in `user/<name>/` built by its
`build.sh` with Mono's `mcs` (apt package `mono-mcs`) into `user/bin`
(reproducible: the script pins the MVID and timestamp); CI does not
rebuild them, as for the other third-party-tool builds. Whole-runtime scenarios
(`SLOW_SCENARIOS`: the JVM ones, dotnet_hello, pwsh_hello) get 300 s
instead of 120 s: a slow CI runner takes over twice as long as a local
run (java_hello: ~35 s locally; it timed out at 120 s once in CI, with
its threads still progressing - much of a JVM's time goes to gala's
lightweight-lock waits, which sleep 1 ms at a time until a kernel address
wait exists). In a shared VM,
scenarios running the same program (two `dotnet.exe` runs) are judged on
that program's n-th `run: begin`..`run: end` section, in group order.
To see why a .NET program fails: the ledger first, then `XTASK_EVENTS=
syscall,image,fault` (the stream lands in `target/debug/<iso>.events.jsonl`);
CoreCLR's own source (dotnet/runtime, MIT) says what it expects of the OS
(read only, as any application's documentation). QEMU gets 512 MiB
plus twice the image size when the image is over 64 MiB (boot modules stay
in memory). A scenario still being brought up goes in `PENDING`: `cargo
xtask boot-test <name>` runs it, `cargo xtask test` and CI do not. A DLL
the loader cannot find is in the ledger (`"what":"missing dll"`); a
missing function is a `missing export`. For a program with many DLLs,
walk the whole import closure (imports and delay imports of every DLL it
loads, following export forwarders such as the api-ms-win-* sets) against
the exports of gala's DLLs before booting, and fix the gaps in batches.
A crashing JVM writes `C:\hs_err_pid<N>.log` (registers, the faulting
instruction's neighbourhood, native frames); to read it, run busybox's
`cat` on it after java in the same boot (`run=java.exe,busybox.exe
args=-version,cat%20C:/hs_err_pid4.log`, busybox.exe in the scenario's
modules). An unhandled exception's code and address are in the ledger;
`XTASK_EVENTS=image` gives each image's base and size to map them.
A test program that needs a resource (a manifest: `manifest_test`) gets
it from its build.rs, which writes a .res file (the documented compiled
resource format) and passes it to the linker; lld-link converts it
itself (its /MANIFEST:EMBED would need Microsoft's mt.exe).
DLLs a program loads at run time (LoadLibrary: the JDK's launcher loads
msvcp140 and jvm.dll) are not in the closure; boot and read the ledger
for those. Exports by ordinal (ws2_32's) come from a build.rs printing
`/EXPORT:name,@n`; lld warns that rustc's .def names them too and keeps
the ordinal one (the warning cannot be silenced).

## Bootable images and USB sticks

`cargo xtask iso [--name N] [--scenario S] [--file PATH[=DIR]]... [cmdline...]`
writes `target/<profile>/gala-<N>.iso` with every program and DLL, a
scenario's files and command line (without `qemu_exit`), host files as
`C:\DIR\<name>`, or a demo (hello, go_hello, go_goroutines). The ISO is
hybrid, so `dd` to a USB stick boots on BIOS and UEFI (README.md has the
steps). Checked by hand in QEMU: `-drive file=...,format=raw` under
SeaBIOS and OVMF, and as `usb-storage` behind `qemu-xhci`. Use `-cpu max`
in manual QEMU runs: QEMU's default CPU model has no RDRAND, and Go
programs then fail fast (0xc0000409, ledger: ProcessPrng(no RDRAND)).

## Screenshots

`cargo xtask screenshot [--uefi] [--at 1,3,5] [--name N] [kernel cmdline...]`
boots an ISO (all test programs and DLLs as boot modules, ntdll in
System32) and writes `target/screenshots/<name>-<t>s.png` at each time,
through QEMU's monitor (`screendump`). The kernel's screen console shows
the same log as the serial port (green header bar; warnings yellow, errors
and panics red), so a screenshot shows what is going on. Example:

    cargo xtask screenshot --name demo --at 6 run=hello.exe,smp_test.exe args=,cpus=2

`--scenario S` adds S's files from the repository (scripts, archives):

    cargo xtask screenshot --name tcc --scenario tcc --at 16 run=busybox.exe "args=sh%20C:/gala/tcc.sh%20C:/gala/tcc.tar"

Boot tests pass `noscreen` (serial only) except in the UEFI scenarios:
drawing text roughly doubles kernel time under emulation.

## Serving to a browser

`cargo xtask serve <scenario> [port]` boots a network scenario (nginx,
nginx_master, nginx_tls, go_http, ...) as its boot test does, but forwards
a fixed host port (default 8080) to the guest's port 7 and keeps the VM
running, with the serial log on stdout. Then open `http://127.0.0.1:8080/`
(or `https://` for nginx_tls; the certificate is self-signed) in a browser.
For a picture, Playwright's Chromium is preinstalled in the cloud
container: `chromium.launch({args: ['--no-proxy-server']})`, because
the agent proxy would otherwise intercept 127.0.0.1.

## gala-um

`cargo xtask test` runs `um-test` right after the unit tests: every
program scenario that `runs_in_um` (xtask) runs as a Linux process with
the real NT layer (the kernel crate with `--features hosted`), checked
against the scenario's `expect` lines; ~50 programs in ~3 s. Run it
alone with `cargo xtask um-test` while changing `kernel/src/nt`: it is
the fastest check there is. A new test program runs in gala-um by
default; if it cannot (child processes, a network card), add it to
`NOT_IN_UM` with the reason.

Run a program directly:

    target/debug/um [--trace] [--cpus=2] \
        --module=Windows/System32/ntdll.dll=user/bin/testntdll.dll \
        user/bin/<name>.exe [arguments]

`--module` adds a file to C: as a boot module would (DLLs under their
`test*` names in user/bin), `--trace` logs every system call, `--cpus`
caps the processors (um-test uses 2, as QEMU). The kernel's log goes to
stderr, the program's output to stdout. When a program passes in QEMU
but not in gala-um, or the other way round, one of the two platforms is
wrong; `cargo xtask report` lists such programs. Program code must stay
below 0x5000_0000_0000 (above is the host's, where `syscall` is not
caught). Needs Linux 5.11+ (syscall user dispatch). `gdb -p` works
(`handle SIGSYS nostop noprint`, `handle SIGURG nostop noprint`), and
`thread apply all bt` shows where NT threads are inside the NT layer:
that is how the lock starvation and the GetContext bug were found.

## busybox

`user/busybox/build.sh` rebuilds `user/bin/busybox.exe` (pinned
busybox-w32 commit, reproducible; the build directory
`/tmp/gala-busybox-build` keeps `busybox_unstripped.exe.map`, which maps
a faulting rip to a function). The `busybox` scenario runs a sh command
line; to try others, change its `args=` (percent-encoded). With the
`trace` flag the kernel also logs every user fault (vector, rip, cr2),
which a system call trace cannot show: an unhandled one ends the process
with its status through ntdll's dispatcher.

The script scenarios (`busybox_files`, `busybox_script`) run checked-in
scripts from `user/busybox/` (boot modules in C:\gala); each prints one
labelled line per result, which is what the scenario expects. A builtin
or in-process applet (cd, mkdir, echo) can be tried in gala-um in
milliseconds (`um ... user/bin/busybox.exe 'sh -c "..."'`: pass the
command line pre-quoted, as one argument); anything that starts a child
needs QEMU. `user/msvcrt_test/` is a C test of gala's msvcrt
(`build.sh` rebuilds the checked-in binary with mingw against msvcrt's
own stdio).

## TinyCC

`user/tcc/build.sh` rebuilds `user/bin/tcc.tar` (pinned TinyCC commit,
reproducible): it compiles `tcc.c` as one unit with mingw-w64 and
writes `config.h` itself instead of running TinyCC's `configure` and
`make` (they would run third-party build scripts on the host; the direct
compile needs none, and `CONFIG_TCC_PREDEFS=0` makes tcc read
`include/tccdefs.h` at run time instead of a header generated by a
host-built tool). Its runtime library (`libtcc1.a`, the startup code) is
not built on the host: the `tcc` scenario (`user/tcc/tcc.sh`, run by
busybox sh) unpacks the archive with busybox's tar, builds the runtime
with that `tcc.exe` as win32/build-tcc.bat does, compiles
`user/tcc/hello.c` and runs it. A program tcc builds imports msvcrt's
startup functions (`__getmainargs`, `__argc`, `_controlfp`,
`_XcptFilter`), and tcc itself uses `_spawnvp`/`_cwait`/`SearchPathA`
only for tools that the scenario does not run. The scenario takes about
4 s in the kernel (16 tcc runs).

The `tcc_self` scenario (`user/tcc/self.sh`) runs tcc.sh, then compiles
TinyCC's sources (`tcc/src/tcc/` in the archive) with tcc on gala into
tcc2.exe, and uses tcc2 to build hello.c and `user/msvcrt_test/main.c`
(about 7 s in the kernel). main.c must therefore build with both mingw's
and TinyCC's headers (TinyCC's float.h lacks `_controlfp`; mingw's
headers send `strtod` to libmingwex, so the test looks msvcrt's up).

The `tcc_make` scenario (`user/tcc/make.sh`) builds `user/tcc/make/`
(boot modules in `C:\gala\make`) with busybox's make, passing
`CC=../tcc/tcc.exe CFLAGS=-B../tcc` so make's echoed commands are the
same on gala and Windows; each step's lines carry the step's name
(`build:`, `after touch:`, `again:`, `broken:`), since a scenario's
expected lines are only searched for.

## Lua

`user/lua/build.sh` repacks the Lua 5.4.7 release into `user/bin/lua.tar`
(checked against lua.org's published SHA-256; nothing is compiled on the
host). The `lua` scenario (`user/lua/lua.sh`) builds it on gala with
Lua's own `src/Makefile`, overriding only its documented settings
(`CC`, `CFLAGS`, `AR=tcc -ar rcs`, `RANLIB=:`, `LIBS=` since msvcrt has
the math functions, `LUA_T`/`LUAC_T` with `.exe`), and runs
`user/lua/test.lua`. Expected lines were written from Lua's and
msvcrt.dll's documented behaviour (compiling Lua on the host to compare
is a third-party build, which the session's permissions refuse; Windows
itself is the reference through `run-all.ps1`). test.lua avoids output
that depends on the script's path (`error(msg, 0)`) and exact decimal
ties (C runtimes round them differently).

## Reproducible user binaries

`cargo xtask export-tests` output must be the same on every machine (CI
compares it). Workspace crates get relative source paths, but crates.io
dependencies (libm in msvcrt) get absolute paths under the cargo home,
which end up in panic locations: xtask's `user_build` remaps the cargo
home to `/cargo` (`--remap-path-prefix`, merged into the target's
rustflags with `--config`). A new dependency needs nothing more.

## SQLite

`user/sqlite/build.sh` fetches the SQLite amalgamation zip (checked
against sqlite.org's SHA3-256) into `user/bin`; the `sqlite` scenario
unzips it on gala and compiles it with tcc and `user/sqlite/sqlrun.c`,
then runs `test.sql` and `reopen.sql` (expected lines computed with
Python's sqlite3 module on the host: same SQL, another SQLite version).
TinyCC's Windows headers and `.def` files are older than SQLite's needs:
`tcc -impdef` makes kernel32.def from the system's kernel32.dll, and
`tcc_win.h` (force-included) declares GetTickCount64.

The scenario's last step runs two writers at once (busybox `&` and
`wait`). To see whether they really contended, run it with
`XTASK_EVENTS=syscall,process` and count NtLockFile results per process
(STATUS_LOCK_NOT_GRANTED 0xc0000055 means a retry).

A program's output and the kernel's log share the serial console, so a
line a program writes in pieces can be split by a log line ("process ...
exited"): expect lines that are written in one piece (a shell `echo` of
a captured value) when that matters.

## Hunting a rare hang

A scenario that fails once in N runs: loop `cargo xtask boot-test <name>`
until it fails and keep that serial log. After a minute without the
program finishing the kernel dumps every thread (see ARCHITECTURE, "Hang
dump"): which system call each NT thread is in, on which object, with
which timeout. Tracing (`trace`) changes the timing enough to hide such
races, so prefer the dump. In gala-um, catch a hung instance live and
use `gdb -p` (`thread apply all bt`); its process is named `exe` after
gala-um re-executes itself without ASLR.

A failed boot scenario's serial log is kept in
`target/failed/<scenario>.log` (also from `cargo xtask test`), so a rare
failure in a full run is not lost. timer_test failed once in three full
runs after tcc_self added load (the check was not captured then; it
passes alone): if it recurs, its log says which bound was missed.

## Debugging

- `XTASK_VERBOSE=1 cargo xtask boot-test name` prints the serial log; logs
  are also kept in `target/debug/test-<name>.serial.log` (shared VMs:
  `target/debug/batch-<first program>.serial.log`).
- `XTASK_QEMU_ARGS="-d int -D qemu.log" cargo xtask boot-test name` passes
  extra QEMU arguments; the interrupt log shows every exception with the
  full register state (EFER, CR4, TR...), which is how per-CPU setup bugs
  show up (e.g. #UD on `syscall` = EFER.SCE missing on that CPU).
- Kernel command-line flag `trace` logs every system call with arguments
  and status (add it to a scenario's `cmdline` temporarily).
- Kernel panics print `backtrace:` return addresses; failed boot tests show
  them symbolized (function and source line), and `cargo xtask symbolize
  <serial log>` resolves any log.
- The event stream: `XTASK_EVENTS=all cargo xtask boot-test name` writes
  JSON lines (system calls with timing, image maps, threads, processes,
  faults) to `target/debug/<iso>.events.jsonl`; filter with `jq` or grep.
- `cargo xtask report` writes `target/report/gala.{json,md}`;
  `cargo xtask compare windows.json` diffs against results collected on
  Windows with `user/bin/run-all.ps1`.
- See `docs/OBSERVABILITY.md` for the planned profiling and telemetry.

## Where things are

| Area | Path |
|---|---|
| Boot, Limine protocol | `kernel/src/boot/` |
| CPU, paging, interrupts, SYSCALL, SMP | `kernel/src/arch/x86_64/` |
| Frames, heap | `kernel/src/mm/` |
| Scheduler | `kernel/src/sched/` |
| NT personality (objects, processes, files, sections, syscalls) | `kernel/src/nt/` |
| Ramfs | `kernel/src/fs/` |
| Test runtime for user programs | `user/ntrt/` |
| Test programs, test DLLs, stand-in ntdll | `user/*_test/`, `user/testdll*`, `user/testntdll/` |
| Checked-in binaries, Windows runner | `user/bin/` |
| Build, boot tests, reports | `xtask/` |

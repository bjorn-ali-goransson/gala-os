//! Build, boot and test driver: `cargo xtask <command>`.
//!
//! Host requirements: `git`, `xorriso`, `qemu-system-x86_64` and an OVMF
//! (UEFI) firmware image. Set `OVMF_CODE` / `OVMF_VARS` to override the search.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Pinned Limine release (prebuilt binary branch tag) and its expected commit.
const LIMINE_TAG: &str = "v11.4.1-binary";
const LIMINE_COMMIT: &str = "5be26a73d7b7b4d4477d18be94e1d16e615adf56";
const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";

const KERNEL_TARGET: &str = "x86_64-unknown-none";
/// Freestanding Windows test programs in `user/`, shipped as boot modules.
const USER_TARGET: &str = "x86_64-pc-windows-msvc";
/// A test DLL: its package, the file it builds, and its name in user/bin.
struct Dll {
    package: &'static str,
    file: &'static str,
    exported_as: &'static str,
}

/// Test DLLs (embedded by test programs or shipped to scenarios; also
/// exported to user/bin).
const USER_DLLS: &[Dll] = &[
    Dll { package: "testdll", file: "testdll.dll", exported_as: "testdll.dll" },
    // Built as ntdll.dll (its export directory says so); exported under
    // another name so user/bin never holds a file called ntdll.dll.
    Dll { package: "testntdll", file: "ntdll.dll", exported_as: "testntdll.dll" },
];

/// Extra boot modules per scenario: (scenario, built DLL file, directory
/// on C: that the kernel puts it in, passed as the module string).
const EXTRA_MODULES: &[(&str, &str, &str)] = &[
    ("startup_test", "ntdll.dll", "Windows/System32"),
    ("exception_test", "ntdll.dll", "Windows/System32"),
    ("guard_test", "ntdll.dll", "Windows/System32"),
];
const USER_PROGRAMS: &[&str] = &[
    "hello",
    "abi_test",
    "crash",
    "vm_test",
    "wait_test",
    "object_test",
    "thread_test",
    "kuser_test",
    "section_test",
    "file_test",
    "image_test",
    "dir_test",
    "peb_test",
    "startup_test",
    "exception_test",
    "guard_test",
];
/// Must match `BOOT_OK_MARKER` in `kernel/src/lib.rs`.
const BOOT_OK_MARKER: &str = "gala: boot ok";
/// QEMU exit statuses for `QemuExitCode::{Success, Failure}` (`(code << 1) | 1`).
const QEMU_EXIT_SUCCESS: i32 = 33;
const QEMU_EXIT_FAILURE: i32 = 35;
/// Logged by the kernel when a finished process's memory is still referenced.
const LEAK_MARKER: &str = "address space still referenced";
const BOOT_TEST_TIMEOUT: Duration = Duration::from_secs(120);

type Result<T = ()> = std::result::Result<T, String>;

const USAGE: &str = "\
usage: cargo xtask <command> [--release]

commands:
  build        build the kernel and a bootable UEFI ISO (target/<profile>/gala-os.iso)
  run          build, then boot in QEMU with serial on stdio [--headless]
  test         run host unit tests, then boot in QEMU and check the result
  boot-test    only the QEMU boot tests [scenario name filter]
  export-tests copy the built test programs to user/bin/ (checked in)
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|a| a == "--release");
    let headless = args.iter().any(|a| a == "--headless");
    let result = match args.first().map(String::as_str) {
        Some("build") => build(release, "gala-os", "").map(drop),
        Some("run") => build(release, "gala-os", "").and_then(|iso| run(&iso, headless)),
        Some("test") => unit_tests().and_then(|()| boot_tests(release, None)),
        Some("boot-test") => boot_tests(release, args.get(1).filter(|a| !a.starts_with("--"))),
        Some("export-tests") => build_kernel(release).and_then(|()| export_tests()),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn cargo() -> Command {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

/// Runs `cmd`, showing its output only if it fails.
fn run_quiet(cmd: &mut Command) -> Result {
    let out = cmd.output().map_err(|e| format!("failed to spawn {cmd:?}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    eprint!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Err(format!("{cmd:?} failed: {}", out.status))
}

fn run_cmd(cmd: &mut Command) -> Result {
    let status = cmd.status().map_err(|e| format!("failed to spawn {cmd:?}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd:?} failed: {status}")) }
}

fn unit_tests() -> Result {
    eprintln!("xtask: running kernel unit tests on the host");
    run_cmd(cargo().args(["test", "--package", "kernel", "--lib"]))
}

/// Builds the kernel and packs it with Limine into a UEFI-bootable ISO named
/// `name.iso` whose boot entry passes `cmdline` to the kernel.
fn build(release: bool, name: &str, cmdline: &str) -> Result<PathBuf> {
    build_kernel(release)?;
    make_iso(release, name, cmdline)
}

fn build_kernel(release: bool) -> Result {
    let mut cmd = cargo();
    cmd.args(["build", "--package", "kernel", "--bin", "kernel", "--target", KERNEL_TARGET]);
    if release {
        cmd.arg("--release");
    }
    run_cmd(&mut cmd)?;
    // User programs are always optimized: without a CRT there is no
    // `memcpy` etc. for unoptimized code to call. DLLs that programs embed
    // are built first.
    let mut cmd = cargo();
    cmd.args(["build", "--release", "--target", USER_TARGET]);
    for dll in USER_DLLS {
        cmd.args(["--package", dll.package]);
    }
    run_cmd(&mut cmd)?;
    let mut cmd = cargo();
    cmd.args(["build", "--release", "--target", USER_TARGET]);
    for program in USER_PROGRAMS {
        cmd.args(["--package", program]);
    }
    run_cmd(&mut cmd)
}

fn make_iso(release: bool, name: &str, cmdline: &str) -> Result<PathBuf> {
    let profile = if release { "release" } else { "debug" };
    let kernel = target_dir().join(KERNEL_TARGET).join(profile).join("kernel");
    let limine = fetch_limine()?;
    let out = target_dir().join(profile);
    let iso_root = out.join(format!("{name}-iso-root"));
    if iso_root.exists() {
        fs::remove_dir_all(&iso_root).map_err(|e| format!("cleaning {iso_root:?}: {e}"))?;
    }
    let efi_boot = iso_root.join("EFI/BOOT");
    let boot = iso_root.join("boot");
    for dir in [&efi_boot, &boot.join("limine")] {
        fs::create_dir_all(dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    }
    copy(&kernel, &boot.join("kernel"))?;
    let mut modules = String::new();
    for program in USER_PROGRAMS {
        let exe = target_dir().join(USER_TARGET).join("release").join(format!("{program}.exe"));
        copy(&exe, &boot.join(format!("{program}.exe")))?;
        modules += &format!("    module_path: boot():/boot/{program}.exe\n");
    }
    let scenario = name.strip_prefix("test-").unwrap_or(name);
    for (_, file, dir) in EXTRA_MODULES.iter().filter(|m| m.0 == scenario) {
        let built = target_dir().join(USER_TARGET).join("release").join(file);
        copy(&built, &boot.join(file))?;
        modules += &format!("    module_path: boot():/boot/{file}\n    module_string: {dir}\n");
    }
    copy(&limine.join("BOOTX64.EFI"), &efi_boot.join("BOOTX64.EFI"))?;
    copy(&limine.join("limine-uefi-cd.bin"), &boot.join("limine/limine-uefi-cd.bin"))?;
    let config = format!(
        "timeout: 0\n\n/gala-os\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n{modules}"
    );
    write(&boot.join("limine/limine.conf"), &config)?;

    let iso = out.join(format!("{name}.iso"));
    run_quiet(
        Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-r", "-J", "-quiet"])
            .args(["--efi-boot", "boot/limine/limine-uefi-cd.bin"])
            .args(["-efi-boot-part", "--efi-boot-image", "--protective-msdos-label"])
            .arg(&iso_root)
            .arg("-o")
            .arg(&iso),
    )?;
    if name == "gala-os" {
        eprintln!("xtask: built {}", iso.display());
    }
    Ok(iso)
}

/// Clones the pinned Limine binary release into the target directory once.
fn fetch_limine() -> Result<PathBuf> {
    let dir = target_dir().join(format!("limine-{LIMINE_TAG}"));
    if !dir.join("BOOTX64.EFI").exists() {
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("cleaning {dir:?}: {e}"))?;
        }
        eprintln!("xtask: fetching Limine {LIMINE_TAG}");
        run_cmd(
            Command::new("git")
                .args(["clone", "--quiet", "--depth", "1", "--branch", LIMINE_TAG, LIMINE_REPO])
                .arg(&dir),
        )?;
    }
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("git rev-parse: {e}"))?;
    let head = String::from_utf8_lossy(&head.stdout);
    if head.trim() != LIMINE_COMMIT {
        return Err(format!(
            "Limine checkout {dir:?} is at {}, expected {LIMINE_COMMIT}; delete it and retry",
            head.trim()
        ));
    }
    Ok(dir)
}

struct Firmware {
    code: PathBuf,
    vars: Option<PathBuf>,
}

fn find_ovmf() -> Result<Firmware> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
        ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"),
        ("/usr/share/edk2/x64/OVMF_CODE.4m.fd", "/usr/share/edk2/x64/OVMF_VARS.4m.fd"),
        ("/usr/share/edk2/ovmf/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"),
        ("/usr/share/qemu/edk2-x86_64-code.fd", "/usr/share/qemu/edk2-i386-vars.fd"),
        (
            "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
            "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
        ),
    ];
    if let Some(code) = env::var_os("OVMF_CODE") {
        let vars = env::var_os("OVMF_VARS").map(PathBuf::from);
        return Ok(Firmware { code: code.into(), vars });
    }
    CANDIDATES
        .iter()
        .find(|(code, _)| Path::new(code).exists())
        .map(|(code, vars)| Firmware {
            code: code.into(),
            vars: Path::new(vars).exists().then(|| vars.into()),
        })
        .ok_or_else(|| "no OVMF firmware found; install OVMF or set OVMF_CODE".into())
}

fn qemu(iso: &Path) -> Result<Command> {
    let fw = find_ovmf()?;
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", "q35", "-cpu", "max", "-m", "512M", "-smp", "2"])
        // KVM when available, otherwise software emulation.
        .args(["-accel", "kvm", "-accel", "tcg"])
        .args(["-no-reboot"])
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,unit=0,readonly=on,file={}", fw.code.display()));
    if let Some(vars) = fw.vars {
        // The firmware writes its variable store; give it a private copy.
        let copy_path = iso.with_extension("ovmf-vars.fd");
        copy(&vars, &copy_path)?;
        cmd.arg("-drive").arg(format!("if=pflash,format=raw,unit=1,file={}", copy_path.display()));
    }
    cmd.arg("-cdrom").arg(iso);
    Ok(cmd)
}

fn run(iso: &Path, headless: bool) -> Result {
    let mut cmd = qemu(iso)?;
    cmd.args(["-serial", "stdio"]);
    if headless {
        cmd.args(["-display", "none"]);
    }
    run_cmd(&mut cmd)
}

/// A boot test: a kernel command line and the expected outcome.
struct Scenario {
    name: &'static str,
    cmdline: &'static str,
    exit_status: i32,
    /// Lines (substrings) that must appear in the serial log.
    expect: &'static [&'static str],
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "guard_test",
        cmdline: "qemu_exit run=guard_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["guard_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "exception_test",
        cmdline: "qemu_exit run=exception_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["exception_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "startup_test",
        cmdline: "qemu_exit run=startup_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["startup_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "peb_test",
        cmdline: "qemu_exit run=peb_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["peb_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "dir_test",
        cmdline: "qemu_exit run=dir_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["dir_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "image_test",
        cmdline: "qemu_exit run=image_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["image_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "file_test",
        cmdline: "qemu_exit run=file_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["file_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "section_test",
        cmdline: "qemu_exit run=section_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["section_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "kuser_test",
        cmdline: "qemu_exit run=kuser_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["kuser_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "thread_test",
        cmdline: "qemu_exit run=thread_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["thread_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "object_test",
        cmdline: "qemu_exit run=object_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["object_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "wait_test",
        cmdline: "qemu_exit run=wait_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["wait_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "vm_test",
        cmdline: "qemu_exit run=vm_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["vm_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "abi_test",
        cmdline: "qemu_exit run=abi_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["abi_test: write ok", "abi_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "crash",
        cmdline: "qemu_exit run=crash.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "crash: writing to address 0",
            "user exception: page fault (#PF)",
            "process crash.exe (pid 4) exited with status 0xc0000005",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        name: "hello",
        cmdline: "qemu_exit run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "Hello from user mode!",
            "process hello.exe (pid 4) exited with status 0x00000000",
            BOOT_OK_MARKER,
        ],
    },
    Scenario {
        name: "selftest",
        cmdline: "qemu_exit selftest",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[BOOT_OK_MARKER, "selftest: "],
    },
    Scenario {
        name: "fault-ud",
        cmdline: "qemu_exit fault=ud",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &["KERNEL PANIC", "CPU exception: invalid opcode (#UD)"],
    },
    Scenario {
        name: "fault-pagefault",
        cmdline: "qemu_exit fault=pagefault",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &["CPU exception: page fault (#PF) error=0x0", "cr2=0x0000000000000008"],
    },
    Scenario {
        name: "fault-stackoverflow",
        cmdline: "qemu_exit fault=stackoverflow",
        exit_status: QEMU_EXIT_FAILURE,
        expect: &["CPU exception: double fault (#DF)"],
    },
];

/// Boots every scenario (matching `filter`) headless, in parallel.
fn boot_tests(release: bool, filter: Option<&String>) -> Result {
    build_kernel(release)?;
    let selected: Vec<&Scenario> =
        SCENARIOS.iter().filter(|s| filter.is_none_or(|f| s.name.contains(f.as_str()))).collect();
    if selected.is_empty() {
        return Err("no boot test scenario matches the filter".into());
    }
    let mut isos = Vec::new();
    for scenario in &selected {
        isos.push(make_iso(release, &format!("test-{}", scenario.name), scenario.cmdline)?);
    }
    let results: Vec<Result> = thread::scope(|scope| {
        let handles: Vec<_> = selected
            .iter()
            .zip(&isos)
            .map(|(scenario, iso)| scope.spawn(move || boot_scenario(scenario, iso)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("panicked".into()))).collect()
    });
    let mut failed = 0;
    for (scenario, result) in selected.iter().zip(&results) {
        match result {
            Ok(()) => eprintln!("xtask: boot test {:<20} ok", scenario.name),
            Err(e) => {
                failed += 1;
                eprintln!("xtask: boot test {:<20} FAILED: {e}", scenario.name);
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} of {} boot tests failed", selected.len()))
    }
}

fn boot_scenario(scenario: &Scenario, iso: &Path) -> Result {
    let log = iso.with_extension("serial.log");
    let _ = fs::remove_file(&log);
    let mut cmd = qemu(iso)?;
    cmd.args(["-display", "none", "-monitor", "none"])
        .arg("-serial")
        .arg(format!("file:{}", log.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn QEMU: {e}"))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if start.elapsed() > BOOT_TEST_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(100));
    };

    let mut serial = String::new();
    if let Ok(mut f) = fs::File::open(&log) {
        let _ = f.read_to_string(&mut serial);
    }
    let missing: Vec<&str> =
        scenario.expect.iter().copied().filter(|e| !serial.contains(e)).collect();
    let verdict = match status {
        None => Err(format!("timed out after {}s", BOOT_TEST_TIMEOUT.as_secs())),
        Some(s) if s.code() != Some(scenario.exit_status) => {
            Err(format!("QEMU exited with {s}, expected status {}", scenario.exit_status))
        }
        Some(_) if !missing.is_empty() => Err(format!("serial log lacks {missing:?}")),
        Some(_) if scenario.exit_status == QEMU_EXIT_SUCCESS && serial.contains(LEAK_MARKER) => {
            Err("a process address space was not freed".into())
        }
        Some(_) => Ok(()),
    };
    if verdict.is_err() || env::var_os("XTASK_VERBOSE").is_some() {
        eprintln!(
            "----- {} serial output ({}) -----\n{}\n-----",
            scenario.name,
            log.display(),
            serial.trim_end()
        );
    }
    verdict
}

/// Copies the test programs to `user/bin/`, where they are checked in so
/// they can be run on real Windows without a Rust toolchain.
fn export_tests() -> Result {
    let dir = root().join("user/bin");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    for program in USER_PROGRAMS {
        let exe = target_dir().join(USER_TARGET).join("release").join(format!("{program}.exe"));
        copy(&exe, &dir.join(format!("{program}.exe")))?;
    }
    for dll in USER_DLLS {
        let file = target_dir().join(USER_TARGET).join("release").join(dll.file);
        copy(&file, &dir.join(dll.exported_as))?;
    }
    eprintln!("xtask: exported {} test programs to {}", USER_PROGRAMS.len(), dir.display());
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to).map(drop).map_err(|e| format!("copying {from:?} to {to:?}: {e}"))
}

fn write(path: &Path, contents: &str) -> Result {
    fs::write(path, contents).map_err(|e| format!("writing {path:?}: {e}"))
}

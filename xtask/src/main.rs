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

mod report;

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
    Dll { package: "testdll2", file: "testdll2.dll", exported_as: "testdll2.dll" },
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
    ("import_test", "ntdll.dll", "Windows/System32"),
    ("import_test", "testdll.dll", "gala"),
    ("fpu_test", "ntdll.dll", "Windows/System32"),
    ("apc_test", "ntdll.dll", "Windows/System32"),
    ("ioapc_test", "ntdll.dll", "Windows/System32"),
    ("sync_test", "ntdll.dll", "Windows/System32"),
    ("symlink_test", "ntdll.dll", "Windows/System32"),
    ("iocp_test", "ntdll.dll", "Windows/System32"),
    ("context_test", "ntdll.dll", "Windows/System32"),
    ("timer_test", "ntdll.dll", "Windows/System32"),
    ("loader_test", "ntdll.dll", "Windows/System32"),
    ("loader_test", "testdll.dll", "gala"),
    ("loader_test", "testdll2.dll", "gala"),
    ("smpsched_test", "ntdll.dll", "Windows/System32"),
    ("smpmem_test", "ntdll.dll", "Windows/System32"),
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
    "import_test",
    "args_test",
    "fpu_test",
    "sysinfo_test",
    "threadinfo_test",
    "filemap_test",
    "cow_test",
    "async_test",
    "apc_test",
    "ioapc_test",
    "sync_test",
    "symlink_test",
    "iocp_test",
    "context_test",
    "timer_test",
    "loader_test",
    "smp_test",
    "smpsched_test",
    "smpmem_test",
    "ledger_test",
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
  screenshot   boot and save PNG screenshots of the screen:
               screenshot [--uefi] [--at 1,3,5] [--name N] [kernel cmdline...]
               (default: the boot log, at 3 s; files in target/screenshots/)
  docker-test  build the test image (docker/test.Dockerfile) and run
               `cargo xtask test` (or the given xtask arguments) inside it
  report       run every test program on gala; write target/report/gala.{json,md}
  compare      compare a report from Windows (user/bin/run-all.ps1) with gala's:
               compare <windows.json> [<gala.json>]
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
        Some("report") => gala_report(release),
        Some("docker-test") => docker_test(&args[1..]),
        Some("screenshot") => screenshot(release, &args[1..]),
        Some("compare") => match args.get(1).filter(|a| !a.starts_with("--")) {
            Some(windows) => compare_reports(Path::new(windows), args.get(2).map(Path::new)),
            None => Err("usage: cargo xtask compare <windows.json> [<gala.json>]".into()),
        },
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
    eprintln!("xtask: running kernel and xtask unit tests on the host");
    run_cmd(cargo().args(["test", "--package", "kernel", "--lib"]))?;
    run_cmd(cargo().args(["test", "--package", "xtask"]))
}

/// Builds the kernel and packs it with Limine into a UEFI-bootable ISO named
/// `name.iso` whose boot entry passes `cmdline` to the kernel.
fn build(release: bool, name: &str, cmdline: &str) -> Result<PathBuf> {
    build_kernel(release)?;
    make_iso(release, name, cmdline, &[])
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

fn make_iso(release: bool, name: &str, cmdline: &str, extra: &[(&str, &str)]) -> Result<PathBuf> {
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
    for (file, dir) in extra {
        let built = target_dir().join(USER_TARGET).join("release").join(file);
        copy(&built, &boot.join(file))?;
        modules += &format!("    module_path: boot():/boot/{file}\n    module_string: {dir}\n");
    }
    copy(&limine.join("BOOTX64.EFI"), &efi_boot.join("BOOTX64.EFI"))?;
    for file in ["limine-uefi-cd.bin", "limine-bios-cd.bin", "limine-bios.sys"] {
        copy(&limine.join(file), &boot.join("limine").join(file))?;
    }
    let config = format!(
        "timeout: 0\n\n/gala-os\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n{modules}"
    );
    write(&boot.join("limine/limine.conf"), &config)?;

    let iso = out.join(format!("{name}.iso"));
    run_quiet(
        Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-r", "-J", "-quiet"])
            .args(["-b", "boot/limine/limine-bios-cd.bin", "-no-emul-boot", "-boot-load-size", "4"])
            .args(["-boot-info-table"])
            .args(["--efi-boot", "boot/limine/limine-uefi-cd.bin"])
            .args(["-efi-boot-part", "--efi-boot-image", "--protective-msdos-label"])
            .arg(&iso_root)
            .arg("-o")
            .arg(&iso),
    )?;
    // Hybrid image: bootable by BIOS (Limine's stages) and UEFI.
    run_quiet(Command::new(limine.join("limine")).arg("bios-install").arg(&iso))?;
    if name == "gala-os" {
        eprintln!("xtask: built {}", iso.display());
    }
    Ok(iso)
}

/// Clones the pinned Limine binary release into the target directory once
/// (once per xtask run too: ISOs are built in parallel, and concurrent
/// clones into one directory fail).
fn fetch_limine() -> Result<PathBuf> {
    static LIMINE: std::sync::OnceLock<Result<PathBuf>> = std::sync::OnceLock::new();
    LIMINE.get_or_init(fetch_limine_once).clone()
}

fn fetch_limine_once() -> Result<PathBuf> {
    // Under the repository's own target/ even with CARGO_TARGET_DIR set, so
    // the host and the Docker image (target/docker) share one checkout.
    let dir = root().join("target").join(format!("limine-{LIMINE_TAG}"));
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
    if !dir.join("limine").exists() {
        // The host tool that installs Limine's BIOS stages into an ISO.
        run_quiet(Command::new("make").arg("-C").arg(&dir))?;
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

/// The firmware a VM boots with. ISOs are hybrid, so either works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boot {
    /// QEMU's SeaBIOS: reaches the kernel about 5x faster than OVMF under
    /// software emulation, so boot tests use it by default.
    Bios,
    /// OVMF (UEFI), the path real machines take.
    Uefi,
}

/// Boot tests that boot through UEFI, so that path stays covered.
const UEFI_SCENARIOS: &[&str] = &["hello", "selftest", "fault-pagefault"];

/// The firmware for scenario `name` (`XTASK_FIRMWARE=bios|uefi` overrides).
fn scenario_boot(name: &str) -> Boot {
    match env::var("XTASK_FIRMWARE").as_deref() {
        Ok("uefi") => Boot::Uefi,
        Ok("bios") => Boot::Bios,
        _ if UEFI_SCENARIOS.contains(&name) => Boot::Uefi,
        _ => Boot::Bios,
    }
}

fn qemu(iso: &Path, boot: Boot) -> Result<Command> {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-machine", "q35", "-cpu", "max", "-m", "512M", "-smp", "2"])
        // KVM when available, otherwise software emulation.
        .args(["-accel", "kvm", "-accel", "tcg"])
        .args(["-no-reboot"]);
    if boot == Boot::Uefi {
        let fw = find_ovmf()?;
        cmd.arg("-drive")
            .arg(format!("if=pflash,format=raw,unit=0,readonly=on,file={}", fw.code.display()));
        if let Some(vars) = fw.vars {
            // The firmware writes its variable store; give it a private copy.
            let copy_path = iso.with_extension("ovmf-vars.fd");
            copy(&vars, &copy_path)?;
            cmd.arg("-drive")
                .arg(format!("if=pflash,format=raw,unit=1,file={}", copy_path.display()));
        }
    }
    cmd.arg("-cdrom").arg(iso);
    // Extra arguments for debugging, e.g. "-d int,cpu_reset -D qemu.log".
    if let Ok(extra) = std::env::var("XTASK_QEMU_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    Ok(cmd)
}

fn run(iso: &Path, headless: bool) -> Result {
    let mut cmd = qemu(iso, Boot::Uefi)?;
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
        name: "timer_test",
        cmdline: "qemu_exit run=timer_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["timer_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "loader_test",
        cmdline: "qemu_exit run=loader_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["loader_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smp_test",
        // QEMU runs with -smp 2 (see `qemu`).
        cmdline: "qemu_exit run=smp_test.exe args=cpus=2",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smp: 2 CPUs online", "smp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smpsched_test",
        cmdline: "qemu_exit run=smpsched_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpsched_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "smpmem_test",
        cmdline: "qemu_exit run=smpmem_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["smpmem_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "ledger_test",
        cmdline: "qemu_exit run=ledger_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "ledger_test: ok",
            "exited with status 0x00000000",
            r#"ledger: {"service":"?","what":"unknown service","detail":4095,"hits":1}"#,
            r#"ledger: {"service":"NtQueryInformationProcess","what":"info class","detail":32512,"hits":2}"#,
        ],
    },
    Scenario {
        name: "context_test",
        cmdline: "qemu_exit run=context_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["context_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "iocp_test",
        cmdline: "qemu_exit run=iocp_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["iocp_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "symlink_test",
        cmdline: "qemu_exit run=symlink_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["symlink_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "sync_test",
        cmdline: "qemu_exit run=sync_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sync_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "ioapc_test",
        cmdline: "qemu_exit run=ioapc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["ioapc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "apc_test",
        cmdline: "qemu_exit run=apc_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["apc_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "async_test",
        cmdline: "qemu_exit run=async_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["async_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "cow_test",
        cmdline: "qemu_exit run=cow_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["cow_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "filemap_test",
        cmdline: "qemu_exit run=filemap_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["filemap_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "threadinfo_test",
        cmdline: "qemu_exit run=threadinfo_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["threadinfo_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "sysinfo_test",
        cmdline: "qemu_exit run=sysinfo_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["sysinfo_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "fpu_test",
        cmdline: "qemu_exit run=fpu_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["fpu_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "args_test",
        // `args=` is percent-encoded: %20 space, %22 quote, %25 percent.
        cmdline: "qemu_exit run=args_test.exe args=alpha%20%22beta%20gamma%22",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["args_test: ok", "exited with status 0x00000000"],
    },
    Scenario {
        name: "trace",
        cmdline: "qemu_exit trace run=hello.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &[
            "trace: pid 4 tid 8 NtWriteFile(",
            ") -> 0x00000000",
            "NtTerminateProcess(0xffffffffffffffff, 0x0)",
            "Hello from user mode!",
        ],
    },
    Scenario {
        name: "import_test",
        cmdline: "qemu_exit run=import_test.exe",
        exit_status: QEMU_EXIT_SUCCESS,
        expect: &["import_test: ok", "exited with status 0x00000000"],
    },
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
/// Programs per shared VM at most (more VMs than CPUs only queue).
const MAX_BATCH: usize = 12;

/// One VM of a boot test run: a scenario of its own, or several
/// compatible scenarios whose programs run one after another.
enum Vm<'a> {
    Single(&'a Scenario),
    Batch(Vec<&'a Scenario>),
}

/// The boot modules (file, directory) scenario `name` ships besides the
/// test programs.
fn extra_modules(name: &str) -> Vec<(&'static str, &'static str)> {
    let mut modules: Vec<_> =
        EXTRA_MODULES.iter().filter(|m| m.0 == name).map(|m| (m.1, m.2)).collect();
    modules.sort_unstable();
    modules
}

/// `(program file, args)` if the scenario can share a VM: it only runs one
/// program (no other kernel flags), expects success, boots by BIOS, and
/// expects nothing that depends on running first (process ids).
fn batchable(s: &Scenario) -> Option<(&'static str, &'static str)> {
    if s.exit_status != QEMU_EXIT_SUCCESS
        || scenario_boot(s.name) != Boot::Bios
        || s.expect.iter().any(|e| e.contains("pid "))
    {
        return None;
    }
    let (mut program, mut args) = (None, "");
    for word in s.cmdline.split_whitespace() {
        match word.split_once('=') {
            _ if word == "qemu_exit" => {}
            Some(("run", p)) if !p.contains(',') => program = Some(p),
            Some(("args", a)) if !a.contains(',') => args = a,
            _ => return None,
        }
    }
    program.map(|p| (p, args))
}

/// Groups the selected scenarios into VMs: batchable ones with identical
/// boot modules share VMs (spread over the host CPUs), the rest boot alone.
fn plan_vms<'a>(selected: &[&'a Scenario]) -> Vec<Vm<'a>> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<Vec<(&str, &str)>, Vec<&Scenario>> = BTreeMap::new();
    let mut vms = Vec::new();
    for &s in selected {
        match batchable(s) {
            Some(_) if env::var_os("XTASK_NO_BATCH").is_none() => {
                groups.entry(extra_modules(s.name)).or_default().push(s)
            }
            _ => vms.push(Vm::Single(s)),
        }
    }
    let cpus = thread::available_parallelism().map_or(2, |n| n.get());
    let batched: usize = groups.values().map(Vec::len).sum();
    let size = batched.div_ceil(cpus).clamp(1, MAX_BATCH);
    for group in groups.into_values() {
        for chunk in group.chunks(size) {
            vms.push(match chunk {
                [one] => Vm::Single(one),
                _ => Vm::Batch(chunk.to_vec()),
            });
        }
    }
    vms
}

fn boot_tests(release: bool, filter: Option<&String>) -> Result {
    let t0 = Instant::now();
    build_kernel(release)?;
    let built = t0.elapsed();
    let selected: Vec<&Scenario> =
        SCENARIOS.iter().filter(|s| filter.is_none_or(|f| s.name.contains(f.as_str()))).collect();
    if selected.is_empty() {
        return Err("no boot test scenario matches the filter".into());
    }
    let vms = plan_vms(&selected);
    let t1 = Instant::now();
    // Tests read the serial log; the screen console stays on only where
    // the scenario boots by UEFI, which keeps that code covered.
    let screen = |s: &Scenario| if scenario_boot(s.name) == Boot::Uefi { "" } else { " noscreen" };
    let isos = run_parallel(&vms, |vm| match vm {
        Vm::Single(s) => make_iso(
            release,
            &format!("test-{}", s.name),
            &format!("{}{}", s.cmdline, screen(s)),
            &extra_modules(s.name),
        ),
        Vm::Batch(group) => {
            let (programs, args): (Vec<_>, Vec<_>) =
                group.iter().filter_map(|s| batchable(s)).unzip();
            let cmdline =
                format!("qemu_exit noscreen run={} args={}", programs.join(","), args.join(","));
            make_iso(
                release,
                &format!("batch-{}", group[0].name),
                &cmdline,
                &extra_modules(group[0].name),
            )
        }
    });
    let isos: Vec<PathBuf> = isos.into_iter().collect::<Result<_>>()?;
    let isos_made = t1.elapsed();
    let t2 = Instant::now();
    let jobs: Vec<(&Vm, &PathBuf)> = vms.iter().zip(&isos).collect();
    let results = run_parallel(&jobs, |(vm, iso)| Ok(boot_vm(vm, iso)));
    let booted = t2.elapsed();
    let mut failed = 0;
    let (mut firmware, mut kernel) = (Duration::ZERO, Duration::ZERO);
    for (vm, result) in vms.iter().zip(results) {
        let Ok((time, verdicts)) = result else { continue };
        firmware += time.to_banner;
        kernel += time.total.saturating_sub(time.to_banner);
        let shared = matches!(vm, Vm::Batch(_));
        for (scenario, verdict) in verdicts {
            match verdict {
                Ok(()) if shared => eprintln!("xtask: boot test {:<20} ok", scenario.name),
                Ok(()) => eprintln!(
                    "xtask: boot test {:<20} ok  ({:.1}s to kernel, {:.1}s in kernel)",
                    scenario.name,
                    time.to_banner.as_secs_f64(),
                    time.total.saturating_sub(time.to_banner).as_secs_f64()
                ),
                Err(e) => {
                    failed += 1;
                    eprintln!("xtask: boot test {:<20} FAILED: {e}", scenario.name);
                }
            }
        }
        if let Vm::Batch(group) = vm {
            eprintln!(
                "xtask:   (shared VM of {} programs: {:.1}s to kernel, {:.1}s in kernel)",
                group.len(),
                time.to_banner.as_secs_f64(),
                time.total.saturating_sub(time.to_banner).as_secs_f64()
            );
        }
    }
    let summary = format!(
        "build {:.1}s, ISOs {:.1}s, boots {:.1}s wall ({} scenarios in {} VMs; summed: {:.1}s firmware+bootloader, {:.1}s kernel)",
        built.as_secs_f64(),
        isos_made.as_secs_f64(),
        booted.as_secs_f64(),
        selected.len(),
        vms.len(),
        firmware.as_secs_f64(),
        kernel.as_secs_f64()
    );
    eprintln!("xtask: timing: {summary}");
    let _ = fs::create_dir_all(report_dir());
    let _ = write(&report_dir().join("timing.txt"), &format!("{summary}\n"));
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} of {} boot tests failed", selected.len()))
    }
}

/// Runs `f` on every job, at most one per host CPU at a time (each job is
/// an emulator that needs a CPU: running them all at once starves them on
/// small CI machines). Results are in job order.
fn run_parallel<J: Sync, T: Send>(
    jobs: &[J],
    f: impl Fn(&J) -> Result<T> + Sync,
) -> Vec<Result<T>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let workers = thread::available_parallelism().map_or(2, |n| n.get()).min(jobs.len().max(1));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<T>>>> = Mutex::new(jobs.iter().map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let result = f(job);
                    if let Ok(mut r) = results.lock() {
                        r[i] = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err("did not run".into())))
        .collect()
}

/// The program a scenario runs, if it is that program's own scenario.
fn scenario_program(scenario: &Scenario) -> Option<&'static str> {
    let program = scenario.cmdline.split_whitespace().find_map(|w| w.strip_prefix("run="))?;
    let program = program.strip_suffix(".exe")?;
    (program == scenario.name).then_some(program)
}

fn report_dir() -> PathBuf {
    target_dir().join("report")
}

/// Runs every test program's scenario and records each outcome.
fn gala_report(release: bool) -> Result {
    build_kernel(release)?;
    let selected: Vec<(&Scenario, &str)> =
        SCENARIOS.iter().filter_map(|s| scenario_program(s).map(|p| (s, p))).collect();
    let mut isos = Vec::new();
    for (scenario, _) in &selected {
        isos.push(make_iso(
            release,
            &format!("report-{}", scenario.name),
            scenario.cmdline,
            &extra_modules(scenario.name),
        )?);
    }
    // The verdicts do not matter here: outcomes are read from the logs.
    let jobs: Vec<(&Scenario, &PathBuf)> = selected.iter().map(|(s, _)| *s).zip(&isos).collect();
    let _ = run_parallel(&jobs, |(scenario, iso)| boot_scenario_quiet(scenario, iso));
    let mut results = Vec::new();
    let mut ledger = Vec::new();
    for ((_, program), iso) in selected.iter().zip(&isos) {
        let serial = fs::read_to_string(iso.with_extension("serial.log")).unwrap_or_default();
        results.push(report::outcome_from_serial(program, &serial));
        report::merge_ledger(&mut ledger, program, &serial);
    }
    let dir = report_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    write(&dir.join("ledger.md"), &report::ledger_markdown(&ledger))?;
    eprintln!(
        "xtask: wrote {} ({} missing features)",
        dir.join("ledger.md").display(),
        ledger.len()
    );
    write(&dir.join("gala.json"), &report::to_json("gala", &results))?;
    let table = report::markdown("gala", &results);
    write(&dir.join("gala.md"), &table)?;
    print!("{table}");
    eprintln!("xtask: wrote {}", dir.join("gala.json").display());
    Ok(())
}

/// Prints how each program fared on Windows and on gala; fails if any
/// program that ran on both ended differently.
fn compare_reports(windows: &Path, gala: Option<&Path>) -> Result {
    let read = |p: &Path| -> Result<(String, Vec<report::Outcome>)> {
        let text = fs::read_to_string(p).map_err(|e| format!("reading {p:?}: {e}"))?;
        report::from_json(&text).map_err(|e| format!("{p:?}: {e}"))
    };
    let gala_path = gala.map_or_else(|| report_dir().join("gala.json"), Path::to_path_buf);
    let (_, w) = read(windows)?;
    let (_, g) = read(&gala_path)?;
    let show = |o: &Option<report::Outcome>| match o {
        Some(report::Outcome { exit: Some(e), .. }) => format!("{e:#010x}"),
        Some(report::Outcome { skipped: Some(s), .. }) => format!("skipped ({s})"),
        _ => "-".to_string(),
    };
    println!("| program | windows | gala | |\n|---|---|---|---|");
    let mut different = 0;
    for (name, win, gal, verdict) in report::compare(&w, &g) {
        let mark = match verdict {
            report::Verdict::Same => "same",
            report::Verdict::Different => {
                different += 1;
                "DIFFERENT"
            }
            report::Verdict::Missing => "missing",
        };
        println!("| {name} | {} | {} | {mark} |", show(&win), show(&gal));
    }
    if different == 0 { Ok(()) } else { Err(format!("{different} programs differ")) }
}

/// `boot_scenario` without printing logs (for reports).
/// How long a boot took: until the kernel's banner appeared on the serial
/// console (firmware and bootloader), and in total.
#[derive(Clone, Copy, Default)]
struct BootTime {
    to_banner: Duration,
    total: Duration,
}

/// The kernel's first serial line (see `kernel_main`).
const KERNEL_BANNER: &str = "gala-os kernel";

/// Boots `iso` (with `boot` firmware) until QEMU exits or `timeout`;
/// returns the exit status (None on timeout), the serial log and timing.
fn run_vm(
    iso: &Path,
    boot: Boot,
    timeout: Duration,
) -> Result<(Option<std::process::ExitStatus>, String, BootTime)> {
    let log = iso.with_extension("serial.log");
    let _ = fs::remove_file(&log);
    let mut cmd = qemu(iso, boot)?;
    cmd.args(["-display", "none", "-monitor", "none"])
        .arg("-serial")
        .arg(format!("file:{}", log.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn QEMU: {e}"))?;
    let start = Instant::now();
    let mut to_banner = None;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if to_banner.is_none()
            && fs::read(&log).is_ok_and(|b| {
                b.windows(KERNEL_BANNER.len()).any(|w| w == KERNEL_BANNER.as_bytes())
            })
        {
            to_banner = Some(start.elapsed());
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let total = start.elapsed();
    let mut serial = String::new();
    if let Ok(mut f) = fs::File::open(&log) {
        let _ = f.read_to_string(&mut serial);
    }
    Ok((status, serial, BootTime { to_banner: to_banner.unwrap_or(total), total }))
}

/// Judges one scenario from the exit status and its part of the log.
fn judge(scenario: &Scenario, status: Option<i32>, timed_out: bool, serial: &str) -> Result {
    let missing: Vec<&str> =
        scenario.expect.iter().copied().filter(|e| !serial.contains(e)).collect();
    match status {
        _ if timed_out => Err(format!("timed out after {}s", BOOT_TEST_TIMEOUT.as_secs())),
        s if s != Some(scenario.exit_status) => {
            Err(format!("QEMU exited with {s:?}, expected status {}", scenario.exit_status))
        }
        _ if !missing.is_empty() => Err(format!("serial log lacks {missing:?}")),
        _ if scenario.exit_status == QEMU_EXIT_SUCCESS && serial.contains(LEAK_MARKER) => {
            Err("a process address space was not freed".into())
        }
        _ => Ok(()),
    }
}

/// The part of a shared VM's log that belongs to `program`, and whether it
/// ran to its end.
fn program_section<'a>(serial: &'a str, program: &str) -> Option<(&'a str, bool)> {
    let start = find_line(serial, &format!("run: begin {program}"))?;
    let rest = &serial[start..];
    Some(match find_line(rest, &format!("run: end {program}")) {
        Some(i) => (&rest[..i], true),
        None => (rest, false),
    })
}

/// Where the first line ending in `text` (CRLF or LF) is in `serial`.
fn find_line(serial: &str, text: &str) -> Option<usize> {
    serial
        .match_indices(text)
        .find(|(i, _)| matches!(serial.as_bytes().get(i + text.len()), Some(b'\r' | b'\n')))
        .map(|(i, _)| i)
}

/// Boots one VM and judges each of its scenarios.
fn boot_vm<'a>(vm: &Vm<'a>, iso: &Path) -> (BootTime, Vec<(&'a Scenario, Result)>) {
    let (scenarios, timeout): (Vec<&Scenario>, Duration) = match vm {
        Vm::Single(s) => (vec![*s], BOOT_TEST_TIMEOUT),
        Vm::Batch(group) => {
            (group.clone(), BOOT_TEST_TIMEOUT + Duration::from_secs(20) * group.len() as u32)
        }
    };
    let boot = scenario_boot(scenarios[0].name);
    let (status, serial, time) = match run_vm(iso, boot, timeout) {
        Ok(r) => r,
        Err(e) => {
            return (BootTime::default(), scenarios.iter().map(|&s| (s, Err(e.clone()))).collect());
        }
    };
    let code = status.and_then(|s| s.code());
    let verdicts: Vec<(&Scenario, Result)> = match vm {
        Vm::Single(s) => vec![(*s, judge(s, code, status.is_none(), &serial))],
        Vm::Batch(group) => group
            .iter()
            .map(|&s| {
                let program = batchable(s).map_or("", |(p, _)| p);
                // Boot-time lines (before the first program) belong to all.
                let preamble = &serial[..serial.find("run: begin ").unwrap_or(0)];
                let verdict = match program_section(&serial, program) {
                    // Finished: judge its section as if it had its own VM.
                    Some((section, true)) => {
                        judge(s, Some(QEMU_EXIT_SUCCESS), false, &format!("{preamble}{section}"))
                    }
                    // Started but the VM ended (or hung) in it.
                    Some((section, false)) => {
                        judge(s, code, status.is_none(), &format!("{preamble}{section}"))
                            .and(Err("the shared VM ended during this program".into()))
                    }
                    None => Err("did not run (the shared VM ended before it)".into()),
                };
                (s, verdict)
            })
            .collect(),
    };
    let show = env::var_os("XTASK_VERBOSE").is_some() || verdicts.iter().any(|(_, v)| v.is_err());
    if show {
        eprintln!(
            "----- {} serial output ({}) -----\n{}\n-----",
            scenarios.iter().map(|s| s.name).collect::<Vec<_>>().join(", "),
            iso.with_extension("serial.log").display(),
            serial.trim_end()
        );
    }
    (time, verdicts)
}

/// Boots a scenario alone (for reports), without printing its log.
fn boot_scenario_quiet(scenario: &Scenario, iso: &Path) -> Result {
    let (status, serial, _) = run_vm(iso, scenario_boot(scenario.name), BOOT_TEST_TIMEOUT)?;
    judge(scenario, status.and_then(|s| s.code()), status.is_none(), &serial)
}

/// Boots an ISO with the given kernel command line (all test programs and
/// test DLLs as boot modules, ntdll in System32) and saves the screen as
/// PNG at each requested time, through QEMU's monitor (`screendump`).
fn screenshot(release: bool, args: &[String]) -> Result {
    let mut boot = Boot::Bios;
    let mut times = vec![3.0f64];
    let mut name = String::from("gala");
    let mut cmdline = Vec::new();
    let mut it = args.iter().filter(|a| *a != "--release");
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--uefi" => boot = Boot::Uefi,
            "--at" => {
                let list = it.next().ok_or("--at needs a list of seconds")?;
                times = list
                    .split(',')
                    .map(|t| t.parse().map_err(|_| format!("bad time {t:?}")))
                    .collect::<Result<_>>()?;
            }
            "--name" => name = it.next().ok_or("--name needs a value")?.clone(),
            _ => cmdline.push(arg.as_str()),
        }
    }
    build_kernel(release)?;
    let modules: Vec<(&str, &str)> = [("ntdll.dll", "Windows/System32")]
        .into_iter()
        .chain(USER_DLLS.iter().filter(|d| d.file != "ntdll.dll").map(|d| (d.file, "gala")))
        .collect();
    let iso = make_iso(release, &format!("shot-{name}"), &cmdline.join(" "), &modules)?;
    let dir = target_dir().join("screenshots");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    let socket = iso.with_extension("monitor.sock");
    let _ = fs::remove_file(&socket);
    let mut cmd = qemu(&iso, boot)?;
    cmd.args(["-display", "none", "-serial", "null", "-monitor"])
        .arg(format!("unix:{},server,nowait", socket.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn QEMU: {e}"))?;
    let start = Instant::now();
    let result = (|| {
        let mut monitor = None;
        for _ in 0..100 {
            if let Ok(s) = std::os::unix::net::UnixStream::connect(&socket) {
                monitor = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut monitor = monitor.ok_or("cannot connect to the QEMU monitor")?;
        for &t in &times {
            let at = Duration::from_secs_f64(t);
            if let Some(wait) = at.checked_sub(start.elapsed()) {
                thread::sleep(wait);
            }
            let ppm = dir.join(format!("{name}-{t}s.ppm"));
            let _ = fs::remove_file(&ppm);
            use std::io::Write;
            writeln!(monitor, "screendump {}", ppm.display()).map_err(|e| e.to_string())?;
            for _ in 0..100 {
                if fs::metadata(&ppm).is_ok_and(|m| m.len() > 0) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            thread::sleep(Duration::from_millis(100));
            let png = dir.join(format!("{name}-{t}s.png"));
            ppm_to_png(&ppm, &png)?;
            let _ = fs::remove_file(&ppm);
            eprintln!("xtask: wrote {}", png.display());
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Converts a binary PPM (P6, 8-bit) to PNG (stored, uncompressed deflate
/// blocks: no dependencies, files are larger but every viewer reads them).
fn ppm_to_png(ppm: &Path, png: &Path) -> Result {
    let data = fs::read(ppm).map_err(|e| format!("reading {ppm:?}: {e}"))?;
    // Header: "P6" whitespace width whitespace height whitespace 255 single whitespace.
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while data.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let start = i;
        while data.get(i).is_some_and(|b| !b.is_ascii_whitespace()) {
            i += 1;
        }
        if start == i {
            return Err(format!("{ppm:?}: truncated PPM header"));
        }
        fields.push(String::from_utf8_lossy(&data[start..i]).into_owned());
    }
    let (w, h): (usize, usize) = match (fields[0].as_str(), fields[1].parse(), fields[2].parse()) {
        ("P6", Ok(w), Ok(h)) if fields[3] == "255" => (w, h),
        _ => return Err(format!("{ppm:?}: not an 8-bit P6 PPM")),
    };
    let pixels = data.get(i + 1..i + 1 + w * h * 3).ok_or(format!("{ppm:?}: truncated PPM"))?;
    // Scanlines with filter byte 0.
    let mut raw = Vec::with_capacity(h * (w * 3 + 1));
    for row in pixels.chunks(w * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    for (n, block) in raw.chunks(65535).enumerate() {
        let last = (n + 1) * 65535 >= raw.len();
        zlib.push(u8::from(last));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    for (kind, body) in [(b"IHDR", &ihdr), (b"IDAT", &zlib), (b"IEND", &Vec::new())] {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    fs::write(png, out).map_err(|e| format!("writing {png:?}: {e}"))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The image `docker-test` builds and runs.
const DOCKER_IMAGE: &str = "gala-os-test";

/// Builds the test image and runs xtask inside it with the repository
/// mounted (`args` default to `test`), using KVM when the host has it.
fn docker_test(args: &[String]) -> Result {
    // Behind a proxy (as in sandboxes): forward its settings, trust its CA,
    // and share the host's network when the proxy listens on localhost.
    const PROXY_VARS: [&str; 6] =
        ["http_proxy", "https_proxy", "no_proxy", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"];
    let proxies: Vec<(&str, String)> =
        PROXY_VARS.iter().filter_map(|&v| env::var(v).ok().map(|value| (v, value))).collect();
    let host_network =
        proxies.iter().any(|(_, v)| v.contains("127.0.0.1") || v.contains("localhost"));
    let ca_dir = root().join("docker/extra-ca");
    if let Some(ca) = env::var_os("XTASK_DOCKER_CA").or_else(|| env::var_os("SSL_CERT_FILE")) {
        copy(Path::new(&ca), &ca_dir.join("extra.crt"))?;
    }
    let mut build = Command::new("docker");
    build
        .args(["build", "--quiet", "-t", DOCKER_IMAGE, "-f"])
        .arg(root().join("docker/test.Dockerfile"));
    if host_network {
        build.args(["--network", "host"]);
    }
    for (var, value) in &proxies {
        build.arg("--build-arg").arg(format!("{var}={value}"));
    }
    run_cmd(build.arg(root().join("docker")))?;
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "-v"]).arg(format!("{}:/work", root().display()));
    if host_network {
        cmd.args(["--network", "host"]);
    }
    for (var, value) in &proxies {
        cmd.arg("-e").arg(format!("{var}={value}"));
    }
    if Path::new("/dev/kvm").exists() {
        cmd.args(["--device", "/dev/kvm"]);
    }
    for var in ["XTASK_VERBOSE", "XTASK_QEMU_ARGS", "XTASK_FIRMWARE", "XTASK_NO_BATCH"] {
        if let Ok(value) = env::var(var) {
            cmd.arg("-e").arg(format!("{var}={value}"));
        }
    }
    cmd.args([DOCKER_IMAGE, "cargo", "xtask"]);
    if args.is_empty() {
        cmd.arg("test");
    } else {
        cmd.args(args);
    }
    run_cmd(&mut cmd)
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

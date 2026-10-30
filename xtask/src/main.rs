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
/// Must match `BOOT_OK_MARKER` in `kernel/src/lib.rs`.
const BOOT_OK_MARKER: &str = "gala: boot ok";
/// QEMU exit status for `QemuExitCode::Success` (`(0x10 << 1) | 1`).
const QEMU_EXIT_SUCCESS: i32 = 33;
const BOOT_TEST_TIMEOUT: Duration = Duration::from_secs(120);

type Result<T = ()> = std::result::Result<T, String>;

const USAGE: &str = "\
usage: cargo xtask <command> [--release]

commands:
  build        build the kernel and a bootable UEFI ISO (target/<profile>/gala-os.iso)
  run          build, then boot in QEMU with serial on stdio [--headless]
  test         run host unit tests, then boot in QEMU and check the result
  boot-test    only the QEMU boot test
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|a| a == "--release");
    let headless = args.iter().any(|a| a == "--headless");
    let result = match args.first().map(String::as_str) {
        Some("build") => build(release, Mode::Interactive).map(drop),
        Some("run") => build(release, Mode::Interactive).and_then(|iso| run(&iso, headless)),
        Some("test") => unit_tests().and_then(|()| boot_test(release)),
        Some("boot-test") => boot_test(release),
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

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Interactive,
    /// Kernel reports its result through QEMU's isa-debug-exit device.
    Test,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn cargo() -> Command {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

fn run_cmd(cmd: &mut Command) -> Result {
    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn {cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed: {status}"))
    }
}

fn unit_tests() -> Result {
    eprintln!("xtask: running kernel unit tests on the host");
    run_cmd(cargo().args(["test", "--package", "kernel", "--lib"]))
}

/// Builds the kernel and packs it with Limine into a UEFI-bootable ISO.
fn build(release: bool, mode: Mode) -> Result<PathBuf> {
    let profile = if release { "release" } else { "debug" };
    let mut cmd = cargo();
    cmd.args([
        "build",
        "--package",
        "kernel",
        "--bin",
        "kernel",
        "--target",
        KERNEL_TARGET,
    ]);
    if release {
        cmd.arg("--release");
    }
    run_cmd(&mut cmd)?;
    let kernel = target_dir()
        .join(KERNEL_TARGET)
        .join(profile)
        .join("kernel");

    let limine = fetch_limine()?;
    let name = match mode {
        Mode::Interactive => "gala-os",
        Mode::Test => "gala-os-test",
    };
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
    copy(&limine.join("BOOTX64.EFI"), &efi_boot.join("BOOTX64.EFI"))?;
    copy(
        &limine.join("limine-uefi-cd.bin"),
        &boot.join("limine/limine-uefi-cd.bin"),
    )?;
    let cmdline = match mode {
        Mode::Interactive => "",
        Mode::Test => "qemu_exit",
    };
    let config = format!(
        "timeout: 0\n\n/gala-os\n    protocol: limine\n    path: boot():/boot/kernel\n    cmdline: {cmdline}\n"
    );
    write(&boot.join("limine/limine.conf"), &config)?;

    let iso = out.join(format!("{name}.iso"));
    run_cmd(
        Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-r", "-J", "-quiet"])
            .args(["--efi-boot", "boot/limine/limine-uefi-cd.bin"])
            .args([
                "-efi-boot-part",
                "--efi-boot-image",
                "--protective-msdos-label",
            ])
            .arg(&iso_root)
            .arg("-o")
            .arg(&iso),
    )?;
    eprintln!("xtask: built {}", iso.display());
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
                .args([
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--branch",
                    LIMINE_TAG,
                    LIMINE_REPO,
                ])
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
        (
            "/usr/share/OVMF/OVMF_CODE_4M.fd",
            "/usr/share/OVMF/OVMF_VARS_4M.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ),
        (
            "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
            "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
        ),
        (
            "/usr/share/edk2/ovmf/OVMF_CODE.fd",
            "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        ),
        (
            "/usr/share/qemu/edk2-x86_64-code.fd",
            "/usr/share/qemu/edk2-i386-vars.fd",
        ),
        (
            "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
            "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
        ),
    ];
    if let Some(code) = env::var_os("OVMF_CODE") {
        let vars = env::var_os("OVMF_VARS").map(PathBuf::from);
        return Ok(Firmware {
            code: code.into(),
            vars,
        });
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
        .arg(format!(
            "if=pflash,format=raw,unit=0,readonly=on,file={}",
            fw.code.display()
        ));
    if let Some(vars) = fw.vars {
        // The firmware writes its variable store; give it a private copy.
        let copy_path = iso.with_extension("ovmf-vars.fd");
        copy(&vars, &copy_path)?;
        cmd.arg("-drive").arg(format!(
            "if=pflash,format=raw,unit=1,file={}",
            copy_path.display()
        ));
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

/// Boots the test ISO headless and checks the kernel's verdict.
fn boot_test(release: bool) -> Result {
    let iso = build(release, Mode::Test)?;
    let log = iso.with_extension("serial.log");
    let mut cmd = qemu(&iso)?;
    cmd.args(["-display", "none", "-monitor", "none"])
        .arg("-serial")
        .arg(format!("file:{}", log.display()))
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .stdin(Stdio::null());
    eprintln!(
        "xtask: booting {} (timeout {}s)",
        iso.display(),
        BOOT_TEST_TIMEOUT.as_secs()
    );
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn QEMU: {e}"))?;
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
    eprintln!(
        "----- serial output -----\n{}\n-------------------------",
        serial.trim_end()
    );

    let marker = serial.lines().any(|l| l.trim_end() == BOOT_OK_MARKER);
    match status {
        None => Err(format!(
            "boot test timed out after {}s",
            BOOT_TEST_TIMEOUT.as_secs()
        )),
        Some(s) if s.code() == Some(QEMU_EXIT_SUCCESS) && marker => {
            eprintln!(
                "xtask: boot test passed in {:.1}s",
                start.elapsed().as_secs_f64()
            );
            Ok(())
        }
        Some(s) if s.code() == Some(QEMU_EXIT_SUCCESS) => {
            Err("kernel reported success but the boot marker is missing".into())
        }
        Some(s) => Err(format!("boot test failed: QEMU exited with {s}")),
    }
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("copying {from:?} to {to:?}: {e}"))
}

fn write(path: &Path, contents: &str) -> Result {
    fs::write(path, contents).map_err(|e| format!("writing {path:?}: {e}"))
}

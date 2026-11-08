//! `cargo xtask lx-expect`: runs the Linux test programs (user/lx, ROADMAP
//! LX) on the host, which must be Linux x86_64, and compares each
//! transcript with the expected.txt next to its source: the expected
//! results are real Linux's, recorded with `--record`.
//!
//! - With musl-gcc on the PATH (or `MUSL_GCC`), `user/lx/build.sh`
//!   rebuilds the binaries into `user/lx/bin` first (the build is
//!   reproducible; CI then checks `git diff --exit-code user/lx/bin`).
//!   Without it, the checked-in binaries are run. `--no-build` skips the
//!   build.
//! - The glibc tests (build.sh's GLIBC_TESTS, dynamically linked) are
//!   built with the host's gcc in the same run; their shared libraries
//!   (`user/lx/bin/lib*.so`) are copied beside each binary run.
//! - Each `user/lx/<name>_test` has two binaries: `<name>_test` (static,
//!   ET_EXEC) and `<name>_test-pie` (static-pie, ET_DYN). Both must give
//!   the same transcript. Each is copied to
//!   `target/lx-expect/<binary>/<name>_test` and run there as
//!   `./<name>_test` (so argv[0] and AT_EXECFN are the same for both) with
//!   [`ARGS`] and only [`ENV`] in its environment.
//! - The transcript is stdout, then `-- stderr:` and stderr's lines if
//!   any, then `-- exit status N` or `-- killed by signal N`. A mismatch
//!   prints a line diff, leaves `actual.txt` in the run directory and
//!   fails the command.
//! - The programs start with descriptors 0-2 only, in a fresh-size
//!   descriptor table: xtask first closes whatever it inherited (a CI
//!   runner leaves descriptors above 100 open), because fork sizes the
//!   child's table by the parent's highest open descriptor and Linux's
//!   answers depend on that size (select ignores descriptors beyond it).
//! - `--runs N` runs each binary N times (default 1) and fails if the
//!   transcripts differ between runs: a check that a test is
//!   deterministic, made before recording (WORKFLOW "Linux test
//!   programs").

use std::fs;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Result, root, target_dir};

/// The arguments every test program gets (after argv[0]); auxv_test
/// prints them and the kernel's stack-layout tests use the same ones.
pub const ARGS: &[&str] = &["first", "", "third arg"];

/// The whole environment every test program gets.
/// Sorted by name: Rust's `Command` passes it in that order whatever the
/// order here.
pub const ENV: &[(&str, &str)] = &[("A", "1"), ("EMPTY", ""), ("LONGER_NAME", "some value")];

/// How long one program may run.
const TIMEOUT: Duration = Duration::from_secs(20);

pub fn lx_expect(args: &[String]) -> Result {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("lx-expect runs the Linux test programs natively: it needs Linux x86_64".into());
    }
    let record = args.iter().any(|a| a == "--record");
    let no_build = args.iter().any(|a| a == "--no-build");
    let runs = runs_arg(args)?;
    close_inherited();
    let lx = root().join("user/lx");
    if !no_build {
        build(&lx)?;
    }
    let tests = tests(&lx)?;
    if tests.is_empty() {
        return Err(format!("no tests in {}", lx.display()));
    }
    let mut failed = Vec::new();
    for test in &tests {
        let expected_path = lx.join(test).join("expected.txt");
        let mut expected = fs::read_to_string(&expected_path).ok();
        for binary in [test.clone(), format!("{test}-pie")] {
            let exe = lx.join("bin").join(&binary);
            if !exe.exists() {
                failed.push(format!("{binary}: no binary {} (user/lx/build.sh)", exe.display()));
                continue;
            }
            let dir = target_dir().join("lx-expect").join(&binary);
            let actual = run(&exe, &dir, test)?;
            let mut varied = false;
            for i in 1..runs {
                let again = run(&exe, &dir, test)?;
                if again != actual {
                    let path = dir.join("varied.txt");
                    let _ = fs::write(&path, &again);
                    eprintln!(
                        "lx-expect: {binary}: run {} differs from run 1 (- run 1, + run {}; all of it in {}):",
                        i + 1,
                        i + 1,
                        path.display()
                    );
                    eprint!("{}", diff(&actual, &again));
                    varied = true;
                    break;
                }
            }
            if varied {
                failed.push(format!("{binary}: not deterministic over {runs} runs"));
                continue;
            }
            if record && binary == *test {
                fs::write(&expected_path, &actual)
                    .map_err(|e| format!("writing {}: {e}", expected_path.display()))?;
                eprintln!("lx-expect: {binary}: recorded {}", expected_path.display());
                expected = Some(actual);
                continue;
            }
            let Some(want) = expected.as_deref() else {
                failed.push(format!(
                    "{binary}: no {} (record it: --record)",
                    expected_path.display()
                ));
                continue;
            };
            if want == actual {
                eprintln!("lx-expect: {binary}: ok ({} lines)", actual.lines().count());
            } else {
                let actual_path = dir.join("actual.txt");
                let _ = fs::write(&actual_path, &actual);
                eprintln!(
                    "lx-expect: {binary}: MISMATCH with {} (- expected, + actual; all of it in {}):",
                    expected_path.display(),
                    actual_path.display()
                );
                eprint!("{}", diff(want, &actual));
                failed.push(format!("{binary}: transcript differs"));
            }
        }
    }
    if failed.is_empty() {
        eprintln!("lx-expect: {} programs, {} binaries: all match", tests.len(), tests.len() * 2);
        Ok(())
    } else {
        Err(format!("lx-expect: {} failed:\n  {}", failed.len(), failed.join("\n  ")))
    }
}

/// `--runs N` (or `--runs=N`): how many times each binary runs.
fn runs_arg(args: &[String]) -> Result<usize> {
    let mut runs = 1;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = if a == "--runs" {
            it.next().map(String::as_str)
        } else if let Some(v) = a.strip_prefix("--runs=") {
            Some(v)
        } else {
            continue;
        };
        runs = value
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .ok_or_else(|| format!("--runs takes a count >= 1, not {value:?}"))?;
    }
    Ok(runs)
}

/// Rebuilds user/lx/bin when musl-gcc is there.
fn build(lx: &Path) -> Result {
    let cc = std::env::var("MUSL_GCC").unwrap_or_else(|_| "musl-gcc".into());
    let found = Command::new(&cc)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !found {
        eprintln!(
            "lx-expect: {cc} not found (apt install musl-tools): running the checked-in binaries"
        );
        return Ok(());
    }
    let status = Command::new(lx.join("build.sh"))
        .env("MUSL_GCC", &cc)
        .status()
        .map_err(|e| format!("running user/lx/build.sh: {e}"))?;
    if !status.success() {
        return Err(format!("user/lx/build.sh failed ({status})"));
    }
    eprintln!("lx-expect: rebuilt user/lx/bin with {cc}");
    Ok(())
}

/// The `<name>_test` directories with a main.c, sorted.
fn tests(lx: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(lx).map_err(|e| format!("reading {}: {e}", lx.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_test") && entry.path().join("main.c").exists() {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// Runs one binary as `./<name>` in `dir` and returns its transcript.
/// Closes every descriptor above 2, which at this point are only the ones
/// xtask inherited (none of its own is open yet), and says which: the
/// programs' fork then sizes their descriptor table from 0-2 and the pipes
/// of their standard streams, as a login shell's would.
fn close_inherited() {
    let fds: Vec<i32> = fs::read_dir("/proc/self/fd")
        .map(|d| {
            d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .filter(|&fd| fd > 2)
                .collect()
        })
        .unwrap_or_default();
    // The directory's own descriptor is closed by now; it was in the list.
    let inherited: Vec<i32> =
        fds.into_iter().filter(|&fd| Path::new(&format!("/proc/self/fd/{fd}")).exists()).collect();
    #[cfg(target_os = "linux")]
    // SAFETY: close_range only closes descriptors; none above 2 belongs to
    // anything xtask holds at this point (see the function's comment).
    unsafe {
        libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32);
    }
    if let Some(max) = inherited.iter().max() {
        eprintln!(
            "lx-expect: closed {} inherited descriptor(s) (highest {max}), so each program starts with 0-2 only",
            inherited.len()
        );
    }
}

fn run(exe: &Path, dir: &PathBuf, name: &str) -> Result<String> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let copy = dir.join(name);
    fs::copy(exe, &copy)
        .map_err(|e| format!("copying {} to {}: {e}", exe.display(), copy.display()))?;
    // The glibc tests' shared libraries (user/lx/bin/lib*.so) beside it:
    // the programs find them through DT_RUNPATH $ORIGIN.
    if let Some(bin) = exe.parent() {
        for lib in shared_libraries(bin)? {
            let to = dir.join(lib.file_name().unwrap_or_default());
            fs::copy(&lib, &to)
                .map_err(|e| format!("copying {} to {}: {e}", lib.display(), to.display()))?;
        }
    }
    let mut child = Command::new(format!("./{name}"))
        .current_dir(dir)
        .args(ARGS)
        .env_clear()
        .envs(ENV.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting {}: {e}", copy.display()))?;
    // Small outputs: read them after the exit (a pipe holds 64 KiB).
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("waiting for {name}: {e}"))? {
            break status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{}: still running after {TIMEOUT:?}: killed", copy.display()));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout).map_err(|e| format!("{name}'s stdout: {e}"))?;
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_string(&mut stderr).map_err(|e| format!("{name}'s stderr: {e}"))?;
    }
    Ok(transcript(&stdout, &stderr, status.code(), status.signal()))
}

/// The shared libraries in `bin` (`lib*.so`), sorted.
fn shared_libraries(bin: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(bin).map_err(|e| format!("reading {}: {e}", bin.display()))?;
    let mut libs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("lib") && n.ends_with(".so"))
        })
        .collect();
    libs.sort();
    Ok(libs)
}

/// stdout, stderr and how the program ended, as expected.txt holds them.
fn transcript(stdout: &str, stderr: &str, code: Option<i32>, signal: Option<i32>) -> String {
    let mut out = String::from(stdout);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str("\n-- (no newline at the end)\n");
    }
    if !stderr.is_empty() {
        out.push_str("-- stderr:\n");
        out.push_str(stderr);
        if !stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    match (code, signal) {
        (Some(code), _) => out.push_str(&format!("-- exit status {code}\n")),
        (None, Some(signal)) => out.push_str(&format!("-- killed by signal {signal}\n")),
        (None, None) => out.push_str("-- ended in an unknown way\n"),
    }
    out
}

/// The transcript of the Linux program a boot ran (`lxrun=`), from the
/// kernel's `lx: pid <p> stdout: <line>`, `.. stderr: ..` and `lx: pid <p>
/// -- exit status <n>` lines (kernel/src/lx/state.rs), in expected.txt's
/// format; None if the log has no end line.
pub fn transcript_from_log(serial: &str) -> Option<String> {
    let (mut stdout, mut stderr, mut end) = (String::new(), String::new(), None);
    for line in serial.lines() {
        let Some((_, rest)) = line.split_once("lx: pid ") else { continue };
        let Some((_pid, rest)) = rest.split_once(' ') else { continue };
        if let Some(text) = rest.strip_prefix("stdout: ") {
            stdout += text;
            stdout.push('\n');
        } else if let Some(text) = rest.strip_prefix("stderr: ") {
            stderr += text;
            stderr.push('\n');
        } else if rest.starts_with("-- ") {
            end = Some(rest.to_string());
        }
    }
    let end = end?;
    let mut out = stdout;
    if !stderr.is_empty() {
        out += "-- stderr:\n";
        out += &stderr;
    }
    out += &end;
    out.push('\n');
    Some(out)
}

/// For a boot scenario that runs a user/lx program (`lxrun=/lx/<binary>`):
/// whether the transcript in its serial log is the program's expected.txt
/// (what real Linux printed), exactly. A program that is not a user/lx
/// test (busybox) has one transcript per scenario:
/// `user/lx/<binary>/<scenario>.txt`. Ok for other scenarios.
pub fn check_boot(scenario: &str, cmdline: &str, serial: &str) -> std::result::Result<(), String> {
    let Some(path) = cmdline.split_whitespace().find_map(|w| w.strip_prefix("lxrun=")) else {
        return Ok(());
    };
    let binary = path.rsplit('/').next().unwrap_or(path);
    let test = binary.strip_suffix("-pie").unwrap_or(binary);
    let expected_path = if test.ends_with("_test") {
        root().join("user/lx").join(test).join("expected.txt")
    } else {
        root().join("user/lx").join(test).join(format!("{scenario}.txt"))
    };
    let want = fs::read_to_string(&expected_path)
        .map_err(|e| format!("reading {}: {e}", expected_path.display()))?;
    let Some(got) = transcript_from_log(serial) else {
        return Err(format!("{binary}: no `lx: pid N -- exit status` line in the log"));
    };
    if got == want {
        return Ok(());
    }
    Err(format!(
        "{binary}'s transcript differs from {} (- expected, + actual):\n{}",
        expected_path.display(),
        diff(&want, &got)
    ))
}

/// A line diff (longest common subsequence): ` ` kept, `-` expected only,
/// `+` actual only.
fn diff(expected: &str, actual: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    // lcs[i][j]: the common subsequence's length of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] =
                if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = String::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push_str(&format!("  {}\n", a[i]));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_from_a_boot_log() {
        let log = "[info ] process hello_test (pid 4, session 1): starting at 0x401000\n\
                   [info ] lx: pid 4 stdout: hello\n\
                   [info ] lx: pid 4 stderr: oops\n\
                   [info ] lx: pid 4 stdout: \n\
                   [info ] lx: pid 4 -- exit status 44\n";
        assert_eq!(
            transcript_from_log(log).unwrap(),
            "hello\n\n-- stderr:\noops\n-- exit status 44\n"
        );
        assert_eq!(transcript_from_log("[info ] lx: pid 4 stdout: x\n"), None);
    }

    #[test]
    fn diff_marks_changed_lines() {
        let d = diff("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(d, "  a\n- b\n+ B\n  c\n+ d\n");
        assert_eq!(diff("x\n", "x\n"), "  x\n");
    }

    #[test]
    fn runs_argument() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(runs_arg(&args(&[])).unwrap(), 1);
        assert_eq!(runs_arg(&args(&["--record", "--runs", "5"])).unwrap(), 5);
        assert_eq!(runs_arg(&args(&["--runs=3"])).unwrap(), 3);
        assert!(runs_arg(&args(&["--runs", "0"])).is_err());
        assert!(runs_arg(&args(&["--runs"])).is_err());
    }

    #[test]
    fn transcript_records_how_it_ended() {
        assert_eq!(transcript("hi\n", "", Some(44), None), "hi\n-- exit status 44\n");
        assert_eq!(
            transcript("x", "oops", None, Some(11)),
            "x\n-- (no newline at the end)\n-- stderr:\noops\n-- killed by signal 11\n"
        );
    }

    /// Every test program has its expected transcript checked in.
    #[test]
    fn every_test_has_expected_output_and_binaries() {
        let lx = root().join("user/lx");
        let tests = tests(&lx).unwrap();
        assert!(tests.len() >= 15, "{tests:?}");
        for t in tests {
            assert!(lx.join(&t).join("expected.txt").exists(), "{t}: no expected.txt");
            assert!(lx.join("bin").join(&t).exists(), "{t}: no binary in user/lx/bin");
            assert!(lx.join("bin").join(format!("{t}-pie")).exists(), "{t}: no -pie binary");
        }
    }
}

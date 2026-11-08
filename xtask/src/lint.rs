//! `cargo xtask lint`: the analyzers that run before every commit (ROADMAP
//! LN; the committed pre-commit hook in `hooks/`). Each proves a property
//! over the source by parsing it (`syn`), so a class of defect we have had
//! cannot come back unnoticed; the scenarios then only verify. Each
//! analyzer has fixtures (code it must flag, code it must not) in the
//! tests below.
//!
//! - `event-reset`: in gala's DLLs, an event is reset (`NtClearEvent`,
//!   `NtResetEvent`, `ResetEvent`) only by the functions that implement
//!   those APIs. A reset after a kernel request that already reported
//!   split an operation Windows documents as atomic: WSAEnumNetworkEvents
//!   reset its event after AFD's copy, and a connection recorded between
//!   the two lost its signal (Chromium's ~240 s page stalls). A site that
//!   needs one says why: `// lint(event-reset): <reason>`.
//! - `wait-site`: the kernel's raw `prepare_wait` only where a wait was
//!   reviewed to register and test its condition under the lock its
//!   waker takes (WAIT_SITES); anything else waits through those. A new
//!   site is added to the list with the review.

use crate::Result;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// One finding: where, which analyzer, what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub lint: &'static str,
    pub message: String,
}

/// The functions allowed to reset an event: the APIs that do just that.
const EVENT_RESETTERS: &[&str] = &["ResetEvent", "WSAResetEvent", "NtClearEvent", "NtResetEvent"];
const EVENT_RESETS: &[&str] = &["NtClearEvent", "NtResetEvent", "ResetEvent"];

/// The reviewed callers of the kernel's raw `prepare_wait` (file under
/// kernel/src, function): each registers and tests its condition under
/// the lock its waker takes before it releases it and waits.
const WAIT_SITES: &[(&str, &str)] = &[
    // A pipe end's waiters list, under the pipe's state lock.
    ("nt/pipe.rs", "wait"),
    // Byte-range locks: the conflict test and the waiter under one lock.
    ("nt/locks.rs", "lock"),
    // Suspension: the suspend count's lock held across the test.
    ("nt/thread.rs", "thread_entry"),
    ("nt/thread.rs", "on_user_return"),
    // A handover's hold: the flag tested again after prepare_wait.
    ("nt/thread.rs", "hold"),
    // The timer thread: a deadline, re-armed by every new timer.
    ("nt/timer.rs", "worker"),
    // The dispatcher wait every NT wait goes through.
    ("nt/wait.rs", "sleep"),
    // Linux waits (futex, poll), the same way.
    ("lx/wait.rs", "prepare"),
    ("lx/fork.rs", "wait_for_exec"),
    // The net thread: KICKED tested after prepare_wait.
    ("net/tcp.rs", "net_thread"),
    // The wrappers themselves and the scheduler.
    ("nt/platform.rs", "prepare_wait"),
    ("platform.rs", "prepare_wait"),
    ("sched/mod.rs", "*"),
    // gala-um's host side and simulator.
    ("hosted/mod.rs", "*"),
    ("hosted/sim.rs", "*"),
    ("hosted/threads.rs", "*"),
];

/// Walks a function body for calls, remembering the enclosing function and
/// whether the call's line carries the justification comment.
struct Calls<'a> {
    file: &'a str,
    source: &'a str,
    function: Vec<String>,
    found: Vec<(String, String, usize)>, // (enclosing fn, callee, line)
}

impl<'ast> Visit<'ast> for Calls<'_> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.function.push(f.sig.ident.to_string());
        syn::visit::visit_item_fn(self, f);
        self.function.pop();
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.function.push(f.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, f);
        self.function.pop();
    }
    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func
            && let Some(last) = p.path.segments.last()
        {
            let line = c.span().start().line;
            let function = self.function.last().cloned().unwrap_or_default();
            self.found.push((function, last.ident.to_string(), line));
        }
        syn::visit::visit_expr_call(self, c);
    }
}

/// Whether line `line` (1-based) or the line above carries
/// `// lint(<name>): <reason>` with a reason.
fn justified(source: &str, line: usize, name: &str) -> bool {
    let tag = format!("lint({name}):");
    let lines: Vec<&str> = source.lines().collect();
    [line.wrapping_sub(1), line.wrapping_sub(2)].iter().any(|&i| {
        lines.get(i).is_some_and(|l| {
            l.split_once(&tag).is_some_and(|(_, reason)| !reason.trim().is_empty())
        })
    })
}

fn calls(file: &str, source: &str) -> std::result::Result<Vec<(String, String, usize)>, String> {
    let ast = syn::parse_file(source).map_err(|e| format!("{file}: {e}"))?;
    let mut v = Calls { file, source, function: Vec::new(), found: Vec::new() };
    v.visit_file(&ast);
    let _ = (v.file, v.source);
    Ok(v.found)
}

/// `event-reset` over one DLL source file.
pub fn event_reset(file: &str, source: &str) -> std::result::Result<Vec<Finding>, String> {
    Ok(calls(file, source)?
        .into_iter()
        .filter(|(function, callee, line)| {
            EVENT_RESETS.contains(&callee.as_str())
                && !EVENT_RESETTERS.contains(&function.as_str())
                && !justified(source, *line, "event-reset")
        })
        .map(|(function, callee, line)| Finding {
            file: file.to_string(),
            line,
            lint: "event-reset",
            message: format!(
                "{function} calls {callee}: an event reset after a kernel request splits what \
                 Windows documents as atomic (the kernel resets it with its report); if it must, \
                 say why: `// lint(event-reset): <reason>`"
            ),
        })
        .collect())
}

/// `wait-site` over one kernel source file (`rel`: its path under kernel/src).
pub fn wait_site(rel: &str, source: &str) -> std::result::Result<Vec<Finding>, String> {
    let allowed = |function: &str| {
        WAIT_SITES.iter().any(|&(f, func)| f == rel && (func == "*" || func == function))
    };
    Ok(calls(rel, source)?
        .into_iter()
        .filter(|(function, callee, _)| callee == "prepare_wait" && !allowed(function))
        .map(|(function, _, line)| Finding {
            file: format!("kernel/src/{rel}"),
            line,
            lint: "wait-site",
            message: format!(
                "{function} calls prepare_wait: a raw wait outside the reviewed sites (a test of \
                 the condition must be under the lock its waker takes, or a wake is lost); wait \
                 through an existing primitive, or add the site to WAIT_SITES with its review"
            ),
        })
        .collect())
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result {
    for e in fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            rust_files(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

/// Runs every analyzer over the tree at `root`; the findings, sorted.
pub fn analyze(root: &Path) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    // gala's DLLs and programs (not the test programs, which call any API
    // as a program would, nor ntrt and the stubs that declare them).
    let user = root.join("user");
    for e in fs::read_dir(&user).map_err(|e| format!("reading {}: {e}", user.display()))? {
        let dir = e.map_err(|e| e.to_string())?.path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !dir.join("src").is_dir() || name.ends_with("_test") || name == "ntrt" {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&dir.join("src"), &mut files)?;
        for f in files {
            let source =
                fs::read_to_string(&f).map_err(|e| format!("reading {}: {e}", f.display()))?;
            let rel = f.strip_prefix(root).unwrap_or(&f).display().to_string();
            findings.extend(event_reset(&rel, &source)?);
        }
    }
    let kernel = root.join("kernel/src");
    let mut files = Vec::new();
    rust_files(&kernel, &mut files)?;
    for f in files {
        let rel = f.strip_prefix(&kernel).unwrap_or(&f).display().to_string();
        if rel.starts_with("arch/") {
            continue;
        }
        let source = fs::read_to_string(&f).map_err(|e| format!("reading {}: {e}", f.display()))?;
        findings.extend(wait_site(&rel, &source)?);
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok(findings)
}

/// Points git at the committed hooks (`hooks/pre-commit` runs `cargo xtask
/// lint`), once: any xtask run in a checkout enables it, so every commit,
/// a session's or a sub-agent's, is checked. A checkout without git, or
/// one whose hooks path was set to something else, is left alone.
pub fn enable_hook(root: &Path) {
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(root).output().ok()
    };
    let Some(current) = git(&["config", "--get", "core.hooksPath"]) else { return };
    let current = String::from_utf8_lossy(&current.stdout).trim().to_string();
    if current.is_empty()
        && root.join("hooks/pre-commit").is_file()
        && git(&["config", "core.hooksPath", "hooks"]).is_some_and(|o| o.status.success())
    {
        eprintln!("xtask: lint: git's pre-commit hook enabled (core.hooksPath = hooks)");
    }
}

/// The commands of CI's Clippy step (`.github/workflows/ci.yml`, the
/// `run: |` block under `- name: Clippy`): one list, which CI and
/// `cargo xtask lint --full` both run.
pub fn ci_clippy(workflow: &str) -> Vec<String> {
    let mut lines = workflow.lines();
    let mut out = Vec::new();
    while let Some(l) = lines.next() {
        if l.trim() != "- name: Clippy" {
            continue;
        }
        let Some(run) = lines.next().filter(|r| r.trim() == "run: |") else { break };
        let indent = run.len() - run.trim_start().len();
        for l in lines.by_ref() {
            if !l.trim().is_empty() && l.len() - l.trim_start().len() <= indent {
                break;
            }
            let cmd = l.trim();
            if cmd.starts_with("cargo ") {
                out.push(cmd.to_string());
            }
        }
        break;
    }
    out
}

/// `cargo xtask lint --full`'s part: CI's Clippy commands, each in turn,
/// all of them run; fails naming those that failed.
fn full(root: &Path) -> Result {
    let path = root.join(".github/workflows/ci.yml");
    let text = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let commands = ci_clippy(&text);
    if commands.is_empty() {
        return Err(format!("no Clippy step's commands in {}", path.display()));
    }
    let mut failed = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        eprintln!("xtask: lint --full: [{}/{}] {cmd}", i + 1, commands.len());
        let ok = std::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(root)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            failed.push(cmd.clone());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "lint --full: {} of {} Clippy commands failed: {failed:?}",
            failed.len(),
            commands.len()
        ))
    }
}

/// `cargo xtask lint [--full]`: the analyzers, then `cargo fmt --check`
/// (and with `--full`, CI's Clippy commands: the pre-push hook's run); a
/// non-zero exit (and each finding as `file:line: [lint] message`) if
/// anything fails.
pub fn cli(root: &Path, with_clippy: bool) -> Result {
    let findings = analyze(root)?;
    for f in &findings {
        eprintln!("{}:{}: [{}] {}", f.file, f.line, f.lint, f.message);
    }
    let fmt = std::process::Command::new("cargo")
        .args(["fmt", "--all", "--check"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("cargo fmt: {e}"))?;
    if !findings.is_empty() || !fmt.success() {
        return Err(format!(
            "lint: {} finding(s){}",
            findings.len(),
            if fmt.success() { "" } else { ", and `cargo fmt --all` would change files" }
        ));
    }
    if with_clippy {
        full(root)?;
    }
    eprintln!("xtask: lint: clean");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_clippy_commands_are_the_steps_run_block() {
        let yml = "      - name: Format\n        run: cargo fmt --all --check\n      - name: Clippy\n        run: |\n          cargo clippy -p kernel -- -D warnings\n          # a comment\n          cargo build -p testdll\n\n          cargo clippy -p xtask -- -D warnings\n      - name: Tests\n        run: cargo xtask test\n";
        assert_eq!(
            ci_clippy(yml),
            [
                "cargo clippy -p kernel -- -D warnings",
                "cargo build -p testdll",
                "cargo clippy -p xtask -- -D warnings"
            ]
        );
    }

    #[test]
    fn the_real_workflow_has_clippy_commands() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let yml = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let c = ci_clippy(&yml);
        assert!(c.len() > 10, "{c:?}");
        assert!(c.iter().any(|l| l.contains("aarch64-unknown-none")));
    }

    #[test]
    fn event_reset_after_a_request_is_flagged() {
        // The defect as it was: WSAEnumNetworkEvents' own reset.
        let src = r#"
            pub extern "system" fn WSAEnumNetworkEvents(s: usize, event: usize, out: *mut u8) -> i32 {
                let got = afd_call(s, IOCTL_AFD_GET_EVENTS, &[], &mut []);
                unsafe { NtClearEvent(event) };
                0
            }
        "#;
        let f = event_reset("user/ws2_32/src/lib.rs", src).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lint, "event-reset");
        assert!(f[0].message.contains("WSAEnumNetworkEvents calls NtClearEvent"));
    }

    #[test]
    fn the_reset_apis_themselves_and_justified_sites_pass() {
        let src = r#"
            pub extern "system" fn ResetEvent(h: usize) -> i32 { unsafe { NtClearEvent(h) } }
            pub extern "system" fn WSAResetEvent(e: usize) -> i32 { unsafe { ResetEvent(e) } }
            fn reuse(e: usize) {
                // lint(event-reset): a fresh event of our own, no request pending on it
                ResetEvent(e);
            }
        "#;
        assert_eq!(event_reset("user/kernel32/src/sync.rs", src).unwrap(), []);
        // A tag without a reason is no justification.
        let bare = "fn f(e: usize) {\n    // lint(event-reset):\n    ResetEvent(e);\n}\n";
        assert_eq!(event_reset("x.rs", bare).unwrap().len(), 1);
    }

    #[test]
    fn a_raw_wait_outside_the_reviewed_sites_is_flagged() {
        let src = r#"
            fn wait(s: Guard) { platform::prepare_wait(None); drop(s); platform::wait(); }
            fn new_wait() { platform::prepare_wait(None); platform::wait(); }
        "#;
        let f = wait_site("nt/pipe.rs", src).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].message.starts_with("new_wait calls prepare_wait"), "{f:?}");
        // Elsewhere, even a function named like a reviewed one is not.
        assert_eq!(wait_site("nt/afd.rs", src).unwrap().len(), 2);
    }

    #[test]
    fn the_tree_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let findings = analyze(root).unwrap();
        assert!(findings.is_empty(), "{findings:#?}");
    }
}

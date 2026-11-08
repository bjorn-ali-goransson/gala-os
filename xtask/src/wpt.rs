//! BR's conformance report: web-platform-tests in Chromium on gala
//! (`chromium_wpt`). xtask fetches WPT at WPT_COMMIT (WPT_DIRS only, a
//! sparse checkout) and runs WPT's own server, wptserve (`./wpt serve`,
//! on the host, with WPT's hosts in /etc/hosts), on PORTS; QEMU forwards
//! the guest's 10.0.2.100 on those ports to it (`xtask relay <port>` per
//! connection), Chromium maps web-platform.test and its subdomains to
//! 10.0.2.100 (as wptrunner does), and `client` drives one headless
//! Chromium session through ChromeDriver: each test page is loaded and
//! its testharness results read back through our testharnessreport.js
//! (`REPORTER`, written into the tree: the file WPT leaves to vendors). Each test's line goes into the
//! serial log (`xtask: wpt: <path>: ...`); the run's results into
//! target/wpt/wptreport.json (wptrunner's format, what wpt.fyi takes).

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::{Result, webdriver};

/// The WPT revision the report runs (a commit of
/// github.com/web-platform-tests/wpt).
pub const WPT_COMMIT: &str = "dd216d1d4f61489b09f566bec2c52d229baa7f49";
/// What is checked out: the harness and the directories whose tests run.
/// (`tools` and `docs` for `./wpt serve`, `common` for the tests).
const WPT_DIRS: &[&str] = &["resources", "common", "docs", "tools", "dom/nodes"];
/// wptserve's ports (http, http alternate, https, https alternate, ws,
/// wss), forwarded from the guest's 10.0.2.100.
pub const PORTS: &[u16] = &[8000, 8001, 8443, 8444, 8666, 8889];
/// The directories whose tests run (WPT_DIRS less the harness).
const TEST_DIRS: &[&str] = &["dom/nodes"];

/// The WPT tree at WPT_COMMIT (target/prebuilt/wpt), fetched on first use
/// with the host's git: a shallow, blob-filtered, sparse checkout.
pub fn tree() -> Result<PathBuf> {
    let dir = PathBuf::from("target/prebuilt/wpt");
    let stamp = dir.join(".gala-commit");
    let want = format!("{WPT_COMMIT} {}", WPT_DIRS.join(" "));
    if fs::read_to_string(&stamp).is_ok_and(|s| s == want) {
        fs::write(dir.join("resources/testharnessreport.js"), REPORTER)
            .map_err(|e| e.to_string())?;
        return fs::canonicalize(&dir).map_err(|e| e.to_string());
    }
    eprintln!("xtask: fetching web-platform-tests at {WPT_COMMIT} ({})", WPT_DIRS.join(", "));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let git = |args: &[&str]| -> Result {
        let out = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)))
        }
    };
    git(&["init", "-q"])?;
    git(&["remote", "add", "origin", "https://github.com/web-platform-tests/wpt.git"])?;
    git(&["sparse-checkout", "init", "--cone"])?;
    let mut set = vec!["sparse-checkout", "set"];
    set.extend_from_slice(WPT_DIRS);
    git(&set)?;
    git(&["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", WPT_COMMIT])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    fs::write(&stamp, want).map_err(|e| e.to_string())?;
    fs::write(dir.join("resources/testharnessreport.js"), REPORTER).map_err(|e| e.to_string())?;
    fs::canonicalize(&dir).map_err(|e| e.to_string())
}

/// Our testharnessreport.js (as wptrunner's: output off, timeouts
/// multiplied for TCG): the completion callback leaves the results in
/// `window.__gala_wpt` as lines: `HARNESS\t<status>\t<message>`, then
/// `<status>\t<name>\t<message>` per subtest (tabs and newlines in the
/// texts become spaces).
const REPORTER: &str = r#"setup({output: false, timeout_multiplier: 4});
add_completion_callback(function (tests, status) {
  var clean = function (s) {
    return String(s === undefined || s === null ? "" : s).replace(/[\t\n\r]/g, " ");
  };
  var lines = ["HARNESS\t" + status.status + "\t" + clean(status.message)];
  tests.forEach(function (t) {
    lines.push(t.status + "\t" + clean(t.name) + "\t" + clean(t.message));
  });
  window.__gala_wpt = lines.join("\n");
});
"#;

/// wptserve on the host: started for a run, stopped with it.
/// `./wpt serve` in a process group of its own: its servers are
/// multiprocessing children that outlive a killed parent (and kept the
/// ports, so a later run talked to a stale server), so the whole group
/// is ended: SIGTERM, then SIGKILL for what is left after 5 s.
pub struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let group = format!("-{}", self.0.id());
        let signal = |sig: &str| {
            let _ = Command::new("kill").args([sig, "--", &group]).status();
        };
        signal("-TERM");
        for _ in 0..50 {
            if !port_answers(8000) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        signal("-KILL");
        let _ = self.0.wait();
    }
}

fn port_answers(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Starts `./wpt serve` (HTTP/2 off) with PORTS fixed (gala-config.json
/// in the tree), its output in target/wpt/wptserve.log, and waits until
/// it answers on 8000 (the first start makes its virtualenv: minutes).
/// WPT's hosts must resolve to the loopback first
/// (`./wpt make-hosts-file >> /etc/hosts`): an error says so.
pub fn start_server() -> Result<Server> {
    let root = tree()?;
    let hosts = fs::read_to_string("/etc/hosts").unwrap_or_default();
    if !hosts.contains("web-platform.test") {
        return Err(format!(
            "WPT's hosts are not in /etc/hosts: run `cd {} && ./wpt make-hosts-file >> /etc/hosts`",
            root.display()
        ));
    }
    // A server already there would answer for ours (a stale one from an
    // earlier run, of another tree or configuration).
    if let Some(port) = PORTS.iter().copied().find(|&p| port_answers(p)) {
        return Err(format!(
            "something already listens on port {port} (a stale `wpt serve` from an earlier \
             run?): end it first"
        ));
    }
    let config =
        r#"{"ports": {"http": [8000, 8001], "https": [8443, 8444], "ws": [8666], "wss": [8889]}}"#;
    fs::write(root.join("gala-config.json"), config).map_err(|e| e.to_string())?;
    fs::create_dir_all("target/wpt").map_err(|e| e.to_string())?;
    let log = fs::File::create("target/wpt/wptserve.log").map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new("./wpt")
        .args(["serve", "--no-h2", "--config", "gala-config.json"])
        .current_dir(&root)
        .process_group(0)
        .stdout(log)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("./wpt serve: {e}"))?;
    let mut server = Server(child);
    for _ in 0..600 {
        if port_answers(8000) {
            relay_preflight()?;
            return Ok(server);
        }
        if let Ok(Some(status)) = server.0.try_wait() {
            return Err(format!("./wpt serve ended ({status}): see target/wpt/wptserve.log"));
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err("./wpt serve did not answer on port 8000 in 300 s (target/wpt/wptserve.log)".into())
}

/// Where each relayed connection is logged (one line: port, bytes each
/// way, or the error), absolute: QEMU runs the relay.
fn relay_log() -> Result<String> {
    let log = std::env::current_dir().map_err(|e| e.to_string())?.join("target/wpt/relay.log");
    let log = log.display().to_string();
    if log.contains(',') || log.contains(' ') {
        return Err(format!("the relay log's path {log:?} cannot go in a -netdev option"));
    }
    Ok(log)
}

/// The command QEMU runs for a guest connection to `port`.
fn relay_command() -> Result<(String, String)> {
    let exe = std::env::current_exe().map_err(|e| format!("xtask's own path: {e}"))?;
    let exe = exe.display().to_string();
    if exe.contains(',') || exe.contains(' ') {
        return Err(format!("xtask's path {exe:?} cannot go in a -netdev option"));
    }
    Ok((exe, relay_log()?))
}

/// The `-netdev user` options forwarding the guest's 10.0.2.100 on PORTS
/// to wptserve, through `xtask relay <port> <log>` (QEMU runs it per
/// connection).
pub fn guestfwd() -> Result<String> {
    let (exe, log) = relay_command()?;
    Ok(PORTS
        .iter()
        .map(|p| format!(",guestfwd=tcp:10.0.2.100:{p}-cmd:{exe} relay {p} {log}"))
        .collect())
}

/// Runs the relay as QEMU will (its command line, a request on standard
/// input) and checks that wptserve's answer comes back: a broken relay
/// fails the run here, in seconds, not as every test timing out.
fn relay_preflight() -> Result {
    use std::io::{Read, Write};
    let (exe, log) = relay_command()?;
    let _ = fs::remove_file(&log);
    let mut child = Command::new(&exe)
        .args(["relay", "8000", &log])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("relay preflight: {e}"))?;
    let request = "GET /resources/testharnessreport.js HTTP/1.1\r\nHost: web-platform.test:8000\r\nConnection: close\r\n\r\n";
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(request.as_bytes()).map_err(|e| format!("relay preflight: {e}"))?;
    }
    let mut answer = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut answer);
    }
    let _ = child.wait();
    let status = answer.lines().next().unwrap_or("").to_string();
    if !status.starts_with("HTTP/1.1 200") || !answer.contains("__gala_wpt") {
        let logged = fs::read_to_string(&log).unwrap_or_default();
        return Err(format!(
            "relay preflight: `{exe} relay 8000` did not bring back our testharnessreport.js \
             (status line {status:?}; relay log: {})",
            logged.trim()
        ));
    }
    Ok(())
}

/// `cargo xtask relay <port> [<log>]`: one guest connection (standard
/// input and output) to 127.0.0.1:<port>, both ways, until the server's
/// side ends; two lines for it in <log>, keyed by the relay's pid: `open`
/// when it starts (a connection without its second line never ended, or
/// the relay was killed), then its start and end (host epoch seconds, as
/// ChromeDriver's log stamps them), bytes each way, and how each side
/// ended, in order (or the connect error).
pub fn relay(port: &str, log: Option<&str>) -> Result {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    let start = now_ms();
    let id = std::process::id();
    let note = |line: String| {
        if let Some(path) = log
            && let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path)
        {
            // One write per line: relays of concurrent connections append
            // to the same file.
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    };
    let Ok(port) = port.parse::<u16>() else {
        note(format!("relay [{id}] {}: bad port {port:?}", stamp(start)));
        return Err(format!("relay: bad port {port:?}"));
    };
    note(format!("relay {port} [{id}] open {}", stamp(start)));
    let stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => {
            note(format!("relay {port} [{id}] {}: connect: {e}", span(start, now_ms())));
            return Err(format!("relay {port}: {e}"));
        }
    };
    let connected = now_ms();
    let mut up = stream.try_clone().map_err(|e| e.to_string())?;
    let sent = std::sync::Arc::new(AtomicU64::new(0));
    // When the guest's side ended (epoch ms, 0 while it has not), and how.
    let guest_end = std::sync::Arc::new(AtomicU64::new(0));
    let guest_how = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let (counted, ended, how) = (sent.clone(), guest_end.clone(), guest_how.clone());
    let to_server = thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 16384];
        let why = loop {
            match stdin.read(&mut buf) {
                Ok(0) => break "the guest closed".to_string(),
                Ok(n) => {
                    if let Err(e) = up.write_all(&buf[..n]) {
                        break format!("writing to the server: {e}");
                    }
                    counted.fetch_add(n as u64, Ordering::Relaxed);
                }
                Err(e) => break format!("reading from the guest: {e}"),
            }
        };
        if let Ok(mut h) = how.lock() {
            *h = why;
        }
        ended.store(now_ms(), Ordering::Release);
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut down = stream;
    let mut out = std::io::stdout().lock();
    let mut buf = [0u8; 16384];
    let mut received = 0u64;
    let server_how = loop {
        match down.read(&mut buf) {
            Ok(0) => break "the server closed".to_string(),
            Ok(n) => {
                if let Err(e) = out.write_all(&buf[..n]).and_then(|()| out.flush()) {
                    break format!("writing to the guest: {e}");
                }
                received += n as u64;
            }
            Err(e) => break format!("reading from the server: {e}"),
        }
    };
    let end = now_ms();
    // The guest's side may stay open after the answer: the count so far.
    drop(to_server);
    let sent = sent.load(Ordering::Relaxed);
    let guest_at = guest_end.load(Ordering::Acquire);
    let after = |t: u64| format!("+{}.{:03} s", (t - start) / 1000, (t - start) % 1000);
    let ends = if guest_at == 0 {
        format!("{server_how} {}, the guest's side still open", after(end))
    } else {
        let guest_how = guest_how.lock().map(|h| h.clone()).unwrap_or_default();
        format!("{guest_how} {}, then {server_how} {}", after(guest_at), after(end))
    };
    note(format!(
        "relay {port} [{id}] {}: connected {}, {sent} bytes to the server, {received} back; {ends}",
        span(start, end),
        after(connected),
    ));
    Ok(())
}

/// Epoch milliseconds, for the relay's log.
fn now_ms() -> u64 {
    u64::try_from(epoch_ms()).unwrap_or(u64::MAX)
}

/// Epoch milliseconds as ChromeDriver stamps its log (`1762512896.789`)
/// and as the UTC time of day.
fn stamp(ms: u64) -> String {
    let day = ms % 86_400_000;
    format!(
        "{}.{:03} ({:02}:{:02}:{:02}.{:03} UTC)",
        ms / 1000,
        ms % 1000,
        day / 3_600_000,
        day / 60_000 % 60,
        day / 1000 % 60,
        day % 1000
    )
}

/// A connection's start and end, and how long it lasted.
fn span(start: u64, end: u64) -> String {
    let d = end.saturating_sub(start);
    format!("{} to {} ({}.{:03} s)", stamp(start), stamp(end), d / 1000, d % 1000)
}

/// The testharness tests under TEST_DIRS, as URL paths: pages that load
/// testharness.js, and `.window.js` / `.any.js` tests as their window
/// pages; not support files, crash tests, manual tests or reftests.
/// XTASK_WPT_FILTER keeps the paths containing it, XTASK_WPT_LIMIT the
/// first so many.
fn tests(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "support" | "resources" | "crashtests" | "tools") {
                    walk(root, &path, out);
                }
                continue;
            }
            if name.contains("-manual") || name.contains("-crash") || name.contains("-ref.") {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if let Some(stem) = rel.strip_suffix(".window.js") {
                out.push(format!("/{stem}.window.html"));
            } else if let Some(stem) = rel.strip_suffix(".any.js") {
                out.push(format!("/{stem}.any.html"));
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("html" | "htm" | "xhtml" | "svg")
            ) && fs::read_to_string(&path)
                .is_ok_and(|s| s.contains("/resources/testharness.js"))
            {
                out.push(format!("/{rel}"));
            }
        }
    }
    let mut out = Vec::new();
    for dir in TEST_DIRS {
        walk(root, &root.join(dir), &mut out);
    }
    out.sort();
    if let Ok(filter) = std::env::var("XTASK_WPT_FILTER") {
        out.retain(|p| p.contains(&filter));
    }
    if let Some(limit) = std::env::var("XTASK_WPT_LIMIT").ok().and_then(|l| l.parse().ok()) {
        out.truncate(limit);
    }
    out
}

/// A JSON string's value (the text between its quotes, unescaped) at
/// the start of `s`, and what follows it.
fn json_text(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 2..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|c| c.1)).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The `"value"` of a WebDriver answer when it is a string.
fn value_text(body: &str) -> Option<String> {
    let at = body.find("\"value\"")?;
    let rest = body[at + 7..].trim_start().strip_prefix(':')?.trim_start();
    json_text(rest).map(|(v, _)| v)
}

/// testharness's names for a harness status and a subtest status.
fn harness_status(code: &str) -> &'static str {
    match code {
        "0" => "OK",
        "1" => "ERROR",
        "2" => "TIMEOUT",
        "3" => "PRECONDITION_FAILED",
        _ => "UNKNOWN",
    }
}

fn subtest_status(code: &str) -> &'static str {
    match code {
        "0" => "PASS",
        "1" => "FAIL",
        "2" => "TIMEOUT",
        "3" => "NOTRUN",
        "4" => "PRECONDITION_FAILED",
        _ => "UNKNOWN",
    }
}

const CAPABILITIES: &str = r#"{"capabilities":{"alwaysMatch":{"timeouts":{"pageLoad":300000,"script":60000},"goog:chromeOptions":{"browserStartupTimeout":180000,"binary":"C:\\chromium\\chrome.exe","args":["--headless","--no-sandbox","--disable-gpu","--no-first-run","--user-data-dir=C:\\tmp\\chrw-{n}","--enable-logging","--log-file=C:\\chromium\\chrome_wpt-{n}.log","--ipc-connection-timeout=120","--host-resolver-rules=MAP web-platform.test 10.0.2.100,MAP *.web-platform.test 10.0.2.100,MAP not-web-platform.test 10.0.2.100,MAP *.not-web-platform.test 10.0.2.100","--ignore-certificate-errors"]}}}}"#;

/// A new session's id.
/// A new session, its Chrome on a profile directory of its own (as
/// wptrunner starts each browser): a previous Chrome still exiting holds
/// its own, so the new one does not hand over to it and exit.
fn session(port: u16) -> Result<String> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let capabilities = CAPABILITIES.replace("{n}", &n.to_string());
    let (status, body) = webdriver(port, "POST", "/session", &capabilities, 600)?;
    match crate::json_string(&body, "sessionId") {
        Some(id) if status == 200 => Ok(id.to_string()),
        _ => Err(format!("POST /session: {status} {body}")),
    }
}

/// One test's result as wptreport.json has it (wptrunner's format, what
/// wpt.fyi takes): the harness status and message, each subtest's.
pub struct TestResult {
    pub test: String,
    pub status: &'static str,
    pub message: String,
    pub subtests: Vec<(String, &'static str, String)>,
    pub duration_ms: u128,
}

/// One test's outcome: its line for the log and its result.
struct Outcome {
    line: String,
    result: TestResult,
    /// The session ended (the browser went away): start another.
    lost: bool,
}

/// A test's source: its page, or the `.window.js`/`.any.js` it wraps.
fn source(root: &Path, test: &str) -> String {
    let rel = test.trim_start_matches('/');
    let rel = rel
        .strip_suffix(".window.html")
        .map(|s| format!("{s}.window.js"))
        .or_else(|| rel.strip_suffix(".any.html").map(|s| format!("{s}.any.js")))
        .unwrap_or_else(|| rel.to_string());
    fs::read_to_string(root.join(rel)).unwrap_or_default()
}

/// How long to wait for a test's results: testharness's own timeout (10
/// s, 60 s for `<meta name="timeout" content="long">`) times REPORTER's
/// multiplier (4), and 20 s for the harness to report it.
fn wait_for(source: &str) -> Duration {
    let long = source.contains("name=\"timeout\" content=\"long\"")
        || source.contains("META: timeout=long");
    Duration::from_secs(if long { 60 } else { 10 } * 4 + 20)
}

fn run_one(port: u16, id: &str, test: &str, wait: Duration) -> Outcome {
    let started = std::time::Instant::now();
    let result = |status: &'static str, message: String| TestResult {
        test: test.to_string(),
        status,
        message,
        subtests: Vec::new(),
        duration_ms: started.elapsed().as_millis(),
    };
    let lost =
        |answer: &str| answer.contains("invalid session id") || answer.contains("disconnected");
    let url = format!("{{\"url\":\"http://web-platform.test:8000{test}\"}}");
    match webdriver(port, "POST", &format!("/session/{id}/url"), &url, 330) {
        Ok((200, _)) => {}
        Ok((status, answer)) => {
            let message = answer.chars().take(200).collect::<String>();
            return Outcome {
                line: format!("{test}: load failed: {status} {message}"),
                result: result("ERROR", format!("load failed: {status} {message}")),
                lost: lost(&answer),
            };
        }
        Err(e) => {
            return Outcome {
                line: format!("{test}: load failed: {e}"),
                result: result("ERROR", format!("load failed: {e}")),
                lost: true,
            };
        }
    }
    let script = r#"{"script":"return window.__gala_wpt || null","args":[]}"#;
    let results = loop {
        match webdriver(port, "POST", &format!("/session/{id}/execute/sync"), script, 120) {
            Ok((200, answer)) => {
                if let Some(text) = value_text(&answer) {
                    break Some(text);
                }
            }
            Ok((_, answer)) if lost(&answer) => {
                return Outcome {
                    line: format!("{test}: session lost"),
                    result: result("CRASH", "the browser went away".into()),
                    lost: true,
                };
            }
            _ => {}
        }
        if started.elapsed() > wait {
            break None;
        }
        thread::sleep(Duration::from_secs(1));
    };
    let Some(text) = results else {
        let state = page_state(port, id);
        return Outcome {
            line: format!("{test}: no results in {} s; the page: {state}", wait.as_secs()),
            result: result(
                "TIMEOUT",
                format!("no results in {} s; the page: {state}", wait.as_secs()),
            ),
            lost: false,
        };
    };
    let mut lines = text.lines();
    let harness: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
    let status = harness_status(harness.get(1).copied().unwrap_or(""));
    let mut r = result(status, harness.get(2).copied().unwrap_or("").to_string());
    for line in lines {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        r.subtests.push((
            parts.get(1).copied().unwrap_or("").to_string(),
            subtest_status(parts.first().copied().unwrap_or("")),
            parts.get(2).copied().unwrap_or("").to_string(),
        ));
    }
    let pass = r.subtests.iter().filter(|s| s.1 == "PASS").count();
    Outcome {
        line: format!("{test}: {status} {pass}/{}", r.subtests.len()),
        result: r,
        lost: false,
    }
}

/// What a page that gave no results is doing, for its TIMEOUT message:
/// its URL (an error page's is chrome-error://), readyState, title,
/// whether testharness.js ran (add_completion_callback defined) and the
/// resources it fetched (name and response status, where the browser
/// gives one).
fn page_state(port: u16, id: &str) -> String {
    let script = r#"{"script":"var r = performance.getEntriesByType('resource').map(function (e) { return e.name.replace(location.origin, '') + ' ' + (e.responseStatus || '?'); }); return [location.href, document.readyState, JSON.stringify(document.title), typeof add_completion_callback === 'function' ? 'testharness ran' : 'testharness did not run', 'resources: ' + (r.join(', ') || 'none')].join('; ')","args":[]}"#;
    match webdriver(port, "POST", &format!("/session/{id}/execute/sync"), script, 60) {
        Ok((200, answer)) => value_text(&answer).unwrap_or_else(|| "(no answer)".into()),
        Ok((status, answer)) => {
            format!("(asked: {status} {})", answer.chars().take(120).collect::<String>())
        }
        Err(e) => format!("(asked: {e})"),
    }
}

/// A JSON string literal (quoted, escaped).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A message as wptreport has it: null when empty.
fn json_message(s: &str) -> String {
    if s.is_empty() { "null".into() } else { json_str(s) }
}

/// The run as wptreport.json: `run_info` (the browser, gala as the OS,
/// the WPT revision), the start and end (ms since the epoch), each
/// test's result.
pub fn wptreport(results: &[TestResult], time_start: u128, time_end: u128) -> String {
    let mut out = format!(
        "{{\"run_info\":{{\"product\":\"chrome\",\"browser_version\":\"159.0.8140.0\",\
         \"os\":\"gala\",\"os_version\":\"10.0.26100\",\"processor\":\"x86_64\",\
         \"headless\":true,\"revision\":\"{WPT_COMMIT}\"}},\
         \"time_start\":{time_start},\"time_end\":{time_end},\"results\":["
    );
    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out += &format!(
            "{{\"test\":{},\"status\":\"{}\",\"message\":{},\"duration\":{},\"subtests\":[",
            json_str(&r.test),
            r.status,
            json_message(&r.message),
            r.duration_ms
        );
        for (j, (name, status, message)) in r.subtests.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out += &format!(
                "{{\"name\":{},\"status\":\"{status}\",\"message\":{}}}",
                json_str(name),
                json_message(message)
            );
        }
        out += "]}";
    }
    out += "]}\n";
    out
}

fn epoch_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// The host side of `chromium_wpt`: every test in one session (a new one
/// when the browser goes away), each test's line appended to `log` (the
/// run's serve log, which goes into the serial log), the results to
/// target/wpt/wptreport.json, a summary line last; then ChromeDriver's
/// /shutdown ends the run. Failing tests are the report, not a failure:
/// Err only when no session can be made.
pub fn client(port: u16, log: &Path, monitor: &Path) -> Result {
    let verdict = start_server().and_then(|server| {
        let v = run(port, log, monitor);
        drop(server);
        v
    });
    let _ = webdriver(port, "GET", "/shutdown", "", 60);
    verdict
}

fn run(port: u16, log: &Path, monitor: &Path) -> Result {
    use std::io::Write;
    let note = |line: &str| {
        if let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(log) {
            let _ = writeln!(f, "xtask: wpt: {line}");
        }
    };
    let root = tree()?;
    let list = tests(&root);
    note(&format!("{} tests at {WPT_COMMIT} ({})", list.len(), TEST_DIRS.join(", ")));
    for _ in 0..60 {
        if matches!(webdriver(port, "GET", "/status", "", 60), Ok((200, ref b)) if b.contains("\"ready\":true"))
        {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    let time_start = epoch_ms();
    let mut id = session(port)?;
    fs::create_dir_all("target/wpt").map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let mut stopped: Option<String> = None;
    for test in &list {
        if let Some(why) = &stopped {
            // Not run: in the report as such, so that it is complete.
            results.push(TestResult {
                test: test.clone(),
                status: "SKIP",
                message: format!("not run: {why}"),
                subtests: Vec::new(),
                duration_ms: 0,
            });
            continue;
        }
        let text = source(&root, test);
        let outcome = run_one(port, &id, test, wait_for(&text));
        note(&outcome.line);
        results.push(outcome.result);
        if outcome.lost {
            // What the guest is doing while the load hangs: its state dump
            // (each thread's system call and wait, in the serial log),
            // before anything is torn down.
            note(&format!("{test}: {}", state_dump(monitor)));
            // End the old session first: ChromeDriver then ends its
            // Chrome, which holds the profile directory a new Chrome
            // needs (otherwise the new one exits at once: "DevToolsActivePort
            // file doesn't exist").
            let ended = webdriver(port, "DELETE", &format!("/session/{id}"), "", 120);
            note(&format!("the session after {test} ended: {}", brief(&ended)));
            match session(port) {
                Ok(new) => id = new,
                Err(e) => {
                    note(&format!("no new session after {test}: {e}; the rest is not run"));
                    stopped = Some(format!("no session after {test}"));
                }
            }
        }
    }
    if stopped.is_none() {
        let _ = webdriver(port, "DELETE", &format!("/session/{id}"), "", 120);
    }
    fs::write("target/wpt/wptreport.json", wptreport(&results, time_start, epoch_ms()))
        .map_err(|e| e.to_string())?;
    let ok = results.iter().filter(|r| r.status == "OK").count();
    let total: usize = results.iter().map(|r| r.subtests.len()).sum();
    let pass: usize =
        results.iter().map(|r| r.subtests.iter().filter(|s| s.1 == "PASS").count()).sum();
    let skipped = results.iter().filter(|r| r.status == "SKIP").count();
    note(&format!(
        "summary: {} tests, {ok} harness OK, {pass} of {total} subtests pass, {skipped} not run (target/wpt/wptreport.json)",
        list.len()
    ));
    Ok(())
}

/// Asks the guest for a state dump (QEMU's `nmi` through the monitor;
/// read-only, the guest goes on) and gives it 60 s to print.
fn state_dump(monitor: &Path) -> String {
    use std::io::Write;
    match std::os::unix::net::UnixStream::connect(monitor).and_then(|mut m| {
        m.write_all(b"nmi\n")?;
        m.flush()?;
        Ok(m)
    }) {
        Ok(m) => {
            thread::sleep(Duration::from_secs(60));
            drop(m);
            "a state dump asked for (statedump: lines in the serial log)".into()
        }
        Err(e) => format!("no state dump (monitor: {e})"),
    }
}

/// A WebDriver answer in a few words, for the run's log.
fn brief(answer: &Result<(u32, String)>) -> String {
    match answer {
        Ok((status, body)) => format!("{status} {}", body.chars().take(120).collect::<String>()),
        Err(e) => e.chars().take(160).collect(),
    }
}

// ---- The comparison with wpt.fyi ----

/// wpt.fyi's runs the report is compared with (the Chromium of our
/// snapshot's line: Edge on Windows, the same Blink on the same OS, and
/// Chrome on Linux), pinned so a comparison can be repeated.
const REFERENCE_RUNS: &[(&str, &str)] =
    &[("edge-159-win", "5094276917362688"), ("chrome-159-linux", "6250565395546112")];

/// A JSON value (enough of JSON for wptreport.json and wpt.fyi's API).
#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }
    pub fn num(&self) -> Option<f64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }
    pub fn arr(&self) -> &[Json] {
        if let Json::Arr(a) = self { a } else { &[] }
    }
}

/// Parses a JSON document.
pub fn parse_json(text: &str) -> Option<Json> {
    let (v, rest) = value(text.trim_start())?;
    rest.trim().is_empty().then_some(v)
}

fn value(s: &str) -> Option<(Json, &str)> {
    let s = s.trim_start();
    match s.chars().next()? {
        '"' => json_text(s).map(|(t, r)| (Json::Str(t), r)),
        '{' => {
            let mut fields = Vec::new();
            let mut s = s[1..].trim_start();
            if let Some(r) = s.strip_prefix('}') {
                return Some((Json::Obj(fields), r));
            }
            loop {
                let (k, r) = json_text(s.trim_start())?;
                let r = r.trim_start().strip_prefix(':')?;
                let (v, r) = value(r)?;
                fields.push((k, v));
                let r = r.trim_start();
                if let Some(r) = r.strip_prefix(',') {
                    s = r;
                } else {
                    return Some((Json::Obj(fields), r.strip_prefix('}')?));
                }
            }
        }
        '[' => {
            let mut items = Vec::new();
            let mut s = s[1..].trim_start();
            if let Some(r) = s.strip_prefix(']') {
                return Some((Json::Arr(items), r));
            }
            loop {
                let (v, r) = value(s)?;
                items.push(v);
                let r = r.trim_start();
                if let Some(r) = r.strip_prefix(',') {
                    s = r;
                } else {
                    return Some((Json::Arr(items), r.strip_prefix(']')?));
                }
            }
        }
        't' => s.strip_prefix("true").map(|r| (Json::Bool(true), r)),
        'f' => s.strip_prefix("false").map(|r| (Json::Bool(false), r)),
        'n' => s.strip_prefix("null").map(|r| (Json::Null, r)),
        _ => {
            let end =
                s.find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c))).unwrap_or(s.len());
            s[..end].parse().ok().map(|n| (Json::Num(n), &s[end..]))
        }
    }
}

/// One test on both sides: gala's subtests passed and run (its harness
/// status), a reference run's passes and total.
struct Row {
    test: String,
    gala: (usize, usize, String),
    reference: Vec<Option<(u64, u64)>>,
    /// gala's first failing subtest and its message.
    first_failure: String,
}

/// `cargo xtask wpt-compare`: target/wpt/wptreport.json against
/// REFERENCE_RUNS on wpt.fyi (its search API, by test: passes and total,
/// fetched with the host's curl): per directory, and every test that
/// passes fewer subtests on gala than on the first reference (gala-only
/// failures, with gala's first failing subtest). Printed and written to
/// target/wpt/compare.txt.
pub fn compare() -> Result {
    let report = fs::read_to_string("target/wpt/wptreport.json")
        .map_err(|e| format!("target/wpt/wptreport.json: {e} (run chromium_wpt first)"))?;
    let report = parse_json(&report).ok_or("target/wpt/wptreport.json: not JSON")?;
    let mut rows = Vec::new();
    let mut dirs = std::collections::BTreeSet::new();
    for r in report.get("results").map(Json::arr).unwrap_or(&[]) {
        let test = r.get("test").and_then(Json::str).unwrap_or("").to_string();
        let status = r.get("status").and_then(Json::str).unwrap_or("").to_string();
        let subtests = r.get("subtests").map(Json::arr).unwrap_or(&[]);
        let pass =
            subtests.iter().filter(|s| s.get("status").and_then(Json::str) == Some("PASS")).count();
        let first_failure = subtests
            .iter()
            .find(|s| s.get("status").and_then(Json::str) != Some("PASS"))
            .map(|s| {
                format!(
                    "{}: {} {}",
                    s.get("name").and_then(Json::str).unwrap_or(""),
                    s.get("status").and_then(Json::str).unwrap_or(""),
                    s.get("message").and_then(Json::str).unwrap_or("")
                )
            })
            .unwrap_or_default();
        if let Some(dir) = test.rsplit_once('/').map(|(d, _)| d.to_string()) {
            dirs.insert(dir);
        }
        rows.push(Row {
            test,
            gala: (pass, subtests.len(), status),
            reference: vec![None; REFERENCE_RUNS.len()],
            first_failure,
        });
    }
    let ids: Vec<&str> = REFERENCE_RUNS.iter().map(|r| r.1).collect();
    for dir in &dirs {
        let url = format!("https://wpt.fyi/api/search?run_ids={}&q={dir}/", ids.join(","));
        let out = Command::new("curl")
            .args(["-sSf", "--max-time", "120", &url])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            return Err(format!("{url}: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let answer = parse_json(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| format!("{url}: not JSON"))?;
        for r in answer.get("results").map(Json::arr).unwrap_or(&[]) {
            let test = r.get("test").and_then(Json::str).unwrap_or("");
            let Some(row) = rows.iter_mut().find(|row| row.test == test) else { continue };
            for (i, s) in r.get("legacy_status").map(Json::arr).unwrap_or(&[]).iter().enumerate() {
                if let (Some(p), Some(t)) =
                    (s.get("passes").and_then(Json::num), s.get("total").and_then(Json::num))
                    && i < row.reference.len()
                {
                    row.reference[i] = Some((p as u64, t as u64));
                }
            }
        }
    }
    let mut text = format!(
        "wpt-compare: gala (chromium_wpt, WPT {WPT_COMMIT}) against {}\n",
        REFERENCE_RUNS
            .iter()
            .map(|r| format!("{} (run {})", r.0, r.1))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let sum = |f: &dyn Fn(&Row) -> Option<(u64, u64)>| {
        rows.iter().filter_map(f).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    };
    let gala = sum(&|r| Some((r.gala.0 as u64, r.gala.1 as u64)));
    text += &format!("tests: {}; subtests passed: gala {} of {}", rows.len(), gala.0, gala.1);
    for (i, (name, _)) in REFERENCE_RUNS.iter().enumerate() {
        let r = sum(&|row| row.reference[i]);
        text += &format!(", {name} {} of {}", r.0, r.1);
    }
    text += "\n";
    let mut worse: Vec<&Row> = rows
        .iter()
        .filter(|r| r.reference[0].is_some_and(|(p, _)| (r.gala.0 as u64) < p))
        .collect();
    worse.sort_by(|a, b| a.test.cmp(&b.test));
    text += &format!(
        "gala-only failures (fewer passes than {}): {}\n",
        REFERENCE_RUNS[0].0,
        worse.len()
    );
    for r in &worse {
        let reference = r.reference[0].map_or("?".to_string(), |(p, t)| format!("{p}/{t}"));
        text += &format!(
            "  {}: gala {}/{} ({}), {} {reference}; first failure: {}\n",
            r.test,
            r.gala.0,
            r.gala.1,
            r.gala.2,
            REFERENCE_RUNS[0].0,
            r.first_failure.chars().take(200).collect::<String>()
        );
    }
    let unknown = rows.iter().filter(|r| r.reference[0].is_none()).count();
    if unknown > 0 {
        text += &format!("not in {}: {unknown} tests\n", REFERENCE_RUNS[0].0);
    }
    print!("{text}");
    fs::write("target/wpt/compare.txt", &text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_stamps_match_chromedriver() {
        // 2025-11-07 01:02:03.045 UTC.
        assert_eq!(stamp(1_762_477_323_045), "1762477323.045 (01:02:03.045 UTC)");
        assert_eq!(
            span(1_762_477_323_045, 1_762_477_544_100),
            "1762477323.045 (01:02:03.045 UTC) to 1762477544.100 (01:05:44.100 UTC) (221.055 s)"
        );
    }

    #[test]
    fn json_text_unescapes() {
        let (v, rest) = json_text(r#""a\tb\nc\"d\\eé" ,"x""#).unwrap();
        assert_eq!(v, "a\tb\nc\"d\\eé");
        assert_eq!(rest, r#" ,"x""#);
        assert_eq!(value_text(r#"{"value":"HARNESS\t0\t"}"#).as_deref(), Some("HARNESS\t0\t"));
        assert_eq!(value_text(r#"{"value":null}"#), None);
    }

    #[test]
    fn parses_json() {
        let v = parse_json(
            r#" {"a": [1, 2.5, -3e2], "b": {"c": "d\"e"}, "t": true, "n": null, "e": []} "#,
        )
        .unwrap();
        assert_eq!(
            v.get("a").unwrap().arr().iter().filter_map(Json::num).collect::<Vec<_>>(),
            [1.0, 2.5, -300.0]
        );
        assert_eq!(v.get("b").and_then(|b| b.get("c")).and_then(Json::str), Some("d\"e"));
        assert_eq!(v.get("t"), Some(&Json::Bool(true)));
        assert_eq!(v.get("n"), Some(&Json::Null));
        assert!(v.get("e").unwrap().arr().is_empty());
        assert!(parse_json("{\"a\": }").is_none());
        // Our own report reads back.
        let results = [TestResult {
            test: "/x.html".into(),
            status: "OK",
            message: String::new(),
            subtests: vec![("s".into(), "PASS", String::new())],
            duration_ms: 1,
        }];
        let back = parse_json(&wptreport(&results, 1, 2)).unwrap();
        let r = &back.get("results").unwrap().arr()[0];
        assert_eq!(r.get("test").and_then(Json::str), Some("/x.html"));
    }

    #[test]
    fn writes_wptreport_json() {
        let results = [TestResult {
            test: "/dom/nodes/a.html".into(),
            status: "OK",
            message: String::new(),
            subtests: vec![
                ("first \"one\"".into(), "PASS", String::new()),
                ("second".into(), "FAIL", "assert_equals: got 1\nwant 2".into()),
            ],
            duration_ms: 1234,
        }];
        let json = wptreport(&results, 10, 20);
        assert!(json.contains(r#""time_start":10,"time_end":20"#));
        assert!(json.contains(r#""revision":"dd216d1d4f61489b09f566bec2c52d229baa7f49""#));
        assert!(json.contains(
            r#"{"test":"/dom/nodes/a.html","status":"OK","message":null,"duration":1234,"subtests":[{"name":"first \"one\"","status":"PASS","message":null},{"name":"second","status":"FAIL","message":"assert_equals: got 1\nwant 2"}]}"#
        ));
    }
}

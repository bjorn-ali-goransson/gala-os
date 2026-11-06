//! The event stream: one JSON object per line on the debug console
//! (`arch::debugcon`), for tools rather than people (docs/OBSERVABILITY.md
//! section 1). Off by default; `events=syscall,image,thread,process,fault,
//! mock,net,logon,console,display,ipc` (or `events=all`) on the kernel command
//! line turns categories on (`mock`: live mocking's changes, such as clock
//! control). A `record`
//! boot's recording also goes here (`"ev":"rec"`, whatever the mask). Each
//! line starts with `"t"` (ns since boot), `"cpu"`, `"by"` (the client id
//! of the thread whose work emitted it: an NT thread id, a Linux tid, 0
//! for a kernel thread) and `"ev"`; the rest depends on the event (`pid`
//! and `tid` in it name its subject, which may be another thread).

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Syscall = 1,
    Image = 2,
    Thread = 4,
    Process = 8,
    Fault = 16,
    /// Live mocking's changes (clock control, record/replay).
    Mock = 32,
    /// Connections: TCP accepted, connected, closed (with the peer).
    Net = 64,
    /// Logons: LogonUser and LSA logons (user, type, result).
    Logon = 128,
    /// Console sessions and pseudo consoles: created, resized, closed.
    Console = 256,
    /// The display: modes set, resizes, device resets, failed commands;
    /// with `gputrace` every virtio-gpu command too.
    Display = 512,
    /// The clock's calibration and its check against the RTC (a wrong
    /// clock is written whatever the mask: `emit_always`).
    Clock = 1024,
    /// Windows (RS step B2): registered and gone (with their session),
    /// an HWND asked for from another session, and each key the desktop
    /// delivers (`key`: its window and owner).
    Window = 2048,
    /// File opens: the name, the handle or the failing status.
    File = 4096,
    /// Interprocess I/O: pipes (create, connect, write and read with byte
    /// offsets, disconnect, close) and I/O completion ports (post and
    /// dequeue, with who queued the packet and when), so data can be
    /// followed from a write to its read and to the thread that took the
    /// completion.
    Ipc = 8192,
    /// The sampling profiler: every `sample_hz=` (default 100) the timer
    /// tick of each busy CPU writes the thread it interrupted (`by`), the
    /// mode and the instruction pointer; `cargo xtask profile` attributes
    /// them to threads, modules and symbols.
    Sample = 16384,
}

const NAMES: [(&str, u32); 15] = [
    ("sample", Category::Sample as u32),
    ("ipc", Category::Ipc as u32),
    ("file", Category::File as u32),
    ("window", Category::Window as u32),
    ("display", Category::Display as u32),
    ("net", Category::Net as u32),
    ("logon", Category::Logon as u32),
    ("console", Category::Console as u32),
    ("syscall", Category::Syscall as u32),
    ("image", Category::Image as u32),
    ("thread", Category::Thread as u32),
    ("process", Category::Process as u32),
    ("fault", Category::Fault as u32),
    ("mock", Category::Mock as u32),
    ("clock", Category::Clock as u32),
];

static MASK: AtomicU32 = AtomicU32::new(0);

/// `events_pid=a,b,..`: the processes whose system calls are written (at
/// most `PIDS`; none listed: every process). The `syscall` category is the
/// stream's bulk; one process of a large program is often all that is
/// needed.
/// `events_match=<text>` adds each process whose command line contains
/// the text when it starts (`note_process`): a program's processes chosen
/// by what they are (Chromium's network service: its `--utility-sub-type`)
/// rather than by pids not known before the run.
const PIDS: usize = 64;
static SYSCALL_PIDS: [AtomicU64; PIDS] = [const { AtomicU64::new(0) }; PIDS];

/// The `events_match=` text (at most 63 bytes; empty: none).
static MATCH: crate::sync::IrqMutex<([u8; 63], usize)> = crate::sync::IrqMutex::new(([0; 63], 0));
static MATCHING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Sets the `events_pid=` list (unparsable entries are ignored).
pub fn set_syscall_pids(value: &str) {
    for pid in value.split(',').filter_map(|p| p.parse::<u64>().ok()) {
        add_syscall_pid(pid);
    }
}

fn add_syscall_pid(pid: u64) {
    if let Some(slot) = SYSCALL_PIDS.iter().find(|s| s.load(Ordering::Relaxed) == 0) {
        slot.store(pid, Ordering::Relaxed);
    }
}

/// Sets the `events_match=` text.
pub fn set_syscall_match(text: &str) {
    let mut m = MATCH.lock();
    let n = text.len().min(m.0.len());
    m.0[..n].copy_from_slice(&text.as_bytes()[..n]);
    m.1 = n;
    MATCHING.store(n > 0, Ordering::Relaxed);
}

/// A process started with `command_line`: its system calls are wanted
/// from now on if it matches `events_match=`.
pub fn note_process(pid: u64, command_line: &str) {
    if !MATCHING.load(Ordering::Relaxed) {
        return;
    }
    let matches = {
        let m = MATCH.lock();
        let needle = &m.0[..m.1];
        command_line.as_bytes().windows(needle.len()).any(|w| w == needle)
    };
    if matches {
        add_syscall_pid(pid);
    }
}

/// Whether process `pid`'s system calls are wanted (`events_pid=`,
/// `events_match=`; neither given: every process's).
pub fn syscall_pid_wanted(pid: u64) -> bool {
    let first = SYSCALL_PIDS[0].load(Ordering::Relaxed);
    if first == 0 {
        return !MATCHING.load(Ordering::Relaxed);
    }
    first == pid || SYSCALL_PIDS[1..].iter().any(|p| p.load(Ordering::Relaxed) == pid)
}
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// The category mask for an `events=` value (unknown names are ignored).
pub fn parse_mask(value: &str) -> u32 {
    value
        .split(',')
        .map(|name| match name {
            "all" => u32::MAX,
            _ => NAMES.iter().find(|(n, _)| *n == name).map_or(0, |&(_, bit)| bit),
        })
        .fold(0, |m, bit| m | bit)
}

pub fn set_mask(mask: u32) {
    MASK.store(mask, Ordering::Relaxed);
}

/// Whether events of `category` are wanted (one relaxed load).
pub fn enabled(category: Category) -> bool {
    MASK.load(Ordering::Relaxed) & category as u32 != 0
}

/// Lines written so far.
pub fn written() -> u64 {
    WRITTEN.load(Ordering::Relaxed)
}

/// The client id of the running thread (0: a kernel thread; and in host
/// tests and gala-um, which write no stream).
#[cfg(not(any(test, feature = "hosted")))]
pub fn current_client() -> u64 {
    crate::arch::percpu::client()
}

#[cfg(any(test, feature = "hosted"))]
pub fn current_client() -> u64 {
    0
}

/// Writes one event: `{"t":..,"cpu":..,"by":..,"ev":"<name>",<fields>}`. `fields`
/// are the remaining JSON members (without braces); strings in them must
/// be wrapped in [`Str`].
#[cfg(not(any(test, feature = "hosted")))]
pub fn emit(category: Category, name: &str, fields: fmt::Arguments) {
    if !enabled(category) {
        return;
    }
    let t = crate::time::raw_monotonic_ns();
    let (cpu, by) = (crate::arch::percpu::cpu_index(), crate::arch::percpu::client());
    let mut port = crate::arch::debugcon::DEBUGCON.lock();
    let _ = writeln!(port, r#"{{"t":{t},"cpu":{cpu},"by":{by},"ev":"{name}",{fields}}}"#);
    WRITTEN.fetch_add(1, Ordering::Relaxed);
}

/// Host unit tests and gala-um have no debug console.
#[cfg(any(test, feature = "hosted"))]
pub fn emit(_category: Category, _name: &str, _fields: fmt::Arguments) {}

/// As [`emit`] for an event that happened earlier, at `t`, on behalf of
/// client `by` (a profiler sample taken in an NMI, written at the next
/// tick).
#[cfg(not(any(test, feature = "hosted")))]
pub fn emit_at(category: Category, name: &str, t: u64, by: u64, fields: fmt::Arguments) {
    if !enabled(category) {
        return;
    }
    let cpu = crate::arch::percpu::cpu_index();
    let mut port = crate::arch::debugcon::DEBUGCON.lock();
    let _ = writeln!(port, r#"{{"t":{t},"cpu":{cpu},"by":{by},"ev":"{name}",{fields}}}"#);
    WRITTEN.fetch_add(1, Ordering::Relaxed);
}

#[cfg(any(test, feature = "hosted"))]
pub fn emit_at(_category: Category, _name: &str, _t: u64, _by: u64, _fields: fmt::Arguments) {}

/// Writes one event whatever the mask (something going wrong that the
/// stream must show: a wrong clock): `{"t":..,"cpu":..,"by":..,<fields>}`,
/// `fields` naming the event (`"ev":"..."`) themselves.
#[cfg(not(any(test, feature = "hosted")))]
pub fn emit_always(fields: fmt::Arguments) {
    let t = crate::time::raw_monotonic_ns();
    let (cpu, by) = (crate::arch::percpu::cpu_index(), crate::arch::percpu::client());
    let mut port = crate::arch::debugcon::DEBUGCON.lock();
    let _ = writeln!(port, r#"{{"t":{t},"cpu":{cpu},"by":{by},{fields}}}"#);
    WRITTEN.fetch_add(1, Ordering::Relaxed);
}

#[cfg(any(test, feature = "hosted"))]
pub fn emit_always(_fields: fmt::Arguments) {}

/// (gala-um records to a file instead: hosted::record_to.)
#[cfg(any(test, feature = "hosted"))]
pub fn emit_record(_bytes: &[u8]) {}

/// A piece of a recording (`record`, kernel/src/replay.rs) on the debug
/// console: `{"t":..,"cpu":..,"ev":"rec","hex":"<bytes>"}`, whatever the
/// event mask; xtask joins the pieces into `<iso>.rec`. Written without
/// allocating.
#[cfg(not(any(test, feature = "hosted")))]
pub fn emit_record(bytes: &[u8]) {
    let t = crate::time::raw_monotonic_ns();
    let cpu = crate::arch::percpu::cpu_index();
    let mut port = crate::arch::debugcon::DEBUGCON.lock();
    let _ = write!(port, r#"{{"t":{t},"cpu":{cpu},"ev":"rec","hex":""#);
    for b in bytes {
        let _ = write!(port, "{b:02x}");
    }
    let _ = writeln!(port, r#""}}"#);
}

/// A string escaped for JSON (quotes, backslashes, control characters).
pub struct Str<'a>(pub &'a str);

impl fmt::Display for Str<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

/// A UTF-16 string (e.g. an NT path) as a JSON string.
pub struct Utf16<'a>(pub &'a [u16]);

impl fmt::Display for Utf16<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in char::decode_utf16(self.0.iter().copied()) {
            match c.unwrap_or(char::REPLACEMENT_CHARACTER) {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(parse_mask("syscall,fault"), 17);
        assert_eq!(parse_mask("all"), u32::MAX);
        assert_eq!(parse_mask("nonsense"), 0);
    }

    #[test]
    fn escaping() {
        assert_eq!(alloc::format!("{}", Str("a\"b\\c\n")), r#""a\"b\\c\u000a""#);
        let path: alloc::vec::Vec<u16> = "C:\\x".encode_utf16().collect();
        assert_eq!(alloc::format!("{}", Utf16(&path)), r#""C:\\x""#);
    }
}

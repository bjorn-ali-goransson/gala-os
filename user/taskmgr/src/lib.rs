//! gala's Task Manager (ROADMAP TM; ARCHITECTURE "The task manager"):
//! the parts without system calls, so they run under `cargo test` on the
//! host.
//!
//! - [`parse_processes`] and [`parse_perf`]: the kernel's records
//!   (GalaTasks ops 0 and 1, `kernel/src/nt/tasks.rs`);
//! - [`Shares`]: each process's share of the processors between two
//!   samples, as Windows' Task Manager shows it (of all processors);
//! - [`rows`]: the Processes view's rows, grouped as Windows groups them
//!   (Apps, Background processes) with the Linux processes grouped by
//!   WSL distribution (gala's UI says "Linux": ARCHITECTURE "The WSL
//!   facade"), each group sorted by the chosen column and filtered by
//!   the search (a name, a PID); [`details`] the Details view's flat
//!   rows;
//! - the numbers' formats ([`memory`], [`percent`], [`cpu_time`],
//!   [`speed`]);
//! - [`History`], a fixed ring of samples, [`line`], its points in a
//!   box (a Performance graph's, a row's sparkline), and [`Trails`],
//!   one ring pair per process behind the CPU and Memory columns'
//!   sparklines.
//!
//! `no_std` with `alloc`: `taskmgr.exe` is freestanding (`gala_heap`).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;

/// The kernel's record flags (nt::tasks).
pub const FLAG_LINUX: u32 = 1;
pub const FLAG_WINDOW: u32 = 2;
pub const FLAG_ELEVATED: u32 = 4;
pub const FLAG_WOW64: u32 = 8;
pub const FLAG_HUNG: u32 = 16;
pub const FLAG_UNINTERRUPTIBLE: u32 = 32;

/// The layout version this program reads.
pub const VERSION: u32 = 1;

/// One process as the kernel lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proc {
    pub pid: u64,
    pub parent: u64,
    /// NT time (100 ns since 1601).
    pub create_time: u64,
    pub cpu_ns: u64,
    pub working_set: u64,
    pub commit: u64,
    pub hwnd: u64,
    pub threads: u32,
    /// Handles (Windows) or open fds (Linux).
    pub handles: u32,
    pub session: u32,
    pub flags: u32,
    pub uid: u32,
    pub image: String,
    pub command_line: String,
    pub user: String,
    /// The WSL distribution (empty: none).
    pub distro: String,
    /// Its taskbar window's title (empty: none).
    pub title: String,
}

impl Proc {
    pub fn is_linux(&self) -> bool {
        self.flags & FLAG_LINUX != 0
    }

    /// Not responding: a window that does not answer, or a Linux
    /// process in an uninterruptible wait.
    pub fn not_responding(&self) -> bool {
        self.flags & (FLAG_HUNG | FLAG_UNINTERRUPTIBLE) != 0
    }

    /// The name the Processes view shows: an app's window title (no
    /// version resource gives a description here), else its image.
    pub fn name(&self) -> &str {
        if self.group() == Group::Apps && !self.title.is_empty() {
            &self.title
        } else {
            &self.image
        }
    }

    pub fn group(&self) -> Group {
        if self.is_linux() {
            Group::Linux(self.distro.clone())
        } else if self.flags & FLAG_WINDOW != 0 {
            Group::Apps
        } else {
            Group::Background
        }
    }

    /// Its platform as the Details view says it.
    pub fn platform(&self) -> &'static str {
        if self.is_linux() {
            "Linux"
        } else if self.flags & FLAG_WOW64 != 0 {
            "Windows (32-bit)"
        } else {
            "Windows"
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The processes' answer: (the monotonic time in ns, the processors,
/// the processes). None for another layout or a short buffer.
pub fn parse_processes(b: &[u8]) -> Option<(u64, u32, Vec<Proc>)> {
    if u32_at(b, 0)? != VERSION {
        return None;
    }
    let count = u32_at(b, 4)? as usize;
    let size = u32_at(b, 8)? as usize;
    let cpus = u32_at(b, 12)?;
    let now = u64_at(b, 16)?;
    let mut out = Vec::new();
    for i in 0..count {
        let r = 32 + size * i;
        let text = |k: usize| -> Option<String> {
            let at = u32_at(b, r + 80 + 8 * k)? as usize;
            let n = u32_at(b, r + 84 + 8 * k)? as usize;
            Some(String::from_utf8_lossy(b.get(at..at + n)?).into_owned())
        };
        out.push(Proc {
            pid: u64_at(b, r)?,
            parent: u64_at(b, r + 8)?,
            create_time: u64_at(b, r + 16)?,
            cpu_ns: u64_at(b, r + 24)?,
            working_set: u64_at(b, r + 32)?,
            commit: u64_at(b, r + 40)?,
            hwnd: u64_at(b, r + 48)?,
            threads: u32_at(b, r + 56)?,
            handles: u32_at(b, r + 60)?,
            session: u32_at(b, r + 64)?,
            flags: u32_at(b, r + 68)?,
            uid: u32_at(b, r + 72)?,
            image: text(0)?,
            command_line: text(1)?,
            user: text(2)?,
            distro: text(3)?,
            title: text(4)?,
        });
    }
    Some((now, cpus, out))
}

/// The machine's load as the kernel gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Perf {
    pub now_ns: u64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub commit: u64,
    pub disk_read: u64,
    pub disk_written: u64,
    pub net_received: u64,
    pub net_sent: u64,
    pub has_disk: bool,
    pub has_network: bool,
    pub processes: u32,
    pub threads: u32,
    pub handles: u32,
    /// Per processor: (busy, idle) ns.
    pub cpus: Vec<(u64, u64)>,
}

pub fn parse_perf(b: &[u8]) -> Option<Perf> {
    if u32_at(b, 0)? != VERSION {
        return None;
    }
    let n = u32_at(b, 4)? as usize;
    let devices = u32_at(b, 72)?;
    let mut cpus = Vec::new();
    for i in 0..n {
        cpus.push((u64_at(b, 128 + 16 * i)?, u64_at(b, 136 + 16 * i)?));
    }
    Some(Perf {
        now_ns: u64_at(b, 8)?,
        memory_total: u64_at(b, 16)?,
        memory_used: u64_at(b, 24)?,
        commit: u64_at(b, 32)?,
        disk_read: u64_at(b, 40)?,
        disk_written: u64_at(b, 48)?,
        net_received: u64_at(b, 56)?,
        net_sent: u64_at(b, 64)?,
        has_disk: devices & 1 != 0,
        has_network: devices & 2 != 0,
        processes: u32_at(b, 76)?,
        threads: u32_at(b, 80)?,
        handles: u32_at(b, 84)?,
        cpus,
    })
}

/// Busy share of a processor between two samples, in per mille.
pub fn cpu_busy(before: (u64, u64), after: (u64, u64)) -> u32 {
    let busy = after.0.saturating_sub(before.0);
    let idle = after.1.saturating_sub(before.1);
    ((busy * 1000).checked_div(busy + idle).unwrap_or(0)).min(1000) as u32
}

/// Each process's CPU share between the previous sample and this one,
/// in per mille of all processors (Windows' Task Manager: 100% is every
/// processor busy).
#[derive(Default)]
pub struct Shares {
    before: Vec<(u64, u64)>,
    before_ns: u64,
}

impl Shares {
    /// The shares for `procs` (sampled at `now_ns` on `cpus`
    /// processors), in their order; the sample is kept for the next
    /// call. The first call (no sample before) gives zeros.
    pub fn update(&mut self, now_ns: u64, cpus: u32, procs: &[Proc]) -> Vec<u32> {
        let span = now_ns.saturating_sub(self.before_ns) * u64::from(cpus.max(1));
        let first = self.before.is_empty() && self.before_ns == 0;
        let shares = procs
            .iter()
            .map(|p| {
                let was = self.before.iter().find(|(pid, _)| *pid == p.pid).map(|&(_, ns)| ns);
                match was {
                    Some(was) if !first && span > 0 => {
                        ((p.cpu_ns.saturating_sub(was) * 1000) / span).min(1000) as u32
                    }
                    // A new process: its time so far since the sample.
                    None if !first && span > 0 => ((p.cpu_ns * 1000) / span).min(1000) as u32,
                    _ => 0,
                }
            })
            .collect();
        self.before = procs.iter().map(|p| (p.pid, p.cpu_ns)).collect();
        self.before_ns = now_ns;
        shares
    }
}

/// A group of the Processes view.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Apps,
    Background,
    /// Linux processes of a WSL distribution (named), or of the Linux
    /// personality itself (empty name).
    Linux(String),
}

impl Group {
    /// Its heading (without the count).
    pub fn heading(&self) -> String {
        match self {
            Group::Apps => String::from("Apps"),
            Group::Background => String::from("Background processes"),
            Group::Linux(d) if d.is_empty() => String::from("Linux"),
            Group::Linux(d) => {
                let mut s = String::from("Linux: ");
                s.push_str(d);
                s
            }
        }
    }
}

/// The Processes and Details views' columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    Memory,
    CpuTime,
    Commit,
    Threads,
    Handles,
    Platform,
    CommandLine,
}

impl Column {
    pub fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Pid => "PID",
            Column::Status => "Status",
            Column::User => "User name",
            Column::Cpu => "CPU",
            Column::Memory => "Memory",
            Column::CpuTime => "CPU time",
            Column::Commit => "Commit size",
            Column::Threads => "Threads",
            Column::Handles => "Handles",
            Column::Platform => "Platform",
            Column::CommandLine => "Command line",
        }
    }

    /// Numbers sort largest first at the first click, as Windows does.
    pub fn descending_first(self) -> bool {
        matches!(
            self,
            Column::Cpu
                | Column::Memory
                | Column::CpuTime
                | Column::Commit
                | Column::Threads
                | Column::Handles
        )
    }
}

/// How the rows are sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

impl Sort {
    /// A click on `column`'s header: the same column flips, another
    /// starts in its first direction.
    pub fn clicked(self, column: Column) -> Sort {
        if self.column == column {
            Sort { column, descending: !self.descending }
        } else {
            Sort { column, descending: column.descending_first() }
        }
    }
}

/// The text a cell shows.
pub fn cell(p: &Proc, share: u32, column: Column, processes_view: bool) -> String {
    let mut s = String::new();
    match column {
        Column::Name => s.push_str(if processes_view { p.name() } else { &p.image }),
        Column::Pid => {
            let _ = write!(s, "{}", p.pid);
        }
        Column::Status => s.push_str(if p.not_responding() {
            "Not responding"
        } else if processes_view {
            ""
        } else {
            "Running"
        }),
        Column::User => s.push_str(&p.user),
        Column::Cpu => s = percent(share),
        Column::Memory => s = memory(p.working_set),
        Column::CpuTime => s = cpu_time(p.cpu_ns),
        Column::Commit => s = memory(p.commit),
        Column::Threads => {
            let _ = write!(s, "{}", p.threads);
        }
        Column::Handles => {
            let _ = write!(s, "{}", p.handles);
        }
        Column::Platform => {
            s.push_str(p.platform());
            if !p.distro.is_empty() {
                s.push_str(" (");
                s.push_str(&p.distro);
                s.push(')');
            }
        }
        Column::CommandLine => s.push_str(&p.command_line),
    }
    s
}

fn compare(a: &Proc, sa: u32, b: &Proc, sb: u32, column: Column, processes_view: bool) -> Ordering {
    let lower = |s: &str| -> String { s.chars().flat_map(char::to_lowercase).collect() };
    let by = match column {
        Column::Name => {
            let (x, y) = if processes_view { (a.name(), b.name()) } else { (&*a.image, &*b.image) };
            lower(x).cmp(&lower(y))
        }
        Column::Pid => a.pid.cmp(&b.pid),
        Column::Status => a.not_responding().cmp(&b.not_responding()),
        Column::User => lower(&a.user).cmp(&lower(&b.user)),
        Column::Cpu => sa.cmp(&sb),
        Column::Memory => a.working_set.cmp(&b.working_set),
        Column::CpuTime => a.cpu_ns.cmp(&b.cpu_ns),
        Column::Commit => a.commit.cmp(&b.commit),
        Column::Threads => a.threads.cmp(&b.threads),
        Column::Handles => a.handles.cmp(&b.handles),
        Column::Platform => a.platform().cmp(b.platform()).then(a.distro.cmp(&b.distro)),
        Column::CommandLine => lower(&a.command_line).cmp(&lower(&b.command_line)),
    };
    // Ties in PID order, whichever way.
    by.then(a.pid.cmp(&b.pid))
}

/// Whether `p` matches the search `filter` (empty: everything): a part
/// of its shown name or image (any case), or its PID's digits.
pub fn matches(p: &Proc, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let lower = |s: &str| -> String { s.chars().flat_map(char::to_lowercase).collect() };
    let f = lower(filter);
    if lower(p.name()).contains(&f) || lower(&p.image).contains(&f) {
        return true;
    }
    let mut pid = String::new();
    let _ = write!(pid, "{}", p.pid);
    pid.starts_with(&f)
}

/// A row of the Processes view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A group's heading and how many processes it shows.
    Heading(Group, usize),
    /// A process (its index in the list).
    Process(usize),
}

/// The Processes view's rows: the groups in Windows' order (Apps,
/// Background processes, then the Linux groups by name, the Linux
/// personality's own first), each heading followed by its processes
/// sorted by `sort`; only what `filter` matches, and no empty group.
pub fn rows(procs: &[Proc], shares: &[u32], sort: Sort, filter: &str) -> Vec<Row> {
    let mut groups: Vec<Group> = Vec::new();
    for p in procs {
        let g = p.group();
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    groups.sort();
    let mut out = Vec::new();
    for g in groups {
        let mut members: Vec<usize> = (0..procs.len())
            .filter(|&i| procs[i].group() == g && matches(&procs[i], filter))
            .collect();
        if members.is_empty() {
            continue;
        }
        sort_indices(&mut members, procs, shares, sort, true);
        out.push(Row::Heading(g, members.len()));
        out.extend(members.into_iter().map(Row::Process));
    }
    out
}

/// The Details view's rows: every process that `filter` matches, sorted.
pub fn details(procs: &[Proc], shares: &[u32], sort: Sort, filter: &str) -> Vec<usize> {
    let mut all: Vec<usize> = (0..procs.len()).filter(|&i| matches(&procs[i], filter)).collect();
    sort_indices(&mut all, procs, shares, sort, false);
    all
}

fn sort_indices(v: &mut [usize], procs: &[Proc], shares: &[u32], sort: Sort, processes_view: bool) {
    let share = |i: usize| shares.get(i).copied().unwrap_or(0);
    v.sort_by(|&a, &b| {
        let o = compare(&procs[a], share(a), &procs[b], share(b), sort.column, processes_view);
        if sort.descending { o.reverse() } else { o }
    });
}

/// Bytes as Task Manager shows memory: "0.1 MB" up to a gigabyte, then
/// "1.2 GB".
pub fn memory(bytes: u64) -> String {
    let mut s = String::new();
    let mb10 = bytes * 10 / (1024 * 1024);
    if mb10 < 10240 {
        let _ = write!(s, "{}.{} MB", mb10 / 10, mb10 % 10);
    } else {
        let gb10 = bytes * 10 / (1024 * 1024 * 1024);
        let _ = write!(s, "{}.{} GB", gb10 / 10, gb10 % 10);
    }
    s
}

/// Per mille as a percent: "0%", "4.5%"... as Task Manager's CPU column
/// ("0%" for nothing, one decimal otherwise).
pub fn percent(per_mille: u32) -> String {
    let mut s = String::new();
    if per_mille == 0 {
        s.push_str("0%");
    } else {
        let _ = write!(s, "{}.{}%", per_mille / 10, per_mille % 10);
    }
    s
}

/// CPU time as the Details view shows it: hours, minutes, seconds.
pub fn cpu_time(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let mut s = String::new();
    let _ = write!(s, "{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    s
}

/// A rate in bytes per second as "12 KB/s".
pub fn speed(bytes_per_second: u64) -> String {
    let mut s = String::new();
    if bytes_per_second < 1024 {
        let _ = write!(s, "{bytes_per_second} B/s");
    } else if bytes_per_second < 1024 * 1024 {
        let _ = write!(s, "{} KB/s", bytes_per_second / 1024);
    } else {
        let _ =
            write!(s, "{}.{} MB/s", bytes_per_second >> 20, ((bytes_per_second * 10) >> 20) % 10);
    }
    s
}

/// A history of samples for a graph or a sparkline: a fixed ring of
/// `cap` of them (a minute at one a second), the newest last. Its
/// buffer is taken once, when the ring is made, and written in place
/// from then on, so a row's line costs nothing per sample however long
/// its process lives.
pub struct History {
    values: Vec<u32>,
    /// The oldest sample's place once the ring is full (0 until then).
    head: usize,
    cap: usize,
}

impl History {
    pub fn new(cap: usize) -> History {
        let cap = cap.max(1);
        History { values: Vec::with_capacity(cap), head: 0, cap }
    }

    /// How many samples it holds at most: the line's width in samples.
    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn push(&mut self, v: u32) {
        if self.values.len() < self.cap {
            self.values.push(v);
        } else {
            self.values[self.head] = v;
            self.head = (self.head + 1) % self.cap;
        }
    }

    /// The samples it holds, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.values[self.head..].iter().chain(&self.values[..self.head]).copied()
    }

    pub fn last(&self) -> u32 {
        let n = self.values.len();
        if n == 0 { 0 } else { self.values[(self.head + n - 1) % n] }
    }

    pub fn max(&self) -> u32 {
        self.values.iter().copied().max().unwrap_or(0)
    }
}

/// A history's points in a box `w` by `height` (its own coordinates, y
/// down from its top left corner): one per sample held, the newest at
/// the box's right edge, each point's height its sample's share of
/// `scale` (clamped to it).
///
/// `over` is how many samples the box is wide. A graph passes the
/// ring's whole `cap`, so a history still filling draws at the right of
/// the box and grows leftwards as Windows' Performance graphs do, under
/// gridlines that mark the minute. A row's cell passes the samples it
/// holds, which spreads them over the cell: a minute at a sample every
/// pixel-and-a-bit would be a dot in the corner for the first seconds,
/// and a cell has no axis to read the time off anyway.
///
/// A history with no samples has no points: nothing is drawn for it,
/// where a history of zeroes is a flat line on the floor. A box one
/// sample wide is that sample, flat from edge to edge.
pub fn line(h: &History, scale: u32, w: i32, height: i32, over: usize) -> Vec<(i32, i32)> {
    if w < 1 || height < 1 {
        return Vec::new();
    }
    let n = h.len();
    let columns = i64::from(w - 1);
    let floor = u64::try_from(height - 1).unwrap_or(0);
    let up = |v: u32| (u64::from(v.min(scale)) * floor / u64::from(scale.max(1))) as i32;
    let mut points = Vec::with_capacity(n.max(2));
    if over < 2 {
        if !h.is_empty() {
            let y = height - 1 - up(h.last());
            points.push((0, y));
            points.push((w - 1, y));
        }
        return points;
    }
    for (k, v) in h.iter().enumerate() {
        let x = ((over.saturating_sub(n) + k) as i64 * columns / (over - 1) as i64) as i32;
        points.push((x, height - 1 - up(v)));
    }
    points
}

/// The steps a pixel is divided into when a line is drawn smoothly:
/// `line` over a box `SUB` times as high gives each point's height in
/// these, and [`stroke`] turns a column of them into pixels.
pub const SUB: i32 = 16;

/// What one column of a sparkline paints ([`stroke`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Stroke {
    /// The first and last pixel row the line itself covers. The rows
    /// between them are wholly its own.
    pub top: i32,
    pub bottom: i32,
    /// How much of `top` and of `bottom` the line covers, in per mille,
    /// and how much of `bottom` lies under it (the area under the line).
    /// In `top`, what is neither is what was behind the line.
    pub covers_top: u32,
    pub covers_bottom: u32,
    pub under: u32,
}

/// What a column paints, from the line's height at its left and right
/// edge (`a` and `b`, in `SUB`ths of a pixel down from the box's top:
/// the centre of the line there, which the caller takes from [`line`]
/// over a box `SUB` times as high but a pixel shorter, half a pixel
/// down) in a box `height` pixels high.
///
/// The line is a pixel thick about the samples it joins, and a column
/// where it climbs covers every row it passes: a line that rises
/// steeply is a line and not a dotted trail. A row the line covers only
/// in part is for the caller to blend, which is what keeps a shallow
/// slope from looking like a staircase.
pub fn stroke(a: i32, b: i32, height: i32) -> Stroke {
    let floor = height.max(1) * SUB;
    let half = SUB / 2;
    let top = (a.min(b) - half).clamp(0, floor - 1);
    let bottom = (a.max(b) + half).clamp(top + 1, floor);
    let row_top = top / SUB;
    let row_bottom = (bottom - 1) / SUB;
    let covers = |row: i32| {
        let (start, end) = (row * SUB, (row + 1) * SUB);
        u32::try_from((bottom.min(end) - top.max(start)).max(0) * 1000 / SUB).unwrap_or(0)
    };
    let end = (row_bottom + 1) * SUB;
    let under = u32::try_from((end - bottom).max(0) * 1000 / SUB).unwrap_or(0);
    Stroke {
        top: row_top,
        bottom: row_bottom,
        covers_top: covers(row_top),
        covers_bottom: covers(row_bottom),
        under,
    }
}

/// The CPU and Memory columns' sample rings, one pair per process: made
/// when a process is first listed and dropped when it is gone, so they
/// never outlast the list and a long session's cost is the rows it
/// shows. A pid and its start time name a row, since a pid given out
/// again is another process and not the one whose line was being drawn.
#[derive(Default)]
pub struct Trails {
    cap: usize,
    of: Vec<Trail>,
}

struct Trail {
    pid: u64,
    create_time: u64,
    cpu: History,
    memory: History,
}

impl Trails {
    pub fn new(cap: usize) -> Trails {
        Trails { cap, of: Vec::new() }
    }

    /// A sample for every process listed -- its share of the processors
    /// (`shares`, by the same index) and its working set as a per mille
    /// of `memory_total` -- and the rows that have ended dropped.
    pub fn update(&mut self, procs: &[Proc], shares: &[u32], memory_total: u64) {
        self.of.retain(|t| procs.iter().any(|p| p.pid == t.pid && p.create_time == t.create_time));
        for (i, p) in procs.iter().enumerate() {
            let share = shares.get(i).copied().unwrap_or(0);
            let used = (p.working_set.saturating_mul(1000) / memory_total.max(1)) as u32;
            let trail = match self.of.iter_mut().position(|t| t.is(p)) {
                Some(k) => &mut self.of[k],
                None => {
                    self.of.push(Trail {
                        pid: p.pid,
                        create_time: p.create_time,
                        cpu: History::new(self.cap),
                        memory: History::new(self.cap),
                    });
                    self.of.last_mut().unwrap_or_else(|| unreachable!())
                }
            };
            trail.cpu.push(share);
            trail.memory.push(used);
        }
    }

    /// `p`'s CPU ring, none before its first sample.
    pub fn cpu(&self, p: &Proc) -> Option<&History> {
        self.of.iter().find(|t| t.is(p)).map(|t| &t.cpu)
    }

    /// `p`'s memory ring (per mille of the machine's memory).
    pub fn memory(&self, p: &Proc) -> Option<&History> {
        self.of.iter().find(|t| t.is(p)).map(|t| &t.memory)
    }

    /// The Memory column's scale: the largest sample the processes
    /// listed hold, at least `floor` per mille, so their lines are
    /// comparable with each other. 100 % of the machine's memory is no
    /// use here: gala's processes are megabytes of a machine that has
    /// gigabytes, which would leave every line on the floor.
    pub fn memory_scale<'a>(&self, listed: impl Iterator<Item = &'a Proc>, floor: u32) -> u32 {
        listed.filter_map(|p| self.memory(p)).map(History::max).max().unwrap_or(0).max(floor)
    }

    /// How many processes have rings.
    pub fn len(&self) -> usize {
        self.of.len()
    }

    pub fn is_empty(&self) -> bool {
        self.of.is_empty()
    }
}

impl Trail {
    fn is(&self, p: &Proc) -> bool {
        self.pid == p.pid && self.create_time == p.create_time
    }
}

/// What End task does with a selected row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// TerminateProcess.
    Windows(u64),
    /// SIGTERM, then SIGKILL if it is still there after the grace time.
    Linux(u64),
    /// The distribution's whole tree.
    Distribution(String),
    /// Nothing to end (Apps, Background processes, the Linux
    /// personality's own group).
    Nothing,
}

/// End task's target for `row`.
pub fn end_of(row: &Row, procs: &[Proc]) -> End {
    match row {
        Row::Heading(Group::Linux(d), _) if !d.is_empty() => End::Distribution(d.clone()),
        Row::Heading(..) => End::Nothing,
        Row::Process(i) => match procs.get(*i) {
            Some(p) if p.is_linux() => End::Linux(p.pid),
            Some(p) => End::Windows(p.pid),
            None => End::Nothing,
        },
    }
}

/// How long a Linux process has after SIGTERM before SIGKILL, in ms.
pub const SIGTERM_GRACE_MS: u64 = 3000;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn p(pid: u64, image: &str, flags: u32, distro: &str) -> Proc {
        Proc {
            pid,
            image: image.into(),
            flags,
            distro: distro.into(),
            title: if flags & FLAG_WINDOW != 0 { "A window".into() } else { String::new() },
            ..Proc::default()
        }
    }

    /// The kernel's encoding (nt::tasks::encode, whose host test checks
    /// the same offsets) built by hand.
    fn record(b: &mut Vec<u8>, texts_at: &mut Vec<u8>, p: &Proc) {
        let base = b.len();
        b.resize(base + 128, 0);
        let mut put = |at: usize, v: &[u8]| b[base + at..base + at + v.len()].copy_from_slice(v);
        put(0, &p.pid.to_le_bytes());
        put(8, &p.parent.to_le_bytes());
        put(24, &p.cpu_ns.to_le_bytes());
        put(32, &p.working_set.to_le_bytes());
        put(56, &p.threads.to_le_bytes());
        put(68, &p.flags.to_le_bytes());
        for (k, t) in [&p.image, &p.command_line, &p.user, &p.distro, &p.title].iter().enumerate() {
            texts_at.extend_from_slice(t.as_bytes());
            let _ = (k, t);
        }
    }

    #[test]
    fn parses_the_kernels_records() {
        let procs = [
            Proc {
                pid: 4,
                parent: 1,
                cpu_ns: 9,
                working_set: 4096,
                threads: 2,
                flags: FLAG_WINDOW,
                ..p(4, "a.exe", FLAG_WINDOW, "")
            },
            Proc {
                command_line: "sleep 600".into(),
                user: "gala".into(),
                ..p(9, "sleep", FLAG_LINUX, "bb")
            },
        ];
        let mut b = vec![0u8; 32];
        b[0..4].copy_from_slice(&VERSION.to_le_bytes());
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        b[8..12].copy_from_slice(&128u32.to_le_bytes());
        b[12..16].copy_from_slice(&4u32.to_le_bytes());
        b[16..24].copy_from_slice(&123u64.to_le_bytes());
        let mut texts = Vec::new();
        for q in &procs {
            record(&mut b, &mut texts, q);
        }
        // The text slots: offsets from the buffer's start.
        let mut at = b.len();
        for (i, q) in procs.iter().enumerate() {
            for (k, t) in
                [&q.image, &q.command_line, &q.user, &q.distro, &q.title].iter().enumerate()
            {
                let r = 32 + 128 * i + 80 + 8 * k;
                b[r..r + 4].copy_from_slice(&(at as u32).to_le_bytes());
                b[r + 4..r + 8].copy_from_slice(&(t.len() as u32).to_le_bytes());
                at += t.len();
            }
        }
        b.extend_from_slice(&texts);
        let (now, cpus, got) = parse_processes(&b).unwrap();
        assert_eq!((now, cpus), (123, 4));
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].pid, got[0].parent, got[0].cpu_ns, got[0].working_set), (4, 1, 9, 4096));
        assert_eq!(got[0].image, "a.exe");
        assert_eq!(got[0].title, "A window");
        assert_eq!(got[1].command_line, "sleep 600");
        assert_eq!(got[1].distro, "bb");
        assert_eq!(got[1].user, "gala");
        assert!(parse_processes(&b[..20]).is_none());
        let mut other = b.clone();
        other[0] = 2;
        assert!(parse_processes(&other).is_none());
    }

    #[test]
    fn groups_as_windows_does_with_linux_by_distribution() {
        let procs = vec![
            p(10, "sleep", FLAG_LINUX, "bb"),
            p(4, "taskmgr.exe", FLAG_WINDOW, ""),
            p(5, "svc.exe", 0, ""),
            p(11, "sh", FLAG_LINUX, "alpine"),
            p(12, "init", FLAG_LINUX, ""),
            p(6, "cmd.exe", 0, ""),
        ];
        let shares = vec![0; procs.len()];
        let sort = Sort { column: Column::Name, descending: false };
        let r = rows(&procs, &shares, sort, "");
        assert_eq!(
            r,
            vec![
                Row::Heading(Group::Apps, 1),
                Row::Process(1),
                Row::Heading(Group::Background, 2),
                Row::Process(5), // cmd.exe
                Row::Process(2), // svc.exe
                Row::Heading(Group::Linux(String::new()), 1),
                Row::Process(4),
                Row::Heading(Group::Linux("alpine".into()), 1),
                Row::Process(3),
                Row::Heading(Group::Linux("bb".into()), 1),
                Row::Process(0),
            ]
        );
        assert_eq!(Group::Linux("bb".into()).heading(), "Linux: bb");
        assert_eq!(Group::Linux(String::new()).heading(), "Linux");
        // A filter keeps only matching processes and their groups.
        let r = rows(&procs, &shares, sort, "SLE");
        assert_eq!(r, vec![Row::Heading(Group::Linux("bb".into()), 1), Row::Process(0)]);
        // An app matches by its window title too, and by PID.
        assert_eq!(rows(&procs, &shares, sort, "a win").len(), 2);
        assert_eq!(
            rows(&procs, &shares, sort, "11"),
            vec![Row::Heading(Group::Linux("alpine".into()), 1), Row::Process(3)]
        );
    }

    #[test]
    fn sorting_within_groups() {
        let mut procs = vec![p(5, "b.exe", 0, ""), p(6, "a.exe", 0, ""), p(7, "c.exe", 0, "")];
        procs[0].working_set = 300;
        procs[1].working_set = 100;
        procs[2].working_set = 200;
        let shares = vec![5, 50, 0];
        let by = |column, descending| {
            rows(&procs, &shares, Sort { column, descending }, "")
                .into_iter()
                .filter_map(|r| match r {
                    Row::Process(i) => Some(procs[i].pid),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(by(Column::Name, false), vec![6, 5, 7]);
        assert_eq!(by(Column::Memory, true), vec![5, 7, 6]);
        assert_eq!(by(Column::Cpu, true), vec![6, 5, 7]);
        assert_eq!(by(Column::Pid, true), vec![7, 6, 5]);
        // The first click on a number column sorts largest first; a
        // second click flips.
        let s = Sort { column: Column::Name, descending: false }.clicked(Column::Cpu);
        assert_eq!(s, Sort { column: Column::Cpu, descending: true });
        assert_eq!(s.clicked(Column::Cpu), Sort { column: Column::Cpu, descending: false });
        assert_eq!(s.clicked(Column::Name), Sort { column: Column::Name, descending: false });
        // Details: flat.
        assert_eq!(
            details(&procs, &shares, Sort { column: Column::Pid, descending: false }, ""),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn shares_of_all_processors() {
        let mut s = Shares::default();
        let mut procs = vec![p(1, "a", 0, ""), p(2, "b", 0, "")];
        assert_eq!(s.update(1_000_000_000, 2, &procs), vec![0, 0]);
        // One second later on two processors: a ran 1 s (one processor
        // busy: 50%), b 0.2 s (10%).
        procs[0].cpu_ns = 1_000_000_000;
        procs[1].cpu_ns = 200_000_000;
        assert_eq!(s.update(2_000_000_000, 2, &procs), vec![500, 100]);
        // A new process counts its time so far.
        procs.push(Proc { cpu_ns: 100_000_000, ..p(3, "c", 0, "") });
        assert_eq!(s.update(3_000_000_000, 2, &procs), vec![0, 0, 50]);
        // A process whose time went back (an ended thread counted twice
        // for a moment) shows 0, not a wrap.
        procs[0].cpu_ns = 900_000_000;
        assert_eq!(s.update(4_000_000_000, 2, &procs)[0], 0);
    }

    #[test]
    fn processor_busy_share() {
        assert_eq!(cpu_busy((0, 0), (250, 750)), 250);
        assert_eq!(cpu_busy((250, 750), (250, 750)), 0);
        assert_eq!(cpu_busy((100, 0), (1100, 0)), 1000);
    }

    #[test]
    fn performance_record() {
        let mut b = vec![0u8; 128 + 32];
        b[0..4].copy_from_slice(&VERSION.to_le_bytes());
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        b[16..24].copy_from_slice(&(512u64 << 20).to_le_bytes());
        b[72..76].copy_from_slice(&2u32.to_le_bytes());
        b[76..80].copy_from_slice(&7u32.to_le_bytes());
        b[144..152].copy_from_slice(&5u64.to_le_bytes());
        let perf = parse_perf(&b).unwrap();
        assert_eq!(perf.memory_total, 512 << 20);
        assert!(!perf.has_disk && perf.has_network);
        assert_eq!(perf.processes, 7);
        assert_eq!(perf.cpus, vec![(0, 0), (5, 0)]);
    }

    #[test]
    fn formats() {
        assert_eq!(memory(0), "0.0 MB");
        assert_eq!(memory(1536 * 1024), "1.5 MB");
        assert_eq!(memory(3 << 30), "3.0 GB");
        assert_eq!(percent(0), "0%");
        assert_eq!(percent(45), "4.5%");
        assert_eq!(percent(1000), "100.0%");
        assert_eq!(cpu_time(3_725_000_000_000), "1:02:05");
        assert_eq!(speed(500), "500 B/s");
        assert_eq!(speed(2048), "2 KB/s");
        assert_eq!(speed(3 << 20), "3.0 MB/s");
    }

    #[test]
    fn end_task_targets() {
        let procs = vec![p(4, "a.exe", 0, ""), p(9, "sleep", FLAG_LINUX, "bb")];
        assert_eq!(end_of(&Row::Process(0), &procs), End::Windows(4));
        assert_eq!(end_of(&Row::Process(1), &procs), End::Linux(9));
        assert_eq!(
            end_of(&Row::Heading(Group::Linux("bb".into()), 1), &procs),
            End::Distribution("bb".into())
        );
        assert_eq!(end_of(&Row::Heading(Group::Linux(String::new()), 1), &procs), End::Nothing);
        assert_eq!(end_of(&Row::Heading(Group::Apps, 1), &procs), End::Nothing);
    }

    #[test]
    fn graph_history() {
        let mut h = History::new(3);
        for v in [1, 5, 2, 9] {
            h.push(v);
        }
        assert_eq!(h.iter().collect::<Vec<u32>>(), vec![5, 2, 9]);
        assert_eq!((h.len(), h.cap(), h.last(), h.max()), (3, 3, 9, 9));
    }

    /// The ring writes in place once it is full: the samples it hands
    /// out are the newest `cap`, oldest first, however many have gone
    /// through it.
    #[test]
    fn the_ring_keeps_the_newest_samples_in_order() {
        let mut h = History::new(4);
        assert!(h.is_empty());
        assert_eq!(h.iter().next(), None);
        for v in 1..=10 {
            h.push(v);
            assert_eq!(h.last(), v);
            assert_eq!(h.len(), (v as usize).min(4));
            let want: Vec<u32> = (v.saturating_sub(3).max(1)..=v).collect();
            assert_eq!(h.iter().collect::<Vec<u32>>(), want, "after {v}");
        }
    }

    #[test]
    fn a_line_rises_with_its_samples() {
        let mut h = History::new(5);
        for v in [0, 250, 500, 750, 1000] {
            h.push(v);
        }
        // A box 5 wide and 5 high: a column per sample, the floor at 4,
        // the top at 0.
        assert_eq!(line(&h, 1000, 5, 5, h.cap()), vec![(0, 4), (1, 3), (2, 2), (3, 1), (4, 0)]);
    }

    #[test]
    fn an_idle_line_is_flat_on_the_floor() {
        let mut h = History::new(5);
        for _ in 0..5 {
            h.push(0);
        }
        let points = line(&h, 1000, 5, 5, h.cap());
        assert!(points.iter().all(|&(_, y)| y == 4), "{points:?}");
    }

    /// A history that is still filling draws at the right of its box,
    /// as the Performance graphs do: the newest sample is always at the
    /// right edge, and a history with no samples draws nothing at all
    /// (an empty line and a line of zeroes are not the same picture).
    #[test]
    fn a_filling_line_grows_from_the_right() {
        let mut h = History::new(5);
        assert_eq!(line(&h, 1000, 9, 5, h.cap()), vec![]);
        h.push(1000);
        assert_eq!(line(&h, 1000, 9, 5, h.cap()), vec![(8, 0)]);
        h.push(0);
        assert_eq!(line(&h, 1000, 9, 5, h.cap()), vec![(6, 0), (8, 4)]);
    }

    /// A line level in its box is a pixel thick, and the rows it
    /// covers in part are left for the caller to blend: that is what
    /// keeps a shallow slope from looking like a staircase.
    #[test]
    fn a_level_line_is_a_pixel_thick() {
        // Half way down a box 4 pixels high, exactly on a pixel's
        // boundary: one row covered whole, the rest under it.
        let s = stroke(2 * SUB, 2 * SUB, 4);
        assert_eq!((s.top, s.bottom), (1, 2));
        assert_eq!((s.covers_top, s.covers_bottom, s.under), (500, 500, 500));
        // Half a pixel lower: one row, half of it under the line.
        let s = stroke(2 * SUB + SUB / 2, 2 * SUB + SUB / 2, 4);
        assert_eq!((s.top, s.bottom), (2, 2));
        assert_eq!((s.covers_top, s.covers_bottom, s.under), (1000, 1000, 0));
    }

    /// A column where the line climbs covers every row it passes, so
    /// the line is joined.
    #[test]
    fn a_climbing_column_is_joined() {
        let s = stroke(3 * SUB, 0, 4);
        assert_eq!((s.top, s.bottom), (0, 3));
        assert_eq!(s.under, 500);
    }

    /// The floor and the ceiling hold the line, a whole pixel of it:
    /// an idle history is a line along the bottom row with nothing
    /// under it, a full one a line along the top row.
    #[test]
    fn the_box_holds_the_line() {
        let s = stroke(3 * SUB + SUB / 2, 3 * SUB + SUB / 2, 4);
        assert_eq!((s.top, s.bottom), (3, 3));
        assert_eq!((s.covers_top, s.under), (1000, 0));
        let s = stroke(SUB / 2, SUB / 2, 4);
        assert_eq!((s.top, s.bottom), (0, 0));
        assert_eq!((s.covers_top, s.under), (1000, 0));
    }

    /// A cell's line is as wide as the samples it holds, so it is a
    /// line from the first seconds and not a dot in the corner; the
    /// newest sample is still at the right edge.
    #[test]
    fn a_cells_line_spreads_the_samples_it_holds() {
        let mut h = History::new(60);
        h.push(1000);
        // One sample: the whole cell is that sample.
        assert_eq!(line(&h, 1000, 9, 5, h.len()), vec![(0, 0), (8, 0)]);
        h.push(0);
        assert_eq!(line(&h, 1000, 9, 5, h.len()), vec![(0, 0), (8, 4)]);
        h.push(1000);
        assert_eq!(line(&h, 1000, 9, 5, h.len()), vec![(0, 0), (4, 4), (8, 0)]);
    }

    #[test]
    fn a_line_is_clamped_to_its_scale() {
        let mut h = History::new(2);
        h.push(4000);
        h.push(2000);
        assert_eq!(line(&h, 1000, 2, 5, h.cap()), vec![(0, 0), (1, 0)]);
    }

    /// A row's rings are made when its process is first listed and
    /// dropped when it ends, so the rings never outlast the list.
    #[test]
    fn a_rows_samples_are_dropped_with_it() {
        let mut t = Trails::new(3);
        let mut procs = vec![p(4, "a.exe", 0, ""), p(9, "sleep", FLAG_LINUX, "bb")];
        procs[0].working_set = 50 << 20;
        t.update(&procs, &[250, 0], 1000 << 20);
        assert_eq!(t.len(), 2);
        assert_eq!(t.cpu(&procs[0]).map(|h| h.last()), Some(250));
        assert_eq!(t.memory(&procs[0]).map(|h| h.last()), Some(50));
        t.update(&procs, &[500, 0], 1000 << 20);
        assert_eq!(t.cpu(&procs[0]).unwrap().iter().collect::<Vec<u32>>(), vec![250, 500]);
        // sleep ends: its rings go with it, the other's stay.
        procs.pop();
        t.update(&procs, &[500, 0], 1000 << 20);
        assert_eq!(t.len(), 1);
        assert!(t.cpu(&p(9, "sleep", FLAG_LINUX, "bb")).is_none());
        assert_eq!(t.cpu(&procs[0]).unwrap().len(), 3);
        // The ring is fixed: a fourth sample drops the first.
        t.update(&procs, &[600, 0], 1000 << 20);
        assert_eq!(t.cpu(&procs[0]).unwrap().iter().collect::<Vec<u32>>(), vec![500, 500, 600]);
    }

    /// A pid Windows or Linux has given out again is another process,
    /// not the one whose line was being drawn (the key is the pid and
    /// its start time, as every other list of processes keys them).
    #[test]
    fn a_reused_pid_starts_its_own_line() {
        let mut t = Trails::new(3);
        let mut procs = vec![p(4, "a.exe", 0, "")];
        procs[0].create_time = 100;
        t.update(&procs, &[250], 1000 << 20);
        procs[0].create_time = 200;
        t.update(&procs, &[700], 1000 << 20);
        assert_eq!(t.len(), 1);
        assert_eq!(t.cpu(&procs[0]).unwrap().iter().collect::<Vec<u32>>(), vec![700]);
    }

    /// The Memory column's scale is the largest sample of the rows
    /// listed, so their lines are comparable with each other; 100 % of
    /// the machine's memory would leave every line on the floor.
    #[test]
    fn the_memory_column_scales_to_the_rows_shown() {
        let mut t = Trails::new(3);
        let mut procs = vec![p(4, "a.exe", 0, ""), p(9, "sleep", FLAG_LINUX, "bb")];
        procs[0].working_set = 100 << 20;
        procs[1].working_set = 4 << 20;
        t.update(&procs, &[0, 0], 1000 << 20);
        assert_eq!(t.memory_scale(procs.iter(), 10), 100);
        assert_eq!(t.memory_scale(procs[1..].iter(), 10), 10);
        assert_eq!(t.memory_scale(core::iter::empty(), 10), 10);
    }
}

//! gala's `taskmgr.exe` (ROADMAP TM step 2; ARCHITECTURE "The task
//! manager"): one window that lists every process gala runs, Windows and
//! Linux together, as Windows 11's Task Manager does, from the kernel's
//! one query (GalaTasks, `kernel/src/nt/tasks.rs`; the model is this
//! crate's library).
//!
//! - **Processes**: grouped as Windows groups them (Apps: a process with
//!   a window on the taskbar; Background processes), then the Linux
//!   processes grouped by WSL distribution ("Linux: <name>"; the Linux
//!   personality's own under "Linux"); columns Name, PID, Status, User
//!   name, CPU (each process's share of all processors, its cell tinted
//!   by it), Memory (the working set); the CPU and Memory cells carry
//!   the process's last minute as a sparkline behind the number; a click on a column's header sorts
//!   by it (again: the other way); the search box (typing anywhere goes
//!   there; Esc clears it) keeps the processes whose name or image has
//!   the text, or whose PID starts with it.
//! - **End task** (the button, or Delete) for the selected row: a Windows
//!   process gets TerminateProcess (exit code 1, as Windows' Task
//!   Manager); a Linux process SIGTERM, then SIGKILL if it is still there
//!   after `SIGTERM_GRACE_MS`; a distribution's heading its whole tree
//!   (`wsl --terminate`'s SIGKILL).
//! - **Performance**: a graph per processor (busy time from the
//!   scheduler), memory in use, disk and network throughput (the
//!   drivers' byte counters), a minute of history each, refreshed every
//!   second; a list on the left gives every metric its value and the
//!   same minute as a sparkline (as Windows' Performance sidebar),
//!   including the process, thread and handle counts.
//! - **Details**: every process in one flat list with the full columns
//!   (CPU time, commit size, threads, handles or fds, platform with the
//!   distribution, command line), sortable and searchable too.
//! - **Ask the agent** (ROADMAP TM step 6): a right click on a process's
//!   row, in Processes or Details, selects it and opens its menu (Esc or
//!   a click elsewhere closes it); its one item, "Ask the agent about
//!   this process", hands the row as text (`taskmgr::row_text`: its
//!   values and their last minute) to the docked agent panel by
//!   WM_COPYDATA, in `agent::handoff`'s process form, as Explorer's
//!   "Make the change" hands a request. The message is sent once the
//!   window procedure has let go of the state, since the panel may send
//!   back before it answers.
//!
//! One instance: a second start brings the first one's window forward.
//! Not shown (the kernel has no counter for them: recorded in the
//! ledger as `taskmgr.exe!Processes(<column>)` at start): per-process disk,
//! network, GPU and power columns.
//!
//! Observability: each line on standard output (`taskmgr: ...`): every
//! click and right click with the part hit (`click Menu.AskTheAgent`),
//! the menu opened (`menu for <image> (pid N)`), every hand-off to the
//! agent panel with the panel's window, its size and the answer
//! (`handed to the agent panel (...): TRUE`, `not taken by the agent
//! panel (...): FALSE`), or why there was none (no panel, the process
//! ended, a row too long), and the command line's bytes cut to fit.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;

use gala_ui::win::Gdi;
use gala_ui::{Canvas, Placed, Set};
use taskmgr::window::{
    self, CPU_COLOR, Load, Look, MEMORY_COLOR, Msg as ViewMsg, Picture, View, WHITE,
};
use taskmgr::{Column, End, Group, History, Perf, Proc, Row, Sender, Shares, Sort, Trails};

type Bool = i32;
type Handle = *mut c_void;
type Hwnd = *mut c_void;

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle;
    fn TerminateProcess(h: Handle, code: u32) -> Bool;
    fn GetTickCount() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(
        h: Handle,
        buf: *const u8,
        n: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> Bool;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, hdc: Handle) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: Bool) -> Bool;
    fn SetTimer(hwnd: Hwnd, id: usize, ms: u32, f: *const c_void) -> usize;
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaTasks(op: u32, arg: usize, buffer: *mut u8, length: u32, returned: *mut u32) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_TIMER: u32 = 0x0113;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_COPYDATA: u32 = 0x004a;
const WM_MOUSEWHEEL: u32 = 0x020a;
const VK_DELETE: usize = 0x2e;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;
const PROCESS_TERMINATE: u32 = 0x0001;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const SIGTERM: u32 = 15;
const SIGKILL: u32 = 9;

const HISTORY: usize = 60;
/// The Memory column's smallest scale, in per mille of the machine's
/// memory: below this the rows' lines stay on the floor instead of
/// magnifying a megabyte into a mountain.
const MEMORY_FLOOR: u32 = 10;
/// How far a graph's area under its line is mixed towards white, in per
/// mille, and the same for a row's cell, where the row's number is read
/// over it.
const GRAPH_SHADE: u32 = 600;
const CELL_SHADE: u32 = 820;
/// A graph's gridlines.
const GRID: u32 = 0xf0f0f0;

/// What is selected: a process (by PID) or a group's heading.
#[derive(Clone, PartialEq, Eq)]
enum Selection {
    None,
    Process(u64),
    Group(Group),
}

struct State {
    hwnd: Hwnd,
    size: (i32, i32),
    view: View,
    procs: Vec<Proc>,
    shares: Vec<u32>,
    sampler: Shares,
    sort: Sort,
    detail_sort: Sort,
    search: String,
    selection: Selection,
    scroll: usize,
    perf: Option<Perf>,
    cpu_total: History,
    cpus: Vec<History>,
    memory: History,
    disk: History,
    network: History,
    processes: History,
    threads: History,
    handles: History,
    /// The CPU and Memory columns' sparklines, a ring per process.
    trails: Trails,
    /// Linux processes sent SIGTERM: (pid, when SIGKILL follows).
    pending: Vec<(u64, u32)>,
    /// The process menu, open at this client point for the selected
    /// process.
    menu: Option<(i32, i32)>,
}

/// COPYDATASTRUCT.
#[repr(C)]
struct CopyData {
    kind: usize,
    size: u32,
    data: *const u8,
}

/// A process to hand to the agent panel, once the state is let go.
struct Handing {
    image: String,
    pid: u64,
    bytes: Vec<u8>,
}

struct Global(core::cell::UnsafeCell<Option<State>>);
// SAFETY: only the window's thread (the program's only one) touches it.
unsafe impl Sync for Global {}
static STATE: Global = Global(core::cell::UnsafeCell::new(None));

fn state() -> &'static mut State {
    // SAFETY: set before the window exists, used by its thread only, never
    // re-entered while borrowed (the window procedure takes it once per
    // message and calls nothing that dispatches messages).
    match unsafe { (*STATE.0.get()).as_mut() } {
        Some(s) => s,
        // SAFETY: ends the process.
        None => unsafe { ExitProcess(3) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// A line on standard output (`taskmgr: ...`).
fn log(text: &str) {
    let mut s = String::from("taskmgr: ");
    s.push_str(text);
    s.push_str("\r\n");
    let mut n = 0;
    // SAFETY: a valid buffer; the standard output handle (or none).
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            s.as_ptr(),
            s.len() as u32,
            &mut n,
            core::ptr::null_mut(),
        )
    };
}

/// The ledger's line for a column this program cannot show
/// (`taskmgr.exe!Processes(<column>)`, unsupported).
fn record_missing(column: &str) {
    let mut name = String::from("taskmgr.exe!Processes(");
    name.push_str(column);
    name.push(')');
    // SAFETY: a valid buffer with its length; a recording call.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, 0) };
}

// ---- The kernel's data ------------------------------------------------------

/// GalaTasks op `op`'s answer.
fn query(op: u32) -> Option<Vec<u8>> {
    let mut size = 32 * 1024;
    for _ in 0..4 {
        let mut buf = alloc::vec![0u8; size];
        let mut needed = 0;
        // SAFETY: a buffer of its stated size and an out pointer.
        let s = unsafe { GalaTasks(op, 0, buf.as_mut_ptr(), size as u32, &mut needed) };
        if s == STATUS_BUFFER_TOO_SMALL {
            size = needed as usize + 8192;
            continue;
        }
        if s != 0 {
            return None;
        }
        buf.truncate(needed as usize);
        return Some(buf);
    }
    None
}

impl State {
    /// A new sample: the processes, their shares, the load's histories.
    fn refresh(&mut self) {
        if let Some((now, cpus, procs)) = query(0).as_deref().and_then(taskmgr::parse_processes) {
            self.shares = self.sampler.update(now, cpus, &procs);
            self.procs = procs;
        }
        if let Some(perf) = query(1).as_deref().and_then(taskmgr::parse_perf) {
            if let Some(before) = &self.perf {
                let secs_ms = perf.now_ns.saturating_sub(before.now_ns) / 1_000_000;
                let mut total = 0u32;
                for (i, (a, b)) in before.cpus.iter().zip(&perf.cpus).enumerate() {
                    let busy = taskmgr::cpu_busy(*a, *b);
                    total += busy;
                    if self.cpus.len() <= i {
                        self.cpus.push(History::new(HISTORY));
                    }
                    self.cpus[i].push(busy);
                }
                self.cpu_total.push(total / perf.cpus.len().max(1) as u32);
                let used = perf.memory_used.saturating_mul(1000) / perf.memory_total.max(1);
                self.memory.push(used as u32);
                let per_second = |a: u64, b: u64| -> u32 {
                    (b.saturating_sub(a).saturating_mul(1000) / secs_ms.max(1))
                        .min(u64::from(u32::MAX)) as u32
                };
                self.disk.push(per_second(
                    before.disk_read + before.disk_written,
                    perf.disk_read + perf.disk_written,
                ));
                self.network.push(per_second(
                    before.net_received + before.net_sent,
                    perf.net_received + perf.net_sent,
                ));
                self.processes.push(perf.processes);
                self.threads.push(perf.threads);
                self.handles.push(perf.handles);
            }
            self.perf = Some(perf);
        }
        // The rows' own rings, the ended ones dropped with their rows.
        let total = self.perf.as_ref().map_or(0, |p| p.memory_total);
        self.trails.update(&self.procs, &self.shares, total);
        // SIGKILL for a Linux process still there after its grace time.
        // SAFETY: a plain call.
        let now = unsafe { GetTickCount() };
        let procs = &self.procs;
        self.pending.retain(|&(pid, at)| {
            if !procs.iter().any(|p| p.pid == pid) {
                return false;
            }
            if now.wrapping_sub(at) < 0x8000_0000 {
                // SAFETY: no buffer.
                unsafe {
                    GalaTasks(
                        2,
                        pid as usize,
                        core::ptr::null_mut(),
                        SIGKILL,
                        core::ptr::null_mut(),
                    )
                };
                return false;
            }
            true
        });
    }

    fn rows(&self) -> Vec<Row> {
        taskmgr::rows(&self.procs, &self.shares, self.sort, &self.search)
    }

    fn details(&self) -> Vec<usize> {
        taskmgr::details(&self.procs, &self.shares, self.detail_sort, &self.search)
    }

    /// The selected row, if it is still listed.
    fn selected_row(&self) -> Option<Row> {
        match &self.selection {
            Selection::None => None,
            Selection::Process(pid) => {
                self.procs.iter().position(|p| p.pid == *pid).map(Row::Process)
            }
            Selection::Group(g) => {
                self.rows().into_iter().find(|r| matches!(r, Row::Heading(h, _) if h == g))
            }
        }
    }

    /// End task on the selected row.
    fn end_task(&mut self) {
        let Some(row) = self.selected_row() else { return };
        match taskmgr::end_of(&row, &self.procs) {
            End::Windows(pid) => {
                // SAFETY: plain calls; the handle is closed.
                unsafe {
                    let h = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
                    if !h.is_null() {
                        TerminateProcess(h, 1);
                        CloseHandle(h);
                    }
                }
            }
            End::Linux(pid) => {
                // SAFETY: no buffer.
                let s = unsafe {
                    GalaTasks(
                        2,
                        pid as usize,
                        core::ptr::null_mut(),
                        SIGTERM,
                        core::ptr::null_mut(),
                    )
                };
                if s == 0 {
                    // SAFETY: a plain call.
                    let at =
                        unsafe { GetTickCount() }.wrapping_add(taskmgr::SIGTERM_GRACE_MS as u32);
                    self.pending.push((pid, at));
                }
            }
            End::Distribution(name) => {
                let mut n = 0;
                // SAFETY: the name's bytes with their length.
                unsafe { GalaTasks(3, 0, name.as_ptr().cast_mut(), name.len() as u32, &mut n) };
            }
            End::Nothing => return,
        }
        self.selection = Selection::None;
        self.refresh();
    }
}

// ---- Drawing ----------------------------------------------------------------

/// The window's view of `s`: what `window::view` is given.
fn with_look<R>(s: &State, f: impl FnOnce(&Look) -> R) -> R {
    let rows = if s.view == View::Processes { s.rows() } else { Vec::new() };
    let details = if s.view == View::Details { s.details() } else { Vec::new() };
    let selected = s.selected_row();
    let can_end = selected.as_ref().is_some_and(|r| taskmgr::end_of(r, &s.procs) != End::Nothing);
    let listed: Vec<&Proc> = if s.view == View::Processes {
        rows.iter()
            .filter_map(|r| match r {
                Row::Process(i) => s.procs.get(*i),
                Row::Heading(..) => None,
            })
            .collect()
    } else {
        details.iter().filter_map(|&i| s.procs.get(i)).collect()
    };
    let memory_scale = s.trails.memory_scale(listed.into_iter(), MEMORY_FLOOR);
    let look = Look {
        size: s.size,
        view: s.view,
        search: &s.search,
        procs: &s.procs,
        shares: &s.shares,
        sort: if s.view == View::Details { s.detail_sort } else { s.sort },
        rows: &rows,
        details: &details,
        scroll: s.scroll,
        selected: selected.as_ref(),
        can_end,
        memory_scale,
        perf: s.perf.as_ref(),
        load: load(s),
        menu: s.menu,
    };
    f(&look)
}

fn load(s: &State) -> Load<'_> {
    Load {
        cpu_total: &s.cpu_total,
        cpus: &s.cpus,
        memory: &s.memory,
        disk: &s.disk,
        network: &s.network,
        processes: &s.processes,
        threads: &s.threads,
        handles: &s.handles,
    }
}

/// The window's view laid out, measured with `g`; None (logged) when
/// gala_ui refuses it.
fn laid_out(s: &State, g: &mut Gdi) -> Option<Vec<Placed<ViewMsg>>> {
    let (w, h) = s.size;
    match with_look(s, |l| gala_ui::place(&window::view(l), w as f32, Some(h as f32), g)) {
        Ok(p) => Some(p),
        Err(e) => {
            let mut l = String::from("view: ");
            let _ = write!(l, "{e}");
            log(&l);
            None
        }
    }
}

/// gala_ui's canvas on the window's DC, which draws the view's
/// pictures (`window::Picture`): the rows' sparklines and
/// Performance's graphs.
struct Pictures<'a> {
    g: Gdi,
    s: &'a State,
}

impl Canvas for Pictures<'_> {
    fn fill(&mut self, rect: [i32; 4], color: u32, radius: i32) {
        self.g.fill(rect, color, radius);
    }
    fn frame(&mut self, rect: [i32; 4], color: u32, radius: i32) {
        self.g.frame(rect, color, radius);
    }
    fn text(&mut self, rect: [i32; 4], text: &str, color: u32, size: f32, bold: bool, set: Set) {
        self.g.text(rect, text, color, size, bold, set);
    }
    fn image(&mut self, [x, y, w, ht]: [i32; 4], src: &str) {
        let s = self.s;
        let g = &mut self.g;
        match Picture::parse(src) {
            // A row with no samples yet draws nothing, where an idle one
            // is a flat line on the cell's floor.
            Some(Picture::Trail { memory, proc, scale, back }) => {
                let Some(p) = s.procs.get(proc) else { return };
                let (history, color) = if memory {
                    (s.trails.memory(p), MEMORY_COLOR)
                } else {
                    (s.trails.cpu(p), CPU_COLOR)
                };
                if let Some(h) = history {
                    spark(g, x, y, w, ht, h, scale, color, back, CELL_SHADE, h.len());
                }
            }
            Some(Picture::Graph { series, scale, color }) => {
                graph(g, x, y, w, ht, load(s).series(series), scale, color)
            }
            Some(Picture::Spark { series, scale, color }) => {
                g.fill([x, y, w, ht], WHITE, 0);
                g.frame([x, y, w, ht], window::mix(color, WHITE, 700), 0);
                if let Some(h) = load(s).series(series) {
                    let (iw, ih) = (w - 2, ht - 2);
                    spark(g, x + 1, y + 1, iw, ih, h, scale, color, WHITE, GRAPH_SHADE, h.cap());
                }
            }
            None => {
                let mut l = String::from("picture: not one of the view's: ");
                l.push_str(src);
                log(&l);
            }
        }
    }
}

fn paint(s: &State, hdc: Handle) {
    let mut g = Gdi::new(hdc);
    if let Some(p) = laid_out(s, &mut g) {
        gala_ui::paint(&p, None, &mut Pictures { g, s });
    }
}

/// `history` as a line in the box (x, y, w, h) over `back`, what is
/// behind it there: the area under the line in `color` mixed `shade`
/// per mille towards white, the line itself in `color`, a pixel thick
/// and joined from column to column (where it climbs it covers every
/// row it passes, so there are no gaps). `over` is how many samples the
/// box is wide (`taskmgr::line`).
///
/// A graph's area is the shade Windows' are; a row's cell is lighter,
/// since the cell's number is read over it.
///
/// The line is drawn in `taskmgr::SUB`ths of a pixel and a row it
/// covers in part is blended with what is behind it
/// (`taskmgr::stroke`): a minute in 94 pixels moves by a fraction of a
/// pixel a sample, and whole pixels would draw that as a staircase.
#[allow(clippy::too_many_arguments)]
fn spark(
    c: &mut dyn Canvas,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    history: &History,
    scale: u32,
    color: u32,
    back: u32,
    shade: u32,
    over: usize,
) {
    // A sample's height is a pixel's centre, so the whole pixel is the
    // line where the samples around it are level: `line` over the
    // centres (a pixel fewer) and half a pixel down.
    let half = taskmgr::SUB / 2;
    let points = taskmgr::line(history, scale, w, (h - 1) * taskmgr::SUB + 1, over);
    let (Some(&(first, _)), Some(&(last, _))) = (points.first(), points.last()) else {
        return;
    };
    let light = window::mix(color, WHITE, shade);
    // The line's height where each column meets the next, from the
    // samples it joins; a column is what `stroke` makes of its two.
    let mut edge: Vec<i32> = Vec::new();
    edge.resize(w as usize + 1, points[0].1);
    for pair in points.windows(2) {
        let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
        for px in ax..=bx {
            edge[px as usize] = ay + (by - ay) * (px - ax) / (bx - ax).max(1);
        }
    }
    // A row the line covers in part keeps the rest of what is in it:
    // the area under the line below, what was behind it above.
    let blend = |covers: u32, under: u32| {
        // What is left of the row is the area under the line and what
        // was behind it, in the proportion the two have there; a row
        // the line covers whole has none of either.
        let rest = 1000 - covers.min(1000);
        let base = match (under * 1000).checked_div(rest) {
            Some(share) => window::mix(back, light, share),
            None => light,
        };
        window::mix(base, color, covers)
    };
    for px in first..=last {
        let a = edge[px as usize] + half;
        let b = if px < last { edge[px as usize + 1] + half } else { a };
        let s = taskmgr::stroke(a, b, h);
        if s.top == s.bottom {
            c.fill([x + px, y + s.top, 1, 1], blend(s.covers_top, s.under), 0);
        } else {
            c.fill([x + px, y + s.top, 1, 1], window::mix(back, color, s.covers_top), 0);
            if s.bottom > s.top + 1 {
                c.fill([x + px, y + s.top + 1, 1, s.bottom - s.top - 1], color, 0);
            }
            c.fill([x + px, y + s.bottom, 1, 1], blend(s.covers_bottom, s.under), 0);
        }
        if s.bottom + 1 < h {
            c.fill([x + px, y + s.bottom + 1, 1, h - s.bottom - 1], light, 0);
        }
    }
}

/// A graph of `history` (per mille of `scale`) in (x, y, w, h): the
/// minute its ring holds, under gridlines, filling from the right.
#[allow(clippy::too_many_arguments)]
fn graph(
    c: &mut dyn Canvas,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    history: Option<&History>,
    scale: u32,
    color: u32,
) {
    c.fill([x, y, w, h], WHITE, 0);
    for k in 1..4 {
        c.fill([x, y + h * k / 4, w, 1], GRID, 0);
    }
    for k in 1..6 {
        c.fill([x + w * k / 6, y, 1, h], GRID, 0);
    }
    c.frame([x, y, w, h], color, 0);
    if let Some(history) = history {
        let white = WHITE;
        spark(
            c,
            x + 1,
            y + 1,
            w - 2,
            h - 2,
            history,
            scale,
            color,
            white,
            GRAPH_SHADE,
            history.cap(),
        );
    }
}

// ---- Input ------------------------------------------------------------------

/// What a click (or, `right`, a right click) at (x, y) hits: the part's
/// id (logged) and its message.
fn clicked(s: &State, x: i32, y: i32, right: bool) -> Option<ViewMsg> {
    // SAFETY: our window's DC, released below after the Gdi is dropped.
    let hdc = unsafe { GetDC(s.hwnd) };
    if hdc.is_null() {
        record_missing("GetDC (the window's layout)");
        return None;
    }
    let hit = laid_out(s, &mut Gdi::new(hdc)).and_then(|p| {
        let hit = if right { gala_ui::right_click(&p, x, y) } else { gala_ui::click(&p, x, y) };
        hit.map(|(id, m)| (String::from(id), m))
    });
    // SAFETY: the DC GetDC gave.
    unsafe { ReleaseDC(s.hwnd, hdc) };
    let (id, m) = hit?;
    let mut l = String::from(if right { "right click " } else { "click " });
    l.push_str(&id);
    log(&l);
    Some(m)
}

/// A right click at (x, y): a process's row is selected and its menu
/// opens there; anywhere else the menu closes.
fn right_click(s: &mut State, x: i32, y: i32) {
    s.menu = None;
    let Some(ViewMsg::Menu(Row::Process(i))) = clicked(s, x, y, true) else { return };
    let Some(p) = s.procs.get(i) else { return };
    s.selection = Selection::Process(p.pid);
    s.menu = Some((x, y));
    let mut l = String::new();
    let _ = write!(l, "menu for {} (pid {})", p.image, p.pid);
    log(&l);
}

/// "Ask the agent about this process" for the selected process: what to
/// hand to the panel, None (logged) when there is nothing to hand.
fn ask(s: &State) -> Option<Handing> {
    let Selection::Process(pid) = s.selection else { return None };
    let Some(i) = s.procs.iter().position(|p| p.pid == pid) else {
        let mut l = String::new();
        let _ = write!(l, "ask the agent about pid {pid}: it has ended");
        log(&l);
        return None;
    };
    let p = &s.procs[i];
    let share = s.shares.get(i).copied().unwrap_or(0);
    let total = s.perf.as_ref().map_or(0, |p| p.memory_total);
    let from = Sender { program: "Task Manager", title: "Task Manager", window: s.hwnd as u64 };
    let mut l = String::new();
    let Some(a) = taskmgr::ask(p, share, &s.trails, total, &from) else {
        let _ = write!(
            l,
            "ask the agent about {} (pid {}): its row does not fit in a hand-off",
            p.image, p.pid
        );
        log(&l);
        return None;
    };
    if a.cut > 0 {
        let _ = write!(
            l,
            "ask the agent about {} (pid {}): {} bytes of its command line cut to fit",
            p.image, p.pid, a.cut
        );
        log(&l);
    }
    Some(Handing { image: p.image.clone(), pid: p.pid, bytes: a.bytes })
}

/// The process goes to the docked agent panel (WM_COPYDATA), logged
/// with the panel's answer. Called with the state let go: the send waits
/// for the panel, which may send to this window meanwhile.
fn hand_off(hwnd: Hwnd, h: &Handing) {
    use agent::handoff;
    let mut l = String::new();
    let _ = write!(l, "ask the agent about {} (pid {}): ", h.image, h.pid);
    let (class, title) = (wide(handoff::CLASS), wide(handoff::TITLE));
    // SAFETY: valid strings.
    let panel = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
    if panel.is_null() {
        l.push_str("no agent panel to hand the process to");
        log(&l);
        return;
    }
    let cds = CopyData { kind: handoff::KIND, size: h.bytes.len() as u32, data: h.bytes.as_ptr() };
    // SAFETY: the panel's window; the structure and its bytes live across
    // the call (user32 copies them to the panel's process).
    let taken =
        unsafe { SendMessageW(panel, WM_COPYDATA, hwnd as usize, &cds as *const _ as isize) };
    let _ = write!(
        l,
        "{} the agent panel (window {:#x}, {} bytes): {}",
        if taken != 0 { "handed to" } else { "not taken by" },
        panel as usize,
        h.bytes.len(),
        if taken != 0 { "TRUE" } else { "FALSE" }
    );
    log(&l);
}

/// A click at (x, y); with the menu open, the click is the menu's (its
/// item asks, anywhere else closes it). What to hand to the agent panel
/// when the item was chosen.
fn click(s: &mut State, x: i32, y: i32) -> Option<Handing> {
    if s.menu.is_some() {
        let m = clicked(s, x, y, false);
        s.menu = None;
        return if m == Some(ViewMsg::Ask) { ask(s) } else { None };
    }
    match clicked(s, x, y, false) {
        Some(ViewMsg::Nav(v)) => {
            s.view = v;
            s.scroll = 0;
        }
        Some(ViewMsg::EndTask) => s.end_task(),
        Some(ViewMsg::Sort(c)) if s.view == View::Details => {
            s.detail_sort = s.detail_sort.clicked(c)
        }
        Some(ViewMsg::Sort(c)) => s.sort = s.sort.clicked(c),
        Some(ViewMsg::Select(Row::Heading(g, _))) => s.selection = Selection::Group(g),
        Some(ViewMsg::Select(Row::Process(i))) => {
            s.selection = s.procs.get(i).map_or(Selection::None, |p| Selection::Process(p.pid))
        }
        Some(ViewMsg::Deselect) => s.selection = Selection::None,
        // The menu's: only while it is open (above).
        Some(ViewMsg::Menu(_) | ViewMsg::Ask) | None => {}
    }
    None
}

fn row_count(s: &State) -> usize {
    match s.view {
        View::Processes => s.rows().len(),
        View::Details => s.details().len(),
        View::Performance => 0,
    }
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_CREATE => {
            // SAFETY: a plain call.
            unsafe { SetTimer(hwnd, 1, 1000, core::ptr::null()) };
            0
        }
        WM_SIZE => {
            let s = state();
            s.size = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let s = state();
            let mut r = Rect::default();
            // SAFETY: our window.
            unsafe { GetClientRect(hwnd, &mut r) };
            s.size = (r.right, r.bottom);
            // SAFETY: a paint structure for our window.
            let mut ps: PaintStruct = unsafe { core::mem::zeroed() };
            // SAFETY: our window.
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            paint(s, hdc);
            // SAFETY: the paint structure BeginPaint filled.
            unsafe { EndPaint(hwnd, &ps) };
            0
        }
        WM_TIMER => {
            state().refresh();
            redraw(hwnd);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
            let handing = click(state(), x, y);
            redraw(hwnd);
            // The state is let go: the panel may send back meanwhile.
            if let Some(h) = handing {
                hand_off(hwnd, &h);
            }
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
            right_click(state(), x, y);
            redraw(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            let s = state();
            let delta = ((wparam >> 16) & 0xffff) as i16;
            let max = row_count(s).saturating_sub(1);
            s.scroll = if delta > 0 { s.scroll.saturating_sub(3) } else { (s.scroll + 3).min(max) };
            redraw(hwnd);
            0
        }
        WM_KEYDOWN => {
            let s = state();
            match wparam {
                VK_DELETE => s.end_task(),
                VK_UP => s.scroll = s.scroll.saturating_sub(1),
                VK_DOWN => s.scroll = (s.scroll + 1).min(row_count(s).saturating_sub(1)),
                _ => return 0,
            }
            redraw(hwnd);
            0
        }
        WM_CHAR => {
            let s = state();
            match wparam {
                0x08 => {
                    s.search.pop();
                }
                // Esc closes the menu, else clears the search.
                0x1b if s.menu.is_some() => {
                    s.menu = None;
                    redraw(hwnd);
                    return 0;
                }
                0x1b => s.search.clear(),
                0x20..=0x7e if s.search.len() < 40 => s.search.push(wparam as u8 as char),
                _ => return 0,
            }
            s.scroll = 0;
            redraw(hwnd);
            0
        }
        WM_DESTROY => {
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default procedure.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn redraw(hwnd: Hwnd) {
    // SAFETY: our window.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let class = wide("TaskManagerWindow");
    let title = wide("Task Manager");
    // One instance: a second start brings the first forward.
    // SAFETY: valid strings; plain calls.
    unsafe {
        let other = FindWindowW(class.as_ptr(), core::ptr::null());
        if !other.is_null() {
            ShowWindow(other, 9);
            SetForegroundWindow(other);
            ExitProcess(0);
        }
    }
    let sort = Sort { column: Column::Name, descending: false };
    // SAFETY: the window does not exist yet: nothing else touches STATE.
    unsafe {
        *STATE.0.get() = Some(State {
            hwnd: core::ptr::null_mut(),
            size: (0, 0),
            view: View::Processes,
            procs: Vec::new(),
            shares: Vec::new(),
            sampler: Shares::default(),
            sort,
            detail_sort: sort,
            search: String::new(),
            selection: Selection::None,
            scroll: 0,
            perf: None,
            cpu_total: History::new(HISTORY),
            cpus: Vec::new(),
            memory: History::new(HISTORY),
            disk: History::new(HISTORY),
            network: History::new(HISTORY),
            processes: History::new(HISTORY),
            threads: History::new(HISTORY),
            handles: History::new(HISTORY),
            trails: Trails::new(HISTORY),
            pending: Vec::new(),
            menu: None,
        });
    }
    let s = state();
    s.refresh();
    for column in ["Disk", "Network", "GPU", "Power usage"] {
        record_missing(column);
    }
    let wc = WndClassW {
        style: 0,
        proc_: window_proc,
        class_extra: 0,
        window_extra: 0,
        instance: core::ptr::null_mut(),
        icon: core::ptr::null_mut(),
        // SAFETY: the stock arrow (IDC_ARROW).
        cursor: unsafe { LoadCursorW(core::ptr::null_mut(), 32512 as *const u16) },
        background: core::ptr::null_mut(),
        menu_name: core::ptr::null(),
        class_name: class.as_ptr(),
    };
    let null = core::ptr::null_mut();
    // SAFETY: a valid class and strings; the message loop.
    unsafe {
        RegisterClassW(&wc);
        // WS_OVERLAPPEDWINDOW at a fixed place (the screen checks click in it).
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0x00cf_0000,
            120,
            40,
            1040,
            700,
            null,
            null,
            null,
            null,
        );
        if hwnd.is_null() {
            ExitProcess(2);
        }
        state().hwnd = hwnd;
        ShowWindow(hwnd, 5);
        let mut msg: Msg = core::mem::zeroed();
        while GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process (no console to say it on: the kernel logs
    // the exit status).
    unsafe { ExitProcess(0xdead) }
}

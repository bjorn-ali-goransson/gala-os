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
//!   by it), Memory (the working set); a click on a column's header sorts
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
//!   second.
//! - **Details**: every process in one flat list with the full columns
//!   (CPU time, commit size, threads, handles or fds, platform with the
//!   distribution, command line), sortable and searchable too.
//!
//! One instance: a second start brings the first one's window forward.
//! Not shown (the kernel has no counter for them: recorded in the
//! ledger as `taskmgr.exe!Processes(<column>)` at start): per-process disk,
//! network, GPU and power columns.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;

use taskmgr::{Column, End, Group, History, Perf, Proc, Row, Shares, Sort};

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
    fn FillRect(hdc: Handle, rect: *const Rect, brush: Handle) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: Bool) -> Bool;
    fn SetTimer(hwnd: Hwnd, id: usize, ms: u32, f: *const c_void) -> usize;
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn CreatePen(style: i32, width: i32, color: u32) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> Bool;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn SetBkMode(hdc: Handle, mode: i32) -> i32;
    fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, n: i32) -> Bool;
    fn GetTextExtentPoint32W(hdc: Handle, text: *const u16, n: i32, size: *mut [i32; 2]) -> Bool;
    fn Polyline(hdc: Handle, points: *const [i32; 2], n: i32) -> Bool;
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
const WM_MOUSEWHEEL: u32 = 0x020a;
const VK_DELETE: usize = 0x2e;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;
const TRANSPARENT: i32 = 1;
const PROCESS_TERMINATE: u32 = 0x0001;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const SIGTERM: u32 = 15;
const SIGKILL: u32 = 9;

// Windows 11's light look (COLORREF: 0x00BBGGRR).
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}
const BACKGROUND: u32 = rgb(0xf3, 0xf3, 0xf3);
const CONTENT: u32 = rgb(0xfb, 0xfb, 0xfb);
const TEXT: u32 = rgb(0x1b, 0x1b, 0x1b);
const TEXT_DIM: u32 = rgb(0x5d, 0x5d, 0x5d);
const LINE: u32 = rgb(0xe5, 0xe5, 0xe5);
const ACCENT: u32 = rgb(0x00, 0x5f, 0xb8);
const SELECTED: u32 = rgb(0xcc, 0xe4, 0xf7);
const NAV_SELECTED: u32 = rgb(0xe3, 0xe3, 0xe3);
const HEADING: u32 = rgb(0xf0, 0xf0, 0xf0);
const WINDOWS_TILE: u32 = rgb(0x00, 0x78, 0xd4);
const LINUX_TILE: u32 = rgb(0xe9, 0x54, 0x20);
const HUNG: u32 = rgb(0xc4, 0x2b, 0x1c);
const CPU_COLOR: u32 = rgb(0x11, 0x7d, 0xbb);
const MEMORY_COLOR: u32 = rgb(0x8b, 0x12, 0xae);
const DISK_COLOR: u32 = rgb(0x4d, 0xa6, 0x0c);
const NETWORK_COLOR: u32 = rgb(0xa7, 0x4f, 0x01);

// The layout (client coordinates).
const TOP_H: i32 = 48;
const NAV_W: i32 = 176;
const NAV_Y: i32 = 56;
const NAV_H: i32 = 40;
const PAGE_Y: i32 = 56;
const COLS_Y: i32 = 100;
const COLS_H: i32 = 28;
const ROWS_Y: i32 = COLS_Y + COLS_H;
const ROW_H: i32 = 24;
const SEARCH: Rect = Rect { left: 260, top: 8, right: 640, bottom: 40 };
const BUTTON_W: i32 = 120;
const HISTORY: usize = 60;

const PROCESS_COLUMNS: &[(Column, i32)] = &[
    (Column::Name, 300),
    (Column::Pid, 70),
    (Column::Status, 120),
    (Column::User, 110),
    (Column::Cpu, 80),
    (Column::Memory, 110),
];
const DETAIL_COLUMNS: &[(Column, i32)] = &[
    (Column::Name, 126),
    (Column::Pid, 48),
    (Column::Status, 70),
    (Column::User, 64),
    (Column::Cpu, 52),
    (Column::CpuTime, 68),
    (Column::Memory, 72),
    (Column::Commit, 76),
    (Column::Threads, 56),
    (Column::Handles, 58),
    (Column::Platform, 120),
    (Column::CommandLine, 400),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Processes,
    Performance,
    Details,
}

const VIEWS: [(View, &str); 3] = [
    (View::Processes, "Processes"),
    (View::Performance, "Performance"),
    (View::Details, "Details"),
];

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
    /// Linux processes sent SIGTERM: (pid, when SIGKILL follows).
    pending: Vec<(u64, u32)>,
    font: Handle,
    bold: Handle,
    title_font: Handle,
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
            }
            self.perf = Some(perf);
        }
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

struct Dc(Handle);

impl Dc {
    fn fill(&self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let r = Rect { left: x, top: y, right: x + w, bottom: y + h };
        // SAFETY: a brush made and freed here.
        unsafe {
            let b = CreateSolidBrush(color);
            FillRect(self.0, &r, b);
            DeleteObject(b);
        }
    }

    fn frame(&self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        self.fill(x, y, w, 1, color);
        self.fill(x, y + h - 1, w, 1, color);
        self.fill(x, y, 1, h, color);
        self.fill(x + w - 1, y, 1, h, color);
    }

    fn width(&self, text: &[u16]) -> i32 {
        let mut size = [0i32; 2];
        // SAFETY: a valid string with its length.
        unsafe { GetTextExtentPoint32W(self.0, text.as_ptr(), text.len() as i32, &mut size) };
        size[0]
    }

    /// `text` at (x, y) in `font` and `color`, cut with "..." to `max`
    /// pixels; right-aligned in `max` with `right`.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, x: i32, y: i32, max: i32, text: &str, font: Handle, color: u32, right: bool) {
        // SAFETY: plain calls on our DC.
        unsafe {
            SelectObject(self.0, font);
            SetTextColor(self.0, color);
        }
        let mut t: Vec<u16> = text.encode_utf16().collect();
        if self.width(&t) > max {
            let dots: [u16; 3] = [46, 46, 46];
            while !t.is_empty() && self.width(&t) + self.width(&dots) > max {
                t.pop();
            }
            t.extend_from_slice(&dots);
        }
        let x = if right { x + max - self.width(&t) } else { x };
        // SAFETY: a valid string with its length.
        unsafe { TextOutW(self.0, x, y, t.as_ptr(), t.len() as i32) };
    }
}

/// A colour between `a` and `b` (`t` of 1000 towards `b`).
fn mix(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(1000);
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xff, (b >> s) & 0xff);
        ((x * (1000 - t) + y * t) / 1000) << s
    };
    ch(0) | ch(8) | ch(16)
}

fn paint(s: &State, dc: &Dc) {
    let (w, h) = s.size;
    dc.fill(0, 0, w, h, BACKGROUND);
    // The title and the search box.
    dc.text(16, 14, 200, "Task Manager", s.bold, TEXT, false);
    let r = SEARCH;
    dc.fill(r.left, r.top, r.right - r.left, r.bottom - r.top, rgb(0xff, 0xff, 0xff));
    dc.frame(r.left, r.top, r.right - r.left, r.bottom - r.top, rgb(0xd0, 0xd0, 0xd0));
    dc.fill(
        r.left + 1,
        r.bottom - 2,
        r.right - r.left - 2,
        1,
        if s.search.is_empty() { LINE } else { ACCENT },
    );
    if s.search.is_empty() {
        dc.text(
            r.left + 12,
            r.top + 8,
            356,
            "Type a name or PID to search",
            s.font,
            TEXT_DIM,
            false,
        );
    } else {
        let mut t = s.search.clone();
        t.push('|');
        dc.text(r.left + 12, r.top + 8, 356, &t, s.font, TEXT, false);
    }
    // The navigation.
    for (i, (view, name)) in VIEWS.iter().enumerate() {
        let y = NAV_Y + NAV_H * i as i32;
        if *view == s.view {
            dc.fill(6, y, NAV_W - 12, NAV_H - 4, NAV_SELECTED);
            dc.fill(6, y + 10, 3, NAV_H - 24, ACCENT);
        }
        dc.text(24, y + 11, NAV_W - 32, name, s.font, TEXT, false);
    }
    // The page.
    dc.fill(NAV_W, TOP_H, w - NAV_W, h - TOP_H, CONTENT);
    dc.fill(NAV_W, TOP_H, 1, h - TOP_H, LINE);
    dc.fill(NAV_W, TOP_H, w - NAV_W, 1, LINE);
    let title = VIEWS.iter().find(|(v, _)| *v == s.view).map_or("", |(_, n)| n);
    dc.text(NAV_W + 20, PAGE_Y + 4, 300, title, s.title_font, TEXT, false);
    match s.view {
        View::Processes => paint_processes(s, dc),
        View::Details => paint_details(s, dc),
        View::Performance => paint_performance(s, dc),
    }
}

/// The End task button (enabled with a selection that can end).
fn paint_button(s: &State, dc: &Dc) {
    let (w, _) = s.size;
    let x = w - BUTTON_W - 20;
    let enabled = s.selected_row().is_some_and(|r| taskmgr::end_of(&r, &s.procs) != End::Nothing);
    let (face, color) = if enabled { (ACCENT, rgb(0xff, 0xff, 0xff)) } else { (LINE, TEXT_DIM) };
    dc.fill(x, PAGE_Y, BUTTON_W, 32, face);
    dc.text(x + 28, PAGE_Y + 8, BUTTON_W - 28, "End task", s.font, color, false);
}

fn paint_header(s: &State, dc: &Dc, columns: &[(Column, i32)], sort: Sort, x0: i32) {
    let (w, _) = s.size;
    dc.fill(NAV_W + 1, COLS_Y, w - NAV_W - 1, COLS_H, CONTENT);
    dc.fill(NAV_W + 1, COLS_Y + COLS_H - 1, w - NAV_W - 1, 1, LINE);
    let mut x = x0;
    for &(column, width) in columns {
        if x >= w {
            break;
        }
        let mut t = String::from(column.title());
        if column == Column::Cpu
            && s.view == View::Processes
            && let Some(total) = s.cpu_total.values.last()
        {
            let _ = write!(t, " {}%", total / 10);
        }
        if sort.column == column {
            t.push_str(if sort.descending { " v" } else { " ^" });
        }
        let right = is_number(column);
        dc.text(x + 6, COLS_Y + 6, width - 12, &t, s.font, TEXT_DIM, right);
        dc.fill(x + width - 1, COLS_Y + 4, 1, COLS_H - 8, LINE);
        x += width;
    }
}

fn is_number(c: Column) -> bool {
    matches!(
        c,
        Column::Pid
            | Column::Cpu
            | Column::Memory
            | Column::CpuTime
            | Column::Commit
            | Column::Threads
            | Column::Handles
    )
}

/// One process's cells from `x0` on row `y`.
fn paint_cells(
    s: &State,
    dc: &Dc,
    i: usize,
    columns: &[(Column, i32)],
    x0: i32,
    y: i32,
    processes_view: bool,
) {
    let (w, _) = s.size;
    let p = &s.procs[i];
    let share = s.shares.get(i).copied().unwrap_or(0);
    let mut x = x0;
    for &(column, width) in columns {
        if x >= w {
            break;
        }
        let width = width.min(w - x);
        // CPU and memory cells tinted by their weight, as Windows does.
        if column == Column::Cpu && share > 0 {
            dc.fill(
                x,
                y,
                width,
                ROW_H,
                mix(rgb(0xff, 0xf4, 0xc4), rgb(0xff, 0xd3, 0x4f), share * 4),
            );
        }
        let mut text = taskmgr::cell(p, share, column, processes_view);
        let mut color = TEXT;
        if column == Column::Status && p.not_responding() {
            color = HUNG;
        }
        if column == Column::Name {
            // A tile with the first letter: blue for Windows, orange for Linux.
            let tile = if p.is_linux() { LINUX_TILE } else { WINDOWS_TILE };
            let indent = if processes_view { 22 } else { 6 };
            dc.fill(x + indent, y + 4, 16, 16, tile);
            let letter: String =
                text.chars().next().map(|c| c.to_ascii_uppercase()).into_iter().collect();
            dc.text(x + indent + 4, y + 4, 12, &letter, s.bold, rgb(0xff, 0xff, 0xff), false);
            let off = indent + 24;
            dc.text(x + off, y + 4, width - off - 6, &text, s.font, color, false);
        } else {
            if text.is_empty() {
                text.push(' ');
            }
            dc.text(x + 6, y + 4, width - 12, &text, s.font, color, is_number(column));
        }
        x += width;
    }
}

fn paint_processes(s: &State, dc: &Dc) {
    let (w, h) = s.size;
    paint_button(s, dc);
    let x0 = NAV_W + 8;
    paint_header(s, dc, PROCESS_COLUMNS, s.sort, x0);
    let rows = s.rows();
    let visible = ((h - ROWS_Y) / ROW_H).max(0) as usize;
    let selected = s.selected_row();
    for (k, row) in rows.iter().skip(s.scroll).take(visible).enumerate() {
        let y = ROWS_Y + ROW_H * k as i32;
        let is_selected = selected.as_ref() == Some(row);
        match row {
            Row::Heading(g, n) => {
                dc.fill(
                    NAV_W + 1,
                    y,
                    w - NAV_W - 1,
                    ROW_H,
                    if is_selected { SELECTED } else { HEADING },
                );
                let mut t = g.heading();
                let _ = write!(t, " ({n})");
                dc.text(
                    x0 + 6,
                    y + 4,
                    420,
                    &t,
                    s.bold,
                    if matches!(g, Group::Linux(_)) { LINUX_TILE } else { ACCENT },
                    false,
                );
            }
            Row::Process(i) => {
                if is_selected {
                    dc.fill(NAV_W + 1, y, w - NAV_W - 1, ROW_H, SELECTED);
                }
                paint_cells(s, dc, *i, PROCESS_COLUMNS, x0, y, true);
            }
        }
    }
    if rows.is_empty() {
        dc.text(x0 + 6, ROWS_Y + 8, 400, "No results", s.font, TEXT_DIM, false);
    }
}

fn paint_details(s: &State, dc: &Dc) {
    let (w, h) = s.size;
    paint_button(s, dc);
    let x0 = NAV_W + 8;
    paint_header(s, dc, DETAIL_COLUMNS, s.detail_sort, x0);
    let list = s.details();
    let visible = ((h - ROWS_Y) / ROW_H).max(0) as usize;
    for (k, &i) in list.iter().skip(s.scroll).take(visible).enumerate() {
        let y = ROWS_Y + ROW_H * k as i32;
        if s.selection == Selection::Process(s.procs[i].pid) {
            dc.fill(NAV_W + 1, y, w - NAV_W - 1, ROW_H, SELECTED);
        }
        paint_cells(s, dc, i, DETAIL_COLUMNS, x0, y, false);
    }
}

/// A graph of `values` (per mille of `scale`) in (x, y, w, h).
#[allow(clippy::too_many_arguments)]
fn graph(dc: &Dc, x: i32, y: i32, w: i32, h: i32, values: &[u32], scale: u32, color: u32) {
    dc.fill(x, y, w, h, rgb(0xff, 0xff, 0xff));
    for k in 1..4 {
        dc.fill(x, y + h * k / 4, w, 1, rgb(0xf0, 0xf0, 0xf0));
    }
    for k in 1..6 {
        dc.fill(x + w * k / 6, y, 1, h, rgb(0xf0, 0xf0, 0xf0));
    }
    dc.frame(x, y, w, h, color);
    if values.is_empty() {
        return;
    }
    let step = (w - 2) as f32 / (HISTORY - 1) as f32;
    let start = HISTORY - values.len();
    let mut points: Vec<[i32; 2]> = Vec::new();
    let light = mix(color, rgb(0xff, 0xff, 0xff), 600);
    for (k, &v) in values.iter().enumerate() {
        let px = x + 1 + ((start + k) as f32 * step) as i32;
        let vh = ((u64::from(v.min(scale)) * (h - 2) as u64) / u64::from(scale.max(1))) as i32;
        let py = y + h - 1 - vh;
        points.push([px, py]);
    }
    // The area under the line, then the line.
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        for px in a[0]..b[0] {
            let t = (px - a[0]) * 1000 / (b[0] - a[0]).max(1);
            let py = a[1] + (b[1] - a[1]) * t / 1000;
            dc.fill(px, py, 1, y + h - 1 - py, light);
        }
    }
    // SAFETY: a pen made, selected and freed here; valid points.
    unsafe {
        let pen = CreatePen(0, 1, color);
        let old = SelectObject(dc.0, pen);
        Polyline(dc.0, points.as_ptr(), points.len() as i32);
        SelectObject(dc.0, old);
        DeleteObject(pen);
    }
}

fn paint_performance(s: &State, dc: &Dc) {
    let (w, h) = s.size;
    let x0 = NAV_W + 20;
    let width = w - x0 - 20;
    let Some(perf) = &s.perf else { return };
    // CPU: the whole width, a graph per processor.
    let mut t = String::from("CPU");
    let _ = write!(t, "   {}%   {} processors", s.cpu_total.last() / 10, perf.cpus.len());
    dc.text(x0, COLS_Y, width, &t, s.bold, TEXT, false);
    let mut t = String::new();
    let _ = write!(
        t,
        "Processes {}   Threads {}   Handles {}",
        perf.processes, perf.threads, perf.handles
    );
    dc.text(x0, COLS_Y + 20, width, &t, s.font, TEXT_DIM, false);
    let n = perf.cpus.len().max(1) as i32;
    let per_row = n.min(4);
    let rows = (n + per_row - 1) / per_row;
    let top = COLS_Y + 44;
    let cpu_h = (h - top) / 2 - 24;
    let gw = (width - 8 * (per_row - 1)) / per_row;
    let gh = (cpu_h - 8 * (rows - 1)) / rows;
    for i in 0..n {
        let (col, row) = (i % per_row, i / per_row);
        let gx = x0 + col * (gw + 8);
        let gy = top + row * (gh + 8);
        let values = s.cpus.get(i as usize).map_or(&[][..], |h| &h.values[..]);
        graph(dc, gx, gy, gw, gh, values, 1000, CPU_COLOR);
        let mut t = String::from("CPU ");
        let _ = write!(t, "{}  {}%", i, s.cpus.get(i as usize).map_or(0, |h| h.last()) / 10);
        dc.text(gx + 6, gy + 4, gw - 12, &t, s.font, TEXT_DIM, false);
    }
    // Memory, disk, network side by side below.
    let by = top + cpu_h + 16;
    let bw = (width - 16) / 3;
    let bh = h - by - 52;
    let panels: [(&str, u32, &History, u32, String, bool); 3] = [
        (
            "Memory",
            MEMORY_COLOR,
            &s.memory,
            1000,
            {
                let mut t = taskmgr::memory(perf.memory_used);
                t.push_str(" of ");
                t.push_str(&taskmgr::memory(perf.memory_total));
                t.push_str(" in use");
                t
            },
            true,
        ),
        (
            "Disk",
            DISK_COLOR,
            &s.disk,
            s.disk.max().max(1 << 20),
            taskmgr::speed(u64::from(s.disk.last())),
            perf.has_disk,
        ),
        (
            "Network",
            NETWORK_COLOR,
            &s.network,
            s.network.max().max(64 << 10),
            taskmgr::speed(u64::from(s.network.last())),
            perf.has_network,
        ),
    ];
    for (k, (name, color, history, scale, value, present)) in panels.iter().enumerate() {
        let px = x0 + k as i32 * (bw + 8);
        dc.text(px, by, bw, name, s.bold, TEXT, false);
        let shown = if *present {
            value.clone()
        } else if k == 1 {
            String::from("No disk")
        } else {
            String::from("No network adapter")
        };
        dc.text(px, by + 20, bw, &shown, s.font, TEXT_DIM, false);
        graph(dc, px, by + 44, bw, bh, &history.values, *scale, *color);
    }
}

// ---- Input ------------------------------------------------------------------

/// A click at (x, y).
fn click(s: &mut State, x: i32, y: i32) {
    let (w, _) = s.size;
    for (i, (view, _)) in VIEWS.iter().enumerate() {
        let top = NAV_Y + NAV_H * i as i32;
        if x < NAV_W && y >= top && y < top + NAV_H {
            s.view = *view;
            s.scroll = 0;
            return;
        }
    }
    if s.view == View::Performance {
        return;
    }
    let bx = w - BUTTON_W - 20;
    if (bx..bx + BUTTON_W).contains(&x) && (PAGE_Y..PAGE_Y + 32).contains(&y) {
        s.end_task();
        return;
    }
    let (columns, processes_view) =
        if s.view == View::Processes { (PROCESS_COLUMNS, true) } else { (DETAIL_COLUMNS, false) };
    if (COLS_Y..COLS_Y + COLS_H).contains(&y) {
        let mut cx = NAV_W + 8;
        for &(column, width) in columns {
            if x >= cx && x < cx + width {
                if processes_view {
                    s.sort = s.sort.clicked(column);
                } else {
                    s.detail_sort = s.detail_sort.clicked(column);
                }
                return;
            }
            cx += width;
        }
        return;
    }
    if y >= ROWS_Y && x > NAV_W {
        let k = ((y - ROWS_Y) / ROW_H) as usize + s.scroll;
        s.selection = if processes_view {
            match s.rows().get(k) {
                Some(Row::Heading(g, _)) => Selection::Group(g.clone()),
                Some(Row::Process(i)) => Selection::Process(s.procs[*i].pid),
                None => Selection::None,
            }
        } else {
            s.details().get(k).map_or(Selection::None, |&i| Selection::Process(s.procs[i].pid))
        };
    }
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
            // SAFETY: our DC.
            unsafe { SetBkMode(hdc, TRANSPARENT) };
            paint(s, &Dc(hdc));
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
            click(state(), x, y);
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

fn font(height: i32, weight: i32) -> Handle {
    let face = wide("Segoe UI");
    // SAFETY: a valid face name.
    unsafe { CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, 0, 0, 0, 0, 0, face.as_ptr()) }
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
            pending: Vec::new(),
            font: font(-13, 400),
            bold: font(-13, 600),
            title_font: font(-20, 600),
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

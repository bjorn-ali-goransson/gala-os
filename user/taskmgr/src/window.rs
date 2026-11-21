//! Task Manager's window as a `gala_ui` view (ROADMAP LY step 3;
//! ARCHITECTURE "The task manager"): the title and the search box, the
//! navigation (Processes, Performance, Details), the page's title, End
//! task, the column headers, the rows, and Performance's graphs and list.
//! Every part that can be clicked has its automation id (SearchBox,
//! NavigationPane.<view>, EndTaskButton, ColumnHeader.<column>,
//! ProcessList, ProcessList.Process.<pid>, ProcessList.Group.<heading>,
//! and the process menu's Menu and Menu.AskTheAgent); `taskmgr.exe`
//! paints the laid-out view and routes clicks through `gala_ui::click`
//! and right clicks through `gala_ui::right_click`.
//!
//! A right click on a process's row opens its menu ([`Look::menu`], drawn
//! last so it is on top of the list): one item, "Ask the agent about this
//! process" ([`Msg::Ask`]), which hands the process to the agent panel.
//!
//! The sparklines and the graphs are images ([`Picture`]): the view says
//! where and of what, the program draws them (`Canvas::image`), since
//! they are the samples' lines, anti-aliased against what is behind them.
//!
//! The geometry is the window's as it was drawn by hand (the screen
//! checks of the `taskmgr` scenario hold): the top bar [`TOP_H`] high, the
//! navigation [`NAV_W`] wide with rows of [`NAV_H`] from [`NAV_Y`], the
//! column headers at [`COLS_Y`], rows of [`ROW_H`] from [`ROWS_Y`].

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use gala_ui::{Align, Justify, Node, button, column, image, row, text};

use crate::{Column, History, Perf, Proc, Row, Sort};

pub const BACKGROUND: u32 = 0xf3f3f3;
pub const CONTENT: u32 = 0xfbfbfb;
pub const TEXT: u32 = 0x1b1b1b;
pub const TEXT_DIM: u32 = 0x5d5d5d;
pub const LINE: u32 = 0xe5e5e5;
pub const ACCENT: u32 = 0x005fb8;
pub const SELECTED: u32 = 0xcce4f7;
pub const NAV_SELECTED: u32 = 0xe3e3e3;
pub const HEADING: u32 = 0xf0f0f0;
pub const WHITE: u32 = 0xffffff;
pub const WINDOWS_TILE: u32 = 0x0078d4;
pub const LINUX_TILE: u32 = 0xe95420;
pub const HUNG: u32 = 0xc42b1c;
pub const CPU_COLOR: u32 = 0x117dbb;
pub const MEMORY_COLOR: u32 = 0x8b12ae;
pub const DISK_COLOR: u32 = 0x4da60c;
pub const NETWORK_COLOR: u32 = 0xa74f01;
const CONTROL_BORDER: u32 = 0xd0d0d0;
const FONT: i32 = 13;
const TITLE: i32 = 20;

pub const TOP_H: i32 = 48;
pub const NAV_W: i32 = 176;
pub const NAV_Y: i32 = 56;
pub const NAV_H: i32 = 40;
pub const PAGE_Y: i32 = 56;
pub const COLS_Y: i32 = 100;
pub const COLS_H: i32 = 28;
pub const ROWS_Y: i32 = COLS_Y + COLS_H;
pub const ROW_H: i32 = 24;
/// The search box: x, y, width, height.
pub const SEARCH: [i32; 4] = [260, 8, 380, 32];
pub const BUTTON_W: i32 = 120;
/// Performance's list: its width and a row's height at most.
pub const SIDE_W: i32 = 196;
pub const SIDE_ROW_H: i32 = 56;
/// Where the columns start.
pub const LIST_X: i32 = NAV_W + 8;
/// The process menu: its width, its edge and an item's height.
pub const MENU_W: i32 = 260;
pub const MENU_EDGE: i32 = 4;
pub const MENU_ROW: i32 = 30;
/// The menu's item, as Windows' context menus put it.
pub const ASK_TEXT: &str = "Ask the agent about this process";

pub const PROCESS_COLUMNS: &[(Column, i32)] = &[
    (Column::Name, 300),
    (Column::Pid, 70),
    (Column::Status, 120),
    (Column::User, 110),
    (Column::Cpu, 80),
    (Column::Memory, 110),
];
pub const DETAIL_COLUMNS: &[(Column, i32)] = &[
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Processes,
    Performance,
    Details,
}

pub const VIEWS: [(View, &str); 3] = [
    (View::Processes, "Processes"),
    (View::Performance, "Performance"),
    (View::Details, "Details"),
];

/// What a click on the window does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Nav(View),
    EndTask,
    Sort(Column),
    /// A row: a process (its index) or a group's heading.
    Select(Row),
    /// The list below its rows: nothing selected.
    Deselect,
    /// A right click on a process's row (its index): its menu.
    Menu(Row),
    /// The menu's "Ask the agent about this process".
    Ask,
}

/// A metric with a minute of samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Series {
    CpuTotal,
    /// A processor's.
    Cpu(usize),
    Memory,
    Disk,
    Network,
    Processes,
    Threads,
    Handles,
}

/// The samples of every [`Series`], borrowed.
pub struct Load<'a> {
    pub cpu_total: &'a History,
    pub cpus: &'a [History],
    pub memory: &'a History,
    pub disk: &'a History,
    pub network: &'a History,
    pub processes: &'a History,
    pub threads: &'a History,
    pub handles: &'a History,
}

impl<'a> Load<'a> {
    pub fn series(&self, s: Series) -> Option<&'a History> {
        Some(match s {
            Series::CpuTotal => self.cpu_total,
            Series::Cpu(i) => return self.cpus.get(i),
            Series::Memory => self.memory,
            Series::Disk => self.disk,
            Series::Network => self.network,
            Series::Processes => self.processes,
            Series::Threads => self.threads,
            Series::Handles => self.handles,
        })
    }
}

/// What an image of the view shows: its `src`, which the program reads
/// back with [`Picture::parse`] to draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// A process's last minute behind its CPU or Memory cell: the
    /// process (its index), the line's scale (per mille of it at the
    /// top) and the cell's colour, which the line is blended with.
    Trail { memory: bool, proc: usize, scale: u32, back: u32 },
    /// A Performance graph: white, gridlines, framed in `color`.
    Graph { series: Series, scale: u32, color: u32 },
    /// A sparkline of Performance's list: white, framed lighter.
    Spark { series: Series, scale: u32, color: u32 },
}

fn series_name(s: Series, out: &mut String) {
    let _ = match s {
        Series::CpuTotal => write!(out, "cpu"),
        Series::Cpu(i) => write!(out, "cpu{i}"),
        Series::Memory => write!(out, "memory"),
        Series::Disk => write!(out, "disk"),
        Series::Network => write!(out, "network"),
        Series::Processes => write!(out, "processes"),
        Series::Threads => write!(out, "threads"),
        Series::Handles => write!(out, "handles"),
    };
}

fn parse_series(s: &str) -> Option<Series> {
    Some(match s {
        "cpu" => Series::CpuTotal,
        "memory" => Series::Memory,
        "disk" => Series::Disk,
        "network" => Series::Network,
        "processes" => Series::Processes,
        "threads" => Series::Threads,
        "handles" => Series::Handles,
        _ => Series::Cpu(s.strip_prefix("cpu")?.parse().ok()?),
    })
}

impl Picture {
    /// `trail:cpu:<proc>:<scale>:<back>`, `graph:<series>:<scale>:<color>`,
    /// `spark:<series>:<scale>:<color>` (colours in hex).
    pub fn src(&self) -> String {
        let mut s = String::new();
        match *self {
            Picture::Trail { memory, proc, scale, back } => {
                let m = if memory { "memory" } else { "cpu" };
                let _ = write!(s, "trail:{m}:{proc}:{scale}:{back:06x}");
            }
            Picture::Graph { series, scale, color } | Picture::Spark { series, scale, color } => {
                s.push_str(if matches!(self, Picture::Graph { .. }) { "graph:" } else { "spark:" });
                series_name(series, &mut s);
                let _ = write!(s, ":{scale}:{color:06x}");
            }
        }
        s
    }

    pub fn parse(src: &str) -> Option<Picture> {
        let mut f = src.split(':');
        let kind = f.next()?;
        let what = f.next()?;
        if kind == "trail" {
            let memory = match what {
                "cpu" => false,
                "memory" => true,
                _ => return None,
            };
            let proc = f.next()?.parse().ok()?;
            let scale = f.next()?.parse().ok()?;
            let back = u32::from_str_radix(f.next()?, 16).ok()?;
            return f.next().is_none().then_some(Picture::Trail { memory, proc, scale, back });
        }
        let series = parse_series(what)?;
        let scale = f.next()?.parse().ok()?;
        let color = u32::from_str_radix(f.next()?, 16).ok()?;
        if f.next().is_some() {
            return None;
        }
        match kind {
            "graph" => Some(Picture::Graph { series, scale, color }),
            "spark" => Some(Picture::Spark { series, scale, color }),
            _ => None,
        }
    }
}

/// What the window shows: the program's state, borrowed.
pub struct Look<'a> {
    /// The client area.
    pub size: (i32, i32),
    pub view: View,
    pub search: &'a str,
    pub procs: &'a [Proc],
    pub shares: &'a [u32],
    /// The shown view's sort.
    pub sort: Sort,
    /// Processes' rows.
    pub rows: &'a [Row],
    /// Details' rows (indices into `procs`).
    pub details: &'a [usize],
    pub scroll: usize,
    pub selected: Option<&'a Row>,
    /// End task has something to end.
    pub can_end: bool,
    /// The Memory column's sparklines' scale (the largest row listed).
    pub memory_scale: u32,
    pub perf: Option<&'a Perf>,
    pub load: Load<'a>,
    /// The process menu, open where it was asked for (the client
    /// point of the right click), for the selected row.
    pub menu: Option<(i32, i32)>,
}

/// The id of a part named `name`: the kind's id, a dot, the name
/// without spaces.
pub fn named_id(kind: &str, name: &str) -> String {
    let mut id = String::from(kind);
    id.push('.');
    id.extend(name.chars().filter(|c| *c != ' '));
    id
}

/// A colour between `a` and `b` (`t` of 1000 towards `b`).
pub fn mix(a: u32, b: u32, t: u32) -> u32 {
    let t = t.min(1000);
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 0xff, (b >> s) & 0xff);
        ((x * (1000 - t) + y * t) / 1000) << s
    };
    ch(0) | ch(8) | ch(16)
}

pub fn is_number(c: Column) -> bool {
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

/// An element at (x, y) of the window, `w` x `h`.
fn at(x: i32, y: i32, w: i32, h: i32) -> Node<Msg> {
    column().pin(x, y).width(w.max(0)).height(h.max(0))
}

/// One line of text at (x, y) cut to `w`.
fn label(t: impl Into<String>, x: i32, y: i32, w: i32) -> Node<Msg> {
    text(t).pin(x, y).width(w.max(0)).ellipsis()
}

/// The columns that show in the window's width: each with its width,
/// the last one cut at the window's edge.
fn shown(columns: &[(Column, i32)], w: i32) -> Vec<(Column, i32)> {
    let mut out = Vec::new();
    let mut x = LIST_X;
    for &(c, width) in columns {
        if x >= w {
            break;
        }
        out.push((c, width.min(w - x)));
        x += width;
    }
    out
}

/// The columns' row in a header or a row (which start at the page's
/// left edge, `NAV_W + 1`).
fn cells_row(w: i32, h: i32) -> Node<Msg> {
    row().pin(LIST_X - NAV_W - 1, 0).width((w - LIST_X).max(0)).height(h)
}

fn top(l: &Look) -> Node<Msg> {
    let [sx, sy, sw, sh] = SEARCH;
    let (hint, color) = if l.search.is_empty() {
        (String::from("Type a name or PID to search"), TEXT_DIM)
    } else {
        let mut t = String::from(l.search);
        t.push('|');
        (t, TEXT)
    };
    column()
        .child(label("Task Manager", 16, 14, 200).bold())
        .child(
            at(sx, sy, sw, sh)
                .id("SearchBox")
                .bg(WHITE)
                .border(CONTROL_BORDER)
                .child(label(hint, 12, 7, sw - 24).color(color))
                .child(at(1, sh - 2, sw - 2, 1).bg(if l.search.is_empty() {
                    LINE
                } else {
                    ACCENT
                })),
        )
        .children(VIEWS.iter().enumerate().map(|(i, &(v, name))| {
            let on = v == l.view;
            at(0, NAV_Y + NAV_H * i as i32, NAV_W, NAV_H)
                .id(named_id("NavigationPane", name))
                .on_click(Msg::Nav(v))
                .child_if(on, at(6, 0, NAV_W - 12, NAV_H - 4).bg(NAV_SELECTED))
                .child_if(on, at(6, 10, 3, NAV_H - 24).bg(ACCENT))
                .child(label(name, 24, 10, NAV_W - 32))
        }))
}

fn end_button(l: &Look) -> Node<Msg> {
    let (w, _) = l.size;
    let (face, color) = if l.can_end { (ACCENT, WHITE) } else { (LINE, TEXT_DIM) };
    button("EndTaskButton", "End task")
        .pin(w - BUTTON_W - 20, PAGE_Y)
        .width(BUTTON_W)
        .height(32)
        .bg(face)
        .color(color)
        .on_click(Msg::EndTask)
}

fn header(l: &Look, columns: &[(Column, i32)]) -> Node<Msg> {
    let (w, _) = l.size;
    let cells = shown(columns, w).into_iter().map(|(c, width)| {
        let mut t = String::from(c.title());
        if c == Column::Cpu && l.view == View::Processes && !l.load.cpu_total.is_empty() {
            let _ = write!(t, " {}%", l.load.cpu_total.last() / 10);
        }
        if l.sort.column == c {
            t.push_str(if l.sort.descending { " v" } else { " ^" });
        }
        row()
            .id(named_id("ColumnHeader", c.title()))
            .width(width)
            .height(COLS_H)
            .padding((0, 6))
            .align(Align::Center)
            .justify(if is_number(c) { Justify::End } else { Justify::Start })
            .on_click(Msg::Sort(c))
            .child(text(t).color(TEXT_DIM).ellipsis().min_width(0))
            .child(at(width - 1, 4, 1, COLS_H - 8).bg(LINE))
    });
    at(NAV_W + 1, COLS_Y, w - NAV_W - 1, COLS_H)
        .bg(CONTENT)
        .child(at(0, COLS_H - 1, w - NAV_W - 1, 1).bg(LINE))
        .child(cells_row(w, COLS_H).children(cells))
}

/// A process's cells over `back` (the row's colour).
fn cells(l: &Look, i: usize, processes_view: bool, back: u32) -> Vec<Node<Msg>> {
    let columns = if processes_view { PROCESS_COLUMNS } else { DETAIL_COLUMNS };
    let p = &l.procs[i];
    let share = l.shares.get(i).copied().unwrap_or(0);
    shown(columns, l.size.0)
        .into_iter()
        .map(|(c, width)| {
            let mut t = crate::cell(p, share, c, processes_view);
            let cell = row().width(width).height(ROW_H).align(Align::Center);
            if c == Column::Name {
                // A tile with the first letter: blue for Windows, orange
                // for Linux.
                let tile = if p.is_linux() { LINUX_TILE } else { WINDOWS_TILE };
                let indent = if processes_view { 22 } else { 6 };
                let letter: String =
                    t.chars().next().map(|c| c.to_ascii_uppercase()).into_iter().collect();
                let off = indent + 24;
                return cell
                    .child(
                        at(indent, 4, 16, 16)
                            .bg(tile)
                            .align(Align::Center)
                            .justify(Justify::Center)
                            .child(text(letter).bold().color(WHITE)),
                    )
                    .child(label(t, off, 4, width - off - 6));
            }
            // CPU and memory cells tinted by their weight, as Windows
            // does, the process's last minute behind the number
            // (ROADMAP TM): the CPU against all the processors, the
            // memory against the largest row listed.
            let mut under = back;
            let mut cell = cell.padding((0, 6)).justify(if is_number(c) {
                Justify::End
            } else {
                Justify::Start
            });
            if c == Column::Cpu && share > 0 {
                under = mix(0xfff4c4, 0xffd34f, share * 4);
                cell = cell.bg(under);
            }
            let trail = match c {
                Column::Cpu => Some((false, 1000)),
                Column::Memory => Some((true, l.memory_scale)),
                _ => None,
            };
            if let Some((memory, scale)) = trail {
                let src = Picture::Trail { memory, proc: i, scale, back: under }.src();
                cell = cell.child(image(src).pin(2, 3).width(width - 4).height(ROW_H - 6));
            }
            if t.is_empty() {
                t.push(' ');
            }
            let hung = c == Column::Status && p.not_responding();
            cell.child(text(t).color(if hung { HUNG } else { TEXT }).ellipsis().min_width(0))
        })
        .collect()
}

fn list(l: &Look) -> Node<Msg> {
    let (w, h) = l.size;
    let processes_view = l.view == View::Processes;
    let fits = ((h - ROWS_Y) / ROW_H).max(0) as usize;
    let row_of = |k: usize, id: String, on: bool, m: Row| {
        // A process's row has its menu; a group's heading has none.
        let menu = matches!(m, Row::Process(_)).then(|| Msg::Menu(m.clone()));
        let r = at(0, ROW_H * k as i32, w - NAV_W - 1, ROW_H)
            .id(id)
            .on_click(Msg::Select(m))
            .bg(if on { SELECTED } else { CONTENT });
        match menu {
            Some(m) => r.on_right_click(m),
            None => r,
        }
    };
    let rows: Vec<Node<Msg>> = if processes_view {
        l.rows
            .iter()
            .skip(l.scroll)
            .take(fits)
            .enumerate()
            .map(|(k, r)| {
                let on = l.selected == Some(r);
                match r {
                    Row::Heading(g, n) => {
                        let mut t = g.heading();
                        let id = named_id("ProcessList.Group", &t);
                        let _ = write!(t, " ({n})");
                        let linux = matches!(g, crate::Group::Linux(_));
                        row_of(k, id, on, r.clone()).bg(if on { SELECTED } else { HEADING }).child(
                            label(t, LIST_X - NAV_W + 5, 4, 420).bold().color(if linux {
                                LINUX_TILE
                            } else {
                                ACCENT
                            }),
                        )
                    }
                    Row::Process(i) => {
                        let back = if on { SELECTED } else { CONTENT };
                        let mut id = String::from("ProcessList.Process.");
                        let _ = write!(id, "{}", l.procs[*i].pid);
                        row_of(k, id, on, r.clone())
                            .child(cells_row(w, ROW_H).children(cells(l, *i, true, back)))
                    }
                }
            })
            .collect()
    } else {
        l.details
            .iter()
            .skip(l.scroll)
            .take(fits)
            .enumerate()
            .map(|(k, &i)| {
                let r = Row::Process(i);
                let on = l.selected == Some(&r);
                let back = if on { SELECTED } else { CONTENT };
                let mut id = String::from("ProcessList.Process.");
                let _ = write!(id, "{}", l.procs[i].pid);
                row_of(k, id, on, r).child(cells_row(w, ROW_H).children(cells(l, i, false, back)))
            })
            .collect()
    };
    let empty = processes_view && l.rows.is_empty();
    at(NAV_W + 1, ROWS_Y, w - NAV_W - 1, h - ROWS_Y)
        .id("ProcessList")
        .on_click(Msg::Deselect)
        .children(rows)
        .child_if(empty, label("No results", LIST_X - NAV_W + 5, 8, 400).color(TEXT_DIM))
}

/// Performance's list on the left (`SIDE_W` wide from `x`, a row every
/// `SIDE_ROW_H` at most from `top`): each processor, memory, disk,
/// network and the process, thread and handle counts, each with its
/// value and the last minute as a sparkline, as Windows' Performance
/// sidebar. CPU and memory are scaled to 100 %, the throughputs and the
/// counts to their own peak, which is written under the value.
fn side(l: &Look, x: i32, top: i32, perf: &Perf) -> Vec<Node<Msg>> {
    let ld = &l.load;
    // (name, value, peak, series, scale, colour)
    type Metric = (String, String, Option<String>, Series, u32, u32);
    let mut rows: Vec<Metric> = Vec::new();
    rows.push((
        String::from("CPU"),
        crate::percent(ld.cpu_total.last()),
        None,
        Series::CpuTotal,
        1000,
        CPU_COLOR,
    ));
    for (i, h) in ld.cpus.iter().enumerate() {
        let mut name = String::from("CPU ");
        let _ = write!(name, "{i}");
        rows.push((name, crate::percent(h.last()), None, Series::Cpu(i), 1000, CPU_COLOR));
    }
    let mut in_use = crate::percent(ld.memory.last());
    let _ = write!(in_use, " ({})", crate::memory(perf.memory_used));
    rows.push((String::from("Memory"), in_use, None, Series::Memory, 1000, MEMORY_COLOR));
    for (name, s, h, color, present, missing) in [
        ("Disk", Series::Disk, ld.disk, DISK_COLOR, perf.has_disk, "No disk"),
        (
            "Network",
            Series::Network,
            ld.network,
            NETWORK_COLOR,
            perf.has_network,
            "No network adapter",
        ),
    ] {
        let mut peak = String::from("peak ");
        peak.push_str(&crate::speed(u64::from(h.max())));
        let value = if present { crate::speed(u64::from(h.last())) } else { String::from(missing) };
        rows.push((String::from(name), value, present.then_some(peak), s, h.max().max(1), color));
    }
    for (name, s, h) in [
        ("Processes", Series::Processes, ld.processes),
        ("Threads", Series::Threads, ld.threads),
        ("Handles", Series::Handles, ld.handles),
    ] {
        let mut value = String::new();
        let _ = write!(value, "{}", h.last());
        let mut peak = String::from("peak ");
        let _ = write!(peak, "{}", h.max());
        rows.push((String::from(name), value, Some(peak), s, h.max().max(1), ACCENT));
    }
    let rh = SIDE_ROW_H.min((l.size.1 - top - 8) / rows.len().max(1) as i32);
    let mut out = Vec::new();
    for (k, (name, value, peak, series, scale, color)) in rows.into_iter().enumerate() {
        let ry = top + rh * k as i32;
        let bh = rh - 18;
        if bh < 8 {
            break;
        }
        out.push(label(name, x, ry, 96).bold());
        out.push(label(value, x, ry + 17, 96).color(TEXT_DIM));
        if let Some(peak) = peak {
            out.push(label(peak, x, ry + 32, 96).color(TEXT_DIM));
        }
        let src = Picture::Spark { series, scale, color }.src();
        out.push(image(src).pin(x + 100, ry).width(SIDE_W - 100).height(bh));
    }
    out
}

fn performance(l: &Look) -> Vec<Node<Msg>> {
    let (w, h) = l.size;
    let Some(perf) = l.perf else { return Vec::new() };
    let ld = &l.load;
    let sx = NAV_W + 20;
    let x0 = sx + SIDE_W + 12;
    let width = w - x0 - 20;
    let top = COLS_Y + 44;
    let mut out = side(l, sx, top, perf);
    // CPU: the whole width, a graph per processor.
    let mut t = String::from("CPU");
    let _ = write!(t, "   {}%   {} processors", ld.cpu_total.last() / 10, perf.cpus.len());
    out.push(label(t, x0, COLS_Y, width).bold());
    let mut t = String::new();
    let _ = write!(
        t,
        "Processes {}   Threads {}   Handles {}",
        perf.processes, perf.threads, perf.handles
    );
    out.push(label(t, x0, COLS_Y + 20, width).color(TEXT_DIM));
    let n = perf.cpus.len().max(1) as i32;
    let per_row = n.min(4);
    let rows = (n + per_row - 1) / per_row;
    let cpu_h = (h - top) / 2 - 24;
    let gw = (width - 8 * (per_row - 1)) / per_row;
    let gh = (cpu_h - 8 * (rows - 1)) / rows;
    for i in 0..n {
        let (col, row) = (i % per_row, i / per_row);
        let gx = x0 + col * (gw + 8);
        let gy = top + row * (gh + 8);
        let series = Series::Cpu(i as usize);
        let src = Picture::Graph { series, scale: 1000, color: CPU_COLOR }.src();
        out.push(image(src).pin(gx, gy).width(gw.max(0)).height(gh.max(0)));
        let mut t = String::from("CPU ");
        let _ = write!(t, "{}  {}%", i, ld.cpus.get(i as usize).map_or(0, |h| h.last()) / 10);
        out.push(label(t, gx + 6, gy + 4, gw - 12).color(TEXT_DIM));
    }
    // Memory, disk, network side by side below.
    let by = top + cpu_h + 16;
    let bw = (width - 16) / 3;
    let bh = h - by - 52;
    let mut memory = crate::memory(perf.memory_used);
    memory.push_str(" of ");
    memory.push_str(&crate::memory(perf.memory_total));
    memory.push_str(" in use");
    let panels = [
        ("Memory", Series::Memory, MEMORY_COLOR, 1000, memory, true, ""),
        (
            "Disk",
            Series::Disk,
            DISK_COLOR,
            ld.disk.max().max(1 << 20),
            crate::speed(u64::from(ld.disk.last())),
            perf.has_disk,
            "No disk",
        ),
        (
            "Network",
            Series::Network,
            NETWORK_COLOR,
            ld.network.max().max(64 << 10),
            crate::speed(u64::from(ld.network.last())),
            perf.has_network,
            "No network adapter",
        ),
    ];
    for (k, (name, series, color, scale, value, present, missing)) in panels.into_iter().enumerate()
    {
        let px = x0 + k as i32 * (bw + 8);
        out.push(label(name, px, by, bw).bold());
        let shown = if present { value } else { String::from(missing) };
        out.push(label(shown, px, by + 20, bw).color(TEXT_DIM));
        let src = Picture::Graph { series, scale, color }.src();
        out.push(image(src).pin(px, by + 44).width(bw.max(0)).height(bh.max(0)));
    }
    out
}

/// Where the process menu opened at the client point `(x, y)` goes: its
/// top left corner at the point, moved in so the whole menu is in the
/// window.
pub fn menu_at(size: (i32, i32), (x, y): (i32, i32)) -> (i32, i32) {
    let (w, h) = size;
    let mh = MENU_ROW + 2 * MENU_EDGE;
    (x.min(w - MENU_W).max(0), y.min(h - mh).max(0))
}

/// The process menu at `at` (from [`menu_at`]): white, framed, its one
/// item.
fn menu(size: (i32, i32), point: (i32, i32)) -> Node<Msg> {
    let (x, y) = menu_at(size, point);
    at(x, y, MENU_W, MENU_ROW + 2 * MENU_EDGE).id("Menu").bg(WHITE).border(CONTROL_BORDER).child(
        at(MENU_EDGE, MENU_EDGE, MENU_W - 2 * MENU_EDGE, MENU_ROW)
            .id("Menu.AskTheAgent")
            .on_click(Msg::Ask)
            .child(label(ASK_TEXT, 12, 7, MENU_W - 2 * MENU_EDGE - 24)),
    )
}

/// The window, laid out for `l.size`.
pub fn view(l: &Look) -> Node<Msg> {
    let (w, h) = l.size;
    let title = VIEWS.iter().find(|(v, _)| *v == l.view).map_or("", |(_, n)| n);
    let page = column()
        .child(at(NAV_W, TOP_H, w - NAV_W, h - TOP_H).bg(CONTENT))
        .child(at(NAV_W, TOP_H, 1, h - TOP_H).bg(LINE))
        .child(at(NAV_W, TOP_H, w - NAV_W, 1).bg(LINE))
        .child(label(title, NAV_W + 20, PAGE_Y + 4, 300).size(TITLE as f32).bold());
    let page = match l.view {
        View::Performance => page.children(performance(l)),
        View::Processes | View::Details => {
            let columns = if l.view == View::Processes { PROCESS_COLUMNS } else { DETAIL_COLUMNS };
            page.child(end_button(l)).child(header(l, columns)).child(list(l))
        }
    };
    column()
        .width(w)
        .height(h)
        .bg(BACKGROUND)
        .size(FONT as f32)
        .color(TEXT)
        .child(top(l))
        .child(page)
        .children(l.menu.map(|at| menu(l.size, at)))
}

#[cfg(test)]
mod tests;

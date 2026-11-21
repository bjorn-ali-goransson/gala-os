use super::*;
use crate::{FLAG_HUNG, FLAG_LINUX, FLAG_WINDOW, Group};
use alloc::vec;
use gala_ui::{Kind, Measure, Placed, place};

/// 7 px a character, lines 17 high (13 px text) or 27 (20 px).
struct Fixed;
impl Measure for Fixed {
    fn text(&mut self, t: &str, size: f32, _: bool, _: Option<f32>) -> (f32, f32) {
        (7.0 * t.chars().count() as f32, if size > 15.0 { 27.0 } else { 17.0 })
    }
}

/// The client area of the 1040 x 700 window.
const W: i32 = 1024;
const H: i32 = 661;

fn proc(pid: u64, image: &str, flags: u32) -> Proc {
    Proc {
        pid,
        image: String::from(image),
        flags,
        working_set: 4 << 20,
        threads: 1,
        ..Proc::default()
    }
}

struct Data {
    procs: Vec<Proc>,
    shares: Vec<u32>,
    rows: Vec<Row>,
    details: Vec<usize>,
    histories: Vec<History>,
    cpus: Vec<History>,
    perf: Perf,
}

fn data() -> Data {
    let procs = vec![
        proc(4, "busy.exe", FLAG_WINDOW | FLAG_HUNG),
        proc(8, "svchost.exe", 0),
        proc(12, "sh", FLAG_LINUX),
    ];
    let sort = Sort { column: Column::Name, descending: false };
    let shares = vec![500, 0, 0];
    let rows = crate::rows(&procs, &shares, sort, "");
    let details = crate::details(&procs, &shares, sort, "");
    let mut h = History::new(60);
    h.push(250);
    h.push(400);
    Data {
        procs,
        shares,
        rows,
        details,
        histories: (0..7)
            .map(|_| {
                let mut c = History::new(60);
                h.iter().for_each(|v| c.push(v));
                c
            })
            .collect(),
        cpus: vec![History::new(60), History::new(60)],
        perf: Perf {
            cpus: vec![(0, 0), (0, 0)],
            memory_total: 1 << 30,
            memory_used: 1 << 28,
            ..Perf::default()
        },
    }
}

fn look<'a>(d: &'a Data, view: View, selected: Option<&'a Row>) -> Look<'a> {
    let h = &d.histories;
    Look {
        size: (W, H),
        view,
        search: "",
        procs: &d.procs,
        shares: &d.shares,
        sort: Sort { column: Column::Name, descending: false },
        rows: &d.rows,
        details: &d.details,
        scroll: 0,
        selected,
        can_end: selected.is_some(),
        memory_scale: 1000,
        perf: Some(&d.perf),
        load: Load {
            cpu_total: &h[0],
            cpus: &d.cpus,
            memory: &h[1],
            disk: &h[2],
            network: &h[3],
            processes: &h[4],
            threads: &h[5],
            handles: &h[6],
        },
        menu: None,
    }
}

fn lay(l: &Look) -> Vec<Placed<Msg>> {
    place(&view(l), W as f32, Some(H as f32), &mut Fixed).unwrap()
}

fn at<'a>(p: &'a [Placed<Msg>], id: &str) -> &'a Placed<Msg> {
    p.iter().find(|x| x.id.as_deref() == Some(id)).unwrap_or_else(|| panic!("no {id}"))
}

/// Whether a filled element covers (x, y) in `color`, the topmost.
fn color_at(p: &[Placed<Msg>], x: i32, y: i32) -> Option<u32> {
    p.iter().rev().find_map(|e| {
        let [ex, ey, w, h] = e.rect;
        (x >= ex && y >= ey && x < ex + w && y < ey + h).then_some(e.background).flatten()
    })
}

#[test]
fn the_parts_are_where_they_were_drawn() {
    let d = data();
    let p = lay(&look(&d, View::Processes, None));
    assert_eq!(at(&p, "SearchBox").rect, SEARCH);
    for (i, (_, name)) in VIEWS.iter().enumerate() {
        let nav = at(&p, &named_id("NavigationPane", name));
        assert_eq!(nav.rect, [0, NAV_Y + NAV_H * i as i32, NAV_W, NAV_H]);
    }
    assert_eq!(at(&p, "EndTaskButton").rect, [W - BUTTON_W - 20, PAGE_Y, BUTTON_W, 32]);
    assert_eq!(at(&p, "ColumnHeader.Name").rect, [LIST_X, COLS_Y, 300, COLS_H]);
    assert_eq!(at(&p, "ColumnHeader.PID").rect, [LIST_X + 300, COLS_Y, 70, COLS_H]);
    assert_eq!(at(&p, "ProcessList").rect, [NAV_W + 1, ROWS_Y, W - NAV_W - 1, H - ROWS_Y]);
    // Apps (busy.exe), then Background processes, then Linux.
    assert_eq!(at(&p, "ProcessList.Group.Apps").rect, [NAV_W + 1, ROWS_Y, W - NAV_W - 1, ROW_H]);
    assert_eq!(at(&p, "ProcessList.Process.4").rect[1], ROWS_Y + ROW_H);
    assert_eq!(at(&p, "ProcessList.Group.Linux").rect[1], ROWS_Y + 4 * ROW_H);
    // The screen checks' pixels: Processes selected and its accent bar,
    // End task disabled.
    assert_eq!(color_at(&p, 100, NAV_Y + 5), Some(NAV_SELECTED));
    assert_eq!(color_at(&p, 7, 58 + 10), Some(ACCENT));
    assert_eq!(color_at(&p, W - 30, PAGE_Y + 4), Some(LINE));
}

#[test]
fn clicks_reach_what_was_hit() {
    let d = data();
    let p = lay(&look(&d, View::Processes, None));
    let click = |x, y| gala_ui::click(&p, x, y).map(|(id, m)| (String::from(id), m));
    assert_eq!(
        click(80, NAV_Y + NAV_H + 5),
        Some((String::from("NavigationPane.Performance"), Msg::Nav(View::Performance)))
    );
    assert_eq!(click(W - 100, PAGE_Y + 10).map(|c| c.1), Some(Msg::EndTask));
    assert_eq!(click(LIST_X + 310, COLS_Y + 10).map(|c| c.1), Some(Msg::Sort(Column::Pid)));
    // A cell's click is its row's.
    let busy = Row::Process(0);
    assert_eq!(click(LIST_X + 400, ROWS_Y + ROW_H + 10).map(|c| c.1), Some(Msg::Select(busy)));
    assert_eq!(
        click(LIST_X + 10, ROWS_Y + 5).map(|c| c.1),
        Some(Msg::Select(Row::Heading(Group::Apps, 1)))
    );
    // Below the rows: nothing selected.
    assert_eq!(click(600, H - 10), Some((String::from("ProcessList"), Msg::Deselect)));
}

#[test]
fn a_selected_row_is_marked_and_end_task_lit() {
    let d = data();
    let r = Row::Process(0);
    let p = lay(&look(&d, View::Processes, Some(&r)));
    let y = at(&p, "ProcessList.Process.4").rect[1];
    assert_eq!(color_at(&p, 700 - 120, y + 3), Some(SELECTED));
    assert_eq!(color_at(&p, W - 30, PAGE_Y + 4), Some(ACCENT));
}

#[test]
fn cells_carry_their_text_and_sparklines() {
    let d = data();
    let p = lay(&look(&d, View::Processes, None));
    let texts: Vec<&str> = p.iter().map(|e| e.text.as_str()).collect();
    assert!(texts.contains(&"Not responding"), "{texts:?}");
    let hung = p.iter().find(|e| e.text == "Not responding").unwrap();
    assert_eq!(hung.color, HUNG);
    // busy.exe's CPU cell is tinted and its trail drawn over the tint;
    // the Memory cell's over the row.
    let row_y = ROWS_Y + ROW_H;
    let images: Vec<(Picture, [i32; 4])> = p
        .iter()
        .filter(|e| e.kind == Kind::Image && e.rect[1] == row_y + 3)
        .map(|e| (Picture::parse(&e.src).unwrap(), e.rect))
        .collect();
    let cpu_x = LIST_X + 300 + 70 + 120 + 110;
    let tint = mix(0xfff4c4, 0xffd34f, 2000);
    assert_eq!(
        images,
        [
            (
                Picture::Trail { memory: false, proc: 0, scale: 1000, back: tint },
                [cpu_x + 2, row_y + 3, 76, 18]
            ),
            (
                Picture::Trail { memory: true, proc: 0, scale: 1000, back: CONTENT },
                [cpu_x + 82, row_y + 3, 106, 18]
            ),
        ]
    );
    // Numbers against the cell's right end, 6 px short of it.
    let pid = p.iter().find(|e| e.text == "4" && e.rect[1] > row_y - 1).unwrap();
    assert_eq!(pid.rect[0] + pid.rect[2], LIST_X + 300 + 70 - 6);
}

#[test]
fn the_last_column_is_cut_at_the_window_edge() {
    let d = data();
    let p = lay(&look(&d, View::Details, None));
    let cl = at(&p, "ColumnHeader.Commandline");
    assert_eq!(cl.rect[0] + cl.rect[2], W);
    assert_eq!(at(&p, "ProcessList.Process.4").rect[1], ROWS_Y);
}

#[test]
fn performance_has_a_graph_per_processor_and_the_list() {
    let d = data();
    let p = lay(&look(&d, View::Performance, None));
    let pics: Vec<Picture> = p
        .iter()
        .filter(|e| e.kind == Kind::Image)
        .map(|e| Picture::parse(&e.src).unwrap())
        .collect();
    let graphs = pics.iter().filter(|p| matches!(p, Picture::Graph { .. })).count();
    // Two processors, memory, disk, network.
    assert_eq!(graphs, 5);
    assert!(pics.contains(&Picture::Spark {
        series: Series::CpuTotal,
        scale: 1000,
        color: CPU_COLOR
    }));
    // No End task, no list.
    assert!(p.iter().all(|e| e.id.as_deref() != Some("EndTaskButton")));
}

#[test]
fn pictures_go_through_their_src() {
    for pic in [
        Picture::Trail { memory: true, proc: 17, scale: 950, back: 0xcce4f7 },
        Picture::Trail { memory: false, proc: 0, scale: 1000, back: 0 },
        Picture::Graph { series: Series::Cpu(3), scale: 1000, color: CPU_COLOR },
        Picture::Spark { series: Series::Handles, scale: 77, color: ACCENT },
        Picture::Graph { series: Series::CpuTotal, scale: 1, color: 0 },
    ] {
        assert_eq!(Picture::parse(&pic.src()), Some(pic), "{}", pic.src());
    }
    for bad in ["", "trail:disk:1:2:3", "graph:cpux:1:0", "spark:memory:1:00:extra", "x:cpu:1:0"] {
        assert_eq!(Picture::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_process_row_has_its_menu_and_a_heading_none() {
    let d = data();
    let p = lay(&look(&d, View::Processes, None));
    let right = |x, y| gala_ui::right_click(&p, x, y).map(|(id, m)| (String::from(id), m));
    // busy.exe's row, on a cell: its menu.
    assert_eq!(
        right(LIST_X + 400, ROWS_Y + ROW_H + 10),
        Some((String::from("ProcessList.Process.4"), Msg::Menu(Row::Process(0))))
    );
    // The Apps heading, the list below the rows, the navigation: none.
    assert_eq!(right(LIST_X + 10, ROWS_Y + 5), None);
    assert_eq!(right(600, H - 10), None);
    assert_eq!(right(80, NAV_Y + 5), None);
    // Details' rows have it too.
    let p = lay(&look(&d, View::Details, None));
    assert_eq!(
        gala_ui::right_click(&p, LIST_X + 10, ROWS_Y + 5).map(|c| c.1),
        Some(Msg::Menu(Row::Process(0)))
    );
}

#[test]
fn the_menu_opens_at_the_click_and_its_item_asks() {
    let d = data();
    let r = Row::Process(0);
    let mut l = look(&d, View::Processes, Some(&r));
    let point = (400, ROWS_Y + ROW_H + 12);
    l.menu = Some(point);
    let p = lay(&l);
    let m = at(&p, "Menu");
    assert_eq!(m.rect, [point.0, point.1, MENU_W, MENU_ROW + 2 * MENU_EDGE]);
    assert_eq!(m.background, Some(WHITE));
    let item = at(&p, "Menu.AskTheAgent");
    assert_eq!(item.rect, [point.0 + MENU_EDGE, point.1 + MENU_EDGE, MENU_W - 8, MENU_ROW]);
    assert!(p.iter().any(|e| e.text == ASK_TEXT));
    // The item's middle (the test program's ITEM) asks; it is over the
    // list, which the menu covers.
    let mid = (point.0 + MENU_W / 2, point.1 + MENU_EDGE + MENU_ROW / 2);
    assert_eq!(gala_ui::click(&p, mid.0, mid.1), Some(("Menu.AskTheAgent", Msg::Ask)));
    assert_eq!(color_at(&p, point.0 + 2, point.1 + 2), Some(WHITE));
    // The menu's edge is not the item: nothing asked there.
    assert_eq!(gala_ui::click(&p, point.0 + 1, point.1 + 1), None);
}

#[test]
fn the_menu_stays_in_the_window() {
    let mh = MENU_ROW + 2 * MENU_EDGE;
    assert_eq!(menu_at((W, H), (W - 10, H - 5)), (W - MENU_W, H - mh));
    assert_eq!(menu_at((W, H), (10, 20)), (10, 20));
    // A window smaller than the menu: at its top left.
    assert_eq!(menu_at((100, 20), (50, 10)), (0, 0));
}

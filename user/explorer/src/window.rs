//! File Explorer's window as a `gala_ui` view (ROADMAP LY step 3;
//! ARCHITECTURE "The file explorer"): the address bar, the command bar,
//! the navigation pane, the items (Details or Icons), the status bar, and
//! over them the context menu and Properties. Every part that can be
//! clicked or inspected has its automation id (BackButton, UpButton,
//! AddressBar, SearchBox, CommandBar.<command>, ViewButton.<view>,
//! NavigationPane.<row>, ColumnHeader.<column>, ItemsView.Item.<name>,
//! ItemsView, StatusBar, Menu.<item>, Properties); `explorer.exe` paints
//! the laid-out view, routes clicks through `gala_ui::click` and Inspect
//! through `gala_ui::hit`.
//!
//! The geometry is the window's as it was drawn by hand (the screen
//! checks of the `explorer` scenario hold): the bars at client y 6 and
//! 44, the panes from y 84, the navigation pane 220 wide, rows of 28
//! (navigation) and 24 (Details), tiles of [`TILE_W`] x [`TILE_H`].

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use gala_ui::{Align, Justify, Node, button, column, grid, px, row, text};

use crate::{COLUMNS, Column, Entry, Kind, STATUS_H, Sort, TILE_H, TILE_W};

pub const BACKGROUND: u32 = 0xf3f3f3;
pub const CONTENT: u32 = 0xffffff;
pub const TEXT: u32 = 0x1b1b1b;
pub const TEXT_DIM: u32 = 0x5d5d5d;
pub const LINE: u32 = 0xe5e5e5;
pub const ACCENT: u32 = 0x005fb8;
pub const SELECTED: u32 = 0xcce4f7;
pub const NAV_SELECTED: u32 = 0xe3e3e3;
const CONTROL_BORDER: u32 = 0xd0d0d0;
const FOLDER_COLOR: u32 = 0xf8c84a;
const FOLDER_TAB: u32 = 0xe09b1a;
const FILE_COLOR: u32 = 0xfafafa;
const FILE_EDGE: u32 = 0x8a8a8a;
const DRIVE_COLOR: u32 = 0x7a8694;
const DRIVE_LIGHT: u32 = 0x6cd06c;
const LINUX_COLOR: u32 = 0xe95420;
const COMPUTER_COLOR: u32 = 0x3b9ae1;
const MENU_BG: u32 = 0xf9f9f9;
const MENU_BORDER: u32 = 0xc8c8c8;
const CARD_BORDER: u32 = 0xa0a0a0;
const FONT: i32 = 13;
const SMALL: i32 = 12;

pub const BAR_Y: i32 = 6;
pub const BAR_H: i32 = 32;
pub const CMD_H: i32 = 32;
pub const PANE_Y: i32 = 84;
pub const NAV_W: i32 = 220;
pub const NAV_ROW: i32 = 28;
pub const HEADER_H: i32 = 28;
pub const ROW_H: i32 = 24;
pub const SEARCH_W: i32 = 240;
pub const MENU_W: i32 = 180;
pub const MENU_ROW: i32 = 30;
/// Properties' width.
pub const CARD_W: i32 = 520;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Open,
    NewFolder,
    Copy,
    Paste,
    Delete,
    Properties,
    Terminal,
}

/// The command bar's commands: what each does, its label and width (its icon, 16 px,
/// and a gap of 8 before the label: [`command_icon`]).
pub const COMMANDS: [(Command, &str, i32); 6] = [
    (Command::NewFolder, "New folder", 128),
    (Command::Copy, "Copy", 88),
    (Command::Paste, "Paste", 88),
    (Command::Delete, "Delete", 94),
    (Command::Properties, "Properties", 120),
    (Command::Terminal, "Open in Terminal", 170),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Details,
    Icons,
}

pub const VIEW_BUTTONS: [(View, &str, i32); 2] =
    [(View::Details, "Details", 104), (View::Icons, "Icons", 88)];

/// What a command bar button shows before its label.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandIcon {
    Run(Command),
    View(View),
}

/// The line art of the command bar's icons (Windows 11's are drawn in
/// one dark tone with an accent where it says what changes).
const GLYPH: u32 = 0x424242;

/// A command bar button's icon: 16 x 16, drawn of rectangles in
/// [`GLYPH`], the folder's colours and the accent.
pub fn command_icon(which: CommandIcon) -> Node<Msg> {
    let at = |x: i32, y: i32, w: i32, h: i32, c: u32| column().pin(x, y).width(w).height(h).bg(c);
    let frame = |x: i32, y: i32, w: i32, h: i32, fill: u32| {
        column().pin(x, y).width(w).height(h).bg(fill).border(GLYPH).radius(1)
    };
    let shape = column().width(16).height(16);
    match which {
        // Open (the context menu's, not on the bar): a page and an arrow.
        CommandIcon::Run(Command::Open) => shape
            .child(frame(0, 0, 11, 14, CONTENT))
            .child(at(7, 11, 9, 2, ACCENT))
            .child(at(13, 9, 2, 6, ACCENT)),
        CommandIcon::Run(Command::NewFolder) => shape
            .child(at(0, 3, 6, 2, FOLDER_TAB))
            .child(at(0, 4, 14, 10, FOLDER_COLOR))
            // The plus: what is made.
            .child(at(9, 10, 7, 2, ACCENT))
            .child(at(11, 8, 2, 7, ACCENT)),
        CommandIcon::Run(Command::Copy) => {
            shape.child(frame(0, 0, 10, 12, CONTENT)).child(frame(5, 4, 10, 12, CONTENT))
        }
        CommandIcon::Run(Command::Paste) => shape
            .child(frame(1, 2, 13, 14, CONTENT))
            .child(at(4, 0, 7, 4, ACCENT))
            .children((0..3).map(|k| at(4, 7 + 3 * k, 7, 1, GLYPH))),
        CommandIcon::Run(Command::Delete) => shape
            .child(at(1, 2, 14, 2, GLYPH))
            .child(at(6, 0, 4, 2, GLYPH))
            .child(frame(3, 4, 10, 12, CONTENT))
            .children((0..2).map(|k| at(6 + 3 * k, 7, 1, 6, GLYPH))),
        CommandIcon::Run(Command::Properties) => shape
            .child(frame(1, 0, 14, 16, CONTENT))
            .children((0..3).map(|k| at(4, 4 + 3 * k, 2, 2, ACCENT)))
            .children((0..3).map(|k| at(7, 4 + 3 * k, 5, 2, GLYPH))),
        CommandIcon::Run(Command::Terminal) => shape
            .child(at(0, 1, 16, 14, 0x1f1f1f).radius(2))
            // ">" and "_" in white.
            .child(at(3, 5, 2, 2, CONTENT))
            .child(at(5, 7, 2, 2, CONTENT))
            .child(at(3, 9, 2, 2, CONTENT))
            .child(at(8, 10, 5, 2, CONTENT)),
        CommandIcon::View(View::Details) => shape.children(
            (0..4).flat_map(|k| [at(0, 1 + 4 * k, 2, 2, ACCENT), at(4, 1 + 4 * k, 12, 2, GLYPH)]),
        ),
        CommandIcon::View(View::Icons) => shape
            .children([(0, 0), (9, 0), (0, 9), (9, 9)].map(|(x, y)| frame(x, y, 7, 7, CONTENT))),
    }
}

/// A navigation pane row: its text, what it opens, its depth, its icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavRow {
    pub name: String,
    pub parsing: String,
    pub depth: i32,
    pub kind: Kind,
}

/// The context menu: where (client), and its items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub x: i32,
    pub y: i32,
    pub items: Vec<(Command, &'static str)>,
}

/// What a click on the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Back,
    Up,
    /// The address bar: edit the folder's path.
    Address,
    Command(Command),
    View(View),
    /// A navigation pane row (index into the rows).
    Nav(usize),
    Sort(Column),
    /// An item (index into the entries).
    Item(usize),
    /// The items' background: nothing selected.
    Pane,
    /// A context menu item.
    Menu(Command),
}

/// What the window shows: the program's state, borrowed.
pub struct Look<'a> {
    /// The client area.
    pub size: (i32, i32),
    /// The folder's display names from the root (the address bar).
    pub crumbs: &'a [String],
    /// The address bar's text while it is being edited.
    pub editing: Option<&'a str>,
    pub search: &'a str,
    pub can_back: bool,
    pub can_up: bool,
    pub view: View,
    pub sort: Sort,
    pub nav: &'a [NavRow],
    /// The folder shown, by parsing name (its navigation row is marked).
    pub current: &'a str,
    pub entries: &'a [Entry],
    /// The rows shown (indices into `entries`), from `scroll`.
    pub rows: &'a [usize],
    pub scroll: usize,
    pub selected: Option<usize>,
    /// F2: the selected item's new name being typed.
    pub renaming: Option<&'a str>,
    pub menu: Option<&'a Menu>,
    pub properties: Option<&'a [String]>,
}

/// The id of a part named `name` (a row, an item, a command): the
/// kind's id, a dot, the name without spaces.
pub fn named_id(kind: &str, name: &str) -> String {
    let mut id = String::from(kind);
    id.push('.');
    id.extend(name.chars().filter(|c| *c != ' '));
    id
}

/// Ids are unique in a view: a name met again (two rows `tmp`) gets
/// `.2`, `.3`, ...
struct Ids(Vec<String>);

impl Ids {
    fn take(&mut self, id: String) -> String {
        let mut out = id.clone();
        let mut n = 1;
        while self.0.contains(&out) {
            n += 1;
            out = id.clone();
            let _ = write!(out, ".{n}");
        }
        self.0.push(out.clone());
        out
    }
}

/// An icon of `kind`, `size` pixels square (shapes on a 16-unit grid).
pub fn icon(kind: Kind, size: i32) -> Node<Msg> {
    let u = size / 16;
    let one = u.max(1);
    let fill = |x: i32, y: i32, w: i32, h: i32, c: u32| {
        column().pin(x * u, y * u).width(w * u).height(h).bg(c)
    };
    let shape = column().width(size).height(size);
    match kind {
        Kind::Folder => shape.child(fill(0, 2, 7, 2 * u, FOLDER_TAB)).child(fill(
            0,
            4,
            16,
            10 * u,
            FOLDER_COLOR,
        )),
        Kind::File => shape
            .child(fill(3, 1, 10, 14 * u, FILE_COLOR).border(FILE_EDGE))
            .children((0..3).map(|k| fill(5, 5 + 3 * k, 6, one, FILE_EDGE))),
        Kind::Drive => {
            shape.child(fill(0, 5, 16, 7 * u, DRIVE_COLOR)).child(fill(11, 9, 3, one, DRIVE_LIGHT))
        }
        Kind::Distro | Kind::Linux => shape
            .child(fill(1, 1, 14, 14 * u, LINUX_COLOR))
            .child(fill(5, 4, 6, 8 * u, TEXT))
            .child(fill(6, 6, 4, 5 * u, CONTENT)),
        Kind::Computer => shape
            .child(fill(0, 2, 16, 10 * u, COMPUTER_COLOR))
            .child(fill(6, 12, 4, 2 * u, DRIVE_COLOR))
            .child(fill(3, 14, 10, one, DRIVE_COLOR)),
    }
}

/// A text box's look: white, framed, with the accent bar and a caret
/// while it takes typing.
fn field(id: &str, width: i32, shown: Node<Msg>, typing: bool) -> Node<Msg> {
    column()
        .id(id)
        .width(width)
        .height(BAR_H)
        .bg(CONTENT)
        .border(CONTROL_BORDER)
        .justify(Justify::Center)
        .child(shown.padding((0, 10)))
        .child_if(typing, column().pin(1, BAR_H - 2).width(width - 2).height(2).bg(ACCENT))
}

fn with_caret(t: &str) -> String {
    let mut s = String::from(t);
    s.push('|');
    s
}

fn top_bar(l: &Look) -> Node<Msg> {
    let (w, _) = l.size;
    let arrow = |id: &str, label: &str, on: bool, m: Msg| {
        button(id, label)
            .width(32)
            .height(BAR_H)
            .bold()
            .color(if on { TEXT } else { LINE })
            .on_click(m)
    };
    let address = match l.editing {
        Some(t) => text(with_caret(t)).ellipsis(),
        None => text(crate::breadcrumbs(l.crumbs)).ellipsis(),
    };
    let search = if l.search.is_empty() {
        let mut hint = String::from("Search ");
        hint.push_str(l.crumbs.last().map_or("", |c| c.as_str()));
        text(hint).color(TEXT_DIM).ellipsis()
    } else {
        text(with_caret(l.search)).ellipsis()
    };
    let aw = w - 84 - SEARCH_W - 20;
    row()
        .margin((BAR_Y, 12, 0, 8))
        .height(BAR_H)
        .gap(4)
        .child(arrow("BackButton", "<", l.can_back, Msg::Back))
        .child(arrow("UpButton", "^", l.can_up, Msg::Up))
        .child(
            field("AddressBar", aw.max(2), address, l.editing.is_some())
                .margin((0, 0, 0, 4))
                .on_click(Msg::Address),
        )
        .child(column().grow(1))
        .child(field("SearchBox", SEARCH_W, search, !l.search.is_empty()))
}

fn command(id: String, icon: CommandIcon, label: &str, width: i32, on: bool, m: Msg) -> Node<Msg> {
    let b = row()
        .id(id)
        .width(width)
        .height(CMD_H)
        .align(Align::Center)
        .gap(8)
        .padding((0, 6, 0, 10))
        .radius(4)
        .on_click(m)
        .child(command_icon(icon))
        .child(text(label).ellipsis());
    if on { b.bg(NAV_SELECTED) } else { b }
}

fn command_bar(l: &Look) -> Node<Msg> {
    row()
        .id("CommandBar")
        .margin((6, 12, 0, 8))
        .height(CMD_H)
        .gap(4)
        .children(COMMANDS.iter().map(|&(c, label, w)| {
            let id = named_id("CommandBar", label);
            command(id, CommandIcon::Run(c), label, w, false, Msg::Command(c))
        }))
        .child(column().grow(1))
        .children(VIEW_BUTTONS.iter().map(|&(v, label, w)| {
            let id = named_id("ViewButton", label);
            command(id, CommandIcon::View(v), label, w, v == l.view, Msg::View(v))
        }))
}

fn nav_pane(l: &Look, ids: &mut Ids, pane_h: i32) -> Node<Msg> {
    let fits = ((pane_h - 4) / NAV_ROW).max(0) as usize;
    column().id("NavigationPane").width(NAV_W).padding((4, 4, 0, 4)).gap(2).children(
        l.nav.iter().enumerate().take(fits).map(|(i, r)| {
            let on = r.parsing == l.current;
            let row = row()
                .id(ids.take(named_id("NavigationPane", &r.name)))
                .height(NAV_ROW - 2)
                .align(Align::Center)
                .gap(6)
                .padding((0, 6, 0, 10 + 16 * r.depth))
                .on_click(Msg::Nav(i))
                .child(icon(r.kind, 16))
                .child(text(r.name.as_str()).ellipsis().min_width(0).grow(1));
            if on {
                row.bg(NAV_SELECTED)
                    .child(column().pin(0, 7).width(3).height(NAV_ROW - 16).bg(ACCENT))
            } else {
                row
            }
        }),
    )
}

/// A Details cell: `width` wide, its text from 12 px in (`right`:
/// against its right end, 12 px short of it).
fn cell(content: Node<Msg>, width: i32, right: bool) -> Node<Msg> {
    row()
        .width(width)
        .padding((0, 12, 0, 0))
        .align(Align::Center)
        .justify(if right { Justify::End } else { Justify::Start })
        .child(content)
}

fn details(l: &Look, ids: &mut Ids, pane_h: i32) -> Node<Msg> {
    let headers = row().id("ColumnHeaders").height(HEADER_H - 1).padding((0, 0, 0, 6)).children(
        COLUMNS.iter().map(|&(c, label, width)| {
            let mut t = String::from(label);
            if l.sort.column == c {
                t.push_str(if l.sort.descending { "  v" } else { "  ^" });
            }
            row()
                .id(named_id("ColumnHeader", label))
                .width(width)
                .padding((0, 6, 0, 6))
                .align(Align::Center)
                .justify(if c == Column::Size { Justify::End } else { Justify::Start })
                .on_click(Msg::Sort(c))
                .child(text(t).color(TEXT_DIM).ellipsis())
        }),
    );
    let fits = ((pane_h - HEADER_H) / ROW_H).max(0) as usize;
    let items = l.rows.iter().skip(l.scroll).take(fits).map(|&i| {
        let e = &l.entries[i];
        let on = Some(i) == l.selected;
        let name = match l.renaming {
            Some(t) if on => row()
                .grow(1)
                .min_width(0)
                .height(ROW_H - 3)
                .margin((0, 0, 0, -4))
                .padding((0, 4))
                .align(Align::Center)
                .bg(CONTENT)
                .border(ACCENT)
                .child(text(with_caret(t)).ellipsis()),
            _ => text(e.name.as_str()).ellipsis().min_width(0).grow(1),
        };
        let dim = |t: String| text(t).color(TEXT_DIM).ellipsis();
        let r = row()
            .id(ids.take(named_id("ItemsView.Item", &e.name)))
            .height(ROW_H - 1)
            .margin((0, 4))
            .padding((0, 0, 0, 8))
            .align(Align::Center)
            .on_click(Msg::Item(i))
            .children(COLUMNS.iter().map(|&(c, _, width)| {
                match c {
                    Column::Name => row()
                        .width(width)
                        .padding((0, 12, 0, 0))
                        .gap(8)
                        .align(Align::Center)
                        .child(icon(e.kind, 16))
                        .child(name.clone()),
                    Column::Modified => cell(dim(crate::time_text(e.modified)), width, false),
                    Column::Type => cell(dim(crate::type_text(e)), width, false),
                    Column::Size => cell(dim(crate::size_text(e)), width, true),
                }
            }));
        if on { r.bg(SELECTED) } else { r }
    });
    column()
        .id("ItemsView")
        .grow(1)
        .bg(CONTENT)
        .on_click(Msg::Pane)
        .child(headers)
        .child(column().height(1).bg(LINE))
        .gap(0)
        .child(column().gap(1).children(items))
}

fn icons(l: &Look, ids: &mut Ids, pane_h: i32) -> Node<Msg> {
    let (w, _) = l.size;
    let per_row = ((w - NAV_W - 1 - 16) / TILE_W).max(1) as usize;
    let fits = ((pane_h - 8) / TILE_H).max(0) as usize * per_row;
    let tiles = l.rows.iter().skip(l.scroll).take(fits).map(|&i| {
        let e = &l.entries[i];
        let t = column()
            .id(ids.take(named_id("ItemsView.Item", &e.name)))
            .width(TILE_W - 8)
            .height(TILE_H - 4)
            .margin((0, 4, 4, 4))
            .padding((6, 4, 0, 4))
            .on_click(Msg::Item(i))
            .child(row().justify(Justify::Center).child(icon(e.kind, 48)))
            .child(text(e.name.as_str()).size(SMALL).ellipsis().margin((8, 0, 0, 0)));
        if Some(i) == l.selected { t.bg(SELECTED) } else { t }
    });
    column()
        .id("ItemsView")
        .grow(1)
        .bg(CONTENT)
        .padding((8, 8, 0, 8))
        .on_click(Msg::Pane)
        .child(grid((0..per_row).map(|_| px(TILE_W))).children(tiles))
}

fn status(l: &Look) -> String {
    let mut s = String::new();
    let n = l.rows.len();
    let _ = write!(s, "{n} {}", if n == 1 { "item" } else { "items" });
    if l.selected.is_some() {
        s.push_str("    1 item selected");
    }
    s
}

fn menu(m: &Menu) -> Node<Msg> {
    let h = MENU_ROW * m.items.len() as i32 + 8;
    column()
        .id("Menu")
        .pin(m.x, m.y)
        .width(MENU_W)
        .height(h)
        .padding((4, 0))
        .bg(MENU_BG)
        .border(MENU_BORDER)
        .children(m.items.iter().map(|&(c, label)| {
            row()
                .id(named_id("Menu", label))
                .height(MENU_ROW)
                .padding((0, 8, 0, 16))
                .align(Align::Center)
                .on_click(Msg::Menu(c))
                .child(text(label).ellipsis())
        }))
}

/// Properties' rectangle in a client area of `size`, for `lines` lines.
pub fn properties_rect(size: (i32, i32), lines: usize) -> [i32; 4] {
    let (w, h) = size;
    let ph = 40 + 26 * lines as i32 + 30;
    [(w - CARD_W) / 2, (h - ph) / 2, CARD_W, ph]
}

fn properties(size: (i32, i32), lines: &[String]) -> Node<Msg> {
    let [x, y, w, h] = properties_rect(size, lines.len());
    column()
        .id("Properties")
        .pin(x, y)
        .width(w)
        .height(h)
        .bg(MENU_BG)
        .border(CARD_BORDER)
        .child(column().height(3).bg(ACCENT))
        .child(column().padding((15, 20, 0, 20)).gap(8).children(lines.iter().enumerate().map(
            |(k, l)| {
                let t = text(l.as_str()).ellipsis();
                if k == 0 { t.bold() } else { t }
            },
        )))
        .child(text("Esc closes").size(SMALL).color(TEXT_DIM).pin(20, h - 30).width(w - 40))
}

/// The window: `l` laid over its client area.
pub fn view(l: &Look) -> Node<Msg> {
    let (w, h) = l.size;
    let pane_h = h - PANE_Y - STATUS_H;
    let mut ids = Ids(Vec::new());
    let items = match l.view {
        View::Details => details(l, &mut ids, pane_h),
        View::Icons => icons(l, &mut ids, pane_h),
    };
    column()
        .width(w)
        .height(h)
        .bg(BACKGROUND)
        .size(FONT)
        .color(TEXT)
        .child(top_bar(l))
        .child(command_bar(l))
        .child(column().height(1).bg(LINE).margin((4, 0, 3, 0)))
        .child(
            row()
                .grow(1)
                .min_height(0)
                .child(nav_pane(l, &mut ids, pane_h))
                .child(column().width(1).bg(LINE))
                .child(items),
        )
        .child(
            row()
                .id("StatusBar")
                .height(STATUS_H)
                .padding((0, 12))
                .align(Align::Center)
                .child(text(status(l)).size(SMALL).color(TEXT_DIM).ellipsis()),
        )
        .child_if(l.menu.is_some(), l.menu.map_or(column(), menu))
        .child_if(l.properties.is_some(), l.properties.map_or(column(), |p| properties(l.size, p)))
}

/// What Inspect says of the part `id`: its kind and, for a named part,
/// the name as the window shows it.
pub fn describe(l: &Look, id: &str) -> String {
    let fixed = [
        ("BackButton", "The Back button"),
        ("UpButton", "The Up button"),
        ("AddressBar", "The address bar"),
        ("SearchBox", "The search box"),
        ("CommandBar", "The command bar"),
        ("NavigationPane", "The navigation pane"),
        ("ColumnHeaders", "The column headers"),
        ("ItemsView", "The list of items"),
        ("StatusBar", "The status bar"),
        ("Menu", "The context menu"),
        ("Properties", "Properties"),
    ];
    if let Some((_, d)) = fixed.iter().find(|(i, _)| *i == id) {
        return String::from(*d);
    }
    let names: Vec<(&str, &str)> = COMMANDS
        .iter()
        .map(|(_, n, _)| ("CommandBar", *n))
        .chain(VIEW_BUTTONS.iter().map(|(_, n, _)| ("ViewButton", *n)))
        .chain(COLUMNS.iter().map(|(_, n, _)| ("ColumnHeader", *n)))
        .chain(l.nav.iter().map(|r| ("NavigationPane", r.name.as_str())))
        .chain(l.entries.iter().map(|e| ("ItemsView.Item", e.name.as_str())))
        .chain(l.menu.iter().flat_map(|m| m.items.iter().map(|(_, n)| ("Menu", *n))))
        .collect();
    let kind = |k: &str| match k {
        "CommandBar" => "The command",
        "ViewButton" => "The view button",
        "ColumnHeader" => "The column header",
        "NavigationPane" => "The navigation pane's",
        "ItemsView.Item" => "The item",
        _ => "The menu item",
    };
    for (k, n) in names {
        let base = named_id(k, n);
        // `.2`, `.3`, ... for a name met again.
        let more = id.strip_prefix(base.as_str()).is_some_and(|r| {
            r.is_empty()
                || r.strip_prefix('.').is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
        });
        if more {
            let mut s = String::from(kind(k));
            s.push_str(" \"");
            s.push_str(n);
            s.push('"');
            return s;
        }
    }
    String::from(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use gala_ui::{Measure, Placed, place};

    /// 7 px a character, lines 18 high (13 px text) or 16 (12 px).
    struct Fixed;
    impl Measure for Fixed {
        fn text(&mut self, t: &str, size: f32, _: bool, _: Option<f32>) -> (f32, f32) {
            (7.0 * t.chars().count() as f32, if size < 12.5 { 16.0 } else { 18.0 })
        }
    }

    const W: i32 = 1078;
    const H: i32 = 666;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: String::from(name),
            parsing: String::from(name),
            kind,
            size: 1234,
            modified: 0,
        }
    }

    struct Data {
        crumbs: Vec<String>,
        nav: Vec<NavRow>,
        entries: Vec<Entry>,
        rows: Vec<usize>,
    }

    fn data() -> Data {
        let nav = |name: &str, depth, kind| NavRow {
            name: String::from(name),
            parsing: String::from(name),
            depth,
            kind,
        };
        Data {
            crumbs: vec![String::from("This PC"), String::from("files")],
            nav: vec![
                nav("This PC", 0, Kind::Computer),
                nav("Local Disk (C:)", 1, Kind::Drive),
                nav("tmp", 2, Kind::Folder),
                nav("tmp", 3, Kind::Folder),
            ],
            entries: vec![entry("docs", Kind::Folder), entry("a.txt", Kind::File)],
            rows: vec![0, 1],
        }
    }

    fn look(d: &Data) -> Look<'_> {
        Look {
            size: (W, H),
            crumbs: &d.crumbs,
            editing: None,
            search: "",
            can_back: false,
            can_up: true,
            view: View::Details,
            sort: Sort::NAME,
            nav: &d.nav,
            current: "tmp",
            entries: &d.entries,
            rows: &d.rows,
            scroll: 0,
            selected: Some(1),
            renaming: None,
            menu: None,
            properties: None,
        }
    }

    fn lay(l: &Look) -> Vec<Placed<Msg>> {
        place(&view(l), W as f32, Some(H as f32), &mut Fixed).expect("the view lays out")
    }

    fn at<'a>(p: &'a [Placed<Msg>], id: &str) -> &'a Placed<Msg> {
        p.iter().find(|e| e.id.as_deref() == Some(id)).unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn the_bars_are_where_the_window_drew_them() {
        let d = data();
        let p = lay(&look(&d));
        assert_eq!(at(&p, "BackButton").rect, [8, BAR_Y, 32, BAR_H]);
        assert_eq!(at(&p, "UpButton").rect, [44, BAR_Y, 32, BAR_H]);
        assert_eq!(at(&p, "AddressBar").rect, [84, BAR_Y, W - 84 - SEARCH_W - 20, BAR_H]);
        assert_eq!(at(&p, "SearchBox").rect, [W - SEARCH_W - 12, BAR_Y, SEARCH_W, BAR_H]);
        assert_eq!(at(&p, "CommandBar.Newfolder").rect, [8, 44, 128, CMD_H]);
        assert_eq!(at(&p, "CommandBar.Copy").rect, [140, 44, 88, CMD_H]);
        assert_eq!(at(&p, "ViewButton.Icons").rect, [W - 12 - 88, 44, 88, CMD_H]);
        assert_eq!(at(&p, "ViewButton.Details").rect, [W - 12 - 88 - 4 - 104, 44, 104, CMD_H]);
        assert_eq!(at(&p, "StatusBar").rect, [0, H - STATUS_H, W, STATUS_H]);
        assert_eq!(gala_ui::click(&p, 20, 20), Some(("BackButton", Msg::Back)));
        assert_eq!(gala_ui::click(&p, 300, 20), Some(("AddressBar", Msg::Address)));
        assert_eq!(
            gala_ui::click(&p, 150, 60),
            Some(("CommandBar.Copy", Msg::Command(Command::Copy)))
        );
    }

    #[test]
    fn each_command_shows_its_icon_before_its_label() {
        let d = data();
        let p = lay(&look(&d));
        let ids = COMMANDS
            .iter()
            .map(|c| named_id("CommandBar", c.1))
            .chain(VIEW_BUTTONS.iter().map(|v| named_id("ViewButton", v.1)));
        for id in ids {
            let b = p.iter().position(|e| e.id.as_deref() == Some(id.as_str())).unwrap();
            let kids: Vec<_> = p.iter().filter(|e| e.parent == Some(b)).collect();
            // The icon, 16 square and drawn (shapes inside it), then the label.
            let [ix, iy, iw, ih] = kids[0].rect;
            assert_eq!((iw, ih), (16, 16), "{id}");
            assert_eq!(iy, 44 + (CMD_H - 16) / 2, "{id}");
            let i = p.iter().position(|e| core::ptr::eq(e, kids[0])).unwrap();
            assert!(p.iter().filter(|e| e.parent == Some(i)).count() >= 2, "{id}");
            let label = kids[1];
            assert_eq!(label.rect[0], ix + 16 + 8, "{id}");
            // The label fits: not cut (7 px a character in the test measure).
            assert!(label.rect[2] >= label.text.chars().count() as i32 * 7, "{id}");
        }
    }

    #[test]
    fn the_panes_rows_and_items_are_where_the_window_drew_them() {
        let d = data();
        let p = lay(&look(&d));
        assert_eq!(at(&p, "NavigationPane").rect, [0, PANE_Y, NAV_W, H - PANE_Y - STATUS_H]);
        // Row k at y 88 + 28 k, 212 x 26; a name met again gets `.2`.
        assert_eq!(at(&p, "NavigationPane.ThisPC").rect, [4, PANE_Y + 4, NAV_W - 8, NAV_ROW - 2]);
        assert_eq!(at(&p, "NavigationPane.tmp").rect[1], PANE_Y + 4 + 2 * NAV_ROW);
        assert_eq!(at(&p, "NavigationPane.tmp.2").rect[1], PANE_Y + 4 + 3 * NAV_ROW);
        assert_eq!(at(&p, "NavigationPane.tmp").background, Some(NAV_SELECTED));
        assert_eq!(
            gala_ui::click(&p, 100, PANE_Y + 10),
            Some(("NavigationPane.ThisPC", Msg::Nav(0)))
        );
        let cx = NAV_W + 1;
        assert_eq!(at(&p, "ItemsView").rect, [cx, PANE_Y, W - cx, H - PANE_Y - STATUS_H]);
        assert_eq!(at(&p, "ColumnHeader.Name").rect, [cx + 6, PANE_Y, 300, HEADER_H - 1]);
        assert_eq!(at(&p, "ColumnHeader.Datemodified").rect[0], cx + 6 + 300);
        // Row k at y 112 + 24 k, from x cx + 4, 23 high; the selected one
        // filled.
        let first = PANE_Y + HEADER_H;
        assert_eq!(at(&p, "ItemsView.Item.docs").rect, [cx + 4, first, W - cx - 8, ROW_H - 1]);
        let a = at(&p, "ItemsView.Item.a.txt");
        assert_eq!((a.rect[1], a.background), (first + ROW_H, Some(SELECTED)));
        assert_eq!(
            gala_ui::click(&p, 600, first + 30),
            Some(("ItemsView.Item.a.txt", Msg::Item(1)))
        );
        assert_eq!(gala_ui::click(&p, 600, first + 200), Some(("ItemsView", Msg::Pane)));
        assert_eq!(
            gala_ui::click(&p, cx + 20, PANE_Y + 10),
            Some(("ColumnHeader.Name", Msg::Sort(Column::Name)))
        );
    }

    #[test]
    fn icons_are_tiles_in_rows_as_wide_as_the_pane() {
        let d = data();
        let mut l = look(&d);
        l.view = View::Icons;
        let p = lay(&l);
        let (cx, first) = (NAV_W + 1, PANE_Y + 8);
        let (x0, y0) = crate::icon_grid(0, W - cx - 16);
        let (x1, y1) = crate::icon_grid(1, W - cx - 16);
        assert_eq!(
            at(&p, "ItemsView.Item.docs").rect,
            [cx + 8 + x0 + 4, first + y0, TILE_W - 8, TILE_H - 4]
        );
        assert_eq!(at(&p, "ItemsView.Item.a.txt").rect[..2], [cx + 8 + x1 + 4, first + y1]);
        assert_eq!(at(&p, "ViewButton.Icons").background, Some(NAV_SELECTED));
    }

    #[test]
    fn the_menu_and_properties_lie_over_the_window() {
        let d = data();
        let m =
            Menu { x: 400, y: 200, items: vec![(Command::Open, "Open"), (Command::Copy, "Copy")] };
        let lines = vec![String::from("a.txt"), String::from("Type: TXT File")];
        let mut l = look(&d);
        l.menu = Some(&m);
        let p = lay(&l);
        assert_eq!(at(&p, "Menu").rect, [400, 200, MENU_W, 2 * MENU_ROW + 8]);
        assert_eq!(gala_ui::click(&p, 450, 240), Some(("Menu.Copy", Msg::Menu(Command::Copy))));
        let mut l = look(&d);
        l.properties = Some(&lines);
        let p = lay(&l);
        let r = properties_rect((W, H), 2);
        assert_eq!(at(&p, "Properties").rect, r);
        let esc = p.iter().find(|e| e.text == "Esc closes").expect("Esc closes");
        assert_eq!(esc.rect[..2], [r[0] + 20, r[1] + r[3] - 30]);
        assert_eq!(describe(&l, "Properties"), "Properties");
    }

    #[test]
    fn inspect_names_each_part() {
        let d = data();
        let l = look(&d);
        assert_eq!(describe(&l, "BackButton"), "The Back button");
        assert_eq!(describe(&l, "CommandBar.Newfolder"), "The command \"New folder\"");
        assert_eq!(describe(&l, "NavigationPane.tmp.2"), "The navigation pane's \"tmp\"");
        assert_eq!(describe(&l, "ItemsView.Item.a.txt"), "The item \"a.txt\"");
    }

    #[test]
    fn a_box_taking_typing_is_underlined_across_its_width() {
        let d = data();
        let mut l = look(&d);
        l.search = "notes";
        let p = lay(&l);
        let sx = W - SEARCH_W - 12;
        let bar = p.iter().find(|e| e.background == Some(ACCENT)).expect("the accent bar");
        assert_eq!(bar.rect, [sx + 1, BAR_Y + BAR_H - 2, SEARCH_W - 2, 2]);
    }

    #[test]
    fn renaming_shows_the_name_typed_in_a_box() {
        let d = data();
        let mut l = look(&d);
        l.renaming = Some("b.txt");
        let p = lay(&l);
        assert!(p.iter().any(|e| e.text == "b.txt|" && e.ellipsis));
        assert!(p.iter().any(|e| e.border == Some(ACCENT)));
    }
}

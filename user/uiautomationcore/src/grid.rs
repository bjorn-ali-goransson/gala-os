//! Report-view list views as grids, as UI Automation's Win32 list-view
//! proxy exposes them: the list (a SysListView32 with LVS_REPORT) has the
//! Grid pattern (RowCount: its items, ColumnCount: its header's columns,
//! GetItem(row, column): a cell) and the Table pattern (ColumnHeaders: the
//! header's items, no row headers, RowOrColumnMajor RowMajor); each item
//! (a ListItem) has a cell per column as its children, which have the
//! GridItem pattern (Row, Column, spans of 1, ContainingGrid: the list)
//! and TableItem (ColumnHeaderItems: the column's header item). The
//! header control (SysHeader32) is a Header element whose children are
//! HeaderItems, one per column, named by the column's title.
//!
//! The cells' and header items' text and rectangles are read with the
//! controls' messages (LVM_GETITEMTEXTW with iSubItem, LVM_GETSUBITEMRECT,
//! HDM_GETITEMW, HDM_GETITEMRECT): above WM_USER, so for another process's
//! list through memory allocated in it (items::Buffer), as for list items.

use core::ffi::c_void;

use crate::element::{self, Node};
use crate::items::{self, Buffer, Kind};
use crate::pattern::{Pattern, alive, node, pattern_add_ref, pattern_query, pattern_release};
use crate::{
    E_INVALIDARG, E_POINTER, Guid, HResult, Hwnd, List, S_OK, SendMessageW,
    UIA_E_ELEMENTNOTAVAILABLE, VTable, Variant, guid, put,
};

/// Node.menu: column `col` of item `item` of report-view list `hwnd`.
pub const CELL: u8 = 4;
/// Node.menu: item `item` of header control `hwnd`.
pub const HEAD: u8 = 5;

pub const GRID: i32 = 10006;
pub const GRID_ITEM: i32 = 10007;
pub const TABLE: i32 = 10012;
pub const TABLE_ITEM: i32 = 10013;

const IID_IUIAUTOMATIONGRIDPATTERN: Guid =
    guid(0x414c3cdc, 0x856b, 0x4f5b, [0x85, 0x38, 0x31, 0x31, 0xc6, 0x30, 0x25, 0x50]);
const IID_IUIAUTOMATIONGRIDITEMPATTERN: Guid =
    guid(0x78f8ef57, 0x66c3, 0x4e09, [0xbd, 0x7c, 0xe7, 0x9b, 0x20, 0x04, 0x89, 0x4d]);
const IID_IUIAUTOMATIONTABLEPATTERN: Guid =
    guid(0x620e691c, 0xea96, 0x4710, [0xa8, 0x50, 0x75, 0x4b, 0x24, 0xce, 0x24, 0x17]);
const IID_IUIAUTOMATIONTABLEITEMPATTERN: Guid =
    guid(0x0b964eb3, 0xef2e, 0x4464, [0x9c, 0x79, 0x61, 0xd6, 0x17, 0x37, 0xa2, 0x7e]);

// Pattern properties (UIAutomationClient.h).
const GRID_ROW_COUNT: i32 = 30062;
const GRID_COLUMN_COUNT: i32 = 30063;
const GRID_ITEM_ROW: i32 = 30064;
const GRID_ITEM_COLUMN: i32 = 30065;
const GRID_ITEM_ROW_SPAN: i32 = 30066;
const GRID_ITEM_COLUMN_SPAN: i32 = 30067;
const GRID_ITEM_CONTAINING_GRID: i32 = 30068;
const TABLE_ROW_HEADERS: i32 = 30081;
const TABLE_COLUMN_HEADERS: i32 = 30082;
const TABLE_ROW_OR_COLUMN_MAJOR: i32 = 30083;
const TABLE_ITEM_ROW_HEADER_ITEMS: i32 = 30084;
const TABLE_ITEM_COLUMN_HEADER_ITEMS: i32 = 30085;
/// RowOrColumnMajor_RowMajor.
const ROW_MAJOR: i32 = 0;

const LVM_GETHEADER: u32 = 0x101f;
const LVS_TYPEMASK: u32 = 0x3;
const LVS_REPORT: u32 = 0x1;
const HDM_GETITEMCOUNT: u32 = 0x1200;
const HDM_GETITEMRECT: u32 = 0x1207;
const HDM_GETITEMW: u32 = 0x120b;
/// HDITEMW (x64): mask (0), cxy (4), pszText (8), hbm (16), cchTextMax
/// (24), fmt (28), lParam (32), iImage (40), iOrder (44), type (48),
/// pvFilter (56), state (64): 72 bytes.
const HDITEM_SIZE: usize = 72;
const HDI_TEXT: u32 = 0x2;
const TEXT_MAX: usize = 260;

fn send(h: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: a plain call (pointers in lParam are in the window's process).
    unsafe { SendMessageW(h, msg, wparam, lparam) }
}

/// Whether `h` is a list view in report view (a grid).
pub fn is_report(h: Hwnd) -> bool {
    items::kind(h) == Some(Kind::ListView) && element::style(h) & LVS_TYPEMASK == LVS_REPORT
}

/// Whether `h` is a header control.
pub fn is_header(h: Hwnd) -> bool {
    element::class_is(h, "SysHeader32")
}

/// The list view's header control (0: none).
pub fn header_of(h: Hwnd) -> Hwnd {
    send(h, LVM_GETHEADER, 0, 0).max(0) as Hwnd
}

/// How many items the header control has.
pub fn header_count(hdr: Hwnd) -> u32 {
    if hdr == 0 {
        return 0;
    }
    send(hdr, HDM_GETITEMCOUNT, 0, 0).clamp(0, 4096) as u32
}

/// A report-view list's columns (its header's items; at least one: a
/// report view without columns still shows its items' text).
pub fn columns(h: Hwnd) -> u32 {
    header_count(header_of(h)).max(1)
}

pub fn rows(h: Hwnd) -> u32 {
    items::count(h, Kind::ListView)
}

/// Header item `i`'s title as a BSTR.
pub fn header_text(hdr: Hwnd, i: u32) -> *mut u16 {
    let Some(buf) = Buffer::new(hdr, HDITEM_SIZE + 2 * TEXT_MAX) else {
        return crate::bstr(&[]);
    };
    let mut it = [0u8; HDITEM_SIZE];
    it[0..4].copy_from_slice(&HDI_TEXT.to_le_bytes());
    it[8..16].copy_from_slice(&(buf.at(HDITEM_SIZE) as u64).to_le_bytes());
    it[24..28].copy_from_slice(&(TEXT_MAX as i32).to_le_bytes());
    if !buf.write(0, &it) || send(hdr, HDM_GETITEMW, i as usize, buf.at(0)) == 0 {
        return crate::bstr(&[]);
    }
    let mut bytes = [0u8; 2 * TEXT_MAX];
    if !buf.read(HDITEM_SIZE, &mut bytes) {
        return crate::bstr(&[]);
    }
    let mut chars = [0u16; TEXT_MAX];
    let mut n = 0;
    for pair in bytes.chunks_exact(2) {
        let c = u16::from_le_bytes([pair[0], pair[1]]);
        if c == 0 {
            break;
        }
        chars[n] = c;
        n += 1;
    }
    crate::bstr(&chars[..n])
}

/// Header item `i`'s rectangle on the screen (clipped to the header).
pub fn header_rect(hdr: Hwnd, i: u32) -> Option<[i32; 4]> {
    let buf = Buffer::new(hdr, 16)?;
    if !buf.write(0, &[0; 16]) || send(hdr, HDM_GETITEMRECT, i as usize, buf.at(0)) == 0 {
        return None;
    }
    let mut bytes = [0u8; 16];
    if !buf.read(0, &mut bytes) {
        return None;
    }
    let mut r = [0i32; 4];
    for (v, b) in r.iter_mut().zip(bytes.chunks_exact(4)) {
        *v = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    items::clip(hdr, r)
}

/// Whether the cell or header item is still there.
pub fn exists(n: Node) -> bool {
    let Some(i) = n.item else { return false };
    match n.menu {
        CELL => is_report(n.hwnd) && i < rows(n.hwnd) && n.col < columns(n.hwnd),
        _ => is_header(n.hwnd) && i < header_count(n.hwnd),
    }
}

/// A cell's parent is its row (the list item), a header item's its
/// header.
pub fn parent(n: Node) -> Node {
    match (n.menu, n.item) {
        (CELL, Some(i)) => Node::item(n.hwnd, i),
        _ => Node::window(n.hwnd),
    }
}

/// The children grids add: a report-view item's cells, a header's items.
pub fn children(n: Node, out: &mut List<Node>) {
    match (n.menu, n.item) {
        (0, Some(i)) if is_report(n.hwnd) => {
            for c in 0..columns(n.hwnd) {
                if !out.push(Node::cell(n.hwnd, i, c)) {
                    return;
                }
            }
        }
        (0, None) if is_header(n.hwnd) => {
            for k in 0..header_count(n.hwnd) {
                if !out.push(Node::menu(n.hwnd, Some(k), HEAD)) {
                    return;
                }
            }
        }
        _ => {}
    }
}

/// The node's rectangle on the screen (empty when it does not show).
pub fn rect(n: Node) -> [i32; 4] {
    let Some(i) = n.item else { return [0; 4] };
    match n.menu {
        CELL => items::cell_rect(n.hwnd, i, n.col),
        _ => header_rect(n.hwnd, i),
    }
    .unwrap_or([0; 4])
}

/// The header item over column `col` of list `h`.
fn column_header(h: Hwnd, col: u32) -> Option<Node> {
    let hdr = header_of(h);
    (col < header_count(hdr)).then(|| Node::menu(hdr, Some(col), HEAD))
}

/// The list's column headers, in column order.
fn column_headers(h: Hwnd) -> List<Node> {
    let mut out = List::new();
    let hdr = header_of(h);
    for k in 0..header_count(hdr) {
        if !out.push(Node::menu(hdr, Some(k), HEAD)) {
            break;
        }
    }
    out
}

// ---- Patterns ----

/// Whether the element has the pattern: Grid and Table on a report-view
/// list, GridItem and TableItem on its cells.
pub fn supports(n: Node, id: i32) -> bool {
    match id {
        GRID | TABLE => n.menu == 0 && n.item.is_none() && is_report(n.hwnd),
        GRID_ITEM | TABLE_ITEM => n.menu == CELL,
        _ => false,
    }
}

/// Whether `id` is a property of these patterns.
pub fn known_property(id: i32) -> bool {
    matches!(id, GRID_ROW_COUNT..=GRID_ITEM_CONTAINING_GRID | TABLE_ROW_HEADERS..=TABLE_ITEM_COLUMN_HEADER_ITEMS)
}

/// A pattern property's value (None without the pattern). Element-valued
/// ones are VT_UNKNOWN (an element or an element array), as UIA's client
/// API gives them.
pub fn property(n: Node, id: i32) -> Option<Variant> {
    let h = n.hwnd;
    let grid = supports(n, GRID);
    let cell = n.menu == CELL;
    let row = n.item.unwrap_or(0);
    Some(match id {
        GRID_ROW_COUNT if grid => Variant::int(rows(h) as i32),
        GRID_COLUMN_COUNT if grid => Variant::int(columns(h) as i32),
        TABLE_ROW_HEADERS if grid => Variant::unknown(element::new_array(List::new())),
        TABLE_COLUMN_HEADERS if grid => Variant::unknown(element::new_array(column_headers(h))),
        TABLE_ROW_OR_COLUMN_MAJOR if grid => Variant::int(ROW_MAJOR),
        GRID_ITEM_ROW if cell => Variant::int(row as i32),
        GRID_ITEM_COLUMN if cell => Variant::int(n.col as i32),
        GRID_ITEM_ROW_SPAN | GRID_ITEM_COLUMN_SPAN if cell => Variant::int(1),
        GRID_ITEM_CONTAINING_GRID if cell => {
            Variant::unknown(element::new_element(Node::window(h)))
        }
        TABLE_ITEM_ROW_HEADER_ITEMS if cell => Variant::unknown(element::new_array(List::new())),
        TABLE_ITEM_COLUMN_HEADER_ITEMS if cell => {
            let mut l = List::new();
            if let Some(c) = column_header(h, n.col) {
                l.push(c);
            }
            Variant::unknown(element::new_array(l))
        }
        _ => return None,
    })
}

/// The pattern's vtable and IID.
pub fn vtable(id: i32) -> (*const (), Guid) {
    match id {
        GRID => ((&GRID_VTABLE as *const VTable<8>).cast(), IID_IUIAUTOMATIONGRIDPATTERN),
        GRID_ITEM => {
            ((&GRID_ITEM_VTABLE as *const VTable<13>).cast(), IID_IUIAUTOMATIONGRIDITEMPATTERN)
        }
        TABLE => ((&TABLE_VTABLE as *const VTable<9>).cast(), IID_IUIAUTOMATIONTABLEPATTERN),
        _ => ((&TABLE_ITEM_VTABLE as *const VTable<7>).cast(), IID_IUIAUTOMATIONTABLEITEMPATTERN),
    }
}

/// An integer property of the pattern's element.
fn int_of(this: *mut Pattern, id: i32, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if let Err(e) = alive(this) {
        return e;
    }
    match property(node(this), id) {
        Some(v) => {
            put(out, v.as_int());
            S_OK
        }
        None => UIA_E_ELEMENTNOTAVAILABLE,
    }
}

/// An element or element-array property of the pattern's element (the
/// VT_UNKNOWN's reference is the caller's).
fn object_of(this: *mut Pattern, id: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if let Err(e) = alive(this) {
        return e;
    }
    match property(node(this), id) {
        Some(v) if v.vt == crate::VT_UNKNOWN => {
            put(out, v.data[0] as usize as *mut c_void);
            S_OK
        }
        Some(_) => crate::E_OUTOFMEMORY,
        None => UIA_E_ELEMENTNOTAVAILABLE,
    }
}

macro_rules! getters {
    ($($name:ident: $how:ident($ty:ty) = $id:expr;)*) => {
        $(
            extern "system" fn $name(this: *mut Pattern, out: *mut $ty) -> HResult {
                $how(this, $id, out)
            }
        )*
    };
}

getters! {
    get_row_count: int_of(i32) = GRID_ROW_COUNT;
    get_column_count: int_of(i32) = GRID_COLUMN_COUNT;
    get_containing_grid: object_of(*mut c_void) = GRID_ITEM_CONTAINING_GRID;
    get_row: int_of(i32) = GRID_ITEM_ROW;
    get_column: int_of(i32) = GRID_ITEM_COLUMN;
    get_row_span: int_of(i32) = GRID_ITEM_ROW_SPAN;
    get_column_span: int_of(i32) = GRID_ITEM_COLUMN_SPAN;
    get_row_headers: object_of(*mut c_void) = TABLE_ROW_HEADERS;
    get_column_headers: object_of(*mut c_void) = TABLE_COLUMN_HEADERS;
    get_row_or_column_major: int_of(i32) = TABLE_ROW_OR_COLUMN_MAJOR;
    get_row_header_items: object_of(*mut c_void) = TABLE_ITEM_ROW_HEADER_ITEMS;
    get_column_header_items: object_of(*mut c_void) = TABLE_ITEM_COLUMN_HEADER_ITEMS;
}

/// GridPattern's GetItem(row, column): the cell; E_INVALIDARG outside the
/// grid (IGridProvider::GetItem documents ArgumentOutOfRange).
extern "system" fn get_item(
    this: *mut Pattern,
    row: i32,
    col: i32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let h = match alive(this) {
        Ok(h) => h,
        Err(e) => return e,
    };
    if row < 0 || col < 0 || row as u32 >= rows(h) || col as u32 >= columns(h) {
        return E_INVALIDARG;
    }
    element::put_element(out, Node::cell(h, row as u32, col as u32))
}

/// The cached getters read current values (no cache).
static GRID_VTABLE: VTable<8> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    get_item as *const (),
    get_row_count as *const (),
    get_column_count as *const (),
    get_row_count as *const (),
    get_column_count as *const (),
]);

static GRID_ITEM_VTABLE: VTable<13> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    get_containing_grid as *const (),
    get_row as *const (),
    get_column as *const (),
    get_row_span as *const (),
    get_column_span as *const (),
    get_containing_grid as *const (),
    get_row as *const (),
    get_column as *const (),
    get_row_span as *const (),
    get_column_span as *const (),
]);

static TABLE_VTABLE: VTable<9> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    get_row_headers as *const (),
    get_column_headers as *const (),
    get_row_or_column_major as *const (),
    get_row_headers as *const (),
    get_column_headers as *const (),
    get_row_or_column_major as *const (),
]);

static TABLE_ITEM_VTABLE: VTable<7> = VTable([
    pattern_query as *const (),
    pattern_add_ref as *const (),
    pattern_release as *const (),
    get_row_header_items as *const (),
    get_column_header_items as *const (),
    get_row_header_items as *const (),
    get_column_header_items as *const (),
]);

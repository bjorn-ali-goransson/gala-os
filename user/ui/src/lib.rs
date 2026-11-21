//! UI components for gala's own programs (ROADMAP LY; ARCHITECTURE "UI
//! components"): a window is one Rust file holding its state, a `view`
//! that builds the element tree from the state with chained calls and
//! inline styles, and an `update` that changes the state for a message.
//! Logic, behaviour, markup and appearance of a window are in one
//! place, so a change to any of them is read and made in that file.
//!
//! ```
//! use gala_ui::{Component, Justify, Node, button, column, input, row, text};
//!
//! const SURFACE: u32 = 0xffffff;
//! const ACCENT: u32 = 0x005fb8;
//!
//! /// The look of a primary button, shared by role: a plain function.
//! fn primary<M>(b: Node<M>) -> Node<M> {
//!     b.bg(ACCENT).color(0xffffff)
//! }
//!
//! #[derive(Clone, Debug)]
//! enum Msg { Request(String), Make }
//!
//! struct ChangeCard { request: String }
//!
//! impl Component for ChangeCard {
//!     type Msg = Msg;
//!     fn view(&self) -> Node<Msg> {
//!         column().id("ChangeCard").width(520).gap(12).padding(16).radius(8).bg(SURFACE)
//!             .child(text("Change File Explorer").size(20).bold())
//!             .child(input("Request", &self.request)
//!                 .placeholder("Describe the change").on_input(Msg::Request))
//!             .child(row().gap(8).justify(Justify::End)
//!                 .child_if(!self.request.is_empty(),
//!                     primary(button("MakeButton", "Make the change").on_click(Msg::Make))))
//!     }
//!     fn update(&mut self, msg: Msg) {
//!         if let Msg::Request(r) = msg { self.request = r }
//!     }
//! }
//! ```
//!
//! - Elements: [`column`], [`row`], [`grid`] place what they hold;
//!   [`text`], [`button`], [`input`], [`image`] and [`rule`] are leaves.
//! - Style is set on the element ([`Node`]'s methods); `color`, `size`
//!   and `bold` are inherited by the elements inside it, nothing else
//!   cascades. Shared looks are plain functions and constants.
//! - Flow first: a container places its children with `gap`, `padding`,
//!   `align`, `justify`; [`Node::pin`] takes an element out of the flow
//!   to a place in its parent (a pinned element sets its width).
//! - The order of the children is paint order (later on top) and
//!   reading order.
//! - `id` is the element's UI Automation AutomationId: what Inspect
//!   picks, what the tests and UIA clients see. Unique; [`button`] and
//!   [`input`] take one, and an element with a handler must have one.
//! - Handlers are messages: [`Node::on_click`] is the message a click
//!   sends, [`Node::on_input`] makes the message for the text typed.
//!   The program logs each message it handles (by the element's id),
//!   updates its state and builds the view again.
//! - [`place`] lays a view out for a size with the program's text
//!   measure (Taffy's flexbox and grid), giving [`Placed`] elements in
//!   paint order; [`hit`] and [`click`] find what is under a point.
//!   Errors (a duplicate id, a handler without an id, the limits
//!   [`MAX_ELEMENTS`] and [`MAX_DEPTH`]) name the element by its path.
//!
//! - [`paint`] draws the [`Placed`] list on a [`Canvas`]; [`uia`] makes
//!   UI Automation elements of it (those with an id). [`win`] (Windows
//!   only) is a window holding a component: drawn with gdi32, clicks and
//!   typing to `update` (each message logged), its UIA provider made
//!   from the same list; [`Component::message`] gives the program the
//!   window messages that are its own.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod fmod;
mod paint;
mod place;
pub mod uia;
#[cfg(windows)]
pub mod win;

pub use paint::{Canvas, FOCUS, FOCUS_BAR, PLACEHOLDER, Set, paint};
pub use place::{DEFAULT_SIZE, Measure, Placed, click, hit, place, right_click, typed};

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// The most elements a view may have.
pub const MAX_ELEMENTS: usize = 4000;
/// The deepest a view may nest.
pub const MAX_DEPTH: usize = 24;

/// A window's part: its state, the view built from it and how a message
/// changes it.
pub trait Component {
    type Msg: Clone + fmt::Debug;
    fn view(&self) -> Node<Self::Msg>;
    fn update(&mut self, msg: Self::Msg);
    /// A window message the window does not handle itself (WM_COPYDATA,
    /// WM_USER and up: the program's own, a notification's callback;
    /// WM_ACTIVATE, whose default handling is still done after; WM_HOTKEY;
    /// WM_SETTINGCHANGE; WM_KEYDOWN, whose default handling is done when
    /// the program has no message for the key):
    /// the program's message for it, if any, handled as a click's is
    /// (logged, `update`, the view built again). `window` is the
    /// window's handle.
    fn message(
        &mut self,
        window: usize,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> Option<Self::Msg> {
        let _ = (window, msg, wparam, lparam);
        None
    }
    /// The same messages, answered: the window message's result when the
    /// program has one (WM_COPYDATA's request done or refused, a count
    /// read back), asked before [`Component::message`]. The view is built
    /// again after an answer (the state may have changed).
    fn answer(&mut self, window: usize, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
        let _ = (window, msg, wparam, lparam);
        None
    }
}

/// The kinds of element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Column,
    Row,
    Grid,
    Text,
    Button,
    Input,
    Image,
    Rule,
}

impl Kind {
    pub fn is_container(self) -> bool {
        matches!(self, Kind::Column | Kind::Row | Kind::Grid)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Column => "column",
            Kind::Row => "row",
            Kind::Grid => "grid",
            Kind::Text => "text",
            Kind::Button => "button",
            Kind::Input => "input",
            Kind::Image => "image",
            Kind::Rule => "rule",
        }
    }
}

/// How a container places its children across its flow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Stretch,
    Start,
    Center,
    End,
}

/// How a container places its children along its flow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    Between,
}

/// A grid column: pixels or a share of what is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    Px(f32),
    Fr(f32),
}

/// A grid column of `v` pixels.
pub fn px(v: impl Num) -> Track {
    Track::Px(v.f())
}

/// A grid column taking `v` shares of what is left.
pub fn fr(v: impl Num) -> Track {
    Track::Fr(v.f())
}

/// A number of pixels, written as an integer or a float.
pub trait Num {
    fn f(self) -> f32;
}
impl Num for i32 {
    fn f(self) -> f32 {
        self as f32
    }
}
impl Num for f32 {
    fn f(self) -> f32 {
        self
    }
}

/// Sides (top, right, bottom, left): `8` for all, `(4, 8)` for
/// vertical and horizontal, `(1, 2, 3, 4)` for each.
pub trait Sides {
    fn sides(self) -> [f32; 4];
}
impl Sides for i32 {
    fn sides(self) -> [f32; 4] {
        [self as f32; 4]
    }
}
impl Sides for (i32, i32) {
    fn sides(self) -> [f32; 4] {
        let (v, h) = (self.0 as f32, self.1 as f32);
        [v, h, v, h]
    }
}
impl Sides for (i32, i32, i32, i32) {
    fn sides(self) -> [f32; 4] {
        [self.0 as f32, self.1 as f32, self.2 as f32, self.3 as f32]
    }
}

/// An element's style. Colours are 0xRRGGBB.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_width: Option<f32>,
    pub grow: f32,
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub gap: f32,
    pub align: Align,
    pub justify: Justify,
    pub columns: Vec<Track>,
    pub pin: Option<(f32, f32)>,
    pub background: Option<u32>,
    pub border: Option<u32>,
    pub radius: f32,
    /// Inherited when None.
    pub color: Option<u32>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    /// One line, cut with "..." to its width (`text`).
    pub ellipsis: bool,
}

/// An element of a view and what it holds; `M` is the program's message.
#[derive(Clone, Debug)]
pub struct Node<M> {
    pub kind: Kind,
    pub id: Option<String>,
    /// The text (`text`, `button`), the value (`input`).
    pub text: String,
    pub placeholder: String,
    /// What an `image` shows; the program names it.
    pub src: String,
    pub style: Style,
    pub children: Vec<Node<M>>,
    pub on_click: Option<M>,
    /// The message a right click (the button's release) sends.
    pub on_right_click: Option<M>,
    pub on_input: Option<fn(String) -> M>,
}

fn node<M>(kind: Kind) -> Node<M> {
    Node {
        kind,
        id: None,
        text: String::new(),
        placeholder: String::new(),
        src: String::new(),
        style: Style::default(),
        children: Vec::new(),
        on_click: None,
        on_right_click: None,
        on_input: None,
    }
}

/// Children one under the other.
pub fn column<M>() -> Node<M> {
    node(Kind::Column)
}

/// Children side by side.
pub fn row<M>() -> Node<M> {
    node(Kind::Row)
}

/// Children in cells, row after row, in these columns.
pub fn grid<M>(columns: impl IntoIterator<Item = Track>) -> Node<M> {
    let mut n = node(Kind::Grid);
    n.style.columns = columns.into_iter().collect();
    n
}

/// Text, wrapped to the width it gets.
pub fn text<M>(t: impl Into<String>) -> Node<M> {
    let mut n = node(Kind::Text);
    n.text = t.into();
    n
}

/// A button with its AutomationId and label.
pub fn button<M>(id: impl Into<String>, label: impl Into<String>) -> Node<M> {
    let mut n = node(Kind::Button);
    n.id = Some(id.into());
    n.text = label.into();
    n
}

/// A one-line text field with its AutomationId and value.
pub fn input<M>(id: impl Into<String>, value: impl Into<String>) -> Node<M> {
    let mut n = node(Kind::Input);
    n.id = Some(id.into());
    n.text = value.into();
    n
}

/// A picture the program names and draws; it sets its size.
pub fn image<M>(src: impl Into<String>) -> Node<M> {
    let mut n = node(Kind::Image);
    n.src = src.into();
    n
}

/// A one-pixel line across its parent's flow.
pub fn rule<M>() -> Node<M> {
    node(Kind::Rule)
}

impl<M> Node<M> {
    /// Its UI Automation AutomationId.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
    pub fn width(mut self, v: impl Num) -> Self {
        self.style.width = Some(v.f());
        self
    }
    pub fn height(mut self, v: impl Num) -> Self {
        self.style.height = Some(v.f());
        self
    }
    pub fn min_width(mut self, v: impl Num) -> Self {
        self.style.min_width = Some(v.f());
        self
    }
    /// At least this high; content taller than it grows the element.
    pub fn min_height(mut self, v: impl Num) -> Self {
        self.style.min_height = Some(v.f());
        self
    }
    pub fn max_width(mut self, v: impl Num) -> Self {
        self.style.max_width = Some(v.f());
        self
    }
    /// Its share of the room left along its parent's flow.
    pub fn grow(mut self, v: impl Num) -> Self {
        self.style.grow = v.f();
        self
    }
    pub fn margin(mut self, s: impl Sides) -> Self {
        self.style.margin = s.sides();
        self
    }
    pub fn padding(mut self, s: impl Sides) -> Self {
        self.style.padding = s.sides();
        self
    }
    pub fn gap(mut self, v: impl Num) -> Self {
        self.style.gap = v.f();
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.style.align = a;
        self
    }
    pub fn justify(mut self, j: Justify) -> Self {
        self.style.justify = j;
        self
    }
    /// Out of the flow, at (x, y) in its parent.
    pub fn pin(mut self, x: impl Num, y: impl Num) -> Self {
        self.style.pin = Some((x.f(), y.f()));
        self
    }
    pub fn bg(mut self, rgb: u32) -> Self {
        self.style.background = Some(rgb);
        self
    }
    pub fn border(mut self, rgb: u32) -> Self {
        self.style.border = Some(rgb);
        self
    }
    pub fn radius(mut self, v: impl Num) -> Self {
        self.style.radius = v.f();
        self
    }
    pub fn color(mut self, rgb: u32) -> Self {
        self.style.color = Some(rgb);
        self
    }
    /// The text size in pixels.
    pub fn size(mut self, v: impl Num) -> Self {
        self.style.size = Some(v.f());
        self
    }
    pub fn bold(mut self) -> Self {
        self.style.bold = Some(true);
        self
    }
    /// One line, cut with "..." where it is wider than its room (a
    /// name in a column); it shrinks to that room.
    pub fn ellipsis(mut self) -> Self {
        self.style.ellipsis = true;
        self
    }
    /// Not bold, where a parent is.
    pub fn regular(mut self) -> Self {
        self.style.bold = Some(false);
        self
    }
    /// What an empty `input` shows.
    pub fn placeholder(mut self, t: impl Into<String>) -> Self {
        self.placeholder = t.into();
        self
    }
    pub fn child(mut self, c: Node<M>) -> Self {
        self.children.push(c);
        self
    }
    /// `c` when `cond`; nothing otherwise (not laid out, not in UIA).
    pub fn child_if(self, cond: bool, c: Node<M>) -> Self {
        if cond { self.child(c) } else { self }
    }
    pub fn children(mut self, cs: impl IntoIterator<Item = Node<M>>) -> Self {
        self.children.extend(cs);
        self
    }
    /// The message a click sends.
    pub fn on_click(mut self, m: M) -> Self {
        self.on_click = Some(m);
        self
    }

    /// The message a right click sends.
    pub fn on_right_click(mut self, m: M) -> Self {
        self.on_right_click = Some(m);
        self
    }
    /// The message for the text typed into an `input`.
    pub fn on_input(mut self, f: fn(String) -> M) -> Self {
        self.on_input = Some(f);
        self
    }
}

/// Why a view could not be laid out: the element's path (ids, or kinds
/// and their place among their siblings) and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub path: String,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.msg)
    }
}

//! Laying a view out for a size: text measured by the program, flexbox
//! and grid by Taffy, the view checked on the way.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use taffy::prelude::{
    AlignItems, AvailableSpace, Dimension, Display, FlexDirection, JustifyContent,
    LengthPercentage, LengthPercentageAuto, NodeId, Position, Rect, Size, TaffyTree,
};

use crate::{Align, Error, Justify, Kind, MAX_DEPTH, MAX_ELEMENTS, Node, Track};

/// The text size when nothing sets one (pixels).
pub const DEFAULT_SIZE: f32 = 14.0;

/// The program's text measure (gdi32 in a program, a fixed advance in
/// tests): the size of `text` at `size` pixels, wrapped to `max_width`
/// when given.
pub trait Measure {
    fn text(&mut self, text: &str, size: f32, bold: bool, max_width: Option<f32>) -> (f32, f32);
}

/// A laid-out element, in paint order (the view's order: later on
/// top). `rect` is (x, y, w, h) in the coordinates the view was given.
#[derive(Clone, Debug)]
pub struct Placed<M> {
    pub kind: Kind,
    pub id: Option<String>,
    pub rect: [i32; 4],
    /// The text (`text`, `button`) or value (`input`).
    pub text: String,
    pub placeholder: String,
    pub src: String,
    pub on_click: Option<M>,
    pub on_input: Option<fn(String) -> M>,
    pub background: Option<u32>,
    pub border: Option<u32>,
    pub radius: i32,
    /// Inherited when the element sets none.
    pub color: u32,
    pub size: f32,
    pub bold: bool,
    /// The padding (top, right, bottom, left): the text's inset.
    pub padding: [i32; 4],
    /// One line, cut with "..." (`Node::ellipsis`).
    pub ellipsis: bool,
    /// The element holding it (its index in the list), None for the root.
    pub parent: Option<usize>,
}

/// The element under (x, y) that has an id: the last such in paint
/// order, so the innermost and the topmost.
pub fn hit<M>(placed: &[Placed<M>], x: i32, y: i32) -> Option<&Placed<M>> {
    placed.iter().rev().find(|p| {
        let [px, py, w, h] = p.rect;
        p.id.is_some() && x >= px && y >= py && x < px + w && y < py + h
    })
}

/// What a click at (x, y) does: the element hit (its id, for the log)
/// and the message it sends, when it sends one.
pub fn click<M: Clone>(placed: &[Placed<M>], x: i32, y: i32) -> Option<(&str, M)> {
    let p = hit(placed, x, y)?;
    Some((p.id.as_deref()?, p.on_click.clone()?))
}

/// The message for `text` typed into the input `id`, when it takes text.
pub fn typed<M>(placed: &[Placed<M>], id: &str, text: String) -> Option<M> {
    let p = placed.iter().find(|p| p.id.as_deref() == Some(id))?;
    Some((p.on_input?)(text))
}

/// What the measure needs of a leaf.
struct Leaf {
    kind: Kind,
    /// One line: measured without wrapping.
    line: bool,
    text: String,
    size: f32,
    bold: bool,
}

struct Builder<'a, M> {
    tree: TaffyTree<usize>,
    leaves: Vec<Leaf>,
    /// Per placed element, in paint order: its node.
    order: Vec<(NodeId, Placed<M>)>,
    /// The element being added and those holding it, with their place
    /// among their siblings: the path an error names.
    path: Vec<(&'a Node<M>, usize)>,
}

fn len(v: f32) -> LengthPercentage {
    LengthPercentage::length(v)
}

fn sides<T>(s: [f32; 4], f: impl Fn(f32) -> T) -> Rect<T> {
    Rect { top: f(s[0]), right: f(s[1]), bottom: f(s[2]), left: f(s[3]) }
}

impl<'a, M: Clone> Builder<'a, M> {
    /// An error at the element being added: `ChangeCard > row 3 > MakeButton`.
    fn err(&self, msg: impl Into<String>) -> Error {
        let mut path = String::new();
        for (i, (n, at)) in self.path.iter().enumerate() {
            if i > 0 {
                path.push_str(" > ");
            }
            match &n.id {
                Some(id) => path.push_str(id),
                None => path.push_str(&format!("{} {}", n.kind.name(), at + 1)),
            }
        }
        Error { path, msg: msg.into() }
    }

    /// Adds `n`, the `at`th of its siblings, and what it holds;
    /// `inherit` is (color, size, bold).
    fn add(
        &mut self,
        n: &'a Node<M>,
        at: usize,
        inherit: (u32, f32, bool),
        root: Option<(f32, Option<f32>)>,
    ) -> Result<NodeId, Error> {
        self.path.push((n, at));
        let id = self.add_inner(n, inherit, root)?;
        self.path.pop();
        Ok(id)
    }

    fn add_inner(
        &mut self,
        n: &'a Node<M>,
        inherit: (u32, f32, bool),
        root: Option<(f32, Option<f32>)>,
    ) -> Result<NodeId, Error> {
        if self.order.len() >= MAX_ELEMENTS {
            return Err(self.err(format!("more than {MAX_ELEMENTS} elements")));
        }
        if self.path.len() > MAX_DEPTH {
            return Err(self.err(format!("nested deeper than {MAX_DEPTH}")));
        }
        if n.id.is_none() && (n.on_click.is_some() || n.on_input.is_some()) {
            return Err(self.err("a handler needs an id"));
        }
        if n.on_input.is_some() && n.kind != Kind::Input {
            return Err(self.err(format!("on_input on a {}", n.kind.name())));
        }
        if !n.kind.is_container() && !n.children.is_empty() {
            return Err(self.err(format!("a {} holds no elements", n.kind.name())));
        }
        let s = &n.style;
        if s.pin.is_some() && s.width.is_none() {
            return Err(self.err("a pinned element sets its width"));
        }
        let (color, size, bold) = (
            s.color.unwrap_or(inherit.0),
            s.size.unwrap_or(inherit.1),
            s.bold.unwrap_or(inherit.2),
        );
        let mut st = taffy::Style::<String> { display: Display::Flex, ..Default::default() };
        let auto = |v: Option<f32>| v.map_or(Dimension::auto(), Dimension::length);
        st.size = Size { width: auto(s.width), height: auto(s.height) };
        if let Some((w, h)) = root {
            if s.width.is_none() {
                st.size.width = Dimension::length(w);
            }
            if let (None, Some(h)) = (s.height, h) {
                st.size.height = Dimension::length(h);
            }
        }
        let lpa =
            |v: Option<f32>| v.map_or(LengthPercentageAuto::auto(), LengthPercentageAuto::length);
        st.min_size.width = lpa(s.min_width);
        st.min_size.height = lpa(s.min_height);
        st.max_size.width = lpa(s.max_width);
        st.flex_grow = s.grow;
        st.margin = sides(s.margin, LengthPercentageAuto::length);
        st.padding = sides(s.padding, len);
        st.gap = Size { width: len(s.gap), height: len(s.gap) };
        st.align_items = match s.align {
            Align::Stretch => AlignItems::STRETCH,
            Align::Start => AlignItems::START,
            Align::Center => AlignItems::CENTER,
            Align::End => AlignItems::END,
        };
        st.justify_content = match s.justify {
            Justify::Start => JustifyContent::START,
            Justify::Center => JustifyContent::CENTER,
            Justify::End => JustifyContent::END,
            Justify::Between => JustifyContent::SPACE_BETWEEN,
        };
        if let Some((x, y)) = s.pin {
            st.position = Position::Absolute;
            st.inset = Rect {
                left: LengthPercentageAuto::length(x),
                top: LengthPercentageAuto::length(y),
                right: LengthPercentageAuto::auto(),
                bottom: LengthPercentageAuto::auto(),
            };
        }
        match n.kind {
            Kind::Column => st.flex_direction = FlexDirection::Column,
            Kind::Row => st.flex_direction = FlexDirection::Row,
            Kind::Grid => {
                st.display = Display::Grid;
                st.grid_template_columns = s
                    .columns
                    .iter()
                    .map(|t| match *t {
                        Track::Px(v) => taffy::style_helpers::length(v),
                        Track::Fr(v) => taffy::style_helpers::fr(v),
                    })
                    .collect();
            }
            Kind::Rule => {
                // One pixel across the parent's flow (a row's rule is upright).
                st.flex_shrink = 0.0;
                st.min_size = Size {
                    width: LengthPercentageAuto::length(1.0),
                    height: LengthPercentageAuto::length(1.0),
                };
            }
            _ => {}
        }
        if s.ellipsis {
            // Its text is cut, so it may be narrower than the text.
            st.min_size.width = LengthPercentageAuto::length(0.0);
        }
        let placed = Placed {
            kind: n.kind,
            id: n.id.clone(),
            rect: [0; 4],
            text: n.text.clone(),
            placeholder: n.placeholder.clone(),
            src: n.src.clone(),
            on_click: n.on_click.clone(),
            on_input: n.on_input,
            background: s.background,
            border: s.border,
            radius: s.radius as i32,
            color,
            size,
            bold,
            padding: s.padding.map(|v| v as i32),
            ellipsis: s.ellipsis,
            parent: None,
        };
        let id = if n.kind.is_container() {
            let id = self.tree.new_leaf(st).map_err(|e| self.err(format!("layout: {e:?}")))?;
            self.order.push((id, placed));
            let mut kids = Vec::new();
            for (i, c) in n.children.iter().enumerate() {
                kids.push(self.add(c, i, (color, size, bold), None)?);
            }
            self.tree.set_children(id, &kids).map_err(|e| self.err(format!("layout: {e:?}")))?;
            id
        } else {
            let measured = if n.kind == Kind::Input && n.text.is_empty() {
                n.placeholder.clone()
            } else {
                n.text.clone()
            };
            self.leaves.push(Leaf {
                kind: n.kind,
                line: n.kind == Kind::Input || s.ellipsis,
                text: measured,
                size,
                bold,
            });
            let id = self
                .tree
                .new_leaf_with_context(st, self.leaves.len() - 1)
                .map_err(|e| self.err(format!("layout: {e:?}")))?;
            self.order.push((id, placed));
            id
        };
        Ok(id)
    }
}

/// Lays `view` out in a `width` (and, when given, `height`) at (0, 0):
/// its elements in paint order. Refuses a view that breaks the rules
/// (crate docs), naming the element.
pub fn place<M: Clone>(
    view: &Node<M>,
    width: f32,
    height: Option<f32>,
    measure: &mut dyn Measure,
) -> Result<Vec<Placed<M>>, Error> {
    let mut b =
        Builder { tree: TaffyTree::new(), leaves: Vec::new(), order: Vec::new(), path: Vec::new() };
    let root = b.add(view, 0, (0x000000, DEFAULT_SIZE, false), Some((width, height)))?;
    // Ids are unique: they are what UIA and the log name.
    let mut ids: Vec<&str> = b.order.iter().filter_map(|(_, p)| p.id.as_deref()).collect();
    ids.sort_unstable();
    if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(Error { path: String::from(w[0]), msg: String::from("the id is used twice") });
    }
    let space = Size {
        width: AvailableSpace::Definite(width),
        height: height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
    };
    let leaves = &b.leaves;
    let fail = |e: taffy::TaffyError| Error { path: String::new(), msg: format!("layout: {e:?}") };
    b.tree
        .compute_layout_with_measure(root, space, |input, _, ctx, style| {
            taffy::compute_leaf_layout(
                input,
                style,
                |_, _| 0.0,
                |known, avail| {
                    let Some(leaf) = ctx.map(|i| &leaves[*i]) else { return Size::ZERO };
                    if !matches!(leaf.kind, Kind::Text | Kind::Button | Kind::Input) {
                        return Size::ZERO;
                    }
                    let max = known.width.or(match avail.width {
                        AvailableSpace::Definite(w) => Some(w),
                        _ => None,
                    });
                    // An input, and a text cut with "...", are one line:
                    // measured without wrapping.
                    let wrap = if leaf.line { None } else { max };
                    let (w, h) = measure.text(&leaf.text, leaf.size, leaf.bold, wrap);
                    let h = if leaf.kind == Kind::Input && leaf.text.is_empty() {
                        measure.text("Ag", leaf.size, leaf.bold, None).1
                    } else {
                        h
                    };
                    Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
                },
            )
        })
        .map_err(fail)?;
    // Absolute places: each node's location is its parent's plus its own.
    let mut origin: Vec<(NodeId, (f32, f32))> = Vec::new();
    let mut out = Vec::with_capacity(b.order.len());
    for (id, mut p) in b.order {
        let l = b.tree.layout(id).map_err(fail)?;
        // The parent was placed before its children: it is in `origin`,
        // at its index in `out`.
        let parent = b.tree.parent(id).and_then(|pid| origin.iter().rposition(|(n, _)| *n == pid));
        let base = parent.map_or((0.0, 0.0), |i| origin[i].1);
        p.parent = parent;
        let (x, y) = (base.0 + l.location.x, base.1 + l.location.y);
        origin.push((id, (x, y)));
        p.rect = [x as i32, y as i32, l.size.width as i32, l.size.height as i32];
        out.push(p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, Justify, button, column, fr, grid, input, px, row, text};
    use alloc::string::ToString;

    /// 7 pixels a character at size 14 (scaled with the size), lines of
    /// size + 4, wrapped at word boundaries.
    struct Fixed;
    impl Measure for Fixed {
        fn text(&mut self, text: &str, size: f32, _bold: bool, max: Option<f32>) -> (f32, f32) {
            let adv = size / 2.0;
            let line = size + 4.0;
            let full = text.chars().count() as f32 * adv;
            match max {
                Some(m) if full > m && m > 0.0 => {
                    let (mut lines, mut cur) = (1.0, 0.0);
                    for w in text.split(' ') {
                        let ww = w.chars().count() as f32 * adv;
                        if cur > 0.0 && cur + adv + ww > m {
                            lines += 1.0;
                            cur = ww;
                        } else {
                            cur += if cur > 0.0 { adv + ww } else { ww };
                        }
                    }
                    (m, lines * line)
                }
                _ => (full, line),
            }
        }
    }

    fn at<'a, M>(p: &'a [Placed<M>], id: &str) -> &'a Placed<M> {
        p.iter().find(|x| x.id.as_deref() == Some(id)).unwrap()
    }

    fn lay<M: Clone>(n: &Node<M>, w: f32) -> Vec<Placed<M>> {
        place(n, w, None, &mut Fixed).unwrap()
    }

    #[test]
    fn a_column_stacks_with_padding_and_gap() {
        let v: Node<()> = column()
            .id("C")
            .padding(10)
            .gap(6)
            .child(text("abcd").id("A"))
            .child(text("xy").id("B").size(20));
        let p = lay(&v, 200.0);
        // The root takes the width given, its height from its content.
        assert_eq!(at(&p, "C").rect, [0, 0, 200, 10 + 18 + 6 + 24 + 10]);
        // Stretched across (align defaults to stretch), stacked with the gap.
        assert_eq!(at(&p, "A").rect, [10, 10, 180, 18]);
        assert_eq!(at(&p, "B").rect, [10, 34, 180, 24]);
        assert_eq!((at(&p, "B").size, at(&p, "A").size), (20.0, DEFAULT_SIZE));
    }

    #[test]
    fn a_row_justifies_and_grows() {
        let v = row()
            .id("R")
            .gap(8)
            .justify(Justify::End)
            .align(crate::Align::Start)
            .child(button("X", "ab").padding((4, 8)).on_click('x'))
            .child(button("Y", "abcd").padding((4, 8)).on_click('y'));
        let p = lay(&v, 300.0);
        // Widths: text + padding (14+16, 28+16); pushed to the end.
        assert_eq!(at(&p, "Y").rect, [300 - 44, 0, 44, 26]);
        assert_eq!(at(&p, "X").rect, [300 - 44 - 8 - 30, 0, 30, 26]);
        assert_eq!(at(&p, "X").on_click, Some('x'));
        let v: Node<()> = row().child(text("a").id("L").grow(1)).child(text("b").id("F").width(50));
        let p = lay(&v, 300.0);
        assert_eq!((at(&p, "L").rect[2], at(&p, "F").rect), (250, [250, 0, 50, 18]));
    }

    #[test]
    fn a_minimum_height_holds_and_content_grows_past_it() {
        let v: Node<()> = column().width(100).child(text("ab").id("S").min_height(40));
        assert_eq!(at(&lay(&v, 400.0), "S").rect, [0, 0, 100, 40]);
        // Three lines of 18 (three words a line) outgrow the 40.
        let v: Node<()> = column()
            .width(100)
            .child(text("aaaa bbbb cccc dddd eeee ffff gggg").id("T").min_height(40));
        assert_eq!(at(&lay(&v, 400.0), "T").rect, [0, 0, 100, 54]);
    }

    #[test]
    fn an_ellipsis_text_is_one_line_and_shrinks_to_its_room() {
        // Wrapped, the text would take two lines; cut, it keeps one and
        // the row's 100 px.
        let v: Node<()> = row().width(100).child(text("aaaa bbbb cccc dddd").id("T").ellipsis());
        let p = lay(&v, 400.0);
        assert_eq!(at(&p, "T").rect, [0, 0, 100, 18]);
        assert!(at(&p, "T").ellipsis);
        // Shorter than its room, it is as wide as its text.
        let v: Node<()> = row().width(100).child(text("ab").id("T").ellipsis());
        assert_eq!(at(&lay(&v, 400.0), "T").rect, [0, 0, 14, 18]);
    }

    #[test]
    fn text_wraps_to_its_width() {
        let v: Node<()> = column().width(100).child(text("aaaa bbbb cccc dddd").id("T"));
        // 7 px a character: two words a line in 100 px.
        assert_eq!(at(&lay(&v, 400.0), "T").rect, [0, 0, 100, 36]);
    }

    #[test]
    fn a_grid_places_cells_row_after_row() {
        let v: Node<()> = grid([px(100), fr(1), fr(2)])
            .gap(10)
            .children(["a", "b", "c", "d"].map(|i| text(i).id(i)));
        let p = lay(&v, 400.0);
        // 400 - 100 - 2*10 = 280 shared 1:2.
        assert_eq!(at(&p, "a").rect, [0, 0, 100, 18]);
        assert_eq!(at(&p, "b").rect, [110, 0, 93, 18]);
        assert_eq!(at(&p, "c").rect, [213, 0, 187, 18]);
        assert_eq!(at(&p, "d").rect, [0, 28, 100, 18]);
    }

    #[test]
    fn a_pinned_element_is_out_of_the_flow_and_painted_in_order() {
        let v: Node<()> = column()
            .id("C")
            .height(100)
            .child(text("body").id("Body").height(100))
            .child(text("9").id("Badge").pin(70, 4).width(20).bg(0xff0000));
        let p = place(&v, 100.0, Some(100.0), &mut Fixed).unwrap();
        assert_eq!(at(&p, "Badge").rect, [70, 4, 20, 18]);
        assert_eq!(at(&p, "Badge").background, Some(0xff0000));
        // The badge is on top where it overlaps the body: it is later.
        assert_eq!(hit(&p, 75, 10).and_then(|x| x.id.as_deref()), Some("Badge"));
        assert_eq!(hit(&p, 10, 10).and_then(|x| x.id.as_deref()), Some("Body"));
        let ids: Vec<_> = p.iter().filter_map(|x| x.id.as_deref()).collect();
        assert_eq!(ids, ["C", "Body", "Badge"]);
    }

    #[test]
    fn style_is_inherited_and_an_empty_input_is_a_line_high() {
        let v: Node<()> = column()
            .id("C")
            .color(0x123456)
            .bold()
            .child(text("Change it").id("T"))
            .child(text("plain").id("P").regular())
            .child(input("I", "").placeholder("Describe it").color(0xffffff));
        let p = lay(&v, 300.0);
        assert_eq!((at(&p, "T").color, at(&p, "T").bold), (0x123456, true));
        assert!(!at(&p, "P").bold);
        let i = at(&p, "I");
        assert_eq!(
            (i.text.as_str(), i.placeholder.as_str(), i.color),
            ("", "Describe it", 0xffffff)
        );
        assert_eq!(i.rect[3], 18);
    }

    #[test]
    fn hit_finds_the_innermost_element_with_an_id() {
        let v: Node<()> = column()
            .id("Pane")
            .padding(10)
            .child(row().child(text("abc").id("Leaf")).child(text("no id")));
        let p = lay(&v, 200.0);
        assert_eq!(hit(&p, 12, 12).and_then(|x| x.id.as_deref()), Some("Leaf"));
        // Over the text without an id: its nearest parent with one.
        assert_eq!(hit(&p, 60, 12).and_then(|x| x.id.as_deref()), Some("Pane"));
        assert!(hit(&p, 500, 500).is_none());
    }

    #[test]
    fn a_broken_view_is_refused_naming_the_element() {
        let e = |v: &Node<u8>| place(v, 100.0, None, &mut Fixed).unwrap_err().to_string();
        let v = column().id("Card").child(row().child(text("x").on_click(1)));
        assert_eq!(e(&v), "Card > row 1 > text 1: a handler needs an id");
        let v = column().child(text("a")).child(button("Go", "go")).child(button("Go", "again"));
        assert_eq!(e(&v), "Go: the id is used twice");
        let v = column().child(text("x").id("Badge").pin(1, 1));
        assert_eq!(e(&v), "column 1 > Badge: a pinned element sets its width");
        let v = column().child(button("B", "b").child(text("x")));
        assert_eq!(e(&v), "column 1 > B: a button holds no elements");
        let mut deep = text("leaf");
        for _ in 0..MAX_DEPTH {
            deep = column().child(deep);
        }
        assert!(e(&deep).ends_with(&format!("text 1: nested deeper than {MAX_DEPTH}")));
        let wide = column().children((0..MAX_ELEMENTS).map(|_| text("x")));
        assert!(e(&wide).ends_with(&format!("more than {MAX_ELEMENTS} elements")));
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Request(String),
        Make,
    }

    struct Card {
        request: String,
        made: u32,
    }

    impl Component for Card {
        type Msg = Msg;
        fn view(&self) -> Node<Msg> {
            column()
                .id("ChangeCard")
                .gap(12)
                .padding(16)
                .child(text("Change File Explorer").size(20).bold())
                .child(input("Request", &self.request).on_input(Msg::Request))
                .child(row().justify(Justify::End).child_if(
                    !self.request.is_empty(),
                    button("MakeButton", "Make the change").on_click(Msg::Make),
                ))
        }
        fn update(&mut self, msg: Msg) {
            match msg {
                Msg::Request(r) => self.request = r,
                Msg::Make => self.made += 1,
            }
        }
    }

    #[test]
    fn a_component_round_trip_typing_then_clicking() {
        let mut card = Card { request: String::new(), made: 0 };
        let p = lay(&card.view(), 520.0);
        // Nothing to make yet: no button, so nothing to hit.
        assert!(p.iter().all(|x| x.id.as_deref() != Some("MakeButton")));
        let m = typed(&p, "Request", "Add a Back button".to_string()).unwrap();
        assert_eq!(m, Msg::Request("Add a Back button".to_string()));
        card.update(m);
        // The view is built again from the state.
        let p = lay(&card.view(), 520.0);
        let [x, y, ..] = at(&p, "MakeButton").rect;
        let (id, m) = click(&p, x + 1, y + 1).unwrap();
        assert_eq!((id, &m), ("MakeButton", &Msg::Make));
        card.update(m);
        assert_eq!(card.made, 1);
        // A click on what sends nothing is no message.
        assert!(click(&p, 2, 2).is_none());
        assert!(typed(&p, "MakeButton", String::new()).is_none());
    }
}

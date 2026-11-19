//! Drawing a laid-out view: what each element paints, in paint order, on
//! the program's canvas (gdi32 in a window, a recorder in tests).

use crate::{Kind, Placed};

/// What an empty input's placeholder is written in.
pub const PLACEHOLDER: u32 = 0x767676;
/// The bar under the input that has the keyboard focus.
pub const FOCUS: u32 = 0x005fb8;
/// How high the focus bar is.
pub const FOCUS_BAR: i32 = 2;

/// How a text is set in its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    /// Wrapped at word boundaries, from the top left (`text`).
    Wrap,
    /// One line, centred both ways (`button`).
    Centre,
    /// One line, from the left, centred upright (`input`).
    Line,
    /// One line, from the left, centred upright, cut with "..." where
    /// it is wider than the rectangle (`text` with `ellipsis`).
    Cut,
}

/// Where the elements are drawn; rectangles are (x, y, w, h), colours
/// 0xRRGGBB.
pub trait Canvas {
    /// Fills `rect`, its corners rounded by `radius`.
    fn fill(&mut self, rect: [i32; 4], color: u32, radius: i32);
    /// A one-pixel line around the inside of `rect`.
    fn frame(&mut self, rect: [i32; 4], color: u32, radius: i32);
    fn text(&mut self, rect: [i32; 4], text: &str, color: u32, size: f32, bold: bool, set: Set);
}

/// Draws `placed` (as [`crate::place`] gives it) in paint order: each
/// element's background, then its border, then its text inset by its
/// padding. An empty input shows its placeholder in [`PLACEHOLDER`];
/// the input whose id is `focus` gets a [`FOCUS`] bar along its bottom.
/// A rule without a background is drawn in its colour. An image draws
/// nothing yet (ARCHITECTURE "UI components": limits).
pub fn paint<M>(placed: &[Placed<M>], focus: Option<&str>, canvas: &mut dyn Canvas) {
    for p in placed {
        let [x, y, w, h] = p.rect;
        if w <= 0 || h <= 0 {
            continue;
        }
        if let Some(bg) = p.background {
            canvas.fill(p.rect, bg, p.radius);
        } else if p.kind == Kind::Rule {
            canvas.fill(p.rect, p.color, 0);
        }
        if let Some(b) = p.border {
            canvas.frame(p.rect, b, p.radius);
        }
        let [t, r, b, l] = p.padding;
        let inner = [x + l, y + t, (w - l - r).max(0), (h - t - b).max(0)];
        match p.kind {
            Kind::Text if !p.text.is_empty() => {
                let set = if p.ellipsis { Set::Cut } else { Set::Wrap };
                canvas.text(inner, &p.text, p.color, p.size, p.bold, set)
            }
            Kind::Button if !p.text.is_empty() => {
                canvas.text(inner, &p.text, p.color, p.size, p.bold, Set::Centre)
            }
            Kind::Input => {
                if !p.text.is_empty() {
                    canvas.text(inner, &p.text, p.color, p.size, p.bold, Set::Line);
                } else if !p.placeholder.is_empty() {
                    canvas.text(inner, &p.placeholder, PLACEHOLDER, p.size, p.bold, Set::Line);
                }
                if focus.is_some() && p.id.as_deref() == focus {
                    canvas.fill([x, y + h - FOCUS_BAR, w, FOCUS_BAR], FOCUS, 0);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Measure, Node, button, column, input, place, row, rule, text};
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// 7 pixels a character, lines of 18.
    struct Fixed;
    impl Measure for Fixed {
        fn text(&mut self, t: &str, _: f32, _: bool, _: Option<f32>) -> (f32, f32) {
            (t.chars().count() as f32 * 7.0, 18.0)
        }
    }

    /// The canvas calls, as lines.
    #[derive(Default)]
    struct Record(Vec<String>);
    impl Canvas for Record {
        fn fill(&mut self, r: [i32; 4], c: u32, radius: i32) {
            self.0.push(format!("fill {r:?} {c:06x} r{radius}"));
        }
        fn frame(&mut self, r: [i32; 4], c: u32, radius: i32) {
            self.0.push(format!("frame {r:?} {c:06x} r{radius}"));
        }
        fn text(&mut self, r: [i32; 4], t: &str, c: u32, size: f32, bold: bool, set: Set) {
            self.0.push(format!("text {r:?} {t:?} {c:06x} {size} {bold} {set:?}"));
        }
    }

    fn draw(v: &Node<()>, focus: Option<&str>) -> Vec<String> {
        let p = place(v, 200.0, None, &mut Fixed).unwrap();
        let mut r = Record::default();
        paint(&p, focus, &mut r);
        r.0
    }

    #[test]
    fn background_then_border_then_text_inset_by_padding() {
        let v = column().id("C").bg(0xffffff).padding(10).child(
            button("B", "Go")
                .bg(0x005fb8)
                .border(0x003a70)
                .color(0xffffff)
                .radius(4)
                .padding((2, 6)),
        );
        assert_eq!(
            draw(&v, None),
            [
                "fill [0, 0, 200, 42] ffffff r0",
                "fill [10, 10, 180, 22] 005fb8 r4",
                "frame [10, 10, 180, 22] 003a70 r4",
                "text [16, 12, 168, 18] \"Go\" ffffff 14 false Centre",
            ]
        );
    }

    #[test]
    fn an_empty_input_shows_its_placeholder_and_the_focus_its_bar() {
        let v = column().child(input("I", "").placeholder("Your name")).child(text("x").bold());
        let d = draw(&v, Some("I"));
        assert_eq!(
            d,
            [
                "text [0, 0, 200, 18] \"Your name\" 767676 14 false Line",
                "fill [0, 16, 200, 2] 005fb8 r0",
                "text [0, 18, 200, 18] \"x\" 000000 14 true Wrap",
            ]
        );
        // Typed text in the text's colour; no bar without the focus.
        let v: Node<()> = column().child(input("I", "hi").color(0x123456));
        assert_eq!(draw(&v, Some("J")), ["text [0, 0, 200, 18] \"hi\" 123456 14 false Line"]);
    }

    #[test]
    fn an_ellipsis_text_is_set_cut() {
        let v: Node<()> = column().child(text("a long name").ellipsis());
        assert_eq!(draw(&v, None), ["text [0, 0, 200, 18] \"a long name\" 000000 14 false Cut"]);
    }

    #[test]
    fn a_rule_is_its_colour_and_empty_elements_draw_nothing() {
        let v: Node<()> = column().color(0x8a8a8a).child(rule()).child(row().id("Empty"));
        assert_eq!(draw(&v, None), ["fill [0, 0, 200, 1] 8a8a8a r0"]);
    }
}

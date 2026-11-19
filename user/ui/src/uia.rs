//! A laid-out view as UI Automation elements: the elements that have an
//! id, each under the nearest element holding it that has one (the
//! window's element, the fragment root, for those under none), in the
//! view's order. What each is: its control type and name.

use crate::{Kind, Placed};

/// UIA_ButtonControlTypeId.
pub const BUTTON: i32 = 50000;
/// UIA_EditControlTypeId.
pub const EDIT: i32 = 50004;
/// UIA_ImageControlTypeId.
pub const IMAGE: i32 = 50006;
/// UIA_TextControlTypeId.
pub const TEXT: i32 = 50020;
/// UIA_GroupControlTypeId.
pub const GROUP: i32 = 50026;
/// UIA_SeparatorControlTypeId.
pub const SEPARATOR: i32 = 50038;

/// The element's UIA control type.
pub fn control_type(kind: Kind) -> i32 {
    match kind {
        Kind::Column | Kind::Row | Kind::Grid => GROUP,
        Kind::Text => TEXT,
        Kind::Button => BUTTON,
        Kind::Input => EDIT,
        Kind::Image => IMAGE,
        Kind::Rule => SEPARATOR,
    }
}

/// The element's UIA name: a text's or button's text, an input's
/// placeholder (its value is not its name), an image's source; a
/// container has none.
pub fn name<M>(p: &Placed<M>) -> &str {
    match p.kind {
        Kind::Text | Kind::Button => &p.text,
        Kind::Input => &p.placeholder,
        Kind::Image => &p.src,
        _ => "",
    }
}

/// The index of the element whose id is `id`.
pub fn find<M>(placed: &[Placed<M>], id: &str) -> Option<usize> {
    placed.iter().position(|p| p.id.as_deref() == Some(id))
}

/// The UIA parent of element `i`: the nearest element holding it that
/// has an id; None for the root (the window's element).
pub fn parent<M>(placed: &[Placed<M>], i: usize) -> Option<usize> {
    let mut at = placed.get(i)?.parent;
    while let Some(a) = at {
        if placed[a].id.is_some() {
            return Some(a);
        }
        at = placed[a].parent;
    }
    None
}

/// The UIA children of `of` (None: the root), in the view's order.
pub fn children<M>(placed: &[Placed<M>], of: Option<usize>) -> impl Iterator<Item = usize> + '_ {
    (0..placed.len())
        .filter(move |&i| placed[i].id.is_some() && Some(i) != of && parent(placed, i) == of)
}

/// The UIA sibling after (`next`) or before element `i`.
pub fn sibling<M>(placed: &[Placed<M>], i: usize, next: bool) -> Option<usize> {
    let kids: alloc::vec::Vec<usize> = children(placed, parent(placed, i)).collect();
    let at = kids.iter().position(|&k| k == i)?;
    if next { kids.get(at + 1).copied() } else { at.checked_sub(1).map(|a| kids[a]) }
}

/// A runtime id's own part for the element `id` (UiaAppendRuntimeId
/// adds the window's): the same for the same id while the window lives
/// (FNV-1a, 31 bits).
pub fn runtime_id(id: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Measure, Node, button, column, input, place, row, text};
    use alloc::vec::Vec;

    struct Fixed;
    impl Measure for Fixed {
        fn text(&mut self, t: &str, _: f32, _: bool, _: Option<f32>) -> (f32, f32) {
            (t.chars().count() as f32 * 7.0, 18.0)
        }
    }

    fn ids<M>(p: &[Placed<M>], it: impl Iterator<Item = usize>) -> Vec<&str> {
        it.map(|i| p[i].id.as_deref().unwrap()).collect()
    }

    #[test]
    fn elements_with_an_id_under_the_nearest_with_one() {
        let v: Node<()> = column()
            .id("Card")
            .child(text("Count: 0").id("Count"))
            .child(row().child(button("Inc", "Add one")).child(button("Reset", "Reset")))
            .child(input("Name", "").placeholder("Your name"))
            .child(text("no id"));
        let p = place(&v, 300.0, None, &mut Fixed).unwrap();
        let at = |id| find(&p, id).unwrap();
        assert_eq!(ids(&p, children(&p, None)), ["Card"]);
        // The row has no id: its buttons are the card's.
        assert_eq!(ids(&p, children(&p, Some(at("Card")))), ["Count", "Inc", "Reset", "Name"]);
        assert_eq!(parent(&p, at("Inc")), Some(at("Card")));
        assert_eq!(parent(&p, at("Card")), None);
        assert_eq!(sibling(&p, at("Inc"), true), Some(at("Reset")));
        assert_eq!(sibling(&p, at("Inc"), false), Some(at("Count")));
        assert_eq!(sibling(&p, at("Name"), true), None);
        assert_eq!(children(&p, Some(at("Inc"))).count(), 0);
        let types: Vec<i32> =
            ["Card", "Count", "Inc", "Name"].map(|i| control_type(p[at(i)].kind)).into();
        assert_eq!(types, [GROUP, TEXT, BUTTON, EDIT]);
        assert_eq!((name(&p[at("Inc")]), name(&p[at("Name")])), ("Add one", "Your name"));
        assert_ne!(runtime_id("Inc"), runtime_id("Reset"));
        assert_eq!(runtime_id("Inc"), runtime_id("Inc"));
        assert!(runtime_id("Inc") > 0);
    }
}

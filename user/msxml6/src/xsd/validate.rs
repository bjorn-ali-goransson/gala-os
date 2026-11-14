//! Validating a document (or an element) against the schemas of a
//! cache: XSD's validation rules for the components `xsd.rs` compiles.
//!
//! The root element needs a global declaration; each element is checked
//! against its type: its attributes (declared, required, fixed, their
//! values' types; wildcards), its content (a content model matched over
//! the element children, text allowed only in mixed and simple content,
//! a simple content's value against its type) and its identity
//! constraints. The first violation ends the validation with MSXML's
//! error code for it (the codes IXMLDOMDocument2::validate's
//! documentation lists) and a reason.
//!
//! What gala does not do while validating is an error too, never a
//! pass: `xsi:type`, `xsi:nil`, a name no schema in the cache declares
//! (`Invalid::unsupported` names it, for the ledger). Default attribute
//! values a schema gives are not added to the tree (counted in
//! `Outcome::defaults`, for the ledger).

use alloc::string::String;
use alloc::vec::Vec;

use super::{
    AttrDecl, AttrTerm, ComplexType, ElemTerm, ElementDecl, Idc, IdcKind, Particle, Path, Process,
    QName, Schema, Set, Step, Term, Type, TypeRef, Wildcard, XSI, simple,
};
use crate::dom::{Kind, Tree};

pub const XML_ELEMENT_UNDECLARED: i32 = 0xc00c_e00du32 as i32;
pub const XML_ELEMENT_NOT_COMPLETE: i32 = 0xc00c_e012u32 as i32;
pub const XML_INVALID_CONTENT: i32 = 0xc00c_e014u32 as i32;
pub const XML_ATTRIBUTE_NOT_DEFINED: i32 = 0xc00c_e015u32 as i32;
pub const XML_ATTRIBUTE_FIXED: i32 = 0xc00c_e016u32 as i32;
pub const XML_ATTRIBUTE_VALUE: i32 = 0xc00c_e017u32 as i32;
pub const XML_ILLEGAL_TEXT: i32 = 0xc00c_e018u32 as i32;
pub const XML_REQUIRED_ATTRIBUTE_MISSING: i32 = 0xc00c_e020u32 as i32;
pub const XML_E_NOTWF: i32 = 0xc00c_e223u32 as i32;
pub const XML_E_NODTD: i32 = 0xc00c_e224u32 as i32;

/// MSXML 6's MaxElementDepth default: deeper documents are refused.
const MAX_DEPTH: u32 = 256;

#[derive(Debug)]
pub struct Invalid {
    pub code: i32,
    pub reason: String,
    /// The node where it was found.
    pub node: u32,
    /// Set when the failure is something gala does not implement (its
    /// name, for the ledger).
    pub unsupported: Option<&'static str>,
}

/// What a validation that passed leaves to report.
#[derive(Default, Debug)]
pub struct Outcome {
    /// Attributes a schema default would have added (gala adds none).
    pub defaults: u32,
}

/// What matched a child element in its parent's content model.
#[derive(Clone, Copy)]
enum Assigned<'a> {
    Decl(&'a Schema, &'a ElementDecl),
    Wild(Process),
}

struct V<'a> {
    set: &'a Set<'a>,
    t: &'a Tree,
    out: Outcome,
}

fn name_of(t: &Tree, i: u32) -> &str {
    &t.node(i).name
}

fn fail(code: i32, node: u32, parts: &[&str]) -> Result<(), Invalid> {
    let mut reason = String::new();
    for p in parts {
        reason.push_str(p);
    }
    reason.push_str("\r\n");
    Err(Invalid { code, reason, node, unsupported: None })
}

fn not_done<T>(node: u32, what: &'static str) -> Result<T, Invalid> {
    let mut reason = String::from("gala does not validate ");
    reason.push_str(what);
    reason.push_str(".\r\n");
    Err(Invalid { code: XML_INVALID_CONTENT, reason, node, unsupported: Some(what) })
}

/// Validates the document `t` (its document element) against `set`.
pub fn document(set: &Set, t: &Tree) -> Result<Outcome, Invalid> {
    let Some(root) = t.root() else {
        return Err(Invalid {
            code: XML_E_NOTWF,
            reason: "The document has no document element.\r\n".into(),
            node: 0,
            unsupported: None,
        });
    };
    element(set, t, root)
}

/// Validates element `el` (and what is in it) by its global
/// declaration.
pub fn element(set: &Set, t: &Tree, el: u32) -> Result<Outcome, Invalid> {
    let n = t.node(el);
    if !set.has_namespace(&n.ns) {
        let mut reason = String::from("No schema in the cache for the namespace '");
        reason.push_str(&n.ns);
        reason.push_str("'.\r\n");
        return Err(Invalid { code: XML_E_NODTD, reason, node: el, unsupported: None });
    }
    let mut v = V { set, t, out: Outcome::default() };
    let q = QName { ns: n.ns.clone(), local: n.local().into() };
    match set.element(&q) {
        Some((s, d)) => v.element(s, d, el, 0)?,
        None => fail(
            XML_ELEMENT_UNDECLARED,
            el,
            &["The element '", name_of(t, el), "' is used but not declared in the schema."],
        )?,
    }
    Ok(v.out)
}

/// The effective content of a complex type (its base's added).
enum Content<'a> {
    Empty,
    Simple(&'a Schema, &'a TypeRef),
    /// The particles in order (an extension's after its base's).
    Elements(Vec<(&'a Schema, &'a Particle)>),
}

struct Effective<'a> {
    mixed: bool,
    attrs: Vec<(&'a Schema, &'a super::AttrUse)>,
    any_attr: Option<&'a Wildcard>,
    content: Content<'a>,
}

impl<'a> V<'a> {
    fn element(
        &mut self,
        s: &'a Schema,
        d: &'a ElementDecl,
        el: u32,
        depth: u32,
    ) -> Result<(), Invalid> {
        let t = self.t;
        if depth > MAX_DEPTH {
            return fail(XML_INVALID_CONTENT, el, &["The document is nested too deeply."]);
        }
        if d.is_abstract {
            return fail(
                XML_INVALID_CONTENT,
                el,
                &["The element '", name_of(t, el), "' is abstract."],
            );
        }
        for &a in &t.node(el).attrs {
            let an = t.node(a);
            if an.ns == XSI {
                match an.local() {
                    "type" => return not_done(a, "xsi:type"),
                    "nil" => return not_done(a, "xsi:nil"),
                    _ => {}
                }
            }
        }
        match &d.typ {
            None => self.lax_content(el, depth),
            Some(r) => match self.resolve(s, r, el)? {
                Resolved::Simple(s2, r2) => {
                    self.no_attributes(el)?;
                    self.simple_element(s2, r2, d, el)
                }
                Resolved::Complex(s2, ct) => {
                    let eff = self.effective(s2, ct, el, 0)?;
                    self.attributes(&eff, el)?;
                    self.content(&eff, d, el, depth)?;
                    self.identity(d, el)
                }
                Resolved::AnyType => self.lax_content(el, depth),
            },
        }
    }

    fn resolve(&self, s: &'a Schema, r: &'a TypeRef, el: u32) -> Result<Resolved<'a>, Invalid> {
        match r {
            TypeRef::Builtin(simple::Builtin::AnyType) => Ok(Resolved::AnyType),
            TypeRef::Builtin(_) => Ok(Resolved::Simple(s, r)),
            TypeRef::Local(i) => match &s.types[*i] {
                Type::Simple(_) => Ok(Resolved::Simple(s, r)),
                Type::Complex(ct) => Ok(Resolved::Complex(s, ct)),
            },
            TypeRef::Named(q) => match self.set.named_type(q) {
                Some((_, Type::Simple(_))) => Ok(Resolved::Simple(s, r)),
                Some((s2, Type::Complex(ct))) => Ok(Resolved::Complex(s2, ct)),
                None => not_done(el, "a type no schema in the cache declares"),
            },
        }
    }

    /// anyType's content: any attributes, any children, each validated
    /// when a schema declares it ("lax").
    fn lax_content(&mut self, el: u32, depth: u32) -> Result<(), Invalid> {
        let t = self.t;
        for &k in &t.node(el).children {
            if t.node(k).kind == Kind::Element {
                self.lax(k, depth + 1)?;
            }
        }
        Ok(())
    }

    fn lax(&mut self, el: u32, depth: u32) -> Result<(), Invalid> {
        let n = self.t.node(el);
        let q = QName { ns: n.ns.clone(), local: n.local().into() };
        match self.set.element(&q) {
            Some((s, d)) => self.element(s, d, el, depth),
            None => self.lax_content(el, depth),
        }
    }

    fn no_attributes(&self, el: u32) -> Result<(), Invalid> {
        let t = self.t;
        for &a in &t.node(el).attrs {
            let an = t.node(a);
            if !an.is_xmlns() && an.ns != XSI {
                return fail(
                    XML_ATTRIBUTE_NOT_DEFINED,
                    a,
                    &[
                        "The attribute '",
                        &an.name,
                        "' on this element is not defined in the schema.",
                    ],
                );
            }
        }
        Ok(())
    }

    fn text_of(&self, el: u32) -> String {
        let mut s = String::new();
        for &k in &self.t.node(el).children {
            let n = self.t.node(k);
            if matches!(n.kind, Kind::Text | Kind::CData) {
                s.push_str(&n.value);
            }
        }
        s
    }

    fn element_children(&self, el: u32) -> Vec<u32> {
        self.t
            .node(el)
            .children
            .iter()
            .copied()
            .filter(|&k| self.t.node(k).kind == Kind::Element)
            .collect()
    }

    fn simple_element(
        &mut self,
        s: &'a Schema,
        r: &'a TypeRef,
        d: &ElementDecl,
        el: u32,
    ) -> Result<(), Invalid> {
        if let Some(&k) = self.element_children(el).first() {
            return fail(
                XML_INVALID_CONTENT,
                k,
                &[
                    "The element '",
                    name_of(self.t, el),
                    "' cannot contain the element '",
                    name_of(self.t, k),
                    "'.",
                ],
            );
        }
        self.value(s, r, d, el)
    }

    /// An element's text against its simple type, its default and fixed
    /// value.
    fn value(
        &mut self,
        s: &'a Schema,
        r: &'a TypeRef,
        d: &ElementDecl,
        el: u32,
    ) -> Result<(), Invalid> {
        let mut text = self.text_of(el);
        if text.is_empty()
            && let Some(def) = &d.default
        {
            text = def.clone();
        }
        if let Some(fixed) = &d.fixed
            && simple::check(self.set, s, r, &text).is_ok()
            && text != *fixed
        {
            return fail(
                XML_INVALID_CONTENT,
                el,
                &["The element '", name_of(self.t, el), "' does not have its fixed value."],
            );
        }
        match simple::check(self.set, s, r, &text) {
            Ok(()) => Ok(()),
            Err(why) => fail(
                XML_INVALID_CONTENT,
                el,
                &[
                    "The element '",
                    name_of(self.t, el),
                    "' has an invalid value according to its type: ",
                    &why,
                ],
            ),
        }
    }

    /// A complex type with its base types' attributes and content.
    fn effective(
        &self,
        s: &'a Schema,
        ct: &'a ComplexType,
        el: u32,
        depth: u32,
    ) -> Result<Effective<'a>, Invalid> {
        if depth > 64 {
            fail(XML_INVALID_CONTENT, el, &["A type derived from itself."])?;
        }
        let mut eff = match &ct.base {
            None => Effective {
                mixed: false,
                attrs: Vec::new(),
                any_attr: None,
                content: Content::Empty,
            },
            Some(b) => match self.resolve(s, b, el)? {
                Resolved::Complex(s2, base) => self.effective(s2, base, el, depth + 1)?,
                Resolved::Simple(s2, r) => Effective {
                    mixed: false,
                    attrs: Vec::new(),
                    any_attr: None,
                    content: Content::Simple(s2, r),
                },
                Resolved::AnyType => return not_done(el, "an extension of anyType"),
            },
        };
        eff.mixed |= ct.mixed;
        eff.attrs.extend(ct.attrs.iter().map(|a| (s, a)));
        if ct.any_attr.is_some() {
            eff.any_attr = ct.any_attr.as_ref();
        }
        if let Some(p) = &ct.particle {
            match &mut eff.content {
                Content::Elements(list) => list.push((s, p)),
                Content::Empty => eff.content = Content::Elements(alloc::vec![(s, p)]),
                Content::Simple(..) => {
                    fail(XML_INVALID_CONTENT, el, &["A simple content extended by elements."])?
                }
            }
        }
        Ok(eff)
    }

    /// The declaration behind an attribute use, with its schema.
    fn attr_decl(
        &self,
        s: &'a Schema,
        u: &'a super::AttrUse,
        el: u32,
    ) -> Result<(&'a Schema, &'a AttrDecl), Invalid> {
        match &u.term {
            AttrTerm::Decl(d) => Ok((s, d)),
            AttrTerm::Ref(q) => match self.set.attribute(q) {
                Some(found) => Ok(found),
                None => not_done(el, "an attribute no schema in the cache declares"),
            },
        }
    }

    fn attr_value(
        &self,
        s: &'a Schema,
        d: &'a AttrDecl,
        a: u32,
        value: &str,
    ) -> Result<(), Invalid> {
        let Some(ty) = &d.typ else { return Ok(()) };
        match simple::check(self.set, s, ty, value) {
            Ok(()) => Ok(()),
            Err(why) => fail(
                XML_ATTRIBUTE_VALUE,
                a,
                &[
                    "The attribute '",
                    name_of(self.t, a),
                    "' has an invalid value according to the schema: ",
                    &why,
                ],
            ),
        }
    }

    fn attributes(&mut self, eff: &Effective<'a>, el: u32) -> Result<(), Invalid> {
        let t = self.t;
        let mut decls = Vec::new();
        for &(s, u) in &eff.attrs {
            let (ds, d) = self.attr_decl(s, u, el)?;
            decls.push((ds, d, u));
        }
        for &a in &t.node(el).attrs {
            let an = t.node(a);
            if an.is_xmlns() || an.ns == XSI {
                continue;
            }
            let found =
                decls.iter().find(|(_, d, _)| d.name.ns == an.ns && d.name.local == an.local());
            match found {
                Some(&(_, _, u)) if u.prohibited => {
                    return fail(
                        XML_ATTRIBUTE_NOT_DEFINED,
                        a,
                        &["The attribute '", &an.name, "' is prohibited on this element."],
                    );
                }
                Some(&(ds, d, u)) => {
                    self.attr_value(ds, d, a, &an.value)?;
                    let fixed = u.fixed.as_ref().or(d.fixed.as_ref());
                    if let Some(f) = fixed
                        && simple::check(
                            self.set,
                            ds,
                            d.typ
                                .as_ref()
                                .unwrap_or(&TypeRef::Builtin(simple::Builtin::AnySimpleType)),
                            &an.value,
                        )
                        .is_ok()
                        && super::collapse(f) != super::collapse(&an.value)
                    {
                        return fail(
                            XML_ATTRIBUTE_FIXED,
                            a,
                            &[
                                "The attribute '",
                                &an.name,
                                "' does not have the fixed value the schema gives it.",
                            ],
                        );
                    }
                }
                None => match eff.any_attr {
                    Some(w) if w.allows(&an.ns) => {
                        if w.process != Process::Skip {
                            let q = QName { ns: an.ns.clone(), local: an.local().into() };
                            match self.set.attribute(&q) {
                                Some((ds, d)) => self.attr_value(ds, d, a, &an.value)?,
                                None if w.process == Process::Strict => {
                                    return fail(
                                        XML_ATTRIBUTE_NOT_DEFINED,
                                        a,
                                        &[
                                            "The attribute '",
                                            &an.name,
                                            "' is not declared in the schema.",
                                        ],
                                    );
                                }
                                None => {}
                            }
                        }
                    }
                    _ => {
                        return fail(
                            XML_ATTRIBUTE_NOT_DEFINED,
                            a,
                            &[
                                "The attribute '",
                                &an.name,
                                "' on this element is not defined in the schema.",
                            ],
                        );
                    }
                },
            }
        }
        for (_, d, u) in decls {
            let present = t.node(el).attrs.iter().any(|&a| {
                let an = t.node(a);
                an.ns == d.name.ns && an.local() == d.name.local
            });
            if present {
                continue;
            }
            if u.required {
                return fail(
                    XML_REQUIRED_ATTRIBUTE_MISSING,
                    el,
                    &["Required attribute '", &d.name.local, "' is missing."],
                );
            }
            if u.default.is_some() || d.default.is_some() || u.fixed.is_some() || d.fixed.is_some()
            {
                self.out.defaults += 1;
            }
        }
        Ok(())
    }

    fn content(
        &mut self,
        eff: &Effective<'a>,
        d: &'a ElementDecl,
        el: u32,
        depth: u32,
    ) -> Result<(), Invalid> {
        let t = self.t;
        let kids = self.element_children(el);
        let has_text = t.node(el).children.iter().any(|&k| {
            let n = t.node(k);
            matches!(n.kind, Kind::Text | Kind::CData)
                && !n.value.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
        });
        match &eff.content {
            Content::Simple(s, r) => self.simple_element(s, r, d, el),
            Content::Empty => {
                if let Some(&k) = kids.first() {
                    return fail(
                        XML_INVALID_CONTENT,
                        k,
                        &[
                            "The element '",
                            name_of(t, el),
                            "' must be empty; it holds '",
                            name_of(t, k),
                            "'.",
                        ],
                    );
                }
                if has_text && !eff.mixed {
                    return fail(
                        XML_ILLEGAL_TEXT,
                        el,
                        &[
                            "Text is not allowed in the element '",
                            name_of(t, el),
                            "' according to the schema.",
                        ],
                    );
                }
                Ok(())
            }
            Content::Elements(parts) => {
                if has_text && !eff.mixed {
                    return fail(
                        XML_ILLEGAL_TEXT,
                        el,
                        &[
                            "Text is not allowed in the element '",
                            name_of(t, el),
                            "' according to the schema.",
                        ],
                    );
                }
                let mut m = Matcher {
                    v: self,
                    kids: &kids,
                    assigned: alloc::vec![None; kids.len()],
                    far: 0,
                    missing: None,
                };
                let mut cur = alloc::vec![0usize];
                for &(s, p) in parts {
                    let mut next = Vec::new();
                    for &x in &cur {
                        for e in m.particle(s, p, x) {
                            add(&mut next, e);
                        }
                    }
                    cur = next;
                }
                if let Some(what) = m.missing {
                    return not_done(el, what);
                }
                if !cur.contains(&kids.len()) {
                    let far = m.far;
                    return if far < kids.len() {
                        let k = kids[far];
                        fail(
                            XML_INVALID_CONTENT,
                            k,
                            &[
                                "The element '",
                                name_of(t, k),
                                "' is unexpected according to the content model of the parent element '",
                                name_of(t, el),
                                "'.",
                            ],
                        )
                    } else {
                        fail(
                            XML_ELEMENT_NOT_COMPLETE,
                            el,
                            &[
                                "The content of the element '",
                                name_of(t, el),
                                "' is incomplete according to the schema.",
                            ],
                        )
                    };
                }
                let assigned = m.assigned;
                for (i, &k) in kids.iter().enumerate() {
                    match assigned[i] {
                        Some(Assigned::Decl(s, cd)) => self.element(s, cd, k, depth + 1)?,
                        Some(Assigned::Wild(Process::Skip)) => {}
                        Some(Assigned::Wild(Process::Lax)) => self.lax(k, depth + 1)?,
                        Some(Assigned::Wild(Process::Strict)) => {
                            let n = t.node(k);
                            let q = QName { ns: n.ns.clone(), local: n.local().into() };
                            match self.set.element(&q) {
                                Some((s, cd)) => self.element(s, cd, k, depth + 1)?,
                                None => fail(
                                    XML_ELEMENT_UNDECLARED,
                                    k,
                                    &[
                                        "The element '",
                                        name_of(t, k),
                                        "' is used but not declared in the schema.",
                                    ],
                                )?,
                            }
                        }
                        None => fail(
                            XML_INVALID_CONTENT,
                            k,
                            &["The element '", name_of(t, k), "' is unexpected."],
                        )?,
                    }
                }
                Ok(())
            }
        }
    }

    /// The nodes `path` reaches from `from`.
    fn walk(&self, from: u32, path: &Path, out: &mut Vec<u32>) {
        let t = self.t;
        let mut cur = alloc::vec![from];
        if path.descendants {
            let mut all = Vec::new();
            let mut stack = alloc::vec![from];
            while let Some(x) = stack.pop() {
                all.push(x);
                for &k in t.node(x).children.iter().rev() {
                    if t.node(k).kind == Kind::Element {
                        stack.push(k);
                    }
                }
            }
            cur = all;
        }
        for step in &path.steps {
            let mut next = Vec::new();
            for &x in &cur {
                match step {
                    Step::Current => next.push(x),
                    Step::Child(ns, local) => {
                        for &k in &t.node(x).children {
                            let n = t.node(k);
                            if n.kind == Kind::Element
                                && ns.as_ref().is_none_or(|ns| *ns == n.ns)
                                && local.as_ref().is_none_or(|l| l == n.local())
                            {
                                next.push(k);
                            }
                        }
                    }
                    Step::Attribute(q) => {
                        for &a in &t.node(x).attrs {
                            let n = t.node(a);
                            if !n.is_xmlns() && n.ns == q.ns && n.local() == q.local {
                                next.push(a);
                            }
                        }
                    }
                }
            }
            cur = next;
        }
        out.extend(cur);
    }

    fn identity(&self, d: &ElementDecl, el: u32) -> Result<(), Invalid> {
        for idc in &d.idcs {
            self.one_identity(idc, el)?;
        }
        Ok(())
    }

    fn one_identity(&self, idc: &Idc, el: u32) -> Result<(), Invalid> {
        let t = self.t;
        let mut targets = Vec::new();
        for p in &idc.selector {
            self.walk(el, p, &mut targets);
        }
        let mut seen: Vec<Vec<String>> = Vec::new();
        for &x in &targets {
            let mut tuple = Vec::new();
            let mut complete = true;
            for field in &idc.fields {
                let mut nodes = Vec::new();
                for p in field {
                    self.walk(x, p, &mut nodes);
                }
                match nodes.len() {
                    0 => complete = false,
                    1 => {
                        let n = t.node(nodes[0]);
                        let v = if n.kind == Kind::Attribute {
                            n.value.clone()
                        } else {
                            self.text_of(nodes[0])
                        };
                        tuple.push(super::collapse(&v));
                    }
                    _ => {
                        return fail(
                            XML_INVALID_CONTENT,
                            x,
                            &[
                                "A field of the identity constraint '",
                                &idc.name.local,
                                "' selects more than one node.",
                            ],
                        );
                    }
                }
            }
            if !complete {
                if idc.kind == IdcKind::Key {
                    return fail(
                        XML_INVALID_CONTENT,
                        x,
                        &["The key '", &idc.name.local, "' is missing a field."],
                    );
                }
                continue;
            }
            if seen.contains(&tuple) {
                let what = if idc.kind == IdcKind::Key { "key" } else { "unique constraint" };
                return fail(
                    XML_INVALID_CONTENT,
                    x,
                    &[
                        "A duplicate value of the ",
                        what,
                        " '",
                        &idc.name.local,
                        "': '",
                        &tuple.join("', '"),
                        "'.",
                    ],
                );
            }
            seen.push(tuple);
        }
        Ok(())
    }
}

enum Resolved<'a> {
    AnyType,
    Simple(&'a Schema, &'a TypeRef),
    Complex(&'a Schema, &'a ComplexType),
}

/// Adds `x` to a set kept as a Vec.
fn add(v: &mut Vec<usize>, x: usize) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// Matching a content model over a parent's element children: the set
/// of positions each particle can end at.
struct Matcher<'m, 'a> {
    v: &'m V<'a>,
    kids: &'m [u32],
    assigned: Vec<Option<Assigned<'a>>>,
    /// The furthest child any term matched up to (where an invalid
    /// content goes wrong).
    far: usize,
    /// A reference gala could not resolve (not a pass: the caller fails).
    missing: Option<&'static str>,
}

impl<'a> Matcher<'_, 'a> {
    fn decl(&mut self, s: &'a Schema, et: &'a ElemTerm) -> Option<(&'a Schema, &'a ElementDecl)> {
        match et {
            ElemTerm::Decl(i) => Some((s, &s.elems[*i])),
            ElemTerm::Ref(q) => {
                let found = self.v.set.element(q);
                if found.is_none() {
                    self.missing = Some("an element no schema in the cache declares");
                }
                found
            }
        }
    }

    fn matches_kid(&self, at: usize, ns: &str, local: &str) -> bool {
        self.kids.get(at).is_some_and(|&k| {
            let n = self.v.t.node(k);
            n.ns == ns && n.local() == local
        })
    }

    fn term(&mut self, s: &'a Schema, term: &'a Term, at: usize) -> Vec<usize> {
        match term {
            Term::Element(et) => {
                let Some((s2, d)) = self.decl(s, et) else { return Vec::new() };
                if self.matches_kid(at, &d.name.ns, &d.name.local) {
                    self.assigned[at] = Some(Assigned::Decl(s2, d));
                    self.far = self.far.max(at + 1);
                    alloc::vec![at + 1]
                } else {
                    Vec::new()
                }
            }
            Term::Any(w) => match self.kids.get(at) {
                Some(&k) if w.allows(&self.v.t.node(k).ns) => {
                    self.assigned[at] = Some(Assigned::Wild(w.process));
                    self.far = self.far.max(at + 1);
                    alloc::vec![at + 1]
                }
                _ => Vec::new(),
            },
            Term::Seq(items) => {
                let mut cur = alloc::vec![at];
                for p in items {
                    let mut next = Vec::new();
                    for &x in &cur {
                        for e in self.particle(s, p, x) {
                            add(&mut next, e);
                        }
                    }
                    cur = next;
                    if cur.is_empty() {
                        break;
                    }
                }
                cur
            }
            Term::Choice(items) => {
                let mut out = Vec::new();
                for p in items {
                    for e in self.particle(s, p, at) {
                        add(&mut out, e);
                    }
                }
                out
            }
            Term::All(items) => {
                let mut used = alloc::vec![false; items.len()];
                let mut pos = at;
                'kids: while pos < self.kids.len() {
                    for (i, p) in items.iter().enumerate() {
                        if used[i] {
                            continue;
                        }
                        if let Term::Element(et) = &p.term
                            && let Some((s2, d)) = self.decl(s, et)
                            && self.matches_kid(pos, &d.name.ns, &d.name.local)
                        {
                            used[i] = true;
                            self.assigned[pos] = Some(Assigned::Decl(s2, d));
                            pos += 1;
                            self.far = self.far.max(pos);
                            continue 'kids;
                        }
                    }
                    break;
                }
                if items.iter().zip(&used).all(|(p, &u)| u || p.min == 0) {
                    alloc::vec![pos]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// The ends of `p` (its term repeated min..=max times) from `at`.
    fn particle(&mut self, s: &'a Schema, p: &'a Particle, at: usize) -> Vec<usize> {
        let mut result = Vec::new();
        if p.min == 0 {
            result.push(at);
        }
        let mut frontier = alloc::vec![at];
        let mut count = 0u32;
        while count < p.max && !frontier.is_empty() {
            let mut next = Vec::new();
            for &x in &frontier {
                for e in self.term(s, &p.term, x) {
                    add(&mut next, e);
                }
            }
            count += 1;
            if count >= p.min {
                // Positions already reached add nothing more.
                next.retain(|e| !result.contains(e));
                for &e in &next {
                    result.push(e);
                }
            }
            frontier = next;
        }
        result
    }
}

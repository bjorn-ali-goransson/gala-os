//! The document tree behind the COM objects: an arena of nodes owned by
//! the document, addressed by index (the document node is index 0). Node
//! objects handed to programs are wrappers holding the document and an
//! index, so a node lives as long as its document.

use alloc::string::String;
use alloc::vec::Vec;

use gala_xml::{Content, XMLNS_NS};

/// "No node" (a parent that is not there).
pub const NONE: u32 = u32::MAX;

/// The DOM node types (DOMNodeType's values).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CData = 4,
    Pi = 7,
    Comment = 8,
    Document = 9,
    Fragment = 11,
}

#[derive(Clone)]
pub struct Node {
    pub kind: Kind,
    /// The qualified name (an element's or attribute's), a processing
    /// instruction's target; empty for the others.
    pub name: String,
    /// The namespace URI (elements and attributes; empty for none).
    pub ns: String,
    /// The value: an attribute's, text's, a comment's, a processing
    /// instruction's data.
    pub value: String,
    /// The parent (for an attribute, the element it is on).
    pub parent: u32,
    pub children: Vec<u32>,
    pub attrs: Vec<u32>,
}

impl Node {
    fn new(kind: Kind, name: &str, ns: &str, value: &str, parent: u32) -> Node {
        Node {
            kind,
            name: name.into(),
            ns: ns.into(),
            value: value.into(),
            parent,
            children: Vec::new(),
            attrs: Vec::new(),
        }
    }

    /// The prefix of the qualified name (empty without one).
    pub fn prefix(&self) -> &str {
        match self.kind {
            Kind::Element | Kind::Attribute => self.name.split_once(':').map_or("", |(p, _)| p),
            _ => "",
        }
    }

    /// The local part of the name (MSXML's baseName).
    pub fn local(&self) -> &str {
        match self.kind {
            Kind::Element | Kind::Attribute => {
                self.name.split_once(':').map_or(self.name.as_str(), |(_, l)| l)
            }
            _ => &self.name,
        }
    }

    /// Whether this attribute is a namespace declaration.
    pub fn is_xmlns(&self) -> bool {
        self.kind == Kind::Attribute && self.ns == XMLNS_NS
    }
}

#[derive(Clone)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    /// An empty document.
    pub fn new() -> Tree {
        Tree { nodes: alloc::vec![Node::new(Kind::Document, "", "", "", NONE)] }
    }

    pub fn node(&self, i: u32) -> &Node {
        &self.nodes[i as usize]
    }

    /// A copy of node `i` (not the document) under `parent` (NONE: a
    /// node of no parent, as cloneNode makes): its attributes always, its
    /// children (each copied deep) when `deep`. Returns the copy's index.
    pub fn copy(&mut self, i: u32, deep: bool, parent: u32) -> u32 {
        let n = self.node(i);
        let fresh = Node::new(n.kind, &n.name, &n.ns, &n.value, parent);
        let (attrs, children) =
            (n.attrs.clone(), if deep { n.children.clone() } else { Vec::new() });
        let copy = self.add(fresh);
        for a in attrs {
            self.copy(a, true, copy);
        }
        for c in children {
            self.copy(c, true, copy);
        }
        copy
    }

    /// A copy of node `i` of another tree (`src`), of no parent, made as
    /// `copy` makes one (iteratively: a tree made by programs has no
    /// depth bound). Returns the copy's index.
    pub fn import(&mut self, src: &Tree, i: u32, deep: bool) -> u32 {
        let n = src.node(i);
        let top = self.add(Node::new(n.kind, &n.name, &n.ns, &n.value, NONE));
        // (node of `src`, the parent of its copy)
        let mut stack: Vec<(u32, u32)> = Vec::new();
        let push = |stack: &mut Vec<(u32, u32)>, n: &Node, to: u32, children: bool| {
            if children {
                stack.extend(n.children.iter().rev().map(|&c| (c, to)));
            }
            stack.extend(n.attrs.iter().rev().map(|&a| (a, to)));
        };
        push(&mut stack, n, top, deep);
        while let Some((s, to)) = stack.pop() {
            let n = src.node(s);
            let copy = self.add(Node::new(n.kind, &n.name, &n.ns, &n.value, to));
            push(&mut stack, n, copy, true);
        }
        top
    }

    /// A new node of no parent (the create methods).
    pub fn create(&mut self, kind: Kind, name: &str, ns: &str, value: &str) -> u32 {
        self.add(Node::new(kind, name, ns, value, NONE))
    }

    /// Takes node `i` from its parent's children (or its element's
    /// attributes): a node of no parent after.
    pub fn detach(&mut self, i: u32) {
        let (p, attr) = {
            let n = self.node(i);
            (n.parent, n.kind == Kind::Attribute)
        };
        if p == NONE {
            return;
        }
        let pn = &mut self.nodes[p as usize];
        let list = if attr { &mut pn.attrs } else { &mut pn.children };
        list.retain(|&c| c != i);
        self.nodes[i as usize].parent = NONE;
    }

    /// Puts node `i` (of no parent) under `parent`: an attribute among its
    /// attributes, at the end; another node among its children, before
    /// `before` (one of them) or at the end.
    pub fn insert(&mut self, parent: u32, i: u32, before: Option<u32>) {
        let attr = self.node(i).kind == Kind::Attribute;
        let pn = &mut self.nodes[parent as usize];
        let list = if attr { &mut pn.attrs } else { &mut pn.children };
        let at = before.and_then(|b| list.iter().position(|&c| c == b)).unwrap_or(list.len());
        list.insert(at, i);
        self.nodes[i as usize].parent = parent;
    }

    /// Whether `a` is `b` or one of its ancestors.
    pub fn contains(&self, a: u32, b: u32) -> bool {
        let mut x = b;
        while x != NONE {
            if x == a {
                return true;
            }
            x = self.node(x).parent;
        }
        false
    }

    /// The URI `prefix` is bound to at element `el` (its declarations and
    /// its ancestors'); "" for none.
    pub fn lookup(&self, el: u32, prefix: &str) -> &str {
        let mut x = el;
        while x != NONE {
            let n = self.node(x);
            for &a in &n.attrs {
                let an = self.node(a);
                let declares = if an.name == "xmlns" { "" } else { an.local() };
                if an.is_xmlns() && declares == prefix {
                    return &an.value;
                }
            }
            if n.kind == Kind::Element && n.prefix() == prefix && !n.ns.is_empty() {
                return &n.ns;
            }
            x = n.parent;
        }
        ""
    }

    /// The namespace bindings in scope at element `el` (XPath's namespace
    /// nodes but `xml`): each prefix ("" the default namespace) with the
    /// URI its innermost declaration gives, innermost first; a name's
    /// own prefix and namespace count as a declaration (a node a program
    /// created carries no xmlns attribute); a default namespace undone
    /// (`xmlns=""`) is left out.
    pub fn in_scope(&self, el: u32) -> Vec<(String, String)> {
        let mut seen: Vec<(&str, &str)> = Vec::new();
        fn add<'a>(seen: &mut Vec<(&'a str, &'a str)>, p: &'a str, u: &'a str) {
            if p != "xml" && !seen.iter().any(|(q, _)| *q == p) {
                seen.push((p, u));
            }
        }
        let mut x = el;
        while x != NONE && self.node(x).kind == Kind::Element {
            let n = self.node(x);
            for &a in &n.attrs {
                let an = self.node(a);
                if an.is_xmlns() {
                    let p = if an.name == "xmlns" { "" } else { an.local() };
                    add(&mut seen, p, &an.value);
                }
            }
            if !n.ns.is_empty() {
                add(&mut seen, n.prefix(), &n.ns);
            }
            for &a in &n.attrs {
                let an = self.node(a);
                if !an.is_xmlns() && !an.prefix().is_empty() && !an.ns.is_empty() {
                    add(&mut seen, an.prefix(), &an.ns);
                }
            }
            x = n.parent;
        }
        seen.into_iter().filter(|(_, u)| !u.is_empty()).map(|(p, u)| (p.into(), u.into())).collect()
    }

    /// Joins adjacent text nodes in the subtree of `i` and drops empty
    /// ones (IXMLDOMElement::normalize).
    pub fn normalize(&mut self, i: u32) {
        let mut stack: Vec<u32> = alloc::vec![i];
        while let Some(e) = stack.pop() {
            let kids = self.node(e).children.clone();
            let mut last_text: Option<u32> = None;
            for k in kids {
                match self.node(k).kind {
                    Kind::Text => {
                        if self.node(k).value.is_empty() {
                            self.detach(k);
                        } else if let Some(t) = last_text {
                            let v = core::mem::take(&mut self.nodes[k as usize].value);
                            self.nodes[t as usize].value.push_str(&v);
                            self.detach(k);
                        } else {
                            last_text = Some(k);
                        }
                    }
                    Kind::Element => {
                        stack.push(k);
                        last_text = None;
                    }
                    _ => last_text = None,
                }
            }
        }
    }

    fn add(&mut self, node: Node) -> u32 {
        let i = self.nodes.len() as u32;
        let parent = node.parent;
        let attr = node.kind == Kind::Attribute;
        self.nodes.push(node);
        if parent != NONE {
            let p = &mut self.nodes[parent as usize];
            if attr { p.attrs.push(i) } else { p.children.push(i) }
        }
        i
    }

    /// The tree of a parsed document; white space only text is dropped
    /// unless `keep_space` (MSXML's preserveWhiteSpace).
    pub fn from_document(doc: &gala_xml::Document, keep_space: bool) -> Tree {
        let mut t = Tree::new();
        for c in &doc.prolog {
            t.content(0, c, None, keep_space);
        }
        t.element(0, &doc.root, keep_space);
        for c in &doc.epilog {
            t.content(0, c, None, keep_space);
        }
        t
    }

    fn content(
        &mut self,
        parent: u32,
        c: &Content,
        el: Option<&gala_xml::Element>,
        keep_space: bool,
    ) {
        let node = match c {
            Content::Element(i) => {
                if let Some(child) = el.and_then(|e| e.children.get(*i)) {
                    self.element(parent, child, keep_space);
                }
                return;
            }
            Content::Text(s) => {
                if !keep_space && s.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) {
                    return;
                }
                Node::new(Kind::Text, "", "", s, parent)
            }
            Content::CData(s) => Node::new(Kind::CData, "", "", s, parent),
            Content::Comment(s) => Node::new(Kind::Comment, "", "", s, parent),
            Content::Pi { target, data } => Node::new(Kind::Pi, target, "", data, parent),
        };
        self.add(node);
    }

    /// Adds `el` and everything in it under `parent` (iteratively: the
    /// nesting is bounded by gala_xml's MAX_DEPTH, not by the stack).
    fn element(&mut self, parent: u32, el: &gala_xml::Element, keep_space: bool) {
        // (element, its node, the next content index)
        let mut stack: Vec<(&gala_xml::Element, u32, usize)> = Vec::new();
        let i = self.open(parent, el);
        stack.push((el, i, 0));
        while let Some(top) = stack.last_mut() {
            let (e, node, next) = *top;
            let Some(c) = e.content.get(next) else {
                stack.pop();
                continue;
            };
            top.2 += 1;
            if let Content::Element(k) = c {
                if let Some(child) = e.children.get(*k) {
                    let ci = self.open(node, child);
                    stack.push((child, ci, 0));
                }
            } else {
                self.content(node, c, Some(e), keep_space);
            }
        }
    }

    fn open(&mut self, parent: u32, el: &gala_xml::Element) -> u32 {
        let i = self.add(Node::new(Kind::Element, &el.qname, &el.ns, "", parent));
        for a in &el.attrs {
            self.add(Node::new(Kind::Attribute, &a.qname, &a.ns, &a.value, i));
        }
        i
    }

    /// The document element (the first element child of the document).
    pub fn root(&self) -> Option<u32> {
        self.node(0).children.iter().copied().find(|&c| self.node(c).kind == Kind::Element)
    }

    /// The siblings around `i` among its parent's children (or
    /// attributes): (previous, next).
    pub fn siblings(&self, i: u32) -> (Option<u32>, Option<u32>) {
        let n = self.node(i);
        if n.parent == NONE || n.kind == Kind::Attribute {
            return (None, None);
        }
        let list = &self.node(n.parent).children;
        match list.iter().position(|&c| c == i) {
            Some(k) => (k.checked_sub(1).map(|p| list[p]), list.get(k + 1).copied()),
            None => (None, None),
        }
    }

    /// The text of a node, as the DOM's `text` property and XPath's
    /// string-value give it: an element's or the document's is the text
    /// of every text and CDATA node in it, in order.
    pub fn text(&self, i: u32, out: &mut String) {
        let n = self.node(i);
        match n.kind {
            Kind::Element | Kind::Document | Kind::Fragment => {
                let mut stack: Vec<u32> = n.children.iter().rev().copied().collect();
                while let Some(c) = stack.pop() {
                    let cn = self.node(c);
                    match cn.kind {
                        Kind::Text | Kind::CData => out.push_str(&cn.value),
                        Kind::Element => stack.extend(cn.children.iter().rev()),
                        _ => {}
                    }
                }
            }
            _ => out.push_str(&n.value),
        }
    }

    /// The node in XML (MSXML's `xml` property): the document's
    /// top-level nodes end with CR LF. An element's text is well formed
    /// with its namespaces: a prefix (or the default namespace) that no
    /// declaration in the text binds, because its declaration is on an
    /// ancestor left out, is declared on the element that uses it,
    /// before its attributes (MSXML's output in Wine's tests); the
    /// declarations the nodes carry are written as they are.
    pub fn xml(&self, i: u32, out: &mut String) {
        let n = self.node(i);
        match n.kind {
            Kind::Document => {
                for &c in &n.children {
                    self.xml(c, out);
                    out.push_str("\r\n");
                }
            }
            Kind::Fragment => {
                for &c in &n.children {
                    self.xml(c, out);
                }
            }
            Kind::Element => {
                // The bindings the text written so far has in scope
                // (prefix, URI; "" the default namespace), innermost last.
                let mut scope: Vec<(&str, &str)> = Vec::new();
                // (node, whether its end tag is due, the scope's length
                // before it)
                let mut stack: Vec<(u32, bool, usize)> = Vec::new();
                stack.push((i, false, 0));
                while let Some((e, close, frame)) = stack.pop() {
                    let en = self.node(e);
                    if close {
                        out.push_str("</");
                        out.push_str(&en.name);
                        out.push('>');
                        scope.truncate(frame);
                        continue;
                    }
                    if en.kind != Kind::Element {
                        self.xml(e, out);
                        continue;
                    }
                    let frame = scope.len();
                    for &a in &en.attrs {
                        let an = self.node(a);
                        if an.is_xmlns() {
                            let prefix = if an.name == "xmlns" { "" } else { an.local() };
                            scope.push((prefix, &an.value));
                        }
                    }
                    out.push('<');
                    out.push_str(&en.name);
                    // The element's own name, then its prefixed attributes.
                    let names = core::iter::once(e)
                        .chain(en.attrs.iter().copied().filter(|&a| !self.node(a).is_xmlns()));
                    for (k, x) in names.enumerate() {
                        let xn = self.node(x);
                        let prefix = xn.prefix();
                        let missing = if prefix == "xml" {
                            false
                        } else if prefix.is_empty() {
                            // Unprefixed attributes are in no namespace.
                            k == 0 && bound(&scope, "").unwrap_or("") != xn.ns
                        } else {
                            // A prefix in no namespace (createElement's
                            // "p:name") is written undeclared.
                            !xn.ns.is_empty() && bound(&scope, prefix) != Some(xn.ns.as_str())
                        };
                        if missing {
                            out.push_str(if prefix.is_empty() { " xmlns" } else { " xmlns:" });
                            out.push_str(prefix);
                            out.push_str("=\"");
                            escape(&xn.ns, true, out);
                            out.push('"');
                            scope.push((prefix, &xn.ns));
                        }
                    }
                    for &a in &en.attrs {
                        out.push(' ');
                        self.xml(a, out);
                    }
                    if en.children.is_empty() {
                        out.push_str("/>");
                        scope.truncate(frame);
                    } else {
                        out.push('>');
                        stack.push((e, true, frame));
                        stack.extend(en.children.iter().rev().map(|&c| (c, false, 0)));
                    }
                }
            }
            Kind::Attribute => {
                out.push_str(&n.name);
                out.push_str("=\"");
                escape(&n.value, true, out);
                out.push('"');
            }
            Kind::Text => escape(&n.value, false, out),
            Kind::CData => {
                out.push_str("<![CDATA[");
                out.push_str(&n.value);
                out.push_str("]]>");
            }
            Kind::Comment => {
                out.push_str("<!--");
                out.push_str(&n.value);
                out.push_str("-->");
            }
            Kind::Pi => {
                out.push_str("<?");
                out.push_str(&n.name);
                if !n.value.is_empty() {
                    out.push(' ');
                    out.push_str(&n.value);
                }
                out.push_str("?>");
            }
        }
    }
}

/// The URI `prefix` is bound to in `scope` (innermost last).
fn bound<'a>(scope: &[(&str, &'a str)], prefix: &str) -> Option<&'a str> {
    scope.iter().rev().find(|(p, _)| *p == prefix).map(|&(_, uri)| uri)
}

/// Markup characters as references (in an attribute value, quotes too).
fn escape(s: &str, attr: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' if !attr => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' if !attr => out.push_str("\r\n"),
            _ => out.push(c),
        }
    }
}

//! XSLT 1.0 (W3C, 1999): the subset that Click-to-Run's App-V pieces
//! run before they validate a package manifest (ROADMAP OF), compiled
//! from a stylesheet's tree and run over a document's tree. What
//! XSLTemplate60, IXSLProcessor and IXMLDOMNode::transformNode use
//! (`com/xslt.rs`).
//!
//! Supported: `xsl:stylesheet`/`xsl:transform` with top-level
//! `template`, `param`, `variable` and `output` (method xml, html or
//! text; encoding; omit-xml-declaration); templates by match pattern
//! (location path patterns of child, attribute and `//` steps with
//! predicates, unions; XSLT's default priorities, an explicit priority,
//! the last of equals) and by name, with modes and parameters; the
//! built-in rules; the instructions apply-templates (select, mode,
//! with-param), call-template, for-each, value-of, copy-of, copy, if,
//! choose/when/otherwise, element and attribute (names and namespaces
//! as attribute value templates), text, variable and param (a select or
//! content: a result tree fragment), comment, processing-instruction,
//! message, fallback (skipped); literal result elements with attribute
//! value templates and their namespace declarations (less the XSLT
//! namespace and `exclude-result-prefixes`). XPath is `xpath.rs`'s,
//! with variables, the namespace axis, current() and
//! `msxsl:node-set()`.
//!
//! Not supported (the stylesheet fails to compile, `Failure` with
//! `unsupported`, which the caller records in the ledger): xsl:sort,
//! number, key, import, include, apply-imports, attribute-set and
//! use-attribute-sets, namespace-alias, decimal-format, strip-space,
//! preserve-space, disable-output-escaping, msxsl:script, the
//! simplified stylesheet (a literal result element as the stylesheet),
//! the functions key(), document(), format-number(), generate-id(),
//! system-property(), unparsed-entity-uri() and the *-available()
//! pair, other msxsl: functions, id() and key() patterns. The html
//! output method writes XML's syntax (no declaration).
//!
//! The result is built in a working copy of the source's tree, where
//! result tree fragments also live (so `msxsl:node-set()` gives nodes
//! XPath walks), then taken out into a tree of its own.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::dom::{Kind, NONE, Tree};
use crate::xpath::{self, Env, NsNodes, Pattern, Query, Value, is_ns};

pub const XSLT_NS: &str = "http://www.w3.org/1999/XSL/Transform";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// How deep templates may nest (the stack bounds it: a template
/// instantiated in a template ...).
const MAX_DEPTH: usize = 200;

/// Why a stylesheet did not compile or a transform failed: a feature
/// gala's XSLT does not have (`unsupported`: the ledger's "unsupported")
/// or an error in the stylesheet or while running it.
pub struct Failure {
    pub unsupported: bool,
    pub what: String,
    /// Where: the expression, or the instruction ("" when not known).
    pub at: String,
}

fn error(what: &str) -> Failure {
    Failure { unsupported: false, what: what.to_string(), at: String::new() }
}

fn unsupported(what: &str) -> Failure {
    Failure { unsupported: true, what: what.to_string(), at: String::new() }
}

impl Failure {
    /// The failure placed at `at`, unless it already has a place.
    fn at(mut self, at: &str) -> Failure {
        if self.at.is_empty() {
            self.at = at.into();
        }
        self
    }
}

/// An XPath error in expression `at` (what gala lacks is
/// unsupported).
fn xpath_failure(at: &str, e: xpath::Error) -> Failure {
    Failure { unsupported: e.contains("not supported"), what: e.into(), at: at.into() }
}

/// The output's method (xsl:output's `method`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Method {
    Xml,
    Html,
    Text,
}

/// xsl:output's settings.
pub struct Output {
    /// None: XSLT's default (html when the result's first element is
    /// `html` in no namespace, else xml).
    pub method: Option<Method>,
    pub encoding: Option<String>,
    pub omit_declaration: bool,
}

/// An attribute value template's parts.
enum Avt {
    Lit(String),
    Expr(Query),
}

enum VarValue {
    Select(Query),
    Body(Vec<Instr>),
    /// No select and no content: the empty string.
    Empty,
}

struct Var {
    name: String,
    value: VarValue,
}

enum Instr {
    Text(String),
    Literal {
        qname: String,
        ns: String,
        /// Namespace declarations (prefix, URI) the element brings.
        decls: Vec<(String, String)>,
        attrs: Vec<(String, String, Vec<Avt>)>,
        body: Vec<Instr>,
    },
    ApplyTemplates {
        select: Option<Query>,
        mode: Option<String>,
        params: Vec<Var>,
    },
    CallTemplate {
        name: String,
        params: Vec<Var>,
    },
    ForEach {
        select: Query,
        body: Vec<Instr>,
    },
    ValueOf(Query),
    CopyOf(Query),
    Copy(Vec<Instr>),
    If {
        test: Query,
        body: Vec<Instr>,
    },
    Choose {
        whens: Vec<(Query, Vec<Instr>)>,
        otherwise: Vec<Instr>,
    },
    Element {
        name: Vec<Avt>,
        ns: Option<Vec<Avt>>,
        /// The declarations in scope at the instruction (a prefixed or
        /// unprefixed name's namespace when `ns` is not given).
        scope: Vec<(String, String)>,
        body: Vec<Instr>,
    },
    Attribute {
        name: Vec<Avt>,
        ns: Option<Vec<Avt>>,
        scope: Vec<(String, String)>,
        body: Vec<Instr>,
    },
    Variable(Var),
    Comment(Vec<Instr>),
    Pi {
        name: Vec<Avt>,
        body: Vec<Instr>,
    },
    Message {
        body: Vec<Instr>,
        terminate: bool,
    },
}

struct Template {
    patterns: Vec<(Pattern, f64)>,
    priority: Option<f64>,
    name: Option<String>,
    mode: Option<String>,
    params: Vec<Var>,
    body: Vec<Instr>,
}

/// A compiled stylesheet.
pub struct Sheet {
    templates: Vec<Template>,
    /// Top-level params (true) and variables, in the stylesheet's order.
    globals: Vec<(bool, Var)>,
    pub output: Output,
}

// ---- Compiling ----

/// An unprefixed attribute of element `el`.
fn attr<'t>(t: &'t Tree, el: u32, name: &str) -> Option<&'t str> {
    t.node(el)
        .attrs
        .iter()
        .map(|&a| t.node(a))
        .find(|a| a.ns.is_empty() && a.name == name)
        .map(|a| a.value.as_str())
}

fn is_xsl(t: &Tree, i: u32, local: &str) -> bool {
    let n = t.node(i);
    n.kind == Kind::Element && n.ns == XSLT_NS && n.local() == local
}

struct Compiler<'t> {
    t: &'t Tree,
    /// The namespaces whose declarations literal result elements leave
    /// out (the XSLT namespace, exclude-result-prefixes and
    /// extension-element-prefixes).
    excluded: Vec<String>,
}

/// Compiles the stylesheet at node `top` of `t` (its document, or the
/// `xsl:stylesheet` element).
pub fn compile(t: &Tree, top: u32) -> Result<Sheet, Failure> {
    let el = if t.node(top).kind == Kind::Document {
        t.root().ok_or_else(|| error("a stylesheet document without an element"))?
    } else {
        top
    };
    let n = t.node(el);
    if n.kind != Kind::Element {
        return Err(error("a stylesheet that is not an element"));
    }
    if !(is_xsl(t, el, "stylesheet") || is_xsl(t, el, "transform")) {
        let simplified = n.attrs.iter().any(|&a| {
            let an = t.node(a);
            an.ns == XSLT_NS && an.local() == "version"
        });
        return Err(if simplified {
            unsupported("a simplified stylesheet (a literal result element as the stylesheet)")
        } else {
            error("not an XSLT stylesheet (no xsl:stylesheet or xsl:transform element)")
        });
    }
    let scope = t.in_scope(el);
    let mut c = Compiler { t, excluded: alloc::vec![XSLT_NS.to_string()] };
    for list in ["exclude-result-prefixes", "extension-element-prefixes"] {
        c.exclude(attr(t, el, list), &scope)?;
    }
    let mut sheet = Sheet {
        templates: Vec::new(),
        globals: Vec::new(),
        output: Output { method: None, encoding: None, omit_declaration: false },
    };
    for &k in &n.children {
        let kn = t.node(k);
        match kn.kind {
            Kind::Element if kn.ns == XSLT_NS => {
                c.top_level(k, &mut sheet).map_err(|f| f.at(&kn.name))?
            }
            Kind::Element if kn.ns == xpath::MSXSL_NS && kn.local() == "script" => {
                return Err(unsupported("msxsl:script"));
            }
            // Top-level elements of other namespaces are ignored.
            Kind::Element => {}
            Kind::Text | Kind::CData if !blank(&kn.value) => {
                return Err(error("text at the top level of a stylesheet"));
            }
            _ => {}
        }
    }
    Ok(sheet)
}

fn blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

impl Compiler<'_> {
    /// Adds the namespaces of a prefix list (`#default`: the default
    /// namespace) to those literal result elements leave out.
    fn exclude(&mut self, list: Option<&str>, scope: &[(String, String)]) -> Result<(), Failure> {
        for p in list.unwrap_or("").split_ascii_whitespace() {
            let p = if p == "#default" { "" } else { p };
            match scope.iter().find(|(q, _)| q == p) {
                Some((_, u)) => self.excluded.push(u.clone()),
                None => return Err(error("an excluded prefix that is not declared")),
            }
        }
        Ok(())
    }

    fn top_level(&mut self, k: u32, sheet: &mut Sheet) -> Result<(), Failure> {
        let t = self.t;
        let local = t.node(k).local();
        match local {
            "template" => {
                let scope = t.in_scope(k);
                let patterns = match attr(t, k, "match") {
                    Some(m) => {
                        xpath::compile_pattern(m, &scope).map_err(|e| xpath_failure(m, e))?
                    }
                    None => Vec::new(),
                };
                let name = attr(t, k, "name").map(String::from);
                if patterns.is_empty() && name.is_none() {
                    return Err(error("a template with neither match nor name"));
                }
                let priority = match attr(t, k, "priority") {
                    Some(p) => {
                        Some(p.trim().parse::<f64>().map_err(|_| error("a template's priority"))?)
                    }
                    None => None,
                };
                let (params, body) = self.params_and_body(k)?;
                sheet.templates.push(Template {
                    patterns,
                    priority,
                    name,
                    mode: attr(t, k, "mode").map(String::from),
                    params,
                    body,
                });
            }
            "param" | "variable" => {
                let v = self.var(k)?;
                sheet.globals.push((local == "param", v));
            }
            "output" => {
                let o = &mut sheet.output;
                if let Some(m) = attr(t, k, "method") {
                    o.method = Some(match m {
                        "xml" => Method::Xml,
                        "html" => Method::Html,
                        "text" => Method::Text,
                        _ => {
                            return Err(unsupported("an output method other than xml, html, text"));
                        }
                    });
                }
                if let Some(e) = attr(t, k, "encoding") {
                    o.encoding = Some(e.into());
                }
                if let Some(v) = attr(t, k, "omit-xml-declaration") {
                    o.omit_declaration = v == "yes";
                }
            }
            "import" | "include" | "key" | "attribute-set" | "namespace-alias"
            | "decimal-format" | "strip-space" | "preserve-space" => {
                let mut what = String::from("xsl:");
                what.push_str(local);
                return Err(unsupported(&what));
            }
            _ => return Err(error("an unknown top-level XSLT element")),
        }
        Ok(())
    }

    /// A template's leading `xsl:param`s and the instructions after.
    fn params_and_body(&mut self, el: u32) -> Result<(Vec<Var>, Vec<Instr>), Failure> {
        let t = self.t;
        let mut params = Vec::new();
        let mut rest = Vec::new();
        for &k in &t.node(el).children {
            let n = t.node(k);
            if rest.is_empty() && matches!(n.kind, Kind::Text | Kind::CData) && blank(&n.value) {
                // White space before the params (stripped anyway).
                continue;
            }
            if rest.is_empty() && is_xsl(t, k, "param") {
                params.push(self.var(k)?);
            } else {
                rest.push(k);
            }
        }
        let body = self.instrs(&rest)?;
        Ok((params, body))
    }

    /// A variable or param (also with-param): its name and value.
    fn var(&mut self, el: u32) -> Result<Var, Failure> {
        let t = self.t;
        let name = attr(t, el, "name").ok_or_else(|| error("a variable without a name"))?;
        let value = match attr(t, el, "select") {
            Some(s) => VarValue::Select(self.expr(el, s)?),
            None if t.node(el).children.is_empty() => VarValue::Empty,
            None => VarValue::Body(self.body(el)?),
        };
        Ok(Var { name: name.into(), value })
    }

    fn expr(&self, el: u32, text: &str) -> Result<Query, Failure> {
        let scope = self.t.in_scope(el);
        xpath::compile_xslt(text, &scope).map_err(|e| xpath_failure(text, e))
    }

    fn required_expr(&self, el: u32, name: &str) -> Result<Query, Failure> {
        match attr(self.t, el, name) {
            Some(s) => self.expr(el, s),
            None => Err(error("an instruction without its required expression attribute")),
        }
    }

    fn avt(&self, el: u32, text: &str) -> Result<Vec<Avt>, Failure> {
        let scope = self.t.in_scope(el);
        let mut parts = Vec::new();
        let mut lit = String::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '{' && chars.get(i + 1) == Some(&'{') {
                lit.push('{');
                i += 2;
            } else if c == '}' && chars.get(i + 1) == Some(&'}') {
                lit.push('}');
                i += 2;
            } else if c == '}' {
                return Err(error("an attribute value template with a lone '}'"));
            } else if c == '{' {
                // The expression up to its '}' (outside string literals).
                let mut j = i + 1;
                let mut quote: Option<char> = None;
                while j < chars.len() {
                    match (quote, chars[j]) {
                        (None, '}') => break,
                        (None, q @ ('\'' | '"')) => quote = Some(q),
                        (Some(q), x) if x == q => quote = None,
                        _ => {}
                    }
                    j += 1;
                }
                if j == chars.len() {
                    return Err(error("an attribute value template with an unclosed '{'"));
                }
                if !lit.is_empty() {
                    parts.push(Avt::Lit(core::mem::take(&mut lit)));
                }
                let e: String = chars[i + 1..j].iter().collect();
                let q = xpath::compile_xslt(&e, &scope).map_err(|x| xpath_failure(&e, x))?;
                parts.push(Avt::Expr(q));
                i = j + 1;
            } else {
                lit.push(c);
                i += 1;
            }
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(Avt::Lit(lit));
        }
        Ok(parts)
    }

    fn body(&mut self, el: u32) -> Result<Vec<Instr>, Failure> {
        let kids = self.t.node(el).children.clone();
        self.instrs(&kids)
    }

    fn instrs(&mut self, nodes: &[u32]) -> Result<Vec<Instr>, Failure> {
        let t = self.t;
        let mut out = Vec::new();
        for &k in nodes {
            let n = t.node(k);
            match n.kind {
                Kind::Text | Kind::CData => {
                    // White space only text in a stylesheet is stripped.
                    if !blank(&n.value) {
                        out.push(Instr::Text(n.value.clone()));
                    }
                }
                Kind::Element if n.ns == XSLT_NS => {
                    if let Some(i) = self.instr(k).map_err(|f| f.at(&n.name))? {
                        out.push(i);
                    }
                }
                Kind::Element if n.ns == xpath::MSXSL_NS => {
                    return Err(unsupported("an msxsl: extension element"));
                }
                Kind::Element => out.push(self.literal(k).map_err(|f| f.at(&n.name))?),
                _ => {}
            }
        }
        Ok(out)
    }

    fn literal(&mut self, el: u32) -> Result<Instr, Failure> {
        let t = self.t;
        let n = t.node(el);
        let mark = self.excluded.len();
        let scope = t.in_scope(el);
        if let Some(list) = n
            .attrs
            .iter()
            .map(|&a| t.node(a))
            .find(|a| a.ns == XSLT_NS && matches!(a.local(), "exclude-result-prefixes"))
        {
            let list = list.value.clone();
            self.exclude(Some(&list), &scope)?;
        }
        let mut attrs = Vec::new();
        for &a in &n.attrs {
            let an = t.node(a);
            if an.is_xmlns() {
                continue;
            }
            if an.ns == XSLT_NS {
                match an.local() {
                    "exclude-result-prefixes" | "extension-element-prefixes" | "version" => {
                        continue;
                    }
                    "use-attribute-sets" => return Err(unsupported("xsl:use-attribute-sets")),
                    _ => return Err(error("an unknown xsl: attribute on a literal element")),
                }
            }
            attrs.push((an.name.clone(), an.ns.clone(), self.avt(el, &an.value)?));
        }
        let decls = scope.into_iter().filter(|(_, u)| !self.excluded.contains(u)).collect();
        let body = self.body(el);
        self.excluded.truncate(mark);
        Ok(Instr::Literal { qname: n.name.clone(), ns: n.ns.clone(), decls, attrs, body: body? })
    }

    /// The with-params of an apply-templates or call-template.
    fn with_params(&mut self, el: u32) -> Result<Vec<Var>, Failure> {
        let t = self.t;
        let mut params = Vec::new();
        for &k in &t.node(el).children {
            if is_xsl(t, k, "with-param") {
                params.push(self.var(k)?);
            } else if is_xsl(t, k, "sort") {
                return Err(unsupported("xsl:sort"));
            } else if t.node(k).kind == Kind::Element
                || (matches!(t.node(k).kind, Kind::Text | Kind::CData) && !blank(&t.node(k).value))
            {
                return Err(error("content other than xsl:with-param"));
            }
        }
        Ok(params)
    }

    fn instr(&mut self, el: u32) -> Result<Option<Instr>, Failure> {
        let t = self.t;
        let local = t.node(el).local();
        if attr(t, el, "disable-output-escaping") == Some("yes") {
            return Err(unsupported("disable-output-escaping"));
        }
        Ok(Some(match local {
            "apply-templates" => Instr::ApplyTemplates {
                select: match attr(t, el, "select") {
                    Some(s) => Some(self.expr(el, s)?),
                    None => None,
                },
                mode: attr(t, el, "mode").map(String::from),
                params: self.with_params(el)?,
            },
            "call-template" => Instr::CallTemplate {
                name: attr(t, el, "name")
                    .ok_or_else(|| error("xsl:call-template without a name"))?
                    .into(),
                params: self.with_params(el)?,
            },
            "for-each" => {
                if t.node(el).children.iter().any(|&k| is_xsl(t, k, "sort")) {
                    return Err(unsupported("xsl:sort"));
                }
                Instr::ForEach { select: self.required_expr(el, "select")?, body: self.body(el)? }
            }
            "value-of" => Instr::ValueOf(self.required_expr(el, "select")?),
            "copy-of" => Instr::CopyOf(self.required_expr(el, "select")?),
            "copy" => {
                if attr(t, el, "use-attribute-sets").is_some() {
                    return Err(unsupported("xsl:use-attribute-sets"));
                }
                Instr::Copy(self.body(el)?)
            }
            "if" => Instr::If { test: self.required_expr(el, "test")?, body: self.body(el)? },
            "choose" => {
                let mut whens = Vec::new();
                let mut otherwise = Vec::new();
                for &k in &t.node(el).children {
                    if is_xsl(t, k, "when") {
                        whens.push((self.required_expr(k, "test")?, self.body(k)?));
                    } else if is_xsl(t, k, "otherwise") {
                        otherwise = self.body(k)?;
                    } else if t.node(k).kind == Kind::Element {
                        return Err(error("xsl:choose with content other than when/otherwise"));
                    }
                }
                Instr::Choose { whens, otherwise }
            }
            "element" | "attribute" => {
                if attr(t, el, "use-attribute-sets").is_some() {
                    return Err(unsupported("xsl:use-attribute-sets"));
                }
                let name =
                    attr(t, el, "name").ok_or_else(|| error("xsl:element without a name"))?;
                let name = self.avt(el, name)?;
                let ns = match attr(t, el, "namespace") {
                    Some(n) => Some(self.avt(el, n)?),
                    None => None,
                };
                let scope = t.in_scope(el);
                let body = self.body(el)?;
                if local == "element" {
                    Instr::Element { name, ns, scope, body }
                } else {
                    Instr::Attribute { name, ns, scope, body }
                }
            }
            "text" => {
                let mut s = String::new();
                t.text(el, &mut s);
                Instr::Text(s)
            }
            "variable" | "param" => Instr::Variable(self.var(el)?),
            "comment" => Instr::Comment(self.body(el)?),
            "processing-instruction" => {
                let name = attr(t, el, "name")
                    .ok_or_else(|| error("xsl:processing-instruction without a name"))?;
                Instr::Pi { name: self.avt(el, name)?, body: self.body(el)? }
            }
            "message" => Instr::Message {
                body: self.body(el)?,
                terminate: attr(t, el, "terminate") == Some("yes"),
            },
            // Only for an instruction a processor does not know: gala's
            // run the instruction itself.
            "fallback" => return Ok(None),
            "number" | "apply-imports" | "sort" => {
                let mut what = String::from("xsl:");
                what.push_str(local);
                return Err(unsupported(&what));
            }
            _ => return Err(error("an unknown XSLT instruction")),
        }))
    }
}

// ---- Running ----

#[derive(Clone, Copy)]
struct Ctx {
    node: u32,
    pos: usize,
    size: usize,
}

struct Run<'s> {
    sheet: &'s Sheet,
    /// The source's tree, the result tree fragments and the result.
    w: Tree,
    rank: Vec<u32>,
    ns: RefCell<NsNodes>,
    globals: Vec<(&'s str, Value)>,
    locals: Vec<(&'s str, Value)>,
    /// A variable an evaluation needed and did not find (a global not
    /// evaluated yet).
    missing: RefCell<Option<String>>,
    /// Where output goes: the result's document, a fragment, an element.
    out: u32,
    depth: usize,
    /// Prefixes made up for attributes in a namespace with none.
    made_up: u32,
}

/// The result of a transform: its tree (a document) and the output
/// method that applies.
pub struct Result_ {
    pub tree: Tree,
    pub method: Method,
}

/// Transforms node `start` of `src` with `sheet`, the top-level params
/// given values `params`, starting in `mode`.
pub fn transform(
    sheet: &Sheet,
    src: &Tree,
    start: u32,
    params: &[(String, Value)],
    mode: Option<&str>,
) -> Result<Result_, Failure> {
    let mut w = src.clone();
    // The XML declaration is not a node in XSLT's data model (gala's
    // DOM keeps it as the document's first processing instruction).
    let decls: Vec<u32> = w
        .node(0)
        .children
        .iter()
        .copied()
        .filter(|&c| w.node(c).kind == Kind::Pi && w.node(c).name == "xml")
        .collect();
    for d in decls {
        w.detach(d);
    }
    let rank = xpath::rank_all(&w);
    let mut run = Run {
        sheet,
        w,
        rank,
        ns: RefCell::new(NsNodes::default()),
        globals: Vec::new(),
        locals: Vec::new(),
        missing: RefCell::new(None),
        out: NONE,
        depth: 0,
        made_up: 0,
    };
    let doc = run.w.create(Kind::Document, "", "", "");
    run.out = doc;
    // The root of the start node's tree: the context of the globals.
    let mut root = start;
    while run.w.node(root).parent != NONE {
        root = run.w.node(root).parent;
    }
    run.globals(root, params)?;
    run.apply(&[start], mode, &[])?;
    let mut tree = Tree::new();
    let kids = run.w.node(doc).children.clone();
    for c in kids {
        let i = tree.import(&run.w, c, true);
        tree.insert(0, i, None);
    }
    let method =
        sheet.output.method.unwrap_or_else(|| {
            let first =
                tree.node(0).children.iter().map(|&c| tree.node(c)).find(|c| {
                    c.kind == Kind::Element || (c.kind == Kind::Text && !blank(&c.value))
                });
            match first {
                Some(e) if e.kind == Kind::Element && e.ns.is_empty() => {
                    if e.name.eq_ignore_ascii_case("html") { Method::Html } else { Method::Xml }
                }
                _ => Method::Xml,
            }
        });
    Ok(Result_ { tree, method })
}

impl<'s> Run<'s> {
    fn eval(&self, q: &Query, c: Ctx) -> Result<Value, Failure> {
        *self.missing.borrow_mut() = None;
        let vars = |name: &str| -> Option<Value> {
            let found = self
                .locals
                .iter()
                .rev()
                .find(|(n, _)| *n == name)
                .or_else(|| self.globals.iter().find(|(n, _)| *n == name))
                .map(|(_, v)| v.clone());
            if found.is_none() {
                *self.missing.borrow_mut() = Some(name.into());
            }
            found
        };
        let env = Env {
            tree: &self.w,
            rank: &self.rank,
            ns_nodes: &self.ns,
            vars: &vars,
            current: c.node,
        };
        q.eval_in(&env, c.node, c.pos, c.size).map_err(|e| {
            let mut f = xpath_failure(q.text(), e);
            if let Some(m) = self.missing.borrow().as_ref() {
                f.what.push_str(": $");
                f.what.push_str(m);
            }
            f
        })
    }

    fn string(&self, v: &Value) -> String {
        match v {
            Value::Str(s) => s.clone(),
            Value::Bool(b) => (if *b { "true" } else { "false" }).into(),
            Value::Num(x) => xpath::number_text(*x),
            Value::Nodes(n) => n.first().map_or(String::new(), |&m| self.string_of(m)),
            Value::Rtf(f) => self.string_of(*f),
        }
    }

    /// A namespace node's prefix and URI.
    fn ns_binding(&self, n: u32) -> (String, String) {
        let table = self.ns.borrow();
        let (_, p, u) = table.get(n);
        (p.into(), u.into())
    }

    fn string_of(&self, n: u32) -> String {
        if is_ns(n) {
            return self.ns.borrow().get(n).2.into();
        }
        let mut s = String::new();
        self.w.text(n, &mut s);
        s
    }

    fn boolean(&self, v: &Value) -> bool {
        match v {
            Value::Nodes(n) => !n.is_empty(),
            Value::Rtf(_) => true,
            Value::Str(s) => !s.is_empty(),
            Value::Num(x) => *x != 0.0 && !x.is_nan(),
            Value::Bool(b) => *b,
        }
    }

    fn nodes(&self, q: &Query, c: Ctx) -> Result<Vec<u32>, Failure> {
        match self.eval(q, c)? {
            Value::Nodes(n) => Ok(n),
            Value::Rtf(_) => Err(error(
                "a result tree fragment where a node-set is needed (msxsl:node-set() converts it)",
            )),
            _ => Err(error("an expression that is not a node-set where one is needed")),
        }
    }

    /// Evaluates the top-level params and variables (a param given a
    /// value takes it). A global may use one later in the stylesheet:
    /// one that needs a global not evaluated yet waits for it; a cycle
    /// is an error.
    fn globals(&mut self, root: u32, params: &[(String, Value)]) -> Result<(), Failure> {
        let sheet = self.sheet;
        let mut pending: Vec<&'s (bool, Var)> = sheet.globals.iter().collect();
        while !pending.is_empty() {
            let mut waiting = Vec::new();
            let before = pending.len();
            for g in pending {
                let (is_param, v) = g;
                let given = params.iter().find(|(n, _)| *n == v.name).map(|(_, x)| x.clone());
                let value = match given {
                    Some(x) if *is_param => Ok(x),
                    _ => {
                        *self.missing.borrow_mut() = None;
                        self.var_value(&v.value, Ctx { node: root, pos: 1, size: 1 })
                    }
                };
                match value {
                    Ok(x) => self.globals.push((&v.name, x)),
                    Err(e) => {
                        let missing = self.missing.borrow_mut().take();
                        let later = missing.is_some_and(|m| {
                            sheet.globals.iter().any(|(_, o)| o.name == m)
                                && !self.globals.iter().any(|(n, _)| *n == m)
                        });
                        if !later {
                            return Err(e);
                        }
                        waiting.push(g);
                    }
                }
            }
            if waiting.len() == before {
                return Err(error("top-level variables that refer to each other in a cycle"));
            }
            pending = waiting;
        }
        Ok(())
    }

    fn var_value(&mut self, v: &'s VarValue, c: Ctx) -> Result<Value, Failure> {
        match v {
            VarValue::Select(q) => self.eval(q, c),
            VarValue::Empty => Ok(Value::Str(String::new())),
            VarValue::Body(body) => {
                let frag = self.w.create(Kind::Fragment, "", "", "");
                let saved = self.out;
                self.out = frag;
                let r = self.exec(body, c);
                self.out = saved;
                r.map(|()| Value::Rtf(frag))
            }
        }
    }

    /// The text a content template makes (an attribute's value, a
    /// comment, a message): its text nodes', in order.
    fn content_text(&mut self, body: &'s [Instr], c: Ctx) -> Result<String, Failure> {
        let frag = self.w.create(Kind::Fragment, "", "", "");
        let saved = self.out;
        self.out = frag;
        let r = self.exec(body, c);
        self.out = saved;
        r?;
        let mut s = String::new();
        self.w.text(frag, &mut s);
        Ok(s)
    }

    fn with_params(&mut self, params: &'s [Var], c: Ctx) -> Result<Vec<(&'s str, Value)>, Failure> {
        let mut out = Vec::new();
        for p in params {
            let v = self.var_value(&p.value, c)?;
            out.push((p.name.as_str(), v));
        }
        Ok(out)
    }

    /// The best template for node `n` in `mode` (the highest priority;
    /// of equals, the last).
    fn find(&self, n: u32, mode: Option<&str>) -> Result<Option<&'s Template>, Failure> {
        if is_ns(n) {
            return Ok(None);
        }
        let vars = |_: &str| -> Option<Value> { None };
        let env =
            Env { tree: &self.w, rank: &self.rank, ns_nodes: &self.ns, vars: &vars, current: n };
        let sheet: &'s Sheet = self.sheet;
        let mut best: Option<(f64, &'s Template)> = None;
        for t in &sheet.templates {
            if t.mode.as_deref() != mode {
                continue;
            }
            for (p, default) in &t.patterns {
                let priority = t.priority.unwrap_or(*default);
                if best.is_some_and(|(b, _)| priority < b) {
                    continue;
                }
                if p.matches(&env, n).map_err(|e| xpath_failure("a match pattern", e))? {
                    best = Some((priority, t));
                }
            }
        }
        Ok(best.map(|(_, t)| t))
    }

    fn apply(
        &mut self,
        nodes: &[u32],
        mode: Option<&str>,
        params: &[(&'s str, Value)],
    ) -> Result<(), Failure> {
        let size = nodes.len();
        for (i, &n) in nodes.iter().enumerate() {
            let c = Ctx { node: n, pos: i + 1, size };
            match self.find(n, mode)? {
                Some(t) => self.instantiate(t, c, params)?,
                None => self.built_in(c, mode)?,
            }
        }
        Ok(())
    }

    /// XSLT's built-in rules: the root and elements apply templates to
    /// their children (in the same mode), text and attributes are
    /// copied as text, the rest gives nothing.
    fn built_in(&mut self, c: Ctx, mode: Option<&str>) -> Result<(), Failure> {
        if is_ns(c.node) {
            return Ok(());
        }
        let n = self.w.node(c.node);
        match n.kind {
            Kind::Document | Kind::Fragment | Kind::Element => {
                let kids = n.children.clone();
                self.apply(&kids, mode, &[])
            }
            Kind::Text | Kind::CData | Kind::Attribute => {
                let v = n.value.clone();
                self.text(&v);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn instantiate(
        &mut self,
        t: &'s Template,
        c: Ctx,
        params: &[(&'s str, Value)],
    ) -> Result<(), Failure> {
        if self.depth >= MAX_DEPTH {
            return Err(error("templates nested deeper than gala's bound (200)"));
        }
        self.depth += 1;
        let saved = core::mem::take(&mut self.locals);
        let mut r = Ok(());
        for p in &t.params {
            let v = match params.iter().find(|(n, _)| *n == p.name) {
                Some((_, v)) => Ok(v.clone()),
                None => self.var_value(&p.value, c),
            };
            match v {
                Ok(v) => self.locals.push((&p.name, v)),
                Err(e) => {
                    r = Err(e);
                    break;
                }
            }
        }
        if r.is_ok() {
            r = self.exec(&t.body, c);
        }
        self.locals = saved;
        self.depth -= 1;
        r
    }

    fn exec(&mut self, body: &'s [Instr], c: Ctx) -> Result<(), Failure> {
        let mark = self.locals.len();
        let mut r = Ok(());
        for i in body {
            r = self.instr(i, c);
            if r.is_err() {
                break;
            }
        }
        self.locals.truncate(mark);
        r
    }

    fn avt(&self, parts: &[Avt], c: Ctx) -> Result<String, Failure> {
        let mut s = String::new();
        for p in parts {
            match p {
                Avt::Lit(l) => s.push_str(l),
                Avt::Expr(q) => {
                    let v = self.eval(q, c)?;
                    s.push_str(&self.string(&v));
                }
            }
        }
        Ok(s)
    }

    fn instr(&mut self, i: &'s Instr, c: Ctx) -> Result<(), Failure> {
        match i {
            Instr::Text(s) => self.text(s),
            Instr::Literal { qname, ns, decls, attrs, body } => {
                let e = self.element(qname, ns);
                for (p, u) in decls {
                    self.declare(e, p, u);
                }
                for (name, ans, avt) in attrs {
                    let v = self.avt(avt, c)?;
                    self.set_attr(e, name, ans, &v);
                }
                return self.inside(e, body, c);
            }
            Instr::ApplyTemplates { select, mode, params } => {
                let nodes = match select {
                    Some(q) => self.nodes(q, c)?,
                    None if is_ns(c.node) => Vec::new(),
                    None => match self.w.node(c.node).kind {
                        Kind::Attribute => Vec::new(),
                        _ => self.w.node(c.node).children.clone(),
                    },
                };
                let params = self.with_params(params, c)?;
                return self.apply(&nodes, mode.as_deref(), &params);
            }
            Instr::CallTemplate { name, params } => {
                let sheet = self.sheet;
                let Some(t) = sheet.templates.iter().rev().find(|t| t.name.as_ref() == Some(name))
                else {
                    return Err(error("xsl:call-template of a template that is not there").at(name));
                };
                let params = self.with_params(params, c)?;
                return self.instantiate(t, c, &params);
            }
            Instr::ForEach { select, body } => {
                let nodes = self.nodes(select, c)?;
                let size = nodes.len();
                for (k, &n) in nodes.iter().enumerate() {
                    self.exec(body, Ctx { node: n, pos: k + 1, size })?;
                }
            }
            Instr::ValueOf(q) => {
                let v = self.eval(q, c)?;
                let s = self.string(&v);
                self.text(&s);
            }
            Instr::CopyOf(q) => match self.eval(q, c)? {
                Value::Nodes(nodes) => {
                    for n in nodes {
                        self.copy_of(n);
                    }
                }
                Value::Rtf(f) => {
                    let kids = self.w.node(f).children.clone();
                    for k in kids {
                        self.copy_of(k);
                    }
                }
                v => {
                    let s = self.string(&v);
                    self.text(&s);
                }
            },
            Instr::Copy(body) => {
                let n = c.node;
                if is_ns(n) {
                    let (p, u) = self.ns_binding(n);
                    let out = self.out;
                    self.declare(out, &p, &u);
                    return Ok(());
                }
                let node = self.w.node(n);
                match node.kind {
                    Kind::Element => {
                        let (name, ns) = (node.name.clone(), node.ns.clone());
                        let e = self.element(&name, &ns);
                        for (p, u) in self.w.in_scope(n) {
                            self.declare(e, &p, &u);
                        }
                        return self.inside(e, body, c);
                    }
                    Kind::Document | Kind::Fragment => return self.exec(body, c),
                    _ => self.copy_of(n),
                }
            }
            Instr::If { test, body } => {
                let v = self.eval(test, c)?;
                if self.boolean(&v) {
                    return self.exec(body, c);
                }
            }
            Instr::Choose { whens, otherwise } => {
                for (test, body) in whens {
                    let v = self.eval(test, c)?;
                    if self.boolean(&v) {
                        return self.exec(body, c);
                    }
                }
                return self.exec(otherwise, c);
            }
            Instr::Element { name, ns, scope, body } => {
                let name = self.avt(name, c)?;
                if !qname_ok(&name) {
                    return Err(error("xsl:element with a name that is not a QName"));
                }
                let (prefix, local) = name.split_once(':').unwrap_or(("", &name));
                let qname = match ns {
                    Some(ns) => {
                        let uri = self.avt(ns, c)?;
                        let q = if prefix.is_empty() {
                            self.element_name(local, &uri)
                        } else {
                            name.clone()
                        };
                        (q, uri)
                    }
                    None => match scope.iter().find(|(p, _)| p == prefix) {
                        Some((_, u)) => (name.clone(), u.clone()),
                        None if prefix.is_empty() => (name.clone(), String::new()),
                        None => {
                            return Err(error("xsl:element with an undeclared prefix"));
                        }
                    },
                };
                let e = self.element(&qname.0, &qname.1);
                return self.inside(e, body, c);
            }
            Instr::Attribute { name, ns, scope, body } => {
                let name = self.avt(name, c)?;
                if !qname_ok(&name) || name == "xmlns" {
                    return Err(error("xsl:attribute with a name that is not an attribute's"));
                }
                let prefix = name.split_once(':').map_or("", |(p, _)| p);
                let uri = match ns {
                    Some(ns) => self.avt(ns, c)?,
                    None if prefix.is_empty() => String::new(),
                    None => match scope.iter().find(|(p, _)| p == prefix) {
                        Some((_, u)) => u.clone(),
                        None => return Err(error("xsl:attribute with an undeclared prefix")),
                    },
                };
                // An attribute in no namespace has no prefix.
                let name = if uri.is_empty() {
                    String::from(name.split_once(':').map_or(name.as_str(), |(_, l)| l))
                } else {
                    name.clone()
                };
                let value = self.content_text(body, c)?;
                let out = self.out;
                self.set_attr(out, &name, &uri, &value);
            }
            Instr::Variable(v) => {
                let value = self.var_value(&v.value, c)?;
                self.locals.push((&v.name, value));
            }
            Instr::Comment(body) => {
                let s = self.content_text(body, c)?;
                let k = self.w.create(Kind::Comment, "", "", &s);
                self.w.insert(self.out, k, None);
            }
            Instr::Pi { name, body } => {
                let name = self.avt(name, c)?;
                let s = self.content_text(body, c)?;
                let k = self.w.create(Kind::Pi, &name, "", &s);
                self.w.insert(self.out, k, None);
            }
            Instr::Message { body, terminate } => {
                let s = self.content_text(body, c)?;
                let mut what = String::from("XSLT message: ");
                what.push_str(&s);
                crate::record(what.as_bytes(), crate::LEDGER_OTHER, u64::from(*terminate));
                if *terminate {
                    return Err(error("xsl:message terminate=\"yes\""));
                }
            }
        }
        Ok(())
    }

    /// Runs `body` with output going into element `e`.
    fn inside(&mut self, e: u32, body: &'s [Instr], c: Ctx) -> Result<(), Failure> {
        let saved = self.out;
        self.out = e;
        let r = self.exec(body, c);
        self.out = saved;
        r
    }

    /// Text into the output (joined with the text before it).
    fn text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let out = self.out;
        if let Some(&last) = self.w.node(out).children.last()
            && self.w.node(last).kind == Kind::Text
        {
            self.w.nodes[last as usize].value.push_str(s);
            return;
        }
        let k = self.w.create(Kind::Text, "", "", s);
        self.w.insert(out, k, None);
    }

    /// A new element in the output, its namespace declared when the
    /// output does not bind its prefix so.
    fn element(&mut self, qname: &str, ns: &str) -> u32 {
        let out = self.out;
        let e = self.w.create(Kind::Element, qname, ns, "");
        self.w.insert(out, e, None);
        let prefix = qname.split_once(':').map_or("", |(p, _)| p);
        if prefix != "xml" && self.w.lookup(out, prefix) != ns {
            self.add_decl(e, prefix, ns);
        }
        e
    }

    /// The name an element `local` in namespace `uri` gets (xsl:element
    /// with a namespace and an unprefixed name): unprefixed when the
    /// default namespace in scope is `uri` or no prefix is bound to it,
    /// else the prefix bound to it (App-V's normalization relies on this
    /// to give its elements a prefix it declared).
    fn element_name(&self, local: &str, uri: &str) -> String {
        let out = self.out;
        if uri.is_empty() || self.w.lookup(out, "") == uri {
            return local.into();
        }
        let scope =
            if self.w.node(out).kind == Kind::Element { self.w.in_scope(out) } else { Vec::new() };
        match scope.iter().find(|(p, u)| !p.is_empty() && u == uri) {
            Some((p, _)) => {
                let mut q = p.clone();
                q.push(':');
                q.push_str(local);
                q
            }
            None => local.into(),
        }
    }

    fn add_decl(&mut self, e: u32, prefix: &str, uri: &str) {
        let name = if prefix.is_empty() {
            String::from("xmlns")
        } else {
            let mut n = String::from("xmlns:");
            n.push_str(prefix);
            n
        };
        let a = self.w.create(Kind::Attribute, &name, XMLNS_NS, uri);
        self.w.insert(e, a, None);
    }

    /// Gives element `e` the namespace node (prefix, uri) (a copied
    /// namespace node): declared unless `e` already binds the prefix so
    /// (or binds it otherwise itself: its own name wins).
    fn declare(&mut self, e: u32, prefix: &str, uri: &str) {
        if prefix == "xml" || self.w.node(e).kind != Kind::Element {
            return;
        }
        let n = self.w.node(e);
        let own = n.attrs.iter().any(|&a| {
            let an = self.w.node(a);
            an.is_xmlns() && (if an.name == "xmlns" { "" } else { an.local() }) == prefix
        }) || (n.prefix() == prefix && !n.ns.is_empty());
        if own || self.w.lookup(e, prefix) == uri {
            return;
        }
        self.add_decl(e, prefix, uri);
    }

    /// Sets attribute `qname` in namespace `ns` on element `e`: its
    /// prefix declared (one made up when the name has none or its
    /// prefix is bound to another namespace there), replacing one of the
    /// same expanded name. On anything but an element with no children
    /// yet it is dropped (XSLT's recovery), and recorded.
    fn set_attr(&mut self, e: u32, qname: &str, ns: &str, value: &str) {
        let n = self.w.node(e);
        if n.kind != Kind::Element || !n.children.is_empty() {
            crate::record(
                b"XSLT: an attribute not on a new element (dropped)",
                crate::LEDGER_OTHER,
                0,
            );
            return;
        }
        let (prefix, local) = qname.split_once(':').unwrap_or(("", qname));
        let mut name = String::from(qname);
        if !ns.is_empty() && prefix != "xml" {
            let bound = !prefix.is_empty() && self.w.lookup(e, prefix) == ns;
            if !bound {
                let free = !prefix.is_empty()
                    && n.prefix() != prefix
                    && !n.attrs.iter().any(|&a| {
                        let an = self.w.node(a);
                        an.is_xmlns() && an.local() == prefix
                    });
                let p = if free {
                    String::from(prefix)
                } else {
                    // A prefix bound to the namespace already, else a new one.
                    match self.w.in_scope(e).into_iter().find(|(p, u)| !p.is_empty() && u == ns) {
                        Some((p, _)) => p,
                        None => self.made_up_prefix(e),
                    }
                };
                if self.w.lookup(e, &p) != ns {
                    self.add_decl(e, &p, ns);
                }
                name = p;
                name.push(':');
                name.push_str(local);
            }
        }
        let same = self.w.node(e).attrs.iter().copied().find(|&a| {
            let an = self.w.node(a);
            !an.is_xmlns() && an.ns == ns && an.local() == local
        });
        if let Some(a) = same {
            self.w.detach(a);
        }
        let a = self.w.create(Kind::Attribute, &name, ns, value);
        self.w.insert(e, a, None);
    }

    fn made_up_prefix(&mut self, e: u32) -> String {
        loop {
            let mut p = String::from("ns");
            p.push_str(&self.made_up.to_string());
            self.made_up += 1;
            if self.w.lookup(e, &p).is_empty() {
                return p;
            }
        }
    }

    /// copy-of of node `n`: a copy of it and everything in it into the
    /// output (an element with the namespaces in scope at it; an
    /// attribute or namespace node onto the output element).
    fn copy_of(&mut self, n: u32) {
        let out = self.out;
        if is_ns(n) {
            let (p, u) = self.ns_binding(n);
            self.declare(out, &p, &u);
            return;
        }
        let node = self.w.node(n);
        match node.kind {
            Kind::Element => {
                let copy = self.w.copy(n, true, NONE);
                self.w.insert(out, copy, None);
                for (p, u) in self.w.in_scope(n) {
                    self.declare(copy, &p, &u);
                }
            }
            Kind::Attribute => {
                if !node.is_xmlns() {
                    let (name, ns, value) =
                        (node.name.clone(), node.ns.clone(), node.value.clone());
                    self.set_attr(out, &name, &ns, &value);
                }
            }
            Kind::Text | Kind::CData => {
                let v = node.value.clone();
                self.text(&v);
            }
            Kind::Comment | Kind::Pi => {
                let copy = self.w.copy(n, false, NONE);
                self.w.insert(out, copy, None);
            }
            Kind::Document | Kind::Fragment => {
                for k in node.children.clone() {
                    self.copy_of(k);
                }
            }
        }
    }
}

/// Whether `s` is a QName (at most one colon, not at either end; XML
/// name characters).
fn qname_ok(s: &str) -> bool {
    let start = |c: char| c.is_alphabetic() || c == '_';
    let rest = |c: char| start(c) || c.is_ascii_digit() || matches!(c, '-' | '.') || !c.is_ascii();
    let mut parts = s.split(':');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| p.chars().next().is_some_and(start) && p.chars().all(rest))
    };
    match (parts.next(), parts.next(), parts.next()) {
        (a, None, None) => ok(a),
        (a, b @ Some(_), None) => ok(a) && ok(b),
        _ => false,
    }
}

/// The result as text: the XML declaration (unless omitted, or the
/// method is html or text) naming `encoding`, then the result's nodes in
/// XML's syntax (the html method too); the text method gives the text
/// alone.
pub fn serialize(sheet: &Sheet, r: &Result_, encoding: &str) -> String {
    let mut s = String::new();
    let t = &r.tree;
    if r.method == Method::Text {
        t.text(0, &mut s);
        return s;
    }
    if r.method == Method::Xml && !sheet.output.omit_declaration {
        s.push_str("<?xml version=\"1.0\" encoding=\"");
        s.push_str(encoding);
        s.push_str("\"?>");
    }
    for &c in &t.node(0).children {
        t.xml(c, &mut s);
    }
    s
}

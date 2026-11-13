//! XPath 1.0 (W3C, 1999) over the document tree: what selectNodes and
//! selectSingleNode evaluate (MSXML 6's only selection language), and
//! the expressions and patterns of XSLT (`xslt.rs`).
//!
//! The whole expression language: location paths on every axis,
//! node tests by name, `*`, `prefix:*`, node(), text(), comment(),
//! processing-instruction(); predicates; unions; the boolean, comparison
//! and arithmetic operators with XPath's conversions; the core function
//! library but `id()` and `lang()` (refused). Prefixes come from the
//! SelectionNamespaces property only (an undeclared one is an error, as
//! in MSXML 6); an unprefixed name is in no namespace. The DOM's
//! selections have no variables (MSXML has none) and refuse the
//! namespace axis (recorded); XSLT's expressions (`compile_xslt`) have
//! both, the functions current() and `msxsl:node-set()`, and result tree
//! fragments as values. Namespace nodes are not in the tree: they are
//! numbered on demand in an `NsNodes` table (ids with `NS_BIT` set).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::dom::{Kind, NONE, Tree};

/// The namespace of MSXML's XSLT extension functions (`msxsl:`).
pub const MSXSL_NS: &str = "urn:schemas-microsoft-com:xslt";
/// The `xml` prefix's namespace (every element has its namespace node).
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// The bit that marks a namespace node's id (tree indices stay below it).
pub const NS_BIT: u32 = 0x8000_0000;

/// The namespace nodes met so far: (element, prefix, URI), numbered by
/// their place here (an id is `NS_BIT | place`); one id per element and
/// prefix, so a node met twice is the same node.
#[derive(Default)]
pub struct NsNodes {
    list: Vec<(u32, String, String)>,
    ids: BTreeMap<(u32, String), u32>,
}

impl NsNodes {
    /// The namespace node `id`: its element, prefix and URI.
    pub fn get(&self, id: u32) -> (u32, &str, &str) {
        let (e, p, u) = &self.list[(id & !NS_BIT) as usize];
        (*e, p, u)
    }

    /// The namespace nodes of element `el` (its in-scope bindings, `xml`
    /// last), numbered.
    fn of(&mut self, t: &Tree, el: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut scope = t.in_scope(el);
        scope.push((String::from("xml"), String::from(XML_NS)));
        for (prefix, uri) in scope {
            let key = (el, prefix);
            let id = match self.ids.get(&key) {
                Some(&id) => id,
                None => {
                    let id = NS_BIT | self.list.len() as u32;
                    self.list.push((el, key.1.clone(), uri));
                    self.ids.insert(key, id);
                    id
                }
            };
            out.push(id);
        }
        out
    }
}

/// Whether `id` names a namespace node.
pub fn is_ns(id: u32) -> bool {
    id & NS_BIT != 0
}

pub type Error = &'static str;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Following,
    Preceding,
    Attribute,
    Namespace,
    Current,
}

#[derive(Debug)]
enum Test {
    /// A name: (namespace URI or None for `*`, local name or None for
    /// `*`).
    Name(Option<String>, Option<String>),
    Node,
    Text,
    Comment,
    Pi(Option<String>),
}

#[derive(Debug)]
struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<Expr>,
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Union,
}

#[derive(Debug)]
enum Expr {
    Binary(Op, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Lit(String),
    Num(f64),
    Call(String, Vec<Expr>),
    /// A location path: absolute (from the root) or not, its steps.
    Path(bool, Vec<Step>),
    /// A filter expression, its predicates, then a relative path.
    Filter(Box<Expr>, Vec<Expr>, Vec<Step>),
    /// A variable reference (XSLT's).
    Var(String),
}

/// A value: a node-set (in document order), a string, a number, a
/// boolean, or (XSLT) a result tree fragment: the root of a fragment
/// in the tree, which only `msxsl:node-set()` makes a node-set of.
#[derive(Clone)]
pub enum Value {
    Nodes(Vec<u32>),
    Str(String),
    Num(f64),
    Bool(bool),
    Rtf(u32),
}

// ---- Tokens ----

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Slash,
    DSlash,
    Dot,
    DDot,
    At,
    Comma,
    LParen,
    RParen,
    LBr,
    RBr,
    Pipe,
    Plus,
    Minus,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `*` as a name test.
    Star,
    /// `*`, `and`, `or`, `div`, `mod` as operators.
    OpName(&'static str),
    Lit(String),
    Num(f64),
    /// A QName or `prefix:*` name test.
    Name(String),
    /// `name::`.
    Axis(String),
    /// A name before `(`: a function or a node type.
    Func(String),
    /// `$name`.
    Var(String),
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

fn lex(s: &str) -> Result<Vec<Tok>, Error> {
    let b: Vec<char> = s.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Whether the previous token ends an operand: then `*` and the
        // operator names are operators (XPath 3.7).
        let after_operand = matches!(
            out.last(),
            Some(
                Tok::RParen
                    | Tok::RBr
                    | Tok::Lit(_)
                    | Tok::Num(_)
                    | Tok::Name(_)
                    | Tok::Star
                    | Tok::Dot
                    | Tok::DDot
                    | Tok::Var(_)
            )
        );
        let next = b.get(i + 1).copied();
        let tok = match c {
            '/' if next == Some('/') => {
                i += 1;
                Tok::DSlash
            }
            '/' => Tok::Slash,
            '.' if next == Some('.') => {
                i += 1;
                Tok::DDot
            }
            '.' if !next.is_some_and(|n| n.is_ascii_digit()) => Tok::Dot,
            '@' => Tok::At,
            '$' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && (is_name_char(b[j]) || b[j] == ':') {
                    j += 1;
                }
                if j == start || !is_name_start(b[start]) {
                    return Err("a variable without a name");
                }
                out.push(Tok::Var(b[start..j].iter().collect()));
                i = j;
                continue;
            }
            ',' => Tok::Comma,
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBr,
            ']' => Tok::RBr,
            '|' => Tok::Pipe,
            '+' => Tok::Plus,
            '-' => Tok::Minus,
            '=' => Tok::Eq,
            '!' if next == Some('=') => {
                i += 1;
                Tok::Ne
            }
            '<' if next == Some('=') => {
                i += 1;
                Tok::Le
            }
            '<' => Tok::Lt,
            '>' if next == Some('=') => {
                i += 1;
                Tok::Ge
            }
            '>' => Tok::Gt,
            '*' if after_operand => Tok::OpName("*"),
            '*' => Tok::Star,
            '"' | '\'' => {
                let start = i + 1;
                let end = b[start..].iter().position(|&x| x == c).ok_or("an unclosed literal")?;
                i = start + end;
                Tok::Lit(b[start..start + end].iter().collect())
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                    i += 1;
                }
                let text: String = b[start..i].iter().collect();
                out.push(Tok::Num(text.parse::<f64>().map_err(|_| "a bad number")?));
                continue;
            }
            _ if is_name_start(c) => {
                let start = i;
                while i < b.len() && is_name_char(b[i]) {
                    i += 1;
                }
                let mut name: String = b[start..i].iter().collect();
                if after_operand && matches!(name.as_str(), "and" | "or" | "div" | "mod") {
                    out.push(Tok::OpName(match name.as_str() {
                        "and" => "and",
                        "or" => "or",
                        "div" => "div",
                        _ => "mod",
                    }));
                    continue;
                }
                if b.get(i) == Some(&':') && b.get(i + 1) == Some(&':') {
                    out.push(Tok::Axis(name));
                    i += 2;
                    continue;
                }
                if b.get(i) == Some(&':') {
                    if b.get(i + 1) == Some(&'*') {
                        name.push_str(":*");
                        i += 2;
                    } else if b.get(i + 1).is_some_and(|&x| is_name_start(x)) {
                        name.push(':');
                        i += 1;
                        while i < b.len() && is_name_char(b[i]) {
                            name.push(b[i]);
                            i += 1;
                        }
                    }
                }
                let mut j = i;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if b.get(j) == Some(&'(') {
                    out.push(Tok::Func(name));
                } else {
                    out.push(Tok::Name(name));
                }
                continue;
            }
            _ => return Err("an unexpected character"),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

// ---- Parsing ----

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    /// (prefix, namespace URI) from SelectionNamespaces (XSLT: the
    /// stylesheet's declarations in scope).
    ns: &'a [(String, String)],
    /// An XSLT expression: variables, the namespace axis, XSLT's and
    /// MSXML's extension functions.
    xslt: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok) -> Result<(), Error> {
        if self.eat(t) { Ok(()) } else { Err("a token expected") }
    }

    fn binary(
        &mut self,
        next: fn(&mut Self) -> Result<Expr, Error>,
        ops: &[(Tok, Op)],
    ) -> Result<Expr, Error> {
        let mut left = next(self)?;
        'outer: loop {
            for (t, op) in ops {
                if self.eat(t) {
                    let right = next(self)?;
                    left = Expr::Binary(*op, Box::new(left), Box::new(right));
                    continue 'outer;
                }
            }
            return Ok(left);
        }
    }

    fn or(&mut self) -> Result<Expr, Error> {
        self.binary(Self::and, &[(Tok::OpName("or"), Op::Or)])
    }

    fn and(&mut self) -> Result<Expr, Error> {
        self.binary(Self::equality, &[(Tok::OpName("and"), Op::And)])
    }

    fn equality(&mut self) -> Result<Expr, Error> {
        self.binary(Self::relational, &[(Tok::Eq, Op::Eq), (Tok::Ne, Op::Ne)])
    }

    fn relational(&mut self) -> Result<Expr, Error> {
        self.binary(
            Self::additive,
            &[(Tok::Lt, Op::Lt), (Tok::Le, Op::Le), (Tok::Gt, Op::Gt), (Tok::Ge, Op::Ge)],
        )
    }

    fn additive(&mut self) -> Result<Expr, Error> {
        self.binary(Self::multiplicative, &[(Tok::Plus, Op::Add), (Tok::Minus, Op::Sub)])
    }

    fn multiplicative(&mut self) -> Result<Expr, Error> {
        self.binary(
            Self::unary,
            &[
                (Tok::OpName("*"), Op::Mul),
                (Tok::OpName("div"), Op::Div),
                (Tok::OpName("mod"), Op::Mod),
            ],
        )
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        if self.eat(&Tok::Minus) {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.binary(Self::path, &[(Tok::Pipe, Op::Union)])
    }

    fn path(&mut self) -> Result<Expr, Error> {
        match self.peek() {
            Some(Tok::Slash) => {
                self.pos += 1;
                let steps = if self.starts_step() { self.relative()? } else { Vec::new() };
                Ok(Expr::Path(true, steps))
            }
            Some(Tok::DSlash) => {
                self.pos += 1;
                let mut steps = Vec::new();
                steps.push(any_node(Axis::DescendantOrSelf));
                steps.extend(self.relative()?);
                Ok(Expr::Path(true, steps))
            }
            _ if self.starts_step() => Ok(Expr::Path(false, self.relative()?)),
            _ => {
                let primary = self.primary()?;
                let preds = self.predicates()?;
                let mut steps = Vec::new();
                if self.eat(&Tok::Slash) {
                    steps = self.relative()?;
                } else if self.eat(&Tok::DSlash) {
                    steps.push(any_node(Axis::DescendantOrSelf));
                    steps.extend(self.relative()?);
                }
                if preds.is_empty() && steps.is_empty() {
                    return Ok(primary);
                }
                Ok(Expr::Filter(Box::new(primary), preds, steps))
            }
        }
    }

    fn starts_step(&self) -> bool {
        match self.peek() {
            Some(Tok::Dot | Tok::DDot | Tok::At | Tok::Axis(_) | Tok::Name(_) | Tok::Star) => true,
            Some(Tok::Func(f)) => {
                matches!(f.as_str(), "node" | "text" | "comment" | "processing-instruction")
            }
            _ => false,
        }
    }

    fn relative(&mut self) -> Result<Vec<Step>, Error> {
        let mut steps = Vec::new();
        steps.push(self.step()?);
        loop {
            if self.eat(&Tok::Slash) {
                steps.push(self.step()?);
            } else if self.eat(&Tok::DSlash) {
                steps.push(any_node(Axis::DescendantOrSelf));
                steps.push(self.step()?);
            } else {
                return Ok(steps);
            }
        }
    }

    fn step(&mut self) -> Result<Step, Error> {
        if self.eat(&Tok::Dot) {
            return Ok(any_node(Axis::Current));
        }
        if self.eat(&Tok::DDot) {
            return Ok(any_node(Axis::Parent));
        }
        let axis = if self.eat(&Tok::At) {
            Axis::Attribute
        } else if let Some(Tok::Axis(a)) = self.peek() {
            let axis = match a.as_str() {
                "child" => Axis::Child,
                "descendant" => Axis::Descendant,
                "descendant-or-self" => Axis::DescendantOrSelf,
                "parent" => Axis::Parent,
                "ancestor" => Axis::Ancestor,
                "ancestor-or-self" => Axis::AncestorOrSelf,
                "following-sibling" => Axis::FollowingSibling,
                "preceding-sibling" => Axis::PrecedingSibling,
                "following" => Axis::Following,
                "preceding" => Axis::Preceding,
                "attribute" => Axis::Attribute,
                "self" => Axis::Current,
                "namespace" if self.xslt => Axis::Namespace,
                "namespace" => return Err("the namespace axis"),
                _ => return Err("an unknown axis"),
            };
            self.pos += 1;
            axis
        } else {
            Axis::Child
        };
        let test = match self.toks.get(self.pos).cloned() {
            Some(Tok::Star) => {
                self.pos += 1;
                Test::Name(None, None)
            }
            Some(Tok::Name(n)) => {
                self.pos += 1;
                match n.split_once(':') {
                    Some((p, l)) => {
                        let uri = self.ns.iter().find(|(q, _)| q == p).map(|(_, u)| u.clone());
                        let Some(uri) = uri else { return Err("an undeclared prefix") };
                        Test::Name(Some(uri), (l != "*").then(|| l.to_string()))
                    }
                    None => Test::Name(Some(String::new()), Some(n)),
                }
            }
            Some(Tok::Func(f)) => {
                self.pos += 1;
                self.expect(&Tok::LParen)?;
                let t = match f.as_str() {
                    "node" => Test::Node,
                    "text" => Test::Text,
                    "comment" => Test::Comment,
                    "processing-instruction" => match self.peek().cloned() {
                        Some(Tok::Lit(l)) => {
                            self.pos += 1;
                            Test::Pi(Some(l))
                        }
                        _ => Test::Pi(None),
                    },
                    _ => return Err("a function where a step is expected"),
                };
                self.expect(&Tok::RParen)?;
                t
            }
            _ => return Err("a step expected"),
        };
        let preds = self.predicates()?;
        Ok(Step { axis, test, preds })
    }

    fn predicates(&mut self) -> Result<Vec<Expr>, Error> {
        let mut preds = Vec::new();
        while self.eat(&Tok::LBr) {
            preds.push(self.or()?);
            self.expect(&Tok::RBr)?;
        }
        Ok(preds)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        match self.toks.get(self.pos).cloned() {
            Some(Tok::Lit(s)) => {
                self.pos += 1;
                Ok(Expr::Lit(s))
            }
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Num(n))
            }
            Some(Tok::Var(v)) if self.xslt => {
                self.pos += 1;
                Ok(Expr::Var(v))
            }
            Some(Tok::Var(_)) => Err("a variable (selections have none)"),
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.or()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Func(f)) => {
                self.pos += 1;
                let f = self.function_name(f)?;
                self.expect(&Tok::LParen)?;
                let mut args = Vec::new();
                if !self.eat(&Tok::RParen) {
                    loop {
                        args.push(self.or()?);
                        if self.eat(&Tok::RParen) {
                            break;
                        }
                        self.expect(&Tok::Comma)?;
                    }
                }
                check_call(&f, args.len(), self.xslt)?;
                Ok(Expr::Call(f, args))
            }
            _ => Err("an expression expected"),
        }
    }

    /// A function's name: a prefixed one resolved through the
    /// declarations, MSXML's `node-set` as `msxsl:node-set` whatever its
    /// prefix.
    fn function_name(&self, f: String) -> Result<String, Error> {
        let Some((p, local)) = f.split_once(':') else { return Ok(f) };
        if !self.xslt {
            return Err("an unknown function");
        }
        let uri = self.ns.iter().find(|(q, _)| q == p).map(|(_, u)| u.as_str());
        match (uri, local) {
            (None, _) => Err("an undeclared prefix"),
            (Some(MSXSL_NS), "node-set") => Ok(String::from("msxsl:node-set")),
            (Some(MSXSL_NS), _) => Err("msxsl: functions but node-set() are not supported"),
            _ => Err("extension functions are not supported"),
        }
    }
}

fn any_node(axis: Axis) -> Step {
    Step { axis, test: Test::Node, preds: Vec::new() }
}

/// Whether `name` is a core function (or, `xslt`, one of XSLT's that
/// gala has) taking `n` arguments.
fn check_call(name: &str, n: usize, xslt: bool) -> Result<(), Error> {
    let (lo, hi) = match name {
        "current" if xslt => (0, 0),
        "msxsl:node-set" if xslt => (1, 1),
        "key" if xslt => return Err("key() is not supported"),
        "document" if xslt => return Err("document() is not supported"),
        "format-number" if xslt => return Err("format-number() is not supported"),
        "generate-id" if xslt => return Err("generate-id() is not supported"),
        "system-property" if xslt => return Err("system-property() is not supported"),
        "unparsed-entity-uri" if xslt => return Err("unparsed-entity-uri() is not supported"),
        "element-available" | "function-available" if xslt => {
            return Err("*-available() is not supported");
        }
        "last" | "position" | "true" | "false" => (0, 0),
        "count" | "not" | "boolean" | "sum" | "floor" | "ceiling" | "round" => (1, 1),
        "local-name" | "name" | "namespace-uri" | "string" | "string-length"
        | "normalize-space" | "number" => (0, 1),
        "starts-with" | "contains" | "substring-before" | "substring-after" => (2, 2),
        "substring" => (2, 3),
        "translate" => (3, 3),
        "concat" => (2, usize::MAX),
        "id" | "lang" => return Err("id() and lang() are not supported"),
        _ => return Err("an unknown function"),
    };
    if n < lo || n > hi { Err("a function given the wrong number of arguments") } else { Ok(()) }
}

/// A compiled expression, and its text (for what a failure records).
pub struct Query(Expr, String);

impl Query {
    pub fn text(&self) -> &str {
        &self.1
    }
}

/// Compiles `text` with the prefixes `ns` (prefix, URI) declares.
pub fn compile(text: &str, ns: &[(String, String)]) -> Result<Query, Error> {
    compile_as(text, ns, false)
}

/// Compiles an XSLT expression (variables, the namespace axis,
/// current() and `msxsl:node-set()`).
pub fn compile_xslt(text: &str, ns: &[(String, String)]) -> Result<Query, Error> {
    compile_as(text, ns, true)
}

fn compile_as(text: &str, ns: &[(String, String)], xslt: bool) -> Result<Query, Error> {
    let mut p = Parser { toks: lex(text)?, pos: 0, ns, xslt };
    if p.toks.is_empty() {
        return Err("an empty expression");
    }
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err("text after the expression");
    }
    Ok(Query(e, text.into()))
}

// ---- Evaluation ----

/// The variables in scope: a variable's value by name.
pub type Vars<'a> = dyn Fn(&str) -> Option<Value> + 'a;

/// What an XSLT evaluation sees beyond the tree: each node's place in
/// document order (`rank`: nodes past its end, made after it was
/// computed, come after every ranked node, in the order they were made),
/// the namespace nodes, the variables in scope and XSLT's current node.
pub struct Env<'a> {
    pub tree: &'a Tree,
    pub rank: &'a [u32],
    pub ns_nodes: &'a RefCell<NsNodes>,
    pub vars: &'a Vars<'a>,
    pub current: u32,
}

struct Eval<'t> {
    t: &'t Tree,
    /// Each node's place in document order (attributes after their
    /// element, before its children); u32::MAX or past the end: unranked.
    rank: &'t [u32],
    /// The namespace nodes (None: a DOM selection, which has no
    /// namespace axis).
    ns: Option<&'t RefCell<NsNodes>>,
    vars: Option<&'t Vars<'t>>,
    current: u32,
}

#[derive(Clone, Copy)]
struct Ctx {
    node: u32,
    pos: usize,
    size: usize,
}

/// Each node's place in document order, from the tops of the trees in
/// the arena (the document and every node of no parent), in their order.
pub fn rank_all(t: &Tree) -> Vec<u32> {
    let mut rank = alloc::vec![u32::MAX; t.nodes.len()];
    let mut n = 0;
    for top in 0..t.nodes.len() as u32 {
        if t.node(top).parent == NONE {
            rank_from(t, top, &mut rank, &mut n);
        }
    }
    rank
}

fn rank_from(t: &Tree, top: u32, rank: &mut [u32], n: &mut u32) {
    let mut stack = alloc::vec![top];
    while let Some(i) = stack.pop() {
        rank[i as usize] = *n;
        *n += 1;
        let nd = t.node(i);
        for &a in &nd.attrs {
            rank[a as usize] = *n;
            *n += 1;
        }
        stack.extend(nd.children.iter().rev());
    }
}

impl Query {
    /// Evaluates against context node `node` (a DOM selection).
    pub fn eval(&self, t: &Tree, node: u32) -> Result<Value, Error> {
        let mut rank = alloc::vec![u32::MAX; t.nodes.len()];
        // The root of the context node's tree (a detached node's own).
        let mut top = node;
        while t.node(top).parent != NONE {
            top = t.node(top).parent;
        }
        rank_from(t, top, &mut rank, &mut 0);
        let ev = Eval { t, rank: &rank, ns: None, vars: None, current: node };
        ev.expr(&self.0, Ctx { node, pos: 1, size: 1 })
    }

    /// Evaluates in an XSLT environment with context node `node` at
    /// position `pos` of `size`.
    pub fn eval_in(&self, env: &Env, node: u32, pos: usize, size: usize) -> Result<Value, Error> {
        let ev = Eval {
            t: env.tree,
            rank: env.rank,
            ns: Some(env.ns_nodes),
            vars: Some(env.vars),
            current: env.current,
        };
        ev.expr(&self.0, Ctx { node, pos, size })
    }
}

/// An XSLT match pattern's alternative (a location path pattern of
/// child, attribute and `//` steps).
pub struct Pattern(Expr);

/// Compiles an XSLT pattern: its alternatives (a union's parts), each
/// with its default priority (XSLT 5.5).
pub fn compile_pattern(text: &str, ns: &[(String, String)]) -> Result<Vec<(Pattern, f64)>, Error> {
    let Query(e, _) = compile_xslt(text, ns)?;
    let mut alts = Vec::new();
    let mut stack = alloc::vec![e];
    while let Some(e) = stack.pop() {
        match e {
            Expr::Binary(Op::Union, a, b) => {
                stack.push(*b);
                stack.push(*a);
            }
            Expr::Path(absolute, steps) => {
                for (k, s) in steps.iter().enumerate() {
                    let filler = s.axis == Axis::DescendantOrSelf
                        && matches!(s.test, Test::Node)
                        && s.preds.is_empty()
                        && k + 1 < steps.len();
                    if !(matches!(s.axis, Axis::Child | Axis::Attribute) || filler) {
                        return Err("pattern axes but child and attribute are not supported");
                    }
                }
                let priority = match steps.as_slice() {
                    [s] if !absolute && s.preds.is_empty() => match &s.test {
                        Test::Name(Some(_), Some(_)) | Test::Pi(Some(_)) => 0.0,
                        Test::Name(Some(_), None) => -0.25,
                        _ => -0.5,
                    },
                    _ => 0.5,
                };
                alts.push((Pattern(Expr::Path(absolute, steps)), priority));
            }
            _ => return Err("id() and key() patterns are not supported"),
        }
    }
    Ok(alts)
}

impl Pattern {
    /// Whether node `n` matches.
    pub fn matches(&self, env: &Env, n: u32) -> Result<bool, Error> {
        let Expr::Path(absolute, steps) = &self.0 else { return Ok(false) };
        let ev = Eval {
            t: env.tree,
            rank: env.rank,
            ns: Some(env.ns_nodes),
            vars: Some(env.vars),
            current: env.current,
        };
        ev.match_steps(*absolute, steps, n)
    }
}

impl Eval<'_> {
    /// A node's place in document order: its element's then 1 + its
    /// number for a namespace node (before the attributes), the rank
    /// for a ranked node, after them all in arena order for the others.
    fn rank(&self, n: u32) -> u64 {
        let base = |m: u32| -> u64 {
            match self.rank.get(m as usize) {
                Some(&r) if r != u32::MAX => u64::from(r),
                _ => (1u64 << 32) + u64::from(m),
            }
        };
        if is_ns(n) {
            let el = self.ns.map_or(0, |t| t.borrow().get(n).0);
            (base(el) << 20) | (1 + u64::from(n & 0x7_ffff))
        } else {
            base(n) << 20
        }
    }

    fn sort(&self, nodes: &mut Vec<u32>) {
        nodes.sort_by_key(|&n| (self.rank(n), n));
        nodes.dedup();
    }

    /// A node's parent (a namespace node's is its element).
    fn parent(&self, n: u32) -> u32 {
        if is_ns(n) { self.ns.map_or(NONE, |t| t.borrow().get(n).0) } else { self.t.node(n).parent }
    }

    fn root_of(&self, mut n: u32) -> u32 {
        while self.parent(n) != NONE {
            n = self.parent(n);
        }
        n
    }

    /// Whether `n` is a root: the document or a fragment, of no parent.
    fn is_root(&self, n: u32) -> bool {
        !is_ns(n)
            && self.t.node(n).parent == NONE
            && matches!(self.t.node(n).kind, Kind::Document | Kind::Fragment)
    }

    fn match_steps(&self, absolute: bool, steps: &[Step], n: u32) -> Result<bool, Error> {
        let Some((last, rest)) = steps.split_last() else {
            return Ok(!absolute || self.is_root(n));
        };
        if last.axis == Axis::DescendantOrSelf {
            // `//`: an ancestor or the node itself matches the rest.
            let mut a = n;
            while a != NONE {
                if self.match_steps(absolute, rest, a)? {
                    return Ok(true);
                }
                a = self.parent(a);
            }
            return Ok(false);
        }
        if is_ns(n) {
            return Ok(false);
        }
        let node = self.t.node(n);
        let is_attr = node.kind == Kind::Attribute;
        if node.parent == NONE || is_attr != (last.axis == Axis::Attribute) || node.is_xmlns() {
            return Ok(false);
        }
        if !self.matches(last, n) {
            return Ok(false);
        }
        if !last.preds.is_empty() {
            let mut found = self.axis(last.axis, node.parent);
            found.retain(|&m| self.matches(last, m));
            for p in &last.preds {
                found = self.filter(found, p)?;
            }
            if !found.contains(&n) {
                return Ok(false);
            }
        }
        self.match_steps(absolute, rest, node.parent)
    }

    fn expr(&self, e: &Expr, c: Ctx) -> Result<Value, Error> {
        Ok(match e {
            Expr::Lit(s) => Value::Str(s.clone()),
            Expr::Num(n) => Value::Num(*n),
            Expr::Neg(e) => Value::Num(-self.number(self.expr(e, c)?)),
            Expr::Var(name) => match self.vars.and_then(|v| v(name)) {
                Some(v) => v,
                None => return Err("a variable that is not defined"),
            },
            Expr::Path(absolute, steps) => {
                let start = if *absolute { self.root_of(c.node) } else { c.node };
                Value::Nodes(self.steps(alloc::vec![start], steps)?)
            }
            Expr::Filter(primary, preds, steps) => {
                let nodes = match self.expr(primary, c)? {
                    Value::Nodes(n) => n,
                    Value::Rtf(_) => {
                        return Err("a result tree fragment used as a node-set (without \\
                                    msxsl:node-set())");
                    }
                    _ => return Err("a filter on a value that is not a node-set"),
                };
                let mut nodes = nodes;
                for p in preds {
                    nodes = self.filter(nodes, p)?;
                }
                Value::Nodes(self.steps(nodes, steps)?)
            }
            Expr::Call(name, args) => self.call(name, args, c)?,
            Expr::Binary(op, a, b) => self.binary(*op, a, b, c)?,
        })
    }

    fn binary(&self, op: Op, a: &Expr, b: &Expr, c: Ctx) -> Result<Value, Error> {
        Ok(match op {
            Op::Or => Value::Bool(self.boolean(self.expr(a, c)?) || self.boolean(self.expr(b, c)?)),
            Op::And => {
                Value::Bool(self.boolean(self.expr(a, c)?) && self.boolean(self.expr(b, c)?))
            }
            Op::Union => {
                let (Value::Nodes(mut x), Value::Nodes(y)) = (self.expr(a, c)?, self.expr(b, c)?)
                else {
                    return Err("a union of values that are not node-sets");
                };
                x.extend(y);
                self.sort(&mut x);
                Value::Nodes(x)
            }
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                Value::Bool(self.compare(op, self.expr(a, c)?, self.expr(b, c)?))
            }
            _ => {
                let x = self.number(self.expr(a, c)?);
                let y = self.number(self.expr(b, c)?);
                Value::Num(match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                    _ => remainder(x, y),
                })
            }
        })
    }

    /// XPath 3.4's comparisons (a result tree fragment compares as a
    /// node-set of its root).
    fn compare(&self, op: Op, a: Value, b: Value) -> bool {
        let as_nodes = |v: Value| match v {
            Value::Rtf(f) => Value::Nodes(alloc::vec![f]),
            v => v,
        };
        let (a, b) = (as_nodes(a), as_nodes(b));
        let num = |x: f64, y: f64| match op {
            Op::Eq => x == y,
            Op::Ne => x != y,
            Op::Lt => x < y,
            Op::Le => x <= y,
            Op::Gt => x > y,
            _ => x >= y,
        };
        let eq_op = matches!(op, Op::Eq | Op::Ne);
        let strings = |x: &str, y: &str| {
            if eq_op { (x == y) == matches!(op, Op::Eq) } else { num(to_number(x), to_number(y)) }
        };
        match (a, b) {
            (Value::Nodes(x), Value::Nodes(y)) => {
                let ys: Vec<String> = y.iter().map(|&n| self.string_of(n)).collect();
                x.iter().any(|&n| {
                    let s = self.string_of(n);
                    ys.iter().any(|t| strings(&s, t))
                })
            }
            (Value::Nodes(x), other) => self.mixed(op, &x, &other, false),
            (other, Value::Nodes(x)) => self.mixed(op, &x, &other, true),
            (a, b) => {
                if eq_op {
                    let same = match (&a, &b) {
                        (Value::Bool(_), _) | (_, Value::Bool(_)) => {
                            self.boolean_ref(&a) == self.boolean_ref(&b)
                        }
                        (Value::Num(_), _) | (_, Value::Num(_)) => {
                            self.number_ref(&a) == self.number_ref(&b)
                        }
                        _ => self.string_ref(&a) == self.string_ref(&b),
                    };
                    same == matches!(op, Op::Eq)
                } else {
                    num(self.number_ref(&a), self.number_ref(&b))
                }
            }
        }
    }

    /// A comparison of a node-set with another value; `flipped` when
    /// the node-set is on the right.
    fn mixed(&self, op: Op, x: &[u32], other: &Value, flipped: bool) -> bool {
        let op = if flipped {
            match op {
                Op::Lt => Op::Gt,
                Op::Le => Op::Ge,
                Op::Gt => Op::Lt,
                Op::Ge => Op::Le,
                o => o,
            }
        } else {
            op
        };
        let num = |a: f64, b: f64| match op {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            _ => a >= b,
        };
        let eq_op = matches!(op, Op::Eq | Op::Ne);
        match other {
            Value::Num(v) => x.iter().any(|&n| num(to_number(&self.string_of(n)), *v)),
            Value::Str(s) => x.iter().any(|&n| {
                let t = self.string_of(n);
                if eq_op {
                    (t == *s) == matches!(op, Op::Eq)
                } else {
                    num(to_number(&t), to_number(s))
                }
            }),
            Value::Bool(v) => {
                let b = !x.is_empty();
                if eq_op {
                    (b == *v) == matches!(op, Op::Eq)
                } else {
                    num(f64::from(u8::from(b)), f64::from(u8::from(*v)))
                }
            }
            Value::Nodes(_) | Value::Rtf(_) => false,
        }
    }

    fn steps(&self, mut nodes: Vec<u32>, steps: &[Step]) -> Result<Vec<u32>, Error> {
        for s in steps {
            let mut out = Vec::new();
            for &n in &nodes {
                let mut found = self.axis(s.axis, n);
                found.retain(|&m| self.matches(s, m));
                for p in &s.preds {
                    found = self.filter(found, p)?;
                }
                out.extend(found);
            }
            self.sort(&mut out);
            nodes = out;
        }
        Ok(nodes)
    }

    /// Keeps the nodes (in axis order) for which `pred` holds.
    fn filter(&self, nodes: Vec<u32>, pred: &Expr) -> Result<Vec<u32>, Error> {
        let size = nodes.len();
        let mut out = Vec::new();
        for (i, &n) in nodes.iter().enumerate() {
            let v = self.expr(pred, Ctx { node: n, pos: i + 1, size })?;
            let keep = match v {
                Value::Num(x) => x == (i + 1) as f64,
                v => self.boolean(v),
            };
            if keep {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// The nodes on `axis` from `n`, in axis order (reverse axes nearest
    /// first).
    fn axis(&self, axis: Axis, n: u32) -> Vec<u32> {
        let t = self.t;
        let mut out = Vec::new();
        if is_ns(n) {
            // A namespace node has its element as parent and nothing
            // below or beside it.
            match axis {
                Axis::Current | Axis::DescendantOrSelf => out.push(n),
                Axis::Parent | Axis::Ancestor | Axis::AncestorOrSelf => {
                    if axis == Axis::AncestorOrSelf {
                        out.push(n);
                    }
                    let mut p = self.parent(n);
                    while p != NONE {
                        out.push(p);
                        if axis == Axis::Parent {
                            break;
                        }
                        p = t.node(p).parent;
                    }
                }
                _ => {}
            }
            return out;
        }
        let node = t.node(n);
        let is_attr = node.kind == Kind::Attribute;
        let descendants = |from: u32, out: &mut Vec<u32>| {
            let mut stack: Vec<u32> = t.node(from).children.iter().rev().copied().collect();
            while let Some(c) = stack.pop() {
                out.push(c);
                stack.extend(t.node(c).children.iter().rev());
            }
        };
        match axis {
            Axis::Current => out.push(n),
            Axis::Child if !is_attr => out.extend(&node.children),
            Axis::Attribute if !is_attr => {
                out.extend(node.attrs.iter().copied().filter(|&a| !t.node(a).is_xmlns()))
            }
            Axis::Namespace if node.kind == Kind::Element => {
                if let Some(table) = self.ns {
                    out.extend(table.borrow_mut().of(t, n));
                }
            }
            Axis::Descendant if !is_attr => descendants(n, &mut out),
            Axis::DescendantOrSelf => {
                out.push(n);
                if !is_attr {
                    descendants(n, &mut out);
                }
            }
            Axis::Parent => {
                if node.parent != NONE {
                    out.push(node.parent);
                }
            }
            Axis::Ancestor | Axis::AncestorOrSelf => {
                if axis == Axis::AncestorOrSelf {
                    out.push(n);
                }
                let mut p = node.parent;
                while p != NONE {
                    out.push(p);
                    p = t.node(p).parent;
                }
            }
            Axis::FollowingSibling | Axis::PrecedingSibling if !is_attr => {
                if node.parent != NONE {
                    let sibs = &t.node(node.parent).children;
                    if let Some(k) = sibs.iter().position(|&c| c == n) {
                        if axis == Axis::FollowingSibling {
                            out.extend(&sibs[k + 1..]);
                        } else {
                            out.extend(sibs[..k].iter().rev());
                        }
                    }
                }
            }
            Axis::Following | Axis::Preceding => {
                // Every node of the tree after (before) n in document
                // order, not counting its descendants (ancestors) or
                // attributes.
                let mut all = Vec::new();
                descendants(self.root_of(n), &mut all);
                let r = self.rank(n);
                let mut ancestors = Vec::new();
                let mut p = node.parent;
                while p != NONE {
                    ancestors.push(p);
                    p = t.node(p).parent;
                }
                if axis == Axis::Following {
                    let mut inside = Vec::new();
                    if !is_attr {
                        descendants(n, &mut inside);
                    }
                    out.extend(
                        all.into_iter().filter(|&m| self.rank(m) > r && !inside.contains(&m)),
                    );
                } else {
                    out.extend(
                        all.into_iter()
                            .rev()
                            .filter(|&m| self.rank(m) < r && !ancestors.contains(&m)),
                    );
                }
            }
            _ => {}
        }
        out
    }

    fn matches(&self, s: &Step, m: u32) -> bool {
        if is_ns(m) {
            // The namespace axis' principal node type: a name test names
            // the prefix.
            let Some(table) = self.ns else { return false };
            let table = table.borrow();
            let prefix = table.get(m).1;
            return match &s.test {
                Test::Node => true,
                Test::Name(ns, local) => {
                    s.axis == Axis::Namespace
                        && ns.as_ref().is_none_or(|u| u.is_empty())
                        && local.as_ref().is_none_or(|l| l == prefix)
                }
                _ => false,
            };
        }
        let node = self.t.node(m);
        match &s.test {
            Test::Node => true,
            Test::Text => matches!(node.kind, Kind::Text | Kind::CData),
            Test::Comment => node.kind == Kind::Comment,
            Test::Pi(target) => {
                node.kind == Kind::Pi && target.as_ref().is_none_or(|t| *t == node.name)
            }
            Test::Name(ns, local) => {
                let principal =
                    if s.axis == Axis::Attribute { Kind::Attribute } else { Kind::Element };
                node.kind == principal
                    && ns.as_ref().is_none_or(|u| *u == node.ns)
                    && local.as_ref().is_none_or(|l| l == node.local())
            }
        }
    }

    fn call(&self, name: &str, args: &[Expr], c: Ctx) -> Result<Value, Error> {
        let arg = |i: usize| self.expr(&args[i], c);
        let str_arg = |i: usize| -> Result<String, Error> { Ok(self.string(arg(i)?)) };
        // A node-set argument's first node, or the context node.
        let first = |c: Ctx| -> Result<Option<u32>, Error> {
            if args.is_empty() {
                return Ok(Some(c.node));
            }
            match arg(0)? {
                Value::Nodes(n) => Ok(n.first().copied()),
                _ => Err("a function given a value that is not a node-set"),
            }
        };
        Ok(match name {
            "last" => Value::Num(c.size as f64),
            "position" => Value::Num(c.pos as f64),
            "count" => match arg(0)? {
                Value::Nodes(n) => Value::Num(n.len() as f64),
                _ => return Err("count() of a value that is not a node-set"),
            },
            "local-name" | "name" | "namespace-uri" => {
                let s = first(c)?.map_or(String::new(), |n| {
                    if is_ns(n) {
                        // A namespace node's name is its prefix; it is in
                        // no namespace.
                        return match (name, self.ns) {
                            ("namespace-uri", _) | (_, None) => String::new(),
                            (_, Some(t)) => t.borrow().get(n).1.into(),
                        };
                    }
                    let node = self.t.node(n);
                    match (name, node.kind) {
                        ("namespace-uri", _) => node.ns.clone(),
                        (_, Kind::Pi) => node.name.clone(),
                        ("name", Kind::Element | Kind::Attribute) => node.name.clone(),
                        (_, Kind::Element | Kind::Attribute) => node.local().into(),
                        _ => String::new(),
                    }
                });
                Value::Str(s)
            }
            "string" => {
                Value::Str(if args.is_empty() { self.string_of(c.node) } else { str_arg(0)? })
            }
            "concat" => {
                let mut s = String::new();
                for i in 0..args.len() {
                    s.push_str(&str_arg(i)?);
                }
                Value::Str(s)
            }
            "starts-with" => Value::Bool(str_arg(0)?.starts_with(str_arg(1)?.as_str())),
            "contains" => Value::Bool(str_arg(0)?.contains(str_arg(1)?.as_str())),
            "substring-before" => {
                let (s, p) = (str_arg(0)?, str_arg(1)?);
                Value::Str(s.find(p.as_str()).map_or(String::new(), |i| s[..i].into()))
            }
            "substring-after" => {
                let (s, p) = (str_arg(0)?, str_arg(1)?);
                Value::Str(s.find(p.as_str()).map_or(String::new(), |i| s[i + p.len()..].into()))
            }
            "substring" => {
                let s = str_arg(0)?;
                let start = round(self.number(arg(1)?));
                let end = if args.len() > 2 {
                    start + round(self.number(arg(2)?))
                } else {
                    f64::INFINITY
                };
                // Characters at positions p (from 1) with start <= p < end.
                Value::Str(
                    s.chars()
                        .enumerate()
                        .filter(|&(i, _)| {
                            let p = (i + 1) as f64;
                            p >= start && p < end
                        })
                        .map(|(_, ch)| ch)
                        .collect(),
                )
            }
            "string-length" => {
                let s = if args.is_empty() { self.string_of(c.node) } else { str_arg(0)? };
                Value::Num(s.chars().count() as f64)
            }
            "normalize-space" => {
                let s = if args.is_empty() { self.string_of(c.node) } else { str_arg(0)? };
                let mut out = String::new();
                for w in s.split([' ', '\t', '\n', '\r']).filter(|w| !w.is_empty()) {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(w);
                }
                Value::Str(out)
            }
            "translate" => {
                let (s, from, to) = (str_arg(0)?, str_arg(1)?, str_arg(2)?);
                let from: Vec<char> = from.chars().collect();
                let to: Vec<char> = to.chars().collect();
                Value::Str(
                    s.chars()
                        .filter_map(|ch| match from.iter().position(|&f| f == ch) {
                            Some(k) => to.get(k).copied(),
                            None => Some(ch),
                        })
                        .collect(),
                )
            }
            "not" => Value::Bool(!self.boolean(arg(0)?)),
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "boolean" => Value::Bool(self.boolean(arg(0)?)),
            "number" => Value::Num(if args.is_empty() {
                to_number(&self.string_of(c.node))
            } else {
                self.number(arg(0)?)
            }),
            "sum" => match arg(0)? {
                Value::Nodes(n) => {
                    Value::Num(n.iter().map(|&m| to_number(&self.string_of(m))).sum())
                }
                _ => return Err("sum() of a value that is not a node-set"),
            },
            "floor" => Value::Num(floor(self.number(arg(0)?))),
            "ceiling" => Value::Num(-floor(-self.number(arg(0)?))),
            "round" => Value::Num(round(self.number(arg(0)?))),
            "current" => Value::Nodes(alloc::vec![self.current]),
            "msxsl:node-set" => match arg(0)? {
                Value::Rtf(f) => Value::Nodes(alloc::vec![f]),
                Value::Nodes(n) => Value::Nodes(n),
                _ => return Err("node-set() of a string, number or boolean is not supported"),
            },
            _ => return Err("an unknown function"),
        })
    }

    fn string_of(&self, n: u32) -> String {
        if is_ns(n) {
            return self.ns.map_or(String::new(), |t| t.borrow().get(n).2.into());
        }
        let mut s = String::new();
        self.t.text(n, &mut s);
        s
    }

    fn string(&self, v: Value) -> String {
        self.string_ref(&v)
    }

    fn string_ref(&self, v: &Value) -> String {
        match v {
            Value::Nodes(n) => n.first().map_or(String::new(), |&m| self.string_of(m)),
            Value::Rtf(f) => self.string_of(*f),
            Value::Str(s) => s.clone(),
            Value::Num(x) => number_text(*x),
            Value::Bool(b) => (if *b { "true" } else { "false" }).into(),
        }
    }

    fn number(&self, v: Value) -> f64 {
        self.number_ref(&v)
    }

    fn number_ref(&self, v: &Value) -> f64 {
        match v {
            Value::Num(x) => *x,
            Value::Bool(b) => f64::from(u8::from(*b)),
            other => to_number(&self.string_ref(other)),
        }
    }

    fn boolean(&self, v: Value) -> bool {
        self.boolean_ref(&v)
    }

    fn boolean_ref(&self, v: &Value) -> bool {
        match v {
            Value::Nodes(n) => !n.is_empty(),
            Value::Rtf(_) => true,
            Value::Str(s) => !s.is_empty(),
            Value::Num(x) => *x != 0.0 && !x.is_nan(),
            Value::Bool(b) => *b,
        }
    }
}

/// XPath's number(): optional white space, an optional minus, digits
/// with an optional point; anything else is NaN.
fn to_number(s: &str) -> f64 {
    let t = s.trim_matches([' ', '\t', '\n', '\r']);
    let digits = t.strip_prefix('-').unwrap_or(t);
    if digits.is_empty()
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        || digits.chars().filter(|&c| c == '.').count() > 1
        || digits == "."
    {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// A number as XPath's string() writes it.
pub fn number_text(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        (if x > 0.0 { "Infinity" } else { "-Infinity" }).into()
    } else if x == 0.0 {
        "0".into()
    } else {
        alloc::format!("{x}")
    }
}

fn floor(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4_503_599_627_370_496.0 {
        return x;
    }
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

/// XPath's `mod`: the remainder of the truncating division (as
/// ECMAScript's `%`; no libm: computed from the truncated quotient).
fn remainder(x: f64, y: f64) -> f64 {
    if y == 0.0 || !x.is_finite() || y.is_nan() {
        return f64::NAN;
    }
    if y.is_infinite() {
        return x;
    }
    let q = x / y;
    let t = if q < 0.0 { -floor(-q) } else { floor(q) };
    x - t * y
}

/// XPath's round(): to the nearest integer, halves up.
fn round(x: f64) -> f64 {
    floor(x + 0.5)
}

/// Parses SelectionNamespaces' value: `xmlns:p='uri'` declarations
/// separated by white space (a default `xmlns='uri'` is kept under the
/// empty prefix, which names nothing in XPath 1.0). None when it does not
/// parse.
pub fn parse_namespaces(s: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let after = rest.strip_prefix("xmlns")?;
        let (prefix, after) = match after.strip_prefix(':') {
            Some(a) => {
                let end = a.find(|c: char| c == '=' || c.is_whitespace())?;
                (&a[..end], &a[end..])
            }
            None => ("", after),
        };
        let after = after.trim_start().strip_prefix('=')?.trim_start();
        let quote = after.chars().next().filter(|&q| q == '\'' || q == '"')?;
        let body = &after[1..];
        let end = body.find(quote)?;
        out.push((prefix.to_string(), body[..end].to_string()));
        rest = body[end + 1..].trim_start();
    }
    Some(out)
}

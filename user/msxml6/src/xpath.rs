//! XPath 1.0 (W3C, 1999) over the document tree: what selectNodes and
//! selectSingleNode evaluate (MSXML 6's only selection language).
//!
//! The whole expression language: location paths on every axis but
//! `namespace` (refused: recorded), node tests by name, `*`, `prefix:*`,
//! node(), text(), comment(), processing-instruction(); predicates;
//! unions; the boolean, comparison and arithmetic operators with
//! XPath's conversions; the core function library but `id()` and
//! `lang()` (refused). Prefixes come from the SelectionNamespaces
//! property only (an undeclared one is an error, as in MSXML 6); an
//! unprefixed name is in no namespace. No variables (MSXML has none).

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Kind, NONE, Tree};

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
}

/// A value: a node-set (in document order), a string, a number, a
/// boolean.
pub enum Value {
    Nodes(Vec<u32>),
    Str(String),
    Num(f64),
    Bool(bool),
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
    /// (prefix, namespace URI) from SelectionNamespaces.
    ns: &'a [(String, String)],
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
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.or()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Func(f)) => {
                self.pos += 1;
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
                check_call(&f, args.len())?;
                Ok(Expr::Call(f, args))
            }
            _ => Err("an expression expected"),
        }
    }
}

fn any_node(axis: Axis) -> Step {
    Step { axis, test: Test::Node, preds: Vec::new() }
}

/// Whether `name` is a core function taking `n` arguments.
fn check_call(name: &str, n: usize) -> Result<(), Error> {
    let (lo, hi) = match name {
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

/// A compiled expression.
pub struct Query(Expr);

/// Compiles `text` with the prefixes `ns` (prefix, URI) declares.
pub fn compile(text: &str, ns: &[(String, String)]) -> Result<Query, Error> {
    let mut p = Parser { toks: lex(text)?, pos: 0, ns };
    if p.toks.is_empty() {
        return Err("an empty expression");
    }
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err("text after the expression");
    }
    Ok(Query(e))
}

// ---- Evaluation ----

struct Eval<'t> {
    t: &'t Tree,
    /// Each node's place in document order (attributes after their
    /// element, before its children).
    rank: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Ctx {
    node: u32,
    pos: usize,
    size: usize,
}

impl Query {
    /// Evaluates against context node `node`.
    pub fn eval(&self, t: &Tree, node: u32) -> Result<Value, Error> {
        let mut rank = alloc::vec![u32::MAX; t.nodes.len()];
        let mut n = 0;
        // The root of the context node's tree (a detached node's own).
        let mut top = node;
        while t.node(top).parent != NONE {
            top = t.node(top).parent;
        }
        let mut stack = alloc::vec![top];
        while let Some(i) = stack.pop() {
            rank[i as usize] = n;
            n += 1;
            let nd = t.node(i);
            for &a in &nd.attrs {
                rank[a as usize] = n;
                n += 1;
            }
            stack.extend(nd.children.iter().rev());
        }
        let ev = Eval { t, rank };
        ev.expr(&self.0, Ctx { node, pos: 1, size: 1 })
    }
}

impl Eval<'_> {
    fn sort(&self, nodes: &mut Vec<u32>) {
        nodes.sort_by_key(|&n| (self.rank[n as usize], n));
        nodes.dedup();
    }

    fn root_of(&self, mut n: u32) -> u32 {
        while self.t.node(n).parent != NONE {
            n = self.t.node(n).parent;
        }
        n
    }

    fn expr(&self, e: &Expr, c: Ctx) -> Result<Value, Error> {
        Ok(match e {
            Expr::Lit(s) => Value::Str(s.clone()),
            Expr::Num(n) => Value::Num(*n),
            Expr::Neg(e) => Value::Num(-self.number(self.expr(e, c)?)),
            Expr::Path(absolute, steps) => {
                let start = if *absolute { self.root_of(c.node) } else { c.node };
                Value::Nodes(self.steps(alloc::vec![start], steps)?)
            }
            Expr::Filter(primary, preds, steps) => {
                let Value::Nodes(mut nodes) = self.expr(primary, c)? else {
                    return Err("a filter on a value that is not a node-set");
                };
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

    /// XPath 3.4's comparisons.
    fn compare(&self, op: Op, a: Value, b: Value) -> bool {
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
            Value::Nodes(_) => false,
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
        let node = t.node(n);
        let is_attr = node.kind == Kind::Attribute;
        let mut out = Vec::new();
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
                let r = self.rank[n as usize];
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
                        all.into_iter()
                            .filter(|&m| self.rank[m as usize] > r && !inside.contains(&m)),
                    );
                } else {
                    out.extend(
                        all.into_iter()
                            .rev()
                            .filter(|&m| self.rank[m as usize] < r && !ancestors.contains(&m)),
                    );
                }
            }
            _ => {}
        }
        out
    }

    fn matches(&self, s: &Step, m: u32) -> bool {
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
            _ => return Err("an unknown function"),
        })
    }

    fn string_of(&self, n: u32) -> String {
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
fn number_text(x: f64) -> String {
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

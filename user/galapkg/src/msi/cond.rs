//! Windows Installer conditional expressions, written from the SDK's
//! "Conditional Statement Syntax", "Examples of Conditional Statement
//! Syntax" and "Using Properties in Conditional Statements" pages
//! (learn.microsoft.com/windows/win32/msi): what the sequence tables,
//! the Condition table, Component.Condition and LaunchCondition hold.
//!
//! The grammar, lowest precedence first (the SDK's logical operator
//! table lists them high to low: Not, And, Or, Xor, Eqv, Imp; all
//! left-associative here):
//!
//! ```text
//! expression := xor-term { IMP xor-term } ... down to:
//! or-term    := and-term { OR and-term }
//! and-term   := factor { AND factor }
//! factor     := NOT factor | term
//! term       := value [ comparison value ] | "(" expression ")"
//! comparison := [~] ( = | <> | > | >= | < | <= | >< | << | >> )
//! value      := symbol | "literal" | integer
//! symbol     := property | %environment | $component-action
//!             | ?component-state | &feature-action | !feature-state
//! ```
//!
//! The value rules: an operator keyword is case-insensitive; property
//! names are case-sensitive, environment names are not; a property that
//! is not set is the empty string; literal text has no escape (no `"`
//! inside it); integers are decimal with an optional `-` and no
//! floating point; a value alone is true when it is a non-empty string
//! or a non-zero integer ("the logical value of a property that has
//! been set is True"). Comparing an integer with a string: the string is
//! taken as an integer when its whole text is one, otherwise the result
//! is false, except `<>`, which is true (the SDK's sentence covers "a
//! string or property value that cannot be converted"; this module
//! converts both kinds). Two strings compare as text (code point order;
//! `~` lowercases both first), two integers as numbers. On integers,
//! `><` is a bitwise AND test, `<<` compares the left's high 16 bits and
//! `>>` its low 16 bits with the right. Feature and component states
//! are the integers the SDK tabulates (-1 unknown, 1 advertised, 2
//! absent, 3 local, 4 source); a name with no state reads as an empty
//! string, like an unset property.
//!
//! An empty condition is true (msiEvaluateConditionNone, which the
//! tables treat as "no condition"). A syntax error is an [`Error`]
//! naming the character position, never a guess.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// INSTALLSTATE_UNKNOWN: no action.
pub const UNKNOWN: i32 = -1;
/// INSTALLSTATE_ADVERTISED (features only).
pub const ADVERTISED: i32 = 1;
/// INSTALLSTATE_ABSENT.
pub const ABSENT: i32 = 2;
/// INSTALLSTATE_LOCAL.
pub const LOCAL: i32 = 3;
/// INSTALLSTATE_SOURCE.
pub const SOURCE: i32 = 4;

/// What a condition can look up.
pub trait Context {
    /// A property's value (None: not set, read as "").
    fn property(&self, name: &str) -> Option<String>;
    /// An environment variable (names compare case-insensitively).
    fn environment(&self, _name: &str) -> Option<String> {
        None
    }
    /// `$component`: the component's action state.
    fn component_action(&self, _name: &str) -> Option<i32> {
        None
    }
    /// `?component`: the component's installed state.
    fn component_installed(&self, _name: &str) -> Option<i32> {
        None
    }
    /// `&feature`: the feature's action state.
    fn feature_action(&self, _name: &str) -> Option<i32> {
        None
    }
    /// `!feature`: the feature's installed state.
    fn feature_installed(&self, _name: &str) -> Option<i32> {
        None
    }
}

impl Context for BTreeMap<String, String> {
    fn property(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

/// A condition that is not valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The character position (0-based) where it went wrong.
    pub at: usize,
    pub what: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "condition syntax error at character {}: {}", self.at + 1, self.what)
    }
}

fn err<T>(at: usize, what: impl Into<String>) -> Result<T, Error> {
    Err(Error { at, what: what.into() })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    /// `><`: contains, or bitwise AND.
    Contains,
    /// `<<`: starts with, or the high 16 bits.
    Starts,
    /// `>>`: ends with, or the low 16 bits.
    Ends,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Logic {
    And,
    Or,
    Xor,
    Eqv,
    Imp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// A symbol with its prefix (0 for a property).
    Sym(u8, String),
    Str(String),
    Int(i32),
    Cmp(Cmp, bool),
    Not,
    Logic(Logic),
    Open,
    Close,
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

fn lex(s: &str) -> Result<Vec<(usize, Tok)>, Error> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let start = i;
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        let tok = match ch {
            '(' => {
                i += 1;
                Tok::Open
            }
            ')' => {
                i += 1;
                Tok::Close
            }
            '"' => {
                let Some(n) = c[i + 1..].iter().position(|&x| x == '"') else {
                    return err(i, "literal text has no closing quotation mark");
                };
                let text: String = c[i + 1..i + 1 + n].iter().collect();
                i += n + 2;
                Tok::Str(text)
            }
            '~' | '=' | '<' | '>' => {
                let ci = ch == '~';
                if ci {
                    i += 1;
                }
                let a = c.get(i).copied();
                let b = c.get(i + 1).copied();
                let (op, n) = match (a, b) {
                    (Some('='), _) => (Cmp::Eq, 1),
                    (Some('<'), Some('>')) => (Cmp::Ne, 2),
                    (Some('<'), Some('=')) => (Cmp::Le, 2),
                    (Some('<'), Some('<')) => (Cmp::Starts, 2),
                    (Some('<'), _) => (Cmp::Lt, 1),
                    (Some('>'), Some('=')) => (Cmp::Ge, 2),
                    (Some('>'), Some('<')) => (Cmp::Contains, 2),
                    (Some('>'), Some('>')) => (Cmp::Ends, 2),
                    (Some('>'), _) => (Cmp::Gt, 1),
                    _ => return err(start, "~ must be followed by a comparison operator"),
                };
                i += n;
                Tok::Cmp(op, ci)
            }
            '%' | '$' | '?' | '&' | '!' => {
                i += 1;
                let n = c[i..].iter().take_while(|&&x| ident_char(x)).count();
                if n == 0 {
                    return err(start, format!("{ch} is not followed by a name"));
                }
                let name: String = c[i..i + n].iter().collect();
                i += n;
                Tok::Sym(ch as u8, name)
            }
            '-' | '0'..='9' => {
                let digits_at = if ch == '-' { i + 1 } else { i };
                let n = c[digits_at..].iter().take_while(|x| x.is_ascii_digit()).count();
                if n == 0 {
                    return err(start, "- is not followed by digits (no arithmetic)");
                }
                let text: String = c[i..digits_at + n].iter().collect();
                i = digits_at + n;
                if c.get(i).is_some_and(|&x| ident_char(x)) {
                    return err(start, format!("{text} runs into a name"));
                }
                match text.parse::<i32>() {
                    Ok(v) => Tok::Int(v),
                    Err(_) => return err(start, format!("integer {text} is out of range")),
                }
            }
            x if x.is_ascii_alphabetic() || x == '_' => {
                let n = c[i..].iter().take_while(|&&x| ident_char(x)).count();
                let name: String = c[i..i + n].iter().collect();
                i += n;
                match name.to_ascii_uppercase().as_str() {
                    "NOT" => Tok::Not,
                    "AND" => Tok::Logic(Logic::And),
                    "OR" => Tok::Logic(Logic::Or),
                    "XOR" => Tok::Logic(Logic::Xor),
                    "EQV" => Tok::Logic(Logic::Eqv),
                    "IMP" => Tok::Logic(Logic::Imp),
                    _ => Tok::Sym(0, name),
                }
            }
            x => return err(i, format!("unexpected character {x:?}")),
        };
        out.push((start, tok));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Operand {
    Sym(u8, String),
    Str(String),
    Int(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    Value(Operand),
    Cmp(Cmp, bool, Operand, Operand),
    Not(Box<Expr>),
    Logic(Logic, Box<Expr>, Box<Expr>),
}

struct Parser {
    toks: Vec<(usize, Tok)>,
    at: usize,
    len: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(_, t)| t)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.at).map_or(self.len, |(p, _)| *p)
    }

    /// Binary levels from the lowest precedence (Imp) to And.
    fn level(&mut self, n: usize) -> Result<Expr, Error> {
        const ORDER: [Logic; 5] = [Logic::Imp, Logic::Eqv, Logic::Xor, Logic::Or, Logic::And];
        if n == ORDER.len() {
            return self.factor();
        }
        let mut left = self.level(n + 1)?;
        while self.peek() == Some(&Tok::Logic(ORDER[n])) {
            self.at += 1;
            let right = self.level(n + 1)?;
            left = Expr::Logic(ORDER[n], Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, Error> {
        if self.peek() == Some(&Tok::Not) {
            self.at += 1;
            return Ok(Expr::Not(Box::new(self.factor()?)));
        }
        self.term()
    }

    fn term(&mut self) -> Result<Expr, Error> {
        if self.peek() == Some(&Tok::Open) {
            self.at += 1;
            let e = self.level(0)?;
            if self.peek() != Some(&Tok::Close) {
                return err(self.pos(), "expected )");
            }
            self.at += 1;
            return Ok(e);
        }
        let left = self.value()?;
        if let Some(&Tok::Cmp(op, ci)) = self.peek() {
            self.at += 1;
            let right = self.value()?;
            return Ok(Expr::Cmp(op, ci, left, right));
        }
        Ok(Expr::Value(left))
    }

    fn value(&mut self) -> Result<Operand, Error> {
        let at = self.pos();
        let v = match self.peek() {
            Some(Tok::Sym(p, n)) => Operand::Sym(*p, n.clone()),
            Some(Tok::Str(s)) => Operand::Str(s.clone()),
            Some(Tok::Int(i)) => Operand::Int(*i),
            Some(t) => return err(at, format!("expected a value, found {}", describe(t))),
            None => return err(at, "expected a value, found the end"),
        };
        self.at += 1;
        Ok(v)
    }
}

fn describe(t: &Tok) -> &'static str {
    match t {
        Tok::Cmp(..) => "a comparison operator",
        Tok::Not => "NOT",
        Tok::Logic(_) => "a logical operator",
        Tok::Open => "(",
        Tok::Close => ")",
        _ => "a value",
    }
}

/// A parsed condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    expr: Option<Expr>,
}

impl Condition {
    /// Parses `text` (empty or blank: no condition, always true).
    pub fn parse(text: &str) -> Result<Condition, Error> {
        let toks = lex(text)?;
        if toks.is_empty() {
            return Ok(Condition { expr: None });
        }
        let mut p = Parser { toks, at: 0, len: text.chars().count() };
        let e = p.level(0)?;
        if p.at != p.toks.len() {
            let what = describe(&p.toks[p.at].1);
            return err(p.pos(), format!("unexpected {what} after a complete expression"));
        }
        Ok(Condition { expr: Some(e) })
    }

    /// Whether the condition is empty.
    pub fn is_empty(&self) -> bool {
        self.expr.is_none()
    }

    pub fn evaluate(&self, ctx: &dyn Context) -> bool {
        self.expr.as_ref().is_none_or(|e| eval(e, ctx))
    }
}

/// Parses and evaluates `text` against `ctx`.
pub fn evaluate(text: &str, ctx: &dyn Context) -> Result<bool, Error> {
    Ok(Condition::parse(text)?.evaluate(ctx))
}

#[derive(Debug)]
enum Val {
    Str(String),
    Int(i32),
}

fn fetch(o: &Operand, ctx: &dyn Context) -> Val {
    let state = |s: Option<i32>| s.map_or(Val::Str(String::new()), Val::Int);
    match o {
        Operand::Str(s) => Val::Str(s.clone()),
        Operand::Int(i) => Val::Int(*i),
        Operand::Sym(0, n) => Val::Str(ctx.property(n).unwrap_or_default()),
        Operand::Sym(b'%', n) => Val::Str(ctx.environment(n).unwrap_or_default()),
        Operand::Sym(b'$', n) => state(ctx.component_action(n)),
        Operand::Sym(b'?', n) => state(ctx.component_installed(n)),
        Operand::Sym(b'&', n) => state(ctx.feature_action(n)),
        Operand::Sym(_, n) => state(ctx.feature_installed(n)),
    }
}

/// A string's integer value when its whole text is a decimal integer.
pub fn as_integer(s: &str) -> Option<i32> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn int_cmp(op: Cmp, a: i32, b: i32) -> bool {
    match op {
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
        Cmp::Contains => a & b != 0,
        Cmp::Starts => ((a as u32) >> 16) as i32 == b,
        Cmp::Ends => (a & 0xffff) == b,
    }
}

fn str_cmp(op: Cmp, ci: bool, a: &str, b: &str) -> bool {
    let (a, b): (String, String) =
        if ci { (a.to_lowercase(), b.to_lowercase()) } else { (a.into(), b.into()) };
    match op {
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
        Cmp::Contains => a.contains(b.as_str()),
        Cmp::Starts => a.starts_with(b.as_str()),
        Cmp::Ends => a.ends_with(b.as_str()),
    }
}

/// Compares two values by the rules in the module documentation.
fn compare(op: Cmp, ci: bool, l: Val, r: Val) -> bool {
    match (l, r) {
        (Val::Int(a), Val::Int(b)) => int_cmp(op, a, b),
        (Val::Str(a), Val::Str(b)) => str_cmp(op, ci, &a, &b),
        (Val::Int(a), Val::Str(s)) => match as_integer(&s) {
            Some(b) => int_cmp(op, a, b),
            None => op == Cmp::Ne,
        },
        (Val::Str(s), Val::Int(b)) => match as_integer(&s) {
            Some(a) => int_cmp(op, a, b),
            None => op == Cmp::Ne,
        },
    }
}

fn eval(e: &Expr, ctx: &dyn Context) -> bool {
    match e {
        Expr::Value(o) => match fetch(o, ctx) {
            Val::Str(s) => !s.is_empty(),
            Val::Int(i) => i != 0,
        },
        Expr::Cmp(op, ci, l, r) => compare(*op, *ci, fetch(l, ctx), fetch(r, ctx)),
        Expr::Not(e) => !eval(e, ctx),
        Expr::Logic(op, l, r) => {
            let (a, b) = (eval(l, ctx), eval(r, ctx));
            match op {
                Logic::And => a && b,
                Logic::Or => a || b,
                Logic::Xor => a != b,
                Logic::Eqv => a == b,
                Logic::Imp => !a || b,
            }
        }
    }
}

#[cfg(test)]
mod tests;

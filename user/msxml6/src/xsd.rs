//! XSD (W3C XML Schema 1.0) for MSXML 6's schema cache
//! (XMLSchemaCache60) and validation: a schema document (a `Tree`)
//! compiled into its components, and an instance validated against the
//! components of every schema in a cache (`validate`).
//!
//! The subset is what Click-to-Run's App-V pieces give MSXML (the
//! schemas App-V's DLLs carry for package manifests, deployment and user
//! configurations, connection groups and file system metadata) and what
//! Wine's msxml3 tests use: global and local element declarations
//! (`name`, `type` or an anonymous type, `ref`, `minOccurs`,
//! `maxOccurs`, `form`, `default`, `fixed`), named and anonymous complex
//! types (`sequence`, `choice` and `all` with occurrence bounds, `any`,
//! `attribute` with `use`, `default`, `fixed`, `ref`, `form`,
//! `anyAttribute`, `mixed`, `simpleContent` and `complexContent`
//! extension), simple types derived by restriction (the facets
//! `enumeration`, `pattern`, `length`, `minLength`, `maxLength`,
//! `minInclusive`, `maxInclusive`, `minExclusive`, `maxExclusive`,
//! `whiteSpace`), the identity constraints `unique` and `key` (with
//! XSD's restricted XPath), `import` (resolved in the cache: MSXML 6
//! loads imports "lax"), annotations (ignored), and the built-in types
//! in `simple.rs`.
//!
//! Anything else in a schema (include, redefine, groups, attribute
//! groups, keyref, list and union types, complex content restriction,
//! substitution groups, other facets, unknown built-in types) is a
//! compile error naming the construct (`Error::Unsupported`): the
//! schema is refused (the cache's add fails, recorded) instead of a
//! document passing a validation gala did not do.

use alloc::string::String;
use alloc::vec::Vec;

use crate::dom::{Kind, Tree};

pub mod regex;
pub mod simple;
pub mod validate;

pub use simple::Builtin;

pub const XS: &str = "http://www.w3.org/2001/XMLSchema";
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// XDR's namespace (MSXML 6 refuses XDR schemas).
const XDR: &str = "urn:schemas-microsoft-com:xml-data";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QName {
    pub ns: String,
    pub local: String,
}

impl QName {
    fn new(ns: &str, local: &str) -> QName {
        QName { ns: ns.into(), local: local.into() }
    }
}

/// A type: built in, named (resolved through the cache), or one of this
/// schema's anonymous types.
#[derive(Clone, Debug)]
pub enum TypeRef {
    Builtin(Builtin),
    Named(QName),
    Local(usize),
}

#[derive(Clone, Debug)]
pub enum Type {
    Simple(SimpleType),
    Complex(ComplexType),
}

#[derive(Clone, Debug, Default)]
pub struct Facets {
    pub enums: Vec<String>,
    /// Patterns of one derivation step are alternatives; each step's
    /// must hold (XSD 4.3.4.3): one Vec per step.
    pub patterns: Vec<Vec<regex::Regex>>,
    pub length: Option<usize>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    /// Bounds of integer types (min/max, inclusive or not).
    pub min: Option<(i128, bool)>,
    pub max: Option<(i128, bool)>,
    pub white: Option<simple::WhiteSpace>,
}

/// A simple type derived by restriction from `base`.
#[derive(Clone, Debug)]
pub struct SimpleType {
    pub base: TypeRef,
    pub facets: Facets,
}

#[derive(Clone, Debug)]
pub struct ComplexType {
    pub mixed: bool,
    /// The base an extension adds to (complex content: its particle
    /// first and its attributes; simple content: the value's type).
    pub base: Option<TypeRef>,
    pub simple_content: bool,
    /// The own content model (None: empty).
    pub particle: Option<Particle>,
    pub attrs: Vec<AttrUse>,
    pub any_attr: Option<Wildcard>,
}

#[derive(Clone, Debug)]
pub struct AttrDecl {
    pub name: QName,
    pub typ: Option<TypeRef>,
    pub default: Option<String>,
    pub fixed: Option<String>,
}

#[derive(Clone, Debug)]
pub enum AttrTerm {
    Decl(AttrDecl),
    Ref(QName),
}

#[derive(Clone, Debug)]
pub struct AttrUse {
    pub term: AttrTerm,
    pub required: bool,
    pub prohibited: bool,
    /// The use's own default and fixed values (over the declaration's).
    pub default: Option<String>,
    pub fixed: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ElementDecl {
    pub name: QName,
    /// None: anyType (an element with neither a type nor a type of its
    /// own).
    pub typ: Option<TypeRef>,
    pub default: Option<String>,
    pub fixed: Option<String>,
    pub is_abstract: bool,
    pub idcs: Vec<Idc>,
}

#[derive(Clone, Debug)]
pub enum ElemTerm {
    /// One of this schema's declarations (`Schema::elems`).
    Decl(usize),
    Ref(QName),
}

#[derive(Clone, Debug)]
pub struct Particle {
    pub min: u32,
    /// u32::MAX: unbounded.
    pub max: u32,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub enum Term {
    Element(ElemTerm),
    Seq(Vec<Particle>),
    Choice(Vec<Particle>),
    All(Vec<Particle>),
    Any(Wildcard),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Process {
    Strict,
    Lax,
    Skip,
}

#[derive(Clone, Debug)]
pub enum NsConstraint {
    Any,
    /// Not this namespace and not no namespace (`##other`).
    Other(String),
    /// These namespaces ("" for no namespace).
    List(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct Wildcard {
    pub ns: NsConstraint,
    pub process: Process,
}

impl Wildcard {
    pub fn allows(&self, ns: &str) -> bool {
        match &self.ns {
            NsConstraint::Any => true,
            NsConstraint::Other(t) => !ns.is_empty() && ns != t,
            NsConstraint::List(l) => l.iter().any(|n| n == ns),
        }
    }
}

/// One step of an identity constraint's path.
#[derive(Clone, Debug)]
pub enum Step {
    /// `.`
    Current,
    /// A child element: None for `*` (any name in that namespace, or
    /// any at all when the namespace is None too).
    Child(Option<String>, Option<String>),
    Attribute(QName),
}

/// A path of a selector or field: `.//` first or not, then steps.
#[derive(Clone, Debug)]
pub struct Path {
    pub descendants: bool,
    pub steps: Vec<Step>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdcKind {
    Unique,
    Key,
}

/// An identity constraint (`unique` or `key`).
#[derive(Clone, Debug)]
pub struct Idc {
    pub kind: IdcKind,
    pub name: QName,
    /// Alternatives (`|`).
    pub selector: Vec<Path>,
    /// Each field's alternatives.
    pub fields: Vec<Vec<Path>>,
}

/// One schema document, compiled.
#[derive(Clone, Debug)]
pub struct Schema {
    pub target: String,
    pub elems: Vec<ElementDecl>,
    /// Global elements: (local name, index in `elems`).
    pub global_elems: Vec<(String, usize)>,
    pub types: Vec<Type>,
    /// Named types: (local name, index in `types`).
    pub named_types: Vec<(String, usize)>,
    pub attrs: Vec<AttrDecl>,
    /// The namespaces it imports.
    pub imports: Vec<String>,
}

impl Schema {
    pub fn element(&self, local: &str) -> Option<&ElementDecl> {
        self.global_elems.iter().find(|(n, _)| n == local).map(|&(_, i)| &self.elems[i])
    }

    pub fn named_type(&self, local: &str) -> Option<&Type> {
        self.named_types.iter().find(|(n, _)| n == local).map(|&(_, i)| &self.types[i])
    }

    pub fn attribute(&self, local: &str) -> Option<&AttrDecl> {
        self.attrs.iter().find(|a| a.name.local == local)
    }
}

/// Why a schema did not compile.
#[derive(Debug)]
pub enum Error {
    /// Not an XSD schema (the root is not xs:schema).
    NotSchema,
    /// An XDR schema (MSXML 6 has none).
    Xdr,
    /// A construct gala does not implement: its name (`xs:redefine`, a
    /// facet, a built-in type, a pattern's escape...).
    Unsupported(String),
    /// A schema that is not valid (a message).
    Invalid(String),
}

fn unsupported<T>(what: &str) -> Result<T, Error> {
    let mut s = String::from(what);
    s.truncate(80);
    Err(Error::Unsupported(s))
}

fn invalid<T>(what: &str, name: &str) -> Result<T, Error> {
    let mut s = String::from(what);
    s.push_str(name);
    Err(Error::Invalid(s))
}

struct Compiler<'t> {
    t: &'t Tree,
    s: Schema,
    qualified_elems: bool,
    qualified_attrs: bool,
}

/// Compiles the schema document `t`.
pub fn compile(t: &Tree) -> Result<Schema, Error> {
    let Some(root) = t.root() else { return Err(Error::NotSchema) };
    let r = t.node(root);
    if r.ns == XDR {
        return Err(Error::Xdr);
    }
    if r.ns != XS || r.local() != "schema" {
        return Err(Error::NotSchema);
    }
    let mut c = Compiler {
        t,
        s: Schema {
            target: attr(t, root, "targetNamespace").unwrap_or("").into(),
            elems: Vec::new(),
            global_elems: Vec::new(),
            types: Vec::new(),
            named_types: Vec::new(),
            attrs: Vec::new(),
            imports: Vec::new(),
        },
        qualified_elems: attr(t, root, "elementFormDefault") == Some("qualified"),
        qualified_attrs: attr(t, root, "attributeFormDefault") == Some("qualified"),
    };
    for k in xs_children(t, root) {
        let name = || attr(t, k, "name").unwrap_or("");
        match t.node(k).local() {
            "annotation" | "notation" => {}
            "import" => {
                if attr(t, k, "schemaLocation").is_some() {
                    // A location to load: MSXML loads it; gala resolves
                    // imports in the cache only.
                    return unsupported("xs:import with schemaLocation");
                }
                c.s.imports.push(attr(t, k, "namespace").unwrap_or("").into());
            }
            "element" => {
                let decl = c.element(k, true)?;
                if c.s.element(name()).is_some() {
                    return invalid("a duplicate global element: ", name());
                }
                let i = c.s.elems.len();
                c.s.elems.push(decl);
                c.s.global_elems.push((name().into(), i));
            }
            "complexType" | "simpleType" => {
                if name().is_empty() {
                    return invalid("a global type without a name", "");
                }
                if c.s.named_type(name()).is_some() {
                    return invalid("a duplicate global type: ", name());
                }
                let ty = c.type_def(k)?;
                let i = c.s.types.len();
                c.s.types.push(ty);
                c.s.named_types.push((name().into(), i));
            }
            "attribute" => {
                let a = c.attr_decl(k, true)?;
                c.s.attrs.push(a);
            }
            other => return unsupported_xs(other),
        }
    }
    Ok(c.s)
}

fn unsupported_xs<T>(local: &str) -> Result<T, Error> {
    let mut s = String::from("xs:");
    s.push_str(local);
    unsupported(&s)
}

/// The value of the unqualified attribute `name` of element `el`.
fn attr<'t>(t: &'t Tree, el: u32, name: &str) -> Option<&'t str> {
    t.node(el)
        .attrs
        .iter()
        .map(|&a| t.node(a))
        .find(|a| a.ns.is_empty() && a.name == name)
        .map(|a| a.value.as_str())
}

/// The element children of `el` in the XSD namespace (others are
/// ignored, as XSD's foreign elements in annotations are; text is not
/// looked at).
fn xs_children(t: &Tree, el: u32) -> impl Iterator<Item = u32> + '_ {
    t.node(el).children.iter().copied().filter(move |&k| {
        let n = t.node(k);
        n.kind == Kind::Element && n.ns == XS && n.local() != "annotation"
    })
}

/// A whitespace-collapsed copy (XSD's `collapse`).
pub fn collapse(s: &str) -> String {
    let mut out = String::new();
    for w in s.split([' ', '\t', '\n', '\r']).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

fn occurs(t: &Tree, el: u32, name: &str) -> Result<u32, Error> {
    match attr(t, el, name).map(collapse) {
        None => Ok(1),
        Some(v) if v == "unbounded" && name == "maxOccurs" => Ok(u32::MAX),
        Some(v) => v.parse::<u32>().or_else(|_| invalid("a bad occurrence bound: ", &v)),
    }
}

impl Compiler<'_> {
    /// A QName in a schema attribute's value, resolved at `el`.
    fn qname(&self, el: u32, value: &str) -> Result<QName, Error> {
        let v = collapse(value);
        let (prefix, local) = v.split_once(':').unwrap_or(("", v.as_str()));
        let ns = self.t.lookup(el, prefix);
        if !prefix.is_empty() && ns.is_empty() {
            return invalid("an undeclared prefix: ", prefix);
        }
        Ok(QName::new(ns, local))
    }

    fn type_ref(&self, el: u32, value: &str) -> Result<TypeRef, Error> {
        let q = self.qname(el, value)?;
        if q.ns == XS {
            return match Builtin::from_name(&q.local) {
                Some(b) => Ok(TypeRef::Builtin(b)),
                None => {
                    let mut s = String::from("the built-in type xs:");
                    s.push_str(&q.local);
                    unsupported(&s)
                }
            };
        }
        Ok(TypeRef::Named(q))
    }

    fn add_local_type(&mut self, ty: Type) -> TypeRef {
        self.s.types.push(ty);
        TypeRef::Local(self.s.types.len() - 1)
    }

    /// The element `k` (global or local) as a declaration.
    fn element(&mut self, k: u32, global: bool) -> Result<ElementDecl, Error> {
        let t = self.t;
        for a in ["substitutionGroup"] {
            if attr(t, k, a).is_some() {
                return unsupported("xs:element substitutionGroup");
            }
        }
        if attr(t, k, "nillable").is_some_and(|v| collapse(v) == "true") {
            return unsupported("xs:element nillable");
        }
        let name = attr(t, k, "name").unwrap_or("");
        if name.is_empty() {
            return invalid("an element without a name", "");
        }
        let qualified = global
            || match attr(t, k, "form") {
                Some(f) => collapse(f) == "qualified",
                None => self.qualified_elems,
            };
        let ns = if qualified { self.s.target.clone() } else { String::new() };
        let mut typ = match attr(t, k, "type") {
            Some(v) => Some(self.type_ref(k, v)?),
            None => None,
        };
        let mut idcs = Vec::new();
        for c in xs_children(t, k) {
            match t.node(c).local() {
                "complexType" | "simpleType" => {
                    if typ.is_some() {
                        return invalid("an element with a type and a type of its own: ", name);
                    }
                    let ty = self.type_def(c)?;
                    typ = Some(self.add_local_type(ty));
                }
                "unique" => idcs.push(self.idc(c, IdcKind::Unique)?),
                "key" => idcs.push(self.idc(c, IdcKind::Key)?),
                other => return unsupported_xs(other),
            }
        }
        Ok(ElementDecl {
            name: QName { ns, local: name.into() },
            typ,
            default: attr(t, k, "default").map(String::from),
            fixed: attr(t, k, "fixed").map(String::from),
            is_abstract: attr(t, k, "abstract").is_some_and(|v| collapse(v) == "true"),
            idcs,
        })
    }

    fn idc(&mut self, k: u32, kind: IdcKind) -> Result<Idc, Error> {
        let t = self.t;
        let mut selector = None;
        let mut fields = Vec::new();
        for c in xs_children(t, k) {
            let xpath = attr(t, c, "xpath").unwrap_or("");
            match t.node(c).local() {
                "selector" => selector = Some(self.paths(c, xpath, false)?),
                "field" => fields.push(self.paths(c, xpath, true)?),
                other => return unsupported_xs(other),
            }
        }
        let Some(selector) = selector else {
            return invalid("an identity constraint without a selector", "");
        };
        if fields.is_empty() {
            return invalid("an identity constraint without fields", "");
        }
        Ok(Idc {
            kind,
            name: QName::new(&self.s.target, attr(t, k, "name").unwrap_or("")),
            selector,
            fields,
        })
    }

    /// An identity constraint's XPath (XSD 3.11.6's subset): `|`
    /// alternatives of `(.//)? step (/ step)*`, a step `.`, a name or
    /// `*` or `p:*`; a field's may end in `@name` (or `attribute::`).
    fn paths(&self, el: u32, xpath: &str, field: bool) -> Result<Vec<Path>, Error> {
        let mut out = Vec::new();
        for alt in xpath.split('|') {
            let mut a = alt.trim();
            let descendants = a.starts_with(".//");
            if descendants {
                a = &a[3..];
            }
            let mut steps = Vec::new();
            let parts: Vec<&str> = a.split('/').map(str::trim).collect();
            for (i, p) in parts.iter().enumerate() {
                let last = i + 1 == parts.len();
                let p = p.strip_prefix("child::").unwrap_or(p);
                if p == "." {
                    steps.push(Step::Current);
                } else if let Some(n) =
                    p.strip_prefix('@').or_else(|| p.strip_prefix("attribute::"))
                {
                    if !field || !last {
                        return invalid("an attribute step in a selector: ", xpath);
                    }
                    if n.contains('*') {
                        return unsupported("an identity constraint's attribute wildcard");
                    }
                    steps.push(Step::Attribute(self.qname(el, n)?));
                } else if p == "*" {
                    steps.push(Step::Child(None, None));
                } else if let Some(prefix) = p.strip_suffix(":*") {
                    let q = self.qname(el, &{
                        let mut s = String::from(prefix);
                        s.push_str(":x");
                        s
                    })?;
                    steps.push(Step::Child(Some(q.ns), None));
                } else if !is_qname(p) {
                    return invalid("an identity constraint path XSD does not allow: ", xpath);
                } else {
                    let q = self.qname(el, p)?;
                    steps.push(Step::Child(Some(q.ns), Some(q.local)));
                }
            }
            out.push(Path { descendants, steps });
        }
        Ok(out)
    }

    /// A complexType or simpleType element.
    fn type_def(&mut self, k: u32) -> Result<Type, Error> {
        match self.t.node(k).local() {
            "simpleType" => Ok(Type::Simple(self.simple_type(k)?)),
            _ => Ok(Type::Complex(self.complex_type(k)?)),
        }
    }

    fn simple_type(&mut self, k: u32) -> Result<SimpleType, Error> {
        let t = self.t;
        let mut out = None;
        for c in xs_children(t, k) {
            match t.node(c).local() {
                "restriction" => out = Some(self.restriction(c)?),
                other => return unsupported_xs(other),
            }
        }
        out.ok_or(Error::Invalid("a simple type without a derivation".into()))
    }

    /// A simple type's restriction (or a simple content's).
    fn restriction(&mut self, k: u32) -> Result<SimpleType, Error> {
        let t = self.t;
        let mut base = match attr(t, k, "base") {
            Some(b) => Some(self.type_ref(k, b)?),
            None => None,
        };
        let mut f = Facets::default();
        let mut patterns = Vec::new();
        for c in xs_children(t, k) {
            let value = attr(t, c, "value").unwrap_or("");
            let num = |v: &str| {
                collapse(v).parse::<usize>().or_else(|_| invalid("a bad facet value: ", v))
            };
            let int = |v: &str| {
                collapse(v)
                    .parse::<i128>()
                    .map_err(|_| Error::Unsupported("a non-integer bound facet".into()))
            };
            match t.node(c).local() {
                "simpleType" => {
                    if base.is_some() {
                        return invalid("a restriction with a base and a type of its own", "");
                    }
                    let ty = self.simple_type(c)?;
                    base = Some(self.add_local_type(Type::Simple(ty)));
                }
                "enumeration" => f.enums.push(value.into()),
                "pattern" => match regex::Regex::compile(value) {
                    Ok(r) => patterns.push(r),
                    Err(why) => {
                        let mut s = String::from("xs:pattern: ");
                        s.push_str(&why);
                        return if why.contains("not supported") {
                            unsupported(&s)
                        } else {
                            Err(Error::Invalid(s))
                        };
                    }
                },
                "length" => f.length = Some(num(value)?),
                "minLength" => f.min_length = Some(num(value)?),
                "maxLength" => f.max_length = Some(num(value)?),
                "minInclusive" => f.min = Some((int(value)?, true)),
                "minExclusive" => f.min = Some((int(value)?, false)),
                "maxInclusive" => f.max = Some((int(value)?, true)),
                "maxExclusive" => f.max = Some((int(value)?, false)),
                "whiteSpace" => {
                    f.white = Some(match collapse(value).as_str() {
                        "preserve" => simple::WhiteSpace::Preserve,
                        "replace" => simple::WhiteSpace::Replace,
                        "collapse" => simple::WhiteSpace::Collapse,
                        _ => return invalid("a bad whiteSpace: ", value),
                    })
                }
                other => {
                    let mut s = String::from("the facet xs:");
                    s.push_str(other);
                    return unsupported(&s);
                }
            }
        }
        if !patterns.is_empty() {
            f.patterns.push(patterns);
        }
        let Some(base) = base else {
            return invalid("a restriction without a base", "");
        };
        Ok(SimpleType { base, facets: f })
    }

    fn complex_type(&mut self, k: u32) -> Result<ComplexType, Error> {
        let t = self.t;
        let mut ct = ComplexType {
            mixed: attr(t, k, "mixed").is_some_and(|v| collapse(v) == "true"),
            base: None,
            simple_content: false,
            particle: None,
            attrs: Vec::new(),
            any_attr: None,
        };
        for c in xs_children(t, k) {
            match t.node(c).local() {
                "simpleContent" | "complexContent" => {
                    let simple = t.node(c).local() == "simpleContent";
                    if attr(t, c, "mixed").is_some_and(|v| collapse(v) == "true") {
                        ct.mixed = true;
                    }
                    let mut derivation = None;
                    for d in xs_children(t, c) {
                        match t.node(d).local() {
                            "extension" => derivation = Some(d),
                            "restriction" if simple => {
                                return unsupported("xs:simpleContent restriction");
                            }
                            "restriction" => return unsupported("xs:complexContent restriction"),
                            other => return unsupported_xs(other),
                        }
                    }
                    let Some(d) = derivation else {
                        return invalid("a content without a derivation", "");
                    };
                    let Some(base) = attr(t, d, "base") else {
                        return invalid("an extension without a base", "");
                    };
                    ct.base = Some(self.type_ref(d, base)?);
                    ct.simple_content = simple;
                    self.content(d, &mut ct, simple)?;
                }
                _ => {}
            }
        }
        if ct.base.is_none() {
            self.content(k, &mut ct, false)?;
        }
        Ok(ct)
    }

    /// The particle and attributes in `k` (a complexType or an
    /// extension).
    fn content(&mut self, k: u32, ct: &mut ComplexType, simple: bool) -> Result<(), Error> {
        let t = self.t;
        for c in xs_children(t, k) {
            match t.node(c).local() {
                "sequence" | "choice" | "all" if !simple => {
                    if ct.particle.is_some() {
                        return invalid("a type with two content models", "");
                    }
                    ct.particle = Some(self.particle(c)?);
                }
                "attribute" => {
                    let u = self.attr_use(c)?;
                    ct.attrs.push(u);
                }
                "anyAttribute" => ct.any_attr = Some(self.wildcard(c)?),
                "simpleContent" | "complexContent" => {}
                other => return unsupported_xs(other),
            }
        }
        Ok(())
    }

    fn wildcard(&self, k: u32) -> Result<Wildcard, Error> {
        let t = self.t;
        let target = &self.s.target;
        let ns = match attr(t, k, "namespace").map(collapse).as_deref() {
            None | Some("##any") => NsConstraint::Any,
            Some("##other") => NsConstraint::Other(target.clone()),
            Some(list) => NsConstraint::List(
                list.split(' ')
                    .map(|n| match n {
                        "##targetNamespace" => target.clone(),
                        "##local" => String::new(),
                        other => other.into(),
                    })
                    .collect(),
            ),
        };
        let process = match attr(t, k, "processContents").map(collapse).as_deref() {
            None | Some("strict") => Process::Strict,
            Some("lax") => Process::Lax,
            Some("skip") => Process::Skip,
            Some(other) => return invalid("a bad processContents: ", other),
        };
        Ok(Wildcard { ns, process })
    }

    /// A sequence, choice, all, element or any, with its bounds.
    fn particle(&mut self, k: u32) -> Result<Particle, Error> {
        let t = self.t;
        let min = occurs(t, k, "minOccurs")?;
        let max = occurs(t, k, "maxOccurs")?;
        if max < min {
            return invalid("maxOccurs below minOccurs", "");
        }
        let local = t.node(k).local();
        let term = match local {
            "element" => {
                if let Some(r) = attr(t, k, "ref") {
                    Term::Element(ElemTerm::Ref(self.qname(k, r)?))
                } else {
                    let decl = self.element(k, false)?;
                    self.s.elems.push(decl);
                    Term::Element(ElemTerm::Decl(self.s.elems.len() - 1))
                }
            }
            "any" => Term::Any(self.wildcard(k)?),
            "sequence" | "choice" | "all" => {
                let mut items = Vec::new();
                for c in xs_children(t, k) {
                    match t.node(c).local() {
                        "element" | "any" | "sequence" | "choice" => {
                            if local == "all" && t.node(c).local() != "element" {
                                return invalid("xs:all holding more than elements", "");
                            }
                            items.push(self.particle(c)?);
                        }
                        other => return unsupported_xs(other),
                    }
                }
                match local {
                    "sequence" => Term::Seq(items),
                    "choice" => Term::Choice(items),
                    _ => {
                        if max > 1 || items.iter().any(|p| p.max > 1) {
                            return invalid("xs:all with maxOccurs above 1", "");
                        }
                        Term::All(items)
                    }
                }
            }
            other => return unsupported_xs(other),
        };
        Ok(Particle { min, max, term })
    }

    fn attr_decl(&mut self, k: u32, global: bool) -> Result<AttrDecl, Error> {
        let t = self.t;
        let name = attr(t, k, "name").unwrap_or("");
        if name.is_empty() {
            return invalid("an attribute without a name", "");
        }
        let qualified = global
            || match attr(t, k, "form") {
                Some(f) => collapse(f) == "qualified",
                None => self.qualified_attrs,
            };
        let mut typ = match attr(t, k, "type") {
            Some(v) => Some(self.type_ref(k, v)?),
            None => None,
        };
        for c in xs_children(t, k) {
            match t.node(c).local() {
                "simpleType" if typ.is_none() => {
                    let ty = self.simple_type(c)?;
                    typ = Some(self.add_local_type(Type::Simple(ty)));
                }
                other => return unsupported_xs(other),
            }
        }
        Ok(AttrDecl {
            name: QName::new(if qualified { &self.s.target } else { "" }, name),
            typ,
            default: attr(t, k, "default").map(String::from),
            fixed: attr(t, k, "fixed").map(String::from),
        })
    }

    fn attr_use(&mut self, k: u32) -> Result<AttrUse, Error> {
        let t = self.t;
        let use_ = attr(t, k, "use").map(collapse);
        let (term, default, fixed) = if let Some(r) = attr(t, k, "ref") {
            let q = self.qname(k, r)?;
            (
                AttrTerm::Ref(q),
                attr(t, k, "default").map(String::from),
                attr(t, k, "fixed").map(String::from),
            )
        } else {
            (AttrTerm::Decl(self.attr_decl(k, false)?), None, None)
        };
        Ok(AttrUse {
            term,
            required: use_.as_deref() == Some("required"),
            prohibited: use_.as_deref() == Some("prohibited"),
            default,
            fixed,
        })
    }
}

fn is_qname(s: &str) -> bool {
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && p.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    match s.split_once(':') {
        Some((a, b)) => ok(a) && ok(b),
        None => ok(s),
    }
}

/// The schemas of a cache, as validation sees them: each namespace's
/// compiled schema documents (MSXML 6 merges a namespace's documents).
pub struct Set<'a> {
    pub schemas: Vec<&'a Schema>,
}

impl<'a> Set<'a> {
    pub fn has_namespace(&self, ns: &str) -> bool {
        self.schemas.iter().any(|s| s.target == ns)
    }

    pub fn element(&self, q: &QName) -> Option<(&'a Schema, &'a ElementDecl)> {
        self.schemas
            .iter()
            .filter(|s| s.target == q.ns)
            .find_map(|s| s.element(&q.local).map(|e| (*s, e)))
    }

    pub fn named_type(&self, q: &QName) -> Option<(&'a Schema, &'a Type)> {
        self.schemas
            .iter()
            .filter(|s| s.target == q.ns)
            .find_map(|s| s.named_type(&q.local).map(|t| (*s, t)))
    }

    pub fn attribute(&self, q: &QName) -> Option<(&'a Schema, &'a AttrDecl)> {
        self.schemas
            .iter()
            .filter(|s| s.target == q.ns)
            .find_map(|s| s.attribute(&q.local).map(|a| (*s, a)))
    }

    /// The first name a schema refers to that no schema here declares
    /// (MSXML 6's compile step "resolve names"), as text.
    pub fn unresolved(&self) -> Option<String> {
        for s in &self.schemas {
            let mut types: Vec<&TypeRef> = Vec::new();
            let mut attrs: Vec<&QName> = Vec::new();
            let mut elems: Vec<QName> = Vec::new();
            for ty in &s.types {
                match ty {
                    Type::Simple(st) => types.push(&st.base),
                    Type::Complex(ct) => {
                        types.extend(&ct.base);
                        for a in &ct.attrs {
                            match &a.term {
                                AttrTerm::Decl(d) => types.extend(&d.typ),
                                AttrTerm::Ref(q) => attrs.push(q),
                            }
                        }
                        if let Some(p) = &ct.particle {
                            element_refs(p, &mut elems);
                        }
                    }
                }
            }
            types.extend(s.attrs.iter().filter_map(|a| a.typ.as_ref()));
            types.extend(s.elems.iter().filter_map(|e| e.typ.as_ref()));
            for r in types {
                if let TypeRef::Named(q) = r
                    && self.named_type(q).is_none()
                {
                    return Some(describe("type", q));
                }
            }
            if let Some(q) = attrs.into_iter().find(|q| self.attribute(q).is_none()) {
                return Some(describe("attribute", q));
            }
            if let Some(q) = elems.into_iter().find(|q| self.element(q).is_none()) {
                return Some(describe("element", &q));
            }
        }
        None
    }
}

fn element_refs(p: &Particle, out: &mut Vec<QName>) {
    match &p.term {
        Term::Element(ElemTerm::Ref(q)) => out.push(q.clone()),
        Term::Seq(v) | Term::Choice(v) | Term::All(v) => {
            v.iter().for_each(|c| element_refs(c, out))
        }
        _ => {}
    }
}

fn describe(what: &str, q: &QName) -> String {
    let mut s = String::from(what);
    s.push_str(" '");
    s.push_str(&q.local);
    s.push_str("' of namespace '");
    s.push_str(&q.ns);
    s.push('\'');
    s
}

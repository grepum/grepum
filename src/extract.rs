//! Tree-sitter based fact extraction: one recursive walker that hands every
//! node to the file's language (`lang::*::visit`), which records definitions,
//! calls, imports and type bindings. Anything a language doesn't claim is
//! walked generically. The walker's helpers here are language-neutral.

use crate::facts::*;
use crate::lang::{self, Language};
use std::cell::RefCell;
use std::collections::HashMap;
use tree_sitter::{Node, Parser};

thread_local! {
    static PARSERS: RefCell<HashMap<Lang, Parser>> = RefCell::new(HashMap::new());
}

pub fn extract(lang: Lang, raw: &[u8]) -> FileFacts {
    let support = lang::support(lang);
    let (rewritten, attrs, extra_defs) = match support.preprocess(raw) {
        Some(p) => (Some(p.src), p.attrs, p.defs),
        None => (None, HashMap::new(), vec![]),
    };
    let src: &[u8] = rewritten.as_deref().unwrap_or(raw);
    let tree = parse_with(lang, src);
    let lines = src.iter().filter(|b| **b == b'\n').count() as u32 + 1;
    let mut w = Walker {
        support,
        src,
        facts: FileFacts { lang, defs: vec![], refs: vec![], imports: vec![], bindings: vec![], exports: vec![], lines },
        scopes: vec![],
        owners: vec![],
        cur_line: 0,
        attrs,
    };
    if let Some(tree) = tree {
        w.visit(tree.root_node());
    }
    w.facts.defs.extend(extra_defs);
    w.facts
}

fn parse_with(lang: Lang, src: &[u8]) -> Option<tree_sitter::Tree> {
    PARSERS.with(|p| {
        let mut map = p.borrow_mut();
        let parser = map.entry(lang).or_insert_with(|| {
            let mut parser = Parser::new();
            parser.set_language(&lang::support(lang).grammar(lang)).expect("grammar ABI mismatch");
            parser
        });
        parser.parse(src, None)
    })
}

/// Parse a file the way extraction does: the tree, and the source it was
/// parsed from when the language rewrote it first (same byte offsets and lines).
pub fn parse(lang: Lang, raw: &[u8]) -> (Option<tree_sitter::Tree>, Option<Vec<u8>>) {
    let rewritten = lang::support(lang).preprocess(raw).map(|p| p.src);
    let tree = parse_with(lang, rewritten.as_deref().unwrap_or(raw));
    (tree, rewritten)
}

/// FNV-1a over the non-whitespace bytes, so reformatting isn't a change.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes.iter().filter(|b| !b.is_ascii_whitespace()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Walks one file's syntax tree, collecting its facts. Languages read and
/// extend its state through the helpers below.
pub struct Walker<'a> {
    support: &'static dyn Language,
    src: &'a [u8],
    pub facts: FileFacts,
    /// Enclosing definitions, innermost last.
    pub scopes: Vec<u32>,
    /// Enclosing type names (for `self`/`this`), innermost last.
    pub owners: Vec<String>,
    /// Start line of the node being handled.
    cur_line: u32,
    /// Attributes (Unreal reflection specifiers) keyed by the line of the declaration they annotate.
    attrs: HashMap<u32, Vec<String>>,
}

pub fn children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

pub fn line(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

impl<'a> Walker<'a> {
    pub fn text(&self, n: Node) -> &'a str {
        std::str::from_utf8(&self.src[n.byte_range()]).unwrap_or("")
    }

    pub fn visit_children(&mut self, n: Node) {
        for ch in children(n) {
            self.visit(ch);
        }
    }

    pub fn visit_opt(&mut self, n: Option<Node>) {
        if let Some(n) = n {
            self.visit(n);
        }
    }

    pub fn visit(&mut self, n: Node) {
        let outer = std::mem::replace(&mut self.cur_line, line(n));
        let handled = self.support.visit(self, n);
        if !handled {
            self.visit_children(n);
        }
        self.cur_line = outer;
    }

    // ---------- shared helpers ----------

    pub fn scope(&self) -> Option<u32> {
        self.scopes.last().copied()
    }

    pub fn binding_scope(&self) -> BindingScope {
        match self.scope() {
            Some(s) => BindingScope::Def(s),
            None => BindingScope::File,
        }
    }

    pub fn enclosing_type_def(&self) -> Option<&Def> {
        self.scope().map(|s| &self.facts.defs[s as usize]).filter(|d| d.kind.is_type())
    }

    /// Drop attributes on `n`'s line that annotate something that isn't a definition.
    pub fn discard_attrs(&mut self, n: Node) {
        self.attrs.remove(&line(n));
    }

    pub fn add_def(&mut self, name: &str, kind: DefKind, node: Node, owner: Option<String>, ret: Option<String>) -> u32 {
        self.add_def_full(name, kind, node, owner, ret, false)
    }

    pub fn add_def_full(&mut self, name: &str, kind: DefKind, node: Node, owner: Option<String>, ret: Option<String>, decl: bool) -> u32 {
        let idx = self.facts.defs.len() as u32;
        let attrs = self.attrs.remove(&line(node)).unwrap_or_default();
        self.facts.defs.push(Def {
            name: name.to_string(),
            kind,
            line: line(node),
            end_line: node.end_position().row as u32 + 1,
            parent: self.scope(),
            owner,
            ret,
            decl,
            attrs,
            body: fingerprint(&self.src[node.byte_range()]),
        });
        idx
    }

    /// Run `body` with `idx` as the innermost scope (and optionally a new owner type).
    pub fn in_scope(&mut self, idx: Option<u32>, owner: Option<String>, body: Option<Node>) {
        if let Some(i) = idx {
            self.scopes.push(i);
        }
        let pushed_owner = owner.is_some();
        if let Some(o) = owner {
            self.owners.push(o);
        }
        self.visit_opt(body);
        if pushed_owner {
            self.owners.pop();
        }
        if idx.is_some() {
            self.scopes.pop();
        }
    }

    pub fn add_ref(&mut self, kind: RefKind, name: &str, qualifier: Option<String>, node: Node) {
        if name.is_empty() {
            return;
        }
        self.facts.refs.push(Ref {
            kind,
            name: name.to_string(),
            qualifier,
            line: line(node),
            scope: self.scope(),
            owner: self.owners.last().cloned(),
        });
    }

    pub fn add_inherit(&mut self, base: Node, subject: &str) {
        let (name, qualifier) = self.split_callee(base);
        if let Some(name) = name {
            self.facts.refs.push(Ref {
                kind: RefKind::Inherit,
                name,
                qualifier,
                line: line(base),
                scope: self.scope(),
                owner: Some(subject.to_string()),
            });
        }
    }

    pub fn add_binding(&mut self, scope: BindingScope, name: &str, ty: TypeHint) {
        if name.is_empty() {
            return;
        }
        // Bound while handling the function itself: a parameter.
        let line = match scope {
            BindingScope::Def(s) if self.facts.defs[s as usize].line == self.cur_line => 0,
            _ => self.cur_line,
        };
        self.facts.bindings.push(Binding { scope, name: name.to_string(), ty, line });
    }

    // Expression helpers over the node kinds grammars share (identifiers,
    // member/field/attribute access, scoped paths, calls, generic types).
    // Each language handles everything else itself in its `visit`.

    /// Text of a receiver/path expression, or `?` if it isn't a plain name chain.
    pub fn qual(&self, n: Node) -> String {
        let s = self.qual_inner(n);
        if s.contains('?') { "?".into() } else { s }
    }

    fn qual_inner(&self, n: Node) -> String {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "identifier" | "self" | "this" | "super" | "crate" | "type_identifier" | "field_identifier"
            | "property_identifier" | "package_identifier" | "private_property_identifier" => {
                self.text(n).to_string()
            }
            "attribute" => match (f("object"), f("attribute")) {
                (Some(o), Some(a)) => format!("{}.{}", self.qual_inner(o), self.text(a)),
                _ => "?".into(),
            },
            "member_expression" => match (f("object"), f("property")) {
                (Some(o), Some(p)) => format!("{}.{}", self.qual_inner(o), self.text(p)),
                _ => "?".into(),
            },
            "field_expression" => match (f("value").or(f("argument")), f("field")) {
                (Some(o), Some(p)) => format!("{}.{}", self.qual_inner(o), self.qual_inner(p)),
                _ => "?".into(),
            },
            "qualified_identifier" => match (f("scope"), f("name")) {
                (Some(sc), Some(nm)) => format!("{}::{}", self.qual_inner(sc), self.qual_inner(nm)),
                (None, Some(nm)) => self.qual_inner(nm),
                _ => "?".into(),
            },
            "namespace_identifier" | "destructor_name" => self.text(n).to_string(),
            "template_type" | "template_function" | "template_method" => {
                f("name").map(|x| self.qual_inner(x)).unwrap_or("?".into())
            }
            "selector_expression" => match (f("operand"), f("field")) {
                (Some(o), Some(p)) => format!("{}.{}", self.qual_inner(o), self.text(p)),
                _ => "?".into(),
            },
            "scoped_identifier" | "scoped_type_identifier" => match (f("path"), f("name")) {
                (Some(p), Some(nm)) => format!("{}::{}", self.qual_inner(p), self.text(nm)),
                (None, Some(nm)) => self.text(nm).to_string(),
                _ => "?".into(),
            },
            "generic_type" => f("type").map(|t| self.qual_inner(t)).unwrap_or("?".into()),
            // `make()` / `repo.get()` as a receiver: the type is whatever the call returns.
            "call" | "call_expression" => match f("function") {
                Some(func) => format!("{}()", self.qual_inner(func)),
                None => "?".into(),
            },
            "parenthesized_expression" | "non_null_expression" => {
                children(n).first().map(|c| self.qual_inner(*c)).unwrap_or("?".into())
            }
            _ => "?".into(),
        }
    }

    /// Split a callee expression into (name, qualifier).
    pub fn split_callee(&self, n: Node) -> (Option<String>, Option<String>) {
        let f = |name: &str| n.child_by_field_name(name);
        let (name_node, qual_node) = match n.kind() {
            "attribute" => (f("attribute"), f("object")),
            "member_expression" => (f("property"), f("object")),
            "field_expression" => {
                let field = f("field");
                let name = field.map(|x| if x.kind() == "template_method" { x.child_by_field_name("name").unwrap_or(x) } else { x });
                (name, f("value").or(f("argument")))
            }
            "qualified_identifier" => {
                // `A::B::name` → innermost name, qualifier `A::B`.
                let mut scopes = vec![];
                let mut cur = n;
                while cur.kind() == "qualified_identifier" {
                    if let Some(sc) = cur.child_by_field_name("scope") {
                        scopes.push(self.qual(sc));
                    }
                    match cur.child_by_field_name("name") {
                        Some(nm) => cur = nm,
                        None => break,
                    }
                }
                let (name, _) = self.split_callee(cur);
                let q = if scopes.is_empty() { None } else { Some(scopes.join("::")) };
                return (name, q);
            }
            "template_function" | "template_method" | "template_type" => {
                return f("name").map(|x| self.split_callee(x)).unwrap_or((None, None));
            }
            "destructor_name" | "namespace_identifier" => (Some(n), None),
            "selector_expression" => (f("field"), f("operand")),
            "scoped_identifier" | "scoped_type_identifier" => (f("name"), f("path")),
            "generic_type" | "generic_function" => {
                let inner = f("type").or(f("function"));
                return inner.map(|i| self.split_callee(i)).unwrap_or((None, None));
            }
            "identifier" | "type_identifier" | "property_identifier" | "field_identifier" => (Some(n), None),
            _ => (None, None),
        };
        (name_node.map(|x| self.text(x).to_string()), qual_node.map(|q| self.qual(q)))
    }

    /// Best-effort nominal type name of a type annotation node.
    pub fn type_name(&self, n: Node) -> Option<String> {
        match n.kind() {
            "string" | "string_literal" => {
                let t = self.text(n).trim_matches(|c| c == '"' || c == '\'');
                return Some(t.rsplit('.').next()?.to_string()).filter(|s| !s.is_empty());
            }
            "identifier" | "type_identifier" => {
                let t = self.text(n);
                return (!self.support.rules().wrapper_types.contains(&t)).then(|| t.to_string());
            }
            "predefined_type" | "primitive_type" | "literal_type" | "placeholder_type_specifier" | "auto" => return None,
            "template_type" => {
                // Wrapper templates forward to their argument; other templates name themselves.
                let nm = n.child_by_field_name("name").map(|x| self.text(x)).unwrap_or("");
                if !self.support.rules().wrapper_types.contains(&nm) {
                    return Some(nm.to_string()).filter(|s| !s.is_empty());
                }
                return n.child_by_field_name("arguments").and_then(|a| self.type_name(a));
            }
            "attribute" | "member_expression" | "nested_type_identifier" | "qualified_type"
            | "scoped_type_identifier" | "scoped_identifier" | "qualified_identifier" => {
                let last = n
                    .child_by_field_name("attribute")
                    .or(n.child_by_field_name("property"))
                    .or(n.child_by_field_name("name"))
                    .or_else(|| children(n).last().copied());
                return last.and_then(|l| self.type_name(l));
            }
            _ => {}
        }
        for ch in children(n) {
            if let Some(t) = self.type_name(ch) {
                return Some(t);
            }
        }
        None
    }

    /// Infer a TypeHint from an initializer expression.
    pub fn value_hint(&self, v: Node) -> Option<TypeHint> {
        match v.kind() {
            "call" | "call_expression" => {
                let func = v.child_by_field_name("function")?;
                if let Some(ty) = self.support.call_result_type(self, func) {
                    return Some(TypeHint::Named(ty));
                }
                let (name, qualifier) = self.split_callee(func);
                Some(TypeHint::CallResult { name: name?, qualifier })
            }
            "new_expression" => match v.child_by_field_name("type") {
                // C++ `new T(...)` names a type; JS `new X()` a constructor expression.
                Some(t) => Some(TypeHint::Named(self.type_name(t)?)),
                None => {
                    let (name, _) = self.split_callee(v.child_by_field_name("constructor")?);
                    Some(TypeHint::Named(name?))
                }
            },
            "struct_expression" => Some(TypeHint::Named(self.type_name(v.child_by_field_name("name")?)?)),
            "composite_literal" => Some(TypeHint::Named(self.type_name(v.child_by_field_name("type")?)?)),
            "await" | "await_expression" | "unary_expression" | "reference_expression" | "parenthesized_expression"
            | "try_expression" | "as_expression" => children(v).first().and_then(|c| self.value_hint(*c)),
            "identifier" => Some(TypeHint::Alias(self.text(v).to_string())),
            "list" | "dictionary" | "set" | "tuple" | "string" | "integer" | "float" | "true" | "false"
            | "list_comprehension" | "dictionary_comprehension" | "array" | "object" | "number"
            | "template_string" | "concatenated_string" => Some(TypeHint::Named(format!("<{}>", v.kind()))),
            _ => None,
        }
    }

    /// `self.repo = repo` inside a method: the field takes the parameter's type.
    pub fn resolve_alias_now(&self, hint: TypeHint) -> TypeHint {
        if let TypeHint::Alias(name) = &hint {
            if let Some(scope) = self.scope() {
                for b in self.facts.bindings.iter().rev() {
                    if matches!(b.scope, BindingScope::Def(s) if s == scope) && &b.name == name {
                        return b.ty.clone();
                    }
                }
            }
        }
        hint
    }

}

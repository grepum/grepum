//! Tree-sitter based fact extraction. One recursive walker, with a small
//! per-language handler that recognises definitions, calls, imports and
//! type bindings. Anything a handler doesn't claim is walked generically.

use crate::facts::*;
use std::cell::RefCell;
use std::collections::HashMap;
use tree_sitter::{Node, Parser};

thread_local! {
    static PARSERS: RefCell<HashMap<Lang, Parser>> = RefCell::new(HashMap::new());
}

pub fn extract(lang: Lang, raw: &[u8]) -> FileFacts {
    let ue = (lang == Lang::Cpp && crate::unreal::looks_like_unreal(raw)).then(|| crate::unreal::normalize(raw));
    let src: &[u8] = ue.as_ref().map(|u| u.src.as_slice()).unwrap_or(raw);
    let tree = PARSERS.with(|p| {
        let mut map = p.borrow_mut();
        let parser = map.entry(lang).or_insert_with(|| {
            let mut parser = Parser::new();
            parser.set_language(&lang.grammar()).expect("grammar ABI mismatch");
            parser
        });
        parser.parse(src, None)
    });
    let lines = src.iter().filter(|b| **b == b'\n').count() as u32 + 1;
    let mut w = Walker {
        lang,
        src,
        facts: FileFacts { lang, defs: vec![], refs: vec![], imports: vec![], bindings: vec![], lines },
        scopes: vec![],
        owners: vec![],
        attrs: ue.as_ref().map(|u| u.attrs.clone()).unwrap_or_default(),
    };
    if let Some(tree) = tree {
        w.visit(tree.root_node());
    }
    if let Some(u) = &ue {
        for (name, line) in &u.delegates {
            w.facts.defs.push(Def {
                name: name.clone(),
                kind: DefKind::Type,
                line: *line,
                end_line: *line,
                parent: None,
                owner: None,
                ret: None,
                decl: false,
                attrs: vec!["DELEGATE".into()],
                body: 0,
            });
        }
    }
    w.facts
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

/// Types that forward to the type they wrap (smart pointers, optionals). Containers
/// are deliberately absent: `items.push()` on a `Vec<Order>` is not `Order::push`.
const WRAPPER_TYPES: &[&str] = &[
    "Optional", "Option", "Union", "Box", "Arc", "Rc", "RefCell", "Cell", "Mutex", "RwLock", "Weak", "Promise",
    "Readonly", "Partial", "Awaitable", "Type", "type", "Final", "ClassVar", "Annotated", "Ref", "Pin",
    // C++ / Unreal
    "TObjectPtr", "TWeakObjectPtr", "TSoftObjectPtr", "TStrongObjectPtr", "TLazyObjectPtr", "TSharedPtr",
    "TSharedRef", "TWeakPtr", "TUniquePtr", "TOptional", "shared_ptr", "unique_ptr", "weak_ptr", "optional",
];

/// C++ templates whose result is (a pointer to) their first type argument.
const TYPE_RETURNING_TEMPLATES: &[&str] = &[
    "Cast", "CastChecked", "ExactCast", "NewObject", "CreateDefaultSubobject", "SpawnActor", "SpawnActorDeferred",
    "LoadObject", "FindObject", "GetSubsystem", "GetGameInstanceSubsystem", "GetWorldSubsystem", "FindComponentByClass",
    "GetComponentByClass", "GetPawn", "GetController", "GetPlayerState", "GetAuthGameMode", "GetGameState",
    "GetOwner", "GetOuter", "GetTypedOuter", "MakeShared", "MakeUnique", "make_shared", "make_unique",
    "static_cast", "dynamic_cast", "reinterpret_cast", "StaticCast", "StaticCastSharedPtr",
];

const FUNC_VALUE_KINDS: &[&str] =
    &["arrow_function", "function_expression", "function", "generator_function"];

struct Walker<'a> {
    lang: Lang,
    src: &'a [u8],
    facts: FileFacts,
    scopes: Vec<u32>,
    owners: Vec<String>,
    /// Unreal reflection specifiers keyed by the line of the declaration they annotate.
    attrs: HashMap<u32, Vec<String>>,
}

fn children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

fn line(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

impl<'a> Walker<'a> {
    fn text(&self, n: Node) -> &'a str {
        std::str::from_utf8(&self.src[n.byte_range()]).unwrap_or("")
    }

    fn visit_children(&mut self, n: Node) {
        for ch in children(n) {
            self.visit(ch);
        }
    }

    fn visit_opt(&mut self, n: Option<Node>) {
        if let Some(n) = n {
            self.visit(n);
        }
    }

    fn visit(&mut self, n: Node) {
        let handled = match self.lang {
            Lang::Python => self.python(n),
            Lang::TypeScript | Lang::Tsx | Lang::JavaScript => self.js(n),
            Lang::Rust => self.rust(n),
            Lang::Go => self.go(n),
            Lang::Cpp => self.cpp(n),
        };
        if !handled {
            self.visit_children(n);
        }
    }

    // ---------- shared helpers ----------

    fn scope(&self) -> Option<u32> {
        self.scopes.last().copied()
    }

    fn binding_scope(&self) -> BindingScope {
        match self.scope() {
            Some(s) => BindingScope::Def(s),
            None => BindingScope::File,
        }
    }

    fn enclosing_type_def(&self) -> Option<&Def> {
        self.scope().map(|s| &self.facts.defs[s as usize]).filter(|d| d.kind.is_type())
    }

    fn add_def(&mut self, name: &str, kind: DefKind, node: Node, owner: Option<String>, ret: Option<String>) -> u32 {
        self.add_def_full(name, kind, node, owner, ret, false)
    }

    fn add_def_full(&mut self, name: &str, kind: DefKind, node: Node, owner: Option<String>, ret: Option<String>, decl: bool) -> u32 {
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
    fn in_scope(&mut self, idx: Option<u32>, owner: Option<String>, body: Option<Node>) {
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

    fn add_ref(&mut self, kind: RefKind, name: &str, qualifier: Option<String>, node: Node) {
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

    fn add_inherit(&mut self, base: Node, subject: &str) {
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

    fn add_binding(&mut self, scope: BindingScope, name: &str, ty: TypeHint) {
        if name.is_empty() {
            return;
        }
        self.facts.bindings.push(Binding { scope, name: name.to_string(), ty });
    }

    /// Text of a receiver/path expression, or `?` if it isn't a plain name chain.
    fn qual(&self, n: Node) -> String {
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
    fn split_callee(&self, n: Node) -> (Option<String>, Option<String>) {
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
    fn type_name(&self, n: Node) -> Option<String> {
        match n.kind() {
            "string" | "string_literal" => {
                let t = self.text(n).trim_matches(|c| c == '"' || c == '\'');
                return Some(t.rsplit('.').next()?.to_string()).filter(|s| !s.is_empty());
            }
            "identifier" | "type_identifier" => {
                let t = self.text(n);
                return (!WRAPPER_TYPES.contains(&t)).then(|| t.to_string());
            }
            "predefined_type" | "primitive_type" | "literal_type" | "placeholder_type_specifier" | "auto" => return None,
            "template_type" => {
                // Wrapper templates forward to their argument; other templates name themselves.
                let nm = n.child_by_field_name("name").map(|x| self.text(x)).unwrap_or("");
                if !WRAPPER_TYPES.contains(&nm) {
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
    fn value_hint(&self, v: Node) -> Option<TypeHint> {
        match v.kind() {
            "call" | "call_expression" => {
                let func = v.child_by_field_name("function")?;
                if self.lang == Lang::Cpp {
                    let tmpl = match func.kind() {
                        "template_function" => Some(func),
                        "field_expression" => func.child_by_field_name("field").filter(|f| f.kind() == "template_method"),
                        _ => None,
                    };
                    if let Some(t) = tmpl {
                        let tname = t.child_by_field_name("name").map(|x| self.text(x)).unwrap_or("");
                        if TYPE_RETURNING_TEMPLATES.contains(&tname) {
                            if let Some(ty) = t.child_by_field_name("arguments").and_then(|a| self.type_name(a)) {
                                return Some(TypeHint::Named(ty));
                            }
                        }
                    }
                }
                let (name, qualifier) = self.split_callee(func);
                Some(TypeHint::CallResult { name: name?, qualifier })
            }
            "new_expression" if self.lang == Lang::Cpp => Some(TypeHint::Named(self.type_name(v.child_by_field_name("type")?)?)),
            "new_expression" => {
                let c = v.child_by_field_name("constructor")?;
                let (name, _) = self.split_callee(c);
                Some(TypeHint::Named(name?))
            }
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

    // ---------- Python ----------

    fn python(&mut self, n: Node) -> bool {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "class_definition" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let idx = self.add_def(name, DefKind::Class, n, None, None);
                if let Some(sup) = f("superclasses") {
                    self.scopes.push(idx);
                    for b in children(sup) {
                        if matches!(b.kind(), "identifier" | "attribute") {
                            self.add_inherit(b, name);
                        }
                    }
                    self.scopes.pop();
                }
                self.in_scope(Some(idx), Some(name.to_string()), f("body"));
                true
            }
            "function_definition" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let class = self.enclosing_type_def().map(|d| d.name.clone());
                let kind = if class.is_some() { DefKind::Method } else { DefKind::Function };
                let ret = f("return_type").and_then(|r| self.type_name(r));
                let idx = self.add_def(name, kind, n, class, ret);
                if let Some(params) = f("parameters") {
                    for p in children(params) {
                        let (pname, pty) = match p.kind() {
                            "typed_parameter" => (children(p).first().copied(), p.child_by_field_name("type")),
                            "typed_default_parameter" => (p.child_by_field_name("name"), p.child_by_field_name("type")),
                            _ => (None, None),
                        };
                        let pty = pty.map(|t| self.type_name(t).unwrap_or_else(|| format!("<{}>", self.text(t))));
                        if let (Some(pn), Some(t)) = (pname, pty) {
                            let pn = self.text(pn);
                            self.add_binding(BindingScope::Def(idx), pn, TypeHint::Named(t));
                        }
                    }
                }
                self.in_scope(Some(idx), None, f("body"));
                true
            }
            "call" => {
                if let Some(func) = f("function") {
                    let (name, qualifier) = self.split_callee(func);
                    if let Some(name) = name {
                        self.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    if func.kind() == "attribute" {
                        self.visit_opt(func.child_by_field_name("object"));
                    } else if func.kind() != "identifier" {
                        self.visit(func);
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "assignment" => {
                if let Some(left) = f("left") {
                    let hint = f("type")
                        .and_then(|t| self.type_name(t).map(TypeHint::Named))
                        .or_else(|| f("right").and_then(|r| self.value_hint(r)));
                    if let Some(hint) = hint {
                        match left.kind() {
                            "identifier" => {
                                let s = self.binding_scope();
                                self.add_binding(s, self.text(left), hint);
                            }
                            "attribute" => {
                                let obj = left.child_by_field_name("object").map(|o| self.text(o));
                                let attr = left.child_by_field_name("attribute").map(|a| self.text(a));
                                if let (Some("self"), Some(attr), Some(owner)) = (obj, attr, self.owners.last().cloned()) {
                                    let hint = self.resolve_alias_now(hint);
                                    self.add_binding(BindingScope::Owner(owner), attr, hint);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.visit_opt(f("right"));
                true
            }
            "import_statement" => {
                for ch in children(n) {
                    let (module, alias) = match ch.kind() {
                        "dotted_name" => (self.text(ch), None),
                        "aliased_import" => (
                            ch.child_by_field_name("name").map(|x| self.text(x)).unwrap_or(""),
                            ch.child_by_field_name("alias").map(|x| self.text(x).to_string()),
                        ),
                        _ => continue,
                    };
                    self.facts.imports.push(Import {
                        module: module.to_string(),
                        names: vec![],
                        alias,
                        wildcard: false,
                        reexport: true,
                        line: line(n),
                    });
                }
                true
            }
            "import_from_statement" => {
                let module = f("module_name").map(|m| self.text(m).to_string()).unwrap_or_default();
                let mut names = vec![];
                let mut wildcard = false;
                let mut c = n.walk();
                for ch in n.children_by_field_name("name", &mut c) {
                    match ch.kind() {
                        "dotted_name" => names.push(ImportName { name: self.text(ch).to_string(), alias: None }),
                        "aliased_import" => names.push(ImportName {
                            name: ch.child_by_field_name("name").map(|x| self.text(x).to_string()).unwrap_or_default(),
                            alias: ch.child_by_field_name("alias").map(|x| self.text(x).to_string()),
                        }),
                        _ => {}
                    }
                }
                if children(n).iter().any(|c| c.kind() == "wildcard_import") {
                    wildcard = true;
                }
                self.facts.imports.push(Import { module, names, alias: None, wildcard, reexport: true, line: line(n) });
                true
            }
            _ => false,
        }
    }

    /// `self.repo = repo` inside a method: the field takes the parameter's type.
    fn resolve_alias_now(&self, hint: TypeHint) -> TypeHint {
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

    // ---------- JavaScript / TypeScript ----------

    fn js_params(&mut self, params: Option<Node>, idx: u32, is_ctor: bool) {
        let Some(params) = params else { return };
        for p in children(params) {
            if !matches!(p.kind(), "required_parameter" | "optional_parameter") {
                continue;
            }
            let Some(pat) = p.child_by_field_name("pattern") else { continue };
            if pat.kind() != "identifier" {
                continue;
            }
            let Some(ty) = p.child_by_field_name("type").and_then(|t| self.type_name(t)) else { continue };
            let name = self.text(pat);
            self.add_binding(BindingScope::Def(idx), name, TypeHint::Named(ty.clone()));
            let is_property = children(p).iter().any(|c| c.kind() == "accessibility_modifier" || self.text(*c) == "readonly");
            if is_ctor && is_property {
                if let Some(owner) = self.owners.last().cloned() {
                    self.add_binding(BindingScope::Owner(owner), name, TypeHint::Named(ty));
                }
            }
        }
    }

    fn js_function(&mut self, name: &str, kind: DefKind, def_node: Node, func: Node, owner: Option<String>) {
        let ret = func.child_by_field_name("return_type").and_then(|r| self.type_name(r));
        let idx = self.add_def(name, kind, def_node, owner, ret);
        self.js_params(func.child_by_field_name("parameters"), idx, name == "constructor");
        self.in_scope(Some(idx), None, func.child_by_field_name("body"));
    }

    fn js(&mut self, n: Node) -> bool {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "class_declaration" | "abstract_class_declaration" | "class" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let idx = self.add_def(name, DefKind::Class, n, None, None);
                self.scopes.push(idx);
                for ch in children(n) {
                    if ch.kind() == "class_heritage" {
                        for clause in children(ch) {
                            for base in children(clause) {
                                if matches!(base.kind(), "identifier" | "type_identifier" | "member_expression" | "generic_type" | "nested_type_identifier") {
                                    self.add_inherit(base, name);
                                }
                            }
                        }
                    }
                }
                self.scopes.pop();
                self.in_scope(Some(idx), Some(name.to_string()), f("body"));
                true
            }
            "interface_declaration" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let idx = self.add_def(name, DefKind::Interface, n, None, None);
                self.scopes.push(idx);
                for ch in children(n) {
                    if ch.kind() == "extends_type_clause" {
                        for base in children(ch) {
                            self.add_inherit(base, name);
                        }
                    }
                }
                self.scopes.pop();
                self.in_scope(Some(idx), Some(name.to_string()), f("body"));
                true
            }
            "function_declaration" | "generator_function_declaration" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                self.js_function(name, DefKind::Function, n, n, None);
                true
            }
            "method_definition" | "method_signature" | "abstract_method_signature" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let owner = self.owners.last().cloned();
                self.js_function(name, DefKind::Method, n, n, owner);
                true
            }
            "variable_declarator" => {
                let name_node = f("name");
                let value = f("value");
                if let (Some(nn), Some(v)) = (name_node, value) {
                    if nn.kind() == "identifier" && FUNC_VALUE_KINDS.contains(&v.kind()) {
                        let name = self.text(nn);
                        self.js_function(name, DefKind::Function, n, v, None);
                        return true;
                    }
                }
                if let Some(nn) = name_node.filter(|x| x.kind() == "identifier") {
                    let hint = f("type")
                        .and_then(|t| self.type_name(t).map(TypeHint::Named))
                        .or_else(|| value.and_then(|v| self.value_hint(v)));
                    if let Some(h) = hint {
                        let s = self.binding_scope();
                        self.add_binding(s, self.text(nn), h);
                    }
                }
                self.visit_opt(value);
                true
            }
            "public_field_definition" | "field_definition" => {
                let name_node = f("name").or(f("property"));
                let value = f("value");
                let owner = self.owners.last().cloned();
                if let (Some(nn), Some(v)) = (name_node, value) {
                    if FUNC_VALUE_KINDS.contains(&v.kind()) {
                        let name = self.text(nn);
                        self.js_function(name, DefKind::Method, n, v, owner);
                        return true;
                    }
                }
                if let (Some(nn), Some(owner)) = (name_node, owner) {
                    let hint = f("type")
                        .and_then(|t| self.type_name(t).map(TypeHint::Named))
                        .or_else(|| value.and_then(|v| self.value_hint(v)));
                    if let Some(h) = hint {
                        self.add_binding(BindingScope::Owner(owner), self.text(nn), h);
                    }
                }
                self.visit_opt(value);
                true
            }
            "type_alias_declaration" => {
                if let Some(name) = f("name").map(|x| self.text(x)) {
                    self.add_def(name, DefKind::Type, n, None, None);
                }
                true
            }
            "enum_declaration" => {
                if let Some(name) = f("name").map(|x| self.text(x)) {
                    self.add_def(name, DefKind::Enum, n, None, None);
                }
                true
            }
            "call_expression" => {
                if let Some(func) = f("function") {
                    let (name, qualifier) = self.split_callee(func);
                    if let Some(name) = name {
                        self.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    match func.kind() {
                        "member_expression" => self.visit_opt(func.child_by_field_name("object")),
                        "identifier" => {}
                        _ => self.visit(func),
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "new_expression" => {
                if let Some(c) = f("constructor") {
                    let (name, qualifier) = self.split_callee(c);
                    if let Some(name) = name {
                        self.add_ref(RefKind::New, &name, qualifier, n);
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "assignment_expression" => {
                if let (Some(left), Some(right)) = (f("left"), f("right")) {
                    if left.kind() == "member_expression" {
                        let obj = left.child_by_field_name("object");
                        let prop = left.child_by_field_name("property");
                        if let (Some(o), Some(p), Some(owner)) = (obj, prop, self.owners.last().cloned()) {
                            if o.kind() == "this" {
                                if let Some(h) = self.value_hint(right) {
                                    let h = self.resolve_alias_now(h);
                                    self.add_binding(BindingScope::Owner(owner), self.text(p), h);
                                }
                            }
                        }
                    }
                    self.visit(right);
                }
                true
            }
            "import_statement" => {
                let Some(src) = f("source") else { return true };
                let module = self.text(src).trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
                let mut imp = Import { module, names: vec![], alias: None, wildcard: false, reexport: false, line: line(n) };
                for clause in children(n).into_iter().filter(|c| c.kind() == "import_clause") {
                    for part in children(clause) {
                        match part.kind() {
                            "identifier" => imp.names.push(ImportName {
                                name: "default".into(),
                                alias: Some(self.text(part).to_string()),
                            }),
                            "namespace_import" => {
                                imp.alias = children(part).first().map(|i| self.text(*i).to_string())
                            }
                            "named_imports" => {
                                for spec in children(part).into_iter().filter(|s| s.kind() == "import_specifier") {
                                    imp.names.push(ImportName {
                                        name: spec.child_by_field_name("name").map(|x| self.text(x).to_string()).unwrap_or_default(),
                                        alias: spec.child_by_field_name("alias").map(|x| self.text(x).to_string()),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                self.facts.imports.push(imp);
                true
            }
            "export_statement" => {
                let Some(src) = f("source") else { return false };
                let module = self.text(src).trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
                let mut imp = Import { module, names: vec![], alias: None, wildcard: true, reexport: true, line: line(n) };
                for ch in children(n) {
                    match ch.kind() {
                        "export_clause" => {
                            imp.wildcard = false;
                            for spec in children(ch).into_iter().filter(|s| s.kind() == "export_specifier") {
                                imp.names.push(ImportName {
                                    name: spec.child_by_field_name("name").map(|x| self.text(x).to_string()).unwrap_or_default(),
                                    alias: spec.child_by_field_name("alias").map(|x| self.text(x).to_string()),
                                });
                            }
                        }
                        "namespace_export" => {
                            imp.wildcard = false;
                            imp.alias = children(ch).first().map(|i| self.text(*i).to_string());
                        }
                        _ => {}
                    }
                }
                self.facts.imports.push(imp);
                true
            }
            _ => false,
        }
    }

    // ---------- Rust ----------

    fn rust_use(&mut self, n: Node, prefix: &str, line_no: u32) {
        let join = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { format!("{a}::{b}") };
        match n.kind() {
            "scoped_identifier" | "identifier" | "crate" | "self" | "super" => {
                let full = join(prefix, self.text(n));
                self.push_rust_import(&full, None, false, line_no);
            }
            "use_as_clause" => {
                if let (Some(p), Some(a)) = (n.child_by_field_name("path"), n.child_by_field_name("alias")) {
                    let full = join(prefix, self.text(p));
                    let alias = self.text(a).to_string();
                    self.push_rust_import(&full, Some(alias), false, line_no);
                }
            }
            "scoped_use_list" => {
                let base = n.child_by_field_name("path").map(|p| join(prefix, self.text(p))).unwrap_or(prefix.to_string());
                if let Some(list) = n.child_by_field_name("list") {
                    for item in children(list) {
                        self.rust_use(item, &base, line_no);
                    }
                }
            }
            "use_list" => {
                for item in children(n) {
                    self.rust_use(item, prefix, line_no);
                }
            }
            "use_wildcard" => {
                let path = children(n).first().map(|p| join(prefix, self.text(*p))).unwrap_or(prefix.to_string());
                self.push_rust_import(&path, None, true, line_no);
            }
            _ => {}
        }
    }

    fn push_rust_import(&mut self, full: &str, alias: Option<String>, wildcard: bool, line_no: u32) {
        let (module, names) = if wildcard {
            (full.to_string(), vec![])
        } else {
            match full.rsplit_once("::") {
                Some((m, name)) if name != "self" => (m.to_string(), vec![ImportName { name: name.to_string(), alias }]),
                Some((m, _)) => (m.to_string(), vec![]),
                None => (String::new(), vec![ImportName { name: full.to_string(), alias }]),
            }
        };
        self.facts.imports.push(Import { module, names, alias: None, wildcard, reexport: false, line: line_no });
    }

    fn rust(&mut self, n: Node) -> bool {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "function_item" | "function_signature_item" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                // A fn item nested inside another fn is not a method of the impl.
                let in_fn = self.scope().is_some_and(|s| self.facts.defs[s as usize].kind.is_callable());
                let owner = if in_fn { None } else { self.owners.last().cloned() };
                let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
                let ret = f("return_type").and_then(|r| self.type_name(r));
                let ret = match (ret.as_deref(), &owner) {
                    (Some("Self"), Some(o)) => Some(o.clone()),
                    _ => ret,
                };
                let idx = self.add_def(name, kind, n, owner, ret);
                if let Some(params) = f("parameters") {
                    for p in children(params).into_iter().filter(|p| p.kind() == "parameter") {
                        let pat = p.child_by_field_name("pattern").filter(|x| x.kind() == "identifier");
                        let ty = p.child_by_field_name("type").and_then(|t| self.type_name(t));
                        if let (Some(pat), Some(ty)) = (pat, ty) {
                            self.add_binding(BindingScope::Def(idx), self.text(pat), TypeHint::Named(ty));
                        }
                    }
                }
                self.in_scope(Some(idx), None, f("body"));
                true
            }
            "impl_item" => {
                let owner = f("type").and_then(|t| self.type_name(t));
                if let (Some(tr), Some(o)) = (f("trait"), owner.as_deref()) {
                    self.add_inherit(tr, o);
                }
                self.in_scope(None, owner, f("body"));
                true
            }
            "trait_item" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let idx = self.add_def(name, DefKind::Trait, n, None, None);
                self.in_scope(Some(idx), Some(name.to_string()), f("body"));
                true
            }
            "struct_item" | "union_item" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                self.add_def(name, DefKind::Struct, n, None, None);
                if let Some(body) = f("body") {
                    for fd in children(body).into_iter().filter(|c| c.kind() == "field_declaration") {
                        let fname = fd.child_by_field_name("name").map(|x| self.text(x));
                        let fty = fd.child_by_field_name("type").and_then(|t| self.type_name(t));
                        if let (Some(fname), Some(fty)) = (fname, fty) {
                            self.add_binding(BindingScope::Owner(name.to_string()), fname, TypeHint::Named(fty));
                        }
                    }
                }
                true
            }
            "enum_item" | "type_item" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let kind = if n.kind() == "enum_item" { DefKind::Enum } else { DefKind::Type };
                self.add_def(name, kind, n, None, None);
                true
            }
            "mod_item" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let idx = self.add_def(name, DefKind::Module, n, None, None);
                self.in_scope(Some(idx), None, f("body"));
                true
            }
            "call_expression" => {
                if let Some(func) = f("function") {
                    let (name, qualifier) = self.split_callee(func);
                    if let Some(name) = name {
                        self.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    match func.kind() {
                        "field_expression" => self.visit_opt(func.child_by_field_name("value")),
                        "identifier" | "scoped_identifier" => {}
                        _ => self.visit(func),
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "struct_expression" => {
                if let Some(nm) = f("name") {
                    let (name, qualifier) = self.split_callee(nm);
                    if let Some(name) = name {
                        self.add_ref(RefKind::New, &name, qualifier, n);
                    }
                }
                self.visit_opt(f("body"));
                true
            }
            "let_declaration" => {
                let pat = f("pattern").filter(|p| p.kind() == "identifier");
                let hint = f("type")
                    .and_then(|t| self.type_name(t).map(TypeHint::Named))
                    .or_else(|| f("value").and_then(|v| self.value_hint(v)));
                if let (Some(p), Some(h)) = (pat, hint) {
                    let s = self.binding_scope();
                    self.add_binding(s, self.text(p), h);
                }
                self.visit_opt(f("value"));
                self.visit_opt(f("alternative"));
                true
            }
            "use_declaration" => {
                if let Some(arg) = f("argument") {
                    self.rust_use(arg, "", line(n));
                }
                true
            }
            "macro_invocation" => true,
            _ => false,
        }
    }

    // ---------- C++ / Unreal ----------

    /// Peel pointer/reference/init/array declarators down to the declared name node,
    /// returning (name node, function_declarator if this declares a function, init value).
    fn cpp_declarator<'t>(&self, mut d: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>, Option<Node<'t>>) {
        let mut value = None;
        loop {
            match d.kind() {
                "init_declarator" => {
                    value = d.child_by_field_name("value");
                    match d.child_by_field_name("declarator") {
                        Some(x) => d = x,
                        None => return (None, None, value),
                    }
                }
                "pointer_declarator" | "reference_declarator" | "array_declarator" | "parenthesized_declarator"
                | "attributed_declarator" => match d.child_by_field_name("declarator").or_else(|| children(d).into_iter().last()) {
                    Some(x) if x != d => d = x,
                    _ => return (None, None, value),
                },
                "function_declarator" => {
                    let inner = d.child_by_field_name("declarator");
                    return (inner, Some(d), value);
                }
                "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name" | "operator_name"
                | "template_function" => return (Some(d), None, value),
                _ => return (None, None, value),
            }
        }
    }

    /// Name and explicit owner (`AFoo::Bar` → ("Bar", Some("AFoo"))) of a declarator name node.
    fn cpp_name(&self, n: Node) -> Option<(String, Option<String>)> {
        match n.kind() {
            "qualified_identifier" => {
                let (name, q) = self.split_callee(n);
                let owner = q.map(|q| q.rsplit("::").next().unwrap_or(&q).to_string());
                Some((name?, owner))
            }
            "template_function" => n.child_by_field_name("name").and_then(|x| self.cpp_name(x)),
            _ => Some((self.text(n).to_string(), None)),
        }
    }

    fn cpp_params(&mut self, func_decl: Node, idx: u32) {
        let Some(params) = func_decl.child_by_field_name("parameters") else { return };
        for p in children(params) {
            if !matches!(p.kind(), "parameter_declaration" | "optional_parameter_declaration") {
                continue;
            }
            let Some(ty) = p.child_by_field_name("type").and_then(|t| self.type_name(t)) else { continue };
            let Some(d) = p.child_by_field_name("declarator") else { continue };
            if let (Some(nm), None, _) = self.cpp_declarator(d) {
                self.add_binding(BindingScope::Def(idx), self.text(nm), TypeHint::Named(ty));
            }
        }
    }

    fn cpp(&mut self, n: Node) -> bool {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                let (Some(name_node), Some(body)) = (f("name"), f("body")) else { return false };
                let Some((name, _)) = self.cpp_name(name_node) else { return false };
                let kind = if n.kind() == "class_specifier" { DefKind::Class } else { DefKind::Struct };
                let idx = self.add_def(&name, kind, n, None, None);
                self.scopes.push(idx);
                for ch in children(n).into_iter().filter(|c| c.kind() == "base_class_clause") {
                    for base in children(ch) {
                        if matches!(base.kind(), "type_identifier" | "qualified_identifier" | "template_type") {
                            self.add_inherit(base, &name);
                        }
                    }
                }
                self.scopes.pop();
                self.in_scope(Some(idx), Some(name), Some(body));
                true
            }
            "enum_specifier" => {
                if let (Some(nm), Some(_)) = (f("name"), f("body")) {
                    let name = self.text(nm).to_string();
                    self.add_def(&name, DefKind::Enum, n, None, None);
                }
                true
            }
            "alias_declaration" | "type_definition" => {
                let nm = f("name").or_else(|| f("declarator").filter(|d| d.kind() == "type_identifier"));
                if let Some(nm) = nm {
                    let name = self.text(nm).to_string();
                    self.add_def(&name, DefKind::Type, n, None, None);
                }
                false
            }
            "function_definition" => {
                let Some(d) = f("declarator") else { return false };
                let (Some(name_node), Some(fdecl), _) = self.cpp_declarator(d) else { return false };
                let Some((name, explicit_owner)) = self.cpp_name(name_node) else { return false };
                let class = self.enclosing_type_def().map(|d| d.name.clone());
                let owner = explicit_owner.clone().or(class);
                let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
                let ret = f("type").and_then(|t| self.type_name(t));
                let idx = self.add_def_full(&name, kind, n, owner, ret, false);
                self.cpp_params(fdecl, idx);
                self.scopes.push(idx);
                let pushed = explicit_owner.is_some();
                if let Some(o) = explicit_owner {
                    self.owners.push(o);
                }
                for ch in children(n) {
                    if matches!(ch.kind(), "field_initializer_list" | "compound_statement" | "try_statement") {
                        self.visit(ch);
                    }
                }
                if pushed {
                    self.owners.pop();
                }
                self.scopes.pop();
                true
            }
            "field_declaration" | "declaration" => {
                let in_class = n.kind() == "field_declaration";
                let ty_node = f("type");
                let ty = ty_node.and_then(|t| self.type_name(t));
                let mut c = n.walk();
                let decls: Vec<Node> = n.children_by_field_name("declarator", &mut c).collect();
                // `UPROPERTY() float Health = 100.f;` keeps its default in `default_value`.
                // ...and `if (T* x = f())` condition declarations keep it in `value`.
                let default_value = f("default_value").or(f("value"));
                for d in decls {
                    let (name_node, fdecl, value) = self.cpp_declarator(d);
                    let Some(name_node) = name_node else { continue };
                    if let Some(fd) = fdecl {
                        // A prototype: method declared in a class, or a free function declared in a header.
                        let Some((name, explicit_owner)) = self.cpp_name(name_node) else { continue };
                        let owner = explicit_owner.or_else(|| if in_class { self.owners.last().cloned() } else { None });
                        let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
                        let _ = fd;
                        self.add_def_full(&name, kind, n, owner, ty.clone(), true);
                        continue;
                    }
                    let value = value.or(default_value);
                    let hint = ty.clone().map(TypeHint::Named).or_else(|| value.and_then(|v| self.value_hint(v)));
                    if let Some(h) = hint {
                        let name = self.text(name_node).to_string();
                        if in_class {
                            if let Some(owner) = self.owners.last().cloned() {
                                self.add_binding(BindingScope::Owner(owner), &name, h);
                            }
                        } else {
                            let s = self.binding_scope();
                            self.add_binding(s, &name, h);
                        }
                    }
                    self.visit_opt(value);
                }
                // Unreal reflection attributes on properties aren't defs; don't let them leak to the next def.
                self.attrs.remove(&line(n));
                // Anonymous struct/class bodies inside a declaration still define things.
                if let Some(t) = ty_node.filter(|t| t.child_by_field_name("body").is_some()) {
                    self.visit(t);
                }
                true
            }
            "call_expression" => {
                if let Some(func) = f("function") {
                    let (name, qualifier) = self.split_callee(func);
                    if let Some(name) = name {
                        self.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    match func.kind() {
                        "field_expression" => self.visit_opt(func.child_by_field_name("argument")),
                        "identifier" | "qualified_identifier" | "template_function" => {}
                        _ => self.visit(func),
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "new_expression" => {
                if let Some(t) = f("type") {
                    let (name, qualifier) = self.split_callee(t);
                    if let Some(name) = name {
                        self.add_ref(RefKind::New, &name, qualifier, n);
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "pointer_expression" => {
                // `&AFoo::Bar` handed to BindAction/AddDynamic/SetTimer: a deferred call.
                if let Some(arg) = f("argument").filter(|a| a.kind() == "qualified_identifier") {
                    if self.text(n).starts_with('&') {
                        let (name, qualifier) = self.split_callee(arg);
                        if let Some(name) = name {
                            self.add_ref(RefKind::Call, &name, qualifier, n);
                        }
                        return true;
                    }
                }
                false
            }
            "preproc_include" => {
                if let Some(p) = f("path").filter(|p| p.kind() == "string_literal") {
                    let module = self.text(p).trim_matches('"').to_string();
                    self.facts.imports.push(Import { module, names: vec![], alias: None, wildcard: true, reexport: true, line: line(n) });
                }
                true
            }
            _ => false,
        }
    }

    // ---------- Go ----------

    fn go_params(&mut self, params: Option<Node>, idx: u32) {
        let Some(params) = params else { return };
        for p in children(params).into_iter().filter(|p| p.kind() == "parameter_declaration") {
            let Some(ty) = p.child_by_field_name("type").and_then(|t| self.type_name(t)) else { continue };
            let mut c = p.walk();
            let names: Vec<_> = p.children_by_field_name("name", &mut c).collect();
            for nm in names {
                self.add_binding(BindingScope::Def(idx), self.text(nm), TypeHint::Named(ty.clone()));
            }
        }
    }

    fn go(&mut self, n: Node) -> bool {
        let f = |name: &str| n.child_by_field_name(name);
        match n.kind() {
            "function_declaration" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let ret = f("result").and_then(|r| self.type_name(r));
                let idx = self.add_def(name, DefKind::Function, n, None, ret);
                self.go_params(f("parameters"), idx);
                self.in_scope(Some(idx), None, f("body"));
                true
            }
            "method_declaration" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let recv = f("receiver")
                    .and_then(|r| children(r).into_iter().find(|p| p.kind() == "parameter_declaration"));
                let owner = recv.and_then(|r| r.child_by_field_name("type")).and_then(|t| self.type_name(t));
                let ret = f("result").and_then(|r| self.type_name(r));
                let idx = self.add_def(name, DefKind::Method, n, owner.clone(), ret);
                if let (Some(r), Some(o)) = (recv, owner.clone()) {
                    if let Some(rn) = r.child_by_field_name("name") {
                        self.add_binding(BindingScope::Def(idx), self.text(rn), TypeHint::Named(o));
                    }
                }
                self.go_params(f("parameters"), idx);
                self.in_scope(Some(idx), owner, f("body"));
                true
            }
            "type_spec" => {
                let Some(name) = f("name").map(|x| self.text(x)) else { return false };
                let ty = f("type");
                let kind = match ty.map(|t| t.kind()) {
                    Some("struct_type") => DefKind::Struct,
                    Some("interface_type") => DefKind::Interface,
                    _ => DefKind::Type,
                };
                self.add_def(name, kind, n, None, None);
                if let Some(st) = ty.filter(|t| t.kind() == "struct_type") {
                    for list in children(st).into_iter().filter(|c| c.kind() == "field_declaration_list") {
                        for fd in children(list).into_iter().filter(|c| c.kind() == "field_declaration") {
                            let Some(fty) = fd.child_by_field_name("type").and_then(|t| self.type_name(t)) else { continue };
                            let mut c = fd.walk();
                            let names: Vec<_> = fd.children_by_field_name("name", &mut c).collect();
                            for nm in names {
                                self.add_binding(BindingScope::Owner(name.to_string()), self.text(nm), TypeHint::Named(fty.clone()));
                            }
                        }
                    }
                }
                true
            }
            "call_expression" => {
                if let Some(func) = f("function") {
                    let (name, qualifier) = self.split_callee(func);
                    if let Some(name) = name {
                        self.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    match func.kind() {
                        "selector_expression" => self.visit_opt(func.child_by_field_name("operand")),
                        "identifier" => {}
                        _ => self.visit(func),
                    }
                }
                self.visit_opt(f("arguments"));
                true
            }
            "short_var_declaration" | "assignment_statement" | "var_spec" => {
                let (lefts, rights): (Vec<Node>, Vec<Node>) = if n.kind() == "var_spec" {
                    let mut c = n.walk();
                    let names = n.children_by_field_name("name", &mut c).collect();
                    let vals = f("value").map(children).unwrap_or_default();
                    (names, vals)
                } else {
                    (f("left").map(children).unwrap_or_default(), f("right").map(children).unwrap_or_default())
                };
                let declared = if n.kind() == "var_spec" { f("type").and_then(|t| self.type_name(t)) } else { None };
                for (i, l) in lefts.iter().enumerate() {
                    if l.kind() != "identifier" {
                        continue;
                    }
                    let hint = declared
                        .clone()
                        .map(TypeHint::Named)
                        .or_else(|| rights.get(i).and_then(|r| self.value_hint(*r)));
                    if let Some(h) = hint {
                        let s = self.binding_scope();
                        self.add_binding(s, self.text(*l), h);
                    }
                }
                for r in rights {
                    self.visit(r);
                }
                true
            }
            "import_spec" => {
                let Some(path) = f("path") else { return true };
                let module = self.text(path).trim_matches('"').to_string();
                let alias = f("name").map(|x| self.text(x).to_string()).or_else(|| {
                    let mut segs = module.rsplit('/');
                    let last = segs.next().unwrap_or("");
                    let is_version = last.len() > 1 && last.starts_with('v') && last[1..].chars().all(|c| c.is_ascii_digit());
                    Some(if is_version { segs.next().unwrap_or(last) } else { last }.to_string())
                });
                self.facts.imports.push(Import { module, names: vec![], alias, wildcard: false, reexport: false, line: line(n) });
                true
            }
            _ => false,
        }
    }
}

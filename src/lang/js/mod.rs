//! JavaScript and TypeScript (and TSX/JSX): classes, functions, arrow
//! functions, object-literal namespaces, JSX components, ES module imports and
//! exports, and module resolution through tsconfig `paths` and workspaces.

mod project;

use super::{dir_of, normalize_path, strip_ext, Language, Modules, Rules, Target};
use crate::extract::{children, line, Walker};
use crate::facts::*;
use crate::graph::GFile;
use project::JsProject;
use std::collections::HashMap;
use tree_sitter::Node;

pub struct Js;

/// Browser and Node globals: a call on one of these is never a project method.
const GLOBALS: &[&str] = &[
    "console", "Math", "JSON", "Object", "Array", "Promise", "Reflect", "Number", "String", "Boolean", "Symbol",
    "Date", "RegExp", "Error", "Intl", "BigInt", "Map", "Set", "WeakMap", "WeakSet", "Atomics", "Proxy", "window",
    "document", "globalThis", "self", "process", "navigator", "localStorage", "sessionStorage", "location",
    "history", "performance", "crypto", "Buffer", "URL", "URLSearchParams", "Response", "Request", "Headers",
    "require", "module", "exports",
];

static RULES: Rules = Rules {
    wrapper_types: &["Promise", "Readonly", "Partial", "Ref"],
    globals: GLOBALS,
    methods_in_class_file: true,
    type_aliases_callable: false,
    ..Rules::DEFAULT
};

const FUNC_VALUE_KINDS: &[&str] = &["arrow_function", "function_expression", "function", "generator_function"];

/// Higher-order calls that return (a wrapped) version of the function they're given.
const FUNCTION_WRAPPERS: &[&str] = &["memo", "forwardRef", "observer", "useCallback", "debounce", "throttle"];

impl Language for Js {
    fn name(&self) -> &'static str {
        "js"
    }

    fn lang_of_extension(&self, ext: &str) -> Option<Lang> {
        Some(match ext {
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            _ => return None,
        })
    }

    fn grammar(&self, lang: Lang) -> tree_sitter::Language {
        match lang {
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    fn visit(&self, w: &mut Walker, n: Node) -> bool {
        visit(w, n)
    }

    fn rules(&self) -> &'static Rules {
        &RULES
    }

    fn is_project_file(&self, path: &str) -> bool {
        project::is_config(path)
    }

    fn modules(&self, files: &[GFile], project: &[(String, Vec<u8>)]) -> Box<dyn Modules> {
        let mut paths = HashMap::new();
        for (fi, f) in files.iter().enumerate().filter(|(_, f)| matches!(f.lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript)) {
            paths.insert(f.path.clone(), fi as u32);
            paths.entry(strip_ext(&f.path).to_string()).or_insert(fi as u32);
        }
        let configs: Vec<(String, Vec<u8>)> = project.iter().filter(|(p, _)| project::is_config(p)).cloned().collect();
        Box::new(JsModules { paths, project: JsProject::parse(&configs) })
    }
}

struct JsModules {
    /// Path, and path without extension -> file.
    paths: HashMap<String, u32>,
    project: JsProject,
}

impl JsModules {
    fn module_file(&self, files: &[GFile], from: u32, spec: &str) -> Option<u32> {
        let importer = &files[from as usize].path;
        let bases = if spec.starts_with('.') {
            vec![normalize_path(&format!("{}/{}", dir_of(importer), spec))]
        } else {
            let mut c = self.project.candidates(importer, spec);
            // Common bundler aliases when no tsconfig says otherwise.
            if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
                c.push(format!("src/{rest}"));
            }
            c
        };
        bases.iter().find_map(|base| self.file(base))
    }

    /// The source file an import path names: exact, without its extension
    /// (`./x.js` -> `x.ts`), or as a directory `index`.
    fn file(&self, base: &str) -> Option<u32> {
        let stem = strip_ext(base);
        [base, stem, &format!("{base}/index"), &format!("{stem}/index")].into_iter().find_map(|c| self.paths.get(c).copied())
    }
}

impl Modules for JsModules {
    fn import(&self, files: &[GFile], from: u32, imp: &Import, name: Option<&str>) -> Target {
        match self.module_file(files, from, &imp.module) {
            Some(f) => match name {
                None => Target::Module(imp.module.clone(), vec![f]),
                Some(n) => Target::Symbol(vec![f], n.to_string()),
            },
            None => Target::External,
        }
    }
}

// ---------- syntax ----------

/// Parameters become bindings of the function's scope: typed ones carry their
/// type (a generic parameter its constraint), the rest are known-local names.
fn params(w: &mut Walker, func: Node, idx: u32, is_ctor: bool) {
    if let Some(single) = func.child_by_field_name("parameter") {
        pattern_names(w, single, BindingScope::Def(idx)); // `x => x()`
    }
    let Some(params) = func.child_by_field_name("parameters") else { return };
    let generics = type_params(w, func);
    for p in children(params) {
        let pat = match p.kind() {
            "required_parameter" | "optional_parameter" => p.child_by_field_name("pattern"),
            "identifier" | "object_pattern" | "array_pattern" | "assignment_pattern" | "rest_pattern" => Some(p), // plain JS
            _ => None,
        };
        let Some(pat) = pat else { continue };
        let ty = p.child_by_field_name("type").and_then(|t| w.type_name(t));
        let ty = ty.map(|t| generics.get(&t).cloned().unwrap_or(t));
        let (Some(ty), "identifier") = (ty, pat.kind()) else {
            pattern_names(w, pat, BindingScope::Def(idx));
            continue;
        };
        let name = w.text(pat);
        w.add_binding(BindingScope::Def(idx), name, TypeHint::Named(ty.clone()));
        let is_property = children(p).iter().any(|c| c.kind() == "accessibility_modifier" || w.text(*c) == "readonly");
        if is_ctor && is_property {
            if let Some(owner) = w.owners.last().cloned() {
                w.add_binding(BindingScope::Owner(owner), name, TypeHint::Named(ty));
            }
        }
    }
}

/// `<T extends Repo>`: generic parameter name -> its constraint's type.
fn type_params(w: &Walker, func: Node) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(tp) = func.child_by_field_name("type_parameters") {
        for p in children(tp).into_iter().filter(|p| p.kind() == "type_parameter") {
            let name = p.child_by_field_name("name").map(|n| w.text(n).to_string());
            let bound = p.child_by_field_name("constraint").and_then(|c| w.type_name(c));
            if let (Some(n), Some(b)) = (name, bound) {
                out.insert(n, b);
            }
        }
    }
    out
}

/// Every name a parameter or destructuring pattern binds, as a local of unknown type.
fn pattern_names(w: &mut Walker, pat: Node, scope: BindingScope) {
    match pat.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => w.add_binding(scope, w.text(pat), TypeHint::Unknown),
        "pair_pattern" => {
            if let Some(v) = pat.child_by_field_name("value") {
                pattern_names(w, v, scope);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(l) = pat.child_by_field_name("left") {
                pattern_names(w, l, scope);
            }
        }
        "object_pattern" | "array_pattern" | "rest_pattern" => {
            for c in children(pat) {
                pattern_names(w, c, scope.clone());
            }
        }
        _ => {}
    }
}

fn function(w: &mut Walker, name: &str, kind: DefKind, def_node: Node, func: Node, owner: Option<String>) {
    let ret = func.child_by_field_name("return_type").and_then(|r| w.type_name(r));
    let idx = w.add_def(name, kind, def_node, owner, ret);
    params(w, func, idx, name == "constructor");
    w.in_scope(Some(idx), None, func.child_by_field_name("body"));
}

/// `memo((props) => ...)`, `forwardRef(function (props, ref) {...})`: a
/// component wrapped by a higher-order function is still that function.
fn wrapped_function<'t>(w: &Walker, v: Node<'t>) -> Option<Node<'t>> {
    if v.kind() != "call_expression" {
        return None;
    }
    let (name, _) = w.split_callee(v.child_by_field_name("function")?);
    if !FUNCTION_WRAPPERS.contains(&name?.as_str()) {
        return None;
    }
    let args = v.child_by_field_name("arguments")?;
    children(args).into_iter().find(|a| FUNC_VALUE_KINDS.contains(&a.kind()) || a.kind() == "call_expression")
        .and_then(|a| if a.kind() == "call_expression" { wrapped_function(w, a) } else { Some(a) })
}

/// The wrapper calls around a wrapped function (`memo(forwardRef(fn), eq)`) are
/// calls too, made from the enclosing scope; so are their other arguments.
fn wrapper_calls(w: &mut Walker, mut call: Node, func: Node) {
    while call.kind() == "call_expression" {
        if let Some((Some(name), qualifier)) = call.child_by_field_name("function").map(|c| w.split_callee(c)) {
            w.add_ref(RefKind::Call, &name, qualifier, call);
        }
        let Some(args) = call.child_by_field_name("arguments") else { return };
        let mut next = None;
        for a in children(args) {
            if a.id() == func.id() || (a.kind() == "call_expression" && next.is_none() && a.byte_range().contains(&func.start_byte())) {
                next = Some(a);
            } else {
                w.visit(a);
            }
        }
        match next {
            Some(a) if a.id() != func.id() => call = a,
            _ => return,
        }
    }
}

/// `const api = { get() {}, put: (x) => x }` at the top level: a namespace
/// object whose function members are its methods.
fn object_namespace(w: &mut Walker, name: &str, decl: Node, obj: Node) -> bool {
    let is_fn_member = |m: &Node| {
        m.kind() == "method_definition"
            || (m.kind() == "pair" && m.child_by_field_name("value").is_some_and(|v| FUNC_VALUE_KINDS.contains(&v.kind())))
    };
    if !w.scopes.is_empty() || !children(obj).iter().any(is_fn_member) {
        return false;
    }
    let idx = w.add_def(name, DefKind::Object, decl, None, None);
    w.scopes.push(idx);
    w.owners.push(name.to_string());
    for m in children(obj) {
        let key = m.child_by_field_name("key").filter(|k| matches!(k.kind(), "property_identifier" | "identifier"));
        match (m.kind(), key, m.child_by_field_name("value")) {
            ("pair", Some(k), Some(v)) if FUNC_VALUE_KINDS.contains(&v.kind()) => {
                let key = w.text(k);
                function(w, key, DefKind::Method, m, v, Some(name.to_string()));
            }
            _ => w.visit(m),
        }
    }
    w.owners.pop();
    w.scopes.pop();
    true
}

fn visit(w: &mut Walker, n: Node) -> bool {
    let f = |name: &str| n.child_by_field_name(name);
    match n.kind() {
        "class_declaration" | "abstract_class_declaration" | "class" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let idx = w.add_def(name, DefKind::Class, n, None, None);
            w.scopes.push(idx);
            for ch in children(n) {
                if ch.kind() == "class_heritage" {
                    for clause in children(ch) {
                        for base in children(clause) {
                            if matches!(base.kind(), "identifier" | "type_identifier" | "member_expression" | "generic_type" | "nested_type_identifier") {
                                w.add_inherit(base, name);
                            }
                        }
                    }
                }
            }
            w.scopes.pop();
            w.in_scope(Some(idx), Some(name.to_string()), f("body"));
            true
        }
        "interface_declaration" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let idx = w.add_def(name, DefKind::Interface, n, None, None);
            w.scopes.push(idx);
            for ch in children(n) {
                if ch.kind() == "extends_type_clause" {
                    for base in children(ch) {
                        w.add_inherit(base, name);
                    }
                }
            }
            w.scopes.pop();
            w.in_scope(Some(idx), Some(name.to_string()), f("body"));
            true
        }
        "function_declaration" | "generator_function_declaration" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            function(w, name, DefKind::Function, n, n, None);
            true
        }
        "method_definition" | "method_signature" | "abstract_method_signature" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let owner = w.owners.last().cloned();
            // `get req(): HonoRequest` makes `c.req` a member of that type.
            let getter = (0..n.child_count()).filter_map(|i| n.child(i as _)).any(|c| c.kind() == "get");
            let ret = f("return_type").and_then(|r| w.type_name(r));
            if let (true, Some(o), Some(r)) = (getter, &owner, ret) {
                w.add_binding(BindingScope::Owner(o.clone()), name, TypeHint::Named(r));
            }
            function(w, name, DefKind::Method, n, n, owner);
            true
        }
        "property_signature" => {
            // `interface Ctx { req: HonoRequest }`
            if let (Some(nn), Some(o)) = (f("name"), w.owners.last().cloned()) {
                if let Some(t) = f("type").and_then(|t| w.type_name(t)) {
                    w.add_binding(BindingScope::Owner(o), w.text(nn), TypeHint::Named(t));
                }
            }
            true
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            // `<Widget />` / `<Toolbar.Item />` renders (calls) a component; lowercase tags are DOM.
            if let Some(nm) = f("name") {
                let (name, qualifier) = w.split_callee(nm);
                let component = qualifier.is_some() || name.as_deref().is_some_and(|x| x.starts_with(|c: char| c.is_ascii_uppercase()));
                if let (true, Some(name)) = (component, name) {
                    w.add_ref(RefKind::Call, &name, qualifier, n);
                }
            }
            false
        }
        "variable_declarator" => {
            let name_node = f("name");
            let value = f("value");
            if let (Some(nn), Some(v)) = (name_node, value) {
                if nn.kind() == "identifier" {
                    let name = w.text(nn);
                    if FUNC_VALUE_KINDS.contains(&v.kind()) {
                        function(w, name, DefKind::Function, n, v, None);
                        return true;
                    }
                    if let Some(func) = wrapped_function(w, v) {
                        wrapper_calls(w, v, func);
                        function(w, name, DefKind::Function, n, func, None);
                        return true;
                    }
                    let mut inner = v;
                    while matches!(inner.kind(), "as_expression" | "satisfies_expression" | "parenthesized_expression") {
                        match children(inner).first() {
                            Some(c) => inner = *c,
                            None => break,
                        }
                    }
                    if inner.kind() == "object" && object_namespace(w, name, n, inner) {
                        return true;
                    }
                }
            }
            let local = w.scope().is_some_and(|s| w.facts.defs[s as usize].kind.is_callable());
            match name_node {
                Some(nn) if nn.kind() == "identifier" => {
                    let hint = f("type")
                        .and_then(|t| w.type_name(t).map(TypeHint::Named))
                        .or_else(|| value.and_then(|v| w.value_hint(v)))
                        // A function's locals shadow same-named functions elsewhere.
                        .or(local.then_some(TypeHint::Unknown));
                    if let Some(h) = hint {
                        let s = w.binding_scope();
                        w.add_binding(s, w.text(nn), h);
                    }
                }
                Some(pat) if local => {
                    let s = w.binding_scope();
                    pattern_names(w, pat, s); // `const { onPick } = props`
                }
                _ => {}
            }
            w.visit_opt(value);
            true
        }
        "public_field_definition" | "field_definition" => {
            let name_node = f("name").or(f("property"));
            let value = f("value");
            let owner = w.owners.last().cloned();
            if let (Some(nn), Some(v)) = (name_node, value) {
                if FUNC_VALUE_KINDS.contains(&v.kind()) {
                    let name = w.text(nn);
                    function(w, name, DefKind::Method, n, v, owner);
                    return true;
                }
            }
            if let (Some(nn), Some(owner)) = (name_node, owner) {
                let hint = f("type")
                    .and_then(|t| w.type_name(t).map(TypeHint::Named))
                    .or_else(|| value.and_then(|v| w.value_hint(v)));
                if let Some(h) = hint {
                    w.add_binding(BindingScope::Owner(owner), w.text(nn), h);
                }
            }
            w.visit_opt(value);
            true
        }
        "type_alias_declaration" => {
            if let Some(name) = f("name").map(|x| w.text(x)) {
                w.add_def(name, DefKind::Type, n, None, None);
            }
            true
        }
        "enum_declaration" => {
            if let Some(name) = f("name").map(|x| w.text(x)) {
                w.add_def(name, DefKind::Enum, n, None, None);
            }
            true
        }
        "call_expression" => {
            if let Some(func) = f("function") {
                let (name, qualifier) = w.split_callee(func);
                if let Some(name) = name {
                    w.add_ref(RefKind::Call, &name, qualifier, n);
                }
                match func.kind() {
                    "member_expression" => w.visit_opt(func.child_by_field_name("object")),
                    "identifier" => {}
                    _ => w.visit(func),
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "new_expression" => {
            if let Some(c) = f("constructor") {
                let (name, qualifier) = w.split_callee(c);
                if let Some(name) = name {
                    w.add_ref(RefKind::New, &name, qualifier, n);
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "assignment_expression" => {
            if let (Some(left), Some(right)) = (f("left"), f("right")) {
                if left.kind() == "member_expression" {
                    let obj = left.child_by_field_name("object");
                    let prop = left.child_by_field_name("property");
                    if let (Some(o), Some(p), Some(owner)) = (obj, prop, w.owners.last().cloned()) {
                        if o.kind() == "this" {
                            if let Some(h) = w.value_hint(right) {
                                let h = w.resolve_alias_now(h);
                                w.add_binding(BindingScope::Owner(owner), w.text(p), h);
                            }
                        }
                    }
                }
                w.visit(right);
            }
            true
        }
        "import_statement" => {
            let Some(src) = f("source") else { return true };
            let module = w.text(src).trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
            let mut imp = Import { module, names: vec![], alias: None, wildcard: false, reexport: false, line: line(n) };
            for clause in children(n).into_iter().filter(|c| c.kind() == "import_clause") {
                for part in children(clause) {
                    match part.kind() {
                        "identifier" => imp.names.push(ImportName {
                            name: "default".into(),
                            alias: Some(w.text(part).to_string()),
                        }),
                        "namespace_import" => {
                            imp.alias = children(part).first().map(|i| w.text(*i).to_string())
                        }
                        "named_imports" => {
                            for spec in children(part).into_iter().filter(|s| s.kind() == "import_specifier") {
                                imp.names.push(ImportName {
                                    name: spec.child_by_field_name("name").map(|x| w.text(x).to_string()).unwrap_or_default(),
                                    alias: spec.child_by_field_name("alias").map(|x| w.text(x).to_string()),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            w.facts.imports.push(imp);
            true
        }
        "export_statement" => {
            let Some(src) = f("source") else {
                local_exports(w, n);
                return false;
            };
            let module = w.text(src).trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string();
            let mut imp = Import { module, names: vec![], alias: None, wildcard: true, reexport: true, line: line(n) };
            for ch in children(n) {
                match ch.kind() {
                    "export_clause" => {
                        imp.wildcard = false;
                        for spec in children(ch).into_iter().filter(|s| s.kind() == "export_specifier") {
                            imp.names.push(ImportName {
                                name: spec.child_by_field_name("name").map(|x| w.text(x).to_string()).unwrap_or_default(),
                                alias: spec.child_by_field_name("alias").map(|x| w.text(x).to_string()),
                            });
                        }
                    }
                    "namespace_export" => {
                        imp.wildcard = false;
                        imp.alias = children(ch).first().map(|i| w.text(*i).to_string());
                    }
                    _ => {}
                }
            }
            w.facts.imports.push(imp);
            true
        }
        _ => false,
    }
}

/// `export { a, b as c }`, `export default name`, `export default class X`.
fn local_exports(w: &mut Walker, n: Node) {
    for clause in children(n).into_iter().filter(|c| c.kind() == "export_clause") {
        for spec in children(clause).into_iter().filter(|s| s.kind() == "export_specifier") {
            let local = spec.child_by_field_name("name").map(|x| w.text(x).to_string()).unwrap_or_default();
            let exported = spec.child_by_field_name("alias").map(|x| w.text(x).to_string()).unwrap_or_else(|| local.clone());
            if !local.is_empty() {
                w.facts.exports.push(Export { local, exported });
            }
        }
    }
    let is_default = (0..n.child_count()).filter_map(|i| n.child(i as _)).any(|c| c.kind() == "default");
    if !is_default {
        return;
    }
    let named = n
        .child_by_field_name("value")
        .filter(|v| v.kind() == "identifier")
        .or_else(|| n.child_by_field_name("declaration").and_then(|d| d.child_by_field_name("name")));
    if let Some(local) = named {
        w.facts.exports.push(Export { local: w.text(local).to_string(), exported: "default".into() });
    }
}

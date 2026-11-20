//! C and C++ (one grammar), with Unreal Engine support: classes, structs,
//! out-of-line `A::B::f` definitions, header prototypes linked to their
//! bodies, `#include` resolution, implicit `this->` members, type aliases,
//! and Unreal macros normalised away before parsing (see `unreal`).

pub mod unreal;

use super::{dir_of, normalize_path, Language, Modules, Preprocessed, Rules, Target};
use crate::extract::{children, line, Walker};
use crate::facts::*;
use crate::graph::{GDef, GFile};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct Cpp;

static RULES: Rules = Rules {
    wrapper_types: &[
        "TObjectPtr", "TWeakObjectPtr", "TSoftObjectPtr", "TStrongObjectPtr", "TLazyObjectPtr", "TSharedPtr",
        "TSharedRef", "TWeakPtr", "TUniquePtr", "TOptional", "shared_ptr", "unique_ptr", "weak_ptr", "optional",
    ],
    builtin_methods: &[super::COMMON_BUILTIN_METHODS, CONTAINER_METHODS],
    self_names: &["ThisClass"],
    implicit_this: true,
    static_scope_calls: true,
    follow_type_aliases: true,
    link_declarations: true,
    ..Rules::DEFAULT
};

/// Unreal and STL container, delegate and handle methods.
const CONTAINER_METHODS: &[&str] = &[
    "Add", "AddUnique", "Num", "Remove", "RemoveAt", "Contains", "Find", "FindRef", "Empty", "Reset", "Get",
    "IsValid", "Broadcast", "AddDynamic", "RemoveDynamic", "AddUObject", "AddLambda", "BindUObject",
    "BindLambda", "Execute", "ExecuteIfBound", "IsBound", "ToString", "size", "begin", "end", "empty",
    "push_back", "emplace_back", "erase", "at", "data", "c_str", "reset", "release", "lock",
];

/// Templates whose result is (a pointer to) their first type argument.
const TYPE_RETURNING_TEMPLATES: &[&str] = &[
    "Cast", "CastChecked", "ExactCast", "NewObject", "CreateDefaultSubobject", "SpawnActor", "SpawnActorDeferred",
    "LoadObject", "FindObject", "GetSubsystem", "GetGameInstanceSubsystem", "GetWorldSubsystem", "FindComponentByClass",
    "GetComponentByClass", "GetPawn", "GetController", "GetPlayerState", "GetAuthGameMode", "GetGameState",
    "GetOwner", "GetOuter", "GetTypedOuter", "MakeShared", "MakeUnique", "make_shared", "make_unique",
    "static_cast", "dynamic_cast", "reinterpret_cast", "StaticCast", "StaticCastSharedPtr",
];

impl Language for Cpp {
    fn name(&self) -> &'static str {
        "cpp"
    }

    /// C is parsed with the C++ grammar: it accepts nearly all C and keeps one resolver.
    fn lang_of_extension(&self, ext: &str) -> Option<Lang> {
        matches!(ext, "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h" | "inl" | "ipp" | "tpp" | "c").then_some(Lang::Cpp)
    }

    fn grammar(&self, _: Lang) -> tree_sitter::Language {
        tree_sitter_cpp::LANGUAGE.into()
    }

    fn preprocess(&self, raw: &[u8]) -> Option<Preprocessed> {
        if !unreal::looks_like_unreal(raw) {
            return None;
        }
        let u = unreal::normalize(raw);
        let defs = u
            .delegates
            .iter()
            .map(|(name, line)| Def {
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
            })
            .collect();
        Some(Preprocessed { src: u.src, attrs: u.attrs, defs })
    }

    fn visit(&self, w: &mut Walker, n: Node) -> bool {
        visit(w, n)
    }

    fn call_result_type(&self, w: &Walker, func: Node) -> Option<String> {
        let tmpl = match func.kind() {
            "template_function" => Some(func),
            "field_expression" => func.child_by_field_name("field").filter(|f| f.kind() == "template_method"),
            _ => None,
        }?;
        let tname = tmpl.child_by_field_name("name").map(|x| w.text(x)).unwrap_or("");
        if !TYPE_RETURNING_TEMPLATES.contains(&tname) {
            return None;
        }
        tmpl.child_by_field_name("arguments").and_then(|a| w.type_name(a))
    }

    fn wrapper_kinds(&self) -> &'static [&'static str] {
        &["template_declaration"]
    }

    fn rules(&self) -> &'static Rules {
        &RULES
    }

    fn modules(&self, files: &[GFile], _: &[(String, Vec<u8>)]) -> Box<dyn Modules> {
        let mut by_basename: HashMap<String, Vec<u32>> = HashMap::new();
        for (fi, f) in files.iter().enumerate().filter(|(_, f)| f.lang == Lang::Cpp) {
            by_basename.entry(f.path.rsplit('/').next().unwrap_or(&f.path).to_string()).or_default().push(fi as u32);
        }
        Box::new(Includes { by_basename })
    }

    /// Unreal RPCs and BlueprintNativeEvents are implemented in `<name>_Implementation`.
    fn definition_key(&self, d: &GDef) -> (String, String) {
        if d.attrs.iter().any(|a| unreal::IMPLEMENTATION_SPECIFIERS.contains(&a.as_str())) {
            (format!("{}_Implementation", d.name), format!("{}_Implementation", d.qual))
        } else {
            (d.name.clone(), d.qual.clone())
        }
    }
}

struct Includes {
    by_basename: HashMap<String, Vec<u32>>,
}

impl Modules for Includes {
    /// `#include "a/b.h"`: relative to the includer first, then any file whose
    /// path ends with the include path, preferring the one sharing the most
    /// leading directories with the includer (same module/plugin).
    fn import(&self, files: &[GFile], from: u32, imp: &Import, _: Option<&str>) -> Target {
        let inc = imp.module.as_str();
        let me = &files[from as usize].path;
        let found = (|| {
            let cands = self.by_basename.get(inc.rsplit('/').next()?)?;
            let rel = normalize_path(&format!("{}/{}", dir_of(me), inc));
            if let Some(f) = cands.iter().find(|f| files[**f as usize].path == rel) {
                return Some(*f);
            }
            let suffix = format!("/{inc}");
            let shared = |p: &str| me.split('/').zip(p.split('/')).take_while(|(a, b)| a == b).count();
            cands
                .iter()
                .copied()
                .filter(|f| {
                    let p = &files[*f as usize].path;
                    p == inc || p.ends_with(&suffix)
                })
                .max_by_key(|f| shared(&files[*f as usize].path))
        })();
        match found {
            Some(f) => Target::Module(imp.module.clone(), vec![f]),
            None => Target::External,
        }
    }
}

// ---------- syntax ----------

/// Peel pointer/reference/init/array declarators down to the declared name node,
/// returning (name node, function_declarator if this declares a function, init value).
fn declarator<'t>(mut d: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>, Option<Node<'t>>) {
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
fn def_name(w: &Walker, n: Node) -> Option<(String, Option<String>)> {
    match n.kind() {
        "qualified_identifier" => {
            let (name, q) = w.split_callee(n);
            let owner = q.map(|q| q.rsplit("::").next().unwrap_or(&q).to_string());
            Some((name?, owner))
        }
        "template_function" => n.child_by_field_name("name").and_then(|x| def_name(w, x)),
        _ => Some((w.text(n).to_string(), None)),
    }
}

fn params(w: &mut Walker, func_decl: Node, idx: u32) {
    let Some(params) = func_decl.child_by_field_name("parameters") else { return };
    for p in children(params) {
        if !matches!(p.kind(), "parameter_declaration" | "optional_parameter_declaration") {
            continue;
        }
        let ty = p.child_by_field_name("type").and_then(|t| w.type_name(t));
        let Some(d) = p.child_by_field_name("declarator") else { continue };
        if let (Some(nm), None, _) = declarator(d) {
            w.add_binding(BindingScope::Def(idx), w.text(nm), ty.map(TypeHint::Named).unwrap_or(TypeHint::Unknown));
        }
    }
}

fn visit(w: &mut Walker, n: Node) -> bool {
    let f = |name: &str| n.child_by_field_name(name);
    match n.kind() {
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            let (Some(name_node), Some(body)) = (f("name"), f("body")) else { return false };
            let Some((name, _)) = def_name(w, name_node) else { return false };
            let kind = if n.kind() == "class_specifier" { DefKind::Class } else { DefKind::Struct };
            let idx = w.add_def(&name, kind, n, None, None);
            w.scopes.push(idx);
            for ch in children(n).into_iter().filter(|c| c.kind() == "base_class_clause") {
                for base in children(ch) {
                    if matches!(base.kind(), "type_identifier" | "qualified_identifier" | "template_type") {
                        w.add_inherit(base, &name);
                    }
                }
            }
            w.scopes.pop();
            w.in_scope(Some(idx), Some(name), Some(body));
            true
        }
        "enum_specifier" => {
            if let (Some(nm), Some(_)) = (f("name"), f("body")) {
                let name = w.text(nm).to_string();
                w.add_def(&name, DefKind::Enum, n, None, None);
            }
            true
        }
        "alias_declaration" | "type_definition" => {
            let nm = f("name").or_else(|| f("declarator").filter(|d| d.kind() == "type_identifier"));
            if let Some(nm) = nm {
                let name = w.text(nm).to_string();
                // `using Core = ns::JobCore;` / `typedef ns::JobCore Core;`: keep the
                // target so a receiver typed `Core` resolves to JobCore's methods.
                let target = f("type")
                    .map(|t| if t.kind() == "type_descriptor" { t.child_by_field_name("type").unwrap_or(t) } else { t })
                    .and_then(|t| w.type_name(t))
                    .filter(|t| *t != name);
                w.add_def(&name, DefKind::Type, n, None, target);
            }
            false
        }
        "function_definition" => {
            let Some(d) = f("declarator") else { return false };
            let (Some(name_node), Some(fdecl), _) = declarator(d) else { return false };
            let Some((name, explicit_owner)) = def_name(w, name_node) else { return false };
            let class = w.enclosing_type_def().map(|d| d.name.clone());
            let owner = explicit_owner.clone().or(class);
            let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
            let ret = f("type").and_then(|t| w.type_name(t));
            let idx = w.add_def_full(&name, kind, n, owner, ret, false);
            params(w, fdecl, idx);
            w.scopes.push(idx);
            let pushed = explicit_owner.is_some();
            if let Some(o) = explicit_owner {
                w.owners.push(o);
            }
            for ch in children(n) {
                if matches!(ch.kind(), "field_initializer_list" | "compound_statement" | "try_statement") {
                    w.visit(ch);
                }
            }
            if pushed {
                w.owners.pop();
            }
            w.scopes.pop();
            true
        }
        "field_declaration" | "declaration" => {
            let in_class = n.kind() == "field_declaration";
            let ty_node = f("type");
            let ty = ty_node.and_then(|t| w.type_name(t));
            let mut c = n.walk();
            let decls: Vec<Node> = n.children_by_field_name("declarator", &mut c).collect();
            // `UPROPERTY() float Health = 100.f;` keeps its default in `default_value`.
            // ...and `if (T* x = f())` condition declarations keep it in `value`.
            let default_value = f("default_value").or(f("value"));
            for d in decls {
                let (name_node, fdecl, value) = declarator(d);
                let Some(name_node) = name_node else { continue };
                if let Some(fd) = fdecl {
                    // A prototype: method declared in a class, or a free function declared in a header.
                    let Some((name, explicit_owner)) = def_name(w, name_node) else { continue };
                    let owner = explicit_owner.or_else(|| if in_class { w.owners.last().cloned() } else { None });
                    let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
                    let _ = fd;
                    w.add_def_full(&name, kind, n, owner, ty.clone(), true);
                    continue;
                }
                let value = value.or(default_value);
                let hint = ty.clone().map(TypeHint::Named).or_else(|| value.and_then(|v| w.value_hint(v)));
                if let Some(h) = hint {
                    let name = w.text(name_node).to_string();
                    if in_class {
                        if let Some(owner) = w.owners.last().cloned() {
                            w.add_binding(BindingScope::Owner(owner), &name, h);
                        }
                    } else {
                        let s = w.binding_scope();
                        w.add_binding(s, &name, h);
                    }
                }
                w.visit_opt(value);
            }
            // Unreal reflection attributes on properties aren't defs; don't let them leak to the next def.
            w.discard_attrs(n);
            // Anonymous struct/class bodies inside a declaration still define things.
            if let Some(t) = ty_node.filter(|t| t.child_by_field_name("body").is_some()) {
                w.visit(t);
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
                    "field_expression" => w.visit_opt(func.child_by_field_name("argument")),
                    "identifier" | "qualified_identifier" | "template_function" => {}
                    _ => w.visit(func),
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "new_expression" => {
            if let Some(t) = f("type") {
                let (name, qualifier) = w.split_callee(t);
                if let Some(name) = name {
                    w.add_ref(RefKind::New, &name, qualifier, n);
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "pointer_expression" => {
            // `&AFoo::Bar` handed to BindAction/AddDynamic/SetTimer: a deferred call.
            if let Some(arg) = f("argument").filter(|a| a.kind() == "qualified_identifier") {
                if w.text(n).starts_with('&') {
                    let (name, qualifier) = w.split_callee(arg);
                    if let Some(name) = name {
                        w.add_ref(RefKind::Call, &name, qualifier, n);
                    }
                    return true;
                }
            }
            false
        }
        "preproc_include" => {
            if let Some(p) = f("path").filter(|p| p.kind() == "string_literal") {
                let module = w.text(p).trim_matches('"').to_string();
                w.facts.imports.push(Import { module, names: vec![], alias: None, wildcard: true, reexport: true, line: line(n) });
            }
            true
        }
        _ => false,
    }
}

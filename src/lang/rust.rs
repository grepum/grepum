//! Rust: functions, `impl` methods (owned by the impl's type, across files),
//! structs, enums, traits, `use` trees and `crate::`/`super::` module paths.

use super::{strip_ext, Language, Modules, Rules, Target};
use crate::extract::{children, line, Walker};
use crate::facts::*;
use crate::graph::GFile;
use std::collections::HashMap;
use tree_sitter::Node;

pub struct Rust;

static RULES: Rules = Rules {
    wrapper_types: &["Option", "Box", "Arc", "Rc", "RefCell", "Cell", "Mutex", "RwLock", "Weak", "Pin"],
    ..Rules::DEFAULT
};

impl Language for Rust {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn lang_of_extension(&self, ext: &str) -> Option<Lang> {
        (ext == "rs").then_some(Lang::Rust)
    }

    fn grammar(&self, _: Lang) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn visit(&self, w: &mut Walker, n: Node) -> bool {
        visit(w, n)
    }

    fn leading_kinds(&self) -> &'static [&'static str] {
        &["line_comment", "block_comment", "attribute_item"]
    }

    fn rules(&self) -> &'static Rules {
        &RULES
    }

    fn modules(&self, files: &[GFile], _: &[(String, Vec<u8>)]) -> Box<dyn Modules> {
        // Module path within its crate (`db/pool` for src/db/pool.rs or src/db/pool/mod.rs).
        let mut by_path: HashMap<String, Vec<u32>> = HashMap::new();
        for (fi, f) in files.iter().enumerate().filter(|(_, f)| f.lang == Lang::Rust) {
            let p = strip_ext(&f.path);
            let rel = p.rsplit_once("src/").map(|(_, r)| r).unwrap_or(p);
            let rel = rel.strip_suffix("/mod").unwrap_or(rel);
            let key = if rel == "lib" || rel == "main" { String::new() } else { rel.to_string() };
            by_path.entry(key).or_default().push(fi as u32);
        }
        Box::new(RustModules { by_path })
    }
}

struct RustModules {
    by_path: HashMap<String, Vec<u32>>,
}

impl Modules for RustModules {
    fn import(&self, files: &[GFile], from: u32, imp: &Import, name: Option<&str>) -> Target {
        let full = match name {
            Some(n) if imp.module.is_empty() => n.to_string(),
            Some(n) => format!("{}::{n}", imp.module),
            None => imp.module.clone(),
        };
        if let Some(f) = self.module(files, from, &full) {
            return Target::Module(full, f);
        }
        match (name, self.module(files, from, &imp.module)) {
            (Some(n), Some(f)) => Target::Symbol(f, n.to_string()),
            _ => Target::External,
        }
    }

    /// `crate::a::b`, `self::x`, `super::y`: files of that module, preferring the importer's crate.
    fn module(&self, files: &[GFile], from: u32, path: &str) -> Option<Vec<u32>> {
        let segs: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        let me = &files[from as usize].path;
        let my_rel = strip_ext(me).rsplit_once("src/").map(|(_, r)| r.to_string()).unwrap_or_default();
        let my_rel = my_rel.strip_suffix("/mod").unwrap_or(&my_rel).to_string();
        let mut cur: Vec<String> = match segs.first() {
            Some(&"crate") => vec![],
            Some(&"self") => my_rel.split('/').filter(|s| !s.is_empty() && *s != "lib" && *s != "main").map(String::from).collect(),
            Some(&"super") => {
                let mut v: Vec<String> = my_rel.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
                v.pop();
                v
            }
            _ => return None,
        };
        for s in &segs[1..] {
            if *s == "super" {
                cur.pop();
            } else {
                cur.push(s.to_string());
            }
        }
        let found = self.by_path.get(&cur.join("/"))?;
        let crate_root = me.rsplit_once("src/").map(|(c, _)| c).unwrap_or("");
        let same: Vec<u32> = found.iter().copied().filter(|f| files[*f as usize].path.starts_with(crate_root)).collect();
        Some(if same.is_empty() { found.clone() } else { same })
    }

    fn is_module_path(&self, q: &str) -> bool {
        let head = q.split("::").next().unwrap_or("");
        matches!(head, "crate" | "super") || (head == "self" && q.contains("::"))
    }
}

// ---------- syntax ----------

fn use_tree(w: &mut Walker, n: Node, prefix: &str, line_no: u32) {
    let join = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { format!("{a}::{b}") };
    match n.kind() {
        "scoped_identifier" | "identifier" | "crate" | "self" | "super" => {
            let full = join(prefix, w.text(n));
            push_import(w, &full, None, false, line_no);
        }
        "use_as_clause" => {
            if let (Some(p), Some(a)) = (n.child_by_field_name("path"), n.child_by_field_name("alias")) {
                let full = join(prefix, w.text(p));
                let alias = w.text(a).to_string();
                push_import(w, &full, Some(alias), false, line_no);
            }
        }
        "scoped_use_list" => {
            let base = n.child_by_field_name("path").map(|p| join(prefix, w.text(p))).unwrap_or(prefix.to_string());
            if let Some(list) = n.child_by_field_name("list") {
                for item in children(list) {
                    use_tree(w, item, &base, line_no);
                }
            }
        }
        "use_list" => {
            for item in children(n) {
                use_tree(w, item, prefix, line_no);
            }
        }
        "use_wildcard" => {
            let path = children(n).first().map(|p| join(prefix, w.text(*p))).unwrap_or(prefix.to_string());
            push_import(w, &path, None, true, line_no);
        }
        _ => {}
    }
}

fn push_import(w: &mut Walker, full: &str, alias: Option<String>, wildcard: bool, line_no: u32) {
    let (module, names) = if wildcard {
        (full.to_string(), vec![])
    } else {
        match full.rsplit_once("::") {
            Some((m, name)) if name != "self" => (m.to_string(), vec![ImportName { name: name.to_string(), alias }]),
            Some((m, _)) => (m.to_string(), vec![]),
            None => (String::new(), vec![ImportName { name: full.to_string(), alias }]),
        }
    };
    w.facts.imports.push(Import { module, names, alias: None, wildcard, reexport: false, line: line_no });
}

fn visit(w: &mut Walker, n: Node) -> bool {
    let f = |name: &str| n.child_by_field_name(name);
    match n.kind() {
        "function_item" | "function_signature_item" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            // A fn item nested inside another fn is not a method of the impl.
            let in_fn = w.scope().is_some_and(|s| w.facts.defs[s as usize].kind.is_callable());
            let owner = if in_fn { None } else { w.owners.last().cloned() };
            let kind = if owner.is_some() { DefKind::Method } else { DefKind::Function };
            let ret = f("return_type").and_then(|r| w.type_name(r));
            let ret = match (ret.as_deref(), &owner) {
                (Some("Self"), Some(o)) => Some(o.clone()),
                _ => ret,
            };
            let idx = w.add_def(name, kind, n, owner, ret);
            if let Some(params) = f("parameters") {
                for p in children(params).into_iter().filter(|p| p.kind() == "parameter") {
                    let pat = p.child_by_field_name("pattern").filter(|x| x.kind() == "identifier");
                    let ty = p.child_by_field_name("type").and_then(|t| w.type_name(t));
                    if let Some(pat) = pat {
                        w.add_binding(BindingScope::Def(idx), w.text(pat), ty.map(TypeHint::Named).unwrap_or(TypeHint::Unknown));
                    }
                }
            }
            w.in_scope(Some(idx), None, f("body"));
            true
        }
        "impl_item" => {
            let owner = f("type").and_then(|t| w.type_name(t));
            if let (Some(tr), Some(o)) = (f("trait"), owner.as_deref()) {
                w.add_inherit(tr, o);
            }
            w.in_scope(None, owner, f("body"));
            true
        }
        "trait_item" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let idx = w.add_def(name, DefKind::Trait, n, None, None);
            w.in_scope(Some(idx), Some(name.to_string()), f("body"));
            true
        }
        "struct_item" | "union_item" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            w.add_def(name, DefKind::Struct, n, None, None);
            if let Some(body) = f("body") {
                for fd in children(body).into_iter().filter(|c| c.kind() == "field_declaration") {
                    let fname = fd.child_by_field_name("name").map(|x| w.text(x));
                    let fty = fd.child_by_field_name("type").and_then(|t| w.type_name(t));
                    if let (Some(fname), Some(fty)) = (fname, fty) {
                        w.add_binding(BindingScope::Owner(name.to_string()), fname, TypeHint::Named(fty));
                    }
                }
            }
            true
        }
        "enum_item" | "type_item" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let kind = if n.kind() == "enum_item" { DefKind::Enum } else { DefKind::Type };
            w.add_def(name, kind, n, None, None);
            true
        }
        "mod_item" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let idx = w.add_def(name, DefKind::Module, n, None, None);
            w.in_scope(Some(idx), None, f("body"));
            true
        }
        "call_expression" => {
            if let Some(func) = f("function") {
                let (name, qualifier) = w.split_callee(func);
                if let Some(name) = name {
                    w.add_ref(RefKind::Call, &name, qualifier, n);
                }
                match func.kind() {
                    "field_expression" => w.visit_opt(func.child_by_field_name("value")),
                    "identifier" | "scoped_identifier" => {}
                    _ => w.visit(func),
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "struct_expression" => {
            if let Some(nm) = f("name") {
                let (name, qualifier) = w.split_callee(nm);
                if let Some(name) = name {
                    w.add_ref(RefKind::New, &name, qualifier, n);
                }
            }
            w.visit_opt(f("body"));
            true
        }
        "let_declaration" => {
            let pat = f("pattern").filter(|p| p.kind() == "identifier");
            let hint = f("type")
                .and_then(|t| w.type_name(t).map(TypeHint::Named))
                .or_else(|| f("value").and_then(|v| w.value_hint(v)));
            if let (Some(p), Some(h)) = (pat, hint) {
                let s = w.binding_scope();
                w.add_binding(s, w.text(p), h);
            }
            w.visit_opt(f("value"));
            w.visit_opt(f("alternative"));
            true
        }
        "use_declaration" => {
            if let Some(arg) = f("argument") {
                use_tree(w, arg, "", line(n));
            }
            true
        }
        "macro_invocation" => true,
        _ => false,
    }
}

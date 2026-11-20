//! Go: functions, methods (owned by their receiver type), structs and
//! interfaces, and packages: an import names a directory, and every file in
//! a directory shares one scope.

use super::{dir_of, Language, Modules, Rules, Target};
use crate::extract::{children, line, Walker};
use crate::facts::*;
use crate::graph::GFile;
use std::collections::HashMap;
use tree_sitter::Node;

pub struct Go;

/// Standard-library methods (buffers, builders, locks, wait groups, atomics, timers).
const STDLIB_METHODS: &[&str] = &[
    "Reset", "Len", "Cap", "String", "Bytes", "Write", "WriteString", "WriteByte", "WriteRune", "Read",
    "ReadString", "ReadByte", "Close", "Flush", "Lock", "Unlock", "RLock", "RUnlock", "Add", "Done", "Wait",
    "Store", "Load", "Swap", "CompareAndSwap", "Stop", "Err", "Error", "Value", "Get", "Set", "Del", "Sum",
];

static RULES: Rules = Rules {
    builtin_methods: &[super::COMMON_BUILTIN_METHODS, STDLIB_METHODS],
    package_scope: true,
    ..Rules::DEFAULT
};

impl Language for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn lang_of_extension(&self, ext: &str) -> Option<Lang> {
        (ext == "go").then_some(Lang::Go)
    }

    fn grammar(&self, _: Lang) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn visit(&self, w: &mut Walker, n: Node) -> bool {
        visit(w, n)
    }

    fn rules(&self) -> &'static Rules {
        &RULES
    }

    fn modules(&self, files: &[GFile], _: &[(String, Vec<u8>)]) -> Box<dyn Modules> {
        let mut packages: HashMap<String, Vec<u32>> = HashMap::new();
        for (fi, f) in files.iter().enumerate().filter(|(_, f)| f.lang == Lang::Go) {
            packages.entry(dir_of(&f.path).to_string()).or_default().push(fi as u32);
        }
        Box::new(GoModules { packages })
    }
}

struct GoModules {
    /// Directory -> its Go files.
    packages: HashMap<String, Vec<u32>>,
}

impl Modules for GoModules {
    /// `example.com/app/internal/db` names the deepest directory it ends with.
    fn import(&self, _: &[GFile], _: u32, imp: &Import, _: Option<&str>) -> Target {
        let mut best: Option<(&String, &Vec<u32>)> = None;
        for (dir, files) in &self.packages {
            if dir.is_empty() {
                continue;
            }
            if (imp.module == *dir || imp.module.ends_with(&format!("/{dir}"))) && best.is_none_or(|(b, _)| dir.len() > b.len()) {
                best = Some((dir, files));
            }
        }
        match best {
            Some((_, f)) => Target::Module(imp.module.clone(), f.clone()),
            None => Target::External,
        }
    }
}

// ---------- syntax ----------

fn params(w: &mut Walker, params: Option<Node>, idx: u32) {
    let Some(params) = params else { return };
    for p in children(params).into_iter().filter(|p| p.kind() == "parameter_declaration") {
        let ty = p.child_by_field_name("type").and_then(|t| w.type_name(t));
        let mut c = p.walk();
        let names: Vec<_> = p.children_by_field_name("name", &mut c).collect();
        for nm in names {
            w.add_binding(BindingScope::Def(idx), w.text(nm), ty.clone().map(TypeHint::Named).unwrap_or(TypeHint::Unknown));
        }
    }
}

fn visit(w: &mut Walker, n: Node) -> bool {
    let f = |name: &str| n.child_by_field_name(name);
    match n.kind() {
        "function_declaration" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let ret = f("result").and_then(|r| w.type_name(r));
            let idx = w.add_def(name, DefKind::Function, n, None, ret);
            params(w, f("parameters"), idx);
            w.in_scope(Some(idx), None, f("body"));
            true
        }
        "method_declaration" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let recv = f("receiver")
                .and_then(|r| children(r).into_iter().find(|p| p.kind() == "parameter_declaration"));
            let owner = recv.and_then(|r| r.child_by_field_name("type")).and_then(|t| w.type_name(t));
            let ret = f("result").and_then(|r| w.type_name(r));
            let idx = w.add_def(name, DefKind::Method, n, owner.clone(), ret);
            if let (Some(r), Some(o)) = (recv, owner.clone()) {
                if let Some(rn) = r.child_by_field_name("name") {
                    w.add_binding(BindingScope::Def(idx), w.text(rn), TypeHint::Named(o));
                }
            }
            params(w, f("parameters"), idx);
            w.in_scope(Some(idx), owner, f("body"));
            true
        }
        "type_spec" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let ty = f("type");
            let kind = match ty.map(|t| t.kind()) {
                Some("struct_type") => DefKind::Struct,
                Some("interface_type") => DefKind::Interface,
                _ => DefKind::Type,
            };
            w.add_def(name, kind, n, None, None);
            if let Some(st) = ty.filter(|t| t.kind() == "struct_type") {
                for list in children(st).into_iter().filter(|c| c.kind() == "field_declaration_list") {
                    for fd in children(list).into_iter().filter(|c| c.kind() == "field_declaration") {
                        let Some(fty) = fd.child_by_field_name("type").and_then(|t| w.type_name(t)) else { continue };
                        let mut c = fd.walk();
                        let names: Vec<_> = fd.children_by_field_name("name", &mut c).collect();
                        for nm in names {
                            w.add_binding(BindingScope::Owner(name.to_string()), w.text(nm), TypeHint::Named(fty.clone()));
                        }
                    }
                }
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
                    "selector_expression" => w.visit_opt(func.child_by_field_name("operand")),
                    "identifier" => {}
                    _ => w.visit(func),
                }
            }
            w.visit_opt(f("arguments"));
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
            let declared = if n.kind() == "var_spec" { f("type").and_then(|t| w.type_name(t)) } else { None };
            for (i, l) in lefts.iter().enumerate() {
                if l.kind() != "identifier" {
                    continue;
                }
                let hint = declared
                    .clone()
                    .map(TypeHint::Named)
                    .or_else(|| rights.get(i).and_then(|r| w.value_hint(*r)));
                if let Some(h) = hint {
                    let s = w.binding_scope();
                    w.add_binding(s, w.text(*l), h);
                }
            }
            for r in rights {
                w.visit(r);
            }
            true
        }
        "import_spec" => {
            let Some(path) = f("path") else { return true };
            let module = w.text(path).trim_matches('"').to_string();
            let alias = f("name").map(|x| w.text(x).to_string()).or_else(|| {
                let mut segs = module.rsplit('/');
                let last = segs.next().unwrap_or("");
                let is_version = last.len() > 1 && last.starts_with('v') && last[1..].chars().all(|c| c.is_ascii_digit());
                Some(if is_version { segs.next().unwrap_or(last) } else { last }.to_string())
            });
            w.facts.imports.push(Import { module, names: vec![], alias, wildcard: false, reexport: false, line: line(n) });
            true
        }
        _ => false,
    }
}

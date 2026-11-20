//! Python: classes, functions and methods; `import`/`from ... import`
//! (relative, package re-exports, `import a.b` binding `a`); parameter, local
//! and `self.x = ...` attribute types.

use super::{dir_of, strip_ext, Language, Modules, Rules, Target};
use crate::extract::{children, line, RefSite, Reference, Walker};
use crate::facts::*;
use crate::graph::GFile;
use std::collections::HashMap;
use tree_sitter::Node;

pub struct Python;

static RULES: Rules = Rules {
    wrapper_types: &["Optional", "Union", "Type", "type", "Final", "ClassVar", "Annotated", "Awaitable"],
    methods_in_class_file: true,
    import_binds_root: true,
    ..Rules::DEFAULT
};

impl Language for Python {
    fn name(&self) -> &'static str {
        "python"
    }

    fn lang_of_extension(&self, ext: &str) -> Option<Lang> {
        matches!(ext, "py" | "pyi").then_some(Lang::Python)
    }

    fn grammar(&self, _: Lang) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn visit(&self, w: &mut Walker, n: Node) -> bool {
        visit(w, n)
    }

    fn wrapper_kinds(&self) -> &'static [&'static str] {
        &["decorated_definition"]
    }

    fn rules(&self) -> &'static Rules {
        &RULES
    }

    fn references(&self, w: &Walker, site: &RefSite, out: &mut Vec<Reference>) -> bool {
        references(w, site, out)
    }

    fn modules(&self, files: &[GFile], _: &[(String, Vec<u8>)]) -> Box<dyn Modules> {
        // Every suffix of the dotted path is a candidate module name; the
        // longest match (fewest skipped leading dirs) wins.
        let mut by_key: HashMap<String, Vec<(u32, u8)>> = HashMap::new();
        for (fi, f) in files.iter().enumerate().filter(|(_, f)| f.lang == Lang::Python) {
            let mut p = strip_ext(&f.path);
            if let Some(pkg) = p.strip_suffix("/__init__") {
                p = pkg;
            }
            let segs: Vec<&str> = p.split('/').collect();
            for skip in 0..segs.len() {
                by_key.entry(segs[skip..].join(".")).or_default().push((fi as u32, skip as u8));
            }
        }
        Box::new(PyModules { by_key })
    }
}

struct PyModules {
    by_key: HashMap<String, Vec<(u32, u8)>>,
}

impl PyModules {
    fn find(&self, key: &str) -> Option<Vec<u32>> {
        let cands = self.by_key.get(key)?;
        let best = cands.iter().map(|(_, s)| *s).min()?;
        Some(cands.iter().filter(|(_, s)| *s == best).map(|(f, _)| *f).collect())
    }

    /// `.models` / `..core.db` relative to the importing file's package.
    fn absolute(files: &[GFile], file: u32, module: &str) -> String {
        let dots = module.chars().take_while(|c| *c == '.').count();
        if dots == 0 {
            return module.to_string();
        }
        let path = &files[file as usize].path;
        let mut pkg: Vec<&str> = dir_of(path).split('/').filter(|s| !s.is_empty()).collect();
        for _ in 1..dots {
            pkg.pop();
        }
        let rest = &module[dots..];
        if !rest.is_empty() {
            pkg.push(rest);
        }
        pkg.join(".")
    }
}

impl Modules for PyModules {
    fn import(&self, files: &[GFile], from: u32, imp: &Import, name: Option<&str>) -> Target {
        let abs = Self::absolute(files, from, &imp.module);
        match name {
            None => match self.find(&abs) {
                Some(f) => Target::Module(abs, f),
                None => Target::External,
            },
            Some(n) => {
                // `from pkg import sub` may name a submodule rather than a symbol.
                let sub = if abs.is_empty() { n.to_string() } else { format!("{abs}.{n}") };
                if let Some(f) = self.find(&sub) {
                    return Target::Module(sub, f);
                }
                match self.find(&abs) {
                    Some(f) => Target::Symbol(f, n.to_string()),
                    None => Target::External,
                }
            }
        }
    }

    fn module(&self, _: &[GFile], _: u32, path: &str) -> Option<Vec<u32>> {
        self.find(path)
    }

    fn submodule(&self, key: &str, seg: &str) -> Option<Vec<u32>> {
        self.find(&format!("{key}.{seg}"))
    }
}

// ---------- syntax ----------

fn visit(w: &mut Walker, n: Node) -> bool {
    let f = |name: &str| n.child_by_field_name(name);
    match n.kind() {
        "class_definition" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let idx = w.add_def(name, DefKind::Class, n, None, None);
            if let Some(sup) = f("superclasses") {
                w.scopes.push(idx);
                for b in children(sup) {
                    if matches!(b.kind(), "identifier" | "attribute") {
                        w.add_inherit(b, name);
                    }
                }
                w.scopes.pop();
            }
            w.in_scope(Some(idx), Some(name.to_string()), f("body"));
            true
        }
        "function_definition" => {
            let Some(name) = f("name").map(|x| w.text(x)) else { return false };
            let class = w.enclosing_type_def().map(|d| d.name.clone());
            let kind = if class.is_some() { DefKind::Method } else { DefKind::Function };
            let ret = f("return_type").and_then(|r| w.type_name(r));
            let idx = w.add_def(name, kind, n, class, ret);
            if let Some(params) = f("parameters") {
                for p in children(params) {
                    let (pname, pty) = match p.kind() {
                        "typed_parameter" => (children(p).first().copied(), p.child_by_field_name("type")),
                        "typed_default_parameter" => (p.child_by_field_name("name"), p.child_by_field_name("type")),
                        "default_parameter" => (p.child_by_field_name("name"), None),
                        "identifier" => (Some(p), None),
                        _ => (None, None),
                    };
                    let pty = pty.map(|t| w.type_name(t).unwrap_or_else(|| format!("<{}>", w.text(t))));
                    if let Some(pn) = pname.filter(|n| n.kind() == "identifier") {
                        let pn = w.text(pn);
                        w.add_binding(BindingScope::Def(idx), pn, pty.map(TypeHint::Named).unwrap_or(TypeHint::Unknown));
                    }
                }
            }
            w.in_scope(Some(idx), None, f("body"));
            true
        }
        "call" => {
            if let Some(func) = f("function") {
                let (name, qualifier) = w.split_callee(func);
                if let Some(name) = name {
                    w.add_ref(RefKind::Call, &name, qualifier, n);
                }
                if func.kind() == "attribute" {
                    w.visit_opt(func.child_by_field_name("object"));
                } else if func.kind() != "identifier" {
                    w.visit(func);
                }
            }
            w.visit_opt(f("arguments"));
            true
        }
        "assignment" => {
            if let Some(left) = f("left") {
                let hint = f("type")
                    .and_then(|t| w.type_name(t).map(TypeHint::Named))
                    .or_else(|| f("right").and_then(|r| w.value_hint(r)));
                match left.kind() {
                    "identifier" => {
                        if let Some(hint) = hint {
                            let s = w.binding_scope();
                            w.add_binding(s, w.text(left), hint);
                        }
                        // `x = 0` / `x: int` in a class body: a class attribute.
                        if let Some(class) = w.enclosing_type_def().map(|d| d.name.clone()) {
                            w.add_field(&class, w.text(left), n, None);
                        }
                    }
                    "attribute" => {
                        let obj = left.child_by_field_name("object").map(|o| w.text(o));
                        let attr = left.child_by_field_name("attribute").map(|a| w.text(a));
                        if let (Some("self"), Some(attr), Some(owner)) = (obj, attr, w.owners.last().cloned()) {
                            let hint = hint.map(|h| w.resolve_alias_now(h));
                            w.add_field(&owner, attr, n, hint);
                        }
                    }
                    _ => {}
                }
            }
            w.visit_opt(f("right"));
            true
        }
        "import_statement" => {
            for ch in children(n) {
                let (module, alias) = match ch.kind() {
                    "dotted_name" => (w.text(ch), None),
                    "aliased_import" => (
                        ch.child_by_field_name("name").map(|x| w.text(x)).unwrap_or(""),
                        ch.child_by_field_name("alias").map(|x| w.text(x).to_string()),
                    ),
                    _ => continue,
                };
                w.facts.imports.push(Import {
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
            let module = f("module_name").map(|m| w.text(m).to_string()).unwrap_or_default();
            let mut names = vec![];
            let mut wildcard = false;
            let mut c = n.walk();
            for ch in n.children_by_field_name("name", &mut c) {
                match ch.kind() {
                    "dotted_name" => names.push(ImportName { name: w.text(ch).to_string(), alias: None }),
                    "aliased_import" => names.push(ImportName {
                        name: ch.child_by_field_name("name").map(|x| w.text(x).to_string()).unwrap_or_default(),
                        alias: ch.child_by_field_name("alias").map(|x| w.text(x).to_string()),
                    }),
                    _ => {}
                }
            }
            if children(n).iter().any(|c| c.kind() == "wildcard_import") {
                wildcard = true;
            }
            w.facts.imports.push(Import { module, names, alias: None, wildcard, reexport: true, line: line(n) });
            true
        }
        _ => false,
    }
}

fn references(w: &Walker, site: &RefSite, out: &mut Vec<Reference>) -> bool {
    let n = site.node;
    match n.kind() {
        "import_statement" | "import_from_statement" | "future_import_statement" => false,
        // Annotations: `Repo`, `db.Pool`, `list[Repo] | None`, and forward references `"Repo"`.
        "type" => {
            annotation(w, n, out);
            false
        }
        "attribute" => {
            if !site.is_callee() {
                out.extend(w.member_ref(n));
            }
            true
        }
        _ => true,
    }
}

fn annotation(w: &Walker, n: Node, out: &mut Vec<Reference>) {
    match n.kind() {
        "identifier" | "attribute" => out.extend(w.type_ref(n)),
        "string" => {
            let text = w.text(n).trim_matches(|c| c == '"' || c == '\'');
            if !text.is_empty() && text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                let (qualifier, name) = match text.rsplit_once('.') {
                    Some((q, nm)) => (Some(q.to_string()), nm),
                    None => (None, text),
                };
                out.push(Reference { kind: RefKind::Type, name: name.to_string(), qualifier, line: line(n) });
            }
        }
        _ => {
            for c in children(n) {
                annotation(w, c, out);
            }
        }
    }
}

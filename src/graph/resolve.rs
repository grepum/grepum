//! Cross-file resolution: turns a set of (path, facts) into a linked graph.
//!
//! Resolution is deliberately conservative. A call edge is `Exact` when the
//! target is proven by scope, imports or receiver types, `Inferred` when the
//! name is globally unique, and dropped when it is ambiguous. A missing edge
//! is cheaper for an agent than a confidently wrong one.

use super::*;
use crate::lang::{self, dir_of, Modules, Rules, Target};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
enum Thing {
    Types(Vec<u32>),
    Module(String, Vec<u32>),
    Unknown,
    External,
}

/// What a call names: `qualifier.name(...)`, on `line` (0 when unknown: then
/// only parameters, not later locals, shadow the name).
#[derive(Clone, Copy)]
struct Callee<'c> {
    name: &'c str,
    qualifier: Option<&'c str>,
    line: u32,
}

struct FileCtx {
    imports: HashMap<String, Target>,
    wildcards: Vec<Vec<u32>>,
    reexports: Vec<(Option<String>, Target)>, // (exported name or None for `*`, target)
    bindings: HashMap<(Option<u32>, String), TypeHint>,
    /// (function, local name) -> first line it is bound on (0 for parameters).
    locals: HashMap<(u32, String), u32>,
}

struct Resolver<'a> {
    inputs: &'a [Input],
    files: Vec<GFile>,
    defs: Vec<GDef>,
    by_name: HashMap<String, Vec<u32>>,
    top_level: HashMap<(u32, String), Vec<u32>>,
    methods: HashMap<(String, String), Vec<u32>>,
    owner_fields: HashMap<(String, String), Vec<(u32, TypeHint)>>,
    dirs: HashMap<String, Vec<u32>>,
    /// Each language's module index, by language name.
    modules: HashMap<&'static str, Box<dyn Modules>>,
    ctx: Vec<FileCtx>,
    bases: HashMap<u32, Vec<u32>>,
    /// (enclosing function, name) -> functions nested in it.
    nested: HashMap<(u32, String), Vec<u32>>,
}

fn qual_segments(q: &str) -> Vec<&str> {
    q.split("::").flat_map(|s| s.split('.')).filter(|s| !s.is_empty()).collect()
}

pub(super) fn link(inputs: Vec<Input>, project: &[(String, Vec<u8>)]) -> Graph {
    let mut r = Resolver::new(&inputs, project);
    r.build_contexts();
    r.resolve_inherits();
    let per_file: Vec<(Vec<Edge>, Stats)> = (0..inputs.len()).into_par_iter().map(|fi| r.resolve_file(fi as u32)).collect();
    let mut stats = Stats::default();
    let mut edges = vec![];
    let mut subclasses: Vec<&u32> = r.bases.keys().collect();
    subclasses.sort();
    for bi in subclasses {
        let d = &r.defs[*bi as usize];
        for b in &r.bases[bi] {
            edges.push(Edge { from_file: d.file, from_def: Some(*bi), to: *b, kind: EdgeKind::Inherit, conf: Conf::Exact, line: d.line });
        }
    }
    for (e, s) in per_file {
        edges.extend(e);
        stats.refs += s.refs;
        stats.exact += s.exact;
        stats.inferred += s.inferred;
        stats.ambiguous += s.ambiguous;
        stats.unresolved += s.unresolved;
    }
    let mut imports: Vec<(u32, u32)> = vec![];
    for (fi, c) in r.ctx.iter().enumerate() {
        let mut seen = HashSet::new();
        for t in c.imports.values().chain(c.reexports.iter().map(|(_, t)| t)) {
            let files = match t {
                Target::Module(_, f) | Target::Symbol(f, _) => f,
                Target::External => continue,
            };
            for f in files {
                if *f != fi as u32 && seen.insert(*f) {
                    imports.push((fi as u32, *f));
                }
            }
        }
        for w in &c.wildcards {
            for f in w {
                if seen.insert(*f) {
                    imports.push((fi as u32, *f));
                }
            }
        }
    }
    imports.sort();
    Graph { files: r.files, defs: r.defs, edges, imports, stats }
}

impl<'a> Resolver<'a> {
    fn new(inputs: &'a [Input], project: &[(String, Vec<u8>)]) -> Self {
        let mut files = Vec::with_capacity(inputs.len());
        let mut defs = Vec::new();
        for (fi, inp) in inputs.iter().enumerate() {
            let first = defs.len() as u32;
            for d in &inp.facts.defs {
                let parent = d.parent.map(|p| first + p);
                let qual = match (parent, &d.owner) {
                    (Some(p), _) => format!("{}.{}", defs.get(p as usize).map(|x: &GDef| x.qual.as_str()).unwrap_or(""), d.name),
                    (None, Some(o)) => format!("{o}.{}", d.name),
                    _ => d.name.clone(),
                };
                defs.push(GDef {
                    file: fi as u32,
                    name: d.name.clone(),
                    qual,
                    kind: d.kind,
                    line: d.line,
                    end_line: d.end_line,
                    parent,
                    owner: d.owner.clone(),
                    decl: d.decl,
                    attrs: d.attrs.clone(),
                });
            }
            files.push(GFile {
                path: inp.path.clone(),
                oid: inp.oid.clone(),
                lang: inp.facts.lang,
                lines: inp.facts.lines,
                first_def: first,
                def_count: inp.facts.defs.len() as u32,
            });
        }
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::new();
        let mut top_level: HashMap<(u32, String), Vec<u32>> = HashMap::new();
        let mut methods: HashMap<(String, String), Vec<u32>> = HashMap::new();
        let mut nested: HashMap<(u32, String), Vec<u32>> = HashMap::new();
        for (i, d) in defs.iter().enumerate() {
            let i = i as u32;
            by_name.entry(d.name.clone()).or_default().push(i);
            if let Some(p) = d.parent.filter(|p| defs[*p as usize].kind.is_callable()) {
                nested.entry((p, d.name.clone())).or_default().push(i);
            }
            if let (DefKind::Method, Some(o)) = (d.kind, &d.owner) {
                methods.entry((o.clone(), d.name.clone())).or_default().push(i);
            } else if d.parent.is_none() || d.kind.is_type() {
                top_level.entry((d.file, d.name.clone())).or_default().push(i);
            }
        }
        let mut owner_fields: HashMap<(String, String), Vec<(u32, TypeHint)>> = HashMap::new();
        let mut dirs: HashMap<String, Vec<u32>> = HashMap::new();
        for (fi, inp) in inputs.iter().enumerate() {
            let fi = fi as u32;
            for b in &inp.facts.bindings {
                if let BindingScope::Owner(o) = &b.scope {
                    owner_fields.entry((o.clone(), b.name.clone())).or_default().push((fi, b.ty.clone()));
                }
            }
            dirs.entry(dir_of(&inp.path).to_string()).or_default().push(fi);
        }
        let modules = lang::ALL.iter().map(|l| (l.name(), l.modules(&files, project))).collect();
        Resolver {
            inputs,
            files,
            defs,
            by_name,
            top_level,
            methods,
            owner_fields,
            dirs,
            modules,
            ctx: vec![],
            bases: HashMap::new(),
            nested,
        }
    }

    fn rules(&self, file: u32) -> &'static Rules {
        lang::support(self.files[file as usize].lang).rules()
    }

    fn modules(&self, file: u32) -> &dyn Modules {
        self.modules[lang::support(self.files[file as usize].lang).name()].as_ref()
    }

    /// Defined inside a function body: only reachable from within it.
    fn is_nested(&self, d: u32) -> bool {
        self.defs[d as usize].parent.is_some_and(|p| self.defs[p as usize].kind.is_callable())
    }

    /// A method a receiver of unknown type could plausibly reach: one declared in
    /// a class-like type, not in a namespace object or a function body.
    fn is_member_method(&self, d: u32) -> bool {
        match self.defs[d as usize].parent {
            Some(p) => self.defs[p as usize].kind.is_type() && self.defs[p as usize].kind != DefKind::Object,
            None => true,
        }
    }

    /// Interfaces, namespace objects and TS type aliases can't be called or constructed.
    fn is_call_target(&self, d: u32) -> bool {
        let def = &self.defs[d as usize];
        match def.kind {
            DefKind::Interface | DefKind::Object => false,
            DefKind::Type => self.rules(def.file).type_aliases_callable,
            _ => true,
        }
    }

    fn call_targets(&self, targets: Vec<u32>) -> Vec<u32> {
        targets.into_iter().filter(|d| self.is_call_target(*d)).collect()
    }

    /// Whether `name`, used on line `at`, is a local (parameter or variable bound
    /// earlier) of `scope` or an enclosing function.
    fn is_local(&self, file: u32, scope: Option<u32>, name: &str, at: u32) -> bool {
        let facts = &self.inputs[file as usize].facts;
        let mut s = scope;
        while let Some(d) = s {
            if self.ctx[file as usize].locals.get(&(d, name.to_string())).is_some_and(|l| *l == 0 || *l < at) {
                return true;
            }
            s = facts.defs[d as usize].parent;
        }
        false
    }

    /// A function declared in `scope` or an enclosing function body.
    fn nested_in_scope(&self, file: u32, scope: Option<u32>, name: &str) -> Vec<u32> {
        let facts = &self.inputs[file as usize].facts;
        let mut s = scope;
        while let Some(d) = s {
            if let Some(v) = self.nested.get(&(self.global_def(file, d), name.to_string())) {
                return v.clone();
            }
            s = facts.defs[d as usize].parent;
        }
        vec![]
    }

    fn global_def(&self, file: u32, local: u32) -> u32 {
        self.files[file as usize].first_def + local
    }

    fn import_target(&self, file: u32, imp: &Import, name: Option<&str>) -> Target {
        self.modules(file).import(&self.files, file, imp, name)
    }

    fn build_contexts(&mut self) {
        let ctx: Vec<FileCtx> = (0..self.inputs.len())
            .into_par_iter()
            .map(|fi| {
                let fi = fi as u32;
                let facts = &self.inputs[fi as usize].facts;
                let rules = self.rules(fi);
                let mut c = FileCtx { imports: HashMap::new(), wildcards: vec![], reexports: vec![], bindings: HashMap::new(), locals: HashMap::new() };
                for imp in &facts.imports {
                    if imp.wildcard {
                        if let Target::Module(_, f) = self.import_target(fi, imp, None) {
                            if imp.reexport {
                                c.reexports.push((None, Target::Module(String::new(), f.clone())));
                            }
                            c.wildcards.push(f);
                        }
                        continue;
                    }
                    if imp.names.is_empty() {
                        let t = self.import_target(fi, imp, None);
                        let local = match &imp.alias {
                            Some(a) => a.clone(),
                            None if rules.import_binds_root => imp.module.split('.').next().unwrap_or("").to_string(),
                            None => continue,
                        };
                        // `import a.b` binds `a`; resolve the head module so `a.b.f()` walks down.
                        let t = if imp.alias.is_none() && rules.import_binds_root {
                            self.modules(fi).module(&self.files, fi, &local).map(|f| Target::Module(local.clone(), f)).unwrap_or(t)
                        } else {
                            t
                        };
                        if imp.reexport {
                            c.reexports.push((Some(local.clone()), t.clone()));
                        }
                        c.imports.insert(local, t);
                        continue;
                    }
                    for n in &imp.names {
                        let t = self.import_target(fi, imp, Some(&n.name));
                        let local = n.alias.clone().unwrap_or_else(|| n.name.clone());
                        if imp.reexport {
                            c.reexports.push((Some(local.clone()), t.clone()));
                        }
                        c.imports.insert(local, t);
                    }
                }
                for b in &facts.bindings {
                    let scope = match &b.scope {
                        BindingScope::File => None,
                        BindingScope::Def(d) => Some(*d),
                        BindingScope::Owner(_) => continue,
                    };
                    if let Some(d) = scope {
                        let first = c.locals.entry((d, b.name.clone())).or_insert(b.line);
                        *first = (*first).min(b.line);
                    }
                    c.bindings.insert((scope, b.name.clone()), b.ty.clone());
                }
                c
            })
            .collect();
        self.ctx = ctx;
    }

    // ---------- symbol lookup ----------

    /// Top-level symbol `name` as seen by importers of `files`, following re-exports.
    fn exported(&self, files: &[u32], name: &str, depth: u8) -> Vec<u32> {
        let mut out = vec![];
        for f in files {
            if let Some(d) = self.top_level.get(&(*f, name.to_string())) {
                out.extend(d.iter().filter(|i| self.defs[**i as usize].parent.is_none()));
            }
        }
        if !out.is_empty() || depth == 0 {
            return out;
        }
        for f in files {
            // `export { local as name }` / `export default local`.
            for ex in self.inputs[*f as usize].facts.exports.iter().filter(|e| e.exported == name) {
                match self.ctx[*f as usize].imports.get(&ex.local) {
                    Some(Target::Symbol(tf, orig)) => out.extend(self.exported(tf, orig, depth - 1)),
                    _ if ex.local != name => out.extend(self.exported(&[*f], &ex.local, depth - 1)),
                    _ => {}
                }
            }
            for (exported, target) in &self.ctx[*f as usize].reexports {
                match (exported.as_deref(), target) {
                    (Some(e), Target::Symbol(tf, orig)) if e == name => out.extend(self.exported(tf, orig, depth - 1)),
                    (None, Target::Module(_, tf)) => out.extend(self.exported(tf, name, depth - 1)),
                    _ => {}
                }
            }
        }
        if out.is_empty() && name == "default" {
            // An anonymous `export default`: fall back to the file's single top-level class/function.
            for f in files.iter().filter(|f| !self.inputs[**f as usize].facts.exports.iter().any(|e| e.exported == "default")) {
                let tops: Vec<u32> = (0..self.files[*f as usize].def_count)
                    .map(|l| self.global_def(*f, l))
                    .filter(|d| self.defs[*d as usize].parent.is_none() && self.defs[*d as usize].kind != DefKind::Method && self.is_call_target(*d))
                    .collect();
                if tops.len() == 1 {
                    out.extend(tops);
                }
            }
        }
        out
    }

    /// Types visible as `name` from `file`, with C++ type aliases followed to
    /// the type they name.
    fn types_named(&self, file: u32, name: &str) -> Vec<u32> {
        self.follow_aliases(self.types_named_raw(file, name), 4)
    }

    fn follow_aliases(&self, types: Vec<u32>, depth: u8) -> Vec<u32> {
        if depth == 0 {
            return types;
        }
        let mut out = Vec::with_capacity(types.len());
        for t in types {
            let d = &self.defs[t as usize];
            let target = (d.kind == DefKind::Type && self.rules(d.file).follow_type_aliases)
                .then(|| self.inputs[d.file as usize].facts.defs[(t - self.files[d.file as usize].first_def) as usize].ret.clone())
                .flatten();
            match target {
                Some(target) => {
                    let resolved = self.follow_aliases(self.types_named_raw(d.file, &target), depth - 1);
                    if resolved.is_empty() { out.push(t) } else { out.extend(resolved) }
                }
                None => out.push(t),
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn types_named_raw(&self, file: u32, name: &str) -> Vec<u32> {
        let local: Vec<u32> = self
            .top_level
            .get(&(file, name.to_string()))
            .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind.is_type()).collect())
            .unwrap_or_default();
        if !local.is_empty() {
            return local;
        }
        match self.ctx[file as usize].imports.get(name) {
            Some(Target::Symbol(f, orig)) => {
                return self.exported(f, orig, 4).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect()
            }
            Some(Target::External) => return vec![],
            _ => {}
        }
        for w in &self.ctx[file as usize].wildcards {
            let t: Vec<u32> = self.exported(w, name, 4).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect();
            if !t.is_empty() {
                return t;
            }
        }
        let all: Vec<u32> = self
            .by_name
            .get(name)
            .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind.is_type()).collect())
            .unwrap_or_default();
        self.narrow(file, all)
    }

    /// Pick the unique candidate, or the unique one in the caller's directory.
    fn narrow(&self, file: u32, cands: Vec<u32>) -> Vec<u32> {
        if cands.len() <= 1 {
            return cands;
        }
        let dir = dir_of(&self.files[file as usize].path);
        let near: Vec<u32> = cands.iter().copied().filter(|d| dir_of(&self.files[self.defs[*d as usize].file as usize].path) == dir).collect();
        if near.len() == 1 { near } else { vec![] }
    }

    fn methods_of(&self, types: &[u32], name: &str) -> Vec<u32> {
        let mut out = vec![];
        let mut seen = HashSet::new();
        let mut queue: Vec<u32> = types.to_vec();
        while let Some(t) = queue.pop() {
            if !seen.insert(t) || seen.len() > 32 {
                continue;
            }
            let tname = &self.defs[t as usize].name;
            if let Some(m) = self.methods.get(&(tname.clone(), name.to_string())) {
                let tfile = self.defs[t as usize].file;
                // Methods attach by owner *name*; for languages with in-class methods keep the same file.
                let filtered: Vec<u32> = if self.rules(tfile).methods_in_class_file {
                    m.iter().copied().filter(|d| self.defs[*d as usize].file == tfile).collect()
                } else {
                    m.clone()
                };
                if !filtered.is_empty() {
                    out.extend(filtered);
                    continue; // nearest definition wins (override)
                }
            }
            if let Some(b) = self.bases.get(&t) {
                queue.extend(b);
            }
        }
        out
    }

    // ---------- types of expressions ----------

    fn binding(&self, file: u32, scope: Option<u32>, name: &str) -> Option<TypeHint> {
        let facts = &self.inputs[file as usize].facts;
        let mut s = scope;
        loop {
            if let Some(h) = self.ctx[file as usize].bindings.get(&(s, name.to_string())) {
                return Some(h.clone());
            }
            s = facts.defs[s? as usize].parent;
        }
    }

    fn hint_types(&self, file: u32, scope: Option<u32>, owner: Option<&str>, hint: &TypeHint, depth: u8) -> Vec<u32> {
        if depth == 0 {
            return vec![];
        }
        match hint {
            TypeHint::Named(t) => self.types_named(file, t),
            TypeHint::Alias(a) => match self.binding(file, scope, a) {
                Some(h) => self.hint_types(file, scope, owner, &h, depth - 1),
                None => vec![],
            },
            TypeHint::CallResult { name, qualifier } => {
                let (targets, _) = self.resolve_call(file, scope, owner, &Callee { name, qualifier: qualifier.as_deref(), line: 0 }, depth - 1);
                let mut out = vec![];
                for t in targets {
                    let d = &self.defs[t as usize];
                    if d.kind.is_type() {
                        out.push(t);
                    } else if let Some(ret) = &self.inputs[d.file as usize].facts.defs[(t - self.files[d.file as usize].first_def) as usize].ret {
                        out.extend(self.types_named(d.file, ret));
                    }
                }
                out
            }
            TypeHint::Unknown => vec![],
        }
    }

    /// A binding with an explicit type name that isn't defined in the repo is
    /// an external type: don't guess a same-named local method for it.
    fn hint_thing(&self, file: u32, scope: Option<u32>, owner: Option<&str>, hint: &TypeHint, depth: u8) -> Thing {
        let ts = self.hint_types(file, scope, owner, hint, depth);
        if !ts.is_empty() {
            return Thing::Types(ts);
        }
        match hint {
            TypeHint::Named(_) => Thing::External,
            TypeHint::Alias(a) => match self.binding(file, scope, a) {
                Some(TypeHint::Named(_)) => Thing::External,
                _ => Thing::Unknown,
            },
            TypeHint::CallResult { .. } | TypeHint::Unknown => Thing::Unknown,
        }
    }

    fn fields_are_named(&self, types: &[u32], field: &str) -> bool {
        types.iter().all(|t| {
            self.owner_fields
                .get(&(self.defs[*t as usize].name.clone(), field.to_string()))
                .is_none_or(|hs| hs.iter().all(|(_, h)| matches!(h, TypeHint::Named(_))))
        })
    }

    fn field_types(&self, types: &[u32], field: &str, depth: u8) -> Vec<u32> {
        let mut out = vec![];
        for t in types {
            let d = &self.defs[*t as usize];
            if let Some(hints) = self.owner_fields.get(&(d.name.clone(), field.to_string())) {
                for (hf, h) in hints {
                    out.extend(self.hint_types(*hf, None, Some(&d.name), h, depth));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Types of member `field` of class `owner`, including members inherited
    /// from in-repo base classes.
    fn member_types(&self, file: u32, owner: &str, field: &str, depth: u8) -> Vec<u32> {
        let mut types = self.owner_types(file, owner);
        let mut i = 0;
        while i < types.len() && types.len() < 32 {
            for b in self.bases.get(&types[i]).cloned().unwrap_or_default() {
                if !types.contains(&b) {
                    types.push(b);
                }
            }
            i += 1;
        }
        self.field_types(&types, field, depth)
    }

    fn owner_types(&self, file: u32, owner: &str) -> Vec<u32> {
        let local: Vec<u32> = self
            .top_level
            .get(&(file, owner.to_string()))
            .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind.is_type()).collect())
            .unwrap_or_default();
        if !local.is_empty() {
            return local;
        }
        self.types_named(file, owner)
    }

    fn resolve_qualifier(&self, file: u32, scope: Option<u32>, owner: Option<&str>, q: &str, depth: u8) -> Thing {
        let rules = self.rules(file);
        let modules = self.modules(file);
        let segs = qual_segments(q);
        let Some(head) = segs.first() else { return Thing::Unknown };
        if modules.is_module_path(q) {
            // A path into the module tree: longest prefix that is a module, then a type.
            let path = q.to_string();
            if let Some(f) = modules.module(&self.files, file, &path) {
                return Thing::Module(path, f);
            }
            if let Some((m, last)) = path.rsplit_once("::") {
                if let Some(f) = modules.module(&self.files, file, m) {
                    let t: Vec<u32> = self.exported(&f, last, 2).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect();
                    return if t.is_empty() { Thing::Unknown } else { Thing::Types(t) };
                }
            }
            return Thing::Unknown;
        }
        let mut thing = match *head {
            h if matches!(h, "self" | "this" | "Self") || rules.self_names.contains(&h) => match owner {
                Some(o) => Thing::Types(self.owner_types(file, o)),
                None => Thing::Unknown,
            },
            "super" | "super()" | "Super" => match owner {
                Some(o) => {
                    let bases: Vec<u32> = self.owner_types(file, o).iter().flat_map(|t| self.bases.get(t).cloned().unwrap_or_default()).collect();
                    // No base in the repo means the parent is a framework class (ACharacter, AActor, ...).
                    if bases.is_empty() { Thing::External } else { Thing::Types(bases) }
                }
                None => Thing::Unknown,
            },
            h if h.ends_with("()") => {
                let (targets, _) = self.resolve_call(file, scope, owner, &Callee { name: &h[..h.len() - 2], qualifier: None, line: 0 }, depth.saturating_sub(1));
                self.returns_of(targets)
            }
            h => {
                let member = || -> Vec<u32> {
                    match owner {
                        Some(o) if rules.implicit_this => self.member_types(file, o, h, depth),
                        _ => vec![],
                    }
                };
                if let Some(hint) = self.binding(file, scope, h) {
                    self.hint_thing(file, scope, owner, &hint, depth)
                } else if let Some(ts) = Some(member()).filter(|t| !t.is_empty()) {
                    // C++ implicit `this->`: a bare name in a method is a member before it's a type.
                    Thing::Types(ts)
                } else if let Some(t) = self.ctx[file as usize].imports.get(h) {
                    match t {
                        Target::Module(k, f) => Thing::Module(k.clone(), f.clone()),
                        Target::Symbol(f, n) => {
                            let ts: Vec<u32> = self.exported(f, n, 4).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect();
                            if !ts.is_empty() {
                                Thing::Types(ts)
                            } else {
                                // An imported module-level variable (`registry = Registry()`).
                                f.iter()
                                    .find_map(|tf| self.ctx[*tf as usize].bindings.get(&(None, n.clone())).map(|h| (*tf, h.clone())))
                                    .map(|(tf, h)| self.hint_thing(tf, None, None, &h, depth))
                                    .unwrap_or(Thing::Unknown)
                            }
                        }
                        Target::External => Thing::External,
                    }
                } else if rules.globals.contains(&h) {
                    Thing::External
                } else {
                    let ts = self.types_named(file, h);
                    if ts.is_empty() { Thing::Unknown } else { Thing::Types(ts) }
                }
            }
        };
        for seg in &segs[1..] {
            if let Some(m) = seg.strip_suffix("()") {
                thing = match thing {
                    Thing::Types(ts) if !ts.is_empty() => self.returns_of(self.methods_of(&ts, m)),
                    Thing::Module(_, files) => self.returns_of(self.exported(&files, m, 4)),
                    Thing::External => return Thing::External,
                    _ => return Thing::Unknown,
                };
                continue;
            }
            thing = match thing {
                Thing::Types(ts) if !ts.is_empty() => {
                    let has_field = ts.iter().any(|t| self.owner_fields.contains_key(&(self.defs[*t as usize].name.clone(), seg.to_string())));
                    let ft = self.field_types(&ts, seg, depth);
                    if !ft.is_empty() {
                        Thing::Types(ft)
                    } else if has_field && self.fields_are_named(&ts, seg) {
                        Thing::External
                    } else {
                        Thing::Unknown
                    }
                }
                Thing::Module(key, files) => {
                    let sub = format!("{key}.{seg}");
                    if let Some(f) = modules.submodule(&key, seg) {
                        Thing::Module(sub, f)
                    } else {
                        let ts: Vec<u32> = self.exported(&files, seg, 4).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect();
                        if ts.is_empty() { Thing::Unknown } else { Thing::Types(ts) }
                    }
                }
                other => return if matches!(other, Thing::External) { Thing::External } else { Thing::Unknown },
            };
        }
        thing
    }

    /// Types produced by calling `targets`: a class constructs itself, a function returns its declared type.
    fn returns_of(&self, targets: Vec<u32>) -> Thing {
        let mut out = vec![];
        for t in targets {
            let d = &self.defs[t as usize];
            if d.kind.is_type() {
                out.push(t);
            } else if let Some(ret) = &self.inputs[d.file as usize].facts.defs[(t - self.files[d.file as usize].first_def) as usize].ret {
                out.extend(self.types_named(d.file, ret));
            }
        }
        if out.is_empty() { Thing::Unknown } else { Thing::Types(out) }
    }

    /// Returns (targets, exact?) for a call/new of `name` with optional receiver `qualifier`.
    /// Returns (targets, exact?) for a call/new of `callee` made in `scope`.
    fn resolve_call(&self, file: u32, scope: Option<u32>, owner: Option<&str>, callee: &Callee, depth: u8) -> (Vec<u32>, Option<Conf>) {
        if depth == 0 {
            return (vec![], None);
        }
        let Callee { name, qualifier, line: at } = *callee;
        let rules = self.rules(file);
        match qualifier {
            None => {
                if rules.implicit_this {
                    if let Some(o) = owner {
                        let m = self.methods_of(&self.owner_types(file, o), name);
                        if !m.is_empty() {
                            return (m, Some(Conf::Exact));
                        }
                    }
                }
                let nested = self.nested_in_scope(file, scope, name);
                if !nested.is_empty() {
                    return (nested, Some(Conf::Exact));
                }
                if self.is_local(file, scope, name, at) {
                    // A parameter or local variable holding a function: its target is a value.
                    return (vec![], None);
                }
                if let Some(d) = self.top_level.get(&(file, name.to_string())) {
                    let d: Vec<u32> = d.iter().copied().filter(|i| self.defs[*i as usize].kind != DefKind::Method && self.is_call_target(*i)).collect();
                    if !d.is_empty() {
                        return (d, Some(Conf::Exact));
                    }
                }
                match self.ctx[file as usize].imports.get(name) {
                    Some(Target::Symbol(f, orig)) => {
                        let d = self.call_targets(self.exported(f, orig, 4));
                        return if d.is_empty() { (d, None) } else { (d, Some(Conf::Exact)) };
                    }
                    Some(_) => return (vec![], None),
                    None => {}
                }
                for w in &self.ctx[file as usize].wildcards {
                    let d = self.call_targets(self.exported(w, name, 4));
                    if !d.is_empty() {
                        return (d, Some(Conf::Exact));
                    }
                }
                if rules.package_scope {
                    let dir = dir_of(&self.files[file as usize].path);
                    if let Some(fs) = self.dirs.get(dir) {
                        let d: Vec<u32> = fs.iter().flat_map(|f| self.exported(&[*f], name, 0)).collect();
                        if !d.is_empty() {
                            return (d, Some(Conf::Exact));
                        }
                    }
                }
                let all: Vec<u32> = self
                    .by_name
                    .get(name)
                    .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind != DefKind::Method && !self.is_nested(*d) && self.is_call_target(*d)).collect())
                    .unwrap_or_default();
                let n = self.narrow(file, all);
                if n.is_empty() { (n, None) } else { (n, Some(Conf::Inferred)) }
            }
            Some(q) => {
                let thing = if q == "?" { Thing::Unknown } else { self.resolve_qualifier(file, scope, owner, q, depth) };
                match thing {
                    Thing::Types(ts) if !ts.is_empty() => {
                        let m = self.methods_of(&ts, name);
                        if m.is_empty() { (m, None) } else { (m, Some(Conf::Exact)) }
                    }
                    Thing::Module(_, files) => {
                        let d = self.call_targets(self.exported(&files, name, 4));
                        if d.is_empty() { (d, None) } else { (d, Some(Conf::Exact)) }
                    }
                    Thing::External => (vec![], None),
                    _ => {
                        // Unknown receiver: only link when exactly one method has this name,
                        // and the name isn't one that built-in containers/streams also have.
                        if rules.is_builtin_method(name) {
                            return (vec![], None);
                        }
                        if rules.static_scope_calls && (q.contains("::") || !q.contains('.') && q != "?") {
                            // `Namespace::Func()` / `Helpers::Clamp()`: a free function or static member.
                            let all: Vec<u32> = self.prefer_definitions(self.by_name.get(name).cloned().unwrap_or_default());
                            let tail = q.rsplit("::").next().unwrap_or(q);
                            let owned: Vec<u32> = all.iter().copied().filter(|d| self.defs[*d as usize].owner.as_deref() == Some(tail)).collect();
                            let n = if owned.is_empty() { self.narrow(file, all) } else { owned };
                            if !n.is_empty() {
                                return (n, Some(Conf::Inferred));
                            }
                        }
                        let all: Vec<u32> = self
                            .by_name
                            .get(name)
                            .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind == DefKind::Method && self.is_member_method(*d)).collect())
                            .unwrap_or_default();
                        if all.len() == 1 { (all, Some(Conf::Inferred)) } else { (vec![], None) }
                    }
                }
            }
        }
    }

    /// A call that resolved to a prototype (C++ header declaration) is linked to
    /// the definition instead, when one exists. Unreal RPCs and
    /// BlueprintNativeEvents are linked to their `_Implementation` body.
    fn prefer_definitions(&self, targets: Vec<u32>) -> Vec<u32> {
        let mut out = vec![];
        for t in targets {
            let d = &self.defs[t as usize];
            if !d.decl {
                out.push(t);
                continue;
            }
            let (name, qual) = lang::support(self.files[d.file as usize].lang).definition_key(d);
            let defs: Vec<u32> = self
                .by_name
                .get(&name)
                .map(|v| v.iter().copied().filter(|x| !self.defs[*x as usize].decl && self.defs[*x as usize].qual == qual).collect())
                .unwrap_or_default();
            if defs.is_empty() { out.push(t) } else { out.extend(defs) }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Overloads share a qualified name, and argument types aren't modelled, so a
    /// call is linked to one of them (the first) rather than to all of them, which
    /// would multiply edges and hub counts.
    fn one_per_overload_set(&self, targets: Vec<u32>) -> Vec<u32> {
        if targets.len() < 2 {
            return targets;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        targets.into_iter().filter(|t| seen.insert(self.defs[*t as usize].qual.as_str())).collect()
    }

    fn resolve_inherits(&mut self) {
        let mut bases: HashMap<u32, Vec<u32>> = HashMap::new();
        for (fi, inp) in self.inputs.iter().enumerate() {
            let fi = fi as u32;
            for r in inp.facts.refs.iter().filter(|r| r.kind == RefKind::Inherit) {
                let Some(subject) = &r.owner else { continue };
                let subjects = self.owner_types(fi, subject);
                let targets = match &r.qualifier {
                    None => self.types_named(fi, &r.name),
                    Some(q) => match self.resolve_qualifier(fi, r.scope, None, &format!("{q}.{}", r.name), 4) {
                        Thing::Types(t) => t,
                        _ => vec![],
                    },
                };
                for s in &subjects {
                    for t in &targets {
                        if s != t {
                            bases.entry(*s).or_default().push(*t);
                        }
                    }
                }
            }
        }
        for v in bases.values_mut() {
            v.sort();
            v.dedup();
        }
        self.bases = bases;
    }

    fn resolve_file(&self, fi: u32) -> (Vec<Edge>, Stats) {
        let facts = &self.inputs[fi as usize].facts;
        let mut edges = vec![];
        let mut stats = Stats::default();
        let mut seen: HashSet<(Option<u32>, u32)> = HashSet::new();
        for r in facts.refs.iter().filter(|r| r.kind != RefKind::Inherit) {
            stats.refs += 1;
            let (targets, conf) = self.resolve_call(fi, r.scope, r.owner.as_deref(), &Callee { name: &r.name, qualifier: r.qualifier.as_deref(), line: r.line }, 6);
            let Some(conf) = conf else {
                let known = self.by_name.contains_key(&r.name);
                if known { stats.ambiguous += 1 } else { stats.unresolved += 1 }
                continue;
            };
            match conf {
                Conf::Exact => stats.exact += 1,
                Conf::Inferred => stats.inferred += 1,
            }
            let from_def = r.scope.map(|s| self.global_def(fi, s));
            let targets = if self.rules(fi).link_declarations { self.prefer_definitions(targets) } else { targets };
            for t in self.one_per_overload_set(targets) {
                if Some(t) == from_def && r.kind == RefKind::New {
                    continue;
                }
                if seen.insert((from_def, t)) {
                    edges.push(Edge { from_file: fi, from_def, to: t, kind: EdgeKind::Call, conf, line: r.line });
                }
            }
        }
        (edges, stats)
    }
}

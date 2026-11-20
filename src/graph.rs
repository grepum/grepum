//! Cross-file resolution: turns a set of (path, facts) into a linked graph.
//!
//! Resolution is deliberately conservative. A call edge is `Exact` when the
//! target is proven by scope, imports or receiver types, `Inferred` when the
//! name is globally unique, and dropped when it is ambiguous. A missing edge
//! is cheaper for an agent than a confidently wrong one.

use crate::facts::*;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GFile {
    pub path: String,
    pub oid: String,
    pub lang: Lang,
    pub lines: u32,
    pub first_def: u32,
    pub def_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GDef {
    pub file: u32,
    pub name: String,
    pub qual: String,
    pub kind: DefKind,
    pub line: u32,
    pub end_line: u32,
    pub parent: Option<u32>,
    pub owner: Option<String>,
    pub decl: bool,
    pub attrs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    Call,
    Inherit,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conf {
    Exact,
    Inferred,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Edge {
    pub from_file: u32,
    pub from_def: Option<u32>,
    pub to: u32,
    pub kind: EdgeKind,
    pub conf: Conf,
    pub line: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Stats {
    pub refs: u64,
    pub exact: u64,
    pub inferred: u64,
    pub ambiguous: u64,
    pub unresolved: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Graph {
    pub files: Vec<GFile>,
    pub defs: Vec<GDef>,
    pub edges: Vec<Edge>,
    /// importer file -> imported file
    pub imports: Vec<(u32, u32)>,
    pub stats: Stats,
}

pub struct Input {
    pub path: String,
    pub oid: String,
    pub facts: Arc<FileFacts>,
}

#[derive(Clone, Debug)]
enum Target {
    Module(String, Vec<u32>),
    Symbol(Vec<u32>, String),
    External,
}

#[derive(Clone, Debug)]
enum Thing {
    Types(Vec<u32>),
    Module(String, Vec<u32>),
    Unknown,
    External,
}

struct FileCtx {
    imports: HashMap<String, Target>,
    wildcards: Vec<Vec<u32>>,
    reexports: Vec<(Option<String>, Target)>, // (exported name or None for `*`, target)
    bindings: HashMap<(Option<u32>, String), TypeHint>,
}

struct Resolver<'a> {
    inputs: &'a [Input],
    files: Vec<GFile>,
    defs: Vec<GDef>,
    by_name: HashMap<String, Vec<u32>>,
    top_level: HashMap<(u32, String), Vec<u32>>,
    methods: HashMap<(String, String), Vec<u32>>,
    owner_fields: HashMap<(String, String), Vec<(u32, TypeHint)>>,
    py_modules: HashMap<String, Vec<(u32, u8)>>,
    js_paths: HashMap<String, u32>,
    dirs: HashMap<String, Vec<u32>>,
    rust_mods: HashMap<String, Vec<u32>>,
    /// C/C++ files by basename, for `#include "x/y.h"` suffix matching.
    cpp_files: HashMap<String, Vec<u32>>,
    ctx: Vec<FileCtx>,
    bases: HashMap<u32, Vec<u32>>,
}

/// Method names that standard containers, strings, promises and streams also
/// have. A call to one of these on a receiver of unknown type is far more
/// likely to hit the builtin than a uniquely-named local method.
const COMMON_METHODS: &[&str] = &[
    "get", "set", "add", "has", "delete", "remove", "clear", "pop", "push", "append", "extend", "insert", "update",
    "items", "keys", "values", "entries", "copy", "sort", "reverse", "index", "count", "find", "filter", "map",
    "reduce", "forEach", "some", "every", "includes", "join", "split", "slice", "splice", "concat", "strip",
    "lower", "upper", "replace", "format", "startswith", "endswith", "encode", "decode", "read", "write",
    "close", "open", "send", "emit", "on", "then", "catch", "finally", "json", "toString", "next", "len",
    "unwrap", "expect", "clone", "iter", "into", "as_str", "to_string", "is_empty", "Error", "String",
    // C++ / Unreal containers, delegates and handles
    "Add", "AddUnique", "Num", "Remove", "RemoveAt", "Contains", "Find", "FindRef", "Empty", "Reset", "Get",
    "IsValid", "Broadcast", "AddDynamic", "RemoveDynamic", "AddUObject", "AddLambda", "BindUObject",
    "BindLambda", "Execute", "ExecuteIfBound", "IsBound", "ToString", "size", "begin", "end", "empty",
    "push_back", "emplace_back", "erase", "at", "data", "c_str", "reset", "release", "lock",
];

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn strip_ext(path: &str) -> &str {
    match path.rsplit_once('.') {
        Some((p, _)) if !p.ends_with('/') => p,
        _ => path,
    }
}

fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn qual_segments(q: &str) -> Vec<&str> {
    q.split("::").flat_map(|s| s.split('.')).filter(|s| !s.is_empty()).collect()
}

pub fn build(inputs: Vec<Input>) -> Graph {
    let mut r = Resolver::new(&inputs);
    r.build_contexts();
    r.resolve_inherits();
    let per_file: Vec<(Vec<Edge>, Stats)> = (0..inputs.len()).into_par_iter().map(|fi| r.resolve_file(fi as u32)).collect();
    let mut stats = Stats::default();
    let mut edges = vec![];
    for (bi, bases) in &r.bases {
        let d = &r.defs[*bi as usize];
        for b in bases {
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
    Graph { files: r.files, defs: r.defs, edges, imports, stats }
}

impl<'a> Resolver<'a> {
    fn new(inputs: &'a [Input]) -> Self {
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
        for (i, d) in defs.iter().enumerate() {
            let i = i as u32;
            by_name.entry(d.name.clone()).or_default().push(i);
            if let (DefKind::Method, Some(o)) = (d.kind, &d.owner) {
                methods.entry((o.clone(), d.name.clone())).or_default().push(i);
            } else if d.parent.is_none() || d.kind.is_type() {
                top_level.entry((d.file, d.name.clone())).or_default().push(i);
            }
        }
        let mut owner_fields: HashMap<(String, String), Vec<(u32, TypeHint)>> = HashMap::new();
        let mut py_modules: HashMap<String, Vec<(u32, u8)>> = HashMap::new();
        let mut js_paths = HashMap::new();
        let mut dirs: HashMap<String, Vec<u32>> = HashMap::new();
        let mut rust_mods: HashMap<String, Vec<u32>> = HashMap::new();
        let mut cpp_files: HashMap<String, Vec<u32>> = HashMap::new();
        for (fi, inp) in inputs.iter().enumerate() {
            let fi = fi as u32;
            for b in &inp.facts.bindings {
                if let BindingScope::Owner(o) = &b.scope {
                    owner_fields.entry((o.clone(), b.name.clone())).or_default().push((fi, b.ty.clone()));
                }
            }
            dirs.entry(dir_of(&inp.path).to_string()).or_default().push(fi);
            match inp.facts.lang {
                Lang::Python => {
                    let mut p = strip_ext(&inp.path);
                    if let Some(pkg) = p.strip_suffix("/__init__") {
                        p = pkg;
                    }
                    let segs: Vec<&str> = p.split('/').collect();
                    for skip in 0..segs.len() {
                        py_modules.entry(segs[skip..].join(".")).or_default().push((fi, skip as u8));
                    }
                }
                l if l.is_js_family() => {
                    js_paths.insert(inp.path.clone(), fi);
                    js_paths.entry(strip_ext(&inp.path).to_string()).or_insert(fi);
                }
                Lang::Cpp => {
                    let base = inp.path.rsplit('/').next().unwrap_or(&inp.path).to_string();
                    cpp_files.entry(base).or_default().push(fi);
                }
                Lang::Rust => {
                    let p = strip_ext(&inp.path);
                    let rel = p.rsplit_once("src/").map(|(_, r)| r).unwrap_or(p);
                    let rel = rel.strip_suffix("/mod").unwrap_or(rel);
                    let key = if rel == "lib" || rel == "main" { String::new() } else { rel.to_string() };
                    rust_mods.entry(key).or_default().push(fi);
                }
                _ => {}
            }
        }
        Resolver {
            inputs,
            files,
            defs,
            by_name,
            top_level,
            methods,
            owner_fields,
            py_modules,
            js_paths,
            dirs,
            rust_mods,
            cpp_files,
            ctx: vec![],
            bases: HashMap::new(),
        }
    }

    fn global_def(&self, file: u32, local: u32) -> u32 {
        self.files[file as usize].first_def + local
    }

    // ---------- module resolution ----------

    fn py_module(&self, key: &str) -> Option<Vec<u32>> {
        let cands = self.py_modules.get(key)?;
        let best = cands.iter().map(|(_, s)| *s).min()?;
        Some(cands.iter().filter(|(_, s)| *s == best).map(|(f, _)| *f).collect())
    }

    fn py_absolute(&self, file: u32, module: &str) -> String {
        let dots = module.chars().take_while(|c| *c == '.').count();
        if dots == 0 {
            return module.to_string();
        }
        let path = &self.files[file as usize].path;
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

    fn js_module(&self, file: u32, spec: &str) -> Option<u32> {
        let base = if spec.starts_with('.') {
            normalize_path(&format!("{}/{}", dir_of(&self.files[file as usize].path), spec))
        } else if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
            format!("src/{rest}")
        } else {
            return None;
        };
        let stem = strip_ext(&base);
        for cand in [base.as_str(), stem, &format!("{base}/index"), &format!("{stem}/index")] {
            if let Some(f) = self.js_paths.get(cand) {
                return Some(*f);
            }
        }
        None
    }

    /// `#include "a/b.h"`: relative to the includer first, then any file whose
    /// path ends with the include path, preferring the one sharing the most
    /// leading directories with the includer (same module/plugin).
    fn cpp_include(&self, file: u32, inc: &str) -> Option<u32> {
        let me = &self.files[file as usize].path;
        let base = inc.rsplit('/').next()?;
        let cands = self.cpp_files.get(base)?;
        let rel = normalize_path(&format!("{}/{}", dir_of(me), inc));
        if let Some(f) = cands.iter().find(|f| self.files[**f as usize].path == rel) {
            return Some(*f);
        }
        let suffix = format!("/{inc}");
        let shared = |p: &str| me.split('/').zip(p.split('/')).take_while(|(a, b)| a == b).count();
        cands
            .iter()
            .copied()
            .filter(|f| {
                let p = &self.files[*f as usize].path;
                p == inc || p.ends_with(&suffix)
            })
            .max_by_key(|f| shared(&self.files[*f as usize].path))
    }

    fn go_package(&self, import_path: &str) -> Option<Vec<u32>> {
        let mut best: Option<(&String, &Vec<u32>)> = None;
        for (dir, files) in &self.dirs {
            if dir.is_empty() || !files.iter().any(|f| self.files[*f as usize].lang == Lang::Go) {
                continue;
            }
            if import_path == dir || import_path.ends_with(&format!("/{dir}")) {
                if best.is_none_or(|(b, _)| dir.len() > b.len()) {
                    best = Some((dir, files));
                }
            }
        }
        best.map(|(_, f)| f.iter().copied().filter(|f| self.files[*f as usize].lang == Lang::Go).collect())
    }

    fn rust_module(&self, file: u32, path: &str) -> Option<Vec<u32>> {
        let segs: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        let me = &self.files[file as usize].path;
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
        let key = cur.join("/");
        let files = self.rust_mods.get(&key)?;
        let crate_root = me.rsplit_once("src/").map(|(c, _)| c).unwrap_or("");
        let same: Vec<u32> = files.iter().copied().filter(|f| self.files[*f as usize].path.starts_with(crate_root)).collect();
        Some(if same.is_empty() { files.clone() } else { same })
    }

    fn import_target(&self, file: u32, lang: Lang, imp: &Import, name: Option<&str>) -> Target {
        match lang {
            Lang::Python => {
                let abs = self.py_absolute(file, &imp.module);
                match name {
                    None => match self.py_module(&abs) {
                        Some(f) => Target::Module(abs, f),
                        None => Target::External,
                    },
                    Some(n) => {
                        let sub = if abs.is_empty() { n.to_string() } else { format!("{abs}.{n}") };
                        if let Some(f) = self.py_module(&sub) {
                            return Target::Module(sub, f);
                        }
                        match self.py_module(&abs) {
                            Some(f) => Target::Symbol(f, n.to_string()),
                            None => Target::External,
                        }
                    }
                }
            }
            l if l.is_js_family() => match self.js_module(file, &imp.module) {
                Some(f) => match name {
                    None => Target::Module(imp.module.clone(), vec![f]),
                    Some(n) => Target::Symbol(vec![f], n.to_string()),
                },
                None => Target::External,
            },
            Lang::Cpp => match self.cpp_include(file, &imp.module) {
                Some(f) => Target::Module(imp.module.clone(), vec![f]),
                None => Target::External,
            },
            Lang::Go => match self.go_package(&imp.module) {
                Some(f) => Target::Module(imp.module.clone(), f),
                None => Target::External,
            },
            Lang::Rust => {
                let full = match name {
                    Some(n) if imp.module.is_empty() => n.to_string(),
                    Some(n) => format!("{}::{n}", imp.module),
                    None => imp.module.clone(),
                };
                if let Some(f) = self.rust_module(file, &full) {
                    return Target::Module(full, f);
                }
                match (name, self.rust_module(file, &imp.module)) {
                    (Some(n), Some(f)) => Target::Symbol(f, n.to_string()),
                    _ => Target::External,
                }
            }
            _ => Target::External,
        }
    }

    fn build_contexts(&mut self) {
        let ctx: Vec<FileCtx> = (0..self.inputs.len())
            .into_par_iter()
            .map(|fi| {
                let fi = fi as u32;
                let facts = &self.inputs[fi as usize].facts;
                let lang = facts.lang;
                let mut c = FileCtx { imports: HashMap::new(), wildcards: vec![], reexports: vec![], bindings: HashMap::new() };
                for imp in &facts.imports {
                    if imp.wildcard {
                        if let Target::Module(_, f) = self.import_target(fi, lang, imp, None) {
                            if imp.reexport {
                                c.reexports.push((None, Target::Module(String::new(), f.clone())));
                            }
                            c.wildcards.push(f);
                        }
                        continue;
                    }
                    if imp.names.is_empty() {
                        let t = self.import_target(fi, lang, imp, None);
                        let local = match (&imp.alias, lang) {
                            (Some(a), _) => a.clone(),
                            (None, Lang::Python) => imp.module.split('.').next().unwrap_or("").to_string(),
                            (None, _) => continue,
                        };
                        // `import a.b` binds `a`; resolve the head module so `a.b.f()` walks down.
                        let t = if imp.alias.is_none() && lang == Lang::Python {
                            self.py_module(&local).map(|f| Target::Module(local.clone(), f)).unwrap_or(t)
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
                        let t = self.import_target(fi, lang, imp, Some(&n.name));
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
            for (exported, target) in &self.ctx[*f as usize].reexports {
                match (exported.as_deref(), target) {
                    (Some(e), Target::Symbol(tf, orig)) if e == name => out.extend(self.exported(tf, orig, depth - 1)),
                    (None, Target::Module(_, tf)) => out.extend(self.exported(tf, name, depth - 1)),
                    _ => {}
                }
            }
        }
        if out.is_empty() && name == "default" {
            // `export default` isn't modelled: fall back to the file's single top-level class/function.
            for f in files {
                let tops: Vec<u32> = (0..self.files[*f as usize].def_count)
                    .map(|l| self.global_def(*f, l))
                    .filter(|d| self.defs[*d as usize].parent.is_none() && self.defs[*d as usize].kind != DefKind::Method)
                    .collect();
                if tops.len() == 1 {
                    out.extend(tops);
                }
            }
        }
        out
    }

    fn types_named(&self, file: u32, name: &str) -> Vec<u32> {
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
                let lang = self.files[tfile as usize].lang;
                // Methods attach by owner *name*; for languages with in-class methods keep the same file.
                let filtered: Vec<u32> = if matches!(lang, Lang::Python) || lang.is_js_family() {
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
            match s {
                Some(d) => s = facts.defs[d as usize].parent,
                None => return None,
            }
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
                let (targets, _) = self.resolve_call(file, scope, owner, name, qualifier.as_deref(), depth - 1);
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
            TypeHint::CallResult { .. } => Thing::Unknown,
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
        let lang = self.files[file as usize].lang;
        let segs = qual_segments(q);
        let Some(head) = segs.first() else { return Thing::Unknown };
        if lang == Lang::Rust && matches!(*head, "crate" | "super") || (*head == "self" && q.contains("::")) {
            // A path into the module tree: longest prefix that is a module, then a type.
            let path = q.to_string();
            if let Some(f) = self.rust_module(file, &path) {
                return Thing::Module(path, f);
            }
            if let Some((m, last)) = path.rsplit_once("::") {
                if let Some(f) = self.rust_module(file, m) {
                    let t: Vec<u32> = self.exported(&f, last, 2).into_iter().filter(|d| self.defs[*d as usize].kind.is_type()).collect();
                    return if t.is_empty() { Thing::Unknown } else { Thing::Types(t) };
                }
            }
            return Thing::Unknown;
        }
        let mut thing = match *head {
            "self" | "this" | "Self" => match owner {
                Some(o) => Thing::Types(self.owner_types(file, o)),
                None => Thing::Unknown,
            },
            "ThisClass" if lang == Lang::Cpp => match owner {
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
                let (targets, _) = self.resolve_call(file, scope, owner, &h[..h.len() - 2], None, depth.saturating_sub(1));
                self.returns_of(targets)
            }
            h => {
                if let Some(hint) = self.binding(file, scope, h) {
                    self.hint_thing(file, scope, owner, &hint, depth)
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
                    if let Some(f) = (lang == Lang::Python).then(|| self.py_module(&sub)).flatten() {
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
    fn resolve_call(&self, file: u32, scope: Option<u32>, owner: Option<&str>, name: &str, qualifier: Option<&str>, depth: u8) -> (Vec<u32>, Option<Conf>) {
        if depth == 0 {
            return (vec![], None);
        }
        let lang = self.files[file as usize].lang;
        match qualifier {
            None => {
                if lang == Lang::Cpp {
                    if let Some(o) = owner {
                        let m = self.methods_of(&self.owner_types(file, o), name);
                        if !m.is_empty() {
                            return (m, Some(Conf::Exact));
                        }
                    }
                }
                if let Some(d) = self.top_level.get(&(file, name.to_string())) {
                    let d: Vec<u32> = d.iter().copied().filter(|i| self.defs[*i as usize].kind != DefKind::Method).collect();
                    if !d.is_empty() {
                        return (d, Some(Conf::Exact));
                    }
                }
                match self.ctx[file as usize].imports.get(name) {
                    Some(Target::Symbol(f, orig)) => {
                        let d = self.exported(f, orig, 4);
                        return if d.is_empty() { (d, None) } else { (d, Some(Conf::Exact)) };
                    }
                    Some(_) => return (vec![], None),
                    None => {}
                }
                for w in &self.ctx[file as usize].wildcards {
                    let d = self.exported(w, name, 4);
                    if !d.is_empty() {
                        return (d, Some(Conf::Exact));
                    }
                }
                if lang == Lang::Go {
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
                    .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind != DefKind::Method).collect())
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
                        let d = self.exported(&files, name, 4);
                        if d.is_empty() { (d, None) } else { (d, Some(Conf::Exact)) }
                    }
                    Thing::External => (vec![], None),
                    _ => {
                        // Unknown receiver: only link when exactly one method has this name,
                        // and the name isn't one that built-in containers/streams also have.
                        if COMMON_METHODS.contains(&name) {
                            return (vec![], None);
                        }
                        if lang == Lang::Cpp && q.contains("::") || lang == Lang::Cpp && !q.contains('.') && q != "?" {
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
                            .map(|v| v.iter().copied().filter(|d| self.defs[*d as usize].kind == DefKind::Method).collect())
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
            let ue_impl = d.attrs.iter().any(|a| crate::unreal::IMPLEMENTATION_SPECIFIERS.contains(&a.as_str()));
            let (name, qual) = if ue_impl {
                (format!("{}_Implementation", d.name), format!("{}_Implementation", d.qual))
            } else {
                (d.name.clone(), d.qual.clone())
            };
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
            let (targets, conf) = self.resolve_call(fi, r.scope, r.owner.as_deref(), &r.name, r.qualifier.as_deref(), 6);
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
            let targets = if facts.lang == Lang::Cpp { self.prefer_definitions(targets) } else { targets };
            for t in targets {
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

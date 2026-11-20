//! `grepum refs`: every place that uses a symbol, by the graph. Calls and
//! constructions, subclasses, type uses (signatures, annotations, generic
//! arguments, struct literals) and member accesses (reads, writes, a method
//! passed as a value). One line per site with its source text, so the output
//! pipes into `grepum show -`.

use crate::facts::DefKind;
use crate::graph::{EdgeKind, Graph};
use crate::query::{self, Index};
use crate::show::Source;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Longest source line printed for a site.
const MAX_CODE_CHARS: usize = 140;

/// How a site uses the symbol.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Use {
    Call,
    /// A call that constructs the type (`new X()`, `X { .. }`, `X(...)`).
    New,
    /// A subclass or implementation.
    Inherit,
    Type,
    Field,
    /// A function or method referenced without calling it (a callback).
    Value,
}

impl Use {
    pub fn label(self) -> &'static str {
        match self {
            Use::Call => "call",
            Use::New => "new",
            Use::Inherit => "inherit",
            Use::Type => "type",
            Use::Field => "field",
            Use::Value => "value",
        }
    }

    pub fn parse(s: &str) -> Option<Use> {
        [Use::Call, Use::New, Use::Inherit, Use::Type, Use::Field, Use::Value].into_iter().find(|u| u.label() == s)
    }
}

pub struct Site {
    pub file: u32,
    pub line: u32,
    /// The definition the use is written in (None: top-level code).
    pub from: Option<u32>,
    pub to: u32,
    pub kind: Use,
}

/// Uses of `targets`, one per (file, line, target), ordered by file and line.
/// A line that both calls and names a type (`Repo { .. }`) counts as the call.
pub fn sites(g: &Graph, idx: &Index, targets: &[u32], only: &[Use]) -> Vec<Site> {
    let mut best: HashMap<(u32, u32, u32), Site> = HashMap::new();
    for t in targets {
        for ei in idx.incoming.get(t).into_iter().flatten() {
            let e = &g.edges[*ei];
            let target = &g.defs[e.to as usize];
            let kind = match e.kind {
                EdgeKind::Call if target.kind.is_type() => Use::New,
                EdgeKind::Call => Use::Call,
                EdgeKind::Inherit => Use::Inherit,
                EdgeKind::Ref if target.kind.is_type() => Use::Type,
                EdgeKind::Ref if target.kind == DefKind::Field => Use::Field,
                EdgeKind::Ref => Use::Value,
            };
            // Inherit edges sit on the subclass; its own line is the site.
            let (file, line) = match (e.kind, e.from_def) {
                (EdgeKind::Inherit, Some(d)) => (g.defs[d as usize].file, g.defs[d as usize].line),
                _ => (e.from_file, e.line),
            };
            let site = Site { file, line, from: e.from_def, to: e.to, kind };
            match best.get(&(file, line, e.to)) {
                Some(s) if s.kind <= kind => {}
                _ => {
                    best.insert((file, line, e.to), site);
                }
            }
        }
    }
    let mut out: Vec<Site> = best.into_values().filter(|s| only.is_empty() || only.contains(&s.kind)).collect();
    out.sort_by(|a, b| (g.files[a.file as usize].path.as_str(), a.line, a.to).cmp(&(g.files[b.file as usize].path.as_str(), b.line, b.to)));
    out
}

fn code(g: &Graph, src: &mut Source, file: u32, line: u32) -> String {
    let Some(raw) = src.read(g, file) else { return String::new() };
    let text = raw.split(|b| *b == b'\n').nth(line.saturating_sub(1) as usize).unwrap_or_default();
    let text = String::from_utf8_lossy(text);
    let text = text.trim();
    match text.char_indices().nth(MAX_CODE_CHARS) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

pub fn print(g: &Graph, src: &mut Source, targets: &[u32], sites: &[Site]) {
    for d in targets {
        println!("{} {}  {}", g.defs[*d as usize].kind.label(), g.defs[*d as usize].qual, query::loc(g, *d));
    }
    let locs: Vec<String> = sites.iter().map(|s| format!("{}:{}", g.files[s.file as usize].path, s.line)).collect();
    let froms: Vec<String> = sites.iter().map(|s| s.from.map(|f| g.defs[f as usize].qual.clone()).unwrap_or_else(|| "(top level)".into())).collect();
    let w_loc = locs.iter().map(|l| l.len()).max().unwrap_or(0);
    let w_from = froms.iter().map(|f| f.chars().count()).max().unwrap_or(0).min(40);
    for (i, s) in sites.iter().enumerate() {
        println!("  {:<w_loc$}  {:<7}  {:<w_from$}  {}", locs[i], s.kind.label(), froms[i], code(g, src, s.file, s.line));
    }
    if sites.is_empty() {
        println!("  (no references found)");
        // Like `callers`: macros, reflection and untyped receivers hide uses from the graph.
        let name = targets.first().map(|d| g.defs[*d as usize].name.as_str()).unwrap_or("");
        println!("  note: not proof it's unused; confirm with a text search, e.g. `rg -w {name}`");
    } else {
        let mut files: Vec<u32> = sites.iter().map(|s| s.file).collect();
        files.sort();
        files.dedup();
        let plural = |n: usize, w: &str| if n == 1 { format!("1 {w}") } else { format!("{n} {w}s") };
        println!("  {} in {}", plural(sites.len(), "reference"), plural(files.len(), "file"));
    }
}

pub fn json(g: &Graph, src: &mut Source, targets: &[u32], sites: &[Site]) -> Value {
    json!({
        "symbols": targets.iter().map(|d| json!({"symbol": g.defs[*d as usize].qual, "kind": g.defs[*d as usize].kind.label(), "at": query::loc(g, *d)})).collect::<Vec<_>>(),
        "references": sites.iter().map(|s| json!({
            "file": g.files[s.file as usize].path, "line": s.line, "kind": s.kind.label(),
            "from": s.from.map(|f| g.defs[f as usize].qual.clone()), "symbol": g.defs[s.to as usize].qual,
            "code": code(g, src, s.file, s.line),
        })).collect::<Vec<_>>(),
    })
}

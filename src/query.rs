//! Read-side operations over a resolved graph.

use crate::graph::{Conf, EdgeKind, Graph};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn loc(g: &Graph, d: u32) -> String {
    let def = &g.defs[d as usize];
    format!("{}:{}", g.files[def.file as usize].path, def.line)
}

fn words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub struct Index {
    pub incoming: HashMap<u32, Vec<usize>>,
    pub outgoing: HashMap<u32, Vec<usize>>,
}

impl Index {
    pub fn new(g: &Graph) -> Index {
        let mut incoming: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut outgoing: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, e) in g.edges.iter().enumerate() {
            incoming.entry(e.to).or_default().push(i);
            if let Some(f) = e.from_def {
                outgoing.entry(f).or_default().push(i);
            }
        }
        Index { incoming, outgoing }
    }
}

/// Ranked symbol search: identifier-aware token matching weighted by rarity.
pub fn find(g: &Graph, idx: &Index, query: &str, limit: usize) -> Vec<(f64, u32)> {
    let q = query.trim().to_lowercase();
    let qtokens: Vec<String> = query.split_whitespace().flat_map(words).collect();
    if qtokens.is_empty() {
        return vec![];
    }
    let def_tokens: Vec<Vec<String>> = g.defs.iter().map(|d| words(&d.qual)).collect();
    let n = g.defs.len().max(1) as f64;
    let idf: Vec<f64> = qtokens
        .iter()
        .map(|t| {
            let df = def_tokens.iter().filter(|toks| toks.iter().any(|w| w.starts_with(t.as_str()))).count() as f64;
            (1.0 + n / (1.0 + df)).ln()
        })
        .collect();
    // A C++ prototype is noise next to its own definition.
    let defined: HashSet<&str> = g.defs.iter().filter(|d| !d.decl).map(|d| d.qual.as_str()).collect();
    let mut scored: Vec<(f64, u32)> = g
        .defs
        .iter()
        .enumerate()
        .filter(|(_, d)| !(d.decl && defined.contains(d.qual.as_str())))
        .filter_map(|(i, d)| {
            let toks = &def_tokens[i];
            let mut s = 0.0;
            let mut matched = 0;
            for (t, w) in qtokens.iter().zip(&idf) {
                let best = toks
                    .iter()
                    .map(|x| if x == t { 1.0 } else if x.starts_with(t.as_str()) { 0.6 } else if x.contains(t.as_str()) { 0.3 } else { 0.0 })
                    .fold(0.0, f64::max);
                if best > 0.0 {
                    matched += 1;
                }
                s += best * w;
            }
            if matched == 0 {
                return None;
            }
            let coverage = matched as f64 / qtokens.len() as f64;
            s *= coverage * coverage;
            if d.qual.to_lowercase() == q {
                s += 100.0;
            } else if d.name.to_lowercase() == q {
                s += 50.0;
            }
            let indeg = idx.incoming.get(&(i as u32)).map(|v| v.len()).unwrap_or(0) as f64;
            s += 0.15 * (1.0 + indeg).ln();
            Some((s, i as u32))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    scored.truncate(limit);
    scored
}

/// `qual`, `name`, or `path/suffix.py:qual` → matching def ids.
pub fn resolve_symbol(g: &Graph, sym: &str) -> Vec<u32> {
    let mut found = resolve_symbol_inner(g, sym);
    // Unreal: calls to an RPC/BlueprintNativeEvent land on `<Name>_Implementation`.
    let extra: Vec<u32> = found
        .iter()
        .filter(|d| g.defs[**d as usize].attrs.iter().any(|a| crate::unreal::IMPLEMENTATION_SPECIFIERS.contains(&a.as_str())))
        .flat_map(|d| {
            let q = format!("{}_Implementation", g.defs[*d as usize].qual);
            (0..g.defs.len() as u32).filter(move |x| g.defs[*x as usize].qual == q)
        })
        .collect();
    found.extend(extra);
    found.sort();
    found.dedup();
    found
}

fn resolve_symbol_inner(g: &Graph, sym: &str) -> Vec<u32> {
    let sym = sym.replace("::", ".");
    let sym = sym.as_str();
    let (file_filter, name) = match sym.rsplit_once(':') {
        Some((f, n)) if f.contains('/') || f.contains('.') && !n.is_empty() && !f.ends_with(':') => (Some(f), n),
        _ => (None, sym),
    };
    let in_file = |d: u32| file_filter.is_none_or(|f| g.files[g.defs[d as usize].file as usize].path.ends_with(f));
    let by_qual: Vec<u32> = (0..g.defs.len() as u32).filter(|d| g.defs[*d as usize].qual == name && in_file(*d)).collect();
    if !by_qual.is_empty() {
        return by_qual;
    }
    let by_suffix: Vec<u32> = (0..g.defs.len() as u32)
        .filter(|d| g.defs[*d as usize].qual.ends_with(&format!(".{name}")) && in_file(*d))
        .collect();
    if !by_suffix.is_empty() {
        return by_suffix;
    }
    (0..g.defs.len() as u32).filter(|d| g.defs[*d as usize].name == name && in_file(*d)).collect()
}

pub struct Hop {
    pub depth: u32,
    pub def: Option<u32>,
    pub file: u32,
    pub line: u32,
    pub conf: Conf,
    pub via: u32,
}

/// Breadth-first walk over call edges. `reverse` = callers.
pub fn walk(g: &Graph, idx: &Index, starts: &[u32], depth: u32, reverse: bool) -> Vec<Hop> {
    let mut out = vec![];
    let mut seen: HashSet<(Option<u32>, u32)> = starts.iter().map(|s| (Some(*s), g.defs[*s as usize].file)).collect();
    let mut frontier: Vec<u32> = starts.to_vec();
    for d in 1..=depth {
        let mut next = vec![];
        for node in &frontier {
            let list = if reverse { idx.incoming.get(node) } else { idx.outgoing.get(node) };
            for ei in list.into_iter().flatten() {
                let e = &g.edges[*ei];
                if e.kind != EdgeKind::Call {
                    continue;
                }
                let (def, file) = if reverse { (e.from_def, e.from_file) } else { (Some(e.to), g.defs[e.to as usize].file) };
                if !seen.insert((def, file)) {
                    continue;
                }
                out.push(Hop { depth: d, def, file, line: e.line, conf: e.conf, via: *node });
                if let Some(x) = def {
                    next.push(x);
                }
            }
        }
        frontier = next;
    }
    out
}

pub fn hop_label(g: &Graph, h: &Hop) -> (String, String) {
    match h.def {
        Some(d) => (g.defs[d as usize].qual.clone(), loc(g, d)),
        None => ("<module>".into(), format!("{}:{}", g.files[h.file as usize].path, h.line)),
    }
}

pub fn hops_json(g: &Graph, hops: &[Hop]) -> Value {
    Value::Array(
        hops.iter()
            .map(|h| {
                let (name, at) = hop_label(g, h);
                json!({"symbol": name, "at": at, "depth": h.depth, "call_line": h.line, "confidence": format!("{:?}", h.conf).to_lowercase(), "via": g.defs[h.via as usize].qual})
            })
            .collect(),
    )
}

pub fn export(g: &Graph) -> Value {
    json!({
        "files": g.files.iter().map(|f| json!({"path": f.path, "oid": f.oid, "lang": format!("{:?}", f.lang)})).collect::<Vec<_>>(),
        "defs": g.defs.iter().enumerate().map(|(i, d)| json!({
            "id": i, "file": g.files[d.file as usize].path, "name": d.name, "qual": d.qual,
            "kind": d.kind.label(), "line": d.line, "end_line": d.end_line, "decl": d.decl, "attrs": d.attrs
        })).collect::<Vec<_>>(),
        "edges": g.edges.iter().map(|e| json!({
            "from": e.from_def, "from_file": g.files[e.from_file as usize].path, "to": e.to,
            "kind": format!("{:?}", e.kind).to_lowercase(), "confidence": format!("{:?}", e.conf).to_lowercase(), "line": e.line
        })).collect::<Vec<_>>(),
        "imports": g.imports.iter().map(|(a, b)| json!([g.files[*a as usize].path, g.files[*b as usize].path])).collect::<Vec<_>>(),
        "stats": {"refs": g.stats.refs, "exact": g.stats.exact, "inferred": g.stats.inferred, "ambiguous": g.stats.ambiguous, "unresolved": g.stats.unresolved},
    })
}

/// Compact neighbourhood text for an agent: best matches plus who they call and who calls them.
pub fn context(g: &Graph, idx: &Index, query: &str, max_symbols: usize) -> String {
    let mut out = String::new();
    let hits = find(g, idx, query, max_symbols);
    if hits.is_empty() {
        return format!("no symbols match {query:?}\n");
    }
    for (_, d) in hits {
        let def = &g.defs[d as usize];
        let tags = if def.attrs.is_empty() { String::new() } else { format!("  [{}]", def.attrs.join(", ")) };
        out.push_str(&format!("{} {}  {}-{}{tags}\n", def.kind.label(), def.qual, loc(g, d), def.end_line));
        let mut seen = BTreeSet::new();
        for h in walk(g, idx, &[d], 1, true).iter().take(8) {
            let (n, at) = hop_label(g, h);
            if seen.insert(n.clone()) {
                out.push_str(&format!("  <- {n}  {at}\n"));
            }
        }
        seen.clear();
        for h in walk(g, idx, &[d], 1, false).iter().take(8) {
            let (n, at) = hop_label(g, h);
            if seen.insert(n.clone()) {
                out.push_str(&format!("  -> {n}  {at}\n"));
            }
        }
        for e in g.edges.iter().filter(|e| e.kind == EdgeKind::Inherit && e.from_def == Some(d)) {
            out.push_str(&format!("  extends {}  {}\n", g.defs[e.to as usize].qual, loc(g, e.to)));
        }
    }
    out
}

type DefKey = (String, String, &'static str);

fn def_keys(g: &Graph) -> HashMap<DefKey, u32> {
    g.defs
        .iter()
        .enumerate()
        .map(|(i, d)| ((g.files[d.file as usize].path.clone(), d.qual.clone(), d.kind.label()), i as u32))
        .collect()
}

fn edge_keys(g: &Graph) -> BTreeSet<(String, String)> {
    g.edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Call)
        .map(|e| {
            let from = match e.from_def {
                Some(d) => format!("{}#{}", g.files[g.defs[d as usize].file as usize].path, g.defs[d as usize].qual),
                None => format!("{}#<module>", g.files[e.from_file as usize].path),
            };
            let to = format!("{}#{}", g.files[g.defs[e.to as usize].file as usize].path, g.defs[e.to as usize].qual);
            (from, to)
        })
        .collect()
}

pub fn diff(a: &Graph, b: &Graph) -> Value {
    let ka = def_keys(a);
    let kb = def_keys(b);
    let fmt = |k: &DefKey, g: &Graph, d: u32| json!({"symbol": k.1, "kind": k.2, "at": loc(g, d)});
    let mut added: Vec<Value> = kb.iter().filter(|(k, _)| !ka.contains_key(*k)).map(|(k, d)| fmt(k, b, *d)).collect();
    let mut removed: Vec<Value> = ka.iter().filter(|(k, _)| !kb.contains_key(*k)).map(|(k, d)| fmt(k, a, *d)).collect();
    added.sort_by_key(|v| v["at"].as_str().unwrap_or("").to_string());
    removed.sort_by_key(|v| v["at"].as_str().unwrap_or("").to_string());
    let ea = edge_keys(a);
    let eb = edge_keys(b);
    let calls_added: Vec<Value> = eb.difference(&ea).map(|(f, t)| json!({"from": f, "to": t})).collect();
    let calls_removed: Vec<Value> = ea.difference(&eb).map(|(f, t)| json!({"from": f, "to": t})).collect();
    json!({"symbols_added": added, "symbols_removed": removed, "calls_added": calls_added, "calls_removed": calls_removed})
}

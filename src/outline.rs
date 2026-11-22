//! `grepum outline`: a table of contents for files, directories and types.
//! One line per definition: its line range, its signature (the source text up
//! to its body, whitespace collapsed) and how many places call it. Bodies are
//! left out; `grepum show path:line` prints the one you want.
//!
//! Function-local definitions are left out (they're part of their function's
//! body). Methods written outside their type (Rust `impl`, C++ namespaces) are
//! grouped under that block's header, so the outline reads like the file.

use crate::extract;
use crate::graph::{EdgeKind, Graph};
use crate::query::{self, Index};
use crate::show::{self, Source};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use tree_sitter::Node;

/// Longest signature printed.
const MAX_SIGNATURE_CHARS: usize = 160;

pub struct Options {
    /// Nesting levels to show; 0 = all.
    pub depth: usize,
}

/// One file's outline: all its definitions, or only those selected.
pub struct Section {
    pub file: u32,
    /// None = the whole file.
    only: Option<HashSet<u32>>,
}

/// One line of an outline.
pub struct Entry {
    /// None for a block header that isn't a definition (`impl Foo for Bar`).
    pub def: Option<u32>,
    pub depth: usize,
    pub line: u32,
    pub end_line: u32,
    pub signature: String,
    /// Distinct places that call it.
    pub callers: usize,
}

/// Sections named by `args`: files (also `path:line`, as `rg -n` prints),
/// directories, or symbols (a type outlines its members). Returns the targets
/// that matched nothing.
pub fn targets(g: &Graph, toplevel: &str, args: &[String], per_name: usize) -> (Vec<Section>, Vec<String>) {
    let paths: HashMap<&str, u32> = g.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i as u32)).collect();
    let mut sections: Vec<Section> = vec![];
    let mut missing = vec![];
    let add = |sections: &mut Vec<Section>, file: u32, defs: Option<Vec<u32>>| {
        let at = match sections.iter().position(|s| s.file == file) {
            Some(i) => i,
            None => {
                sections.push(Section { file, only: Some(HashSet::new()) });
                sections.len() - 1
            }
        };
        match (defs, &mut sections[at].only) {
            (None, only) => *only = None,
            (Some(d), Some(only)) => only.extend(d),
            (Some(_), None) => {}
        }
    };
    for arg in args {
        let a = arg.trim().replace('\\', "/");
        let a = a.strip_prefix(toplevel).map(|p| p.trim_start_matches('/')).unwrap_or(&a);
        let a = a.trim_start_matches("./").trim_end_matches('/');
        let path = a.split_whitespace().next().unwrap_or("");
        // `path`, or `path:line[:col][:text]`.
        let file = paths.get(path).or_else(|| path.split(':').next().and_then(|p| paths.get(p)));
        if let Some(f) = file {
            add(&mut sections, *f, None);
            continue;
        }
        let in_dir: Vec<u32> = if a.is_empty() || a == "." {
            (0..g.files.len() as u32).collect()
        } else {
            let prefix = format!("{a}/");
            (0..g.files.len() as u32).filter(|f| g.files[*f as usize].path.starts_with(&prefix)).collect()
        };
        if !in_dir.is_empty() {
            for f in in_dir {
                add(&mut sections, f, None);
            }
            continue;
        }
        if a.contains(char::is_whitespace) {
            missing.push(arg.clone());
            continue;
        }
        let mut found = query::resolve_symbol(g, a);
        found.truncate(per_name);
        if found.is_empty() {
            missing.push(arg.clone());
        }
        for d in found {
            for (file, defs) in members(g, d) {
                add(&mut sections, file, Some(defs));
            }
        }
    }
    (sections, missing)
}

/// A definition and, for a type, its members, wherever they are written:
/// nested in it, or attached by owner (Rust `impl`, Go receivers, C++
/// out-of-line definitions).
fn members(g: &Graph, d: u32) -> Vec<(u32, Vec<u32>)> {
    let def = &g.defs[d as usize];
    let mut by_file: Vec<(u32, Vec<u32>)> = vec![(def.file, vec![d])];
    if !def.kind.is_type() {
        return by_file;
    }
    let lang = g.files[def.file as usize].lang;
    // Files that define their own type of this name keep their methods.
    let rivals: HashSet<u32> =
        g.defs.iter().filter(|o| o.kind.is_type() && o.name == def.name && o.file != def.file).map(|o| o.file).collect();
    for (i, o) in g.defs.iter().enumerate() {
        let i = i as u32;
        let nested = ancestors(g, i).contains(&d);
        let attached = o.owner.as_deref() == Some(def.name.as_str())
            && o.parent.is_none()
            && g.files[o.file as usize].lang == lang
            && (o.file == def.file || !rivals.contains(&o.file));
        if i == d || !(nested || attached) {
            continue;
        }
        match by_file.iter_mut().find(|(f, _)| *f == o.file) {
            Some((_, v)) => v.push(i),
            None => by_file.push((o.file, vec![i])),
        }
    }
    by_file
}

/// Enclosing definitions, innermost first.
fn ancestors(g: &Graph, d: u32) -> Vec<u32> {
    let mut out = vec![];
    let mut cur = g.defs[d as usize].parent;
    while let Some(p) = cur {
        out.push(p);
        cur = g.defs[p as usize].parent;
    }
    out
}

/// The file's line count (as `wc -l` counts a file that ends in a newline) and
/// the section's entries, in file order.
pub fn entries(g: &Graph, idx: &Index, src: &mut Source, s: &Section, opts: &Options) -> Option<(usize, Vec<Entry>)> {
    let f = &g.files[s.file as usize];
    let raw = src.read(g, s.file)?.clone();
    let line_count = raw.iter().filter(|b| **b == b'\n').count() + usize::from(raw.last().is_some_and(|b| *b != b'\n'));
    let (tree, rewritten) = extract::parse(f.lang, &raw);
    let parsed = rewritten.as_deref().unwrap_or(&raw);
    let lines = show::split_lines(parsed);
    let raw_lines = show::split_lines(&raw);
    let mut out = vec![];
    let mut group: Option<(usize, usize)> = None;
    for d in f.first_def..f.first_def + f.def_count {
        let def = &g.defs[d as usize];
        let up = ancestors(g, d);
        // Function-local definitions belong to their function's body.
        if up.iter().any(|a| g.defs[*a as usize].kind.is_callable()) {
            continue;
        }
        if s.only.as_ref().is_some_and(|only| !only.contains(&d)) {
            continue;
        }
        let node = tree.as_ref().and_then(|t| show::def_node(t, &lines, def.line, def.end_line));
        let mut depth = up.len();
        // A method written in a block that isn't itself a definition (`impl`): show the block once.
        if def.parent.is_none() && def.owner.is_some() {
            if let Some(c) = node.and_then(container) {
                if group != Some((c.start_byte(), c.end_byte())) {
                    group = Some((c.start_byte(), c.end_byte()));
                    out.push(Entry {
                        def: None,
                        depth: 0,
                        line: c.start_position().row as u32 + 1,
                        end_line: c.end_position().row as u32 + 1,
                        signature: signature(&raw, c),
                        callers: 0,
                    });
                }
                depth += 1;
            }
        }
        if opts.depth > 0 && depth >= opts.depth {
            continue;
        }
        let signature = match node {
            Some(n) => signature(&raw, n),
            None => tidy(&String::from_utf8_lossy(raw_lines.get(def.line as usize - 1).copied().unwrap_or_default())),
        };
        let callers = if def.kind.is_callable() { callers(g, idx, d) } else { 0 };
        out.push(Entry { def: Some(d), depth, line: def.line, end_line: def.end_line, signature, callers });
    }
    Some((line_count, out))
}

/// The nearest enclosing block whose body holds `n` (`impl ... { }`, `namespace x { }`).
fn container(n: Node) -> Option<Node> {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        p.parent()?;
        if p.child_by_field_name("body").is_some_and(|b| b.start_byte() <= n.start_byte() && n.end_byte() <= b.end_byte()) {
            return Some(p);
        }
        cur = p;
    }
    None
}

/// A definition's source up to its body: `pub fn find(&self, id: u32) -> u32`.
/// Without a body (prototypes, aliases, Go structs): the whole text if short,
/// else its first line.
fn signature(raw: &[u8], n: Node) -> String {
    let start = n.start_byte().min(raw.len());
    let text = match body_start(n, 0) {
        Some(end) => String::from_utf8_lossy(&raw[start..end.min(raw.len())]).into_owned(),
        None => {
            let all = String::from_utf8_lossy(&raw[start..n.end_byte().min(raw.len())]).into_owned();
            if all.lines().count() <= 2 { all } else { all.lines().next().unwrap_or("").to_string() }
        }
    };
    tidy(&text)
}

/// Where the signature of `n` ends: before its body, and before comments
/// that precede the body (Python's `def f():  # note`). Looks a few levels
/// down, so that `export const f = (x) => { ... }` finds the arrow function's
/// body, but not into an object literal's methods.
fn body_start(n: Node, depth: usize) -> Option<usize> {
    if let Some(b) = n.child_by_field_name("body") {
        let mut end = b.start_byte();
        let mut cursor = n.walk();
        for c in n.children(&mut cursor) {
            if c.id() == b.id() {
                break;
            }
            if !c.kind().contains("comment") {
                end = c.end_byte();
            }
        }
        return Some(end.min(b.start_byte()));
    }
    if depth >= 3 {
        return None;
    }
    let mut cursor = n.walk();
    let children: Vec<Node> = n.named_children(&mut cursor).collect();
    children.into_iter().find_map(|c| body_start(c, depth + 1))
}

/// One line, whitespace collapsed, without the punctuation that opens a body.
fn tidy(text: &str) -> String {
    let mut s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        ("( ", "("),
        (" )", ")"),
        (", )", ")"),
        (",)", ")"),
        ("[ ", "["),
        (" ]", "]"),
        ("< ", "<"),
        (", >", ">"),
        (",>", ">"),
    ] {
        s = s.replace(from, to);
    }
    let s = s.trim_end_matches(|c: char| c == '{' || c == ':' || c == ';' || c.is_whitespace());
    match s.char_indices().nth(MAX_SIGNATURE_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Distinct definitions (or files, for top-level code) that call `d`.
fn callers(g: &Graph, idx: &Index, d: u32) -> usize {
    let Some(es) = idx.incoming.get(&d) else { return 0 };
    es.iter()
        .map(|i| &g.edges[*i])
        .filter(|e| e.kind == EdgeKind::Call)
        .map(|e| (e.from_file, e.from_def))
        .collect::<HashSet<_>>()
        .len()
}

pub fn print(out: &mut impl Write, g: &Graph, src: &mut Source, sections: &[Section], opts: &Options) -> io::Result<()> {
    let idx = Index::new(g);
    let mut first = true;
    for s in sections {
        let file = &g.files[s.file as usize];
        let Some((lines, entries)) = entries(g, &idx, src, s, opts) else {
            eprintln!("grepum: can't read {}", file.path);
            continue;
        };
        // In a directory listing, files without definitions are noise.
        if entries.is_empty() && sections.len() > 1 {
            continue;
        }
        if !first {
            writeln!(out)?;
        }
        first = false;
        writeln!(out, "{}  ({lines} lines)", file.path)?;
        if entries.is_empty() {
            writeln!(out, "  (no definitions)")?;
        }
        let range = |e: &Entry| if e.end_line > e.line { format!("{}-{}", e.line, e.end_line) } else { e.line.to_string() };
        let width = entries.iter().map(|e| range(e).len()).max().unwrap_or(0);
        for e in &entries {
            let range = range(e);
            let called = match e.callers {
                0 => String::new(),
                1 => "  (1 caller)".to_string(),
                n => format!("  ({n} callers)"),
            };
            writeln!(out, "  {range:<width$}  {}{}{called}", "  ".repeat(e.depth), e.signature)?;
        }
    }
    Ok(())
}

pub fn json(g: &Graph, src: &mut Source, sections: &[Section], opts: &Options) -> Value {
    let idx = Index::new(g);
    let files: Vec<Value> = sections
        .iter()
        .filter_map(|s| {
            let (lines, entries) = entries(g, &idx, src, s, opts)?;
            let file = &g.files[s.file as usize];
            Some(json!({
                "file": file.path,
                "lines": lines,
                "definitions": entries.iter().map(|e| {
                    let def = e.def.map(|d| &g.defs[d as usize]);
                    json!({
                        "symbol": def.map(|d| d.qual.clone()),
                        "kind": def.map(|d| d.kind.label()).unwrap_or("block"),
                        "line": e.line, "end_line": e.end_line, "depth": e.depth,
                        "signature": e.signature, "callers": e.callers,
                    })
                }).collect::<Vec<_>>(),
            }))
        })
        .collect();
    Value::Array(files)
}

#[cfg(test)]
mod tests {
    use super::tidy;

    #[test]
    fn signatures_read_as_one_line() {
        assert_eq!(tidy("def load(\n    uid,\n    cache=None,\n) -> User:\n"), "def load(uid, cache=None) -> User");
        assert_eq!(tidy("pub fn find(&self, id: u32) -> u32 "), "pub fn find(&self, id: u32) -> u32");
        assert_eq!(tidy("void Attack(float Power);"), "void Attack(float Power)");
        assert_eq!(tidy("class Hono<\n  E extends Env = Env,\n  P = '/',\n> {"), "class Hono<E extends Env = Env, P = '/'>");
        assert_eq!(tidy("export const f = async (id: string) => "), "export const f = async (id: string) =>");
    }
}

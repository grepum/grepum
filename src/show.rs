//! `grepum show`: print a definition's source, exactly as far as its syntax
//! reaches (braces, indentation), with what belongs to it: doc comments,
//! attributes, decorators, `template<...>`, Unreal `UFUNCTION(...)` lines.
//!
//! Targets are symbol names, or `-` to read them from stdin: any `path:line`
//! in the input (grepum's own output, `rg -n`, compiler errors) names the
//! innermost definition around that line; a line that is a single word is
//! taken as a symbol name.

use crate::extract;
use crate::facts::Lang;
use crate::git::Repo;
use crate::graph::Graph;
use crate::lang;
use crate::query;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Point, Tree};

pub struct Options {
    /// 0 = no limit. Longer definitions keep their start and end.
    pub max_lines: usize,
    pub line_numbers: bool,
    /// Also show what each definition calls (one level).
    pub callees: bool,
}

/// Longest line printed; minified code would otherwise flood the output.
const MAX_LINE_CHARS: usize = 400;

/// Where file contents come from: the worktree, or blobs of `--rev`.
pub struct Source<'r> {
    repo: &'r Repo,
    from_git: bool,
    cache: HashMap<u32, Option<Vec<u8>>>,
}

impl<'r> Source<'r> {
    pub fn new(repo: &'r Repo, rev: Option<&str>) -> Self {
        Source { repo, from_git: rev.is_some(), cache: HashMap::new() }
    }

    pub(crate) fn read(&mut self, g: &Graph, file: u32) -> Option<&Vec<u8>> {
        if !self.cache.contains_key(&file) {
            let f = &g.files[file as usize];
            let bytes = if self.from_git {
                self.repo.read_blobs(std::slice::from_ref(&f.oid)).ok().and_then(|mut m| m.remove(&f.oid))
            } else {
                std::fs::read(self.repo.toplevel.join(&f.path)).ok()
            };
            self.cache.insert(file, bytes);
        }
        self.cache.get(&file).and_then(|b| b.as_ref())
    }
}

/// What to show: definitions in order, and which of them were named by
/// location (shown as they are there, not swapped for their definition).
#[derive(Default)]
pub struct Picked {
    pub defs: Vec<u32>,
    at: HashSet<u32>,
}

/// Definitions named by `targets` (symbols, `path:line`s), in order, without
/// repeats, and the targets that matched nothing.
pub fn targets(g: &Graph, toplevel: &str, args: &[String], per_name: usize) -> (Picked, Vec<String>) {
    let mut out = Picked::default();
    let mut missing = vec![];
    let mut seen = HashSet::new();
    let paths: HashMap<&str, u32> = g.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i as u32)).collect();
    for arg in args {
        let found: Vec<u32> = match location(arg, toplevel, &paths) {
            Some((file, line)) => {
                let d = innermost(g, file, line);
                out.at.extend(d);
                d.into_iter().collect()
            }
            None if !arg.trim().contains(char::is_whitespace) => {
                let mut d = query::resolve_symbol(g, arg.trim());
                d.truncate(per_name);
                d
            }
            None => vec![],
        };
        if found.is_empty() {
            missing.push(arg.clone());
        }
        for d in found {
            if seen.insert(d) {
                out.defs.push(d);
            }
        }
    }
    (out, missing)
}

/// The first `path:line` in a line of text whose path is a file of the graph.
fn location(text: &str, toplevel: &str, paths: &HashMap<&str, u32>) -> Option<(u32, u32)> {
    for token in text.split_whitespace() {
        let token = token.replace('\\', "/");
        for (pos, _) in token.match_indices(':') {
            let digits: String = token[pos + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let path = token[..pos].trim_start_matches("./");
            let path = path.strip_prefix(toplevel).map(|p| p.trim_start_matches('/')).unwrap_or(path);
            if let (Some(f), Ok(line)) = (paths.get(path), digits.parse()) {
                return Some((*f, line));
            }
        }
    }
    None
}

/// The smallest definition in `file` whose lines contain `line`.
fn innermost(g: &Graph, file: u32, line: u32) -> Option<u32> {
    let f = &g.files[file as usize];
    (f.first_def..f.first_def + f.def_count)
        .filter(|d| {
            let def = &g.defs[*d as usize];
            def.line <= line && line <= def.end_line
        })
        .min_by_key(|d| g.defs[*d as usize].end_line - g.defs[*d as usize].line)
}

/// A header prototype stands for its definition (C++), when there is one.
fn definition_of(g: &Graph, d: u32) -> u32 {
    let def = &g.defs[d as usize];
    if !def.decl {
        return d;
    }
    let (name, qual) = lang::support(g.files[def.file as usize].lang).definition_key(def);
    (0..g.defs.len() as u32)
        .find(|x| {
            let o = &g.defs[*x as usize];
            !o.decl && o.name == name && o.qual == qual
        })
        .unwrap_or(d)
}

/// One printed definition.
struct Shown {
    def: u32,
    declared_at: Option<u32>,
    /// First and last line (1-based), including what belongs to the definition.
    start: u32,
    end: u32,
    /// Lines above the definition itself (doc comments, attributes, decorators).
    lead: usize,
    lines: Vec<String>,
}

fn render_one(g: &Graph, src: &mut Source, d: u32, follow: bool) -> Option<Shown> {
    let def_id = if follow { definition_of(g, d) } else { d };
    let def = &g.defs[def_id as usize];
    let file = &g.files[def.file as usize];
    let raw = src.read(g, def.file)?.clone();
    let text = String::from_utf8_lossy(&raw);
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let (start, end) = extent(file.lang, &raw, &lines, def.line, def.end_line);
    let end = end.min(lines.len() as u32);
    Some(Shown {
        def: def_id,
        declared_at: (def_id != d).then_some(d),
        start,
        end,
        lead: def.line.saturating_sub(start) as usize,
        lines: lines[(start - 1) as usize..end as usize].iter().map(|l| l.to_string()).collect(),
    })
}

/// Lines a definition spans, from its syntax: the node's own range, grown to
/// the wrappers and leading comments/attributes that belong to it.
fn extent(lang: Lang, raw: &[u8], lines: &[&str], line: u32, end_line: u32) -> (u32, u32) {
    let support = lang::support(lang);
    let (tree, rewritten) = extract::parse(lang, raw);
    let Some(tree) = tree else { return (line, end_line) };
    let parsed = rewritten.as_deref().unwrap_or(raw);
    let parsed_lines = split_lines(parsed);
    let row = (line - 1) as usize;
    let end_row = (end_line - 1) as usize;
    let indent = |r: usize| indent_of(&parsed_lines, r);
    let Some(mut n) = def_node(&tree, &parsed_lines, line, end_line) else { return (line, end_line) };
    // ...then the wrappers that own it.
    while let Some(p) = n.parent().filter(|p| support.wrapper_kinds().contains(&p.kind())) {
        n = p;
    }
    let mut start = n.start_position().row.min(row);
    let end = n.end_position().row.max(end_row);
    // Doc comments and attributes directly above (each on its own line, no gap).
    let mut cur: Node = n;
    while let Some(prev) = cur.prev_named_sibling() {
        let own_line = indent(prev.start_position().row) == prev.start_position().column;
        if !support.leading_kinds().contains(&prev.kind()) || !own_line || prev.end_position().row + 1 < start {
            break;
        }
        start = prev.start_position().row;
        cur = prev;
    }
    // Lines the language blanked before parsing (Unreal `UFUNCTION(...)`) belong to what follows.
    if rewritten.is_some() {
        while start > 0
            && parsed_lines.get(start - 1).is_some_and(|l| l.iter().all(|b| b.is_ascii_whitespace()))
            && lines.get(start - 1).is_some_and(|l| !l.trim().is_empty())
        {
            start -= 1;
        }
    }
    (start as u32 + 1, end as u32 + 1)
}

pub(crate) fn split_lines(src: &[u8]) -> Vec<&[u8]> {
    src.split(|b| *b == b'\n').collect()
}

fn indent_of(lines: &[&[u8]], row: usize) -> usize {
    lines.get(row).map(|l| l.iter().take_while(|b| b.is_ascii_whitespace()).count()).unwrap_or(0)
}

/// The outermost syntax node spanning a definition's lines (from its first
/// non-blank column to its last), before any wrapper.
pub(crate) fn def_node<'t>(tree: &'t Tree, lines: &[&[u8]], line: u32, end_line: u32) -> Option<Node<'t>> {
    let row = line.checked_sub(1)? as usize;
    let end_row = (end_line.max(line) - 1) as usize;
    let last_col = lines.get(end_row).map(|l| l.iter().rposition(|b| !b.is_ascii_whitespace()).unwrap_or(0)).unwrap_or(0);
    let mut n = tree.root_node().descendant_for_point_range(Point::new(row, indent_of(lines, row)), Point::new(end_row, last_col))?;
    while let Some(p) = n.parent().filter(|p| p.start_byte() == n.start_byte() && p.end_byte() == n.end_byte() && p.parent().is_some()) {
        n = p;
    }
    Some(n)
}

/// Definitions to print: the targets, then (with `--callees`) what each calls.
/// (definition, called by, follow prototypes to their definition) to print.
fn expand(g: &Graph, picked: &Picked, callees: bool) -> Vec<(u32, Option<u32>, bool)> {
    // A prototype and its definition are one thing; keep the prototype (for the note) once.
    let key = |d: u32| if picked.at.contains(&d) { d } else { definition_of(g, d) };
    let mut by_definition: HashMap<u32, usize> = HashMap::new();
    let mut out: Vec<(u32, Option<u32>, bool)> = vec![];
    for d in &picked.defs {
        match by_definition.get(&key(*d)) {
            Some(i) if g.defs[*d as usize].decl => out[*i].0 = *d,
            Some(_) => {}
            None => {
                by_definition.insert(key(*d), out.len());
                out.push((*d, None, !picked.at.contains(d)));
            }
        }
    }
    let defs: Vec<u32> = out.iter().map(|(d, ..)| *d).collect();
    if callees {
        let idx = query::Index::new(g);
        let mut seen: HashSet<u32> = defs.iter().map(|d| definition_of(g, *d)).collect();
        for d in &defs {
            for h in query::walk(g, &idx, &[*d], 1, false) {
                if let Some(c) = h.def.filter(|c| seen.insert(definition_of(g, *c))) {
                    out.push((c, Some(*d), true));
                }
            }
        }
    }
    out
}

pub fn print(g: &Graph, src: &mut Source, picked: &Picked, opts: &Options) {
    let mut first = true;
    for (d, caller, follow) in expand(g, picked, opts.callees) {
        let Some(s) = render_one(g, src, d, follow) else {
            eprintln!("grepum: can't read {}", g.files[g.defs[d as usize].file as usize].path);
            continue;
        };
        if !first {
            println!();
        }
        first = false;
        let def = &g.defs[s.def as usize];
        let path = &g.files[def.file as usize].path;
        let mut head = format!("{} {}  {path}:{}-{}", def.kind.label(), def.qual, s.start, s.end);
        if let Some(decl) = s.declared_at {
            head.push_str(&format!("  (declared at {})", query::loc(g, decl)));
        }
        if let Some(c) = caller {
            head.push_str(&format!("  (called by {})", g.defs[c as usize].qual));
        }
        println!("{head}");
        let width = s.end.to_string().len();
        let gutter = |n: Option<u32>| match (opts.line_numbers, n) {
            (false, _) => String::new(),
            (true, Some(n)) => format!("{n:>width$} │ "),
            (true, None) => format!("{:>width$} │ ", ""),
        };
        let mut next = 0;
        for (from, to) in keep(s.lines.len(), s.lead, opts.max_lines) {
            if from > next {
                println!("{}⋮ {} lines omitted (--max-lines 0 shows all)", gutter(None), from - next);
            }
            for (k, l) in s.lines[from..to].iter().enumerate() {
                println!("{}{}", gutter(Some(s.start + (from + k) as u32)), clip(l));
            }
            next = to;
        }
    }
}

pub fn json(g: &Graph, src: &mut Source, picked: &Picked, opts: &Options) -> Value {
    let items: Vec<Value> = expand(g, picked, opts.callees)
        .into_iter()
        .filter_map(|(d, caller, follow)| {
            let s = render_one(g, src, d, follow)?;
            let def = &g.defs[s.def as usize];
            let ranges = keep(s.lines.len(), s.lead, opts.max_lines);
            let omitted = s.lines.len() - ranges.iter().map(|(a, b)| b - a).sum::<usize>();
            let mut code: Vec<&str> = vec![];
            let mut next = 0;
            for (from, to) in ranges {
                if from > next {
                    code.push("⋮");
                }
                code.extend(s.lines[from..to].iter().map(|l| l.as_str()));
                next = to;
            }
            Some(json!({
                "symbol": def.qual, "kind": def.kind.label(), "file": g.files[def.file as usize].path,
                "line": s.start, "end_line": s.end, "code": code.join("\n"), "omitted": omitted,
                "declared_at": s.declared_at.map(|x| query::loc(g, x)),
                "called_by": caller.map(|c| g.defs[c as usize].qual.clone()),
            }))
        })
        .collect();
    Value::Array(items)
}

/// Line ranges to print of a definition of `n` lines whose first `lead` are
/// its doc comment/attributes. Within `max`, everything; beyond it, the first
/// two lead lines, then the code's start and its end, so the signature and
/// the exits stay visible.
fn keep(n: usize, lead: usize, max: usize) -> Vec<(usize, usize)> {
    if max == 0 || n <= max {
        return vec![(0, n)];
    }
    let mut out = vec![];
    let lead_kept = lead.min(2);
    if lead_kept > 0 {
        out.push((0, lead_kept));
    }
    let budget = max.saturating_sub(lead_kept).max(2);
    let tail = (budget / 3).max(1);
    let head_end = (lead + budget - tail).min(n - tail);
    if head_end > lead {
        out.push((lead, head_end));
    }
    out.push((n - tail, n));
    out
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => format!("{}… ({} more chars)", &line[..i], line[i..].chars().count()),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::keep;

    #[test]
    fn long_definitions_keep_start_and_end() {
        assert_eq!(keep(10, 0, 200), vec![(0, 10)]);
        assert_eq!(keep(300, 0, 0), vec![(0, 300)]);
        assert_eq!(keep(300, 0, 30), vec![(0, 20), (290, 300)]);
        // A long doc comment is cut to two lines; the budget goes to the code.
        assert_eq!(keep(300, 25, 32), vec![(0, 2), (25, 45), (290, 300)]);
    }
}

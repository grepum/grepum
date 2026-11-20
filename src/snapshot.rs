//! Producing a graph for "a revision" or "this worktree right now".
//!
//! Layer 1 (facts by blob id) and layer 2 (graph by tree id) live in the
//! shared common dir; layer 3 (overlay for uncommitted edits) is private to
//! the worktree and keyed by the exact dirty state, so it's reused until the
//! next edit.

use crate::extract::extract;
use crate::facts::{FileFacts, Lang};
use crate::git::{Change, Repo};
use crate::graph::{self, Graph, Input};
use crate::store::{self, Store};
use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

const MAX_FILE_BYTES: usize = 1 << 20;

#[derive(Default, Debug)]
pub struct Report {
    pub source: &'static str,
    pub files: usize,
    pub facts_cached: usize,
    pub facts_extracted: usize,
    pub ms: f64,
}

pub struct Snapshot {
    pub graph: Graph,
    pub label: String,
    pub report: Report,
}

#[derive(Serialize, Deserialize)]
struct Overlay {
    key: String,
    graph: Graph,
}

fn source_entries(entries: Vec<(String, String)>) -> Vec<(String, String, Lang)> {
    entries
        .into_iter()
        .filter_map(|(p, o)| Lang::from_path(&p).map(|l| (p, o, l)))
        .collect()
}

/// Build a graph from (path, oid, lang). Blob contents come from the cache,
/// then `inline` (worktree files), then the object database.
fn build(repo: &Repo, store: &Store, entries: Vec<(String, String, Lang)>, inline: &HashMap<String, Vec<u8>>, report: &mut Report) -> Result<Graph> {
    let cached: Vec<Option<FileFacts>> = entries.par_iter().map(|(_, oid, lang)| store.load_facts(oid, *lang)).collect();
    let missing: Vec<String> = entries
        .iter()
        .zip(&cached)
        .filter(|(e, c)| c.is_none() && !inline.contains_key(&e.1))
        .map(|(e, _)| e.1.clone())
        .collect();
    let blobs = repo.read_blobs(&missing)?;
    let extracted = std::sync::atomic::AtomicUsize::new(0);
    let facts: Vec<Option<Arc<FileFacts>>> = entries
        .par_iter()
        .zip(cached)
        .map(|((_, oid, lang), c)| {
            if let Some(c) = c {
                return Some(Arc::new(c));
            }
            let content = inline.get(oid).or_else(|| blobs.get(oid))?;
            if content.len() > MAX_FILE_BYTES {
                return None;
            }
            let f = extract(*lang, content);
            extracted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = store.save_facts(oid, *lang, &f);
            Some(Arc::new(f))
        })
        .collect();
    let mut inputs = Vec::with_capacity(entries.len());
    for ((path, oid, _), f) in entries.into_iter().zip(facts) {
        if let Some(f) = f {
            inputs.push(Input { path, oid, facts: f });
        }
    }
    report.facts_extracted = extracted.into_inner();
    report.facts_cached = inputs.len() - report.facts_extracted;
    report.files = inputs.len();
    Ok(graph::build(inputs))
}

pub fn for_rev(repo: &Repo, store: &Store, rev: &str, rebuild: bool) -> Result<Snapshot> {
    let t = Instant::now();
    let mut report = Report::default();
    let Some(tree) = repo.tree_of(rev)? else {
        anyhow::bail!("unknown revision: {rev}");
    };
    let path = store.tree_path(&tree);
    if !rebuild {
        if let Some(graph) = store::load::<Graph>(&path) {
            report.source = "tree cache";
            report.files = graph.files.len();
            report.ms = t.elapsed().as_secs_f64() * 1e3;
            return Ok(Snapshot { graph, label: format!("{rev} (tree {})", &tree[..12]), report });
        }
    }
    let entries = repo.ls_tree(&tree)?.into_iter().map(|e| (e.path, e.oid)).collect();
    let graph = build(repo, store, source_entries(entries), &HashMap::new(), &mut report)?;
    store::save(&path, &graph)?;
    report.source = "built";
    report.ms = t.elapsed().as_secs_f64() * 1e3;
    Ok(Snapshot { graph, label: format!("{rev} (tree {})", &tree[..12]), report })
}

pub fn for_worktree(repo: &Repo, store: &Store, rebuild: bool) -> Result<Snapshot> {
    let t = Instant::now();
    let changes: Vec<Change> = repo
        .changes()?
        .into_iter()
        .filter(|c| match c {
            Change::Present(p) | Change::Deleted(p) => Lang::from_path(p).is_some(),
        })
        .collect();
    let head_tree = repo.tree_of("HEAD")?;
    if changes.is_empty() {
        if head_tree.is_some() {
            let mut s = for_rev(repo, store, "HEAD", rebuild)?;
            s.label = format!("worktree, clean at {}", s.label);
            s.report.ms = t.elapsed().as_secs_f64() * 1e3;
            return Ok(s);
        }
        return Ok(Snapshot { graph: Graph::default(), label: "empty".into(), report: Report::default() });
    }

    // Dirty: base tree entries with the worktree's changes applied.
    let present: Vec<String> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Present(p) if repo.toplevel.join(p).is_file() => Some(p.clone()),
            _ => None,
        })
        .collect();
    let oids = repo.hash_files(&present)?;
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    if let Some(tree) = &head_tree {
        for e in repo.ls_tree(tree)? {
            files.insert(e.path, e.oid);
        }
    }
    for c in &changes {
        if let Change::Deleted(p) = c {
            files.remove(p);
        }
    }
    for (p, o) in present.iter().zip(&oids) {
        files.insert(p.clone(), o.clone());
    }

    let mut hasher = Sha256::new();
    hasher.update(head_tree.as_deref().unwrap_or("unborn"));
    for c in &changes {
        match c {
            Change::Deleted(p) => hasher.update(format!("\0-{p}")),
            Change::Present(p) => hasher.update(format!("\0+{p}={}", files.get(p).map(|s| s.as_str()).unwrap_or(""))),
        }
    }
    let key = format!("{:x}", hasher.finalize());
    let label = format!("worktree, {} uncommitted file(s) on top of HEAD", changes.len());
    let mut report = Report::default();

    if !rebuild {
        if let Some(ov) = store::load::<Overlay>(&store.overlay_path()) {
            if ov.key == key {
                report.source = "overlay cache";
                report.files = ov.graph.files.len();
                report.ms = t.elapsed().as_secs_f64() * 1e3;
                return Ok(Snapshot { graph: ov.graph, label, report });
            }
        }
    }

    // Only the dirty files that aren't cached yet need to be read from disk.
    let mut inline = HashMap::new();
    for (p, o) in present.iter().zip(&oids) {
        if let Some(lang) = Lang::from_path(p) {
            if store.load_facts(o, lang).is_none() {
                if let Ok(bytes) = std::fs::read(repo.toplevel.join(p)) {
                    inline.insert(o.clone(), bytes);
                }
            }
        }
    }
    let graph = build(repo, store, source_entries(files.into_iter().collect()), &inline, &mut report)?;
    let ov = Overlay { key, graph };
    store::save(&store.overlay_path(), &ov)?;
    report.source = "built overlay";
    report.ms = t.elapsed().as_secs_f64() * 1e3;
    Ok(Snapshot { graph: ov.graph, label, report })
}

//! Symbol-level history from git.
//!
//! `git log --raw` gives, per commit, the old and new blob ids of every changed
//! file without reading any file contents. Facts for those blobs come from the
//! L1 cache (parsing only versions never seen before), and each definition's
//! body fingerprint tells "changed" from "unchanged". Comparing the definitions
//! of a commit's changed files before and after yields added / modified /
//! moved / removed events for every symbol.

use crate::extract::extract;
use crate::facts::{DefKind, FileFacts, Lang};
use crate::git::{is_null_oid, Commit, Repo};
use crate::store::{self, Store};
use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const MAX_FILE_BYTES: usize = 1 << 20;
/// Blobs read from git per batch, to bound memory on long histories.
const BLOB_BATCH: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Added,
    Modified,
    Moved,
    Removed,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Modified => "modified",
            Change::Moved => "moved",
            Change::Removed => "removed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub commit: String,
    pub time: i64,
    pub author: String,
    pub subject: String,
    pub change: Change,
    pub kind: &'static str,
    pub qual: String,
    pub name: String,
    /// Where the symbol is after the commit (before it, for `removed`).
    pub path: String,
    pub line: u32,
    /// Previous path, for `moved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

pub struct History {
    pub commits: usize,
    pub events: Vec<Event>,
    /// Commits whose symbol changes came from the cache.
    pub commits_cached: usize,
    /// File versions parsed / loaded from the facts cache for the other commits.
    pub parsed: usize,
    pub cached: usize,
}

/// What one commit did to one symbol; cached per commit id, commit metadata
/// comes from `git log`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Delta {
    change: Change,
    kind: DefKind,
    qual: String,
    name: String,
    path: String,
    line: u32,
    from: Option<String>,
}

struct Sym {
    qual: String,
    name: String,
    kind: DefKind,
    line: u32,
    body: u64,
}

/// Definitions worth tracking, with qualified names built like the graph's.
fn symbols(f: &FileFacts) -> Vec<Sym> {
    let mut quals: Vec<String> = Vec::with_capacity(f.defs.len());
    let mut out = vec![];
    for d in &f.defs {
        let qual = match (d.parent, &d.owner) {
            (Some(p), _) => format!("{}.{}", quals.get(p as usize).map(String::as_str).unwrap_or(""), d.name),
            (None, Some(o)) => format!("{o}.{}", d.name),
            _ => d.name.clone(),
        };
        quals.push(qual.clone());
        if d.decl || d.kind == DefKind::Module {
            continue;
        }
        out.push(Sym { qual, name: d.name.clone(), kind: d.kind, line: d.line, body: d.body });
    }
    out
}

/// Facts for every (blob, lang) the history touches: cache first, then parse.
fn load_facts(repo: &Repo, store: &Store, wanted: Vec<(String, Lang)>) -> Result<(HashMap<String, Arc<FileFacts>>, usize, usize)> {
    let cached: Vec<Option<FileFacts>> = wanted.par_iter().map(|(oid, lang)| store.load_facts(oid, *lang)).collect();
    let mut facts: HashMap<String, Arc<FileFacts>> = HashMap::with_capacity(wanted.len());
    let mut missing = vec![];
    for ((oid, lang), c) in wanted.into_iter().zip(cached) {
        match c {
            Some(f) => {
                facts.insert(oid, Arc::new(f));
            }
            None => missing.push((oid, lang)),
        }
    }
    let n_cached = facts.len();
    let mut n_parsed = 0;
    for chunk in missing.chunks(BLOB_BATCH) {
        let oids: Vec<String> = chunk.iter().map(|(o, _)| o.clone()).collect();
        let blobs = repo.read_blobs(&oids)?;
        let parsed: Vec<(String, FileFacts)> = chunk
            .par_iter()
            .filter_map(|(oid, lang)| {
                let content = blobs.get(oid)?;
                if content.len() > MAX_FILE_BYTES {
                    return None;
                }
                let f = extract(*lang, content);
                let _ = store.save_facts(oid, *lang, &f);
                Some((oid.clone(), f))
            })
            .collect();
        n_parsed += parsed.len();
        facts.extend(parsed.into_iter().map(|(o, f)| (o, Arc::new(f))));
    }
    Ok((facts, n_cached, n_parsed))
}

/// Every symbol event on the first-parent history of `rev`, newest first.
pub fn scan(repo: &Repo, store: &Store, rev: &str, max_commits: Option<usize>) -> Result<History> {
    let mut commits = repo.log_raw(rev, max_commits)?;
    for c in &mut commits {
        c.files.retain(|f| Lang::from_path(&f.path).is_some());
    }
    let mut deltas: Vec<Option<Vec<Delta>>> = commits
        .par_iter()
        .map(|c| if c.files.is_empty() { Some(vec![]) } else { store::load(&store.commit_path(&c.oid)) })
        .collect();
    let commits_cached = commits.iter().zip(&deltas).filter(|(c, d)| !c.files.is_empty() && d.is_some()).count();

    let mut wanted: HashSet<(String, Lang)> = HashSet::new();
    for (c, _) in commits.iter().zip(&deltas).filter(|(_, d)| d.is_none()) {
        for f in &c.files {
            let lang = Lang::from_path(&f.path).unwrap();
            for oid in [&f.old, &f.new] {
                if !is_null_oid(oid) {
                    wanted.insert((oid.clone(), lang));
                }
            }
        }
    }
    let (facts, cached, parsed) = load_facts(repo, store, wanted.into_iter().collect())?;
    commits.par_iter().zip(deltas.par_iter_mut()).filter(|(_, d)| d.is_none()).for_each(|(c, d)| {
        let fresh = diff_commit(c, &facts);
        let _ = store::save(&store.commit_path(&c.oid), &fresh);
        *d = Some(fresh);
    });

    let events = commits
        .iter()
        .zip(deltas)
        .flat_map(|(c, d)| {
            d.unwrap_or_default().into_iter().map(move |x| Event {
                commit: c.oid.clone(),
                time: c.time,
                author: c.author.clone(),
                subject: c.subject.clone(),
                change: x.change,
                kind: x.kind.label(),
                qual: x.qual,
                name: x.name,
                path: x.path,
                line: x.line,
                from: x.from,
            })
        })
        .collect();
    Ok(History { commits: commits.len(), events, commits_cached, parsed, cached })
}

type Key = (String, DefKind, String, usize); // qual, kind, path, n-th occurrence in that file

fn side(c: &Commit, facts: &HashMap<String, Arc<FileFacts>>, old: bool) -> HashMap<Key, Sym> {
    let mut out = HashMap::new();
    for f in &c.files {
        let oid = if old { &f.old } else { &f.new };
        let Some(ff) = (!is_null_oid(oid)).then(|| facts.get(oid)).flatten() else { continue };
        let mut seen: HashMap<(String, DefKind), usize> = HashMap::new();
        for s in symbols(ff) {
            let n = seen.entry((s.qual.clone(), s.kind)).or_default();
            out.insert((s.qual.clone(), s.kind, f.path.clone(), *n), s);
            *n += 1;
        }
    }
    out
}

fn diff_commit(c: &Commit, facts: &HashMap<String, Arc<FileFacts>>) -> Vec<Delta> {
    let before = side(c, facts, true);
    let after = side(c, facts, false);
    let ev = |change, s: &Sym, path: &str, from: Option<String>| Delta {
        change,
        kind: s.kind,
        qual: s.qual.clone(),
        name: s.name.clone(),
        path: path.to_string(),
        line: s.line,
        from,
    };
    let mut events = vec![];
    // Symbols that vanished from one file, available to pair with an appearance elsewhere.
    let mut gone: HashMap<(String, DefKind), Vec<&Key>> = HashMap::new();
    let mut keys: Vec<&Key> = before.keys().collect();
    keys.sort();
    for k in keys {
        match after.get(k) {
            Some(new) if new.body != before[k].body => events.push(ev(Change::Modified, new, &k.2, None)),
            Some(_) => {}
            None => gone.entry((k.0.clone(), k.1)).or_default().push(k),
        }
    }
    let mut keys: Vec<&Key> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    keys.sort();
    for k in keys {
        let s = &after[k];
        let moved_from = gone.get_mut(&(k.0.clone(), k.1)).and_then(|v| {
            let i = v.iter().position(|o| o.2 != k.2)?;
            Some(v.remove(i))
        });
        match moved_from {
            Some(old) => events.push(ev(Change::Moved, s, &k.2, Some(old.2.clone()))),
            None => events.push(ev(Change::Added, s, &k.2, None)),
        }
    }
    let mut left: Vec<&Key> = gone.into_values().flatten().collect();
    left.sort();
    for k in left {
        events.push(ev(Change::Removed, &before[k], &k.2, None));
    }
    events
}

/// `Name`, `Owner.name`, `Owner::name` or a dotted suffix of the qualified name.
pub fn matches(e: &Event, query: &str) -> bool {
    let q = query.replace("::", ".");
    e.qual == q || e.name == q || e.qual.ends_with(&format!(".{q}"))
}

/// Symbols removed in history and not defined anymore at `current`, newest removal first.
pub fn deleted<'a>(h: &'a History, current: &HashSet<String>) -> Vec<&'a Event> {
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut out = vec![];
    // Events are newest first; the first removal seen per symbol is its latest.
    // A later re-add (seen earlier in this walk) means it isn't deleted.
    let mut readded: HashSet<(&str, &str)> = HashSet::new();
    for e in &h.events {
        let key = (e.qual.as_str(), e.kind);
        match e.change {
            Change::Added | Change::Moved | Change::Modified => {
                readded.insert(key);
            }
            Change::Removed => {
                if !readded.contains(&key) && !current.contains(&e.qual) && seen.insert(key) {
                    out.push(e);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn commit(dir: &Path, files: &[(&str, Option<&str>)], msg: &str) {
        for (path, content) in files {
            let p = dir.join(path);
            match content {
                Some(c) => {
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, c).unwrap();
                }
                None => std::fs::remove_file(&p).unwrap(),
            }
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", msg]);
    }

    #[test]
    fn added_modified_moved_removed() {
        let dir = std::env::temp_dir().join(format!("grepum-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        commit(&dir, &[("a.py", Some("def keep():\n    return 1\n\ndef gone():\n    return 2\n"))], "add");
        // whitespace-only change: not a modification
        commit(&dir, &[("a.py", Some("def keep():\n        return 1\n\ndef gone():\n    return 2\n"))], "reformat");
        commit(&dir, &[("a.py", Some("def keep():\n    return 10\n\ndef gone():\n    return 2\n"))], "change");
        commit(&dir, &[("a.py", Some("def keep():\n    return 10\n")), ("b.py", Some("def gone():\n    return 2\n"))], "move");
        commit(&dir, &[("b.py", None)], "delete");

        let repo = Repo::discover(&dir).unwrap();
        let store = Store::open(&repo);
        let h = scan(&repo, &store, "HEAD", None).unwrap();
        let got: Vec<(String, &str, &str)> =
            h.events.iter().map(|e| (e.subject.clone(), e.qual.as_str(), e.change.label())).collect();
        let want = [
            ("delete", "gone", "removed"),
            ("move", "gone", "moved"),
            ("change", "keep", "modified"),
            ("add", "gone", "added"),
            ("add", "keep", "added"),
        ];
        let want: Vec<(String, &str, &str)> = want.iter().map(|(a, b, c)| (a.to_string(), *b, *c)).collect();
        assert_eq!(got, want);

        // cached second pass gives the same answer
        let again = scan(&repo, &store, "HEAD", None).unwrap();
        assert_eq!(again.events.len(), h.events.len());
        assert_eq!(again.commits_cached, 5);

        let current: HashSet<String> = HashSet::from(["keep".to_string()]);
        let gone = deleted(&h, &current);
        assert_eq!(gone.len(), 1);
        assert_eq!((gone[0].qual.as_str(), gone[0].path.as_str()), ("gone", "b.py"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

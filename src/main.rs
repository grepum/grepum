mod extract;
mod facts;
mod git;
mod git_hooks;
mod graph;
mod history;
mod html;
mod install;
mod lang;
mod query;
mod snapshot;
mod store;
mod upgrade;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use git::Repo;
use snapshot::Snapshot;
use std::path::PathBuf;
use store::Store;

#[derive(Parser)]
#[command(name = "grepum", version, about = "A code graph woven into git. Shared across worktrees, queryable at any revision.")]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true)]
    dir: Option<PathBuf>,
    /// Query a revision instead of the current worktree state.
    #[arg(long, global = true)]
    rev: Option<String>,
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build (or refresh) the graph for the worktree or --rev.
    Index {
        #[arg(long)]
        rebuild: bool,
        #[arg(long, short)]
        quiet: bool,
    },
    /// Show what the graph is built from and how big the cache is.
    Status,
    /// Short orientation for an agent's context: size, languages, most-called symbols, uncommitted changes.
    Summary {
        #[arg(short, long, default_value_t = 8)]
        top: usize,
    },
    /// Ranked symbol search.
    Find {
        query: Vec<String>,
        #[arg(short, long, default_value_t = 15)]
        limit: usize,
        /// Also search symbols that were deleted in the history of HEAD (or --rev).
        #[arg(long)]
        history: bool,
    },
    /// History of a symbol: the commits that added, modified, moved or removed it.
    Log {
        symbol: String,
        /// Show at most this many events.
        #[arg(short = 'n', long, default_value_t = 30)]
        limit: usize,
        /// Only look at the newest N commits.
        #[arg(long)]
        max_commits: Option<usize>,
    },
    /// Who calls this symbol (transitively with --depth).
    Callers {
        symbol: String,
        #[arg(short, long, default_value_t = 1)]
        depth: u32,
    },
    /// What this symbol calls.
    Callees {
        symbol: String,
        #[arg(short, long, default_value_t = 1)]
        depth: u32,
    },
    /// Compact neighbourhood of the best matches, sized for an agent prompt.
    Context {
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 5)]
        symbols: usize,
    },
    /// Structural diff: symbols and call edges added/removed between two states.
    Diff {
        from: String,
        /// Defaults to the current worktree (including uncommitted edits).
        to: Option<String>,
    },
    /// Dump the whole graph as JSON.
    Export,
    /// Set up an integration: `claude` (skill + code-navigator agent + session hook) or `git-hooks`.
    Install {
        target: Target,
        /// Claude Code: `user` (~/.claude, all projects) or `project` (./.claude, commit to share).
        #[arg(long, value_enum, default_value_t = install::Scope::User)]
        scope: install::Scope,
        /// Overwrite skill/agent files that weren't written by grepum.
        #[arg(long)]
        force: bool,
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove an integration installed with `install`.
    Uninstall {
        target: Target,
        #[arg(long, value_enum, default_value_t = install::Scope::User)]
        scope: install::Scope,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Install git hooks that keep the graph warm after commit/checkout/merge/rewrite.
    Hooks {
        #[arg(value_parser = ["install", "uninstall"])]
        action: String,
        /// Install even into a hooks directory that is part of the working tree.
        #[arg(long)]
        force: bool,
    },
    /// Write a self-contained HTML page to explore the graph (map + call neighbourhoods).
    Html {
        /// Open this symbol's call neighbourhood first.
        symbol: Option<String>,
        /// Where to write the page (default: inside this worktree's git dir).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Open it in the default browser.
        #[arg(long)]
        open: bool,
    },
    /// Delete all cached data for this repository.
    Clear,
    /// Update grepum to the latest release (for installs made by the release installer).
    Upgrade {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
        /// Reinstall even when already up to date.
        #[arg(long)]
        force: bool,
    },
    /// Print the facts extracted from one file (no git needed). For debugging extractors.
    #[command(hide = true)]
    Facts { file: PathBuf },
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Target {
    /// Claude Code skill, subagent and SessionStart hook.
    Claude,
    /// post-commit/checkout/merge/rewrite hooks in this repository.
    GitHooks,
}

fn snapshot(repo: &Repo, store: &Store, rev: Option<&str>, rebuild: bool) -> Result<Snapshot> {
    match rev {
        Some(r) => snapshot::for_rev(repo, store, r, rebuild),
        None => snapshot::for_worktree(repo, store, rebuild),
    }
}

/// Deleted symbols ranked by how well their names match the query words.
fn rank_deleted<'a>(gone: &[&'a history::Event], q: &str, limit: usize) -> Vec<&'a history::Event> {
    let qt: Vec<String> = q.split_whitespace().flat_map(query::words).collect();
    let mut scored: Vec<(f64, usize)> = gone
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let toks = query::words(&e.qual);
            let mut s = 0.0;
            let mut hit = 0;
            for t in &qt {
                let best = toks
                    .iter()
                    .map(|x| if x == t { 1.0 } else if x.starts_with(t.as_str()) { 0.6 } else if x.contains(t.as_str()) { 0.3 } else { 0.0 })
                    .fold(0.0, f64::max);
                if best > 0.0 {
                    hit += 1;
                }
                s += best;
            }
            (hit > 0).then(|| (s * (hit as f64 / qt.len() as f64).powi(2), i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, i)| gone[i]).collect()
}

/// UTC date of a unix timestamp (civil-from-days, no date library needed).
fn date(ts: i64) -> String {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    rayon::ThreadPoolBuilder::new().stack_size(16 << 20).build_global().ok();
    if let Cmd::Facts { file } = &cli.cmd {
        let lang = facts::Lang::from_path(&file.to_string_lossy()).ok_or_else(|| anyhow::anyhow!("unsupported file type"))?;
        let f = extract::extract(lang, &std::fs::read(file)?);
        println!("{}", serde_json::to_string_pretty(&f)?);
        return Ok(());
    }
    let cwd = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()?,
    };
    match cli.cmd {
        Cmd::Upgrade { check, force } => return upgrade::run(&upgrade::Options { check, force }),
        Cmd::Install { target: Target::Claude, scope, force, dry_run } => {
            return install::install_claude(&cwd, &install::Options { scope, force, dry_run });
        }
        Cmd::Uninstall { target: Target::Claude, scope, force, dry_run } => {
            return install::uninstall_claude(&cwd, &install::Options { scope, force, dry_run });
        }
        _ => {}
    }
    let repo = Repo::discover(&cwd)?;
    let store = Store::open(&repo);
    let rev = cli.rev.as_deref();

    match cli.cmd {
        Cmd::Index { rebuild, quiet } => {
            let s = snapshot(&repo, &store, rev, rebuild)?;
            if cli.json {
                let st = &s.graph.stats;
                print_json(&serde_json::json!({
                    "label": s.label, "source": s.report.source, "files": s.graph.files.len(), "defs": s.graph.defs.len(),
                    "edges": s.graph.edges.len(), "facts_cached": s.report.facts_cached, "facts_extracted": s.report.facts_extracted,
                    "ms": s.report.ms, "refs": st.refs, "exact": st.exact, "inferred": st.inferred,
                }));
            } else if !quiet {
                let r = &s.report;
                println!("{}", s.label);
                println!(
                    "  {} files, {} symbols, {} edges  [{}; {} facts reused, {} extracted]  {:.1} ms",
                    s.graph.files.len(), s.graph.defs.len(), s.graph.edges.len(), r.source, r.facts_cached, r.facts_extracted, r.ms
                );
                let st = &s.graph.stats;
                if st.refs > 0 {
                    println!(
                        "  references: {} exact, {} inferred, {} ambiguous (dropped), {} external/unknown",
                        st.exact, st.inferred, st.ambiguous, st.unresolved
                    );
                }
            }
        }
        Cmd::Status => {
            let (shared, local) = store.disk_usage();
            let s = snapshot(&repo, &store, rev, false)?;
            println!("worktree      {}", repo.toplevel.display());
            println!("graph         {}  ({} files, {} symbols, {} edges, {})", s.label, s.graph.files.len(), s.graph.defs.len(), s.graph.edges.len(), s.report.source);
            println!("shared cache  {}  ({:.1} KiB, all worktrees)", store.shared.display(), shared as f64 / 1024.0);
            println!("local cache   {}  ({:.1} KiB, this worktree)", store.local.display(), local as f64 / 1024.0);
        }
        Cmd::Summary { top } => {
            let s = snapshot(&repo, &store, rev, false)?;
            let g = &s.graph;
            let mut langs: std::collections::BTreeMap<String, usize> = Default::default();
            for f in &g.files {
                *langs.entry(format!("{:?}", f.lang)).or_default() += 1;
            }
            let langs: Vec<String> = langs.iter().map(|(l, n)| format!("{l} {n}")).collect();
            println!("grepum code graph: {}", s.label);
            println!("  {} files ({}), {} symbols, {} call/inherit edges", g.files.len(), langs.join(", "), g.defs.len(), g.edges.len());
            let idx = query::Index::new(g);
            let mut hubs: Vec<(usize, u32)> = idx
                .incoming
                .iter()
                .map(|(d, v)| (v.iter().filter(|e| g.edges[**e].kind == graph::EdgeKind::Call).count(), *d))
                .filter(|(n, _)| *n > 0)
                .collect();
            hubs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let hubs: Vec<String> = hubs.iter().take(top).map(|(n, d)| format!("{} ({n})", g.defs[*d as usize].qual)).collect();
            if !hubs.is_empty() {
                println!("  most-called: {}", hubs.join(", "));
            }
            println!("  query it: grepum find <words> | callers <Symbol> | callees <Symbol> | context <words> | diff <rev>");
        }
        Cmd::Find { query, limit, history } => {
            let s = snapshot(&repo, &store, rev, false)?;
            let idx = query::Index::new(&s.graph);
            let hits = query::find(&s.graph, &idx, &query.join(" "), limit);
            if cli.json {
                print_json(&serde_json::Value::Array(
                    hits.iter()
                        .map(|(sc, d)| {
                            let def = &s.graph.defs[*d as usize];
                            serde_json::json!({"symbol": def.qual, "kind": def.kind.label(), "at": query::loc(&s.graph, *d), "score": sc})
                        })
                        .collect(),
                ));
            } else {
                for (_, d) in hits {
                    let def = &s.graph.defs[d as usize];
                    let tags = if def.attrs.is_empty() { String::new() } else { format!("  [{}]", def.attrs.join(", ")) };
                    println!("{:<9} {:<40} {}{tags}", def.kind.label(), def.qual, query::loc(&s.graph, d));
                }
            }
            if history {
                let t = std::time::Instant::now();
                let h = history::scan(&repo, &store, rev.unwrap_or("HEAD"), None)?;
                let current: std::collections::HashSet<String> =
                    s.graph.defs.iter().filter(|d| !d.decl).map(|d| d.qual.clone()).collect();
                let gone = history::deleted(&h, &current);
                let ranked = rank_deleted(&gone, &query.join(" "), limit);
                if cli.json {
                    print_json(&serde_json::json!({"deleted": ranked}));
                } else {
                    println!();
                    if ranked.is_empty() {
                        println!("no deleted symbols match ({} commits searched)", h.commits);
                    } else {
                        println!("deleted symbols ({} commits searched in {:.0} ms):", h.commits, t.elapsed().as_secs_f64() * 1e3);
                        for e in ranked {
                            let short = &e.commit[..10.min(e.commit.len())];
                            println!(
                                "{:<9} {:<40} {}:{}  removed {} in {short}  (git show {short}^:{})",
                                e.kind, e.qual, e.path, e.line, date(e.time), e.path
                            );
                        }
                    }
                }
            }
        }
        Cmd::Log { ref symbol, limit, max_commits } => {
            let t = std::time::Instant::now();
            let h = history::scan(&repo, &store, rev.unwrap_or("HEAD"), max_commits)?;
            let hits: Vec<&history::Event> = h.events.iter().filter(|e| history::matches(e, symbol)).take(limit).collect();
            if cli.json {
                print_json(&serde_json::json!({"symbol": symbol, "commits_searched": h.commits, "events": hits}));
            } else {
                if hits.is_empty() {
                    println!("no history for {symbol} in {} commits", h.commits);
                }
                for e in &hits {
                    let from = e.from.as_ref().map(|f| format!(" (from {f})")).unwrap_or_default();
                    println!(
                        "{}  {}  {:<8}  {:<6} {}  {}:{}{from}  {}",
                        date(e.time), &e.commit[..10.min(e.commit.len())], e.change.label(), e.kind, e.qual, e.path, e.line,
                        truncate(&e.subject, 60)
                    );
                }
                eprintln!(
                    "{} commits searched in {:.0} ms ({} from cache; {} file versions parsed, {} reused)",
                    h.commits, t.elapsed().as_secs_f64() * 1e3, h.commits_cached, h.parsed, h.cached
                );
            }
        }
        Cmd::Callers { ref symbol, depth } | Cmd::Callees { ref symbol, depth } => {
            let reverse = matches!(cli.cmd, Cmd::Callers { .. });
            let s = snapshot(&repo, &store, rev, false)?;
            let g = &s.graph;
            let starts = query::resolve_symbol(g, symbol);
            let idx = query::Index::new(g);
            if starts.is_empty() {
                // Suggest the closest names instead of making the caller guess again.
                let words = symbol.replace("::", " ").replace(['.', '_'], " ");
                let near: Vec<String> = query::find(g, &idx, &words, 5).into_iter().map(|(_, d)| g.defs[d as usize].qual.clone()).collect();
                if near.is_empty() {
                    bail!("no symbol named {symbol:?} (try `grepum find <words>`)");
                }
                bail!("no symbol named {symbol:?}; did you mean:\n  {}", near.join("\n  "));
            }
            let hops = query::walk(g, &idx, &starts, depth, reverse);
            if cli.json {
                print_json(&serde_json::json!({
                    "symbols": starts.iter().map(|d| serde_json::json!({"symbol": g.defs[*d as usize].qual, "at": query::loc(g, *d)})).collect::<Vec<_>>(),
                    if reverse { "callers" } else { "callees" }: query::hops_json(g, &hops),
                }));
            } else {
                for d in &starts {
                    println!("{} {}  {}", g.defs[*d as usize].kind.label(), g.defs[*d as usize].qual, query::loc(g, *d));
                }
                let arrow = if reverse { "<-" } else { "->" };
                for h in &hops {
                    let (n, at) = query::hop_label(g, h);
                    let mark = if h.conf == graph::Conf::Inferred { "  (inferred)" } else { "" };
                    println!("{}{arrow} {n}  {at}{mark}", "  ".repeat(h.depth as usize));
                }
                if hops.is_empty() {
                    println!("  (no {} found)", if reverse { "callers" } else { "callees" });
                    if reverse {
                        // A call graph can miss callers (receivers it can't type, macros,
                        // reflection, data-driven dispatch). Don't let this read as "dead code".
                        let name = g.defs[starts[0] as usize].name.as_str();
                        println!("  note: not proof it's unused; confirm with a text search, e.g. `rg -w {name}`");
                    }
                }
            }
        }
        Cmd::Context { query, symbols } => {
            let s = snapshot(&repo, &store, rev, false)?;
            let idx = query::Index::new(&s.graph);
            print!("{}", query::context(&s.graph, &idx, &query.join(" "), symbols));
        }
        Cmd::Diff { from, to } => {
            let a = snapshot::for_rev(&repo, &store, &from, false)?;
            let b = snapshot(&repo, &store, to.as_deref(), false)?;
            let d = query::diff(&a.graph, &b.graph);
            if cli.json {
                print_json(&d);
            } else {
                println!("{}  ->  {}", a.label, b.label);
                for (key, sign) in [("symbols_added", "+"), ("symbols_removed", "-")] {
                    for v in d[key].as_array().unwrap() {
                        println!("{sign} {:<9} {:<40} {}", v["kind"].as_str().unwrap(), v["symbol"].as_str().unwrap(), v["at"].as_str().unwrap());
                    }
                }
                for (key, sign) in [("calls_added", "+"), ("calls_removed", "-")] {
                    for v in d[key].as_array().unwrap() {
                        println!("{sign} call  {}  ->  {}", v["from"].as_str().unwrap(), v["to"].as_str().unwrap());
                    }
                }
            }
        }
        Cmd::Export => {
            let s = snapshot(&repo, &store, rev, false)?;
            println!("{}", serde_json::to_string(&query::export(&s.graph))?);
        }
        Cmd::Html { symbol, output, open } => {
            let s = snapshot(&repo, &store, rev, false)?;
            let page = html::render(&s.graph, &s.label)?;
            let path = output.unwrap_or_else(|| store.local.join("graph.html"));
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, page)?;
            let path = path.canonicalize().unwrap_or(path);
            let frag = symbol.map(|sym| format!("#focus={}", sym.replace('%', "%25").replace(' ', "%20").replace('#', "%23"))).unwrap_or_default();
            let shown = path.display().to_string();
            let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown).replace('\\', "/");
            println!("wrote {} ({:.0} KiB)", path.display(), std::fs::metadata(&path)?.len() as f64 / 1024.0);
            println!("file://{}{}{frag}", if shown.starts_with('/') { "" } else { "/" }, shown);
            if open {
                html::open(&path);
            }
        }
        Cmd::Hooks { action, force } => git_hooks::run(&repo, action == "install", force)?,
        Cmd::Facts { .. } | Cmd::Upgrade { .. } => unreachable!(),
        Cmd::Install { target, force, .. } | Cmd::Uninstall { target, force, .. } => {
            debug_assert!(target == Target::GitHooks);
            git_hooks::run(&repo, matches!(cli.cmd, Cmd::Install { .. }), force)?
        }
        Cmd::Clear => {
            store.clear()?;
            println!("cleared {}", store.shared.parent().unwrap().display());
        }
    }
    Ok(())
}

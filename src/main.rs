mod extract;
mod facts;
mod git;
mod graph;
mod install;
mod query;
mod snapshot;
mod store;
mod unreal;

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
    },
    /// Delete all cached data for this repository.
    Clear,
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

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

const HOOKS: &[&str] = &["post-commit", "post-checkout", "post-merge", "post-rewrite"];
const HOOK_BEGIN: &str = "# >>> grepum >>>";
const HOOK_END: &str = "# <<< grepum <<<";

fn hooks(repo: &Repo, install: bool) -> Result<()> {
    let dir = repo.hooks_dir()?;
    std::fs::create_dir_all(&dir)?;
    for h in HOOKS {
        let path = dir.join(h);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let stripped = match (existing.find(HOOK_BEGIN), existing.find(HOOK_END)) {
            (Some(a), Some(b)) => format!("{}{}", &existing[..a], existing[b + HOOK_END.len()..].trim_start_matches('\n')),
            _ => existing.clone(),
        };
        let content = if install {
            let base = if stripped.trim().is_empty() { "#!/bin/sh\n".to_string() } else { stripped };
            format!(
                "{base}{}{HOOK_BEGIN}\n# Refresh the code graph in the background; never blocks or fails git.\ncommand -v grepum >/dev/null 2>&1 && (grepum index --quiet >/dev/null 2>&1 &)\n{HOOK_END}\n",
                if base.ends_with('\n') { "" } else { "\n" }
            )
        } else {
            stripped
        };
        if content.trim() == "#!/bin/sh" || content.trim().is_empty() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    println!("{} hooks in {}", if install { "installed" } else { "removed" }, dir.display());
    Ok(())
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
        Cmd::Find { query, limit } => {
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
        }
        Cmd::Callers { ref symbol, depth } | Cmd::Callees { ref symbol, depth } => {
            let reverse = matches!(cli.cmd, Cmd::Callers { .. });
            let s = snapshot(&repo, &store, rev, false)?;
            let g = &s.graph;
            let starts = query::resolve_symbol(g, symbol);
            if starts.is_empty() {
                bail!("no symbol named {symbol:?} (try `grepum find {symbol}`)");
            }
            let idx = query::Index::new(g);
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
                    println!("  (no {})", if reverse { "callers" } else { "callees" });
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
        Cmd::Hooks { action } => hooks(&repo, action == "install")?,
        Cmd::Facts { .. } => unreachable!(),
        Cmd::Install { target, .. } | Cmd::Uninstall { target, .. } => {
            debug_assert!(target == Target::GitHooks);
            hooks(&repo, matches!(cli.cmd, Cmd::Install { .. }))?
        }
        Cmd::Clear => {
            store.clear()?;
            println!("cleared {}", store.shared.parent().unwrap().display());
        }
    }
    Ok(())
}

//! `grepum hooks install|uninstall`: keep the graph fresh after commits,
//! checkouts, merges and rewrites. Only ever appends a marked block to
//! shell-script hooks, never takes over someone else's hook, and refuses a
//! tracked `core.hooksPath` inside the worktree unless forced.

use crate::git::Repo;
use anyhow::{bail, Result};

const HOOKS: &[&str] = &["post-commit", "post-checkout", "post-merge", "post-rewrite"];
const HOOK_BEGIN: &str = "# >>> grepum >>>";
const HOOK_END: &str = "# <<< grepum <<<";

const HOOK_BODY: &str = "# Refresh the code graph in the background; never blocks or fails git.\n\
command -v grepum >/dev/null 2>&1 && (unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE; grepum index --quiet >/dev/null 2>&1 &)\n";

/// Hook text with our marked block removed (unchanged if there is none).
fn strip_hook_block(text: &str) -> String {
    match (text.find(HOOK_BEGIN), text.find(HOOK_END)) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &text[..a], text[b + HOOK_END.len()..].trim_start_matches('\n')),
        _ => text.to_string(),
    }
}

/// The interpreter of a hook that isn't a POSIX-shell script, if so. Git runs
/// shebang-less hooks with sh, so those count as shell.
fn non_shell_interpreter(text: &str) -> Option<String> {
    let first = text.lines().next().unwrap_or("");
    let rest = first.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut prog = words.next()?.rsplit(['/', '\\']).next()?.to_string();
    if prog == "env" {
        prog = words.find(|w| !w.starts_with('-'))?.to_string();
    }
    const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "mksh", "ash", "busybox"];
    (!SHELLS.contains(&prog.as_str())).then_some(prog)
}

/// Why installing into `dir` would change files the team shares, if it would:
/// a hooks directory inside the working tree that git tracks (core.hooksPath).
fn shared_hooks_dir(repo: &Repo, dir: &std::path::Path) -> Option<String> {
    let canon = |p: &std::path::Path| -> std::path::PathBuf {
        // Canonicalize the nearest existing ancestor; the hooks dir may not exist yet.
        let mut base = p.to_path_buf();
        let mut tail = vec![];
        while !base.exists() {
            match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
                (Some(n), Some(parent)) => {
                    tail.push(n);
                    base = parent.to_path_buf();
                }
                _ => break,
            }
        }
        let mut out = base.canonicalize().unwrap_or(base);
        out.extend(tail.into_iter().rev());
        out
    };
    let dir_c = canon(dir);
    if dir_c.starts_with(canon(&repo.common_dir)) || dir_c.starts_with(canon(&repo.git_dir)) {
        return None;
    }
    // Ask about a hook file, not the dir: husky-style setups ignore the files
    // inside (`.husky/_/.gitignore` = `*`) rather than the directory itself.
    if !dir_c.starts_with(canon(&repo.toplevel)) || repo.is_ignored(&dir.join(HOOKS[0])) {
        return None;
    }
    Some(format!(
        "{} is inside the working tree and not ignored (core.hooksPath), so installing would change files \
         everyone who clones the repo gets. Add this line to your shared hooks instead, or rerun with --force:\n  {}",
        dir.display(),
        HOOK_BODY.lines().nth(1).unwrap_or("")
    ))
}

pub fn run(repo: &Repo, install: bool, force: bool) -> Result<()> {
    let dir = repo.hooks_dir()?;
    if install && !force {
        if let Some(why) = shared_hooks_dir(repo, &dir) {
            bail!("not installing hooks: {why}");
        }
    }
    if install {
        std::fs::create_dir_all(&dir)?;
    }
    for h in HOOKS {
        let path = dir.join(h);
        let existing = std::fs::read_to_string(&path).ok();
        let stripped = existing.as_deref().map(strip_hook_block);
        if !install {
            match (&existing, &stripped) {
                (Some(e), Some(s)) if e != s => {
                    if s.trim().is_empty() || s.trim() == "#!/bin/sh" {
                        std::fs::remove_file(&path)?;
                    } else {
                        std::fs::write(&path, s)?; // existing file: permissions are kept
                    }
                }
                _ => {}
            }
            continue;
        }
        let base = stripped.filter(|s| !s.trim().is_empty());
        if let Some(interp) = base.as_deref().and_then(non_shell_interpreter) {
            eprintln!("  skip {h}: it's a {interp} script, so grepum can't add a shell line to it");
            continue;
        }
        let created = base.is_none();
        let base = base.unwrap_or_else(|| "#!/bin/sh\n".to_string());
        let sep = if base.ends_with('\n') { "" } else { "\n" };
        let content = format!("{base}{sep}{HOOK_BEGIN}\n{HOOK_BODY}{HOOK_END}\n");
        if existing.as_deref() == Some(content.as_str()) {
            continue;
        }
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if created {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            } else if std::fs::metadata(&path)?.permissions().mode() & 0o111 == 0 {
                // Leave it as it is: making it executable would switch on the user's own hook too.
                eprintln!("  note: {h} isn't executable, so git doesn't run it (grepum's line included)");
            }
        }
        #[cfg(not(unix))]
        let _ = created;
    }
    println!("{} hooks in {}", if install { "installed" } else { "removed" }, dir.display());
    Ok(())
}

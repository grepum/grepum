//! `grepum install claude`: put the skill, the code-navigator subagent and
//! the session-start hook where Claude Code finds them.
//!
//! - user scope:    $CLAUDE_CONFIG_DIR or ~/.claude   (every project)
//! - project scope: <worktree>/.claude                (commit it to share with the team)
//!
//! Files we write carry a marker line. We only overwrite files that have it
//! (or `--force`), and `settings.json` is merged: only our own hook entry is
//! added, updated or removed, everything else is left byte-for-byte in order.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SKILL: &str = include_str!("../integrations/claude-code/skills/grepum/SKILL.md");
const AGENT: &str = include_str!("../integrations/claude-code/agents/code-navigator.md");
const MARKER: &str = "<!-- installed by grepum; `grepum uninstall claude` removes it -->";
const HOOK_MATCHER: &str = "startup|resume|clear|compact";

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Scope {
    User,
    Project,
}

pub struct Options {
    pub scope: Scope,
    pub force: bool,
    pub dry_run: bool,
}

fn user_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).context("cannot find home directory")?;
    Ok(PathBuf::from(home).join(".claude"))
}

pub fn claude_dir(scope: Scope, cwd: &Path) -> Result<PathBuf> {
    match scope {
        Scope::User => user_dir(),
        Scope::Project => Ok(crate::git::Repo::discover(cwd).context("--scope project must run inside a git worktree")?.toplevel.join(".claude")),
    }
}

/// Hook command. The binary's absolute path makes the hook independent of the
/// PATH Claude Code's shell happens to get; the guard keeps it silent if the
/// binary is later removed.
fn hook_command() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "grepum".into());
    let q = format!("'{}'", exe.replace('\'', r"'\''"));
    format!("[ -x {q} ] && {q} summary 2>/dev/null || true")
}

fn is_our_hook(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains("grepum") && c.contains(" summary"))
}

/// Remove our SessionStart entries. Containers are pruned only when *we*
/// emptied them; anything the user had (even empty) is left alone.
fn strip_hook(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return false };
    let Some(groups) = hooks.get_mut("SessionStart").and_then(|s| s.as_array_mut()) else { return false };
    let mut emptied = vec![false; groups.len()];
    let mut changed = false;
    for (i, g) in groups.iter_mut().enumerate() {
        if let Some(list) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let before = list.len();
            list.retain(|e| !is_our_hook(e));
            if list.len() != before {
                changed = true;
                emptied[i] = list.is_empty();
            }
        }
    }
    if !changed {
        return false;
    }
    let mut i = 0;
    groups.retain(|_| {
        i += 1;
        !emptied[i - 1]
    });
    if groups.is_empty() {
        hooks.remove("SessionStart");
        if hooks.is_empty() {
            settings.as_object_mut().unwrap().remove("hooks");
        }
    }
    true
}

fn read_settings(path: &Path) -> Result<Option<Value>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    if text.trim().is_empty() {
        return Ok(Some(json!({})));
    }
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON; fix it or add the hook by hand (see integrations/claude-code/hooks/hooks.json)", path.display()))?;
    if !v.is_object() {
        bail!("{} is not a JSON object; refusing to modify it", path.display());
    }
    Ok(Some(v))
}

/// User-scope settings get a backup; project-scope settings live in git, which is the backup.
fn write_settings(path: &Path, v: &Value, backup: bool, dry: bool) -> Result<()> {
    if dry {
        return Ok(());
    }
    if backup {
        std::fs::copy(path, path.with_extension("json.grepum.bak"))?;
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.grepum.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(v)? + "\n")?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

enum FileAction {
    Created,
    Updated,
    Unchanged,
    SkippedForeign,
}

fn put_file(path: &Path, body: &str, opts: &Options) -> Result<FileAction> {
    let content = format!("{}\n{MARKER}\n", body.trim_end());
    let action = match std::fs::read_to_string(path) {
        Err(_) => FileAction::Created,
        Ok(old) if old == content => FileAction::Unchanged,
        Ok(old) if old.contains(MARKER) || opts.force => FileAction::Updated,
        Ok(_) => FileAction::SkippedForeign,
    };
    if matches!(action, FileAction::Created | FileAction::Updated) && !opts.dry_run {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, content)?;
    }
    Ok(action)
}

pub fn install_claude(cwd: &Path, opts: &Options) -> Result<()> {
    let dir = claude_dir(opts.scope, cwd)?;
    let pre = if opts.dry_run { "would " } else { "" };
    println!("Claude Code ({} scope): {}", if opts.scope == Scope::User { "user" } else { "project" }, dir.display());

    for (rel, body) in [("skills/grepum/SKILL.md", SKILL), ("agents/code-navigator.md", AGENT)] {
        let path = dir.join(rel);
        match put_file(&path, body, opts)? {
            FileAction::Created => println!("  {pre}create  {rel}"),
            FileAction::Updated => println!("  {pre}update  {rel}"),
            FileAction::Unchanged => println!("  ok      {rel}"),
            FileAction::SkippedForeign => println!("  skip    {rel}  (exists and wasn't written by grepum; --force to replace)"),
        }
    }

    let settings_path = dir.join("settings.json");
    let existing = read_settings(&settings_path)?;
    let existed = existing.is_some();
    let mut settings = existing.unwrap_or_else(|| json!({}));
    let before = settings.clone();
    strip_hook(&mut settings);
    let entry = json!({
        "matcher": HOOK_MATCHER,
        "hooks": [{"type": "command", "command": hook_command(), "timeout": 30}]
    });
    let obj = settings.as_object_mut().unwrap();
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        bail!("{}: \"hooks\" is not an object; refusing to modify it", settings_path.display());
    }
    let ss = hooks.as_object_mut().unwrap().entry("SessionStart").or_insert_with(|| json!([]));
    match ss.as_array_mut() {
        Some(a) => a.push(entry),
        None => bail!("{}: \"hooks.SessionStart\" is not an array; refusing to modify it", settings_path.display()),
    }
    if settings == before {
        println!("  ok      settings.json  (SessionStart hook)");
    } else {
        let backup = existed && opts.scope == Scope::User;
        write_settings(&settings_path, &settings, backup, opts.dry_run)?;
        let backup = if backup && !opts.dry_run { "; previous version saved as settings.json.grepum.bak" } else { "" };
        println!("  {pre}{}  settings.json  (SessionStart hook{backup})", if existed { "update" } else { "create" });
    }

    if opts.scope == Scope::Project {
        println!("\nCommit .claude/ to share this setup with your team.");
    }
    println!("Start a new Claude Code session (or /clear) to pick it up.");
    Ok(())
}

pub fn uninstall_claude(cwd: &Path, opts: &Options) -> Result<()> {
    let dir = claude_dir(opts.scope, cwd)?;
    let pre = if opts.dry_run { "would " } else { "" };
    println!("Claude Code ({} scope): {}", if opts.scope == Scope::User { "user" } else { "project" }, dir.display());
    for rel in ["skills/grepum/SKILL.md", "agents/code-navigator.md"] {
        let path = dir.join(rel);
        match std::fs::read_to_string(&path) {
            Ok(text) if text.contains(MARKER) || opts.force => {
                if !opts.dry_run {
                    std::fs::remove_file(&path)?;
                    if rel.starts_with("skills/") {
                        let _ = std::fs::remove_dir(path.parent().unwrap());
                    }
                }
                println!("  {pre}remove  {rel}");
            }
            Ok(_) => println!("  skip    {rel}  (not written by grepum; --force to remove)"),
            Err(_) => {}
        }
    }
    let settings_path = dir.join("settings.json");
    if let Some(mut settings) = read_settings(&settings_path)? {
        if strip_hook(&mut settings) {
            if opts.scope == Scope::Project && settings == json!({}) {
                if !opts.dry_run {
                    std::fs::remove_file(&settings_path)?;
                }
                println!("  {pre}remove  settings.json  (only held the grepum hook)");
            } else {
                write_settings(&settings_path, &settings, opts.scope == Scope::User, opts.dry_run)?;
                println!("  {pre}update  settings.json  (removed SessionStart hook)");
            }
        }
    }
    if opts.scope == Scope::Project && !opts.dry_run {
        // Tidy directories we created; remove_dir only succeeds when they're empty.
        for d in ["skills", "agents", ""] {
            let _ = std::fs::remove_dir(dir.join(d));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_foreign_hooks_and_is_idempotent() {
        let mut s: Value = serde_json::from_str(
            r#"{"model":"opus","hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"echo hi"}]}],"PreToolUse":[]}}"#,
        )
        .unwrap();
        for _ in 0..2 {
            strip_hook(&mut s);
            let ss = s["hooks"]["SessionStart"].as_array_mut().unwrap();
            ss.push(json!({"matcher": HOOK_MATCHER, "hooks": [{"type": "command", "command": "'/x/grepum' summary"}]}));
        }
        assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert!(strip_hook(&mut s));
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"model":"opus","hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"echo hi"}]}],"PreToolUse":[]}}"#
        );
    }

    #[test]
    fn strip_leaves_foreign_empty_containers() {
        let mut s = json!({"hooks": {"SessionStart": [{"matcher": "x", "hooks": []}]}, "x": 1});
        assert!(!strip_hook(&mut s));
        assert_eq!(s, json!({"hooks": {"SessionStart": [{"matcher": "x", "hooks": []}]}, "x": 1}));
    }

    #[test]
    fn strip_removes_containers_it_emptied() {
        let mut s = json!({"hooks": {"SessionStart": [{"matcher": HOOK_MATCHER, "hooks": [{"type": "command", "command": "grepum summary"}]}]}});
        assert!(strip_hook(&mut s));
        assert_eq!(s, json!({}));
    }
}

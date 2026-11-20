//! Thin layer over git plumbing commands.
//!
//! The MVP talks to git through its stable plumbing interface (`ls-tree`,
//! `cat-file --batch`, `status --porcelain`, `hash-object`). That keeps us
//! correct for every repository layout git supports (linked worktrees,
//! submodules, sha256 repos, filters) without reimplementing any of it.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Repo {
    /// Top of the current worktree.
    pub toplevel: PathBuf,
    /// `.git` dir shared by all worktrees (objects, refs, our shared cache).
    pub common_dir: PathBuf,
    /// Private git dir of this worktree (`.git/worktrees/<name>` for linked ones).
    pub git_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub path: String,
    pub oid: String,
}

/// One commit of `git log --raw`, diffed against its first parent.
#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: String,
    pub time: i64,
    pub author: String,
    pub subject: String,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    /// All-zero when the file was added.
    pub old: String,
    /// All-zero when the file was deleted.
    pub new: String,
}

pub fn is_null_oid(oid: &str) -> bool {
    oid.bytes().all(|b| b == b'0')
}

#[derive(Debug)]
pub enum Change {
    /// Path exists in the worktree with content that differs from HEAD (or is new).
    Present(String),
    /// Path is gone from the worktree.
    Deleted(String),
}

fn git(cwd: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(cwd);
    // Never take index.lock for read-only operations: several agents may be
    // querying the same worktree at once.
    c.env("GIT_OPTIONAL_LOCKS", "0");
    c
}

fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    let out = cmd.output().context("failed to spawn git")?;
    if !out.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn run_str(cmd: &mut Command) -> Result<String> {
    Ok(String::from_utf8(run(cmd)?)?.trim_end().to_string())
}

impl Repo {
    pub fn discover(cwd: &Path) -> Result<Repo> {
        let out = run_str(git(cwd).args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
            "--git-dir",
        ]))
        .context("not inside a git worktree")?;
        let mut lines = out.lines();
        let toplevel = PathBuf::from(lines.next().unwrap_or_default());
        let common_dir = PathBuf::from(lines.next().unwrap_or_default());
        let git_dir = PathBuf::from(lines.next().unwrap_or_default());
        Ok(Repo { toplevel, common_dir, git_dir })
    }

    fn cmd(&self) -> Command {
        git(&self.toplevel)
    }

    /// Resolve `<rev>^{tree}`. Returns None for an unborn HEAD.
    pub fn tree_of(&self, rev: &str) -> Result<Option<String>> {
        let out = self
            .cmd()
            .args(["rev-parse", "--verify", "--quiet", &format!("{rev}^{{tree}}")])
            .output()?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8(out.stdout)?.trim().to_string()))
    }

    /// Every blob in a tree, recursively.
    pub fn ls_tree(&self, tree: &str) -> Result<Vec<TreeEntry>> {
        let out = run(self.cmd().args(["ls-tree", "-r", "-z", "--full-tree", tree]))?;
        let mut entries = Vec::new();
        for rec in out.split(|b| *b == 0) {
            if rec.is_empty() {
                continue;
            }
            // "<mode> SP <type> SP <oid> TAB <path>"
            let rec = std::str::from_utf8(rec)?;
            let (meta, path) = rec.split_once('\t').context("bad ls-tree line")?;
            let mut parts = meta.split(' ');
            let _mode = parts.next();
            let kind = parts.next().unwrap_or("");
            let oid = parts.next().unwrap_or("");
            if kind == "blob" {
                entries.push(TreeEntry { path: path.to_string(), oid: oid.to_string() });
            }
        }
        Ok(entries)
    }

    /// Stream blob contents out of the object database with a single process.
    pub fn read_blobs(&self, oids: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        let mut result = HashMap::with_capacity(oids.len());
        if oids.is_empty() {
            return Ok(result);
        }
        let mut child = self
            .cmd()
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let input: Vec<u8> = oids.iter().flat_map(|o| format!("{o}\n").into_bytes()).collect();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let mut reader = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        let mut header = String::new();
        for _ in oids {
            header.clear();
            reader.read_line(&mut header)?;
            let mut parts = header.trim_end().split(' ');
            let oid = parts.next().unwrap_or("").to_string();
            let kind = parts.next().unwrap_or("");
            if kind == "missing" {
                continue;
            }
            let size: usize = parts.next().unwrap_or("0").parse()?;
            let mut buf = vec![0u8; size + 1]; // content + trailing LF
            reader.read_exact(&mut buf)?;
            buf.truncate(size);
            result.insert(oid, buf);
        }
        writer.join().map_err(|_| anyhow::anyhow!("writer panicked"))??;
        child.wait()?;
        Ok(result)
    }

    /// First-parent history of `rev`, newest first, with the blob ids of every
    /// changed file. Reads only commit and tree objects, never file contents.
    pub fn log_raw(&self, rev: &str, max_commits: Option<usize>) -> Result<Vec<Commit>> {
        let mut cmd = self.cmd();
        cmd.args([
            "log", "--first-parent", "--diff-merges=first-parent", "--no-renames", "--raw", "--no-abbrev", "-z",
            "--format=%x01%H%x00%ct%x00%an%x00%s%x00",
        ]);
        if let Some(n) = max_commits {
            cmd.arg(format!("--max-count={n}"));
        }
        cmd.arg(rev).arg("--");
        let out = run(&mut cmd)?;
        let text = String::from_utf8_lossy(&out);
        let mut commits = vec![];
        for chunk in text.split('\u{1}').filter(|c| !c.is_empty()) {
            let mut fields = chunk.split('\0');
            let (Some(oid), Some(time), Some(author), Some(subject)) = (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let mut files = vec![];
            let mut meta: Option<&str> = None;
            for tok in fields {
                let tok = tok.trim_start_matches('\n');
                if tok.is_empty() {
                    continue;
                }
                match meta.take() {
                    None if tok.starts_with(':') => meta = Some(tok),
                    None => {}
                    Some(m) => {
                        // ":100644 100644 <old> <new> M"
                        let parts: Vec<&str> = m.split(' ').collect();
                        if parts.len() >= 5 {
                            files.push(FileChange { path: tok.to_string(), old: parts[2].to_string(), new: parts[3].to_string() });
                        }
                    }
                }
            }
            commits.push(Commit {
                oid: oid.to_string(),
                time: time.parse().unwrap_or(0),
                author: author.to_string(),
                subject: subject.to_string(),
                files,
            });
        }
        Ok(commits)
    }

    /// Paths whose worktree content differs from HEAD, including untracked,
    /// non-ignored files. Git's index stat cache makes this cheap.
    pub fn changes(&self) -> Result<Vec<Change>> {
        let out = run(self.cmd().args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules",
        ]))?;
        let mut changes = Vec::new();
        for rec in out.split(|b| *b == 0) {
            if rec.len() < 4 {
                continue;
            }
            let rec = std::str::from_utf8(rec)?;
            let (xy, path) = rec.split_at(3);
            let path = path.to_string();
            let xy = xy.as_bytes();
            if xy[0] == b'D' || xy[1] == b'D' {
                changes.push(Change::Deleted(path));
            } else {
                changes.push(Change::Present(path));
            }
        }
        Ok(changes)
    }

    /// Hash worktree files the way `git add` would (filters applied).
    pub fn hash_files(&self, paths: &[String]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(vec![]);
        }
        let mut child = self
            .cmd()
            .args(["hash-object", "--stdin-paths"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let input: Vec<u8> = paths.iter().flat_map(|p| format!("{p}\n").into_bytes()).collect();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out)?;
        writer.join().map_err(|_| anyhow::anyhow!("writer panicked"))??;
        child.wait()?;
        Ok(out.lines().map(|s| s.to_string()).collect())
    }

    pub fn hooks_dir(&self) -> Result<PathBuf> {
        let p = run_str(self.cmd().args(["rev-parse", "--path-format=absolute", "--git-path", "hooks"]))?;
        Ok(PathBuf::from(p))
    }
}

//! On-disk cache, laid out inside git's own directories:
//!
//! ```text
//! <git-common-dir>/grepum/<version>/blobs/ab/<oid>.<lang>   per-file facts (shared by all worktrees)
//! <git-common-dir>/grepum/<version>/trees/<tree-oid>.graph  resolved graph of a commit tree (shared)
//! <git-dir>/grepum/<version>/overlay.graph                 this worktree's uncommitted state (private)
//! ```
//!
//! Every write is tmp-file + rename, so concurrent worktrees can never observe
//! a torn file. Two writers racing on the same key write identical bytes.

use crate::facts::{FileFacts, Lang};
use crate::git::Repo;
use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

/// Bump whenever extraction or resolution output changes meaning.
pub const SCHEMA: u32 = 6;

pub struct Store {
    pub shared: PathBuf,
    pub local: PathBuf,
}

fn version_dir() -> String {
    format!("v{}-s{}", env!("CARGO_PKG_VERSION"), SCHEMA)
}

impl Store {
    pub fn open(repo: &Repo) -> Store {
        Store {
            shared: repo.common_dir.join("grepum").join(version_dir()),
            local: repo.git_dir.join("grepum").join(version_dir()),
        }
    }

    fn blob_path(&self, oid: &str, lang: Lang) -> PathBuf {
        let (a, b) = oid.split_at(2);
        self.shared.join("blobs").join(a).join(format!("{b}.{lang:?}"))
    }

    pub fn tree_path(&self, tree: &str) -> PathBuf {
        self.shared.join("trees").join(format!("{tree}.graph"))
    }

    /// Symbol changes a commit made relative to its first parent (immutable, so shared).
    pub fn commit_path(&self, commit: &str) -> PathBuf {
        let (a, b) = commit.split_at(2);
        self.shared.join("commits").join(a).join(format!("{b}.delta"))
    }

    pub fn overlay_path(&self) -> PathBuf {
        self.local.join("overlay.graph")
    }

    pub fn load_facts(&self, oid: &str, lang: Lang) -> Option<FileFacts> {
        load(&self.blob_path(oid, lang))
    }

    pub fn save_facts(&self, oid: &str, lang: Lang, facts: &FileFacts) -> Result<()> {
        save(&self.blob_path(oid, lang), facts)
    }

    pub fn clear(&self) -> Result<()> {
        for dir in [&self.shared, &self.local] {
            if let Some(parent) = dir.parent() {
                if parent.exists() {
                    std::fs::remove_dir_all(parent)?;
                }
            }
        }
        Ok(())
    }

    pub fn disk_usage(&self) -> (u64, u64) {
        (dir_size(&self.shared), dir_size(&self.local))
    }
}

pub fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    postcard::from_bytes(&bytes).ok()
}

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = postcard::to_stdvec(value)?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".tmp-{}-{:?}-{nonce}", std::process::id(), std::thread::current().id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

//! The linked graph of a snapshot: files, definitions and edges. Built by
//! `resolve` from per-file facts; read by every query, the viewer and history.

mod resolve;

use crate::facts::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GFile {
    pub path: String,
    pub oid: String,
    pub lang: Lang,
    pub lines: u32,
    pub first_def: u32,
    pub def_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GDef {
    pub file: u32,
    pub name: String,
    pub qual: String,
    pub kind: DefKind,
    pub line: u32,
    pub end_line: u32,
    pub parent: Option<u32>,
    pub owner: Option<String>,
    pub decl: bool,
    pub attrs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    Call,
    Inherit,
    /// A type use or member access (`RefKind::Type`/`Field`): one edge per site.
    Ref,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conf {
    Exact,
    Inferred,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Edge {
    pub from_file: u32,
    pub from_def: Option<u32>,
    pub to: u32,
    pub kind: EdgeKind,
    pub conf: Conf,
    pub line: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Stats {
    pub refs: u64,
    pub exact: u64,
    pub inferred: u64,
    pub ambiguous: u64,
    pub unresolved: u64,
    /// Type uses and member accesses seen, and how many linked to a project definition.
    pub uses: u64,
    pub uses_linked: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Graph {
    pub files: Vec<GFile>,
    pub defs: Vec<GDef>,
    pub edges: Vec<Edge>,
    /// importer file -> imported file
    pub imports: Vec<(u32, u32)>,
    pub stats: Stats,
}

impl Graph {
    /// Call and inherit edges, and type-use/member-access edges.
    pub fn edge_counts(&self) -> (usize, usize) {
        let refs = self.edges.iter().filter(|e| e.kind == EdgeKind::Ref).count();
        (self.edges.len() - refs, refs)
    }
}

pub struct Input {
    pub path: String,
    pub oid: String,
    pub facts: Arc<FileFacts>,
}

/// Link a snapshot. `project` holds the non-source files languages read to
/// resolve imports (see `lang::is_project_file`).
pub fn build(inputs: Vec<Input>, project: &[(String, Vec<u8>)]) -> Graph {
    resolve::link(inputs, project)
}

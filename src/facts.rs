//! Per-file facts: everything we learn from a single file in isolation.
//!
//! Facts are a pure function of (file content, language, extractor version),
//! never of the file's path or its neighbours. That is what lets us key them
//! by git blob id and share them across every commit and worktree.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
    Go,
    Cpp,
}

impl Lang {
    /// The language of a source file, by extension (see `lang::of_path`).
    pub fn from_path(path: &str) -> Option<Lang> {
        crate::lang::of_path(path)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DefKind {
    Function,
    Method,
    Class,
    Interface,
    Struct,
    Enum,
    Trait,
    Type,
    Module,
    /// A JS/TS object literal bound to a top-level name (`const api = { get() {} }`):
    /// a namespace of methods, reached as `api.get()`.
    Object,
}

impl DefKind {
    pub fn is_type(self) -> bool {
        matches!(
            self,
            DefKind::Class | DefKind::Interface | DefKind::Struct | DefKind::Enum | DefKind::Trait | DefKind::Type | DefKind::Object
        )
    }
    pub fn is_callable(self) -> bool {
        matches!(self, DefKind::Function | DefKind::Method)
    }
    pub fn label(self) -> &'static str {
        match self {
            DefKind::Function => "fn",
            DefKind::Method => "method",
            DefKind::Class => "class",
            DefKind::Interface => "interface",
            DefKind::Struct => "struct",
            DefKind::Enum => "enum",
            DefKind::Trait => "trait",
            DefKind::Type => "type",
            DefKind::Module => "mod",
            DefKind::Object => "object",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub line: u32,
    pub end_line: u32,
    /// Index of the enclosing def in the same file.
    pub parent: Option<u32>,
    /// Name of the type this method belongs to (class, impl target, Go receiver).
    pub owner: Option<String>,
    /// Declared return type, when cheaply known (used to type `x = make()`).
    pub ret: Option<String>,
    /// Declaration without a body (C++ header prototype, abstract/interface method).
    pub decl: bool,
    /// Annotations that change meaning, e.g. Unreal `UFUNCTION(Server, Reliable)` specifiers.
    pub attrs: Vec<String>,
    /// Fingerprint of the definition's source text, whitespace ignored. Lets
    /// history queries tell "changed" from "unchanged" without the source.
    pub body: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// Function/method call.
    Call,
    /// Constructor call (`new X()`), or anything that targets a type.
    New,
    /// `subject` extends/implements `name`.
    Inherit,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ref {
    pub kind: RefKind,
    pub name: String,
    /// Receiver/path text before the name: `self.repo`, `utils`, `crate::db`.
    /// `?` when the receiver is an expression we don't model (e.g. a call).
    pub qualifier: Option<String>,
    pub line: u32,
    /// Innermost enclosing def.
    pub scope: Option<u32>,
    /// Innermost enclosing type name (for `self`/`this`), or the subclass for Inherit.
    pub owner: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportName {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Import {
    /// Module as written: `app.models`, `.models`, `./models`, `crate::db`, `example.com/x/db`.
    pub module: String,
    /// Specific names imported. Empty means "the module itself".
    pub names: Vec<ImportName>,
    /// Local name bound to the module itself (`import x as y`, `import * as y`, Go package name).
    pub alias: Option<String>,
    /// `from x import *` / `export * from`.
    pub wildcard: bool,
    /// Re-exported to importers of this file (`export ... from`, Python package re-exports).
    pub reexport: bool,
    pub line: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BindingScope {
    /// Module-level variable.
    File,
    /// Local variable / parameter of a def.
    Def(u32),
    /// Field of a type, keyed by type name (survives Rust/Go split impls).
    Owner(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TypeHint {
    Named(String),
    /// Type is whatever `qualifier.name(...)` returns / constructs.
    CallResult { name: String, qualifier: Option<String> },
    /// Same type as another binding in the same scope (`self.repo = repo`).
    Alias(String),
    /// A local name whose type isn't known (an untyped parameter, a destructured
    /// value). Recorded so it shadows same-named functions elsewhere.
    Unknown,
}

/// `export { local as exported }` / `export default local` without a `from`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Export {
    pub local: String,
    pub exported: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Binding {
    pub scope: BindingScope,
    pub name: String,
    pub ty: TypeHint,
    /// Line the name is bound on; 0 for parameters (bound for the whole body).
    /// A local only shadows from here on: `let globs = globs(..)` calls the outer `globs`.
    pub line: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileFacts {
    pub lang: Lang,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub imports: Vec<Import>,
    pub bindings: Vec<Binding>,
    pub exports: Vec<Export>,
    pub lines: u32,
}

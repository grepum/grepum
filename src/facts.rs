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
    pub fn from_path(path: &str) -> Option<Lang> {
        let ext = path.rsplit_once('.')?.1;
        Some(match ext {
            "py" | "pyi" => Lang::Python,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "rs" => Lang::Rust,
            "go" => Lang::Go,
            // C is parsed with the C++ grammar: it accepts nearly all C and keeps one resolver.
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h" | "inl" | "ipp" | "tpp" | "c" => Lang::Cpp,
            _ => return None,
        })
    }

    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        }
    }

    pub fn is_js_family(self) -> bool {
        matches!(self, Lang::TypeScript | Lang::Tsx | Lang::JavaScript)
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
}

impl DefKind {
    pub fn is_type(self) -> bool {
        matches!(
            self,
            DefKind::Class | DefKind::Interface | DefKind::Struct | DefKind::Enum | DefKind::Trait | DefKind::Type
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Binding {
    pub scope: BindingScope,
    pub name: String,
    pub ty: TypeHint,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileFacts {
    pub lang: Lang,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub imports: Vec<Import>,
    pub bindings: Vec<Binding>,
    pub lines: u32,
}

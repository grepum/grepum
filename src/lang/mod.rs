//! Per-language support, behind one interface.
//!
//! The extractor (`extract.rs`) and the resolver (`graph.rs`) know nothing
//! about any particular language. Each language module plugs in:
//!
//! - **syntax** ([`Language::visit`]): which tree-sitter nodes are definitions,
//!   calls, imports and type bindings, recorded as language-neutral facts;
//! - **modules** ([`Modules`]): which files an import names;
//! - **rules** ([`Rules`]): the few places where resolution differs, such as
//!   C++'s implicit `this->`, Go's package scope or JS globals.
//!
//! Adding a language means a module implementing [`Language`], a [`Lang`]
//! variant (it is part of the cached facts) and a line in [`support`].

pub mod cpp;
pub mod go;
pub mod js;
pub mod python;
pub mod rust;

use crate::extract::Walker;
use crate::facts::{Def, Import, Lang};
use crate::graph::GFile;
use tree_sitter::Node;

/// Every supported language, in a fixed order.
pub const ALL: &[&dyn Language] = &[&python::Python, &js::Js, &rust::Rust, &go::Go, &cpp::Cpp];

pub fn support(lang: Lang) -> &'static dyn Language {
    match lang {
        Lang::Python => &python::Python,
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => &js::Js,
        Lang::Rust => &rust::Rust,
        Lang::Go => &go::Go,
        Lang::Cpp => &cpp::Cpp,
    }
}

/// The language of a source file, by extension.
pub fn of_path(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.')?.1;
    ALL.iter().find_map(|l| l.lang_of_extension(ext))
}

/// A non-source file that changes how a language resolves imports
/// (tsconfig.json, package.json, ...). These are fed to [`Language::modules`].
pub fn is_project_file(path: &str) -> bool {
    ALL.iter().any(|l| l.is_project_file(path))
}

pub trait Language: Sync {
    /// Stable name, also the key of this language's module index.
    fn name(&self) -> &'static str;

    /// The `Lang` for a file extension this language owns.
    fn lang_of_extension(&self, ext: &str) -> Option<Lang>;

    fn grammar(&self, lang: Lang) -> tree_sitter::Language;

    /// Rewrite source before parsing (Unreal macros), keeping byte offsets.
    fn preprocess(&self, _raw: &[u8]) -> Option<Preprocessed> {
        None
    }

    /// Record facts for `n` and return true, or return false to have the
    /// walker descend into its children.
    fn visit(&self, w: &mut Walker, n: Node) -> bool;

    /// The type a call produces when the callee spells it out
    /// (C++ `Cast<AFoo>(x)`, `NewObject<UBar>()`).
    fn call_result_type(&self, _w: &Walker, _func: Node) -> Option<String> {
        None
    }

    fn rules(&self) -> &'static Rules;

    fn is_project_file(&self, _path: &str) -> bool {
        false
    }

    /// Module index over the snapshot's files (only this language's are relevant).
    fn modules(&self, files: &[GFile], project: &[(String, Vec<u8>)]) -> Box<dyn Modules>;

    /// Syntax nodes that wrap a definition and belong to it when showing its
    /// source: Python decorators, C++ `template<...>`, JS `export const`.
    fn wrapper_kinds(&self) -> &'static [&'static str] {
        &[]
    }

    /// Sibling nodes directly above a definition that belong to it: doc
    /// comments, attributes, decorators.
    fn leading_kinds(&self) -> &'static [&'static str] {
        &["comment"]
    }

    /// Name and qualified name of the definition a declaration stands for,
    /// when [`Rules::link_declarations`] is set.
    fn definition_key(&self, decl: &crate::graph::GDef) -> (String, String) {
        (decl.name.clone(), decl.qual.clone())
    }
}

/// Source after [`Language::preprocess`].
pub struct Preprocessed {
    pub src: Vec<u8>,
    /// Attributes keyed by the line of the declaration they annotate.
    pub attrs: std::collections::HashMap<u32, Vec<String>>,
    /// Definitions the rewrite removed from the source (macro-declared types).
    pub defs: Vec<Def>,
}

/// What an import, or an imported name, refers to.
#[derive(Clone, Debug)]
pub enum Target {
    /// A module: its key as written and its files.
    Module(String, Vec<u32>),
    /// A symbol exported by these files.
    Symbol(Vec<u32>, String),
    External,
}

/// Resolves module references to files of the snapshot.
pub trait Modules: Sync {
    /// The target of `imp` in file `from`; `name` is the imported symbol, if any.
    fn import(&self, files: &[GFile], from: u32, imp: &Import, name: Option<&str>) -> Target;

    /// Files of a module named by a path as written in code (`a.b`, `crate::db`).
    fn module(&self, _files: &[GFile], _from: u32, _path: &str) -> Option<Vec<u32>> {
        None
    }

    /// `key.seg` as a nested module, for walking `pkg.sub.f()`.
    fn submodule(&self, _key: &str, _seg: &str) -> Option<Vec<u32>> {
        None
    }

    /// Whether a qualifier is a path into the module tree (`crate::db::Pool`).
    fn is_module_path(&self, _qualifier: &str) -> bool {
        false
    }
}

/// Resolution differences between languages, as data.
pub struct Rules {
    /// Generic types that stand for their argument (`Option<T>`, `TObjectPtr<T>`).
    pub wrapper_types: &'static [&'static str],
    /// Method names built-in containers, strings and streams also have: a call
    /// to one on a receiver of unknown type is far more likely to hit the
    /// builtin than a uniquely-named project method, so it is never guessed.
    pub builtin_methods: &'static [&'static [&'static str]],
    /// Names that never refer to project symbols as a receiver (JS `console`).
    pub globals: &'static [&'static str],
    /// Extra spellings of "the enclosing class" besides `self`/`this`/`Self`.
    pub self_names: &'static [&'static str],
    /// Methods are written inside their class, so they live in its file.
    pub methods_in_class_file: bool,
    /// A bare name in a method may be a member of its class (C++ implicit `this->`).
    pub implicit_this: bool,
    /// Top-level names of every file in the same directory are visible (Go packages).
    pub package_scope: bool,
    /// `Scope::f()` with an unresolved scope may name a static or namespace function.
    pub static_scope_calls: bool,
    /// A type alias can be called or constructed (C++), rather than being a type only (TS).
    pub type_aliases_callable: bool,
    /// Type aliases are followed to the type they name (`using Core = JobCore`).
    pub follow_type_aliases: bool,
    /// A call that resolved to a declaration links to its definition instead.
    pub link_declarations: bool,
    /// `import a.b` binds `a`, the root package.
    pub import_binds_root: bool,
}

impl Rules {
    pub fn is_builtin_method(&self, name: &str) -> bool {
        self.builtin_methods.iter().any(|list| list.contains(&name))
    }

    pub const DEFAULT: Rules = Rules {
        wrapper_types: &[],
        builtin_methods: &[COMMON_BUILTIN_METHODS],
        globals: &[],
        self_names: &[],
        methods_in_class_file: false,
        implicit_this: false,
        package_scope: false,
        static_scope_calls: false,
        type_aliases_callable: true,
        follow_type_aliases: false,
        link_declarations: false,
        import_binds_root: false,
    };
}

/// Method names standard collections, strings, promises and streams have in
/// most languages.
pub const COMMON_BUILTIN_METHODS: &[&str] = &[
    "get", "set", "add", "has", "delete", "remove", "clear", "pop", "push", "append", "extend", "insert", "update",
    "items", "keys", "values", "entries", "copy", "sort", "reverse", "index", "count", "find", "filter", "map",
    "reduce", "forEach", "some", "every", "includes", "join", "split", "slice", "splice", "concat", "strip",
    "lower", "upper", "replace", "format", "startswith", "endswith", "encode", "decode", "read", "write",
    "close", "open", "send", "emit", "on", "then", "catch", "finally", "json", "toString", "next", "len",
    "unwrap", "expect", "clone", "iter", "into", "as_str", "to_string", "is_empty", "Error", "String",
];

/// Repo-relative path helpers shared by module resolvers.
pub fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

pub fn strip_ext(path: &str) -> &str {
    match path.rsplit_once('.') {
        Some((p, _)) if !p.ends_with('/') => p,
        _ => path,
    }
}

pub fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

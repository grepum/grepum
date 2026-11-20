//! `grepum refs`: type uses and member accesses beside calls, per language,
//! and the cases that must not link (generic parameters, unknown receivers,
//! same-named types elsewhere).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn setup(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-refs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "src/lib.rs", "\
pub struct Binding {
    pub name: String,
    pub line: u32,
}

impl Binding {
    pub fn shift(&mut self, by: u32) -> u32 {
        self.line += by;
        self.line
    }
}

pub fn make(name: String) -> Binding {
    let b = Binding { name, line: 0 };
    let Binding { line, .. } = &b;
    let _ = line;
    b
}

pub fn generic<Binding: Clone>(x: Binding) -> Binding {
    x
}

pub fn unknown(x: &dyn std::any::Any) {
    let _ = x.line;
}
");
    write(&root, "app/models.py", "\
class Repo:
    limit = 10

    def __init__(self, url: str):
        self.url = url

    def describe(self) -> str:
        return self.url

    def handlers(self):
        return [self.describe]


def use(repo: Repo, other) -> \"Repo\":
    print(repo.url, other.url, Repo.limit)
    return repo
");
    write(&root, "web/store.ts", "\
export interface Item { id: string }

export class Store {
  items: Item[] = []
  constructor(private readonly prefix: string) {}
  add(item: Item): void {
    this.items.push(item)
    console.log(this.prefix + item.id)
  }
}

export function first<Item>(xs: Item[]): Item { return xs[0] }

it('a', () => {
  type Expected = { a: 1 }
  const x: Expected = { a: 1 }
})
it('b', () => {
  type Expected = { b: 2 }
  const y: Expected = { b: 2 }
})
");
    write(&root, "pkg/conn.go", "\
package pkg

type Conn struct {
\tid   int
\tname string
}

func NewConn() *Conn {
\treturn &Conn{id: 1, name: \"x\"}
}

func (c *Conn) ID() int {
\treturn c.id
}
");
    write(&root, "core/pool.h", "\
class Pool {
public:
    void Drain(int n);
    int size_;
};
");
    write(&root, "core/pool.cpp", "\
#include \"pool.h\"
void Pool::Drain(int n) {
    int size_copy = size_;
    size_ = n;
    this->size_ += size_copy;
}
");
    write(&root, "other/pool2.cpp", "\
class Pool {
public:
    int size_;
    void Grow() { size_ += 1; }
};
");
    write(&root, "Source/Game/Hero.h", "\
#pragma once
UCLASS()
class GAME_API AHero : public AActor {
    GENERATED_BODY()
public:
    UPROPERTY(EditAnywhere, Category = \"Combat\")
    float Health = 100.f;
    void Hit(float Damage);
};
");
    write(&root, "Source/Game/Hero.cpp", "\
#include \"Hero.h\"
void AHero::Hit(float Damage) {
    Health -= Damage;
}
");
    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "i"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
    root
}

fn grepum(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_grepum")).args(args).current_dir(root).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// `path:line kind from` of every reference to `sym`.
fn refs(root: &Path, sym: &str) -> BTreeSet<String> {
    let v: serde_json::Value = serde_json::from_str(&grepum(root, &["--json", "refs", sym])).unwrap();
    v["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{}:{} {} {}", r["file"].as_str().unwrap(), r["line"], r["kind"].as_str().unwrap(), r["from"].as_str().unwrap_or("-")))
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rust_fields_and_types() {
    let root = setup("rust");
    // Reads, writes, struct literal fields and patterns; not `x.line` on an unknown receiver.
    assert_eq!(
        refs(&root, "Binding.line"),
        set(&["src/lib.rs:8 field Binding.shift", "src/lib.rs:9 field Binding.shift", "src/lib.rs:14 field make", "src/lib.rs:15 field make"])
    );
    // Type uses, with the literal counted as a construction; the generic parameter
    // named `Binding` is not the struct.
    assert_eq!(
        refs(&root, "Binding"),
        set(&["src/lib.rs:6 type -", "src/lib.rs:13 type make", "src/lib.rs:14 new make", "src/lib.rs:15 type make"])
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn python_typescript_go_members() {
    let root = setup("dynamic");
    // `self.url` and a typed parameter's `repo.url`; `other.url` is unknown and not guessed.
    assert_eq!(
        refs(&root, "Repo.url"),
        set(&["app/models.py:5 field Repo.__init__", "app/models.py:8 field Repo.describe", "app/models.py:15 field use"])
    );
    assert_eq!(refs(&root, "Repo.limit"), set(&["app/models.py:15 field use"]));
    // A method passed as a value, and annotations including a forward reference.
    assert_eq!(refs(&root, "Repo.describe"), set(&["app/models.py:11 value Repo.handlers"]));
    assert_eq!(refs(&root, "app/models.py:Repo"), set(&["app/models.py:14 type use"]));

    // TS: fields, constructor parameter properties, interface members; `<Item>` is a generic parameter.
    assert_eq!(refs(&root, "Store.items"), set(&["web/store.ts:7 field Store.add"]));
    assert_eq!(refs(&root, "Store.prefix"), set(&["web/store.ts:8 field Store.add"]));
    assert_eq!(refs(&root, "Item.id"), set(&["web/store.ts:8 field Store.add"]));
    assert_eq!(refs(&root, "Item"), set(&["web/store.ts:4 type Store", "web/store.ts:6 type Store.add"]));
    // Block-scoped types of the same name: each use links to the nearest one above.
    let expected = grepum(&root, &["--json", "refs", "web/store.ts:Expected"]);
    let v: serde_json::Value = serde_json::from_str(&expected).unwrap();
    let pairs: BTreeSet<(u64, String)> = v["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["line"].as_u64().unwrap(), r["symbol"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(pairs.len(), 2, "one link per use, not one per same-named type: {pairs:?}");

    // Go: composite literal keys and selectors; the receiver type isn't a use.
    assert_eq!(refs(&root, "Conn.id"), set(&["pkg/conn.go:9 field NewConn", "pkg/conn.go:13 field Conn.ID"]));
    assert_eq!(refs(&root, "Conn"), set(&["pkg/conn.go:8 type NewConn", "pkg/conn.go:9 type NewConn"]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cpp_members_through_implicit_this() {
    let root = setup("cpp");
    // Bare `size_` (implicit this->) and `this->size_` in the out-of-line method;
    // the other file's same-named class keeps its own field.
    assert_eq!(
        refs(&root, "core/pool.h:Pool.size_"),
        set(&["core/pool.cpp:3 field Pool.Drain", "core/pool.cpp:4 field Pool.Drain", "core/pool.cpp:5 field Pool.Drain"])
    );
    assert_eq!(refs(&root, "other/pool2.cpp:Pool.size_"), set(&["other/pool2.cpp:4 field Pool.Grow"]));
    // Unreal: a UPROPERTY is a field, with its specifiers.
    assert_eq!(refs(&root, "AHero.Health"), set(&["Source/Game/Hero.cpp:3 field AHero.Hit"]));
    let outline = grepum(&root, &["--json", "outline", "AHero"]);
    assert!(outline.contains("\"symbol\": \"AHero.Health\""), "{outline}");
    let export = grepum(&root, &["export"]);
    let g: serde_json::Value = serde_json::from_str(&export).unwrap();
    let health = g["defs"].as_array().unwrap().iter().find(|d| d["qual"] == "AHero.Health").unwrap();
    assert_eq!(health["kind"], "field");
    assert!(health["attrs"].as_array().unwrap().iter().any(|a| a == "EditAnywhere"), "{health}");

    // --kind filters; text output pipes into `show -`.
    let only = grepum(&root, &["refs", "-k", "call", "core/pool.h:Pool.size_"]);
    assert!(only.contains("(no references found)"), "{only}");
    let _ = std::fs::remove_dir_all(&root);
}

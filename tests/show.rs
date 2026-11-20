//! `grepum show`: a definition's source as far as its syntax reaches, with
//! the comments, attributes and decorators that belong to it, per language.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@e"];
    all.extend_from_slice(args);
    assert!(Command::new("git").args(&all).current_dir(root).status().unwrap().success());
}

fn setup(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-show-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "app/svc.py", "\
import functools

x = 1  # trailing comment, not part of what follows

# Loads a user.
@functools.cache
def load(uid):
    return uid

def other():
    return load(1)
");
    write(&root, "src/lib.rs", "\
struct Repo;

impl Repo {
    /// Finds a row.
    #[inline]
    pub fn find(&self, id: u32) -> u32 {
        id
    }

    pub fn after(&self) {}
}
");
    write(&root, "web/api.ts", "\
/**
 * Fetches a user.
 */
export const fetchUser = async (id: string) => {
  return id
}

export class Store {
  // Saves it.
  save(x: number) {
    return x
  }
}
");
    write(&root, "core/pool.hpp", "\
#pragma once
// Picks the larger.
template <typename T>
T pick(T a, T b) {
    return a > b ? a : b;
}
");
    write(&root, "Source/Game/Hero.h", "\
#pragma once
UCLASS()
class GAME_API AHero : public AActor {
    GENERATED_BODY()
public:
    UFUNCTION(BlueprintCallable, Category = \"Combat\")
    void Attack(float Power);
};
");
    write(&root, "Source/Game/Hero.cpp", "\
#include \"Hero.h\"

void AHero::Attack(float Power) {
    Power += 1.0f;
}
");
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "i"]);
    root
}

fn grepum(root: &Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grepum"))
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn show(root: &Path, sym: &str) -> String {
    grepum(root, &["show", "-N", sym], None)
}

#[test]
fn shows_definitions_with_what_belongs_to_them() {
    let root = setup("syntax");

    let py = show(&root, "load");
    assert!(py.starts_with("fn load  app/svc.py:5-8\n# Loads a user.\n@functools.cache\ndef load(uid):\n    return uid\n"), "{py}");
    assert!(!py.contains("trailing comment") && !py.contains("def other"), "{py}");

    let rs = show(&root, "Repo.find");
    assert!(rs.contains("/// Finds a row.\n    #[inline]\n    pub fn find") && rs.trim_end().ends_with("    }"), "{rs}");
    assert!(!rs.contains("after") && !rs.contains("impl Repo"), "only the method, not its impl:\n{rs}");

    let ts = show(&root, "fetchUser");
    assert!(ts.contains("/**\n * Fetches a user.\n */\nexport const fetchUser") && ts.trim_end().ends_with('}'), "{ts}");
    let method = show(&root, "Store.save");
    assert!(method.contains("  // Saves it.\n  save(x: number) {") && !method.contains("class Store"), "{method}");

    let tpl = show(&root, "pick");
    assert!(tpl.contains("// Picks the larger.\ntemplate <typename T>\nT pick(T a, T b) {"), "{tpl}");

    // A header prototype shows its out-of-line definition.
    let ue = show(&root, "AHero.Attack");
    assert!(ue.starts_with("method AHero.Attack  Source/Game/Hero.cpp:3-5  (declared at Source/Game/Hero.h:7)"), "{ue}");
    // ...and pointing at the header line shows the prototype with its Unreal macro.
    let decl = grepum(&root, &["show", "-N", "Source/Game/Hero.h:7"], None);
    assert!(decl.contains("UFUNCTION(BlueprintCallable, Category = \"Combat\")\n    void Attack(float Power);"), "{decl}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reads_targets_from_pipes_and_revisions() {
    let root = setup("pipes");

    // grepum's own output, rg-style `path:line:text`, and bare names.
    let piped = grepum(&root, &["show", "-N", "-"], Some("fn other  app/svc.py:10\nweb/api.ts:5:  return id\nRepo.find\n(no callers found)\n"));
    assert!(piped.contains("fn other  app/svc.py:10-11"), "{piped}");
    assert!(piped.contains("fn fetchUser  web/api.ts:1-6"), "the function around the matched line:\n{piped}");
    assert!(piped.contains("method Repo.find"), "{piped}");

    // --rev reads the committed text, not the edited worktree.
    write(&root, "app/svc.py", "# Loads a user.\n@functools.cache\ndef load(uid):\n    return uid + 1\n");
    assert!(show(&root, "load").contains("uid + 1"));
    let old = grepum(&root, &["--rev", "HEAD", "show", "-N", "load"], None);
    assert!(old.contains("    return uid\n") && !old.contains("uid + 1"), "{old}");

    // --callees adds what it calls.
    git(&root, &["checkout", "-q", "--", "."]);
    let with = grepum(&root, &["show", "-N", "--callees", "other"], None);
    assert!(with.contains("fn load  app/svc.py:5-8  (called by other)"), "{with}");

    let _ = std::fs::remove_dir_all(&root);
}

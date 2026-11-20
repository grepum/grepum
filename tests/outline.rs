//! `grepum outline`: one line per definition (range, signature, callers), per language.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn setup(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-outline-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "app/svc.py", "\
class Service:
    def load(
        self,
        uid: int,
    ) -> dict:
        def helper():
            return uid
        return helper()

def run():
    return Service().load(1)
");
    write(&root, "src/repo.rs", "\
pub struct Repo;

impl Repo {
    /// Finds a row.
    pub fn find(&self, id: u32) -> u32 {
        id
    }
}

pub fn main() {
    Repo.find(1);
}
");
    write(&root, "src/repo_extra.rs", "\
use crate::repo::Repo;

impl std::fmt::Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}
");
    write(&root, "web/api.ts", "\
export const fetchUser = async (id: string): Promise<string> => {
  return id
}

export class Store<
  T extends object,
> {
  save(x: T) {
    return x
  }
}
");
    write(&root, "core/pool.h", "\
namespace core {
class Pool {
public:
    void Drain(int n);
};
}
");
    write(&root, "core/pool.cpp", "\
#include \"pool.h\"
namespace core {
void Pool::Drain(int n) {
    n += 1;
}
}
");
    write(&root, "pkg/conn.go", "\
package pkg

type Conn struct {
\tid int
}

func (c *Conn) Close() error {
\treturn nil
}
");
    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "i"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
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

fn outline(root: &Path, args: &[&str]) -> String {
    let mut all = vec!["outline"];
    all.extend_from_slice(args);
    grepum(root, &all, None)
}

#[test]
fn outlines_files_with_signatures_not_bodies() {
    let root = setup("files");

    let py = outline(&root, &["app/svc.py"]);
    assert!(py.starts_with("app/svc.py  (11 lines)\n  1-8    class Service\n  2-8      def load(self, uid: int) -> dict  (1 caller)\n"), "{py}");
    assert!(!py.contains("helper") && !py.contains("return"), "no function-local defs, no bodies:\n{py}");

    // Methods in an `impl` sit under its header.
    let rs = outline(&root, &["src/repo.rs"]);
    assert!(rs.contains("  3-8    impl Repo\n  5-7      pub fn find(&self, id: u32) -> u32  (1 caller)\n"), "{rs}");

    let ts = outline(&root, &["web/api.ts"]);
    assert!(ts.contains("export const fetchUser = async (id: string): Promise<string> =>"), "{ts}");
    assert!(ts.contains("export class Store<T extends object>\n") && ts.contains("    save(x: T)\n"), "{ts}");

    let go = outline(&root, &["pkg"]);
    assert!(go.contains("type Conn struct\n") && go.contains("func (c *Conn) Close() error"), "{go}");

    // -d 1: top level only.
    let top = outline(&root, &["-d", "1", "app/svc.py"]);
    assert!(top.contains("class Service") && !top.contains("def load") && top.contains("def run()"), "{top}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn outlines_a_type_wherever_its_members_are_written() {
    let root = setup("types");

    // Rust: the struct and both impl blocks, across files.
    let rs = outline(&root, &["Repo"]);
    assert!(rs.contains("pub struct Repo") && rs.contains("impl Repo") && rs.contains("pub fn find"), "{rs}");
    assert!(rs.contains("src/repo_extra.rs") && rs.contains("impl std::fmt::Display for Repo") && rs.contains("fn fmt("), "{rs}");
    assert!(!rs.contains("pub fn main"), "only the type's members:\n{rs}");

    // C++: the class in the header and its out-of-line method in the .cpp.
    let cpp = outline(&root, &["Pool"]);
    assert!(cpp.contains("core/pool.h") && cpp.contains("void Drain(int n)"), "{cpp}");
    assert!(cpp.contains("core/pool.cpp") && cpp.contains("namespace core\n") && cpp.contains("void Pool::Drain(int n)"), "{cpp}");

    // Targets from `rg -n` output name files.
    let piped = grepum(&root, &["outline", "-"], Some("src/repo.rs:5:    pub fn find\napp/svc.py:10:def run():\n"));
    assert!(piped.contains("src/repo.rs  (12 lines)") && piped.contains("app/svc.py  (11 lines)"), "{piped}");

    let json = grepum(&root, &["--json", "outline", "Service"], None);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let defs = v[0]["definitions"].as_array().unwrap();
    assert_eq!(defs[1]["symbol"], "Service.load");
    assert_eq!(defs[1]["signature"], "def load(self, uid: int) -> dict");

    let _ = std::fs::remove_dir_all(&root);
}

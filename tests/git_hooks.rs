//! `grepum hooks install/uninstall` must never break or dirty anything that
//! isn't ours: other people's hooks, non-shell hooks, or tracked hook dirs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn grepum(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grepum")).args(args).current_dir(dir).output().unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grepum-hooks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("a.py"), "def f():\n    pass\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    dir
}

#[test]
fn fresh_install_and_uninstall_leave_nothing() {
    let r = repo("fresh");
    assert!(grepum(&r, &["hooks", "install"]).status.success());
    let hook = std::fs::read_to_string(r.join(".git/hooks/post-commit")).unwrap();
    assert!(hook.starts_with("#!/bin/sh\n") && hook.contains("grepum index --quiet"));
    assert!(hook.contains("unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE"));
    assert!(grepum(&r, &["hooks", "uninstall"]).status.success());
    assert!(!r.join(".git/hooks/post-commit").exists());
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn existing_shell_hook_is_restored_exactly() {
    let r = repo("shell");
    let p = r.join(".git/hooks/post-merge");
    let mine = "#!/usr/bin/env bash\nset -e\necho mine\n";
    std::fs::write(&p, mine).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(grepum(&r, &["hooks", "install"]).status.success());
    let with = std::fs::read_to_string(&p).unwrap();
    assert!(with.starts_with(mine) && with.contains("grepum index"));
    assert!(grepum(&r, &["hooks", "install"]).status.success(), "idempotent");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), with);
    assert!(grepum(&r, &["hooks", "uninstall"]).status.success());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700, "mode kept");
    }
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn non_shell_hook_is_left_alone() {
    let r = repo("python");
    let p = r.join(".git/hooks/post-commit");
    let mine = "#!/usr/bin/env python3\nprint('my python hook')\n";
    std::fs::write(&p, mine).unwrap();
    let out = grepum(&r, &["hooks", "install"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("python3 script"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
    assert!(r.join(".git/hooks/post-merge").exists(), "other hooks still installed");
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn tracked_hooks_dir_is_refused_unless_forced() {
    let r = repo("tracked");
    std::fs::create_dir_all(r.join(".githooks")).unwrap();
    std::fs::write(r.join(".githooks/post-merge"), "#!/bin/sh\necho team\n").unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "team hooks"]);
    git(&r, &["config", "core.hooksPath", ".githooks"]);
    let out = grepum(&r, &["hooks", "install"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--force"));
    assert_eq!(git(&r, &["status", "--porcelain"]), "", "worktree untouched");
    assert!(grepum(&r, &["hooks", "install", "--force"]).status.success());
    assert!(std::fs::read_to_string(r.join(".githooks/post-merge")).unwrap().contains("grepum index"));
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn ignored_hooks_dir_in_worktree_is_fine() {
    let r = repo("husky");
    std::fs::create_dir_all(r.join(".husky/_")).unwrap();
    std::fs::write(r.join(".husky/_/.gitignore"), "*\n").unwrap();
    git(&r, &["config", "core.hooksPath", ".husky/_"]);
    assert!(grepum(&r, &["hooks", "install"]).status.success());
    assert!(r.join(".husky/_/post-commit").exists());
    assert_eq!(git(&r, &["status", "--porcelain"]), "");
    let _ = std::fs::remove_dir_all(&r);
}

//! The SessionStart command `grepum install claude` writes must actually run
//! in bash (Git Bash on Windows) and print the summary.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bash() -> PathBuf {
    if cfg!(windows) {
        // Not System32\bash.exe, which is WSL.
        for c in [r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files\Git\usr\bin\bash.exe"] {
            if Path::new(c).exists() {
                return c.into();
            }
        }
    }
    "bash".into()
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn hook_in(settings: &Path) -> String {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(settings).unwrap()).unwrap();
    v["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["hooks"].as_array().unwrap().iter())
        .map(|h| h["command"].as_str().unwrap().to_string())
        .find(|c| c.contains("grepum"))
        .expect("no grepum hook written")
}

#[test]
fn generated_session_hooks_run_in_bash() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_grepum"));
    let root = std::env::temp_dir().join(format!("grepum-hook-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    let config = root.join("claude-config");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("app.py"), "def main():\n    helper()\n\ndef helper():\n    pass\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);

    let mut path = vec![exe.parent().unwrap().to_path_buf()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    let path = std::env::join_paths(path).unwrap();

    for (scope, settings) in [("user", config.join("settings.json")), ("project", repo.join(".claude").join("settings.json"))] {
        let ok = Command::new(&exe)
            .args(["install", "claude", "--scope", scope])
            .current_dir(&repo)
            .env("CLAUDE_CONFIG_DIR", &config)
            .output()
            .unwrap();
        assert!(ok.status.success(), "install {scope}: {}", String::from_utf8_lossy(&ok.stderr));
        let cmd = hook_in(&settings);
        assert!(!cmd.contains(r"\\?\"), "{scope} hook has a verbatim Windows path: {cmd}");
        if scope == "project" {
            assert!(!cmd.contains('/') || cmd.starts_with("command -v"), "project hook pins a path: {cmd}");
        }
        let out = Command::new(bash()).args(["-c", &cmd]).current_dir(&repo).env("PATH", &path).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("grepum code graph"), "{scope} hook printed nothing.\ncommand: {cmd}\nstdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    }
    let _ = std::fs::remove_dir_all(&root);
}

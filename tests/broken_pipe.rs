//! Piping large output into a reader that closes early (e.g. `| head`) must
//! exit cleanly without panicking on `BrokenPipe`.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn setup() -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-pipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // ~4000 functions produces >200 KiB of outline/json/export output,
    // exceeding OS pipe buffers (typically 16-64 KiB) so closing stdout early
    // triggers BrokenPipe on Unix and Windows alike.
    let mut big = String::with_capacity(4000 * 50);
    for i in 1..=4000 {
        big.push_str(&format!("export function fn{i}(): number {{ return {i}; }}\n"));
    }
    std::fs::write(root.join("big.ts"), big).unwrap();
    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "init"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
    root
}

fn assert_clean_early_close(root: &Path, args: &[&str], lines_to_read: usize) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_grepum"))
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    for _ in 0..lines_to_read {
        let n = stdout.read_line(&mut first).unwrap();
        assert!(n > 0, "{args:?}: expected output before closing pipe");
    }
    // Close the reading end while grepum is still writing.
    drop(stdout);

    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{args:?}: exited with {status}, stderr:\n{stderr}");
    assert!(stderr.is_empty(), "{args:?}: unexpected stderr:\n{stderr}");
}

#[test]
fn exits_cleanly_when_stdout_pipe_closes_early() {
    let root = setup();
    // Warm the index so the piped commands only write output.
    let idx = Command::new(env!("CARGO_BIN_EXE_grepum")).arg("index").current_dir(&root).output().unwrap();
    assert!(idx.status.success(), "{}", String::from_utf8_lossy(&idx.stderr));

    assert_clean_early_close(&root, &["outline", "big.ts"], 2);
    assert_clean_early_close(&root, &["--json", "outline", "big.ts"], 2);
    assert_clean_early_close(&root, &["export"], 1);

    let _ = std::fs::remove_dir_all(&root);
}

//! `grepum upgrade`: replace this binary with the latest GitHub release by re-running
//! the release installer into the directory the binary already lives in.
//! Needs `curl` (and PowerShell on Windows); no HTTP client is linked into grepum.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "grepum/grepum";

pub struct Options {
    pub check: bool,
    pub force: bool,
}

pub fn run(out: &mut impl Write, opts: &Options) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_version().context("could not look up the latest grepum release")?;
    let newer = is_newer(&latest, current);
    if opts.check {
        if newer {
            writeln!(out, "grepum {current} is installed; {latest} is available. Run `grepum upgrade`.")?;
        } else {
            writeln!(out, "grepum {current} is up to date.")?;
        }
        return Ok(());
    }
    if !newer && !opts.force {
        writeln!(out, "grepum {current} is up to date.")?;
        return Ok(());
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    let dir = exe.parent().context("binary has no parent directory")?.to_path_buf();
    if let Some(hint) = managed_elsewhere(&dir) {
        bail!("this grepum ({}) is managed by another tool; upgrade it with: {hint}", exe.display());
    }

    writeln!(out, "upgrading grepum {current} -> {latest} in {}", dir.display())?;
    out.flush()?;
    install_into(&dir, &exe)?;
    let cmd_out = Command::new(&exe).arg("--version").output()?;
    write!(out, "{}", String::from_utf8_lossy(&cmd_out.stdout))?;
    Ok(())
}

/// Resolve `releases/latest` through its redirect (no API token or rate limit involved).
fn latest_version() -> Result<String> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args(["-fsSLI", "-o", if cfg!(windows) { "NUL" } else { "/dev/null" }, "-w", "%{url_effective}", &url])
        .output()
        .context("failed to run curl")?;
    if !out.status.success() {
        bail!("curl {url} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let effective = String::from_utf8_lossy(&out.stdout);
    match effective.rsplit_once("/tag/") {
        Some((_, tag)) => Ok(tag.trim().trim_start_matches('v').to_string()),
        None => bail!("no published release found at {url}"),
    }
}

fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Vec<u64> {
        let core = v.split(['-', '+']).next().unwrap_or(v);
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    parse(candidate) > parse(current)
}

/// Installs made by cargo, Homebrew or a dev build must be upgraded by that tool.
fn managed_elsewhere(dir: &Path) -> Option<&'static str> {
    let s = dir.to_string_lossy().replace('\\', "/");
    if s.contains("/Cellar/") || s.contains("/homebrew/") || s.contains("/linuxbrew/") {
        return Some("brew upgrade grepum");
    }
    if s.contains("/target/release") || s.contains("/target/debug") {
        return Some("git pull && cargo build --release");
    }
    let cargo_bin = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".cargo")))
        .map(|c| c.join("bin"));
    if cargo_bin.and_then(|c| c.canonicalize().ok()).as_deref() == Some(dir) {
        return Some("git pull && ./install.sh --from-source");
    }
    None
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn installer_cmd(dir: &Path) -> Command {
    let base = format!("https://github.com/{REPO}/releases/latest/download");
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(format!("irm {base}/grepum-installer.ps1 | iex"));
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", "curl --proto '=https' --tlsv1.2 -LsSf \"$1\" | sh", "sh"])
            .arg(format!("{base}/grepum-installer.sh"));
        c
    };
    // Flat install into the current directory; the PATH is already set up.
    cmd.env("GREPUM_INSTALL_DIR", dir).env("GREPUM_NO_MODIFY_PATH", "1");
    cmd
}

fn install_into(dir: &Path, exe: &Path) -> Result<()> {
    // Windows cannot overwrite a running executable, but it can rename it.
    let parked = exe.with_extension("old.exe");
    if cfg!(windows) {
        let _ = std::fs::remove_file(&parked);
        std::fs::rename(exe, &parked).context("could not move the running binary aside")?;
    }
    let status = installer_cmd(dir).status().context("failed to start the installer")?;
    if !status.success() {
        if cfg!(windows) && !exe.exists() {
            let _ = std::fs::rename(&parked, exe);
        }
        bail!("the installer failed ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }
}

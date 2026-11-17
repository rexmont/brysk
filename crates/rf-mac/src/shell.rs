//! Opening files and folders with the system's default application (or a named program), the
//! macOS half of `rf-win::shell`.

use std::path::Path;

use anyhow::{Context, Result};

/// Opens `path` with its default application (or in the Finder for folders).
pub fn open(path: &Path) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open").arg(path).status().context("open başlatılamadı")?;
    anyhow::ensure!(status.success(), "open {status}");
    Ok(())
}

/// "Open with…": the Finder's application chooser is Apple Events work; until then this opens
/// the file with the default application.
pub fn open_with(path: &Path) -> Result<()> {
    open(path)
}

/// Opens `path` with the application at `app` (a `.app` bundle).
pub fn open_with_app(path: &Path, app: &Path) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open").arg("-a").arg(app).arg(path).status().context("open başlatılamadı")?;
    anyhow::ensure!(status.success(), "open {status}");
    Ok(())
}

/// Runs `program` with `args`, in `cwd` when given, detached from this process.
pub fn run_program(program: &Path, args: &str, cwd: Option<&Path>) -> Result<()> {
    let mut cmd = std::process::Command::new(program);
    if !args.is_empty() {
        cmd.args(args.split_whitespace());
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.spawn().with_context(|| format!("{} başlatılamadı", program.display()))?;
    Ok(())
}

/// Windows-only: stops the OS from putting up a "this program stopped working" dialog.
pub fn suppress_critical_error_dialogs() {}

/// Shows `path` in the Finder with the item selected (`reveal in Finder`).
pub fn reveal(path: &Path) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).status().context("open başlatılamadı")?;
    anyhow::ensure!(status.success(), "open {status}");
    Ok(())
}

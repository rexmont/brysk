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

/// "Open with…": choose an application (`/Applications`) and open the file with it.
pub fn open_with(path: &Path) -> Result<()> {
    let Some(app) = crate::dialogs::pick_application("Birlikte aç") else { return Ok(()) };
    let status = std::process::Command::new("/usr/bin/open").arg("-a").arg(&app).arg(path).status().context("open başlatılamadı")?;
    anyhow::ensure!(status.success(), "open {status}");
    Ok(())
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

/// macOS has no COM apartments; the callers only use this to mark shell work.
pub fn ensure_com() {}

/// `shell:` names and `::`-CLSIDs are Windows; a path that exists is returned as it is.
pub fn resolve_shell_path(s: &str) -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(s.trim());
    p.exists().then_some(p)
}

/// Opening a terminal as administrator: `sudo` lives in the terminal on macOS.
pub fn open_terminal_admin(dir: &Path) -> Result<()> {
    open_terminal(dir)
}

/// Opens Terminal.app in `dir`.
pub fn open_terminal(dir: &Path) -> Result<()> {
    let script = format!("tell application \"Terminal\" to do script \"cd {}\"", dir.display().to_string().replace('"', "\\\""));
    let status = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).status().context("Terminal açılamadı")?;
    anyhow::ensure!(status.success(), "Terminal açılamadı");
    Ok(())
}

//! Opening files and folders with the system's default application (or a named program), the
//! macOS half of `rf-win::shell`.

use std::path::Path;

use anyhow::{Context, Result};

/// Opens `path` with its default application (or in the Finder for folders).
pub fn open(path: &Path) -> Result<()> {
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    use objc2_foundation::NSError;
    let url = open_url(path)?;
    let config = open_configuration();
    let done = block2::RcBlock::new(|_: *mut NSRunningApplication, error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } { log::warn!("open: {error}"); }
    });
    // NSWorkspace can present macOS' missing-application/error UI; the `open` CLI cannot.
    NSWorkspace::sharedWorkspace().openURL_configuration_completionHandler(&url, &config, Some(&done));
    Ok(())
}

fn open_configuration() -> objc2::rc::Retained<objc2_app_kit::NSWorkspaceOpenConfiguration> {
    let config = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
    config.setPromptsUserIfNeeded(true);
    config
}

fn open_url(path: &Path) -> Result<objc2::rc::Retained<objc2_foundation::NSURL>> {
    use objc2_foundation::{NSString, NSURL};
    let text = path.to_string_lossy();
    let scheme = text.split_once(':').map(|(s, _)| s).filter(|s| {
        s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
    });
    if !path.is_absolute() && scheme.is_some() && !path.exists() {
        NSURL::URLWithString(&NSString::from_str(&text)).context("Geçersiz URL")
    } else {
        Ok(NSURL::fileURLWithPath(&NSString::from_str(&text)))
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[test]
    fn system_open_enables_native_prompts_and_preserves_files_and_url_schemes() {
        assert!(open_configuration().promptsUserIfNeeded());
        for path in ["/tmp/test.nsp", "/tmp/a file #1.txt", "relative.nsp"] {
            let url = open_url(Path::new(path)).unwrap();
            assert!(url.isFileURL());
            assert!(url.path().unwrap().to_string().ends_with(path));
        }
        for link in ["https://ko-fi.com/J3J3WWZKO", "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"] {
            let url = open_url(Path::new(link)).unwrap();
            assert!(!url.isFileURL());
            assert_eq!(url.absoluteString().unwrap().to_string(), link);
        }
    }
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
///
/// Through LaunchServices, not AppleScript: `open -a Terminal <folder>` asks Terminal itself
/// to open a window at that folder and activates the app, so the window comes to the front of
/// Brysk. `tell application "Terminal" to do script` made a window but often left Terminal
/// behind the active app.
pub fn open_terminal(dir: &Path) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg("Terminal")
        .arg(dir)
        .status()
        .context("Terminal açılamadı")?;
    anyhow::ensure!(status.success(), "Terminal açılamadı");
    Ok(())
}

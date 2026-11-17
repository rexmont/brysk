//! File and folder pickers, the macOS half of `rf-win::dialogs`.
//!
//! The panels are AppKit's (`NSOpenPanel` / `NSSavePanel`), which must run on the main thread;
//! the caller already runs them on its shell thread (`sta::run`). "Start with the system" is a
//! Windows login-item concept; macOS uses a Launch Agent, which ships with the packaging work.

use std::path::{Path, PathBuf};

use crate::HWND;

pub enum Pick<'a> {
    /// Open an existing file; filters are (label, pattern) pairs like ("Ayar dosyası", "*.toml").
    File { filters: &'a [(&'a str, &'a str)] },
    /// Choose a folder.
    Folder,
    /// Save as `default_name`.
    Save { filters: &'a [(&'a str, &'a str)], default_name: &'a str },
}

/// Shows the panel; `None` when cancelled. The AppKit panel is application-modal, so the owner
/// handle is not needed.
pub fn pick(owner: HWND, title: &str, kind: Pick<'_>) -> Option<PathBuf> {
    let _ = (owner, title, kind);
    // The native panels are part of the next milestone (they need the AppKit main thread);
    // the app's own address bar and folder lists are fully usable without them.
    None
}

/// Whether the app starts with the system (a Launch Agent on macOS).
pub fn starts_with_windows() -> bool {
    false
}

pub fn starts_with_windows_as(_name: &str) -> bool {
    false
}

pub fn set_start_with_windows(_exe: &Path, _on: bool) -> anyhow::Result<()> {
    Ok(())
}

pub fn set_start_with_windows_as(_name: &str, _exe: &Path, _args: &[&str], _on: bool) -> anyhow::Result<()> {
    Ok(())
}
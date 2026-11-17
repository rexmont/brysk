//! File and folder pickers, the macOS half of `rf-win::dialogs`.
//!
//! AppKit's `NSOpenPanel`/`NSSavePanel` must run on the main thread; the callers run their
//! pickers on a worker (the Windows shell thread), so the panel is dispatched to the main
//! queue and the caller waits for its answer.

use std::path::{Path, PathBuf};

use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSString};

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
pub fn pick(_owner: HWND, title: &str, kind: Pick<'_>) -> Option<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
    let title = title.to_string();
    let (save, folder, default_name) = match &kind {
        Pick::File { .. } => (false, false, String::new()),
        Pick::Folder => (false, true, String::new()),
        Pick::Save { default_name, .. } => (true, false, (*default_name).to_string()),
    };
    objc2::rc::autoreleasepool(|_| {
        let job = move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = tx.send(None);
                return;
            };
            let answer = {
                let title = NSString::from_str(&title);
                if save {
                    let panel = NSSavePanel::new(mtm);
                    panel.setTitle(Some(&title));
                    if !default_name.is_empty() {
                        panel.setNameFieldStringValue(&NSString::from_str(&default_name));
                    }
                    panel.setCanCreateDirectories(true);
                    (panel.runModal() == NSModalResponseOK).then(|| panel.URL()).flatten()
                } else {
                    let panel = NSOpenPanel::new(mtm);
                    panel.setTitle(Some(&title));
                    panel.setCanChooseFiles(!folder);
                    panel.setCanChooseDirectories(folder);
                    panel.setAllowsMultipleSelection(false);
                    (panel.runModal() == NSModalResponseOK).then(|| panel.URL()).flatten()
                }
            };
            let path = answer.and_then(|u| u.path().map(|p| PathBuf::from(p.to_string())));
            let _ = tx.send(path);
        };
        if objc2_foundation::MainThreadMarker::new().is_some() {
            job();
        } else {
            dispatch2::DispatchQueue::main().exec_async(job);
        }
    });
    rx.recv_timeout(std::time::Duration::from_secs(600)).ok().flatten()
}

/// "Open With…": an application chooser starting in `/Applications`. Returns the `.app`.
pub fn pick_application(title: &str) -> Option<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
    let title = title.to_string();
    objc2::rc::autoreleasepool(|_| {
        let job = move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = tx.send(None);
                return;
            };
            let panel = NSOpenPanel::new(mtm);
            panel.setTitle(Some(&NSString::from_str(&title)));
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            let apps = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str("/Applications"));
            panel.setDirectoryURL(Some(&apps));
            let app_type = NSString::from_str("app");
            let types = objc2_foundation::NSArray::from_slice(&[&*app_type]);
            panel.setAllowedFileTypes(Some(&types));
            let answer = (panel.runModal() == NSModalResponseOK).then(|| panel.URL()).flatten();
            let path = answer.and_then(|u| u.path().map(|p| PathBuf::from(p.to_string())));
            let _ = tx.send(path);
        };
        if MainThreadMarker::new().is_some() {
            job();
        } else {
            dispatch2::DispatchQueue::main().exec_async(job);
        }
    });
    rx.recv_timeout(std::time::Duration::from_secs(600)).ok().flatten()
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

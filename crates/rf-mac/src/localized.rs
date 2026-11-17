//! Finder-localized folder names, the macOS half of `rf-win::localized`.
//!
//! macOS localizes the standard folders (Desktop, Downloads…) through the system, not through
//! a `desktop.ini` next to the folder: there is nothing extra to read for them, and folders
//! can carry a `.localized` directory with `*.lproj` name files. That file is read when present.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::known::KnownFolder;

fn localized_name_file(dir: &Path) -> Option<PathBuf> {
    // A folder with `.localized` and `en.lproj/.localizedName` (or any lproj) shows that name.
    let loc = dir.join(".localized");
    if !loc.is_dir() {
        return None;
    }
    let mut best: Option<(u8, PathBuf)> = None;
    for e in std::fs::read_dir(&loc).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".lproj") {
            continue;
        }
        let pref = if name.starts_with("en") { 2 } else { 1 };
        let f = e.path().join(".localizedName");
        if f.is_file() && best.as_ref().is_none_or(|(p, _)| *p < pref) {
            best = Some((pref, f));
        }
    }
    best.map(|(_, f)| f)
}

/// The name Finder shows for `dir`, when it differs from the name on disk.
pub fn folder_label(dir: &Path, _attrs: Option<u32>) -> Option<Arc<str>> {
    if let Some(f) = localized_name_file(dir) {
        let s = std::fs::read_to_string(f).ok()?;
        let s = s.trim();
        if !s.is_empty() {
            return Some(Arc::from(s));
        }
    }
    // The standard folders are localized by the system; the list already carries their names.
    let _ = dir;
    None
}

pub fn label_if_known(dir: &Path) -> Option<Arc<str>> {
    dir.file_name().and_then(|n| {
        KnownFolder::PINNED_DEFAULTS.iter().find(|k| k.path().as_deref().and_then(|p| p.file_name()) == Some(n)).map(|k| Arc::from(k.title()))
    })
}

pub fn is_known(dir: &Path) -> bool {
    KnownFolder::PINNED_DEFAULTS.iter().any(|k| k.path().as_deref() == Some(dir))
}

/// Nothing to warm: the labels are read from the folder itself.
pub fn warm<I: IntoIterator<Item = PathBuf>>(_dirs: I) -> usize {
    0
}

pub fn is_network(p: &Path) -> bool {
    crate::drives::slow_to_read(p)
}

/// `~`, `$HOME` and `%VAR%` in an address the user typed.
pub fn expand_env(s: &str) -> String {
    let mut out = s.to_string();
    if out == "~" || out.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            out = format!("{}{}", home.display(), &out[1..]);
        }
    }
    let mut from = 0;
    while let Some(i) = out[from..].find('%').map(|i| i + from) {
        let Some(j) = out[i + 1..].find('%').map(|j| j + i + 1) else { break };
        let name = &out[i + 1..j];
        let val = std::env::var(name).unwrap_or_default();
        out.replace_range(i..=j, &val);
        from = i + val.len();
    }
    if let Some(home) = dirs::home_dir() {
        out = out.replace("$HOME", &home.display().to_string());
    }
    out
}
//! Finder-localized folder names, the macOS half of `rf-win::localized`.
//!
//! macOS localizes the standard folders (Desktop, Downloads…) through the system, not through
//! a `desktop.ini` next to the folder: there is nothing extra to read for them, and folders
//! can carry a `.localized` directory with `*.lproj` name files. That file is read when present.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::known::KnownFolder;

/// Translates a Turkish source key into the interface language. `rf-app::i18n::set` installs it
/// once at startup; without it (this crate's own tests) the key itself is returned.
static TRANSLATOR: OnceLock<fn(&str) -> String> = OnceLock::new();

/// Installs the UI-language translator for the known folders' titles. Called by
/// `rf-app::i18n::set`, after the language has been resolved.
pub fn set_label_translator(f: fn(&str) -> String) {
    let _ = TRANSLATOR.set(f);
}

fn label(key: &'static str) -> Arc<str> {
    match TRANSLATOR.get() {
        Some(t) => Arc::from(t(key)),
        None => Arc::from(key),
    }
}

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

/// The name the interface shows for a standard folder. The folder itself is known by its path,
/// so this works for a pin, a tab title, a breadcrumb and the Home page alike; the title is
/// translated through the interface's hook (see [`set_label_translator`]).
pub fn label_if_known(dir: &Path) -> Option<Arc<str>> {
    // Path comparison only, no `is_dir` per known folder: this runs for every breadcrumb frame.
    KnownFolder::PINNED_DEFAULTS
        .iter()
        .find(|k| k.path_unchecked().as_deref() == Some(dir))
        .map(|k| label(k.title()))
}

pub fn is_known(dir: &Path) -> bool {
    KnownFolder::PINNED_DEFAULTS.iter().any(|k| k.path_unchecked().as_deref() == Some(dir))
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
//! Folder rules (the organizer) are part of the Windows build; on macOS the menu entry and page
//! are not shown. These keep the calls from the window code compiling.

use std::path::PathBuf;

use gpui_kit::*;

pub fn overlay(_window: &Window, _cx: &App) -> Option<AnyElement> {
    None
}

pub fn is_open(_window: &Window, _cx: &App) -> bool {
    false
}

pub fn organize_folder(_dir: PathBuf, _window: &mut Window, _cx: &mut App) {}

pub fn not_a_folder(_window: &mut Window, _cx: &mut App) {}

pub fn has_rules_for(_dir: &std::path::Path, _cx: &App) -> bool {
    false
}

pub fn init(_cx: &mut App) {}

//! The Windows property-column chooser; macOS uses its own columns, so the overlay never shows.

use gpui_kit::*;

pub fn open(_window: &mut Window, _cx: &mut App) {}

pub fn overlay(_window: &Window, _cx: &App) -> Option<AnyElement> {
    None
}

pub fn is_open(_window: &Window, _cx: &App) -> bool {
    false
}
